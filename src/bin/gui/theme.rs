//! Theme palettes, fonts and visuals (split from gui.rs, round 12).

use super::*;

// Two-tier colour rule (deliberate, not drift):
//  * The chrome ACCENT the photographer picks ([`AccentPref`], gold by
//    default) is the ONE chrome accent — primary verbs, panel selections,
//    badges, the active variant. Its text-sized and selection forms are
//    DERIVED per theme ([`ThemeColors::accent_text`] etc.) so every pairing
//    clears the contrast bar whatever accent sits on whatever surface.
//  * ACCENT blue is for ON-CANVAS tool overlays ONLY (mask knobs, region
//    box): they sit on arbitrary photo content, and a warm accent vanishes on
//    warm frames (a golden canyon) exactly where masks get drawn most — a
//    colour the chrome accents never use reads as "tool, not photo, not
//    chrome". That is why no chrome accent is blue. Overlays are
//    theme-independent for the same reason: they sit on the photo.
pub(crate) const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x4c, 0x8b, 0xf5);

/// The historical chrome gold: the [`AccentPref::Gold`] fill.
pub(crate) const PILL: egui::Color32 = egui::Color32::from_rgb(0xc9, 0xa1, 0x4a);

/// The persisted UI look: four surface schemes (2026-10-13, user's pick —
/// 深灰 / 浅色 / 暖墨 / 冷蓝). Dark is the historical default; Light is a
/// full second scheme, not an inversion; Warm ink and Cool blue are dark
/// schemes with a tinted neutral. Every theme-dependent chrome colour routes
/// through [`ThemeColors`], and every theme × accent must pass the contrast
/// test (`every_theme_and_accent_passes_contrast_checks`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum ThemePref {
    #[default]
    Dark,
    Light,
    WarmInk,
    CoolBlue,
}

/// The chrome accent, picked independently of the surfaces.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum AccentPref {
    #[default]
    Gold,
    Teal,
    Coral,
    Violet,
    Green,
}

/// The surface colours of one scheme — everything that does NOT follow the
/// accent. Plot interiors (histogram, tone curve) are not here on purpose:
/// they stay dark in every theme, the pro-imaging convention that keeps
/// colour judgement stable while the chrome around them changes.
pub(crate) struct Surfaces {
    /// Undecoded gallery-thumbnail placeholder block.
    pub(crate) thumb_placeholder: egui::Color32,
    /// Central-canvas bed — one step below the panel fill, so the photo
    /// sits on a recessed stage while the chrome reads as raised.
    pub(crate) canvas_bed: egui::Color32,
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
    /// An open fold's plate ([`crate::headings::card_fold`]): one step above
    /// the panel fill, with a hairline a step above that.
    pub(crate) card_fill: egui::Color32,
    pub(crate) card_stroke: egui::Color32,
    /// The hairline every framed widget (button, field, menu) wears at rest,
    /// so a control has an edge on the plate instead of a flat grey slab.
    pub(crate) widget_edge: egui::Color32,
    /// A slider's knob ([`paint_slider_knob`]): a light disc on the dark
    /// themes, a white one with an edge on the light theme — never the
    /// rail's own grey, which is what egui paints it.
    pub(crate) knob_fill: egui::Color32,
    pub(crate) knob_stroke: egui::Color32,
    /// Surface tiers (2026-10-13, user: 「整体 UI 的配色需要继续打磨」): the
    /// title/tool bar and status bar sit darkest, the side panels a step up,
    /// an open fold's card a step above that (`card_fill`), the canvas bed
    /// recessed below all of them — Lightroom's stack. egui's stock style
    /// painted the bars, the panels and the dialogs one identical grey.
    pub(crate) bar_fill: egui::Color32,
    pub(crate) panel_fill: egui::Color32,
    /// A framed control at rest and under the pointer.
    pub(crate) widget_fill: egui::Color32,
    pub(crate) widget_hover: egui::Color32,
    /// Structural hairlines (the panel edge, list separators, framed groups),
    /// one step quieter than egui's stock grey — structure is carried by
    /// headings and air (`headings.rs`), and a stock-weight line read as a
    /// rule drawn across the panel (2026-10-01).
    pub(crate) hairline: egui::Color32,
    /// The text-edit well; `None` keeps egui's stock well for the scheme.
    pub(crate) text_well: Option<egui::Color32>,
    /// Alpha of the black elevation shadow: black-on-dark barely registers,
    /// so the dark schemes carry more.
    pub(crate) shadow_alpha: u8,
}

