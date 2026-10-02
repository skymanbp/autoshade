// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the save line, the import banner, the rotation warning, calibration losses and the control census against the engine.

    /// M6a: the save status line the user actually reads. The projection's
    /// losses were disclosed on the IMPORT side four times over and on the
    /// EXPORT side never — a sidecar quietly missing two AI masks looked like
    /// a clean "XMP + recipe saved". The counts here come from the writer's
    /// own verdicts (`xmp::mask_export_losses`); this pins that every category
    /// reaches the UI, in both languages, and that a faithful save says
    /// NOTHING (an unconditional line would train the user to ignore it).
    #[test]
    fn the_save_line_names_every_projection_loss_and_stays_quiet_otherwise() {
        use autoshade::xmp::{MaskLoss, MaskLossReason as R};
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
                    "bitmap components omitted ×1",
                    "radial rotation ×1",
                    "recolour gains ×1",
                ],
            ),
            (
                crate::i18n::Lang::Zh,
                [
                    "位图蒙版 ×2",
                    "已静音蒙版 ×1",
                    "位图组件未写入 ×1",
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

    /// The line a ZONED save actually produces now, which is two different
    /// sentences in one breath.
    ///
    /// The sky/land zones ride out as Lightroom's own Select Sky, so their
    /// entry is the AI one — nothing was left out of the sidecar, and what the
    /// reader must not assume is that the alpha was Adobe's. Hard spatial
    /// tiles export as gradients; only retained refined tiles and free-form
    /// field masks keep the bitmap loss and its count. Before the
    /// carrier change this line said 「bitmap masks ×4」 and the sky was one of
    /// the four.
    ///
    /// MUTATION: fold `AiMaskRecomputed` into the `Bitmap` arm of
    /// `xmp_loss_line` and the two-category assertion fails.
    #[test]
    #[ignore = "lane measurement: AUTOSHADE_R35_RECIPE must name a scratch recipe inside this worktree"]
    fn r35_scratch_recipe_save_line_counts_only_the_remaining_bitmap_masks() {
        use autoshade::recipe::MaskGeometry;
        let path = std::path::PathBuf::from(std::env::var("AUTOSHADE_R35_RECIPE").expect("scratch recipe"));
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap();
        assert!(path.canonicalize().unwrap().starts_with(&root));
        let recipe: EditRecipe = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let losses = autoshade::xmp::mask_export_losses(&recipe);
        let globals = autoshade::xmp::global_export_losses(&recipe);
        let count = recipe.masks.iter().filter(|m| m.enabled && matches!(m.mask, MaskGeometry::Bitmap { .. })).count();
        let line = xmp_loss_line(crate::i18n::Lang::Zh, &losses, &globals).unwrap_or_default();
        if count == 0 { assert!(!line.contains("位图蒙版")); }
        else { assert!(line.contains(&format!("位图蒙版 ×{count}")), "{line}"); }
        eprintln!("R35 save line: {line}");
        eprintln!("R35 native tile names: {:?}", recipe.masks.iter().filter(|m|
            m.name.starts_with("Spatial tile") && matches!(m.mask, MaskGeometry::Linear { .. })
        ).map(|m| &m.name).collect::<Vec<_>>());
    }

    #[test]
    fn fit_mask_status_reads_the_recipe_and_never_counts_a_truncated_history() {
        use autoshade::recipe::{LocalAdjustment, MaskGeometry, MaskRole};
        use autoshade::rationale::{keys, Note, TRUNCATED_SENTINEL};
        let recipe = EditRecipe { masks: vec![
            LocalAdjustment { name: "Spatial tile r0c0".into(), ..Default::default() },
            LocalAdjustment { name: "Spatial tile r1c0".into(),
                mask: MaskGeometry::Bitmap { path: "refined.png".into() }, ..Default::default() },
            LocalAdjustment { name: "Spatial tile r2c0".into(), enabled: false, ..Default::default() },
            LocalAdjustment { name: "field-zone-1".into(),
                mask: MaskGeometry::Bitmap { path: "free.png".into() }, ..Default::default() },
            LocalAdjustment { role: MaskRole::ZoneSky,
                mask: MaskGeometry::select_sky(0.5, 0.25, false, "sky.png".into()), ..Default::default() },
        ], ..Default::default() };
        let mut history = vec![Note::plain(keys::TILE_ATTACHED),
            Note::plain(keys::MASK_REFINEMENT_KEPT), Note::plain(keys::MASK_REFINEMENT_ABSTAINED)];
        let complete = fit_mask_notes(&recipe, &history);
        assert!(complete.iter().any(|n| matches!(n, FitNote::IncludesSpatialTiles(2))));
        assert!(complete.iter().any(|n| matches!(n, FitNote::MaskRefinement { kept: 1, abstained: 1 })));
        history.push(Note::plain(TRUNCATED_SENTINEL));
        let truncated = fit_mask_notes(&recipe, &history);
        assert!(truncated.iter().any(|n| matches!(n, FitNote::IncludesSkyZone)));
        assert!(truncated.iter().any(|n| matches!(n, FitNote::IncludesSpatialTiles(2))));
        assert!(!truncated.iter().any(|n| matches!(n, FitNote::MaskRefinement { .. })));
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            for note in &truncated {
                let line = AutoShadeApp::render_fit_note(lang, note);
                assert!(line.contains("XMP"));
                assert!(!line.contains("global part only") && !line.contains("全局部分"));
                assert!(!line.contains("omitted from classic XMP"));
            }
        }
    }

    #[test]
    fn a_zoned_save_counts_only_free_or_refined_bitmaps_and_exports_geometry_tiles() {
        use autoshade::recipe::{LocalAdjustment, MaskCombine, MaskComponent, MaskGeometry};
        let tile = |name: &str| LocalAdjustment {
            name: name.into(),
            components: (0..3).map(|_| MaskComponent {
                mode: MaskCombine::Intersect, ..Default::default()
            }).collect(),
            ..Default::default()
        };
        let recipe = EditRecipe { masks: vec![
            LocalAdjustment {
                name: "Sky (reverse-fit)".into(),
                mask: MaskGeometry::select_sky(0.5, 0.25, false, "sky.png".into()),
                ..Default::default()
            },
            tile("Spatial tile d2 r1 c3"), tile("Spatial tile d2 r2 c0"),
            LocalAdjustment {
                name: "field-zone-1".into(), mask: MaskGeometry::Bitmap { path: "free.png".into() },
                ..Default::default()
            },
        ], ..Default::default() };
        let losses = autoshade::xmp::mask_export_losses(&recipe);
        for (lang, ai, bitmap) in [
            (
                crate::i18n::Lang::En,
                "AI masks ×1 re-derived locally — not Adobe's raster",
                "bitmap masks ×1",
            ),
            (
                crate::i18n::Lang::Zh,
                "AI 蒙版 ×1 由本机重算——非 Adobe 原栅格",
                "位图蒙版 ×1",
            ),
        ] {
            let line =
                xmp_loss_line(lang, &losses, &[]).expect("a zoned save has something to say");
            assert!(line.contains(ai), "{lang:?}: the zone's own sentence: {line}");
            assert!(line.contains(bitmap), "{lang:?}: the free raster is counted apart from native tiles: {line}");
            assert_eq!(
                line.matches('×').count(),
                2,
                "{lang:?}: two categories, never one: {line}"
            );
            // Both are NAMED, which is the actionable half of either sentence.
            assert!(line.contains("Sky (reverse-fit)"), "{lang:?}: {line}");
            assert!(line.contains("field-zone-1"), "{lang:?}: {line}");
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
        use autoshade::xmp::{MaskImportLoss, MaskImportReason as R};
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
    /// why matters because the answer is not a bug — v0.32.0 measured
    /// `crs:Angle`'s sign, pivot and magnitude, so what is left is the one
    /// case the fold cannot serve: the DOCUMENT DECLARES NO FRAME
    /// (`xmp::FrameAspect`), and without the aspect there is no pixel↔
    /// normalised conversion to make — and a line that does not say so reads
    /// as a bug.
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
        use autoshade::xmp::{
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
                    "Rotation {a}° not written to XMP (frame size unknown)",
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
                    "Rotation {a}° read as 0 (frame size unknown)",
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
            AutoShadeApp { recipe: autoshade::xmp::xmp_to_recipe(doc), ..Default::default() };
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
        use autoshade::advisor::catalogue::{Tier, RECIPE_CONTROLS, STAMPED_CALIBRATION};
        use autoshade::xmp::{MaskLoss, MaskLossReason as R};

        // Premise, from the registry rather than from memory. It used to read
        // "every global the export can lose IS engine calibration", and R33 §G
        // ended that: `colour_field` is an unexportable global the USER asked
        // for. So the premise is now the split itself — seven rows in the
        // tier since 2026-10-01's fill layers, of which exactly the
        // un-actionable ones are quiet.
        let rows: Vec<_> = RECIPE_CONTROLS
            .iter()
            .filter(|c| c.tier == Some(Tier::RenderedNotExported))
            .collect();
        assert_eq!(rows.len(), 7, "the tier's membership moved — re-read this test");
        assert!(
            rows.iter().all(|c| c.engine_only),
            "premise: no unexportable global is a value the advisor can state"
        );
        let (calibration, chosen): (Vec<&str>, Vec<&str>) = rows
            .iter()
            .map(|c| c.name)
            .partition(|name| STAMPED_CALIBRATION.contains(name));
        // Registry order, which is the order the panels draw in — Adobe's
        // Upright solution rides with the Transform panel, above the two the
        // engine measures for itself.
        assert_eq!(
            calibration,
            vec!["upright_transform", "look", "base_curve", "lens_profile"],
            "per-photo measurements, whether this engine took them or read Adobe's"
        );
        // v1.5.0 F9 gives this arm its second member, and the contrast with
        // `look` — which joined the QUIET arm in F7 — is the whole judgement.
        // A creative profile is on 92% of the library, has no picker to act on
        // and is preserved verbatim by the merge, so a warning on every save
        // would be noise. A retouch area is on 14% of it, a FRESH sidecar
        // write really does drop all 121 of them, and the panel now offers
        // something to do about it (「✨ Regenerate those areas」). Actionable
        // and not universal is exactly what the loud arm is for. A fill
        // layer (2026-10-01) is the same kind: the photographer made it, and
        // no sidecar write of any kind carries it.
        assert_eq!(
            chosen,
            vec!["retouch", "pixel_layers", "colour_field"],
            "…and the three the photographer asked for and can still act on"
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
        assert!(
            !xmp_loss_interrupts(&[], &["upright_transform"]),
            "…nor Adobe's own Upright solution, which the merge leaves in the document anyway"
        );
        // v1.5.0 F7, the widest member yet: 161 of the reference library's 175
        // sidecars carry a creative Look, 152 of them Lightroom's own default.
        // The merge preserves the element verbatim, so this one is not even a
        // loss on the path that matters — and there is no picker to act on.
        assert!(
            !xmp_loss_interrupts(&[], &["look"]),
            "…nor the creative profile Lightroom stamps on almost every file it touches"
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
        // R33 §G, the case that made the rule stop asking `engine_only`: the
        // advisor cannot state ninety-six grid vertices, but the photographer
        // asked for the field by raising Strength and removes it in one click.
        assert!(
            RECIPE_CONTROLS.iter().any(|c| c.name == "colour_field" && c.engine_only),
            "premise: it IS engine-only, which is what used to make it quiet"
        );
        assert!(
            xmp_loss_interrupts(&[], &["colour_field"]),
            "an unexportable control the user chose is actionable — it interrupts"
        );
        assert!(
            xmp_loss_line(crate::i18n::Lang::En, &[], &["colour_field"])
                .is_some_and(|line| line.contains("colour field")),
            "…and it is named in words, not by its registry symbol"
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
        let lib = std::env::temp_dir().join(format!("autoshade-lowr3-library-{}", std::process::id()));
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
            !delivery_root_shadows_photo(&std::env::temp_dir().join(format!("autoshade-lowr3-out-{}", std::process::id())), p),
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
        use autoshade::advisor::catalogue::{
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
            // recipes field by field and would swamp the extraction. The parts
            // under tests/ are this file (include!d), skipped the same way.
            if p.file_name().is_some_and(|n| n == "tests.rs")
                || p.parent().and_then(|d| d.file_name()).is_some_and(|d| d == "tests")
            {
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
