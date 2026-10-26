// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the embedded packet, tool disarming, refine budgets, landings, the history gate and the persisted strip.

    #[test]
    fn an_embedded_raw_xmp_packet_restores_when_nothing_else_answers() {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-packet-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_packet.dng");
        let doc = autoshade::xmp::recipe_to_xmp(&EditRecipe {
            exposure_ev: 0.8,
            ..Default::default()
        });
        std::fs::write(&src, tiff_with_xmp(doc.as_bytes())).unwrap();
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]);

        let restored = read_saved_develop(&src);
        let SavedDevelop::Restored(r, kind) = restored.saved else {
            panic!("the baked develop must restore");
        };
        assert_eq!(kind, "XMP (embedded in the RAW)");
        assert_eq!(r.exposure_ev, 0.8);
        assert!(restored.packet_unreadable.is_none());

        // The batch snapshot answers the same develop (anti-drift).
        let snap = autoshade::store::read_develop_snapshot(&src).unwrap();
        let (batch, batch_kind) = crate::export::resolve_snapshot_develop(&src, &snap, &mut Vec::new())
            .unwrap()
            .expect("the batch resolves the packet too");
        assert_eq!(batch_kind, kind);
        assert_eq!(batch.exposure_ev, 0.8);
    }

    /// The packet never outranks anything, and an explicit clear sticks
    /// against it: the packet lives in a file this app never writes, so
    /// without the marker gate Reset+Save would resurrect the baked develop
    /// on the very next open. An unreadable packet is disclosed, not folded
    /// into absence.
    #[test]
    fn an_embedded_packet_never_outranks_the_store_or_a_clear() {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-packet-rank-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_packet_rank.dng");
        let doc = autoshade::xmp::recipe_to_xmp(&EditRecipe {
            exposure_ev: 0.8,
            ..Default::default()
        });
        std::fs::write(&src, tiff_with_xmp(doc.as_bytes())).unwrap();
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]);

        // (a) A stored develop outranks the packet.
        std::fs::write(
            autoshade::store::recipe_target(&src),
            serde_json::to_string(&EditRecipe { exposure_ev: 0.5, ..Default::default() })
                .unwrap(),
        )
        .unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(&src).saved else {
            panic!("the store must answer");
        };
        assert_eq!(kind, "recipe.json");
        assert_eq!(r.exposure_ev, 0.5);

        // (a2) A NEUTRAL recipe.json is a store file expressing neutral
        // intent — it bars the packet too (Codex L05 EMBED-01: a web-side
        // neutral save has no cleared.txt, and the packet must not
        // out-answer it on the next open), on the open path AND the batch
        // snapshot alike.
        std::fs::write(
            autoshade::store::recipe_target(&src),
            serde_json::to_string(&EditRecipe::default()).unwrap(),
        )
        .unwrap();
        assert!(
            matches!(read_saved_develop(&src).saved, SavedDevelop::NoopOnly),
            "a neutral store file bars the packet"
        );
        let snap = autoshade::store::read_develop_snapshot(&src).unwrap();
        assert!(
            crate::export::resolve_snapshot_develop(&src, &snap, &mut Vec::new()).unwrap().is_none(),
            "the batch answers the same: neutral store, no packet"
        );

        // (b) An explicit clear sticks: marker present, no store files — the
        // packet must NOT resurrect the cleared develop.
        std::fs::remove_file(autoshade::store::recipe_target(&src)).unwrap();
        std::fs::write(dev.join("cleared.txt"), b"cleared").unwrap();
        assert!(
            matches!(read_saved_develop(&src).saved, SavedDevelop::Nothing),
            "a cleared develop stays cleared"
        );
        std::fs::remove_file(dev.join("cleared.txt")).unwrap();

        // (c) Unreadable ≠ absent: a non-text packet is disclosed.
        std::fs::write(&src, tiff_with_xmp(&[0xFF, 0xFE, 0x00, 0x01, 0x02])).unwrap();
        let restored = read_saved_develop(&src);
        assert!(matches!(restored.saved, SavedDevelop::Nothing));
        let why = restored.packet_unreadable.expect("the unreadable packet is disclosed");
        assert!(why.contains("not UTF-8"), "{why}");
    }

    /// L06#3: selection moved ⇒ every index-armed tool whose target is no
    /// longer the selected mask dies with the old row — ↻ Redraw and
    /// add-component arm silently (their indicators live inside the
    /// selected-mask block and the canvas hint discards the PlaceTarget),
    /// so a stranded arming rewrote the OLD mask while the user looked at
    /// another row. A NewMask placement and the non-mask tools are spared.
    #[test]
    fn a_selection_switch_disarms_index_armed_tools() {
        let mut app = AutoShadeApp::default();
        app.recipe.masks =
            vec![autoshade::recipe::LocalAdjustment::default(), Default::default()];

        app.placing_mask = Some((MaskKind::Linear, PlaceTarget::Redraw(0)));
        app.place_start = Some((0.5, 0.5));
        assert!(!app.disarm_selection_bound_tools(Some(1)), "no brush session died");
        assert!(app.placing_mask.is_none(), "an armed Redraw dies with its row");
        assert!(app.place_start.is_none(), "…and its gesture anchor with it");

        app.placing_mask = Some((
            MaskKind::Radial,
            PlaceTarget::Component(0, autoshade::recipe::MaskCombine::Add),
        ));
        app.disarm_selection_bound_tools(None); // same-row deselect
        assert!(app.placing_mask.is_none(), "deselecting disarms too");

        // NewMask is NOT selection-bound — browsing the list keeps it armed;
        // non-mask tools (crop) are none of this helper's business.
        app.placing_mask = Some((MaskKind::Linear, PlaceTarget::NewMask));
        app.crop_mode = true;
        app.disarm_selection_bound_tools(Some(1));
        assert!(app.placing_mask.is_some(), "a NewMask placement survives");
        assert!(app.crop_mode, "non-mask tools survive");

        // The kept row's own arming survives.
        app.placing_mask = Some((MaskKind::Linear, PlaceTarget::Redraw(1)));
        app.disarm_selection_bound_tools(Some(1));
        assert!(app.placing_mask.is_some(), "the selected row keeps its arming");
    }

    /// L06#5: the plate under the canvas was replaced — a live brush
    /// session is dimension-locked to the OLD plate (start_mask_brush sizes
    /// canvas + weight buffer together), so it dies with it and 「Apply」
    /// has nothing stale left to bake. A selection switch ends an
    /// off-selection session the same way, disclosed; the kept row's
    /// session survives.
    #[test]
    fn a_plate_replacement_ends_the_mask_brush_session() {
        let mut app = AutoShadeApp {
            base_preview: Some(std::sync::Arc::new(image::DynamicImage::new_rgba8(8, 8))),
            ..Default::default()
        };
        app.start_mask_brush(None);
        assert!(
            app.mask_brush.is_some() && app.mask_brush_gray.is_some() && app.paint_mode,
            "fixture: a session is armed"
        );

        app.rebind_paint_canvas(16, 16);
        assert!(app.mask_brush.is_none(), "the session dies with the plate");
        assert!(app.mask_brush_gray.is_none(), "no weight buffer left to bake");
        assert!(!app.paint_mode);
        let m = app.mask_paint.as_ref().expect("a fresh canvas at the new size");
        assert_eq!((m.width(), m.height()), (16, 16));

        app.recipe.masks =
            vec![autoshade::recipe::LocalAdjustment::default(), Default::default()];
        app.start_mask_brush(Some(0));
        assert!(
            app.disarm_selection_bound_tools(Some(1)),
            "an off-selection brush session dies, and says so (return drives the toast)"
        );
        assert!(app.mask_brush.is_none());

        app.start_mask_brush(Some(1));
        assert!(!app.disarm_selection_bound_tools(Some(1)));
        assert!(app.mask_brush.is_some(), "the selected row keeps its session");
    }

    /// R22 #2: the full-resolution mask refine refuses a source whose own
    /// resolution would put the refined raster past the mask-raster budget —
    /// and the refusal is a TYPED fact worded at LANDING (L12#4), not a
    /// sentence the worker built with a stale language.
    ///
    /// The mask must come out of it UNCHANGED: nothing was written, so the row
    /// still points at the raster it pointed at before, and busy is released.
    #[test]
    fn an_over_budget_refine_refuses_with_the_dimensions_and_keeps_the_mask() {
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp {
            busy: true,
            recipe: EditRecipe {
                masks: vec![autoshade::recipe::LocalAdjustment {
                    mask: MaskGeometry::Bitmap { path: "mask-1.png".into() },
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        // The gate's own arithmetic decides what "over budget" is — pin the
        // dimensions against it instead of hard-coding a pixel count that a
        // budget change would silently make meaningless. Asked with an EMPTY
        // recipe, so this fixture is over the budget on its own resolution
        // alone; the aggregate arm (a second refine that fits alone but not
        // beside the recipe's other rasters) is pinned in render.rs's
        // `a_second_full_resolution_refine_is_refused_by_the_aggregate_budget`.
        assert!(
            !autoshade::render::mask_raster_write_fits_budget(
                &EditRecipe::default(),
                None,
                12_000,
                9_000
            ),
            "fixture: 108 MP must be over the mask budget"
        );
        app.tx
            .send(Msg::MaskRefined(Ok(MaskRefineOutcome::OverBudget { w: 12_000, h: 9_000 })))
            .unwrap();
        app.poll_workers(&ctx);

        assert!(!app.busy, "the refusal releases the worker gate");
        assert!(
            matches!(&app.recipe.masks[0].mask, MaskGeometry::Bitmap { path } if path == "mask-1.png"),
            "nothing was written, so the mask keeps its own raster"
        );
        assert!(
            app.status.contains("12000") && app.status.contains("9000"),
            "the refusal names the source it is talking about: {}",
            app.status
        );
        assert!(
            app.toasts.iter().any(|t| matches!(t.kind, ToastKind::Error)),
            "a refusal the user did not ask for is a toast, not just a status line"
        );
    }

    /// L06#4: a recipe edit made while the retouch worker runs survives as
    /// its OWN undo step — committing only AFTER the plate swap folded it
    /// into the pixel step, so one Ctrl+Z reverted both and the slider move
    /// could not be kept while dropping the retouch.
    #[test]
    fn a_retouch_landing_does_not_fold_a_mid_flight_recipe_edit_into_the_pixel_step() {
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp::default();
        let b0 = std::sync::Arc::new(image::DynamicImage::new_rgba8(4, 4));
        app.variants = vec![Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: Some(b0.clone()),
            origin: None,
            thumb: None,
        }];
        app.active = 0;
        app.base_preview = Some(b0.clone());
        app.reset_history(); // committed = (neutral recipe, b0)

        // A slider edit still mid-gesture when the worker returns.
        app.recipe.exposure_ev = 0.5;

        let epoch = app.gen_epoch;
        app.on_retouched(
            &ctx,
            Lang::En,
            epoch,
            Ok((
                image::DynamicImage::new_rgba8(4, 4),
                RetouchNote::Healed {
                    n: 1,
                    out: std::path::PathBuf::from("out/_retouch_order_test.png"),
                    ai_prose: String::new(),
                    notes: Vec::new(),
                    in_face: false,
                },
                std::path::PathBuf::from("out/_retouch_order_test.png"),
                RetouchKind::InPlace,
            )),
        );

        assert_eq!(app.undo_stack.len(), 2, "slider step + pixel step, not one folded step");
        let undone = app.undo_stack.last().unwrap();
        assert_eq!(undone.recipe.exposure_ev, 0.5, "the slider edit is NOT in the pixel step");
        assert!(
            undone.base.as_ref().is_some_and(|b| std::sync::Arc::ptr_eq(b, &b0)),
            "one Ctrl+Z drops the retouch and keeps the slider move"
        );
        assert!(
            app.committed.base.as_ref().is_some_and(|b| !std::sync::Arc::ptr_eq(b, &b0)),
            "the head holds the retouched pixels"
        );
    }

    /// L06#6: a background card's thumb has exactly one writer (the
    /// ACTIVE-only finish_redevelop) and the switch makes the acceptance
    /// gate reject any frame still in flight — so leaving with an edit
    /// pending must drop the card to the honest "…" placeholder, and a
    /// settled switch must keep it.
    #[test]
    fn a_variant_switch_drops_a_card_whose_frame_never_landed() {
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp::default();
        let tex = ctx.load_texture(
            "l06_thumb",
            egui::ColorImage::example(),
            egui::TextureOptions::LINEAR,
        );
        app.variants = vec![
            Variant {
                id: String::new(),
                name: None,
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base: None,
                origin: None,
                thumb: Some(tex),
            },
            Variant {
                id: String::new(),
                name: None,
                kind: VariantKind::Fitted,
                recipe: EditRecipe::default(),
                base: None,
                origin: None,
                thumb: None,
            },
        ];
        app.active = 0;
        app.dirty = false;
        app.develop_inflight = false;
        app.switch_variant(1, &ctx);
        assert!(app.variants[0].thumb.is_some(), "a settled card keeps its thumb");

        app.dirty = false;
        app.develop_inflight = false;
        app.switch_variant(0, &ctx);
        app.dirty = true; // an edit awaiting dispatch — no frame depicts it
        app.switch_variant(1, &ctx);
        assert!(
            app.variants[0].thumb.is_none(),
            "no completed frame depicts the edit — the honest … placeholder takes over"
        );
    }

    #[test]
    fn a_stale_keep_request_is_refused_without_the_same_path_fact() {
        // The KEEP arm honours the request only when open_path's recorded
        // FACT agrees the flight was same-path: honouring a stale request
        // grafted the outgoing photo's whole strip onto the incoming one.
        // Deleting `&& self.open_same_path` at the KEEP arm turns phase 1
        // into a graft and fails its assert.
        let mut app = AutoShadeApp::default();
        let ctx = egui::Context::default();
        let mk = || Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Generated,
            recipe: EditRecipe::default(),
            base: None,
            origin: Some(PathBuf::from("out/_keepfact_gen.png")),
            thumb: None,
        };
        app.variants = vec![mk(), mk()];
        // Stem must be globally improbable: the fresh arm's
        // read_saved_develop runs store::migrate_legacy, which scans the
        // cwd ./out for {stem}.recipe.json / {stem}.xmp and MOVES any hit
        // into the develop dir keyed to this fake path — a generic stem
        // ("b") could relocate a real user's legacy sidecars into an
        // unreachable key.
        app.src_path =
            Some(PathBuf::from("D:/__autoshade_keepfact__/__autoshade_keepfact__.ARW"));
        app.keep_recipe = true;
        app.open_same_path = false; // stale request, cross-photo fact
        let base = Arc::new(image::DynamicImage::new_rgb8(8, 6));
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                base.clone(),
                Vec::new(),
                Default::default(),
                None,
                None,
                (1280, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants.len(), 1, "fresh arm: the strip is rebuilt, never grafted");
        // ...and an HONOURED request (fact agrees) preserves the strip.
        app.variants = vec![mk(), mk()];
        app.keep_recipe = true;
        app.open_same_path = true;
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                base,
                Vec::new(),
                Default::default(),
                None,
                None,
                (1280, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants.len(), 2, "keep arm: the strip survives a same-path re-decode");
        // ...and the REQUEST half is load-bearing too: fact without request
        // (a same-path FRESH reopen) reloads fresh. Reducing the KEEP arm to
        // `let keep = self.open_same_path;` turns this into a graft — the
        // reload never happens and the one-shot request goes sticky — and
        // fails the assert.
        app.variants = vec![mk(), mk()];
        app.keep_recipe = false;
        app.open_same_path = true;
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                Arc::new(image::DynamicImage::new_rgb8(8, 6)),
                Vec::new(),
                Default::default(),
                None,
                None,
                (1280, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants.len(), 1, "fresh arm: a reopen without the request reloads");
    }

    #[test]
    fn the_history_gate_takes_request_and_fact_before_repairing() {
        // apply_step is history's one exit, and its repair gate takes
        // request AND fact: during a same-path FRESH reopen the rebuild
        // discards the repair, so the gate refuses; a same-path keep-flight
        // is admitted. (The keyboard undo is busy-gated now, so this test
        // drives the states directly — the gate is defence-in-depth, and
        // this is what keeps it honest.) The memo seam keeps this
        // decodable-RAW-free — the repair consults the memo before
        // estimating, so a primed answer makes the gate's verdict visible
        // as the era stamp. Deleting `&& self.keep_recipe` at the
        // apply_step gate repairs phase 1's install and fails its assert.
        let mut app = AutoShadeApp::default();
        let ctx = egui::Context::default();
        let p = PathBuf::from("D:/__autoshade_histgate__/__autoshade_histgate__.ARW");
        // Pre-era fingerprint: version 1, interior x >= 0.5, darkens > 0.05.
        let washy = vec![[0.0, 0.0], [0.6, 0.4], [1.0, 1.0]];
        let primed = vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]];
        autoshade::pipeline::prime_curve_memo(
            &p,
            autoshade::pipeline::curve_ident(&p),
            primed.clone(),
        );
        let pre_era =
            EditRecipe { version: 1, base_curve: washy.clone(), ..Default::default() };
        app.src_path = Some(p);
        app.variants = vec![Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Original,
            recipe: pre_era.clone(),
            base: None,
            origin: None,
            thumb: None,
        }];
        app.recipe = pre_era.clone();
        app.committed = UndoStep { recipe: pre_era.clone(), base: None, origin: None };
        app.undo_stack.push(UndoStep { recipe: pre_era.clone(), base: None, origin: None });
        // Same-path FRESH reopen in flight: fact without request — refused.
        app.open_in_flight = true;
        app.open_same_path = true;
        app.keep_recipe = false;
        app.undo(&ctx);
        assert_eq!(app.recipe.version, 1, "fresh-reopen flight: the install is NOT repaired");
        assert_eq!(app.recipe.base_curve, washy, "…and the washed curve is untouched");
        // Same-path keep-flight: request AND fact — admitted, and the memo's
        // primed answer is adopted with its era stamp.
        app.keep_recipe = true;
        app.redo(&ctx);
        assert_eq!(
            app.recipe.version,
            autoshade::recipe::CALIB_ERA,
            "keep-flight: the reinstalled pre-era pair is repaired"
        );
        assert_eq!(app.recipe.base_curve, primed, "…with the primed answer, not a re-estimate");
        assert_eq!(
            app.variants[0].recipe.base_curve, primed,
            "…and the strip entry follows the healed canvas"
        );
        // ...and the FACT half is load-bearing too: the redo above pushed the
        // still-washed `committed` back onto the undo stack, so one more undo
        // reinstalls it — this time with the request but a CROSS-PHOTO fact,
        // where src_path already points at the incoming photo and repairing
        // would estimate the wrong RAW onto this canvas. Deleting
        // `&& self.open_same_path` from the apply_step gate repairs it and
        // fails these asserts.
        app.open_same_path = false;
        app.undo(&ctx);
        assert_eq!(app.recipe.version, 1, "cross-photo flight: the install is NOT repaired");
        assert_eq!(app.recipe.base_curve, washy, "…and the washed curve is untouched");
    }

    #[test]
    fn a_landing_that_cannot_move_the_canvas_puts_the_switch_back() {
        // The px combo mutates preview_edge BEFORE the flight starts, so
        // every landing that leaves the canvas where it was must put the
        // switch back — otherwise the displayed resolution is one the
        // preview never reached, it persists into Prefs, and five bake sites
        // read it (a heal in that window installs a new-edge raster under an
        // old-edge canvas). Three outcomes, three asserts.
        let ctx = egui::Context::default();
        let armed = |base: Option<Arc<image::DynamicImage>>, origin: Option<PathBuf>| AutoShadeApp {
            src_path: Some(PathBuf::from(
                "D:/__autoshade_edgeflight__/__autoshade_edgeflight__.ARW",
            )),
            variants: vec![Variant {
                id: String::new(),
                name: None,
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base,
                origin,
                thumb: None,
            }],
            preview_edge: 4096,             // the combo already switched...
            edge_before_flight: Some(1280), // ...and armed the flight
            keep_recipe: true,
            open_same_path: true,
            open_in_flight: true,
            // What open_path leaves on the bar for the whole flight.
            status: "decoding … ".to_string(),
            ..Default::default()
        };
        let fresh = || Arc::new(image::DynamicImage::new_rgb8(8, 6));

        // (1) The re-decode FAILED: the photo and its canvas are untouched.
        let mut app = armed(None, None);
        app.tx.send(Msg::Opened(Box::new(Err(anyhow::anyhow!("decode failed"))))).unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.preview_edge, 1280, "a failed keep-flight puts the switch back");
        assert!(app.src_path.is_some(), "…and leaves the still-open photo alone");

        // (2) It landed, but the canvas renders an UNSAVED retouch — no
        // pixels.json, so the worker brought no master and base_preview
        // would have kept its old-edge raster under a 4096 preview_edge.
        let old_raster = Arc::new(image::DynamicImage::new_rgb8(4, 3));
        let old_source = Arc::new(image::DynamicImage::new_rgb8(6, 4));
        // The canvas master EXISTS: this is the arm where saving is the real
        // remedy, and without a live file the missing-master arm would answer
        // instead — leaving the record-less arm unpinned (a mutant that always
        // emits the other message used to survive the whole suite).
        let master = PathBuf::from("out/__autoshade_edgeflight__.heal.png");
        std::fs::create_dir_all("out").unwrap();
        let _scrub = Scrub(vec![master.clone()]);
        std::fs::write(&master, b"png").unwrap();
        let mut app = armed(Some(old_raster.clone()), Some(master.clone()));
        app.base_preview = Some(old_raster.clone());
        app.source_preview = Some(old_source.clone()); // the OLD-edge decode
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                fresh(),
                Vec::new(),
                Default::default(),
                None,
                None,
                (4096, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.preview_edge, 1280, "a switch the canvas cannot take is not recorded");
        assert!(
            app.base_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &old_raster)),
            "…the retouched canvas is untouched"
        );
        assert!(
            app.source_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &old_source)),
            "…and the OLD-edge source survives instead of being replaced by the new one, \
             so a later undo-to-source cannot disagree the other way"
        );
        assert!(
            app.status.starts_with("preview resolution kept"),
            "…and the refusal replaces the flight's 'decoding …', which never expires: {}",
            app.status
        );
        assert!(
            app.status.contains("save the photo"),
            "…prescribing the remedy that DOES work when the master is on disk: {}",
            app.status
        );

        // (3) The SAME master, recorded: the switch applies. The two
        // spellings differ — an in-session retouch records ./out relative,
        // store::write_pixel_source absolutizes — and comparing them
        // literally left 1:1 inspection at the old resolution.
        let rel = PathBuf::from("out/_edgeflight.png");
        let abs = std::path::absolute(&rel).unwrap();
        assert_ne!(rel, abs, "premise: the two spellings are not equal as paths");
        let mut app = armed(Some(old_raster.clone()), Some(rel));
        app.base_preview = Some(old_raster.clone());
        let master = fresh();
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                fresh(),
                Vec::new(),
                Default::default(),
                None,
                Some((master.clone(), abs, false)),
                (4096, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.preview_edge, 4096, "a canvas that CAN be re-pointed keeps the switch");
        assert!(
            app.base_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &master)),
            "…and renders the master re-decoded at the new edge"
        );
        assert!(
            app.variants[0].base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &master)),
            "…with the variant re-pointed at it"
        );
    }

    /// Removes its paths on DROP, so a failing assert cannot leave fixtures
    /// behind in the user's real central store or in ./out — a panic skips
    /// trailing cleanup lines, and the failing case is exactly the one a
    /// regression hits.
    struct Scrub(Vec<PathBuf>);
    impl Drop for Scrub {
        fn drop(&mut self) {
            for p in &self.0 {
                let _ = std::fs::remove_file(p);
                let _ = std::fs::remove_dir_all(p);
            }
        }
    }

    fn persisted_two_card_fixture() -> AutoShadeApp {
        let original_recipe = EditRecipe { exposure_ev: 0.4, ..Default::default() };
        let fitted_recipe = EditRecipe { contrast: 18.0, ..Default::default() };
        AutoShadeApp {
            src_path: Some(PathBuf::from("D:/library/__variant-baseline__.ARW")),
            recipe: original_recipe.clone(),
            saved_recipe: original_recipe.clone(),
            pixels_on_disk: None,
            variants: vec![
                Variant {
                    kind: VariantKind::Original,
                    id: "card-x".into(),
                    name: Some("negative".into()),
                    recipe: original_recipe,
                    base: None,
                    origin: None,
                    thumb: None,
                },
                Variant {
                    kind: VariantKind::Fitted,
                    id: "card-y".into(),
                    name: Some("fitted".into()),
                    recipe: fitted_recipe.clone(),
                    base: None,
                    origin: None,
                    thumb: None,
                },
            ],
            active: 0,
            saved_strip: Some(autoshade::store::VariantsRecord {
                v: 1,
                active_kind: "original".into(),
                active_pos: 0,
                active_id: Some("card-x".into()),
                active_name: Some("negative".into()),
                others: vec![autoshade::store::VariantEntry {
                    kind: "fitted".into(),
                    recipe: fitted_recipe,
                    origin: None,
                    id: Some("card-y".into()),
                    name: Some("fitted".into()),
                    extra: Default::default(),
                }],
                extra: Default::default(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn switching_persisted_cards_keeps_all_four_unsaved_consumers_clean() {
        let mut app = persisted_two_card_fixture();
        let ctx = egui::Context::default();
        app.switch_variant(1, &ctx);

        assert!(!app.unsaved_marker_dirty(), "the ● marker predicate stays clean");
        assert!(!app.quit_guard_open_dirty(), "the window-close predicate stays clean");
        assert!(!app.nav_stash_gate_dirty(), "the navigation-stash gate stays clean");
        assert!(!app.pending_save_gate_dirty(), "the PendingSave gate stays clean");
        assert_eq!(
            app.open_dirty_variants(),
            0,
            "the saved-active card is now background and resolves through recipe.json"
        );
        assert_eq!(
            app.saved_strip.as_ref().and_then(|r| r.active_id.as_deref()),
            Some("card-x"),
            "viewing another card does not move the persisted selection"
        );

        let old = app.src_path.clone().unwrap();
        app.open_path(PathBuf::from("D:/library/__variant-baseline-next__.ARW"));
        assert!(
            !app.nav_stash.contains_key(&old),
            "the production navigation call site must not stash a clean card switch"
        );
    }

    #[test]
    fn edit_save_and_switch_back_use_each_cards_persisted_develop() {
        let mut app = persisted_two_card_fixture();
        let ctx = egui::Context::default();
        app.switch_variant(1, &ctx);
        app.recipe.contrast = 27.0;
        assert!(app.unsaved_marker_dirty(), "editing the switched-to card is dirty");
        assert!(app.quit_guard_open_dirty(), "the close guard sees that edit");
        assert!(app.nav_stash_gate_dirty(), "the nav gate sees that edit");
        assert!(app.pending_save_gate_dirty(), "the quit save list sees that edit");

        app.variants[app.active].recipe = app.recipe.clone();
        app.saved_recipe = app.recipe.clone();
        app.pixels_on_disk = app.active_variant().and_then(|v| v.origin.clone());
        app.saved_strip = app.current_strip_record();
        assert!(!app.unsaved_marker_dirty(), "saving on card Y advances Y's baseline");
        assert_eq!(app.open_dirty_variants(), 0, "the whole strip is clean after that save");

        app.switch_variant(0, &ctx);
        assert!(!app.unsaved_marker_dirty(), "card X now resolves from saved_strip.others");
        assert!(!app.quit_guard_open_dirty(), "switching back does not arm close");
        assert!(!app.nav_stash_gate_dirty(), "switching back does not arm navigation");
        assert!(!app.pending_save_gate_dirty(), "switching back adds no PendingSave");
        assert_eq!(app.open_dirty_variants(), 0, "both persisted cards still match");
    }

    #[test]
    fn pushed_and_deleted_cards_remain_unsaved_strip_work() {
        let mut pushed = persisted_two_card_fixture();
        pushed.variants.push(Variant {
            kind: VariantKind::Generated,
            id: "card-new".into(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: Some(PathBuf::from("D:/masters/new.png")),
            thumb: None,
        });
        pushed.active = 2;
        pushed.recipe = EditRecipe::default();
        assert!(pushed.unsaved_marker_dirty(), "a pushed active card has no baseline");
        assert!(pushed.open_dirty_variants() > 0, "the pushed card is absent on disk");
        assert!(pushed.nav_stash_gate_dirty(), "navigation protects the pushed card");
        assert!(pushed.pending_save_gate_dirty(), "quit Save-all includes the pushed card");

        let mut deleted = persisted_two_card_fixture();
        deleted.variants.remove(1);
        assert!(!deleted.unsaved_marker_dirty(), "the surviving active card itself is clean");
        assert!(deleted.open_dirty_variants() > 0, "a persisted card was deleted live");
        assert!(deleted.nav_stash_gate_dirty(), "navigation protects the deletion");
        assert!(deleted.pending_save_gate_dirty(), "quit Save-all persists the deletion");
        assert!(
            deleted.quit_guard_open_dirty() || deleted.inactive_dirty_variants() > 0,
            "the complete quit guard remains armed for a deletion"
        );
    }

    #[test]
    fn persisted_card_ids_outrank_kind_and_position() {
        let mut app = persisted_two_card_fixture();
        app.variants.swap(0, 1);
        app.active = 0;
        app.recipe = app.variants[0].recipe.clone();

        assert_eq!(
            app.active_baseline().map(|b| b.recipe.contrast),
            Some(18.0),
            "card-y follows its stable id to the saved others entry"
        );
        assert!(!app.unsaved_marker_dirty(), "position cannot override a matching id");
        assert_eq!(
            app.open_dirty_variants(),
            0,
            "the saved-active card also follows its id after becoming background"
        );
    }

    #[test]
    fn legacy_idless_strip_uses_kind_and_position_baselines() {
        let mut app = persisted_two_card_fixture();
        let rec = app.saved_strip.as_mut().unwrap();
        rec.active_id = None;
        rec.others[0].id = None;
        let ctx = egui::Context::default();
        app.switch_variant(1, &ctx);

        assert_eq!(
            app.active_baseline().map(|b| b.recipe.contrast),
            Some(18.0),
            "the id-less fitted entry resolves by its kind and strip position"
        );
        assert!(!app.unsaved_marker_dirty());
        assert!(!app.quit_guard_open_dirty());
        assert!(!app.nav_stash_gate_dirty());
        assert!(!app.pending_save_gate_dirty());
        assert_eq!(app.open_dirty_variants(), 0);
    }

    #[test]
    fn switched_generated_card_uses_its_own_persisted_pixel_origin() {
        let mut app = persisted_two_card_fixture();
        let original_origin = PathBuf::from("D:/masters/original-heal.png");
        let generated_origin = PathBuf::from("D:/masters/generated.png");
        app.variants[0].origin = Some(original_origin.clone());
        app.pixels_on_disk = Some(original_origin);
        app.variants[1].kind = VariantKind::Generated;
        app.variants[1].origin = Some(generated_origin.clone());
        app.variants[1].base = Some(Arc::new(image::DynamicImage::new_rgb8(4, 3)));
        let entry = &mut app.saved_strip.as_mut().unwrap().others[0];
        entry.kind = "generated".into();
        entry.origin = Some(generated_origin);
        let ctx = egui::Context::default();
        app.switch_variant(1, &ctx);

        assert!(!app.unsaved_marker_dirty(), "pixels.json belongs to card X, not card Y");
        assert!(!app.quit_guard_open_dirty(), "the quit pixel half uses Y's entry");
        assert!(!app.nav_stash_gate_dirty(), "the nav pixel half uses Y's entry");
        assert!(!app.pending_save_gate_dirty(), "PendingSave uses Y's entry");

        app.variants[1].origin = Some(PathBuf::from("D:/masters/generated-edited.png"));
        assert!(app.unsaved_marker_dirty(), "changing Y's origin re-arms the marker");
        assert!(app.quit_guard_open_dirty(), "changing Y's origin re-arms close");
        assert!(app.nav_stash_gate_dirty(), "changing Y's origin re-arms navigation");
        assert!(app.pending_save_gate_dirty(), "changing Y's origin re-arms Save-all");
    }

    #[test]
    fn renaming_a_switched_to_card_is_still_unsaved_work() {
        let mut app = persisted_two_card_fixture();
        let ctx = egui::Context::default();
        app.switch_variant(1, &ctx);
        app.variants[1].name = Some("new fitted name".into());

        assert!(!app.unsaved_marker_dirty(), "the canvas develop itself did not change");
        assert!(app.open_dirty_variants() > 0, "the card name's only home changed");
        assert!(app.nav_stash_gate_dirty(), "navigation protects the rename");
        assert!(app.pending_save_gate_dirty(), "Save-all includes the rename");
        assert!(
            app.quit_guard_open_dirty() || app.inactive_dirty_variants() > 0,
            "the complete quit guard remains armed for the rename"
        );
    }

    #[test]
    fn a_saved_retouch_is_not_re_reported_as_unsaved() {
        // The store records the master ABSOLUTIZED while an in-session
        // retouch holds the same file's ./out path RELATIVE, and nothing
        // rewrites the in-memory origin at save time. Comparing the two
        // literally reported a fully saved photo as unsaved: the stash gate
        // armed on the way out, its restore re-lit the ● on the way back, and
        // the quit dialog listed a photo with nothing to save.
        // Unique stem + full scrub: this writes into the real central store.
        let src = std::path::Path::new("D:/library/__autoshade_masterid__.ARW");
        let dev = autoshade::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all("out").unwrap();
        let master = PathBuf::from("out/__autoshade_masterid__.heal.png");
        let _scrub = Scrub(vec![master.clone(), dev.clone()]);
        std::fs::write(&master, b"png").unwrap();
        autoshade::store::write_pixel_source(src, &master, false).unwrap();
        let mk = |origin: PathBuf| Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
            origin: Some(origin),
            thumb: None,
        };
        let mut app = AutoShadeApp {
            src_path: Some(src.to_path_buf()),
            // The RELATIVE spelling, exactly as the retouch handler records it.
            variants: vec![mk(master.clone()), mk(master.clone())],
            pixels_on_disk: Some(std::path::absolute(&master).unwrap()),
            ..Default::default()
        };
        // Not persisted yet (no mirror): the background card IS unsaved work.
        assert_eq!(app.open_dirty_variants(), 1, "premise: unsaved until the strip persists");
        // The persisted record spells the master ABSOLUTIZED (read_variants
        // resolves to the dev dir / absolute), the live variant holds the
        // relative ./out spelling — same_master_opt must bridge them, or a
        // fully saved strip re-reports as unsaved (the original regression,
        // re-expressed against the v0.22 mirror).
        app.saved_strip = Some(autoshade::store::VariantsRecord {
            extra: Default::default(),
            active_id: None,
            active_name: None,
            v: 1,
            active_kind: VariantKind::Original.store_str().to_string(),
            active_pos: 0,
            others: vec![autoshade::store::VariantEntry {
                extra: Default::default(),
                id: None,
                name: None,
                kind: VariantKind::Original.store_str().to_string(),
                recipe: EditRecipe::default(),
                origin: Some(std::path::absolute(&master).unwrap()),
            }],
        });
        assert_eq!(
            app.open_dirty_variants(),
            0,
            "a background variant matching the persisted strip record is not unsaved work"
        );
        app.variants.truncate(1);
        // Mirror follows the trivial strip (as a save would persist it) —
        // this half of the test pins the PIXELS spelling comparison in the
        // stash gate, not strip dirtiness.
        app.saved_strip = None;
        app.open_path(PathBuf::from("D:/library/__autoshade_masterid_next__.ARW"));
        assert!(
            !app.nav_stash.contains_key(src),
            "a saved retouch must not stash as unsaved just because the store spells its master absolutely"
        );
    }

    #[test]
    fn fitted_kind_survives_the_navigation_stash() {
        // Round-9 issue 1 (variant "renames itself"): StashEntry carried the
        // active card only as `generated: bool`, so a 「◭ Reverse-fit」 card
        // came back from navigation as 「▣ 原片」. The three-valued kind must
        // round-trip through the stash write verbatim.
        let (mut app, _scrub) = app_with_masked_photo("stashkind");
        let src = PathBuf::from("D:/library/__autoshade_stashkind__.ARW");
        app.src_path = Some(src.clone());
        app.variants.push(Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Fitted,
            recipe: EditRecipe { contrast: 21.0, ..Default::default() },
            base: None,
            origin: None,
            thumb: None,
        });
        app.active = 1;
        app.recipe = EditRecipe { contrast: 21.0, ..Default::default() };
        app.open_path(PathBuf::from("D:/library/__autoshade_stashkind_next__.ARW"));
        let st = app.nav_stash.get(&src).expect("a dirty strip stashes on nav-away");
        assert_eq!(
            st.kind,
            VariantKind::Fitted,
            "the stash records the ACTIVE card's real kind, not a generated bool"
        );
    }

    #[test]
    fn a_persisted_strip_makes_background_variants_saved_work() {
        // Round-9 issue 3 (quit livelock): after generate→fit the inactive
        // Generated card's origin can NEVER equal the photo's single recorded
        // master, so the old dirty rule held the quit guard armed forever and
        // 「Save all & quit」 bounced closed→cancelled every frame. The fix:
        // dirtiness compares the strip against the persisted variants.json
        // mirror, and persisting the strip is what Ctrl+S / Save-all now do —
        // after it, the guard's `inactive_dirty_variants() > 0` term is 0 and
        // the close goes through.
        let src = std::path::Path::new("D:/library/__autoshade_striprec__.ARW");
        let dev = autoshade::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dev.clone()]);
        let master = dev.join("reimagine-1.png");
        std::fs::write(&master, b"png").unwrap();
        let mut app = AutoShadeApp {
            src_path: Some(src.to_path_buf()),
            variants: vec![
                Variant {
                    id: String::new(),
                    name: None,
                    kind: VariantKind::Original,
                    recipe: EditRecipe::default(),
                    base: None,
                    origin: None,
                    thumb: None,
                },
                Variant {
                    id: String::new(),
                    name: None,
                    kind: VariantKind::Generated,
                    recipe: EditRecipe::default(),
                    base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
                    origin: Some(master.clone()),
                    thumb: None,
                },
                Variant {
                    id: String::new(),
                    name: None,
                    kind: VariantKind::Fitted,
                    recipe: EditRecipe { contrast: 40.0, ..Default::default() },
                    base: None,
                    origin: None,
                    thumb: None,
                },
            ],
            active: 2,
            pixels_on_disk: None,
            ..Default::default()
        };
        // The exact pre-fix trap: unsaved strip, guard armed…
        assert!(app.inactive_dirty_variants() > 0, "premise: the strip is unsaved work");
        // …then the save persists the strip (what Ctrl+S / Save-all do now)…
        app.persist_strip(src).expect("strip persists");
        // …and the guard's orphan term must be genuinely clean: this is the
        // assertion whose absence was the livelock.
        assert_eq!(
            app.inactive_dirty_variants(),
            0,
            "after a save the close guard must let the window go"
        );
        // The record really is on disk and restores the three-valued kinds.
        let rec = autoshade::store::read_variants(src).expect("record on disk");
        assert_eq!(rec.active_kind, "fitted");
        assert_eq!(rec.others.len(), 2);
        assert_eq!(rec.others[0].kind, "original");
        assert_eq!(rec.others[1].kind, "generated");
        assert_eq!(rec.others[1].origin.as_deref(), Some(master.as_path()));
        // Any strip mutation re-arms the protection: deleting the Generated
        // card is unsaved again until the next persist.
        app.variants.remove(1);
        app.active = 1;
        assert!(
            app.inactive_dirty_variants() > 0,
            "a deleted background variant is unsaved work against the mirror"
        );
        // …and switching the active card (identity drift) counts too.
        app.persist_strip(src).expect("re-persist");
        assert_eq!(app.inactive_dirty_variants(), 0);
        app.active = 0;
        app.variants.swap(0, 1);
        assert!(
            app.open_dirty_variants() > 0,
            "active-card identity drift reopens as a different strip"
        );
    }