/// One theme × accent: the surfaces plus the accent's derived forms.
/// Reads of surface fields go through `Deref`, so `c.panel_fill` and
/// `c.accent_text` are spelled alike at every call site.
pub(crate) struct ThemeColors {
    surfaces: &'static Surfaces,
    /// The accent as a solid fill (the primary verb, the active-card ring) —
    /// light enough that [`crate::buttons::PRIMARY_FG`] reads on it.
    pub(crate) accent_fill: egui::Color32,
    /// The accent at text size — readable on this theme's panels and on the
    /// selected row.
    pub(crate) accent_text: egui::Color32,
    /// Widget selection fill (premultiplied) / stroke (combo rows, text
    /// selection).
    pub(crate) selection_fill: egui::Color32,
    pub(crate) selection_stroke: egui::Color32,
    /// Gallery selected-row fill: the accent family at panel depth.
    pub(crate) sel_bg: egui::Color32,
    /// Multi-select row fill — dimmer than `sel_bg`, same family.
    pub(crate) sel_bg_dim: egui::Color32,
}

impl std::ops::Deref for ThemeColors {
    type Target = Surfaces;
    fn deref(&self) -> &Surfaces {
        self.surfaces
    }
}

const DARK_SURFACES: Surfaces = Surfaces {
    thumb_placeholder: egui::Color32::from_gray(24),
    canvas_bed: egui::Color32::from_rgb(17, 17, 19),
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
    card_fill: egui::Color32::from_rgb(36, 37, 41),
    card_stroke: egui::Color32::from_rgb(47, 48, 53),
    widget_edge: egui::Color32::from_rgb(62, 63, 69),
    knob_fill: egui::Color32::from_gray(214),
    knob_stroke: egui::Color32::from_gray(30),
    bar_fill: egui::Color32::from_rgb(22, 22, 25),
    panel_fill: egui::Color32::from_rgb(29, 30, 33),
    widget_fill: egui::Color32::from_rgb(45, 46, 51),
    widget_hover: egui::Color32::from_rgb(56, 57, 63),
    hairline: egui::Color32::from_gray(46),
    text_well: Some(egui::Color32::from_gray(20)),
    shadow_alpha: 140,
};

const LIGHT_SURFACES: Surfaces = Surfaces {
    thumb_placeholder: egui::Color32::from_gray(224),
    canvas_bed: egui::Color32::from_rgb(226, 226, 229),
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
    card_fill: egui::Color32::from_gray(253),
    card_stroke: egui::Color32::from_gray(222),
    widget_edge: egui::Color32::from_gray(196),
    knob_fill: egui::Color32::WHITE,
    knob_stroke: egui::Color32::from_gray(150),
    bar_fill: egui::Color32::from_rgb(234, 234, 237),
    panel_fill: egui::Color32::from_rgb(244, 244, 246),
    widget_fill: egui::Color32::from_rgb(232, 232, 236),
    widget_hover: egui::Color32::from_rgb(221, 221, 227),
    hairline: egui::Color32::from_gray(212),
    text_well: None,
    shadow_alpha: 45,
};

/// 暖墨: the dark stack on a brown-black neutral, like ink on aged paper.
const WARM_INK_SURFACES: Surfaces = Surfaces {
    thumb_placeholder: egui::Color32::from_rgb(27, 24, 21),
    canvas_bed: egui::Color32::from_rgb(18, 16, 14),
    card_fill: egui::Color32::from_rgb(41, 37, 32),
    card_stroke: egui::Color32::from_rgb(54, 48, 42),
    widget_edge: egui::Color32::from_rgb(71, 64, 56),
    knob_fill: egui::Color32::from_rgb(222, 214, 200),
    knob_stroke: egui::Color32::from_rgb(30, 26, 22),
    bar_fill: egui::Color32::from_rgb(24, 21, 18),
    panel_fill: egui::Color32::from_rgb(32, 28, 24),
    widget_fill: egui::Color32::from_rgb(51, 46, 40),
    widget_hover: egui::Color32::from_rgb(63, 57, 50),
    hairline: egui::Color32::from_rgb(50, 45, 39),
    text_well: Some(egui::Color32::from_rgb(21, 19, 16)),
    ..DARK_SURFACES
};

