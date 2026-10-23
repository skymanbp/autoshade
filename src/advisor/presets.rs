//! The fixed one-click looks (the desktop app's Normal mode, user ruling
//! 2026-10-23: 「就这 6 档」).
//!
//! A preset is not a set of slider numbers: it is a DIRECTION the model reads
//! (`pipeline::Guide::Direction`), so the same answer schema as every other
//! analysis comes back and the model still looks at the photo it is grading.
//! Each text says what to do AND what not to do; only the black-and-white
//! preset may turn the photo grey, and every other one says not to.
//!
//! The ids are stable keys (the GUI translates the display names); the texts
//! are English because the model reads them, and they never reach the UI.

/// One fixed look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// The stable key, also the English display name's source.
    pub id: &'static str,
    /// The direction the model is given, verbatim.
    pub direction: &'static str,
}

/// The six looks, in the order the GUI lays them (3 × 2).
pub const PRESETS: [Preset; 6] = [
    Preset {
        id: "natural",
        direction: "Natural: a faithful, true-to-life version of this scene. Correct the exposure and the white \
                    balance so neutral things read neutral, bring back clipped highlights and open blocked shadows \
                    gently, and keep contrast and saturation close to what the eye saw. Do not stylise: no colour \
                    grading, no strong vignette, no heavy clarity, texture or dehaze. Keep the photo in colour: \
                    leave convert_to_grayscale false.",
    },
    Preset {
        id: "bright_clean",
        direction: "Bright and clean: an airy, light look. Lift the exposure and the shadows so the photo feels \
                    bright, keep the whites clean and neutral without clipping them, keep contrast moderate and \
                    colours fresh but not loud. Do not crush the blacks, do not add a colour cast, grain or a dark \
                    vignette. Keep the photo in colour: leave convert_to_grayscale false.",
    },
    Preset {
        id: "warm_sunset",
        direction: "Warm: a golden, sunlit look. Move the white balance warmer and lean the highlights toward gold \
                    and amber with colour grading, keep skin and natural tones believable and contrast moderate. \
                    Do not turn the whole photo orange or muddy, and do not clip the bright areas. Keep the photo \
                    in colour: leave convert_to_grayscale false.",
    },
    Preset {
        id: "cool_night",
        direction: "Cool night: a cool, clean night look. Move the white balance cooler toward blue, keep the sky \
                    and the shadows deep but not crushed, keep street lights, windows and stars from blowing out, \
                    and keep noise under control with modest contrast. Do not brighten the night into daylight and \
                    do not add a magenta or green cast. Keep the photo in colour: leave convert_to_grayscale false.",
    },
    Preset {
        id: "film",
        direction: "Film: a soft analogue film look. Lift the blacks slightly with the tone curve for a gentle fade, \
                    keep contrast soft, mute the saturation a little, add a mild warm-highlight / cool-shadow split \
                    with colour grading and a little fine grain. Do not overdo the fade or the grain, and do not \
                    push colours to unnatural hues. Keep the photo in colour: leave convert_to_grayscale false.",
    },
    Preset {
        id: "mono",
        direction: "Black and white: convert the photo to grayscale (set convert_to_grayscale to true) and shape \
                    it with the grey mix bands (gray_red, gray_orange, gray_yellow, gray_green, gray_aqua, \
                    gray_blue, gray_purple, gray_magenta) so the sky, skin and foliage separate into distinct greys; \
                    give it rich contrast with clean blacks and clean whites. Do not add colour grading or a colour \
                    tint, and do not leave the photo in colour.",
    },
];

/// The preset called `id`.
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// The direction text of the preset called `id`.
pub fn preset_guidance(id: &str) -> Option<&'static str> {
    preset(id).map(|p| p.direction)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sentence every colour preset ends with: a colour look never comes
    /// back grey because the model read "film" or "night" as monochrome.
    const STAY_IN_COLOUR: &str = "Keep the photo in colour: leave convert_to_grayscale false.";

    /// MUTATION: duplicate an id, empty a text, or drop the grayscale
    /// instruction from `mono` (or from any colour preset's closing
    /// sentence) and a line below fails.
    #[test]
    fn six_presets_unique_ids_and_only_mono_turns_grey() {
        assert_eq!(PRESETS.len(), 6);
        let ids: std::collections::BTreeSet<&str> = PRESETS.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), PRESETS.len(), "ids are unique");
        assert_eq!(
            PRESETS.iter().map(|p| p.id).collect::<Vec<_>>(),
            ["natural", "bright_clean", "warm_sunset", "cool_night", "film", "mono"],
            "the six of the ruling, in the GUI's order"
        );
        for p in &PRESETS {
            assert!(!p.direction.trim().is_empty(), "{}: a text", p.id);
            assert!(p.id.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{}: a stable key", p.id);
            assert!(p.direction.contains("Do not") || p.direction.contains("do not"), "{}: says what not to do", p.id);
            assert_eq!(preset_guidance(p.id), Some(p.direction));
        }
        let mono = preset_guidance("mono").unwrap();
        assert!(mono.contains("set convert_to_grayscale to true"), "mono asks for grayscale");
        assert!(mono.contains("gray_red") && mono.contains("gray_blue"), "mono shapes the grey bands");
        for p in PRESETS.iter().filter(|p| p.id != "mono") {
            assert!(p.direction.ends_with(STAY_IN_COLOUR), "{}: forbids black and white", p.id);
            assert!(!p.direction.contains("to true"), "{}: never asks for grey", p.id);
        }
        assert_eq!(preset_guidance("sepia"), None);
    }

    /// Every control a preset names is a control the model's schema has —
    /// a misspelt name would be a direction the model cannot follow.
    #[test]
    fn every_control_a_preset_names_is_in_the_catalogue() {
        let known: Vec<&str> = crate::advisor::catalogue::RECIPE_CONTROLS.iter().map(|c| c.name).collect();
        for p in &PRESETS {
            for word in p.direction.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if word.contains('_') {
                    assert!(known.contains(&word), "{}: names {word:?}, which is no control", p.id);
                }
            }
        }
    }
}
