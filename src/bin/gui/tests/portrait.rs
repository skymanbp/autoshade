// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the
// Portraits mode's panel (panels/portrait.rs, design-portraits-mode.md §3–§4;
// v1.14.0).

    /// The four masks the find step makes, over rasters that need not exist
    /// (the panel reads the recipe, not the pixels).
    fn found_masks() -> Vec<autoshade::recipe::LocalAdjustment> {
        vec![
            autoshade::face::portrait_adjustment(autoshade::recipe::MaskRole::Face, "mask-face.png".into()),
            autoshade::face::portrait_adjustment(autoshade::recipe::MaskRole::Eyes, "mask-eyes.png".into()),
            autoshade::face::skin_adjustment("mask-face.png".into(), [0.8, 0.6, 0.5], [0.4, 0.3]),
            autoshade::face::background_adjustment("mask-subject.png".into()),
        ]
    }

    /// The photo with every button armed, in Portraits, with or without the
    /// people found.
    fn portrait_app(lang: Lang, found: bool) -> AutoShadeApp {
        let mut app = app_with_every_button(lang);
        app.choose_mode(Mode::Portraits);
        if found {
            app.recipe.masks.extend(found_masks());
        }
        app
    }

    /// Every text the Portraits panel draws on its own, in one tall frame.
    fn portrait_panel_texts(app: &mut AutoShadeApp) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut out = None;
        for i in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 4000.0))),
                time: Some(i as f64),
                ..Default::default()
            };
            out = Some(ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.portrait_panel(ui));
            }));
        }
        drawn_texts(&out.unwrap().shapes)
    }

    /// Design §3 on one frame, in both languages: the panel's heading and its
    /// four groups with every control the sketch names, and none of the
    /// workspace's trade words — the panel speaks to people who photograph
    /// people, even though the full workspace stays below it.
    ///
    /// MUTATION THIS CATCHES: drop a group or a control from `portrait_panel`,
    /// or word one of them with a trade term (a "mask", a "develop"), and a
    /// line below fails.
    #[test]
    fn the_portrait_panel_draws_four_groups_in_plain_words_in_either_language() {
        let words = ["配方", "显影", "反推", "变体", "蒙版", "API", "recipe", "develop", "reverse-fit", "variant", "mask"];
        for lang in [Lang::En, Lang::Zh] {
            let mut app = portrait_app(lang, true);
            let texts = portrait_panel_texts(&mut app);
            let has = |s: &str| texts.iter().any(|t| t.starts_with(s));
            // Through a function pointer: the keys below are data here, and
            // the i18n audit reads a `tr` call's argument as a call site.
            let say: fn(Lang, &'static str) -> &'static str = tr;
            for key in [
                "Portraits",
                "People and background",
                "🤖 Find people and faces",
                "✨ AI portrait look",
                "Look strength",
                "Skin",
                "Soften skin",
                "Skin warmth",
                "Skin colour intensity",
                "Keep skin natural",
                "Eyes",
                "Brighten eyes",
                "Crisp eyes",
                "Background",
                "Blur amount",
                "Sharp range",
                "Keep the person sharp",
                "Darken background",
                "Fade background colour",
            ] {
                assert!(has(say(lang, key)), "{lang:?}: the panel draws {key:?}");
            }
            assert!(!has(tr(lang, "No face found in this photo")), "{lang:?}: faces are found, nothing is greyed");
            for text in &texts {
                let low = text.to_lowercase();
                for w in words {
                    let hit = if w.is_ascii() && w != "API" { low.contains(w) } else { text.contains(w) };
                    assert!(!hit, "{lang:?}: {text:?} says {w:?}");
                }
            }
            // The hovers are drawn only under the pointer: read them from the
            // source, where every one is a literal handed to the translator.
            let src = include_str!("../panels/portrait.rs");
            let body = &src[src.find("fn portrait_panel(").unwrap()..];
            let call = format!("{}(lang, \"", "tr");
            for lit in body.split(call.as_str()).skip(1).filter_map(|s| s.split('"').next()) {
                let shown = say(lang, lit);
                for w in words {
                    let hit = if w.is_ascii() && w != "API" { shown.to_lowercase().contains(w) } else { shown.contains(w) };
                    assert!(!hit, "{lang:?}: {shown:?} says {w:?}");
                }
            }
        }
    }

    /// The panel sits at the TOP of the sidebar in Portraits, above the full
    /// workspace, and is drawn in no other mode.
    ///
    /// MUTATION THIS CATCHES: `portrait_panel` off for Portraits (or on for
    /// another row of the table), or the panel drawn below the AI panel.
    #[test]
    fn the_portrait_panel_tops_the_sidebar_in_portraits_only() {
        for mode in Mode::ALL {
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
                    controls_panel(ctx, &mut app, 320.0);
                }));
            }
            let texts = drawn_texts(&out.unwrap().shapes);
            let at = |key: &str| texts.iter().position(|t| t.starts_with(key));
            let panel = at("People and background");
            assert_eq!(panel.is_some(), mode == Mode::Portraits, "{mode:?}");
            assert_eq!(mode.shell().portrait_panel, mode == Mode::Portraits);
            if mode == Mode::Portraits {
                let ai = at("AI generation & reverse-fit").expect("the full workspace stays");
                assert!(panel.unwrap() < ai, "the panel is drawn first");
                assert!(at("Tone & WB").is_some(), "the develop panel stays too");
            }
        }
    }

    /// No face found: the skin and eyes groups say so and are closed, the
    /// background group still works with the person alone; before any search
    /// they ask for one instead.
    #[test]
    fn without_a_face_the_skin_and_eyes_groups_are_greyed_and_say_why() {
        for lang in [Lang::En, Lang::Zh] {
            let mut app = portrait_app(lang, false);
            let photo = app.src_path.clone().unwrap();
            assert_eq!(app.portrait_face_gate(lang), Some(tr(lang, "Find the people and faces first (the first group).")));
            // A search that found the person and no face.
            app.portrait.found = Some((photo, 0));
            app.recipe.masks.push(autoshade::face::background_adjustment("mask-subject.png".into()));
            let no_face = tr(lang, "No face found in this photo");
            assert_eq!(app.portrait_face_gate(lang), Some(no_face));
            let texts = portrait_panel_texts(&mut app);
            assert_eq!(texts.iter().filter(|t| *t == no_face).count(), 2, "{lang:?}: both groups say it: {texts:?}");
            assert!(
                !texts.iter().any(|t| t == tr(lang, "Find the people and faces first (the first group).")),
                "{lang:?}: the background group works with the person alone"
            );
            assert_eq!(autoshade_subject(&app), Some("mask-subject.png".into()));
            // With a face, the gate opens.
            app.recipe.masks.extend(found_masks());
            assert_eq!(app.portrait_face_gate(lang), None);
        }
    }

    fn autoshade_subject(app: &AutoShadeApp) -> Option<String> {
        crate::panels::portrait::subject_raster(&app.recipe)
    }

    /// Each control writes the field the design names, on the mask with its
    /// role — and nothing else: soften = texture− and clarity− on the skin
    /// mask, eyes brighten = exposure and crisp = sharpness + clarity on the
    /// eyes mask, darken = exposure− and fade = saturation− on the
    /// background mask, keep-the-person-sharp = the blur's protect raster.
    ///
    /// MUTATION THIS CATCHES: any mapping in `panels/portrait.rs` changed (a
    /// share, a sign, a field), or the skin group aimed at the face mask
    /// while a skin mask exists.
    #[test]
    fn every_portrait_control_writes_its_field_on_its_role() {
        use crate::panels::portrait::*;
        let mut app = portrait_app(Lang::En, true);
        let r = &mut app.recipe;
        let skin = skin_target(r).unwrap();
        assert_eq!(skin.role, autoshade::recipe::MaskRole::Skin, "the skin group writes the skin mask");
        set_soften(skin, 40.0);
        assert_eq!((skin.texture, skin.clarity), (-40.0, -20.0));
        assert_eq!(soften_of(skin), 40.0);
        let eyes = role_mask(r, autoshade::recipe::MaskRole::Eyes).unwrap();
        set_crisp(eyes, 30.0);
        assert_eq!((eyes.sharpness, eyes.clarity), (30.0, 15.0));
        assert_eq!(crisp_of(eyes), 30.0);
        let bg = role_mask(r, autoshade::recipe::MaskRole::Background).unwrap();
        set_darken(bg, 50.0);
        assert_eq!(bg.exposure_ev, -1.0, "100 = two stops down");
        assert_eq!(darken_of(bg), 50.0);
        set_fade(bg, 25.0);
        assert_eq!(bg.saturation, -25.0);
        assert_eq!(fade_of(bg), 25.0);
        // Only the four fields moved; the face mask was not touched.
        let face = role_mask(r, autoshade::recipe::MaskRole::Face).unwrap();
        assert_eq!(*face, autoshade::face::portrait_adjustment(autoshade::recipe::MaskRole::Face, "mask-face.png".into()));
        // Without a skin mask (an AI look's face mask alone) the skin group
        // writes the face mask.
        let mut only_face = EditRecipe { masks: vec![autoshade::face::portrait_adjustment(autoshade::recipe::MaskRole::Face, "f.png".into())], ..Default::default() };
        assert_eq!(skin_target(&mut only_face).unwrap().role, autoshade::recipe::MaskRole::Face);
        // Keep the person sharp: the blur protects the background mask's own
        // raster — the subject matte — and off is no protection.
        assert!(app.set_keep_person_sharp(true));
        assert_eq!(app.recipe.lens_blur.as_ref().unwrap().protect.as_deref(), Some("mask-subject.png"));
        assert!(app.set_keep_person_sharp(false));
        assert_eq!(app.recipe.lens_blur.as_ref().unwrap().protect, None);
        let mut alone = portrait_app(Lang::En, false);
        assert!(!alone.set_keep_person_sharp(true), "nothing to protect with before the person is found");
        assert_eq!(alone.recipe.lens_blur.as_ref().unwrap().protect, None);
    }

    /// The find step's landing: the four masks with their roles; a second
    /// search re-points them and keeps their sliders; no face is said as
    /// such; a landing for another photo is dropped.
    #[test]
    fn finding_lands_four_role_masks_and_refreshes_them_in_place() {
        use crate::panels::portrait::{FoundFaces, PortraitFind, PortraitLine};
        let mut app = portrait_app(Lang::En, false);
        let photo = app.src_path.clone().unwrap();
        let before = app.recipe.masks.len();
        let find = |tag: &str, n: usize| PortraitFind {
            photo: photo.clone(),
            subject: Ok(PathBuf::from(format!("mask-subject-{tag}.png"))),
            faces: Ok(Some(FoundFaces {
                n,
                face: PathBuf::from(format!("mask-face-{tag}.png")),
                eyes: PathBuf::from(format!("mask-eyes-{tag}.png")),
                seed: Some(([0.8, 0.6, 0.5], [0.4, 0.3])),
            })),
        };
        app.busy = true;
        app.on_portrait_found(Lang::En, find("a", 2));
        assert!(!app.busy);
        let roles: Vec<autoshade::recipe::MaskRole> = app.recipe.masks[before..].iter().map(|m| m.role).collect();
        assert_eq!(roles, [autoshade::recipe::MaskRole::Face, autoshade::recipe::MaskRole::Eyes, autoshade::recipe::MaskRole::Skin, autoshade::recipe::MaskRole::Background]);
        let bg = &app.recipe.masks[before + 3];
        assert!(bg.inverted, "the background is the person, inverted");
        assert!(app.recipe.masks[before + 2].range.is_some(), "skin = the face narrowed by colour");
        assert_eq!(app.status, "Found 2 faces");
        assert_eq!(app.portrait.found, Some((photo.clone(), 2)));
        // The user softens; a second search keeps that and moves the raster.
        crate::panels::portrait::set_soften(&mut app.recipe.masks[before + 2], 30.0);
        app.on_portrait_found(Lang::En, find("b", 1));
        assert_eq!(app.recipe.masks.len(), before + 4, "refreshed, not stacked");
        let skin = &app.recipe.masks[before + 2];
        assert_eq!(skin.texture, -30.0, "its sliders stay");
        assert_eq!(skin.mask, autoshade::recipe::MaskGeometry::Bitmap { path: "mask-face-b.png".into() });
        assert_eq!(app.status, "Found 1 face");
        // No face: the line says so and the person still lands.
        let mut none = portrait_app(Lang::En, false);
        none.on_portrait_found(Lang::En, PortraitFind { faces: Ok(None), ..find("c", 0) });
        assert_eq!(none.status, "No face found in this photo");
        assert_eq!(none.recipe.masks.iter().filter(|m| m.role != autoshade::recipe::MaskRole::Custom).count(), 1, "the background alone");
        assert_eq!(none.portrait_face_gate(Lang::En), Some("No face found in this photo"));
        // Both halves failed: nothing lands, the line says why.
        let mut failed = portrait_app(Lang::En, false);
        failed.on_portrait_found(
            Lang::En,
            PortraitFind { photo: photo.clone(), subject: Err("no torch".into()), faces: Err("no mediapipe".into()) },
        );
        assert!(matches!(&failed.portrait.line, PortraitLine::Failed(t) if t.contains("no mediapipe")));
        assert_eq!(failed.recipe.masks.len(), before);
        // Another photo is open now: the landing is dropped.
        let mut moved = portrait_app(Lang::En, false);
        moved.src_path = Some(PathBuf::from("D:/library/_another.ARW"));
        moved.on_portrait_found(Lang::En, find("d", 1));
        assert_eq!(moved.recipe.masks.len(), before);
    }

    /// Design §4 in the GUI: with "Keep skin natural" on, a write past the
    /// limits — the user's own and an AI landing's alike — is pulled back
    /// at the frame's one place (`guard_portrait_skin`); off, nothing is
    /// clamped; and outside Portraits the switch does not reach.
    ///
    /// MUTATION THIS CATCHES: drop the `guard_portrait_skin` call from the
    /// frame, ignore the switch, or run it in every mode.
    #[test]
    fn keep_skin_natural_holds_user_and_ai_writes_and_off_holds_none() {
        // ON: a card inside the limits opens with the switch on, and a later
        // write past them is pulled back by a real frame of the app.
        let mut app = portrait_app(Lang::En, true);
        assert!(app.keep_skin_on(), "a card inside the limits opens with the switch on");
        push_skin(&mut app);
        let ctx = egui::Context::default();
        app.mode_chosen_this_launch = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.run_frame(ctx));
        let skin = crate::panels::portrait::skin_target(&mut app.recipe).unwrap().clone();
        assert_eq!((skin.saturation, skin.sharpness, skin.texture), (15.0, 0.0, -60.0), "pulled back to the limits");
        // OFF (the user's flip): the same write stands.
        let mut free = portrait_app(Lang::En, true);
        assert!(free.keep_skin_on());
        free.set_keep_skin(false);
        push_skin(&mut free);
        free.guard_portrait_skin();
        let skin = crate::panels::portrait::skin_target(&mut free.recipe).unwrap();
        assert_eq!((skin.saturation, skin.sharpness, skin.texture), (60.0, 40.0, -90.0), "off clamps nothing");
        // Another mode: the switch is not drawn there, so it does not hold.
        let mut landscape = portrait_app(Lang::En, true);
        landscape.mode = Mode::Landscape;
        push_skin(&mut landscape);
        landscape.guard_portrait_skin();
        assert_eq!(crate::panels::portrait::skin_target(&mut landscape.recipe).unwrap().saturation, 60.0);
    }

    /// Three skin values past the limits on the skin mask.
    fn push_skin(app: &mut AutoShadeApp) {
        let skin = crate::panels::portrait::skin_target(&mut app.recipe).unwrap();
        skin.saturation = 60.0;
        skin.sharpness = 40.0;
        skin.texture = -90.0;
    }

    /// Opening never changes a value: a saved version that already goes past
    /// the skin limits is read through the real `load_version` path and
    /// stays as saved, with the switch OFF for that card only; turning it on
    /// is the explicit act that brings the values back, and the status line
    /// says how many. A version inside the limits reads with the switch on,
    /// and a forked card keeps its parent's switch.
    ///
    /// MUTATION THIS CATCHES: drop `decide_keep_skin_for_card` from
    /// `load_version` (the first frame clamps the saved edit), make the
    /// switch one global value (the other card follows), drop the count from
    /// `set_keep_skin`, or drop `inherit_keep_skin`.
    #[test]
    fn a_saved_edit_beyond_the_limits_opens_unchanged_with_the_switch_off() {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-keep-skin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_keep_skin.jpg");
        std::fs::write(&src, b"jpg").unwrap();
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir, dev]);
        let mut past = portrait_app(Lang::En, true);
        push_skin(&mut past);
        let inside = EditRecipe { masks: found_masks(), ..Default::default() };
        for (n, r) in [(1, &past.recipe), (2, &inside)] {
            std::fs::write(autoshade::store::version_target(&src, n), serde_json::to_string(r).unwrap()).unwrap();
        }
        let mut app = portrait_app(Lang::En, true);
        app.src_path = Some(src.clone());
        app.load_version(1);
        let ctx = egui::Context::default();
        app.mode_chosen_this_launch = true;
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| app.run_frame(ctx));
        }
        let read = |app: &mut AutoShadeApp| {
            let s = crate::panels::portrait::skin_target(&mut app.recipe).unwrap();
            (s.saturation, s.sharpness, s.texture)
        };
        assert_eq!(read(&mut app), (60.0, 40.0, -90.0), "opening changed nothing");
        assert!(!app.keep_skin_on(), "the switch opens off for this card");
        // Per card: the other card has no say from this one.
        let card = app.active;
        app.active = 0;
        assert!(app.keep_skin_on(), "another card is not touched");
        app.active = card;
        // Turning it on is the explicit act: three values come back, said so.
        app.dirty = false;
        app.set_keep_skin(true);
        assert_eq!(read(&mut app), (15.0, 0.0, -60.0));
        assert!(app.dirty, "an edit like any other");
        assert_eq!(app.status, "Skin kept natural: 3 values brought back within the limits");
        assert!(app.keep_skin_on());
        // A fork keeps the switch of the card it came from.
        app.set_keep_skin(false);
        let from = app.variants[app.active].id.clone();
        app.inherit_keep_skin(&from, "forked-card");
        assert_eq!(app.portrait.keep_skin.get(&(src.clone(), "forked-card".to_string())), Some(&false));
        // A version inside the limits reads with the switch on, and the
        // guardrail holds a later write past them.
        app.load_version(2);
        assert!(app.keep_skin_on(), "inside the limits: on");
        push_skin(&mut app);
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.run_frame(ctx));
        assert_eq!(read(&mut app), (15.0, 0.0, -60.0));
        // The photo's own open decides every card of its strip the same way.
        app.portrait.keep_skin.clear();
        app.set_keep_skin(false);
        push_skin(&mut app);
        app.decide_keep_skin_for_photo();
        assert!(!app.keep_skin_on(), "the open card past the limits: off");
        app.active = 0;
        assert!(app.keep_skin_on(), "a card inside them: on");
    }

    /// A stand-in AI portrait look: a face mask pushed past every skin limit,
    /// the way an over-eager model might answer.
    fn portrait_look_runner(
        _photo: &std::path::Path,
        _req: &autoshade::pipeline::GradeRequest,
    ) -> anyhow::Result<(EditRecipe, autoshade::advisor::Verdict, Vec<autoshade::rationale::Note>)> {
        let face = autoshade::recipe::LocalAdjustment {
            saturation: 45.0,
            hue: 80.0,
            temperature: -60.0,
            clarity: -90.0,
            ..autoshade::face::portrait_adjustment(autoshade::recipe::MaskRole::Face, "mask-face-ai.png".into())
        };
        let recipe = EditRecipe { exposure_ev: 0.2, masks: vec![face], ..Default::default() };
        let verdict =
            autoshade::advisor::Verdict { decision: autoshade::advisor::Decision::Accept, reasons: Vec::new(), revised_hint: None };
        Ok((recipe, verdict, Vec::new()))
    }

    /// ✨ AI portrait look: the portrait direction goes out (the preset's text,
    /// the look strength, the Normal presets' request), the landing keeps the
    /// photo's found masks that the look has no mask for — with their sliders
    /// at rest — and its lens blur, and the guardrail holds the look's face
    /// mask at the same place it holds the user's sliders.
    #[test]
    fn the_ai_portrait_look_asks_for_the_portrait_direction_and_lands_under_the_guardrail() {
        let (mut app, _src, _scrub) = normal_photo("portrait-look");
        app.choose_mode(Mode::Portraits);
        app.normal.runner = Some(portrait_look_runner);
        let mut skin = autoshade::face::skin_adjustment("mask-face.png".into(), [0.8, 0.6, 0.5], [0.4, 0.3]);
        skin.texture = -25.0;
        app.recipe.masks.push(skin);
        app.recipe.lens_blur = Some(autoshade::render::LensBlur { depth_map: "depth.png".into(), amount: 70.0, ..Default::default() });
        app.grade_strength = 0.4;
        app.start_portrait_look();
        assert!(app.busy && app.analyze_inflight);
        let (req, guidance, library) = app.normal.last_request.clone().expect("the analysis was asked");
        assert_eq!(guidance.as_deref(), Some(autoshade::advisor::presets::PORTRAIT.direction));
        assert_eq!(req.guide, autoshade::pipeline::Guide::Direction);
        assert_eq!(req.strength.get(), 0.4, "the look strength");
        assert_eq!(library, autoshade::library::DEFAULT_NAME);
        let ctx = egui::Context::default();
        land_analysis(&mut app, &ctx);
        assert!(!app.busy);
        assert_eq!(app.recipe.exposure_ev, 0.2, "the look landed");
        let roles: Vec<autoshade::recipe::MaskRole> = app.recipe.masks.iter().map(|m| m.role).collect();
        assert_eq!(roles, [autoshade::recipe::MaskRole::Face, autoshade::recipe::MaskRole::Skin], "the look's face mask, and the photo's skin mask back");
        let back = &app.recipe.masks[1];
        assert_eq!((back.texture, back.mask.clone()), (0.0, autoshade::recipe::MaskGeometry::Bitmap { path: "mask-face.png".into() }));
        assert!(back.range.is_some(), "its colour range comes back with it");
        assert_eq!(app.recipe.lens_blur.as_ref().map(|l| l.amount), Some(70.0), "the blur stays");
        assert_eq!(app.portrait.line, crate::panels::portrait::PortraitLine::Looked);
        app.guard_portrait_skin();
        let face = &app.recipe.masks[0];
        let hue = autoshade::face::skin_hue_max();
        assert_eq!(
            (face.saturation, face.hue, face.temperature, face.clarity),
            (15.0, hue, -20.0, -40.0),
            "the AI's face mask meets the user's limits"
        );
    }
