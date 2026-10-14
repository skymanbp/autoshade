// One part of the GUI's tests (src/bin/gui/tests.rs includes it): section dots, the curves, colour, effects, detail and transform sections, profiles, the render gap line and the settings panel.

    /// R22 #16d: the Local Masks header's ● was `n_masks > 0` while every ROW
    /// dot below it used the engine's own rule. So a list of muted or parked
    /// masks — no adjustment set, or the eye off — claimed an active local
    /// adjustment, and when the claim WAS true it only repeated the count the
    /// header already prints. Both now read `util::mask_active`.
    #[test]
    fn the_local_masks_dot_matches_its_row_dots() {
        let parked = |name: &str| autoshade::recipe::LocalAdjustment {
            mask: autoshade::recipe::MaskGeometry::Radial {
                top: 0.2, left: 0.2, bottom: 0.8, right: 0.8,
                feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                midpoint: 50.0, mask_version: 2,
            },
            name: name.into(),
            ..Default::default()
        };
        let mut app = AutoShadeApp::default();
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
        use autoshade::advisor::catalogue::{family_is_active, CONTROL_FAMILIES};
        /// (family, the header the panel draws for it, a move inside it)
        type Case = (&'static str, &'static str, fn(&mut autoshade::recipe::EditRecipe));
        let cases: [Case; 8] = [
            ("presence", "Presence", |r| r.dehaze = 40.0),
            ("detail", "Detail", |r| r.noise_reduction = 25.0),
            ("hsl", "Color Mixer (HSL)", |r| r.hsl.saturation[3] = -30.0),
            // v1.5.0: two more families under the mixer's own header — the
            // B&W treatment that replaces it and the point colours that
            // follow it — and the Calibration panel's own section.
            ("black_white", "Color Mixer (HSL)", |r| r.convert_to_grayscale = true),
            ("point_color", "Color Mixer (HSL)", |r| {
                r.point_colors = vec![autoshade::recipe::PointColor {
                    hue_shift: 0.3,
                    ..autoshade::recipe::PointColor::sampled(0.9, 0.7, 0.5)
                }]
            }),
            ("calibration", "Calibration", |r| r.cal_blue_hue = -40.0),
            ("color_grade", "Color Grading", |r| r.color_grade.highlight_sat = 20.0),
            ("curves", "Curves", |r| {
                r.green_curve = vec![autoshade::recipe::CurvePoint { input: 128, output: 140 }]
            }),
        ];
        for (family, header, mutate) in cases {
            let f = CONTROL_FAMILIES
                .iter()
                .find(|f| f.name == family)
                .unwrap_or_else(|| panic!("{family} is not a declared family"));
            let mut app = AutoShadeApp::default();
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

    /// v1.5.0: the Curves section draws Lightroom's parametric curve under the
    /// point curve — four regions and three splits, in both languages — and
    /// its ● is the OR of the two curve families: a moved REGION lights it,
    /// a moved split alone does not (a split alone renders nothing,
    /// `catalogue::DOT_EXEMPT`).
    ///
    /// MUTATION THIS CATCHES: the section's dot left at `fam("curves")` (a
    /// parametric curve becomes an invisible adjustment), or a row dropped
    /// from `parametric_curve`.
    #[test]
    fn the_curves_section_draws_the_parametric_curve() {
        for (lang, rows) in [
            (
                crate::i18n::Lang::En,
                ["Parametric curve", "Lights", "Darks", "Shadow split", "Midtone split", "Highlight split"],
            ),
            (crate::i18n::Lang::Zh, ["参数曲线", "亮调", "暗调", "阴影分界", "中间调分界", "高光分界"]),
        ] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            for row in rows {
                assert!(seen.iter().any(|t| t == row), "{lang:?}: Curves has no {row:?} row: {seen:?}");
            }
        }
        let mut app = AutoShadeApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(seen.iter().any(|t| t == "Curves"), "premise: the header is drawn: {seen:?}");
        assert!(!seen.iter().any(|t| t == "Curves  ●"), "premise: a rest recipe lights nothing");
        app.recipe.param_midtone_split = 60.0;
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(!seen.iter().any(|t| t == "Curves  ●"), "a split alone renders nothing: {seen:?}");
        app.recipe.param_darks = 30.0;
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(seen.iter().any(|t| t == "Curves  ●"), "a moved region must light Curves: {seen:?}");
    }

    /// v1.5.0: the Color Mixer section draws Lightroom's WHOLE mixer in both
    /// languages — the Black & White switch, the grey mix in place of the
    /// colour tabs while it is on, and one block per Point Color swatch with
    /// its Range — and the Calibration section draws its seven sliders under
    /// Lightroom's own captions.
    ///
    /// MUTATIONS THIS CATCHES: the grey mix drawn always (or never); a swatch
    /// block that lost its Range, the one slider whose stored number the
    /// render never reads (`PointColor::set_range` rebuilds the windows it
    /// does read, which is why the slider has to exist here); a Calibration
    /// primary dropped.
    #[test]
    fn the_color_mixer_draws_the_bw_mix_the_point_colours_and_calibration() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            let drawn = |seen: &[String], want: &str| seen.iter().any(|t| t == want);
            // Each key spelt AT its `tr` (the i18n audit reads call sites, and
            // a loop variable is a site it cannot read).
            for want in [
                tr(lang, "Black & White"),
                tr(lang, "Point Color"),
                tr(lang, "💧 Pick a color"),
                tr(lang, "Shadows"),
                tr(lang, "Red primary"),
                tr(lang, "Green primary"),
                tr(lang, "Blue primary"),
            ] {
                assert!(drawn(&seen, want), "{lang:?}: no {want:?} row: {seen:?}");
            }
            assert!(
                !drawn(&seen, tr(lang, "B&W mix")),
                "{lang:?}: a colour photo shows the colour bands, not the grey mix: {seen:?}"
            );
            assert!(
                !drawn(&seen, &trf(lang, "Swatch {n}", &[("n", "1")])),
                "{lang:?}: no swatch, no swatch block: {seen:?}"
            );

            // Black & White on, one swatch sampled: the grey mix stands where
            // the tabs were, and the swatch brings its four sliders with it.
            app.recipe.convert_to_grayscale = true;
            app.recipe.point_colors.push(autoshade::recipe::PointColor::sampled(0.9, 0.7, 0.5));
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            for want in [
                tr(lang, "B&W mix").to_string(),
                trf(lang, "Swatch {n}", &[("n", "1")]),
                tr(lang, "Range").to_string(),
            ] {
                assert!(drawn(&seen, &want), "{lang:?}: no {want:?} row: {seen:?}");
            }
        }
        // The Range slider writes through `PointColor::set_range`, which
        // REBUILDS the three windows the engine matches pixels against. Storing
        // the number alone would leave the windows where they were — a slider
        // that moves no pixel, which this repo calls the worst kind of bug —
        // and no frame can drive a drag, so the choice is pinned in the source.
        let develop = include_str!("../panels/develop.rs");
        let rows = &develop[develop.find("fn point_color_rows(").expect("point_color_rows moved")..];
        let rows = &rows[..rows.find("\n    /// ").unwrap_or(rows.len())];
        assert!(
            rows.contains("set_range("),
            "the Range slider no longer rebuilds the swatch's windows"
        );
    }

    /// v1.5.0: the Point Color eyedropper — a canvas tool like the WB one, and
    /// what ONE sample does.
    ///
    /// The three outcomes are the design: a colour becomes a swatch keyed to
    /// that colour and the tool disarms; a near-grey spot adds nothing, says
    /// why and stays armed (the engine gives such a pixel to no swatch, so its
    /// sliders would move nothing); a full list refuses out loud instead of
    /// pushing a swatch `EditRecipe::clamp` would drop on the floor.
    ///
    /// MUTATIONS THIS CATCHES: `point_color_picking` left out of `tool_armed`
    /// (Esc would leave it armed and the canvas would keep taking its clicks)
    /// or out of `disarm_tools`; a grey sample stored as a swatch; the limit
    /// unchecked.
    #[test]
    fn the_point_color_eyedropper_makes_a_swatch_of_the_colour_clicked() {
        use autoshade::recipe::MAX_POINT_COLORS;
        let mut app = AutoShadeApp::default();
        assert!(!app.tool_armed(), "premise: a fresh app has no tool armed");
        app.point_color_picking = true;
        assert!(app.tool_armed(), "an armed eyedropper is an armed canvas tool");
        app.disarm_tools();
        assert!(!app.point_color_picking, "Esc's disarm puts it down with the rest");

        app.point_color_picking = true;
        app.pick_point_color([0.5, 0.5, 0.5]);
        assert!(app.recipe.point_colors.is_empty(), "a grey spot is no swatch: {}", app.status);
        assert!(app.point_color_picking, "…and the tool stays armed for the retry");
        assert!(!app.dirty, "…and nothing was edited");

        app.pick_point_color([0.8, 0.3, 0.2]);
        assert_eq!(app.recipe.point_colors.len(), 1, "{}", app.status);
        let chip = autoshade::render::point_color_rgb(&app.recipe.point_colors[0]);
        for (c, want) in chip.iter().zip([0.8, 0.3, 0.2]) {
            assert!(
                (c - want).abs() < 2e-3,
                "the swatch is keyed to the colour clicked: {chip:?}"
            );
        }
        assert!(!app.point_color_picking, "a landed swatch disarms the tool");
        assert!(app.dirty, "…and asks for a redevelop");

        app.recipe.point_colors = vec![app.recipe.point_colors[0].clone(); MAX_POINT_COLORS];
        app.point_color_picking = true;
        app.pick_point_color([0.2, 0.3, 0.8]);
        assert_eq!(app.recipe.point_colors.len(), MAX_POINT_COLORS, "a full list takes no more");
        assert!(!app.point_color_picking, "…and the refusal disarms");
        assert!(
            app.status.contains(&MAX_POINT_COLORS.to_string()),
            "…naming the limit: {}",
            app.status
        );
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
        let mut app = AutoShadeApp::default();
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
            let mut app = AutoShadeApp { lang, ..Default::default() };
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
            let mut app = AutoShadeApp { lang, ..Default::default() };
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

    /// R25 B3: the Detail section really lays out its eleven controls — every
    /// one rendered since v1.5.0, where R25 rendered two and carried eight —
    /// and the Lens section its manual CA pair, the auto switch and the six
    /// de-fringe rows.
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
            let mut app = AutoShadeApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            for row in rows {
                assert!(
                    seen.iter().any(|t| t == row),
                    "{lang:?}: the {row:?} control is missing from the develop panel: {seen:?}"
                );
            }
        }
        // …and the eight detail axes light the Detail ● (the section holds two
        // families since B3, and its dot is the OR of them).
        let mut app = AutoShadeApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(seen.iter().any(|t| t == "Detail"), "premise: the header is drawn: {seen:?}");
        assert!(!seen.iter().any(|t| t == "Detail  ●"), "premise: a rest recipe lights nothing");
        app.recipe.color_nr = 25.0;
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Detail  ●"),
            "a detail axis must not be an invisible adjustment: {seen:?}"
        );
        // v1.5.0: an EXPLICIT zero on a companion is an adjustment too — its
        // number is the rest recipe's, its render is not.
        let mut app = AutoShadeApp::default();
        app.recipe.set_resolved("sharpen_detail", 0.0);
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Detail  ●"),
            "Detail 0 renders differently from an absent Detail: {seen:?}"
        );
        // The same for the Lens section and the de-fringe half of its family.
        let mut app = AutoShadeApp::default();
        app.recipe.defringe_purple = 3.0;
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Lens  ●"),
            "a de-fringe value must light the Lens dot: {seen:?}"
        );
    }

    /// v1.5.0 F6: the Transform section has SLIDERS now, and the read-out that
    /// used to be the whole section is the tail underneath them.
    ///
    /// R25 B4 drew it read-only and said why: pass-through meant no band, no
    /// clamp, no neutral and no idea what a value did to a pixel, and a slider
    /// needs all four. F6 supplies all four — the eight `crs:Perspective*` keys
    /// are owned controls and `render::perspective` moves the pixels — so the
    /// test's negative half moved WITH the design rather than being dropped: it
    /// now guards the keys that are still carried. Those are the Upright
    /// solver's own bookkeeping, and the proof they are a read-out is still the
    /// SPELLING — every slider in this panel formats through
    /// `fixed_decimals(0|1|2)`, so none can draw `0.422764964` or
    /// `Adobe Standard`.
    #[test]
    fn the_transform_section_has_sliders_and_a_verbatim_tail() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            // The section is ALWAYS there now: it holds controls, not a
            // read-out of something the file may or may not have carried.
            assert!(
                seen.iter().any(|t| t == tr(lang, "Transform")),
                "{lang:?}: the Transform section must draw on every photo: {seen:?}"
            );
            // Every one of the eight controls, by its own label — this is the
            // half that used to be impossible. Each `tr` takes its key as a
            // LITERAL rather than a loop variable, so the i18n audit sees ten
            // ordinary call sites instead of one dynamic key it would have to
            // be taught to trust.
            for want in [
                tr(lang, "Upright"),
                tr(lang, "Off"),
                tr(lang, "Vertical"),
                tr(lang, "Horizontal"),
                tr(lang, "Rotate (°)"),
                tr(lang, "Transform scale"),
                tr(lang, "Aspect"),
                tr(lang, "X offset"),
                tr(lang, "Y offset"),
                tr(lang, "Constrain crop"),
            ] {
                assert!(seen.iter().any(|t| t == want), "{lang:?}: {want:?} missing: {seen:?}");
            }
            // …and no read-out tail at all while the document carried none —
            // neither the carried block's heading nor, since v1.5.0 F7, the
            // profile rows above it, which are their own `is_empty` branch.
            for absent in [
                tr(lang, "Upright solver bookkeeping"),
                tr(lang, "Camera profile"),
                tr(lang, "Creative profile"),
            ] {
                assert!(
                    !seen.iter().any(|t| t == absent),
                    "{lang:?}: a heading over an empty list is a promise about a file that never \
                     had one: {absent:?} in {seen:?}"
                );
            }

            // The tail, on a document that DID carry the solver's bookkeeping.
            // Real spellings from the library's own sidecars.
            //
            // The profile NAME sits beside them rather than among them since
            // v1.5.0 F7: it left `PASSTHROUGH_CRS` for a control of its own, so
            // the row is drawn from `recipe.camera_profile` while the solver's
            // two keys are still the carried block. Both halves are read-outs,
            // and the proof is the same for both — a slider would reformat the
            // spelling, and neither spelling can survive `fixed_decimals`.
            app.recipe.camera_profile = "Adobe Standard".to_string();
            app.recipe.passthrough = [
                ("UprightCenterNormX", "0.422764964"),
                ("UprightFocalLength35mm", "13.992594916"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            for want in [
                tr(lang, "Upright solver bookkeeping"),
                tr(lang, "Camera profile"),
                tr(lang, "Carried through to the sidecar unchanged; AutoShade never interprets these"),
            ] {
                assert!(seen.iter().any(|t| t == want), "{lang:?}: {want:?} missing: {seen:?}");
            }
            // Adobe's own property names for the values we still cannot
            // describe in our own words — one label, one key, no invented
            // friendly name.
            for key in ["crs:UprightCenterNormX", "crs:UprightFocalLength35mm"] {
                assert!(seen.iter().any(|t| t == key), "{lang:?}: {key} row missing: {seen:?}");
            }
            // STILL NO SLIDER on those: the spellings no slider can produce,
            // drawn exactly as Lightroom wrote them.
            for verbatim in ["0.422764964", "13.992594916", "Adobe Standard"] {
                assert!(
                    seen.iter().any(|t| t == verbatim),
                    "{lang:?}: {verbatim:?} is not on screen as itself — either the row became a \
                     slider (which would reformat it) or the value was interpreted: {seen:?}"
                );
            }
            // A key the document never carried is never invented.
            assert!(
                !seen.iter().any(|t| t == "crs:UprightVersion"),
                "{lang:?}: an absent carried key must stay absent: {seen:?}"
            );
        }
        // And the section's ● lights from its own family, like every other.
        let mut app = AutoShadeApp::default();
        app.recipe.perspective_vertical = -35.0;
        let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Transform  ●"),
            "a keystone must light the Transform dot: {seen:?}"
        );
    }

    /// v1.5.0 F7: the creative profile names itself, and then names the half
    /// of itself this engine cannot render.
    ///
    /// The disclosure is the point. A `crs:Look` has two halves — baked
    /// sliders and a baked tone curve, which `render::build_tone_lut` and
    /// `render::apply_rgb_curves` compose as the base rendition, and a
    /// `crs:LookTable` creative colour table, whose payload does not decode
    /// (`src/dcp.rs` records what was measured). A panel that printed
    /// 「Adobe Landscape」 and stopped would be claiming the whole profile
    /// renders, on a photograph where the colour half does not — the exact
    /// shape of 「a slider that moves a number and no pixel」 this file's own
    /// rules call the worst kind of bug here.
    ///
    /// MUTATION: drop the weak line, or draw it when `table` is empty.
    #[test]
    fn a_creative_profile_names_itself_and_the_half_that_does_not_render() {
        use autoshade::recipe::{CreativeLook, CurvePoint};
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            // Adobe Color, the shape 152 of the library's 161 Looks have: one
            // baked S-curve and a colour table we cannot read.
            let mut app = AutoShadeApp { lang, ..Default::default() };
            app.recipe.look = Some(CreativeLook {
                name: "Adobe Color".to_string(),
                amount: 1.0,
                base_profile: "Adobe Standard".to_string(),
                table: "0B3BFB5CFB7DBF7FF175E98F24D316B0".to_string(),
                tone_curve: vec![
                    CurvePoint { input: 0, output: 0 },
                    CurvePoint { input: 22, output: 16 },
                    CurvePoint { input: 255, output: 255 },
                ],
                ..Default::default()
            });
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            for want in [
                tr(lang, "Creative profile"),
                tr(
                    lang,
                    "Its baked tone curve and sliders render; its creative colour table does not",
                ),
            ] {
                assert!(seen.iter().any(|t| t == want), "{lang:?}: {want:?} missing: {seen:?}");
            }
            // The NAME as Adobe spelled it — no friendly rewrite, no slider.
            assert!(
                seen.iter().any(|t| t == "Adobe Color"),
                "{lang:?}: the profile's own name must be on screen: {seen:?}"
            );
            // This photograph has a creative profile and NO camera profile, so
            // the camera row must not appear at all. A labelled row with an
            // empty value beside it reads as "your camera profile is (blank)",
            // which is a claim about the file rather than a description of it —
            // and each of the two rows answers for itself, because either can
            // be present without the other.
            assert!(
                !seen.iter().any(|t| t == tr(lang, "Camera profile")),
                "{lang:?}: a row with nothing in it is not a read-out: {seen:?}"
            );
            // …and the Look's own camera profile is NOT shown as the
            // photographer's: the Description carried none here, and the
            // baked `base_profile` is the profile's business, not a choice
            // to attribute to them.
            assert!(
                !seen.iter().any(|t| t == "Adobe Standard"),
                "{lang:?}: a Look's baked base profile is not the photo's own: {seen:?}"
            );

            // A Look with NO table renders whole, so there is nothing to
            // disclose and the line must not appear — an unconditional
            // disclaimer is noise that teaches the reader to skip it.
            if let Some(look) = app.recipe.look.as_mut() {
                look.table.clear();
            }
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            assert!(
                seen.iter().any(|t| t == tr(lang, "Creative profile")),
                "{lang:?}: the name still shows: {seen:?}"
            );
            assert!(
                !seen.iter().any(|t| t
                    == tr(
                        lang,
                        "Its baked tone curve and sliders render; its creative colour table does not",
                    )),
                "{lang:?}: nothing is missing, so nothing is disclosed: {seen:?}"
            );
        }
    }

    /// v1.5.0 F7: a monochrome creative profile turns the PANEL black and
    /// white, not only the canvas.
    ///
    /// The panel and the engine have to answer 「is this photograph grey?」 the
    /// same way. `render::apply_develop` skips `apply_hsl` and
    /// `apply_point_colors` when `renders_grayscale()`, so a panel that read
    /// only the photographer's own checkbox would show a Color Mixer and a
    /// Point Color picker over a grey canvas — eight hue sliders and a swatch
    /// list that move numbers and no pixels, on the four 「Adobe Monochrome」
    /// photographs in the reference library.
    ///
    /// WHAT THIS DOES NOT COVER: the Point Color block is greyed through
    /// `add_enabled_ui`, and egui draws a disabled widget's text exactly as it
    /// draws an enabled one — so a frame dump cannot see that half. It is the
    /// same one-line question (`renders_grayscale`) at the same kind of site,
    /// changed with this one, and it is stated here rather than left to look
    /// covered.
    ///
    /// MUTATION: read `convert_to_grayscale` at the mixer site.
    #[test]
    fn a_monochrome_profile_turns_the_mixer_black_and_white_too() {
        use autoshade::recipe::CreativeLook;
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            // The photographer's own switch is OFF throughout: this is the
            // profile's doing, which is the whole point.
            let mut app = AutoShadeApp { lang, ..Default::default() };
            app.recipe.look = Some(CreativeLook {
                name: "Adobe Monochrome".to_string(),
                amount: 1.0,
                grayscale: true,
                ..Default::default()
            });
            assert!(!app.recipe.convert_to_grayscale, "{lang:?}: the checkbox stays off");
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            // The B&W mixer's own caption is up, in place of the three
            // Hue/Saturation/Luminance tabs the colour mixer draws.
            assert!(
                seen.iter().any(|t| t == tr(lang, "B&W mix")),
                "{lang:?}: a grey photograph gets the B&W mixer: {seen:?}"
            );
            // The complement, which is what makes the line above discriminating
            // rather than a caption that is always there. The colour mixer's own
            // 「Hue / Saturation / Luminance」 tabs are NOT the probe: Color
            // Grading and Calibration carry those three words too, so they are
            // on screen either way and counting them proves nothing.
            let mut colour = AutoShadeApp { lang, ..Default::default() };
            colour.recipe.look = Some(CreativeLook {
                name: "Adobe Color".to_string(),
                amount: 1.0,
                ..Default::default()
            });
            let colour_seen = tall_frame(&mut colour, |a, ui| a.develop_panel(ui));
            assert!(
                !colour_seen.iter().any(|t| t == tr(lang, "B&W mix")),
                "{lang:?}: a COLOUR profile leaves the colour mixer alone: {colour_seen:?}"
            );
            // …and the photographer's own switch still does it on its own.
            let mut own = AutoShadeApp { lang, ..Default::default() };
            own.recipe.convert_to_grayscale = true;
            let seen = tall_frame(&mut own, |a, ui| a.develop_panel(ui));
            assert!(
                seen.iter().any(|t| t == tr(lang, "B&W mix")),
                "{lang:?}: either switch, not both: {seen:?}"
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
        use autoshade::recipe::EditRecipe;
        use autoshade::xmp::global_render_gaps;
        // **v1.5.0 first**: there is nothing left to carry. A recipe holding an
        // imported de-fringe, an auto-CA switch, colour noise reduction and
        // grain all at once produces NO line, because every one of them renders
        // now (`render/detail.rs`, `render/finish.rs`, `render/lens.rs`) — and
        // this is asserted through the real `global_render_gaps`, which is the
        // only part of this test that can notice a row slipping back.
        let was_carried = EditRecipe {
            grain: 30.0,
            grain_size: 25.0,
            color_nr: 25.0,
            defringe_purple: 3.0,
            defringe_purple_lo: 19.0,
            auto_lateral_ca: true,
            // A pass-through block on the same recipe, which must NOT reach
            // this line either: with nothing interpreted there is no neutral to
            // compare against, so it would name itself on every Lightroom photo
            // (xmp::global_render_gaps states it). Its surface is the read-only
            // Transform / camera-profile section, tested above.
            passthrough: [("PerspectiveVertical".to_string(), "-35".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert!(
            global_render_gaps(&was_carried).is_empty(),
            "Track F left nothing carried: {:?}",
            global_render_gaps(&was_carried)
        );
        // …and the LINE still works, which is the half that has to survive the
        // emptiness. Fed by NAME — `render_gap_line` takes the gap list, not a
        // recipe — so the label mapping, the deduplication and the underscore
        // rule stay under test on the day a carried control comes back.
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            assert!(
                render_gap_line(lang, &global_render_gaps(&was_carried)).is_none(),
                "{lang:?}: nothing carried, no line"
            );
            assert!(render_gap_line(lang, &[]).is_none(), "{lang:?}: nothing in, nothing out");

            // One gap, named by its SECTION — the word the photographer already
            // has on screen, never `defringe_purple`.
            let line = render_gap_line(lang, &["defringe_purple"])
                .unwrap_or_else(|| panic!("{lang:?}: a named gap makes a line"));
            assert!(line.contains(tr(lang, "Lens")), "{lang:?}: {line}");
            assert!(line.contains(tr(lang, "Carried to Lightroom, not rendered here")));

            // Every section once each, deduplicated: seven lens controls are
            // one 「Lens」, and a second family is a second word.
            let line = render_gap_line(
                lang,
                &[
                    "defringe_purple",
                    "defringe_purple_lo",
                    "auto_lateral_ca",
                    "defringe_green",
                    "grain",
                ],
            )
            .expect("two sections");
            assert_eq!(
                line.matches(tr(lang, "Lens")).count(),
                1,
                "{lang:?}: four lens controls are ONE section: {line}"
            );
            assert!(
                line.contains(tr(lang, "Effects")),
                "{lang:?}: a second family is a second word: {line}"
            );
            assert!(
                !line.contains(tr(lang, "Transform")),
                "{lang:?}: a pass-through block is not a member of any gap family: {line}"
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
            let mut app = AutoShadeApp { lang, ..Default::default() };
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
            let mut app = AutoShadeApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            let lens_header = if lang == crate::i18n::Lang::En { "Lens" } else { "镜头 · Lens" };
            assert!(
                seen.iter().any(|t| t == lens_header),
                "{lang:?}: the Lens section was not drawn — this proves nothing: {seen:?}"
            );
            // The explanation is folded to one line since 2026-10-14
            // (`headings::explainer`); the paragraph is its tooltip.
            let at = seen.iter().position(|t| t == lens_header).unwrap();
            assert!(
                seen[at..].iter().any(|t| t == crate::i18n::tr(lang, "ⓘ How this works")),
                "{lang:?}: the lens explainer line itself is missing: {seen:?}"
            );
            let paragraph = crate::i18n::tr(lang,
                "Vignette: positive brightens the corners (compensates falloff), negative darkens; a radial gain in linear light. Distortion: positive fixes barrel (wide-angle bulge), negative fixes pincushion (tele pinch); auto-scales to fill the frame, and masks / brush still position on the corrected image. Preview / export / XMP match. Manual CA, the auto-CA switch and de-fringe all render here too.",
            );
            assert!(paragraph.contains("XMP"), "{lang:?}: not the lens paragraph: {paragraph}");
            assert!(
                !paragraph.contains(promise) && !seen.iter().any(|t| t.contains(promise)),
                "{lang:?}: the retired {promise:?} promise is still in the panel"
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
        let mut app = AutoShadeApp::default();
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

    /// Both camera-on and sidecar-disabled calibration are as-opened states.
    /// A manual correction or a move away from either stamp lights the header.
    #[test]
    fn the_lens_dot_measures_edits_away_from_the_stamp() {
        use autoshade::recipe::{LensProfile, MaskWarpSource};
        type Case = (&'static str, fn(&mut EditRecipe), bool);
        let camera_cases: [Case; 7] = [
            ("as stamped", |_| {}, false),
            ("vignette switched off", |r| r.lens_profile.vignette_on = false, true),
            ("manual vignette", |r| r.lens_vignette = -20.0, true),
            ("profile distortion strength", |r| r.lens_profile_distortion_scale = 80.0, true),
            ("profile vignette strength", |r| r.lens_profile_vignetting_scale = 80.0, true),
            ("enabled without data", |r| r.lens_profile.vignette.clear(), true),
            ("all components switched off by hand", |r| {
                r.lens_profile.vignette_on = false;
                r.lens_profile.distortion_on = false;
                r.lens_profile.ca_on = false;
            }, true),
        ];
        let sidecar_cases: [Case; 3] = [
            ("as opened", |_| {}, false),
            ("vignette switched on", |r| r.lens_profile.vignette_on = true, true),
            ("manual vignette", |r| r.lens_vignette = -20.0, true),
        ];
        let camera = LensProfile {
            vignette: vec![1.1; 16],
            distortion: vec![1.001; 16],
            ca_r: vec![1.002; 16],
            ca_b: vec![0.999; 16],
            vignette_on: true,
            distortion_on: true,
            ca_on: true,
            mask_warp_src: MaskWarpSource::CameraMetadata,
            ..Default::default()
        };
        let sidecar = LensProfile {
            vignette_on: false,
            distortion_on: false,
            ca_on: false,
            mask_warp_src: MaskWarpSource::DisabledInSidecar,
            ..camera.clone()
        };
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let title = tr(lang, "Lens");
            let lit = format!("{title}  ●");
            for (base, profile, cases) in [
                ("camera", &camera, &camera_cases[..]),
                ("sidecar", &sidecar, &sidecar_cases[..]),
            ] {
                for &(case, change, active) in cases {
                    let mut app = AutoShadeApp { lang, ..Default::default() };
                    app.recipe.lens_profile = profile.clone();
                    change(&mut app.recipe);
                    let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
                    let headers: Vec<&str> = seen.iter().map(String::as_str)
                        .filter(|t| *t == title || *t == lit).collect();
                    assert_eq!(
                        headers, vec![if active { lit.as_str() } else { title }],
                        "{lang:?}: {base}, {case} must light the Lens dot only for an edit"
                    );
                }
            }
        }
    }

    /// Export settings are saved delivery preferences, not photo adjustments.
    /// Read the drawn header so a detached predicate cannot satisfy this pin.
    #[test]
    fn the_export_header_carries_no_dot_whatever_the_delivery_settings() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = AutoShadeApp {
                lang,
                exp_format: ExportFormat::Jpeg,
                exp_quality: 100.0,
                exp_long_edge: 2048,
                exp_sharpen: 1.0,
                exp_space: 1,
                exp_dest: ExportDest::Ask,
                save_denoise: true,
                ..Default::default()
            };
            let title = tr(lang, "Export");
            let lit = format!("{title}  ●");
            let seen = tall_frame(&mut app, |a, ui| a.develop_panel(ui));
            let headers: Vec<&str> = seen.iter().map(String::as_str)
                .filter(|t| *t == title || *t == lit).collect();
            assert_eq!(headers, vec![title], "{lang:?}: delivery preferences must leave the Export title plain");
        }
    }

    /// R22 #16h (verification, no behaviour change): the export-side MaskLoss
    /// disclosure rides `toast()`, so repeating the SAME save must not stack
    /// copies — the dedup refreshes the live toast and moves it to the BACK of
    /// the 5-deep ring, which is also what stops a repeat from evicting the
    /// other four. Pinned because the release note describes this behaviour.
    #[test]
    fn an_identical_toast_refreshes_instead_of_stacking() {
        let loss = "the Lightroom sidecar dropped 2 bitmap masks";
        let mut app = AutoShadeApp::default();
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
