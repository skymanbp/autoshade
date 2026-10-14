// One part of the GUI's tests (src/bin/gui/tests.rs includes it): theme contrast, the embedded fonts, the pickers and the curve editor.

    /// Every theme × accent must keep every text-on-chrome pairing at WCAG
    /// AA (4.5:1 for text, 3:1 for indicators). The accent forms are derived
    /// to clear these bars (`theme::build`); this checks the outcome on the
    /// surfaces the screen really shows — the installed panel fill and text
    /// well, not egui's stock ones. Pairings tested = pairings actually
    /// rendered (see each line's comment); `clip_tri_off` is deliberately dim
    /// (disarmed state) and exempt.
    #[test]
    fn every_theme_and_accent_passes_contrast_checks() {
        use crate::theme::{composite, contrast as ratio, palette, AccentPref};
        for theme in ThemePref::ALL {
            for accent in AccentPref::ALL {
                let c = palette(theme, accent);
                let visuals = if theme.is_dark() { egui::Visuals::dark() } else { egui::Visuals::light() };
                let panel = c.panel_fill;
                let well = c.text_well.unwrap_or(visuals.extreme_bg_color);
                let text = visuals.widgets.noninteractive.fg_stroke.color;
                let checks: [(&str, egui::Color32, egui::Color32, f64); 10] = [
                    // (what, fg, bg, minimum). The selected row carries ONLY
                    // accent-coloured text (name + badges — see the gallery
                    // row); multi-select rows render default-colour names.
                    ("default text on panels", text, panel, 4.5),
                    ("default text on an open fold's card", text, c.card_fill, 4.5),
                    ("accent text on panels", c.accent_text, panel, 4.5),
                    ("accent text on the selected row", c.accent_text, c.sel_bg, 4.5),
                    ("default text on a multi-select row", text, c.sel_bg_dim, 4.5),
                    ("success toast", c.toast_ok_fg, c.toast_ok_bg, 4.5),
                    ("error toast", c.toast_err_fg, c.toast_err_bg, 4.5),
                    // Coloured text that sits on panel chrome.
                    ("armed-tool hint", c.armed_hint, panel, 4.5),
                    // Non-text indicator: WCAG AA for UI components is 3:1.
                    ("armed clipping triangle", c.clip_tri_on, panel, 3.0),
                    // R38: the primary verb's near-black text on its solid fill.
                    ("primary verb text on its accent fill", crate::buttons::PRIMARY_FG, c.accent_fill, 4.5),
                ];
                for (what, fg, bg, min) in checks {
                    let r = ratio(fg, bg);
                    assert!(r >= min, "{theme:?} × {accent:?}: {what} is {r:.2}:1, needs {min}:1");
                }
                // selection_fill is PREMULTIPLIED-alpha: what renders is its
                // composite over the surface beneath.
                for (surface, bg) in [("panels", panel), ("the text-edit well", well)] {
                    let under = composite(c.selection_fill, bg);
                    // Default text over a TRANSIENT text-selection highlight:
                    // the 3:1 component bar, not the 4.5:1 text bar — holding
                    // 4.5:1 would force the highlight to near-invisibility on
                    // a dark panel's tiny luminance headroom.
                    let r = ratio(text, under);
                    assert!(r >= 3.0, "{theme:?} × {accent:?}: default text on selection over {surface} is {r:.2}:1");
                    // Selected COMBO rows draw their text with
                    // selection_stroke — persistent state, full text bar.
                    let r = ratio(c.selection_stroke, under);
                    assert!(r >= 4.5, "{theme:?} × {accent:?}: selection-stroke text over {surface} is {r:.2}:1");
                }
                // The selection outline is a non-text UI component: 3:1.
                let r = ratio(c.selection_stroke, panel);
                assert!(r >= 3.0, "{theme:?} × {accent:?}: selection stroke on panels is {r:.2}:1");
                for (i, col) in c.curve_labels.iter().enumerate() {
                    let r = ratio(*col, panel);
                    assert!(r >= 4.5, "{theme:?} × {accent:?}: curve label {i} is {r:.2}:1");
                }
            }
        }
        // The default look is the historical one: dark grey, the gold fill.
        assert_eq!(palette(ThemePref::Dark, AccentPref::Gold).accent_fill, crate::theme::PILL);
    }

    /// Picking a theme and an accent re-installs the style with both, and the
    /// pair survives a save and a restart.
    #[test]
    fn the_theme_and_accent_install_together_and_persist() {
        use crate::theme::AccentPref;
        let ctx = egui::Context::default();
        crate::theme::install_look(&ctx, ThemePref::CoolBlue, AccentPref::Teal);
        let c = crate::theme::palette(ThemePref::CoolBlue, AccentPref::Teal);
        assert_eq!(ctx.style().visuals.panel_fill, c.panel_fill);
        assert_eq!(ctx.style().visuals.selection.stroke.color, c.selection_stroke);
        assert_eq!(crate::theme::current_theme().colors().accent_fill, c.accent_fill, "colors() follows the installed accent");
        // A theme change alone keeps the accent.
        crate::theme::install_theme(&ctx, ThemePref::WarmInk);
        assert_eq!(crate::theme::current_accent(), AccentPref::Teal);
        assert!(ctx.style().visuals.dark_mode, "warm ink is a dark scheme");

        let json = serde_json::to_string(&Prefs { theme: ThemePref::WarmInk, accent: AccentPref::Coral, ..Prefs::default() }).unwrap();
        let back: Prefs = serde_json::from_str(&json).unwrap();
        assert_eq!((back.theme, back.accent), (ThemePref::WarmInk, AccentPref::Coral));
        // A save from before the accent existed opens in the historical gold.
        let old: Prefs = serde_json::from_str(r#"{"theme":"Light"}"#).unwrap();
        assert_eq!((old.theme, old.accent), (ThemePref::Light, AccentPref::Gold));
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
        let dir = std::env::temp_dir().join(format!("autoshade-claim-{}", std::process::id()));
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
        let profile = autoshade::recipe::LensProfile {
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
        let id = autoshade::render::curve_lut(&[]);
        assert!(id[0].abs() < 1e-6 && (id[255] - 1.0).abs() < 1e-6);
        assert!((id[128] - 128.0 / 255.0).abs() < 1e-3);

        let mut r = EditRecipe::default();
        let pts = curve_points_mut(&mut r, CurveTarget::Global, 0).expect("global");
        insert_curve_point(pts, 0, 0);
        insert_curve_point(pts, 255, 255);
        insert_curve_point(pts, 64, 96); // classic shadow lift between pinned ends
        let lut = autoshade::render::curve_lut(pts);
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
        let mut app = AutoShadeApp { curve_channel: 2, ..Default::default() };
        let ctx = egui::Context::default();
        let run_pass = |app: &mut AutoShadeApp, events: Vec<egui::Event>| -> bool {
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
    fn app_with_masked_photo(tag: &str) -> (AutoShadeApp, Scrub) {
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
            masks: vec![autoshade::recipe::LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: mask_path.to_string_lossy().into_owned() },
                exposure_ev: 0.4,
                color_gains: Some([1.2, 0.95, 0.7]),
                ..Default::default()
            }],
            ..Default::default()
        };
        let scrub = Scrub(vec![mask_path.clone()]);
        let app = AutoShadeApp {
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
            ..AutoShadeApp::default()
        };
        (app, scrub)
    }
