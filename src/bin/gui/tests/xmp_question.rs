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