/// 冷蓝: the dark stack on a slate-blue neutral.
const COOL_BLUE_SURFACES: Surfaces = Surfaces {
    thumb_placeholder: egui::Color32::from_rgb(20, 24, 32),
    canvas_bed: egui::Color32::from_rgb(12, 15, 21),
    card_fill: egui::Color32::from_rgb(31, 37, 48),
    card_stroke: egui::Color32::from_rgb(42, 49, 62),
    widget_edge: egui::Color32::from_rgb(58, 67, 84),
    knob_fill: egui::Color32::from_rgb(208, 216, 228),
    knob_stroke: egui::Color32::from_rgb(22, 26, 34),
    bar_fill: egui::Color32::from_rgb(17, 21, 28),
    panel_fill: egui::Color32::from_rgb(23, 28, 37),
    widget_fill: egui::Color32::from_rgb(40, 47, 61),
    widget_hover: egui::Color32::from_rgb(52, 60, 77),
    hairline: egui::Color32::from_rgb(40, 47, 60),
    text_well: Some(egui::Color32::from_rgb(15, 18, 25)),
    ..DARK_SURFACES
};

impl ThemePref {
    pub(crate) const ALL: [ThemePref; 4] = [ThemePref::Dark, ThemePref::Light, ThemePref::WarmInk, ThemePref::CoolBlue];

    /// Whether egui's dark base (and the OS's dark titlebar) carries it.
    pub(crate) fn is_dark(self) -> bool {
        self != ThemePref::Light
    }

    pub(crate) fn system(self) -> egui::SystemTheme {
        if self.is_dark() { egui::SystemTheme::Dark } else { egui::SystemTheme::Light }
    }

    pub(crate) fn surfaces(self) -> &'static Surfaces {
        match self {
            ThemePref::Dark => &DARK_SURFACES,
            ThemePref::Light => &LIGHT_SURFACES,
            ThemePref::WarmInk => &WARM_INK_SURFACES,
            ThemePref::CoolBlue => &COOL_BLUE_SURFACES,
        }
    }

    /// This theme under the accent [`install_look`] last installed.
    pub(crate) fn colors(self) -> &'static ThemeColors {
        palette(self, current_accent())
    }

    /// English label — the i18n skeleton key (zh comes from `tr`).
    pub(crate) fn label(self) -> &'static str {
        match self {
            ThemePref::Dark => "Dark",
            ThemePref::Light => "Light",
            ThemePref::WarmInk => "Warm ink",
            ThemePref::CoolBlue => "Cool blue",
        }
    }
}

impl AccentPref {
    pub(crate) const ALL: [AccentPref; 5] =
        [AccentPref::Gold, AccentPref::Teal, AccentPref::Coral, AccentPref::Violet, AccentPref::Green];

    /// The accent's own hue, as the picker's swatch shows it.
    pub(crate) fn hue(self) -> egui::Color32 {
        match self {
            AccentPref::Gold => PILL,
            AccentPref::Teal => egui::Color32::from_rgb(0x3f, 0xb8, 0xa8),
            AccentPref::Coral => egui::Color32::from_rgb(0xe8, 0x7a, 0x5d),
            AccentPref::Violet => egui::Color32::from_rgb(0xa8, 0x8b, 0xeb),
            AccentPref::Green => egui::Color32::from_rgb(0x7c, 0xb8, 0x5a),
        }
    }

    /// English label — the i18n skeleton key.
    pub(crate) fn label(self) -> &'static str {
        match self {
            AccentPref::Gold => "Gold",
            AccentPref::Teal => "Teal",
            AccentPref::Coral => "Coral",
            AccentPref::Violet => "Violet",
            AccentPref::Green => "Green",
        }
    }
}

