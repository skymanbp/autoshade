// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the
// reverse-fit's stepped strength in the prefs file — the step a file written
// before the stepped control is read as, and the round trip that keeps
// writing the old switches for an older build (2026-11-10).

    /// A prefs file in eframe's own text form, decoded the way the app
    /// decodes it at startup.
    fn fit_level_prefs_from_text(text: &str) -> Prefs {
        let mut storage = PrefsMemory::default();
        eframe::Storage::set_string(&mut storage, eframe::APP_KEY, text.into());
        eframe::get_value(&storage, eframe::APP_KEY).expect("the prefs text decodes")
    }

    /// A file written before the stepped control has no `fit_level`: the
    /// step is derived ONCE from its separate switches — review on → 5; else
    /// four regions → 4; else not zoned → 1; else strength ≥ 0.925 → 4,
    /// ≥ 0.75 → 3, otherwise 2 — and the app restores that step.
    ///
    /// MUTATION THIS CATCHES: `Prefs::fit_level` ignoring the old switches
    /// (every row reads 2), `restore_prefs` not taking the derived step, a
    /// missing key decoding to anything but "derive" (`Prefs::default`'s
    /// `fit_level` set to a step), or a rule of `FitLevel::from_switches`
    /// reordered.
    #[test]
    fn fit_level_prefs_from_an_older_file_derive_the_step_from_its_switches() {
        use autoshade::fit::FitLevel;
        for (case, text, want) in [
            ("no reverse-fit keys at all", "(prefs_era:2)", FitLevel::Standard),
            ("review on", "(prefs_era:2,fit_ai_judge:true,zoned_fit:false,fit_strength:0.3)", FitLevel::Reviewed),
            ("review and retry on", "(prefs_era:2,fit_ai_judge:true,fit_deep:true)", FitLevel::Reviewed),
            ("four regions", "(prefs_era:2,zoned_four_regions:true,zoned_fit:false)", FitLevel::Full),
            ("not zoned", "(prefs_era:2,zoned_fit:false,fit_strength:1.0)", FitLevel::Light),
            ("zoned, strength 1.0", "(prefs_era:2,zoned_fit:true,fit_strength:1.0)", FitLevel::Full),
            ("zoned, strength 0.925", "(prefs_era:2,fit_strength:0.925)", FitLevel::Full),
            ("zoned, strength 0.9", "(prefs_era:2,fit_strength:0.9)", FitLevel::Strong),
            ("zoned, strength 0.75", "(prefs_era:2,fit_strength:0.75)", FitLevel::Strong),
            ("zoned, strength 0.7", "(prefs_era:2,fit_strength:0.7)", FitLevel::Standard),
            ("zoned, strength 0.2", "(prefs_era:2,fit_strength:0.2)", FitLevel::Standard),
        ] {
            let prefs = fit_level_prefs_from_text(text);
            assert_eq!(prefs.fit_level, 0, "{case}: a missing key means derive");
            assert_eq!(prefs.fit_level(), want, "{case}");
            let mut app = AutoShadeApp::default();
            app.restore_prefs(prefs);
            assert_eq!(app.fit_level, want, "{case}: the app restores the derived step");
        }
        // A step the file names wins over its switches, and a code no build
        // knows (a newer one's) falls back to them.
        let named = fit_level_prefs_from_text("(prefs_era:2,fit_level:1,fit_ai_judge:true)");
        assert_eq!(named.fit_level(), FitLevel::Light);
        let unknown = fit_level_prefs_from_text("(prefs_era:2,fit_level:9,fit_strength:0.85)");
        assert_eq!(unknown.fit_level(), FitLevel::Strong);
    }

    /// Every step round-trips through the prefs file, and the file keeps
    /// carrying the step's row in the old switches, so a build from before
    /// the stepped control that reads it fits the same way — and this build
    /// reading those switches alone lands on the same step again.
    ///
    /// MUTATION THIS CATCHES: `save` writing a constant or the old field
    /// values instead of the level's row, dropping `fit_level`, or a step
    /// whose row does not derive back to itself.
    #[test]
    fn fit_level_prefs_round_trip_and_keep_writing_the_old_switches() {
        use autoshade::fit::FitLevel;
        for level in FitLevel::ALL {
            let mut storage = PrefsMemory::default();
            let mut app = AutoShadeApp { fit_level: level, ..Default::default() };
            eframe::App::save(&mut app, &mut storage);
            let prefs: Prefs = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
            let s = level.settings();
            assert_eq!(prefs.fit_level, level.step(), "{level:?}");
            assert_eq!(
                (prefs.fit_strength, prefs.zoned_fit, prefs.zoned_four_regions, prefs.fit_ai_judge, prefs.fit_deep),
                (s.strength, s.zoned, s.regions > autoshade::fit_zoned::semantic::DEFAULT_SEMANTIC_REGIONS, s.ai_judge, s.deep),
                "{level:?}: the old switches carry the step's row"
            );
            let mut back = AutoShadeApp::default();
            back.restore_prefs(prefs);
            assert_eq!(back.fit_level, level, "{level:?}: round trip");
            // What a build without `fit_level` would hand the next build.
            let older = Prefs { fit_level: 0, ..eframe::get_value(&storage, eframe::APP_KEY).unwrap() };
            assert_eq!(older.fit_level(), level, "{level:?}: the old switches alone derive the same step");
        }
    }
