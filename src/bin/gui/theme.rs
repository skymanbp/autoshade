//! Theme palettes, fonts and visuals (split from gui.rs, round 12).

use super::*;

// Two-tier colour rule (deliberate, not drift):
//  * PILL gold is the ONE chrome accent — panel selections, badges, active
//    variant, theme selection stroke all use the gold family below. Text-sized
//    gold goes through `ThemeColors::accent_text` (dark gold on the light
//    theme — #c9a14a on near-white fails contrast).
//  * ACCENT blue is for ON-CANVAS tool overlays ONLY (mask knobs, region
//    box): they sit on arbitrary photo content, and gold vanishes on warm
//    frames (a golden canyon) exactly where masks get drawn most — a colour
//    the theme never uses reads as "tool, not photo, not chrome". Overlays
//    are theme-independent for the same reason: they sit on the photo.
pub(crate) const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x4c, 0x8b, 0xf5);

pub(crate) const PILL: egui::Color32 = egui::Color32::from_rgb(0xc9, 0xa1, 0x4a);

/// The persisted UI look. Dark is the historical default; Light is a full
/// second scheme, not an inversion. Both route every theme-dependent chrome
/// colour through [`ThemeColors`], and both must pass the contrast test
/// (`both_themes_pass_contrast_checks`) — that is the guard that keeps a
/// future tweak from shipping unreadable text on one theme.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum ThemePref {
    #[default]
    Dark,
    Light,
}

/// Every chrome colour that must differ between the two themes. Plot
/// interiors (histogram, tone curve) are NOT here on purpose: they stay dark
/// in both themes, the pro-imaging convention that keeps colour judgement
/// stable while the chrome around them changes.
pub(crate) struct ThemeColors {
    /// Gold accent at text size — readable on this theme's panels.
    pub(crate) accent_text: egui::Color32,
    /// Widget selection fill / stroke (combo rows, text selection).
    pub(crate) selection_fill: egui::Color32,
    pub(crate) selection_stroke: egui::Color32,
    /// Gallery selected-row fill: the gold family at panel depth.
    pub(crate) sel_bg: egui::Color32,
    /// Multi-select row fill — dimmer than `sel_bg`, same family.
    pub(crate) sel_bg_dim: egui::Color32,
    /// Undecoded gallery-thumbnail placeholder block.
    pub(crate) thumb_placeholder: egui::Color32,
    /// Histogram clipping triangles: armed-but-clean / disarmed.
    pub(crate) clip_tri_on: egui::Color32,
    pub(crate) clip_tri_off: egui::Color32,
    /// Toast pill colours (bg, fg) per kind.
    pub(crate) toast_ok_bg: egui::Color32,
    pub(crate) toast_ok_fg: egui::Color32,
    pub(crate) toast_err_bg: egui::Color32,
    pub(crate) toast_err_fg: egui::Color32,
    /// Curve-editor channel PICKER labels (Master/R/G/B) — they sit on panel
    /// chrome, unlike the plot colours in [`CURVE_CHANNELS`] which stay on
    /// the always-dark curve square.
    pub(crate) curve_labels: [egui::Color32; 4],
    /// The armed-tool hint line: ACCENT-family text on chrome, darkened on
    /// the light theme where the canvas blue fails AA at text size.
    pub(crate) armed_hint: egui::Color32,
}