/// WCAG 2.x contrast ratio of two opaque colours.
pub(crate) fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
    fn lum(c: egui::Color32) -> f64 {
        let ch = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A premultiplied fill composited over an opaque surface — what the screen
/// shows, which is the pairing contrast must be judged on.
pub(crate) fn composite(fill: egui::Color32, bg: egui::Color32) -> egui::Color32 {
    let a = f64::from(fill.a()) / 255.0;
    let ch = |f: u8, b: u8| (f64::from(f) + f64::from(b) * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    egui::Color32::from_rgb(ch(fill.r(), bg.r()), ch(fill.g(), bg.g()), ch(fill.b(), bg.b()))
}

fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let ch = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    egui::Color32::from_rgb(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()))
}

/// `c` moved toward `pole` in small steps until it reaches `min` contrast
/// against every surface in `on`. The steps stop at the pole, which clears
/// any bar a real surface sets.
fn until_readable(c: egui::Color32, pole: egui::Color32, on: &[egui::Color32], min: f64) -> egui::Color32 {
    (0..=50)
        .map(|i| mix(c, pole, i as f32 / 50.0))
        .find(|x| on.iter().all(|&bg| contrast(*x, bg) >= min))
        .unwrap_or(pole)
}

/// Contrast bars with a hair of headroom over the test's, so rounding in a
/// composite never lands a derived colour exactly on the line.
const TEXT_BAR: f64 = 4.6;
const COMPONENT_BAR: f64 = 3.05;

/// Derive one theme × accent. Every accent form is pushed until it clears
/// the bar its pairing needs, so a new accent or surface cannot ship an
/// unreadable combination — the contrast test checks the outcome.
fn build(theme: ThemePref, accent: AccentPref) -> ThemeColors {
    let s = theme.surfaces();
    let hue = accent.hue();
    let base = if theme.is_dark() { egui::Visuals::dark() } else { egui::Visuals::light() };
    let text = base.widgets.noninteractive.fg_stroke.color;
    let well = s.text_well.unwrap_or(base.extreme_bg_color);
    let (toward_text, away_from_text) =
        if theme.is_dark() { (egui::Color32::WHITE, egui::Color32::BLACK) } else { (egui::Color32::BLACK, egui::Color32::WHITE) };
    let accent_fill = until_readable(hue, egui::Color32::WHITE, &[crate::buttons::PRIMARY_FG], TEXT_BAR);
    let sel_bg = mix(s.panel_fill, hue, if theme.is_dark() { 0.22 } else { 0.30 });
    let accent_text = until_readable(hue, toward_text, &[s.panel_fill, sel_bg], TEXT_BAR);
    // Multi-select rows carry default-colour names, so the tint moves AWAY
    // from the text until those names read.
    let sel_bg_dim =
        until_readable(mix(s.panel_fill, hue, if theme.is_dark() { 0.12 } else { 0.14 }), away_from_text, &[text], TEXT_BAR);
    // The highlight under text being selected: as strong as the 3:1
    // component bar for the default text on it allows.
    let premul = |alpha: u8| {
        let k = f32::from(alpha) / 255.0;
        let ch = |v: u8| (f32::from(v) * k).round() as u8;
        egui::Color32::from_rgba_premultiplied(ch(hue.r()), ch(hue.g()), ch(hue.b()), alpha)
    };
    let start_alpha: u8 = if theme.is_dark() { 64 } else { 90 };
    let selection_fill = (0..=start_alpha / 4)
        .map(|i| premul(start_alpha - 4 * i))
        .find(|f| [s.panel_fill, well].iter().all(|&bg| contrast(text, composite(*f, bg)) >= COMPONENT_BAR))
        .unwrap_or(egui::Color32::TRANSPARENT);
    let composites = [composite(selection_fill, s.panel_fill), composite(selection_fill, well)];
    // Selected combo rows draw their text with this stroke over the highlight.
    let start = if theme.is_dark() { mix(hue, egui::Color32::WHITE, 0.25) } else { accent_text };
    let selection_stroke = until_readable(start, toward_text, &composites, TEXT_BAR);
    ThemeColors { surfaces: s, accent_fill, accent_text, selection_fill, selection_stroke, sel_bg, sel_bg_dim }
}

