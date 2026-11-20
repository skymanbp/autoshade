// One part of the GUI's tests (src/bin/gui/tests.rs includes it): what the
// controls panel's content does while its width is dragged (2026-11-17,
// user: 「那一栏的宽度一调整，里面的各种按钮、控件就开始跟着动」), and the
// one look of the panel's checkboxes, fields and dropdowns.

    /// Every drawn thing of one frame that a person would see move: texts and
    /// rectangles (button plates, fields, cards, rails), cut by their clip.
    /// What is clipped to `skip` — a scrolling box's own content — is left
    /// out when one is given.
    fn probe_items(
        shapes: &[egui::epaint::ClippedShape],
        panel: egui::Rect,
        skip: Option<egui::Rect>,
    ) -> Vec<(String, String, egui::Rect)> {
        fn walk(s: &egui::Shape, out: &mut Vec<(String, String, egui::Rect)>) {
            match s {
                egui::Shape::Text(t) => {
                    let text = t.galley.text().trim().replace(['\n', '\t'], " ");
                    if !text.is_empty() {
                        // The text's laid-out box, not its ink: a name cut
                        // with 「…」 can lose its only descender, and its ink
                        // would then end a pixel higher on the same line.
                        // (`galley.rect`, not its size: a right-aligned
                        // galley lies to the LEFT of its position.)
                        out.push(("text".into(), text, t.galley.rect.translate(t.pos.to_vec2())));
                    }
                }
                egui::Shape::Rect(r) => {
                    let filled = r.fill != egui::Color32::TRANSPARENT;
                    let stroked = r.stroke.width > 0.0 && r.stroke.color != egui::Color32::TRANSPARENT;
                    if filled || stroked {
                        let kind = match (filled, stroked) {
                            (true, true) => "rect-fill-stroke",
                            (true, false) => "rect-fill",
                            _ => "rect-stroke",
                        };
                        out.push((kind.into(), String::new(), r.rect));
                    }
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for c in shapes.iter().filter(|c| c.clip_rect.right() <= panel.right() + 1.0) {
            // A vertical scroll area clips its content top and bottom only —
            // across, the clip stays its parent's — so the box's inside is
            // what is clipped within the box's height.
            if skip.is_some_and(|b| c.clip_rect.top() >= b.top() - 1.0 && c.clip_rect.bottom() <= b.bottom() + 1.0) {
                continue;
            }
            let mut items = Vec::new();
            walk(&c.shape, &mut items);
            out.extend(items.into_iter().filter(|(_, _, r)| r.intersects(c.clip_rect)));
        }
        out
    }

    fn probe_table(items: &[(String, String, egui::Rect)]) -> String {
        let mut s = String::from("kind\tleft\ttop\tright\tbottom\ttext\n");
        for (kind, text, r) in items {
            s.push_str(&format!("{kind}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{text}\n", r.left(), r.top(), r.right(), r.bottom()));
        }
        s
    }

    /// The settled drawing of the controls panel with its stored width set to
    /// `w`, as a drag of its edge would leave it: a fresh context, six frames,
    /// every fold open, on a screen tall enough to show the whole column.
    /// Returns what was drawn outside the analysis box, and the panel's rect.
    fn settled_controls(app: &mut AutoShadeApp, w: f32) -> (Vec<(String, String, egui::Rect)>, egui::Rect) {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let stored = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 100.0));
        ctx.data_mut(|d| d.insert_persisted(egui::Id::new("controls"), egui::containers::panel::PanelState { rect: stored }));
        let mut last = (Vec::new(), egui::Rect::NOTHING);
        for i in 0..6 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 16_000.0))),
                time: Some(i as f64 / 60.0),
                ..Default::default()
            };
            let mut panel = egui::Rect::NOTHING;
            let out = ctx.run(input, |ctx| {
                ctx.memory_mut(|m| m.set_everything_is_visible(true));
                panel = crate::side_panel::controls_side_panel(ctx, |ui| app.controls_column(ui)).response.rect;
            });
            // A scroll area clips its content to its inner rect grown by the
            // style's clip margin; that clip is what marks the box's inside.
            let inside = app.analysis_box_rect.map(|b| b.expand(ctx.style().visuals.clip_rect_margin));
            last = (probe_items(&out.shapes, panel, inside), panel);
        }
        last
    }

    /// How far the rows of the controls column reached past the column they
    /// were given, at the frame `settled_controls` laid last — in points,
    /// negative when the widest row ends short of the edge. The LAYOUT's own
    /// answer (`side_panel::laid_column`): a row cut away by the panel's clip
    /// is drawn nowhere, so no list of drawn things can report it.
    fn column_overreach() -> f32 {
        let (given, covered) = crate::side_panel::laid_column("controls").expect("the controls column was laid");
        covered.right() - given.right()
    }

    /// How far past its own right edge the column's clip is opened by
    /// [`opened_column`].
    const PAST_ROOM: f32 = 600.0;

    /// The controls panel laid `w` px wide exactly as the app lays it, but
    /// with the column's clip opened [`PAST_ROOM`] px to the right, so what
    /// the panel would cut away is drawn: the settled frame's shapes that
    /// belong to the column, and the rect the column was given. (The
    /// show-everything hook also pops every tooltip; those are clipped to the
    /// whole screen and are left out.)
    fn opened_column(app: &mut AutoShadeApp, w: f32) -> (Vec<egui::epaint::ClippedShape>, egui::Rect) {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let stored = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 100.0));
        ctx.data_mut(|d| d.insert_persisted(egui::Id::new("controls"), egui::containers::panel::PanelState { rect: stored }));
        let mut opened = egui::Rect::NOTHING;
        let mut shapes = Vec::new();
        for i in 0..6 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w + 2.0 * PAST_ROOM, 16_000.0))),
                time: Some(i as f64 / 60.0),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                ctx.memory_mut(|m| m.set_everything_is_visible(true));
                crate::side_panel::controls_side_panel(ctx, |ui| {
                    opened = ui.clip_rect();
                    opened.max.x += PAST_ROOM;
                    ui.set_clip_rect(opened);
                    app.controls_column(ui)
                });
            });
            shapes = out.shapes;
        }
        let (given, _) = crate::side_panel::laid_column("controls").expect("the controls column was laid");
        shapes.retain(|c| c.clip_rect.right() <= opened.right() + 0.5 && c.clip_rect.right() > given.right() + 1.0);
        (shapes, given)
    }

    /// Every text and plate of `shapes` as it reaches the screen — cut by its
    /// own clip (a prompt's hint runs on inside its field and is no
    /// overflow) — top to bottom, then left to right.
    fn seen_items(shapes: &[egui::epaint::ClippedShape]) -> Vec<(egui::Rect, String)> {
        fn walk(s: &egui::Shape, clip: egui::Rect, out: &mut Vec<(egui::Rect, String)>) {
            match s {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, out)),
                egui::Shape::Noop => {}
                s => {
                    let r = s.visual_bounding_rect().intersect(clip);
                    if r.is_positive() {
                        let what = match s {
                            egui::Shape::Text(t) => format!("text {:?}", t.galley.text().trim()),
                            egui::Shape::Rect(_) => "a plate".to_owned(),
                            _ => "a drawing".to_owned(),
                        };
                        out.push((r, what));
                    }
                }
            }
        }
        let mut out = Vec::new();
        for c in shapes {
            walk(&c.shape, c.clip_rect, &mut out);
        }
        out.sort_by(|a, b| a.0.top().total_cmp(&b.0.top()).then(a.0.left().total_cmp(&b.0.left())));
        out
    }

    /// What a too-wide row pushed past the column's right edge, top to bottom,
    /// for a failure message that names it ([`opened_column`]). The first
    /// entries are the rows at fault; a row under one of them may only be
    /// following it (a `Ui` a row overran hands every later row the wider
    /// width). The panel's own plate starts left of the column and is not a
    /// row.
    fn past_the_column(app: &mut AutoShadeApp, w: f32) -> Vec<String> {
        let (shapes, given) = opened_column(app, w);
        seen_items(&shapes)
            .into_iter()
            .filter(|(r, _)| r.left() >= given.left() - 0.5 && r.right() > given.right() + 0.5)
            .map(|(r, what)| format!("y {:.0}: {what} reaches x {:.0}, {:.0} px past", r.top(), r.right(), r.right() - given.right()))
            .collect()
    }

    /// Not a check — the list for a person to read when
    /// `every_kind_of_selected_mask_fits_the_column` is red:
    /// `AUTOSHADE_SHOT_DIR=<dir> cargo test … mask_rows_table -- --ignored`
    /// writes, for each kind of mask and of range in both languages, every
    /// text and plate between the 「Local & Pixel」 and 「Versions & Export」
    /// captions with how far its right edge stands from the column's (the
    /// column's clip opened, so nothing is cut away).
    #[test]
    #[ignore = "writes tables for a person to read"]
    fn mask_rows_table() {
        use autoshade::recipe::RangeMask;
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        let ranges: [(&str, Option<RangeMask>); 3] = [
            ("plain", None),
            ("luminance", Some(RangeMask::Luminance { lo_outer: 0.125, lo: 0.25, hi: 0.75, hi_outer: 0.875 })),
            ("colour", Some(RangeMask::Color { r: 0.2, g: 0.4, b: 0.8, amount: 0.5, px: 0.5, py: 0.5 })),
        ];
        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            for (kind, mask) in one_mask_of_each_kind() {
                for (range_name, range) in ranges {
                    let mut app = app_with_every_button(lang);
                    app.recipe.masks = vec![autoshade::recipe::LocalAdjustment { range, ..mask.clone() }];
                    app.sel_mask = Some(0);
                    let (shapes, given) = opened_column(&mut app, crate::theme::CONTROLS_W_MIN);
                    let items = seen_items(&shapes);
                    let top_of = |caption: &str| items.iter().find(|(_, what)| *what == format!("text {caption:?}")).map(|(r, _)| r.top());
                    let from = top_of(tr(lang, "Local & Pixel")).expect("the masks' group caption is drawn");
                    let to = top_of(tr(lang, "Versions & Export")).expect("the next group's caption is drawn");
                    let mut table = format!("# column right edge {:.1}\ntop\tleft\tright\tpast\twhat\n", given.right());
                    for (r, what) in items.iter().filter(|(r, _)| r.top() >= from && r.top() < to) {
                        table.push_str(&format!("{:.0}\t{:.0}\t{:.0}\t{:+.0}\t{what}\n", r.top(), r.left(), r.right(), r.right() - given.right()));
                    }
                    let kind = kind.split(',').next().unwrap();
                    std::fs::write(dir.join(format!("mask-rows-{lang:?}-{kind}-{range_name}.tsv")), table).unwrap();
                }
            }
        }
    }

    /// Not a check — the camera for a selected mask's rows: each kind of
    /// mask, plain and with a luminance range, in both languages, as the
    /// controls panel draws it at its default width; a second, muted mask
    /// with a name no row holds stands under it in the list.
    /// `AUTOSHADE_SHOT_DIR=<dir> cargo test … mask_screenshots -- --ignored`.
    #[test]
    #[ignore = "writes PNGs for a person to look at"]
    fn mask_screenshots() {
        use autoshade::recipe::{LocalAdjustment, RangeMask};
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        let luminance = RangeMask::Luminance { lo_outer: 0.125, lo: 0.25, hi: 0.75, hi_outer: 0.875 };
        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            for (kind, mask) in one_mask_of_each_kind() {
                for (range_name, range) in [("plain", None), ("luminance", Some(luminance))] {
                    let mut app = app_with_every_button(lang);
                    let long = LocalAdjustment {
                        name: "a second mask whose name is longer than any row".into(),
                        range: Some(luminance),
                        exposure_ev: 0.5,
                        enabled: false,
                        ..Default::default()
                    };
                    app.recipe.masks = vec![LocalAdjustment { range, ..mask.clone() }, long];
                    app.sel_mask = Some(0);
                    let img = controls_cut_shot(
                        &mut app,
                        crate::theme::ThemePref::Dark,
                        crate::theme::CONTROLS_W_MIN,
                        tr(lang, "Local & Pixel"),
                        tr(lang, "Versions & Export"),
                    );
                    let kind = kind.split(',').next().unwrap();
                    img.save(dir.join(format!("mask-{lang:?}-{kind}-{range_name}.png"))).unwrap();
                }
            }
        }
    }

    /// One mask of each kind the mask list can hold — the kinds whose
    /// selected rows differ: the 「↻ Redraw」 row (linear, radial), the
    /// feather / angle / flip rows (radial), the raster tools (a mask
    /// with pixels) — each carrying one component of every kind and the
    /// reverse fit's recolour, so every row a selected mask can draw is
    /// drawn.
    fn one_mask_of_each_kind() -> Vec<(&'static str, autoshade::recipe::LocalAdjustment)> {
        use autoshade::recipe::{LocalAdjustment, MaskCombine, MaskComponent, MaskGeometry};
        let ai = |raster| MaskGeometry::AiMask {
            name: "Sky 1".into(),
            subtype: 2,
            ref_x: 0.5,
            ref_y: 0.2,
            blend_mode: 0,
            value: 1.0,
            inverted: false,
            mask_version: 1,
            provenance: Vec::new(),
            gesture: Vec::new(),
            raster,
        };
        let geometries: Vec<(&'static str, MaskGeometry)> = vec![
            ("linear", MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 0.5 }),
            ("shaped", MaskGeometry::Shaped { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 0.5, falloff: vec![0.0, 0.4, 1.0] }),
            (
                "radial",
                MaskGeometry::Radial {
                    top: 0.2, left: 0.2, bottom: 0.8, right: 0.8,
                    feather: 0.5, roundness: 0.0, flipped: false, angle: 15.0,
                    midpoint: 50.0, mask_version: 2,
                },
            ),
            ("bitmap", MaskGeometry::Bitmap { path: "mask-kinds.png".into() }),
            ("brush", MaskGeometry::Brush { name: "Brush 1".into(), blend_mode: 0, value: 1.0, inverted: false, strokes: Vec::new() }),
            ("ai, not yet drawn", ai(None)),
        ];
        let components: Vec<MaskComponent> = geometries
            .iter()
            .zip([MaskCombine::Add, MaskCombine::Subtract, MaskCombine::Intersect].into_iter().cycle())
            .map(|((_, g), mode)| MaskComponent { geometry: g.clone(), mode, inverted: false })
            .collect();
        geometries
            .into_iter()
            .map(|(kind, mask)| {
                let m = LocalAdjustment {
                    mask,
                    components: components.clone(),
                    name: "sky gradient".into(),
                    color_gains: Some([1.3, 1.0, 0.7]),
                    ..Default::default()
                };
                (kind, m)
            })
            .collect()
    }

    /// User report 2026-11-19 (a radial mask selected: a black band beside
    /// the controls panel, every slider under the mask without its number).
    /// The rows a selected mask draws depend on the KIND of mask and of its
    /// range, and the layout tests had laid one kind — a bitmap with no
    /// range. Every kind of mask with every kind of range, one component of
    /// every kind under it, in both languages, from the narrowest panel to
    /// its cap: no row reaches past the column it was given, and no row
    /// stands at another height when the panel is wider.
    ///
    /// MUTATIONS THIS CATCHES (each was this pin's own red on 2026-11-19,
    /// before its fix, at the default width): a control laid beside a slider
    /// in one `horizontal` — the radial's 「Flip」 beside 「Edge feather」; the
    /// slider takes the whole row, the box stands past it, and every row
    /// under it grows by as much (97 px); a list row as wide as its name (a
    /// mask with a luminance range, 41 px in English); 「↻ Redraw」 beside the
    /// name with the name floored at 110 px (19 px in Chinese, 38 in
    /// English); a shape's row laid name-first at its own width (33 px in
    /// English); the luminance range's four bounds beside their name (12 px
    /// in Chinese, 59 in English).
    #[test]
    fn every_kind_of_selected_mask_fits_the_column() {
        use autoshade::recipe::RangeMask;
        use crate::i18n::Lang;
        let ranges: [(&str, Option<RangeMask>, bool); 4] = [
            ("no range", None, false),
            ("a luminance range", Some(RangeMask::Luminance { lo_outer: 0.125, lo: 0.25, hi: 0.75, hi_outer: 0.875 }), false),
            ("a colour range", Some(RangeMask::Color { r: 0.2, g: 0.4, b: 0.8, amount: 0.5, px: 0.5, py: 0.5 }), false),
            ("a colour range being sampled", Some(RangeMask::Color { r: 0.2, g: 0.4, b: 0.8, amount: 0.5, px: 0.5, py: 0.5 }), true),
        ];
        let narrowest = crate::theme::CONTROLS_W_MIN;
        let mut wrong: Vec<String> = Vec::new();
        for lang in [Lang::Zh, Lang::En] {
            for (kind, mask) in one_mask_of_each_kind() {
                for (range_name, range, sampling) in ranges {
                    let make = || {
                        let mut app = app_with_every_button(lang);
                        app.recipe.masks = vec![autoshade::recipe::LocalAdjustment { range, ..mask.clone() }];
                        app.sel_mask = Some(0);
                        app.range_picking = sampling.then_some(0);
                        app
                    };
                    let what = format!("{lang:?}, a {kind} mask with {range_name}");
                    let (base, base_panel) = settled_controls(&mut make(), narrowest);
                    let over = column_overreach();
                    // Premise: the selected mask's own rows were laid — the
                    // last slider of its Color group is among what was drawn.
                    assert!(
                        base.iter().any(|(_, text, _)| text == tr(lang, "Color tint hue")),
                        "{what}: the selected mask's rows were not drawn"
                    );
                    if over > 0.5 {
                        let named: Vec<String> = past_the_column(&mut make(), narrowest).into_iter().take(6).collect();
                        wrong.push(format!(
                            "{what}: a row reaches {over:.1} px past the {narrowest} px panel's column — the first things past its edge:\n    {}",
                            named.join("\n    ")
                        ));
                    }
                    // 2000 px is past the cap: the panel stops at its widest.
                    let (wide, wide_panel) = settled_controls(&mut make(), 2000.0);
                    let over = column_overreach();
                    assert!(wide_panel.width() > base_panel.width() + 30.0, "premise: the panel took the wider width ({:.1})", wide_panel.width());
                    if over > 0.5 {
                        wrong.push(format!("{what}: a row reaches {over:.1} px past the {:.0} px panel's column", wide_panel.width()));
                    }
                    if base.len() != wide.len() {
                        wrong.push(format!(
                            "{what}: the panel draws {} things at {narrowest} px and {} at {:.0} px",
                            base.len(),
                            wide.len(),
                            wide_panel.width()
                        ));
                        continue;
                    }
                    if let Some(((kind, text, at), (_, _, now))) = base
                        .iter()
                        .zip(&wide)
                        .find(|((_, _, at), (_, _, now))| (at.top() - now.top()).abs() > 0.6 || (at.bottom() - now.bottom()).abs() > 0.6)
                    {
                        wrong.push(format!(
                            "{what}: {kind} {text:?} stands at y {:.1}..{:.1} in a {narrowest} px panel and at {:.1}..{:.1} in a {:.0} px one",
                            at.top(),
                            at.bottom(),
                            now.top(),
                            now.bottom(),
                            wide_panel.width()
                        ));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "{} of the selected-mask layouts are wrong:\n{}", wrong.len(), wrong.join("\n"));
    }

    /// An analysis of a few lines: shorter than its box, and long enough to
    /// take a line fewer when the panel is wider.
    fn with_short_analysis(app: &mut AutoShadeApp) {
        app.rationale = concat!(
            "Exposure lifted half a stop for the shaded wall, highlights pulled to hold the sunlit rim, ",
            "and a cooler white balance on the sky band only.",
        )
        .into();
        app.recipe.rationale = app.rationale.clone();
        app.verdict = Some((autoshade::advisor::Decision::Accept, vec!["balanced".into(), "on target".into()]));
    }

    /// User report 2026-11-17: 「那一栏的宽度一调整，里面的各种按钮、控件就开始
    /// 跟着动，很诡异」. Dragging the controls panel's edge may change how WIDE
    /// a row is, never how TALL: every text and every plate the panel draws
    /// stands at the same height at every width from the narrowest to the
    /// cap, in both languages, in all three modes, with no analysis text, a
    /// few lines of it and an overflowing one — and nothing reaches past the
    /// panel's right edge. (The prose inside the analysis box re-wraps to the
    /// box's width; the box itself keeps its height, so nothing outside it
    /// moves, and its inside is not compared.)
    ///
    /// MUTATIONS THIS CATCHES: a row that wraps on the panel's real width (a
    /// `horizontal_wrapped` of own-width items, a note laid with `ui.label`
    /// instead of `steady_note`); anything square sized from
    /// `available_width()` (the grading discs, the curve editor); the
    /// analysis box shrinking to its text again, or its 「View full text…」
    /// verb decided by the drawn frame; a row wider than the narrowest panel.
    #[test]
    fn the_controls_column_keeps_every_row_in_place_at_any_width() {
        use crate::i18n::Lang;
        type Fixture = (&'static str, fn(Lang) -> AutoShadeApp);
        let fixtures: [Fixture; 5] = [
            ("the full workspace, nothing analysed", app_with_every_button),
            ("the full workspace, a few lines of analysis", |lang| {
                let mut app = app_with_every_button(lang);
                with_short_analysis(&mut app);
                app
            }),
            ("the full workspace, an overflowing analysis", |lang| {
                let mut app = app_with_every_button(lang);
                with_long_analysis(&mut app);
                app
            }),
            ("Portraits, people found", |lang| {
                let mut app = portrait_app(lang, true);
                with_short_analysis(&mut app);
                app
            }),
            ("Normal", |lang| app_in(Mode::Normal, lang)),
        ];
        let narrowest = crate::theme::CONTROLS_W_MIN;
        for lang in [Lang::Zh, Lang::En] {
            for (what, make) in fixtures {
                let (base, base_panel) = settled_controls(&mut make(lang), narrowest);
                assert!(
                    (base_panel.width() - narrowest).abs() <= 0.5,
                    "premise: the stored width is the panel's width ({:.1})",
                    base_panel.width()
                );
                // Premise: the panel drew its controls, or every comparison
                // below passes on an empty list.
                assert!(base.len() > 40, "{lang:?}, {what}: only {} things were drawn", base.len());
                // 2000 px is past the cap: the panel stops at its widest.
                for asked in [narrowest, narrowest + 37.0, narrowest + 83.0, 2000.0] {
                    let (items, panel) = settled_controls(&mut make(lang), asked);
                    // The layout's own word, beside the drawn things below:
                    // the panel's rect is the laid one whatever its rows do
                    // (`drag_owned_panel`), and a row past the edge is
                    // clipped — neither can show a row that is too wide.
                    let over = column_overreach();
                    assert!(
                        over <= 0.5,
                        "{lang:?}, {what}, panel {:.0} px: a row reaches {over:.1} px past the column it was given",
                        panel.width()
                    );
                    if asked > narrowest {
                        assert!(panel.width() > narrowest + 30.0, "premise: the panel took the wider width ({:.1})", panel.width());
                    }
                    for (kind, text, r) in &items {
                        assert!(
                            r.right() <= panel.right() + 0.5,
                            "{lang:?}, {what}, panel {:.0} px: {kind} {text:?} reaches x {:.1}, past the panel's edge",
                            panel.width(),
                            r.right()
                        );
                    }
                    assert_eq!(
                        base.len(),
                        items.len(),
                        "{lang:?}, {what}: the panel draws {} things at {narrowest} px and {} at {:.0} px",
                        base.len(),
                        items.len(),
                        panel.width()
                    );
                    for ((kind, text, at), (_, _, now)) in base.iter().zip(&items) {
                        assert!(
                            (at.top() - now.top()).abs() <= 0.6 && (at.bottom() - now.bottom()).abs() <= 0.6,
                            "{lang:?}, {what}: {kind} {text:?} stands at y {:.1}..{:.1} in a {narrowest} px panel and at {:.1}..{:.1} in a {:.0} px one",
                            at.top(),
                            at.bottom(),
                            now.top(),
                            now.bottom(),
                            panel.width()
                        );
                    }
                }
            }
        }
    }

    /// The GUI's own sources, tests left out: the files a look rule is read
    /// against.
    fn gui_sources_outside_tests() -> Vec<(String, String)> {
        fn walk_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).expect("gui source dir listable") {
                let p = e.expect("dir entry").path();
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n != "tests") {
                        walk_rs(&p, out);
                    }
                } else if p.extension().is_some_and(|x| x == "rs") && p.file_name().is_some_and(|n| n != "tests.rs") {
                    out.push(p);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("bin").join("gui");
        let mut files = Vec::new();
        walk_rs(&root, &mut files);
        assert!(files.len() >= 20, "expected the split module tree, found {} files", files.len());
        files
            .into_iter()
            .map(|p| {
                let name = p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                (name, std::fs::read_to_string(&p).expect("gui source readable"))
            })
            .collect()
    }

    /// The lines after each place `needle` stands in a source, as one string
    /// — the builder chain that follows a constructor.
    fn chains_after<'a>(text: &'a str, needle: &str, lines: usize) -> Vec<(usize, String)> {
        let all: Vec<&'a str> = text.lines().collect();
        all.iter()
            .enumerate()
            // A comment that names the constructor is not a call of it.
            .filter(|(_, l)| l.contains(needle) && !l.trim_start().starts_with("//"))
            .map(|(i, _)| (i + 1, all[i..(i + lines).min(all.len())].join("\n")))
            .collect()
    }

    /// One look for each kind of input (2026-11-17, user: 「编辑那一栏的按钮、
    /// 输入框之类的还是很奇怪」), read off the sources so a new control cannot
    /// arrive in the old look:
    ///
    ///  * a checkbox is drawn by `buttons::check` / `check_if` — a square box.
    ///    egui's own checkbox takes the theme's widget rounding, which turns
    ///    its 14 px box into a circle that reads as a radio button;
    ///  * a single-line text field takes `util::field_margin`, so it stands
    ///    one row tall like the buttons and dropdowns beside it;
    ///  * a dropdown in a side panel states its width — the rest of its row
    ///    or a cell of the grid — and is not sized by its current text.
    #[test]
    fn every_input_of_a_kind_has_one_look() {
        let sources = gui_sources_outside_tests();
        let mut fields = 0;
        let mut dropdowns = 0;
        for (name, text) in &sources {
            if name != "buttons.rs" {
                // Each needle is spelt in two halves: `scripts/audit_i18n.py`
                // reads this file as well, and a whole call spelt inside a
                // string here would read to it as a call with a bare label.
                for needle in [concat!(".check", "box("), concat!("Check", "box::new(")] {
                    let hits = chains_after(text, needle, 1);
                    assert!(hits.is_empty(), "{name}:{}: a checkbox drawn outside `buttons::check`", hits[0].0);
                }
            }
            for (line, chain) in chains_after(text, "TextEdit::singleline(", 8) {
                fields += 1;
                assert!(chain.contains(".margin("), "{name}:{line}: a text field without the shared field margin\n{chain}");
            }
            assert!(
                name == "util.rs" || !text.contains("Margin::symmetric(4.0, 2.0)"),
                "{name}: a text field's margin written out by hand — `util::field_margin` is the one margin"
            );
            // The side panels' dropdowns; the dialogs and the Settings window
            // size theirs through their own form rows.
            if name.starts_with("panels/") && !name.starts_with("panels/settings") {
                for (line, chain) in chains_after(text, "ComboBox::from_id_salt(", 12) {
                    dropdowns += 1;
                    assert!(chain.contains(".width("), "{name}:{line}: a dropdown sized by its own text\n{chain}");
                }
            }
        }
        // Premise: the scan found the controls it is about.
        assert!(fields >= 7, "the scan found only {fields} text fields");
        assert!(dropdowns >= 12, "the scan found only {dropdowns} side-panel dropdowns");
        let buttons = &sources.iter().find(|(n, _)| n == "buttons.rs").expect("buttons.rs").1;
        assert!(buttons.contains("state.rounding = RADIUS_SM.into()"), "`check_if` no longer squares the box");
    }

    /// The disabled button keeps its plate and the selected toggle keeps its
    /// corners (2026-11-17). egui fades a disabled widget toward the panel's
    /// colour, which left a disabled verb as bare grey text with no button
    /// around it, and draws a selected button with square corners unless a
    /// rounding is handed to it — so an armed toggle was the one square
    /// plate in a row of rounded ones.
    #[test]
    fn a_disabled_button_has_a_plate_and_a_selected_toggle_has_round_corners() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        let mut rects = Vec::new();
        let mut out = None;
        for i in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0))),
                time: Some(i as f64 / 60.0),
                ..Default::default()
            };
            out = Some(ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let cell = columns(ui, 1);
                    // Two labels the panels really draw: any text would do.
                    let (off, armed) = (tr(crate::i18n::Lang::En, "↺ Clear"), tr(crate::i18n::Lang::En, "Linear"));
                    rects = vec![action_in(ui, cell, false, off).rect, toggle_in(ui, cell, true, armed).rect];
                });
            }));
        }
        let mut plates = Vec::new();
        fn rect_shapes(s: &egui::Shape, out: &mut Vec<egui::epaint::RectShape>) {
            match s {
                egui::Shape::Rect(r) => out.push(*r),
                egui::Shape::Vec(v) => v.iter().for_each(|s| rect_shapes(s, out)),
                _ => {}
            }
        }
        for c in &out.expect("two frames ran").shapes {
            rect_shapes(&c.shape, &mut plates);
        }
        let plate_of = |r: egui::Rect| {
            plates
                .iter()
                .filter(|p| p.fill.a() > 0 && (p.rect.center() - r.center()).length() < 1.0 && (p.rect.width() - r.width()).abs() < 2.0)
                .copied()
                .collect::<Vec<_>>()
        };
        let disabled = plate_of(rects[0]);
        assert!(!disabled.is_empty(), "a disabled button draws no plate");
        // Every fill at the disabled button's place is the resting fill: one
        // faded toward the panel's colour, painted over the plate, is what
        // made the button vanish.
        let resting = ctx.style().visuals.widgets.inactive.weak_bg_fill;
        assert!(resting.a() > 0, "premise: a side-panel button rests on a plate");
        for p in &disabled {
            assert_eq!(p.fill, resting, "a disabled button's plate is not the resting fill");
        }
        let armed = plate_of(rects[1]);
        assert!(!armed.is_empty(), "a selected toggle draws no plate");
        let want: egui::Rounding = ctx.style().visuals.widgets.inactive.rounding;
        assert!(want.nw > 0.0, "premise: the theme rounds its buttons");
        for p in disabled.iter().chain(&armed) {
            assert_eq!(p.rounding, want, "a button plate with corners of its own: {:?}", p.rounding);
        }
    }

    /// Not a check — the camera for the controls panel across the widths a
    /// drag reaches, every fold open, with a table of every drawn text and
    /// rectangle per width:
    /// `AUTOSHADE_SHOT_DIR=<dir> cargo test … controls_width_sweep -- --ignored`.
    #[test]
    #[ignore = "writes PNGs and tables for a person to look at"]
    fn controls_width_sweep() {
        let dir = std::path::PathBuf::from(std::env::var("AUTOSHADE_SHOT_DIR").expect("AUTOSHADE_SHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        // AUTOSHADE_PROBE_WIDTHS="320,450", AUTOSHADE_PROBE_PPP="2",
        // AUTOSHADE_PROBE_THEME="Light" narrow or sharpen the run.
        let widths: Vec<f32> = std::env::var("AUTOSHADE_PROBE_WIDTHS")
            .map(|s| s.split(',').map(|w| w.trim().parse().expect("a width")).collect())
            .unwrap_or_else(|_| (0..=7).map(|k| crate::theme::CONTROLS_W_MIN + 20.0 * k as f32).collect());
        let ppp: f32 = std::env::var("AUTOSHADE_PROBE_PPP").map_or(1.0, |s| s.parse().expect("a scale"));
        let theme = match std::env::var("AUTOSHADE_PROBE_THEME").as_deref() {
            Ok("Light") => crate::theme::ThemePref::Light,
            _ => crate::theme::ThemePref::Dark,
        };
        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            for &w in &widths {
                let mut app = app_with_every_button(lang);
                with_long_analysis(&mut app);
                let (img, out, panel) = controls_shot(&mut app, theme, egui::vec2(w + 40.0, 11_500.0), w, ppp, 6);
                let crop = image::imageops::crop_imm(&img, 0, 0, (panel.right() * ppp) as u32, img.height());
                crop.to_image().save(dir.join(format!("sweep-{lang:?}-{w}.png"))).unwrap();
                let table = format!("# panel {:.1}..{:.1}\n{}", panel.left(), panel.right(), probe_table(&probe_items(&out.shapes, panel, None)));
                std::fs::write(dir.join(format!("sweep-{lang:?}-{w}.tsv")), table).unwrap();
            }
        }
    }
