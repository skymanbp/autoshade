// Round-12 split: body moved verbatim from main.rs's inline
// `mod tests` (indentation kept — raw-string fixtures must not
// change by one byte). `super::*` still resolves to the root.
    use super::*;

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
    /// shipped: those glyphs existed only on SOME machines. Scans STRING
    /// LITERALS of gui.rs + i18n.rs (comments never render); CJK ranges are
    /// exempt — ideographs come from the runtime system-font fallback by
    /// design. Fails ⇒ re-run scripts/subset_gui_fonts.py and commit the
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
        let pts = curve_points_mut(&mut r, 0);
        insert_curve_point(pts, 0, 0);
        insert_curve_point(pts, 255, 255);
        insert_curve_point(pts, 64, 96); // classic shadow lift between pinned ends
        let lut = autoshop::render::curve_lut(pts);
        assert!(lut[0].abs() < 1e-6 && (lut[255] - 1.0).abs() < 1e-6);
        assert!((lut[64] - 96.0 / 255.0).abs() < 1e-3, "anchored point maps exactly");
        // The channel selector reaches the right recipe field (master only here).
        for ch in 0..4 {
            assert_eq!(curve_points(&r, ch).len(), if ch == 0 { 3 } else { 0 });
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
            curve_points(&app.recipe, 2).len(),
            1,
            "the click adds one point to the SELECTED channel"
        );
        for ch in [0usize, 1, 3] {
            assert!(curve_points(&app.recipe, ch).is_empty(), "no cross-channel write (ch {ch})");
        }
        let p = &curve_points(&app.recipe, 2)[0];
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
                recipe: app.recipe.clone(),
                base: None,
                origin: None,
                kind: VariantKind::Original,
                others: vec![StashedVariant {
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
        let (batch, batch_kind) = crate::export::resolve_snapshot_develop(&src, &snap)
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
        let (batch, batch_kind) = crate::export::resolve_snapshot_develop(&src, &snap)
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
            v: 1,
            active_kind: VariantKind::Original.store_str().to_string(),
            active_pos: 0,
            others: vec![autoshop::store::VariantEntry {
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
                    kind: VariantKind::Original,
                    recipe: EditRecipe::default(),
                    base: None,
                    origin: None,
                    thumb: None,
                },
                Variant {
                    kind: VariantKind::Generated,
                    recipe: EditRecipe::default(),
                    base: Some(Arc::new(image::DynamicImage::new_rgb8(4, 3))),
                    origin: Some(master.clone()),
                    thumb: None,
                },
                Variant {
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
        app.delete_variant(1, &ctx);
        assert_eq!(app.active, 0, "premise: the strip re-anchored");
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
        app.delete_variant(1, &ctx);
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
                &(base.clone(), Vec::new(), Default::default(), None, None, (1280, None, None)),
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
        app.remember_master(p, 1280, master.clone());
        let hit = app.cached_master(p, 1280).expect("same path + edge hits");
        assert_eq!(hit.dimensions(), (6, 4), "the decoded pixels ride the entry");
        assert!(
            app.cached_master(p, 4096).is_none(),
            "a different edge must MISS — the entry holds 1280-edge pixels"
        );
        // Re-remembering the same (path, edge) replaces rather than stacks.
        app.remember_master(p, 1280, master.clone());
        assert_eq!(app.master_cache.len(), 1, "same key replaces its entry");
        let others: Vec<std::path::PathBuf> = (0..MASTER_CACHE_CAP)
            .map(|i| std::path::PathBuf::from(format!("D:/__autoshop_nonexistent__/m{i}.tif")))
            .collect();
        for o in &others {
            app.remember_master(o, 1280, master.clone());
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
