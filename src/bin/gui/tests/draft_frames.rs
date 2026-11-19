// One part of the GUI's tests (src/bin/gui/tests.rs includes it): draft frames — what the canvas shows while a slow photo is being edited (draft.rs).

    /// A photo large enough to draft — its long edge is twice the draft floor
    /// — on a real (tiny) file, so its cards have a path and a source frame,
    /// with the source's film edge stated: the canvas base is the reduced
    /// preview of a larger negative, as it is on a real photo, so the Detail
    /// passes of the recipe (Sharpening 60) depend on the film edge. Two
    /// cards share the base, as two cards on one source do. The scheduler's
    /// clock is the tests' own (`*_at`): nothing here waits for wall time.
    fn draft_fixture(tag: &str) -> (AutoShadeApp, Scrub) {
        let (w, h) = (2048u32, 1366u32);
        let base = Arc::new(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let fine = ((x * 7 + y * 13) % 32) as u8;
            image::Rgb([(x * 200 / w) as u8 + fine, (y * 200 / h) as u8 + fine, 96 + fine])
        })));
        std::fs::create_dir_all("out").ok();
        let photo = PathBuf::from(format!("out/_gui_draft_{tag}.png"));
        image::RgbImage::new(8, 8).save(&photo).unwrap();
        let modified = std::fs::metadata(&photo).and_then(|m| m.modified()).ok();
        let card = |id: &str| Variant {
            id: id.into(),
            name: None,
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        let app = AutoShadeApp {
            src_path: Some(photo.clone()),
            film_edge: Some((photo.clone(), modified, Some(4096))),
            source_preview: Some(base.clone()),
            base_preview: Some(base),
            variants: vec![card("a"), card("b")],
            recipe: EditRecipe { exposure_ev: 0.3, sharpening: 60.0, ..Default::default() },
            ..AutoShadeApp::default()
        };
        (app, Scrub(vec![photo]))
    }

    /// The next worker result, waited for (one develop of the fixture).
    fn next_develop(app: &AutoShadeApp) -> Msg {
        app.rx.as_ref().unwrap().recv_timeout(Duration::from_secs(300)).expect("a develop was dispatched and lands")
    }

    /// The draft the scheduler dispatched, as it lands.
    fn next_draft(app: &AutoShadeApp) -> DraftDone {
        match next_develop(app) {
            Msg::DraftDeveloped(done) => (*done).expect("the draft develops"),
            _ => panic!("the scheduler dispatched something other than a draft"),
        }
    }

    /// The full frame the scheduler dispatched, as it lands — with the time
    /// its develop took STATED, so no test depends on how fast this machine is.
    fn next_full(app: &AutoShadeApp, took_ms: u64) -> PreviewDone {
        match next_develop(app) {
            Msg::Developed(done) => {
                let mut frame = (*done).expect("the full frame develops");
                frame.took = Duration::from_millis(took_ms);
                frame
            }
            _ => panic!("the scheduler dispatched something other than a full develop"),
        }
    }

    /// What the canvas develop of `app`'s live recipe is on `base`.
    fn expected_frame(app: &mut AutoShadeApp, base: Arc<image::DynamicImage>) -> PreviewDone {
        let (film, source) = (app.film_short_edge(), app.source_frame().unwrap());
        build_preview(base, app.recipe.clone(), false, film, source)
    }

    /// 2026-11-19 (user: 「响应速度（从调整到预览出效果）很慢」). On a photo whose
    /// full develop is slow, a tick during a gesture develops a DRAFT — the
    /// same develop on a reduced copy of the base — and the draft is shown
    /// even though the hand has moved on, then followed by the live recipe's
    /// own; when the gesture settles the full base is developed and replaces
    /// the draft in place, and that frame is `build_preview` on the full base
    /// byte for byte.
    ///
    /// MUTATION: `plan_develop` answers `Full` always (nothing drafts: the
    /// first assert); the gesture clause dropped from `passing_frame_shows`
    /// (the moved-past draft is not shown); `None` for the film edge in the
    /// draft's dispatch (the draft is no longer the reduced base's develop at
    /// the full frame's film edge); `plan_develop` answers `Draft` after the
    /// settle too (the full frame never comes).
    #[test]
    fn a_slow_photo_is_drawn_in_drafts_and_settles_on_the_full_frame() {
        let (mut app, _scrub) = draft_fixture("slow");
        let ctx = egui::Context::default();
        let base = app.base_preview.clone().unwrap();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        // The photo's last full develop took two seconds.
        app.draft.note_full_cost(&base, Duration::from_millis(2000));

        // An edit: the tick develops a draft, and the full develop stays owed.
        app.recipe.exposure_ev = 0.5;
        app.dirty = true;
        app.start_redevelop_at(at(0));
        assert!(app.draft.inflight, "a slow photo's tick develops a draft");
        assert!(!app.develop_inflight, "…and no full develop while the gesture runs");
        assert!(app.dirty, "the full develop of this recipe is still owed");
        let (first, edge, reduced) = next_draft(&app);
        assert_eq!(edge, 1024, "two seconds over a 2048 px base: the floor");

        // The hand moves on before the draft lands; one draft at a time.
        app.recipe.exposure_ev = 0.7;
        app.start_redevelop_at(at(40));
        app.finish_draft_at(&ctx, Ok((first, edge, reduced)), at(60));
        assert_eq!(app.draft.passing_count, 1, "the newest finished frame is shown, though the recipe moved on");
        assert_eq!(app.develop_count, 0, "a draft is not an accepted frame");
        assert_eq!(app.after_tex.as_ref().expect("the draft is on the canvas").size(), [1024, 683]);
        assert_eq!(app.draft.stands_for, Some(egui::vec2(2048.0, 1366.0)), "…laid out at the full frame's size");
        assert!(app.dirty, "the develop of the live recipe still follows");
        let shown = app.after_tex.as_ref().unwrap().id();
        app.canvas_fade = None;

        // …followed by the live recipe's own draft, which is the canvas's
        // develop on the reduced base, at the full frame's film edge.
        app.start_redevelop_at(at(70));
        assert!(app.draft.inflight, "the next tick drafts the live recipe");
        let (second, edge, reduced) = next_draft(&app);
        let small = Arc::new(autoshade::render::thumbnail(&base, 1024, 1024));
        assert!(Arc::ptr_eq(&second.base, &base), "a draft is a frame OF the canvas's base");
        assert!(second.rgb == expected_frame(&mut app, small).rgb, "a draft is the same develop on the reduced base");
        app.finish_draft_at(&ctx, Ok((second, edge, reduced)), at(90));
        assert_eq!(app.draft.passing_count, 2);

        // Inside the settle time nothing more is started…
        app.start_redevelop_at(at(100));
        assert!(!app.develop_inflight && !app.draft.inflight, "the gesture has not settled and its draft is shown");
        // …and DRAFT_SETTLE after the last edit (at 40 ms) the full base is.
        app.start_redevelop_at(at(40) + DRAFT_SETTLE);
        assert!(app.develop_inflight && !app.draft.inflight, "the settle develops the full base");
        assert!(!app.dirty, "…which pays the debt");
        let full = next_full(&app, 2000);
        let expect = expected_frame(&mut app, base.clone());
        assert!(full.rgb == expect.rgb && full.after == expect.after, "the settled frame is build_preview on the full base, byte for byte");
        app.finish_redevelop_at(&ctx, Ok(full), at(900));
        assert_eq!(app.develop_count, 1, "the full frame is the accepted one");
        assert_eq!(app.after_tex.as_ref().unwrap().size(), [2048, 1366]);
        assert_eq!(app.after_tex.as_ref().unwrap().id(), shown, "it replaces the draft in place");
        assert!(app.canvas_fade.is_none(), "…with no fade");
        assert_eq!(app.draft.stands_for, None, "a full frame is laid out at its own size");
        assert!(app.last_rgb.as_ref() == Some(&expect.rgb), "the canvas holds the full frame's pixels");
    }

    /// A photo whose full develop is inside the budget never drafts: every
    /// tick is the full develop it always was — and so is the first develop of
    /// a photo, whose cost nobody knows yet. With the rule's own table: where
    /// the budget lands, the floor, and "a draft at least halves the pixels".
    ///
    /// MUTATION: `plan_develop` drafts at the floor whatever the develop cost
    /// (a quick photo drafts); the halving rule dropped from `draft_edge`
    /// (a 130 ms photo is developed twice per tick for 23 % fewer pixels).
    #[test]
    fn a_photo_inside_the_budget_never_drafts() {
        for cost in [None, Some(80)] {
            let (mut app, _scrub) = draft_fixture("quick");
            let base = app.base_preview.clone().unwrap();
            if let Some(ms) = cost {
                app.draft.note_full_cost(&base, Duration::from_millis(ms));
            }
            let t0 = Instant::now();
            app.recipe.exposure_ev = 0.5;
            app.dirty = true;
            app.start_redevelop_at(t0);
            assert!(app.develop_inflight && !app.draft.inflight, "{cost:?}: the tick is a full develop");
            assert!(!app.dirty, "{cost:?}: …which consumes the edit");
            // A further edit while it is in flight waits for it, as it always did.
            app.recipe.exposure_ev = 0.7;
            app.dirty = true;
            app.start_redevelop_at(t0 + Duration::from_millis(20));
            assert!(!app.draft.inflight && app.dirty, "{cost:?}: no draft beside it");
            let frame = next_full(&app, 80);
            assert_eq!((frame.rgb.width(), frame.rgb.height()), (2048, 1366), "{cost:?}");
            assert_eq!(frame.stands_for, None, "{cost:?}");
        }
        // The rule: (base long edge, full develop ms) → the draft's long edge.
        for (long, ms, edge) in [
            (2048, 80.0, None),         // inside the budget
            (2048, 120.0, None),        // on it
            (2048, 130.0, None),        // 1792 px would keep 77 % of the pixels
            (2048, 2000.0, Some(1024)), // the floor
            (1400, 2000.0, None),       // the floor would not halve a 1400 px base
            (4096, 3800.0, Some(1024)),
            (4096, 500.0, Some(1792)),  // 4096·√(120/500) = 2006, down to a step
            (4096, 260.0, Some(2560)),
            (4096, 213.0, None),        // 3072 px is more than half the pixels
        ] {
            assert_eq!(draft_edge(long, ms), edge, "{long} px base, {ms} ms");
        }
    }

    /// During a gesture the newest finished frame is shown even when the
    /// recipe has moved on — on any photo, drafting or not — and the develop
    /// of the live recipe follows it. Outside a gesture a frame for a moved-on
    /// recipe is dropped as it always was: nothing stale is put on screen.
    ///
    /// MUTATION: `passing_frame_shows` without its gesture clause, either way
    /// round — never (`frame.recipe == self.recipe` alone: the first half) or
    /// always (the second half).
    #[test]
    fn a_frame_the_gesture_moved_past_is_shown_and_a_stale_one_outside_a_gesture_is_dropped() {
        let (mut app, _scrub) = draft_fixture("passing");
        let ctx = egui::Context::default();
        let base = app.base_preview.clone().unwrap();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);

        app.recipe.exposure_ev = 0.5;
        app.dirty = true;
        app.start_redevelop_at(at(0));
        let first = next_full(&app, 80);
        let first_rgb = first.rgb.clone();
        // The hand moves on while it is in flight.
        app.recipe.exposure_ev = 0.7;
        app.dirty = true;
        app.start_redevelop_at(at(30));
        app.finish_redevelop_at(&ctx, Ok(first), at(50));
        assert_eq!(app.draft.passing_count, 1, "a frame the gesture moved past is shown on its way");
        assert_eq!(app.develop_count, 0, "…without being the accepted frame");
        assert!(app.last_rgb.as_ref() == Some(&first_rgb));
        assert!(app.variants[0].thumb.is_none(), "…or the card's thumbnail");
        assert!(app.dirty && !app.develop_inflight, "the live recipe's develop is owed");
        app.start_redevelop_at(at(60));
        assert!(app.develop_inflight, "…and starts on the next tick");
        let second = next_full(&app, 80);
        let expect = expected_frame(&mut app, base.clone());
        assert!(second.rgb == expect.rgb);
        app.finish_redevelop_at(&ctx, Ok(second), at(100));
        assert_eq!(app.develop_count, 1, "the live recipe's frame is the accepted one");

        // No gesture: an edit, its develop in flight, another edit — and the
        // frame lands three seconds after the last one.
        app.recipe.exposure_ev = 0.9;
        app.dirty = true;
        app.start_redevelop_at(at(1000));
        let stale = next_full(&app, 80);
        app.recipe.exposure_ev = 1.1;
        app.start_redevelop_at(at(2000));
        app.dirty = false;
        app.finish_redevelop_at(&ctx, Ok(stale), at(5000));
        assert_eq!(app.draft.passing_count, 1, "outside a gesture a stale frame is not shown");
        assert_eq!(app.develop_count, 1);
        assert!(app.last_rgb.as_ref() == Some(&expect.rgb), "the canvas keeps the last frame it accepted");
        assert!(app.dirty, "…and the live recipe's develop is re-armed");
    }

    /// Frames land in any order, and the canvas only ever moves forward: a
    /// full develop the gesture moved past does not replace the newer draft
    /// that overtook it, and a draft of the very recipe whose FULL frame is on
    /// the canvas does not soften it again. (A draft starts beside a full
    /// develop that is still in flight — each has its own guard.)
    ///
    /// MUTATION: the dispatch-order clause dropped from `passing_frame_shows`
    /// (the overtaken full frame goes over the newer draft); the
    /// `shows_full_of` clause dropped (the late draft replaces the settled
    /// full frame of its own recipe).
    #[test]
    fn an_overtaken_frame_never_replaces_a_newer_one() {
        let (mut app, _scrub) = draft_fixture("order");
        let ctx = egui::Context::default();
        let base = app.base_preview.clone().unwrap();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        app.draft.note_full_cost(&base, Duration::from_millis(2000));

        // A gesture, its draft, the settle: a full develop is in flight.
        app.recipe.exposure_ev = 0.5;
        app.dirty = true;
        app.start_redevelop_at(at(0));
        app.finish_draft_at(&ctx, Ok(next_draft(&app)), at(10));
        app.start_redevelop_at(at(200));
        assert!(app.develop_inflight, "premise: the settle's full develop is in flight");
        let overtaken = next_full(&app, 2000);
        // The hand moves again: a draft starts beside it, and lands first.
        app.recipe.exposure_ev = 0.7;
        app.dirty = true;
        app.start_redevelop_at(at(300));
        assert!(app.draft.inflight && app.develop_inflight, "a draft starts beside the full develop the gesture moved past");
        app.finish_draft_at(&ctx, Ok(next_draft(&app)), at(320));
        assert_eq!(app.draft.passing_count, 2);
        app.finish_redevelop_at(&ctx, Ok(overtaken), at(330));
        assert_eq!(app.draft.passing_count, 2, "the overtaken full frame does not go over the newer draft");
        assert_eq!(app.last_rgb.as_ref().map(|p| p.width()), Some(1024), "the canvas still shows the live recipe's draft");
        assert!(app.dirty && !app.develop_inflight);

        // The settle: the full frame of the live recipe is on the canvas.
        app.start_redevelop_at(at(300) + DRAFT_SETTLE);
        let full = next_full(&app, 2000);
        let full_rgb = full.rgb.clone();
        app.finish_redevelop_at(&ctx, Ok(full), at(900));
        assert_eq!(app.develop_count, 1);
        // A draft of that same recipe, dispatched later (the hand left and
        // came back to the value), lands in a running gesture.
        let (film, source) = (app.film_short_edge(), app.source_frame().unwrap());
        let (late, edge, reduced) = build_draft(base.clone(), None, 1024, app.recipe.clone(), false, film, source);
        let late = late.tagged(app.frame_tag());
        app.finish_draft_at(&ctx, Ok((late, edge, reduced)), at(910));
        assert_eq!(app.draft.passing_count, 2, "a draft does not replace the full frame of its own recipe");
        assert!(app.last_rgb.as_ref() == Some(&full_rgb));
        assert_eq!(app.draft.stands_for, None);
    }

    /// After the settle, everything that keeps a canvas frame holds the FULL
    /// one: the card's thumbnail is written by it alone, the clipping layer
    /// and the pixels the clipping toggle rebuilds from are its own, and so is
    /// the histogram. Until then "has this recipe's develop landed" — what the
    /// card switch asks before it trusts a thumbnail — stays "no".
    ///
    /// MUTATION: `show_passing_frame` also writes the card's thumbnail; the
    /// draft's dispatch clears `dirty` as the full develop's does (the card
    /// switch then trusts a frame that never landed, and the settle has
    /// nothing to develop).
    #[test]
    fn after_the_settle_the_card_and_the_clipping_layer_hold_the_full_frame() {
        let (mut app, _scrub) = draft_fixture("keeps");
        let ctx = egui::Context::default();
        let base = app.base_preview.clone().unwrap();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        app.draft.note_full_cost(&base, Duration::from_millis(2000));
        app.show_clipping = true;

        app.recipe.exposure_ev = 2.5; // blown, so the clipping layer has something to say
        app.dirty = true;
        app.start_redevelop_at(at(0));
        assert!(app.dirty || app.develop_inflight, "no frame of this recipe has landed");
        app.finish_draft_at(&ctx, Ok(next_draft(&app)), at(20));
        assert_eq!(app.draft.passing_count, 1, "premise: the draft is on the canvas");
        assert_eq!(app.clip_tex.as_ref().expect("the draft's own clipping layer").size(), [1024, 683]);
        assert!(app.variants[0].thumb.is_none(), "a draft is never the card's thumbnail");
        assert!(app.dirty || app.develop_inflight, "…and never counts as the recipe's develop having landed");

        app.start_redevelop_at(at(0) + DRAFT_SETTLE);
        assert!(app.dirty || app.develop_inflight, "the full develop is in flight, not landed");
        let full = next_full(&app, 2000);
        let mut expect = expected_frame(&mut app, base.clone());
        app.finish_redevelop_at(&ctx, Ok(full), at(900));
        assert!(!app.dirty && !app.develop_inflight, "now it has landed");
        assert!(app.variants[0].thumb.is_some(), "the full frame writes the card's thumbnail");
        assert!(app.last_rgb.as_ref() == Some(&expect.rgb));
        assert!(app.histogram.take() == Some(std::mem::take(&mut expect.histogram)), "the histogram is the full frame's");
        assert_eq!(app.clip_tex.as_ref().unwrap().size(), [2048, 1366], "the clipping layer is the full frame's");
        // The toggle rebuilds the layer from the retained pixels: the full frame's.
        app.toggle_clipping(&ctx);
        assert!(app.clip_tex.is_none());
        app.toggle_clipping(&ctx);
        assert_eq!(app.clip_tex.as_ref().unwrap().size(), [2048, 1366]);
    }

    /// A frame is shown on the card it was developed for and nowhere else: a
    /// card or a photo switched in the middle of a gesture shows nothing of
    /// the old one — not its draft, not its full frame — even where the two
    /// share their pixels (two cards on one source do), and a frame of another
    /// base is dropped whole.
    ///
    /// MUTATION: the card clause dropped from `passing_frame_shows` (the old
    /// card's draft is drawn over the new card).
    #[test]
    fn a_card_or_photo_switched_mid_gesture_shows_nothing_of_the_old_one() {
        let (mut app, _scrub) = draft_fixture("switch");
        let ctx = egui::Context::default();
        let base = app.base_preview.clone().unwrap();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        app.draft.note_full_cost(&base, Duration::from_millis(2000));
        let (film, source) = (app.film_short_edge(), app.source_frame().unwrap());
        let nothing_shown = |app: &AutoShadeApp, why: &str| {
            assert!(app.after_tex.is_none() && app.last_rgb.is_none(), "{why}");
            assert_eq!((app.draft.passing_count, app.develop_count), (0, 0), "{why}");
        };

        // Card a, mid-gesture: its draft is in flight, and a full frame of it too.
        app.recipe.exposure_ev = 0.5;
        app.dirty = true;
        app.start_redevelop_at(at(0));
        let draft_a = next_draft(&app);
        let full_a = build_preview(base.clone(), app.recipe.clone(), false, film, source.clone()).tagged(app.frame_tag());
        // The other card of the same photo, on the same pixels.
        app.active = 1;
        app.recipe = EditRecipe { contrast: 20.0, ..Default::default() };
        app.finish_draft_at(&ctx, Ok(draft_a), at(30));
        nothing_shown(&app, "another card's draft");
        app.finish_redevelop_at(&ctx, Ok(full_a), at(40));
        nothing_shown(&app, "another card's full frame");
        assert!(app.dirty, "the new card's own develop is owed");

        // Another photo (the worst case: the same pixels under another path).
        let (draft_b, edge, reduced) = build_draft(base.clone(), None, 1024, app.recipe.clone(), false, film, source.clone());
        let draft_b = draft_b.tagged(app.frame_tag());
        app.src_path = Some(PathBuf::from("out/_gui_draft_another_photo.png"));
        app.finish_draft_at(&ctx, Ok((draft_b, edge, reduced)), at(50));
        nothing_shown(&app, "another photo's draft");

        // Another base: the frame is not this canvas's at all.
        let (draft_c, edge, reduced) = build_draft(base, None, 1024, app.recipe.clone(), false, film, source);
        let draft_c = draft_c.tagged(app.frame_tag());
        app.base_preview = Some(Arc::new(image::DynamicImage::ImageRgb8(image::RgbImage::new(2048, 1366))));
        app.finish_draft_at(&ctx, Ok((draft_c, edge, reduced)), at(60));
        nothing_shown(&app, "a draft of another base");
    }

    /// A held pointer IS the gesture: however long the hand rests on a slider
    /// with the button down, the full develop waits for the release.
    ///
    /// MUTATION: `in_gesture` reads the settle time alone (the full develop
    /// starts under a held slider, and the next tick of the drag waits on it).
    #[test]
    fn a_held_pointer_keeps_the_gesture_open_until_it_is_released() {
        let (mut app, _scrub) = draft_fixture("held");
        let ctx = egui::Context::default();
        let button = |pressed| egui::Event::PointerButton {
            pos: egui::pos2(10.0, 10.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let feed = |ctx: &egui::Context, events: Vec<egui::Event>| {
            let _ = ctx.run(egui::RawInput { events, ..Default::default() }, |_| {});
        };
        feed(&ctx, vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0)), button(true)]);
        app.egui_ctx = ctx.clone();
        let base = app.base_preview.clone().unwrap();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        app.draft.note_full_cost(&base, Duration::from_millis(2000));

        app.recipe.exposure_ev = 0.5;
        app.dirty = true;
        app.start_redevelop_at(at(0));
        app.finish_draft_at(&ctx, Ok(next_draft(&app)), at(20));
        // Two seconds on, the button still down, nothing edited since.
        app.start_redevelop_at(at(2000));
        assert!(!app.develop_inflight && !app.draft.inflight, "a held pointer holds the full develop back");
        assert!(app.dirty, "…which stays owed");
        feed(&ctx, vec![button(false)]);
        app.start_redevelop_at(at(2010));
        assert!(app.develop_inflight, "the release settles the gesture: the full base is developed");
    }

    /// What is saved and what is exported is developed from the photo, never
    /// read off the canvas — so a draft cannot reach a file. Pinned in the
    /// source, the house pattern for a property no offline test can drive
    /// through every path: in the files that save and export, the canvas's
    /// frame is only ever CLEARED.
    ///
    /// MUTATION: a read of `self.last_rgb` (or the After texture) added to
    /// `export.rs`, `persist.rs` or `actions.rs`.
    #[test]
    fn nothing_that_saves_or_exports_reads_a_canvas_frame() {
        for (name, src) in [
            ("export.rs", include_str!("../export.rs")),
            ("persist.rs", include_str!("../persist.rs")),
            ("actions.rs", include_str!("../actions.rs")),
        ] {
            for field in ["self.last_rgb", "self.after_tex"] {
                for (at, _) in src.match_indices(field) {
                    let rest = &src[at + field.len()..];
                    let line: String = src[at..].chars().take_while(|c| *c != '\n').collect();
                    assert!(rest.starts_with(" = None;"), "{name} reads a canvas frame: {line}");
                }
            }
        }
    }