pub(crate) const DARK_COLORS: ThemeColors = ThemeColors {
    accent_text: PILL,
    // PILL (201,161,74) at alpha 64, premultiplied: (201,161,74)·64/255.
    // Tuned against the COMPOSITE the screen shows (fill over panel), not the
    // raw constant: at alpha 90 the composite put default text at 2.57:1 —
    // the old validator compared raw colours no pixel ever rendered.
    selection_fill: egui::Color32::from_rgba_premultiplied(50, 40, 19, 64),
    // Brighter than PILL: egui draws SELECTED-row text with this stroke, and
    // PILL over the gold composite sat at 3.6:1 — under AA for text.
    selection_stroke: egui::Color32::from_rgb(233, 196, 106),
    sel_bg: egui::Color32::from_rgb(0x45, 0x38, 0x1a),
    // A step darker than the historical 0x2e2612: default-colour text renders
    // on multi-select rows, and that pairing sat at ~4.45:1 — just under AA.
    sel_bg_dim: egui::Color32::from_rgb(0x24, 0x1e, 0x0c),
    thumb_placeholder: egui::Color32::from_gray(24),
    clip_tri_on: egui::Color32::from_gray(130),
    clip_tri_off: egui::Color32::from_gray(60),
    toast_ok_bg: egui::Color32::from_rgb(22, 58, 34),
    toast_ok_fg: egui::Color32::from_rgb(150, 230, 170),
    toast_err_bg: egui::Color32::from_rgb(70, 26, 26),
    toast_err_fg: egui::Color32::from_rgb(255, 165, 165),
    curve_labels: [
        egui::Color32::from_gray(225),
        egui::Color32::from_rgb(235, 90, 90),
        egui::Color32::from_rgb(90, 205, 90),
        egui::Color32::from_rgb(90, 130, 240),
    ],
    armed_hint: ACCENT,
};

pub(crate) const LIGHT_COLORS: ThemeColors = ThemeColors {
    accent_text: egui::Color32::from_rgb(0x6f, 0x51, 0x0b),
    // PILL (201,161,74) at alpha 90, premultiplied: (201,161,74)·90/255 — a
    // soft cream-gold once composited over the light panel. The old alpha-200
    // fill composited so saturated that the selection stroke's own text on it
    // fell to 2.7:1.
    selection_fill: egui::Color32::from_rgba_premultiplied(71, 57, 26, 90),
    // Matches accent_text: dark enough to stay AA as selected-row TEXT over
    // the cream composite, and well past 3:1 as an outline on panels.
    selection_stroke: egui::Color32::from_rgb(0x6f, 0x51, 0x0b),
    sel_bg: egui::Color32::from_rgb(0xf0, 0xe2, 0xc0),
    sel_bg_dim: egui::Color32::from_rgb(0xf6, 0xef, 0xdd),
    thumb_placeholder: egui::Color32::from_gray(224),
    clip_tri_on: egui::Color32::from_gray(95),
    clip_tri_off: egui::Color32::from_gray(190),
    toast_ok_bg: egui::Color32::from_rgb(0xdf, 0xf0, 0xe2),
    toast_ok_fg: egui::Color32::from_rgb(0x14, 0x52, 0x2a),
    toast_err_bg: egui::Color32::from_rgb(0xf9, 0xe3, 0xe3),
    toast_err_fg: egui::Color32::from_rgb(0x8c, 0x1d, 0x1d),
    curve_labels: [
        egui::Color32::from_gray(60),
        egui::Color32::from_rgb(0xb3, 0x24, 0x24),
        egui::Color32::from_rgb(0x1b, 0x6e, 0x2e),
        egui::Color32::from_rgb(0x2b, 0x4f, 0xc0),
    ],
    armed_hint: egui::Color32::from_rgb(0x24, 0x56, 0xb0),
};

impl ThemePref {
    pub(crate) fn colors(self) -> &'static ThemeColors {
        match self {
            ThemePref::Dark => &DARK_COLORS,
            ThemePref::Light => &LIGHT_COLORS,
        }
    }

    /// English label — the i18n skeleton key (zh comes from `tr`).
    pub(crate) fn label(self) -> &'static str {
        match self {
            ThemePref::Dark => "Dark",
            ThemePref::Light => "Light",
        }
    }
}

