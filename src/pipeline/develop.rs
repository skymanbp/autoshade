//! The two ways the analysis can answer, and when the second one is tried.
//!
//! * CONTROLS — the vision model answers control values ([`super::produce_recipe`]:
//!   propose → verify → judge → revise). Every batch, web, eval and Normal-mode
//!   preset path keeps this, and it is [`AnalysisApproach`]'s struct default.
//! * STATEMENT — the vision model states the finished look of each area in
//!   CIELAB numbers and the reverse fit finds the settings
//!   ([`super::statement::statement_recipe`]).
//! * AUTO — the controls answer exactly as the caller asked for it; the stated
//!   look is tried only when the visual judge's last judgement of the develop
//!   that chain HOLDS at its end is not accept. Then the statement's develop
//!   and today's go side by side in ONE look (`advisor::judge_choice`), and
//!   the statement's is adopted only when the judge prefers it, so the new
//!   path can replace today's answer only by winning the comparison. An
//!   accepted develop, or one no judge looked at, stands and nothing more is
//!   spent.

use std::path::Path;

use anyhow::Result;

use super::statement::{StatementTools, statement_recipe};
use super::{GradeRequest, Guide};
use crate::advisor::{Decision, Verdict};
use crate::config::Config;
use crate::rationale::{Note, keys};
use crate::recipe::EditRecipe;

/// Which way the analysis answers. The struct default is
/// [`Self::Controls`] (today's behaviour); the interactive surfaces default to
/// [`Self::Auto`] on their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalysisApproach {
    /// The control values first; the stated look only when the visual judge
    /// did not accept them, and then one visual comparison picks.
    Auto,
    /// The stated look only, reverse-fitted.
    Statement,
    /// The control values only (today's analysis).
    #[default]
    Controls,
}

impl AnalysisApproach {
    pub const ALL: [AnalysisApproach; 3] = [AnalysisApproach::Auto, AnalysisApproach::Statement, AnalysisApproach::Controls];

    /// The command-line spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            AnalysisApproach::Auto => "auto",
            AnalysisApproach::Statement => "statement",
            AnalysisApproach::Controls => "controls",
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let t = text.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|a| a.as_str() == t)
            .ok_or_else(|| anyhow::anyhow!("unknown analysis approach {text:?} — use \"auto\", \"statement\" or \"controls\""))
    }
}

/// Why the statement path cannot run on this develop, or `None` when it can.
fn statement_unavailable(
    raw: &Path,
    pixels: Option<&Path>,
    cfg: &Config,
    base: Option<&EditRecipe>,
    tools: &StatementTools<'_>,
) -> Option<&'static str> {
    if base.is_some() {
        Some("it adjusts an existing develop")
    } else if pixels.is_some() {
        Some("the develop is on generated pixels")
    } else if !(tools.is_raw)(raw) {
        Some("the photo is not a camera RAW")
    } else if cfg.openai_api_key.is_none() {
        Some("no image-model key is configured")
    } else {
        None
    }
}

/// `Auto`'s one rule: the stated look is tried only when the visual judge's
/// last judgement of the develop the controls chain holds was revise or
/// reject. `Err` carries the note that says why it is not tried: the judge
/// accepted that develop, or no judgement exists (the judge switched off, no
/// model produced the recipe, the judge unavailable).
fn statement_wanted(judged: Option<&Decision>) -> std::result::Result<(), &'static str> {
    match judged {
        Some(Decision::Revise | Decision::Reject) => Ok(()),
        Some(Decision::Accept) => Err(keys::STATEMENT_NOT_NEEDED),
        None => Err(keys::STATEMENT_NOT_JUDGED),
    }
}

