//! What the advisor is told about the references: whose aim leads, and the
//! block of words that carries them.

use super::Hit;

/// The Style value at which the references stop being a ceiling and become
/// the look to reproduce.
pub const STYLE_TARGET_MIN: f32 = 0.85;

/// WHOSE aim the reference block states.
///
/// User ruling 2026-09-01: **when a direction is given, the references are
/// background and the direction leads** ([`Self::Background`]), chosen by the
/// direction-adherence dial whose whole job is "how strongly should this
/// direction be followed". Without a leading direction the Style axis decides
/// between a ceiling and a target.
///
/// `Default` is [`Self::Ceiling`], the voice that says nothing is leading: a
/// forgotten field must not tell a reviewer the references are background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StyleVoice {
    /// Below [`STYLE_TARGET_MIN`], nothing leading: stay within the
    /// references' level of grading.
    #[default]
    Ceiling,
    /// At or above [`STYLE_TARGET_MIN`], nothing leading: reproduce their look.
    Target,
    /// A non-empty direction at adherence tier `Direct` or `Brief`: the
    /// references are background and the direction overrides them.
    Background,
}

impl StyleVoice {
    /// The ONE derivation every surface that renders or forecasts the block
    /// calls. A blank direction cannot lead, and a `Hint` direction has asked
    /// not to.
    pub fn choose(style: f32, direction: Option<&str>, adherence: crate::recipe::DirectionAdherence) -> Self {
        let leads = direction.is_some_and(|d| !d.trim().is_empty())
            && matches!(
                adherence.tier(),
                crate::recipe::AdherenceTier::Direct | crate::recipe::AdherenceTier::Brief
            );
        if leads { Self::Background } else { Self::for_style(style) }
    }

    /// The split for entry points with no direction to offer.
    pub fn for_style(style: f32) -> Self {
        if style >= STYLE_TARGET_MIN { Self::Target } else { Self::Ceiling }
    }

    fn instruction(self) -> &'static str {
        match self {
            StyleVoice::Ceiling => {
                "Use them as a soft guide to this photographer's taste: grade toward their tonality and colour treatment, and do not go further than they do."
            }
            StyleVoice::Target => "Reproduce their look on this photo: tonality, contrast and colour treatment.",
            StyleVoice::Background => {
                "The photographer's direction leads; these looks are background for continuity only, and the direction overrides them wherever they disagree."
            }
        }
    }
}

/// Phrases as one line of the block: control characters become spaces so no
/// phrase can forge a line of its own, and length is bounded.
fn one_line(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(super::MAX_PHRASE_CHARS).collect()
}

/// The reference block the proposer reads, or `None` when nothing was found.
///
/// `images[i]`, when present, is the request position of hit `i`'s attached
/// image (the develop target is image 1); an empty slice means the references
/// go as words only. The block is UNTRUSTED — phrases are file data — and the
/// provider fences it.
pub fn reference_block(hits: &[Hit<'_>], images: &[Option<usize>], voice: StyleVoice, by_prompt: bool) -> Option<String> {
    if hits.is_empty() {
        return None;
    }
    let chosen = if by_prompt { "closest to this photo and to the direction" } else { "closest to this photo" };
    let mut out = format!(
        "REFERENCE LOOKS — {} finished photo(s) from the photographer's reference library, {chosen}. Match their grading, never their subject, framing or content. {}",
        hits.len(),
        voice.instruction()
    );
    for (i, h) in hits.iter().enumerate() {
        let image = images.get(i).copied().flatten().map(|n| format!(" (IMAGE {n})")).unwrap_or_default();
        let look = h.entry.phrases.as_deref().map(one_line).filter(|p| !p.is_empty());
        out.push_str(&format!("\n  reference {}{image}: {}", i + 1, look.as_deref().unwrap_or("(no phrases)")));
    }
    Some(out)
}

/// The look in one short line for the reviewers (the judge never sees the
/// reference block): the first reference's phrases.
pub fn look_summary(hits: &[Hit<'_>]) -> Option<String> {
    hits.iter().find_map(|h| h.entry.phrases.as_deref()).map(one_line).filter(|p| !p.is_empty())
}