/// Embedded font subsets (see assets/fonts/README.md): egui's bundled fonts
/// lack most of the toolbar/panel symbols (⧉ ⊖ ◭ ▭ ◯ ◌ ✓ ✕ 🖌 …), and leaning
/// on system fonts made rendering machine-dependent — a stock Windows install
/// shows tofu boxes because DengXian lacks them too. ~623 KB total buys
/// identical rendering on every OS. Order = lookup priority; the
/// `embedded_fonts_cover_every_ui_symbol` test pins full-chain coverage.
///
/// The `cjk-ui` subset carries the hanzi the CHINESE UI ITSELF renders, so
/// picking 中文 no longer depends on the machine owning a system CJK font —
/// without one the whole window used to be tofu. It is last in priority so
/// the symbol donors keep the shared punctuation, and it is deliberately not
/// a full CJK face (~16 MB): the runtime fallback below still handles text
/// this static extraction cannot know, like a user's own file names.
pub(crate) const EMBEDDED_SYMBOL_FONTS: &[(&str, &[u8])] = &[
    ("symbols2", include_bytes!("../../../assets/fonts/NotoSansSymbols2-autoshop.ttf")),
    ("symbols", include_bytes!("../../../assets/fonts/NotoSansSymbols-autoshop.ttf")),
    ("math", include_bytes!("../../../assets/fonts/NotoSansMath-autoshop.ttf")),
    ("emoji-extra", include_bytes!("../../../assets/fonts/NotoEmoji-autoshop.ttf")),
    ("cjk-ui", include_bytes!("../../../assets/fonts/NotoSansSC-autoshop.ttf")),
];

/// Per-face byte budget for a RUNTIME system font (L12#3): the old path
/// `std::fs::read` an arbitrary file unbounded. Every real candidate is
/// well under this (malgun.ttf, the largest, is ~13.5 MB).
pub(crate) const MAX_FALLBACK_FONT_BYTES: u64 = 24 * 1024 * 1024;
/// …and a budget on the SUM of all accepted runtime faces (CJK + scripts).
pub(crate) const MAX_FALLBACK_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// Script tags whose glyphs actually made it into the runtime chain —
/// filled once by [`install_fonts`], read by [`installed_scripts`] for the
/// tofu disclosure (a name in a script with no installed face renders as
/// boxes; disclosure-not-silence).
static INSTALLED_SCRIPTS: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();

pub(crate) fn installed_scripts() -> &'static [&'static str] {
    INSTALLED_SCRIPTS.get().map(Vec::as_slice).unwrap_or(&[])
}

/// The writing-script of one char, for the ranges the fallback chain knows
/// about. `None` = Latin/symbols/etc — covered by egui's bundle + the
/// embedded subsets, never disclosed.
pub(crate) fn script_of(c: char) -> Option<&'static str> {
    Some(match c as u32 {
        0x0590..=0x05FF => "hebrew",
        0x0600..=0x06FF | 0x0750..=0x077F => "arabic",
        0x0900..=0x097F => "devanagari",
        0x0980..=0x09FF => "bengali",
        0x0B80..=0x0BFF => "tamil",
        0x0E00..=0x0E7F => "thai",
        0x1100..=0x11FF | 0xAC00..=0xD7AF => "hangul",
        0x3040..=0x30FF => "kana",
        0x3400..=0x4DBF | 0x4E00..=0x9FFF => "han",
        _ => return None,
    })
}

