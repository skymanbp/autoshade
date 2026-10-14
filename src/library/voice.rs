//! What the advisor is told about the references: how closely to follow them,
//! and the block of words that carries them.

use super::Hit;

/// The Style value at which the references stop being a ceiling and become
/// the look to reproduce.
pub const STYLE_TARGET_MIN: f32 = 0.85;

/// What the reference block asks of the references.
///
/// Used only when the develop is guided by the library ([`crate::pipeline::Guide`]);
/// a develop guided by a typed direction never reads the library (user ruling
/// 2026-10-14: the two are either/or). `Default` is [`Self::Ceiling`], the
/// milder ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StyleVoice {
    /// Below [`STYLE_TARGET_MIN`]: stay within the references' level of grading.
    #[default]
    Ceiling,
    /// At or above [`STYLE_TARGET_MIN`]: reproduce their look.
    Target,
}

impl StyleVoice {
    /// The split by the Style axis.
    pub fn for_style(style: f32) -> Self {
        if style >= STYLE_TARGET_MIN { Self::Target } else { Self::Ceiling }
    }

    fn instruction(self) -> &'static str {
        match self {
            StyleVoice::Ceiling => {
                "Use them as a soft guide to this photographer's taste: grade toward their tonality and colour treatment, and do not go further than they do."
            }
            StyleVoice::Target => "Reproduce their look on this photo: tonality, contrast and colour treatment.",
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
pub fn reference_block(hits: &[Hit<'_>], images: &[Option<usize>], voice: StyleVoice) -> Option<String> {
    if hits.is_empty() {
        return None;
    }
    let mut out = format!(
        "REFERENCE LOOKS — {} finished photo(s) from the photographer's reference library, closest to this photo. Match their grading, never their subject, framing or content. {}",
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
