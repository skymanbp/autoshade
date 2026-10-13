// One part of the GUI's tests (src/bin/gui/tests.rs includes it): turned photos, the two subject backends, preference round trips, the storage key, button rows, window ids, the paint canvas and the side panel cells.

    /// R27 A6 — the toolbar rotate is a RECIPE edit, and the base plates
    /// follow it through `sync_base_turns` rather than through a rotation
    /// stage in the canvas. Everything downstream (`view_norm_to_orig`, the
    /// crop handles, the coverage overlay, the paint canvas) is defined
    /// against the plate, so turning the pixels turns all of them at once.
    ///
    /// Drives the pure pieces with a headless egui context — never
    /// `run_native`.
    ///
    /// MUTATION THIS CATCHES: make `sync_base_turns` turn by
    /// `recipe.quarter_turns` instead of by the delta against `base_turns` and
    /// the SECOND turn below over-rotates (the plate ends 6x8, not 8x6) —
    /// the double-application hazard, on the GUI side of the fence.
    #[test]
    fn the_toolbar_turn_moves_the_recipe_and_the_plate_together() {
        let (mut app, _scrub) = app_with_masked_photo("rotate-plate");
        let ctx = egui::Context::default();
        // A deliberately non-square plate, so a turn is visible in the dims.
        let plate = std::sync::Arc::new(image::DynamicImage::new_rgb8(8, 6));
        app.base_preview = Some(plate.clone());
        app.source_preview = Some(plate);
        app.base_turns = 0;
        let src = std::env::temp_dir().join(format!("autoshade-rotate-plate-{}.arw", std::process::id()));
        std::fs::write(&src, b"raw").unwrap();
        app.src_path = Some(src);

        assert!(app.can_rotate(), "a parametric photo with a plate is rotatable");
        app.rotate_photo(1, &ctx);
        assert_eq!(app.recipe.quarter_turns, 1);
        assert_eq!(app.base_turns, 1);
        assert_eq!(
            app.base_preview.as_ref().unwrap().dimensions(),
            (6, 8),
            "one clockwise quarter turn transposes the plate"
        );
        assert_eq!(app.source_preview.as_ref().unwrap().dimensions(), (6, 8));

        // A second turn moves by the DELTA, not by the running total.
        app.rotate_photo(1, &ctx);
        assert_eq!(app.recipe.quarter_turns, 2);
        assert_eq!(app.base_preview.as_ref().unwrap().dimensions(), (8, 6));

        // An UNDO changes only the recipe; the per-frame reconciler is what
        // brings the plate back — one mover for both directions.
        app.undo(&ctx);
        assert_eq!(app.recipe.quarter_turns, 1, "one Ctrl+Z per turn (7.7)");
        app.sync_base_turns(&ctx);
        assert_eq!(app.base_preview.as_ref().unwrap().dimensions(), (6, 8));
        assert_eq!(app.base_turns, 1);

        // Idempotent: the common per-frame call is a u8 compare that moves
        // nothing (a reconciler that turned on every frame would spin the
        // canvas).
        app.sync_base_turns(&ctx);
        assert_eq!(app.base_preview.as_ref().unwrap().dimensions(), (6, 8));

        // BAKED PIXELS close the door, whole strip (a limitation by design):
        // a master raster is a file in the frame it was baked in, and this
        // build cannot record that it predates a turn.
        app.variants[0].origin = Some("master.png".into());
        assert!(!app.can_rotate(), "a baked master must block the turn, not be turned under");
        let before = app.recipe.quarter_turns;
        app.rotate_photo(1, &ctx);
        assert_eq!(app.recipe.quarter_turns, before, "a refused turn changes nothing");
    }

    /// R29 C3/C4 item 1 — the GUI tells the two subject backends apart, and it
    /// does so in TWO SENTENCES, not one.
    ///
    /// R29 B4 pinned BiRefNet and kept U²-Net as a named fallback tier, then
    /// registered exactly this hole: the sidecar announced the degradation on
    /// stderr, which a `windows_subsystem = "windows"` build has no console to
    /// receive, so both tiers produced the same "AI mask added" line. A
    /// photographer whose machine had quietly dropped to the fallback — softer
    /// edges, no strand structure on hair, a subject invented on a landscape
    /// that has none — could not tell from anything on screen.
    ///
    /// MUTATION-LINED: dropping the backend from `Msg::Segmented`, dropping the
    /// status suffix, or raising the same toast (or none) for both tiers fails
    /// one of the four assertions below. The pair is deliberate — a test that
    /// only checked the fallback would pass on a build that warned about
    /// everything.
    #[test]
    fn the_two_subject_backends_land_as_two_different_sentences() {
        use std::path::PathBuf;
        let ctx = egui::Context::default();

        // THE PINNED MODEL: named in the status, and no alarm raised.
        let mut ok = AutoShadeApp { busy: true, ..Default::default() };
        ok.tx
            .send(Msg::Segmented(Ok((
                "Subject".into(),
                PathBuf::from("mask-subject.png"),
                "BiRefNet e2bf8e4460fc".into(),
            ))))
            .unwrap();
        ok.poll_workers(&ctx);
        assert!(
            ok.status.contains("BiRefNet e2bf8e4460fc"),
            "the run must name the model that drew the mask: {}",
            ok.status
        );
        assert!(
            !ok.toasts.iter().any(|t| matches!(t.kind, ToastKind::Error)),
            "the model the user ruled for is not a warning"
        );
        assert_eq!(ok.recipe.masks.len(), 1, "the mask still lands");

        // THE FALLBACK TIER: named in the status AND escalated to an error
        // toast, because a status line nobody is looking at is not a
        // disclosure. The remedy travels with it.
        let mut degraded = AutoShadeApp { busy: true, ..Default::default() };
        degraded
            .tx
            .send(Msg::Segmented(Ok((
                "Subject".into(),
                PathBuf::from("mask-subject.png"),
                "U^2-Net (FALLBACK - BiRefNet did not run)".into(),
            ))))
            .unwrap();
        degraded.poll_workers(&ctx);
        assert!(
            degraded.status.contains("FALLBACK"),
            "the degraded run must say so where the good one said its model: {}",
            degraded.status
        );
        let alarm = degraded
            .toasts
            .iter()
            .find(|t| matches!(t.kind, ToastKind::Error))
            .expect("a silent degradation is the bug this closes");
        assert!(
            alarm.text.contains("torchvision"),
            "the warning must carry the remedy, not just the bad news: {}",
            alarm.text
        );
        assert_ne!(
            ok.status, degraded.status,
            "two backends, two sentences — this is the whole point"
        );

        // A sidecar too old to print the line says nothing, and the landing
        // must not invent a model name for it.
        let mut silent = AutoShadeApp { busy: true, ..Default::default() };
        silent
            .tx
            .send(Msg::Segmented(Ok((
                "Sky".into(),
                PathBuf::from("mask-sky.png"),
                String::new(),
            ))))
            .unwrap();
        silent.poll_workers(&ctx);
        assert!(
            !silent.status.contains("drawn by") && !silent.status.contains("绘制"),
            "an empty label must add no clause at all: {}",
            silent.status
        );
        assert!(
            !silent.toasts.iter().any(|t| matches!(t.kind, ToastKind::Error)),
            "unknown is not a fallback"
        );
    }

    /// The Adherence slider is INERT without a direction, and the tier the dial
    /// picks is the one the develop's request carries.
    ///
    /// The old spelling of this test asserted that a default app has an empty
    /// direction and the shipped adherence default - two facts about
    /// `Default`, both true whether or not the slider is gated at all. It named
    /// the disabling and checked none of it.
    ///
    /// MUTATION: delete the `add_enabled_ui(has_direction, ...)` wrapper in
    /// panels/ai.rs and phase (2) fails.
    #[test]
    fn gui_adherence_slider_is_disabled_without_a_direction() {
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let frame = |app: &mut AutoShadeApp| -> Option<bool> {
            app.adherence_gate_enabled = None; // this frame's evidence only
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
            app.adherence_gate_enabled
        };
        // (1) no direction: the dial exists but cannot be moved.
        let mut app = AutoShadeApp::default();
        assert!(app.guidance.trim().is_empty(), "premise: the default app has no direction");
        assert_eq!(frame(&mut app), Some(false), "a blank direction must leave the slider inert");
        // (2) whitespace is still blank - the gate trims.
        app.guidance = "   \n\t ".into();
        assert_eq!(frame(&mut app), Some(false), "whitespace is not a direction");
        // (3) a real direction: live.
        app.guidance = "warmer, moodier".into();
        assert_eq!(frame(&mut app), Some(true), "with a direction the dial must be usable");
        // …and the DIAL reaches the request as a tier, which is the only thing
        // the value is for.
        app.direction_adherence = 0.1;
        let hint = autoshade::recipe::DirectionAdherence::new(app.direction_adherence);
        assert_eq!(hint.tier(), autoshade::recipe::AdherenceTier::Hint);
        app.direction_adherence = 0.95;
        let brief = autoshade::recipe::DirectionAdherence::new(app.direction_adherence);
        assert_eq!(brief.tier(), autoshade::recipe::AdherenceTier::Brief);
        assert_ne!(hint.tier(), brief.tier(), "the dial must be able to change the tier");
        // …and the tooltip NAMES the tiers, which is what a user needs to know
        // the dial is not a magnitude.
        let tip = tr(
            Lang::En,
            "How closely the AI follows your direction; disabled until Direction has text: <=40% Hint, 40-70% Direct, above 70% Brief. Prompt intent only - it never moves a render limit. Direct and Brief also decide WHO LEADS: your style library becomes background and its distillation pull is skipped, so a direction can take a photo somewhere your past edits never went. Hint leaves the library in the lead.",
        );
        for tier in ["Hint", "Direct", "Brief"] {
            assert!(tip.contains(tier), "the tooltip must name the {tier} tier: {tip}");
        }
        // v1.2.3: …and that two of those three tiers ALSO decide who leads.
        // A user reading only "prompt intent only" would have no way to know
        // that this dial is what stops their own library from being distilled
        // into the answer.
        for clause in ["WHO LEADS", "background", "Hint leaves the library in the lead"] {
            assert!(tip.contains(clause), "the tooltip must say {clause:?}: {tip}");
        }
        // The zh half is a translation of the SAME key, so a drifted pair
        // would fall back to English here rather than fail silently.
        let zh = tr(Lang::Zh, "How closely the AI follows your direction; disabled until Direction has text: <=40% Hint, 40-70% Direct, above 70% Brief. Prompt intent only - it never moves a render limit. Direct and Brief also decide WHO LEADS: your style library becomes background and its distillation pull is skipped, so a direction can take a photo somewhere your past edits never went. Hint leaves the library in the lead.");
        assert!(zh.contains("主导"), "the zh tooltip must carry the clause too: {zh}");
    }

    /// S2: the description preference survives a restart, defaults OFF in
    /// BOTH defaults, is what both build workers read, and cannot be reached
    /// while the embedding it depends on is off.
    ///
    /// OFF in both defaults is the load-bearing half. The pass downloads a
    /// 4.3 GB checkpoint and adds GPU minutes to every library build; a user
    /// who never asked for it must never pay for it, and a preferences file
    /// written before this key existed must decode to the same answer as a
    /// fresh install rather than to `true`.
    ///
    /// MUTATION THIS KILLS: default the key to `true` in either `Prefs` or
    /// `AutoShadeApp`, drop it from the saved preferences (the round trip then
    /// loses it), or wire one of the two build workers to a literal switch
    /// instead of the preference.
    #[test]
    fn gui_describe_pref_round_trips() {
        assert!(
            !Prefs::default().style_describe,
            "a prefs file written before this key existed must decode to OFF"
        );
        assert!(
            !AutoShadeApp::default().style_describe,
            "a fresh app must not start a 4.3 GB download"
        );
        // A pre-S2 preferences file really is such a file: it has no key at all.
        let old: Prefs = serde_json::from_str("{}").expect("an empty prefs file decodes");
        assert!(!old.style_describe, "a missing key must not read as ON");

        let prefs = Prefs { style_embed: true, style_describe: true, ..Prefs::default() };
        let json = serde_json::to_string(&prefs).expect("prefs serialize");
        assert!(json.contains("style_describe"), "the key must be written, not skipped: {json}");
        let decoded: Prefs = serde_json::from_str(&json).expect("prefs deserialize");
        assert!(decoded.style_describe, "the preference must survive a restart");

        // The preference is what the workers resolve, and the resolver answers
        // a VALUE — no test and no build writes the process environment.
        let on = AutoShadeApp { style_describe: true, ..Default::default() };
        let off = AutoShadeApp { style_describe: false, ..Default::default() };
        let unset = |_: &str| None;
        assert!(autoshade::style::DescribeSwitch::resolve_with(None, on.style_describe, unset).on());
        assert!(!autoshade::style::DescribeSwitch::resolve_with(None, off.style_describe, unset).on());

        let actions = include_str!("../actions.rs");
        assert_eq!(
            actions.matches("DescribeSwitch::resolve(None, self.style_describe)").count(),
            2,
            "the RAW build and the look build both read the preference"
        );
        assert!(
            actions.contains("app.style_describe = prefs.style_describe;"),
            "the saved preference must be restored at start-up"
        );
        assert!(
            include_str!("../app.rs").contains("style_describe: self.style_describe,"),
            "…and written back when preferences are saved"
        );
        // The checkbox is unreachable while the embedding is off: the prose
        // only enters the ranking through the SigLIP text tower, so a
        // description-only build would produce a field nothing can retrieve on.
        let panel = include_str!("../panels/ai.rs");
        let gated = panel
            .split("ui.add_enabled_ui(self.style_embed, |ui| {")
            .nth(1)
            .expect("the describe checkbox sits behind an enablement gate");
        // NB the needle is spelled without its leading `ui.`: a literal that
        // reads as a widget constructor makes `scripts/audit_i18n.py`'s
        // bypass scanner start parsing THIS test as GUI code and desynchronise
        // for the rest of the file (109 phantom findings, measured).
        let first = gated.lines().find(|l| l.contains("checkbox(")).unwrap_or("");
        assert!(
            first.contains("&mut self.style_describe"),
            "the gated block must be the describe checkbox itself: {first}"
        );
    }

    #[test]
    fn gui_prefs_round_trip_the_three_new_keys() {
        let prefs = Prefs {
            style_embed: true,
            looks_src_dir: Some(std::path::PathBuf::from("D:/looks")),
            use_looks: false,
            direction_adherence: 0.91,
            ..Prefs::default()
        };
        let json = serde_json::to_string(&prefs).expect("prefs serialize");
        let decoded: Prefs = serde_json::from_str(&json).expect("prefs deserialize");
        assert!(decoded.style_embed);
        assert_eq!(decoded.looks_src_dir, prefs.looks_src_dir);
        assert!(!decoded.use_looks);
        assert!((decoded.direction_adherence - 0.91).abs() < 1e-6);
    }

    /// C2 migration (user ruling 2026-08-31: migrate, do not accept the old
    /// name): the eframe prefs directory follows the rename. The core runs on
    /// explicit paths — the resolver half only names the two directories via
    /// `eframe::storage_dir` and is exercised by every real launch.
    ///
    /// The paths here are the ones production actually passes:
    /// `eframe::storage_dir(key)` is `<roaming>/<key>/data` on Windows, so the
    /// two directories are TWO levels deep and `<roaming>/AutoShade` does not
    /// exist on a machine that never ran the new name. An earlier version of
    /// this test renamed siblings under one existing base — a shape Windows
    /// never produces — and asserted that a missing destination parent SHOULD
    /// fall back, which pinned the shipped defect as correct behaviour.
    #[test]
    fn pre_rename_prefs_are_adopted_renamed_kept_or_fallen_back() {
        let base = std::env::temp_dir()
            .join(format!("autoshade-c2-prefs-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // `eframe::storage_dir` shape: <base>/<key>/data, two levels deep.
        let storage_dir = |key: &str| base.join(key).join("data");
        let legacy = storage_dir("Autoshop");
        let current = storage_dir("AutoShade");
        // Nothing to adopt.
        assert_eq!(adopt_prefs_between(&current, &legacy, true), PrefsAdoption::Nothing);
        // The real thing: the directory moves wholesale, prefs file included —
        // and `base/AutoShade` is deliberately absent, exactly as on a machine
        // upgrading from a pre-rename build.
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("app.ron"), "(prefs)").unwrap();
        assert!(!base.join("AutoShade").exists(), "the destination parent is absent");
        assert_eq!(adopt_prefs_between(&current, &legacy, true), PrefsAdoption::Migrated);
        assert!(current.join("app.ron").is_file(), "the prefs came along");
        assert!(!legacy.exists(), "moved, not copied");
        // Both spellings present: the new one wins and the old is untouched.
        std::fs::create_dir_all(&legacy).unwrap();
        assert_eq!(adopt_prefs_between(&current, &legacy, true), PrefsAdoption::KeptBoth);
        assert!(legacy.exists(), "kept, not merged or deleted");
        // A rename that CANNOT be rescued by making the parent: a regular file
        // sits where the destination's parent directory would go, so
        // `create_dir_all` fails and the session keeps the legacy key rather
        // than resetting anyone's prefs.
        let blocked_parent = base.join("blocked");
        std::fs::write(&blocked_parent, "not a directory").unwrap();
        let blocked = blocked_parent.join("AutoShade").join("data");
        assert_eq!(adopt_prefs_between(&blocked, &legacy, true), PrefsAdoption::FellBack);
        assert!(legacy.exists(), "nothing lost on the failed arm");
        // The macOS opt-out arm: adopt=false answers Nothing even with a
        // legacy directory present.
        assert_eq!(adopt_prefs_between(&current, &legacy, false), PrefsAdoption::Nothing);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The key pair and the platform gate are one decision (the shape of
    /// store.rs's per-platform test): one spelling everywhere, adoption off
    /// only where `Application Support` could hold a stranger's directory.
    #[test]
    fn the_storage_key_and_its_adoption_are_one_decision_per_platform() {
        assert_eq!(
            (STORAGE_KEY, LEGACY_STORAGE_KEY, ADOPT_PRE_RENAME_PREFS),
            ("AutoShade", "Autoshop", !cfg!(target_os = "macos")),
            "the eframe key, the adopted spelling, and the macOS opt-out"
        );
    }

    /// The widest button rows, shared by the default-width and readable-width pins.
    fn app_with_every_button(lang: crate::i18n::Lang) -> AutoShadeApp {
        use autoshade::recipe::{ColourField, LocalAdjustment, MaskComponent, MaskGeometry};
        use crate::model::{Variant, VariantKind};
        let mk = |kind| Variant {
            kind,
            id: crate::model::new_variant_id(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        let mut app = AutoShadeApp {
            lang,
            src_path: Some(PathBuf::from("D:/library/_buttons.ARW")),
            variants: vec![mk(VariantKind::Original), mk(VariantKind::Generated)],
            active: 1,
            versions: vec![1],
            fit_ref: Some(PathBuf::from("D:/library/_buttons-reference.png")),
            multi_sel: [0usize].into_iter().collect(),
            ..Default::default()
        };
        app.base_preview = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(64, 96)));
        app.recipe.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "mask-buttons.png".into() },
            components: vec![MaskComponent::default()],
            ..Default::default()
        });
        app.recipe.colour_field =
            Some(ColourField { x: 1, y: 1, b: 1, grid: vec![[0.0; 5]], amount: 0.5, enabled: true });
        // v1.5.0: a Point Color swatch — its row is a chip, a name and a
        // ✕, and its four sliders are laid beside the mixer's own.
        app.recipe.point_colors.push(autoshade::recipe::PointColor::sampled(0.9, 0.7, 0.5));
        // A histogram, so the readout's width can be pinned (2026-09-27).
        app.histogram = Some(vec![[0.5; 4]; 256]);
        app.sel_mask = Some(0);
        app.start_mask_brush(None);
        assert!(app.mask_brush.is_some(), "{lang:?}: the brush session armed");
        app
    }

    /// R38: every button in the vocabulary (`buttons.rs`) stands exactly one
    /// row tall, icon-only verbs are squares of that row, and no label wraps
    /// inside its grid cell — rendered at the default panel widths in both
    /// languages with the widest rows armed: a second variant card, a mask
    /// brush session, a raster mask selected with a component, a colour
    /// field, a Point Color swatch, a version, a reference, a
    /// multi-selection. The helpers fill the registry themselves, so a button
    /// that bypasses the vocabulary is simply not here; the count floor keeps
    /// the pin from passing on an empty registry.
    ///
    /// Three frames, with both side panels' widths as witnesses. A cell or a
    /// scoped widget laid past its line's end stands outside the panel and
    /// widens an auto-fitting panel every frame (R19's runaway, met again as
    /// +47 px/frame while this vocabulary went in), so a panel that holds
    /// its default width for three frames has no overflowing row anywhere
    /// in it — the strongest form of the witness, not merely "stable".
    #[test]
    fn every_button_stands_one_row_tall_at_the_default_widths() {
        use crate::buttons::DRAWN;
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let row = ctx.style().spacing.interact_size.y;
            let mut app = app_with_every_button(lang);
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 20_000.0),
                )),
                ..Default::default()
            };
            let mut widths: Vec<[f32; 2]> = Vec::new();
            let mut rects: Vec<[egui::Rect; 2]> = Vec::new();
            let mut sections = [0.0f32; 3];
            for _ in 0..3 {
                DRAWN.with_borrow_mut(Vec::clear); // the LAST frame's registry only
                let _ = ctx.run(input(), |ctx| {
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    app.upd_top_bar(ctx);
                    let gallery = egui::SidePanel::left("gallery")
                        .default_width(240.0)
                        .show(ctx, |ui| app.gallery_panel(ui));
                    let controls = egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            app.ai_panel(ui);
                            sections[0] = ui.min_rect().width();
                            app.develop_panel(ui);
                            sections[1] = ui.min_rect().width();
                            app.retouch_panel(ui);
                            sections[2] = ui.min_rect().width();
                        });
                    });
                    egui::Window::new(tr(lang, "Settings"))
                        .id(egui::Id::new("settings_window"))
                        .default_width(480.0)
                        .show(ctx, |ui| app.settings_ui(ui));
                    widths.push([gallery.response.rect.width(), controls.response.rect.width()]);
                    rects.push([gallery.response.rect, controls.response.rect]);
                });
            }
            let drawn = DRAWN.with_borrow(|d| d.clone());
            for (i, (panel, default)) in [("gallery", 240.0f32), ("controls", 320.0)].into_iter().enumerate() {
                let seen: Vec<f32> = widths.iter().map(|w| w[i]).collect();
                let edge = rects[2][i].left() + default + 0.5;
                let past: Vec<String> = drawn
                    .iter()
                    .filter(|d| d.rect.left() >= rects[2][i].left() && d.rect.right() > edge)
                    .map(|d| format!("{} {:?} {:?}", d.kind, d.label, d.rect))
                    .collect();
                assert!(
                    seen.iter().all(|w| (w - default).abs() <= 0.5),
                    "{lang:?}: the {panel} panel left its {default} px default: {seen:?} — a row overflows it                      (section widths ai/develop/retouch {sections:?}; buttons past the edge: {past:?})"
                );
            }
            assert!(drawn.iter().any(|d| d.label == tr(lang, "✨ Adjust")),
                "{lang:?}: the new adjust button reached DRAWN");
            assert!(drawn.len() >= 60, "{lang:?}: only {} buttons reached the registry", drawn.len());
            for d in &drawn {
                assert!(
                    (d.rect.height() - row).abs() <= 0.5,
                    "{lang:?}: {} {:?} stands {:.1} px, not one row ({row})",
                    d.kind, d.label, d.rect.height()
                );
                if d.kind.starts_with("glyph") {
                    assert!(
                        (d.rect.width() - row).abs() <= 0.5,
                        "{lang:?}: {} {:?} is {:.1} px wide, not a square",
                        d.kind, d.label, d.rect.width()
                    );
                }
                assert!(d.fits, "{lang:?}: {} {:?} does not fit its cell on one line", d.kind, d.label);
            }
            assert!(
                drawn.iter().filter(|d| d.kind == "primary").count() >= 8,
                "{lang:?}: the primary verbs are on screen"
            );
        }
    }

    /// The panel stays free to grow for the curve and HSL editors, while every
    /// row-filling button shares the prompts' readable ceiling. Three frames
    /// also catch a row that asks for more than it has and grows the panel.
    #[test]
    fn a_button_row_never_grows_past_the_readable_width() {
        use crate::buttons::DRAWN;
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            for controls_width in [800.0, 1600.0] {
                let ctx = egui::Context::default();
                crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
                let mut app = app_with_every_button(lang);
                let mut widths = Vec::new();
                for frame in 0..3 {
                    DRAWN.with_borrow_mut(Vec::clear);
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(2400.0, 1200.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            ctx.memory_mut(|m| m.set_everything_is_visible(true));
                            let gallery = egui::SidePanel::left("gallery")
                                .default_width(800.0)
                                .show(ctx, |ui| app.gallery_panel(ui));
                            let controls = controls_panel(ctx, &mut app, controls_width);
                            widths.push([gallery.response.rect.width(), controls.width()]);
                        },
                    );
                    let drawn = DRAWN.with_borrow(|d| d.clone());
                    // The histogram readout used to be the one widget that grew
                    // with the panel without limit (user report 2026-09-27).
                    let hist = app.histogram_rect.expect("the histogram readout was drawn");
                    assert!(
                        hist.width() <= crate::theme::FIELD_W_MAX + 0.5,
                        "{lang:?}, {controls_width} px, frame {frame}: the histogram is {:.1} px wide — past the {:.0} px readable ceiling",
                        hist.width(), crate::theme::FIELD_W_MAX
                    );
                    for label in [
                        tr(lang, "🤖 AI heal (auto)"), tr(lang, "Heal area"),
                        tr(lang, "⎘ Enter stamp"), tr(lang, "⎘ Clone area"),
                        tr(lang, "Reverse-fit recipe"), tr(lang, "Extract style"),
                        tr(lang, "Copy recipe"), tr(lang, "＋ Linear"), tr(lang, "Erase"),
                    ] {
                        assert!(
                            drawn.iter().any(|d| d.label == label),
                            "{lang:?}, {controls_width} px, frame {frame}: missing button {label:?}"
                        );
                    }
                    for d in &drawn {
                        assert!(
                            d.rect.width() <= crate::theme::FIELD_W_MAX + 0.5,
                            "{lang:?}, {controls_width} px, frame {frame}: {} {:?} is {:.1} px wide — \
                             past the {:.0} px readable ceiling",
                            d.kind, d.label, d.rect.width(), crate::theme::FIELD_W_MAX
                        );
                        assert!(
                            d.fits,
                            "{lang:?}, {controls_width} px, frame {frame}: {} {:?} does not fit its cell on one line",
                            d.kind, d.label
                        );
                    }
                }
                for (i, (panel, expected)) in [("gallery", 800.0), ("controls", controls_width)].into_iter().enumerate() {
                    let seen: Vec<f32> = widths.iter().map(|w| w[i]).collect();
                    assert!(
                        (seen[0] - seen[2]).abs() < 0.5,
                        "{lang:?}: the wide {panel} panel must not grow across frames: {seen:?}"
                    );
                    assert!(
                        seen.iter().all(|w| (w - expected).abs() <= 0.5),
                        "{lang:?}: the {panel} panel left its {expected} px width: {seen:?}"
                    );
                }
            }
        }
    }

    /// R27: the clone stamp is a pixel worker like heal and fill — its mask
    /// is exported from the turned canvas while `retouch::clone_stamp`
    /// re-develops the un-turned frame — so it takes the same refusal gate,
    /// before any output claim or worker. MUTATION: drop the
    /// `|| self.refuse_pixel_work_on_a_turned_photo()` in start_clone and the
    /// status below becomes the Alt+click prompt.
    #[test]
    fn a_clone_on_a_turned_photo_is_refused_before_it_asks_for_a_source_point() {
        for lang in [Lang::En, Lang::Zh] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            app.src_path = Some(PathBuf::from("clone-turn-guard.png"));
            app.clone_src = None; // the gate answers before this is even asked for
            let refusal = tr(
                lang,
                "this photo is turned, and pixel repairs still work on the un-turned frame — turn it back to 0 first",
            );
            let ask = tr(lang, "Alt+click to set the clone source first");
            for turns in [1u8, 2, 3, 5] {
                app.recipe.quarter_turns = turns;
                app.status.clear();
                app.start_clone();
                assert!(!app.busy, "quarter_turns={turns}: a refused clone never reaches a worker");
                assert_eq!(app.status, refusal, "quarter_turns={turns}");
            }
            // No turn, or a full one, IS the un-turned frame: the verb goes on
            // to its own next check, exactly as heal and fill do.
            for turns in [0u8, 4] {
                app.recipe.quarter_turns = turns;
                app.status.clear();
                app.start_clone();
                assert!(!app.busy);
                assert_eq!(app.status, ask, "quarter_turns={turns}");
            }
        }
    }

    /// The AI mask refine is a pixel worker too (its guide is decoded in the
    /// EXIF frame, the raster it refines lives in the turned plate frame), so
    /// it takes the same refusal gate before any worker. MUTATION: drop the
    /// `|| self.refuse_pixel_work_on_a_turned_photo()` in start_mask_refine.
    #[test]
    fn a_mask_refine_on_a_turned_photo_is_refused_before_it_spawns() {
        for lang in [Lang::En, Lang::Zh] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            app.src_path = Some(PathBuf::from("refine-turn-guard.png"));
            let refusal = tr(
                lang,
                "this photo is turned, and pixel repairs still work on the un-turned frame — turn it back to 0 first",
            );
            for turns in [1u8, 2, 3] {
                app.recipe.quarter_turns = turns;
                app.status.clear();
                app.start_mask_refine(0);
                assert!(!app.busy, "quarter_turns={turns}: a refused refine never reaches a worker");
                assert_eq!(app.status, refusal, "quarter_turns={turns}");
            }
            // Un-turned, with no mask at that index: the verb returns quietly.
            app.recipe.quarter_turns = 0;
            app.status.clear();
            app.start_mask_refine(0);
            assert!(!app.busy);
            assert!(app.status.is_empty(), "{}", app.status);
        }
    }

    /// An unreadable CENTRAL XMP is `Unreadable`, exactly as an unreadable
    /// recipe.json is — not `NoopOnly`. The XMP loop set `any` and broke
    /// without recording the error, so the store answered "sidecars exist,
    /// nothing in them": the open note said nothing and the next Ctrl+S
    /// overwrote the file with no version backup. MUTATION: drop the
    /// `parse_err` assignment in the XMP loop's read-error arm and this
    /// sees NoopOnly again.
    #[test]
    fn an_unreadable_central_xmp_is_reported_not_folded_into_noop() {
        let src = std::path::Path::new("D:/library/_xmp_unreadable_test.ARW"); // never touched
        let dev = autoshade::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev); // a crashed earlier run may have left files
        std::fs::create_dir_all(&dev).unwrap();
        let xp = autoshade::pipeline::xmp_target(src);
        let _scrub = Scrub(vec![dev.clone(), xp.clone()]);
        // Present but unreadable: `read_text_capped` answers InvalidData (not
        // NotFound) for bytes that are not UTF-8 — the same door an over-cap
        // or permission-denied file comes through.
        std::fs::write(&xp, [0xffu8, 0xfe, 0x00, 0x80]).unwrap();
        let RestoredDevelop { saved, .. } = read_saved_develop(src);
        let SavedDevelop::Unreadable { err, fallback } = saved else {
            panic!("an unreadable central XMP must be reported, not skipped");
        };
        assert!(fallback.is_none(), "nothing else answered");
        assert!(err.starts_with("read "), "the recipe loop's own phrasing: {err}");
    }

    /// A strip entry's recipe is untrusted input like the active card's —
    /// `read_saved_develop_locked` clamps recipe.json on the way in, but
    /// `strip_from_record` copied variants.json recipes verbatim, and a card
    /// is one click from BEING the canvas: a crop wider than the frame
    /// reached the crop-nudge maths in `upd_shortcuts`
    /// (`x.clamp(0.0, 1.0 - w)` with `w > 1`), which panics on `min > max`.
    /// MUTATION: drop the `recipe.clamp()` in strip_from_record.
    #[test]
    fn strip_from_record_clamps_each_restored_recipe() {
        let wild = EditRecipe {
            crop: Some(autoshade::recipe::Crop { left: 0.0, top: 0.0, right: 2.0, bottom: 1.5 }),
            contrast: 9_000.0,
            ..Default::default()
        };
        let rec = autoshade::store::VariantsRecord {
            extra: Default::default(),
            v: 1,
            active_kind: "original".into(),
            active_pos: 0,
            active_id: None,
            active_name: None,
            others: vec![autoshade::store::VariantEntry {
                extra: Default::default(),
                kind: "fitted".into(),
                recipe: wild.clone(),
                origin: None,
                id: Some("card-wild".into()),
                name: None,
            }],
        };
        let (strip, _) = crate::persist::strip_from_record(&rec, None);
        let mut expected = wild;
        let _ = expected.clamp();
        assert_eq!(strip[0].recipe, expected, "the restored card carries the CLAMPED recipe");
        let c = strip[0].recipe.crop.expect("a clamped crop is kept: ordered and in-frame");
        let w = c.right - c.left;
        assert!(w <= 1.0, "crop width {w} would panic the keyboard nudge's clamp(0.0, 1.0 - w)");
        // The nudge maths itself, as upd_shortcuts spells it — a panic here is the bug.
        let _ = (c.left + 0.005).clamp(0.0, 1.0 - w);
    }

    /// egui keys a window's remembered state (position) off its id, and the
    /// id defaults to the TITLE — every window title here is localised, so a
    /// language switch dropped the Shortcuts window's state while Settings,
    /// with a fixed id, kept its own. Pinned in the source: a headless test
    /// cannot drive eframe's window memory. Each builder chain is located by
    /// its constructor, never by its title text. MUTATION: delete the
    /// `.id(...)` line under the Shortcuts window.
    #[test]
    fn every_window_in_app_rs_carries_a_fixed_egui_id() {
        let frame = include_str!("../app.rs");
        let mut seen = 0;
        // The needle stops at the path: `scripts/audit_i18n.py` walks the
        // parentheses of every window constructor it sees in GUI source,
        // string literals included, so spelling the call out here would
        // derail its literal scanner for the rest of the file.
        for (start, _) in frame.match_indices("egui::Window::") {
            let end = frame[start..].find(".show(ctx").expect("the window's show call") + start;
            let builder = &frame[start..end];
            assert!(
                builder.contains(".id(egui::Id::new("),
                "a window whose id is its localised title, so its state resets with the language:\n{builder}"
            );
            seen += 1;
        }
        assert!(seen >= 2, "premise: the Settings and Shortcuts windows are both built here ({seen} found)");
    }

    /// R27, every door: each pixel worker that lands a BAKED master takes
    /// the turned-photo gate before any output claim or worker thread — the
    /// masked ones because their mask is in the turned frame, and the
    /// mask-free ones (AI denoise, stack, reimagine) because their master
    /// lands as a ◈/▦/✨ card whose raster is in the EXIF frame: `load_active`
    /// installs it as the plate and `sync_base_turns` cannot turn a baked
    /// card. MUTATION: drop the gate at any one of the four doors below and
    /// that door's status is no longer the refusal.
    #[test]
    fn every_pixel_worker_refuses_a_turned_photo() {
        let (mut app, _scrub) = app_with_masked_photo("turned-doors");
        let src = std::env::temp_dir().join(format!("autoshade-turned-doors-{}.arw", std::process::id()));
        std::fs::write(&src, b"raw").unwrap();
        let _src_scrub = Scrub(vec![src.clone()]);
        app.src_path = Some(src.clone());
        app.recipe.quarter_turns = 1;
        type Door = fn(&mut AutoShadeApp, PathBuf);
        let doors: [(&str, Door); 4] = [
            ("start_ai_denoise", |a, _| a.start_ai_denoise()),
            ("start_clone", |a, _| a.start_clone()),
            ("start_reimagine", |a, _| a.start_reimagine()),
            ("start_stack", |a, p| a.start_stack(vec![p])),
        ];
        for (name, door) in doors {
            app.busy = false;
            app.status.clear();
            door(&mut app, src.clone());
            assert!(!app.busy, "{name}: a refused pixel worker never reaches a worker thread");
            assert!(app.status.contains("this photo is turned"), "{name}: {}", app.status);
        }
    }

    /// A turn moves the plate AND the paint canvas: the brush canvas is sized
    /// to the plate, so after `sync_base_turns` transposes the plate a canvas
    /// left in the old frame painted sideways (and off the long edge). The
    /// canvas is REBOUND — empty — to the plate's new frame. MUTATION: drop
    /// the `rebind_paint_canvas` call in sync_base_turns.
    #[test]
    fn a_turn_rebinds_the_paint_canvas_to_the_transposed_plate() {
        let (mut app, _scrub) = app_with_masked_photo("rotate-paint");
        let ctx = egui::Context::default();
        let plate = std::sync::Arc::new(image::DynamicImage::new_rgb8(8, 6));
        app.base_preview = Some(plate.clone());
        app.source_preview = Some(plate);
        app.base_turns = 0;
        app.mask_paint = Some(image::RgbaImage::new(8, 6));
        app.recipe.quarter_turns = 1;
        app.sync_base_turns(&ctx);
        assert_eq!(app.base_preview.as_ref().unwrap().dimensions(), (6, 8));
        assert_eq!(
            app.mask_paint.as_ref().unwrap().dimensions(),
            (6, 8),
            "the paint canvas follows the plate's frame"
        );
        assert!(!app.has_painted_mask(), "a rebound canvas starts empty");
    }

    /// The coverage overlay and the range eyedropper share the one-slot
    /// `overlay_ref` cache, compared by recipe equality, so both must build
    /// the reference recipe through ONE builder — the hand-copied twin that
    /// forgot the manual CA pair missed the cache on every click under a
    /// non-zero Red/cyan or Blue/yellow and paid a full develop on the UI
    /// thread. MUTATION: drop `pre.ca_r = 0.0` from range_reference_recipe,
    /// or rebuild the copy by hand in either caller.
    #[test]
    fn the_range_reference_is_one_builder_with_every_geometry_field_stripped() {
        let (mut app, _scrub) = app_with_masked_photo("range-ref");
        app.recipe.masks.push(app.recipe.masks[0].clone());
        app.recipe.ca_r = 0.4;
        app.recipe.ca_b = -0.3;
        app.recipe.straighten_deg = 2.0;
        app.recipe.lens_distortion = 0.1;
        let pre = app.range_reference_recipe(1);
        assert_eq!(pre.masks.len(), 1, "mask 1's reference is the prefix before it");
        assert_eq!(
            (pre.ca_r, pre.ca_b),
            (0.0, 0.0),
            "manual CA is geometry — stripped, or the eyedropper misses the overlay's cache"
        );
        assert_eq!((pre.straighten_deg, pre.lens_distortion), (0.0, 0.0));
        assert!(pre.crop.is_none());
        let canvas = include_str!("../canvas.rs");
        for head in ["pub(crate) fn refresh_mask_overlay(", "pub(crate) fn handle_range_pick("] {
            let body = &canvas[canvas.find(head).unwrap_or_else(|| panic!("{head} moved"))..];
            let end = body[head.len()..].find("\n    pub(crate) fn ").expect("the next method") + head.len();
            assert!(
                body[..end].contains("self.range_reference_recipe("),
                "{head} builds its own reference copy again"
            );
        }
    }

    /// 2026-09-27 (user report 「UI 按钮的大小参差不齐」, decided on a probe of
    /// every drawn button): in the side panels every text verb is laid in a
    /// grid cell — a row's one verb fills the row, two or three share it, a
    /// verb beside a checkbox or a label takes one cell of the two-column
    /// grid — so no verb is sized by its own label. Two list/header verbs are
    /// the exceptions (a version row's 「Load」, the gallery's 「🗂 Open
    /// folder…」 beside its heading); the toolbar, the dialogs and Settings
    /// keep own-width verbs. Also pinned: the AI section's rows start on the
    /// same x as the Develop and Retouch rows (its outer fold used to indent
    /// them 18 px further), and each brush fold carries its own
    /// 「🖌 Paint area」 / 「Clear area」 pair.
    #[test]
    fn every_side_panel_text_verb_is_laid_in_a_cell_and_the_sections_align() {
        use crate::buttons::DRAWN;
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = app_with_every_button(lang);
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 20_000.0))),
                ..Default::default()
            };
            let mut rects = [egui::Rect::NOTHING; 2];
            let mut sections: Vec<(&'static str, f32, f32)> = Vec::new();
            for _ in 0..3 {
                DRAWN.with_borrow_mut(Vec::clear);
                let mut inner = Vec::new();
                let _ = ctx.run(input(), |ctx| {
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    app.upd_top_bar(ctx);
                    let gallery = egui::SidePanel::left("gallery")
                        .default_width(240.0)
                        .show(ctx, |ui| app.gallery_panel(ui));
                    let controls = egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let y0 = ui.min_rect().top();
                            app.ai_panel(ui);
                            let y1 = ui.min_rect().bottom();
                            app.develop_panel(ui);
                            let y2 = ui.min_rect().bottom();
                            app.retouch_panel(ui);
                            let y3 = ui.min_rect().bottom();
                            inner = vec![("ai", y0, y1), ("develop", y1, y2), ("retouch", y2, y3)];
                        });
                    });
                    rects = [gallery.response.rect, controls.response.rect];
                });
                sections = inner;
            }
            let drawn = DRAWN.with_borrow(|d| d.clone());
            let text = |d: &crate::buttons::Drawn| !d.kind.starts_with("glyph");
            let load = tr(lang, "Load");
            let open = tr(lang, "🗂 Open folder…");
            for d in drawn.iter().filter(|d| text(d)) {
                let c = d.rect.center();
                let (panel, exempt) = if rects[1].contains(c) {
                    ("controls", d.label == load)
                } else if rects[0].contains(c) {
                    ("gallery", d.label == open)
                } else {
                    continue;
                };
                assert!(
                    d.cell || exempt,
                    "{lang:?}: {} {:?} in the {panel} panel is laid at its own width ({:.0} px), not in a cell",
                    d.kind, d.label, d.rect.width()
                );
            }
            let left_of = |name: &str| -> f32 {
                let (_, y0, y1) = sections.iter().find(|s| s.0 == name).copied().expect("the section was laid");
                drawn
                    .iter()
                    .filter(|d| text(d) && rects[1].contains(d.rect.center()))
                    .filter(|d| d.rect.center().y >= y0 && d.rect.center().y < y1)
                    .map(|d| d.rect.left())
                    .fold(f32::INFINITY, f32::min)
            };
            let (ai, dev, ret) = (left_of("ai"), left_of("develop"), left_of("retouch"));
            assert!(ai.is_finite() && dev.is_finite() && ret.is_finite(), "{lang:?}: every section drew a text verb");
            assert!(
                (ai - dev).abs() <= 0.5 && (dev - ret).abs() <= 0.5,
                "{lang:?}: the sections' rows start at different x: ai {ai}, develop {dev}, retouch {ret}"
            );
            let count = |label: &str| drawn.iter().filter(|d| d.label == label).count();
            assert_eq!(count(tr(lang, "🖌 Paint area")), 3, "{lang:?}: Fill, Heal and Adjust each carry their own brush toggle");
            assert_eq!(count(tr(lang, "Clear area")), 4, "{lang:?}: Fill, Heal, the Stamp and Adjust each carry their own clear verb");
        }
    }

    /// Each tool keeps its own painted area (user decision 2026-09-27): what
    /// is painted for Heal is not what Fill, the Stamp or Adjust read; a mask
    /// brush session stashes the live area and gives it back; a plate rebind
    /// starts every tool blank; a landing clears only the area its result
    /// consumed.
    #[test]
    fn each_tool_keeps_its_own_painted_area() {
        let paint = |app: &mut AutoShadeApp| {
            app.mask_paint.as_mut().unwrap().put_pixel(1, 1, image::Rgba([255, 64, 64, 160]));
            app.paint_mask_changed(Some(true));
        };
        let mut app = AutoShadeApp {
            base_preview: Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(8, 6))),
            ..Default::default()
        };
        app.rebind_paint_canvas(8, 6);
        assert_eq!(app.paint_owner, BrushOwner::Fill, "Fill's area is the canvas by default");

        app.arm_brush(BrushOwner::Heal);
        assert!(app.paint_mode && app.brush_armed(BrushOwner::Heal) && !app.brush_armed(BrushOwner::Fill));
        paint(&mut app);
        assert!(app.area_painted(BrushOwner::Heal));
        assert!(!app.area_painted(BrushOwner::Fill) && !app.area_painted(BrushOwner::Adjust));

        // Fill's turn: a blank canvas, and Heal's strokes wait in the store
        // with their memo — reading them back scans nothing.
        let scans = app.mask_presence_scans.get();
        app.arm_brush(BrushOwner::Fill);
        assert!(!app.has_painted_mask(), "Fill sees its own blank area, not Heal's strokes");
        assert!(app.area_painted(BrushOwner::Heal), "Heal's strokes survive off the canvas");
        assert_eq!(app.mask_presence_scans.get(), scans, "a stored memo answers without a scan");
        assert!(app.export_area_png(BrushOwner::Heal).is_some());
        assert!(app.export_area_png(BrushOwner::Fill).is_none());

        // The Stamp arms through clone_mode and takes the paint flag down.
        app.arm_brush(BrushOwner::Stamp);
        assert!(app.clone_mode && !app.paint_mode && app.brush_armed(BrushOwner::Stamp));
        paint(&mut app);
        assert!(app.area_painted(BrushOwner::Stamp) && app.area_painted(BrushOwner::Heal));

        // Clearing one tool's area leaves the others'.
        app.clear_area(BrushOwner::Heal);
        assert!(!app.area_painted(BrushOwner::Heal) && app.area_painted(BrushOwner::Stamp));
        app.arm_brush(BrushOwner::Heal);
        assert!(!app.has_painted_mask(), "Heal comes back blank");

        // A mask-brush session stashes the live area and gives it back.
        app.arm_brush(BrushOwner::Adjust);
        paint(&mut app);
        app.start_mask_brush(None);
        assert!(!app.has_painted_mask(), "the session starts on its own blank canvas");
        assert!(app.area_painted(BrushOwner::Adjust), "Adjust's strokes wait in the store");
        assert!(!app.brush_armed(BrushOwner::Adjust), "a session belongs to no tool");
        app.disarm_brush();
        assert!(app.mask_brush.is_none() && !app.paint_mode);
        assert!(
            app.has_painted_mask() && app.paint_owner == BrushOwner::Adjust,
            "the session's end restores Adjust's area"
        );

        // The landing clears only the area its result consumed.
        assert_eq!(RetouchNote::Filled(PathBuf::from("x.png")).consumed_area(), Some(BrushOwner::Fill));
        assert_eq!(RetouchNote::Cloned { n: 1, out: PathBuf::from("x.png") }.consumed_area(), Some(BrushOwner::Stamp));
        assert_eq!(
            RetouchNote::Reimagined { out: PathBuf::from("x.png"), divergence: 0.1, discarded: None }.consumed_area(),
            None
        );
        assert!(
            include_str!("../workers.rs").contains("if let Some(owner) = note.consumed_area()"),
            "the landing clears through consumed_area"
        );

        // A plate rebind starts every tool blank.
        app.rebind_paint_canvas(8, 6);
        assert!(!app.area_painted(BrushOwner::Adjust) && !app.area_painted(BrushOwner::Stamp));
        assert!(app.paint_store.iter().all(Option::is_none));
    }
