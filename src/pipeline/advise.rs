//! The analysis chain's revision helpers ([`super::produce_recipe`]): what a
//! revision round shows the proposer, and how its reply is read against the
//! answer it revises.
//!
//! Until this version a revision round re-sent the ORIGINAL frame with one
//! sentence of reviewer hint, so "lift the shadows about +15" referred to
//! numbers the model could not see and every other control was rolled afresh.
//! A revision now edits the model's own answer while looking at it, through
//! the ADJUST protocol ([`super::adjust_envelope`]) with the held recipe in
//! the place of the photographer's edit.

use crate::advisor::Opinions;
use crate::recipe::EditRecipe;

/// The request a revision round answers: the reviewer's note (it rides the
/// fenced `hint`) applied to the model's own previous answer, which is shown
/// rendered as IMAGE 1 and spelled as the CURRENT EDIT.
///
/// `view_json` is the held recipe as the render used it, `request` the
/// photographer's own words for this develop (their direction, or an ADJUST's
/// request), and `render_hist` the histogram summary of IMAGE 1 — the metadata
/// block's HISTOGRAM stays the source's, so the two read side by side.
pub(crate) fn revision_envelope(view_json: &str, request: Option<&str>, render_hist: &str) -> String {
    let request = match request.map(str::trim).filter(|r| !r.is_empty()) {
        Some(words) => format!(
            "keep honouring the photographer's request ({words}) and apply the REVISION NOTE at the \
             end; change what the note asks for and keep every other value as it stands"
        ),
        None => String::from(
            "apply the REVISION NOTE at the end; change what the note asks for and keep every other \
             value as it stands",
        ),
    };
    format!(
        "REVISION ROUND — you are revising YOUR OWN previous answer. IMAGE 1 is not the straight \
         preview this time: it is your previous answer RENDERED, and the CURRENT EDIT below is \
         that answer's EditRecipe. An automated reviewer looked at it; its REVISION NOTE refers to \
         this picture and these numbers. HISTOGRAM OF IMAGE 1 (your previous answer rendered): \
         {render_hist}. The HISTOGRAM in the metadata is the source's, before any edit. {}",
        super::adjust_envelope(view_json, Some(&request))
    )
}

/// Read a revision reply's NULLS against the answer it revises.
///
/// The prompt defines null as "no opinion — the current value stands", and in
/// a revision round the current value is the held recipe's: a control the
/// previous answer stated and the revision left at null keeps the previous
/// answer's value, and stays stated (so an ADJUST's final carry-over does not
/// put the photographer's base value back over it). A control neither answer
/// stated stays unstated. Returns the revision's opinions after the merge.
pub(crate) fn inherit_held_opinions(
    revised: &mut EditRecipe,
    revised_opinions: Opinions,
    held: &EditRecipe,
    held_opinions: Opinions,
) -> Opinions {
    let inherit: Vec<&str> = crate::advisor::catalogue::nullable_controls()
        .into_iter()
        .filter(|k| held_opinions.stated(k) && !revised_opinions.stated(k))
        .collect();
    if inherit.is_empty() {
        return revised_opinions;
    }
    let (Ok(mut out), Ok(from)) = (serde_json::to_value(&*revised), serde_json::to_value(held)) else {
        // An EditRecipe always serialises (it is plain data with no maps keyed
        // by non-strings); unreachable in practice, and the reply then stands
        // as the model sent it.
        return revised_opinions;
    };
    let mut opinions = revised_opinions;
    for key in inherit {
        if let Some(v) = from.get(key) {
            out[key] = v.clone();
            opinions = opinions.with(key);
        }
    }
    if let Ok(merged) = serde_json::from_value::<EditRecipe>(out) {
        *revised = merged;
        opinions
    } else {
        revised_opinions
    }
}

/// The verifier's verdicts, RANKED: Accept above Revise above Reject. A
/// verifier-driven revision replaces the held recipe only when its verdict
/// ranks strictly higher (`produce_recipe`'s do-no-harm rule for a reviewer
/// that never sees an image).
pub(crate) fn verdict_rank(d: &crate::advisor::Decision) -> u8 {
    use crate::advisor::Decision;
    match d {
        Decision::Reject => 0,
        Decision::Revise => 1,
        Decision::Accept => 2,
    }
}

#[cfg(test)]
#[path = "advise_tests.rs"]
mod tests;
