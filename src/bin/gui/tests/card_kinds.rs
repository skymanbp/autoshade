// One part of the GUI's tests (src/bin/gui/tests.rs includes it): a card's
// kind across a reopen — written once when the card is made, restored
// verbatim, never re-derived — and the repair of the strips that lost it.

    /// A photo saved with a two-card strip whose ACTIVE card is `active_kind`
    /// (id `card-b`, beside the ▣ card `original` with a develop of its own),
    /// a neutral recipe.json and, when `master` names one, a pixel record
    /// `(file name, generated, on disk)` for a master beside the fake RAW. The
    /// store keys by path and nothing reads the RAW. Returns the app, the RAW,
    /// the master path (absolute — the record's spelling) and the scrub guard.
    fn saved_strip_fixture(
        tag: &str,
        active_kind: &str,
        master: Option<(&str, bool, bool)>,
    ) -> (AutoShadeApp, PathBuf, Option<PathBuf>, Scrub) {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-card-kind-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join(format!("_gui_card_kind_{tag}.ARW"));
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let scrub = Scrub(vec![dir.clone(), dev.clone()]);
        std::fs::write(autoshade::store::recipe_target(&src), serde_json::to_string(&EditRecipe::default()).unwrap()).unwrap();
        let master = master.map(|(file, generated, on_disk)| {
            let path = std::path::absolute(dir.join(format!("_gui_card_kind_{tag}.{file}"))).unwrap();
            if on_disk {
                image::DynamicImage::new_rgb8(6, 4).save(&path).unwrap();
            }
            autoshade::store::write_pixel_source(&src, &path, generated).unwrap();
            path
        });
        if !active_kind.is_empty() {
            autoshade::store::write_variants(
                &src,
                &autoshade::store::VariantsRecord {
                    extra: Default::default(),
                    v: 1,
                    active_kind: active_kind.into(),
                    active_pos: 1,
                    active_id: Some("card-b".into()),
                    active_name: None,
                    others: vec![autoshade::store::VariantEntry {
                        extra: Default::default(),
                        kind: "original".into(),
                        recipe: EditRecipe { contrast: 4.0, ..Default::default() },
                        origin: None,
                        id: Some(ORIGINAL_VARIANT_ID.into()),
                        name: None,
                    }],
                },
            )
            .unwrap();
        }
        let app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        (app, src, master, scrub)
    }

    /// The `Msg::Opened` the open worker sends for `src`, landed through the
    /// real door. The baked master is what the worker itself would bring:
    /// `read_pixel_source` answers, and a recorded master that is not on disk
    /// brings nothing.
    fn land_cold_open(app: &mut AutoShadeApp, ctx: &egui::Context, src: &std::path::Path) {
        let baked = autoshade::store::read_pixel_source(src)
            .map(|(origin, generated)| (Arc::new(image::DynamicImage::new_rgb8(6, 4)), origin, generated));
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                Arc::new(image::DynamicImage::new_rgb8(6, 4)),
                Default::default(),
                Default::default(),
                None,
                baked,
                (1280, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(ctx);
    }

    fn original_kind_cards(app: &AutoShadeApp) -> usize {
        app.variants.iter().filter(|v| v.kind == VariantKind::Original).count()
    }

    /// Part 1, the cold half: a ◈ card saved as the ACTIVE card comes back as
    /// ◈ — still carrying its master — not as a second 「▣ Original」. The
    /// landing used to honour the record's word for two kinds only (fitted,
    /// edited), so every ◈ reopen relabelled the card and the next Ctrl+S
    /// wrote `"original"` under the card's minted id.
    ///
    /// MUTATION: the old two-word special case (`if rec.active_kind ==
    /// "fitted" … else if … "edited"`) in place of the verbatim restore.
    #[test]
    fn a_denoised_card_reopens_as_denoised_with_its_master() {
        let (mut app, src, master, _scrub) =
            saved_strip_fixture("dn-cold", "denoised", Some(("denoise-2.png", false, true)));
        let master = master.unwrap();
        let ctx = egui::Context::default();
        land_cold_open(&mut app, &ctx, &src);
        assert_eq!(strip_kinds(&app), vec![VariantKind::Original, VariantKind::Denoised], "{}", app.status);
        assert_eq!(app.active, 1, "the ◈ card is under the canvas");
        let card = &app.variants[1];
        assert_eq!(card.id, "card-b", "…with its own identity");
        assert_eq!(card.origin.as_deref(), Some(master.as_path()), "…and its master");
        assert!(card.base.is_some(), "…whose pixels it renders");
        assert!(!app.unsaved_marker_dirty(), "a clean reopen is clean");
        // Restored, not repaired: the door's repair of lost words would turn
        // a second ▣ card on a `denoise` master back into ◈ as well, so the
        // card's kind alone cannot tell the two apart — the repair's toast
        // can, and a healthy record needs no repair.
        assert_eq!(repaired_toasts(&app), 0, "a healthy record is restored, not repaired");
    }

    /// A card's kind, its id and its master through the session stash:
    /// navigate away with an unsaved edit on a `kind` card, then come back.
    /// The stash held the right kind all along; the landing's constructor
    /// threw it away for every kind but ◭ and a still-decoding master.
    fn kind_survives_the_stash(kind: VariantKind, tag: &str) {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-card-kind-stash-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join(format!("_gui_card_kind_stash_{tag}_a.ARW"));
        let b = dir.join(format!("_gui_card_kind_stash_{tag}_b.ARW"));
        let _scrub = Scrub(vec![dir.clone(), autoshade::store::develop_dir(&a), autoshade::store::develop_dir(&b)]);
        let master = dir.join(format!("_gui_card_kind_stash_{tag}_a.master.png"));
        let pixels = Arc::new(image::DynamicImage::new_rgb8(6, 4));
        let edit = EditRecipe { contrast: 9.0, ..Default::default() };
        let mut app = AutoShadeApp {
            src_path: Some(a.clone()),
            variants: vec![
                Variant {
                    kind: VariantKind::Original,
                    id: ORIGINAL_VARIANT_ID.into(),
                    name: None,
                    recipe: EditRecipe::default(),
                    base: None,
                    origin: None,
                    thumb: None,
                },
                Variant {
                    kind,
                    id: "card-b".into(),
                    name: Some("clean".into()),
                    recipe: edit.clone(),
                    base: Some(pixels.clone()),
                    origin: Some(master.clone()),
                    thumb: None,
                },
            ],
            active: 1,
            recipe: edit.clone(),
            ..Default::default()
        };
        assert!(app.nav_stash_gate_dirty(), "premise: the edit is unsaved work");
        let ctx = egui::Context::default();
        app.open_path(b.clone());
        assert!(
            app.nav_stash.get(&a).is_some_and(|st| st.kind == kind),
            "premise: the stash holds the card's kind"
        );
        // B's own open (a file that is not there) lands first.
        let landed = app.rx.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(60)).expect("B's open lands");
        app.tx.send(landed).unwrap();
        app.poll_workers(&ctx);
        // Back on A: A has nothing on disk, so the stash is the only authority.
        app.src_path = Some(a.clone());
        land_cold_open(&mut app, &ctx, &a);
        assert_eq!(strip_kinds(&app), vec![VariantKind::Original, kind], "{tag}: {}", app.status);
        assert_eq!(app.active, 1, "{tag}: the card is under the canvas again");
        let card = &app.variants[1];
        assert_eq!(card.id, "card-b", "{tag}: same identity");
        assert_eq!(card.name.as_deref(), Some("clean"), "{tag}: same name");
        assert_eq!(card.origin.as_deref(), Some(master.as_path()), "{tag}: same master");
        assert!(card.base.as_ref().is_some_and(|p| Arc::ptr_eq(p, &pixels)), "{tag}: same pixels");
        assert_eq!(app.recipe.contrast, 9.0, "{tag}: the unsaved edit came back with it");
    }

    /// Part 1 for ▦, the stash half: navigate away with an unsaved edit on
    /// the ▦ card and back — still ▦, same id, same origin. (The cold half is
    /// `a_stacked_card_reopens_as_stacked_with_its_master`.)
    ///
    /// MUTATION: the old three-way constructor (`Fitted` or `Original`
    /// unless a master is still decoding).
    #[test]
    fn a_stacked_card_keeps_its_kind_across_a_navigation() {
        kind_survives_the_stash(VariantKind::Stacked, "stacked");
    }

    /// Part 1, the stash half for ◈: navigate away with an unsaved edit on
    /// the ◈ card and back — still ◈, same id, same origin.
    ///
    /// MUTATION: the old three-way constructor.
    #[test]
    fn a_denoised_card_keeps_its_kind_across_a_navigation() {
        kind_survives_the_stash(VariantKind::Denoised, "denoised");
    }

    /// Part 1, a master that could not be loaded: the card KEEPS its recorded
    /// kind and its recorded origin, as a background card of the strip
    /// already does (`load_active` + `spawn_master_load`), instead of coming
    /// back as a second 「▣ Original」 with no origin. For a ◈ card (an
    /// in-place master) and a ✨ card (a generated one).
    ///
    /// MUTATION: dropping the recorded origin (`baked` alone) — or the old
    /// constructor, which named both cards ▣.
    #[test]
    fn a_card_whose_master_is_missing_keeps_its_kind_and_its_master_link() {
        for (word, kind, generated) in [("denoised", VariantKind::Denoised, false), ("generated", VariantKind::Generated, true)] {
            let (mut app, src, master, _scrub) =
                saved_strip_fixture(&format!("gone-{word}"), word, Some(("gone.png", generated, false)));
            let master = master.unwrap();
            assert!(!master.exists(), "premise: the master is gone");
            let ctx = egui::Context::default();
            land_cold_open(&mut app, &ctx, &src);
            assert_eq!(strip_kinds(&app), vec![VariantKind::Original, kind], "{word}: {}", app.status);
            assert_eq!(original_kind_cards(&app), 1, "{word}: exactly one ▣ card");
            let card = &app.variants[app.active];
            assert_eq!(card.id, "card-b", "{word}");
            assert_eq!(card.origin.as_deref(), Some(master.as_path()), "{word}: the recorded master link stays");
            assert!(card.base.is_none(), "{word}: …with no pixels behind it");
            assert!(!app.unsaved_marker_dirty(), "{word}: nothing was changed, so nothing is unsaved");
            assert!(
                app.toasts.iter().any(|t| t.kind == ToastKind::Error && t.text.contains("could not be loaded")),
                "{word}: the open says so: {:?}",
                app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
            );
        }
    }

    /// Part 1, the savers: Ctrl+S over a card whose master is missing leaves
    /// the pixel link exactly as it is on disk and the record's word as it
    /// was — the save writes what the card says, and the card says what was
    /// recorded.
    ///
    /// MUTATION: a saver that drops the link of a card with no pixels
    /// (`CommitMember::Clear` when `base` is `None`).
    #[test]
    fn ctrl_s_over_a_card_whose_master_is_missing_keeps_the_link_byte_for_byte() {
        for (word, generated) in [("denoised", false), ("generated", true)] {
            let (mut app, src, _, _scrub) =
                saved_strip_fixture(&format!("gone-save-{word}"), word, Some(("gone.png", generated, false)));
            let link = autoshade::store::pixel_source_path(&src);
            let before = std::fs::read(&link).expect("premise: a pixel record");
            let ctx = egui::Context::default();
            land_cold_open(&mut app, &ctx, &src);
            // An edit on the ◈ card, so the save writes a develop. Not on the ✨
            // card: an edit there continues on a new ✎ card (the immutability
            // rule), and its master link alone already takes the save path.
            if !generated {
                app.recipe.contrast = 5.0;
            }
            app.save_xmp();
            assert!(app.status.contains("saved"), "{word}: {}", app.status);
            assert_eq!(std::fs::read(&link).ok(), Some(before), "{word}: the pixel record is byte-identical");
            assert_eq!(saved_strip_of(&src).active_kind, word, "{word}: the record names the same kind");
            assert_eq!(saved_strip_of(&src).active_id.as_deref(), Some("card-b"), "{word}: …for the same card");
        }
    }

    /// Part 1, the one case where the kind IS derived: a develop with no
    /// strip record at all has no word, and the one fact that exists is the
    /// pixel record's `generated` flag — today's behaviour, pinned.
    #[test]
    fn a_develop_with_no_strip_record_takes_its_kind_from_the_pixel_record() {
        for (generated, kind) in [(true, VariantKind::Generated), (false, VariantKind::Original)] {
            let tag = if generated { "no-record-gen" } else { "no-record-inplace" };
            let (mut app, src, master, _scrub) = saved_strip_fixture(tag, "", Some(("master.png", generated, true)));
            let ctx = egui::Context::default();
            land_cold_open(&mut app, &ctx, &src);
            assert_eq!(strip_kinds(&app), vec![kind], "{tag}");
            assert_eq!(app.variants[0].id, ORIGINAL_VARIANT_ID, "{tag}: the fixed identity");
            assert_eq!(app.variants[0].origin, master, "{tag}: on its master");
        }
    }

    /// A photo saved with exactly `rec` as its strip record and a neutral
    /// recipe.json — no pixel record. Returns the app, the RAW and the
    /// scrub guard.
    fn photo_with_strip_record(tag: &str, rec: autoshade::store::VariantsRecord) -> (AutoShadeApp, PathBuf, Scrub) {
        let (app, src, _, scrub) = saved_strip_fixture(tag, "", None);
        autoshade::store::write_variants(&src, &rec).unwrap();
        (app, src, scrub)
    }

    /// One strip-record entry of kind `kind` with this id, develop and name.
    fn entry(kind: &str, id: &str, recipe: EditRecipe, name: Option<&str>) -> autoshade::store::VariantEntry {
        autoshade::store::VariantEntry {
            extra: Default::default(),
            kind: kind.into(),
            recipe,
            origin: None,
            id: Some(id.into()),
            name: name.map(Into::into),
        }
    }

    fn repaired_toasts(app: &AutoShadeApp) -> usize {
        app.toasts.iter().filter(|t| t.text.contains("variant list was repaired")).count()
    }

    /// Part 2, THE USER'S CASE: the bug already wrote a second Original card
    /// — `active: original (id "original")`, `others: [original (minted id,
    /// neutral, no master, no name)]` — and that card could not be deleted,
    /// kept the strip non-trivial for ever, and so kept Ctrl+S from ever
    /// clearing the photo's develop. At the door it is an empty duplicate:
    /// removed, said once; and Ctrl+S then takes the clear branch.
    ///
    /// MUTATION: skipping rule (b) (no removal) — two cards remain, the strip
    /// is not trivial and the develop survives the save.
    #[test]
    fn an_empty_second_original_is_removed_and_ctrl_s_clears_the_photo_again() {
        let (mut app, src, _scrub) = photo_with_strip_record(
            "user-case",
            autoshade::store::VariantsRecord {
                extra: Default::default(),
                v: 1,
                active_kind: "original".into(),
                active_pos: 0,
                active_id: Some(ORIGINAL_VARIANT_ID.into()),
                active_name: None,
                others: vec![entry("original", "4f2a-18b7c-3", EditRecipe::default(), None)],
            },
        );
        let ctx = egui::Context::default();
        land_cold_open(&mut app, &ctx, &src);
        assert_eq!(strip_kinds(&app), vec![VariantKind::Original], "{}", app.status);
        assert_eq!(app.active, 0);
        assert_eq!(app.variants[0].id, ORIGINAL_VARIANT_ID, "the base stays");
        assert!(app.current_strip_record().is_none(), "the strip is trivial again");
        assert_eq!(repaired_toasts(&app), 1, "said once: {:?}", app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
        app.save_xmp();
        assert!(!autoshade::store::has_develop(&src), "Ctrl+S clears the neutral photo: {}", app.status);
    }

    /// Part 2 (a): an extra Original-kind card whose master's own file name
    /// says what made it gets its word back — id, name, develop and master
    /// untouched. A master named by nothing this app knows is left alone.
    #[test]
    fn a_lost_word_comes_back_from_the_masters_file_name() {
        let card = |kind, id: &str, origin: Option<&str>, name: Option<&str>, contrast: f32| Variant {
            kind,
            id: id.into(),
            name: name.map(Into::into),
            recipe: EditRecipe { contrast, ..Default::default() },
            base: None,
            origin: origin.map(PathBuf::from),
            thumb: None,
        };
        let mut app = AutoShadeApp {
            variants: vec![
                card(VariantKind::Original, ORIGINAL_VARIANT_ID, None, None, 0.0),
                card(VariantKind::Original, "dn", Some("masters/_p.ARW.denoise-2.png"), Some("denoised"), 3.0),
                card(VariantKind::Original, "st", Some("masters/_p.ARW.stack.png"), None, 5.0),
                card(VariantKind::Original, "hl", Some("masters/_p.ARW.heal.png"), None, 6.0),
            ],
            ..Default::default()
        };
        let repair = app.repair_lost_card_kinds();
        assert_eq!(repair.restored, vec![VariantKind::Denoised, VariantKind::Stacked]);
        assert_eq!(repair.removed, 0);
        assert_eq!(
            strip_kinds(&app),
            vec![VariantKind::Original, VariantKind::Denoised, VariantKind::Stacked, VariantKind::Original]
        );
        let dn = &app.variants[1];
        assert_eq!((dn.id.as_str(), dn.name.as_deref(), dn.recipe.contrast), ("dn", Some("denoised"), 3.0));
        assert_eq!(dn.origin.as_deref(), Some(std::path::Path::new("masters/_p.ARW.denoise-2.png")));
        assert_eq!(app.variants[2].recipe.contrast, 5.0, "the ▦ card keeps its develop");
        // The reader's edges: the tag is the last one, a number after a dash
        // only, and the writers name their masters with the same words.
        let named = |s: &str| crate::model::kind_named_by_master(std::path::Path::new(s));
        assert_eq!(named("_p.ARW.stack.denoise.png"), Some(VariantKind::Denoised));
        assert_eq!(named("_p.ARW.denoise-x.png"), None);
        assert_eq!(named("_p.ARW.denoise.tif"), None);
        let retouch = include_str!("../panels/retouch.rs");
        assert!(retouch.contains("unique_out(&path, autoshade::store::DENOISE_MASTER_TAG)"), "the ◈ writer names its own tag");
        assert!(retouch.contains("unique_out(&path, autoshade::store::STACK_MASTER_TAG)"), "the ▦ writer names its own tag");
        let serve = include_str!("../../../serve.rs");
        assert!(serve.contains("unique_out(&raws[0], crate::store::STACK_MASTER_TAG)"), "the web UI's ▦ writer names the same tag");
    }

    /// Part 2 (c): an extra Original-kind card with edits of its own and
    /// nothing that says what it was stays — and the strip OFFERS its ✕,
    /// while the base negative's ✕ is still not offered. The guard asks
    /// "is this the base?", through the one owner of that choice.
    ///
    /// MUTATION: the old guard (`kind != VariantKind::Original`) — the extra
    /// card is undeletable again.
    #[test]
    fn an_extra_original_with_edits_stays_and_can_be_deleted() {
        let mut app = AutoShadeApp {
            variants: vec![
                Variant {
                    kind: VariantKind::Original,
                    id: ORIGINAL_VARIANT_ID.into(),
                    name: None,
                    recipe: EditRecipe::default(),
                    base: None,
                    origin: None,
                    thumb: None,
                },
                Variant {
                    kind: VariantKind::Original,
                    id: "4f2a-18b7c-9".into(),
                    name: None,
                    recipe: EditRecipe { contrast: 7.0, ..Default::default() },
                    base: None,
                    origin: None,
                    thumb: None,
                },
            ],
            active: 1,
            ..Default::default()
        };
        assert_eq!(app.repair_lost_card_kinds(), LostKindRepair::default(), "nothing says what it was");
        assert_eq!(app.variants.len(), 2, "…so it stays");
        assert_eq!(app.original_index(), Some(0), "the base is the card with the fixed id");
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))),
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::bottom("variants")
                    .exact_height(AutoShadeApp::VARIANT_STRIP_H)
                    .show(ctx, |ui| app.variant_strip(ui));
            },
        );
        assert_eq!(
            app.strip_card_actions,
            vec!["0:", "1:＋▣✕"],
            "the base offers no ✕; the extra card offers ✕ (and can be applied onto the base)"
        );
    }

    /// Part 2 (b) when the empty duplicate is the ACTIVE card and stands
    /// BEFORE the base: it goes, the base is active, and the canvas shows the
    /// base's develop, not the removed card's.
    #[test]
    fn an_active_empty_duplicate_before_the_base_gives_the_canvas_to_the_base() {
        let (mut app, src, _scrub) = photo_with_strip_record(
            "dup-first",
            autoshade::store::VariantsRecord {
                extra: Default::default(),
                v: 1,
                active_kind: "original".into(),
                active_pos: 0,
                active_id: Some("4f2a-18b7c-5".into()),
                active_name: None,
                others: vec![entry("original", ORIGINAL_VARIANT_ID, EditRecipe { contrast: 4.0, ..Default::default() }, None)],
            },
        );
        let ctx = egui::Context::default();
        land_cold_open(&mut app, &ctx, &src);
        assert_eq!(strip_kinds(&app), vec![VariantKind::Original]);
        assert_eq!(app.variants[0].id, ORIGINAL_VARIANT_ID);
        assert_eq!(app.active, 0);
        assert_eq!(app.recipe.contrast, 4.0, "the canvas is the base's develop");
        assert_eq!(repaired_toasts(&app), 1);
    }

    /// Part 2, idempotence and restraint: a second pass changes nothing, and
    /// a healthy strip is never touched (so its open says nothing).
    #[test]
    fn the_card_repair_is_idempotent_and_leaves_healthy_strips_alone() {
        let card = |kind, id: &str, origin: Option<&str>, contrast: f32| Variant {
            kind,
            id: id.into(),
            name: None,
            recipe: EditRecipe { contrast, ..Default::default() },
            base: None,
            origin: origin.map(PathBuf::from),
            thumb: None,
        };
        let snapshot = |app: &AutoShadeApp| -> Vec<(VariantKind, String)> {
            app.variants.iter().map(|v| (v.kind, v.id.clone())).collect()
        };
        let mut app = AutoShadeApp {
            variants: vec![
                card(VariantKind::Original, "dup", None, 0.0),
                card(VariantKind::Original, ORIGINAL_VARIANT_ID, None, 2.0),
                card(VariantKind::Original, "dn", Some("m/_p.denoise.png"), 0.0),
                card(VariantKind::Original, "kept", None, 9.0),
            ],
            active: 3,
            ..Default::default()
        };
        let first = app.repair_lost_card_kinds();
        assert_eq!((first.removed, first.restored.clone()), (1, vec![VariantKind::Denoised]));
        assert_eq!(app.active, 2, "the active card moved with the removal before it");
        let after = snapshot(&app);
        assert_eq!(app.repair_lost_card_kinds(), LostKindRepair::default(), "the second pass is a no-op");
        assert_eq!(snapshot(&app), after);
        for healthy in [
            vec![card(VariantKind::Original, ORIGINAL_VARIANT_ID, None, 0.0)],
            vec![
                card(VariantKind::Original, ORIGINAL_VARIANT_ID, None, 0.0),
                card(VariantKind::Generated, "g", Some("m/_p.reimagine.png"), 0.0),
                card(VariantKind::Edited, "e", Some("m/_p.reimagine.png"), 3.0),
            ],
            vec![
                card(VariantKind::Original, ORIGINAL_VARIANT_ID, None, 0.0),
                card(VariantKind::Denoised, "d", Some("m/_p.denoise.png"), 0.0),
            ],
        ] {
            let mut app = AutoShadeApp { variants: healthy, ..Default::default() };
            let before = snapshot(&app);
            assert_eq!(app.repair_lost_card_kinds(), LostKindRepair::default(), "{before:?}");
            assert_eq!(snapshot(&app), before);
        }
    }

    // ---- Follow-up 1: one vocabulary for the card kinds, and the ▦ record.

    /// The card-kind words have ONE home, the store's `CARD_KIND_WORDS`, and
    /// the GUI's kinds and that list agree in both directions: every kind's
    /// `store_str` is a word the store reads back, and every word the store
    /// reads back names a kind. The list is walked through an exhaustive
    /// `match`, so a seventh kind does not compile until it is listed here.
    /// Until 2026-11-18 the store kept its own copy, which never learned
    /// `"stacked"`.
    ///
    /// MUTATION: `"stacked"` removed from `CARD_KIND_WORDS` again.
    #[test]
    fn the_gui_and_the_store_share_one_card_kind_vocabulary() {
        let every_kind = |k: VariantKind| match k {
            VariantKind::Original
            | VariantKind::Generated
            | VariantKind::Fitted
            | VariantKind::Edited
            | VariantKind::Denoised
            | VariantKind::Stacked => k,
        };
        let kinds = [
            VariantKind::Original,
            VariantKind::Generated,
            VariantKind::Fitted,
            VariantKind::Edited,
            VariantKind::Denoised,
            VariantKind::Stacked,
        ]
        .map(every_kind);
        for kind in kinds {
            assert!(
                autoshade::store::CARD_KIND_WORDS.contains(&kind.store_str()),
                "{kind:?} is saved as {:?}, a word the store refuses to read back",
                kind.store_str()
            );
        }
        for word in autoshade::store::CARD_KIND_WORDS {
            assert!(VariantKind::from_store_str(word).is_some(), "the store reads {word:?}, which names no kind");
        }
        assert_eq!(autoshade::store::CARD_KIND_WORDS.len(), kinds.len(), "one word per kind");
    }

    /// Part 1 for ▦, the cold half: a ▦ card saved as the ACTIVE card comes
    /// back as ▦ on its master.
    #[test]
    fn a_stacked_card_reopens_as_stacked_with_its_master() {
        let (mut app, src, master, _scrub) =
            saved_strip_fixture("st-cold", "stacked", Some(("stack.png", false, true)));
        let ctx = egui::Context::default();
        land_cold_open(&mut app, &ctx, &src);
        assert_eq!(strip_kinds(&app), vec![VariantKind::Original, VariantKind::Stacked], "{}", app.status);
        assert_eq!(app.active, 1);
        assert_eq!(app.variants[1].origin, master, "…on its master");
        assert!(app.variants[1].base.is_some());
        assert_eq!(repaired_toasts(&app), 0, "restored, not repaired");
    }

    /// The case broken on users' disks until 2026-11-18: a strip whose ▦
    /// card is a BACKGROUND card (the ▣ card active) reopens whole. The
    /// store did not know the word `"stacked"`, so the record read back as
    /// unresolved: the ▦ card stayed hidden and every save was refused.
    #[test]
    fn a_strip_with_a_background_stacked_card_reopens_whole() {
        let master = std::env::temp_dir().join(format!("_gui_card_kind_bg_stack_{}.stack.png", std::process::id()));
        let (mut app, src, _scrub) = photo_with_strip_record(
            "bg-stack",
            autoshade::store::VariantsRecord {
                extra: Default::default(),
                v: 1,
                active_kind: "original".into(),
                active_pos: 0,
                active_id: Some(ORIGINAL_VARIANT_ID.into()),
                active_name: None,
                others: vec![autoshade::store::VariantEntry {
                    origin: Some(master.clone()),
                    ..entry("stacked", "st-1", EditRecipe { contrast: 6.0, ..Default::default() }, Some("bracket"))
                }],
            },
        );
        let ctx = egui::Context::default();
        land_cold_open(&mut app, &ctx, &src);
        assert!(
            !app.toasts.iter().any(|t| t.text.contains("cannot be read")),
            "the strip record reads back: {:?}",
            app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
        );
        assert_eq!(strip_kinds(&app), vec![VariantKind::Original, VariantKind::Stacked], "{}", app.status);
        let card = &app.variants[1];
        assert_eq!((card.id.as_str(), card.name.as_deref(), card.recipe.contrast), ("st-1", Some("bracket"), 6.0));
        assert_eq!(card.origin.as_deref(), Some(master.as_path()));
    }

    /// Follow-up 1 item 5, widened by follow-up 2: a recorded word for a kind
    /// that OWNS an image (✨ / ✎ an AI raster, ◈ a remade negative) with NO
    /// pixel record at all — nothing was ever recorded, so nothing failed to
    /// load — keeps its kind (the word is the authority), and the open says
    /// once, plainly, that the variant's own image is missing and the photo
    /// is shown instead.
    ///
    /// MUTATIONS: the toast's condition dropped (no toast); the condition
    /// narrowed back to `on_ai_pixels()` (the ◈ case says nothing).
    #[test]
    fn a_card_that_owns_an_image_with_no_pixel_record_keeps_its_kind_and_says_so() {
        for (word, kind) in [
            ("generated", VariantKind::Generated),
            ("edited", VariantKind::Edited),
            ("denoised", VariantKind::Denoised),
        ] {
            let (mut app, src, _, _scrub) = saved_strip_fixture(&format!("no-px-{word}"), word, None);
            assert!(!autoshade::store::has_pixel_source(&src), "premise: no pixel record at all");
            let ctx = egui::Context::default();
            land_cold_open(&mut app, &ctx, &src);
            assert_eq!(strip_kinds(&app), vec![VariantKind::Original, kind], "{word}: the word is kept");
            let said: Vec<&String> = app
                .toasts
                .iter()
                .filter(|t| t.kind == ToastKind::Error && t.text.contains("own image is missing"))
                .map(|t| &t.text)
                .collect();
            assert_eq!(said.len(), 1, "{word}: said once: {:?}", app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
        }
        // A word for a kind that owns no image (◭ develops the negative) with
        // no pixel record is the ordinary case and says nothing.
        let (mut app, src, _, _scrub) = saved_strip_fixture("no-px-fitted", "fitted", None);
        land_cold_open(&mut app, &egui::Context::default(), &src);
        assert!(!app.toasts.iter().any(|t| t.text.contains("own image is missing")));
    }

    /// The desktop app's test binary never resolves the user's real develop
    /// store (2026-11-18). It links the library's NORMAL build, so a
    /// `#[cfg(test)]` default inside the library never reached it, and every
    /// fixture here that calls `autoshade::store::develop_dir` before any app
    /// exists wrote into the user's store whenever `AUTOSHADE_DATA_DIR` was
    /// unset. The store's one rule covers it: a test executable — any program
    /// cargo starts from a `deps` folder — uses the `test-store` folder beside
    /// that folder. With the variable set, the variable wins.
    ///
    /// The behaviour bites only in a run with the variable UNSET; a run that
    /// sets it cannot reach the step, so the step's place in the order is
    /// pinned in the store's source too.
    ///
    /// MUTATION: the `deps` step removed from `store_root_with_trust`'s order.
    #[test]
    fn the_gui_test_binary_never_resolves_the_per_user_store() {
        let root_rs = include_str!("../../../store/root.rs");
        let body = root_rs.split("pub fn store_root_with_trust()").nth(1).expect("store_root_with_trust is gone");
        let head = body.split("per_user_data_dir()").next().unwrap();
        assert!(
            head.contains(".or_else(|| test_executable_root().map(|d| (d, RootTrust::PerUser)))"),
            "the test-executable step no longer comes before the per-user root"
        );
        let exe = std::env::current_exe().unwrap();
        assert_eq!(exe.parent().and_then(|d| d.file_name()).and_then(|n| n.to_str()), Some("deps"), "premise: cargo runs tests from deps");
        let expected = std::path::absolute(exe.parent().unwrap().parent().unwrap().join("test-store")).unwrap();
        match std::env::var_os("AUTOSHADE_DATA_DIR") {
            None => {
                let root = autoshade::store::store_root();
                assert_eq!(root, expected, "an unset variable resolves the test store");
                if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                    let local = PathBuf::from(local);
                    for name in ["AutoShade", "autoshade", "autoshop"] {
                        assert!(!root.starts_with(local.join(name)), "{} is the user's store", root.display());
                    }
                }
            }
            Some(explicit) => {
                let explicit = PathBuf::from(explicit);
                assert_eq!(autoshade::store::store_root(), std::path::absolute(&explicit).unwrap_or(explicit), "the explicit variable wins");
            }
        }
    }
