// One part of the GUI's tests (src/bin/gui/tests.rs includes it): brush clearing, zoom and crop geometry, retouch artifacts, the export destination and the sidecar hand-off.

    /// L15-8: Clear must clear what Apply BAKES (the greyscale weight
    /// buffer), not only what the canvas shows — the session's own contract
    /// is "bakes exactly what it shows".
    #[test]
    fn clearing_the_brush_clears_what_apply_would_bake() {
        let mut app = AutoShadeApp {
            mask_paint: Some(image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 64, 64, 160]))),
            mask_brush_gray: Some(image::GrayImage::from_pixel(4, 4, image::Luma([255]))),
            ..Default::default()
        };
        app.clear_mask();
        assert!(
            app.mask_brush_gray.unwrap().pixels().all(|p| p[0] == 0),
            "Apply after Clear must bake nothing"
        );
    }

    /// L13-11: the inverse map keeps working on a sub-point drawn rect — the
    /// old max(1.0) floor replaced the tiny dimension with a full point and
    /// compressed the whole axis.
    #[test]
    fn to_norm_survives_a_sub_point_rect() {
        let xf = ViewXform {
            rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(0.5, 300.0)),
            uv: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        };
        let (nx, _) = xf.to_norm(egui::pos2(0.25, 150.0));
        assert!((nx - 0.5).abs() < 1e-3, "the middle of a 0.5-pt-wide rect is uv 0.5: {nx}");
    }

    /// L13-2: past fit the 4× upscale cap is dropped — a zoomed canvas fills
    /// the pane instead of being re-fit SMALLER as the visible window
    /// shrinks.
    #[test]
    fn a_zoomed_canvas_is_never_re_fit_smaller() {
        let fit = fit_in(egui::vec2(50.0, 50.0), 600.0, 450.0);
        assert_eq!(fit, egui::vec2(200.0, 200.0), "fit view keeps the 4× cap");
        let zoomed = fit_in_capped(egui::vec2(50.0, 50.0), 600.0, 450.0, f32::INFINITY);
        assert_eq!(zoomed, egui::vec2(450.0, 450.0), "a zoomed view fills the pane box");
    }

    /// L13-1: `pan` is stored in the current WINDOW's coordinates, so the
    /// crop-mode flip must rebase the value — not reinterpret it.
    #[test]
    fn entering_crop_mode_rebases_the_pan_it_reinterprets() {
        let mut app = AutoShadeApp::default();
        app.recipe.crop =
            Some(autoshade::recipe::Crop { left: 0.5, top: 0.5, right: 1.0, bottom: 1.0 });
        app.pan = egui::vec2(0.5, 0.5); // centre of the CROP window
        app.set_crop_mode(true);
        assert!(
            (app.pan.x - 0.75).abs() < 1e-4 && (app.pan.y - 0.75).abs() < 1e-4,
            "the same viewport centre, now in full-frame coords: {:?}",
            app.pan
        );
        app.set_crop_mode(false);
        assert!(
            (app.pan.x - 0.5).abs() < 1e-4 && (app.pan.y - 0.5).abs() < 1e-4,
            "leaving the tool rebases back: {:?}",
            app.pan
        );
    }

    /// L12-7: the cancel toast promises "the late result is discarded" — a
    /// late SUCCESS's unreferenced ./out artifact is removed, not
    /// accumulated.
    #[test]
    fn a_cancelled_retouchs_late_artifact_is_discarded_from_disk() {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-gui-late-artifact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = dir.join("late.retouch.png");
        std::fs::write(&artifact, b"orphan bytes").unwrap();

        let ctx = egui::Context::default();
        // The cancel already bumped past this task's epoch 6.
        let mut app = AutoShadeApp { gen_epoch: 7, ..Default::default() };
        app.on_retouched(
            &ctx,
            Lang::En,
            6,
            Ok((
                image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 2)),
                RetouchNote::Filled(artifact.clone()),
                artifact.clone(),
                RetouchKind::InPlace,
            )),
        );
        assert!(!artifact.exists(), "the promised discard includes the disk artifact");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// User screenshot 2026-10-18: at a panel dragged to its narrowest, the
    /// reference library's 「🗂 RAW 文件夹…」 grew past its half-row cell and
    /// pushed 「✕ 清除」 out of the card. A cell button truncates its label
    /// instead (`buttons::place`), so no cell button reaches past the panel
    /// at the minimum width, in either language, laid in the app's REAL fonts.
    /// MUTATION: drop the `.truncate()` in `place` and the rows grow the
    /// 280 px panel (281 px in English, 282 px in Chinese) — red.
    #[test]
    fn no_cell_button_reaches_past_the_narrowest_panel() {
        use crate::buttons::DRAWN;
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            // The REAL fonts (embedded symbols + the system CJK face): egui's
            // bundle lays the Chinese labels narrower than the app draws them,
            // which is how the default-width pins missed this row.
            crate::theme::install_fonts(&ctx);
            let mut app = AutoShadeApp { lang, style_raw_dir: Some("D:/raw".into()), ..Default::default() };
            let mut panel = egui::Rect::NOTHING;
            for _ in 0..3 {
                DRAWN.with_borrow_mut(Vec::clear);
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 20_000.0))),
                    ..Default::default()
                };
                let _ = ctx.run(input, |ctx| {
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    panel = egui::SidePanel::left("controls")
                        .exact_width(crate::theme::CONTROLS_W_MIN)
                        .show(ctx, |ui| egui::ScrollArea::vertical().show(ui, |ui| app.ai_panel(ui)))
                        .response
                        .rect;
                });
            }
            let drawn = DRAWN.with_borrow(|d| d.clone());
            assert!(drawn.iter().any(|d| d.label == tr(lang, "🗂 RAW folder…")), "{lang:?}: the library row was drawn");
            // A label wider than its cell widens its button, and the row then
            // widens the panel past its fixed width: the panel is the witness.
            assert!(
                (panel.width() - crate::theme::CONTROLS_W_MIN).abs() <= 0.5,
                "{lang:?}: the panel grew to {:.1} px, a row overflows it",
                panel.width()
            );
            for d in drawn.iter().filter(|d| d.cell) {
                assert!(
                    d.rect.right() <= panel.right() + 0.5,
                    "{lang:?}: {} {:?} ends at {:.1}, past the {:.1} px panel edge",
                    d.kind, d.label, d.rect.right(), panel.right()
                );
            }
        }
    }

    /// User report 2026-10-18: the ▣ card carried an old in-place denoise
    /// master the user had since deleted with the delivery folder, so AI
    /// denoise on 「原片」 failed with "open image …denoise.png: not found".
    /// A master load that fails because the file is GONE drops the ▣ card's
    /// dead link (its pixels are the photo file again); a ◈ card keeps its
    /// link — it has no other pixels — and so does a ▣ master that exists
    /// but will not decode. MUTATION: drop the `v.origin = None` and the
    /// first assertion turns red.
    #[test]
    fn a_vanished_original_master_hands_the_card_back_to_the_photo() {
        use crate::model::{Variant, VariantKind};
        let dir = std::env::temp_dir()
            .join(format!("autoshade-gui-vanished-master-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("_vanished.ARW");
        let gone = dir.join("_vanished.denoise.png"); // never written
        let broken = dir.join("_broken.denoise.png");
        std::fs::write(&broken, b"not a png").unwrap();
        let mk = |kind, origin: &std::path::Path| Variant {
            kind,
            id: crate::model::new_variant_id(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: Some(origin.to_path_buf()),
            thumb: None,
        };
        let mut app = AutoShadeApp {
            src_path: Some(photo.clone()),
            variants: vec![mk(VariantKind::Original, &gone), mk(VariantKind::Denoised, &gone)],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let lost = |o: &std::path::Path| Msg::MasterLoaded {
            photo: photo.clone(),
            origin: o.to_path_buf(),
            edge: 1280,
            stamp: None,
            img: Box::new(Err(anyhow::anyhow!("open image {}: not found", o.display()))),
        };
        app.tx.send(lost(&gone)).unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants[0].origin, None, "the ▣ card let go of the deleted master");
        assert_eq!(app.active_source_path(), Some(photo.clone()), "AI denoise now reads the photo");
        assert_eq!(app.variants[1].origin.as_deref(), Some(gone.as_path()), "a ◈ card keeps its link");

        app.variants[0].origin = Some(broken.clone());
        app.tx.send(lost(&broken)).unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.variants[0].origin.as_deref(), Some(broken.as_path()), "an undecodable master is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L12-2: the master cache keys on the SPAWN-time stamp — a file
    /// rewritten during the decode must MISS on the next probe instead of
    /// serving the old pixels under the new identity.
    #[test]
    fn a_master_rewritten_during_decode_misses_the_cache() {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-gui-master-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("master.png");
        std::fs::write(&p, b"generation A").unwrap();
        let spawn_stamp = file_stamp(&p);
        // The rewrite that lands while the decode is still running (longer
        // content — the stamp's length half moves even on coarse mtime).
        std::fs::write(&p, b"generation B, longer bytes").unwrap();

        let mut app = AutoShadeApp::default();
        let pixels = std::sync::Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 2)));
        app.remember_master(&p, 1280, spawn_stamp, pixels);
        assert!(
            app.cached_master(&p, 1280).is_none(),
            "the old pixels were filed under the OLD identity, so the rewritten file misses"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L14-1: the format dropdown owns the downloaded file's container — a
    /// typed foreign extension is rewritten, a same-container spelling is
    /// kept exactly as typed.
    #[test]
    fn the_format_dropdown_owns_the_downloaded_extension() {
        use std::path::PathBuf;
        let n = |p: &str, ext: &str| normalize_export_target(PathBuf::from(p), ext);
        assert_eq!(n("photo.png", "jpg"), PathBuf::from("photo.jpg"));
        assert_eq!(n("photo", "tif"), PathBuf::from("photo.tif"));
        assert_eq!(n("photo.jpeg", "jpg"), PathBuf::from("photo.jpeg"), "same container, as typed");
        assert_eq!(n("photo.TIFF", "tif"), PathBuf::from("photo.TIFF"));
        assert_eq!(n("photo.developed.png", "png"), PathBuf::from("photo.developed.png"));
    }

    /// R22-7: the Destination setting's three states, plus the fourth case that
    /// only the pure resolver can express — "last used folder" before anything
    /// has been exported. `./out` must stay the default answer (the CLI/batch
    /// root), a remembered folder must be used VERBATIM, and both asking states
    /// must be the SAME `None` so one code path handles them.
    ///
    /// The paths here do not exist, on purpose: the resolver must not stat
    /// anything (the toolbar hover resolves it every frame, and a deleted
    /// folder is re-created by the render rather than turning into a dialog).
    #[test]
    fn the_destination_setting_resolves_to_one_delivery_folder() {
        use std::path::Path;
        let last = Path::new("D:/deliver/tripA");
        // R24-5 M8: this arm resolves to the DELIVERY ROOT setting, not to a
        // literal `./out` — the same one `pipeline::default_out` claims names
        // under, so this window, the CLI, the web download route and a batch
        // render name one folder. Re-derived here rather than spelled, so the
        // assertion holds whatever the setting says (unset ⇒ `out`, which is
        // what this arm always returned).
        // (The default itself — "unset ⇒ ./out" — is pinned as a pure
        // function in `config::the_delivery_root_defaults_to_the_out_folder_it_replaced`;
        // asserting it again HERE would make this test fail for a developer
        // who has AUTOSHADE_OUT_DIR set, which is not what it is about.)
        let root = autoshade::config::delivery_root();
        assert_eq!(
            crate::export::export_dest_dir(ExportDest::OutFolder, None),
            Some(root.clone()),
            "the default is the CLI's and the batch renderer's own root"
        );
        assert_eq!(
            crate::export::export_dest_dir(ExportDest::OutFolder, Some(last)),
            Some(root),
            "…and a remembered folder does not override an explicit delivery root"
        );
        assert_eq!(
            crate::export::export_dest_dir(ExportDest::LastUsed, Some(last)),
            Some(last.to_path_buf()),
            "the remembered folder is used verbatim, not joined onto ./out"
        );
        assert_eq!(
            crate::export::export_dest_dir(ExportDest::LastUsed, None),
            None,
            "nothing remembered yet ⇒ ask once and let the answer seed the memory"
        );
        assert_eq!(crate::export::export_dest_dir(ExportDest::Ask, Some(last)), None);
        // Every code is round-trippable, and an unknown one degrades to ./out
        // rather than to a dialog (a prefs file from a newer build must not turn
        // every export into a prompt).
        for d in ExportDest::ALL {
            assert_eq!(ExportDest::from_pref(d.pref_code()), d);
        }
        assert_eq!(ExportDest::from_pref(0), ExportDest::OutFolder, "serde's default");
        assert_eq!(ExportDest::from_pref(200), ExportDest::OutFolder);
    }

    /// R22-7: 「Ask every time」 must reach the dialog, never a silent write.
    /// The seam under test is the ROUTE — the decision the Export button makes
    /// before anything is written — because the dialog itself cannot be opened
    /// from a headless test. The mutation this catches is the tempting one: an
    /// `unwrap_or_else(|| "out".into())` in the resolver, which would turn every
    /// ask into a silent ./out delivery.
    #[test]
    fn the_ask_destination_routes_to_the_dialog_instead_of_writing() {
        use std::path::PathBuf;
        let src = PathBuf::from("D:/library/DSC00042.ARW");
        let asking = AutoShadeApp {
            src_path: Some(src.clone()),
            exp_dest: ExportDest::Ask,
            exp_format: ExportFormat::Jpeg,
            // A remembered folder is present ON PURPOSE: "ask every time" must
            // outrank it, or the setting silently becomes "last used".
            last_export_dir: Some(PathBuf::from("D:/deliver/tripA")),
            ..Default::default()
        };
        assert_eq!(asking.export_route(), ExportRoute::Ask);
        // The out-folder destination is the configured delivery root, `out`
        // only while the machine's settings leave it unset.
        let candidate = autoshade::config::delivery_root().join("DSC00042.developed.jpg");
        assert!(
            !candidate.exists(),
            "deciding the route must not have created {}",
            candidate.display()
        );
        // The same app with the default destination DOES resolve to a file —
        // without this half the assertion above could pass on a broken route
        // that never renders anything at all.
        let direct = AutoShadeApp {
            src_path: Some(src),
            exp_dest: ExportDest::OutFolder,
            exp_format: ExportFormat::Jpeg,
            ..Default::default()
        };
        assert_eq!(direct.export_route(), ExportRoute::Render(candidate));
    }

    /// R22-7: the landing names an ABSOLUTE path. `./out` is relative to the
    /// directory the app was launched from, so the old message pointed at a
    /// folder the user had to guess at — and a windowed build has no shell to
    /// resolve it in.
    #[test]
    fn a_finished_export_names_an_absolute_path() {
        use std::path::PathBuf;
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { busy: true, ..Default::default() };
        let rel = PathBuf::from("out").join("_r22_abs.developed.tif");
        let abs = std::path::absolute(&rel).unwrap();
        assert_ne!(abs, rel, "fixture: the path must actually be relative");
        app.tx
            .send(Msg::Exported(Ok(ExportOutcome::Single { out: rel, relooked: false })))
            .unwrap();
        app.poll_workers(&ctx);
        assert!(
            app.status.contains(&abs.display().to_string()),
            "the completion message must be actionable: {}",
            app.status
        );
        assert!(
            !app.status.contains(r"\\?\"),
            "…and readable — no canonicalize verbatim prefix: {}",
            app.status
        );
    }

    /// R22 M1: the batch destination is remembered where the files LAND, not
    /// where the dialog pointed. Picking a folder inside the photo library made
    /// `guard_readonly` refuse every photo — and the old code had already
    /// written that folder into `last_export_dir`, so `ExportDest::LastUsed`
    /// then aimed at a destination that could only ever refuse, permanently.
    #[test]
    fn a_batch_that_delivered_nothing_does_not_become_the_remembered_destination() {
        use std::path::PathBuf;
        let ctx = egui::Context::default();
        let kept = PathBuf::from("D:/deliver/tripA");
        let refused = PathBuf::from("D:/library/2026-08");
        let mut app = AutoShadeApp {
            busy: true,
            last_export_dir: Some(kept.clone()),
            ..Default::default()
        };
        app.tx
            .send(Msg::Exported(Ok(ExportOutcome::Batch {
                ok: 0,
                errs: vec!["the photo library is read-only: D:/library/2026-08".into()],
                renamed: Vec::new(),
                relooked: 0,
                warns: Vec::new(),
                dest: refused.clone(),
                refused: 0,
            })))
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(
            app.last_export_dir.as_deref(),
            Some(kept.as_path()),
            "a batch that delivered nothing must leave the memory alone"
        );

        // The other half, or the assertion above would pass on a build that
        // never remembers anything: a batch that DID deliver seeds the memory.
        let landed = PathBuf::from("D:/deliver/tripB");
        let mut ok_app = AutoShadeApp { busy: true, ..Default::default() };
        ok_app
            .tx
            .send(Msg::Exported(Ok(ExportOutcome::Batch {
                ok: 2,
                errs: Vec::new(),
                renamed: Vec::new(),
                relooked: 0,
                warns: Vec::new(),
                dest: landed.clone(),
                refused: 0,
            })))
            .unwrap();
        ok_app.poll_workers(&ctx);
        assert_eq!(
            ok_app.last_export_dir.as_deref(),
            Some(landed.as_path()),
            "the folder the files really landed in becomes ExportDest::LastUsed"
        );
    }

    /// R22-8 / SF8-A: the two-click confirm. A sidecar already beside the photo
    /// (Lightroom's own) ARMS the button instead of being overwritten; the
    /// second click replaces it and disarms. Runs against the real develop store
    /// for a temp fixture photo, like every other store-touching GUI test.
    #[test]
    fn handing_the_sidecar_to_lightroom_never_overwrites_in_silence() {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-gui-xmp-beside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_r22_handoff.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = autoshade::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(autoshade::store::xmp_target(&raw), b"<x:xmpmeta>ours</x:xmpmeta>").unwrap();
        let beside = raw.with_extension("xmp");

        let mut app = AutoShadeApp { src_path: Some(raw.clone()), ..Default::default() };
        assert!(app.can_export_xmp_beside(), "a RAW can hand its sidecar over");

        // First delivery: nothing in the way, so it lands with no confirmation.
        app.export_xmp_beside();
        assert!(!app.xmp_beside_confirm, "an unobstructed hand-off needs no confirm");
        assert_eq!(std::fs::read(&beside).unwrap(), b"<x:xmpmeta>ours</x:xmpmeta>");
        assert!(
            app.status.contains(&abs_display(&beside)),
            "the status names where it landed: {}",
            app.status
        );

        // Lightroom's own file appears there. One click must ARM, not write.
        std::fs::write(&beside, b"<x:xmpmeta>LIGHTROOM</x:xmpmeta>").unwrap();
        app.export_xmp_beside();
        assert!(app.xmp_beside_confirm, "the armed state is the confirmation");
        assert_eq!(
            std::fs::read(&beside).unwrap(),
            b"<x:xmpmeta>LIGHTROOM</x:xmpmeta>",
            "the first click left their sidecar untouched"
        );
        assert!(
            app.toasts.iter().any(|t| matches!(t.kind, ToastKind::Error)),
            "a refusal must be seen, not only appear in a status line"
        );

        // Second click: armed ⇒ overwrite, then disarm.
        app.export_xmp_beside();
        assert!(!app.xmp_beside_confirm, "the arm is spent");
        assert_eq!(std::fs::read(&beside).unwrap(), b"<x:xmpmeta>ours</x:xmpmeta>");

        // A non-RAW cannot: its neighbouring .xmp is another program's file.
        let baked = dir.join("_r22_handoff.png");
        std::fs::write(&baked, b"png").unwrap();
        let baked_app = AutoShadeApp { src_path: Some(baked), ..Default::default() };
        assert!(
            !baked_app.can_export_xmp_beside(),
            "only a RAW has the Lightroom sidecar convention"
        );
        assert!(
            !AutoShadeApp::default().can_export_xmp_beside(),
            "…and with no photo open there is nothing to hand over"
        );

        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One frame of the paint canvas (1024 × 768 at the origin of a central
    /// panel) with `events`; the brush cursor drawn as the canvas draws it.
    /// The frame's shapes and the cursor egui was asked for.
    fn paint_frame(
        app: &mut AutoShadeApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        time: f64,
    ) -> (Vec<egui::epaint::ClippedShape>, egui::CursorIcon, egui::Rect) {
        let mut canvas = egui::Rect::NOTHING;
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 850.0))),
                events,
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(1024.0, 768.0), egui::Sense::click_and_drag());
                    canvas = rect;
                    let xf = ViewXform { rect, uv: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)) };
                    app.handle_paint(&resp, xf);
                    app.draw_brush_cursor(ui, &resp, rect);
                });
            },
        );
        (out.shapes, out.platform_output.cursor_icon, canvas)
    }

    fn painted_pixels(app: &AutoShadeApp) -> usize {
        app.mask_paint.as_ref().map_or(0, |m| m.pixels().filter(|p| p[3] > 10).count())
    }

    /// User report 2026-10-13: 「操作感觉也不连贯」. A press on the canvas inks
    /// at once — before the pointer moves and before the button comes up —
    /// where egui's drag (≥ 6 px of travel) and click (on release) used to
    /// leave the canvas blank. Space held is the pan chord and must not ink.
    ///
    /// MUTATION THIS CATCHES: the stroke gated on `dragged_by` again.
    #[test]
    fn a_brush_press_inks_before_the_pointer_moves_or_lifts() {
        let fresh = || {
            let mut app = AutoShadeApp { paint_mode: true, ..Default::default() };
            app.mask_paint = Some(image::RgbaImage::new(1024, 768));
            app
        };
        let press = |pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        let mut app = fresh();
        let ctx = egui::Context::default();
        let (_, _, canvas) = paint_frame(&mut app, &ctx, vec![], 0.0);
        let at = canvas.center();
        paint_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at), press(at)], 0.1);
        let inked = painted_pixels(&app);
        assert!(inked > 0, "a still press left the canvas blank until release");
        // Moving 3 px — under egui's drag threshold — extends the stroke.
        paint_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at + egui::vec2(40.0, 0.0))], 0.2);
        assert!(painted_pixels(&app) > inked, "the stroke did not follow the pointer");

        let mut app = fresh();
        let ctx = egui::Context::default();
        paint_frame(&mut app, &ctx, vec![], 0.0);
        let space = egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        paint_frame(&mut app, &ctx, vec![space, egui::Event::PointerMoved(at), press(at)], 0.1);
        assert_eq!(painted_pixels(&app), 0, "Space + press is a pan, not a dab");
    }

    /// User report 2026-10-13: 「点了之后不知道大小」. Over the canvas the brush
    /// shows as a ring of its own radius in place of the OS cursor; after a
    /// size change the ring also flashes with the size, at the canvas centre
    /// when the pointer is elsewhere, and goes away after the flash.
    ///
    /// MUTATION THIS CATCHES: the ring drawn at a fixed radius, or the
    /// cursor left as the crosshair.
    #[test]
    fn the_brush_shows_its_size_where_it_will_land() {
        fn rings(shapes: &[egui::epaint::ClippedShape], r: f32) -> Vec<egui::Pos2> {
            fn walk(s: &egui::Shape, r: f32, out: &mut Vec<egui::Pos2>) {
                match s {
                    egui::Shape::Circle(c) if (c.radius - r).abs() < 0.01 && c.stroke.width > 0.0 => out.push(c.center),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, r, out)),
                    _ => {}
                }
            }
            let mut out = Vec::new();
            shapes.iter().for_each(|c| walk(&c.shape, r, &mut out));
            out
        }
        let mut app = AutoShadeApp { paint_mode: true, ..Default::default() };
        app.mask_paint = Some(image::RgbaImage::new(1024, 768));
        app.set_brush(57.0, 0.0);
        let ctx = egui::Context::default();
        let (_, _, canvas) = paint_frame(&mut app, &ctx, vec![], 0.0);
        let at = canvas.center() + egui::vec2(-100.0, 50.0);
        let (shapes, cursor, _) = paint_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at)], 0.1);
        assert!(rings(&shapes, 57.0).iter().any(|c| c.distance(at) < 0.5), "no 57 px ring under the pointer");
        assert_eq!(cursor, egui::CursorIcon::None, "the ring replaces the OS cursor over the canvas");
        let texts = drawn_texts(&shapes);
        assert!(texts.iter().any(|t| t.ends_with(" 57")), "the size flashes beside the ring: {texts:?}");

        // Off the canvas, during the flash: the ring at the centre.
        let (shapes, _, _) = paint_frame(&mut app, &ctx, vec![egui::Event::PointerGone], 0.2);
        assert!(rings(&shapes, 57.0).iter().any(|c| c.distance(canvas.center()) < 0.5), "no flash ring at the centre");
        // …and gone after it.
        let (shapes, _, _) = paint_frame(&mut app, &ctx, vec![], 5.0);
        assert!(rings(&shapes, 57.0).is_empty(), "the flash ring outlived its second");
        // The range clamps.
        app.set_brush(10_000.0, 6.0);
        assert_eq!(app.brush, crate::panels::retouch::BRUSH_R_MAX);
    }

    /// User report 2026-10-13: 「UI 现在最上方 -□x 条，太丑了」. On Windows the
    /// toolbar is the title bar; its groups must stop short of the three
    /// window buttons at its right end however narrow the window, and the
    /// window must be undecorated exactly where the toolbar draws them.
    ///
    /// MUTATION THIS CATCHES: the toolbar row taking the full width again
    /// (a group under the window buttons).
    #[test]
    fn the_toolbar_leaves_the_window_buttons_their_corner() {
        use crate::buttons::DRAWN;
        for width in [1400.0f32, 1000.0] {
            let mut app = app_with_every_button(crate::i18n::Lang::Zh);
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            DRAWN.with_borrow_mut(Vec::clear);
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 800.0))),
                    ..Default::default()
                },
                |ctx| app.upd_top_bar(ctx),
            );
            let limit = width - crate::window_chrome::controls_width();
            let drawn = DRAWN.with_borrow(|d| d.clone());
            assert!(!drawn.is_empty(), "the toolbar drew no buttons");
            for d in &drawn {
                assert!(
                    d.rect.right() <= limit + 0.5,
                    "{width} px: {:?} ends at {:.0}, under the window buttons (from {limit:.0})",
                    d.label,
                    d.rect.right()
                );
            }
        }
        let main = include_str!("../main.rs");
        assert!(main.contains(".with_decorations(!window_chrome::CUSTOM_CHROME)"), "the frame and the drawn buttons must agree");
    }

    /// User request 2026-10-13 (画笔羽化 / 流量): Feather fades the disc's
    /// outer part, Flow scales one dab, and the defaults are the old hard
    /// full-strength disc to the pixel — what the fill / heal / stamp areas
    /// and every existing mask were painted with.
    ///
    /// MUTATION THIS CATCHES: the weight ignoring feather or flow, or HARD
    /// no longer landing exactly on the target value.
    #[test]
    fn a_brush_tip_feathers_its_rim_and_scales_by_flow() {
        let red = image::Rgba([255, 64, 64, 160]);
        let alpha = |tip: BrushTip, x: u32| {
            let mut m = image::RgbaImage::new(101, 1);
            stamp_dot_px(&mut m, (50.0, 0.0), 40.0, red, tip);
            m.get_pixel(x, 0)[3]
        };
        // HARD: full target inside, untouched outside — the old disc.
        assert_eq!(alpha(BrushTip::HARD, 50), 160);
        assert_eq!(alpha(BrushTip::HARD, 89), 160);
        assert_eq!(alpha(BrushTip::HARD, 91), 0);
        // Feather 0.5: full out to 20 px, fading over the outer 20.
        let soft = BrushTip { feather: 0.5, flow: 1.0 };
        assert_eq!(alpha(soft, 65), 160, "inside the feather's start");
        let (near, mid, rim) = (alpha(soft, 72), alpha(soft, 80), alpha(soft, 88));
        assert!(160 > near && near > mid && mid > rim && rim > 0, "no fade across the rim: {near} {mid} {rim}");
        // Flow 0.5: one dab lays half; a second over it climbs toward full.
        let half = BrushTip { feather: 0.0, flow: 0.5 };
        assert_eq!(alpha(half, 50), 80);
        let mut m = image::RgbaImage::new(101, 1);
        stamp_dot_px(&mut m, (50.0, 0.0), 40.0, red, half);
        stamp_dot_px(&mut m, (50.0, 0.0), 40.0, red, half);
        assert_eq!(m.get_pixel(50, 0)[3], 120, "flow builds up pass by pass");
        // Erase moves toward clear by the same weights.
        let mut g = image::GrayImage::from_pixel(101, 1, image::Luma([255]));
        stamp_dot_gray(&mut g, (50.0, 0.0), 40.0, 0, half);
        assert_eq!(g.get_pixel(50, 0)[0], 128);
    }

    /// The grading wheel (2026-10-13) maps angle → hue and distance →
    /// saturation and back in the recipe's whole steps: what the puck shows is
    /// what a drag to that spot writes, a centred puck has no hue, and the
    /// rim clamps at 100.
    ///
    /// MUTATION THIS CATCHES: the y axis not flipped (hues mirrored), or
    /// saturation not clamped at the rim.
    #[test]
    fn a_grade_wheel_point_is_the_hue_and_saturation_it_shows() {
        use crate::panels::grade_wheel::{wheel_offset, wheel_value};
        let r = 80.0;
        for (h, s) in [(0.0, 40.0), (30.0, 100.0), (120.0, 25.0), (210.0, 60.0), (300.0, 85.0), (359.0, 50.0)] {
            assert_eq!(wheel_value(wheel_offset(h, s, r), r), (h, s), "round trip of hue {h} sat {s}");
        }
        assert_eq!(wheel_value(egui::vec2(0.0, -40.0), r), (90.0, 50.0), "up is 90° (screen y grows down)");
        assert_eq!(wheel_value(egui::Vec2::ZERO, r), (0.0, 0.0));
        assert_eq!(wheel_value(egui::vec2(500.0, 0.0), r), (0.0, 100.0), "past the rim clamps");
    }
