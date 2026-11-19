// One part of the GUI's tests (src/bin/gui/tests.rs includes it): the .xmp question — an .xmp found with a photo is asked about once per version of the file and never adopted.

    /// A never-edited photo in a temp folder with `photo.xmp` beside it,
    /// written NEWER than anything the store holds (set, not slept for). The
    /// RAW is a stand-in: the store keys by path and the synthetic open never
    /// decodes it.
    fn xmp_beside_fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf, Scrub) {
        let dir = std::env::temp_dir().join(format!("autoshade-gui-xmpq-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("photo.ARW");
        std::fs::write(&src, b"raw").unwrap();
        let dev = autoshade::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        let scrub = Scrub(vec![dir.clone(), dev.clone()]);
        let xmp = dir.join("photo.xmp");
        write_newer_xmp(&xmp, 33.0, 3600);
        (src, xmp, dev, scrub)
    }

    /// Another program's .xmp holding `contrast`, its time `ahead` seconds
    /// past now — later than every store file a test writes before it.
    fn write_newer_xmp(path: &std::path::Path, contrast: f32, ahead: u64) {
        std::fs::write(path, autoshade::xmp::recipe_to_xmp(&EditRecipe { contrast, ..Default::default() }))
            .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(ahead))
            .unwrap();
    }

    /// One frame of the question with `keys` pressed.
    fn xmp_prompt_frame(app: &mut AutoShadeApp, ctx: &egui::Context, keys: &[egui::Key]) {
        let input = egui::RawInput {
            events: keys
                .iter()
                .map(|&key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .collect(),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.xmp_prompt_layer(ctx));
    }

    /// The develop folder's files, minus the lock the store's writes take.
    fn develop_files(dev: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dev)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.retain(|n| n != ".develop.lock");
        names.sort();
        names
    }

    /// Complaint 1 (the open half): a never-edited photo with an .xmp beside
    /// it opens NEUTRAL — the open restores only what this app saved — wears
    /// no ● badge, and the question is raised instead. Nothing is written by
    /// opening.
    ///
    /// MUTATION: the gallery badge back on `has_develop_or_sidecar`.
    #[test]
    fn opening_never_adopts_the_xmp_beside_the_photo() {
        let (src, xmp, dev, _scrub) = xmp_beside_fixture("open");
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        let ctx = egui::Context::default();
        land_cold_open(&mut app, &ctx, &src);
        assert!(app.recipe.is_noop(), "the .xmp's contrast is not on the canvas: {}", app.recipe.contrast);
        assert!(!app.unsaved_marker_dirty(), "a clean open: {}", app.status);
        let prompt = app.xmp_prompt.as_ref().expect("the question is raised");
        assert_eq!(prompt.beside.as_deref(), Some(xmp.as_path()));
        assert!(!autoshade::store::has_develop(&src), "nothing saved, so no ● badge");
        assert!(develop_files(&dev).is_empty(), "opening wrote nothing: {:?}", develop_files(&dev));
        let gallery = include_str!("../panels/gallery.rs");
        assert!(
            gallery.contains("autoshade::store::has_develop(path)") && !gallery.contains("has_develop_or_sidecar"),
            "the ● badge means what this app saved, nothing else"
        );
    }

    /// Complaint 2: asked exactly once per version of the file. "Don't read"
    /// is recorded at once, so reopening does not ask; another program
    /// rewriting the file (new content, newer time) asks again. The Normal
    /// workspace never asks and records nothing.
    ///
    /// MUTATION: the `record_xmp_answer` call dropped from the "Don't read"
    /// branch (reopening asks again); the Normal-mode return removed.
    #[test]
    fn the_xmp_question_is_asked_once_per_version_of_the_file() {
        let (src, xmp, dev, _scrub) = xmp_beside_fixture("once");
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut app, &ctx, &src);
        assert!(app.xmp_prompt.is_some(), "asked on the first open");
        xmp_prompt_frame(&mut app, &ctx, &[egui::Key::Escape]);
        assert!(app.xmp_prompt.is_none(), "Esc answers \"Don't read\"");
        assert_eq!(app.status, "photo.xmp was not read");
        assert!(app.recipe.is_noop(), "nothing read in");
        assert_eq!(develop_files(&dev), ["external-xmp.answered"], "only the answer is recorded");
        assert!(!autoshade::store::has_develop(&src), "an answer is not a develop");

        let mut again = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut again, &ctx, &src);
        assert!(again.xmp_prompt.is_none(), "the same file is not asked about twice");

        write_newer_xmp(&xmp, 12.0, 7200);
        let mut changed = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut changed, &ctx, &src);
        assert!(changed.xmp_prompt.is_some(), "a rewritten file is news again");

        std::fs::remove_dir_all(&dev).unwrap();
        let mut normal = AutoShadeApp { src_path: Some(src.clone()), mode: Mode::Normal, ..Default::default() };
        land_cold_open(&mut normal, &ctx, &src);
        assert!(normal.xmp_prompt.is_none(), "Normal never asks");
        assert!(normal.recipe.is_noop(), "…and never adopts");
        assert!(develop_files(&dev).is_empty(), "…and records nothing: {:?}", develop_files(&dev));
    }

    /// Complaint 3 (the read half): "Read them in" puts the adjustments on
    /// the canvas as ONE undo step with ● lit, and writes nothing but the
    /// answer — the develop is saved only by Ctrl+S.
    ///
    /// MUTATION: the landing's `self.recipe = imp.recipe` removed.
    #[test]
    fn reading_the_xmp_in_lands_one_undoable_unsaved_step() {
        let (src, _xmp, dev, _scrub) = xmp_beside_fixture("read");
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut app, &ctx, &src);
        let before = app.recipe.clone();
        let depth = app.undo_stack.len();
        xmp_prompt_frame(&mut app, &ctx, &[egui::Key::Enter]);
        assert!(app.xmp_prompt.is_none(), "Enter answers \"Read them in\"");
        assert_eq!(app.recipe.contrast, 33.0, "the adjustments are on the canvas");
        assert!(app.unsaved_marker_dirty(), "● lit: {}", app.status);
        assert_eq!(
            app.status,
            "Adjustments read in from photo.xmp — not saved yet (Ctrl+S saves, Ctrl+Z undoes)"
        );
        assert_eq!(develop_files(&dev), ["external-xmp.answered"], "nothing saved but the answer");
        assert!(!autoshade::store::has_develop(&src));
        // The next frame commits the landing, as for every wholesale change.
        app.commit_if_settled(&ctx);
        assert_eq!(app.undo_stack.len(), depth + 1, "one step");
        app.undo(&ctx);
        assert_eq!(app.recipe, before, "one Ctrl+Z returns to the canvas the user had");
    }

    /// An .xmp describes the photo itself: opened onto an AI-generated card,
    /// the import switches to the ▣ Original card first and says so; the ✨
    /// card is left as it was.
    ///
    /// MUTATION: `read_xmp_in` landing on the active card whatever its kind.
    #[test]
    fn an_import_onto_ai_pixels_lands_on_the_original_card() {
        let (_first, ctx, _dir, src, master, ai_px, _scrub) = ai_card_fixture("xmpq", EditRecipe::default());
        // The .xmp arrives after the store's files; a second open sees it.
        write_newer_xmp(&src.with_extension("xmp"), 33.0, 3600);
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        open_onto_ai_card(&mut app, &ctx, &ai_px, &master);
        let generated = app.active;
        assert_eq!(app.variants[generated].kind, VariantKind::Generated, "the premise: on the ✨ card");
        assert!(app.xmp_prompt.is_some(), "asked");
        xmp_prompt_frame(&mut app, &ctx, &[egui::Key::Enter]);
        let original = app.original_index().expect("the strip has its ▣ card");
        assert_eq!(app.active, original, "the import moved to the ▣ Original card");
        assert_eq!(app.recipe.contrast, 33.0, "…and landed there");
        assert_eq!(app.variants[generated].recipe.contrast, 0.0, "the ✨ card is untouched");
        assert!(
            app.toasts.iter().any(|t| t.text.starts_with("Switched to the ▣ Original variant first")),
            "the move is said"
        );
    }

    /// While the question is up it owns the keyboard (D13): a tool key does
    /// not reach the canvas behind it.
    ///
    /// MUTATION: `modal_up` without the `xmp_prompt` term.
    #[test]
    fn the_xmp_question_owns_the_keyboard() {
        let (src, _xmp, _dev, _scrub) = xmp_beside_fixture("keys");
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut app, &ctx, &src);
        assert!(app.xmp_prompt.is_some());
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::R,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.upd_shortcuts(ctx));
        assert!(!app.crop_mode, "R is dead under the question");
    }

    /// Complaint 1 (the save half): Ctrl+S on a never-edited photo with an
    /// .xmp beside it says there is nothing to save and writes NOTHING — no
    /// recipe.json, no .xmp, and no clear stamp that would silence the
    /// question about the file beside it.
    ///
    /// MUTATION: the `clear_has_work_in` early return removed from the store's
    /// `clear_develop_unlocked_in` (the clear runs and leaves its stamp).
    #[test]
    fn ctrl_s_on_a_never_edited_photo_saves_nothing() {
        let (src, _xmp, dev, _scrub) = xmp_beside_fixture("nothing");
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut app, &ctx, &src);
        app.save_xmp();
        assert_eq!(app.status, "nothing to save — this photo has no adjustments");
        assert!(develop_files(&dev).is_empty(), "nothing written: {:?}", develop_files(&dev));
        assert!(!autoshade::store::has_develop(&src), "no ● badge");
        let mut again = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut again, &ctx, &src);
        assert!(again.xmp_prompt.is_some(), "the unanswered question still stands");
    }

    /// Complaint 3 (the save half): what was read in is saved by Ctrl+S as a
    /// develop — recipe.json beside the answer — and never as an .xmp.
    ///
    /// MUTATION: the landing's `record_xmp_answer` call dropped from
    /// `read_xmp_in` (the folder holds no answer, and the reopen asks again).
    #[test]
    fn a_read_in_xmp_is_saved_as_a_develop_never_as_an_xmp() {
        let (src, xmp, dev, _scrub) = xmp_beside_fixture("readsave");
        let before = std::fs::read(&xmp).unwrap();
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut app, &ctx, &src);
        xmp_prompt_frame(&mut app, &ctx, &[egui::Key::Enter]);
        app.commit_if_settled(&ctx);
        app.save_xmp();
        assert_eq!(app.status, "develop saved");
        assert_eq!(
            develop_files(&dev),
            ["external-xmp.answered", "recipe.json", "source.txt"],
            "the develop, its breadcrumb and the answer — no .xmp in the store"
        );
        assert!(autoshade::store::has_develop(&src), "● edited");
        assert_eq!(std::fs::read(&xmp).unwrap(), before, "the file beside the photo is untouched");
        let mut again = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut again, &ctx, &src);
        assert_eq!(again.recipe.contrast, 33.0, "reopening restores the saved develop");
        assert!(again.xmp_prompt.is_none(), "…and does not ask again");
    }

    /// Complaint 4: the ● badge follows what this app saved, in a folder
    /// where every photo has an .xmp beside it. Edit + Ctrl+S lights it;
    /// Reset + Ctrl+S puts it out and says the adjustments were cleared. The
    /// sibling's badge and its question are untouched throughout.
    ///
    /// MUTATION: `save_xmp`'s neutral branch skipped — a reset canvas is
    /// saved as a (neutral) develop and the badge stays on.
    #[test]
    fn reset_and_save_puts_the_badge_out_beside_an_xmp() {
        let (src, _xmp, _dev, _scrub) = xmp_beside_fixture("badge");
        let dir = src.parent().unwrap().to_path_buf();
        let sibling = dir.join("other.ARW");
        std::fs::write(&sibling, b"raw").unwrap();
        write_newer_xmp(&dir.join("other.xmp"), 12.0, 3600);
        let sibling_dev = autoshade::store::develop_dir(&sibling);
        let _ = std::fs::remove_dir_all(&sibling_dev);
        let _scrub2 = Scrub(vec![sibling_dev.clone()]);
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut app, &ctx, &src);
        app.xmp_prompt = None;
        app.recipe.contrast = 5.0;
        app.save_xmp();
        assert_eq!(app.status, "develop saved");
        assert!(autoshade::store::has_develop(&src), "the badge is on");
        app.recipe.contrast = 0.0;
        assert!(app.recipe.is_noop(), "the premise: Reset");
        app.save_xmp();
        assert_eq!(app.status, "saved adjustments cleared — this photo is back to untouched");
        assert!(!autoshade::store::has_develop(&src), "the badge is off");
        assert!(!autoshade::store::has_develop(&sibling), "the sibling never had one");
        assert!(
            matches!(autoshade::store::external_xmp(&sibling), autoshade::store::ExternalXmp::Beside { .. }),
            "the sibling's question is still open"
        );
    }

    /// Complaint 5: no save path leaves an .xmp in the store, and a save over
    /// a develop folder holding an older build's `<stem>.xmp` removes it —
    /// Ctrl+S, an Analyze landing, a version load + Ctrl+S, and a paste,
    /// driven through their real doors. Save-all and the reverse-fit landing
    /// commit through the same member; the source pin below holds every
    /// commit site in the window to it.
    ///
    /// MUTATION: the Analyze landing's member back to the written projection.
    #[test]
    fn no_save_path_leaves_an_xmp_in_the_store() {
        let (src, _xmp, dev, _scrub) = xmp_beside_fixture("nosave");
        let old = autoshade::store::xmp_target(&src);
        let plant = || {
            std::fs::create_dir_all(&dev).unwrap();
            std::fs::write(&old, autoshade::xmp::recipe_to_xmp(&EditRecipe { saturation: 9.0, ..Default::default() }))
                .unwrap();
        };
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut app, &ctx, &src);
        app.xmp_prompt = None;

        plant();
        app.recipe.contrast = 4.0;
        app.save_xmp();
        assert!(!old.exists(), "Ctrl+S retires the old store .xmp: {}", app.status);

        plant();
        let epoch = app.gen_epoch;
        let accept = autoshade::advisor::Verdict {
            decision: autoshade::advisor::Decision::Accept,
            reasons: Vec::new(),
            revised_hint: None,
        };
        app.on_analyzed(
            Lang::En,
            epoch,
            Box::new(Ok((EditRecipe { contrast: 6.0, ..Default::default() }, accept, Vec::new()))),
        );
        assert!(app.status.starts_with("AI develop applied · saved"), "{}", app.status);
        assert!(!old.exists(), "the Analyze landing retires it too");

        plant();
        std::fs::write(
            autoshade::store::version_target(&src, 1),
            serde_json::to_string(&EditRecipe { contrast: 2.0, ..Default::default() }).unwrap(),
        )
        .unwrap();
        app.load_version(1);
        app.save_xmp();
        assert!(!old.exists(), "a loaded version saved by Ctrl+S: {}", app.status);

        plant();
        let other = src.parent().unwrap().join("pasted.ARW");
        std::fs::write(&other, b"raw").unwrap();
        let other_dev = autoshade::store::develop_dir(&other);
        let _ = std::fs::remove_dir_all(&other_dev);
        let _scrub2 = Scrub(vec![other_dev.clone()]);
        std::fs::create_dir_all(&other_dev).unwrap();
        let other_old = autoshade::store::xmp_target(&other);
        std::fs::copy(&old, &other_old).unwrap();
        app.start_paste_to(EditRecipe { contrast: 3.0, ..Default::default() }, None, vec![other.clone()], false);
        let start = std::time::Instant::now();
        while app.busy {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "the paste never landed");
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.poll_workers(&ctx);
        }
        assert_eq!(app.status, "Adjustments pasted to 1 photo(s)");
        assert!(!other_old.exists(), "a paste retires it too");
        assert!(!other.with_extension("xmp").exists(), "and writes nothing beside the photo");

        // Every commit the window makes — Ctrl+S, Save-all, the Analyze
        // landing, the reverse-fit landing, the paste — retires the store .xmp.
        let sources = [
            include_str!("../export.rs"),
            include_str!("../actions.rs"),
            include_str!("../workers.rs"),
        ];
        let sites: usize = sources.iter().map(|s| s.matches("autoshade::store::DevelopCommit {").count()).sum();
        let cleared: usize =
            sources.iter().map(|s| s.matches("xmp: autoshade::store::CommitMember::Clear").count()).sum();
        assert_eq!(sites, 5, "the five commit sites");
        assert_eq!(cleared, sites, "every one retires the store .xmp");
        assert!(
            sources.iter().all(|s| !s.contains("xmp_projection_member")),
            "no save path builds a projection"
        );
    }

    /// Acceptance 6 — "save as an .xmp" is the user's own act: 「Export .xmp
    /// beside the photo」 writes what is ON THE CANVAS into the photo's own
    /// folder (an unsaved slider change is in the file), saves nothing in the
    /// store but the answer for its own file, and reopening the photo neither
    /// asks about that file nor adopts it.
    ///
    /// MUTATION: `export_xmp_beside` exporting `self.saved_recipe` instead of
    /// the canvas.
    #[test]
    fn export_xmp_writes_the_canvas_beside_the_photo_and_is_not_asked_about() {
        let (src, xmp, dev, _scrub) = xmp_beside_fixture("export");
        // This test's .xmp is the app's own: nothing sits beside the photo yet.
        std::fs::remove_file(&xmp).unwrap();
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut app, &ctx, &src);
        assert!(app.xmp_prompt.is_none(), "premise: nothing to ask about");
        app.recipe.contrast = 21.0; // never saved
        assert!(app.can_export_xmp_beside());
        app.export_xmp_beside();
        assert!(app.status.starts_with(".xmp written beside the photo → "), "{}", app.status);
        let written = autoshade::xmp::xmp_to_recipe(&std::fs::read_to_string(&xmp).unwrap());
        assert_eq!(written.contrast, 21.0, "the unsaved change is in the file");
        assert_eq!(develop_files(&dev), ["external-xmp.answered"], "the export saved no develop");
        assert!(!autoshade::store::has_develop(&src), "…so the photo wears no ● badge");

        let mut again = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut again, &ctx, &src);
        assert!(again.xmp_prompt.is_none(), "the app does not ask about its own export");
        assert!(again.recipe.is_noop(), "…and does not adopt it: {}", again.recipe.contrast);
    }

    /// Two things 「Export .xmp beside the photo」 refuses. A canvas with no
    /// adjustments has nothing to export — and its two clicks must not
    /// replace the adjustments another program keeps beside the photo with
    /// neutral ones. A variant on AI-generated pixels cannot be described by
    /// an .xmp: the button is off, and the door writes nothing either.
    ///
    /// MUTATIONS: the neutral-canvas refusal removed from `export_xmp_beside`
    /// (the first click arms the overwrite); `can_export_xmp_beside` back to
    /// "any camera RAW".
    #[test]
    fn export_xmp_refuses_a_neutral_canvas_and_ai_pixels() {
        let (src, xmp, _dev, _scrub) = xmp_beside_fixture("exportno");
        let theirs = std::fs::read(&xmp).unwrap();
        let ctx = egui::Context::default();
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        land_cold_open(&mut app, &ctx, &src);
        app.xmp_prompt = None;
        assert!(app.recipe.is_noop(), "premise: an untouched canvas");
        app.export_xmp_beside();
        app.export_xmp_beside();
        assert_eq!(app.status, "nothing to export — this variant has no adjustments");
        assert!(!app.xmp_beside_confirm, "nothing was armed");
        assert_eq!(std::fs::read(&xmp).unwrap(), theirs, "the other program's file is untouched");

        let (_first, ctx, _dir, src, master, ai_px, _scrub) = ai_card_fixture("xmpexp", EditRecipe::default());
        let mut app = AutoShadeApp { src_path: Some(src.clone()), ..Default::default() };
        open_onto_ai_card(&mut app, &ctx, &ai_px, &master);
        assert!(app.active_on_ai_pixels(), "the premise: on the ✨ variant");
        assert!(!app.can_export_xmp_beside(), "the button is off on AI pixels");
        app.recipe.contrast = 9.0;
        app.export_xmp_beside();
        assert!(!src.with_extension("xmp").exists(), "and the door writes nothing: {}", app.status);
    }