/// The scripts in `text` that NO installed face covers, each with one
/// sample char for the disclosure. Pure — `installed` is a parameter so
/// tests need no real fonts. (The embedded cjk-ui subset covers the UI's
/// own hanzi, so a han/kana file name MAY coincidentally render without a
/// system CJK face — the disclosure stays, conservatively.)
pub(crate) fn undrawable_scripts(
    text: &str,
    installed: &[&'static str],
) -> Vec<(&'static str, char)> {
    let mut out: Vec<(&'static str, char)> = Vec::new();
    for c in text.chars() {
        if let Some(s) = script_of(c)
            && !installed.contains(&s)
            && !out.iter().any(|(t, _)| *t == s)
        {
            out.push((s, c));
        }
    }
    out
}

/// Read one system font under a byte budget — stat BEFORE read, so a
/// mis-pointed path cannot balloon the heap; past-budget is disclosed and
/// skipped, never truncated (a truncated font is a parse error at best).
pub(crate) fn read_font_capped(p: &str, cap: u64) -> Option<Vec<u8>> {
    let len = std::fs::metadata(p).ok()?.len();
    if len > cap {
        eprintln!("⚠ system font {p} is {len} bytes — past the {cap}-byte budget, skipped");
        return None;
    }
    std::fs::read(p).ok()
}

/// Build the GUI font chain: egui defaults → embedded symbol subsets → system
/// CJK → per-script system faces (L12#3). The embedded subsets are
/// compile-time constants; every runtime face is read under a byte budget
/// and pre-validated with `ab_glyph` (egui's own backend) before handing it
/// over — egui PANICS on a font it can't parse, so a missing/odd font must
/// be skipped, not registered. Everything is appended as FALLBACKS, and the
/// embedded subsets stay AHEAD of every runtime face, so Latin text keeps
/// egui's default look and the symbol glyphs stay machine-independent.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    // Single-face TTFs first (always parse); TTC collections (face 0) last.
    // System font directories per OS (not user-specific paths); a miss just
    // falls through to the next candidate, so distro variance is harmless.
    #[cfg(target_os = "windows")]
    const CJK_CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\Deng.ttf",   // DengXian — clean modern UI face
        r"C:\Windows\Fonts\simhei.ttf", // SimHei
        r"C:\Windows\Fonts\msyh.ttc",   // Microsoft YaHei (collection, face 0)
        r"C:\Windows\Fonts\simsun.ttc", // SimSun (collection, face 0)
    ];
    #[cfg(target_os = "macos")]
    const CJK_CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/PingFang.ttc", // PingFang SC — macOS system CJK
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
    ];
    #[cfg(all(unix, not(target_os = "macos")))]
    const CJK_CANDIDATES: &[&str] = &[
        // Noto Sans CJK across the major distro layouts, then WenQuanYi.
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", // Debian/Ubuntu
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",      // Arch
        "/usr/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-Regular.ttc", // Fedora
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
    ];
    // Non-CJK scripts a user's own FILE NAMES can carry (L12#3): the CJK
    // face covers none of Arabic/Thai/Hebrew/Hangul/Indic (probed with
    // fontTools on this host, 2026-08-09 scan). Windows faces verified on
    // this machine: micross (Arabic+Thai+Hebrew), Nirmala.ttc face 0
    // (Devanagari+Bengali+Tamil), malgunsl (Hangul). macOS/Linux tables
    // stay EMPTY on purpose — no path ships unverified into a const; the
    // tofu disclosure covers those hosts honestly instead.
    #[cfg(target_os = "windows")]
    const SCRIPT_FALLBACKS: &[(&str, &[&str])] = &[
        ("arabic", &[r"C:\Windows\Fonts\micross.ttf", r"C:\Windows\Fonts\tahoma.ttf"]),
        ("hebrew", &[r"C:\Windows\Fonts\micross.ttf", r"C:\Windows\Fonts\tahoma.ttf"]),
        ("thai", &[r"C:\Windows\Fonts\micross.ttf", r"C:\Windows\Fonts\LeelawUI.ttf"]),
        ("hangul", &[r"C:\Windows\Fonts\malgunsl.ttf", r"C:\Windows\Fonts\malgun.ttf"]),
        ("devanagari", &[r"C:\Windows\Fonts\Nirmala.ttc"]),
        ("bengali", &[r"C:\Windows\Fonts\Nirmala.ttc"]),
        ("tamil", &[r"C:\Windows\Fonts\Nirmala.ttc"]),
    ];
    #[cfg(not(target_os = "windows"))]
    const SCRIPT_FALLBACKS: &[(&str, &[&str])] = &[];

    let mut total: u64 = 0;
    let mut load = |p: &str| -> Option<Vec<u8>> {
        let b = read_font_capped(p, MAX_FALLBACK_FONT_BYTES)?;
        if total + b.len() as u64 > MAX_FALLBACK_TOTAL_BYTES {
            eprintln!("⚠ system font {p} would exceed the total runtime-font budget, skipped");
            return None;
        }
        // Only accept it if egui's backend can actually parse face 0 (no panic).
        ab_glyph::FontVec::try_from_vec_and_index(b.clone(), 0).ok()?;
        total += b.len() as u64;
        Some(b)
    };
    let cjk = CJK_CANDIDATES.iter().find_map(|p| load(p));
    // One face can serve several scripts (micross, Nirmala) — key the font
    // data by PATH so shared bytes are inserted once.
    let mut script_faces: Vec<(String, Vec<u8>)> = Vec::new();
    let mut installed: Vec<&'static str> = Vec::new();
    for (script, candidates) in SCRIPT_FALLBACKS {
        for p in *candidates {
            if script_faces.iter().any(|(path, _)| path == p) {
                installed.push(script);
                break;
            }
            if let Some(b) = load(p) {
                script_faces.push(((*p).to_string(), b));
                installed.push(script);
                break;
            }
        }
    }
    if cjk.is_some() {
        // The CJK candidates all carry kana too (probed); han rides the
        // same face. Without it, han/kana names fall to the embedded
        // UI-subset's incidental coverage and the disclosure fires.
        installed.push("han");
        installed.push("kana");
    }
    let _ = INSTALLED_SCRIPTS.set(installed);

    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in EMBEDDED_SYMBOL_FONTS {
        fonts
            .font_data
            .insert((*name).to_owned(), egui::FontData::from_static(bytes));
    }
    if let Some(bytes) = cjk.clone() {
        fonts.font_data.insert("cjk".to_owned(), egui::FontData::from_owned(bytes));
    }
    for (path, bytes) in &script_faces {
        fonts
            .font_data
            .insert(format!("script:{path}"), egui::FontData::from_owned(bytes.clone()));
    }
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(fam).or_default();
        for (name, _) in EMBEDDED_SYMBOL_FONTS {
            list.push((*name).to_owned());
        }
        if cjk.is_some() {
            // AFTER the embedded subsets: CJK faces carry some symbol
            // glyphs too, and the embedded subsets must win those so
            // rendering stays machine-independent.
            list.push("cjk".to_owned());
        }
        // Script faces last for the same reason.
        for (path, _) in &script_faces {
            list.push(format!("script:{path}"));
        }
    }
    ctx.set_fonts(fonts);
}

