// Round-12 split: body moved verbatim from main.rs's inline
// `mod tests` (indentation kept — raw-string fixtures must not
// change by one byte). `super::*` still resolves to the root.
    use super::*;

    // The tests themselves, in parts under tests/: each file is spliced in here
    // by include!, so its items are items of this module and every test keeps
    // its name and its path. The order is the order the old file had.
    include!("tests/cards_and_versions.rs");
    include!("tests/canvas_and_delivery.rs");
    include!("tests/theme_fonts_curves.rs");
    include!("tests/develop_and_stash.rs");
    include!("tests/ai_cards.rs");
    include!("tests/strip_and_landing.rs");
    include!("tests/resolution_and_language.rs");
    include!("tests/prefs_and_workers.rs");
    include!("tests/disclosures_and_census.rs");
    include!("tests/panels_and_denoise_prefs.rs");
    include!("tests/sections_and_dots.rs");
    include!("tests/ai_panel.rs");
    include!("tests/turns_and_layout.rs");
    include!("tests/headings_and_widths.rs");
