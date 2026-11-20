// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the update
// check (update_check.rs, panels/settings/pages/update_row.rs), v1.18.5. No
// window and no network: the fetch is a closure, the clock an argument.

    /// A parsed version, for brevity.
    fn update_v(s: &str) -> autoshade::update::Version {
        autoshade::update::Version::parse(s).unwrap_or_else(|| panic!("{s:?} parses"))
    }

    /// A release reply naming `v`.
    fn update_rel(v: &str) -> autoshade::update::Release {
        autoshade::update::Release { version: update_v(v), page: autoshade::update::RELEASES_PAGE.into() }
    }

    /// An app on build 1.18.5 whose fetch answers `answer` and counts its calls.
    fn update_app(
        answer: Result<autoshade::update::Release, autoshade::update::CheckError>,
    ) -> (AutoShadeApp, Arc<std::sync::atomic::AtomicUsize>) {
        // The Settings window open, for the tests that draw the About page.
        let mut app = AutoShadeApp { show_settings: true, ..Default::default() };
        app.update.current = update_v("1.18.5");
        app.update.env_off = false; // whatever this shell's environment says
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        app.update.fetch = Arc::new(move || {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            answer.clone()
        });
        (app, calls)
    }

    /// Land the check in flight (bounded: a lost answer fails, never hangs).
    fn update_settle(app: &mut AutoShadeApp) {
        let t0 = Instant::now();
        while app.update.checking() {
            assert!(t0.elapsed() < Duration::from_secs(5), "the check never landed");
            app.poll_update_check();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn update_calls(c: &std::sync::atomic::AtomicUsize) -> usize {
        c.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The notices naming `version`.
    fn update_notices(app: &AutoShadeApp, version: &str) -> usize {
        app.toasts.iter().filter(|t| t.text.contains(version)).count()
    }

    const UPDATE_T0: u64 = 1_800_000_000;

    /// The checkbox and the environment switch each turn the automatic check
    /// off; with both on it runs. The variable counts only when non-empty.
    /// And a frame under `cfg(test)` never starts it on its own.
    ///
    /// MUTATION THIS CATCHES: `maybe_start_auto_check` ignoring `prefs.auto`
    /// or `env_off`; `switched_off` treating an empty value as set.
    #[test]
    fn update_auto_check_respects_the_checkbox_and_the_environment_switch() {
        let (mut app, calls) = update_app(Ok(update_rel("1.18.5")));
        app.update.prefs.auto = false;
        assert!(!app.maybe_start_auto_check(UPDATE_T0, false), "checkbox off");
        assert!(!app.maybe_start_auto_check(UPDATE_T0, true) && !app.update.checking(), "env off");
        app.update.prefs.auto = true;
        assert!(!app.maybe_start_auto_check(UPDATE_T0, true), "checkbox on, env off");
        assert_eq!(update_calls(&calls), 0);
        assert_eq!(app.update.prefs.last_auto_check, 0, "a check that did not run stamps nothing");
        assert!(app.maybe_start_auto_check(UPDATE_T0, false), "both on");
        update_settle(&mut app);
        assert_eq!(update_calls(&calls), 1);

        use crate::update_check::switched_off;
        assert!(!switched_off(None));
        assert!(!switched_off(Some(std::ffi::OsStr::new(""))));
        assert!(switched_off(Some(std::ffi::OsStr::new("1"))));
        assert!(switched_off(Some(std::ffi::OsStr::new("no"))), "anything non-empty");

        let (mut app, calls) = update_app(Ok(update_rel("1.18.9")));
        for _ in 0..5 {
            app.update_check_frame();
        }
        assert_eq!(update_calls(&calls), 0, "no automatic check under cfg(test)");
        assert!(!app.update.checking());
        // And an app nobody injected a fetch into never reaches the network.
        let plain = AutoShadeApp::default();
        assert_eq!((plain.update.fetch)(), Err(autoshade::update::CheckError::Offline));
    }

    /// At most once every 24 hours, stamped when it starts; a clock set back
    /// before the stamp counts as due; never two at once.
    ///
    /// MUTATION THIS CATCHES: `auto_due` with a shorter spacing, `>` for
    /// `>=`, or no spacing at all; the stamp not written.
    #[test]
    fn update_auto_check_runs_at_most_once_a_day() {
        use crate::update_check::AUTO_SPACING_SECS as DAY;
        assert_eq!(DAY, 86_400);
        let (mut app, calls) = update_app(Err(autoshade::update::CheckError::Offline));
        assert!(app.maybe_start_auto_check(UPDATE_T0, false), "never checked: due");
        assert!(!app.maybe_start_auto_check(UPDATE_T0 + DAY, false), "one in flight: not a second");
        update_settle(&mut app);
        assert_eq!(app.update.prefs.last_auto_check, UPDATE_T0);
        assert!(app.prefs_save_due, "the stamp is written out");
        for later in [1, 60, 3_600, DAY - 1] {
            assert!(!app.maybe_start_auto_check(UPDATE_T0 + later, false), "{later} s later: not due");
        }
        assert_eq!(update_calls(&calls), 1, "a failure does not make it ask again sooner");
        assert!(app.maybe_start_auto_check(UPDATE_T0 + DAY, false), "24 h later: due");
        update_settle(&mut app);
        assert_eq!(app.update.prefs.last_auto_check, UPDATE_T0 + DAY);
        assert!(app.maybe_start_auto_check(UPDATE_T0, false), "clock set back: due once");
        update_settle(&mut app);
        assert!(!app.maybe_start_auto_check(UPDATE_T0 + 10, false), "…and spaced again from there");
        assert_eq!(update_calls(&calls), 3);
    }

    /// A newer version found by the automatic check is announced once —
    /// after the mode page, which draws no toasts — and the row shows it;
    /// a failure is silent and leaves the row alone.
    ///
    /// MUTATION THIS CATCHES: the notice raised on every frame, or not at
    /// all; raised while the mode page is up (it would fade unseen); a
    /// failed automatic check writing the row or toasting.
    #[test]
    fn update_a_newer_version_raises_the_notice_once() {
        let (mut app, _) = update_app(Ok(update_rel("1.18.99")));
        app.mode_chosen_this_launch = false;
        assert!(app.maybe_start_auto_check(UPDATE_T0, false));
        update_settle(&mut app);
        for _ in 0..3 {
            app.update_check_frame();
        }
        assert_eq!(update_notices(&app, "1.18.99"), 0, "not over the mode page");
        app.mode_chosen_this_launch = true;
        for _ in 0..5 {
            app.update_check_frame();
        }
        assert_eq!(update_notices(&app, "1.18.99"), 1, "{:?}", app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
        let t = app.toasts.iter().find(|t| t.text.contains("1.18.99")).unwrap();
        assert!(t.kind == ToastKind::Success);
        assert_eq!(t.text, trf(Lang::En, "AutoShade {version} is available — see Settings → About.", &[("version", "1.18.99")]));
        assert_eq!(app.update.row, crate::update_check::UpdateRow::Newer(update_rel("1.18.99")));
        assert_eq!(app.update.prefs.known_newer.as_deref(), Some("1.18.99"));

        // A failing automatic check: no toast, row untouched.
        let (mut app, _) = update_app(Err(autoshade::update::CheckError::Refused));
        app.mode_chosen_this_launch = true;
        app.maybe_start_auto_check(UPDATE_T0, false);
        update_settle(&mut app);
        app.update_check_frame();
        assert!(app.toasts.is_empty());
        assert_eq!(app.update.row, crate::update_check::UpdateRow::NotChecked);
        // Same or older: no toast either.
        for v in ["1.18.5", "1.18.4"] {
            let (mut app, _) = update_app(Ok(update_rel(v)));
            app.mode_chosen_this_launch = true;
            app.maybe_start_auto_check(UPDATE_T0, false);
            update_settle(&mut app);
            app.update_check_frame();
            assert!(app.toasts.is_empty(), "{v}");
        }
    }

    /// A skipped version is not announced by the automatic check, and About
    /// still shows it (and says it was skipped); a version newer than the
    /// skipped one is announced.
    ///
    /// MUTATION THIS CATCHES: the skip ignored by the automatic check; the
    /// skip hiding the version from About too.
    #[test]
    fn update_a_skipped_version_stays_quiet_and_about_still_shows_it() {
        use crate::panels::settings::SettingsTab;
        let (mut app, _) = update_app(Ok(update_rel("1.18.99")));
        app.mode_chosen_this_launch = true;
        app.skip_update_version(update_v("1.18.99"));
        assert_eq!(app.update.prefs.skipped.as_deref(), Some("1.18.99"));
        assert!(app.maybe_start_auto_check(UPDATE_T0, false));
        update_settle(&mut app);
        app.update_check_frame();
        assert_eq!(update_notices(&app, "1.18.99"), 0, "skipped: quiet");
        for lang in [Lang::En, Lang::Zh] {
            app.lang = lang;
            let (texts, past) = whole_page(&mut app, SettingsTab::About);
            assert!(past.is_empty(), "{lang:?}: {past:#?}");
            for want in [
                trf(lang, "Version {version} is available.", &[("version", "1.18.99")]),
                tr(lang, "Open the download page").to_owned(),
                tr(lang, "You chose to skip this version: the automatic check will not mention it again.").to_owned(),
            ] {
                assert!(texts.contains(&want), "{lang:?}: {want:?} not drawn: {texts:?}");
            }
            assert!(!texts.iter().any(|t| t == tr(lang, "Skip this version")), "{lang:?}: already skipped");
        }
        // A newer release than the skipped one is news again.
        app.update.fetch = Arc::new(|| Ok(update_rel("1.18.100")));
        app.update.prefs.last_auto_check = 0;
        assert!(app.maybe_start_auto_check(UPDATE_T0, false));
        update_settle(&mut app);
        app.update_check_frame();
        assert_eq!(update_notices(&app, "1.18.100"), 1);
    }

    /// 「Check now」 always asks, says 「Checking…」 meanwhile, and each
    /// outcome lands in the row and on the About page, in both languages —
    /// never as a toast.
    ///
    /// MUTATION THIS CATCHES: a manual failure dropped like an automatic
    /// one; an outcome mapped to the wrong line; the open button shown
    /// without a newer version.
    #[test]
    fn update_each_manual_outcome_sets_the_row() {
        use crate::panels::settings::SettingsTab;
        use crate::update_check::UpdateRow;
        use autoshade::update::CheckError;
        for lang in [Lang::En, Lang::Zh] {
            let cases: [(Result<autoshade::update::Release, CheckError>, UpdateRow, String); 6] = [
                (
                    Ok(update_rel("1.18.10")),
                    UpdateRow::Newer(update_rel("1.18.10")),
                    trf(lang, "Version {version} is available.", &[("version", "1.18.10")]),
                ),
                (
                    Ok(update_rel("1.18.5")),
                    UpdateRow::UpToDate(update_v("1.18.5")),
                    trf(lang, "You have the latest version ({version}).", &[("version", "1.18.5")]),
                ),
                (
                    Ok(update_rel("1.17.0")),
                    UpdateRow::UpToDate(update_v("1.18.5")),
                    trf(lang, "You have the latest version ({version}).", &[("version", "1.18.5")]),
                ),
                (Err(CheckError::Offline), UpdateRow::Failed(CheckError::Offline), tr(lang, "Could not check: no connection.").into()),
                (
                    Err(CheckError::Refused),
                    UpdateRow::Failed(CheckError::Refused),
                    tr(lang, "Could not check: GitHub refused the request.").into(),
                ),
                (
                    Err(CheckError::Unreadable),
                    UpdateRow::Failed(CheckError::Unreadable),
                    tr(lang, "Could not check: GitHub's answer could not be read.").into(),
                ),
            ];
            for (answer, row, line) in cases {
                let (mut app, calls) = update_app(answer);
                app.lang = lang;
                app.mode_chosen_this_launch = true;
                let (texts, _) = whole_page(&mut app, SettingsTab::About);
                assert!(texts.iter().any(|t| t == tr(lang, "Not checked yet.")), "{lang:?}: {texts:?}");
                app.update.prefs.last_auto_check = UPDATE_T0; // a manual check ignores the spacing
                app.start_manual_update_check();
                assert_eq!(app.update.row, UpdateRow::Checking);
                let (texts, _) = whole_page(&mut app, SettingsTab::About);
                assert!(texts.iter().any(|t| t == tr(lang, "Checking…")), "{lang:?}: {texts:?}");
                update_settle(&mut app);
                app.update_check_frame();
                assert_eq!(update_calls(&calls), 1);
                assert_eq!(app.update.row, row, "{lang:?} {line}");
                assert!(app.toasts.is_empty(), "{lang:?} {line}: a manual check never toasts");
                let (texts, past) = whole_page(&mut app, SettingsTab::About);
                assert!(past.is_empty(), "{lang:?} {line}: {past:#?}");
                assert!(texts.contains(&line), "{lang:?}: {line:?} not drawn: {texts:?}");
                for l in [tr(lang, "Updates"), tr(lang, "Check now"), tr(lang, "Check for updates automatically")] {
                    assert!(texts.iter().any(|t| t == l), "{lang:?}: {l:?} not drawn");
                }
                let newer = matches!(row, UpdateRow::Newer(_));
                for l in [tr(lang, "Open the download page"), tr(lang, "Skip this version")] {
                    assert_eq!(texts.iter().any(|t| t == l), newer, "{lang:?} {line}: {l:?}");
                }
            }
        }
    }

    /// With `AUTOSHADE_NO_UPDATE_CHECK` set, the row shows the automatic
    /// check as off — the switch drawn disabled with the reason under it —
    /// while the stored choice stays as it was and 「Check now」 still works;
    /// the first frame does not start a check either.
    ///
    /// MUTATION THIS CATCHES: the env line not drawn, the stored switch
    /// overwritten, the frame hook ignoring `env_off`.
    #[test]
    fn update_the_environment_switch_shows_the_automatic_check_as_off() {
        use crate::panels::settings::SettingsTab;
        for lang in [Lang::En, Lang::Zh] {
            let (mut app, calls) = update_app(Ok(update_rel("1.18.5")));
            app.lang = lang;
            app.update.env_off = true;
            assert!(app.update.prefs.auto, "premise: the stored switch is on");
            let (texts, past) = whole_page(&mut app, SettingsTab::About);
            assert!(past.is_empty(), "{lang:?}: {past:#?}");
            let reason = tr(lang, "Turned off by the AUTOSHADE_NO_UPDATE_CHECK environment variable.");
            assert!(texts.iter().any(|t| t == reason), "{lang:?}: {texts:?}");
            assert!(texts.iter().any(|t| t == tr(lang, "Check for updates automatically")), "{lang:?}");
            assert!(app.update.prefs.auto, "{lang:?}: the stored choice is untouched");
            assert!(!app.prefs_save_due, "{lang:?}: nothing to save");
            assert!(!app.maybe_start_auto_check(UPDATE_T0, app.update.env_off));
            assert_eq!(update_calls(&calls), 0);
            app.start_manual_update_check();
            update_settle(&mut app);
            assert_eq!(update_calls(&calls), 1, "Check now still asks");
            // Without the variable the reason is gone.
            app.update.env_off = false;
            let (texts, _) = whole_page(&mut app, SettingsTab::About);
            assert!(!texts.iter().any(|t| t == reason), "{lang:?}: {texts:?}");
        }
    }

    /// A prefs file from before v1.18.5 loads with the defaults (automatic
    /// on, never checked, nothing skipped or known); a partial section keeps
    /// what it has; the section round-trips; a remembered version shows in
    /// About only while it is newer than this build.
    ///
    /// MUTATION THIS CATCHES: `UpdatePrefs::default` with `auto: false`;
    /// `save` not writing the section; `restore` keeping a stale version.
    #[test]
    fn update_old_prefs_without_the_new_fields_load_with_the_defaults() {
        use crate::update_check::{UpdatePrefs, UpdateRow};
        let defaults = UpdatePrefs { auto: true, last_auto_check: 0, skipped: None, known_newer: None };
        assert_eq!(fit_level_prefs_from_text("(prefs_era:2)").update_check, defaults);
        assert_eq!(fit_level_prefs_from_text("(prefs_era:2,lang:Zh,fit_level:3)").update_check, defaults);
        let partial = fit_level_prefs_from_text("(prefs_era:2,update_check:(skipped:Some(\"1.18.7\")))");
        assert_eq!(partial.update_check, UpdatePrefs { skipped: Some("1.18.7".into()), ..defaults.clone() });

        let saved = UpdatePrefs { auto: false, last_auto_check: UPDATE_T0, skipped: Some("1.18.7".into()), known_newer: Some("1.18.8".into()) };
        let mut app = AutoShadeApp::default();
        app.update.prefs = saved.clone();
        let mut storage = PrefsMemory::default();
        eframe::App::save(&mut app, &mut storage);
        let prefs: Prefs = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
        assert_eq!(prefs.update_check, saved, "round trip");

        let mut back = AutoShadeApp::default();
        back.update.current = update_v("1.18.5");
        back.restore_prefs(prefs);
        assert_eq!(back.update.prefs, saved);
        assert_eq!(back.update.row, UpdateRow::Newer(update_rel("1.18.8")), "still newer: About names it");
        let mut upgraded = AutoShadeApp::default();
        upgraded.update.current = update_v("1.18.8");
        upgraded.restore_prefs(Prefs { update_check: saved.clone(), ..Prefs::default() });
        assert_eq!(upgraded.update.row, UpdateRow::NotChecked, "this build is that version: forgotten");
        assert_eq!(upgraded.update.prefs.known_newer, None);
    }

    /// An answer that arrives after the window closed, or after a second
    /// check replaced the first, is dropped without a panic, a toast or a
    /// stale row.
    #[test]
    fn update_a_late_answer_is_dropped_without_panic() {
        use crate::update_check::UpdateRow;
        // A fetch held at a gate until the test releases it.
        fn gated(v: &'static str) -> (crate::update_check::Fetch, std::sync::mpsc::Sender<()>) {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let rx = std::sync::Mutex::new(rx);
            let f: crate::update_check::Fetch = Arc::new(move || {
                let _ = rx.lock().unwrap().recv();
                Ok(update_rel(v))
            });
            (f, tx)
        }
        let wait_alone = |f: &crate::update_check::Fetch| {
            let t0 = Instant::now();
            while Arc::strong_count(f) > 1 {
                assert!(t0.elapsed() < Duration::from_secs(5), "the check thread never finished");
                std::thread::sleep(Duration::from_millis(2));
            }
        };

        // The window closes mid-check.
        let (f, gate) = gated("1.18.99");
        let (mut app, _) = update_app(Ok(update_rel("1.18.5")));
        app.update.fetch = Arc::clone(&f);
        app.start_manual_update_check();
        drop(app);
        gate.send(()).unwrap();
        wait_alone(&f);

        // A second check replaces the first; the first answers last.
        let (slow, gate) = gated("1.18.99");
        let (mut app, _) = update_app(Ok(update_rel("1.18.5")));
        app.mode_chosen_this_launch = true;
        app.update.fetch = Arc::clone(&slow);
        assert!(app.maybe_start_auto_check(UPDATE_T0, false));
        app.update.fetch = Arc::new(|| Ok(update_rel("1.18.5")));
        app.start_manual_update_check();
        update_settle(&mut app);
        assert_eq!(app.update.row, UpdateRow::UpToDate(update_v("1.18.5")));
        gate.send(()).unwrap();
        wait_alone(&slow);
        for _ in 0..3 {
            app.update_check_frame();
        }
        assert_eq!(app.update.row, UpdateRow::UpToDate(update_v("1.18.5")), "the replaced answer is never read");
        assert!(app.toasts.is_empty(), "{:?}", app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
    }

    /// The Settings window at `screen` points and 2×, over the canvas bed.
    fn update_settings_shot(app: &mut AutoShadeApp, screen: egui::Vec2) -> image::RgbaImage {
        let theme = app.theme;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, theme);
        let mut tex = ShotTextures::default();
        let mut last = None;
        for i in 0..6 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
                time: Some(i as f64 / 2.0),
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(2.0);
            let out = ctx.run(input, |ctx| app.settings_window(ctx));
            tex.apply(&out.textures_delta);
            last = Some(out);
        }
        let out = last.expect("frames ran");
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        rasterise(&prims, &tex, screen, out.pixels_per_point, theme.colors().canvas_bed)
    }

    /// Not a check — the camera for Settings → About's update row: up to
    /// date, a newer version, could not check, and the automatic check
    /// turned off by the environment variable; English and Chinese; the
    /// normal screen and the narrowest window the app allows (980 × 620):
    /// `AUTOSHADE_SHOT_DIR=<dir> cargo test … update_screenshots -- --ignored`.
    #[test]
    #[ignore = "writes PNGs for a person to look at"]
    fn update_screenshots() {
        use crate::update_check::UpdateRow;
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        let states = [
            ("uptodate", UpdateRow::UpToDate(update_v("1.18.5"))),
            ("newer", UpdateRow::Newer(update_rel("1.18.6"))),
            ("failed", UpdateRow::Failed(autoshade::update::CheckError::Offline)),
            ("envoff", UpdateRow::UpToDate(update_v("1.18.5"))),
        ];
        for lang in [Lang::En, Lang::Zh] {
            for (name, row) in &states {
                for (w, screen) in [("normal", egui::vec2(1100.0, 760.0)), ("narrowest", egui::vec2(980.0, 620.0))] {
                    let mut app = app_with_settings_form(lang);
                    app.settings_tab = crate::panels::settings::SettingsTab::About;
                    app.update.row = row.clone();
                    app.update.env_off = *name == "envoff";
                    update_settings_shot(&mut app, screen).save(dir.join(format!("about-update-{name}-{lang:?}-{w}.png"))).unwrap();
                }
            }
        }
    }
