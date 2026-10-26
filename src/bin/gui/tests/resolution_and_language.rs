// One part of the GUI's tests (src/bin/gui/tests.rs includes it): bake resolution disclosures, history identity, the decode caches, the mask brush session and the two languages.

    #[test]
    fn a_bake_follows_the_canvas_resolution_not_the_preference() {
        // A background baked variant keeps its own resolution while
        // preview_edge moves on (switching variants cannot re-decode a
        // master), so baking at the preference installed a new-edge raster
        // under an old-edge canvas — the frame jumped mid-retouch.
        //
        // COVERAGE BOUND, stated instead of implied: this pins the RULER, not
        // the five call sites that use it — the bake starters spawn network
        // workers and cannot run headless, so reverting one of those
        // single-expression uses would not fail here. Reviewed by reading.
        let mut app = AutoShadeApp { preview_edge: 4096, ..Default::default() };
        assert_eq!(app.canvas_edge(), 4096, "no canvas yet: the preference is all there is");
        app.base_preview = Some(Arc::new(image::DynamicImage::new_rgb8(1280, 853)));
        assert_eq!(app.canvas_edge(), 1280, "a baked canvas bakes at its OWN resolution");
        app.base_preview = Some(Arc::new(image::DynamicImage::new_rgb8(853, 1280)));
        assert_eq!(app.canvas_edge(), 1280, "…measured on the long edge, portrait included");
    }

    #[test]
    fn a_refusal_never_prescribes_a_remedy_that_cannot_work() {
        // A master that WAS recorded but no longer resolves (moved, deleted,
        // unreadable) is not cured by saving — saving re-records the same
        // broken link and the refusal repeats forever. The two causes must
        // get different words.
        let src = std::path::Path::new("D:/library/__autoshade_gonemaster__.ARW");
        let dev = autoshade::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dev.clone()]);
        let gone = dev.join("gone-master.png");
        std::fs::write(&gone, b"png").unwrap();
        autoshade::store::write_pixel_source(src, &gone, false).unwrap();
        std::fs::remove_file(&gone).unwrap(); // recorded, and now unresolvable
        assert!(autoshade::store::has_pixel_source(src), "premise: a record survives");
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp {
            src_path: Some(src.to_path_buf()),
            variants: vec![Variant {
                id: String::new(),
                name: None,
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
                origin: Some(gone.clone()),
                thumb: None,
            }],
            preview_edge: 4096,
            edge_before_flight: Some(1280),
            keep_recipe: true,
            open_same_path: true,
            open_in_flight: true,
            ..Default::default()
        };
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                Arc::new(image::DynamicImage::new_rgb8(8, 6)),
                Vec::new(),
                Default::default(),
                None,
                None,
                (4096, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.preview_edge, 1280, "premise: the switch is refused");
        assert!(
            app.status.contains("no longer on disk"),
            "the missing-master case names its own cause: {}",
            app.status
        );
        assert!(
            !app.status.contains("save the photo"),
            "…and never prescribes a save that cannot restore a master that is gone: {}",
            app.status
        );
        // ...and a GENERATED canvas is not told to save either: Ctrl+S refuses
        // a generated variant outright, so that remedy is unreachable for it.
        let present = dev.join("present-master.png");
        std::fs::write(&present, b"png").unwrap();
        let mut app = AutoShadeApp {
            src_path: Some(src.to_path_buf()),
            variants: vec![Variant {
                id: String::new(),
                name: None,
                kind: VariantKind::Generated,
                recipe: EditRecipe::default(),
                base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
                origin: Some(present.clone()),
                thumb: None,
            }],
            preview_edge: 4096,
            edge_before_flight: Some(1280),
            keep_recipe: true,
            open_same_path: true,
            open_in_flight: true,
            ..Default::default()
        };
        app.tx
            .send(Msg::Opened(Box::new(Ok((
                Arc::new(image::DynamicImage::new_rgb8(8, 6)),
                Vec::new(),
                Default::default(),
                None,
                None,
                (4096, None, None),
                None,
            )))))
            .unwrap();
        app.poll_workers(&ctx);
        assert!(
            app.status.contains("generated variant"),
            "a generated canvas names its own cause: {}",
            app.status
        );
        assert!(
            !app.status.contains("save the photo"),
            "…and is not told to save, which Ctrl+S refuses for it: {}",
            app.status
        );
    }

    #[test]
    fn only_a_baked_canvas_discloses_its_own_resolution() {
        // The disclosure must key on the canvas HOLDING baked pixels, not on
        // the sizes differing: a RAW smaller than the preference decodes
        // un-upscaled, and calling that source canvas a stale bake is a lie on
        // the surface that never expires.
        let ctx = egui::Context::default();
        let src = |base: Option<Arc<image::DynamicImage>>| Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base,
            origin: None,
            thumb: None,
        };
        // (1) source-based canvas, sensor below the preference: no disclosure.
        let mut app = AutoShadeApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            variants: vec![src(None), src(None)],
            ..Default::default()
        };
        app.switch_variant(1, &ctx);
        assert_eq!(app.canvas_edge(), 800, "premise: the canvas is below the preference");
        assert!(
            !app.status.contains("baked"),
            "a source-based canvas is not a bake: {}",
            app.status
        );
        // (2) the same size gap, but the canvas really is a baked raster.
        let mut app = AutoShadeApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            variants: vec![
                src(None),
                src(Some(Arc::new(image::DynamicImage::new_rgb8(640, 480)))),
            ],
            ..Default::default()
        };
        app.switch_variant(1, &ctx);
        assert!(
            app.status.contains("640px"),
            "a baked canvas says which resolution it kept: {}",
            app.status
        );
        // (3) a baked canvas at the resolution the source DELIVERS — a
        // sub-preference sensor, freshly healed. Measured against the raw
        // preference this fresh bake was called a stale one.
        let mut app = AutoShadeApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            variants: vec![
                src(None),
                src(Some(Arc::new(image::DynamicImage::new_rgb8(800, 600)))),
            ],
            ..Default::default()
        };
        app.switch_variant(1, &ctx);
        assert!(
            !app.status.contains("800px"),
            "a bake at the delivered resolution is not a stale bake: {}",
            app.status
        );
        // (4) …and a baked canvas at the PREFERENCE on that same photo — a
        // recorded master re-decoded on open, which `thumbnail` UPSCALES to
        // exactly the preference. Measured against the delivered edge alone,
        // this said "stays at 1280px … not the preview preference" while the
        // preference was 1280.
        let mut app = AutoShadeApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            variants: vec![
                src(None),
                src(Some(Arc::new(image::DynamicImage::new_rgb8(1280, 960)))),
            ],
            ..Default::default()
        };
        app.switch_variant(1, &ctx);
        assert!(
            !app.status.contains("1280px"),
            "a canvas at the preference is not a stale bake either: {}",
            app.status
        );
        // (A third value — neither the preference nor what the source
        // delivers — is case (2) above: 640 under a 1280 preference on an 800
        // source. A separate case here would have been that fixture again.)
    }

    #[test]
    fn an_undo_to_a_superseded_master_discloses_its_resolution() {
        // Undo/redo wrote no status at all, so the quietest door onto the same
        // canvas/preference disagreement said nothing.
        let ctx = egui::Context::default();
        let superseded = Arc::new(image::DynamicImage::new_rgb8(640, 480));
        // 800 is what the SOURCE delivers here, and it is NOT the preference
        // (1280): the retraction therefore comes from the delivered-edge arm
        // specifically, and the exact-string assert below is what turns that
        // into a pin. With the two equal — and with a substring assert — this
        // test passed under either arm and pinned neither.
        let current = Arc::new(image::DynamicImage::new_rgb8(800, 600));
        let step = |base: &Arc<image::DynamicImage>, tag: &str| UndoStep {
            recipe: EditRecipe::default(),
            base: Some(base.clone()),
            origin: Some(PathBuf::from(format!("out/_{tag}.png"))),
            keep_skin: None,
        };
        let mut app = AutoShadeApp {
            preview_edge: 1280,
            // A REAL source: without it the ruler took its `unwrap_or`
            // fallback and never the delivered-edge path this test covers.
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(800, 600))),
            base_preview: Some(current.clone()),
            variants: vec![Variant {
                id: String::new(),
                name: None,
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base: Some(current.clone()),
                origin: Some(PathBuf::from("out/_current.png")),
                thumb: None,
            }],
            committed: step(&current, "current"),
            ..Default::default()
        };
        app.undo_stack.push(step(&superseded, "superseded"));
        app.undo(&ctx);
        assert!(
            app.base_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &superseded)),
            "premise: the superseded raster is back on the canvas"
        );
        assert!(
            app.status.contains("640px"),
            "the undo door discloses the resolution it restored: {}",
            app.status
        );
        // ...and REDOING back to a reachable canvas must RETRACT it: a
        // disclosure left standing is false in both halves.
        app.redo(&ctx);
        assert!(
            app.base_preview.as_ref().is_some_and(|b| Arc::ptr_eq(b, &current)),
            "premise: the matching-edge raster is back"
        );
        // EXACT, not `!contains("640px")`: a negative substring cannot tell
        // "no claim" from "a different claim", so it survived deleting the
        // delivered-edge conjunct (the redo then claims 800px, which contains
        // no "640px"). This is what makes the fixture pin that arm.
        assert_eq!(
            app.status, "restored the canvas pixels",
            "the claim is retracted when the disagreement ends, not merely reworded"
        );
    }

    #[test]
    fn deleting_the_active_variant_discloses_the_canvas_it_lands_on() {
        // The fourth door: deleting the active variant re-anchors onto a
        // BACKGROUND variant, whose baked raster the preference cannot
        // re-decode — silently, and carrying the deleted canvas's own
        // disclosure forward.
        let ctx = egui::Context::default();
        let baked = |w: u32, h: u32, tag: &str| Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Generated,
            recipe: EditRecipe::default(),
            base: Some(Arc::new(image::DynamicImage::new_rgb8(w, h))),
            origin: Some(PathBuf::from(format!("out/_{tag}.png"))),
            thumb: None,
        };
        let mut app = AutoShadeApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(1280, 853))),
            variants: vec![baked(640, 480, "keep"), baked(1280, 853, "drop")],
            active: 1,
            ..Default::default()
        };
        // R24-4: the ✕ ARMS first — a deleted card cannot be brought back, so
        // the first call asks and changes nothing about the strip.
        app.delete_variant(1, &ctx);
        assert_eq!(app.variants.len(), 2, "the arming click deletes nothing");
        assert_eq!(app.active, 1, "…and does not re-anchor the strip either");
        assert_eq!(
            app.variant_delete_confirm,
            Some(1),
            "the arming click names the card the next one deletes"
        );
        app.delete_variant(1, &ctx);
        assert_eq!(app.active, 0, "premise: the strip re-anchored");
        assert_eq!(app.variant_delete_confirm, None, "the arm is spent");
        assert!(
            app.status.contains("640px"),
            "the landing canvas's resolution is disclosed: {}",
            app.status
        );
        // ...and landing on a canvas the preference DOES reach says so plainly.
        let mut app = AutoShadeApp {
            preview_edge: 1280,
            source_preview: Some(Arc::new(image::DynamicImage::new_rgb8(1280, 853))),
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
                baked(1280, 853, "drop2"),
            ],
            active: 1,
            ..Default::default()
        };
        app.delete_variant(1, &ctx); // arm
        app.delete_variant(1, &ctx); // …and confirm
        assert!(
            app.status.contains("variant removed"),
            "no disagreement, no claim: {}",
            app.status
        );
    }

    #[test]
    fn history_pixel_identity_compares_the_master_not_its_spelling() {
        // The last comparison the batch-80 sweep left on `==`.
        let base = Arc::new(image::DynamicImage::new_rgb8(4, 3));
        let rel = UndoStep {
            recipe: EditRecipe::default(),
            base: Some(base.clone()),
            origin: Some(PathBuf::from("out/_spelling.png")),
            keep_skin: None,
        };
        let abs = UndoStep {
            recipe: EditRecipe::default(),
            base: Some(base),
            origin: Some(std::path::absolute("out/_spelling.png").unwrap()),
            keep_skin: None,
        };
        assert!(rel.same_pixels(&abs), "one master, two spellings, one pixel identity");
        let other = UndoStep {
            recipe: EditRecipe::default(),
            base: rel.base.clone(),
            origin: Some(PathBuf::from("out/_spelling-2.png")),
            keep_skin: None,
        };
        assert!(!rel.same_pixels(&other), "…but a DIFFERENT master still differs");
    }

    #[test]
    fn a_failed_open_takes_the_dead_photos_history_with_it() {
        // The Err arm dropped src_path, the strip and the canvas but left the
        // undo stack standing, so an undo afterwards restored a canvas that no
        // longer existed — the keyboard path reached it because it checked
        // only `busy`. Gating the key is the guard; clearing the stack is the
        // reason there is nothing to guard. (The photo's unsaved work is not
        // lost: open_path stashed it before the flight.)
        let mut app = AutoShadeApp::default();
        let ctx = egui::Context::default();
        app.src_path = Some(PathBuf::from("D:/__autoshade_deadopen__/__autoshade_deadopen__.ARW"));
        app.variants = vec![Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
            origin: Some(PathBuf::from("out/_deadopen.png")),
            thumb: None,
        }];
        let step = || UndoStep {
            recipe: EditRecipe { contrast: 11.0, ..Default::default() },
            base: None,
            origin: None,
            keep_skin: None,
        };
        app.undo_stack.push(step());
        app.redo_stack.push(step());
        app.tx
            .send(Msg::Opened(Box::new(Err(anyhow::anyhow!("decode failed")))))
            .unwrap();
        app.poll_workers(&ctx);
        assert!(app.src_path.is_none(), "premise: a FRESH open failed into the no-photo state");
        assert!(
            app.undo_stack.is_empty() && app.redo_stack.is_empty(),
            "the dead photo's history goes with it"
        );
        // …so an undo here cannot reinstate the recipe of a photo that is gone.
        app.undo(&ctx);
        assert_eq!(app.recipe.contrast, 0.0, "nothing left to restore");
        assert!(app.variants.is_empty(), "…and no canvas to restore it onto");
    }

    #[test]
    fn decoded_base_lru_hits_by_key_and_evicts_least_recent() {
        // Nonexistent paths give mtime None on both sides, which must match
        // itself (the cache still works where metadata is unavailable).
        let mut app = AutoShadeApp::default();
        let base = Arc::new(image::DynamicImage::new_rgb8(4, 3));
        let knots: Vec<[f32; 2]> = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        let lens = autoshade::recipe::LensProfile {
            vignette: vec![1.0, 1.2],
            vignette_on: true,
            ..Default::default()
        };
        let p = std::path::Path::new("D:/__autoshade_nonexistent__/x.ARW");
        // The entry's edge (2560) deliberately DIFFERS from app.preview_edge
        // (1280): the key must come from the worker's pre-read ident riding
        // OpenedBase — a body that re-derives the EDGE from app state files
        // under 1280 and both edge asserts flip. (The stamp halves are
        // None-on-None for these nonexistent fixtures, so only the edge
        // component is pinned here; identity-before-read for the stamps is
        // review-enforced and recorded in the remember_base doc.)
        app.remember_base(
            p,
            &(
                base.clone(),
                knots.clone(),
                lens.clone(),
                Some((4830.0, 6.0)),
                None,
                (2560, None, None),
                None,
            ),
        );
        let hit = app.cached_base(p, 2560);
        assert!(hit.is_some(), "same path + the IDENT's edge hits");
        let hit = hit.unwrap();
        assert_eq!(hit.1, knots, "base-look knots ride the cache entry");
        assert_eq!(hit.2, lens, "the lens profile rides the cache entry too");
        assert_eq!(hit.3, Some((4830.0, 6.0)), "the as-shot anchor rides the cache entry too");
        assert!(
            app.cached_base(p, 1280).is_none(),
            "the key's edge comes from the pre-read ident, not preview_edge"
        );
        // Filling past the cap evicts the least-recently-used entry.
        let others: Vec<std::path::PathBuf> = (0..BASE_CACHE_CAP)
            .map(|i| std::path::PathBuf::from(format!("D:/__autoshade_nonexistent__/{i}.ARW")))
            .collect();
        for o in &others {
            app.remember_base(
                o,
                &(base.clone(), Vec::new(), Default::default(), None, None, (1280, None, None), None),
            );
        }
        assert!(app.cached_base(p, 2560).is_none(), "least-recent evicted at cap");
        assert!(app.cached_base(&others[1], 1280).is_some(), "newer entries survive");
        assert!(app.base_cache.len() <= BASE_CACHE_CAP, "cap holds");
    }

    /// L02: the cold-variant master LRU — hits by (origin, edge), misses on a
    /// different edge (so a px-preference change re-decodes rather than
    /// serving the old size), and evicts least-recent at the cap. Nonexistent
    /// paths give stamp None on both sides, which matches itself.
    #[test]
    fn cold_master_lru_hits_by_key_and_evicts_least_recent() {
        let mut app = AutoShadeApp::default();
        let master = Arc::new(image::DynamicImage::new_rgb8(6, 4));
        let p = std::path::Path::new("D:/__autoshade_nonexistent__/master.tif");
        app.remember_master(p, 1280, file_stamp(p), master.clone());
        let hit = app.cached_master(p, 1280).expect("same path + edge hits");
        assert_eq!(hit.dimensions(), (6, 4), "the decoded pixels ride the entry");
        assert!(
            app.cached_master(p, 4096).is_none(),
            "a different edge must MISS — the entry holds 1280-edge pixels"
        );
        // Re-remembering the same (path, edge) replaces rather than stacks.
        app.remember_master(p, 1280, file_stamp(p), master.clone());
        assert_eq!(app.master_cache.len(), 1, "same key replaces its entry");
        let others: Vec<std::path::PathBuf> = (0..MASTER_CACHE_CAP)
            .map(|i| std::path::PathBuf::from(format!("D:/__autoshade_nonexistent__/m{i}.tif")))
            .collect();
        for o in &others {
            app.remember_master(o, 1280, file_stamp(o), master.clone());
        }
        assert!(app.cached_master(p, 1280).is_none(), "least-recent evicted at cap");
        assert!(app.cached_master(&others[1], 1280).is_some(), "newer entries survive");
        assert!(app.master_cache.len() <= MASTER_CACHE_CAP, "cap holds");
    }

    #[test]
    fn the_mask_brush_session_follows_index_remaps() {
        let mut app = AutoShadeApp::default();
        app.recipe.masks =
            vec![Default::default(), Default::default(), Default::default()];
        // Session open on mask 2, deleting mask 0 shifts it to 1 — the stroke
        // must keep committing into the SAME mask, not whatever slid under
        // index 2.
        app.mask_brush = Some((Some(2), false));
        app.mask_brush_gray = Some(image::GrayImage::new(4, 4));
        app.paint_mode = true;
        app.recipe.masks.remove(0);
        app.remap_mask_indices(|s| match s {
            0 => None,
            s => Some(s - 1),
        });
        assert_eq!(app.mask_brush, Some((Some(1), false)), "target follows its mask");
        assert!(app.mask_brush_gray.is_some(), "a surviving session keeps its buffer");

        // Deleting the session's OWN mask ends it like Esc: buffer gone,
        // paint mode disarmed — never a new-mask fallback that would
        // resurrect the deleted mask under a fresh slot.
        app.recipe.masks.remove(1);
        app.remap_mask_indices(|s| match s {
            1 => None,
            s => Some(s),
        });
        assert_eq!(app.mask_brush, None, "a vanished target ends the session");
        assert!(app.mask_brush_gray.is_none(), "the weight buffer dies with it");
        assert!(!app.paint_mode, "paint mode disarms with the dead session");

        // A NEW-mask session carries no index and survives any remap.
        app.mask_brush = Some((None, true));
        app.remap_mask_indices(|_| None);
        assert_eq!(app.mask_brush, Some((None, true)));
    }

    #[test]
    fn an_async_variant_push_commits_a_typed_mask_rename_first() {
        let mut app = AutoShadeApp::default();
        app.recipe.masks = vec![autoshade::recipe::LocalAdjustment {
            name: "old".into(),
            ..Default::default()
        }];
        // A photo is open: its strip holds the outgoing variant that
        // push_variant snapshots the live canvas into.
        app.variants = vec![Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        }];
        app.active = 0;
        // The user is mid-typing "sky gradient" when a reverse-fit lands and
        // push_variant auto-switches: the switch's M15 boundary clear used to
        // discard the buffer, and the outgoing variant snapshotted "old".
        app.mask_name_buf = Some((0, "old".into(), "sky gradient".into()));
        app.push_variant(
            Variant {
                id: String::new(),
                name: None,
                kind: VariantKind::Fitted,
                recipe: EditRecipe::default(),
                base: None,
                origin: None,
                thumb: None,
            },
            &egui::Context::default(),
        );
        assert_eq!(
            app.variants[0].recipe.masks[0].name, "sky gradient",
            "the typed rename survives into the outgoing variant's snapshot"
        );
    }

    /// v1.2.4 A7 — the projection's two NOT-MEASURABLE clauses reach the
    /// Chinese UI. Both keys existed with translations and neither had ever
    /// been produced, so nothing checked that the catalogue actually answers
    /// for them; `trf` falls back to the English key SILENTLY, which is
    /// exactly the shape of failure a translation gate cannot see from the
    /// outside. The note-key path is the panel's own (`trf(lang, n.key, …)`
    /// in panels/ai.rs), so this exercises the site the user reads through.
    #[test]
    fn the_projections_not_measurable_clauses_reach_both_languages() {
        for note in [
            autoshade::rationale::Note::plain(
                autoshade::rationale::keys::FIT_NOTE_CAST_PROJECTED_FAN_NA,
            ),
            autoshade::rationale::Note::plain(
                autoshade::rationale::keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA,
            ),
        ] {
            let en = trf(crate::i18n::Lang::En, note.key, &[]);
            let zh = trf(crate::i18n::Lang::Zh, note.key, &[]);
            assert_eq!(en, note.key, "English is the key itself");
            assert!(en.contains("not measurable"), "the English clause says so: {en}");
            assert_ne!(zh, en, "the Chinese clause must not fall back to English: {zh}");
            assert!(zh.contains("无法测量"), "…and must say the same thing: {zh}");
            for text in [en, zh] {
                assert!(
                    !text.chars().any(|c| c.is_ascii_digit()),
                    "an unmeasured reading must not print a number in either language: {text}"
                );
            }
        }
    }

    /// L12#2A: the verdict is TYPED data rendered at draw time — the zh
    /// catalogue must actually translate every decision word, or the map
    /// silently falls back to English (tr's contract) and the gate that
    /// exists to see that (audit_i18n's extraction of decision_key) would
    /// be the only witness. This is the runtime half of that gate.
    #[test]
    fn a_verdict_names_its_decision_in_the_current_language() {
        use autoshade::advisor::{decision_key, Decision};
        // One authority: the typed decision maps to exactly these keys…
        assert_eq!(decision_key(&Decision::Accept), "Accept");
        assert_eq!(decision_key(&Decision::Revise), "Revise");
        assert_eq!(decision_key(&Decision::Reject), "Reject");
        // …and the catalogue actually translates each of them (tr falls
        // back to English SILENTLY, so equality-with-key would be the only
        // symptom). Literal keys on purpose — the audit treats non-literal
        // tr() arguments as dynamic sites needing registration.
        assert_eq!(tr(Lang::Zh, "Accept"), "接受");
        assert_eq!(tr(Lang::Zh, "Revise"), "修订");
        assert_eq!(tr(Lang::Zh, "Reject"), "驳回");
        assert_eq!(tr(Lang::En, "Accept"), "Accept");
        // The verdict line skeleton interpolates both halves.
        let line = trf(
            Lang::Zh,
            "{decision} — {reasons}",
            &[("decision", tr(Lang::Zh, "Revise")), ("reasons", "太亮")],
        );
        assert_eq!(line, "修订 — 太亮");
    }

    /// A5: an argument value that is a WORD renders in the session language.
    ///
    /// `FIT_NOTE_VETO_DISCLOSED` interpolates `{kind}`, whose value the engine
    /// picks from a fixed set of two English phrases. Copied verbatim, that was
    /// the one untranslated fragment inside an otherwise Chinese sentence.
    ///
    /// MUTATION: substitute `value` instead of `tr_value(lang, value)` in `trf`
    /// and the zh assertion below finds "luma ranges" in the rendered line.
    #[test]
    fn an_enumerated_argument_value_renders_in_the_session_language() {
        use autoshade::rationale::values;
        // Spelled in full at every `trf` site on purpose: `audit_i18n.py`'s
        // dynamic-key registry recognises `rationale::keys::…`, and a site it
        // cannot recognise is one it cannot prove has a zh row.
        let render = |lang, kind: &str| {
            trf(
                lang,
                autoshade::rationale::keys::FIT_NOTE_VETO_DISCLOSED,
                &[("kind", kind), ("ranges", "[0.1-0.2]")],
            )
        };
        let zh = render(Lang::Zh, values::LUMA_RANGES);
        assert!(zh.contains("明度范围"), "the value stayed English: {zh}");
        assert!(!zh.contains("luma ranges"), "the value stayed English: {zh}");
        let zh_hue = render(Lang::Zh, values::HUE_BANDS);
        assert!(zh_hue.contains("色相带"), "{zh_hue}");
        assert!(!zh_hue.contains("hue bands"), "{zh_hue}");
        // English is unchanged, value included — the persisted rationale and
        // the en UI must still read exactly what the engine wrote.
        let en = render(Lang::En, values::LUMA_RANGES);
        assert!(en.contains("luma ranges"), "{en}");
        // A value that is NOT in the registry is never rewritten by a
        // catalogue lookup: measurements, paths and model prose ride through
        // verbatim, and the match is on the WHOLE value, so an enumerated
        // phrase merely CONTAINED in a sentence is left alone too.
        let free = trf(
            Lang::Zh,
            autoshade::rationale::keys::FIT_NOTE_VETO_DISCLOSED,
            &[("kind", values::HUE_BANDS), ("ranges", "[0.10-0.20] and luma ranges nearby")],
        );
        assert!(
            free.contains("[0.10-0.20] and luma ranges nearby"),
            "a free-text arg was rewritten: {free}"
        );
    }

    /// A10: the provider's own disclosures are keys, in both languages.
    ///
    /// Both used to be English prose pushed straight into `recipe.rationale`,
    /// where the suffix-strip contract reads them as the MODEL's words — so the
    /// zh panel showed an English sentence with nothing to translate.
    #[test]
    fn the_providers_own_disclosures_render_in_both_languages() {
        // Unrolled rather than looped, and the constants spelled in full: a
        // render call whose key argument is a loop variable is exactly the
        // shape `audit_i18n.py`'s dynamic-key registry exists to refuse — and
        // the audit reads raw source, so writing that shape inside a COMMENT
        // trips it too.
        let both = |zh: String, en: String, cn: &str, arg: &str| {
            assert_ne!(zh, en, "no zh rendering: {en}");
            assert!(zh.contains(cn), "rendered as {zh}");
            // The argument still lands, in both.
            assert!(zh.contains(arg), "{zh}");
            assert!(en.contains(arg), "{en}");
        };
        let axes = [("axes", "hue had 7")];
        both(
            trf(Lang::Zh, autoshade::rationale::keys::HSL_AXIS_LENGTH_REPAIRED, &axes),
            trf(Lang::En, autoshade::rationale::keys::HSL_AXIS_LENGTH_REPAIRED, &axes),
            "色彩混合器",
            "hue had 7",
        );
        let dropped = [("dropped", "exposure_ev")];
        both(
            trf(Lang::Zh, autoshade::rationale::keys::PROPOSAL_LIMITS_DISCARDED, &dropped),
            trf(Lang::En, autoshade::rationale::keys::PROPOSAL_LIMITS_DISCARDED, &dropped),
            "配方上限",
            "exposure_ev",
        );
    }

    /// L12#2B: typed rationale notes describe ONE develop's rationale tail.
    /// Every wholesale recipe swap (undo / version load / variant switch /
    /// AI apply) goes through resync_recipe_display — if that did not clear
    /// the vec, the panel would strip-and-render a stale zh text over a
    /// DIFFERENT develop's rationale. The landings that produce fresh notes
    /// install them AFTER the resync.
    #[test]
    fn a_recipe_swap_clears_stale_rationale_notes() {
        let mut app = AutoShadeApp::default();
        app.recipe.rationale = "old english tail".into();
        app.rationale_notes = vec![autoshade::rationale::Note::plain(
            autoshade::rationale::keys::FIT_NOTE_SAT_PEGGED,
        )];
        app.recipe = EditRecipe { rationale: "a different develop".into(), ..Default::default() };
        app.resync_recipe_display();
        assert_eq!(app.rationale, "a different develop");
        assert!(
            app.rationale_notes.is_empty(),
            "stale notes must not survive a recipe swap — they rendered another rationale"
        );
    }

    /// L12#3: the script classifier + the undrawable projection are pure —
    /// `installed` is a parameter, so no real font is needed. Each probe
    /// char must land in exactly one script, and a name in an uncovered
    /// script names the char it cannot draw.
    #[test]
    fn undrawable_scripts_names_the_char_it_cannot_draw() {
        // Probe chars are CONSTRUCTED, not literals: the font gate (and the
        // subset extractor) scan this file's string literals, and a literal
        // Thai/Hebrew probe would demand embedded glyphs for text no UI
        // ever renders.
        let ch = |u: u32| char::from_u32(u).expect("probe codepoint");
        let probes: [(&str, u32); 9] = [
            ("hebrew", 0x05D0),
            ("arabic", 0x0628),
            ("devanagari", 0x0915),
            ("bengali", 0x0985),
            ("tamil", 0x0B95),
            ("thai", 0x0E01),
            ("hangul", 0xAC00),
            ("kana", 0x3042),
            ("han", 0x5199),
        ];
        for (want, u) in probes {
            assert_eq!(script_of(ch(u)), Some(want), "U+{u:04X} classifies as exactly {want}");
        }
        assert_eq!(script_of('A'), None, "Latin never discloses");
        assert_eq!(
            script_of(ch(0x25ED)),
            None,
            "symbols never disclose (embedded subsets own them)"
        );

        let thai_name = format!("DSC_{}{}{}_01", ch(0x0E1F), ch(0x0E49), ch(0x0E32));
        let hits = undrawable_scripts(&thai_name, &[]);
        assert_eq!(hits.len(), 1, "one script, one entry: {hits:?}");
        assert_eq!(hits[0].0, "thai");
        assert_eq!(script_of(hits[0].1), Some("thai"), "the sample char is from the name");
        assert!(
            undrawable_scripts(&thai_name, &["thai"]).is_empty(),
            "an installed script is drawable — no disclosure"
        );
        let mixed = format!("{}{}_{}{}", ch(0xC11C), ch(0xC6B8), ch(0x062F), ch(0x0628));
        let two = undrawable_scripts(&mixed, &["hangul"]);
        assert_eq!(two.len(), 1, "only the uncovered script remains: {two:?}");
        assert_eq!(two[0].0, "arabic");
    }

    /// L12#3: a runtime font read is bounded — stat before read, past-budget
    /// skipped (never truncated: a cut font is a parse error at best), and
    /// the folder-open disclosure fires ONCE per script per session.
    #[test]
    fn a_fallback_font_past_the_byte_cap_is_skipped_and_disclosed_once() {
        // The cap logic, against on-disk fixtures with a tiny budget.
        let dir = std::env::temp_dir().join(format!("autoshade-fontcap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.ttf");
        let big = dir.join("big.ttf");
        std::fs::write(&small, b"tiny").unwrap();
        std::fs::write(&big, vec![0u8; 64]).unwrap();
        assert_eq!(
            read_font_capped(small.to_str().unwrap(), 32).as_deref(),
            Some(b"tiny".as_slice()),
            "under budget reads whole"
        );
        assert!(
            read_font_capped(big.to_str().unwrap(), 32).is_none(),
            "past budget is skipped, not truncated"
        );
        assert!(read_font_capped(dir.join("absent.ttf").to_str().unwrap(), 32).is_none());
        let _ = std::fs::remove_dir_all(&dir);

        // Disclosure-once: two folder opens with Thai names disclose once.
        // (Constructed chars — see undrawable_scripts_names_the_char…)
        let thai = char::from_u32(0x0E1F).expect("probe codepoint");
        let mut app = AutoShadeApp {
            gallery: vec![std::path::PathBuf::from(format!("DSC_{thai}_01.arw"))],
            ..Default::default()
        };
        // No fonts installed in a test process ⇒ installed_scripts() is
        // empty or CJK-only; thai is never in it, so the disclosure fires.
        app.disclose_undrawable_names();
        let after_first = app.toasts.len();
        assert_eq!(after_first, 1, "one script, one toast");
        app.disclose_undrawable_names();
        assert_eq!(app.toasts.len(), after_first, "the second open stays silent");
        assert!(app.disclosed_scripts.contains("thai"));
    }