/// Every theme × accent, derived once on first use.
pub(crate) fn palette(theme: ThemePref, accent: AccentPref) -> &'static ThemeColors {
    static TABLE: std::sync::OnceLock<Vec<ThemeColors>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        ThemePref::ALL.iter().flat_map(|&t| AccentPref::ALL.iter().map(move |&a| build(t, a))).collect()
    });
    let t = ThemePref::ALL.iter().position(|&x| x == theme).unwrap_or(0);
    let a = AccentPref::ALL.iter().position(|&x| x == accent).unwrap_or(0);
    &table[t * AccentPref::ALL.len() + a]
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
    ("symbols2", include_bytes!("../../../assets/fonts/NotoSansSymbols2-autoshade.ttf")),
    ("symbols", include_bytes!("../../../assets/fonts/NotoSansSymbols-autoshade.ttf")),
    ("math", include_bytes!("../../../assets/fonts/NotoSansMath-autoshade.ttf")),
    ("emoji-extra", include_bytes!("../../../assets/fonts/NotoEmoji-autoshade.ttf")),
    ("cjk-ui", include_bytes!("../../../assets/fonts/NotoSansSC-autoshade.ttf")),
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

/// System faces that WERE there and were refused for size, with the numbers.
///
/// A refusal is a decision this app made, and it is not the same fact as "no
/// such font is installed" — only the first is something a user can act on.
/// Both used to reach `eprintln!` alone, which in a windowed build reaches
/// nobody. macOS is where this stops being theoretical: its system CJK
/// collection is far larger than any Windows candidate the budget was
/// calibrated against, so the honest answer there is a disclosure and the
/// existing tofu warning, not silence.
static REFUSED_FONTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