/// Install the full UI style for one theme: a fresh egui dark/light Visuals
/// base (so toggling is idempotent — no residue from the other theme), the
/// warm-gold accent family from [`ThemeColors`], softer rounding, a calmer
/// type scale, and restrained hover feedback. Geometry and typography are
/// shared between themes; only colours differ. Idempotent — called at
/// startup, after prefs restore, and on every picker change.
pub(crate) fn install_theme(ctx: &egui::Context, theme: ThemePref) {
    use egui::{FontFamily, FontId, Rounding, Stroke, TextStyle};
    let c = theme.colors();
    let mut style = (*ctx.style()).clone();
    style.visuals = match theme {
        ThemePref::Dark => egui::Visuals::dark(),
        ThemePref::Light => egui::Visuals::light(),
    };
    style.visuals.selection.bg_fill = c.selection_fill;
    style.visuals.selection.stroke = Stroke::new(1.0, c.selection_stroke);
    style.visuals.hyperlink_color = c.accent_text;
    // Hover reads as a whisper of the accent, not a colour change — the
    // stroke carries the feedback so fills stay calm on both themes.
    style.visuals.widgets.hovered.bg_stroke =
        Stroke::new(1.0, c.selection_stroke.gamma_multiply(0.55));
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, c.selection_stroke);
    let rounding = Rounding::same(6.0);
    for w in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.rounding = rounding;
    }
    style.visuals.window_rounding = Rounding::same(10.0);
    style.visuals.menu_rounding = Rounding::same(8.0);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    // Type scale: headings one step DOWN from egui's shouty 18 so sections
    // group instead of competing; body/buttons half a step UP from 12.5 —
    // CJK at 12.5 px is where the "看着有点晕" complaint lives. Small stays
    // proportionally readable instead of egui's 9 px squint.
    for (ts, size) in [
        (TextStyle::Heading, 16.5),
        (TextStyle::Body, 13.0),
        (TextStyle::Button, 13.0),
        (TextStyle::Small, 10.5),
    ] {
        style.text_styles.insert(ts, FontId::new(size, FontFamily::Proportional));
    }
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace));
    ctx.set_style(style);
}
