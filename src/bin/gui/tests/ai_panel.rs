// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the AI panel's folds, the reference libraries, the picker, deep thinking and the style library build.

    /// R23-2 (feedback #6): the AI panel now OWNS the style reference library —
    /// its status, its build entry, and the honesty of the Style slider above
    /// it. All three read the ONE cached status, so this pins the rendered
    /// panel in each state rather than the predicate alone.
    ///
    /// R30 moved that section out of `ai_analysis` into the 「Reference
    /// libraries」 fold and renamed its caption to say WHICH library it is
    /// (「My Lightroom edits library (RAW + .xmp)」, against the finished-photo
    /// look library one rung below). The states, the entry point and the
    /// slider's flag are unchanged — only the name this test looks for is.
    ///
    /// The status is INJECTED: the production panel spawns a worker to read it
    /// (the file reaches 32 MB), and a headless frame must not go to the
    /// developer's own store for an answer.
    #[test]
    fn the_ai_panel_says_which_style_library_it_is_referencing() {
        use autoshade::style::{StyleIndexInfo, StyleIndexState};
        let info =
            |state| Some(StyleIndexInfo { path: "C:/store/style-index.json".into(), state });

        // ── Nothing built: the entry point, the pointer sentence, AND the
        // slider's own warning — the defect was a slider showing 30% with
        // nothing behind it and no way in the app to build one.
        let mut app =
            AutoShadeApp { style_info: info(StyleIndexState::Absent), ..Default::default() };
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "My Lightroom edits library (RAW + .xmp)"),
            "the section must exist at all — otherwise this test proves nothing: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.contains("No library built yet")),
            "an unbuilt library must say so: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t.contains("folder you edit in Lightroom")),
            "…and where to point it (an AutoShade output folder yields nothing): {seen:?}"
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
        let mut app = AutoShadeApp {
            style_info: info(StyleIndexState::Built {
                total: 412,
                version: 3,
                source_dir: Some("D:/photos/edited".into()),
                scenes: vec![("wide/mid/midday/landscape".into(), 12)],
                // The v5 half of the state, which the AI panel prints beside
                // the count ("embeddings 300/412 - looks 7"). This fixture
                // predates those fields and had stopped compiling.
                with_embedding: 300,
                looks: 7,
                looks_dir: Some("D:/photos/finished".into()),
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
        let mut app = AutoShadeApp {
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

    /// R30: the AI panel is FOUR folds, in one order, each saying what it
    /// costs — and the middle one is a three-rung dependency ladder.
    ///
    /// The reported defect was that the panel mixed paid and local controls
    /// with nothing to tell them apart, so the fix is only real if the cost
    /// tag is IN THE HEADER (the one thing a collapsed fold still shows) and
    /// the rungs sit in dependency order: the edits library is usable on its
    /// own, the retrieval engine is what the look library is reached through,
    /// so the engine must sit between them and not after both.
    ///
    /// Both languages, because a tag that only exists in English is not a
    /// disclosure for the user who reported this.
    ///
    /// MUTATION THIS KILLS: drop a cost tag, reorder the sub-areas, or move
    /// the retrieval engine below the look library it feeds.
    #[test]
    fn the_ai_panel_is_four_folds_that_each_say_what_they_cost() {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = AutoShadeApp { lang, ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
            let want = [
                tr(lang, "Analysis · paid API"),
                tr(lang, "Reference libraries · local"),
                tr(lang, "My Lightroom edits library (RAW + .xmp)"),
                tr(lang, "Retrieval engine (what a build computes, what a query matches)"),
                tr(lang, "Finished-photo look library (JPEG)"),
                tr(lang, "Reimagine (whole image) · paid API"),
                tr(lang, "Reverse-fit · local; AI review is paid"),
            ];
            let mut at = 0usize;
            for w in want {
                let i = seen.iter().skip(at).position(|t| t == w).unwrap_or_else(|| {
                    panic!("{lang:?}: {w:?} is missing or out of order in {seen:?}")
                });
                at += i + 1;
            }
        }
    }

    /// R30 gate (a), split the way the user ruled it: only what CONSUMES a
    /// library goes dark at Style 0. Everything that shapes one stays live.
    ///
    /// At Style 0 the pipeline opens no index at all — `pipeline.rs`'s
    /// `(req.style > 0.0).then(load_effective)` — so the two switches that do
    /// nothing but feed an analysis (rung 1's reference-photo switch, rung 3's
    /// 「Use look library」) are decoration at that setting and are drawn
    /// disabled with the reason above them. Three things are NOT: the folder
    /// pickers, the two Build buttons, and the whole retrieval-engine rung,
    /// whose two switches `actions.rs` resolves when it STARTS a build and so
    /// decide what that build computes. Greying those would have made the
    /// library nobody has yet the one library nobody can make — and the Style
    /// slider's own 「⚠ no library」 flag points straight at them.
    ///
    /// The threshold is the PIPELINE'S, not a rounded band, so 1% must re-arm
    /// the read side — that is why phase ③ exists.
    ///
    /// MUTATION THIS KILLS: widen the read gate over the build row (phase ②'s
    /// build witness goes false), wrap the retrieval-engine rung back in it
    /// (the retrieval witness goes false), drop the wrapper from EITHER use
    /// switch (the read witness ORs the two, so one ungated switch flips phase
    /// ② on its own), widen the comparison to `>= 0.0`, or drop the sentence
    /// that says why.
    #[test]
    fn the_reference_libraries_build_at_style_zero_but_do_not_read() {
        let ctx = egui::Context::default();
        crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
        // (build row, retrieval rung, use switches) — one frame answers all
        // three, so a change that moves the boundary between them cannot pass
        // by moving the lot.
        let frame = |app: &mut AutoShadeApp| -> (Option<bool>, Option<bool>, Option<bool>) {
            app.ai_library_build_enabled = None; // this frame's evidence only
            app.retrieval_engine_enabled = None;
            app.ai_library_read_enabled = None;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    // The sub-area ships COLLAPSED; egui's own test hook opens
                    // every collapsible, so a fold state cannot make this
                    // vacuous.
                    ctx.memory_mut(|m| m.set_everything_is_visible(true));
                    egui::SidePanel::left("controls").default_width(320.0).show(ctx, |ui| {
                        app.ai_panel(ui);
                    });
                },
            );
            (
                app.ai_library_build_enabled,
                app.retrieval_engine_enabled,
                app.ai_library_read_enabled,
            )
        };
        // ① the shipped default: Style is above 0, so everything is live.
        let mut app = AutoShadeApp::default();
        assert!(app.style_strength > 0.0, "premise: the app ships with Style above 0");
        assert_eq!(
            frame(&mut app),
            (Some(true), Some(true), Some(true)),
            "above Style 0 a library is buildable, configurable and read"
        );
        // ② Style at 0: the two USE switches go dark, and nothing else does.
        app.style_strength = 0.0;
        let (build, retrieval, read) = frame(&mut app);
        assert_eq!(
            read,
            Some(false),
            "at Style 0 the pipeline loads no index, so neither use switch can change it"
        );
        assert_eq!(
            build,
            Some(true),
            "building a library is how a user gets something for Style to read (user ruling)"
        );
        // The retrieval engine is BUILD-side too, which is why it is named
        // separately: `actions.rs` resolves both of its switches when it starts
        // an index build, so greying them at Style 0 would let a user start a
        // build they were not allowed to configure.
        assert_eq!(
            retrieval,
            Some(true),
            "the SigLIP 2 switch configures the BUILD as well as the query — it must stay usable at Style 0"
        );
        // …and the reason is ON SCREEN, not only in a tooltip.
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t.contains("Style is at 0%")),
            "a greyed control must say why it is grey: {seen:?}"
        );
        // …beside a build entry that is still there to be used.
        assert!(
            seen.iter().any(|t| t.contains("Build / rebuild")),
            "the build entry stays on the panel at Style 0: {seen:?}"
        );
        // ③ the smallest step off 0 re-arms the read side.
        app.style_strength = 0.01;
        assert_eq!(
            frame(&mut app),
            (Some(true), Some(true), Some(true)),
            "any non-zero Style opens a library"
        );
    }

    /// R30 gates (b) and (c): 「Use look library」 was a switch that shipped ON
    /// over a retrieval that could not run.
    ///
    /// The look library is reached ONLY through the SigLIP 2 query vector
    /// (`pipeline.rs` builds it as `req.style > 0.0 && req.embed.on()`, and
    /// `StyleIndex::retrieve_looks_with_terms` returns an EMPTY list when
    /// neither query vector exists), while `use_looks` defaults to true and
    /// `style_embed` to false. So a fresh install carried a ticked switch that
    /// read nothing, and the only disclosure was the rationale's
    /// `looks_unreachable` note — after an analysis had already been billed.
    ///
    /// MUTATION THIS KILLS: drop `retrievable` from the switch's enablement,
    /// delete the warn label, or leave the label up once the embedding is on.
    #[test]
    fn the_look_library_switch_is_dead_and_says_so_while_siglip_is_off() {
        use autoshade::style::{StyleIndexInfo, StyleIndexState};
        let built = |looks| {
            Some(StyleIndexInfo {
                path: "C:/store/style-index.json".into(),
                state: StyleIndexState::Built {
                    total: 40,
                    version: 6,
                    source_dir: Some("D:/photos/edited".into()),
                    scenes: Vec::new(),
                    with_embedding: 40,
                    looks,
                    looks_dir: Some("D:/photos/finished".into()),
                    age: Some(std::time::Duration::from_secs(3600)),
                },
            })
        };
        // The DEFAULTS are the trap, and that is the premise this rests on.
        assert!(AutoShadeApp::default().use_looks, "premise: the look library ships ON");
        assert!(!AutoShadeApp::default().style_embed, "…over an embedding that ships OFF");

        // ── looks exist, SigLIP off: inert, and it says why on the panel.
        let mut app = AutoShadeApp { style_info: built(94), ..Default::default() };
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Use look library"),
            "premise: the switch is on screen at all: {seen:?}"
        );
        assert_eq!(
            app.looks_switch_enabled,
            Some(false),
            "a library that cannot be retrieved must not offer a live switch"
        );
        assert!(
            seen.iter().any(|t| t.contains("ticked, but unreachable")),
            "the default trap must be visible BEFORE an analysis is paid for: {seen:?}"
        );

        // ── SigLIP on: retrievable, so the switch is live and the flag is gone.
        app.style_embed = true;
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert_eq!(
            app.looks_switch_enabled,
            Some(true),
            "with the embedding on, the switch is usable"
        );
        assert!(
            !seen.iter().any(|t| t.contains("ticked, but unreachable")),
            "and the warning must not outlive its cause: {seen:?}"
        );

        // ── no finished photos: still inert, but for the OTHER half — and an
        // UNTICKED switch is not a trap, so no warning either.
        let mut app = AutoShadeApp {
            style_info: built(0),
            use_looks: false,
            style_embed: true,
            ..Default::default()
        };
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert_eq!(
            app.looks_switch_enabled,
            Some(false),
            "no finished photos means nothing to use"
        );
        assert!(
            !seen.iter().any(|t| t.contains("ticked, but unreachable")),
            "an unticked switch is not the default trap: {seen:?}"
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
        let reference = std::path::PathBuf::from("D:/exports/P21-lightroom.tif");
        // A stock app: ONE Original variant, nothing generated.
        let mut app = AutoShadeApp::default();
        assert_eq!(app.fit_target(), None, "premise: nothing to fit against yet");
        app.fit_ref = Some(reference.clone());
        assert_eq!(
            app.fit_target(),
            Some(reference.clone()),
            "an explicitly chosen reference IS the target"
        );
        // …and it OUTRANKS a generated variant: an explicit choice must not
        // be shadowed by whichever card happens to be active.
        let generated = std::path::PathBuf::from("./out/P21.reimagine.png");
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
        let mut app = AutoShadeApp::default();
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
        app.fit_ref = Some(std::path::PathBuf::from("D:/exports/P21-lightroom.tif"));
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t.contains("P21-lightroom.tif")),
            "the chosen reference must be named on the panel: {seen:?}"
        );
    }

    /// Step-6: the generation-side fidelity retry buys a SECOND image per
    /// diverged reimagine, so it is off in BOTH defaults — a fresh install
    /// and an older prefs file must give the same answer — and its switch
    /// renders inside the Reimagine section it governs. Supervisor mutation
    /// ME (the default flipped on) goes red here.
    #[test]
    fn the_reimagine_fidelity_retry_is_off_in_both_defaults() {
        assert!(!AutoShadeApp::default().reimagine_retry, "spending is opt-in");
        assert!(
            !Prefs::default().reimagine_retry,
            "an older prefs file must decode to the same answer"
        );
        let mut app = AutoShadeApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t.contains("auto-retry")),
            "the switch must exist: {seen:?}"
        );
    }

    /// R23-6 D (user decision 2026-08-17 ⑥): the deep reverse-fit is a PAID
    /// opt-in on top of another paid opt-in, so it must be off in both
    /// defaults — a prefs file written before the key existed has to decode
    /// to the same answer a fresh install gives — and it must not be
    /// reachable without the review it iterates.
    #[test]
    fn the_deep_reverse_fit_is_off_by_default_and_gated_on_the_review() {
        assert!(!AutoShadeApp::default().fit_deep, "spending is opt-in");
        assert!(
            !Prefs::default().fit_deep,
            "an older prefs file must decode to the same answer"
        );
        let mut app = AutoShadeApp::default();
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

    /// The sidecar folder (`style-index --xmp-dir`'s GUI half) is remembered
    /// BESIDE the library folder it qualifies, and defaults to "beside each
    /// RAW" in both places.
    ///
    /// MUTATION: drop `style_xmp_dir` from either `Prefs` or the save/restore
    /// pair and this fails — a remembered library folder with a forgotten
    /// sidecar folder rebuilds to an EMPTY index on the next launch, which is
    /// the one failure mode this control exists to remove.
    #[test]
    fn the_sidecar_folder_is_persisted_beside_the_library_folder() {
        assert_eq!(
            AutoShadeApp::default().style_xmp_dir,
            None,
            "the default is the beside-the-RAW convention every build has used"
        );
        assert_eq!(
            Prefs::default().style_xmp_dir,
            None,
            "and a prefs file written before this key existed decodes to the same answer"
        );
        let prefs = Prefs {
            style_src_dir: Some(std::path::PathBuf::from("D:/library")),
            style_xmp_dir: Some(std::path::PathBuf::from("D:/sidecars")),
            ..Prefs::default()
        };
        let json = serde_json::to_string(&prefs).expect("prefs serialize");
        let decoded: Prefs = serde_json::from_str(&json).expect("prefs deserialize");
        assert_eq!(decoded.style_xmp_dir, prefs.style_xmp_dir);
        assert_eq!(decoded.style_src_dir, prefs.style_src_dir);
        // An older file that predates the key still loads, and answers None.
        let older = json.replace(r#""style_xmp_dir":"D:/sidecars","#, "");
        assert!(!older.contains("style_xmp_dir"), "the key really was removed: {older}");
        let decoded: Prefs = serde_json::from_str(&older).expect("an older prefs file loads");
        assert_eq!(decoded.style_xmp_dir, None);
        assert_eq!(decoded.style_src_dir, prefs.style_src_dir, "and loses nothing else");
    }

    /// R23-2: the reference-PHOTO switch is off by default in BOTH defaults
    /// (the app's and the prefs'), so neither a fresh install nor an upgraded
    /// prefs file silently starts putting a second image on every paid call.
    /// The wire-level proof that the switch is what adds the image lives in
    /// `advisor::openai`'s stub-endpoint test.
    #[test]
    fn the_style_reference_photo_switch_is_off_in_both_defaults() {
        assert!(
            !AutoShadeApp::default().send_style_ref_image,
            "spending is opt-in — a fresh app must not send a second image"
        );
        assert!(
            !Prefs::default().send_style_ref_image,
            "a prefs file written before this key existed must decode to the SAME answer"
        );
        assert_eq!(Prefs::default().style_src_dir, None);
        // …and the request the analyze worker builds carries the flag, so the
        // checkbox cannot become decoration.
        let app = AutoShadeApp { send_style_ref_image: true, ..Default::default() };
        let req = autoshade::pipeline::GradeRequest {
            style: app.style_strength,
            send_reference_image: app.send_style_ref_image,
            strength: autoshade::recipe::GradeStrength::new(app.grade_strength),
            think: app.deep_think,
            adherence: autoshade::recipe::DirectionAdherence::default(),
            use_looks: true,
            embed: autoshade::style::EmbeddingSwitch::resolve(None, app.style_embed),
            weights: autoshade::style::RetrievalWeights::from_env(),
        };
        assert!(req.send_reference_image);
        assert!(
            !autoshade::pipeline::GradeRequest::with_style(0.65).send_reference_image,
            "every non-GUI surface stays on the text reference"
        );
    }

    /// W2-2: the panel's 「style embedding」 checkbox reaches the DEVELOP, not
    /// only the index build.
    ///
    /// `produce_recipe` used to resolve the switch itself, with the preference
    /// hard-coded to `false` (`style::embedding_effective(false)`), so the
    /// checkbox governed `Build style library` and nothing else: from the
    /// desktop app the look library and every embedding term were dead unless
    /// the user happened to set AUTOSHADE_STYLE_EMBED in their environment. The
    /// switch is now a value on the request, read on the UI thread with the
    /// rest of it.
    ///
    /// MUTATION: build the request with `EmbeddingSwitch::OFF`, or make
    /// `produce_recipe` resolve the switch again instead of reading
    /// `req.embed`, and this fails.
    #[test]
    fn gui_embedding_pref_reaches_the_develop_query() {
        // The environment is NOT set here, and is not touched: the preference
        // alone must be enough.
        let on = AutoShadeApp { style_embed: true, ..Default::default() };
        let off = AutoShadeApp { style_embed: false, ..Default::default() };
        let request_from = |app: &AutoShadeApp| autoshade::pipeline::GradeRequest {
            style: app.style_strength,
            send_reference_image: app.send_style_ref_image,
            strength: autoshade::recipe::GradeStrength::new(app.grade_strength),
            think: app.deep_think,
            adherence: autoshade::recipe::DirectionAdherence::default(),
            use_looks: app.use_looks,
            embed: autoshade::style::EmbeddingSwitch::resolve_with(
                None, app.style_embed, |_| None,
            ),
            weights: autoshade::style::RetrievalWeights::SHIPPED,
        };
        assert!(request_from(&on).embed.on(), "the checkbox must reach the develop's request");
        assert!(!request_from(&off).embed.on(), "…and un-checking it must too");

        // …and the pipeline READS the request instead of resolving its own.
        // A behavioural assertion is out of reach here (the develop path is a
        // paid network chain), so the invariant is that the old door is gone
        // and the new one is the only read.
        let pipeline = include_str!("../../../pipeline.rs");
        assert!(
            !pipeline.contains("embedding_effective"),
            "the pipeline must not resolve the switch itself"
        );
        assert_eq!(
            pipeline.matches("req.embed.on()").count(),
            1,
            "exactly one read of the request's switch"
        );
        // The two GUI workers build their switch from the SAME preference, so
        // an index built with it and a develop querying it cannot disagree.
        let actions = include_str!("../actions.rs");
        assert_eq!(
            actions.matches("EmbeddingSwitch::resolve(None, self.style_embed)").count(),
            3,
            "the RAW build, the look build and the develop request all read the preference"
        );
    }

    /// User feedback 2026-09-11: the deep-thinking working listed INLINE in
    /// the rationale sentence ran the Analysis fold to a wall of text. The
    /// four notes only a thinking analysis produces (R23-4's three sentences
    /// and R23-1b's pixel-tool line) now draw in a framed box of their own
    /// under the sentence, one row each with its subject as the lead, while
    /// the sentence keeps its deterministic tail — nothing the rationale said
    /// is lost, only moved. Both languages, since the leads are ours.
    ///
    /// The split is a DISPLAY decision over the typed notes. A develop whose
    /// string no longer matches its notes (the disk-restored case) shows the
    /// raw English whole, exactly as before — and the box must not print the
    /// working a second time.
    ///
    /// MUTATION THIS KILLS: leaving the working inside the sentence, dropping
    /// a row, or boxing the working on the fallback path too.
    #[test]
    fn the_deep_thinking_working_draws_in_its_own_box_under_the_rationale() {
        use autoshade::rationale::{keys, render_one, Note};
        let notes = vec![
            Note::new(keys::JUDGE_SCORE, vec![("score", "88".into()), ("critique", "balanced".into())]),
            Note::new(keys::THINK_SCENE, vec![("scene", "a harbour at dusk".into())]),
            Note::new(keys::THINK_LOOK, vec![("look", "cool and quiet".into())]),
            Note::new(keys::THINK_CRITIQUE, vec![("critique", "a touch under the target".into())]),
            Note::new(keys::PIXEL_TOOLS, vec![("tools", "denoise (grain in the sky)".into())]),
        ];
        let rationale: String =
            std::iter::once("Prose.".to_string()).chain(notes.iter().map(render_one)).collect();
        let working =
            ["a harbour at dusk", "cool and quiet", "a touch under the target", "denoise (grain in the sky)"];
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let mut app = AutoShadeApp {
                lang,
                rationale: rationale.clone(),
                rationale_notes: notes.clone(),
                ..Default::default()
            };
            app.recipe.rationale = rationale.clone();
            let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
            let sentence = seen
                .iter()
                .find(|t| t.starts_with("“Prose."))
                .unwrap_or_else(|| panic!("{lang:?}: the rationale sentence is missing: {seen:?}"));
            let judge = trf(lang, autoshade::rationale::keys::JUDGE_SCORE, &[("score", "88"), ("critique", "balanced")]);
            assert!(
                sentence.contains(&judge),
                "{lang:?}: the deterministic tail stays in the sentence: {sentence:?}"
            );
            for w in working {
                assert!(!sentence.contains(w), "{lang:?}: the working left the sentence: {sentence:?}");
                assert_eq!(
                    seen.iter().filter(|t| t.contains(w)).count(),
                    1,
                    "{lang:?}: {w:?} is drawn exactly once, in the box: {seen:?}"
                );
            }
            for lead in [
                tr(lang, "Deep thinking · its working"),
                tr(lang, "What it saw:"),
                tr(lang, "The look it aimed for:"),
                tr(lang, "Its own critique against your strength target:"),
                tr(lang, "Pixel tools it suggests (nothing was run):"),
            ] {
                assert!(seen.iter().any(|t| t == lead), "{lang:?}: {lead:?} is missing: {seen:?}");
            }
        }
        // Fallback: no typed notes → the raw English, whole, and no box.
        let mut app = AutoShadeApp { rationale: rationale.clone(), ..Default::default() };
        app.recipe.rationale = rationale.clone();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        let title = tr(app.lang, "Deep thinking · its working");
        assert!(
            seen.iter().any(|t| t.starts_with("“Prose.") && working.iter().all(|w| t.contains(w))),
            "without notes the working stays inside the raw sentence: {seen:?}"
        );
        assert!(
            !seen.iter().any(|t| t == title),
            "and no box is drawn for a sentence that still carries it: {seen:?}"
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
            !AutoShadeApp::default().deep_think,
            "a fresh app must not start paying for a deeper analyze"
        );
        assert!(
            !Prefs::default().deep_think,
            "a prefs file written before this key existed must decode to the SAME answer"
        );
        let mut app = AutoShadeApp::default();
        let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
        assert!(
            seen.iter().any(|t| t == "Deep thinking"),
            "the switch was not drawn in a 320 px panel: {seen:?}"
        );
        // Same rule as `fit_ai_judge` / `send_style_ref_image`: a remembered
        // preference is not "this photo's AI inputs carry state".
        app.deep_think = true;
        assert!(!app.ai_section_active(), "a preference must not light the section ●");

        let app = AutoShadeApp { deep_think: true, ..Default::default() };
        let req = autoshade::pipeline::GradeRequest {
            style: app.style_strength,
            send_reference_image: app.send_style_ref_image,
            strength: autoshade::recipe::GradeStrength::new(app.grade_strength),
            think: app.deep_think,
            adherence: autoshade::recipe::DirectionAdherence::default(),
            use_looks: true,
            embed: autoshade::style::EmbeddingSwitch::resolve(None, app.style_embed),
            weights: autoshade::style::RetrievalWeights::from_env(),
        };
        assert!(req.think, "the checkbox must reach the worker's request");
        assert!(
            !autoshade::pipeline::GradeRequest::with_style(0.65).think,
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
        let mut app = AutoShadeApp {
            style_build_inflight: true,
            style_build_progress: Some((autoshade::style::BuildStage::Frames, 7, 9)),
            ..Default::default()
        };
        app.tx
            .send(Msg::StyleBuilt(Box::new(StyleBuildOutcome::Saved {
                total: 412,
                dir: dir.clone(),
                without_embedding: 0,
                described: 0,
            })))
            .unwrap();
        app.poll_workers(&ctx);
        assert!(!app.style_build_inflight, "the button re-arms");
        assert_eq!(app.style_build_progress, None, "the counter belongs to ONE build");
        assert!(app.status.contains("412"), "{}", app.status);
        assert!(
            !app.status.contains("without a style embedding"),
            "a build with nothing to disclose must not grow a clause: {}",
            app.status
        );
        assert_eq!(
            app.style_src_dir.as_deref(),
            Some(dir.as_path()),
            "the folder is remembered"
        );
        // The landing ARMS a fresh status read (start_style_info), but that
        // read runs on a real worker thread and `poll_workers` drains up to 64
        // messages per call — on a fast scheduler the StyleInfo answer lands
        // inside this same drain and legitimately clears the flag again (first
        // observed losing that race on CI ubuntu). The durable claim is "the
        // build asked for a fresh read": still in flight OR already answered.
        // A fresh app starts with `style_info = None`, so dropping the
        // `start_style_info` call from the landing fails both halves.
        assert!(
            app.style_info_loading || app.style_info.is_some(),
            "a build invalidates the cached status"
        );
        assert!(app.toasts.iter().any(|t| matches!(t.kind, ToastKind::Success)));

        // R28 Batch-4 4b: a build where the embedding sidecar failed must NOT
        // land the same sentence as one where it worked. The release GUI is
        // `windows_subsystem = "windows"` and has no console, so the per-photo
        // stderr line the CLI shows is invisible there — this toast is the
        // user's only account of it (adjudication F3).
        //
        // MUTATION THIS KILLS: revert `on_style_built` to the single
        // unconditional sentence, or drop `without_embedding` from the outcome
        // — the status then reads exactly like the all-embedded case above and
        // this assertion fails. It is the same status in BOTH languages, so it
        // is asserted in both.
        for (lang, needle) in [(Lang::En, "without a style embedding"), (Lang::Zh, "没有嵌入向量")] {
            let mut app = AutoShadeApp {
                style_build_inflight: true,
                lang,
                ..Default::default()
            };
            app.tx
                .send(Msg::StyleBuilt(Box::new(StyleBuildOutcome::Saved {
                    total: 412,
                    dir: dir.clone(),
                    without_embedding: 400,
                    described: 0,
                })))
                .unwrap();
            app.poll_workers(&ctx);
            assert!(
                app.status.contains("400") && app.status.contains(needle),
                "a degraded build must say how many photos lost their vector ({lang:?}): {}",
                app.status
            );
            // …and it is still a SUCCESS: the index published, the folder is
            // remembered, the panel re-reads. A degradation is not a failure.
            assert!(app.toasts.iter().any(|t| matches!(t.kind, ToastKind::Success)));
            assert_eq!(app.style_src_dir.as_deref(), Some(dir.as_path()));
        }

        // Nothing indexed: the SHARED refusal wording (the CLI and the web say
        // the same), an ERROR toast, and NOTHING remembered — the folder was
        // the wrong kind of folder.
        let mut app = AutoShadeApp { style_build_inflight: true, ..Default::default() };
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
        let mut app = AutoShadeApp { style_build_inflight: true, ..Default::default() };
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
        let mut app = AutoShadeApp { style_build_inflight: true, ..Default::default() };
        app.tx
            .send(Msg::StyleBuildProgress {
                stage: autoshade::style::BuildStage::Frames,
                done: 40,
                total: 300,
            })
            .unwrap();
        app.poll_workers(&ctx);
        assert_eq!(
            app.style_build_progress,
            Some((autoshade::style::BuildStage::Frames, 40, 300))
        );
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
                AutoShadeApp::render_fit_note(
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

    /// R27 L-25. `explorer.exe` re-parses its own command line and splits it on
    /// COMMAS — that is why its documented switches are spelled
    /// `/select,<path>`. Rust's `Command::arg` quotes for spaces and quotes
    /// only, so a develop directory carrying a photo stem like `a,b` arrived as
    /// two arguments and opened the wrong window. The path now goes onto the
    /// line in double quotes, verbatim, through `CommandExt::raw_arg`.
    ///
    /// The refusal is the safety half: a raw command line is only safe because
    /// nothing inside the quotes can terminate them, so the builder declines
    /// rather than assume it.
    ///
    /// MUTATION THIS CATCHES: return the bare path instead of the quoted one
    /// and the comma row loses its quotes — the pre-R27 behaviour exactly;
    /// drop the `contains('"')` guard and the last row hands back a fragment
    /// with an unbalanced quote inside it.
    #[test]
    fn the_explorer_argument_survives_a_comma() {
        use std::path::PathBuf;
        let q = |s: &str| crate::util::explorer_quoted_arg(&PathBuf::from(s));
        assert_eq!(
            q(r"C:\Users\me\autoshade\a,b.arw"),
            Some(r#""C:\Users\me\autoshade\a,b.arw""#.to_string()),
            "the comma has to sit INSIDE the quotes"
        );
        // Ordinary paths take the same route — one rule, not a comma special
        // case, or the untested branch would be the common one.
        assert_eq!(q(r"C:\photos\2026"), Some(r#""C:\photos\2026""#.to_string()));
        // Spaces were already fine and stay fine.
        assert_eq!(q(r"C:\my photos"), Some(r#""C:\my photos""#.to_string()));
        // A quote cannot occur in a Windows path; if one ever does, no raw
        // line is built at all and the caller falls back to `Command::arg`.
        assert_eq!(q("C:\\odd\"q"), None);
    }

    /// User report 2026-10-13: 「思考过程输出有时候太长了会占满屏幕」. The verdict,
    /// the rationale and the working share ONE box bounded to
    /// `ANALYSIS_BOX_MAX_H`; past it a 「View full text…」 verb appears, and the
    /// window it opens draws the whole text. A short analysis gets no verb.
    ///
    /// MUTATION THIS CATCHES: the box's `max_height` dropped (the box grows
    /// with the text), or the window drawing a truncated copy.
    #[test]
    fn a_long_analysis_scrolls_in_its_box_and_opens_whole_in_a_window() {
        let long = format!("{}END-OF-RATIONALE", "The sky is lifted and the shadows are opened a little. ".repeat(60));
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            let verb = tr(lang, "View full text…");
            let mut app = AutoShadeApp { lang, rationale: long.clone(), ..Default::default() };
            let seen = tall_frame(&mut app, |a, ui| a.ai_panel(ui));
            let rect = app.analysis_box_rect.expect("the analysis box was drawn");
            assert!(
                rect.height() <= crate::panels::ai::ANALYSIS_BOX_MAX_H + 0.5,
                "{lang:?}: the analysis box grew to {:.0} px with its text",
                rect.height()
            );
            assert!(seen.iter().any(|t| t == verb), "{lang:?}: an overflowing analysis offers {verb:?}: {seen:?}");

            app.show_analysis_full = true;
            let ctx = egui::Context::default();
            crate::theme::install_theme(&ctx, crate::theme::ThemePref::Dark);
            // A window spends its first frame measuring itself (egui's sizing
            // pass draws nothing), so the text is read off the third.
            let mut window = Vec::new();
            for i in 0..3 {
                let out = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 20_000.0))),
                        time: Some(f64::from(i) / 60.0),
                        ..Default::default()
                    },
                    |ctx| app.analysis_full_window(ctx),
                );
                window = drawn_texts(&out.shapes);
            }
            assert!(
                window.iter().any(|t| t.contains("END-OF-RATIONALE")),
                "{lang:?}: the window must draw the whole rationale"
            );
            assert!(app.show_analysis_full, "{lang:?}: the window stays open while there is text");

            let mut short = AutoShadeApp { lang, rationale: "Balanced.".into(), ..Default::default() };
            let seen = tall_frame(&mut short, |a, ui| a.ai_panel(ui));
            assert!(!seen.iter().any(|t| t == verb), "{lang:?}: a short analysis offers no {verb:?}");
        }
    }