pub(crate) fn refused_system_fonts() -> &'static [String] {
    REFUSED_FONTS.get().map(Vec::as_slice).unwrap_or(&[])
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
    let mut refused: Vec<String> = Vec::new();
    let mut load = |p: &str| -> Option<Vec<u8>> {
        let Some(b) = read_font_capped(p, MAX_FALLBACK_FONT_BYTES) else {
            // Tell "over budget" from "not installed": one extra stat per
            // candidate, once, at startup. Only a face that EXISTS and was
            // turned away is worth telling the user about.
            if let Ok(m) = std::fs::metadata(p)
                && m.len() > MAX_FALLBACK_FONT_BYTES
            {
                refused.push(format!(
                    "{p} ({} MB — past this build's {} MB per-face budget)",
                    m.len() / (1024 * 1024),
                    MAX_FALLBACK_FONT_BYTES / (1024 * 1024)
                ));
            }
            return None;
        };
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
    let _ = REFUSED_FONTS.set(refused);

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

/// Spacing tokens (round-12 阶段5): ONE scale for the whole GUI instead of
/// per-site literals — breather / sub-group / section fence / group fence.
/// The values are the dominant rhythm the panels already used; the tokens
/// exist so every surface moves together when the scale is tuned.
pub(crate) const SPACE_XS: f32 = 2.0;
pub(crate) const SPACE_SM: f32 = 4.0;
pub(crate) const SPACE_MD: f32 = 6.0;
pub(crate) const SPACE_LG: f32 = 12.0;
/// Radius tokens: small inline chrome (thumbnails, plot beds) / widgets
/// (also toasts) / windows. Menus sit between MD and LG on purpose.
pub(crate) const RADIUS_SM: f32 = 3.0;
pub(crate) const RADIUS_MD: f32 = 6.0;
pub(crate) const RADIUS_LG: f32 = 10.0;
/// How far an open fold's plate reaches past its rows on each side
/// ([`crate::headings::card_fold`]).
pub(crate) const FOLD_CARD_PAD: f32 = 6.0;

/// Body and button text size — also the size a hand-built fold title
/// (`util::section_title`) lays its name at, since a `LayoutJob` carries its
/// own font instead of the style's.
pub(crate) const TEXT_BODY: f32 = 13.0;

/// Width tokens (#14a). One readable width for every row-filling control.
/// Since 2026-10-13 the controls PANEL carries the same ceiling
/// ([`controls_w_max`]): with the cap only on some controls, a panel dragged
/// wide ended its prompt fields and button rows at 420 px while folds, plates
/// and labels ran on to the panel edge — every section had its own right edge
/// (user: 「最大宽度会随着展开的编辑选项变化而变化」). The control-side clamps
/// stay for surfaces that are not the panel (the Settings window).
/// Prompt fields and `buttons::columns` share this ceiling. Prompts are
/// sentences, and a field that simply takes `available_width()` becomes an
/// 800 px ribbon the moment the side panel is dragged wide — the eye then
/// tracks a single line across the whole panel. Controls clamp DOWNWARD to this
/// (`buttons::columns`, `util::prompt_field`, and the Reimagine row's measured
/// arithmetic `.min()`s its result), which is why the cap cannot resurrect the R19
/// runaway: that one came from a row asking for MORE than it had.
///
/// NOTE — [`TOAST_W_MAX`] below happens to hold the SAME number for a
/// different reason (a wrapped paragraph's comfortable measure vs. one
/// clipped line's). They are two tokens on purpose: tuning one must not drag
/// the other along, and the pre-#14a code had this value as a bare `420.0`
/// literal in app.rs with no name at all.
pub(crate) const FIELD_W_MAX: f32 = 420.0;
/// …and a field's floor: below this a prompt box shows ~6 characters and the
/// row reads as broken. Inherited from the Reimagine row's own `.max(80.0)`
/// (R19), which is now this token's one definition.
pub(crate) const FIELD_W_MIN: f32 = 80.0;
/// Toast pill measure: a WRAPPED paragraph, capped so a long error does not
/// stretch across a 4K canvas (see [`FIELD_W_MAX`] on why these are two).
pub(crate) const TOAST_W_MAX: f32 = 420.0;

/// MOTION (2026-10-14, user: folds, toasts, compare and image swaps should
/// move rather than jump). One place for every duration, in seconds.
///
/// A fold opening or closing — egui's own `animation_time`, which its
/// collapsing headers, combo boxes and scroll-to read. egui ships 1/12 s,
/// short enough that a fold read as a jump; 0.18 s reads as a slide and is
/// still under the ~200 ms where a UI starts to feel like it is waiting.
pub(crate) const MOTION_FOLD_S: f32 = 0.18;
/// The B-key compare cross-fade and a new photo's (or version's) first frame
/// fading in over the old one. Short: the compare is a glance tool.
pub(crate) const MOTION_FADE_S: f32 = 0.16;
/// A toast sliding in from the right edge, eased out.
pub(crate) const MOTION_TOAST_IN_S: f32 = 0.22;
/// …and how far it travels, in points.
pub(crate) const MOTION_TOAST_SLIDE: f32 = 36.0;

/// Ease-out cubic over `t` in 0..1 — fast start, soft landing, the curve
/// every motion here uses so no two of them feel different.
pub(crate) fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// The controls panel's width: where it opens, and the narrowest a drag may
/// take it (below this the two-cell button rows clip their labels).
pub(crate) const CONTROLS_W_DEFAULT: f32 = 320.0;
pub(crate) const CONTROLS_W_MIN: f32 = 280.0;

/// The widest the controls panel may be dragged: the panel whose content
/// column is exactly [`FIELD_W_MAX`] wide — the side panel frame's own
/// margins plus the scrollbar gutter `controls_column` keeps — so at the cap
/// every row-filling control and every fold plate end on one right edge.
pub(crate) fn controls_w_max(style: &egui::Style) -> f32 {
    FIELD_W_MAX
        + egui::Frame::side_top_panel(style).inner_margin.sum().x
        + controls_gutter(style)
}

/// The right gutter of the controls column: the floating scrollbar's width
/// and a breath (see `AutoShadeApp::controls_column`).
pub(crate) fn controls_gutter(style: &egui::Style) -> f32 {
    style.spacing.scroll.bar_width + SPACE_SM
}

// The theme `install_theme` last installed on this thread. egui hands a
// `WidgetText` its colour at paint time only for `PLACEHOLDER`, so a
// two-colour title built away from any `Ui` (the fold's activity dot,
// `util::section_title`) reads the accent from here. Thread-local, not a
// process static: the GUI runs on one thread, and parallel tests that
// install different themes must not see each other's.
thread_local! {
    static CURRENT_THEME: std::cell::Cell<ThemePref> = const { std::cell::Cell::new(ThemePref::Dark) };
    static CURRENT_ACCENT: std::cell::Cell<AccentPref> = const { std::cell::Cell::new(AccentPref::Gold) };
}

pub(crate) fn current_theme() -> ThemePref {
    CURRENT_THEME.get()
}

pub(crate) fn current_accent() -> AccentPref {
    CURRENT_ACCENT.get()
}

/// The height of each of a slider's two lines (name + number, then the rail),
/// below the 26 px `interact_size` a button row stands: the rail line sizes
/// egui's knob (`rect.height() / 2.5`), and a full row made it a 10 px disc.
pub(crate) const SLIDER_LINE_H: f32 = 16.0;
/// The rail's thickness — Lightroom's hairline track, not egui's 8 px bar.
pub(crate) const SLIDER_RAIL_H: f32 = 4.0;

/// Paint the knob over an `egui::Slider` drawn by [`crate::AutoShadeApp`]'s
/// slider: egui fills its knob with the same grey it fills the rail with, so
/// the knob only showed by its outline. This disc sits exactly on egui's —
/// same centre (egui's own mapping: the rail's x range shrunk by the knob's
/// radius, log10-linear on a logarithmic positive track), same radius plus
/// the hover expansion — so it covers it on every state.
pub(crate) fn paint_slider_knob(ui: &egui::Ui, rail: &egui::Response, value: f32, min: f32, max: f32, log: bool) {
    let rect = rail.rect;
    if !ui.is_rect_visible(rect) || max <= min {
        return;
    }
    let radius = rect.height() / 2.5;
    let t = if log && min > 0.0 {
        (value.max(min).log10() - min.log10()) / (max.log10() - min.log10())
    } else {
        (value - min) / (max - min)
    };
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
    let c = theme_of(ui).colors();
    let state = ui.style().interact(rail);
    let fill = if rail.hovered() || rail.dragged() { c.knob_fill.gamma_multiply(1.08) } else { c.knob_fill };
    let fill = if ui.is_enabled() { fill } else { fill.gamma_multiply(0.45) };
    ui.painter().circle(
        egui::pos2(x, rect.center().y),
        radius + state.expansion,
        fill,
        egui::Stroke::new(1.0, c.knob_stroke),
    );
}

/// [`install_look`] keeping the accent last installed — a theme change alone.
pub(crate) fn install_theme(ctx: &egui::Context, theme: ThemePref) {
    install_look(ctx, theme, current_accent());
}

/// Install the full UI style for one theme and accent: a fresh egui
/// dark/light Visuals base (so toggling is idempotent — no residue from the
/// other theme), the accent family from [`ThemeColors`], softer rounding, a
/// calmer type scale, and restrained hover feedback. Geometry and typography
/// are shared between themes; only colours differ. Idempotent — called at
/// startup, after prefs restore, and on every picker change.
pub(crate) fn install_look(ctx: &egui::Context, theme: ThemePref, accent: AccentPref) {
    use egui::{FontFamily, FontId, Rounding, Stroke, TextStyle};
    let c = palette(theme, accent);
    CURRENT_THEME.set(theme);
    CURRENT_ACCENT.set(accent);
    // Pin egui's dark/light switch to the APP's choice. egui 0.29 defaults
    // to ThemePreference::System and `set_style` writes only the ACTIVE
    // theme's style slot (memory/mod.rs style_mut) — so on a light-mode OS
    // the startup install landed in the dark slot while the screen rendered
    // the STOCK light style: every token below was silently discarded and
    // the Settings theme picker looked dead. Pinning first makes the slot
    // we style the slot egui renders, on every OS mode.
    ctx.options_mut(|o| {
        o.theme_preference =
            if theme.is_dark() { egui::ThemePreference::Dark } else { egui::ThemePreference::Light };
    });
    // …and ask the OS for a matching titlebar (no-op headless / unsupported).
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(theme.system()));
    let mut style = (*ctx.style()).clone();
    style.visuals = if theme.is_dark() { egui::Visuals::dark() } else { egui::Visuals::light() };
    style.visuals.selection.bg_fill = c.selection_fill;
    style.visuals.selection.stroke = Stroke::new(1.0, c.selection_stroke);
    style.visuals.hyperlink_color = c.accent_text;
    // Hover reads as a whisper of the accent, not a colour change — the
    // stroke carries the feedback so fills stay calm on both themes.
    style.visuals.widgets.hovered.bg_stroke =
        Stroke::new(1.0, c.selection_stroke.gamma_multiply(0.55));
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, c.selection_stroke);
    // Hairlines one step quieter than egui's stock grey (`Surfaces::hairline`).
    style.visuals.widgets.noninteractive.bg_stroke.color = c.hairline;
    let rounding = Rounding::same(RADIUS_MD);
    for w in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.rounding = rounding;
    }
    style.visuals.window_rounding = Rounding::same(RADIUS_LG);
    style.visuals.menu_rounding = Rounding::same(8.0);
    // Depth: floating chrome casts a soft DOWNWARD penumbra — the stock
    // diagonal (10,20) reads as a misregistered copy, a vertical one as
    // elevation. Black-on-dark barely registers, so dark carries more alpha.
    let elevation = |offset: f32, blur: f32| egui::epaint::Shadow {
        offset: egui::vec2(0.0, offset),
        blur,
        spread: 0.0,
        color: egui::Color32::from_black_alpha(c.shadow_alpha),
    };
    style.visuals.window_shadow = elevation(6.0, 18.0);
    style.visuals.popup_shadow = elevation(3.0, 10.0);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style.spacing.slider_rail_height = SLIDER_RAIL_H;
    style.animation_time = MOTION_FOLD_S;
    // Controls get an edge at rest, and fields stop being black holes on the
    // dark plate; a fold's plate replaces the indent hairline (`card_fold`).
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, c.widget_edge);
    style.visuals.indent_has_left_vline = false;
    // The surface tiers (`ThemeColors::bar_fill`): panels and dialogs on
    // the panel tier, framed controls a step up and a step more on hover.
    style.visuals.panel_fill = c.panel_fill;
    style.visuals.window_fill = c.panel_fill;
    style.visuals.faint_bg_color = c.card_fill;
    style.visuals.widgets.inactive.bg_fill = c.widget_fill;
    style.visuals.widgets.inactive.weak_bg_fill = c.widget_fill;
    style.visuals.widgets.hovered.bg_fill = c.widget_hover;
    style.visuals.widgets.hovered.weak_bg_fill = c.widget_hover;
    if let Some(well) = c.text_well {
        style.visuals.extreme_bg_color = well;
    }
    // Type scale: headings one step DOWN from egui's shouty 18 so sections
    // group instead of competing; body/buttons half a step UP from 12.5 —
    // CJK at 12.5 px is where the "看着有点晕" complaint lives. Small stays
    // proportionally readable instead of egui's 9 px squint.
    for (ts, size) in [
        // 17.5 since 2026-10-01: Heading now marks only the three panel
        // titles (`headings::panel_heading`), which must out-rank the folds.
        (TextStyle::Heading, 17.5),
        (TextStyle::Body, TEXT_BODY),
        (TextStyle::Button, TEXT_BODY),
        (TextStyle::Small, 10.5),
    ] {
        style.text_styles.insert(ts, FontId::new(size, FontFamily::Proportional));
    }
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace));
    ctx.set_style(style);
}

/// The theme a `Ui` is drawn in: the one `install_look` last installed, so
/// long as egui's dark/light switch agrees with it (it pins that switch);
/// a `Ui` styled some other way gets the plain scheme of its own mode.
pub(crate) fn theme_of(ui: &egui::Ui) -> ThemePref {
    let t = current_theme();
    match (ui.visuals().dark_mode, t.is_dark()) {
        (true, true) | (false, false) => t,
        (true, false) => ThemePref::Dark,
        (false, true) => ThemePref::Light,
    }
}

/// One accent choice in the settings picker: a filled disc of the accent's
/// hue, ringed in the strong text colour when it is the one in use.
pub(crate) fn accent_swatch(ui: &mut egui::Ui, hue: egui::Color32, chosen: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    let centre = rect.center();
    let lift = if resp.hovered() { 1.0 } else { 0.0 };
    ui.painter().circle_filled(centre, 8.0 + lift, hue);
    if chosen {
        ui.painter().circle_stroke(centre, 11.0, egui::Stroke::new(1.5, ui.visuals().strong_text_color()));
    }
    resp
}