/// The reference library's results for the statement path: the same search
/// [`super::produce_recipe`] runs (this photo's image vector, the photo itself
/// excluded, [`crate::library::REFERENCE_COUNT`] hits), or nothing when the
/// library is not guiding, absent, unusable, or the embedding is off. A gap is
/// not this path's to disclose: the controls answer already does, and on its
/// own the statement simply goes without references.
fn with_references<R>(raw: &Path, cfg: &Config, req: &GradeRequest, f: impl FnOnce(&[crate::library::Hit<'_>]) -> R) -> R {
    let lib = match (req.library_style() > 0.0 && req.embed.on()).then(crate::library::load) {
        Some(crate::library::Loaded::Ready(lib)) => Some(lib),
        _ => None,
    };
    let query = lib.as_ref().and_then(|_| {
        let decoded = crate::decode::decode_any(raw).ok()?;
        let opts = crate::embed::EmbedOpts::from_config(cfg);
        crate::library::query_vector(&opts, &decoded.preview, &crate::store::store_root()).ok()
    });
    let hits = match (&lib, &query) {
        (Some(lib), Some(q)) => {
            lib.search(&crate::library::Query { image: q, exclude: Some(raw) }, crate::library::REFERENCE_COUNT)
        }
        _ => Vec::new(),
    };
    f(&hits)
}

/// A recipe as labelled judge context: oversize context is OMITTED, not
/// truncated (the judge's own rule).
fn context(r: &EditRecipe) -> Option<String> {
    serde_json::to_string(r).ok().filter(|s| s.len() <= 16 * 1024)
}

fn jpeg(img: &image::DynamicImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    img.to_rgb8().write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)?;
    Ok(out)
}

fn push(recipe: &mut EditRecipe, notes: &mut Vec<Note>, note: Note) {
    crate::rationale::push_note(&mut recipe.rationale, notes, note);
}

/// [`super::produce_recipe`]'s signature, answering by `req.approach`:
///
/// * `Controls` — `produce_recipe` as is.
/// * `Statement` — the statement path, then ONE visual judgement of its
///   deliverable (the judge's first look) whose decision is the verdict.
/// * `Auto` — `produce_recipe` as asked (its own judge loop untouched). When
///   the judge's last judgement of the develop it returns is accept, or there
///   is none, that develop stands and nothing more is spent. Otherwise the
///   statement path, then ONE `judge_choice` between the two deliverables
///   (reference = the calibration-only develop, A = today's, B = the
///   statement's, all rendered from the RAW as delivered). B is adopted only
///   when preferred; a tie, a failed comparison or a failed statement keeps
///   today's answer. The loser's claimed rasters are released.
///
/// Where the statement path cannot run (an ADJUST, generated pixels, a baked
/// file, no image-model key) `Auto` and `Statement` run as `Controls` and one
/// note says why.
#[allow(clippy::too_many_arguments)]
pub fn produce_develop(
    raw: &Path,
    pixels: Option<&Path>,
    cfg: &Config,
    verbose: bool,
    guidance: Option<&str>,
    base: Option<&EditRecipe>,
    req: GradeRequest,
    judge: bool,
    sink: &dyn crate::diag::Sink,
) -> Result<(EditRecipe, Verdict, Vec<Note>)> {
    if req.approach == AnalysisApproach::Controls {
        return super::produce_recipe(raw, pixels, cfg, verbose, guidance, base, req, judge, sink);
    }
    let tools = StatementTools::production(cfg, sink);
    produce_develop_with(raw, pixels, cfg, verbose, guidance, base, req, judge, sink, &tools)
}

/// [`produce_develop`] over explicit [`StatementTools`] (the tests' fakes).
#[allow(clippy::too_many_arguments)]
pub(crate) fn produce_develop_with(
    raw: &Path,
    pixels: Option<&Path>,
    cfg: &Config,
    verbose: bool,
    guidance: Option<&str>,
    base: Option<&EditRecipe>,
    req: GradeRequest,
    judge: bool,
    sink: &dyn crate::diag::Sink,
    tools: &StatementTools<'_>,
) -> Result<(EditRecipe, Verdict, Vec<Note>)> {
    if req.approach == AnalysisApproach::Controls {
        return super::produce_recipe(raw, pixels, cfg, verbose, guidance, base, req, judge, sink);
    }
    if let Some(why) = statement_unavailable(raw, pixels, cfg, base, tools) {
        let (mut recipe, verdict, mut notes) = super::produce_recipe(raw, pixels, cfg, verbose, guidance, base, req, judge, sink)?;
        push(&mut recipe, &mut notes, Note::new(keys::STATEMENT_UNAVAILABLE, vec![("why", why.to_string())]));
        return Ok((recipe, verdict, notes));
    }
    let direction = match req.guide {
        Guide::Library => None,
        Guide::Direction => guidance.filter(|g| !g.trim().is_empty()),
    };
    let say = |s: &str| {
        if verbose {
            println!("statement: {s}");
        }
    };
    // `Auto`: today's chain first, and its judge decides whether anything
    // more is spent — before the reference search, which is the statement's.
    let mut today = None;
    if req.approach == AnalysisApproach::Auto {
        let (mut held, held_verdict, mut held_notes, judged) =
            super::produce_recipe_judged(raw, pixels, cfg, verbose, guidance, base, req, judge, sink)?;
        if let Err(key) = statement_wanted(judged.as_ref()) {
            say(if key == keys::STATEMENT_NOT_NEEDED {
                "the visual review accepted the controls develop — the stated look is not tried"
            } else {
                "the controls develop was not visually reviewed — the stated look is not tried"
            });
            push(&mut held, &mut held_notes, Note::plain(key));
            return Ok((held, held_verdict, held_notes));
        }
        today = Some((held, held_verdict, held_notes));
    }
    with_references(raw, cfg, &req, |hits| {
        let style_look = crate::library::look_summary(hits);
        let intent = super::grade_intent(&req, direction, style_look.as_deref());
        let render = |r: &EditRecipe| -> Result<Vec<u8>> { jpeg(&(tools.render)(raw, r)?) };
        // The calibration-only develop: the picture both answers start from.
        let reference = || render(&super::calibration_recipe(super::photo_calibration(raw)));
        let Some((mut held, held_verdict, mut held_notes)) = today else {
            say("the stated look, then its reverse fit");
            let mut out = statement_recipe(raw, cfg, &req, direction, hits, tools, sink)?;
            if !judge {
                let verdict = Verdict {
                    decision: Decision::Revise,
                    reasons: vec!["the stated look was not visually reviewed".into()],
                    revised_hint: None,
                };
                return Ok((out.recipe, verdict, out.notes));
            }
            let judged = (|| -> Result<crate::advisor::Judgement> {
                let (r, c) = (reference()?, render(&out.recipe)?);
                Ok(crate::advisor::judge_pair(
                    cfg,
                    crate::advisor::JudgeImages { reference: &r, candidate: &c },
                    crate::advisor::JudgeTask::Develop,
                    context(&out.recipe).as_deref(),
                    Some(intent),
                )?)
            })();
            let verdict = match judged {
                Ok(j) => {
                    say(&format!("judge {:.0}/100 — {}", j.score, j.critique));
                    push(
                        &mut out.recipe,
                        &mut out.notes,
                        Note::new(keys::STATEMENT_JUDGE, vec![("score", format!("{:.0}", j.score)), ("critique", j.critique.clone())]),
                    );
                    Verdict { decision: j.decision, reasons: vec![j.critique], revised_hint: j.hint }
                }
                Err(e) => {
                    let e = crate::rationale::error_line(&e);
                    push(&mut out.recipe, &mut out.notes, Note::new(keys::JUDGE_UNAVAILABLE, vec![("e", e.clone())]));
                    Verdict {
                        decision: Decision::Revise,
                        reasons: vec![format!("the stated look could not be visually reviewed ({e})")],
                        revised_hint: None,
                    }
                }
            };
            return Ok((out.recipe, verdict, out.notes));
        };
        say("the visual review did not accept the controls develop — the stated look, then its reverse fit, then one look at both develops");
        let st = match statement_recipe(raw, cfg, &req, direction, hits, tools, sink) {
            Ok(st) => st,
            Err(e) => {
                let e = crate::rationale::error_line(&e);
                push(&mut held, &mut held_notes, Note::new(keys::STATEMENT_FAILED, vec![("e", e)]));
                return Ok((held, held_verdict, held_notes));
            }
        };
        let choice = (|| -> Result<crate::advisor::Choice> {
            let (r, a, b) = (reference()?, render(&held)?, render(&st.recipe)?);
            Ok(crate::advisor::judge_choice(
                cfg,
                crate::advisor::ChoiceImages { reference: &r, held: &a, candidate: &b },
                context(&held).as_deref(),
                context(&st.recipe).as_deref(),
                Some(intent),
            )?)
        })();
        match choice {
            Ok(c) if c.prefers_candidate => {
                say(&format!("the judge preferred the stated look ({:.0}/100)", c.winner.score));
                super::discard_rasters(&held, &st.recipe);
                let (mut recipe, mut notes) = (st.recipe, st.notes);
                let w = c.winner;
                push(
                    &mut recipe,
                    &mut notes,
                    Note::new(keys::STATEMENT_CHOSEN, vec![("score", format!("{:.0}", w.score)), ("critique", w.critique.clone())]),
                );
                Ok((recipe, Verdict { decision: w.decision, reasons: vec![w.critique], revised_hint: w.hint }, notes))
            }
            Ok(c) => {
                say(&format!("the judge kept the controls develop ({:.0}/100)", c.winner.score));
                super::discard_rasters(&st.recipe, &held);
                push(
                    &mut held,
                    &mut held_notes,
                    Note::new(
                        keys::STATEMENT_NOT_CHOSEN,
                        vec![("score", format!("{:.0}", c.winner.score)), ("critique", c.winner.critique)],
                    ),
                );
                Ok((held, held_verdict, held_notes))
            }
            Err(e) => {
                super::discard_rasters(&st.recipe, &held);
                let e = crate::rationale::error_line(&e);
                push(&mut held, &mut held_notes, Note::new(keys::STATEMENT_CHOICE_FAILED, vec![("e", e)]));
                Ok((held, held_verdict, held_notes))
            }
        }
    })
}

#[cfg(test)]
#[path = "develop_tests.rs"]
mod tests;
