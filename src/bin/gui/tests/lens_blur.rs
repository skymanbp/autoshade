// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the AI lens
// blur section (2026-10-13) — the depth landing, the focus pick's arming,
// and the two places a per-photo depth map must not travel.

    /// A first depth estimate lands a blur on the card and arms the focus
    /// pick (disarming any other canvas tool); a re-estimate swaps the map
    /// and keeps the focus and dials the photographer chose; a failure says
    /// so and leaves the card as it was.
    #[test]
    fn a_depth_estimate_lands_a_blur_and_arms_the_focus_pick() {
        let mut app = AutoShadeApp { wb_picking: true, busy: true, ..Default::default() };
        app.on_depth_estimated(Ok(PathBuf::from("D:/store/depth.png")));
        let lb = app.recipe.lens_blur.clone().expect("the first estimate lands a blur");
        assert_eq!(lb.depth_map, "D:/store/depth.png");
        assert!(app.lens_focus_picking, "a first estimate arms the focus pick");
        assert!(!app.wb_picking, "one canvas tool at a time");
        assert!(!app.busy && app.dirty);

        let chosen = autoshade::render::LensBlur { focus: [0.2, 0.8], amount: 70.0, range: 30.0, ..lb };
        app.recipe.lens_blur = Some(chosen.clone());
        app.lens_focus_picking = false;
        app.on_depth_estimated(Ok(PathBuf::from("D:/store/depth-2.png")));
        let again = app.recipe.lens_blur.clone().unwrap();
        assert_eq!(again.depth_map, "D:/store/depth-2.png");
        assert_eq!((again.focus, again.amount, again.range), (chosen.focus, chosen.amount, chosen.range));
        assert!(!app.lens_focus_picking, "a re-estimate does not re-arm the pick");

        let before = app.recipe.clone();
        app.on_depth_estimated(Err(anyhow::anyhow!("no GPU")));
        assert_eq!(app.recipe, before, "a failed estimate changes nothing");
    }

    /// A depth map measures one photo's scene: pasting onto another photo
    /// drops the blur (and says so), and a turn is refused while any card
    /// carries one, as for a fill layer.
    #[test]
    fn a_lens_blur_neither_pastes_across_nor_survives_a_turn() {
        let blur = autoshade::render::LensBlur { depth_map: "depth.png".into(), ..Default::default() };
        let src = EditRecipe { exposure_ev: 0.5, lens_blur: Some(blur.clone()), ..Default::default() };
        let p = crate::export::paste_payload(src, true);
        assert!(p.lens_blur && p.foreign.lens_blur.is_none(), "another photo gets the edit without the blur");
        assert_eq!(p.own.lens_blur.as_ref(), Some(&blur), "the source photo keeps it");

        let mut app = AutoShadeApp {
            src_path: Some(PathBuf::from("D:/library/turn.ARW")),
            base_preview: Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(6, 4))),
            ..Default::default()
        };
        assert!(app.can_rotate(), "premise: the photo is rotatable without a blur");
        app.recipe.lens_blur = Some(blur);
        assert!(!app.can_rotate(), "a depth map estimated in this frame must block the turn");
    }
