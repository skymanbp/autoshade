// One part of the GUI's tests (src/bin/gui/tests.rs includes it): async develop frames, the navigation stash, reading a saved develop, batch export and the projection on open and save.

    #[test]
    fn async_develop_discards_stale_frames_latest_wins() {
        // The whole point of the async scheduler: a frame built for an OLD
        // recipe must be dropped when the live recipe has moved on, and an
        // in-flight guard must prevent a second dispatch. Drives the pure
        // pieces (build_preview + finish_redevelop) with a headless egui ctx —
        // never run_native.
        let (mut app, _scrub) = app_with_masked_photo("latest");
        let ctx = egui::Context::default();
        let base = app.base_preview.clone().unwrap();

        // Dispatch itself (U10): arming must set the in-flight flag and
        // consume the pending edit, and the guard must swallow a SECOND
        // dispatch while one is armed — this is also what makes the
        // "inflight cleared" assertion below non-vacuous (develop_inflight
        // used to be false from construction to the end).
        app.dirty = true;
        app.start_redevelop();
        assert!(app.develop_inflight, "dispatch arms the in-flight flag");
        assert!(!app.dirty, "dispatch consumes the pending edit");
        app.dirty = true;
        app.start_redevelop();
        assert!(app.dirty, "the in-flight guard swallows a second dispatch (edit stays armed)");

        // A matching frame is accepted and bumps the counter + sets the texture.
        let good = build_preview(base.clone(), app.recipe.clone(), false, None, false);
        app.finish_redevelop(&ctx, Ok(good));
        assert_eq!(app.develop_count, 1, "matching frame accepted");
        assert!(app.after_tex.is_some(), "after texture set");
        assert!(!app.develop_inflight, "inflight cleared on completion");

        // Build a frame for the CURRENT recipe, then move the recipe on before
        // it "arrives": the stale frame must be discarded (counter unchanged)
        // AND the pending edit re-armed. `dirty` is cleared first — it was
        // still true from the swallowed dispatch above, so the re-arm
        // assertion used to be vacuously satisfiable (L26).
        let stale = build_preview(base, app.recipe.clone(), false, None, false);
        app.recipe.masks[0].exposure_ev = 1.9; // user kept dragging
        app.dirty = false;
        app.finish_redevelop(&ctx, Ok(stale));
        assert_eq!(app.develop_count, 1, "stale frame (recipe moved) discarded");
        assert!(app.dirty, "a discarded stale frame re-arms the pending edit");

    }

    /// MOTION (2026-10-14): a re-render of the same photo and version updates
    /// the texture IN PLACE with no fade (a slider drag must not flicker); the
    /// first frame of another version lands on a fresh texture and starts a
    /// cross-fade from the old one, which the canvas clears once it is done.
    ///
    /// MUTATION: drop the `self.after_shows == shows` guard (every frame
    /// fades) or the fade arm (no swap fades).
    #[test]
    fn a_swap_fades_in_and_a_rerender_does_not() {
        let (mut app, _scrub) = app_with_masked_photo("fade");
        // The fixture opens no file; the fade keys on (photo, version).
        app.src_path.get_or_insert_with(|| PathBuf::from("fade-photo.arw"));
        let ctx = egui::Context::default();
        let base = app.base_preview.clone().unwrap();
        let frame = |app: &AutoShadeApp| build_preview(base.clone(), app.recipe.clone(), false, None, false);

        app.finish_redevelop(&ctx, Ok(frame(&app)));
        assert!(app.canvas_fade.is_some(), "the first frame fades in from the empty canvas");
        let first = app.after_tex.as_ref().unwrap().id();
        app.canvas_fade = None;

        app.finish_redevelop(&ctx, Ok(frame(&app)));
        assert!(app.canvas_fade.is_none(), "a re-render of the same version must not fade");
        assert_eq!(app.after_tex.as_ref().unwrap().id(), first, "…and updates the texture in place");

        app.active += 1; // the version the frame is for changed
        app.finish_redevelop(&ctx, Ok(frame(&app)));
        let (old, _) = app.canvas_fade.as_ref().expect("a swap fades");
        assert_eq!(old.as_ref().map(|t| t.id()), Some(first), "the outgoing frame is the one faded out");
        assert_ne!(app.after_tex.as_ref().unwrap().id(), first, "the new frame gets its own texture");

        // Past the fade, the canvas drops the outgoing frame.
        app.canvas_fade = Some((None, Instant::now() - Duration::from_secs(1)));
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.after_view(ui, 400.0, 300.0, false));
        });
        assert!(app.canvas_fade.is_none(), "a finished fade is cleared");
    }

    #[test]
    fn mask_rename_flushes_at_entry_points() {
        // A typed-but-uncommitted mask rename (TextEdit still focused, so
        // the panel's lost-focus commit has not run) used to be silently
        // dropped by every entry point except Ctrl+S (U10). Driven here:
        // open_path (flush before the stash) and save_xmp (flush at its
        // head). The close guard's own call site can't be driven headless
        // (it needs a viewport close event through update()); its
        // precondition — a flushed rename reads as UNSAVED — is pinned
        // instead (Codex batch 41).
        let (mut app, _scrub) = app_with_masked_photo("rename");
        app.recipe.masks[0].name = "sky".into();
        app.saved_recipe = app.recipe.clone();
        let old = std::path::PathBuf::from("out/_rename_old.arw");
        app.src_path = Some(old.clone());
        app.mask_name_buf = Some((0, "sky".into(), "sky gradient".into()));

        app.open_path(std::path::PathBuf::from("out/_rename_new.arw"));
        assert_eq!(
            app.recipe.masks[0].name, "sky gradient",
            "open_path itself must flush the pending rename"
        );
        assert!(
            dirty_vs(&app.recipe, &app.saved_recipe),
            "the flushed rename must read as unsaved (the close guard's gate)"
        );
        let stash = app.nav_stash.get(&old).expect("a dirty canvas must be stashed");
        assert_eq!(
            stash.recipe.masks[0].name, "sky gradient",
            "the stash carries the TYPED name, not the pre-focus one"
        );

        // save_xmp's head flush, driven WITHOUT disk side effects: with no
        // photo open save_xmp returns right after its head flush (since
        // 2026-09-13 a generated card no longer refuses — it saves like any
        // other — so the empty-path return is the disk-free door).
        app.src_path = None;
        app.mask_name_buf = Some((0, "sky gradient".into(), "sky gradient 2".into()));
        app.save_xmp();
        assert_eq!(
            app.recipe.masks[0].name, "sky gradient 2",
            "save_xmp itself must flush the pending rename"
        );

    }

    #[test]
    fn a_clean_photo_is_not_stashed_for_another_photos_background_work() {
        // Batch 47's stash gate keyed on a per-photo count; batch 56 widened
        // the count for the quit dialog and the gate silently inherited it —
        // one photo with a dirty background variant then chain-stashed every
        // clean photo the user merely visited, the quit dialog listed them
        // as unsaved, and Save-all wrote sidecars for zero user edits.
        let (mut app, _scrub) = app_with_masked_photo("chainstash");
        app.saved_recipe = app.recipe.clone(); // this canvas is clean
        app.variants[0].origin = None;
        app.pixels_on_disk = None;
        let clean = PathBuf::from("D:/__autoshade_chain__/clean.ARW");
        app.src_path = Some(clean.clone());
        // ANOTHER photo's stash holds a dirty background variant.
        app.nav_stash.insert(
            PathBuf::from("D:/__autoshade_chain__/other.ARW"),
            StashEntry {
                id: String::new(),
                name: None,
                recipe: app.recipe.clone(),
                base: None,
                origin: None,
                kind: VariantKind::Original,
                others: vec![StashedVariant {
                    id: String::new(),
                    name: None,
                    kind: VariantKind::Generated,
                    recipe: EditRecipe { contrast: 33.0, ..Default::default() },
                    base: None,
                    origin: Some(PathBuf::from("out/_chain_gen.png")),
                }],
                active_pos: 0,
            },
        );
        assert_eq!(app.open_dirty_variants(), 0, "this photo's strip is clean");
        assert!(
            app.inactive_dirty_variants() > 0,
            "the quit surfaces still see the other photo's work"
        );
        app.open_path(PathBuf::from("D:/__autoshade_chain__/next.ARW"));
        assert!(
            !app.nav_stash.contains_key(&clean),
            "a clean photo must not be stashed for another photo's background work"
        );

    }

    #[test]
    fn navigation_stash_restores_background_variants() {
        // H4: a dirty BACKGROUND variant must survive nav-away-and-back —
        // the stash used to carry only the active canvas, so the strip
        // collapsed and the background variant's unsaved work died.
        let (mut app, _scrub) = app_with_masked_photo("h4stash");
        let ctx = egui::Context::default();
        let gen_base = Arc::new(image::DynamicImage::new_rgb8(8, 6));
        // An ✎ card: the only kind a develop over AI pixels can be since the
        // immutability rule (a ✨ card carrying edits is split at the door).
        app.variants.push(Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Edited,
            recipe: EditRecipe { contrast: 33.0, ..Default::default() },
            base: Some(gen_base),
            origin: Some(PathBuf::from("out/_h4_gen.png")),
            thumb: None,
        });
        app.saved_recipe = app.recipe.clone(); // active canvas clean
        // Unique stem, load-bearing for THIS path: the synthetic Opened
        // below lands on the fresh arm, whose read_saved_develop migrates
        // cwd ./out legacy sidecars by stem (see the keep-fact test). The
        // nav target's own rename is defence-in-depth — its decode fails
        // into the Err arm, which never reads a sidecar.
        let old = PathBuf::from("D:/__autoshade_h4__/__autoshade_h4_a__.ARW");
        app.src_path = Some(old.clone());
        assert_eq!(app.inactive_dirty_variants(), 1, "premise: background dirty");
        // Navigate away — the stash snapshot is written synchronously.
        app.open_path(PathBuf::from("D:/__autoshade_h4__/__autoshade_h4_b__.ARW"));
        {
            let st = app.nav_stash.get(&old).expect("background work must stash the strip");
            assert_eq!(st.others.len(), 1);
            assert!(matches!(st.others[0].kind, VariantKind::Edited));
        }
        // Drain the (failing) decode of the nav target so its Err cannot
        // interleave with the synthetic return below.
        for _ in 0..200 {
            app.poll_workers(&ctx);
            if !app.busy {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!app.busy, "the failed decode of the nav target must land");
        // Return: simulate the Opened result for the original photo.
        app.src_path = Some(old.clone());
        let base = Arc::new(image::DynamicImage::new_rgb8(8, 6));
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
        assert_eq!(app.variants.len(), 2, "the background variant survives the round trip");
        assert!(
            app.variants
                .iter()
                .any(|v| v.kind == VariantKind::Edited && v.recipe.contrast == 33.0),
            "…with its unsaved recipe intact"
        );
        // The commit claimed "active position included", and nothing pinned
        // it: with the fixture's active == 0 the whole restore could append
        // the active variant at the END and leave self.active at 0 — canvas
        // and index disagreeing — and every assertion above still passed.
        assert_eq!(app.active, 0, "the active position comes back with the strip");
        assert!(
            matches!(app.variants[app.active].kind, VariantKind::Original),
            "…pointing at the variant that WAS active, not merely at a valid index"
        );
        assert_eq!(
            app.variants[app.active].recipe, app.recipe,
            "the canvas and the active slot must agree after a restore"
        );

    }

    #[test]
    fn overlay_skips_rebuild_for_local_effect_sliders() {
        // The coverage-aware key: dragging a mask's Exposure/Temp/color_gains
        // changes WHAT it does, not WHERE — so the full-frame coverage raster
        // must NOT rebuild. Geometry / amount / inversion MUST rebuild.
        let (mut app, _scrub) = app_with_masked_photo("overlay");
        let ctx = egui::Context::default();

        app.refresh_mask_overlay(&ctx);
        assert_eq!(app.overlay_build_count, 1, "first coverage build");
        assert!(app.mask_overlay_tex.is_some());

        // Local effect sliders: no rebuild.
        app.recipe.masks[0].exposure_ev = -2.0;
        app.refresh_mask_overlay(&ctx);
        app.recipe.masks[0].temperature = 55.0;
        app.refresh_mask_overlay(&ctx);
        app.recipe.masks[0].color_gains = Some([1.6, 0.8, 0.5]);
        app.refresh_mask_overlay(&ctx);
        assert_eq!(app.overlay_build_count, 1, "local effect sliders must not rebuild coverage");

        // Amount is coverage-relevant: rebuild.
        app.recipe.masks[0].amount = 0.5;
        app.refresh_mask_overlay(&ctx);
        assert_eq!(app.overlay_build_count, 2, "amount change rebuilds coverage");

        // Inversion is coverage-relevant: rebuild.
        app.recipe.masks[0].inverted = true;
        app.refresh_mask_overlay(&ctx);
        assert_eq!(app.overlay_build_count, 3, "inversion rebuilds coverage");

    }

    #[test]
    fn reorder_move_remap_matches_actual_remove_insert() {
        // The remap returned by reorder_move must agree with what physically
        // happens to a vec under remove(from) + insert(to) — for EVERY
        // element, every (from, insert) pair, including the append slot
        // (insert == len). The two no-op slots are the caller's guard.
        for len in 1..=5usize {
            for from in 0..len {
                for insert in 0..=len {
                    if insert == from || insert == from + 1 {
                        continue; // no-op drop slots, skipped by the GUI
                    }
                    let mut v: Vec<usize> = (0..len).collect();
                    let (to, remap) = reorder_move(from, insert);
                    let m = v.remove(from);
                    v.insert(to, m);
                    for orig in 0..len {
                        let now = v.iter().position(|&x| x == orig).unwrap();
                        assert_eq!(
                            remap(orig),
                            now,
                            "len {len} from {from} insert {insert}: element {orig}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn read_saved_develop_prefers_recipe_json_then_xmp() {
        // The open-path restore contract: no sidecar → Nothing; a NEUTRAL
        // sidecar → NoopOnly (never announced as a restore); XMP-only → the
        // reverse crs import; recipe.json present → preferred (lossless); a
        // DAMAGED recipe.json → Unreadable with the XMP fallback attached
        // (loud degradation, never a silent fall-through); a LEGACY ./out
        // sidecar → migrated into the central store on first read. Unique stem
        // so parallel tests can't race; the whole develop dir is scrubbed
        // before and after (its key is derived from this fake path only).
        let src = std::path::Path::new("D:/library/_sidecar_prio_test.ARW"); // never touched
        let dev = autoshade::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev); // a crashed earlier run may have left files
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::create_dir_all("out").unwrap();
        let rj = autoshade::store::recipe_target(src);
        let xp = autoshade::pipeline::xmp_target(src);
        let legacy_rj = autoshade::store::legacy_recipe(src);
        let _ = std::fs::remove_file(&legacy_rj);
        // Scrub on DROP: a failing assert is exactly the regression case, and
        // the tail cleanup never runs then — leaving fixtures in the real
        // central store, in ./out and beside the fake library path.
        let _scrub = Scrub(vec![dev.clone(), xp.clone(), legacy_rj.clone()]);

        assert!(
            matches!(read_saved_develop(src).saved, SavedDevelop::Nothing),
            "no sidecar → Nothing"
        );

        // A NEUTRAL XMP (foreign file, or ours with nothing set) restores nothing.
        std::fs::write(&xp, autoshade::xmp::recipe_to_xmp(&EditRecipe::default())).unwrap();
        assert!(
            matches!(read_saved_develop(src).saved, SavedDevelop::NoopOnly),
            "a no-op XMP must not claim a restore"
        );

        // A sidecar whose ONLY edit is CORRUPT imports as a no-op — the
        // disclosure list must still surface, or a later save silently
        // overwrites the corrupt original (Codex batch-32 #1).
        let doc = autoshade::xmp::recipe_to_xmp(&EditRecipe::default())
            .replace("crs:Exposure2012=\"0.00\"", "crs:Exposure2012=\"broken\"");
        assert!(doc.contains("broken"), "fixture: the corrupt attribute must exist");
        std::fs::write(&xp, &doc).unwrap();
        let RestoredDevelop { saved, xmp_bad: warn, .. } = read_saved_develop(src);
        assert!(matches!(saved, SavedDevelop::NoopOnly), "corrupt-only still restores nothing");
        assert!(warn.contains(&"Exposure2012".to_string()), "{warn:?}");

        // XMP with real edits → imported through the reverse crs mapping.
        let edited = EditRecipe { contrast: 22.0, ..Default::default() };
        std::fs::write(&xp, autoshade::xmp::recipe_to_xmp(&edited)).unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(src).saved else {
            panic!("an edited XMP restores");
        };
        assert_eq!((r.contrast, kind), (22.0, "XMP"));

        // recipe.json appears → preferred over the XMP.
        let full = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        std::fs::write(&rj, serde_json::to_string(&full).unwrap()).unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(src).saved else {
            panic!("recipe.json restores");
        };
        assert_eq!((r.exposure_ev, kind), (0.5, "recipe.json"));

        // A NEUTRAL recipe.json beside a projection that holds edits: the
        // projection is derived from the recipe and can only disagree by
        // being stale (2026-09-13 — a sidecar left by a deleted reverse-fit
        // card restored its grade over a pristine generated card). The
        // store answers NoopOnly; the projection is not consulted — and the
        // batch resolver draws the same line.
        std::fs::write(&rj, serde_json::to_string(&EditRecipe::default()).unwrap()).unwrap();
        assert!(
            matches!(read_saved_develop(src).saved, SavedDevelop::NoopOnly),
            "a present neutral recipe.json ends the walk"
        );
        let snap = autoshade::store::read_develop_snapshot(src).unwrap();
        assert!(snap.store_xmp.is_some(), "premise: the edited projection is still there");
        assert!(
            crate::export::resolve_snapshot_develop(src, &snap, &mut Vec::new())
                .unwrap()
                .is_none(),
            "the batch answers neutral too"
        );

        // A damaged recipe.json degrades LOUDLY, XMP fallback attached.
        std::fs::write(&rj, "{ not json").unwrap();
        let SavedDevelop::Unreadable { fallback, .. } = read_saved_develop(src).saved else {
            panic!("a damaged recipe.json must be reported, not skipped");
        };
        assert_eq!(fallback.expect("XMP fallback rides along").1, "XMP");

        // A pre-store legacy ./out sidecar is migrated in on FIRST read and
        // then restores from the CENTRAL copy (kind says recipe.json, not the
        // legacy fallback). Migration COPIES and then suppresses the legacy
        // fallback with a tombstone rather than unlinking it: a `./out` name is
        // keyed by stem alone, so two photos with the same stem in different
        // folders share it and deleting one photo's legacy bytes destroyed the
        // other's. A SEPARATE photo path: migrate_legacy memoizes per photo per
        // process, so `src` (already touched above with nothing legacy) would
        // correctly skip the scan — the production contract, not a test bug.
        let src2 = std::path::Path::new("D:/library/_sidecar_prio_test_legacy.ARW");
        let dev2 = autoshade::store::develop_dir(src2);
        let _ = std::fs::remove_dir_all(&dev2);
        let legacy_rj2 =
            PathBuf::from("out").join("_sidecar_prio_test_legacy.recipe.json");
        let _scrub2 = Scrub(vec![dev2.clone(), legacy_rj2.clone()]);
        let legacy = EditRecipe { contrast: -11.0, ..Default::default() };
        std::fs::write(&legacy_rj2, serde_json::to_string(&legacy).unwrap()).unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(src2).saved else {
            panic!("a legacy ./out recipe restores");
        };
        assert_eq!((r.contrast, kind), (-11.0, "recipe.json"));
        assert!(
            legacy_rj2.exists(),
            "the stem-keyed legacy file is COPIED, never unlinked — another \
             photo with the same stem may still need it"
        );
        assert!(
            autoshade::store::recipe_target(src2).exists(),
            "…and now lives in the central store"
        );
    }

    /// The reopen note's claim is about the WHOLE saved develop, so it must
    /// be decided from the whole store. Deciding it from the active card
    /// alone reported "holds no effective edits" on photos whose saved work
    /// lives in a background variant card or in baked pixels (user report,
    /// 2026-08-25). recipe.json is never rewritten by the fix — only the
    /// reading side widened.
    #[test]
    fn a_neutral_active_card_does_not_deny_strip_or_pixel_work() {
        let src = std::path::Path::new("D:/library/_noop_note_scope_test.ARW"); // never touched
        let dev = autoshade::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dev.clone()]);
        let rj = autoshade::store::recipe_target(src);
        std::fs::write(&rj, serde_json::to_string(&EditRecipe::default()).unwrap()).unwrap();
        let recipe_bytes = std::fs::read(&rj).unwrap();
        assert!(
            matches!(read_saved_develop(src).saved, SavedDevelop::NoopOnly),
            "premise: a neutral active card restores as NoopOnly"
        );

        // Nothing anywhere else — the old sentence is TRUE and must stay.
        let plain = resolve_saved_develop(Lang::En, SavedDevelop::NoopOnly, true, Some(src));
        assert_eq!(
            plain.open_note.as_deref(),
            Some("a saved develop exists but holds no effective edits"),
            "a genuinely empty develop keeps the plain sentence"
        );

        // A background card with a real edit: the same reopen used to deny
        // the work. The note must now say WHERE the edits live instead.
        let strip = autoshade::store::VariantsRecord {
            v: 1,
            active_kind: "original".into(),
            active_pos: 0,
            others: vec![autoshade::store::VariantEntry {
                kind: "fitted".into(),
                recipe: EditRecipe { exposure_ev: 1.0, ..Default::default() },
                origin: None,
                id: None,
                name: None,
                extra: Default::default(),
            }],
            active_id: None,
            active_name: None,
            extra: Default::default(),
        };
        autoshade::store::write_variants(src, &strip).unwrap();
        let noted = resolve_saved_develop(Lang::En, SavedDevelop::NoopOnly, true, Some(src));
        let note = noted.open_note.as_deref().expect("the strip work must be named");
        assert!(note.contains("1 background variant"), "{note}");
        assert!(!note.contains("no effective edits"), "the false denial is back: {note}");

        // A strip whose background cards are themselves neutral holds no
        // work — the plain sentence returns.
        let neutral_strip = autoshade::store::VariantsRecord {
            others: vec![autoshade::store::VariantEntry {
                kind: "original".into(),
                recipe: EditRecipe::default(),
                origin: None,
                id: None,
                name: None,
                extra: Default::default(),
            }],
            ..strip.clone()
        };
        autoshade::store::write_variants(src, &neutral_strip).unwrap();
        let plain2 = resolve_saved_develop(Lang::En, SavedDevelop::NoopOnly, true, Some(src));
        assert_eq!(
            plain2.open_note.as_deref(),
            Some("a saved develop exists but holds no effective edits"),
            "neutral background cards are not work"
        );

        // A recorded baked pixel master: the canvas shows the work, so
        // there is no neutral-looking open to explain — no note at all.
        std::fs::write(autoshade::store::pixel_source_path(src), "{}").unwrap();
        let silent = resolve_saved_develop(Lang::En, SavedDevelop::NoopOnly, true, Some(src));
        assert_eq!(silent.open_note, None, "{:?}", silent.open_note);

        // The fix is READ-side only: recipe.json — the active card's sole
        // authority — is byte-identical throughout.
        assert_eq!(std::fs::read(&rj).unwrap(), recipe_bytes, "recipe.json was rewritten");
    }

    #[test]
    fn read_saved_develop_lets_a_newer_lightroom_sidecar_win() {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-lr-sidecar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_lr_probe.ARW");
        std::fs::write(&src, b"raw").unwrap();
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]); // …even if an assert fires
        // The stored develop (older).
        let saved = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        std::fs::write(
            autoshade::store::recipe_target(&src),
            serde_json::to_string(&saved).unwrap(),
        )
        .unwrap();
        // Lightroom's own sidecar beside the RAW, stamped NEWER (set, not
        // slept for).
        let lr = dir.join("_gui_lr_probe.xmp");
        std::fs::write(
            &lr,
            autoshade::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() }),
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
        let SavedDevelop::Restored(r, kind) = read_saved_develop(&src).saved else {
            panic!("a newer Lightroom sidecar must restore");
        };
        assert_eq!(r.contrast, 33.0, "the newer Lightroom edit wins over the stored develop");
        assert!(kind.starts_with("XMP"), "stamped like every XMP restore: {kind}");
        assert!(kind.contains("Lightroom"), "the source is disclosed: {kind}");
        // The store copy is untouched — only an explicit save adopts it.
        let kept: EditRecipe = serde_json::from_str(
            &std::fs::read_to_string(autoshade::store::recipe_target(&src)).unwrap(),
        )
        .unwrap();
        assert_eq!(kept.exposure_ev, 0.5);
    }

    /// L13#1 anti-drift gate: the batch renderer's snapshot resolution must
    /// answer EXACTLY what opening the photo restores — XMP-only develops
    /// included, and a newer Lightroom sidecar out-ranking the recipe
    /// included. The open result is stamped the way the open caller stamps
    /// it (stamp_calibration) before comparing.
    #[test]
    fn batch_export_resolves_the_same_develop_the_open_path_restores() {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-batch-antidrift-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_batch_drift.ARW");
        std::fs::write(&src, b"raw").unwrap();
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]);
        let stamp_like_open = |mut r: EditRecipe| {
            let (ask, ast) = autoshade::pipeline::fresh_as_shot_wb(&src);
            stamp_calibration(
                &mut r,
                &autoshade::pipeline::photo_base_knots(&src),
                &autoshade::pipeline::fresh_lens_profile(&src),
                ask.zip(ast),
            );
            r
        };

        // Phase 1: an XMP-ONLY develop (the store projection).
        std::fs::write(
            autoshade::store::xmp_target(&src),
            autoshade::xmp::recipe_to_xmp(&EditRecipe { contrast: 21.0, ..Default::default() }),
        )
        .unwrap();
        let snap = autoshade::store::read_develop_snapshot(&src).unwrap();
        let (batch, batch_kind) = crate::export::resolve_snapshot_develop(&src, &snap, &mut Vec::new())
            .unwrap()
            .expect("an XMP-only develop must resolve for the batch");
        let SavedDevelop::Restored(open, open_kind) = read_saved_develop(&src).saved else {
            panic!("the open path restores the XMP-only develop");
        };
        assert_eq!(batch_kind, open_kind);
        let mut open = stamp_like_open(open);
        open.clamp();
        assert_eq!(batch, open, "batch and open answer the same develop (XMP-only)");

        // Phase 2: recipe.json exists but a NEWER Lightroom sidecar wins.
        std::fs::write(
            autoshade::store::recipe_target(&src),
            serde_json::to_string(&EditRecipe { exposure_ev: 0.5, ..Default::default() })
                .unwrap(),
        )
        .unwrap();
        let lr = src.with_extension("xmp");
        std::fs::write(
            &lr,
            autoshade::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() }),
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
        let snap = autoshade::store::read_develop_snapshot(&src).unwrap();
        let (batch, batch_kind) = crate::export::resolve_snapshot_develop(&src, &snap, &mut Vec::new())
            .unwrap()
            .expect("the Lightroom develop must resolve for the batch");
        assert!(batch_kind.contains("Lightroom"), "{batch_kind}");
        let SavedDevelop::Restored(open, open_kind) = read_saved_develop(&src).saved else {
            panic!("the open path restores the Lightroom develop");
        };
        assert_eq!(batch_kind, open_kind);
        let mut open = stamp_like_open(open);
        open.clamp();
        assert_eq!(batch, open, "batch and open answer the same develop (LR newer)");
        assert_eq!(batch.contrast, 33.0, "and it is the Lightroom edit");
    }

    /// A minimal little-endian TIFF whose root IFD carries one XMP entry
    /// (tag 0x02BC, type BYTE) — the decode-side reader has its own copy;
    /// bin tests cannot reach lib test code.
    fn tiff_with_xmp(payload: &[u8]) -> Vec<u8> {
        let mut f: Vec<u8> = Vec::new();
        f.extend(b"II");
        f.extend(42u16.to_le_bytes());
        f.extend(8u32.to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(0x02BCu16.to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend((payload.len() as u32).to_le_bytes());
        if payload.len() <= 4 {
            let mut v = [0u8; 4];
            v[..payload.len()].copy_from_slice(payload);
            f.extend(v);
        } else {
            f.extend(26u32.to_le_bytes());
        }
        f.extend(0u32.to_le_bytes());
        if payload.len() > 4 {
            f.extend(payload);
        }
        f
    }

    /// L05#6: a DNG whose develop Lightroom baked INTO the file used to open
    /// neutral with no word — the packet is now the strictly LOWEST-priority
    /// restore source, on the open path and the batch snapshot alike (the
    /// L13 rule: the surfaces answer one develop).
    /// The user's exact on-disk state of 2026-09-13, opened: an AI-generated
    /// card active (`pixels.json` generated, `recipe.json` neutral with the
    /// RAW's calibration), the ▣ card's develop in `variants.json`, and a
    /// `<stem>.xmp` projection left by a reverse-fit card that was deleted
    /// before the last save (+90 saturation, −1.3 EV). The open path walked
    /// past the neutral recipe.json into that projection and rendered the
    /// generated pixels through the dead card's grade. The canvas must be
    /// the pristine card: neutral, clean, on the AI pixels.
    #[test]
    fn a_stale_projection_never_cooks_a_pristine_generated_card_on_open() {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-gui-stale-projection-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A fake RAW path: the store keys by path, and nothing reads the file.
        let src = dir.join("_gui_stale_projection.ARW");
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]);
        let master = dir.join("_gui_stale_projection.reimagine.png");
        image::DynamicImage::new_rgb8(6, 4).save(&master).unwrap();
        // recipe.json: neutral apart from the RAW's calibration (the disk
        // form of a generated card's develop).
        std::fs::write(
            autoshade::store::recipe_target(&src),
            serde_json::to_string(&EditRecipe {
                base_curve: vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]],
                as_shot_k: Some(5653.0),
                as_shot_tint: Some(0.0),
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        autoshade::store::write_pixel_source(&src, &master, true).unwrap();
        autoshade::store::write_variants(
            &src,
            &autoshade::store::VariantsRecord {
                extra: Default::default(),
                v: 1,
                active_kind: "generated".into(),
                active_pos: 1,
                active_id: Some("gen-1".into()),
                active_name: None,
                others: vec![autoshade::store::VariantEntry {
                    extra: Default::default(),
                    kind: "original".into(),
                    recipe: EditRecipe { contrast: 4.0, shadows: 28.0, ..Default::default() },
                    origin: None,
                    id: Some("original".into()),
                    name: None,
                }],
            },
        )
        .unwrap();
        // The dead card's projection.
        std::fs::write(
            autoshade::pipeline::xmp_target(&src),
            autoshade::xmp::recipe_to_xmp(&EditRecipe {
                saturation: 90.0,
                exposure_ev: -1.3,
                ..Default::default()
            }),
        )
        .unwrap();

        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        let ctx = egui::Context::default();
        let ai_px = Arc::new(image::DynamicImage::new_rgb8(6, 4));
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                Arc::new(image::DynamicImage::new_rgb8(6, 4)),
                vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]],
                Default::default(),
                Some((5653.0, 0.0)),
                Some((ai_px.clone(), master.clone(), true)),
                (1280, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(&ctx);

        assert_eq!(app.variants.len(), 2, "the ▣ card and the ✨ card");
        assert_eq!(app.active, 1);
        assert_eq!(app.variants[1].kind, VariantKind::Generated);
        assert!(
            app.variants[1].base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &ai_px)),
            "the canvas sits on the AI pixels"
        );
        assert!(
            app.recipe.is_noop(),
            "the pristine card, not the deleted reverse-fit's grade: {:?}",
            (app.recipe.saturation, app.recipe.exposure_ev)
        );
        assert_eq!(app.recipe.saturation, 0.0);
        assert!(app.recipe.base_curve.is_empty(), "calibration stripped on AI pixels");
        assert!(app.saved_recipe.is_noop(), "the ● baseline agrees");
        assert!(!app.unsaved_marker_dirty(), "a clean open: {}", app.status);
        assert!(
            !app.status.contains("(XMP)"),
            "nothing was restored from the projection: {}",
            app.status
        );
        assert_eq!(app.variants[0].recipe.contrast, 4.0, "the ▣ card keeps its own develop");
    }

    /// Ctrl+S publishes the Lightroom projection in the recipe's own
    /// generation — a projection standing from an earlier develop is
    /// replaced by the commit, not by a write that runs after it.
    #[test]
    fn ctrl_s_publishes_the_projection_in_the_recipes_generation() {
        let src = std::path::Path::new("D:/library/_gui_ctrl_s_projection.ARW"); // never touched
        let dev = autoshade::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dev.clone()]);
        let xp = autoshade::pipeline::xmp_target(src);
        std::fs::write(
            &xp,
            autoshade::xmp::recipe_to_xmp(&EditRecipe { saturation: 90.0, ..Default::default() }),
        )
        .unwrap();
        let mut app = AutoShadeApp {
            src_path: Some(src.to_path_buf()),
            recipe: EditRecipe { contrast: 7.0, ..Default::default() },
            ..Default::default()
        };
        app.save_xmp();
        assert!(autoshade::store::recipe_target(src).exists(), "{}", app.status);
        let back = autoshade::xmp::xmp_to_recipe(&std::fs::read_to_string(&xp).unwrap());
        assert_eq!(back.contrast, 7.0, "the projection is this generation's");
        assert_eq!(back.saturation, 0.0, "…and the stale one is gone");
        assert!(app.status.starts_with("XMP + recipe saved"), "{}", app.status);
        assert!(!dev.join(".commit").exists(), "the stage is consumed");
    }

    /// A photo opened onto its AI card: a temp-dir fake RAW (the store keys
    /// by path; nothing reads the file), a real 6×4 reimagine master, the
    /// RAW's calibration saved in recipe.json around `active_develop` (the
    /// disk form of the active card's develop), and the record [▣ (contrast
    /// 4), ✨ "sky" (id gen-1)]. Returns the app, its context, the dir, the
    /// RAW, the master and the AI preview Arc; the scrub guard removes both
    /// dirs. The premise assertions belong to each test, since a pristine
    /// develop opens as [▣, ✨] and one carrying edits is split at the door.
    fn ai_card_fixture(
        tag: &str,
        active_develop: EditRecipe,
    ) -> (AutoShadeApp, egui::Context, PathBuf, PathBuf, PathBuf, Arc<image::DynamicImage>, Scrub) {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-gui-ai-card-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join(format!("_gui_ai_card_{}.ARW", tag.replace('-', "_")));
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let scrub = Scrub(vec![dir.clone(), dev.clone()]);
        let master = dir.join("reimagine.png");
        image::DynamicImage::new_rgb8(6, 4).save(&master).unwrap();
        let knots = vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]];
        std::fs::write(
            autoshade::store::recipe_target(&src),
            serde_json::to_string(&EditRecipe {
                base_curve: knots.clone(),
                as_shot_k: Some(5653.0),
                as_shot_tint: Some(0.0),
                ..active_develop
            })
            .unwrap(),
        )
        .unwrap();
        autoshade::store::write_pixel_source(&src, &master, true).unwrap();
        autoshade::store::write_variants(
            &src,
            &autoshade::store::VariantsRecord {
                extra: Default::default(),
                v: 1,
                active_kind: "generated".into(),
                active_pos: 1,
                active_id: Some("gen-1".into()),
                active_name: Some("sky".into()),
                others: vec![autoshade::store::VariantEntry {
                    extra: Default::default(),
                    kind: "original".into(),
                    recipe: EditRecipe { contrast: 4.0, ..Default::default() },
                    origin: None,
                    id: Some("original".into()),
                    name: None,
                }],
            },
        )
        .unwrap();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        let ctx = egui::Context::default();
        let ai_px = Arc::new(image::DynamicImage::new_rgb8(6, 4));
        open_onto_ai_card(&mut app, &ctx, &ai_px, &master);
        (app, ctx, dir, src, master, ai_px, scrub)
    }

    /// The synthetic `Msg::Opened` for [`ai_card_fixture`]'s photo — the RAW's
    /// calibration plus the decoded generated master — landed through the
    /// real door.
    fn open_onto_ai_card(
        app: &mut AutoShadeApp,
        ctx: &egui::Context,
        ai_px: &Arc<image::DynamicImage>,
        master: &std::path::Path,
    ) {
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                Arc::new(image::DynamicImage::new_rgb8(6, 4)),
                vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]],
                Default::default(),
                Some((5653.0, 0.0)),
                Some((ai_px.clone(), master.to_path_buf(), true)),
                (1280, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(ctx);
    }

    fn strip_kinds(app: &AutoShadeApp) -> Vec<VariantKind> {
        app.variants.iter().map(|v| v.kind).collect()
    }

    fn saved_strip_of(src: &std::path::Path) -> autoshade::store::VariantsRecord {
        match autoshade::store::read_variants_checked(src) {
            autoshade::store::VariantsRead::Strip(r) => r,
            _ => panic!("a strip record is on disk"),
        }
    }
