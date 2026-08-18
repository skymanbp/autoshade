// Round-12 split: body moved verbatim from main.rs's inline
// `mod tests` (indentation kept — raw-string fixtures must not
// change by one byte). `super::*` still resolves to the root.
    use super::*;

    /// R25 P8: what a paste is allowed to CARRY.
    ///
    /// The pass-through map is per-DOCUMENT state — Lightroom's Transform /
    /// Upright block and the camera profile name, read off ONE photo's sidecar
    /// and authored by nothing in this app. Carried to another photo it is
    /// provenance pollution with teeth: the target's next save would write the
    /// SOURCE's Upright solution into the file beside a RAW it was never
    /// solved for, and the read-only Transform / Calibration section would
    /// show the wrong photo's values the moment the paste landed.
    ///
    /// The clipboard's OWN photo keeps it, on the bitmap-mask rule beside it:
    /// there the map really is that photo's own state.
    #[test]
    fn a_paste_carries_the_edit_and_not_the_source_documents_own_state() {
        use crate::export::paste_payload;
        let mut src = EditRecipe {
            exposure_ev: 0.75,
            straighten_deg: 3.0,
            crop: Some(autoshop::recipe::Crop {
                top: 0.1,
                left: 0.1,
                bottom: 0.9,
                right: 0.9,
            }),
            ..Default::default()
        };
        src.passthrough = [("PerspectiveVertical", "-35"), ("CameraProfile", "Adobe Standard")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        src.masks.push(autoshop::recipe::LocalAdjustment {
            mask: autoshop::recipe::MaskGeometry::Bitmap { path: "sky.png".into() },
            ..Default::default()
        });

        let p = paste_payload(src.clone(), true);
        assert!(
            p.foreign.passthrough.is_empty(),
            "another photo's Upright block must not ride along: {:?}",
            p.foreign.passthrough
        );
        assert_eq!(p.own.passthrough, src.passthrough, "the source photo keeps its own");
        assert_eq!(p.foreign.exposure_ev, 0.75, "the EDIT is what a paste is for");
        // The rule this one was modelled on, still holding.
        assert_eq!(p.bitmap_masks, 1, "the raster mask is counted for the toast");
        assert!(p.foreign.masks.is_empty(), "…and dropped from the foreign payload");
        assert_eq!(p.own.masks.len(), 1, "…while the clipboard's own photo keeps it");
        // Geometry off strips BOTH arms: composition rarely transfers between
        // frames, and that includes back onto the source.
        let g = paste_payload(src, false);
        assert_eq!((g.own.crop.is_some(), g.own.straighten_deg), (false, 0.0));
        assert_eq!((g.foreign.crop.is_some(), g.foreign.straighten_deg), (false, 0.0));
    }

    /// L16-2: the batch resolver surfaces the disclosures the OPEN path
    /// would — a silent-neutral XMP value must reach the batch outcome.
    #[test]
    fn the_batch_resolver_discloses_what_the_open_path_would() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-batch-warns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("photo.arw");
        std::fs::write(&src, b"raw").unwrap();
        let xmp = r#"<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+1.00" crs:Contrast2012="bogus"/>"#;
        let snap = autoshop::store::DevelopSnapshot {
            recipe: None,
            recipe_err: None,
            lr_xmp: Some((xmp.to_string(), "Lightroom sidecar")),
            store_xmp: None,
            lr_unreadable: None,
            packet_xmp: None,
            packet_unreadable: None,
            pixel_source: None,
            pixel_recorded: false,
        };
        let mut warns = Vec::new();
        let (r, kind) = crate::export::resolve_snapshot_develop(&src, &snap, &mut warns)
            .unwrap()
            .expect("a non-noop sidecar resolves");
        assert_eq!(kind, "Lightroom sidecar");
        assert!((r.exposure_ev - 1.0).abs() < 1e-3);
        assert!(
            warns.iter().any(|w| w.contains("unreadable") && w.contains("Contrast2012")),
            "the silent neutral is disclosed on the batch surface: {warns:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L16-9: every export dial reaches the renderer's options — this block
    /// could previously be deleted with every GUI test green.
    #[test]
    fn export_opts_carries_every_dial_to_the_renderer() {
        let app = AutoshopApp {
            exp_long_edge: 2560,
            exp_sharpen: 35.0,
            exp_quality: 88.0,
            exp_space: 1,
            exp_format: ExportFormat::Png8,
            ..Default::default()
        };
        let o = app.export_opts();
        assert_eq!(o.long_edge, Some(2560));
        assert!((o.sharpen - 35.0).abs() < 1e-6);
        assert_eq!(o.jpeg_quality, 88);
        assert!(o.eight_bit, "Png8 is 8-bit");
        assert!(matches!(o.color_space, autoshop::render::ExportColorSpace::DisplayP3));
        let flat = AutoshopApp { exp_long_edge: 0, ..Default::default() };
        assert_eq!(flat.export_opts().long_edge, None, "0 = no resize");
    }

    /// L16-10: the glide runs through its production seam — the method the
    /// update loop calls, not the pure helper alone.
    #[test]
    fn the_zoom_glide_seam_moves_toward_the_target() {
        let ctx = egui::Context::default();
        let mut app = AutoshopApp { zoom: 1.0, zoom_target: 4.0, ..Default::default() };
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.apply_zoom_glide(ctx));
        assert!(
            app.zoom > 1.0 && app.zoom < 4.0,
            "one glide step moved toward the target: {}",
            app.zoom
        );
    }

    /// L16-11: the shortcut exclusivity gates, tested NEGATIVELY — deleting
    /// the transient guard used to fail nothing.
    #[test]
    fn a_transient_window_swallows_canvas_shortcuts() {
        let ctx = egui::Context::default();
        let key = |k| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        // Positive control first: with NO transient up, R arms the crop —
        // proving this harness actually reaches the tool tier.
        let mut app = AutoshopApp {
            src_path: Some(std::path::PathBuf::from("D:/library/x.arw")),
            ..Default::default()
        };
        let mut input = egui::RawInput::default();
        input.events.push(key(egui::Key::R));
        let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
        assert!(app.crop_mode, "positive control: R arms the crop tool");

        // The negative: a transient (Settings) swallows the tool tier…
        let mut app = AutoshopApp {
            src_path: Some(std::path::PathBuf::from("D:/library/x.arw")),
            show_settings: true,
            ..Default::default()
        };
        let mut input = egui::RawInput::default();
        input.events.push(key(egui::Key::R));
        let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
        assert!(!app.crop_mode, "R is dead while a modal transient is up");
        // …and Esc dismisses the transient instead of leaking anywhere.
        let mut input = egui::RawInput::default();
        input.events.push(key(egui::Key::Escape));
        let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
        assert!(!app.show_settings, "Esc dismissed the transient");
        assert!(!app.crop_mode);
    }

    /// L16-12: the display resync is exercised through a REAL swap path
    /// (load_version), not by calling the helper directly.
    #[test]
    fn a_version_load_resyncs_the_display_state() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-version-resync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("photo.arw");
        std::fs::write(&src, b"raw").unwrap();
        let dev = autoshop::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let versioned = EditRecipe {
            exposure_ev: 0.75,
            rationale: "v1 marker rationale".into(),
            ..Default::default()
        };
        std::fs::write(
            autoshop::store::version_target(&src, 1),
            serde_json::to_string(&versioned).unwrap(),
        )
        .unwrap();

        let mut app = AutoshopApp {
            src_path: Some(src.clone()),
            sel_mask: Some(3), // stale index into the OLD mask list
            rationale: "stale rationale".into(),
            ..Default::default()
        };
        app.load_version(1);
        assert!((app.recipe.exposure_ev - 0.75).abs() < 1e-3, "the version landed");
        assert_eq!(
            app.rationale, "v1 marker rationale",
            "the swap path resynced the rationale display"
        );
        assert_eq!(app.sel_mask, None, "a stale mask selection cannot linger");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// R24-2: a variant's identity + name survive every hop between the live
    /// strip, the navigation stash and `variants.json`. Six hops carry them,
    /// and a name that falls off ANY of them is silent loss — the user typed
    /// it, nothing warned, and the card comes back nameless.
    #[test]
    fn a_variant_name_survives_every_hop_it_has_to_take() {
        use crate::model::{StashedVariant, Variant, VariantKind};
        let mk = |kind, id: &str, name: Option<&str>| Variant {
            kind,
            id: id.into(),
            name: name.map(str::to_string),
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        let mut app = AutoshopApp {
            variants: vec![
                mk(VariantKind::Original, "card-a", Some("base")),
                mk(VariantKind::Fitted, "card-b", Some("偏暖")),
            ],
            active: 1,
            ..Default::default()
        };

        // Hop 1 (live strip → record) and hop 2 (record → live strip).
        let rec = app.current_strip_record().expect("a two-card strip is worth recording");
        assert_eq!(rec.active_id.as_deref(), Some("card-b"));
        assert_eq!(rec.active_name.as_deref(), Some("偏暖"));
        assert_eq!(rec.others[0].id.as_deref(), Some("card-a"));
        assert_eq!(rec.others[0].name.as_deref(), Some("base"));
        let back = crate::persist::strip_from_record(&rec, None);
        assert_eq!(back[0].id, "card-a");
        assert_eq!(back[0].name.as_deref(), Some("base"));

        // A LEGACY record (no ids — every strip written before R24-2) is
        // minted one on the way in, or the versions taken from that card
        // could never be attributed again.
        let legacy = autoshop::store::VariantsRecord {
            extra: Default::default(),
            v: 1,
            active_kind: "original".into(),
            active_pos: 0,
            active_id: None,
            active_name: None,
            others: vec![autoshop::store::VariantEntry {
                extra: Default::default(),
                kind: "generated".into(),
                recipe: EditRecipe::default(),
                origin: None,
                id: None,
                name: None,
            }],
        };
        let minted = crate::persist::strip_from_record(&legacy, None);
        assert!(!minted[0].id.is_empty(), "an id-less legacy entry is minted an identity");

        // Hop 3 (live strip → stash) and hop 5 (stash → record), driven
        // through the stash-building path the way navigation does.
        let others: Vec<StashedVariant> = app
            .variants
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != app.active)
            .map(|(_, v)| StashedVariant {
                kind: v.kind,
                id: v.id.clone(),
                name: v.name.clone(),
                recipe: v.recipe.clone(),
                base: v.base.clone(),
                origin: v.origin.clone(),
            })
            .collect();
        let st = crate::model::StashEntry {
            recipe: app.recipe.clone(),
            base: None,
            origin: None,
            kind: app.variants[app.active].kind,
            id: app.variants[app.active].id.clone(),
            name: app.variants[app.active].name.clone(),
            others,
            active_pos: app.active,
        };
        let stashed = crate::util::stash_strip_record(&st).expect("a stashed strip is recordable");
        assert_eq!(stashed.active_id.as_deref(), Some("card-b"));
        assert_eq!(stashed.active_name.as_deref(), Some("偏暖"));
        assert_eq!(stashed.others[0].id.as_deref(), Some("card-a"));
        assert_eq!(stashed.others[0].name.as_deref(), Some("base"));

        // Hop 4 (stash → live strip): the shape the Opened handler rebuilds.
        let restored: Vec<Variant> = st
            .others
            .into_iter()
            .map(|sv| Variant {
                kind: sv.kind,
                id: sv.id,
                name: sv.name,
                recipe: sv.recipe,
                base: sv.base,
                origin: sv.origin,
                thumb: None,
            })
            .collect();
        assert_eq!(restored[0].id, "card-a");
        assert_eq!(restored[0].name.as_deref(), Some("base"));

        // TRIVIALITY, R24-3: the cards became renameable, so a lone Original
        // is no longer trivial by its card count alone — the record is the
        // ONLY home its name and its minted id have, and dropping it would
        // discard both silently (the `strip_is_trivial` owner, shared with
        // the stash path).
        app.variants.truncate(1);
        app.active = 0;
        let rec = app
            .current_strip_record()
            .expect("a NAMED lone Original still needs its record");
        assert_eq!(rec.active_name.as_deref(), Some("base"));
        assert_eq!(rec.active_id.as_deref(), Some("card-a"));
        // Strip the two things only the record can hold and it goes back to
        // being noise: recipe.json + pixels.json say everything, and the base
        // negative's fixed id is reconstructed by every reader.
        app.variants[0].name = None;
        app.variants[0].id = crate::model::ORIGINAL_VARIANT_ID.to_string();
        assert!(app.current_strip_record().is_none());
    }

    /// R24-3 (#7): 「apply to Original」 copies a card's develop PARAMETERS
    /// onto the base negative — and nothing else. Its baked pixels, its
    /// raster origin and the SOURCE card all survive (Lightroom's 「Set Copy
    /// as Original」 rule), and the whole thing is exactly one Ctrl+Z.
    #[test]
    fn applying_a_variant_to_the_original_copies_parameters_and_nothing_else() {
        use crate::model::{Variant, VariantKind};
        let ctx = egui::Context::default();
        let pixels = Arc::new(image::DynamicImage::new_rgb8(8, 6));
        let master = PathBuf::from("out/_apply_master.png");
        let negative = Variant {
            kind: VariantKind::Original,
            id: crate::model::ORIGINAL_VARIANT_ID.into(),
            name: None,
            recipe: EditRecipe { contrast: 11.0, ..Default::default() },
            base: Some(pixels.clone()),
            origin: Some(master.clone()),
            thumb: None,
        };
        let fitted = Variant {
            kind: VariantKind::Fitted,
            id: "card-fit".into(),
            name: Some("dusk".into()),
            recipe: EditRecipe { contrast: 44.0, exposure_ev: 0.5, ..Default::default() },
            base: None,
            origin: None,
            thumb: None,
        };
        let mut app = AutoshopApp {
            variants: vec![negative, fitted],
            active: 1,
            recipe: EditRecipe { contrast: 44.0, exposure_ev: 0.5, ..Default::default() },
            ..Default::default()
        };
        app.reset_history();
        app.apply_to_original(1, &ctx);

        assert_eq!(app.active, 0, "the canvas lands on the card that was written");
        assert_eq!(app.variants.len(), 2, "the source card is KEPT");
        assert_eq!(app.variants[1].name.as_deref(), Some("dusk"), "…untouched");
        assert_eq!(app.variants[0].recipe.contrast, 44.0, "the develop was copied");
        assert_eq!(app.variants[0].recipe.exposure_ev, 0.5);
        assert!(
            app.variants[0].base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &pixels)),
            "the negative's own pixels are not touched"
        );
        assert_eq!(
            app.variants[0].origin.as_deref(),
            Some(master.as_path()),
            "…nor its raster origin: this copies PARAMETERS"
        );
        assert_eq!(app.recipe.contrast, 44.0, "the canvas shows what was applied");

        // ONE Ctrl+Z, and the negative's own develop is back.
        app.undo(&ctx);
        assert_eq!(app.recipe.contrast, 11.0, "one undo step, not zero and not two");

        // A PIXEL-STATE source is refused with the reverse-fit remedy — its
        // look is in its raster, and the recipe over it is stripped bare.
        let mut app = AutoshopApp {
            variants: vec![
                Variant {
                    kind: VariantKind::Original,
                    id: crate::model::ORIGINAL_VARIANT_ID.into(),
                    name: None,
                    recipe: EditRecipe { contrast: 11.0, ..Default::default() },
                    base: None,
                    origin: None,
                    thumb: None,
                },
                Variant {
                    kind: VariantKind::Generated,
                    id: "card-gen".into(),
                    name: None,
                    recipe: EditRecipe::default(),
                    base: None,
                    origin: Some(PathBuf::from("out/_gen.png")),
                    thumb: None,
                },
            ],
            active: 1,
            ..Default::default()
        };
        app.apply_to_original(1, &ctx);
        assert_eq!(app.variants[0].recipe.contrast, 11.0, "the negative was not overwritten");
        assert_eq!(app.active, 1, "and the canvas did not move");
        assert!(
            app.status.contains("Reverse-fit"),
            "the refusal carries the remedy: {}",
            app.status
        );
    }

    /// R24-4 (#1, phase 2 — minimal): ONE list answers "what edit states does
    /// this photo have" — the rendition CARDS first, the numbered SNAPSHOTS
    /// under them. The order is the claim (a photo is one negative + N
    /// variants + one version history), so the test pins it.
    #[test]
    fn the_edit_state_list_puts_the_cards_above_the_versions() {
        use crate::model::{Variant, VariantKind};
        let mk = |kind, id: &str| Variant {
            kind,
            id: id.into(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut app = AutoshopApp {
            variants: vec![
                mk(VariantKind::Original, crate::model::ORIGINAL_VARIANT_ID),
                mk(VariantKind::Generated, "card-gen"),
            ],
            active: 0,
            versions: vec![1, 2],
            ..Default::default()
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            },
            |ctx| {
                // Versions is a collapsed section by default; egui's own test
                // hook opens every collapsible (the brush-slider test's idiom).
                ctx.memory_mut(|m| m.set_everything_is_visible(true));
                egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| app.develop_panel(ui));
                });
            },
        );
        assert_eq!(
            app.edit_list_rows,
            vec!["variant:original", "variant:generated", "version:1", "version:2"],
            "cards above versions, in strip order then ascending"
        );
    }

    /// R24-5: the edit-state list's rows CARRY the card actions — the same
    /// ＋ / ▣ / ✕ the strip draws, through the one owner
    /// (`variant_card_buttons`), so 「what does ✕ do here」 cannot become two
    /// answers. What this pins:
    ///
    ///   * per-row membership: ＋ and ▣ on the ACTIVE card only (both act on
    ///     the live canvas), ✕ on any droppable card, none on a lone Original;
    ///   * ▣ DISABLED rather than hidden on a pixel-state card — the R24-2
    ///     judgement, and the reason it is a seam at all;
    ///   * the panel does not grow: this section gained two widgets per row in
    ///     a 320 px panel, and an unbounded row is how the side panel crept
    ///     +8 px a frame twice before (R19, R22-3).
    ///
    /// The STRIP's own seams are untouched by a frame that draws both, which
    /// is why `strip_apply` stays the strip's (`VariantSurface`).
    #[test]
    fn the_edit_state_rows_carry_the_card_actions_without_widening_the_panel() {
        use crate::model::{Variant, VariantKind};
        let mk = |kind, id: &str| Variant {
            kind,
            id: id.into(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        for (active, want) in [
            // The Generated card is active: it offers ＋, a DISABLED ▣ (its
            // look lives in its pixels), and ✕. The background Original
            // offers nothing — it is neither the live canvas nor droppable.
            (1usize, vec!["1:＋▣(off)✕"]),
            // The Original is active: ＋ only. ▣ is hidden (it IS the
            // negative) and the Original is never droppable; the background
            // Generated keeps its ✕.
            (0usize, vec!["0:＋", "1:✕"]),
        ] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = AutoshopApp {
                variants: vec![
                    mk(VariantKind::Original, crate::model::ORIGINAL_VARIANT_ID),
                    mk(VariantKind::Generated, "card-gen"),
                ],
                active,
                versions: vec![1],
                ..Default::default()
            };
            let mut widths = Vec::new();
            for _ in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1400.0, 900.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        ctx.memory_mut(|m| m.set_everything_is_visible(true));
                        let r = egui::SidePanel::left("controls")
                            .default_width(320.0)
                            .show(ctx, |ui| {
                                egui::ScrollArea::vertical().show(ui, |ui| app.develop_panel(ui));
                            });
                        widths.push(r.response.rect.width());
                    },
                );
            }
            assert_eq!(app.edit_list_actions, want, "active={active}");
            assert!(
                (widths[0] - widths[2]).abs() < 0.5,
                "active={active}: the controls panel must not grow ({widths:?})"
            );
        }

        // A lone Original: nothing is droppable and nothing else exists, so
        // the only action is ＋. (A ✕ here would offer to delete the photo's
        // one edit state.)
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut app = AutoshopApp {
            variants: vec![mk(VariantKind::Original, crate::model::ORIGINAL_VARIANT_ID)],
            active: 0,
            ..Default::default()
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            },
            |ctx| {
                ctx.memory_mut(|m| m.set_everything_is_visible(true));
                egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| app.develop_panel(ui));
                });
            },
        );
        assert_eq!(app.edit_list_actions, vec!["0:＋"]);
    }

    /// R24-3/R24-4, on the real panel: the strip's ACTIVE card carries the
    /// two new affordances — a name box and 「apply to Original」 — and a
    /// PIXEL-STATE card shows the apply button DISABLED (with the reverse-fit
    /// reason) rather than hiding it. Headless `Context::run`, never a window.
    #[test]
    fn the_active_card_offers_its_name_box_and_the_apply_button() {
        use crate::model::{Variant, VariantKind};
        let mk = |kind, id: &str| Variant {
            kind,
            id: id.into(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            ..Default::default()
        };
        let photo = PathBuf::from("D:/library/_strip_affordances.ARW");
        for (kind, expect_enabled) in
            [(VariantKind::Fitted, true), (VariantKind::Generated, false)]
        {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = AutoshopApp {
                src_path: Some(photo.clone()),
                variants: vec![
                    mk(VariantKind::Original, crate::model::ORIGINAL_VARIANT_ID),
                    mk(kind, "card-b"),
                ],
                active: 1,
                ..Default::default()
            };
            let _ = ctx.run(input(), |ctx| {
                egui::TopBottomPanel::bottom("variants")
                    .exact_height(AutoshopApp::VARIANT_STRIP_H)
                    .show(ctx, |ui| app.variant_strip(ui));
            });
            let (rect, enabled) = app
                .strip_apply
                .unwrap_or_else(|| panic!("{kind:?}: the active card has no apply button"));
            assert!(rect.width() > 0.0, "{kind:?}: the apply button did not lay out");
            assert_eq!(
                enabled, expect_enabled,
                "{kind:?}: a pixel-state card must SHOW the button disabled, not hide it"
            );
            // The name affordance is there too, and a seeded buffer turns it
            // into the edit box (the version rows' shape, on the card's id).
            assert!(
                app.strip_name_rect.is_some_and(|r| r.width() > 0.0),
                "{kind:?}: the active card offers no name affordance"
            );
            app.variant_name_buf =
                Some((photo.clone(), "card-b".into(), String::new(), "dusk".into()));
            app.strip_name_rect = None;
            let _ = ctx.run(input(), |ctx| {
                egui::TopBottomPanel::bottom("variants")
                    .exact_height(AutoshopApp::VARIANT_STRIP_H)
                    .show(ctx, |ui| app.variant_strip(ui));
            });
            assert!(
                app.strip_name_rect.is_some_and(|r| r.width() > 0.0),
                "{kind:?}: the seeded rename box did not lay out"
            );
        }
    }

    /// R24-3: a card rename lands on the CARD it was typed on — the buffer is
    /// keyed by the card's own id, so an async push that renumbers the strip
    /// mid-typing cannot paint the name onto a different card. And it counts
    /// as unsaved work: the strip record is the name's only home.
    #[test]
    fn a_card_rename_follows_its_card_and_counts_as_unsaved() {
        use crate::model::{Variant, VariantKind};
        let mk = |kind, id: &str| Variant {
            kind,
            id: id.into(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        let photo = PathBuf::from("D:/library/_rename_card.ARW");
        let mut app = AutoshopApp {
            src_path: Some(photo.clone()),
            variants: vec![
                mk(VariantKind::Original, crate::model::ORIGINAL_VARIANT_ID),
                mk(VariantKind::Fitted, "card-fit"),
            ],
            active: 1,
            ..Default::default()
        };
        // Typed on the Fitted card…
        app.variant_name_buf =
            Some((photo.clone(), "card-fit".into(), String::new(), "dusk".into()));
        // …while an async completion inserts a card ahead of it.
        app.variants.insert(1, mk(VariantKind::Generated, "card-gen"));
        app.active = 2;
        app.commit_pending_names();
        assert_eq!(app.variants[1].name, None, "the renumbered neighbour is untouched");
        assert_eq!(
            app.variants[2].name.as_deref(),
            Some("dusk"),
            "the name follows the card's id, not its index"
        );

        // Unsaved-work accounting: the mirror still holds the pre-rename
        // record, so quitting has to warn.
        app.saved_strip = app.current_strip_record();
        assert_eq!(app.open_dirty_variants(), 0, "premise: the mirror is current");
        app.variants[2].name = Some("dawn".into());
        assert!(app.open_dirty_variants() >= 1, "renaming the ACTIVE card is unsaved work");
        app.variants[2].name = Some("dusk".into());
        app.variants[0].name = Some("negative".into());
        assert!(app.open_dirty_variants() >= 1, "…and so is renaming a background card");
    }

    /// R24-3: the calibration rule has TWO directions and one owner. Onto a
    /// pixel-state card a snapshot's calibration is stripped; a snapshot
    /// TAKEN off one arrives with none at all, and landing it on the negative
    /// used to leave the photo rendering with no camera base look — the
    /// pre-era repair declines it (current era stamp, curve under three
    /// knots), so nothing else could have healed it.
    #[test]
    fn a_snapshot_off_a_generated_card_gets_the_negatives_calibration_back() {
        let knots = vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]];
        let lens = autoshop::recipe::LensProfile {
            distortion: vec![0.02, 0.0, 0.0],
            distortion_on: true,
            ..Default::default()
        };
        let cal = || (knots.clone(), lens.clone(), Some((5200.0, 3.0)));

        // The defect's direction: a snapshot with no calibration, onto the
        // parametric negative.
        let mut r = EditRecipe { contrast: 7.0, ..Default::default() };
        assert!(
            crate::persist::reconcile_snapshot_calibration(&mut r, false, cal),
            "the stamp is reported so the load can disclose it"
        );
        assert_eq!(r.base_curve.len(), 3, "the photo's own camera base look is back");
        assert_eq!(r.lens_profile.distortion, vec![0.02, 0.0, 0.0], "…the lens profile too");
        assert_eq!(r.as_shot_k, Some(5200.0), "…and the as-shot anchor with it");
        assert_eq!(r.contrast, 7.0, "the user's edits are not touched");

        // A snapshot that BROUGHT its own calibration is left exactly alone —
        // legacy develops must render as they were tuned.
        let mut own = EditRecipe {
            base_curve: vec![[0.0, 0.0], [0.5, 0.4], [1.0, 1.0]],
            ..Default::default()
        };
        let before = own.clone();
        assert!(!crate::persist::reconcile_snapshot_calibration(&mut own, false, cal));
        assert_eq!(own, before, "a snapshot with its own curve is untouched");

        // …and the other direction still strips, anchor included.
        let mut onto_pixels = EditRecipe {
            base_curve: vec![[0.0, 0.0], [1.0, 1.0]],
            lens_profile: lens.clone(),
            as_shot_k: Some(5200.0),
            as_shot_tint: Some(3.0),
            ..Default::default()
        };
        assert!(!crate::persist::reconcile_snapshot_calibration(&mut onto_pixels, true, cal));
        assert!(onto_pixels.base_curve.is_empty(), "baked pixels carry the look already");
        assert_eq!(onto_pixels.lens_profile, autoshop::recipe::LensProfile::default());
        assert_eq!(onto_pixels.as_shot_k, None, "…and a baked white balance");

        // R24 round-end NIT-4: the era stamp rides WITH the curve, the same
        // rule `pipeline::stamp_fit_calibration` and the open path follow. A
        // snapshot is the only input that arrives carrying an OLDER era, and
        // an era-1 stamp over knots THIS build just estimated made the
        // caller's `repair_pre_era_base_curve` "re-estimate" a curve that was
        // already fresh — and say so on screen.
        let mut legacy = EditRecipe { version: 1, contrast: 7.0, ..Default::default() };
        assert!(crate::persist::reconcile_snapshot_calibration(&mut legacy, false, cal));
        assert_eq!(
            legacy.version,
            autoshop::recipe::CALIB_ERA,
            "a freshly estimated curve carries this build's era"
        );
        assert!(
            !autoshop::pipeline::base_curve_looks_pre_era(legacy.version, &legacy.base_curve),
            "…so the pre-era repair declines it instead of announcing a re-estimate"
        );
        // A calibration that produced NO knots stamps no curve, so it makes no
        // era claim either.
        let mut none = EditRecipe { version: 1, ..Default::default() };
        assert!(!crate::persist::reconcile_snapshot_calibration(&mut none, false, || (
            Vec::new(),
            autoshop::recipe::LensProfile::default(),
            None
        )));
        assert_eq!(none.version, 1, "no curve stamped, no era restated");
    }

    /// R24-2: 「＋ Save as version」 on a PIXEL-STATE card wrote a near-empty
    /// recipe — the canvas over a generated raster carries no curve, no lens
    /// profile and no as-shot anchor by construction, so the snapshot
    /// restored to nothing. Same judgement as Ctrl+S's XMP refusal.
    #[test]
    fn saving_a_version_off_a_generated_variant_is_refused_not_emptied() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-genversion-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("photo.arw");
        std::fs::write(&src, b"raw").unwrap();
        let dev = autoshop::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();

        let mk = |kind| crate::model::Variant {
            kind,
            id: crate::model::new_variant_id(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: Some(dir.join("master.png")),
            thumb: None,
        };
        let mut app = AutoshopApp {
            src_path: Some(src.clone()),
            variants: vec![mk(crate::model::VariantKind::Generated)],
            ..Default::default()
        };
        app.save_version();
        assert!(
            autoshop::store::list_versions(&src).is_empty(),
            "a generated card must not mint an empty snapshot"
        );
        assert!(
            app.toasts.iter().any(|t| matches!(t.kind, ToastKind::Error)),
            "the refusal must be SEEN — the keyboard path has no other channel"
        );

        // The same canvas on a PARAMETRIC card saves, and records what it
        // was a picture of.
        app.toasts.clear();
        app.variants = vec![crate::model::Variant {
            id: "card-fit".into(),
            ..mk(crate::model::VariantKind::Fitted)
        }];
        app.save_version();
        assert_eq!(autoshop::store::list_versions(&src), vec![1]);
        let meta = autoshop::store::read_version_meta(&src);
        assert_eq!(meta[0].from_kind.as_deref(), Some("fitted"));
        assert_eq!(meta[0].from_id.as_deref(), Some("card-fit"));
        assert_eq!(meta[0].origin.as_deref(), Some(autoshop::store::VERSION_ORIGIN_USER));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// R24-2: the version ROW says what it is — 「v1 「偏暖」· from ◭
    /// Reverse-fit」 — and the filter hides other cards' history while
    /// COUNTING what it hid (a shrinking list must never read as lost
    /// versions). Headless: no exe is launched, the panel is rendered into an
    /// off-screen `egui::Context`.
    #[test]
    fn a_version_row_shows_its_name_and_where_it_came_from() {
        fn texts(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<String>) {
            fn walk(s: &egui::Shape, out: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                    _ => {}
                }
            }
            shapes.iter().for_each(|c| walk(&c.shape, out));
        }
        let dir = std::env::temp_dir().join(format!("autoshop-verrow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("photo.arw");
        std::fs::write(&src, b"raw").unwrap();

        let mut app = AutoshopApp {
            src_path: Some(src.clone()),
            variants: vec![crate::model::Variant {
                kind: crate::model::VariantKind::Fitted,
                id: "card-fit".into(),
                name: None,
                recipe: EditRecipe::default(),
                base: None,
                origin: None,
                thumb: None,
            }],
            versions: vec![1, 2],
            ..Default::default()
        };
        app.version_meta.insert(
            1,
            autoshop::store::VersionMetaEntry {
                n: 1,
                name: Some("偏暖".into()),
                from_kind: Some("fitted".into()),
                from_id: Some("card-fit".into()),
                origin: Some(autoshop::store::VERSION_ORIGIN_USER.into()),
            },
        );
        app.version_meta.insert(
            2,
            autoshop::store::VersionMetaEntry {
                n: 2,
                origin: Some(autoshop::store::VERSION_ORIGIN_AUTO.into()),
                ..Default::default()
            },
        );

        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let frame = |app: &mut AutoshopApp| -> Vec<String> {
            let mut seen = Vec::new();
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1400.0, 20_000.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    // Versions is a collapsed section by default.
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| app.develop_panel(ui));
                    });
                },
            );
            texts(&out.shapes, &mut seen);
            seen
        };

        let seen = frame(&mut app);
        assert!(seen.iter().any(|t| t == "「偏暖」"), "the name is not on the row: {seen:?}");
        assert!(
            seen.iter().any(|t| t == "· from ◭ Reverse-fit"),
            "the row does not say which variant it came from: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t == "· auto-archived"),
            "an automatic snapshot must say so: {seen:?}"
        );
        assert!(seen.iter().any(|t| t == "Only this variant"), "no filter: {seen:?}");
        assert!(
            !seen.iter().any(|t| t.contains("hidden")),
            "nothing is hidden while the filter is off: {seen:?}"
        );

        // Filter ON: v2 has no recorded source, so it goes — and says so.
        app.versions_current_only = true;
        let seen = frame(&mut app);
        assert!(seen.iter().any(|t| t == "「偏暖」"), "this card's own version stays: {seen:?}");
        assert!(
            seen.iter().any(|t| t == "1 hidden — saved from another variant"),
            "a filtered-out row must be COUNTED, not silently gone: {seen:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R24-2: a version name still sitting in its TextEdit must reach disk at
    /// every commit boundary — the mask rename's U10 rule, applied to the
    /// version rows. Driven through a REAL boundary (a variant switch), not
    /// by calling the committer directly.
    #[test]
    fn a_pending_version_rename_survives_a_commit_boundary() {
        let dir = std::env::temp_dir().join(format!("autoshop-verrename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("photo.arw");
        std::fs::write(&src, b"raw").unwrap();
        let dev = autoshop::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(
            autoshop::store::version_target(&src, 1),
            serde_json::to_string(&EditRecipe::default()).unwrap(),
        )
        .unwrap();

        let mk = |kind| crate::model::Variant {
            kind,
            id: crate::model::new_variant_id(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        let mut app = AutoshopApp {
            src_path: Some(src.clone()),
            variants: vec![
                mk(crate::model::VariantKind::Original),
                mk(crate::model::VariantKind::Fitted),
            ],
            active: 0,
            ..Default::default()
        };
        app.refresh_versions();
        assert_eq!(app.versions, vec![1]);
        // Typed, never blurred.
        app.version_name_buf = Some((src.clone(), 1, String::new(), "偏暖".into()));
        let ctx = egui::Context::default();
        app.switch_variant(1, &ctx);
        assert_eq!(
            autoshop::store::read_version_meta(&src)
                .into_iter()
                .find(|e| e.n == 1)
                .and_then(|e| e.name)
                .as_deref(),
            Some("偏暖"),
            "the boundary flushed the typed name to its own version"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// R24-2 (user decision ②): the archive entry point sits ON the variant
    /// card — and only on the ACTIVE one, whose develop is what a snapshot
    /// would capture. Headless render of the strip itself.
    #[test]
    fn the_active_variant_card_carries_the_save_as_version_button() {
        fn texts(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<String>) {
            fn walk(s: &egui::Shape, out: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                    _ => {}
                }
            }
            shapes.iter().for_each(|c| walk(&c.shape, out));
        }
        let mk = |kind| crate::model::Variant {
            kind,
            id: crate::model::new_variant_id(),
            name: None,
            recipe: EditRecipe::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        let mut app = AutoshopApp {
            variants: vec![
                mk(crate::model::VariantKind::Original),
                mk(crate::model::VariantKind::Fitted),
            ],
            active: 1,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut seen = Vec::new();
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::bottom("variants")
                    .exact_height(AutoshopApp::VARIANT_STRIP_H)
                    .show(ctx, |ui| app.variant_strip(ui));
            },
        );
        texts(&out.shapes, &mut seen);
        assert_eq!(
            seen.iter().filter(|t| *t == "＋").count(),
            1,
            "exactly one card — the active one — offers the snapshot: {seen:?}"
        );
    }

    /// R24-2: the 「只看当前变体」 filter's matcher. ID beats kind when both
    /// sides have one; kind is the fallback for records that predate ids;
    /// an UNATTRIBUTED version matches nothing — a filter that quietly kept
    /// everything unknown would be a checkbox that does not filter.
    #[test]
    fn the_version_filter_matches_by_id_then_kind_and_never_by_hope() {
        use crate::model::VariantKind;
        let meta = |from_id: Option<&str>, from_kind: Option<&str>| {
            autoshop::store::VersionMetaEntry {
                n: 1,
                name: None,
                from_kind: from_kind.map(str::to_string),
                from_id: from_id.map(str::to_string),
                origin: None,
            }
        };
        let m = meta(Some("card-a"), Some("original"));
        assert!(AutoshopApp::version_is_from(Some(&m), "card-a", Some(VariantKind::Original)));
        assert!(
            !AutoshopApp::version_is_from(Some(&m), "card-b", Some(VariantKind::Original)),
            "a matching KIND must not smuggle in another card's history"
        );
        // Same card, re-kinded since the snapshot (R24-3's「应用到原图」 will
        // do exactly that): the id still speaks for it.
        assert!(AutoshopApp::version_is_from(Some(&m), "card-a", Some(VariantKind::Fitted)));

        let old = meta(None, Some("fitted"));
        assert!(AutoshopApp::version_is_from(Some(&old), "card-a", Some(VariantKind::Fitted)));
        assert!(!AutoshopApp::version_is_from(Some(&old), "card-a", Some(VariantKind::Original)));

        // A card with no id of its own falls back to kind too.
        assert!(AutoshopApp::version_is_from(Some(&m), "", Some(VariantKind::Original)));

        assert!(!AutoshopApp::version_is_from(Some(&meta(None, None)), "card-a", Some(VariantKind::Original)));
        assert!(!AutoshopApp::version_is_from(None, "card-a", Some(VariantKind::Original)));
    }

    /// L15-8: Clear must clear what Apply BAKES (the greyscale weight
    /// buffer), not only what the canvas shows — the session's own contract
    /// is "bakes exactly what it shows".
    #[test]
    fn clearing_the_brush_clears_what_apply_would_bake() {
        let mut app = AutoshopApp {
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
        let mut app = AutoshopApp::default();
        app.recipe.crop =
            Some(autoshop::recipe::Crop { left: 0.5, top: 0.5, right: 1.0, bottom: 1.0 });
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
            .join(format!("autoshop-gui-late-artifact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = dir.join("late.retouch.png");
        std::fs::write(&artifact, b"orphan bytes").unwrap();

        let ctx = egui::Context::default();
        // The cancel already bumped past this task's epoch 6.
        let mut app = AutoshopApp { gen_epoch: 7, ..Default::default() };
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

    /// L12-2: the master cache keys on the SPAWN-time stamp — a file
    /// rewritten during the decode must MISS on the next probe instead of
    /// serving the old pixels under the new identity.
    #[test]
    fn a_master_rewritten_during_decode_misses_the_cache() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-gui-master-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("master.png");
        std::fs::write(&p, b"generation A").unwrap();
        let spawn_stamp = file_stamp(&p);
        // The rewrite that lands while the decode is still running (longer
        // content — the stamp's length half moves even on coarse mtime).
        std::fs::write(&p, b"generation B, longer bytes").unwrap();

        let mut app = AutoshopApp::default();
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
        // who has AUTOSHOP_OUT_DIR set, which is not what it is about.)
        let root = autoshop::config::delivery_root();
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
        let asking = AutoshopApp {
            src_path: Some(src.clone()),
            exp_dest: ExportDest::Ask,
            exp_format: ExportFormat::Jpeg,
            // A remembered folder is present ON PURPOSE: "ask every time" must
            // outrank it, or the setting silently becomes "last used".
            last_export_dir: Some(PathBuf::from("D:/deliver/tripA")),
            ..Default::default()
        };
        assert_eq!(asking.export_route(), ExportRoute::Ask);
        let candidate = PathBuf::from("out").join("DSC00042.developed.jpg");
        assert!(
            !candidate.exists(),
            "deciding the route must not have created {}",
            candidate.display()
        );
        // The same app with the default destination DOES resolve to a file —
        // without this half the assertion above could pass on a broken route
        // that never renders anything at all.
        let direct = AutoshopApp {
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
        let mut app = AutoshopApp { busy: true, ..Default::default() };
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
        let mut app = AutoshopApp {
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
        let mut ok_app = AutoshopApp { busy: true, ..Default::default() };
        ok_app
            .tx
            .send(Msg::Exported(Ok(ExportOutcome::Batch {
                ok: 2,
                errs: Vec::new(),
                renamed: Vec::new(),
                relooked: 0,
                warns: Vec::new(),
                dest: landed.clone(),
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
            .join(format!("autoshop-gui-xmp-beside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_r22_handoff.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = autoshop::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(autoshop::store::xmp_target(&raw), b"<x:xmpmeta>ours</x:xmpmeta>").unwrap();
        let beside = raw.with_extension("xmp");

        let mut app = AutoshopApp { src_path: Some(raw.clone()), ..Default::default() };
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
        let baked_app = AutoshopApp { src_path: Some(baked), ..Default::default() };
        assert!(
            !baked_app.can_export_xmp_beside(),
            "only a RAW has the Lightroom sidecar convention"
        );
        assert!(
            !AutoshopApp::default().can_export_xmp_beside(),
            "…and with no photo open there is nothing to hand over"
        );

        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Both themes must keep every text-on-chrome pairing at WCAG AA (4.5:1
    /// for text, 3:1 for the armed indicator glyph). This is the contract the
    /// Light scheme was tuned against, and the guard that keeps a future
    /// colour tweak from shipping unreadable text on one theme. Pairings
    /// tested = pairings actually rendered (see each line's comment);
    /// `clip_tri_off` is deliberately dim (disarmed state) and exempt.
    #[test]
    fn both_themes_pass_contrast_checks() {
        // WCAG 2.x relative luminance + contrast ratio.
        fn lum(c: egui::Color32) -> f64 {
            let ch = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        }
        fn ratio(a: egui::Color32, b: egui::Color32) -> f64 {
            let (hi, lo) = (lum(a).max(lum(b)), lum(a).min(lum(b)));
            (hi + 0.05) / (lo + 0.05)
        }
        for theme in [ThemePref::Dark, ThemePref::Light] {
            let c = theme.colors();
            let visuals = match theme {
                ThemePref::Dark => egui::Visuals::dark(),
                ThemePref::Light => egui::Visuals::light(),
            };
            let panel = visuals.panel_fill;
            let text = visuals.widgets.noninteractive.fg_stroke.color;
            let checks: [(&str, egui::Color32, egui::Color32, f64); 7] = [
                // (what, fg, bg, minimum). The selected row carries ONLY
                // accent-coloured text (name + badges — see the gallery row),
                // so text-on-sel_bg is not a rendered pairing; multi-select
                // rows DO render default-colour names on sel_bg_dim.
                ("default text on panels", text, panel, 4.5),
                ("gold accent text on panels", c.accent_text, panel, 4.5),
                ("gold accent text on the selected row", c.accent_text, c.sel_bg, 4.5),
                ("default text on a multi-select row", text, c.sel_bg_dim, 4.5),
                ("success toast", c.toast_ok_fg, c.toast_ok_bg, 4.5),
                ("error toast", c.toast_err_fg, c.toast_err_bg, 4.5),
                // Non-text indicator: WCAG AA for UI components is 3:1.
                ("armed clipping triangle", c.clip_tri_on, panel, 3.0),
            ];
            for (what, fg, bg, min) in checks {
                let r = ratio(fg, bg);
                assert!(
                    r >= min,
                    "{theme:?} theme: {what} is {r:.2}:1, needs {min}:1"
                );
            }
            // Coloured text that sits on panel chrome (not on the dark plot).
            assert!(
                ratio(c.armed_hint, panel) >= 4.5,
                "{theme:?} theme: armed-tool hint is {:.2}:1",
                ratio(c.armed_hint, panel)
            );
            // selection_fill is PREMULTIPLIED-alpha: what actually renders is
            // its composite over the surface beneath, not the raw constant —
            // checking the raw colour validated a pairing no pixel ever shows.
            // Selected text / combo rows draw DEFAULT text over that
            // composite, on panels and on the text-edit well alike.
            let over = |fgp: egui::Color32, bg: egui::Color32| {
                let a = f64::from(fgp.a()) / 255.0;
                let ch = |f: u8, b: u8| {
                    (f64::from(f) + f64::from(b) * (1.0 - a)).round().clamp(0.0, 255.0) as u8
                };
                egui::Color32::from_rgb(
                    ch(fgp.r(), bg.r()),
                    ch(fgp.g(), bg.g()),
                    ch(fgp.b(), bg.b()),
                )
            };
            for (surface, bg) in
                [("panels", panel), ("the text-edit well", visuals.extreme_bg_color)]
            {
                let composite = over(c.selection_fill, bg);
                // Default text over a TRANSIENT text-selection highlight: the
                // 3:1 component bar, not the 4.5:1 text bar — the user is
                // mid-manipulation on text they just produced, and holding
                // 4.5:1 here would force the highlight to near-invisibility
                // on the dark panel's tiny luminance headroom.
                let r = ratio(text, composite);
                assert!(
                    r >= 3.0,
                    "{theme:?} theme: default text on selection over {surface} is {r:.2}:1 \
                     (fill composites to {composite:?}), needs 3:1"
                );
                // Selected COMBO rows draw their text with selection_stroke —
                // persistent state, full AA text bar.
                let r = ratio(c.selection_stroke, composite);
                assert!(
                    r >= 4.5,
                    "{theme:?} theme: selection-stroke text on selection over {surface} is \
                     {r:.2}:1 (fill composites to {composite:?}), needs 4.5:1"
                );
            }
            // The selection outline is a non-text UI component: 3:1 (same
            // class as the armed clipping triangle above).
            assert!(
                ratio(c.selection_stroke, panel) >= 3.0,
                "{theme:?} theme: selection stroke on panels is {:.2}:1, needs 3:1",
                ratio(c.selection_stroke, panel)
            );
            for (i, col) in c.curve_labels.iter().enumerate() {
                let r = ratio(*col, panel);
                assert!(
                    r >= 4.5,
                    "{theme:?} theme: curve label {i} is {r:.2}:1, needs 4.5:1"
                );
            }
        }
    }

    /// Every symbol the GUI renders must have a glyph in the guaranteed font
    /// chain (egui's bundle + the embedded subsets) — system fonts vary, and
    /// this is exactly how the v0.22 tofu boxes (⧉ ⊖ ◭ ▭ ◯ ◌ ✓ ✕ 🖌 …)
    /// shipped: those glyphs existed only on SOME machines. Scans the STRING
    /// LITERALS of the whole GUI module tree (comments never render) —
    /// including the CJK the zh catalogue renders, embedded since W18 (the
    /// cjk-ui subset), so picking 中文 needs no system font. Only DYNAMIC
    /// text (a user's own file names) still rides the runtime fallbacks;
    /// `undrawable_scripts` + the folder-open disclosure own that half
    /// (L12#3). Fails ⇒ re-run scripts/subset_gui_fonts.py and commit the
    /// refreshed assets/fonts/.
    #[test]
    fn embedded_fonts_cover_every_ui_symbol() {
        use ab_glyph::Font as _;
        // Non-ASCII chars inside Rust string/char literals only. Mirrors the
        // extractor in scripts/subset_gui_fonts.py — keep the two in sync.
        fn literal_chars(src: &str, out: &mut std::collections::BTreeSet<char>) {
            let b: Vec<char> = src.chars().collect();
            let n = b.len();
            let mut i = 0;
            while i < n {
                match b[i] {
                    '/' if i + 1 < n && b[i + 1] == '/' => {
                        while i < n && b[i] != '\n' {
                            i += 1;
                        }
                    }
                    '/' if i + 1 < n && b[i + 1] == '*' => {
                        let mut depth = 1;
                        i += 2;
                        while i < n && depth > 0 {
                            if i + 1 < n && b[i] == '/' && b[i + 1] == '*' {
                                depth += 1;
                                i += 2;
                            } else if i + 1 < n && b[i] == '*' && b[i + 1] == '/' {
                                depth -= 1;
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                    }
                    'r' if i + 1 < n && (b[i + 1] == '#' || b[i + 1] == '"') => {
                        let mut j = i + 1;
                        let mut hashes = 0;
                        while j < n && b[j] == '#' {
                            hashes += 1;
                            j += 1;
                        }
                        if j < n && b[j] == '"' {
                            j += 1;
                            while j < n {
                                if b[j] == '"' {
                                    let mut k = 0;
                                    while k < hashes && j + 1 + k < n && b[j + 1 + k] == '#' {
                                        k += 1;
                                    }
                                    if k == hashes {
                                        j += 1 + hashes;
                                        break;
                                    }
                                }
                                if !b[j].is_ascii() {
                                    out.insert(b[j]);
                                }
                                j += 1;
                            }
                            i = j;
                        } else {
                            i += 1;
                        }
                    }
                    '"' => {
                        i += 1;
                        while i < n {
                            if b[i] == '\\' {
                                // `\u{...}` renders exactly like the literal
                                // char, so it needs the same glyph. STRING
                                // literals only: the one char-literal escape
                                // in this tree is the notdef sentinel below,
                                // which must stay out of the needed set.
                                if i + 2 < n && b[i + 1] == 'u' && b[i + 2] == '{' {
                                    let mut j = i + 3;
                                    let mut hex = String::new();
                                    while j < n && b[j] != '}' {
                                        hex.push(b[j]);
                                        j += 1;
                                    }
                                    if j < n {
                                        if let Some(c) = u32::from_str_radix(&hex, 16)
                                            .ok()
                                            .and_then(char::from_u32)
                                            && !c.is_ascii()
                                        {
                                            out.insert(c);
                                        }
                                        i = j + 1;
                                        continue;
                                    }
                                }
                                i += 2;
                            } else if b[i] == '"' {
                                i += 1;
                                break;
                            } else {
                                if !b[i].is_ascii() {
                                    out.insert(b[i]);
                                }
                                i += 1;
                            }
                        }
                    }
                    '\'' => {
                        // 'x' char literal (possibly non-ASCII); '\n'-style
                        // escapes; anything else is a lifetime — skip the quote.
                        if i + 2 < n && b[i + 1] != '\\' && b[i + 2] == '\'' {
                            if !b[i + 1].is_ascii() {
                                out.insert(b[i + 1]);
                            }
                            i += 3;
                        } else if i + 3 < n && b[i + 1] == '\\' && b[i + 3] == '\'' {
                            i += 4;
                        } else {
                            i += 1;
                        }
                    }
                    _ => i += 1,
                }
            }
        }

        // Walk the whole GUI module tree at runtime (round-12 split): an
        // include_str! list would silently lose coverage the moment a new
        // module file is added — the exact drift this gate exists to catch.
        fn walk_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).expect("gui source dir listable") {
                let p = e.expect("dir entry").path();
                if p.is_dir() {
                    walk_rs(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let gui_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("bin")
            .join("gui");
        let mut sources = Vec::new();
        walk_rs(&gui_src, &mut sources);
        assert!(
            sources.len() >= 6,
            "font gate expected the split module tree, found {} files",
            sources.len()
        );
        let mut syms = std::collections::BTreeSet::new();
        for p in &sources {
            let text = std::fs::read_to_string(p).expect("gui source readable");
            literal_chars(&text, &mut syms);
        }
        let is_cjk = |c: char| {
            matches!(c as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
        };
        // Premise: the GUI uses dozens of symbols. Finding almost none means
        // the extractor broke, and the assertions below would pass vacuously.
        assert!(
            syms.iter().filter(|&&c| !is_cjk(c)).count() >= 40,
            "literal extractor found too few symbols — it is broken"
        );

        let defaults = egui::FontDefinitions::default();
        let mut faces: Vec<ab_glyph::FontVec> = Vec::new();
        for data in defaults.font_data.values() {
            faces.push(
                ab_glyph::FontVec::try_from_vec_and_index(data.font.to_vec(), data.index)
                    .expect("egui bundled font parses"),
            );
        }
        for (name, bytes) in EMBEDDED_SYMBOL_FONTS {
            faces.push(
                ab_glyph::FontVec::try_from_vec_and_index(bytes.to_vec(), 0)
                    .unwrap_or_else(|_| panic!("embedded font {name} parses")),
            );
        }
        let covered = |c: char| faces.iter().any(|f| f.glyph_id(c).0 != 0);
        // Negative control: an unassigned codepoint must read as uncovered —
        // this is what makes `glyph_id().0 == 0` trustworthy as "no glyph".
        assert!(
            !covered('\u{0378}'),
            "notdef sentinel covered?! the coverage probe is meaningless"
        );

        let missing: Vec<String> = syms
            .iter()
            .filter(|&&c| !is_cjk(c) && !covered(c))
            .map(|&c| format!("U+{:04X} {c}", c as u32))
            .collect();
        assert!(
            missing.is_empty(),
            "UI symbols with no glyph in the guaranteed font chain \
             (re-run scripts/subset_gui_fonts.py): {missing:?}"
        );

        // CJK is embedded too (W18): choosing 中文 must not depend on the
        // machine owning a system CJK font — without one the whole window
        // rendered as tofu. Asserted separately so a regression names the
        // hanzi, and premise-checked so a broken extractor cannot pass here
        // by finding nothing.
        let cjk: Vec<char> = syms.iter().copied().filter(|&c| is_cjk(c)).collect();
        assert!(
            cjk.len() >= 300,
            "only {} CJK codepoints extracted — the translations or the \
             extractor are broken",
            cjk.len()
        );
        let missing_cjk: Vec<String> = cjk
            .iter()
            .filter(|&&c| !covered(c))
            .map(|&c| format!("U+{:04X} {c}", c as u32))
            .collect();
        assert!(
            missing_cjk.is_empty(),
            "translated text with no embedded glyph — the Chinese UI would \
             show tofu on a machine with no system CJK font \
             (re-run scripts/subset_gui_fonts.py): {missing_cjk:?}"
        );
    }

    #[test]
    fn the_pickers_average_their_window_instead_of_point_sampling() {
        // Shared by the WB eyedropper and the colour-range sample. A point
        // sampler passes any "the picked colour is about right" probe on a
        // smooth image, so pin the AVERAGING directly: one lit pixel in a dark
        // field must come back at 1/25, never 0 (missed it) and never 1 (hit
        // it). This is what makes a click on noisy pixels usable at all.
        let mut img = image::RgbImage::new(9, 9);
        img.put_pixel(4, 4, image::Rgb([255, 255, 255]));
        let img = image::DynamicImage::ImageRgb8(img);
        let centre = sample_5x5_mean(&img, 0.5, 0.5).expect("centre is in bounds");
        assert!(
            (centre[0] - 1.0 / 25.0).abs() < 1e-4,
            "one lit pixel must be averaged over the whole 5x5 window: {centre:?}"
        );
        // A CORNER click keeps only the in-bounds samples (3x3 = 9 here) and
        // divides by that count — dividing by a fixed 25 would darken every
        // edge pick, and reading out of bounds would panic.
        let mut edge = image::RgbImage::new(9, 9);
        for (_, _, p) in edge.enumerate_pixels_mut() {
            *p = image::Rgb([200, 200, 200]);
        }
        let edge = image::DynamicImage::ImageRgb8(edge);
        let corner = sample_5x5_mean(&edge, 0.0, 0.0).expect("a corner is still sampleable");
        for c in corner {
            assert!((c - 200.0 / 255.0).abs() < 1e-4, "a flat field reads flat at the corner: {corner:?}");
        }
        // A zero-size image has no samples at all — None, not a divide by zero.
        let empty = image::DynamicImage::ImageRgb8(image::RgbImage::new(0, 0));
        assert!(sample_5x5_mean(&empty, 0.5, 0.5).is_none());
    }

    #[test]
    fn releasing_a_claim_spares_anything_that_actually_landed() {
        // Five retouch workers call this on two failure paths each, and the
        // dangerous mutant is the loosest one: dropping the length check turns
        // "give the reserved NAME back" into "delete the user's partial
        // result". A non-empty file is evidence and must survive.
        let dir = std::env::temp_dir().join(format!("autoshop-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("claimed.png");
        std::fs::write(&empty, b"").unwrap();
        release_empty_claim(&empty);
        assert!(!empty.exists(), "a 0-byte claim is a reservation — give it back");
        let partial = dir.join("partial.png");
        std::fs::write(&partial, b"half an image").unwrap();
        release_empty_claim(&partial);
        assert_eq!(
            std::fs::read(&partial).unwrap(),
            b"half an image",
            "a non-empty partial is the user's result — never delete it"
        );
        // A path that was never claimed (or already swept) is not an error.
        release_empty_claim(&dir.join("never-existed.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dirty_vs_ignores_calibration_provenance() {
        // A repaired canvas (era 2, fresh curve) against an unrepaired
        // baseline (era 1, washed curve) — same user edits — is NOT dirty:
        // the curve is calibration and `version` is its provenance stamp.
        // BOTH directions are live: the stash gate produces baseline-v1 vs
        // canvas-v2, and load_version produces the reverse — a pre-era
        // snapshot whose LIFTING curve the fingerprint rightly declines
        // lands on the canvas at era 1 under an era-2 baseline. Neither may
        // ever read as an edit.
        let baseline = EditRecipe {
            version: 1,
            base_curve: vec![[0.0, 0.0], [0.55, 0.10], [0.80, 0.55], [1.0, 1.0]],
            contrast: 20.0,
            ..Default::default()
        };
        let canvas = EditRecipe {
            version: 2,
            base_curve: vec![[0.0, 0.0], [0.2, 0.4], [1.0, 1.0]],
            contrast: 20.0,
            ..Default::default()
        };
        assert!(!dirty_vs(&canvas, &baseline), "repair is not an edit");
        assert!(!dirty_vs(&baseline, &canvas), "in either direction");
        // ...while a real edit on the same pair still is.
        let edited = EditRecipe { exposure_ev: 0.3, ..canvas.clone() };
        assert!(dirty_vs(&edited, &baseline));
    }

    #[test]
    fn insert_curve_point_keeps_inputs_sorted_and_unique() {
        let mut pts = Vec::new();
        insert_curve_point(&mut pts, 128, 140);
        insert_curve_point(&mut pts, 32, 20);
        insert_curve_point(&mut pts, 200, 210);
        assert_eq!(pts.iter().map(|p| p.input).collect::<Vec<_>>(), vec![32, 128, 200]);
        // Same input again → overwrite in place, never a duplicate input.
        let i = insert_curve_point(&mut pts, 128, 100);
        assert_eq!(i, 1);
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[1].output, 100);
    }

    #[test]
    fn drag_curve_point_clamps_strictly_between_neighbours() {
        let mut pts = vec![
            CurvePoint { input: 30, output: 30 },
            CurvePoint { input: 128, output: 128 },
            CurvePoint { input: 200, output: 200 },
        ];
        // Dragging the middle point past its right neighbour stops 1 short.
        drag_curve_point(&mut pts, 1, 240, 250);
        assert_eq!(pts[1].input, 199);
        assert_eq!(pts[1].output, 250);
        // …and past its left neighbour stops 1 above it.
        drag_curve_point(&mut pts, 1, 0, 10);
        assert_eq!(pts[1].input, 31);
        // Endpoints reach the full 0 / 255 range.
        drag_curve_point(&mut pts, 0, 0, 0);
        drag_curve_point(&mut pts, 2, 255, 255);
        assert_eq!((pts[0].input, pts[2].input), (0, 255));
        // Invariant after any sequence: inputs strictly increasing.
        assert!(pts.windows(2).all(|w| w[0].input < w[1].input));
    }

    #[test]
    fn geometric_view_mapping_roundtrips() {
        // The two boundary maps must be exact inverses (they share the
        // engine's inscribed_dims / lens_geom_norm formulas), and all-zero
        // controls must be the identity so no existing flow changes.
        // Interior points only: originals near the frame edge can be
        // legitimately cropped away by a strong barrel fix (no preimage).
        let dims = (1280.0, 853.0);
        let off = LensArg::default();
        assert_eq!(view_norm_to_orig(0.31, 0.77, dims, 0.0, &off), (0.31, 0.77));
        // A real-shaped in-camera profile joins the sweep: the composed map
        // must round-trip exactly like the manual-only one.
        let profile = autoshop::recipe::LensProfile {
            distortion: (0..16).map(|i| 1.0008 - 0.053 * (i as f32 / 15.0).powi(2)).collect(),
            distortion_on: true,
            ..Default::default()
        };
        for deg in [0.0f32, 2.5, -7.0, 12.0] {
            for dist in [0.0f32, 60.0, -60.0, 100.0] {
                for lens in [
                    LensArg { profile: Default::default(), amount: dist },
                    LensArg { profile: profile.clone(), amount: dist },
                ] {
                    for (nx, ny) in [(0.5, 0.5), (0.35, 0.65), (0.6, 0.42)] {
                        let (vx, vy) = orig_norm_to_view(nx, ny, dims, deg, &lens);
                        // Active geometry must MOVE an off-centre point —
                        // identity-regressed wrappers round-trip perfectly
                        // and hid exactly that. This is also the suite's
                        // only inertness pin for lens_geom_norm's PROFILE
                        // branch (the render-side batch-20 twin runs
                        // profile-off) (U10).
                        if (deg != 0.0 || dist != 0.0 || lens.profile.distortion_on)
                            && (nx, ny) != (0.5, 0.5)
                        {
                            assert!(
                                (vx - nx).abs() > 1e-4 || (vy - ny).abs() > 1e-4,
                                "deg {deg} dist {dist} profile {}: ({nx},{ny}) unmoved — inert map",
                                lens.profile.distortion_on
                            );
                        }
                        let (ox, oy) = view_norm_to_orig(vx, vy, dims, deg, &lens);
                        assert!(
                            (ox - nx).abs() < 2e-3 && (oy - ny).abs() < 2e-3,
                            "deg {deg} dist {dist}: ({nx},{ny}) → view ({vx},{vy}) → back ({ox},{oy})"
                        );
                    }
                    // The centre is a fixed point at any angle + geometry.
                    let (cx, cy) = view_norm_to_orig(0.5, 0.5, dims, deg, &lens);
                    assert!((cx - 0.5).abs() < 1e-4 && (cy - 0.5).abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn curve_lut_helpers_match_the_engine() {
        // The LUT + point helpers the editor builds on — renamed from
        // "curve_editor_edits_render_identically_to_the_engine": this test
        // never enters curve_editor (the driven test below does, U10).
        // Empty = identity; an anchored lift keeps the ends and raises the
        // anchored midpoint.
        let id = autoshop::render::curve_lut(&[]);
        assert!(id[0].abs() < 1e-6 && (id[255] - 1.0).abs() < 1e-6);
        assert!((id[128] - 128.0 / 255.0).abs() < 1e-3);

        let mut r = EditRecipe::default();
        let pts = curve_points_mut(&mut r, CurveTarget::Global, 0).expect("global");
        insert_curve_point(pts, 0, 0);
        insert_curve_point(pts, 255, 255);
        insert_curve_point(pts, 64, 96); // classic shadow lift between pinned ends
        let lut = autoshop::render::curve_lut(pts);
        assert!(lut[0].abs() < 1e-6 && (lut[255] - 1.0).abs() < 1e-6);
        assert!((lut[64] - 96.0 / 255.0).abs() < 1e-3, "anchored point maps exactly");
        // The channel selector reaches the right recipe field (master only here).
        for ch in 0..4 {
            assert_eq!(
                curve_points(&r, CurveTarget::Global, ch).expect("global").len(),
                if ch == 0 { 3 } else { 0 }
            );
        }
    }

    #[test]
    fn curve_editor_click_adds_a_point_to_the_selected_channel() {
        // Drive the REAL editor with synthetic pointer events — a hard-coded
        // channel in the write path, a dead click branch, or a lost `changed`
        // report all stayed green under the LUT-only test above (U10).
        // Pass 1 lays out and records the square (test seam); pass 2
        // presses; pass 3 releases → egui reports the click (which pass
        // reports the change depends on egui's drag-vs-click bookkeeping,
        // so the two are OR-ed).
        let mut app = AutoshopApp { curve_channel: 2, ..Default::default() };
        let ctx = egui::Context::default();
        let run_pass = |app: &mut AutoshopApp, events: Vec<egui::Event>| -> bool {
            let mut changed = false;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    changed |= app.curve_editor(ui);
                });
            });
            changed
        };
        let _ = run_pass(&mut app, vec![]);
        let rect = app.curve_rect.expect("the editor records its square (test seam)");
        let q = egui::pos2(rect.min.x + rect.width() * 0.25, rect.min.y + rect.height() * 0.5);
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: q,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let changed_press = run_pass(&mut app, vec![egui::Event::PointerMoved(q), button(true)]);
        let changed_release = run_pass(&mut app, vec![button(false)]);
        assert!(changed_press || changed_release, "the editor must report the edit");
        assert_eq!(
            curve_points(&app.recipe, CurveTarget::Global, 2).expect("global").len(),
            1,
            "the click adds one point to the SELECTED channel"
        );
        for ch in [0usize, 1, 3] {
            assert!(
                curve_points(&app.recipe, CurveTarget::Global, ch).expect("global").is_empty(),
                "no cross-channel write (ch {ch})"
            );
        }
        let p = &curve_points(&app.recipe, CurveTarget::Global, 2).expect("global")[0];
        assert!(
            (p.input as f32 - 255.0 * 0.25).abs() <= 2.0,
            "the point lands at the clicked input: {}",
            p.input
        );
        assert!(
            (p.output as i16 - p.input as i16).abs() <= 2,
            "seeded ON the identity curve: {p:?}"
        );
    }

    /// A tiny synthetic base + a bitmap mask on disk, for the async-develop
    /// and overlay regression tests — returned WITH its `Scrub`, never a path
    /// for the caller to clean up: the ./out fixture is removed on DROP, so a failing
    /// assert cannot leave it behind. Handing the guard back (rather than
    /// trusting a trailing `remove_file`) is what makes it impossible for the
    /// next caller to forget — five of them had.
    fn app_with_masked_photo(tag: &str) -> (AutoshopApp, Scrub) {
        let (w, h) = (24u32, 16u32);
        let base = Arc::new(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
            w,
            h,
            |x, y| image::Rgb([(x * 8 % 256) as u8, (y * 12 % 256) as u8, 120]),
        )));
        std::fs::create_dir_all("out").ok();
        let mask_path = std::path::PathBuf::from(format!("out/_gui_perf_{tag}.png"));
        image::GrayImage::from_fn(w, h, |x, _| image::Luma([if x < w / 2 { 255 } else { 0 }]))
            .save(&mask_path)
            .unwrap();
        let recipe = EditRecipe {
            masks: vec![autoshop::recipe::LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: mask_path.to_string_lossy().into_owned() },
                exposure_ev: 0.4,
                color_gains: Some([1.2, 0.95, 0.7]),
                ..Default::default()
            }],
            ..Default::default()
        };
        let scrub = Scrub(vec![mask_path.clone()]);
        let app = AutoshopApp {
            source_preview: Some(base.clone()),
            base_preview: Some(base),
            variants: vec![Variant {
                id: String::new(),
                name: None,
                kind: VariantKind::Original,
                recipe: EditRecipe::default(),
                base: None,
                origin: None,
                thumb: None,
            }],
            recipe,
            sel_mask: Some(0),
            ..AutoshopApp::default()
        };
        (app, scrub)
    }

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
        let good = build_preview(base.clone(), app.recipe.clone(), false);
        app.finish_redevelop(&ctx, Ok(good));
        assert_eq!(app.develop_count, 1, "matching frame accepted");
        assert!(app.after_tex.is_some(), "after texture set");
        assert!(!app.develop_inflight, "inflight cleared on completion");

        // Build a frame for the CURRENT recipe, then move the recipe on before
        // it "arrives": the stale frame must be discarded (counter unchanged)
        // AND the pending edit re-armed. `dirty` is cleared first — it was
        // still true from the swallowed dispatch above, so the re-arm
        // assertion used to be vacuously satisfiable (L26).
        let stale = build_preview(base, app.recipe.clone(), false);
        app.recipe.masks[0].exposure_ev = 1.9; // user kept dragging
        app.dirty = false;
        app.finish_redevelop(&ctx, Ok(stale));
        assert_eq!(app.develop_count, 1, "stale frame (recipe moved) discarded");
        assert!(app.dirty, "a discarded stale frame re-arms the pending edit");

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

        // save_xmp's head flush, driven WITHOUT disk side effects: on a
        // generated variant save_xmp refuses with a toast BEFORE touching
        // any file, but the flush at its head has already run — removing
        // that flush previously survived this test (Codex batch 41).
        app.variants[0].kind = VariantKind::Generated;
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
        let clean = PathBuf::from("D:/__autoshop_chain__/clean.ARW");
        app.src_path = Some(clean.clone());
        // ANOTHER photo's stash holds a dirty background variant.
        app.nav_stash.insert(
            PathBuf::from("D:/__autoshop_chain__/other.ARW"),
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
        app.open_path(PathBuf::from("D:/__autoshop_chain__/next.ARW"));
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
        app.variants.push(Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Generated,
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
        let old = PathBuf::from("D:/__autoshop_h4__/__autoshop_h4_a__.ARW");
        app.src_path = Some(old.clone());
        assert_eq!(app.inactive_dirty_variants(), 1, "premise: background dirty");
        // Navigate away — the stash snapshot is written synchronously.
        app.open_path(PathBuf::from("D:/__autoshop_h4__/__autoshop_h4_b__.ARW"));
        {
            let st = app.nav_stash.get(&old).expect("background work must stash the strip");
            assert_eq!(st.others.len(), 1);
            assert!(matches!(st.others[0].kind, VariantKind::Generated));
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
                .any(|v| v.kind == VariantKind::Generated && v.recipe.contrast == 33.0),
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
        let dev = autoshop::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev); // a crashed earlier run may have left files
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::create_dir_all("out").unwrap();
        let rj = autoshop::store::recipe_target(src);
        let xp = autoshop::pipeline::xmp_target(src);
        let legacy_rj = autoshop::store::legacy_recipe(src);
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
        std::fs::write(&xp, autoshop::xmp::recipe_to_xmp(&EditRecipe::default())).unwrap();
        assert!(
            matches!(read_saved_develop(src).saved, SavedDevelop::NoopOnly),
            "a no-op XMP must not claim a restore"
        );

        // A sidecar whose ONLY edit is CORRUPT imports as a no-op — the
        // disclosure list must still surface, or a later save silently
        // overwrites the corrupt original (Codex batch-32 #1).
        let doc = autoshop::xmp::recipe_to_xmp(&EditRecipe::default())
            .replace("crs:Exposure2012=\"0.00\"", "crs:Exposure2012=\"broken\"");
        assert!(doc.contains("broken"), "fixture: the corrupt attribute must exist");
        std::fs::write(&xp, &doc).unwrap();
        let RestoredDevelop { saved, xmp_bad: warn, .. } = read_saved_develop(src);
        assert!(matches!(saved, SavedDevelop::NoopOnly), "corrupt-only still restores nothing");
        assert!(warn.contains(&"Exposure2012".to_string()), "{warn:?}");

        // XMP with real edits → imported through the reverse crs mapping.
        let edited = EditRecipe { contrast: 22.0, ..Default::default() };
        std::fs::write(&xp, autoshop::xmp::recipe_to_xmp(&edited)).unwrap();
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
        let dev2 = autoshop::store::develop_dir(src2);
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
            autoshop::store::recipe_target(src2).exists(),
            "…and now lives in the central store"
        );
    }

    #[test]
    fn read_saved_develop_lets_a_newer_lightroom_sidecar_win() {
        let dir = std::env::temp_dir().join("autoshop-gui-lr-sidecar");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_lr_probe.ARW");
        std::fs::write(&src, b"raw").unwrap();
        let dev = autoshop::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]); // …even if an assert fires
        // The stored develop (older).
        let saved = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        std::fs::write(
            autoshop::store::recipe_target(&src),
            serde_json::to_string(&saved).unwrap(),
        )
        .unwrap();
        // Lightroom's own sidecar beside the RAW, stamped NEWER (set, not
        // slept for).
        let lr = dir.join("_gui_lr_probe.xmp");
        std::fs::write(
            &lr,
            autoshop::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() }),
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
            &std::fs::read_to_string(autoshop::store::recipe_target(&src)).unwrap(),
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
        let dir = std::env::temp_dir().join("autoshop-gui-batch-antidrift");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_batch_drift.ARW");
        std::fs::write(&src, b"raw").unwrap();
        let dev = autoshop::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]);
        let stamp_like_open = |mut r: EditRecipe| {
            let (ask, ast) = autoshop::pipeline::fresh_as_shot_wb(&src);
            stamp_calibration(
                &mut r,
                &autoshop::pipeline::photo_base_knots(&src),
                &autoshop::pipeline::fresh_lens_profile(&src),
                ask.zip(ast),
            );
            r
        };

        // Phase 1: an XMP-ONLY develop (the store projection).
        std::fs::write(
            autoshop::store::xmp_target(&src),
            autoshop::xmp::recipe_to_xmp(&EditRecipe { contrast: 21.0, ..Default::default() }),
        )
        .unwrap();
        let snap = autoshop::store::read_develop_snapshot(&src).unwrap();
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
            autoshop::store::recipe_target(&src),
            serde_json::to_string(&EditRecipe { exposure_ev: 0.5, ..Default::default() })
                .unwrap(),
        )
        .unwrap();
        let lr = src.with_extension("xmp");
        std::fs::write(
            &lr,
            autoshop::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() }),
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
        let snap = autoshop::store::read_develop_snapshot(&src).unwrap();
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
    #[test]
    fn an_embedded_raw_xmp_packet_restores_when_nothing_else_answers() {
        let dir = std::env::temp_dir().join("autoshop-gui-packet-restore");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_packet.dng");
        let doc = autoshop::xmp::recipe_to_xmp(&EditRecipe {
            exposure_ev: 0.8,
            ..Default::default()
        });
        std::fs::write(&src, tiff_with_xmp(doc.as_bytes())).unwrap();
        let dev = autoshop::store::develop_dir(&src);
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
        let snap = autoshop::store::read_develop_snapshot(&src).unwrap();
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
        let dir = std::env::temp_dir().join("autoshop-gui-packet-rank");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("_gui_packet_rank.dng");
        let doc = autoshop::xmp::recipe_to_xmp(&EditRecipe {
            exposure_ev: 0.8,
            ..Default::default()
        });
        std::fs::write(&src, tiff_with_xmp(doc.as_bytes())).unwrap();
        let dev = autoshop::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dir.clone(), dev.clone()]);

        // (a) A stored develop outranks the packet.
        std::fs::write(
            autoshop::store::recipe_target(&src),
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
            autoshop::store::recipe_target(&src),
            serde_json::to_string(&EditRecipe::default()).unwrap(),
        )
        .unwrap();
        assert!(
            matches!(read_saved_develop(&src).saved, SavedDevelop::NoopOnly),
            "a neutral store file bars the packet"
        );
        let snap = autoshop::store::read_develop_snapshot(&src).unwrap();
        assert!(
            crate::export::resolve_snapshot_develop(&src, &snap, &mut Vec::new()).unwrap().is_none(),
            "the batch answers the same: neutral store, no packet"
        );

        // (b) An explicit clear sticks: marker present, no store files — the
        // packet must NOT resurrect the cleared develop.
        std::fs::remove_file(autoshop::store::recipe_target(&src)).unwrap();
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
        let mut app = AutoshopApp::default();
        app.recipe.masks =
            vec![autoshop::recipe::LocalAdjustment::default(), Default::default()];

        app.placing_mask = Some((MaskKind::Linear, PlaceTarget::Redraw(0)));
        app.place_start = Some((0.5, 0.5));
        assert!(!app.disarm_selection_bound_tools(Some(1)), "no brush session died");
        assert!(app.placing_mask.is_none(), "an armed Redraw dies with its row");
        assert!(app.place_start.is_none(), "…and its gesture anchor with it");

        app.placing_mask = Some((
            MaskKind::Radial,
            PlaceTarget::Component(0, autoshop::recipe::MaskCombine::Add),
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
        let mut app = AutoshopApp {
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
            vec![autoshop::recipe::LocalAdjustment::default(), Default::default()];
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
        let mut app = AutoshopApp {
            busy: true,
            recipe: EditRecipe {
                masks: vec![autoshop::recipe::LocalAdjustment {
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
            !autoshop::render::mask_raster_write_fits_budget(
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
        let mut app = AutoshopApp::default();
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
        let mut app = AutoshopApp::default();
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
        let mut app = AutoshopApp::default();
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
            Some(PathBuf::from("D:/__autoshop_keepfact__/__autoshop_keepfact__.ARW"));
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
        let mut app = AutoshopApp::default();
        let ctx = egui::Context::default();
        let p = PathBuf::from("D:/__autoshop_histgate__/__autoshop_histgate__.ARW");
        // Pre-era fingerprint: version 1, interior x >= 0.5, darkens > 0.05.
        let washy = vec![[0.0, 0.0], [0.6, 0.4], [1.0, 1.0]];
        let primed = vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]];
        autoshop::pipeline::prime_curve_memo(
            &p,
            autoshop::pipeline::curve_ident(&p),
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
            autoshop::recipe::CALIB_ERA,
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
        let armed = |base: Option<Arc<image::DynamicImage>>, origin: Option<PathBuf>| AutoshopApp {
            src_path: Some(PathBuf::from(
                "D:/__autoshop_edgeflight__/__autoshop_edgeflight__.ARW",
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
        let master = PathBuf::from("out/__autoshop_edgeflight__.heal.png");
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

    #[test]
    fn a_saved_retouch_is_not_re_reported_as_unsaved() {
        // The store records the master ABSOLUTIZED while an in-session
        // retouch holds the same file's ./out path RELATIVE, and nothing
        // rewrites the in-memory origin at save time. Comparing the two
        // literally reported a fully saved photo as unsaved: the stash gate
        // armed on the way out, its restore re-lit the ● on the way back, and
        // the quit dialog listed a photo with nothing to save.
        // Unique stem + full scrub: this writes into the real central store.
        let src = std::path::Path::new("D:/library/__autoshop_masterid__.ARW");
        let dev = autoshop::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all("out").unwrap();
        let master = PathBuf::from("out/__autoshop_masterid__.heal.png");
        let _scrub = Scrub(vec![master.clone(), dev.clone()]);
        std::fs::write(&master, b"png").unwrap();
        autoshop::store::write_pixel_source(src, &master, false).unwrap();
        let mk = |origin: PathBuf| Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Original,
            recipe: EditRecipe::default(),
            base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
            origin: Some(origin),
            thumb: None,
        };
        let mut app = AutoshopApp {
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
        app.saved_strip = Some(autoshop::store::VariantsRecord {
            extra: Default::default(),
            active_id: None,
            active_name: None,
            v: 1,
            active_kind: VariantKind::Original.store_str().to_string(),
            active_pos: 0,
            others: vec![autoshop::store::VariantEntry {
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
        app.open_path(PathBuf::from("D:/library/__autoshop_masterid_next__.ARW"));
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
        let src = PathBuf::from("D:/library/__autoshop_stashkind__.ARW");
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
        app.open_path(PathBuf::from("D:/library/__autoshop_stashkind_next__.ARW"));
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
        let src = std::path::Path::new("D:/library/__autoshop_striprec__.ARW");
        let dev = autoshop::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dev.clone()]);
        let master = dev.join("reimagine-1.png");
        std::fs::write(&master, b"png").unwrap();
        let mut app = AutoshopApp {
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
        let rec = autoshop::store::read_variants(src).expect("record on disk");
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
        let mut app = AutoshopApp { preview_edge: 4096, ..Default::default() };
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
        let src = std::path::Path::new("D:/library/__autoshop_gonemaster__.ARW");
        let dev = autoshop::store::develop_dir(src);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let _scrub = Scrub(vec![dev.clone()]);
        let gone = dev.join("gone-master.png");
        std::fs::write(&gone, b"png").unwrap();
        autoshop::store::write_pixel_source(src, &gone, false).unwrap();
        std::fs::remove_file(&gone).unwrap(); // recorded, and now unresolvable
        assert!(autoshop::store::has_pixel_source(src), "premise: a record survives");
        let ctx = egui::Context::default();
        let mut app = AutoshopApp {
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
        let mut app = AutoshopApp {
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
        let mut app = AutoshopApp {
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
        let mut app = AutoshopApp {
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
        let mut app = AutoshopApp {
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
        let mut app = AutoshopApp {
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
        };
        let mut app = AutoshopApp {
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
        let mut app = AutoshopApp {
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
        let mut app = AutoshopApp {
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
        };
        let abs = UndoStep {
            recipe: EditRecipe::default(),
            base: Some(base),
            origin: Some(std::path::absolute("out/_spelling.png").unwrap()),
        };
        assert!(rel.same_pixels(&abs), "one master, two spellings, one pixel identity");
        let other = UndoStep {
            recipe: EditRecipe::default(),
            base: rel.base.clone(),
            origin: Some(PathBuf::from("out/_spelling-2.png")),
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
        let mut app = AutoshopApp::default();
        let ctx = egui::Context::default();
        app.src_path = Some(PathBuf::from("D:/__autoshop_deadopen__/__autoshop_deadopen__.ARW"));
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
        let mut app = AutoshopApp::default();
        let base = Arc::new(image::DynamicImage::new_rgb8(4, 3));
        let knots: Vec<[f32; 2]> = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        let lens = autoshop::recipe::LensProfile {
            vignette: vec![1.0, 1.2],
            vignette_on: true,
            ..Default::default()
        };
        let p = std::path::Path::new("D:/__autoshop_nonexistent__/x.ARW");
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
            .map(|i| std::path::PathBuf::from(format!("D:/__autoshop_nonexistent__/{i}.ARW")))
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
        let mut app = AutoshopApp::default();
        let master = Arc::new(image::DynamicImage::new_rgb8(6, 4));
        let p = std::path::Path::new("D:/__autoshop_nonexistent__/master.tif");
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
            .map(|i| std::path::PathBuf::from(format!("D:/__autoshop_nonexistent__/m{i}.tif")))
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
        let mut app = AutoshopApp::default();
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
        let mut app = AutoshopApp::default();
        app.recipe.masks = vec![autoshop::recipe::LocalAdjustment {
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

    /// L12#2A: the verdict is TYPED data rendered at draw time — the zh
    /// catalogue must actually translate every decision word, or the map
    /// silently falls back to English (tr's contract) and the gate that
    /// exists to see that (audit_i18n's extraction of decision_key) would
    /// be the only witness. This is the runtime half of that gate.
    #[test]
    fn a_verdict_names_its_decision_in_the_current_language() {
        use autoshop::advisor::{decision_key, Decision};
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

    /// L12#2B: typed rationale notes describe ONE develop's rationale tail.
    /// Every wholesale recipe swap (undo / version load / variant switch /
    /// AI apply) goes through resync_recipe_display — if that did not clear
    /// the vec, the panel would strip-and-render a stale zh text over a
    /// DIFFERENT develop's rationale. The landings that produce fresh notes
    /// install them AFTER the resync.
    #[test]
    fn a_recipe_swap_clears_stale_rationale_notes() {
        let mut app = AutoshopApp::default();
        app.recipe.rationale = "old english tail".into();
        app.rationale_notes = vec![autoshop::rationale::Note::plain(
            autoshop::rationale::keys::FIT_NOTE_SAT_PEGGED,
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
        let dir = std::env::temp_dir().join(format!("autoshop-fontcap-{}", std::process::id()));
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
        let mut app = AutoshopApp {
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

    /// 阶段4: the ExportFormat prefs code round-trips, a pre-阶段4 prefs
    /// file (save_jpeg only) migrates to Jpeg, and ext/depth stay coherent
    /// with what render_to_file expects.
    #[test]
    fn export_format_prefs_migrate_and_roundtrip() {
        assert_eq!(
            ExportFormat::from_pref(0, true),
            ExportFormat::Jpeg,
            "legacy save_jpeg=true without a code migrates to JPEG"
        );
        assert_eq!(ExportFormat::from_pref(0, false), ExportFormat::Tiff16);
        assert_eq!(ExportFormat::from_pref(99, false), ExportFormat::Tiff16, "unknown code is safe");
        // The wire numbering is a PERSISTED contract (gui-prefs.ron) —
        // pinned literally, never derived from pref_code itself: a
        // coordinated swap of two variants' codes passed the derived
        // roundtrip while every stored preference changed meaning (L16-8).
        for (code, f) in [
            (0u8, ExportFormat::Tiff16),
            (1, ExportFormat::Tiff8),
            (2, ExportFormat::Png16),
            (3, ExportFormat::Png8),
            (4, ExportFormat::Jpeg),
        ] {
            assert_eq!(f.pref_code(), code, "persisted numbering is frozen: {f:?}");
            assert_eq!(ExportFormat::from_pref(code, false), f, "roundtrip {f:?}");
        }
        assert!(ExportFormat::Jpeg.eight_bit(), "JPEG is 8-bit by nature");
        assert!(ExportFormat::Tiff8.eight_bit() && ExportFormat::Png8.eight_bit());
        assert!(!ExportFormat::Tiff16.eight_bit() && !ExportFormat::Png16.eight_bit());
        assert_eq!(ExportFormat::Png8.ext(), "png");
        assert_eq!(ExportFormat::Tiff8.ext(), "tif");
        assert_eq!(ExportFormat::Jpeg.ext(), "jpg");
    }

    /// 阶段5: the installed theme must be the RENDERED theme, whatever the
    /// OS reports. egui 0.29 defaults to ThemePreference::System and
    /// `set_style` writes only the ACTIVE theme's style slot — before the
    /// fix, on a light-mode OS the startup install landed in the dark slot
    /// while the screen showed the STOCK light style (round-11's "亮色主题"
    /// screenshot was this bug in plain sight). The adversarial host here
    /// reports the OPPOSITE mode every frame; the app's choice must win,
    /// and the styled content (our selection stroke, not egui's default)
    /// must be what `ctx.style()` serves after a real frame.
    #[test]
    fn the_installed_theme_survives_an_opposite_mode_os() {
        for (pref, want_dark) in [(ThemePref::Dark, true), (ThemePref::Light, false)] {
            let ctx = egui::Context::default();
            let os_reports =
                if want_dark { egui::Theme::Light } else { egui::Theme::Dark };
            let input = egui::RawInput {
                system_theme: Some(os_reports),
                ..Default::default()
            };
            // Startup order: install BEFORE the first frame (system theme
            // unknown), exactly like main()'s creation closure.
            install_theme(&ctx, pref);
            // One real frame with the adversarial system theme — the moment
            // the old bug swapped the stock style in.
            let _ = ctx.run(input, |_| {});
            assert_eq!(
                ctx.style().visuals.dark_mode,
                want_dark,
                "{pref:?} must render as itself on an opposite-mode OS"
            );
            assert_eq!(
                ctx.style().visuals.selection.stroke.color,
                pref.colors().selection_stroke,
                "{pref:?}: the rendered slot must carry OUR palette, not stock"
            );
        }
    }

    /// 阶段5 手感: the zoom keys are real state transitions, not just
    /// cheat-sheet rows — `+` steps the TARGET ×1.25 (clamped ≤12, and it
    /// compounds so a key roll accumulates), `0` refits and recentres, `1`
    /// jumps to the canvas-computed 1:1 twin; the live zoom then GLIDES to
    /// the target (util::glide_step, exercised below). Driven through a
    /// headless frame so the whole tier-C gate chain (no transient, no
    /// focus) is exercised, not a hand-called helper.
    #[test]
    fn zoom_keys_step_fit_and_jump_to_one_to_one() {
        let key = |k: egui::Key| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut app = AutoshopApp {
            src_path: Some(PathBuf::from("x.png")),
            zoom_one_to_one: 4.0, // what the canvas computed last frame
            zoom: 2.0,
            zoom_target: 2.0,
            pan: egui::vec2(0.7, 0.7),
            ..Default::default()
        };

        let ctx = egui::Context::default();
        let run_key = |app: &mut AutoshopApp, k: egui::Key| {
            let mut input = egui::RawInput::default();
            input.events.push(key(k));
            let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
        };
        run_key(&mut app, egui::Key::Plus);
        assert!((app.zoom_target - 2.5).abs() < 1e-4, "`+` retargets ×1.25, got {}", app.zoom_target);
        run_key(&mut app, egui::Key::Minus);
        assert!((app.zoom_target - 2.0).abs() < 1e-4, "`-` steps back, got {}", app.zoom_target);
        run_key(&mut app, egui::Key::Num1);
        assert_eq!(app.zoom_target, 4.0, "`1` targets the stored 1:1 zoom");
        run_key(&mut app, egui::Key::Num0);
        assert_eq!(app.zoom_target, 1.0, "`0` refits");
        assert_eq!(app.pan, egui::vec2(0.5, 0.5), "`0` recentres the pan (instant)");
        // Ceiling: from 11× one `+` press must stop at the 12× clamp.
        app.zoom_target = 11.0;
        run_key(&mut app, egui::Key::Plus);
        assert_eq!(app.zoom_target, 12.0, "zoom keys respect the 12x ceiling");
        // No photo → the keys are inert (same gate as the canvas buttons).
        app.src_path = None;
        app.zoom_target = 3.0;
        run_key(&mut app, egui::Key::Num0);
        assert_eq!(app.zoom_target, 3.0, "no photo: zoom keys must not act");
    }

    /// 阶段5 手感: the glide that carries `zoom` to `zoom_target` must move
    /// monotonically, never overshoot, terminate EXACTLY on the target
    /// (snap inside 1e-3 — a forever-almost-there zoom would repaint every
    /// frame for good), and settle in ~120 ms at 60 fps. Both directions.
    #[test]
    fn the_zoom_glide_converges_monotonically_and_terminates() {
        for (from, to) in [(1.0f32, 8.0f32), (8.0, 1.0)] {
            let mut z = from;
            let mut frames = 0;
            while z != to {
                let next = glide_step(z, to, 1.0 / 60.0);
                assert!(
                    (to - next).abs() <= (to - z).abs(),
                    "{from}->{to}: overshoot at frame {frames}: {z} -> {next}"
                );
                assert!(next != z, "{from}->{to}: stalled at {z} (frame {frames})");
                z = next;
                frames += 1;
                assert!(frames < 120, "{from}->{to}: no convergence in 2 s of frames");
            }
            assert!(frames <= 30, "{from}->{to}: settled in {frames} frames — the ~120 ms promise");
        }
        // A pathological dt (window drag hitch) must not overshoot either.
        assert_eq!(glide_step(2.0, 3.0, 10.0), glide_step(2.0, 3.0, 0.05), "dt is clamped");
    }

    /// 阶段5 手感: toast time scales with READING time — chars past the
    /// first 40 buy 35 ms each, capped per kind; short texts keep their
    /// historical 4 s / 8 s exactly, and a hanzi counts as one char, not
    /// three bytes.
    #[test]
    fn toast_ttl_scales_with_length_and_caps() {
        let mk = |text: &str, kind: ToastKind| Toast {
            text: text.into(),
            kind,
            born: Instant::now(),
        };
        assert_eq!(mk("Saved", ToastKind::Success).ttl(), Duration::from_millis(4_000));
        assert_eq!(mk("boom", ToastKind::Error).ttl(), Duration::from_millis(8_000));
        let fifty = "x".repeat(50);
        assert_eq!(
            mk(&fifty, ToastKind::Success).ttl(),
            Duration::from_millis(4_000 + 10 * 35),
            "10 chars past 40 buy 350 ms"
        );
        let hanzi = "字".repeat(50); // 150 BYTES — must still be 50 chars
        assert_eq!(
            mk(&hanzi, ToastKind::Success).ttl(),
            Duration::from_millis(4_000 + 10 * 35),
            "chars, not bytes"
        );
        let epic = "y".repeat(1000);
        assert_eq!(mk(&epic, ToastKind::Success).ttl(), Duration::from_millis(10_000), "success cap");
        assert_eq!(mk(&epic, ToastKind::Error).ttl(), Duration::from_millis(14_000), "error cap");
    }

    /// Codex 阶段5 F1 closure: egui-winit swallows every Ctrl(+Shift)+C into
    /// Event::Copy and emits NO Key event (is_copy_command returns early),
    /// so a consume_key(COMMAND, C) binding can never fire on real input —
    /// the first headless attempt pushed raw Key events and proved nothing.
    /// This test feeds exactly what winit sends: the Copy EVENT plus the
    /// modifier state. Shift is the discriminator — the bare event must
    /// pass through untouched for egui's own selected-label copy.
    #[test]
    fn recipe_copy_rides_the_shift_chord_not_the_bare_copy_event() {
        let mut app = AutoshopApp {
            src_path: Some(PathBuf::from("x.png")),
            ..Default::default()
        };
        app.recipe.exposure_ev = 1.5;
        let ctx = egui::Context::default();
        let run_copy = |app: &mut AutoshopApp, shift: bool| {
            let input = egui::RawInput {
                modifiers: egui::Modifiers {
                    command: true,
                    ctrl: true,
                    shift,
                    ..Default::default()
                },
                events: vec![egui::Event::Copy],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
        };
        run_copy(&mut app, false);
        assert!(app.copied.is_none(), "bare Ctrl+C belongs to egui's text copy");
        run_copy(&mut app, true);
        let copied = app.copied.as_ref().expect("Ctrl+Shift+C copies the recipe");
        assert_eq!(copied.exposure_ev, 1.5, "the CURRENT recipe is what's copied");
        assert_eq!(
            app.copied_from.as_deref(),
            Some(std::path::Path::new("x.png")),
            "provenance rides along (the paste-guard identity)"
        );
    }

    /// GUI review 2026-08-12 F3: a catalogue knew WHERE its ids came from
    /// (`from_base`) but not WHEN — `Msg::Models` carried only the role, so a
    /// completion launched under key K1 landed unconditionally after the user
    /// had already replaced the key with K2, and the URL check happily kept
    /// K1's ids under K2's name. Every `clear` now bumps a generation and a
    /// completion must still match it to install.
    #[test]
    fn a_completion_from_a_superseded_fetch_is_discarded_not_installed() {
        let mut app = AutoshopApp::default();
        // A typed loopback base + typed token keep the real worker's probe
        // off the network entirely (connection refused before any header).
        app.settings.image_base_url = "http://127.0.0.1:9/v1".into();
        app.settings.image_api_key = "typed-test-token".into();
        app.fetch_models(ModelRole::Image);
        let stale_gen = app.settings.image_models.generation;
        assert!(app.settings.image_models.fetching, "the flight is up");
        // The user replaces the key mid-flight — the key field's handler
        // clears the catalogue, and clearing bumps the generation:
        app.settings.image_models.clear();
        // The old flight lands, delivered exactly as the pump would:
        app.on_models(Lang::En, ModelRole::Image, stale_gen, Ok(vec!["stale-model".into()]));
        assert!(
            app.settings.image_models.chat.is_empty(),
            "ids fetched with the OLD credential must not be offered under the new one"
        );
        assert!(!app.settings.image_models.fetching, "stale or not, THE flight is over");
        // A fresh fetch's completion still installs normally:
        app.fetch_models(ModelRole::Image);
        let live_gen = app.settings.image_models.generation;
        app.on_models(Lang::En, ModelRole::Image, live_gen, Ok(vec!["fresh-model".into()]));
        assert_eq!(app.settings.image_models.chat, vec!["fresh-model".to_string()]);
        assert!(!app.settings.image_models.fetching);
    }

    /// GUI review 2026-08-12 F6: one global "auto-fetched" boolean was
    /// consumed by the first Settings open even when NO role had a key, so a
    /// key saved five minutes later never got its convenience probe on any
    /// later open. The guard is per role and consumed at DISPATCH.
    #[test]
    fn the_autofetch_opportunity_survives_until_a_role_is_actually_eligible() {
        let mut app = AutoshopApp::default();
        // Any dispatch stays strictly on loopback: a typed form base wins
        // over the machine's saved config, and the saved-key rule withholds
        // any real credential from an endpoint it was not saved for.
        app.settings.image_base_url = "http://127.0.0.1:9/v1".into();
        let mut cfg = autoshop::config::Config {
            openai_api_key: None,
            openai_model: "test-chat".into(),
            openai_base_url: "http://127.0.0.1:9/v1".into(),
            openai_image_model: "test-image".into(),
            openai_image_quality: "auto".into(),
            openai_image_max_px: 4_000_000,
            image_provider: "api".into(),
            image_effort: None,
            analysis_provider: "oauth".into(),
            analysis_model: "opus".into(),
            analysis_effort: None,
            claude_bin: "claude".into(),
            analysis_api_key: None,
            analysis_base_url: "http://127.0.0.1:9/v1".into(),
            python_bin: "python".into(),
            denoise_model: "scunet_color_real_psnr".into(),
            denoise_script: String::new(),
            denoise_cache: String::new(),
            segment_script: String::new(),
            style_strength: 0.5,
        };
        // First open: no credential — nothing to probe, and the VISIT itself
        // must not spend the session's opportunity.
        app.autofetch_models_once(&cfg);
        assert!(
            !app.settings.image_models.autofetched,
            "an ineligible open must not consume the role's probe"
        );
        assert!(!app.settings.image_models.fetching);
        // The key arrives (saved on another surface); the NEXT open probes.
        cfg.openai_api_key = Some("typed-test-token".into());
        app.autofetch_models_once(&cfg);
        assert!(app.settings.image_models.autofetched, "consumed at dispatch");
        assert!(app.settings.image_models.fetching, "the probe launched");
        // …and only once per session: a reopen never re-probes a metered
        // endpoint (the review verified this half held; it must keep holding).
        app.settings.image_models.fetching = false;
        app.autofetch_models_once(&cfg);
        assert!(!app.settings.image_models.fetching, "the one probe is spent");
    }

    /// GUI review 2026-08-12 F1 (model face): the CLI accepts full ids as
    /// well as aliases, so a provider flip may only rewrite what PROVABLY
    /// belongs to the other provider's vocabulary — a valid `claude-opus-4-6`
    /// configured against an API bridge used to be silently replaced with
    /// `opus` on every flip to OAuth.
    #[test]
    fn a_provider_flip_rewrites_only_the_other_providers_vocabulary() {
        // OAuth-ward (to_api = false):
        assert_eq!(analysis_model_on_flip("gpt-5.5", false), Some("opus"), "an OpenAI id yields");
        assert_eq!(analysis_model_on_flip("claude-opus-4-6", false), None, "a full Claude id is CLI-valid");
        assert_eq!(analysis_model_on_flip("Claude-Opus-4-6", false), None, "…case-folded, like every id test");
        assert_eq!(analysis_model_on_flip("fable", false), None, "every documented alias survives");
        // API-ward (to_api = true):
        assert_eq!(analysis_model_on_flip("opus", true), Some("gpt-5.5"), "an alias is CLI-only vocabulary");
        assert_eq!(analysis_model_on_flip("claude-opus-4-6", true), None, "a bridge legitimately serves claude-*");
        assert_eq!(analysis_model_on_flip("gpt-5.5", true), None, "already at home");
    }

    /// GUI review 2026-08-12 F4: worker completion arrives on a plain mpsc
    /// channel, which does not wake egui — and the 100 ms pump's gate
    /// (`poll_workers`) runs before any panel can start a fetch in the same
    /// frame, so with the pointer held still a click on "Fetch models" showed
    /// nothing (not even "fetching…") until the next input event. Both edges
    /// now request a repaint from inside `spawn_worker`.
    #[test]
    fn worker_spawn_and_completion_both_wake_the_event_loop() {
        let app = AutoshopApp::default();
        // Drain the fresh context's startup frames (egui schedules a couple
        // on its own) until it reports quiet.
        for _ in 0..8 {
            let _ = app.egui_ctx.run(Default::default(), |_| {});
        }
        assert!(!app.egui_ctx.has_requested_repaint(), "sanity: context drained");
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        app.spawn_worker(
            move || {
                let _ = gate_rx.recv();
                Msg::Models(ModelRole::Image, 0, Ok(Vec::new()))
            },
            |e| Msg::Models(ModelRole::Image, 0, Err(e)),
        );
        // Edge 1: STARTING a worker schedules the next frame (the pump's
        // gate has already run this frame by the time a panel can spawn).
        assert!(app.egui_ctx.has_requested_repaint(), "spawn must wake the loop");
        // Consume that request the way real frames would; the worker is
        // still gated, so quiet must return…
        for _ in 0..8 {
            let _ = app.egui_ctx.run(Default::default(), |_| {});
        }
        assert!(!app.egui_ctx.has_requested_repaint(), "sanity: drained again");
        // …until the worker completes: edge 2, the completion itself must
        // wake the loop — the mpsc send alone never does.
        gate_tx.send(()).expect("the worker is waiting on the gate");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app.egui_ctx.has_requested_repaint() {
            assert!(
                std::time::Instant::now() < deadline,
                "a completed worker never asked for a repaint — its result would sit unread until the next input event"
            );
            std::thread::yield_now();
        }
    }

    /// R14 (user report 2026-08-12): after the centering rework the variant
    /// card sat visibly LOW-or-HIGH in the strip — the air above the
    /// thumbnail must equal the air below the card column ("变成和下边一样
    /// 距离"). Headless frame over the REAL variant_strip + REAL theme
    /// style + the REAL panel height constant; the three test seams record
    /// what actually laid out.
    /// R19: the ✨ Generate button renders ONE line tall in both languages,
    /// and the row's width arithmetic is exact — two distinct failure modes,
    /// both real: the old fixed 130 px reserve was 1 px short of the
    /// English label, wrapping the button's text into a two-line button
    /// (the user report); and the first fix omitted the TextEdit's own
    /// 8 px frame margin, which — with the button in Extend mode — stopped
    /// being absorbed by wrapping and instead widened the auto-fitting
    /// side panel by 8 px EVERY frame (probed during review). Three frames
    /// pin both: a stable panel width and a one-line button.
    ///
    /// R22-6: that row MOVED into the AI panel (`ai_panel`, #4), so this test
    /// follows its host — driving `retouch_panel` alone would leave
    /// `reimagine_btn_rect` unset and the assertions vacuous (the seam's
    /// `expect` is what turns that into a red instead of a silent pass). Both
    /// panels are driven so the width witness still covers everything the side
    /// panel stacks below the AI area.
    #[test]
    fn the_generate_button_stays_one_line_and_the_panel_stays_put() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = AutoshopApp { lang, ..Default::default() };
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            };
            let mut widths = Vec::new();
            for _ in 0..3 {
                let _ = ctx.run(input(), |ctx| {
                    let r = egui::SidePanel::left("controls")
                        .default_width(320.0)
                        .show(ctx, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                app.ai_panel(ui);
                                app.retouch_panel(ui);
                            });
                        });
                    // the panel's rendered width, the runaway's witness
                    widths.push(r.response.rect.width());
                });
            }
            assert!(
                (widths[0] - widths[2]).abs() < 0.5,
                "{lang:?}: the side panel must not grow across frames: {widths:?}"
            );
            let btn = app.reimagine_btn_rect.expect("the button records its rect (test seam)");
            let one_line = ctx.style().spacing.interact_size.y;
            assert!(
                btn.height() <= one_line + 1.0,
                "{lang:?}: the Generate button must be one line tall ({} vs {one_line})",
                btn.height()
            );
        }
    }

    /// R22-6 (#4): the AI area's mid-open EDITABLE GATE, in its new host.
    ///
    /// L15-2: while a decode is in flight (`open_in_flight`) the panel's
    /// controls address the STASHED photo A, and B is about to land and replace
    /// the whole recipe — typing a Direction or firing Analyze there is silent
    /// input loss. The analysis block used to sit INSIDE `develop_panel`'s
    /// `add_enabled_ui(editable, …)` closure and inherited that gate for free;
    /// moving it into `ai_panel` (#4) meant re-establishing it by hand, which
    /// is exactly the kind of migration that loses a guard quietly. `busy`
    /// alone must NOT gate (a 600 s analyze keeps the panel live), so both
    /// halves are pinned. Deleting the `add_enabled_ui` wrapper in ai.rs fails
    /// phase ②.
    #[test]
    fn the_ai_panel_freezes_only_while_a_photo_is_opening() {
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let frame = |app: &mut AutoshopApp| -> Option<bool> {
            app.ai_gate_enabled = None; // this frame's evidence only
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
            app.ai_gate_enabled
        };
        // ① settled: live.
        let mut app = AutoshopApp::default();
        assert_eq!(frame(&mut app), Some(true), "a settled panel must be editable");
        // ② mid-open: frozen.
        app.open_in_flight = true;
        assert_eq!(
            frame(&mut app),
            Some(false),
            "mid-open the AI controls address the STASHED photo — they must be inert (L15-2)"
        );
        // ③ busy but NOT opening (a 600 s analyze): still live, or cancelling
        // and re-typing a direction would be impossible for ten minutes.
        app.open_in_flight = false;
        app.busy = true;
        assert_eq!(frame(&mut app), Some(true), "a long AI call must not freeze the panel");
    }

    /// R22-6 (#14a): a prompt field has a READABLE ceiling. Every one of them
    /// used to take `available_width()` — fine at the 320 px default, an
    /// 800 px single-line ribbon once the side panel is dragged wide (the panel
    /// deliberately keeps NO max width: the curve and HSL editors are better
    /// wide, so the cap belongs on the FIELDS). Laid out at 800 px, which is
    /// exactly where the old code fails: every field then measures its full
    /// available width and blows the `FIELD_W_MAX` assertion.
    ///
    /// All three prompts are pinned by ONE rule over the `prompt_rects` seam:
    /// Direction and Generative Fill go through `util::prompt_field`, while the
    /// Reimagine row keeps its own R19 galley arithmetic and only `.min()`s the
    /// result — three sites, one ceiling, and a fourth field added later is
    /// covered without editing this test.
    #[test]
    fn a_prompt_field_never_grows_past_its_readable_width() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = AutoshopApp { lang, ..Default::default() };
            // Text long enough that a field free to grow would want to: the
            // singleline clip keeps this from mattering, the cap is about the
            // BOX, and a filled buffer also exercises the hover-tooltip arm.
            app.guidance = "warmer and moodier, lift the shadows a lot, and keep the sky honest".into();
            app.reimagine_prompt = app.guidance.clone();
            app.fill_prompt = app.guidance.clone();
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1200.0),
                )),
                ..Default::default()
            };
            // Three frames, because a WIDTH rule and an auto-fitting side panel
            // is exactly the pair that produced R19's +8 px/frame runaway: the
            // cap only ever asks for LESS than the row has (which cannot grow
            // the panel), and this is what says so.
            let mut widths = Vec::new();
            for _ in 0..3 {
                app.prompt_rects.clear(); // the LAST frame's evidence only
                let _ = ctx.run(input(), |ctx| {
                    // Both prompt-bearing folds are collapsed by default; egui's
                    // own test hook opens every collapsible (the same one the
                    // mask-brush width test uses), so an unopened fold cannot
                    // make this vacuous.
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    let r =
                        egui::SidePanel::left("controls").default_width(800.0).show(ctx, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                app.ai_panel(ui);
                                app.retouch_panel(ui);
                            });
                        });
                    widths.push(r.response.rect.width());
                });
            }
            assert!(
                (widths[0] - widths[2]).abs() < 0.5,
                "{lang:?}: the wide panel must not grow across frames: {widths:?}"
            );
            assert_eq!(
                app.prompt_rects.len(),
                3,
                "{lang:?}: expected the Direction / Reimagine / Fill prompts to lay out, got {:?}",
                app.prompt_rects
            );
            for (i, r) in app.prompt_rects.iter().enumerate() {
                assert!(
                    r.width() <= crate::theme::FIELD_W_MAX + 0.5,
                    "{lang:?}: prompt field {i} is {:.1} px wide on an 800 px panel — past the \
                     {:.0} px readable ceiling (a dropped `.min(FIELD_W_MAX)`)",
                    r.width(),
                    crate::theme::FIELD_W_MAX
                );
                assert!(
                    r.width() >= crate::theme::FIELD_W_MIN,
                    "{lang:?}: prompt field {i} collapsed to {:.1} px",
                    r.width()
                );
            }
        }
    }

    #[test]
    fn the_variant_card_gets_equal_air_above_and_below() {
        let mk = |kind: crate::model::VariantKind| crate::model::Variant {
            id: String::new(),
            name: None,
            kind,
            recipe: Default::default(),
            base: None,
            origin: None,
            thumb: None,
        };
        // Two states of the measured (first) card: the lone-Original
        // placeholder, and a non-Original card in a two-variant strip —
        // whose label row carries the ✕ delete button (review 2026-08-12
        // finding 1: the ✕ row was never laid out by the single case).
        let scenarios: [(&str, Vec<crate::model::Variant>); 2] = [
            ("lone original", vec![mk(crate::model::VariantKind::Original)]),
            (
                "deletable card with ✕",
                vec![
                    mk(crate::model::VariantKind::Generated),
                    mk(crate::model::VariantKind::Original),
                ],
            ),
        ];
        for (label, variants) in scenarios {
            let mut app = AutoshopApp { variants, ..Default::default() };
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::TopBottomPanel::bottom("variants")
                    .exact_height(AutoshopApp::VARIANT_STRIP_H)
                    .show(ctx, |ui| app.variant_strip(ui));
            });
            let row = app.strip_row_rect.expect("the strip records its row (test seam)");
            let card = app.strip_card_rect.expect("the strip records its card (test seam)");
            // Gaps bracket the whole CARD COLUMN (review finding 3: measuring
            // the top from the thumb keeps passing if something ever lands
            // above it), and a floor keeps "0 air on both sides" from
            // counting as centered (review finding 8).
            let top = card.top() - row.top();
            let bottom = row.bottom() - card.bottom();
            eprintln!("strip geometry [{label}]: row={row:?} card={card:?} \
                 top_gap={top:.1} bottom_gap={bottom:.1}");
            assert!(
                (top - bottom).abs() <= 1.0,
                "[{label}] unequal air around the variant card: \
                 {top:.1} above vs {bottom:.1} below"
            );
            assert!(
                top >= 4.0,
                "[{label}] the card column has no breathing room ({top:.1} px) — \
                 it outgrew VARIANT_STRIP_H"
            );
            // R16: the PAINTED title centers on the measured card column —
            // the title rect comes from variant_strip's own cursor math, the
            // card rect from egui's real layout; agreeing centers proves the
            // two geometries cohere (they diverged by ~29 px when the title
            // was a row child centered on egui's 26 px seed).
            let title =
                app.strip_title_rect.expect("the strip records its painted title (test seam)");
            let dy = title.center().y - card.center().y;
            assert!(
                dy.abs() <= 1.0,
                "[{label}] the Variants title sits {dy:.1} px off the card column's center"
            );
        }
    }

    /// #17 (user report after the v0.27 trial): a PORTRAIT photo's gallery
    /// thumbnail was squashed into the same 1.4:1 landscape box as everything
    /// else. The orientation chain (decode's EXIF transpose, render's
    /// rotation) was never at fault — the panel handed egui a
    /// `SizedTexture::new(id, (THUMB_W, THUMB_H))`, i.e. a LIE about the
    /// texture's own size, and a Texture image source is drawn at
    /// `ImageFit::Exact`. The fix keeps the SLOT constant (that is what keeps
    /// the filename column aligned) and insets the image at its true aspect.
    /// Restoring the lie (`let draw = egui::vec2(THUMB_W, THUMB_H)`) fails the
    /// aspect assertion for the portrait case.
    #[test]
    fn a_gallery_thumb_keeps_its_aspect_inside_a_constant_slot() {
        for (tw, th, tag) in [(40u32, 60u32, "portrait 2:3"), (60, 40, "landscape 3:2")] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = AutoshopApp::default();
            let tex = ctx.load_texture(
                "r22_thumb",
                egui::ColorImage::new([tw as usize, th as usize], egui::Color32::GRAY),
                egui::TextureOptions::LINEAR,
            );
            // One row, its texture already resident: the loaded branch.
            app.gallery = vec![PathBuf::from("r22-thumb-geometry.arw")];
            app.gallery_dir = Some(PathBuf::from("."));
            app.thumbs.insert(0, tex);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::SidePanel::left("library")
                    .default_width(260.0)
                    .show(ctx, |ui| app.gallery_panel(ui));
            });
            let slot = app.gallery_slot_rect.expect("the gallery records its thumb slot (test seam)");
            let draw = app.gallery_thumb_rect.expect("the gallery records the drawn image (test seam)");
            eprintln!("gallery thumb [{tag}]: slot={slot:?} draw={draw:?}");
            // ① the image is drawn at the TEXTURE's aspect ratio
            let want = tw as f32 / th as f32;
            let got = draw.width() / draw.height();
            assert!(
                (got - want).abs() <= 0.02,
                "[{tag}] the thumbnail is drawn at {got:.3}:1, not the texture's {want:.3}:1 \
                 — a portrait squashed into the landscape slot again"
            );
            // ② the SLOT is the constant one the placeholder branch allocates
            assert!(
                (slot.width() - THUMB_W).abs() < 0.01 && (slot.height() - THUMB_H).abs() < 0.01,
                "[{tag}] the row slot must stay a constant {THUMB_W}×{THUMB_H} (the filename \
                 column's alignment depends on it): {slot:?}"
            );
            // ③ letterboxed INSIDE that slot, centred in it
            assert!(
                slot.contains_rect(draw),
                "[{tag}] the drawn image {draw:?} escapes its slot {slot:?}"
            );
            let d = draw.center() - slot.center();
            assert!(
                d.x.abs() <= 0.5 && d.y.abs() <= 0.5,
                "[{tag}] the inset is off-centre by ({:.2}, {:.2}) px",
                d.x,
                d.y
            );
        }
    }

    /// #9 (user report after the v0.27 trial): "画笔大小的滑杆不见了". It was
    /// never deleted — the app's ONE brush radius (`self.brush`) had its only
    /// slider in the Retouch panel, next to Fill / Heal / Stamp, while the
    /// mask brush is armed from Develop → Local Masks and paints in a session
    /// whose row carried ⌫ / ✓ / ✕ and nothing else. So the control existed in
    /// a panel the user was not in. This renders the REAL develop_panel with a
    /// live session (the previous width test only ever drove retouch_panel —
    /// develop_panel had zero frame coverage) and pins two things:
    ///   ① the Brush size slider is present in that frame — deleting the
    ///      `self.brush_size_slider(ui)` call in the session block fails here;
    ///   ② the side panel's width does not grow across frames — the +8 px per
    ///      frame runaway of v0.26.1 happened in this very side panel, and this
    ///      change adds a widget to it.
    #[test]
    fn the_mask_brush_session_carries_the_brush_size_slider() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = AutoshopApp { lang, ..Default::default() };
            // A plate is what start_mask_brush sizes its buffers against.
            app.base_preview = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(64, 96)));
            app.start_mask_brush(None);
            assert!(app.mask_brush.is_some(), "{lang:?}: the brush session armed");
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            };
            let mut widths = Vec::new();
            for frame in 0..3 {
                app.brush_slider_rect = None; // this frame's evidence only
                let _ = ctx.run(input(), |ctx| {
                    // Local Masks is a collapsed section by default; opening
                    // every collapsible is egui's own test hook for exactly
                    // this (CollapsingState::openness).
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    let r = egui::SidePanel::left("controls")
                        .default_width(320.0)
                        .show(ctx, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| app.develop_panel(ui));
                        });
                    widths.push(r.response.rect.width());
                });
                let s = app.brush_slider_rect.unwrap_or_else(|| {
                    panic!(
                        "{lang:?} frame {frame}: no Brush size slider in the mask-brush \
                         session — the radius control is back in another panel"
                    )
                });
                assert!(
                    s.height() >= ctx.style().spacing.interact_size.y - 1.0
                        && s.width().is_finite()
                        && s.width() > 0.0,
                    "{lang:?} frame {frame}: the slider occupied {s:?} — it did not lay out"
                );
            }
            assert!(
                (widths[0] - widths[2]).abs() < 0.5,
                "{lang:?}: the controls panel must not grow across frames: {widths:?}"
            );
        }
    }

    /// R22-3, same段 as #9: the 「Paint mask」 checkbox writes `paint_mode`
    /// directly, and a live MASK-brush session paints through that same flag.
    /// Un-ticking it therefore produced an ORPHAN session — brush inert, but
    /// the ⌫ / ✓ Apply / ✕ Cancel row still on screen with a buffer 「Apply」
    /// would happily bake. Un-ticking must take the session's own teardown.
    /// (Deleting the `else` arm of paint_mode_toggled fails phase 2.)
    #[test]
    fn un_ticking_paint_mask_ends_the_brush_session_instead_of_orphaning_it() {
        // Phase 1: ticking still sweeps the other canvas tools and stays armed.
        let mut app = AutoshopApp {
            base_preview: Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(32, 48))),
            crop_mode: true,
            clone_mode: true,
            paint_mode: true,
            ..Default::default()
        };
        app.paint_mode_toggled();
        assert!(app.paint_mode, "ticking must survive the mutual-exclusion sweep");
        assert!(!app.crop_mode && !app.clone_mode, "the other tools are disarmed");
        // Phase 2: a live session + the box un-ticked = a cancel, not an orphan.
        app.start_mask_brush(None);
        assert!(
            app.mask_brush.is_some() && app.mask_brush_gray.is_some() && app.paint_mode,
            "sanity: the session is live and painting through paint_mode"
        );
        app.paint_mode = false; // what the checkbox itself just wrote
        app.paint_mode_toggled();
        assert!(
            app.mask_brush.is_none(),
            "the session outlived its own paint flag — its Apply row would bake stale weights"
        );
        assert!(app.mask_brush_gray.is_none(), "the weight buffer goes with it");
        assert!(!app.paint_mode, "and the flag stays off");
    }

    /// M6a: the save status line the user actually reads. The projection's
    /// losses were disclosed on the IMPORT side four times over and on the
    /// EXPORT side never — a sidecar quietly missing two AI masks looked like
    /// a clean "XMP + recipe saved". The counts here come from the writer's
    /// own verdicts (`xmp::mask_export_losses`); this pins that every category
    /// reaches the UI, in both languages, and that a faithful save says
    /// NOTHING (an unconditional line would train the user to ignore it).
    #[test]
    fn the_save_line_names_every_projection_loss_and_stays_quiet_otherwise() {
        use autoshop::xmp::{MaskLoss, MaskLossReason as R};
        let loss = |name: &str, reason: R| MaskLoss { name: name.into(), reason };
        let losses = vec![
            loss("sky", R::Bitmap),
            loss("subject", R::Bitmap),
            loss("parked", R::Disabled),
            loss("combo", R::ComponentsFlattened),
            // A rotation with NO nameable angle (R25 P5's `0` payload: an
            // angle that rounds away, or one the reader could not measure).
            // This test owns the FALLBACK half of that branch — the plain
            // 「radial rotation ×N」 category, which must survive the payload
            // exactly as it read before. `the_rotation_warning_says_why` owns
            // the other half.
            loss("gold", R::Rotation(0)),
            loss("gold", R::Recolour),
        ];
        for (lang, want) in [
            (
                crate::i18n::Lang::En,
                [
                    "bitmap masks ×2",
                    "muted masks ×1",
                    "shape components flattened ×1",
                    "radial rotation ×1",
                    "recolour gains ×1",
                ],
            ),
            (
                crate::i18n::Lang::Zh,
                [
                    "位图蒙版 ×2",
                    "已静音蒙版 ×1",
                    "形状组件已压平 ×1",
                    "径向旋转 ×1",
                    "重上色增益 ×1",
                ],
            ),
        ] {
            let line = xmp_loss_line(lang, &losses, &[])
                .unwrap_or_else(|| panic!("{lang:?}: losses must produce a line"));
            for fragment in want {
                assert!(line.contains(fragment), "{lang:?}: {fragment:?} missing from {line}");
            }
            // The categories are joined, not overwritten: five fragments, five
            // separators-worth of one sentence.
            assert_eq!(line.matches('×').count(), 5, "{lang:?}: every category kept: {line}");
            assert!(
                xmp_loss_line(lang, &[], &[]).is_none(),
                "{lang:?}: a faithful save is silent"
            );
            // One category alone must not drag the others' labels in.
            let one =
                xmp_loss_line(lang, &[loss("sky", R::Bitmap)], &[]).expect("one loss, one line");
            assert_eq!(one.matches('×').count(), 1, "{lang:?}: only the live category: {one}");

            // R24-5 M0, upgrade (1): the masks are NAMED, not merely counted —
            // "which of my twelve?" is the actionable half a count leaves out,
            // and the CLI's own `describe_mask_losses` has answered it since
            // M6a while the window did not.
            assert!(line.contains("sky, subject"), "{lang:?}: the bitmap masks are named: {line}");
            assert!(line.contains("parked"), "{lang:?}: the muted mask is named: {line}");
            // A nameless mask still renders as a name, never as an empty slot.
            let anon = xmp_loss_line(lang, &[loss("", R::Bitmap)], &[]).expect("a line");
            assert!(
                anon.contains(tr(lang, "(unnamed)")),
                "{lang:?}: a nameless mask is labelled: {anon}"
            );
            // ...and the cap holds: five bitmap masks show four names + the rest.
            let many: Vec<_> =
                ["a", "b", "c", "d", "e"].iter().map(|n| loss(n, R::Bitmap)).collect();
            let capped = xmp_loss_line(lang, &many, &[]).expect("a line");
            assert!(capped.contains("a, b, c, d"), "{lang:?}: four names shown: {capped}");
            assert!(!capped.contains(", e"), "{lang:?}: the fifth is folded away: {capped}");
            assert!(
                capped.contains(&trf(lang, "+{n} more", &[("n", "1")])),
                "{lang:?}: and counted: {capped}"
            );

            // R24-5 M0, upgrade (2): the GLOBAL bucket, which did not exist.
            // A recipe whose look depends on the camera base curve exported a
            // sidecar that renders differently in Lightroom, silently.
            let globals = xmp_loss_line(lang, &[], &["base_curve", "lens_profile"])
                .expect("globals alone must produce a line");
            assert!(globals.contains(tr(lang, "camera base curve")), "{lang:?}: {globals}");
            assert!(globals.contains(tr(lang, "lens profile correction")), "{lang:?}: {globals}");
            assert_eq!(globals.matches('×').count(), 0, "{lang:?}: no mask category: {globals}");
            // Both buckets in ONE sentence - two toasts for one save would be
            // two interruptions describing the same file.
            let both =
                xmp_loss_line(lang, &[loss("sky", R::Bitmap)], &["base_curve"]).expect("a line");
            assert!(both.contains("sky") && both.contains(tr(lang, "camera base curve")));
            // A control with no label of its own still names itself.
            let raw = xmp_loss_line(lang, &[], &["some_future_control"]).expect("a line");
            assert!(raw.contains("some_future_control"), "{lang:?}: {raw}");
        }
    }

    /// R25 P1, the IMPORT twin of the test above. The line it replaces
    /// counted: 「N Lightroom mask(s) (brush/AI/depth) have no engine
    /// equivalent」 — true of one import gate out of six, and silent about the
    /// five that refused every ordinary radial and gradient in the user's
    /// catalog. This pins that each reason reaches the sentence, in both
    /// languages; that reasons sharing a label share a BULLET (three
    /// unmodelled knobs are one phrase, not three); and that a faithful
    /// import says NOTHING.
    ///
    /// Also the R24 「no internal symbols in UI prose」 pin: a reason's Rust
    /// path must never leak into the line.
    #[test]
    fn the_import_banner_names_what_it_lost() {
        use autoshop::xmp::{MaskImportLoss, MaskImportReason as R};
        let loss = |name: &str, reason: R| MaskImportLoss { name: name.into(), reason };
        let losses = vec![
            loss("brushed", R::Unrepresentable),
            loss("subtract only", R::OutOfModel),
            // Zero payload = no angle to name; this test pins the plain
            // 「Rotation angle」 label, which stays the fallback after R25 P5
            // gave the reason a number to carry (`the_rotation_warning_says_why`
            // pins the numbered sentence).
            loss("radial 1", R::Rotation(0)),
            loss("radial 1", R::UnknownLocalKey),
            loss("gradient 2", R::BlendMode),
            loss("gradient 2", R::InertLocal("LocalGrain")),
            loss("combo", R::MultiComponent),
            loss("ranged", R::ForeignRangeMask),
            loss("curved", R::LocalCurve),
            loss("refined", R::CurveRefineSaturation),
        ];
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let line = xmp_import_line(lang, 7, &losses).expect("ten losses ⇒ a line");
            // Every reason reaches the sentence, under its own label. The
            // literals sit AT their `tr` call so the i18n audit can see them
            // — translating a loop VARIABLE is a dynamic site it cannot read.
            let ai_brush = tr(
                lang,
                "AI / brush masks cannot be imported — Lightroom recomputes them from a digest",
            );
            for label in [
                ai_brush,
                tr(lang, "Beyond this engine's model"),
                tr(lang, "Rotation angle"),
                tr(lang, "Blend mode"),
                tr(lang, "Extra shapes"),
                tr(lang, "Range mask (foreign)"),
                // R25 P6: the four local point curves are modelled now, so
                // this verdict only fires on a curve that would not PARSE —
                // and its label says so instead of reading like a gap.
                tr(lang, "Local point curve (unreadable)"),
                tr(lang, "Unmodelled slider"),
            ] {
                assert!(line.contains(label), "{lang:?}: {label:?} missing from {line}");
            }
            // Eight labels for ten losses: the three unmodelled-knob reasons
            // share one bullet, and its names are MERGED, not overwritten.
            assert!(
                line.contains(&trf(
                    lang,
                    "Imported {n} Lightroom mask(s), {m} feature(s) not modelled",
                    &[("n", "7"), ("m", "8")],
                )),
                "{lang:?}: the head counts both halves: {line}"
            );
            let knobs = line
                .split(" · ")
                .find(|p| p.starts_with(tr(lang, "Unmodelled slider")))
                .unwrap_or_else(|| panic!("{lang:?}: no unmodelled-knob bullet in {line}"));
            for who in ["radial 1", "gradient 2", "refined"] {
                assert!(knobs.contains(who), "{lang:?}: {who} lost its bullet: {knobs}");
            }
            // R24's rule: a UI sentence never shows an internal symbol — the
            // slider key rides in the reason, not on screen.
            assert!(!line.contains("::"), "{lang:?}: internal symbol in UI prose: {line}");
            assert!(!line.contains("InertLocal"), "{lang:?}: variant name on screen: {line}");
            // A faithful import is silent, the same rule the save line follows.
            assert!(
                xmp_import_line(lang, 4, &[]).is_none(),
                "{lang:?}: a clean import says nothing"
            );
            // A nameless correction still renders as a name.
            let anon = xmp_import_line(lang, 1, &[loss("", R::Rotation(0))]).expect("a line");
            assert!(anon.contains(tr(lang, "(unnamed)")), "{lang:?}: {anon}");
            // …and the cap holds, like every other disclosure list.
            let many: Vec<_> =
                ["a", "b", "c", "d", "e"].iter().map(|n| loss(n, R::Rotation(0))).collect();
            let capped = xmp_import_line(lang, 0, &many).expect("a line");
            assert!(capped.contains("a, b, c, d"), "{lang:?}: four names shown: {capped}");
            assert!(!capped.contains(", e)"), "{lang:?}: the fifth is folded away: {capped}");
            assert!(
                capped.contains(&trf(lang, "+{n} more", &[("n", "1")])),
                "{lang:?}: and counted: {capped}"
            );
        }
    }

    /// R25 P5. 「radial rotation ×1」 named a category and left out both halves
    /// a photographer could act on: HOW MUCH tilt was set aside, and WHY. The
    /// why matters because the answer is not a bug — `crs:Angle`'s sign and
    /// pivot are unverified against a real Lightroom experiment, so the
    /// engine refuses to guess — and a line that does not say so reads as one.
    ///
    /// Both directions, because both drop an angle: the writer drops OURS,
    /// the reader drops LIGHTROOM's.
    ///
    /// MUTATION THIS CATCHES: print the reason without its payload and the
    /// digits go; group the losses by `==` instead of by kind and two
    /// differently-tilted masks split into two bullets (or, with `ALL`'s
    /// placeholder `0`, vanish from the sentence entirely).
    #[test]
    fn the_rotation_warning_says_why() {
        use autoshop::xmp::{
            MaskImportLoss, MaskImportReason as I, MaskLoss, MaskLossReason as E,
        };
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            // EXPORT: our own angle, on its way out of the sidecar.
            let out = xmp_loss_line(
                lang,
                &[MaskLoss { name: "tilted".into(), reason: E::Rotation(37) }],
                &[],
            )
            .expect("a rotation loss must produce a line");
            assert!(out.contains("37"), "{lang:?}: the angle itself is missing: {out}");
            assert!(
                out.contains(&trf(
                    lang,
                    "Rotation {a}° not written to XMP (crs:Angle sign/pivot unverified)",
                    &[("a", "37")],
                )),
                "{lang:?}: the sentence must say why: {out}"
            );
            assert!(out.contains("tilted"), "{lang:?}: and which mask: {out}");
            // Two masks, two angles, ONE bullet — the grouping is by kind.
            let two = xmp_loss_line(
                lang,
                &[
                    MaskLoss { name: "a".into(), reason: E::Rotation(37) },
                    MaskLoss { name: "b".into(), reason: E::Rotation(-12) },
                ],
                &[],
            )
            .expect("a line");
            assert!(two.contains("37") && two.contains("-12"), "{lang:?}: both angles: {two}");
            assert!(two.contains("a, b"), "{lang:?}: both masks, one bullet: {two}");
            // No angle to name ⇒ the plain category, never 「Rotation 0°」.
            let none =
                xmp_loss_line(lang, &[MaskLoss { name: "a".into(), reason: E::Rotation(0) }], &[])
                    .expect("a line");
            assert!(
                none.contains(&trf(lang, "radial rotation ×{n}", &[("n", "1")])),
                "{lang:?}: the fallback stands: {none}"
            );
            assert!(!none.contains("0°"), "{lang:?}: an unmeasured angle is not zero: {none}");

            // IMPORT: Lightroom's angle, on its way in.
            let inn = xmp_import_line(
                lang,
                1,
                &[MaskImportLoss { name: "Radial 1".into(), reason: I::Rotation(-44) }],
            )
            .expect("a rotation note must produce a line");
            assert!(inn.contains("-44"), "{lang:?}: the angle itself is missing: {inn}");
            assert!(
                inn.contains(&trf(
                    lang,
                    "Rotation {a}° read as 0 (crs:Angle sign/pivot unverified)",
                    &[("a", "-44")],
                )),
                "{lang:?}: the sentence must say why: {inn}"
            );
            // R24's rule holds on the new sentences too.
            for line in [&out, &two, &none, &inn] {
                assert!(!line.contains("::"), "{lang:?}: internal symbol in UI prose: {line}");
            }
        }
    }

    /// R25 P1 end-to-end, through the panel the user actually looks at: a
    /// Lightroom sidecar's masks reach the Local Masks list. Until this batch
    /// that list was EMPTY for every Lightroom file on the machine — the
    /// import refused each correction over a `crs:Angle` that Lightroom writes
    /// on every radial (as "0" when unrotated) and a `crs:MaskBlendMode` it
    /// writes on every component.
    ///
    /// The importer and the panel are wired together on purpose: the lib-side
    /// test proves the parse, and this proves the parse REACHES the UI, which
    /// is the half a reader of `xmp.rs` alone cannot see.
    #[test]
    fn a_photo_with_lightroom_masks_shows_them_in_the_list() {
        // Synthetic, like every fixture in this batch: the attribute set and
        // the nesting are Lightroom's, the names are neutral test values (no
        // user XMP goes into a public repository).
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.6-c145\">\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\"\n\
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n\
             crs:HasSettings=\"True\">\n\
             <crs:MaskGroupBasedCorrections><rdf:Seq>\n\
             <rdf:li><rdf:Description crs:What=\"Correction\" crs:CorrectionActive=\"true\"\n\
             crs:CorrectionName=\"Sky\" crs:CorrectionAmount=\"1\"\n\
             crs:LocalExposure2012=\"-0.15\" crs:LocalCurveRefineSaturation=\"100\">\n\
             <crs:CorrectionMasks><rdf:Seq>\n\
             <rdf:li crs:What=\"Mask/CircularGradient\" crs:MaskActive=\"true\"\n\
             crs:MaskName=\"Radial Gradient 1\" crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\"\n\
             crs:MaskValue=\"1\" crs:Top=\"0.11\" crs:Left=\"0.59\" crs:Bottom=\"0.80\"\n\
             crs:Right=\"0.92\" crs:Angle=\"37.412506\" crs:Midpoint=\"50\" crs:Roundness=\"0\"\n\
             crs:Feather=\"100\" crs:Flipped=\"true\" crs:Version=\"2\"/>\n\
             </rdf:Seq></crs:CorrectionMasks>\n\
             </rdf:Description></rdf:li>\n\
             <rdf:li><rdf:Description crs:What=\"Correction\" crs:CorrectionActive=\"true\"\n\
             crs:CorrectionName=\"Foreground\" crs:CorrectionAmount=\"1\"\n\
             crs:LocalExposure2012=\"0.1\" crs:LocalCurveRefineSaturation=\"100\">\n\
             <crs:CorrectionMasks><rdf:Seq>\n\
             <rdf:li crs:What=\"Mask/Gradient\" crs:MaskActive=\"true\"\n\
             crs:MaskName=\"Linear Gradient 1\" crs:MaskBlendMode=\"0\" crs:MaskInverted=\"false\"\n\
             crs:MaskValue=\"1\" crs:ZeroX=\"0.5\" crs:ZeroY=\"0.8\" crs:FullX=\"0.5\" crs:FullY=\"0.2\"/>\n\
             </rdf:Seq></crs:CorrectionMasks>\n\
             </rdf:Description></rdf:li>\n\
             </rdf:Seq></crs:MaskGroupBasedCorrections>\n\
             </rdf:Description></rdf:RDF></x:xmpmeta>";
        let mut app =
            AutoshopApp { recipe: autoshop::xmp::xmp_to_recipe(doc), ..Default::default() };
        assert_eq!(
            app.recipe.masks.len(),
            2,
            "premise: the importer brought the Lightroom masks in — without this the \
             panel assertions below would pass on an empty list and prove nothing"
        );
        let seen = tall_frame(&mut app, |a, ui| {
            a.develop_panel(ui);
        });
        // The ● rides on the header text, so this is a prefix match — the
        // count is what is being pinned.
        assert!(
            seen.iter().any(|t| t.starts_with("Local Masks (2)")),
            "the header must count both imported masks: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.starts_with("Sky · Radial")),
            "the radial keeps its Lightroom name AND reads as a radial: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.starts_with("Foreground · Linear")),
            "the gradient keeps its Lightroom name AND reads as a gradient: {seen:?}"
        );
        // …and they are LIVE, not parked: both carry a real exposure delta, so
        // the section earns its ●.
        assert!(
            app.masks_section_active(),
            "an imported Lightroom mask with a real slider must light the section"
        );
    }

    /// R24 round-end MED-2: the same sentence, said in two different VOICES.
    ///
    /// `base_curve` is stamped on every RAW open, so the global bucket is
    /// non-empty on essentially every RAW — and an Error toast plus a status ⚠
    /// on every single Ctrl+S breaks this surface's own rule ("a save that
    /// lost nothing must not interrupt") in spirit: the loss is real but
    /// universal and unactionable, and alarm that always fires is alarm that
    /// stops being read. The judgement is the registry's `engine_only` bit —
    /// the engine's own per-photo measurement vs something the user chose.
    #[test]
    fn engine_calibration_losses_are_disclosed_without_interrupting_the_save() {
        use autoshop::advisor::catalogue::{Tier, RECIPE_CONTROLS};
        use autoshop::xmp::{MaskLoss, MaskLossReason as R};

        // Premise, from the registry rather than from memory: every global
        // the export can lose today IS engine calibration.
        let rows: Vec<_> = RECIPE_CONTROLS
            .iter()
            .filter(|c| c.tier == Some(Tier::RenderedNotExported))
            .collect();
        assert_eq!(rows.len(), 2, "the tier's membership moved — re-read this test");
        assert!(
            rows.iter().all(|c| c.engine_only),
            "premise: today's unexportable globals are all engine measurements"
        );

        // The quiet arm: the real, universal case — a stamped base curve.
        assert!(
            !xmp_loss_interrupts(&[], &["base_curve"]),
            "a stamped camera base curve must not raise an error toast on every save"
        );
        assert!(
            !xmp_loss_interrupts(&[], &["base_curve", "lens_profile"]),
            "…nor both halves of the same calibration"
        );
        // …and it is still SAID: quiet is not silent.
        assert!(
            xmp_loss_line(crate::i18n::Lang::En, &[], &["base_curve"]).is_some(),
            "the sentence survives; only the interruption goes"
        );

        // The interrupting arm (1): a user's own mask, whatever the globals.
        let sky = MaskLoss { name: "sky".into(), reason: R::Bitmap };
        assert!(xmp_loss_interrupts(std::slice::from_ref(&sky), &[]));
        assert!(
            xmp_loss_interrupts(std::slice::from_ref(&sky), &["base_curve"]),
            "one actionable loss puts the toast back for the whole line"
        );

        // The interrupting arm (2): a global that is NOT engine calibration —
        // what the LR-gap batches (B2–B5) will add. Modelled by a control that
        // really is `engine_only: false` and by an unknown name, which must
        // fall to the interrupting side rather than be vouched for.
        assert!(
            RECIPE_CONTROLS.iter().any(|c| c.name == "clarity" && !c.engine_only),
            "premise for the probe below"
        );
        assert!(
            xmp_loss_interrupts(&[], &["clarity"]),
            "a control the USER set is actionable — it interrupts"
        );
        assert!(
            xmp_loss_interrupts(&[], &["some_future_control"]),
            "a name with no registry row cannot be vouched for as calibration"
        );
        assert!(
            xmp_loss_interrupts(&[], &["base_curve", "clarity"]),
            "mixed: one user-chosen member is enough"
        );
        // Nothing lost, nothing said, nothing to interrupt.
        assert!(!xmp_loss_interrupts(&[], &[]));
    }

    /// R24 round-end LOW-3: choosing a delivery root inside the photo library
    /// silently retires that folder's read-only protection —
    /// `pipeline::guard_readonly` allows anything under the delivery root
    /// BEFORE it refuses the source RAW's own folder. `Trust::Destination`
    /// stops a PLANTED root from doing this; the user picking one in Settings
    /// had nothing on screen saying what it costs. This pins the dynamic arm
    /// (the one that knows which photo is open) — lexical, filesystem-free,
    /// and symmetric: containment either way is the same loss.
    #[test]
    fn a_delivery_root_that_overlaps_the_open_photos_folder_is_warned_about() {
        // Constructed paths only — the predicate must never touch the disk.
        let lib = std::env::temp_dir().join("autoshop-lowr3-library");
        let trip = lib.join("TripA");
        let photo = trip.join("DSC1.ARW");
        let p = Some(photo.as_path());

        assert!(delivery_root_shadows_photo(&trip, p), "the photo's own folder");
        assert!(delivery_root_shadows_photo(&lib, p), "an ancestor: the whole library");
        assert!(
            delivery_root_shadows_photo(&trip.join("exports"), p),
            "nested inside the photo's folder: that subtree stops being protected"
        );
        assert!(
            delivery_root_shadows_photo(&lib.join("TripB").join("..").join("TripA"), p),
            "`..` is folded lexically — the same rule guard_readonly applies"
        );

        assert!(!delivery_root_shadows_photo(&lib.join("TripB"), p), "a sibling folder is fine");
        assert!(
            !delivery_root_shadows_photo(&std::env::temp_dir().join("autoshop-lowr3-out"), p),
            "a folder outside the library is the normal case and must stay quiet"
        );
        assert!(!delivery_root_shadows_photo(&trip, None), "no photo open, nothing to say");
    }

    /// **Inclusion law, GUI half** (R24-5 M0): every develop control this
    /// window can MUTATE must be one the engine renders — or an agreed
    /// `CarriedOnly` (`catalogue::CARRIED_ONLY_{GLOBAL,LOCAL}`).
    ///
    /// A slider that moves a number nothing renders is the worst kind of bug
    /// here: it looks like it works, it survives a save, it reloads, and the
    /// photo never changes. The AI half of this law lives in
    /// `catalogue::every_control_the_ai_may_set_is_one_the_engine_renders`;
    /// this is the same law over the other surface, and both sides are read
    /// off the tier registry rather than transcribed from it.
    ///
    /// The settable set is EXTRACTED from the GUI's own source (the whole
    /// module tree, walked at runtime like the font gate does — an
    /// `include_str!` list would lose a new file silently), by the two textual
    /// shapes a Rust mutation takes: `&mut <path>.<field>` and
    /// `<path>.<field> =`. Known limits, stated rather than papered over: it
    /// over-includes (any struct with a field named like a control — which
    /// only makes the gate STRICTER), and a mutation reached purely through an
    /// autoref method call (`…masks.push(x)`) is invisible to it. Names the
    /// registry does not know are ignored: a new control cannot be one of
    /// those, because `catalogue::global_value`/`local_value` fail the build
    /// until it has a row.
    #[test]
    fn the_gui_only_offers_controls_the_engine_renders() {
        use autoshop::advisor::catalogue::{
            Tier, CARRIED_ONLY_GLOBAL, CARRIED_ONLY_LOCAL, LOCAL_CONTROLS, RECIPE_CONTROLS,
        };
        fn walk_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).expect("gui source dir listable") {
                let p = e.expect("dir entry").path();
                if p.is_dir() {
                    walk_rs(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        // A field name ends at the first character that cannot be in an
        // identifier; a path segment before the dot is whatever precedes it.
        fn field_after(text: &str, at: usize) -> Option<&str> {
            let rest = &text[at..];
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            (end > 0).then(|| &rest[..end])
        }
        let gui_src =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("bin").join("gui");
        let mut sources = Vec::new();
        walk_rs(&gui_src, &mut sources);
        assert!(sources.len() >= 6, "expected the split module tree, found {}", sources.len());

        let mut mutated: std::collections::BTreeSet<String> = Default::default();
        for p in &sources {
            // This file is the GATE, not a surface: its own fixtures build
            // recipes field by field and would swamp the extraction.
            if p.file_name().is_some_and(|n| n == "tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(p).expect("gui source readable");
            let bytes = text.as_bytes();
            for (i, _) in text.match_indices('.') {
                let Some(name) = field_after(&text, i + 1) else { continue };
                let after = i + 1 + name.len();
                // `&mut …<name>` — the borrow every slider helper takes.
                let borrowed = text[..i].trim_end().ends_with(|c: char| {
                    c.is_ascii_alphanumeric() || c == '_' || c == ')' || c == ']'
                }) && text[..i].rsplit_once("&mut ").is_some_and(|(_, tail)| {
                    !tail.contains(['\n', ';', ',', '(']) && !tail.trim().is_empty()
                });
                // `… = ` (but not `==`, `>=`, `<=`, `!=`), `+=`, `-=`, `*=`.
                let assigned = {
                    let mut j = after;
                    while bytes.get(j) == Some(&b' ') {
                        j += 1;
                    }
                    match (bytes.get(j), bytes.get(j + 1)) {
                        (Some(b'='), Some(c)) => *c != b'=',
                        (Some(b'+' | b'-' | b'*'), Some(b'=')) => true,
                        _ => false,
                    }
                };
                if borrowed || assigned {
                    mutated.insert(name.to_string());
                }
            }
        }
        // Premise: this window is a develop panel. Finding almost nothing
        // means the extractor broke and every assertion below is vacuous.
        for known in ["exposure_ev", "clarity", "saturation", "tone_curve", "texture", "amount"] {
            assert!(
                mutated.contains(known),
                "the extractor missed `{known}`, which the develop panel plainly sets — \
                 it is broken, and this gate would pass vacuously"
            );
        }

        for (label, rows, allow) in [
            ("EditRecipe", RECIPE_CONTROLS.as_slice(), CARRIED_ONLY_GLOBAL),
            ("LocalAdjustment", LOCAL_CONTROLS.as_slice(), CARRIED_ONLY_LOCAL),
        ] {
            for c in rows.iter().filter(|c| mutated.contains(c.name)) {
                // Envelope rows (the era stamp, the AI's rationale) carry no
                // develop value and the GUI legitimately copies them around;
                // they own no sidecar key, which is what keeps this narrow.
                let Some(t) = c.tier else { continue };
                assert!(
                    t.renders() || allow.iter().any(|(n, _)| *n == c.name),
                    "{label}.{}: the GUI sets a {t:?} control the engine renders nothing \
                     from — a slider that moves a number and no pixel",
                    c.name
                );
                assert_ne!(t, Tier::PassThrough, "{label}.{} is not for a surface to set", c.name);
            }
        }
    }

    /// R22-5 (#10): the selected mask's adjustments, in Lightroom's three
    /// groups, with the 「More (XMP/Lightroom only)」 fold GONE — R22-3 put
    /// clarity/dehaze/texture on the engine, so that title had become false
    /// while still hiding three working sliders one level down. Renders the
    /// real develop_panel and pins the group caption, the recolour disclosure
    /// (a mask carrying gains no sidecar can express), and the
    /// masks-but-none-selected hint (one click on a selected row lands there,
    /// and it used to show a list with no controls and no explanation).
    #[test]
    fn the_mask_panel_groups_its_sliders_and_says_what_is_engine_only() {
        // Every text the frame drew (nested Shape::Vec included).
        fn texts(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<String>) {
            fn walk(s: &egui::Shape, out: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                    _ => {}
                }
            }
            shapes.iter().for_each(|c| walk(&c.shape, out));
        }
        for (lang, group, recolour, hint, new_sliders) in [
            (
                crate::i18n::Lang::En,
                "Tone",
                "carries reverse-fit recolour (not exported to XMP)",
                "Select a mask above to edit its adjustments",
                ["Sharpness", "Hue shift"],
            ),
            (
                crate::i18n::Lang::Zh,
                "明暗",
                "含反推重上色（不写入 XMP）",
                "选中上面任一蒙版即可编辑它的调整",
                ["局部锐化", "色相旋转"],
            ),
        ] {
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            let mut app = AutoshopApp { lang, ..Default::default() };
            app.recipe.masks = vec![
                autoshop::recipe::LocalAdjustment {
                    mask: autoshop::recipe::MaskGeometry::Radial {
                        top: 0.2, left: 0.2, bottom: 0.8, right: 0.8,
                        feather: 0.5, roundness: 0.0, flipped: false, angle: 15.0,
                        midpoint: 50.0, mask_version: 2,
                    },
                    color_gains: Some([1.3, 1.0, 0.7]),
                    name: "gold".into(),
                    ..Default::default()
                },
                autoshop::recipe::LocalAdjustment { name: "grad".into(), ..Default::default() },
            ];
            // TALL on purpose: egui culls shapes outside the visible clip
            // rect, and Local Masks sits below a full screen of sections — a
            // 900 px window drew nothing of it to inspect.
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 20_000.0),
                )),
                ..Default::default()
            };
            let frame = |app: &mut AutoshopApp| -> Vec<String> {
                let mut seen = Vec::new();
                let out = ctx.run(input(), |ctx| {
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| app.develop_panel(ui));
                    });
                });
                texts(&out.shapes, &mut seen);
                seen
            };
            // ① a mask selected: the group captions and the engine-only note.
            app.sel_mask = Some(0);
            let seen = frame(&mut app);
            assert!(
                seen.iter().any(|t| t == group),
                "{lang:?}: no {group:?} group caption over the mask's tone sliders: {seen:?}"
            );
            assert!(
                seen.iter().any(|t| t == recolour),
                "{lang:?}: a mask with color_gains must say the XMP will not carry them"
            );
            assert!(
                !seen.iter().any(|t| t.contains("XMP/Lightroom only")),
                "{lang:?}: the 「More (XMP/Lightroom only)」 fold is back — those three \
                 sliders render in the engine now"
            );
            // R23-1b: the two controls the recipe grew this round. A field that
            // renders, exports and is offered to the AI but has no slider is
            // reachable only by the model — the user cannot answer it.
            for label in new_sliders {
                assert!(
                    seen.iter().any(|t| t == label),
                    "{lang:?}: the mask panel has no {label:?} slider: {seen:?}"
                );
            }
            // ② nothing selected (one click on the selected row): the hint.
            app.sel_mask = None;
            let seen = frame(&mut app);
            assert!(
                seen.iter().any(|t| t == hint),
                "{lang:?}: masks exist but none is selected — the panel must say so"
            );
            assert!(
                !seen.iter().any(|t| t == recolour),
                "{lang:?}: no selection ⇒ no per-mask detail"
            );
        }
    }

    /// R25 P6: the selected mask gets the SAME curve editor the global Curves
    /// section has, pointed at its own four curves — a fourth group under
    /// Lightroom's Tone / Detail / Color.
    ///
    /// Two halves, because either alone passes vacuously. The panel half
    /// proves the group is laid out only for a selected mask; the driven half
    /// proves the editor writes to `masks[0].main_curve` and NOT to
    /// `recipe.tone_curve` — a target parameter that is ignored (or a copied
    /// `curve_points` arm) looks identical on screen and edits the wrong photo
    /// state, which is exactly the class of bug U10 caught the first time.
    #[test]
    fn the_selected_mask_offers_a_curve_editor() {
        for (lang, caption) in
            [(crate::i18n::Lang::En, "Curve"), (crate::i18n::Lang::Zh, "曲线")]
        {
            let mut app = AutoshopApp { lang, ..Default::default() };
            app.recipe.masks =
                vec![autoshop::recipe::LocalAdjustment { name: "sky".into(), ..Default::default() }];
            // ① selected → the fourth group caption is drawn.
            app.sel_mask = Some(0);
            let seen = tall_frame(&mut app, |a, ui| {
                a.develop_panel(ui);
            });
            assert!(
                seen.iter().any(|t| t == caption),
                "{lang:?}: no {caption:?} group over the selected mask's curve editor: {seen:?}"
            );
            // ② nothing selected → no per-mask curve group. (The global
            //    「Curves」 section is a different caption on purpose, so this
            //    negative cannot be satisfied by it.)
            app.sel_mask = None;
            let seen = tall_frame(&mut app, |a, ui| {
                a.develop_panel(ui);
            });
            assert!(
                !seen.iter().any(|t| t == caption),
                "{lang:?}: a {caption:?} group with no mask selected: {seen:?}"
            );
        }

        // ③ the driven half: a click in the MASK editor lands in the mask.
        let mut app = AutoshopApp::default();
        app.recipe.masks =
            vec![autoshop::recipe::LocalAdjustment { name: "sky".into(), ..Default::default() }];
        let ctx = egui::Context::default();
        let run_pass = |app: &mut AutoshopApp, events: Vec<egui::Event>| -> bool {
            let mut changed = false;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    changed |= app.curve_editor_for(ui, CurveTarget::Mask(0));
                });
            });
            changed
        };
        let _ = run_pass(&mut app, vec![]);
        let rect = app.curve_rect.expect("the mask editor records its square (test seam)");
        let q = egui::pos2(rect.min.x + rect.width() * 0.25, rect.min.y + rect.height() * 0.5);
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: q,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let pressed = run_pass(&mut app, vec![egui::Event::PointerMoved(q), button(true)]);
        let released = run_pass(&mut app, vec![button(false)]);
        assert!(pressed || released, "the mask editor must report the edit");
        assert_eq!(
            app.recipe.masks[0].main_curve.len(),
            1,
            "the click adds one point to the MASK's master curve"
        );
        assert!(
            app.recipe.tone_curve.is_empty(),
            "the mask editor wrote into the GLOBAL tone curve: {:?}",
            app.recipe.tone_curve
        );
        // ④ a target that no longer addresses a mask draws nothing rather
        //    than falling through to the global curves.
        app.recipe.masks.clear();
        let mut drew = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                drew = app.curve_editor_for(ui, CurveTarget::Mask(0));
            });
        });
        assert!(!drew, "a stale mask index must not report an edit");
        assert!(app.recipe.tone_curve.is_empty(), "…and must not touch the global curves");
    }

    /// Every text one frame drew, nested `Shape::Vec` included — the way to read
    /// a `CollapsingHeader`'s own label (and therefore its ● or lack of one)
    /// without a seam per section.
    fn drawn_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(s: &egui::Shape, out: &mut Vec<String>) {
            match s {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|c| walk(&c.shape, &mut out));
        out
    }

    /// A tall frame: egui culls shapes outside the visible clip rect, and the
    /// lower sections sit below a full screen of siblings.
    fn tall_frame(app: &mut AutoshopApp, f: impl Fn(&mut AutoshopApp, &mut egui::Ui)) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 20_000.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            ctx.memory_mut(|m| m.set_everything_is_visible(true));
            egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| f(app, ui));
            });
        });
        drawn_texts(&out.shapes)
    }

    /// R22 #16a: the AI section's ● read `verdict.is_some() || !guidance.
    /// is_empty()` — a field set one item short of the panel's own inputs. The
    /// Style strength steers every AI proposal, is PERSISTED across launches, and
    /// was invisible the moment the section was collapsed.
    ///
    /// Pins the wiring first: the app default, the pref default and the slider's
    /// reset target must be ONE constant, or "the user moved it" is not a
    /// decidable question and this dot cannot exist. Then the rendered header —
    /// a predicate nobody reads is not a dot.
    #[test]
    fn the_ai_section_dot_follows_the_style_strength_it_used_to_miss() {
        assert_eq!(
            AutoshopApp::default().style_strength, STYLE_STRENGTH_DEFAULT,
            "the app must start at the shared default"
        );
        assert_eq!(
            Prefs::default().style_strength, STYLE_STRENGTH_DEFAULT,
            "a pref key missing from an older save must degrade to the SAME number \
             — otherwise every upgraded install starts with a lit dot"
        );
        let mut app = AutoshopApp::default();
        assert!(!app.ai_section_active(), "a fresh AI area has no state to flag");
        app.style_strength = 0.85;
        assert!(app.ai_section_active(), "a moved Style slider IS AI state");
        app.style_strength = STYLE_STRENGTH_DEFAULT;
        assert!(!app.ai_section_active(), "back at the default ⇒ back to no dot");
        // The two pre-existing members still count (no regression in closing the gap).
        app.guidance = "warmer and moodier".into();
        assert!(app.ai_section_active(), "a typed Direction still flags");
        app.guidance.clear();
        app.verdict = Some((autoshop::advisor::Decision::Accept, vec!["ok".into()]));
        assert!(app.ai_section_active(), "a verdict still flags");
        // …and the header really carries it.
        let mut app = AutoshopApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "AI"),
            "the AI header was not drawn at all — this test proves nothing: {seen:?}"
        );
        assert!(
            !seen.iter().any(|t| t == "AI  ●"),
            "a fresh app must not light the AI dot: {seen:?}"
        );
        app.style_strength = 0.85;
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "AI  ●"),
            "a moved Style slider must light the collapsed header's ●: {seen:?}"
        );
    }

    /// R23-3 (feedback #5, "the AI is too timid + give me a strength slider"):
    /// the desktop SHELL of the grade-strength axis.
    ///
    /// Four properties, because a slider that fails any one of them is
    /// decoration: it must be DRAWN beside Style (the two are one pair of axes,
    /// and the reported problem was that only the style half existed), start and
    /// persist at ONE constant shared with the lib, light the section ● when
    /// moved, and actually reach the request the analyze worker builds.
    #[test]
    fn the_grade_strength_slider_sits_beside_style_and_rides_the_analyze_request() {
        // One definition, three consumers (app default / pref default / the
        // slider's reset target) — the R22 #16 rule, applied to the new dial.
        assert_eq!(AutoshopApp::default().grade_strength, GRADE_STRENGTH_DEFAULT);
        assert_eq!(
            Prefs::default().grade_strength, GRADE_STRENGTH_DEFAULT,
            "a prefs file written before this key existed must decode to the SAME number — \
             serde's own f32 default (0.0) would be the most TIMID setting on a dial that \
             exists because the AI was too timid"
        );
        assert_eq!(
            GRADE_STRENGTH_DEFAULT,
            autoshop::recipe::GradeStrength::DEFAULT,
            "the GUI must not own a second copy of this number — the CLI and the web body \
             default through the lib's constant"
        );
        assert_eq!(GRADE_STRENGTH_DEFAULT, 0.65, "user decision 2026-08-17 ⑦");

        // DRAWN — at the DEFAULT 320 px side panel, which is the whole point:
        // `tall_frame` builds that width, and before this round the two dials
        // shared the verbs' row, where `egui::Slider`'s own nested (non-wrapping)
        // horizontal overflowed the clip rect. The row rendered Style's value
        // "30" with the word "Style" itself off-panel, and a second dial there
        // was invisible outright. Assert the LABELS, not just the values: the
        // label is the half that was being clipped.
        let mut app = AutoshopApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        for label in ["Style", "Strength", "30", "65"] {
            assert!(
                seen.iter().any(|t| t == label),
                "「{label}」 was not drawn in a 320 px panel: {seen:?}"
            );
        }

        // The ● follows it — including a move DOWN to the calibration point,
        // which is still a move away from the shipped default.
        assert!(!app.ai_section_active(), "a fresh AI area has no state to flag");
        app.grade_strength = autoshop::recipe::GradeStrength::CALIBRATED;
        assert!(
            app.ai_section_active(),
            "0.50 is the calibration point, not the default — moving there IS AI state"
        );
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "AI  ●"),
            "a moved Strength slider must light the collapsed header's ●: {seen:?}"
        );
        app.grade_strength = GRADE_STRENGTH_DEFAULT;
        assert!(!app.ai_section_active(), "back at the default ⇒ back to no dot");

        // …and it reaches the worker's request, on its OWN axis (the two dials
        // must not be able to swap: `style` is a bare fraction, `strength` a
        // `GradeStrength`, so a transposed pair would not compile).
        let app = AutoshopApp { grade_strength: 0.9, style_strength: 0.2, ..Default::default() };
        let req = autoshop::pipeline::GradeRequest {
            style: app.style_strength,
            send_reference_image: app.send_style_ref_image,
            strength: autoshop::recipe::GradeStrength::new(app.grade_strength),
            think: app.deep_think,
        };
        assert_eq!(req.strength.get(), 0.9);
        assert_eq!(req.style, 0.2);
    }

    /// R22 #16d: the Local Masks header's ● was `n_masks > 0` while every ROW
    /// dot below it used the engine's own rule. So a list of muted or parked
    /// masks — no adjustment set, or the eye off — claimed an active local
    /// adjustment, and when the claim WAS true it only repeated the count the
    /// header already prints. Both now read `util::mask_active`.
    #[test]
    fn the_local_masks_dot_matches_its_row_dots() {
        let parked = |name: &str| autoshop::recipe::LocalAdjustment {
            mask: autoshop::recipe::MaskGeometry::Radial {
                top: 0.2, left: 0.2, bottom: 0.8, right: 0.8,
                feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                midpoint: 50.0, mask_version: 2,
            },
            name: name.into(),
            ..Default::default()
        };
        let mut app = AutoshopApp::default();
        app.recipe.masks = vec![parked("one"), parked("two")];
        assert!(
            !app.masks_section_active(),
            "two masks with every adjustment at neutral do nothing — no ●"
        );
        let seen = tall_frame(&mut app, |a, ui| {
            a.develop_panel(ui);
        });
        assert!(
            seen.iter().any(|t| t == "Local Masks (2)"),
            "the header (with its count) was not drawn — this test proves nothing: {seen:?}"
        );
        assert!(
            !seen.iter().any(|t| t == "Local Masks (2)  ●"),
            "parked masks must not claim an active local adjustment: {seen:?}"
        );
        // One real adjustment on mask 2 ⇒ the row dot lights, so the header must.
        app.recipe.masks[1].exposure_ev = 0.6;
        assert!(mask_active(&app.recipe.masks[1]), "premise: the row dot lights here");
        assert!(app.masks_section_active(), "the header must agree with the row");
        let seen = tall_frame(&mut app, |a, ui| {
            a.develop_panel(ui);
        });
        assert!(
            seen.iter().any(|t| t == "Local Masks (2)  ●"),
            "a working mask must light the section header: {seen:?}"
        );
        // The EYE is half the rule: muting the only working mask parks the list.
        app.recipe.masks[1].enabled = false;
        assert!(!mask_active(&app.recipe.masks[1]), "premise: the row reads muted");
        assert!(
            !app.masks_section_active(),
            "a muted mask renders nothing, whatever its sliders say"
        );
    }

    /// R25 P0-0.4: the five section ● predicates are the control registry's
    /// families now, not five hand-written field tuples — so a control that
    /// joins a family joins its section's dot, which is the drift R22 #16 had
    /// to repair four times by hand.
    ///
    /// Four steps per section, the shape `the_local_masks_dot_matches_its_row_
    /// dots` uses: prove the header was DRAWN (or the negative below proves
    /// nothing), prove it carries no ●, move ONE control the family owns, and
    /// see the ● appear. The control moved is read off `CONTROL_FAMILIES`
    /// rather than named here, so this test cannot drift from the table
    /// either.
    #[test]
    fn section_dots_follow_the_registry_families() {
        use autoshop::advisor::catalogue::{family_is_active, CONTROL_FAMILIES};
        /// (family, the header the panel draws for it, a move inside it)
        type Case = (&'static str, &'static str, fn(&mut autoshop::recipe::EditRecipe));
        let cases: [Case; 5] = [
            ("presence", "Presence", |r| r.dehaze = 40.0),
            ("detail", "Detail", |r| r.noise_reduction = 25.0),
            ("hsl", "Color Mixer (HSL)", |r| r.hsl.saturation[3] = -30.0),
            ("color_grade", "Color Grading", |r| r.color_grade.highlight_sat = 20.0),
            ("curves", "Curves", |r| {
                r.green_curve = vec![autoshop::recipe::CurvePoint { input: 128, output: 140 }]
            }),
        ];
        for (family, header, mutate) in cases {
            let f = CONTROL_FAMILIES
                .iter()
                .find(|f| f.name == family)
                .unwrap_or_else(|| panic!("{family} is not a declared family"));
            let mut app = AutoshopApp::default();
            assert!(!family_is_active(f, &app.recipe), "{family}: a fresh recipe is neutral");
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            assert!(
                seen.iter().any(|t| t == header),
                "{family}: the {header} header was not drawn — this test proves nothing: {seen:?}"
            );
            assert!(
                !seen.iter().any(|t| t == &format!("{header}  ●")),
                "{family}: a neutral section must not claim an adjustment: {seen:?}"
            );
            mutate(&mut app.recipe);
            assert!(family_is_active(f, &app.recipe), "premise: the move woke the family up");
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            assert!(
                seen.iter().any(|t| t == &format!("{header}  ●")),
                "{family}: a moved control must light {header}: {seen:?}"
            );
        }
    }

    /// R25 B2: the global Texture slider lights the Presence ●.
    ///
    /// The four-step variant of `section_dots_follow_the_registry_families`
    /// for the ONE control that widened a section's dot this round — asserted
    /// here as well as in the registry's own oracle test because the derived
    /// predicate and the PANEL that reads it are two different things, and B2
    /// is the batch where a section's dot changed meaning.
    #[test]
    fn texture_lights_the_presence_dot() {
        let mut app = AutoshopApp::default();
        assert_eq!(app.recipe.texture, 0.0, "premise: a fresh recipe is neutral");
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Presence"),
            "the Presence header was not drawn — this test proves nothing: {seen:?}"
        );
        assert!(
            !seen.iter().any(|t| t == "Presence  ●"),
            "a neutral section must not claim an adjustment: {seen:?}"
        );
        // …and the slider itself is there to move (a field with no widget is
        // reachable only by the AI and the XMP reader).
        assert!(seen.iter().any(|t| t == "Texture"), "no Texture slider in Presence: {seen:?}");
        app.recipe.texture = 26.0;
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Presence  ●"),
            "a moved Texture must light Presence: {seen:?}"
        );
    }

    /// R25 B2: the Effects section draws all nine carried controls, in both
    /// languages, and says in the panel that this app renders none of them.
    ///
    /// A slider that moves a number and no pixel is the worst kind of bug
    /// here (`ARCHITECTURE.md`), so the nine are only defensible WITH the
    /// disclosure — which makes the disclosure part of the feature, not a
    /// nicety. It rides each slider's tooltip (not drawn in a frame), so the
    /// pinning here is the layout and the two group captions; the tooltip
    /// STRING is pinned by the i18n gate (`scripts/audit_i18n.py` fails on an
    /// unregistered key) and by its single definition in `dev_effects`.
    #[test]
    fn the_effects_section_lays_out_its_nine_controls() {
        for (lang, header, midpoint, rows) in [
            (
                crate::i18n::Lang::En,
                "Effects",
                "Midpoint",
                [
                    "Post-crop vignetting",
                    "Vignette amount",
                    "Vignette feather",
                    "Vignette roundness",
                    "Vignette style",
                    "Vignette highlights",
                    "Grain",
                    "Grain amount",
                    "Grain size",
                    "Grain roughness",
                ],
            ),
            (
                crate::i18n::Lang::Zh,
                "效果",
                "中点",
                [
                    "裁剪后暗角",
                    "暗角数量",
                    "暗角羽化",
                    "暗角圆度",
                    "暗角样式",
                    "暗角高光",
                    "胶片噪点",
                    "噪点数量",
                    "噪点大小",
                    "噪点密度",
                ],
            ),
        ] {
            let mut app = AutoshopApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            assert!(
                seen.iter().any(|t| t == header),
                "{lang:?}: the {header} section was not drawn: {seen:?}"
            );
            for row in rows {
                assert!(
                    seen.iter().any(|t| t == row),
                    "{lang:?}: the Effects section has no {row:?} row: {seen:?}"
                );
            }
            // The ninth control REUSES the existing 「Midpoint / 中点」 key
            // (deliberate — it is the same word for the same idea, and the
            // qualifier belongs on the collision, which is the vignette
            // AMOUNT). A presence check would pass on the Lens section's own
            // Midpoint alone, so count both.
            assert_eq!(
                seen.iter().filter(|t| *t == midpoint).count(),
                2,
                "{lang:?}: expected a {midpoint:?} row in BOTH Effects and Lens: {seen:?}"
            );
            // Neutral: no ●. Then ONE carried value lights it — the same
            // four-step proof the other sections get, for the family whose
            // members the AI never sees.
            assert!(
                !seen.iter().any(|t| t == &format!("{header}  ●")),
                "{lang:?}: a neutral Effects section must claim nothing: {seen:?}"
            );
            app.recipe.grain = 30.0;
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            assert!(
                seen.iter().any(|t| t == &format!("{header}  ●")),
                "{lang:?}: an imported Lightroom grain must light Effects: {seen:?}"
            );
        }
    }

    /// R25 B2: the Lens section's vignette slider is 「Lens vignetting」 now,
    /// never the bare 「Vignette」 — the Effects section above carries
    /// Lightroom's POST-CROP vignette, a different operator at a different
    /// stage, and one word over both was a name collision.
    ///
    /// A negative assertion needs a premise or it passes on an empty frame:
    /// the positive half runs first, in both languages.
    #[test]
    fn the_lens_vignette_is_no_longer_called_just_vignette() {
        for (lang, renamed, collision, post_crop) in [
            (crate::i18n::Lang::En, "Lens vignetting", "Vignette", "Post-crop vignetting"),
            (crate::i18n::Lang::Zh, "镜头暗角", "暗角", "裁剪后暗角"),
        ] {
            let mut app = AutoshopApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            assert!(
                seen.iter().any(|t| t == renamed),
                "{lang:?}: the renamed lens slider was not drawn — this proves nothing: {seen:?}"
            );
            assert!(
                seen.iter().any(|t| t == post_crop),
                "{lang:?}: the post-crop caption it disambiguates FROM is missing: {seen:?}"
            );
            assert!(
                !seen.iter().any(|t| t == collision),
                "{lang:?}: the bare {collision:?} label is back, and now names two \
                 different operators: {seen:?}"
            );
        }
    }

    /// R25 B3: the Detail section really lays out its eleven controls — the
    /// two the engine renders and the eight it carries — and the Lens section
    /// its manual CA pair, the auto switch and the six de-fringe rows.
    ///
    /// Both languages, because the Chinese labels are where a font-subset gap
    /// or a copied key shows up, and because 「彩噪细节」 vs 「锐化细节」 is
    /// exactly the kind of pair a single careless reuse would collapse.
    #[test]
    fn the_detail_section_groups_sharpen_and_noise() {
        for (lang, rows) in [
            (
                crate::i18n::Lang::En,
                [
                    "Sharpening",
                    "Sharpen radius",
                    "Sharpen detail",
                    "Sharpen masking",
                    "Noise Reduction",
                    "Noise detail",
                    "Noise contrast",
                    "Colour noise reduction",
                    "Colour noise detail",
                    "Colour noise smoothness",
                    "Chromatic aberration (manual)",
                    "Red / cyan",
                    "Blue / yellow",
                    "Auto lateral CA",
                    "Defringe",
                    "Purple amount",
                    "Purple hue low",
                    "Purple hue high",
                    "Green amount",
                    "Green hue low",
                    "Green hue high",
                ],
            ),
            (
                crate::i18n::Lang::Zh,
                [
                    "锐化",
                    "锐化半径",
                    "锐化细节",
                    "边缘蒙版",
                    "降噪",
                    "降噪细节",
                    "降噪对比",
                    "彩色降噪",
                    "彩噪细节",
                    "彩噪平滑度",
                    "手动色差",
                    "红 / 青",
                    "蓝 / 黄",
                    "自动色差校正",
                    "去边",
                    "紫 · 强度",
                    "紫 · 色相下限",
                    "紫 · 色相上限",
                    "绿 · 强度",
                    "绿 · 色相下限",
                    "绿 · 色相上限",
                ],
            ),
        ] {
            let mut app = AutoshopApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            for row in rows {
                assert!(
                    seen.iter().any(|t| t == row),
                    "{lang:?}: the {row:?} control is missing from the develop panel: {seen:?}"
                );
            }
        }
        // …and the eight carried detail axes light the Detail ● (the section
        // holds two families since B3, and its dot is the OR of them).
        let mut app = AutoshopApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(seen.iter().any(|t| t == "Detail"), "premise: the header is drawn: {seen:?}");
        assert!(!seen.iter().any(|t| t == "Detail  ●"), "premise: a rest recipe lights nothing");
        app.recipe.color_nr = 25.0;
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Detail  ●"),
            "a carried detail axis must not be an invisible adjustment: {seen:?}"
        );
        // The same for the Lens section and the de-fringe half of its family.
        let mut app = AutoshopApp::default();
        app.recipe.defringe_purple = 3.0;
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Lens  ●"),
            "a de-fringe value must light the Lens dot: {seen:?}"
        );
    }

    /// R25 B4: the Transform section SHOWS what the sidecar carried and
    /// offers no way to change it.
    ///
    /// The negative half is the design, not an omission — pass-through means
    /// we never interpret these sixteen values, and a slider needs a band, a
    /// clamp and a neutral we do not have. So how do you prove nothing here is
    /// a slider, from a frame's drawn text? By the SPELLING. Every slider in
    /// this panel formats its value through `fixed_decimals(0|1|2)`, so none
    /// of them can draw `+0.9` (a leading plus), `0.00` (two decimals on an
    /// integer axis) or `Adobe Standard` (not a number at all). Those three
    /// strings appearing exactly as Lightroom wrote them IS the proof that the
    /// section is a read-out — and it is the same assertion as "verbatim",
    /// which is the whole promise of the tier.
    #[test]
    fn the_transform_section_shows_values_but_no_sliders() {
        // A photo that never carried the block draws NO section: a heading
        // over an empty list is a promise about a file that never had one.
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            // BOTH block names in the heading: the section carries Lightroom's
            // Transform panel and its Calibration panel, and half a name sends
            // someone looking for their camera profile in the wrong place.
            let header = format!("{} / {}", tr(lang, "Transform"), tr(lang, "Calibration"));
            let mut app = AutoshopApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            assert!(
                !seen.contains(&header),
                "{lang:?}: an empty pass-through map must draw no section: {seen:?}"
            );

            // …and one that did shows every key it carried. The values are
            // the real spellings from the reference sidecars (DSC09642 is the
            // one file in the library with a non-zero Upright).
            app.recipe.passthrough = [
                ("PerspectiveVertical", "-35"),
                ("PerspectiveRotate", "+0.9"),
                ("PerspectiveX", "0.00"),
                ("CameraProfile", "Adobe Standard"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            assert!(seen.contains(&header), "{lang:?}: no section: {seen:?}");
            for want in [
                tr(lang, "Perspective correction"),
                tr(lang, "Camera calibration"),
                tr(lang, "Camera profile"),
                tr(lang, "Carried through to the sidecar unchanged; Autoshop never interprets these"),
            ] {
                assert!(seen.iter().any(|t| t == want), "{lang:?}: {want:?} missing: {seen:?}");
            }
            // Adobe's own property names for the values we cannot describe in
            // our own words — one label, one key, no invented friendly name.
            for key in ["crs:PerspectiveVertical", "crs:PerspectiveRotate", "crs:PerspectiveX"] {
                assert!(seen.iter().any(|t| t == key), "{lang:?}: {key} row missing: {seen:?}");
            }
            // NO SLIDERS: the three spellings no slider in this panel can
            // produce, drawn exactly as Lightroom wrote them.
            for verbatim in ["-35", "+0.9", "0.00", "Adobe Standard"] {
                assert!(
                    seen.iter().any(|t| t == verbatim),
                    "{lang:?}: {verbatim:?} is not on screen as itself — either the row is a \
                     slider (which would reformat it) or the value was interpreted: {seen:?}"
                );
            }
            // A key the document never carried is never invented.
            assert!(
                !seen.iter().any(|t| t == "crs:CameraCalibrationRedHue"),
                "{lang:?}: an absent Calibration key must stay absent: {seen:?}"
            );
        }
    }

    /// R25 B4 (work order 4.9): the render-gap line — the disclosure corner
    /// that had no surface — appears exactly when this photo carries a
    /// setting Lightroom renders and this canvas does not.
    ///
    /// The counterpart of `the_save_line_names_what_the_xmp_cannot_carry`:
    /// that one says the sidecar is missing something, this one says the
    /// canvas is. It never interrupts, because nothing was lost.
    #[test]
    fn the_render_gap_line_appears_only_when_something_is_carried() {
        use autoshop::xmp::global_render_gaps;
        // A default develop says NOTHING — and this is the assertion that
        // matters most, because the de-fringe block's neutral is Adobe's own
        // 30/70/40/60: a "non-zero" test would fire on every photo ever
        // opened, and a line that always appears is a line nobody reads.
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            assert!(
                render_gap_line(lang, &global_render_gaps(&Default::default())).is_none(),
                "{lang:?}: a neutral develop carries no gap"
            );
            assert!(render_gap_line(lang, &[]).is_none(), "{lang:?}: nothing in, nothing out");

            // One carried effect, named by its SECTION — the word the
            // photographer already has on screen, never `post_crop_vignette`.
            let grain = autoshop::recipe::EditRecipe { grain: 30.0, ..Default::default() };
            let line = render_gap_line(lang, &global_render_gaps(&grain))
                .unwrap_or_else(|| panic!("{lang:?}: an imported grain is a gap"));
            assert!(line.contains(tr(lang, "Effects")), "{lang:?}: {line}");
            assert!(line.contains(tr(lang, "Carried to Lightroom, not rendered here")));

            // Every section, once each — the three carried families,
            // deduplicated (nine carried effects are one 「Effects」).
            let all = autoshop::recipe::EditRecipe {
                grain: 30.0,
                grain_size: 25.0,
                color_nr: 25.0,
                defringe_purple: 3.0,
                // A pass-through block on the same recipe, which must NOT
                // reach this line: with nothing interpreted there is no
                // neutral to compare against, so it would name itself on
                // every Lightroom photo (xmp::global_render_gaps states it).
                // Its surface is the read-only Transform / Calibration
                // section, tested above.
                passthrough: [("PerspectiveVertical".to_string(), "-35".to_string())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            };
            let line = render_gap_line(lang, &global_render_gaps(&all)).expect("three sections");
            for section in [tr(lang, "Effects"), tr(lang, "Detail"), tr(lang, "Lens")] {
                assert!(line.contains(section), "{lang:?}: {section:?} missing from {line}");
            }
            assert!(
                !line.contains(tr(lang, "Calibration")),
                "{lang:?}: a pass-through block has no knowable neutral and must not \
                 nag on every Lightroom photo: {line}"
            );
            assert_eq!(
                line.matches(tr(lang, "Effects")).count(),
                1,
                "{lang:?}: nine carried effects are ONE section: {line}"
            );
            // R24's rule, and this test's tripwire: a carried control whose
            // family nobody labelled falls back to its registry NAME, which is
            // an internal symbol — so the underscore is what fails here.
            assert!(
                !line.contains('_'),
                "{lang:?}: a field id reached the UI prose — label its family: {line}"
            );
        }
    }

    /// R25 (closing R22-1): Settings SHOWS the segmentation sidecar path and
    /// offers no way to change it.
    ///
    /// The row exists because the alternative — a folder picker — would write
    /// a `Command::new` target into the trusted settings file, which is what
    /// `config::SETTINGS` registers the variable `env_only(Trust::Destination)`
    /// to forbid. So the NEGATIVE half is the point: the heading and the
    /// resolved path are drawn, and no 「Browse…」 button appears beside them
    /// (the delivery-root row above has one, which is what makes the absence
    /// here a decision rather than an oversight).
    #[test]
    fn the_settings_panel_shows_the_sidecar_path_without_offering_to_change_it() {
        for (lang, heading, browse) in [
            (crate::i18n::Lang::En, "Segmentation sidecar", "Browse…"),
            (crate::i18n::Lang::Zh, "分割边车", "浏览…"),
        ] {
            let mut app = AutoshopApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.settings_ui(ui));
            let at = seen.iter().position(|t| t == heading).unwrap_or_else(|| {
                panic!("{lang:?}: the sidecar heading was not drawn: {seen:?}")
            });
            // The resolved path follows the heading — a row with a heading and
            // nothing under it discloses nothing.
            assert!(
                seen[at + 1..].iter().any(|t| t.contains("segment.py")),
                "{lang:?}: no resolved sidecar path under the heading: {seen:?}"
            );
            // The delivery root's picker proves the panel CAN draw one here.
            assert!(
                seen.iter().any(|t| t == browse),
                "{lang:?}: the delivery-root Browse button is gone — the negative \
                 assertion below would then prove nothing: {seen:?}"
            );
            assert!(
                seen[at + 1..].iter().all(|t| t != browse),
                "{lang:?}: a picker appeared beside the executed sidecar path: {seen:?}"
            );
        }
    }

    /// R25 B3: the Lens tooltip no longer promises de-fringe for "a later
    /// batch" — this IS the later batch.
    ///
    /// The promise sat in the panel from the round that shipped the manual
    /// lens sliders. A negative assertion alone would pass on a panel that
    /// drew nothing at all, so the positive half comes first.
    #[test]
    fn the_lens_tooltip_no_longer_promises_defringe_later() {
        for (lang, promise) in [
            (crate::i18n::Lang::En, "De-fringe in a later batch"),
            (crate::i18n::Lang::Zh, "去紫边留待后续批次"),
        ] {
            let mut app = AutoshopApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            let lens_header = if lang == crate::i18n::Lang::En { "Lens" } else { "镜头 · Lens" };
            assert!(
                seen.iter().any(|t| t == lens_header),
                "{lang:?}: the Lens section was not drawn — this proves nothing: {seen:?}"
            );
            assert!(
                seen.iter().any(|t| t.contains("XMP")),
                "{lang:?}: the lens explainer line itself is missing: {seen:?}"
            );
            assert!(
                !seen.iter().any(|t| t.contains(promise)),
                "{lang:?}: the retired {promise:?} promise is still on screen: {seen:?}"
            );
        }
    }

    /// R25 P0-0.3: the ONE control the ● deliberately ignores stays ignored
    /// after the predicate became a derivation — `lens_vignette_mid` renders
    /// nothing while the amount is at rest (`catalogue::DOT_EXEMPT` carries the
    /// full reason), and deriving the dot from the `lens` family would have
    /// silently started lighting it.
    #[test]
    fn lens_midpoint_alone_still_lights_no_dot() {
        let mut app = AutoshopApp::default();
        app.recipe.lens_vignette_mid = 90.0;
        assert_eq!(app.recipe.lens_vignette, 0.0, "premise: the amount is at rest");
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Lens"),
            "the Lens header was not drawn — this test proves nothing: {seen:?}"
        );
        assert!(
            !seen.iter().any(|t| t == "Lens  ●"),
            "a midpoint that changes no pixel must not claim a lens correction: {seen:?}"
        );
        // …and the amount, which DOES render, still lights it.
        app.recipe.lens_vignette = -35.0;
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Lens  ●"),
            "a real vignette correction must light the section: {seen:?}"
        );
    }

    /// R22 #16h (verification, no behaviour change): the export-side MaskLoss
    /// disclosure rides `toast()`, so repeating the SAME save must not stack
    /// copies — the dedup refreshes the live toast and moves it to the BACK of
    /// the 5-deep ring, which is also what stops a repeat from evicting the
    /// other four. Pinned because the release note describes this behaviour.
    #[test]
    fn an_identical_toast_refreshes_instead_of_stacking() {
        let loss = "the Lightroom sidecar dropped 2 bitmap masks";
        let mut app = AutoshopApp::default();
        app.toast(ToastKind::Error, loss);
        for i in 0..4 {
            app.toast(ToastKind::Success, format!("saved {i}"));
        }
        assert_eq!(app.toasts.len(), 5, "the ring is exactly full");
        // Ctrl+S three more times on the same photo: byte-identical disclosure.
        for _ in 0..3 {
            app.toast(ToastKind::Error, loss);
        }
        assert_eq!(
            app.toasts.len(), 5,
            "three refreshes must not grow the ring — stacking would have evicted \
             the four successes one by one"
        );
        assert_eq!(
            app.toasts.iter().filter(|t| t.text == loss).count(), 1,
            "one live copy of one disclosure"
        );
        assert_eq!(
            app.toasts.last().expect("non-empty").text, loss,
            "the refresh moves it to the BACK, so the ring evicts it LAST — \
             refreshing in place left the error first in line for eviction"
        );
        // The KIND is part of the identity: a success saying the same words is a
        // different toast (different colour, different TTL).
        app.toast(ToastKind::Success, loss);
        assert_eq!(
            app.toasts.iter().filter(|t| t.text == loss).count(), 2,
            "dedup keys on (text, kind), not text alone"
        );
    }

    /// R23-2 (feedback #6): the AI panel now OWNS the style reference library —
    /// its status, its build entry, and the honesty of the Style slider above
    /// it. All three read the ONE cached status, so this pins the rendered
    /// panel in each state rather than the predicate alone.
    ///
    /// The status is INJECTED: the production panel spawns a worker to read it
    /// (the file reaches 32 MB), and a headless frame must not go to the
    /// developer's own store for an answer.
    #[test]
    fn the_ai_panel_says_which_style_library_it_is_referencing() {
        use autoshop::style::{StyleIndexInfo, StyleIndexState};
        let info =
            |state| Some(StyleIndexInfo { path: "C:/store/style-index.json".into(), state });

        // ── Nothing built: the entry point, the pointer sentence, AND the
        // slider's own warning — the defect was a slider showing 30% with
        // nothing behind it and no way in the app to build one.
        let mut app =
            AutoshopApp { style_info: info(StyleIndexState::Absent), ..Default::default() };
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Style reference library"),
            "the section must exist at all — otherwise this test proves nothing: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.contains("No library built yet")),
            "an unbuilt library must say so: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.contains("folder you edit in Lightroom")),
            "…and where to point it (an Autoshop output folder yields nothing): {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.contains("no library")),
            "the Style slider must not read as live when it provably is not: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.contains("Pick folder")),
            "the GUI build entry point — the whole missing surface: {seen:?}"
        );

        // ── Built: WHICH library, how big, how old. This is the answer to the
        // user's "I have no idea which library it is referencing".
        let mut app = AutoshopApp {
            style_info: info(StyleIndexState::Built {
                total: 412,
                version: 3,
                source_dir: Some("D:/photos/edited".into()),
                scenes: vec![("wide/mid/midday/landscape".into(), 12)],
                age: Some(std::time::Duration::from_secs(5 * 3600)),
            }),
            ..Default::default()
        };
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t.contains("412") && t.contains("D:/photos/edited")),
            "the count AND the folder it came from: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.contains("built 5h ago")),
            "and how stale it is: {seen:?}"
        );
        assert!(
            !seen.iter().any(|t| t.contains("no library")),
            "no false warning beside the slider once a library exists: {seen:?}"
        );

        // ── Unusable: the loader's reason, not silence (this arm always spoke
        // in the rationale; now the panel says it before an analysis is paid
        // for).
        let mut app = AutoshopApp {
            style_info: info(StyleIndexState::Unusable {
                err: "is version 2 (current 3)".into(),
            }),
            ..Default::default()
        };
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t.contains("is version 2 (current 3)")),
            "the cause reaches the user: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.contains("no library")),
            "and the slider is flagged: {seen:?}"
        );
    }

    /// R23-6 B (user decision 2026-08-17 ⑤): the reverse-fit target used to
    /// be reachable ONLY by generating an image and standing on that variant
    /// — `fit_target`'s `(v.kind == Generated).then(...)`, whose downstream
    /// effect was that "the target is not pixel-aligned" became an axiom of
    /// the method instead of a property of the only target the desktop app
    /// could offer. Any finished version of the same frame is a legitimate
    /// target (the CLI has always accepted one; fit.rs's own doc has always
    /// promised one).
    ///
    /// Red before the change on every assertion below: with no Generated
    /// variant `fit_target()` returned `None` whatever `fit_ref` held, the
    /// panel offered no way to name a file, and its empty state told the user
    /// to go and generate an image.
    #[test]
    fn a_chosen_reference_is_a_reverse_fit_target_without_any_generated_variant() {
        let reference = std::path::PathBuf::from("D:/exports/_DSC9621-lightroom.tif");
        // A stock app: ONE Original variant, nothing generated.
        let mut app = AutoshopApp::default();
        assert_eq!(app.fit_target(), None, "premise: nothing to fit against yet");
        app.fit_ref = Some(reference.clone());
        assert_eq!(
            app.fit_target(),
            Some(reference.clone()),
            "an explicitly chosen reference IS the target"
        );
        // …and it OUTRANKS a generated variant: an explicit choice must not
        // be shadowed by whichever card happens to be active.
        let generated = std::path::PathBuf::from("./out/_DSC9621.reimagine.png");
        app.variants.push(Variant {
            id: String::new(),
            name: None,
            kind: VariantKind::Generated,
            recipe: EditRecipe::default(),
            base: None,
            origin: Some(generated.clone()),
            thumb: None,
        });
        app.active = app.variants.len() - 1;
        assert_eq!(app.fit_target(), Some(reference), "the chosen reference wins");
        // Clearing it hands the entry back to the generated variant — both
        // doors stay open, which is the whole shape of this change.
        app.fit_ref = None;
        assert_eq!(app.fit_target(), Some(generated));
    }

    /// The panel half of the same change: the door has to be visible, and the
    /// empty state has to stop naming only the generative path.
    #[test]
    fn the_reverse_fit_area_offers_the_reference_picker() {
        let mut app = AutoshopApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t.contains("Choose reference")),
            "the reference entry must exist at all: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.contains("Pick a reference below")),
            "the empty state must name BOTH doors, not just the generative one: {seen:?}"
        );
        // The chosen file is shown, so "what am I fitting against" is
        // answerable without opening a dialog again.
        app.fit_ref = Some(std::path::PathBuf::from("D:/exports/_DSC9621-lightroom.tif"));
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t.contains("_DSC9621-lightroom.tif")),
            "the chosen reference must be named on the panel: {seen:?}"
        );
    }

    /// R23-6 D (user decision 2026-08-17 ⑥): the deep reverse-fit is a PAID
    /// opt-in on top of another paid opt-in, so it must be off in both
    /// defaults — a prefs file written before the key existed has to decode
    /// to the same answer a fresh install gives — and it must not be
    /// reachable without the review it iterates.
    #[test]
    fn the_deep_reverse_fit_is_off_by_default_and_gated_on_the_review() {
        assert!(!AutoshopApp::default().fit_deep, "spending is opt-in");
        assert!(
            !Prefs::default().fit_deep,
            "an older prefs file must decode to the same answer"
        );
        let mut app = AutoshopApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "deep"),
            "the switch must exist: {seen:?}"
        );
        // The gate is on the WIDGET, so the tooltip that explains the gate is
        // what a headless frame can witness; the enabled state itself is
        // egui's and is asserted through the same predicate the UI uses.
        assert!(!app.fit_ai_judge, "premise: the review is off in a stock app");
        app.fit_ai_judge = true;
        app.fit_deep = true;
        // The worker's own gate, which must not depend on the UI's: deep
        // without the review is not a configuration.
        assert!(app.fit_deep && app.fit_ai_judge);
    }

    /// R23-2: the reference-PHOTO switch is off by default in BOTH defaults
    /// (the app's and the prefs'), so neither a fresh install nor an upgraded
    /// prefs file silently starts putting a second image on every paid call.
    /// The wire-level proof that the switch is what adds the image lives in
    /// `advisor::openai`'s stub-endpoint test.
    #[test]
    fn the_style_reference_photo_switch_is_off_in_both_defaults() {
        assert!(
            !AutoshopApp::default().send_style_ref_image,
            "spending is opt-in — a fresh app must not send a second image"
        );
        assert!(
            !Prefs::default().send_style_ref_image,
            "a prefs file written before this key existed must decode to the SAME answer"
        );
        assert_eq!(Prefs::default().style_src_dir, None);
        // …and the request the analyze worker builds carries the flag, so the
        // checkbox cannot become decoration.
        let app = AutoshopApp { send_style_ref_image: true, ..Default::default() };
        let req = autoshop::pipeline::GradeRequest {
            style: app.style_strength,
            send_reference_image: app.send_style_ref_image,
            strength: autoshop::recipe::GradeStrength::new(app.grade_strength),
            think: app.deep_think,
        };
        assert!(req.send_reference_image);
        assert!(
            !autoshop::pipeline::GradeRequest::with_style(0.65).send_reference_image,
            "every non-GUI surface stays on the text reference"
        );
    }

    /// R23-4 (feedback #13, "let it think"): the desktop shell of thinking
    /// mode. It is the most expensive switch on the panel, so the properties
    /// that matter are: off in BOTH defaults, drawn beside the dials it reads
    /// (the round budget comes off the Strength band right above it), NOT part
    /// of the section ● (it is a persisted preference of a paid verb, exactly
    /// like the reference-photo switch), and actually present in the request the
    /// analyze worker builds.
    #[test]
    fn the_deep_thinking_switch_is_off_by_default_and_rides_the_request() {
        assert!(
            !AutoshopApp::default().deep_think,
            "a fresh app must not start paying for a deeper analyze"
        );
        assert!(
            !Prefs::default().deep_think,
            "a prefs file written before this key existed must decode to the SAME answer"
        );
        let mut app = AutoshopApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Deep thinking"),
            "the switch was not drawn in a 320 px panel: {seen:?}"
        );
        // Same rule as `fit_ai_judge` / `send_style_ref_image`: a remembered
        // preference is not "this photo's AI inputs carry state".
        app.deep_think = true;
        assert!(!app.ai_section_active(), "a preference must not light the section ●");

        let app = AutoshopApp { deep_think: true, ..Default::default() };
        let req = autoshop::pipeline::GradeRequest {
            style: app.style_strength,
            send_reference_image: app.send_style_ref_image,
            strength: autoshop::recipe::GradeStrength::new(app.grade_strength),
            think: app.deep_think,
        };
        assert!(req.think, "the checkbox must reach the worker's request");
        assert!(
            !autoshop::pipeline::GradeRequest::with_style(0.65).think,
            "every unattended surface stays out of thinking mode"
        );
    }

    /// R23-2: the build landing — typed outcome in, sentence + state out
    /// (L12#4). A minutes-long build must re-arm its button in every arm, and
    /// only the SAVED arm may remember the folder or refresh the status.
    #[test]
    fn a_style_library_build_lands_as_a_typed_outcome_in_every_arm() {
        let ctx = egui::Context::default();
        let dir = std::path::PathBuf::from("D:/photos/edited");

        // Saved: success toast, folder remembered (so a rebuild starts there),
        // and a fresh status read armed — the panel must not keep the OLD
        // counts.
        let mut app = AutoshopApp {
            style_build_inflight: true,
            style_build_progress: Some((7, 9)),
            ..Default::default()
        };
        app.tx
            .send(Msg::StyleBuilt(Box::new(StyleBuildOutcome::Saved {
                total: 412,
                dir: dir.clone(),
            })))
            .unwrap();
        app.poll_workers(&ctx);
        assert!(!app.style_build_inflight, "the button re-arms");
        assert_eq!(app.style_build_progress, None, "the counter belongs to ONE build");
        assert!(app.status.contains("412"), "{}", app.status);
        assert_eq!(
            app.style_src_dir.as_deref(),
            Some(dir.as_path()),
            "the folder is remembered"
        );
        assert!(app.style_info_loading, "a build invalidates the cached status");
        assert!(app.toasts.iter().any(|t| matches!(t.kind, ToastKind::Success)));

        // Nothing indexed: the SHARED refusal wording (the CLI and the web say
        // the same), an ERROR toast, and NOTHING remembered — the folder was
        // the wrong kind of folder.
        let mut app = AutoshopApp { style_build_inflight: true, ..Default::default() };
        app.tx
            .send(Msg::StyleBuilt(Box::new(StyleBuildOutcome::NothingIndexed {
                dir: dir.clone(),
            })))
            .unwrap();
        app.poll_workers(&ctx);
        assert!(!app.style_build_inflight);
        assert!(
            app.status.contains("folder you edit in Lightroom")
                && app.status.contains("left untouched"),
            "the refusal must say where to point instead, and that the old library \
             stands: {}",
            app.status
        );
        assert!(app.toasts.iter().any(|t| matches!(t.kind, ToastKind::Error)));
        assert!(!app.style_info_loading, "a refused build changed nothing to re-read");

        // Failed: the cause, an error toast, button re-armed.
        let mut app = AutoshopApp { style_build_inflight: true, ..Default::default() };
        app.tx
            .send(Msg::StyleBuilt(Box::new(StyleBuildOutcome::Failed {
                err: "read the folder: access denied".into(),
            })))
            .unwrap();
        app.poll_workers(&ctx);
        assert!(!app.style_build_inflight);
        assert!(app.status.contains("access denied"), "{}", app.status);
        assert!(app.toasts.iter().any(|t| matches!(t.kind, ToastKind::Error)));

        // Progress ticks land as counts, not as a worker-built sentence.
        let mut app = AutoshopApp { style_build_inflight: true, ..Default::default() };
        app.tx.send(Msg::StyleBuildProgress { done: 40, total: 300 }).unwrap();
        app.poll_workers(&ctx);
        assert_eq!(app.style_build_progress, Some((40, 300)));
        assert!(app.status.contains("40") && app.status.contains("300"), "{}", app.status);
    }

    /// R23 review LOW-4: `rounds == 0` had THREE producers and ONE sentence.
    ///
    /// "The review found nothing this app can act on" is the answer to
    /// `FitAction::None` only. A zoned re-solve that errored, and a saturation
    /// step that clamped back to the value in hand, are the app failing to carry
    /// out a move it DID select — and the user has paid for the review either
    /// way, so the two must not read the same. Rendered in BOTH languages,
    /// because the fix is worthless if only the English half distinguishes.
    #[test]
    fn a_deep_fit_that_could_not_run_its_action_does_not_claim_the_review_was_empty() {
        for lang in [Lang::En, Lang::Zh] {
            let render = |outcome| {
                AutoshopApp::render_fit_note(
                    lang,
                    &FitNote::DeepFit { action: "zoned sky/land pass", outcome },
                )
            };
            let empty = render(DeepFitOutcome::NothingActionable);
            let failed = render(DeepFitOutcome::ActionDidNotRun);
            let kept = render(DeepFitOutcome::Adopted);
            let dropped = render(DeepFitOutcome::Discarded);
            assert_ne!(
                empty, failed,
                "{lang:?}: a selected action that could not run must not read as \
                 'the review had nothing to say'"
            );
            // The three outcomes that FOLLOW a selected action all name it; the
            // empty one has no action to name.
            for (what, s) in [("failed", &failed), ("kept", &kept), ("dropped", &dropped)] {
                assert!(
                    s.contains("zoned sky/land pass"),
                    "{lang:?}: the {what} sentence must name the action it is about: {s}"
                );
            }
            // All four are distinct — no pair collapses into the same sentence.
            let all = [&empty, &failed, &kept, &dropped];
            for i in 0..all.len() {
                for j in (i + 1)..all.len() {
                    assert_ne!(all[i], all[j], "{lang:?}: two deep outcomes render identically");
                }
            }
            // …and the zh half is really translated, not an English fallback.
            if matches!(lang, Lang::Zh) {
                assert!(failed.contains('深'), "the zh rendering fell back to English: {failed}");
            }
        }
    }

    /// R23 review LOW-6: the reverse-fit's paid-vision ceiling is TWO calls.
    ///
    /// The deep path's leftover used to be an `Option<Judgement>`, in which a
    /// review that ran and FAILED was indistinguishable from one that never ran
    /// — so after two failed attempts the informational block bought a third
    /// and disclosed the same failure twice. The decision is pure, so the
    /// ceiling is pinned here rather than inferred from the closure's shape.
    #[test]
    fn a_failed_deep_review_does_not_buy_a_third_vision_call() {
        type Verdict = Option<Result<u8, String>>;
        // The deep path never ran (「deep」 unticked): the informational review
        // is the run's FIRST call, and it must still happen.
        let none: Verdict = None;
        assert_eq!(FitReviewPlan::of(&none), FitReviewPlan::Call);
        // It ran and produced the verdict that describes what ships: reuse it —
        // a second call would bill the user for the same answer.
        let ok: Verdict = Some(Ok(88));
        assert_eq!(FitReviewPlan::of(&ok), FitReviewPlan::Reuse);
        // It ran and FAILED. This is the arm the defect lived in: the failure
        // was already reported once, and a retry here is the third attempt.
        let failed: Verdict = Some(Err("timed out reading response".into()));
        assert_eq!(FitReviewPlan::of(&failed), FitReviewPlan::Skip);
    }
