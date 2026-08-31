//! Shared pipeline core used by both the CLI (`main.rs`) and the web UI
//! (`serve.rs`): run the advise chain for one RAW and write its outputs to the
//! right place. Keeping this in one module means the CLI and the server can
//! never drift apart.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::advisor::{
    Advisor, ClaudeProvider, Decision, HeuristicProposer, LensOpinion, OpenAiProvider,
    OpenAiVerifier, Preview, Verdict,
};
use crate::config::Config;
use crate::decode;
use crate::recipe::{DirectionAdherence, EditRecipe, GradeStrength, StrengthTier};
use crate::xmp;

/// The TASTE dials one develop is asked for — the two independent axes plus the
/// style library's opt-in second image.
///
/// A struct rather than three more parameters: `produce_recipe` was already at
/// clippy's argument ceiling. It carries both axes because both are per-develop
/// user intent, but they are deliberately SEPARATE fields with separate names:
///   * [`style`](Self::style) = "how much like MY past edits" — mean-regression
///     toward the user's own habits (R23-2);
///   * [`strength`](Self::strength) = "how hard to push" — how committed the
///     grade is (R23-3).
///
/// They used to be ONE thing by accident: a non-zero Style injected a reference
/// block that said "do NOT exceed it", so asking for more personal style bought
/// more restraint and the app had no way to ask for less restraint at all
/// (feedback #5).
///
/// Renamed from `StyleRequest` in R23-3: the field it was named after is now one
/// of three, and the old name would have read as "the strength in here is the
/// style strength" at every call site.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradeRequest {
    /// 0..1 — how far the proposal leans on the user's past edits (the GUI's
    /// 「Style」 slider, `--style` on the CLI). 0 disables retrieval entirely.
    pub style: f32,
    /// Also SHOW the model the single most similar past photo, as a second
    /// input image. Opt-in and off by default: it is an extra image on every
    /// call of a paid analysis (the GUI's checkbox discloses the cost).
    pub send_reference_image: bool,
    /// How COMMITTED the grade should be (the GUI's 「Strength」 slider,
    /// `--strength` on the CLI). Defaults to [`GradeStrength::DEFAULT`].
    pub strength: GradeStrength,
    /// DEEP THINKING (R23-4, feedback #13): ask the proposer for its structured
    /// working in the same strict response, raise this run's reasoning tier one
    /// step, and let the visual judge converge over more than one round.
    ///
    /// `false` by DEFAULT and by construction on the unattended surfaces: batch
    /// and eval build their request through [`Self::default`] /
    /// [`Self::with_style`], and `produce_recipe` calls the proposer
    /// unconditionally — before any judge gate — so a thinking chain riding on
    /// `propose` would double a 500-photo batch's spend and stop eval from
    /// measuring the bare proposal the restraint constants were calibrated
    /// against. It is an INTERACTIVE opt-in: the GUI checkbox, `--deep`, the web
    /// body's `deep`.
    pub think: bool,
    /// How closely to follow the optional direction.
    pub adherence: DirectionAdherence,
    /// Whether the separate finished-photo look library may answer retrieval.
    pub use_looks: bool,
    /// May the SigLIP sidecar answer this develop's style query? A VALUE, not
    /// a process-environment read: the CLI resolves it from `--embed` /
    /// `--no-embed`, the GUI from its own preference, and both go through
    /// [`crate::style::EmbeddingSwitch::resolve`]. Until this batch the CLI
    /// flag was implemented by WRITING `AUTOSHADE_STYLE_EMBED` into the process
    /// and the pipeline read it back — a flag as a global side effect, and a
    /// GUI preference that could never reach the develop at all (the read was
    /// hard-coded `pref = false`).
    pub embed: crate::style::EmbeddingSwitch,
    /// The retrieval weights in force for this develop, resolved once.
    pub weights: crate::style::RetrievalWeights,
}

impl Default for GradeRequest {
    /// The struct defaults, PLUS the two fields whose default is the
    /// environment's answer — resolved here, once per request, so nothing
    /// downstream reads the process environment again.
    fn default() -> Self {
        GradeRequest {
            style: Default::default(),
            send_reference_image: Default::default(),
            strength: Default::default(),
            think: Default::default(),
            adherence: Default::default(),
            use_looks: Default::default(),
            embed: crate::style::EmbeddingSwitch::resolve(None, false),
            weights: crate::style::RetrievalWeights::from_env(),
        }
    }
}

impl GradeRequest {
    /// Text reference only, at the shipped grade strength — the shape every
    /// non-GUI surface uses unless it says otherwise.
    pub fn with_style(style: f32) -> Self {
        Self { style, ..Self::default() }
    }
}

/// The long edge of the JPEG preview handed to the vision proposer. Named
/// since R23-2 so the opt-in style REFERENCE photo goes through the same
/// sizing as the photo being developed instead of a second hand-typed number.
const ADVISOR_PREVIEW_EDGE: u32 = 1568;

/// What ONE style retrieval produced — the prompt block, the blend targets,
/// and (R23-2) the two DISCLOSURE facts the old two-tuple had nowhere to put:
/// which shots answered, and where the nearest one lives.
struct StyleRetrieval {
    /// The soft text reference block, or `None` when nothing was retrieved.
    reference: Option<String>,
    targets: std::collections::BTreeMap<&'static str, f32>,
    /// File names of the shots used, bounded (`style::neighbour_stems`).
    stems: Vec<String>,
    /// Absolute path of the NEAREST shot, when the index recorded one.
    nearest: Option<String>,
    /// Finished-photo look reference block, if the look library answered.
    look_reference: Option<String>,
    /// Nearest look image path, used only when the reference-image opt-in is on.
    look_nearest: Option<String>,
    look_stem: Option<String>,
    look_tags: Vec<String>,
    /// The LOOK this retrieval is asking for, as phrases, for the DOWNSTREAM
    /// REVIEWERS (B2) — `style::StyleIndex::look_summary`. `None` when the
    /// library carried no tags at all, which is the state a pre-S1 index and an
    /// undescribed one share.
    look_summary: Option<String>,
    looks_unreachable: bool,
    looks_count: usize,
}

/// The intent every downstream reviewer of one analysis shares (R23-3, B2).
///
/// A FUNCTION and not four inline literals: `produce_recipe` hands this to the
/// verifier, to the first judgement, and to the re-judge inside every
/// hint-guided revision round, and a second construction site is exactly how
/// the revision round would come to judge against a different brief than the
/// round it is revising — which is the mechanism that flattened the look in the
/// first place (six showcase runs, revision hints that were pure subtraction).
fn grade_intent<'a>(
    req: &GradeRequest,
    direction: Option<&'a str>,
    style_look: Option<&'a str>,
) -> crate::advisor::GradeIntent<'a> {
    crate::advisor::GradeIntent {
        strength: req.strength,
        adherence: req.adherence,
        direction,
        style_look,
    }
}

/// The single retrieval entry point shared by the develop pipeline and the
/// offline `style-query` diagnostic. Keeping the selection here prevents the
/// diagnostic from growing a subtly different ranking path.
pub fn retrieve_style<'a>(
    ix: &'a crate::style::StyleIndex,
    meta: &decode::Meta,
    histogram: &decode::Histogram,
    query: crate::style::StyleQuery<'_>,
    raw: &Path,
    use_looks: bool,
) -> (Vec<&'a crate::style::StyleExemplar>, Vec<&'a crate::style::LookExemplar>) {
    let exemplars = ix.retrieve_with_embed(meta, histogram, query, crate::style::RETRIEVE_K, raw);
    let looks = if use_looks { ix.retrieve_looks(query, 2) } else { Vec::new() };
    (exemplars, looks)
}

fn reference_image_choice(retrieved: &StyleRetrieval) -> Option<(&str, bool)> {
    retrieved
        .look_nearest
        .as_deref()
        .map(|path| (path, true))
        .or_else(|| retrieved.nearest.as_deref().map(|path| (path, false)))
}

/// The RAW neighbour, when the look the chooser preferred could not be read.
///
/// A look-library file that has been moved or deleted used to cost the develop
/// its reference image ENTIRELY: `reference_image_choice` prefers the look, the
/// decode failed, and the RAW neighbour that was sitting there all along was
/// never tried. The user had paid for the two-image call and got one image and
/// a note. Falling back is the honest behaviour, and the note now says which
/// photo actually went.
fn reference_image_fallback(retrieved: &StyleRetrieval) -> Option<&str> {
    retrieved.nearest.as_deref()
}

pub fn direction_adherence_tier(
    direction: Option<&str>,
    adherence: DirectionAdherence,
) -> Option<&'static str> {
    direction
        .filter(|text| !text.trim().is_empty())
        .map(|_| match adherence.tier() {
            crate::recipe::AdherenceTier::Hint => "hint",
            crate::recipe::AdherenceTier::Direct => "direct",
            crate::recipe::AdherenceTier::Brief => "brief",
        })
}

/// Encode one past photo as the JPEG preview the vision model receives —
/// the same decode → resize → JPEG path the photo being developed takes
/// ([`ADVISOR_PREVIEW_EDGE`]), so the two frames arrive comparable.
///
/// Takes a decode permit: this is a SECOND full decode inside one analyze, and
/// the process-wide cap exists precisely so concurrent decodes cannot stack
/// their ~180 MB buffers.
fn reference_preview(path: &Path) -> Result<Vec<u8>> {
    let _permit = decode::DecodePermit::acquire();
    let decoded = decode::decode_any(path)
        .with_context(|| format!("decode the style reference photo {}", path.display()))?;
    let img = decoded.preview_resized(ADVISOR_PREVIEW_EDGE);
    let mut jpeg = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .context("encode the style reference preview as JPEG")?;
    Ok(jpeg)
}

/// Run the full advise chain for one RAW: decode → propose (GPT or heuristic
/// fallback) → Claude verify → up to two verifier-driven revision rounds →
/// (when `judge`) the visual closed loop, which since R23-4 converges over one
/// to three guided rounds depending on the strength axis and `req.think`.
/// `verbose` prints the proposer/verifier/judge lines and, in thinking mode,
/// the model's full tool plan (CLI uses true, the server uses false).
/// Run the advise chain for one RAW. `guidance` is an optional user direction
/// (a prompt steering the edit, e.g. "warmer and moodier") woven into the GPT
/// prompt.
///
/// `sink` is where this call's DISCLOSURES go (R29-1). The chain raises two
/// ungated lines — the proposer fallback and the style-embedding degrade — and
/// until this parameter existed they were hard-coded `eprintln!`s a caller
/// could neither route nor order (see [`crate::diag`]). Pass
/// [`crate::diag::stderr`] for the shipped behaviour; a pooled caller passes a
/// [`crate::diag::Collector`] and renders the lines into the photo's own block.
/// The SUBJECT is bound here from `raw`, so no caller can attribute this
/// photo's warnings to another.
// One advise chain's own inputs — every one of them is a decision a caller
// makes per call (which photo, whose config, how loud, what direction, what
// base, which grade, judge or not, where the disclosures go). Bundling them
// into a struct would move the arity, not remove it, and `GradeRequest`
// already carries the half that groups.
#[allow(clippy::too_many_arguments)]
pub fn produce_recipe(
    raw: &Path,
    cfg: &Config,
    verbose: bool,
    guidance: Option<&str>,
    base: Option<&EditRecipe>,
    req: GradeRequest,
    judge: bool,
    sink: &dyn crate::diag::Sink,
) -> Result<(EditRecipe, Verdict, Vec<crate::rationale::Note>)> {
    // The third element (L12#2B): every DETERMINISTIC rationale fragment this
    // call appended, as typed notes rendering the rationale string's SUFFIX
    // (the prefix is the model's own prose). In-process only — the GUI
    // renders them localized; CLI/web/persistence keep the English string.
    //
    // The verifier's key is a requirement this flow is CERTAIN to hit, and
    // the old order discovered it only after the proposal had been billed —
    // a missing analysis key threw the paid proposal away (L10-8). Checked
    // before even the decode; the OAuth provider needs no key.
    if cfg.analysis_is_api() && cfg.analysis_api_key.is_none() {
        anyhow::bail!(
            "the analysis provider is 'api' but no analysis API key is configured — the \
             verify step would fail AFTER the paid proposal; set the key in Settings (or \
             AUTOSHADE_ANALYSIS_API_KEY), or switch the analysis provider to oauth"
        );
    }
    // This call's diagnostics channel, bound to the photograph it is about.
    let diag = crate::diag::Diag::about(sink, raw);
    // decode_any: a camera RAW, or an already-baked PNG/TIFF/JPEG (PNG-source mode).
    let decoded = decode::decode_any(raw)?;

    // The photographer's OWN words, captured BEFORE the Refine envelope below
    // shadows `guidance` with a whole EditRecipe JSON. Gates 3 and 4 (verifier,
    // visual judge) want the intent, not the recipe they are already reading.
    let user_direction = guidance;

    // Refine mode: when `base` (the user's CURRENT edit) is given, fold it into
    // the direction so GPT adjusts that edit rather than proposing from scratch.
    // Absent a base, behaviour is unchanged — a fresh proposal from the original.
    let refine_owned: Option<String> = base.map(|b| {
        // The AI proposes/verifies over the camera's embedded preview, where
        // the base look AND the lens corrections are already IN the pixels —
        // strip both from the copy woven into the prompt (either would
        // double-apply in the verifier's render). The strip lives HERE, not
        // in the callers: `base` must keep the user's REAL profile so
        // carry_over_unrepresentable below can preserve unsaved lens toggles
        // (pre-stripped callers made every Refine revert them to the saved
        // profile).
        let mut stripped = b.clone();
        stripped.base_curve = Vec::new();
        stripped.lens_profile = Default::default();
        let base_json = serde_json::to_string(&stripped).unwrap_or_default();
        format!(
            "REFINE the photographer's CURRENT edit instead of starting over — keep its choices and \
             change only what this direction implies. CURRENT EDIT (EditRecipe JSON): {base_json}. \
             Direction: {}",
            guidance.unwrap_or("make a small, tasteful improvement")
        )
    });
    let guidance = refine_owned.as_deref().or(guidance);

    let preview_img = decoded.preview_resized(ADVISOR_PREVIEW_EDGE);
    let mut jpeg = Vec::new();
    preview_img
        .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .context("encode preview JPEG for advisor")?;
    let preview = Preview { jpeg };
    // The style QUERY embedding (R27 Batch-5), taken here because this is the
    // last point the camera's own full preview is in scope — and because the
    // index's vectors were built from exactly this buffer through exactly this
    // helper (`style::embed_preview`), which is what makes a query vector and a
    // stored vector comparable at all.
    //
    // OFF unless the user asked (`req.embed`, resolved from the CLI flag, the
    // environment or the GUI preference before this call): the sidecar reloads
    // 1.5 GB of weights per call, so this is seconds of latency on every
    // develop, spent only when the index it is being compared against has the
    // vectors to spend it on. A failure degrades to the 14-dim retrieval with
    // a stderr line, never an aborted develop.
    let mut query_text: Option<Vec<f32>> = None;
    let query_embed: Option<Vec<f32>> = (req.style > 0.0 && req.embed.on())
        .then(|| crate::embed::EmbedOpts::from_config(cfg))
        .filter(|o| o.available())
        .and_then(|o| {
            match crate::style::embed_preview_with_text(
                &o,
                &decoded.preview,
                &crate::store::store_root(),
                "query",
                user_direction,
            ) {
                Ok(r) => { query_text = r.text_vector; Some(r.vector) },
                Err(e) => {
                    // A SIBLING of the four lines the adjudication enumerated,
                    // found by re-walking `eprintln!` in this file at R28 HEAD
                    // (Batch-2 did the same re-enumeration for the render's
                    // two): per-photo, ungated, worker-reachable, and it named
                    // no photograph either. It travels the caller's channel for
                    // the same reason.
                    diag.warn(format!(
                        "style embedding unavailable ({e:#}) — retrieving on the 14-dim \
                         feature alone"
                    ));
                    None
                }
            }
        });
    // The full-resolution preview buffer is DEAD from here on — only meta and
    // histogram feed the advise chain, which can stall on the network for
    // minutes. Keeping `decoded` whole pinned hundreds of MB of 61MP pixels
    // for that entire window.
    let decode::Decoded { meta, histogram, .. } = decoded;

    // Style influence: retrieve the user's edits on the most SIMILAR past shots
    // (needs a built style library). strength == 0 disables it entirely;
    // otherwise we inject a soft text reference AND, at higher strength, gently
    // pull the FINAL recipe toward those historical means (the blend below).
    // `style::load_effective` owns the central-then-legacy walk for every
    // surface (R23-2) — this used to be a third hand-written copy of it.
    let mut style_err: Option<String> = None;
    let style_ix = match (req.style > 0.0).then(crate::style::load_effective) {
        Some(crate::style::EffectiveIndex::Loaded(ix, _)) => Some(ix),
        Some(crate::style::EffectiveIndex::Unusable { err, .. }) => {
            // Surfaced ONCE on stderr: the original `.ok()` swallowed the
            // version-gate message, so a stale index silently disabled the
            // style reference with nothing to say why. The windowed GUI has no
            // console, so the same fact also rides the rationale below (L08
            // disclosure threading).
            //
            // NOT attributed to a photograph, and now the TYPE says so
            // (R29-1): the `Once` makes this a statement about the RUN — the
            // index is stale for every photo in it — and naming whichever
            // worker happened to reach it first would read as "this photo's
            // index is stale". R28 Batch-5 5c could only leave it un-stamped
            // and explain the gap in a comment; `Subject::Run` states it, on
            // the caller's own channel, so a sink can file it once for the run
            // instead of under an arbitrary photo. The per-photo half of the
            // same fact is `style_err`, which the rationale carries on every
            // photo.
            static ONCE: std::sync::Once = std::sync::Once::new();
            let once_msg = err.clone();
            let run = diag.rebound(crate::diag::Subject::Run);
            ONCE.call_once(move || {
                run.warn(format!(
                    "style reference unavailable ({once_msg}) — the Style slider has \
                     no effect until the index is rebuilt"
                ));
            });
            style_err = Some(err);
            None
        }
        // Nothing built yet, or nothing asked for. NOT silent any more: the
        // rationale block after the AI chain discloses it (R23-2).
        Some(crate::style::EffectiveIndex::Absent) | None => None,
    };
    let style_query =
        crate::style::StyleQuery::new(query_embed.as_deref(), query_text.as_deref(), req.weights);
    // Whether the DIRECTION had any part in ranking the looks. Computed from
    // the weights in force, once, and carried into both places that would
    // otherwise claim it: the look block and the IMAGE 2 sentence.
    let look_by_direction = crate::style::StyleIndex::look_ranked_by_direction(style_query);
    let retrieved = style_ix.as_ref().map(|ix| {
        let (ex, looks) =
            retrieve_style(ix, &meta, &histogram, style_query, raw, req.use_looks);
        let look_reference = ix.render_look_reference(&looks, look_by_direction);
        StyleRetrieval {
            // GATE 5: the reference block's own "do not exceed it" clauses are
            // templated on the STYLE axis, not on grade strength.
            reference: ix.render_reference_for_style(&ex, req.style),
            targets: crate::style::style_targets(&ex),
            stems: crate::style::neighbour_stems(&ex),
            // The nearest shot's own file, for the opt-in reference IMAGE. A
            // pre-path index records none — that degrades with a note rather
            // than silently sending nothing (see below).
            nearest: ex.first().and_then(|e| e.path.clone()),
            look_nearest: looks.first().map(|l| l.path.clone()),
            look_stem: looks.first().map(|l| l.stem.clone()),
            look_tags: looks.first().map(|l| l.tags.clone()).unwrap_or_default(),
            look_summary: crate::style::StyleIndex::look_summary(&looks, &ex),
            look_reference,
            looks_unreachable: req.use_looks && (query_embed.is_none() && query_text.is_none()) && !ix.looks.is_empty(),
            looks_count: ix.looks.len(),
        }
    });
    let reference: Option<String> = retrieved.as_ref().and_then(|r| r.reference.clone());
    let ref_str = reference.as_deref();
    // B2: the intent every downstream reviewer in this function shares (R23-3),
    // built HERE rather than above the retrieval because it now carries what
    // the retrieval found. ONE value, spread into the verifier, the first
    // judgement and every hint-guided re-judge below.
    let style_look = retrieved.as_ref().and_then(|r| r.look_summary.as_deref());
    let intent = grade_intent(&req, user_direction, style_look);
    if verbose && ref_str.is_some() {
        println!("style    : reference from similar past edits (strength {:.0}%)", req.style * 100.0);
    }
    // R23-2 opt-in: SHOW the model the nearest past photo, not just its
    // numbers. Fail-open in every arm — a missing or unreadable reference must
    // degrade this develop to the text reference (which is the historical
    // behaviour), never fail a paid analysis — and every degradation says so.
    let mut ref_preview: Option<Preview> = None;
    let mut ref_image_stem: Option<String> = None;
    let mut ref_image_is_look = false;
    let mut ref_image_err: Option<String> = None;
    if req.send_reference_image
        && let Some(r) = &retrieved
    {
        match reference_image_choice(r) {
            Some((p, is_look)) => match reference_preview(Path::new(p)) {
                Ok(jpeg) => {
                    ref_image_stem = Some(stem(Path::new(p)).to_string());
                    ref_image_is_look = is_look;
                    ref_preview = Some(Preview { jpeg });
                }
                // A LOOK that would not load falls back to the RAW neighbour
                // rather than costing this develop its reference image. Both
                // outcomes are disclosed: the fallback names the file it could
                // not read, and the RAW-neighbour arm is a plain failure as
                // before (there is nothing further to fall back to).
                Err(e) => {
                    let first = crate::rationale::error_line(&e);
                    match reference_image_fallback(r).filter(|_| is_look) {
                        Some(raw_path) => match reference_preview(Path::new(raw_path)) {
                            Ok(jpeg) => {
                                ref_image_stem = Some(stem(Path::new(raw_path)).to_string());
                                ref_image_is_look = false;
                                ref_preview = Some(Preview { jpeg });
                                ref_image_err = Some(format!(
                                    "{first}; the closest RAW neighbour went instead"
                                ));
                            }
                            Err(e2) => {
                                ref_image_err = Some(format!(
                                    "{first}; the RAW neighbour did not load either ({})",
                                    crate::rationale::error_line(&e2)
                                ))
                            }
                        },
                        None => ref_image_err = Some(first),
                    }
                }
            },
            // No path recorded, yet exemplars WERE retrieved: a pre-R23 index.
            // (Nothing retrieved at all is the no-reference note's business.)
            None if !r.stems.is_empty() => {
                ref_image_err = Some(
                    "this style index was built before per-photo paths were recorded — \
                     rebuild it to send a reference photo"
                        .into(),
                )
            }
            None => {}
        }
    }
    if verbose
        && let Some(file) = &ref_image_stem
    {
        println!(
            "style    : {file} is going to the vision model as IMAGE 2 (the reference-image \
             option is on) — one extra image on each call of this analysis"
        );
    }
    if verbose
        && let Some(e) = &ref_image_err {
            println!("style    : no reference image ({e}) — the text reference only");
        }
    if verbose
        && let Some(g) = guidance {
            println!("direction: {g}");
        }

    let (meta, hist) = (&meta, &histogram);

    // The WB anchor, ONE read for all THREE of its consumers (the single-read
    // rule): the PROPOSER's prompt, the visual judge's render clone and the
    // deliverable stamp consume the same values, so they can never disagree
    // across a concurrent writer's retire/publish window.
    //
    // Read HERE, above the propose call, since R23-1: `temperature_k` is an
    // ABSOLUTE Kelvin target and `tint` a shift RELATIVE to the same anchor, so
    // the prompt must state the anchor the deliverable will actually carry
    // (feedback #12) — a second, fresher read at the prompt would re-decode the
    // RAW and could quote the model a different anchor than the render uses.
    // The base-look snapshot deliberately stays where it was (below, after the
    // AI chain): it has ONE consumer, the deliverable stamp, so reading it late
    // keeps it as fresh as it has always been instead of widening its staleness
    // window across minutes of network calls for no gain.
    let (anchor_k, anchor_tint) = match saved_recipe_snapshot(raw) {
        // Saved-first, like the base-look stamp below (a legacy save keeps
        // None -> the 5500 K anchor -> byte-identical rendering of its
        // tuned Kelvin).
        Some(saved) => (saved.as_shot_k, saved.as_shot_tint),
        None => fresh_as_shot_wb(raw),
    };

    // GPT vision when a key is set; on failure (quota/network) warn and fall back
    // to the heuristic so we still produce a recipe (disclosure, not masking).
    let openai = OpenAiProvider::new(cfg);
    // The per-call inputs every propose in this function shares (the revision
    // rounds differ only in `hint`).
    let propose_ctx = crate::advisor::ProposeContext {
        reference: ref_str,
        guidance,
        hint: None,
        as_shot_k: anchor_k,
        // Rides EVERY round of this analysis (the revision rounds spread this
        // struct): a reference the first call saw and the revision did not
        // would make the two rounds answer different questions.
        reference_image: ref_preview.as_ref(),
        look_reference: retrieved.as_ref().and_then(|r| r.look_reference.as_deref()),
        reference_image_is_look: ref_image_is_look,
        look_ranked_by_direction: look_by_direction,
        // GATE 1 (prompt) + GATE 2 (`temper`, inside the provider).
        strength: req.strength,
        // R23-4: the structured working + the deepened tier, on EVERY propose
        // this analysis makes (the revision rounds spread this struct) — a
        // first call that planned and a revision that did not would answer two
        // different questions, exactly like the reference image above.
        think: req.think,
        adherence: req.adherence,
    };
    let mut det_notes: Vec<crate::rationale::Note> = Vec::new();
    // The working of the proposal that SURVIVES (R23-4). Every arm that
    // replaces `recipe` wholesale replaces this with it — a plan describing a
    // discarded candidate is worse than none.
    let mut thinking: Option<crate::advisor::Thinking> = None;
    // …and which manual lens controls THAT proposal spoke about (R23-1b),
    // tracked the same way and for the same reason: `carry_over_unrepresentable`
    // must not re-impose the base's lens values over an opinion the surviving
    // recipe actually stated. Nothing stated = the historical behaviour.
    let mut lens_opinion = LensOpinion::default();
    let (mut recipe, can_revise) = if cfg.openai_api_key.is_some() {
        if verbose {
            println!("proposer : OpenAI ({})", cfg.openai_model);
        }
        match openai.propose_planned(&preview, meta, hist, &propose_ctx) {
            Ok(p) => {
                thinking = p.thinking;
                lens_opinion = p.lens;
                (p.recipe, true)
            }
            Err(e) if base.is_some() => {
                // REFINE means "adjust MY edit": the heuristic fallback
                // proposes from scratch and cannot see the base, so falling
                // back here REPLACED the user's current edit with a generic
                // baseline — and an Accept verdict then auto-saved it. A
                // failed refine fails loudly; the edit on screen stays as it
                // was.
                return Err(anyhow::Error::from(e)
                    .context("the AI refine failed — your current edit is unchanged"));
            }
            Err(e) => {
                // THE line `jobs`' module doc names by file:line as the one a
                // parallel batch reorders. R28 Batch-5 5c stamped it with the
                // photograph; R29-1 puts it on the caller's channel, so a
                // pooled caller can also decide WHEN it appears. The same fact
                // rides the typed note channel below — which `batch` used to
                // discard and now renders into the photo's block.
                diag.warn(format!(
                    "GPT proposer failed ({e})\n  → falling back to the heuristic baseline."
                ));
                // Hand the REAL cause to the heuristic: this stderr line is
                // invisible in the windowed GUI, so the recipe's rationale is
                // the only place the user can learn why the AI didn't run.
                let heuristic = HeuristicProposer {
                    fallback_reason: Some(crate::rationale::error_line(&anyhow::Error::new(e))),
                };
                let (r, note) = heuristic.propose_noted(hist, req.strength)?;
                det_notes.push(note);
                (r, false)
            }
        }
    } else {
        // Refine is not a proposal — it is "adjust THIS edit". The heuristic
        // cannot do that: it never receives `base` and builds a fresh recipe
        // from the histogram alone, so letting it answer a refine replaces
        // the user's work with a generic baseline AND saves it (the verifier
        // accepts a plain baseline happily). The has-key path already refuses
        // for the same reason; this branch is reachable with no key at all,
        // and both UIs offer the Refine control without checking for one.
        if base.is_some() {
            anyhow::bail!(
                "Refine needs the image model, and no OPENAI_API_KEY is set — \
                 your current edit is unchanged"
            );
        }
        if verbose {
            println!("proposer : heuristic baseline (set OPENAI_API_KEY to use GPT vision)");
        }
        let heuristic = HeuristicProposer::default();
        let (r, note) = heuristic.propose_noted(hist, req.strength)?;
        det_notes.push(note);
        (r, false)
    };

    // Verifier (analysis role): OAuth `claude` CLI by default, or an
    // OpenAI-compatible API when the analysis provider is set to `api`.
    let verifier: Box<dyn Advisor> = if cfg.analysis_is_api() {
        Box::new(OpenAiVerifier::new(cfg))
    } else {
        Box::new(ClaudeProvider::new(cfg))
    };
    if verbose {
        let who = if cfg.analysis_is_api() { "OpenAI-API" } else { "Claude (OAuth)" };
        println!("verifier : {who} ({})", cfg.analysis_model);
    }
    let mut verdict = verifier.verify(&recipe, meta, hist, &intent)?;

    // Bounded verify→revise loop (only if GPT actually produced the recipe). With
    // the now-symmetric verifier — which pushes a too-flat edit to commit AND a
    // too-cooked one to ease — a few rounds converge toward a finished look instead
    // of just ratcheting down. Capped at MAX_REVISIONS to bound cost/latency; we
    // stop early on Accept or when the verifier stops giving a revision hint.
    const MAX_REVISIONS: usize = 2;
    let mut round = 0;
    while can_revise && round < MAX_REVISIONS && verdict.decision != Decision::Accept {
        let Some(hint) = verdict.revised_hint.clone() else { break };
        round += 1;
        if verbose {
            println!("verdict {:?} → revision {round}/{MAX_REVISIONS} (hint: {hint})", verdict.decision);
        }
        // Disclosure, not masking (same policy as the propose fallback above): a
        // transient provider failure in a LATER round must not throw away the
        // already-paid-for, already-VERIFIED recipe from the previous round.
        // Keep that (recipe, verdict) pair — they stay consistent, so the
        // returned verdict still describes the returned recipe — and record why
        // the loop stopped in the rationale, the one channel all three surfaces
        // show (the windowed GUI has no console for the CLI's stderr). A
        // FIRST-round failure still errors: there is no good pair to keep.
        let revised = match openai.propose_planned(
            &preview,
            meta,
            hist,
            &crate::advisor::ProposeContext { hint: Some(&hint), ..propose_ctx },
        ) {
            Ok(r) => r,
            Err(e) => {
                crate::rationale::push_note(
                    &mut recipe.rationale,
                    &mut det_notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::REVISION_FAILED,
                        vec![("round", round.to_string()), ("e", crate::rationale::error_line(&anyhow::Error::new(e)))],
                    ),
                );
                break;
            }
        };
        match verifier.verify(&revised.recipe, meta, hist, &intent) {
            Ok(v) => {
                recipe = revised.recipe;
                // Fresh model prose — the notes described the DISCARDED
                // recipe's tail, so they reset with it (the suffix contract).
                det_notes.clear();
                // …and the working travels with the proposal it explains.
                thinking = revised.thinking;
                lens_opinion = revised.lens;
                verdict = v;
            }
            Err(e) => {
                crate::rationale::push_note(
                    &mut recipe.rationale,
                    &mut det_notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::REVISION_VERIFY_FAILED,
                        vec![("round", round.to_string()), ("e", crate::rationale::error_line(&anyhow::Error::new(e)))],
                    ),
                );
                break;
            }
        }
    }

    // Distill toward the user's historical style: a gentle, capped pull of the
    // global sliders toward similar past edits. The Style axis supplies
    // `style_pull` (0.18 at the shipped 0.3, full at 1.0), so grade strength
    // never silently changes the historical-style blend.
    //
    // DELIBERATELY NOT on the grade-strength axis (R23-3, GATE 5's other half):
    // this Style pull bounds MEAN REGRESSION — how far the proposal is dragged
    // toward the arithmetic mean of `style::style_targets` — and its whole job is
    // to keep the AI's scene-specific reading from being averaged away. Coupling
    // it to strength would mean "push harder" silently became "look MORE like my
    // average past edit", which is the opposite direction on the other axis and
    // exactly the entanglement this round exists to undo. What grade strength changes
    // is the WORDING the model reads about the reference
    // (`style::render_reference`), never the blend arithmetic; the Style slider
    // keeps sole ownership of that number.
    if let Some(r) = &retrieved {
        let pre_blend = recipe.clone();
        crate::style::blend_toward(&mut recipe, &r.targets, crate::style::style_pull(req.style));
        recipe.clamp();
        // The blend mutates the recipe AFTER the rationale was written and
        // AFTER the verdict above. When it actually changed something (a pull
        // toward targets the recipe already matches is a no-op — claiming a
        // pull then would be its own honesty bug), disclose it in the
        // rationale — the derivation numbers must not silently contradict the
        // final sliders (a real verifier flagged exactly that on a heuristic
        // fallback whose "exposure +0.0EV" text sat next to blended
        // -0.06EV/-20/+4 values) — then re-verify the FINAL recipe so the
        // returned verdict honestly reflects what will actually be applied.
        if recipe != pre_blend {
            crate::rationale::push_note(
                &mut recipe.rationale,
                &mut det_notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::STYLE_DISTILLED,
                    vec![("pct", format!("{:.0}", crate::style::style_pull(req.style) * 100.0))],
                ),
            );
            // Degrade like the revision loop above: a transient verifier
            // failure at this LAST step used to error out the whole call,
            // discarding the paid, already-verified proposal. Keep the pair
            // and disclose which recipe the verdict describes.
            match verifier.verify(&recipe, meta, hist, &intent) {
                Ok(v) => verdict = v,
                Err(e) => {
                    crate::rationale::push_note(
                        &mut recipe.rationale,
                        &mut det_notes,
                        crate::rationale::Note::new(
                            crate::rationale::keys::STYLE_REVERIFY_FAILED,
                            vec![("e", crate::rationale::error_line(&anyhow::Error::new(e)))],
                        ),
                    );
                }
            }
        }
    }
    // (The style DISCLOSURE block used to sit here. It moved below the judge
    // — R23-2: an adopted visual revision replaces `recipe` wholesale and
    // clears `det_notes` with it, so every note written here was silently
    // dropped on exactly the runs that changed the most.)
    // R20: the visual CLOSED LOOP — the first eye ever laid on the result.
    // The proposer emits numbers blind (it never sees what they render to)
    // and the verifier judges data-only by contract, so a plausible-but-off
    // develop sailed through both. Render the verified proposal and put it
    // in front of the vision model as a JUDGE; on a revise verdict run ONE
    // guided round, adopted only when the re-judge scores at least as high
    // (do-no-harm — the fit's own rule applied here). Every branch discloses
    // through the rationale; a judge failure degrades, never errors.
    //
    // Placed at the END of the look-mutation chain on purpose (review R20:
    // M1+S1+S2 shared this one upstream cause — the judge used to sit in
    // the middle): the style distillation has already landed, the WB anchor
    // is the hoisted one the deliverable will carry (temperature_k is
    // ABSOLUTE Kelvin against it — an unanchored clone rendered a tungsten
    // shot's correction as identity), and a refine's bitmap masks are
    // carried into the render clone. The judged render IS the look the user
    // receives; only the calibration stamp below remains, and the clone
    // deliberately excludes it — the embedded preview already CONTAINS the
    // base look and lens corrections, so the clone keeps base_curve empty
    // and lens_profile default (the refine-strip rule) while carrying the
    // real WB anchor.
    //
    // COST: paid vision calls (1 judge; +1 propose, +1 verify, +1 judge per
    // guided round bought). `judge` is therefore an explicit caller
    // decision: interactive analyze passes true (this closed loop IS the
    // R20 strengthening); batch and eval pass false — a 500-photo batch
    // must not silently multiply spend, and eval measures the RAW proposal
    // (review R20-M2/M3).
    //
    // R23-4 turns the single round into a bounded CONVERGENCE loop
    // (`judge_convergence` / `judge_next_round`): up to 3 rounds, and only
    // where the photographer asked for depth or for a bold grade — every
    // other path keeps exactly the one round R20 shipped. The three R20
    // decisions hold PER ROUND, not once: the fee is the caller's explicit
    // choice (this same `judge` gate), a revision is adopted only when the
    // re-judge holds the score (`visual_review_round`'s `>=`), and an
    // identical revision short-circuits before paying for a verify or a
    // re-judge (that check lives inside `visual_review_round`, so it fires
    // on every round by construction). Worst case for one analyze: 11 API
    // calls today (6 carrying images, 8 high-detail frames), 17 with deep
    // thinking at a committed strength (10 with images, 14 frames) — the
    // numbers the GUI tooltip and `--deep`'s help quote.
    if judge && can_revise {
        let judge_view = |r: &EditRecipe, lens: LensOpinion| -> EditRecipe {
            let mut v = r.clone();
            if let Some(b) = base {
                // A refine's bitmap masks are part of the look under
                // judgement (the response schema cannot return them; the
                // deliverable gets them re-attached below) — without this
                // the judge saw the user's sky mask missing and bought a
                // revision to fix what was already fixed (review R20-S1).
                // The lens opinion rides along for the same reason: the judge
                // must score the frame the deliverable will BE, and since
                // R23-1b those two differ whenever the model stated a lens
                // value (it would otherwise be judged with the base's).
                carry_over_unrepresentable(&mut v, b, lens, None);
            }
            v.base_curve = Vec::new();
            v.lens_profile = Default::default();
            v.as_shot_k = anchor_k;
            v.as_shot_tint = anchor_tint;
            v
        };
        let judge_of = |r: &EditRecipe,
                        lens: LensOpinion|
         -> Result<crate::advisor::Judgement, crate::advisor::AdvisorError> {
            let rendered = crate::render::develop_preview(&preview_img, &judge_view(r, lens));
            let mut jpeg = Vec::new();
            rendered
                .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
                .map_err(|e| {
                    crate::advisor::AdvisorError::Io(std::io::Error::other(format!(
                        "encode judge render JPEG: {e}"
                    )))
                })?;
            crate::advisor::judge_pair(
                cfg,
                crate::advisor::JudgeImages { reference: &preview.jpeg, candidate: &jpeg },
                crate::advisor::JudgeTask::Develop,
                // Oversize context is OMITTED, not truncated: a cut JSON is
                // a malformed blob to the judge (review R20-N5); judge_pair's
                // own 16 KiB bound stays as the backstop.
                serde_json::to_string(r)
                    .ok()
                    .filter(|s| s.len() <= 16 * 1024)
                    .as_deref(),
                // GATE 4: the judge can BUY a revision, so a rubric that does
                // not know the target strength does not merely mis-report — it
                // re-timid-ifies the develop the user asked to push.
                Some(intent),
            )
        };
        // R23-4: how far this analysis may iterate, from ONE pure decision
        // (`judge_convergence`) — the target the rubric already stated in
        // words, and the round cap that keeps it bounded.
        let (target, cap) = judge_convergence(req.strength, req.think);
        // Every note this block writes, HELD until the loop ends. An adopted
        // round replaces `recipe` wholesale and clears `det_notes` with it (the
        // rationale-suffix contract), so a note pushed mid-loop would be
        // dropped by the very round it describes — the same defect R23-2 fixed
        // for the style block by moving it below the judge. Flushed onto the
        // FINAL recipe in order, which on the single-round path is
        // byte-identical to the old inline pushes (nothing is ever dropped
        // there).
        let mut log: Vec<crate::rationale::Note> = Vec::new();
        // The CANDIDATE's lens opinion, written by the propose closure and read
        // by the judge closure inside the same `visual_review_round` call. A
        // `Cell` because those two closures are alive together (one `FnMut`, one
        // `Fn`) and a plain `&mut` capture in the first would forbid the read in
        // the second; `LensOpinion` is `Copy`, so this is a plain slot, not
        // shared mutability of anything the round can observe out of order — the
        // round always proposes before it judges.
        let candidate_lens = std::cell::Cell::new(LensOpinion::default());
        match judge_of(&recipe, lens_opinion) {
            Err(e) => {
                log.push(crate::rationale::Note::new(
                    crate::rationale::keys::JUDGE_UNAVAILABLE,
                    vec![("e", crate::rationale::error_line(&anyhow::Error::new(e)))],
                ));
            }
            Ok(first) => {
                if verbose {
                    println!(
                        "judge    : {:.0}/100 {:?} — {}",
                        first.score, first.decision, first.critique
                    );
                }
                // The judgement describing the recipe currently held, and how
                // many guided rounds have been paid for.
                let mut current = first;
                let mut rounds_done = 0usize;
                loop {
                    let score1 = format!("{:.0}", current.score);
                    // R20's rule owns round 1 (a non-Accept verdict WITH an
                    // instruction); the target and the cap own every round
                    // after it. See `judge_next_round`.
                    let Some(hint) = judge_next_round(rounds_done, &current, target, cap) else {
                        log.push(crate::rationale::Note::new(
                            crate::rationale::keys::JUDGE_SCORE,
                            vec![("score", score1), ("critique", current.critique.clone())],
                        ));
                        break;
                    };
                    // Short prefix: the whole hint is bounded at 1024 in
                    // the proposer, so every prefix byte comes out of the
                    // judge's own instruction (review R20-N2).
                    let h = format!("visual judge (it SAW your previous render): {hint}");
                    if verbose {
                        println!(
                            "judge    : guided revision {}/{cap} (hint: {h})",
                            rounds_done + 1
                        );
                    }
                    let mut candidate_distilled = false;
                    let mut candidate_thinking: Option<crate::advisor::Thinking> = None;
                    let outcome = visual_review_round(
                        &h,
                        &recipe,
                        |h| {
                            let p = openai.propose_planned(
                                &preview,
                                meta,
                                hist,
                                &crate::advisor::ProposeContext {
                                    hint: Some(h),
                                    ..propose_ctx
                                },
                            )?;
                            let mut r = p.recipe;
                            candidate_thinking = p.thinking;
                            candidate_lens.set(p.lens);
                            // The candidate walks the SAME look chain the
                            // original walked (style distillation above)
                            // — otherwise adopting it would silently
                            // undo the user's style pull (review R20-S2).
                            if let Some(sr) = &retrieved {
                                let pre = r.clone();
                                crate::style::blend_toward(
                                    &mut r,
                                    &sr.targets,
                                    crate::style::style_pull(req.style),
                                );
                                r.clamp();
                                candidate_distilled = r != pre;
                            }
                            Ok(r)
                        },
                        |r| verifier.verify(r, meta, hist, &intent),
                        // The candidate's own opinion: `visual_review_round`
                        // judges only the recipe its propose closure just
                        // returned (`judge(&revised)`), never the held one.
                        |r| judge_of(r, candidate_lens.get()),
                        current.score,
                    );
                    rounds_done += 1;
                    match outcome {
                        VisualRound::Adopted { recipe: r2, verdict: v2, second } => {
                            recipe = *r2;
                            // Fresh model prose — the notes described the
                            // DISCARDED recipe's tail (the suffix contract).
                            det_notes.clear();
                            // …and the working travels with its proposal.
                            thinking = candidate_thinking;
                            lens_opinion = candidate_lens.get();
                            verdict = v2;
                            if verbose {
                                println!(
                                    "judge    : re-scored {:.0}/100 — adopted",
                                    second.score
                                );
                            }
                            let score2 = format!("{:.0}", second.score);
                            // Look ahead with the SAME predicate the loop
                            // head uses: an adoption that is the last word
                            // keeps R20's terminal note (byte-identical on
                            // the single-round path), while one the loop will
                            // iterate past logs the round and moves on.
                            let more =
                                judge_next_round(rounds_done, &second, target, cap).is_some();
                            log.push(crate::rationale::Note::new(
                                if more {
                                    crate::rationale::keys::JUDGE_ROUND
                                } else {
                                    crate::rationale::keys::JUDGE_ADOPTED
                                },
                                if more {
                                    vec![
                                        ("round", rounds_done.to_string()),
                                        ("score1", score1),
                                        ("score2", score2),
                                        ("target", format!("{target:.0}")),
                                    ]
                                } else {
                                    vec![
                                        ("score1", score1),
                                        ("score2", score2),
                                        ("critique", second.critique.clone()),
                                    ]
                                },
                            ));
                            if candidate_distilled {
                                log.push(crate::rationale::Note::new(
                                    crate::rationale::keys::STYLE_DISTILLED,
                                    vec![(
                                        "pct",
                                        format!(
                                            "{:.0}",
                                            crate::style::style_pull(req.style) * 100.0
                                        ),
                                    )],
                                ));
                            }
                            if !more {
                                break;
                            }
                            current = second;
                        }
                        VisualRound::Unchanged => {
                            // The model returned the SAME settings: another
                            // round would ask the same question of the same
                            // recipe and pay for the same answer. (R20 定案 3
                            // — the short-circuit lives inside
                            // `visual_review_round`, so it fires on EVERY
                            // round, not just the first.)
                            log.push(crate::rationale::Note::new(
                                crate::rationale::keys::JUDGE_UNCHANGED,
                                vec![("score", score1), ("critique", current.critique.clone())],
                            ));
                            break;
                        }
                        VisualRound::KeptLower { second_score } => {
                            if verbose {
                                println!(
                                    "judge    : re-scored {second_score:.0}/100 — lower, revision discarded"
                                );
                            }
                            log.push(crate::rationale::Note::new(
                                crate::rationale::keys::JUDGE_KEPT,
                                vec![
                                    ("score1", score1),
                                    ("critique", current.critique.clone()),
                                    ("score2", format!("{second_score:.0}")),
                                ],
                            ));
                            break;
                        }
                        VisualRound::RoundFailed { e } => {
                            log.push(crate::rationale::Note::new(
                                crate::rationale::keys::JUDGE_ROUND_FAILED,
                                vec![
                                    ("score", score1),
                                    ("critique", current.critique.clone()),
                                    ("e", crate::rationale::error_line(&anyhow::anyhow!("{}", e))),
                                ],
                            ));
                            break;
                        }
                        VisualRound::RejudgeFailed { e } => {
                            log.push(crate::rationale::Note::new(
                                crate::rationale::keys::JUDGE_REJUDGE_FAILED,
                                vec![
                                    ("score", score1),
                                    ("critique", current.critique.clone()),
                                    ("e", crate::rationale::error_line(&anyhow::anyhow!("{}", e))),
                                ],
                            ));
                            break;
                        }
                    }
                }
            }
        }
        // Onto the recipe that SURVIVED the loop.
        for note in log {
            crate::rationale::push_note(&mut recipe.rationale, &mut det_notes, note);
        }
    }

    // ── R23-4 THINKING disclosure, same placement and same reason ──────────
    // The three single-sentence fields of the working that belongs to the
    // recipe actually being returned. Bounded at the trust boundary already
    // (advisor::THINK_FIELD_MAX_BYTES), so what lands here is three sentences,
    // not a transcript — the plan's per-family reasoning is deliberately NOT in
    // the rationale (it is 9 clauses long, and this string is capped and
    // reprinted on five surfaces); the CLI prints it in full below.
    if let Some(t) = &thinking {
        for (key, arg, text) in [
            (crate::rationale::keys::THINK_SCENE, "scene", &t.scene),
            (crate::rationale::keys::THINK_LOOK, "look", &t.intended_look),
            (crate::rationale::keys::THINK_CRITIQUE, "critique", &t.self_critique),
        ] {
            if !text.is_empty() {
                crate::rationale::push_note(
                    &mut recipe.rationale,
                    &mut det_notes,
                    crate::rationale::Note::new(key, vec![(arg, text.clone())]),
                );
            }
        }
        // R23-1b: the PIXEL-tool suggestions, on the SAME channel and bounded
        // the same way (≤3 entries, each `why` already capped at the trust
        // boundary). One line, because that is all this is: the model naming
        // work the develop cannot do. Nothing is executed and no parameters are
        // implied — a button here would read as "the AI has this configured",
        // and R20 settled that a paid, destructive operation is the explicit
        // caller's decision.
        if !t.pixel_tools.is_empty() {
            let tools = t
                .pixel_tools
                .iter()
                .map(|s| {
                    if s.why.is_empty() {
                        s.tool.wire().to_string()
                    } else {
                        format!("{} ({})", s.tool.wire(), s.why)
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            crate::rationale::push_note(
                &mut recipe.rationale,
                &mut det_notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::PIXEL_TOOLS,
                    vec![("tools", tools)],
                ),
            );
        }
        if verbose && !t.tool_plan.is_empty() {
            println!("plan     : the model's tool plan for this photo");
            for step in &t.tool_plan {
                println!(
                    "  {} {:<14} {}",
                    if step.used { "USE " } else { "skip" },
                    step.control,
                    step.why
                );
            }
        }
    }

    // ── R23-2 style DISCLOSURE, after every wholesale recipe replacement ──
    // The one channel all three surfaces show. Three facts, in reading order:
    // which of the user's shots this develop leaned on (feedback #6's headline
    // — "I have no idea which library it is referencing"), whether a reference
    // PHOTO went along, and whether the whole mechanism came up empty.
    if let Some(note) = retrieved.as_ref().and_then(|r| style_neighbours_note(&r.stems)) {
        crate::rationale::push_note(&mut recipe.rationale, &mut det_notes, note);
    }
    if let Some(file) = &ref_image_stem {
        crate::rationale::push_note(
            &mut recipe.rationale,
            &mut det_notes,
            crate::rationale::Note::new(
                crate::rationale::keys::STYLE_REF_IMAGE,
                vec![("file", file.clone())],
            ),
        );
    }
    if let Some(e) = &ref_image_err {
        crate::rationale::push_note(
            &mut recipe.rationale,
            &mut det_notes,
            crate::rationale::Note::new(
                crate::rationale::keys::STYLE_REF_IMAGE_FAILED,
                vec![("e", e.clone())],
            ),
        );
    }
    if let Some(r) = &retrieved {
        if let Some(stem) = &r.look_stem {
            crate::rationale::push_note(
                &mut recipe.rationale,
                &mut det_notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::STYLE_LOOK_REFERENCE,
                    vec![
                        ("stem", stem.clone()),
                        ("tags", r.look_tags.join(", ")),
                    ],
                ),
            );
        }
        if r.looks_unreachable {
            crate::rationale::push_note(
                &mut recipe.rationale,
                &mut det_notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::STYLE_LOOKS_UNREACHABLE,
                    vec![("n", r.looks_count.to_string())],
                ),
            );
        }
    }
    if let Some(stem) = &ref_image_stem && ref_image_is_look {
            crate::rationale::push_note(
                &mut recipe.rationale,
                &mut det_notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::STYLE_LOOK_IMAGE,
                    vec![("stem", stem.clone())],
                ),
            );
    }
    if let Some(tier) = direction_adherence_tier(user_direction, req.adherence) {
        crate::rationale::push_note(
            &mut recipe.rationale,
            &mut det_notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ADVISOR_NOTE_DIRECTION_ADHERENCE,
                vec![("tier", tier.into())],
            ),
        );
    }
    if let Some(note) = style_gap_note(req.style, ref_str, style_err.as_deref()) {
        crate::rationale::push_note(&mut recipe.rationale, &mut det_notes, note);
    }

    // Base look, stamped in ONE place for every surface: the proposal and the
    // verification above both ran over the camera's embedded preview — the
    // very base the curve approximates — so the AI's JSON round-trip never
    // decides it. A saved recipe.json owns its curve verbatim (a legacy save
    // must keep rendering as it was tuned); otherwise a fresh estimate.
    // Without this, a CLI-written analyze recipe carried an empty curve and
    // the open-time "recipe.json keeps its saved curve" rule then pinned the
    // dark pre-base-look rendering onto that photo forever.
    //
    // Its OWN read of the saved recipe (the WB anchor above took an earlier one
    // because the prompt needed it): this stamp is the snapshot's only consumer,
    // so it stays as late — and as fresh — as it has always been.
    match saved_recipe_snapshot(raw) {
        Some(saved) => {
            // The era stamp travels WITH the curve (the paste rule): the
            // proposer's recipe is era-2 by Default — or whatever integer the
            // model chose to emit, since the strict schema forces the field —
            // and stamping a saved era-1 curve under either laundered it past
            // every repair permanently, from the very writer (Analyze) that
            // produces the canonical recipe.json.
            recipe.version = saved.version;
            recipe.base_curve = saved.base_curve;
            recipe.lens_profile = saved.lens_profile;
        }
        None => {
            // A fresh estimate by THIS build's sampler — and the stamp is
            // authored here, never by the model: version is provenance, and
            // the response schema makes the model emit SOMETHING for it.
            recipe.version = crate::recipe::CALIB_ERA;
            recipe.base_curve = photo_base_knots(raw);
            recipe.lens_profile = fresh_lens_profile(raw);
        }
    }
    // The as-shot WB anchor is the third calibration half — same saved-first
    // rule, via the ONE early snapshot (the prompt named these very values and
    // the judge above rendered against them, so the anchor the model was told,
    // the anchor it was scored on and the anchor delivered are all one).
    recipe.as_shot_k = anchor_k;
    recipe.as_shot_tint = anchor_tint;
    // REFINE means "adjust MY edit", so it must not delete work the model was
    // never able to return. The strict response schema
    // (advisor::catalogue::edit_recipe_schema) can express only LINEAR and RADIAL
    // primary geometry; it carries no components, no enabled toggle, no colour
    // gains and no mask role — so a round-trip silently dropped every bitmap
    // mask (AI-selected sky/subject, painted, reverse-fit zones) with its
    // recolour gains. The result then auto-saves. Carry those back from the base
    // the user actually had — and, for the three manual lens fields the schema
    // DOES carry since R23-1b, keep the photographer's value only where this
    // proposal said nothing about them.
    if let Some(b) = base {
        carry_over_unrepresentable(&mut recipe, b, lens_opinion, Some(&mut det_notes));
    }
    Ok((recipe, verdict, det_notes))
}

/// "These are the shots it referenced" — the transparency half of feedback #6
/// (R23-2). `None` when nothing was retrieved: the gap note below covers that
/// case, and a "referenced 0 shots" line would be noise.
///
/// A pure function beside [`style_gap_note`] for the same reason: the
/// production path reaches it only after minutes of paid network work.
pub(crate) fn style_neighbours_note(stems: &[String]) -> Option<crate::rationale::Note> {
    if stems.is_empty() {
        return None;
    }
    Some(crate::rationale::Note::new(
        crate::rationale::keys::STYLE_NEIGHBOURS,
        vec![("files", stems.join(", ")), ("n", stems.len().to_string())],
    ))
}

/// "This develop asked for personal style and got NOTHING" — the disclosure
/// that closes feedback #6's three silent arms with ONE condition (R23-2).
///
/// The old test was "an index FILE exists and failed to load", which is why
/// only one of the three arms ever said anything:
///   a) no index file at all (fresh install)     → silent, and the GUI had no
///      way to build one either;
///   b) the index loaded but `retrieve` matched nothing, so `render_reference`
///      returned `None` (`style.rs`)              → silent;
///   c) an unusable file (version gate, corrupt) → the only arm that spoke.
/// Keying off the FINAL reference covers all three: (a) and (b) are the same
/// user-visible fact ("no reference for this photo") and share a note, while
/// (c) keeps the loader's own message, which names what to fix.
///
/// A pure function so all three arms are testable without a paid call — the
/// production path reaches this only after minutes of network work.
pub(crate) fn style_gap_note(
    strength: f32,
    reference: Option<&str>,
    load_err: Option<&str>,
) -> Option<crate::rationale::Note> {
    // Nothing was asked for, or the reference arrived: nothing to disclose.
    // (A claim about a mechanism the user switched off is its own honesty bug.)
    if strength <= 0.0 || reference.is_some() {
        return None;
    }
    Some(match load_err {
        Some(e) => crate::rationale::Note::new(
            crate::rationale::keys::STYLE_UNAVAILABLE,
            vec![("e", crate::rationale::error_line(&anyhow::anyhow!("{}", e)))],
        ),
        None => crate::rationale::Note::new(
            crate::rationale::keys::STYLE_NO_REFERENCE,
            vec![("pct", format!("{:.0}", strength.clamp(0.0, 1.0) * 100.0))],
        ),
    })
}

// ── judge CONVERGENCE (R23-4, feedback #13) ────────────────────────────────
//
// R20 shipped a loop that could only ask "did that make it worse?"
// (`visual_review_round`'s do-no-harm `>=`). The judge's rubric has stated an
// ABSOLUTE scale since day one — `judge.rs`: 90+ ship as-is, 75-89 good with
// minor polish left, 50-74 clearly improvable — and nothing read it, so a
// develop scored 62 and stopped there as contentedly as one scored 91.
//
// These are the three target scores that scale, one per strength band. They are
// TASTE constants, not measurements: named and adjacent so a retune is one edit
// with the whole ladder visible, and pinned by
// `the_convergence_ladder_is_gated_and_ordered`.
const JUDGE_TARGET_RESTRAINED: f32 = 78.0;
const JUDGE_TARGET_BALANCED: f32 = 82.0;
const JUDGE_TARGET_COMMITTED: f32 = 88.0;

/// The score this analysis iterates toward, and the CAP on guided rounds.
///
/// Both scale with the strength axis: a restrained develop is finished sooner,
/// by definition, than one the photographer asked to commit. The cap is GATED,
/// though — R20's first定案 is that a paid visual round is an explicit caller
/// decision, and multiplying rounds multiplies that spend, so anything past the
/// historical single round needs the user to have asked for depth (`think`) or
/// for a bold grade (the committed band). Everything else keeps exactly the
/// round budget every release since R20 has had.
///
/// The gate is a DISJUNCTION and every cost disclosure has to read that way
/// (R23 review MED-2): `think` alone raises the ceiling, and a Strength above
/// 70% alone raises it too, with no box ticked — the worst case of 17 calls
/// (10 carrying images) belongs to EITHER, not to both together. The GUI's
/// three tooltips and the CLI's `--strength` help all say it disjunctively for
/// that reason.
///
/// `judge = false` (batch, eval) never reaches this: the whole block is behind
/// that gate, so the unattended surfaces cannot iterate at all.
pub(crate) fn judge_convergence(strength: GradeStrength, think: bool) -> (f32, usize) {
    let (target, cap) = match strength.tier() {
        StrengthTier::Restrained => (JUDGE_TARGET_RESTRAINED, 1),
        StrengthTier::Balanced => (JUDGE_TARGET_BALANCED, 2),
        StrengthTier::Committed => (JUDGE_TARGET_COMMITTED, 3),
    };
    let deep = think || strength.tier() == StrengthTier::Committed;
    (target, if deep { cap } else { 1 })
}

/// May the judge buy ANOTHER guided round, and with what instruction?
///
/// `Some(hint)` runs a round; `None` means the reviewed develop stands. The
/// FIRST round's test is R20's, unchanged and unconditional: a non-Accept
/// verdict carrying an instruction. `target` deliberately does NOT apply at
/// `rounds_done == 0` — a default-path analysis must behave exactly as it did
/// before R23-4 whatever the target says, and suppressing the historical round
/// because a 79 already cleared 78 would be a silent behaviour change dressed
/// up as convergence. From the second round on the target IS the stop
/// condition, so the loop finally knows the difference between "not worse" and
/// "good enough".
pub(crate) fn judge_next_round(
    rounds_done: usize,
    j: &crate::advisor::Judgement,
    target: f32,
    cap: usize,
) -> Option<&str> {
    if rounds_done >= cap || (rounds_done > 0 && j.score >= target) {
        return None;
    }
    // An Accept's hint (if any) is advice, not a request — only a
    // revise/reject verdict WITH an instruction buys the round.
    if j.decision == Decision::Accept {
        return None;
    }
    j.hint.as_deref().map(str::trim).filter(|h| !h.is_empty())
}

/// Outcome of one judge-guided visual revision round (R20 closed loop).
pub(crate) enum VisualRound {
    /// The revision re-scored at least as high — it replaces the recipe,
    /// and `verdict` is the data verifier's fresh word on it.
    Adopted {
        recipe: Box<EditRecipe>,
        verdict: Verdict,
        second: crate::advisor::Judgement,
    },
    /// The revision came back with IDENTICAL settings — nothing to verify or
    /// re-judge (two paid calls saved), and no "was adopted" claim for a
    /// recipe that never changed (review R20-S3).
    Unchanged,
    /// The revision re-scored LOWER — discarded (do-no-harm).
    KeptLower { second_score: f32 },
    /// Propose or verify failed — nothing to compare, keep the reviewed recipe.
    RoundFailed { e: String },
    /// The revision exists but could not be re-judged — an UNCOMPARED swap
    /// would gamble the reviewed recipe on it, so it is discarded.
    RejudgeFailed { e: String },
}

/// ONE guided revision, adopt-or-keep. Factored over closures so the
/// contract — a revision must re-judge AT LEAST AS HIGH to replace the
/// reviewed recipe, an identical revision short-circuits, and every failure
/// keeps the reviewed recipe — is testable without a network (the three
/// steps are remote in production).
pub(crate) fn visual_review_round(
    hint: &str,
    current: &EditRecipe,
    propose: impl FnOnce(&str) -> Result<EditRecipe, crate::advisor::AdvisorError>,
    verify: impl FnOnce(&EditRecipe) -> Result<Verdict, crate::advisor::AdvisorError>,
    judge: impl FnOnce(&EditRecipe) -> Result<crate::advisor::Judgement, crate::advisor::AdvisorError>,
    first_score: f32,
) -> VisualRound {
    let revised = match propose(hint) {
        Ok(r) => r,
        Err(e) => return VisualRound::RoundFailed { e: e.to_string() },
    };
    // SETTINGS equality, not struct equality: the model's prose (rationale,
    // confidence) differs every call, so raw == would never fire — the no-op
    // being caught is "same sliders/curves/masks", the render-identical case.
    let strip = |r: &EditRecipe| {
        let mut c = r.clone();
        c.rationale = String::new();
        c.confidence = 0.0;
        c
    };
    if strip(&revised) == strip(current) {
        return VisualRound::Unchanged;
    }
    let v2 = match verify(&revised) {
        Ok(v) => v,
        Err(e) => return VisualRound::RoundFailed { e: e.to_string() },
    };
    match judge(&revised) {
        // >= not >: at EQUAL scores the revision wins because the judge asked
        // for it — the hint was followed and nothing was lost.
        Ok(second) if second.score >= first_score => VisualRound::Adopted {
            recipe: Box::new(revised),
            verdict: v2,
            second,
        },
        Ok(second) => VisualRound::KeptLower { second_score: second.score },
        Err(e) => VisualRound::RejudgeFailed { e: e.to_string() },
    }
}

/// Re-attach what the AI's response schema CANNOT express, from the refine
/// base the photographer actually had.
///
/// `advisor::catalogue::edit_recipe_schema` can encode only LINEAR and RADIAL
/// primary geometry; it carries no components, no enabled toggle, no colour
/// gains and no mask role. That loss list is the `engine_only` column of
/// `advisor::catalogue::LOCAL_CONTROLS` / `RECIPE_CONTROLS` plus the Bitmap
/// geometry the schema omits — re-checked at R23-1b, which SHRANK it: the
/// radial `angle` and the three manual lens fields are in the schema now, so
/// neither is carried blindly any more (the lens trio is settled by `lens`
/// below, and a rotated ellipse is simply returned). A missing bitmap mask in
/// the response carries NO intent — the model had no way to return one — yet
/// the refined recipe auto-saves, so every AI-selected sky/subject mask,
/// painted mask and reverse-fit zone (with its recolour gains) silently
/// disappeared the moment the user clicked Refine.
///
/// `lens` is the model's OPINION on the three manual lens controls (R23-1b).
/// The overwrite below used to be unconditional, with a sound reason at the
/// time — the model never saw those fields, so its zeros meant nothing. Now
/// that it can state them, an unconditional overwrite would make the schema
/// addition a no-op on exactly the path (Refine) where a lens correction is
/// most likely to exist. `LensOpinion::default()` (nothing stated) reproduces
/// the historical behaviour exactly, which is what every non-schema proposal
/// passes.
pub(crate) fn carry_over_unrepresentable(
    recipe: &mut EditRecipe,
    base: &EditRecipe,
    lens: LensOpinion,
    notes: Option<&mut Vec<crate::rationale::Note>>,
) {
    use crate::recipe::{MaskGeometry, MaskRole};

    let schema_loses = |m: &crate::recipe::LocalAdjustment| {
        matches!(&m.mask, MaskGeometry::Bitmap { .. })
            || !m.components.is_empty()
            || !m.enabled
            || m.color_gains.is_some()
            || m.role != MaskRole::Custom
    };

    let mut carried_indices = Vec::new();
    let mut unmatched = false;
    for (base_index, original) in base.masks.iter().enumerate().filter(|(_, m)| schema_loses(m)) {
        let returned_matches: Vec<usize> = recipe
            .masks
            .iter()
            .enumerate()
            .filter_map(|(i, m)| (m.name == original.name).then_some(i))
            .collect();
        let base_name_is_unique = !original.name.is_empty()
            && base.masks.iter().filter(|m| m.name == original.name).count() == 1;

        if base_name_is_unique && returned_matches.len() == 1 {
            let refined = &mut recipe.masks[returned_matches[0]];
            // Only the raster geometry is un-returnable now: the model can
            // state a radial `angle` (R23-1b), so a rotated ellipse it sends
            // back IS its answer and re-imposing the base's rotation would
            // ignore it.
            if matches!(&original.mask, MaskGeometry::Bitmap { .. }) {
                refined.mask = original.mask.clone();
            }
            refined.components = original.components.clone();
            refined.enabled = original.enabled;
            refined.color_gains = original.color_gains;
            refined.role = original.role;
        } else if matches!(&original.mask, MaskGeometry::Bitmap { .. })
            && original.name.is_empty()
        {
            // No schema response can be a round-trip copy of an unnamed Bitmap
            // selection, so preserve the existing prepend behaviour.
            carried_indices.push(base_index);
        } else {
            // A state-bearing mask the response did not identifiably return.
            // The returned list may hold its RENAMED refined copy — keeping
            // both applies the coverage twice, and a same-name discard only
            // covered the obedient half: a renamed copy sailed through it.
            // No guess is safe, so the wholesale fallback below takes over.
            unmatched = true;
        }
    }

    if unmatched {
        // Conservative wholesale fallback: the base masks stand unchanged and
        // the response's MASK edits are discarded (its global refinements
        // stay). The prompt pins mask names, so this is the disobedient-
        // response path — and the rationale says what happened, because a
        // silent revert here reads as "the model ignored my masks".
        recipe.masks = base.masks.clone();
        let note = crate::rationale::Note::plain(crate::rationale::keys::MASKS_NOT_PRESERVED);
        recipe.rationale.push_str(&crate::rationale::render_one(&note));
        if let Some(v) = notes
            && v.len() < crate::rationale::MAX_NOTES
        {
            v.push(note);
        }
    } else {
        carried_indices.sort_unstable();
        carried_indices.dedup();
        if !carried_indices.is_empty() {
            let proposed = std::mem::take(&mut recipe.masks);
            recipe.masks =
                carried_indices.into_iter().map(|i| base.masks[i].clone()).collect();
            recipe.masks.extend(proposed);
        }
    }
    // Manual lens corrections are geometry the photographer dialled in, and a
    // response that says NOTHING about them (null, or a proposer with no such
    // field at all) must not re-warp the frame by defaulting them. A response
    // that DOES state one is answering the question it was asked — the
    // catalogue tells it null means "no opinion" and 0 means "zero it" — so
    // that value stands.
    if !lens.distortion {
        recipe.lens_distortion = base.lens_distortion;
    }
    if !lens.vignette {
        recipe.lens_vignette = base.lens_vignette;
    }
    if !lens.vignette_mid {
        recipe.lens_vignette_mid = base.lens_vignette_mid;
    }
    // The lens PROFILE (with the user's toggles) rides too — but only when
    // the base actually carries one: a refine caller that still pre-strips
    // it to Default is sending a sentinel, never "the user turned everything
    // off" (a real profile keeps its in-camera data vectors even with every
    // toggle off, and a data-less photo's profile equals Default on both
    // sides). Without this, Refine reverted unsaved lens toggles to the
    // saved profile stamped above.
    if base.lens_profile != Default::default() {
        recipe.lens_profile = base.lens_profile.clone();
    }
    // The nine CARRIED effects (R25 B2) ride too, and they have no other way
    // home: they are `engine_only`, so the response schema cannot restate
    // them, and unlike the calibration nothing re-stamps them per photo. A
    // refine that dropped them would strip the photographer's imported
    // Lightroom grain / post-crop vignette from the recipe AND — because we
    // own those keys now, so the merge strips before rewriting — from the
    // sidecar beside the RAW on the very next save. That is data loss, not a
    // missing feature. `carried_effects_survive_a_refine` re-derives the field
    // list from `Tier::CarriedOnly`, so the B3/B4 rows fail loudly here until
    // this block is widened with them.
    recipe.post_crop_vignette = base.post_crop_vignette;
    recipe.post_crop_vignette_mid = base.post_crop_vignette_mid;
    recipe.post_crop_vignette_feather = base.post_crop_vignette_feather;
    recipe.post_crop_vignette_round = base.post_crop_vignette_round;
    recipe.post_crop_vignette_style = base.post_crop_vignette_style;
    recipe.post_crop_vignette_hl = base.post_crop_vignette_hl;
    recipe.grain = base.grain;
    recipe.grain_size = base.grain_size;
    recipe.grain_rough = base.grain_rough;
    // R25 B3 widened the block by fifteen: the eight carried detail axes, the
    // auto-CA switch and the six de-fringe keys. Same argument, one sharper
    // edge — de-fringe's neutral is ADOBE'S DEFAULT, not zero, so a dropped
    // hue window would not merely lose a value, it would write 0/0 into the
    // sidecar and change what Lightroom renders.
    recipe.sharpen_radius = base.sharpen_radius;
    recipe.sharpen_detail = base.sharpen_detail;
    recipe.sharpen_mask = base.sharpen_mask;
    recipe.nr_detail = base.nr_detail;
    recipe.nr_contrast = base.nr_contrast;
    recipe.color_nr = base.color_nr;
    recipe.color_nr_detail = base.color_nr_detail;
    recipe.color_nr_smooth = base.color_nr_smooth;
    recipe.auto_lateral_ca = base.auto_lateral_ca;
    recipe.defringe_purple = base.defringe_purple;
    recipe.defringe_purple_lo = base.defringe_purple_lo;
    recipe.defringe_purple_hi = base.defringe_purple_hi;
    recipe.defringe_green = base.defringe_green;
    recipe.defringe_green_lo = base.defringe_green_lo;
    recipe.defringe_green_hi = base.defringe_green_hi;
    // The manual CA pair rides too, and it is the ONE pair here that RENDERS.
    // It is `engine_only` for a reason of its own (catalogue.rs: a 1–3 px
    // edge artefact the advisor's ~1024 px preview cannot show), which means
    // the schema never asks for it and it has no other way home — exactly the
    // carried case, minus the tier. `carried_effects_survive_a_refine` covers
    // it explicitly, since the derivation there is by `Tier::CarriedOnly`.
    recipe.ca_r = base.ca_r;
    recipe.ca_b = base.ca_b;
    // R25 B4: the Transform / Calibration pass-through block rides for the
    // SAME reason and with the sharpest edge of all. It is `EngineCarrier`, so
    // no response can restate it; we now OWN its sixteen keys, so the next
    // save's merge strips them out of the user's sidecar before rewriting —
    // and the writer emits only what the map holds. A Refine that dropped this
    // map would therefore delete the photographer's Upright correction and
    // camera profile from the file beside their RAW, permanently, without a
    // single control having moved. `carried_effects_survive_a_refine` derives
    // its field list from the tiers that render nothing, so this line is
    // required to exist by the same test the B2/B3 blocks answer to.
    recipe.passthrough = base.passthrough.clone();
    carry_radial_carried_attributes(recipe, base);
    recipe.clamp(); // the size caps still apply after re-attaching
}

/// `crs:Midpoint` and `crs:Version` ride home from the refine base (R27 L-09).
///
/// The two are CARRIED-ONLY radial attributes: Lightroom writes them on every
/// radial, this engine never consults them, and `advisor::catalogue`'s mask
/// geometry schema deliberately omits them — a model cannot know a value it
/// never saw, and a required property is a property it would invent. The cost
/// was that a Refine returned the geometry with both at their serde defaults
/// (50 / 2), so refining an imported Lightroom radial silently rewrote the
/// photographer's Midpoint on the very next save.
///
/// R25 registered this as 「not worth fixing」 on the ground that the fix meant
/// widening `schema_loses` above — the state-bearing predicate that decides
/// which masks take the WHOLESALE-REVERT path. That reasoning stands, so this
/// does not touch it: it is a separate pass over the matched masks, it can
/// neither set `unmatched` nor change which masks are carried, and a base
/// whose masks the response did not identifiably return has already had them
/// restored verbatim by then (making this a self-copy).
///
/// Matching is the SAME rule the block above uses — the base name is unique
/// and exactly one returned mask answers to it — and only Radial → Radial
/// copies: a response that sent back a different geometry KIND is a new shape,
/// with no midpoint of the old one to inherit.
fn carry_radial_carried_attributes(recipe: &mut EditRecipe, base: &EditRecipe) {
    use crate::recipe::MaskGeometry;
    let carried = |g: &MaskGeometry| match g {
        MaskGeometry::Radial { midpoint, mask_version, .. } => Some((*midpoint, *mask_version)),
        _ => None,
    };
    for original in base.masks.iter() {
        let Some((midpoint, mask_version)) = carried(&original.mask) else { continue };
        if original.name.is_empty()
            || base.masks.iter().filter(|m| m.name == original.name).count() != 1
        {
            continue;
        }
        let answers: Vec<usize> = recipe
            .masks
            .iter()
            .enumerate()
            .filter_map(|(i, m)| (m.name == original.name).then_some(i))
            .collect();
        let [i] = answers[..] else { continue };
        if let MaskGeometry::Radial { midpoint: mp, mask_version: mv, .. } =
            &mut recipe.masks[i].mask
        {
            *mp = midpoint;
            *mv = mask_version;
        }
    }
}

/// Does this saved curve carry the fingerprint of the +0.5 preview bias?
///
/// `camera_base_knots` pins `[0,0]` and `[1,1]` and fills the middle with
/// quantiles of the NEUTRAL develop (x) against the camera rendition (y).
/// A develop biased by +0.5 puts every sample in the top half by
/// construction, so every interior x lands at or above 0.5. A real neutral
/// never does — it is the dark render the base curve exists to lift (the
/// user's own pre-bias develops top out at x = 0.493).
///
/// Era-stamped recipes are exempt: only an era-1 curve has unknown
/// provenance. A false positive costs one re-estimate that reproduces the
/// same curve, so this errs toward checking.
pub fn base_curve_looks_pre_era(version: u32, curve: &[[f32; 2]]) -> bool {
    if version >= crate::recipe::CALIB_ERA || curve.len() < 3 {
        return false;
    }
    let interior = &curve[1..curve.len() - 1];
    // (a) Every interior INPUT in the top half — what a +0.5 bias produces by
    // construction.
    if !interior.iter().all(|k| k[0] >= 0.5) {
        return false;
    }
    // (b) ...AND the curve DARKENS somewhere. This half is what keeps a
    // legitimately high-key photo — one whose darkest 2% really does sit above
    // mid-grey — from being re-estimated: (a) alone is a property of the
    // SCENE, not of the bias, and replacing a saved look the current estimator
    // need not reproduce (an older estimator's, or a hand-authored one) would
    // change how that photo renders for no reason.
    //
    // A camera base look LIFTS: it maps a dark neutral develop onto the
    // camera's own brighter rendition, which is the entire reason it exists,
    // and every legitimately derived curve in the user's store has y > x
    // throughout. The bias shifts only the INPUT side up by 0.5 while the
    // camera side keeps its true, low values — so a washed curve comes out
    // strongly DARKENING, which no real camera base look is.
    // No interior knot LIFTS, and at least one measurably darkens.
    //
    // Requiring every knot to darken STRICTLY was wrong at the clipped end:
    // the bias pushes every neutral sample to the top of the range, so the
    // last interior knot's input is 1.0 for any frame with sky or highlights,
    // and when the camera rendition also clips there (a blown window, the sun,
    // a specular highlight) that knot is exactly [1.0, 1.0]. A tie is not a
    // lift, but `1.0 < 1.0` is false, so one saturated frame disabled the
    // repair for the whole photo. The toe is where the bias is unmistakable —
    // it darkens hugely there — so a small margin on the maximum keeps the
    // shallow cases a 0.15 margin used to lose, without demanding anything of
    // the saturated end. What the margin lets escape is bounded by itself: a
    // curve that darkens NOWHERE by more than 0.05 can also mis-render by no
    // more than that, and only a clipped-bright frame (all knots pinned near
    // [1,1]) produces one — while dropping the margin would re-estimate
    // legitimate pre-cap near-flat curves over quantile noise, the exact harm
    // the SCENE/bias split above exists to avoid.
    interior.iter().all(|k| k[1] <= k[0]) && interior.iter().any(|k| k[1] < k[0] - 0.05)
}

/// Successful estimates already computed this run, keyed by photo identity —
/// including the EMPTY answer, which is the estimator's identity verdict
/// ("this photo needs no base look"), but never an inability: those must
/// retry once the file is readable again.
///
/// The repair is asked on every open AND, since the render funnel carries it,
/// on every render of an affected photo. Each estimate costs a RAW decode plus
/// a working-resolution develop, and the answer cannot change while the
/// process runs, so paying it once per photo is the difference between a
/// correction and a stall on the UI thread.
///
/// Identity = size + mtime (the thumbnail cache's convention): an in-place
/// replacement is caught whenever it changes either; an equal-size,
/// timestamp-preserving swap is NOT, and content-hashing a 50-120 MB RAW per
/// check would cost the very read this memo exists to avoid.
type CurveMemoKey = (PathBuf, CurveIdent);

fn fresh_curve_memo()
-> &'static std::sync::Mutex<std::collections::HashMap<CurveMemoKey, Vec<[f32; 2]>>> {
    static MEMO: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<CurveMemoKey, Vec<[f32; 2]>>>,
    > = std::sync::OnceLock::new();
    MEMO.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// The (size, mtime) half of the memo key. CAPTURE IT BEFORE READING the
/// file: an answer must be filed under the identity of the content it was
/// computed FROM, so a file replaced mid-read lands under the OLD key and
/// the next reader (new identity) recomputes. The repair does this; primers
/// must too — stat'ing after a multi-second develop filed a mid-open
/// replacement's NEW identity with the OLD content's answer, permanently.
pub type CurveIdent = (u64, Option<std::time::SystemTime>);

pub fn curve_ident(raw: &Path) -> CurveIdent {
    std::fs::metadata(raw).ok().map(|m| (m.len(), m.modified().ok())).unwrap_or((0, None))
}

/// Seed the memo with an ANSWER a caller already computed. The GUI open
/// worker develops the neutral and CDF-matches the camera rendition anyway;
/// discarding that answer made every later repair site (Ctrl+Z's
/// apply_step, a variant switch, Ctrl+S) pay a second full decode + develop
/// on the UI thread. Answers only — an inability must never be primed (it
/// stays uncached so the next reader retries). FIRST ANSWER WINS
/// (`or_insert`): the memo's contract is that the answer cannot change
/// while the process runs, and the GUI's working edge is a user preference
/// — overwriting on a resolution switch would have made the repaired curve
/// a function of that preference. The caller's edge may differ from the
/// repair's 2048 cap; each side is box-binned to <=1024 INDEPENDENTLY (the
/// neutral by `render::estimation_base`'s cap, the camera side inside
/// `camera_base_knots`), so answers agree within the estimator's documented
/// tolerance — not byte-identically, which is exactly why one answer is
/// recorded and kept.
pub fn prime_curve_memo(raw: &Path, ident: CurveIdent, knots: Vec<[f32; 2]>) {
    if let Ok(mut m) = fresh_curve_memo().lock() {
        m.entry((raw.to_path_buf(), ident)).or_insert(knots);
    }
}

/// Re-estimate a base curve that was fitted against a washed frame, and say
/// so. Returns the disclosure when the curve was replaced.
///
/// The bias was a PREVIEW defect only in the sense that it never touched an
/// export's pixels directly. It reached deliverables anyway: the estimate
/// runs on a capped develop ([`photo_base_knots`]), the result is persisted
/// like any user edit, and `build_tone_lut` composes it under the full-
/// resolution render. Batch 43 fixed the sampler and thereby made an
/// already-stored curve WORSE — it is now laid over a correct develop, which
/// is what turns "slightly off" into several stops dark.
pub fn repair_pre_era_base_curve(raw: &Path, r: &mut EditRecipe) -> Option<String> {
    if !base_curve_looks_pre_era(r.version, &r.base_curve) {
        return None;
    }
    let ident = curve_ident(raw);
    let key = (raw.to_path_buf(), ident);
    let memo = fresh_curve_memo();
    let cached = memo.lock().ok().and_then(|m| m.get(&key).cloned());
    let fresh = match cached {
        Some(c) => Some(c),
        None => {
            let c = photo_base_knots_checked(raw);
            // ANSWERS are cached — the empty identity verdict included. An
            // inability (`None`) is not: it means the decode, the preview or
            // the neutral develop did not work THIS time (a locked file, a
            // disconnected share), and caching it would leave the photo
            // unrepaired for the rest of the process even after it became
            // readable. FIRST ANSWER WINS here too: the lock was released
            // across the estimate, and the open worker can prime this very
            // key in that gap (the same-path keep-flight is where the two
            // threads meet) — a bare insert would put a different curve in
            // the memo than the one this call installs, so the repair
            // ADOPTS whichever answer won.
            match (c, memo.lock()) {
                (Some(knots), Ok(mut m)) => Some(m.entry(key).or_insert(knots).clone()),
                (c, _) => c,
            }
        }
    };
    // Tri-state, and the difference is the whole repair: an ANSWER replaces
    // the washed curve — the EMPTY answer too, which says "this photo needs
    // no base look" (treating it as a failure left the washed curve rendering
    // stops-dark forever, and re-paid the full decode + develop on every
    // reader, because inabilities are uncached). An INABILITY leaves the
    // curve AND the era-1 stamp alone, so the next reader retries.
    let knots = fresh?;
    r.base_curve = knots;
    r.version = crate::recipe::CALIB_ERA;
    Some(
        "this photo's camera base look was re-estimated: it was saved by a version whose \
         preview sampler ran bright, so the stored base look rendered too dark"
            .to_string(),
    )
}

/// What [`migrate_recipe_coord_frame`] did, for the caller's disclosure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoordMigration {
    /// The EXIF orientation the geometry was turned by.
    pub orientation: rawler::Orientation,
    /// The recipe also carries RASTER masks — [`crate::recipe::MaskGeometry::
    /// Bitmap`], whose pixels are an image file and cannot be turned by a
    /// coordinate rewrite. The one honest gap in an otherwise lossless
    /// migration, and since R29 C1 the ONLY one: the brush left this set when
    /// its dab stream started turning numerically.
    pub rasters_left: bool,
}

/// The photo's SOURCE frame and its EXIF turn — [`crate::decode::source_frame`]
/// with a memo, keyed like the curve memo.
///
/// Both halves, not just the orientation: reading either costs a full
/// `RawSource::new` (rawler slurps the whole file, 60–120 MB for a 61 MP ARW),
/// `source_frame` answers both from ONE header walk, and since R29 C1 the
/// migration needs the SIZE as well as the turn — a brush's radii are in width
/// units and a quarter turn rescales them by the frame aspect
/// ([`crate::render::CoordFrame`]). The strip-card and batch-export paths ask
/// per recipe, not per photo, which is what the memo is for.
type SourceFrame = ((usize, usize), rawler::Orientation);

fn orient_memo() -> &'static std::sync::Mutex<std::collections::HashMap<CurveMemoKey, SourceFrame>>
{
    static MEMO: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<CurveMemoKey, SourceFrame>>,
    > = std::sync::OnceLock::new();
    MEMO.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// [`orient_memo`]'s reader. Inabilities are NOT cached — a locked file must be
/// retried, the same rule the curve memo follows.
fn source_frame_memo(path: &Path) -> Option<SourceFrame> {
    let key = (path.to_path_buf(), curve_ident(path));
    if let Some(hit) = orient_memo().lock().ok().and_then(|m| m.get(&key).copied()) {
        return Some(hit);
    }
    let answer = crate::decode::source_frame(path).ok()?;
    Some(match orient_memo().lock() {
        Ok(mut m) => *m.entry(key).or_insert(answer),
        Err(_) => answer,
    })
}

/// Bring a saved recipe's geometry into the DISPLAY frame — the load-time
/// half of [`crate::recipe::COORD_ERA`].
///
/// Every recipe written up to v0.29.x stored its crop rectangle and mask
/// geometries against the frame the app actually drew, and for a
/// rotated/flipped RAW that was the SENSOR frame: rawler 0.7.2 reports
/// `Orientation::Normal` for everything but DNG/QTK, so a portrait ARW was
/// displayed sideways and the user drew on a sideways canvas. Now that
/// `render::orient_f32` turns the frame for real, those coordinates land on
/// the wrong axis unless they are turned with it.
///
/// Tri-state, the [`repair_pre_era_base_curve`] discipline:
///   * already era-current, or a photo with no rotation → stamp, no note;
///   * turned → stamp AND say so;
///   * the orientation could NOT be read (unreadable/locked RAW) → nothing is
///     stamped and nothing is moved, so the next reader retries. Stamping on
///     an inability would declare the coordinates migrated when they are not,
///     and no later reader could tell.
pub fn migrate_recipe_coord_frame(raw: &Path, r: &mut EditRecipe) -> Option<CoordMigration> {
    if r.coord_era >= crate::recipe::COORD_ERA {
        return None;
    }
    // A baked image is not a RAW: `decode::load_image` has applied its EXIF
    // orientation since long before this era, so its saved coordinates are
    // already display-frame. Stamp and stop — asking rawler would only fail.
    if !crate::decode::is_raw(raw) {
        r.coord_era = crate::recipe::COORD_ERA;
        return None;
    }
    let ((sw, sh), orientation) = source_frame_memo(raw)?;
    // The composed orientation, not the EXIF one (R27): era 0 means "these
    // numbers are in the SENSOR frame", and the frame they must reach is the
    // one this build DISPLAYS — EXIF plus the photographer's own turns. Today
    // the two are always equal here (`quarter_turns` did not exist before
    // v0.33, and no era-0 recipe can carry one), so this is correctness by
    // construction rather than a behaviour change; writing `raw_orientation`
    // alone would be a latent bug the day a v0.33 recipe is hand-edited to
    // era 0, or the day a future era forces a second migration.
    let orientation = crate::render::compose_orientation(orientation, r.quarter_turns);
    let rasters_left = crate::render::recipe_has_raster_masks(r);
    // Era-0 numbers are SENSOR-frame by definition, so the frame the brush
    // rewrite must rescale out of is the source rectangle `source_frame` just
    // reported — not the display one (R29 C1, `render::CoordFrame`).
    let frame = crate::render::CoordFrame::new(sw as f64, sh as f64);
    let moved = crate::render::orient_recipe_coords(r, orientation, frame);
    // The stamp lands either way once the orientation is KNOWN: a Normal
    // photo's coordinates are already display-frame, and leaving it era-0
    // would pay the metadata read again on every future load.
    r.coord_era = crate::recipe::COORD_ERA;
    // Nothing to say when nothing moved — an unrotated photo, or a recipe
    // whose only content is global sliders.
    (moved && (crate::render::recipe_has_frame_coords(r) || rasters_left))
        .then_some(CoordMigration { orientation, rasters_left })
}

/// Everything a saved recipe must be brought through before it becomes a live
/// one — the ONE call every load and every re-save shares, so a new migration
/// cannot be wired into some surfaces and forgotten on others (the defect
/// class `render_source_checked`'s comment describes: two surfaces of the same
/// build disagreeing about the same file).
#[derive(Debug, Clone, Default)]
pub struct LoadMigration {
    /// The washed pre-era base curve was re-estimated; the engine's sentence.
    pub relook: Option<String>,
    /// The geometry was turned into the display frame.
    pub reframe: Option<CoordMigration>,
}

impl LoadMigration {
    /// Did anything change? (Drives ● / thumb invalidation at the call sites
    /// that only need "is this recipe different now".)
    pub fn any(&self) -> bool {
        self.relook.is_some() || self.reframe.is_some()
    }

    /// The chained ENGINE sentence for the CLI / HTTP surfaces. The GUI has
    /// its own localized pair — these two facts are disclosed separately
    /// everywhere, because they are unrelated corrections.
    pub fn note(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(n) = &self.relook {
            parts.push(n.clone());
        }
        if let Some(c) = &self.reframe {
            parts.push(coord_migration_note(*c));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

/// The engine (English, CLI/HTTP) sentence for a coordinate-frame migration.
pub fn coord_migration_note(c: CoordMigration) -> String {
    let base = "this photo's saved crop and masks were moved to match the RAW's EXIF \
                orientation: earlier versions displayed rotated RAWs sideways, so their \
                coordinates were stored against the sideways frame"
        .to_string();
    if c.rasters_left {
        format!(
            "{base}; its raster mask(s) are image files and could NOT be turned — \
             check and re-generate them"
        )
    } else {
        base
    }
}

/// What a [`rotate_recipe`] call actually did, for the caller's disclosure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RotateOutcome {
    /// The recipe's `quarter_turns` AFTER the turn (0..=3).
    pub quarter_turns: u8,
    /// How many raster masks were re-written, turned, under fresh names — and
    /// then actually RE-ATTACHED to the recipe.
    ///
    /// `Bitmap` masks only. An `AiMask`'s cached alpha used to be counted here
    /// (it rides the storage walk, `recipe::LocalAdjustment::bitmap_paths_mut`)
    /// although phase 2 discards it, so the number promised turned masks the
    /// recipe no longer carried — R28 Batch-3 3b removed it from the walk
    /// rather than from the count, which is the same fix seen from the other
    /// side (adjudication F8-B).
    pub rasters_turned: usize,
}

/// Turn a photo's saved develop by `delta` CLOCKWISE quarter turns: the
/// photographer's rotate action, in one place for every surface.
///
/// Three things move together, and they have to move together or the photo
/// comes back with its masks somewhere else:
///
///  1. **The stored geometry**, through [`crate::render::orient_recipe_coords`]
///     — crop rectangle, every mask geometry, the Range-Mask sample point, the
///     straighten sign, and since R29 C1 every brush DAB (coordinates turned,
///     radii rescaled by the frame aspect; see phase 0 below for where that
///     aspect comes from). That function is the era-0 → era-1 migration's own
///     engine, proven exact for tilted radials by
///     `rotated_radial_mask_covers_the_rotated_pixels`.
///  2. **The raster masks** — really turned, unlike the `coord_era` migration,
///     which could only disclose them. The difference is ownership: those were
///     files an OLD build had already written against a frame nobody could
///     re-derive, whereas these are our own PNGs and `image`'s `rotate90` is
///     lossless. Turned copies land under FRESHLY CLAIMED names
///     (`store::claim_raster`) and the old files stay put — version snapshots
///     freeze their own copies and a saved recipe elsewhere may still point at
///     them, so rewriting in place would silently change what an old version
///     renders (the same rule the zoned reverse-fit follows).
///     **`Bitmap` rasters only** (`LocalAdjustment::turnable_raster_paths_mut`):
///     an `AiMask`'s alpha is a CACHE this turn invalidates, and item 1 clears
///     it so the next develop re-segments at the turned reference point.
///  3. **`quarter_turns` itself**, which is what makes the render, the export
///     and the next load agree with the geometry above.
///
/// **The turn passed to `orient_recipe_coords` is the DELTA, never the composed
/// value.** The stored coordinates are already in the CURRENT display frame;
/// re-applying the accumulated turn would move them a second time. (This is
/// the one hazard the ROADMAP skeleton called out by name.)
///
/// **All-or-nothing, in memory AND on disk.** Every raster is turned into its
/// new file BEFORE the recipe is touched; if one cannot be read or written,
/// nothing changes and the caller gets the error. A half-turned develop —
/// parametric masks moved, a painted mask left behind — is the exact
/// silent-corruption shape `backup_saved_develop` refuses for the same reason.
/// The DISK half is R28 Batch-3 3b (adjudication F8-A): a failure now also
/// deletes the turned copies already written and the claim of the raster that
/// failed, because "nothing was changed" was a sentence the GUI printed
/// (`gui/actions.rs`) over a develop dir that had grown a fresh orphan PNG per
/// attempt.
///
/// `delta` folds `% 4`; `0` is a no-op that still reports the current state.
pub fn rotate_recipe(
    r: &mut EditRecipe,
    src: &Path,
    delta: u8,
) -> std::io::Result<RotateOutcome> {
    let delta = delta % 4;
    if delta == 0 {
        return Ok(RotateOutcome { quarter_turns: r.quarter_turns % 4, rasters_turned: 0 });
    }
    let o = crate::render::quarter_turn_orientation(delta);

    // --- Phase 0: the frame shape, and ONLY when a brush needs it (R29 C1).
    //
    // A brush's radii are in WIDTH units while its dabs are circles in PIXELS,
    // so turning one needs the aspect of the frame the strokes are currently in
    // — the CURRENT display frame, i.e. the source rectangle carried through
    // the EXIF turn AND the quarter turns already applied, which is exactly
    // `frame_size_turned(src, r.quarter_turns)` before the count below moves.
    //
    // Lazily, because the read is a metadata walk of the photo (a `RawSource`
    // slurp for a RAW) and every other geometry this function turns is
    // aspect-free: a develop with no brush must not start paying for one. It
    // runs BEFORE phase 1 so a failure still leaves nothing behind on disk, and
    // it is an ERROR rather than a silent skip because a brush turned without
    // its rescale is a mask drawn in the wrong shape, which is the outcome this
    // batch exists to end.
    let frame = if crate::render::recipe_has_brush_strokes(r) {
        let (w, h) = crate::decode::frame_size_turned(src, r.quarter_turns).map_err(|e| {
            std::io::Error::other(format!(
                "read the frame of {} to turn its brush strokes: {e:#}",
                src.display()
            ))
        })?;
        crate::render::CoordFrame::new(w as f64, h as f64)
    } else {
        None
    };

    // --- Phase 1: turn every raster into a fresh file. Nothing in `r` moves
    // until all of them are on disk.
    //
    // `turnable_raster_paths_mut`, NOT `bitmap_paths_mut`: the storage walk
    // includes an AI mask's cached alpha and this one does not (R28 Batch-3 3b,
    // adjudication F8-B). See that method for why the two sets differ — in
    // short, phase 2 below DROPS the AI cache by design, so turning it produced
    // an orphan file and a `rasters_turned` count that over-promised.
    let mut rewritten: Vec<(String, String)> = Vec::new();
    let staged = {
        let mut probe = r.clone();
        (|| -> std::io::Result<()> {
            for m in probe.masks.iter_mut() {
                for path in m.turnable_raster_paths_mut() {
                    if rewritten.iter().any(|(from, _)| from == path.as_str()) {
                        continue; // one file, several masks — turn it once
                    }
                    let turned = turn_raster_file(Path::new(path.as_str()), src, o)?;
                    rewritten.push((path.clone(), turned));
                }
            }
            Ok(())
        })()
    };
    if let Err(e) = staged {
        // ALL-OR-NOTHING ON DISK TOO (R28 Batch-3 3b, adjudication F8-A). The
        // in-memory half was already true — `r` has not been touched yet — but
        // the rasters turned BEFORE the failure were finished files in the
        // photo's develop dir that nothing would ever reference again, and the
        // GUI told the photographer "nothing was changed" (`gui/actions.rs`)
        // while they accumulated one per attempt. `turn_raster_file` releases
        // the claim of the raster that failed; these are the ones that
        // succeeded. Best-effort per file, like every other cleanup in the
        // store (`store::detach_rasters`, `gui/masks.rs`): a delete that cannot
        // happen must not replace the real error with a filesystem one.
        for (_, made) in rewritten.iter() {
            let _ = std::fs::remove_file(made);
        }
        return Err(e);
    }

    // --- Phase 2: commit. Geometry, then the raster references, then the
    // turn count.
    crate::render::orient_recipe_coords(r, o, frame);
    for m in r.masks.iter_mut() {
        // The SAME walk phase 1 staged from, so a path can never be re-pointed
        // at a file that phase never made.
        for path in m.turnable_raster_paths_mut() {
            if let Some((_, to)) = rewritten.iter().find(|(from, _)| from == path.as_str()) {
                *path = to.clone();
            }
        }
    }
    r.quarter_turns = (r.quarter_turns + delta) % 4;
    Ok(RotateOutcome { quarter_turns: r.quarter_turns, rasters_turned: rewritten.len() })
}

/// One raster mask, turned by `o` into a freshly claimed file inside `src`'s
/// develop dir. Returns the new ABSOLUTE path (the frame every live recipe
/// holds; `store::relativize_mask_paths` bares it again at write time).
///
/// The claim prefix is the old file's stem with any `-<n>` claim suffix
/// stripped, so turning `mask-sky-2.png` produces `mask-sky-3.png` rather than
/// `mask-sky-2-2.png` — the same numbering `store::claim_raster` hands out
/// everywhere else.
fn turn_raster_file(
    from: &Path,
    src: &Path,
    o: rawler::Orientation,
) -> std::io::Result<String> {
    let img = crate::render::open_mask_bounded(from).map_err(|e| {
        std::io::Error::other(format!("turn the mask raster {}: {e:#}", from.display()))
    })?;
    let turned = crate::render::oriented(img, o);
    let stem = from.file_stem().and_then(|s| s.to_str()).unwrap_or("mask");
    let prefix = stem.rsplit_once('-').filter(|(_, n)| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty()).map_or(stem, |(head, _)| head);
    let target = crate::store::claim_raster(src, prefix)?;
    if let Err(e) = turned.to_luma8().save(&target) {
        // RELEASE THE CLAIM WE COULD NOT FILL — `claim_raster` reserves the
        // name by creating a 0-byte file, so a failed write leaves a file that
        // looks like a mask, decodes as nothing, and burns the name for every
        // later claim. Same rule at the repo's two other claim sites
        // (`store::detach_rasters`, `gui/masks.rs`'s failed segment run), and
        // it is what lets `rotate_recipe`'s caller be told nothing changed.
        let _ = std::fs::remove_file(&target);
        return Err(std::io::Error::other(format!("write {}: {e}", target.display())));
    }
    Ok(target.to_string_lossy().into_owned())
}

/// [`repair_pre_era_base_curve`] + [`migrate_recipe_coord_frame`], in that
/// order — the load-time migration funnel. Order is not arbitrary: the repair
/// reads the photo's PIXELS (decode + neutral develop) while the reframe reads
/// only its metadata, and the repair's early-out is the cheaper of the two.
pub fn migrate_loaded_recipe(raw: &Path, r: &mut EditRecipe) -> LoadMigration {
    LoadMigration {
        relook: repair_pre_era_base_curve(raw, r),
        reframe: migrate_recipe_coord_frame(raw, r),
    }
}

/// The photo's saved recipe (central store first, then legacy), parsed once —
/// the calibration-stamping snapshot both fields must come from.
///
/// The pre-era repair happens HERE, in the one funnel every library consumer
/// shares, so no surface can render a washed-frame curve by forgetting to ask.
fn saved_recipe_snapshot(raw: &Path) -> Option<EditRecipe> {
    // A crashed publish's survivor is a save like any other — republish it
    // BEFORE reading, or this snapshot returns None inside the retire window
    // and the caller stamps fresh calibration over the survivor's (the same
    // touch-time rule migrate_legacy follows; a failed recovery is re-decided
    // by the backup gate downstream).
    let _ = crate::store::recover_orphan_baks(raw);
    // Newest intent decides WHICH develop's calibration carries forward
    // (L13#4). When Lightroom's own sidecar out-ranks the store, recipe.json
    // is a superseded generation: both restore surfaces already re-stamp
    // FRESH calibration for that photo (serve.rs api_recipe, the GUI's
    // stamp_calibration on an XMP restore), and stamping the superseded
    // recipe's curve/profile/anchor onto a brand-new AI recipe made Analyze
    // hand back a canvas that renders unlike the one it replaced. `None` is
    // the right answer: the fresh arm in every consumer stamps exactly what
    // the restore surfaces stamp.
    match crate::store::lightroom_sidecar(raw) {
        crate::store::LrSidecar::Only(t) | crate::store::LrSidecar::NewerThanStore(t)
            if !crate::xmp::xmp_to_recipe_for_photo(&t, raw).is_noop() =>
        {
            return None;
        }
        crate::store::LrSidecar::Unreadable(why) => {
            // Unreadable is not absent — but it cannot outrank either.
            // Disclosed, then the stored develop stands.
            eprintln!(
                "⚠ {}: a Lightroom sidecar sits beside this photo but could not be read ({why}) — calibration falls back to the stored develop",
                stem(raw)
            );
        }
        _ => {}
    }
    for p in [crate::store::recipe_target(raw), crate::store::legacy_recipe(raw)] {
        if let Ok(text) = crate::store::read_text_capped(&p, crate::store::MAX_STORE_JSON)
            && let Ok(mut r) = serde_json::from_str::<EditRecipe>(&text)
        {
            if let Some(note) = repair_pre_era_base_curve(raw, &mut r) {
                eprintln!("⚠ {}: {note}", stem(raw));
            }
            return Some(r);
        }
    }
    None
}

/// The per-photo calibration a programmatic writer stamps — every field from
/// ONE saved-recipe snapshot. The era stamp is a FIELD of it: it describes
/// the curve's provenance, so a writer that stamps the curve without its
/// version launders a saved era-1 curve under its own era-2 recipe (the
/// defect the paste path shipped, then fixed). The two split accessors this
/// replaces (`photo_base_curve`, `photo_lens_profile`) had no remaining
/// callers and re-created both hazards for the next one.
pub struct PhotoCalibration {
    pub version: u32,
    pub base_curve: Vec<[f32; 2]>,
    pub lens_profile: crate::recipe::LensProfile,
    pub as_shot_k: Option<f32>,
    pub as_shot_tint: Option<f32>,
}

/// ALL calibration halves from ONE saved-recipe snapshot: independent reads
/// could pair an OLD curve with a NEW profile when a concurrent publish lands
/// between them (the same single-snapshot rule `produce_recipe` follows).
/// The fresh arm is era-stamped by construction — those estimates come from
/// THIS build's sampler.
pub fn photo_calibration(raw: &Path) -> PhotoCalibration {
    match saved_recipe_snapshot(raw) {
        Some(r) => PhotoCalibration {
            version: r.version,
            base_curve: r.base_curve,
            lens_profile: r.lens_profile,
            as_shot_k: r.as_shot_k,
            as_shot_tint: r.as_shot_tint,
        },
        None => fresh_photo_calibration(raw),
    }
}

/// [`photo_calibration`]'s fresh arm on its own — era-stamped by
/// construction (these estimates come from THIS build's sampler).
pub(crate) fn fresh_photo_calibration(raw: &Path) -> PhotoCalibration {
    let (as_shot_k, as_shot_tint) = fresh_as_shot_wb(raw);
    PhotoCalibration {
        version: crate::recipe::CALIB_ERA,
        base_curve: photo_base_knots(raw),
        lens_profile: fresh_lens_profile(raw),
        as_shot_k,
        as_shot_tint,
    }
}

/// True when a calibration carries NOTHING the render would act on — no
/// base curve, no active lens component (the recipe's own activity
/// predicates — re-implementing them here is the drift the codebase
/// already fixed once), no as-shot WB anchor.
pub fn calibration_is_neutral(cal: &PhotoCalibration) -> bool {
    cal.base_curve.is_empty()
        && !cal.lens_profile.vignette_active()
        && !cal.lens_profile.geometry_active()
        && cal.as_shot_k.is_none()
}

/// The calibration authority for REVERSE-FIT surfaces (CLI `match` + GUI
/// 反推): saved-first like [`photo_calibration`], but an all-NEUTRAL saved
/// calibration falls through to the fresh estimate. A neutral saved
/// calibration is either a pre-base-curve legacy save or a previous
/// UNSTAMPED fit (the R15 poison: an old fit recipe with an empty curve
/// became the saved-first authority, so every later fit inherited the
/// empty base and the seed never engaged). Falling through is safe for a
/// fit specifically — the solve compensates ON TOP of whatever base it is
/// given, so the final look is unchanged; only the conditioning improves —
/// while ordinary develops keep the legacy render-as-saved contract via
/// [`photo_calibration`].
/// Narrow disclosed edge: a photo whose saved develop turned every lens
/// component OFF and has neither curve nor as-shot anchor reads as
/// "neutral" and a fit re-stamps the fresh all-on profile — a fit is a NEW
/// develop, and all-on matches the in-camera default that stamping always
/// produces. Branching on the snapshot directly keeps the fresh estimate
/// (a demosaic for a RAW) to at most ONE run.
pub fn fit_calibration(raw: &Path) -> PhotoCalibration {
    match saved_recipe_snapshot(raw) {
        Some(r) => {
            let cal = PhotoCalibration {
                version: r.version,
                base_curve: r.base_curve,
                lens_profile: r.lens_profile,
                as_shot_k: r.as_shot_k,
                as_shot_tint: r.as_shot_tint,
            };
            if calibration_is_neutral(&cal) { fresh_photo_calibration(raw) } else { cal }
        }
        None => fresh_photo_calibration(raw),
    }
}

/// Stamp a reverse-fit recipe with the photo's calibration — ONE rule shared
/// by the CLI `match` command and the GUI 反推 worker: without it the fitted
/// deltas landed on a much darker base than the one they were solved
/// against, and the render disagreed with the fit's own numbers. The era
/// stamp rides WITH the curve (the paste rule): the fitted recipe is era-2
/// by Default, and stamping a saved era-1 curve under it would launder the
/// provenance the pre-era repair keys on.
pub fn stamp_fit_calibration(recipe: &mut crate::recipe::EditRecipe, cal: PhotoCalibration) {
    recipe.version = cal.version;
    recipe.base_curve = cal.base_curve;
    recipe.lens_profile = cal.lens_profile;
    recipe.as_shot_k = cal.as_shot_k;
    recipe.as_shot_tint = cal.as_shot_tint;
}

/// A calibration-only [`EditRecipe`] — the `base` the R16 fit composes
/// into its solve (`fit::fit_recipe_from` / `fit_zoned::fit_recipe_zoned_from`).
/// The as-shot WB anchors only RIDE (no temperature is set, so
/// `apply_recipe_wb` never fires — they exist for the develop panel's WB
/// baseline). This retired the v0.24.0 two-pass seed
/// (`fit_calibration_seed`): pre-rendering the calibration clipped
/// saturated channels at the pass boundary (`scale_chroma` clamp order,
/// measured up to ~18.7/255 mean on saturated fixtures), while composing
/// the base INTO the solve makes every candidate render the canvas's own
/// one-pass `user(base(x))` — the gap is gone by construction and the fit's
/// residual numbers describe exactly what the user sees.
pub fn calibration_recipe(cal: PhotoCalibration) -> crate::recipe::EditRecipe {
    crate::recipe::EditRecipe {
        version: cal.version,
        base_curve: cal.base_curve,
        lens_profile: cal.lens_profile,
        as_shot_k: cal.as_shot_k,
        as_shot_tint: cal.as_shot_tint,
        ..Default::default()
    }
}

/// Fresh in-camera lens profile for `raw`, stamped "all available components
/// on" (the user's chosen default: match the in-camera JPEG, which applies
/// these same corrections). Cheap — one TIFF metadata parse, no decode.
pub fn fresh_lens_profile(raw: &Path) -> crate::recipe::LensProfile {
    let mut p = crate::lensmeta::read(raw);
    p.vignette_on = !p.vignette.is_empty();
    p.distortion_on = !p.distortion.is_empty();
    p.ca_on = !p.ca_r.is_empty() && !p.ca_b.is_empty();
    p
}

/// [`fresh_lens_profile`] for a photo whose masks came from a SIDECAR — the
/// only form that can answer the mask-warp question correctly (R29 Batch-3).
///
/// `crs:LensProfileEnable="0"` says Lightroom applied no lens correction, so
/// RADIAL geometry stays in its stored frame. LINEAR is different: Lightroom
/// transports its two corrected-frame handles through the available camera map
/// and reconstructs a straight gradient in the raw frame. The ordinary
/// `mask_warp` therefore remains identity for RADIAL while the same solved map
/// moves once into `linear_handle_warp` for that LINEAR-only operation.
///
/// Only the WARP is answered from the sidecar. The vignette / distortion / CA
/// toggles are the photographer's own and are left exactly as
/// [`fresh_lens_profile`] stamped them — see [`crate::xmp::lens_profile_enabled`]
/// for why reading Lightroom's switch as an instruction would be wrong.
///
/// `None` sidecar, or a document that says nothing, is [`fresh_lens_profile`]
/// unchanged.
pub fn fresh_lens_profile_for_sidecar(
    raw: &Path,
    sidecar: Option<&str>,
) -> crate::recipe::LensProfile {
    let mut p = fresh_lens_profile(raw);
    if sidecar.and_then(crate::xmp::lens_profile_enabled) == Some(false) {
        retain_disabled_linear_handle_warp(&mut p);
    }
    p
}

/// Preserve the solved camera/LCP map across the disabled-sidecar boundary for
/// LINEAR H2 without exposing it to RADIAL's `mask_warp` path. Kept at this
/// upstream boundary so every importer and render surface receives one coherent
/// frame fact rather than reconstructing it at individual call sites.
fn retain_disabled_linear_handle_warp(p: &mut crate::recipe::LensProfile) {
    p.linear_handle_warp = std::mem::take(&mut p.mask_warp);
    p.mask_warp_src = crate::recipe::MaskWarpSource::DisabledInSidecar;
}

/// Fresh as-shot WB anchor for `raw` — the camera's absolute Kelvin + tint
/// from its own metadata (`render::as_shot_wb`; metadata-only decode, no
/// demosaic). `(None, None)` when unavailable (non-RAW, no colour matrix,
/// damaged coefficients): the engine then keeps its historical 5500 K anchor.
pub fn fresh_as_shot_wb(raw: &Path) -> (Option<f32>, Option<f32>) {
    match crate::render::as_shot_wb(raw) {
        Some((k, t)) => (Some(k), Some(t)),
        None => (None, None),
    }
}

/// Fresh camera-matched base-look estimate for `raw`: a neutral develop
/// CDF-matched against the embedded preview (`render::camera_base_knots`).
/// Costs a demosaic for a RAW — callers that already hold a neutral render
/// (the GUI open worker) call the estimator directly instead. Best-effort by
/// design: the base look is an enhancement, so a develop/decode failure here
/// yields "no base look" rather than failing the caller's real operation
/// (whose own render will surface the same error loudly). Callers that must
/// tell that failure apart from the estimator's own empty answer use
/// [`photo_base_knots_checked`].
pub fn photo_base_knots(raw: &Path) -> Vec<[f32; 2]> {
    photo_base_knots_checked(raw).unwrap_or_default()
}

/// The tri-state form the pre-era repair needs: `None` = no estimate could be
/// PRODUCED this time (not a RAW, unreadable or absent embedded preview,
/// failed neutral develop); `Some(knots)` is the estimator's ANSWER —
/// possibly EMPTY, `camera_base_knots`' documented identity verdict ("this
/// photo needs no base look"). An answer may replace a saved curve; an
/// inability must leave it alone.
pub fn photo_base_knots_checked(raw: &Path) -> Option<Vec<[f32; 2]>> {
    if !decode::is_raw(raw) {
        return None;
    }
    let camera = match decode::embedded_preview(raw) {
        Ok(Some(c)) => c,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("⚠ base look skipped: embedded preview of {} failed ({e})", raw.display());
            return None;
        }
    };
    // The estimate is CDF statistics — a ≤2048 working develop carries the
    // same histogram signal as 61 MP at a fraction of the transients.
    match crate::render::render_to_image(raw, &EditRecipe::default(), None, Some(2048)) {
        Ok(neutral) => {
            // Estimate on the profile-vignette-corrected neutral — the same
            // base a stamped canvas starts from (see render::estimation_base).
            let est = crate::render::estimation_base(&neutral, &fresh_lens_profile(raw));
            match crate::render::camera_base_knots(&est, &camera) {
                Some(k) => Some(k),
                None => {
                    // Could not JUDGE (too few pixels on a side) — an
                    // inability like the arms above, never a verdict.
                    eprintln!(
                        "⚠ base look skipped: too few pixels to compare for {}",
                        raw.display()
                    );
                    None
                }
            }
        }
        Err(e) => {
            // Disclosed, not silent: the caller's own render will surface the
            // same failure loudly, but the resulting darker-than-canvas output
            // needs a traceable cause in the log.
            eprintln!("⚠ base look skipped: neutral develop of {} failed ({e})", raw.display());
            None
        }
    }
}

/// Publish the recipe for `raw`. `sink` receives the clamp disclosure, if the
/// write costs anything (R29-1: it used to be a bare `eprintln!`).
pub fn write_recipe(
    raw: &Path,
    recipe: &EditRecipe,
    out: Option<PathBuf>,
    sink: &dyn crate::diag::Sink,
) -> Result<PathBuf> {
    let out = out.unwrap_or_else(|| crate::store::recipe_target(raw));
    // `-o some_dir` names an existing DIRECTORY: refuse instead of renaming
    // the whole directory to .bak below and publishing a recipe FILE in its
    // place (the user meant "write into it", which this API does not do).
    if out.is_dir() {
        anyhow::bail!(
            "recipe target {} is a directory — pass a file path (e.g. {}/recipe.json)",
            out.display(),
            out.display()
        );
    }
    ensure_parent(&out)?;
    let bytes = recipe_bytes_for(recipe, out.parent(), &crate::diag::Diag::about(sink, raw))?;
    // Publish via tmp+rename rather than truncating the AUTHORITATIVE file in
    // place: a crash mid-write used to leave a half-written recipe.json (loud
    // Unreadable, but the develop was gone). The old file is retired to .bak
    // first for CRASH RECOVERY (fs::rename does replace an existing file on
    // Windows — verified empirically) — worst case is a briefly missing file
    // with the intact .tmp beside it, never corrupt JSON.
    // Per-process AND per-call tmp name from the ONE shared store counter
    // (see store::next_tmp_seq): a GUI and a web server saving the same
    // photo used to share one fixed .tmp, the web server threads REQUESTS,
    // and a same-process migration publish mints into the SAME
    // `.json.tmp.<pid>.<seq>` namespace — every site must share one counter.
    // (.bak stays shared — the retire/restore pair below is
    // last-writer-wins by design; one photo has one interactive writer in
    // practice.)
    if out == crate::store::recipe_target(raw) {
        crate::store::durable_retire_and_write(
            &out,
            &out.with_extension("json.bak"),
            &bytes,
        )
        .with_context(|| format!("publish recipe {}", out.display()))?;
        crate::store::note_source(raw); // breadcrumb for the hashed store dir
    } else {
        // A redirected `-o` (and every v<N> snapshot) is OUTSIDE
        // recover_orphan_baks' pair list: retiring here stranded the old
        // bytes as an unrecoverable `.bak` beside a MISSING live file when a
        // crash hit the retire window — `apply` then saw no recipe at all.
        // Stage+rename replaces the previous bytes atomically instead: no
        // missing-file window, no orphan a reader will never republish.
        crate::store::durable_write(&out, &bytes)
            .with_context(|| format!("publish recipe {}", out.display()))?;
    }
    Ok(out)
}

/// The exact on-disk recipe bytes for `anchor`: a relativized, clamped COPY —
/// the caller's in-memory recipe keeps its absolute paths for rendering.
/// Rasters living beside the recipe are stored by bare file name so the
/// develop dir stays relocatable (store::resolve_mask_paths re-anchors them
/// at load). The size caps belong to the WRITE, not to each caller's memory:
/// every render route clamped its untrusted input and `api_xmp` — the one
/// that persists — did not, so a hostile body landed on disk as the photo's
/// authoritative recipe.json and was re-parsed by every later reader. The
/// routes that already clamp see a no-op — a floor no future route can
/// forget to stand on.
///
/// `diag` is the caller's channel for the clamp disclosure below, carrying the
/// photograph this recipe belongs to (R28 Batch-5 5c threaded the path;
/// R29-1 threads the whole channel). `anchor` cannot stand in for the
/// photograph: it is a DIRECTORY (the develop dir, or wherever `-o` pointed),
/// so it names the store's hash, not the picture.
fn recipe_bytes_for(
    recipe: &EditRecipe,
    anchor: Option<&std::path::Path>,
    diag: &crate::diag::Diag<'_>,
) -> Result<Vec<u8>> {
    let mut on_disk = recipe.clone();
    let dropped = on_disk.clamp();
    if !dropped.is_empty() {
        // describe(): only the non-zero losses — curve/string truncation was
        // invisible behind a "0 mask(s)" line (16-lane scan L16).
        diag.emit(
            crate::diag::Mark::WarningWord,
            format!("recipe limits discarded {}", dropped.describe()),
        );
    }
    if let Some(parent) = anchor {
        crate::store::relativize_mask_paths(&mut on_disk, parent);
    }
    Ok(serde_json::to_vec_pretty(&on_disk)?)
}

/// The CENTRAL-STORE recipe bytes for `raw` — identical to what a plain
/// [`write_recipe`] publishes — for staging into a
/// [`crate::store::commit_develop`] single-generation save.
pub fn recipe_store_bytes(
    raw: &Path,
    recipe: &EditRecipe,
    sink: &dyn crate::diag::Sink,
) -> Result<Vec<u8>> {
    let target = crate::store::recipe_target(raw);
    recipe_bytes_for(recipe, target.parent(), &crate::diag::Diag::about(sink, raw))
}

/// First FREE ./out artifact path for `tag` (`tag`, `tag-2`, … `tag-999`),
/// CLAIMED atomically (`create_new`): pixels.json links these paths, GUI undo
/// history holds them, and a CANCELLED worker may still be running toward the
/// name it probed — an existence probe alone would hand the same name to the
/// replacement task and let the two write over each other. The claimed
/// placeholder is simply overwritten by the worker's save. `None` past the
/// 999 cap (refuse, never alias). Shared by the GUI's five starters and the
/// web fill/heal handlers — a fixed web output name used to overwrite a
/// master an earlier develop still referenced.
pub fn unique_out(path: &Path, tag: &str) -> Option<PathBuf> {
    // n = 0 → "tag"; n = 1..=998 → "tag-2".."tag-999" (never tag-1000).
    for n in 0..=998u32 {
        let t = if n == 0 { tag.to_string() } else { format!("{tag}-{}", n + 1) };
        let cand = default_out(path, &t, "png");
        if ensure_parent(&cand).is_err() {
            return None;
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&cand) {
            Ok(_) => return Some(cand),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Write the develop's XMP projection: the published path, the note when
/// the merge could not be performed and the sidecar was REGENERATED instead
/// (see [`write_xmp_doc`] for why that is a loss worth telling the user
/// about), and the per-mask list of what the projection itself could not
/// carry (M6a — bitmap/muted masks skipped, extra shapes flattened, radial
/// rotation and recolour gains dropped; [`xmp::mask_export_losses`]).
/// Every caller receives both (round-12 disclosure threading): the old
/// note-dropping `write_xmp` wrapper was how five of seven surfaces stayed
/// silent, so the values ride the RETURN TYPE where a surface has to look at
/// them. stderr already hears both via `write_xmp_doc`; UI surfaces route
/// what they are handed here.
///
/// `sink` receives the two ungated lines this path raises (the merge note and
/// the mask-loss list) — R29-1: the caller decides where they go, and the
/// SUBJECT is bound from `raw` here rather than taken on trust.
pub fn write_xmp(
    raw: &Path,
    recipe: &EditRecipe,
    sink: &dyn crate::diag::Sink,
) -> Result<(PathBuf, Option<String>, Vec<xmp::MaskLoss>)> {
    use crate::store::SidecarRead;
    let target = xmp_target(raw);
    // MERGE, never regenerate over Lightroom's work (A11): the base is the
    // sidecar Lightroom itself writes (beside the RAW) when one exists — its
    // LR-only properties (the camera profile / Look, PointColor, LR
    // lens-profile data, foreign namespaces) survive our save, so the file
    // the user copies back beside the RAW keeps them. Else the previous
    // projection at the destination, which carries forward whatever an
    // earlier merge preserved.
    //
    // The base carries ITS OWN PATH so a failed merge can name the file whose
    // properties are lost — the note used to name the OUTPUT for every base,
    // pointing users at a file that was never the problem. And an UNREADABLE
    // base is a disclosure, never a silent fall-through: `read_sidecar`'s
    // one-Option shape let an over-the-cap Lightroom sidecar be silently
    // replaced by our own previous projection as the merge base, so a REAL
    // loss produced no note — the exact silence this function exists to end.
    let mut notes: Vec<String> = Vec::new();
    let mut base: Option<(PathBuf, String)> = None;
    // Only a camera RAW has a Lightroom-sidecar convention; a baked
    // PNG/TIFF's neighbouring .xmp is someone else's file (store::
    // lightroom_sidecar draws the same line for restore). Reading it anyway
    // meant a foreign sidecar was consulted on every save of a baked photo.
    if crate::decode::is_raw(raw) {
        let lr = raw.with_extension("xmp");
        match crate::store::read_sidecar_checked(&lr) {
            // A blank file carries nothing to merge and nothing to lose.
            SidecarRead::Ok(t) if !t.trim().is_empty() => base = Some((lr, t)),
            SidecarRead::Ok(_) | SidecarRead::Missing => {}
            // Deliberately says "may not be", not "are not": the save falls
            // through to the previous projection below, which carries forward
            // whatever an earlier merge preserved — so some of that sidecar's
            // properties can survive by that route. What is certain is that
            // anything added to it SINCE is not represented, and that is the
            // loss worth naming.
            SidecarRead::Unreadable(why) => notes.push(format!(
                "the Lightroom sidecar at {} could not be read ({why}), so this save was \
                 merged against AutoShade's own previous version instead — anything changed \
                 in that sidecar since is not in the new file; the sidecar itself is untouched",
                lr.display()
            )),
        }
    }
    if base.is_none() {
        match crate::store::read_sidecar_checked(&target) {
            SidecarRead::Ok(t) if !t.trim().is_empty() => base = Some((target.clone(), t)),
            SidecarRead::Ok(_) | SidecarRead::Missing => {}
            SidecarRead::Unreadable(why) => notes.push(format!(
                "the previous XMP at {} could not be used as the merge base ({why}) — \
                 it is replaced by a regenerated file that carries none of its properties",
                target.display()
            )),
        }
    }
    write_xmp_doc(target, recipe, base, &crate::diag::Diag::about(sink, raw), notes)
}

/// Write the XMP to an EXPLICIT path. Used when the recipe was redirected with
/// `-o`: the two halves of one develop must stay in the same folder, or the
/// GUI/web would keep restoring an older `out/<stem>.xmp` instead.
///
/// `diag` carries BOTH halves the old `photo: Option<&Path>` parameter was
/// standing in for (R29-1): the photograph — read back out for the frame
/// aspect — and the channel its disclosures travel on.
pub fn write_xmp_at(
    out: PathBuf,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
) -> Result<(PathBuf, Option<String>, Vec<xmp::MaskLoss>)> {
    use crate::store::SidecarRead;
    let mut notes: Vec<String> = Vec::new();
    let base = match crate::store::read_sidecar_checked(&out) {
        SidecarRead::Ok(t) if !t.trim().is_empty() => Some((out.clone(), t)),
        SidecarRead::Ok(_) | SidecarRead::Missing => None,
        SidecarRead::Unreadable(why) => {
            notes.push(format!(
                "the previous XMP at {} could not be used as the merge base ({why}) — \
                 it is replaced by a regenerated file that carries none of its properties",
                out.display()
            ));
            None
        }
    };
    write_xmp_doc(out, recipe, base, diag, notes)
}

/// Returns the published path and, when the merge could not run, a note naming
/// what that cost.
///
/// A base the splicer cannot account for falls back to a FRESH document rather
/// than failing the save — but that fallback DROPS exactly what the merge
/// exists to protect: the camera profile / creative `Look`, `crs:PointColor`,
/// Lightroom's own lens-profile block, foreign namespaces, the xpacket
/// wrapper. Doing it silently re-entered the A11 data loss through the back
/// door: the user saw a plain "saved" and found their Lightroom work gone the
/// next time they opened the catalog. Everywhere else in this codebase a
/// degraded result is disclosed (`render_source_checked` refuses,
/// `unparsable_crs_numbers` warns about far smaller losses); this now matches.
fn write_xmp_doc(
    out: PathBuf,
    recipe: &EditRecipe,
    merge_base: Option<(PathBuf, String)>,
    diag: &crate::diag::Diag<'_>,
    mut notes: Vec<String>,
) -> Result<(PathBuf, Option<String>, Vec<xmp::MaskLoss>)> {
    // ONE source for the photograph: the channel this write discloses on. A
    // separate `photo` parameter beside a separate attribution could disagree,
    // and the frame aspect below is decided by the same identity the warnings
    // are attributed to.
    let photo = diag.photo();
    ensure_parent(&out)?;
    // The same floor `write_recipe` stands on, for the same reason: this is a
    // persistence boundary, the string caps exist because `rationale` is
    // filled from an upstream failure body, and the XMP lands BESIDE THE RAW
    // in the user's library. recipe.json was protected structurally and this
    // half only by the convention that every caller clamps first.
    let mut bounded = recipe.clone();
    bounded.clamp();
    let recipe = &bounded;
    let base_path = merge_base.as_ref().map(|(p, _)| p.clone());
    // The frame the geometry projection writes INTO (`xmp::FrameAspect`).
    // A base sidecar that declares its own `tiff:ImageWidth/ImageLength` still
    // wins over this — see `merge_recipe_into_xmp_in_frame`. Unreadable = no
    // frame = the loss is disclosed rather than guessed, the same tri-state
    // `migrate_recipe_coord_frame` takes on the orientation it cannot read.
    //
    // R27 WIDENED THE GATE. v0.32.0 paid for this only on a rotated radial,
    // because nothing else in the projection needed an aspect. Two
    // measurements changed that: a tilted CROP is a rotated-corner encoding in
    // the source frame and cannot be written without `W/H`
    // (`P3-cropangle-model.md` §2), and EVERY geometry on a portrait capture
    // has to be turned back into the sensor frame before it is written
    // (`P1-portrait-mask-frame.md` §5). So the gate is now "does this recipe
    // carry frame coordinates at all", which is one memoised metadata read per
    // photo per process — and it is no longer "a cost for nothing".
    //
    // R29 C1 widened what that PREDICATE answers rather than the gate itself: a
    // brush group is a frame coordinate now, so a brush-only recipe fetches the
    // aspect where it used to skip it. That is not incidental — `in_source_
    // frame` needs the frame to un-turn the dab stream, and without this the
    // writer would have emitted a portrait capture's brush in the display
    // frame while its `tiff:` block declared the sensor one.
    let frame = photo
        .filter(|_| {
            crate::render::recipe_has_frame_coords(recipe) || recipe.straighten_deg != 0.0
        })
        .and_then(|p| photo_frame_aspect(p, recipe.quarter_turns));
    let merged = merge_base.and_then(|(_, b)| {
        xmp::merge_recipe_into_xmp_in_frame_for_photo(&b, recipe, frame, photo)
    });
    // A merge that SUCCEEDED can still have losses it could not avoid (the
    // base's unrepresentable mask block giving way to the recipe's own
    // masks) — its notes ride the same channel as the regeneration note.
    let merged = merged.map(|mut o| {
        notes.append(&mut o.notes);
        (o.doc, o.losses)
    });
    if merged.is_none()
        && let Some(bp) = base_path
    {
        // The note names the file whose properties are LOST — the base — and
        // is honest about what happened to it: a base at the output path is
        // genuinely replaced by the regeneration; a base beside the RAW is
        // not touched at all, only unrepresented in the new file. "The camera
        // creative Look, LR lens-profile data" are EXAMPLES of what a
        // Lightroom base carries, not a claim about this one — a ratings-only
        // sidecar loses its ratings the same way. (Lightroom's Texture used to
        // head that list; R25 B2 models it, so a regenerated file DOES carry
        // it and naming it here would have made the disclosure false. The
        // "camera profile" went the same way in R25 B4: `crs:CameraProfile`
        // and the Transform block ride in `EditRecipe::passthrough` and are
        // rewritten by the regeneration too — the creative `Look` is a NESTED
        // element with no string spelling and is still genuinely lost, which
        // is why it is the example that stayed.)
        notes.push(if bp == out {
            format!(
                "the existing sidecar at {} could not be merged — it was regenerated, \
                 so properties it carried (e.g. Lightroom's creative Look, \
                 LR lens-profile data) are not in the new file",
                out.display()
            )
        } else {
            format!(
                "the sidecar at {} could not be merged — the new file at {} was \
                 regenerated without the properties it carried (e.g. Lightroom's \
                 creative Look, LR lens-profile data, ratings); \
                 the sidecar itself is untouched",
                bp.display(),
                out.display()
            )
        });
    }
    // Both disclosure lines in this function travel the caller's channel and
    // carry the photograph when the caller knows one (R28 Batch-5 5c stamped
    // them; R29-1 routes them). The merge/export path had the identity all
    // along and simply did not print it, which is what made a `--jobs 3`
    // transcript unreadable.
    let note = (!notes.is_empty()).then(|| {
        let msg = notes.join("; ");
        diag.warn(msg.clone());
        // The RETURNED note is unprefixed: it travels the structured channel to
        // a caller that already knows which photo it asked about (and the GUI
        // localises it), so stamping it there would print the stem twice.
        msg
    });
    // M6a: the projection's OWN lossy edges (bitmap/muted masks skipped, extra
    // shapes flattened, rotation + recolour dropped) — judged by the WRITER on
    // the CLAMPED recipe, i.e. exactly the masks the document carries. The
    // import direction had four disclosure sites and the export direction had
    // none; this is that half. The caller's channel covers every CLI path from
    // here — one line, one place — the same deal the merge note above has;
    // UI/web surfaces localise the structured list they get back.
    //
    // The document and the verdicts arrive TOGETHER (R22 NIT-1): both arms
    // produce the list while emitting the mask block, so this no longer calls
    // `mask_export_losses` and builds that block a second time per save.
    let (doc, mask_losses) = match merged {
        Some(pair) => pair,
        None => xmp::recipe_to_xmp_in_frame(recipe, frame),
    };
    if let Some(m) = xmp::describe_mask_losses(&mask_losses) {
        diag.warn(m);
    }
    // Stage + rename, never truncate in place: `fs::write` opens the LIVE
    // sidecar with O_TRUNC, so a full disk, an interruption or a competing
    // writer left a truncated file where a valid Lightroom sidecar used to
    // be — the previous projection destroyed by the failed attempt to
    // replace it. (fs::rename replaces the destination on every platform.)
    crate::store::durable_write(&out, doc.as_bytes())
        .with_context(|| format!("publish xmp {}", out.display()))?;
    Ok((out, note, mask_losses))
}

/// The photo's own SOURCE frame — un-rotated size plus the turn that carries
/// it to the display frame — memoised per file the way [`orient_memo`] is:
/// both answers cost the same metadata read, and the strip-card and batch
/// surfaces ask per recipe, not per photo. `None` for a photo whose frame
/// cannot be read; the caller discloses that rather than guessing an aspect.
///
/// **The SOURCE frame, not the display one** (R27 A7). `decode::frame_size`
/// applies the orientation and hands back what the render produces; the frame
/// a `crs:` coordinate is measured against is the one the FILE stores
/// (`P1-portrait-mask-frame.md` §1: a portrait export's mask numbers are
/// fractions of the 9504 × 6336 sensor array even though its pixels are
/// 6336 × 9504). Feeding the display dimensions here inverted the aspect for
/// every portrait capture — the projection's `s = W/H` came out as `H/W`.
///
/// `quarter_turns` is the photographer's own rotation, composed onto the EXIF
/// state, so the declared `tiff:Orientation` describes the frame this build
/// actually displays — closing the Batch-2 registration that a sidecar written
/// for a turned photo described the un-turned frame.
fn photo_frame_aspect(photo: &Path, quarter_turns: u8) -> Option<xmp::FrameAspect> {
    /// `(un-turned source size, the file's own EXIF orientation)`.
    type SourceFrame = ((usize, usize), rawler::Orientation);
    static MEMO: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<CurveMemoKey, SourceFrame>>,
    > = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let key = (photo.to_path_buf(), curve_ident(photo));
    let cached = memo.lock().ok().and_then(|m| m.get(&key).copied());
    let ((w, h), exif) = match cached {
        Some(v) => v,
        None => {
            // Inabilities are NOT cached — a locked file must be retried, the
            // rule the curve and orientation memos already follow.
            let v = crate::decode::source_frame(photo).ok()?;
            match memo.lock() {
                Ok(mut m) => *m.entry(key).or_insert(v),
                Err(_) => v,
            }
        }
    };
    xmp::FrameAspect::from_size_turned(
        w as f64,
        h as f64,
        crate::render::compose_orientation(exif, quarter_turns),
    )
}

/// Where the .xmp for `raw` goes — the photo's central develop dir (see
/// `store`; the photo library itself stays read-only). Kept here because every
/// surface already imports it from pipeline.
pub fn xmp_target(raw: &Path) -> PathBuf {
    crate::store::xmp_target(raw)
}

/// By default, AutoShade keeps the source library read-only. If the configured
/// Delivery folder is inside or above a photo’s folder, that delivery subtree is
/// intentionally writable; Settings warns when this removes the folder’s
/// protection. “Export .xmp beside the photo” is the separate, confirmed
/// per-photo sidecar exception.
///
/// The PROJECT's ./out and the per-user store root are always writable, even
/// when the source itself lives there (e.g. `match` fitting a look onto a
/// previously exported preview) — the rule protects the photo LIBRARY, not our
/// own output areas. A folder that merely happens to be NAMED "out" inside the
/// library is still refused.
/// Fold `.`/`..` LEXICALLY (no filesystem access — the target may not exist
/// yet). Shared by [`guard_readonly`] and the CLI's canonical-path equality
/// check (`-o` spelled with `dir/../` segments must still classify as the
/// canonical file). See the RootDir note inside for the drive-relative trap.
pub fn normalize_lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut n = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => match n.components().next_back() {
                Some(Component::Normal(_)) => {
                    n.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {
                    // The root's parent IS the root: "D:/../lib" folds to
                    // "D:/lib" exactly as the filesystem resolves it.
                    // Popping the root would yield drive-relative "D:lib",
                    // which dodges starts_with checks.
                }
                _ => n.push(".."),
            },
            Component::CurDir => {}
            other => n.push(other.as_os_str()),
        }
    }
    n
}

/// Canonicalize the deepest EXISTING ancestor, then re-attach the
/// not-yet-created tail: junction/subst/symlink aliases and case-flipped
/// spellings resolve to the true on-disk form even when the LEAF is absent.
/// Shared by [`guard_readonly`] and the CLI's canonical-path equality check.
pub fn resolve_existing_pub(p: &Path) -> PathBuf {
    let mut cur = p;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(mut c) = std::fs::canonicalize(cur) {
            for t in tail.iter().rev() {
                c.push(t);
            }
            return c;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(par), Some(name)) if !par.as_os_str().is_empty() => {
                tail.push(name);
                cur = par;
            }
            _ => return p.to_path_buf(),
        }
    }
}

pub fn guard_readonly(out: &Path, raw: &Path) -> Result<()> {
    use std::path::absolute;
    // Fold `.`/`..` LEXICALLY (no filesystem access — the target may not
    // exist yet): `std::path::absolute` keeps `..` segments, so a path like
    // "out/../<library>/x.tif" used to START WITH ./out for the allow-check
    // while the filesystem resolved it INTO the protected library.
    let normalize = normalize_lexical; // extracted for the CLI's same_path
    // Canonicalize the deepest EXISTING ancestor, then re-attach the
    // not-yet-created tail: a junction / subst / symlink alias or a
    // case-flipped spelling ("d:\photography" for "D:\Photography" — NTFS is
    // case-insensitive, starts_with is not) otherwise denotes the library
    // while failing every lexical comparison below. Canonical paths carry the
    // TRUE on-disk casing and resolved links, and all sides go through the
    // same lens, so the \\?\ verbatim prefix cancels out.
    let resolve_existing = resolve_existing_pub; // extracted for same_path
    let (Ok(out_abs), Ok(raw_abs)) = (absolute(out), absolute(raw)) else {
        return Ok(());
    };
    let (out_abs, raw_abs) =
        (resolve_existing(&normalize(&out_abs)), resolve_existing(&normalize(&raw_abs)));
    if out_abs.starts_with(resolve_existing(&crate::store::store_root())) {
        return Ok(());
    }
    // OUR own output areas. Two entries, not one: the configured delivery
    // root (M8) and the historical `./out`, which keeps holding the pixel
    // masters an older develop already links to — a user who repoints the
    // root must not find that `match`-ing on one of those old exports is
    // suddenly refused. Both are `Destination`-trusted or literal, so no
    // planted file can widen this allowance (see `config::SETTINGS`).
    for own in [crate::config::delivery_root(), PathBuf::from(crate::config::DEFAULT_DELIVERY_ROOT)]
    {
        if let Ok(own_out) = absolute(&own)
            && out_abs.starts_with(resolve_existing(&normalize(&own_out)))
        {
            return Ok(());
        }
    }
    if let Some(raw_dir) = raw_abs.parent()
        && out_abs.starts_with(raw_dir) {
            anyhow::bail!(
                "refusing to write into the source RAW's folder ({}) — the photo library is \
                 read-only. Write outputs to ./out (the default) instead.",
                raw_dir.display()
            );
        }
    // NEVER clobber an existing PHOTO outside our own output areas. The rule
    // above guards only the RAW's OWN folder, so a SIBLING folder of the same
    // library (D:/Photos/TripB while the RAW sits in D:/Photos/TripA) was
    // fully writable and `-o` could destroy a library photo. We cannot know
    // where the library ROOT is — there is no such setting — but we do not
    // need to: outside ./out and the develop store (both allowed above,
    // where overwriting OUR OWN deliverables is the point), refusing to
    // replace an existing image file is stricter than any root guess and
    // needs no configuration. Writing a NEW file elsewhere stays allowed:
    // the user asked for that path, and nothing of theirs is lost.
    if is_source(&out_abs) && out_abs.exists() {
        anyhow::bail!(
            "refusing to overwrite the existing photo {} — outputs must not replace photos \
             outside ./out. Pick a new name, or write to ./out (the default).",
            out_abs.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod visual_round_tests {
    use super::*;
    use crate::advisor::{AdvisorError, Decision, Judgement};

    fn j(score: f32) -> Judgement {
        Judgement { score, decision: Decision::Revise, critique: "c".into(), hint: None }
    }

    fn accept() -> Verdict {
        Verdict { decision: Decision::Accept, reasons: vec![], revised_hint: None }
    }

    /// The adopt-or-keep contract: ≥ adopts (equality included — the judge
    /// asked for the round), < keeps, and EVERY failure keeps the reviewed
    /// recipe — a revision that cannot be compared must never replace it.
    #[test]
    fn a_revision_replaces_the_recipe_only_when_the_rejudge_holds_the_score() {
        let cur = EditRecipe::default();
        let revised = || {
            Ok(EditRecipe { exposure_ev: 1.25, ..Default::default() })
        };
        // Higher re-score adopts, and carries the fresh verdict.
        match visual_review_round("h", &cur, |_| revised(), |_| Ok(accept()), |_| Ok(j(80.0)), 70.0)
        {
            VisualRound::Adopted { recipe, second, .. } => {
                assert_eq!(recipe.exposure_ev, 1.25, "the REVISED recipe rides out");
                assert_eq!(second.score, 80.0);
            }
            _ => panic!("a higher re-score adopts"),
        }
        // EQUAL re-score adopts too: the hint was followed, nothing was lost.
        assert!(matches!(
            visual_review_round("h", &cur, |_| revised(), |_| Ok(accept()), |_| Ok(j(70.0)), 70.0),
            VisualRound::Adopted { .. }
        ));
        // Lower re-score keeps (do-no-harm), reporting the losing score.
        assert!(matches!(
            visual_review_round("h", &cur, |_| revised(), |_| Ok(accept()), |_| Ok(j(69.9)), 70.0),
            VisualRound::KeptLower { second_score } if second_score == 69.9
        ));
        // The hint the judge wrote is what the proposer receives.
        let mut got = String::new();
        let _ = visual_review_round(
            "lift shadows",
            &cur,
            |h| {
                got = h.to_string();
                revised()
            },
            |_| Ok(accept()),
            |_| Ok(j(90.0)),
            70.0,
        );
        assert_eq!(got, "lift shadows");
    }

    /// R20-S3: a revision whose SETTINGS are identical short-circuits —
    /// prose differences (rationale/confidence) must not defeat the check,
    /// and neither verify nor re-judge may be paid for a no-op.
    #[test]
    fn an_identical_revision_short_circuits_without_paying_for_more_calls() {
        let cur = EditRecipe { exposure_ev: 0.5, rationale: "original prose".into(), ..Default::default() };
        let same_settings = EditRecipe {
            exposure_ev: 0.5,
            rationale: "fresh prose, same sliders".into(),
            confidence: 0.9,
            ..Default::default()
        };
        let out = visual_review_round(
            "h",
            &cur,
            |_| Ok(same_settings),
            |_| panic!("verify must not be paid for a no-op revision"),
            |_| panic!("re-judge must not be paid for a no-op revision"),
            70.0,
        );
        assert!(matches!(out, VisualRound::Unchanged));
    }

    /// R23-4 (feedback #13), the loop's own contract: how far one analysis may
    /// iterate, and who is allowed to pay for it.
    ///
    /// Two properties the cost guardrail rests on: the DEFAULT path keeps
    /// exactly R20's single round at every strength (nothing gets more
    /// expensive because a constant moved), and the extra rounds unlock only
    /// where the user asked — deep thinking, or a committed grade.
    #[test]
    fn the_convergence_ladder_is_gated_and_ordered() {
        let at = |s: f32, think: bool| judge_convergence(GradeStrength::new(s), think);
        // Ungated: one round, whatever the band — R20's budget, unchanged.
        for s in [0.0, 0.4, 0.5, GradeStrength::DEFAULT, 0.7] {
            assert_eq!(at(s, false).1, 1, "an unasked-for round must not appear at {s}");
        }
        // The committed band buys its own depth (the user asked for a bold
        // grade, and that is what needs the iterations).
        assert_eq!(at(0.9, false), (JUDGE_TARGET_COMMITTED, 3));
        // Deep thinking buys it in the two lower bands too.
        assert_eq!(at(0.2, true), (JUDGE_TARGET_RESTRAINED, 1), "restrained stays one round");
        assert_eq!(at(GradeStrength::DEFAULT, true), (JUDGE_TARGET_BALANCED, 2));
        assert_eq!(at(0.9, true), (JUDGE_TARGET_COMMITTED, 3));
        // The targets rise with the band — a bolder develop is finished later.
        // (Read through `judge_convergence` rather than the consts directly:
        // clippy refuses a compile-time-constant assertion, and the property
        // that matters is the one the LOOP sees anyway.)
        let targets: Vec<f32> = [0.2, 0.5, 0.9].iter().map(|&s| at(s, true).0).collect();
        assert!(
            targets[0] < targets[1] && targets[1] < targets[2],
            "the ladder must be monotone: a stronger grade cannot have a LOWER bar: {targets:?}"
        );
        // …and every target sits inside judge.rs's stated rubric bands (below
        // 90 "ship as-is", above 75 "minor polish left"), or the loop would
        // either never stop or stop at a develop the rubric calls improvable.
        for t in [JUDGE_TARGET_RESTRAINED, JUDGE_TARGET_BALANCED, JUDGE_TARGET_COMMITTED] {
            assert!((75.0..=90.0).contains(&t), "{t} is outside the judge's own scale");
        }
    }

    /// The round predicate. R20's rule owns round 1 unconditionally; the target
    /// owns every round after it.
    #[test]
    fn the_round_predicate_keeps_r20s_first_round_and_then_converges() {
        let with = |score: f32, decision, hint: Option<&str>| Judgement {
            score,
            decision,
            critique: "c".into(),
            hint: hint.map(str::to_string),
        };
        let revise = |score, hint| with(score, Decision::Revise, hint);

        // Round 1 — exactly R20: a non-Accept verdict WITH an instruction.
        assert_eq!(judge_next_round(0, &revise(60.0, Some("lift")), 82.0, 2), Some("lift"));
        assert_eq!(
            judge_next_round(0, &revise(95.0, Some("lift")), 82.0, 2),
            Some("lift"),
            "the target must NOT suppress the historical first round — that would be a \
             silent behaviour change on the default path"
        );
        assert_eq!(judge_next_round(0, &revise(60.0, None), 82.0, 2), None, "no hint, no round");
        assert_eq!(
            judge_next_round(0, &with(60.0, Decision::Accept, Some("polish")), 82.0, 2),
            None,
            "an Accept's hint is advice, not a request"
        );
        assert_eq!(judge_next_round(0, &revise(60.0, Some("   ")), 82.0, 2), None, "blank hint");

        // Round 2+ — the target is the stop condition, and the cap is absolute.
        assert_eq!(judge_next_round(1, &revise(81.9, Some("more")), 82.0, 2), Some("more"));
        assert_eq!(
            judge_next_round(1, &revise(82.0, Some("more")), 82.0, 2),
            None,
            "at the target the loop stops even with budget left and a hint offered"
        );
        assert_eq!(judge_next_round(2, &revise(10.0, Some("more")), 82.0, 2), None, "cap");
        assert_eq!(judge_next_round(1, &revise(10.0, Some("more")), 82.0, 1), None, "cap");
    }

    /// The production loop's SHAPE, driven over canned score sequences with the
    /// real `visual_review_round` and the real predicate — the part that cannot
    /// be tested through `produce_recipe` without a RAW, a network and money.
    ///
    /// The three R20 decisions must survive multi-round化, and each one is a
    /// COUNT here rather than a claim: the same-settings short-circuit fires on
    /// EVERY round (not just the first), do-no-harm still ends the chain, and
    /// nothing runs at all past the round budget.
    #[test]
    fn a_multi_round_chain_converges_short_circuits_and_does_no_harm() {
        struct Run {
            rounds: usize,
            proposes: usize,
            verifies: usize,
            judges: usize,
            final_exposure: f32,
        }
        // `repeat_at`: the round on which the proposer returns the CURRENT
        // recipe's settings verbatim (the R20-3 short-circuit case).
        let drive = |scores: &[f32], strength: f32, think: bool, repeat_at: Option<usize>| -> Run {
            let (target, cap) = judge_convergence(GradeStrength::new(strength), think);
            let judged = |i: usize| Judgement {
                score: scores[i.min(scores.len() - 1)],
                decision: Decision::Revise,
                critique: "c".into(),
                hint: Some("keep going".into()),
            };
            let (mut proposes, mut verifies, mut judges) = (0usize, 0usize, 0usize);
            let mut recipe = EditRecipe::default();
            let mut current = judged(0);
            let mut rounds = 0usize;
            while let Some(hint) = judge_next_round(rounds, &current, target, cap) {
                assert_eq!(hint, "keep going");
                let next_round = rounds + 1;
                let outcome = visual_review_round(
                    hint,
                    &recipe,
                    |_| {
                        proposes += 1;
                        Ok(if repeat_at == Some(next_round) {
                            // Same SETTINGS, fresh prose — the shape the
                            // short-circuit exists to catch.
                            EditRecipe { rationale: "new words".into(), ..recipe.clone() }
                        } else {
                            EditRecipe { exposure_ev: next_round as f32 * 0.25, ..Default::default() }
                        })
                    },
                    |_| {
                        verifies += 1;
                        Ok(accept())
                    },
                    |_| {
                        judges += 1;
                        Ok(judged(next_round))
                    },
                    current.score,
                );
                rounds += 1;
                match outcome {
                    VisualRound::Adopted { recipe: r2, second, .. } => {
                        recipe = *r2;
                        current = second;
                    }
                    _ => break,
                }
            }
            Run {
                rounds,
                proposes,
                verifies,
                judges,
                final_exposure: recipe.exposure_ev,
            }
        };

        // Default path (balanced, no深度): ONE round, exactly as R20 shipped —
        // the score sequence would happily support three.
        let r = drive(&[60.0, 70.0, 80.0, 90.0], GradeStrength::DEFAULT, false, None);
        assert_eq!((r.rounds, r.proposes, r.verifies, r.judges), (1, 1, 1, 1));

        // Deep + committed: iterate to the cap while the score stays under the
        // target, adopting each improvement.
        let r = drive(&[60.0, 70.0, 80.0, 86.0], 0.9, true, None);
        assert_eq!((r.rounds, r.proposes, r.verifies, r.judges), (3, 3, 3, 3));
        assert_eq!(r.final_exposure, 0.75, "the last adopted candidate is the one kept");

        // …and it STOPS the moment the target is met, budget unspent.
        let r = drive(&[60.0, 92.0, 95.0, 99.0], 0.9, true, None);
        assert_eq!(
            (r.rounds, r.proposes, r.judges),
            (1, 1, 1),
            "92 clears the 88 target — a second round would be paid for nothing"
        );

        // R20 定案 3, per ROUND: the model repeating itself on round 2 must not
        // buy a verify or a re-judge, and must end the chain.
        let r = drive(&[60.0, 70.0, 80.0, 90.0], 0.9, true, Some(2));
        assert_eq!(
            (r.rounds, r.proposes, r.verifies, r.judges),
            (2, 2, 1, 1),
            "the second round proposed and short-circuited: 2 proposals, but only the \
             FIRST round's verify + re-judge were paid for"
        );

        // R20 定案 2, do-no-harm: a lower re-score ends the chain and keeps the
        // reviewed recipe (exposure stays at the previous round's value).
        let r = drive(&[60.0, 70.0, 55.0, 99.0], 0.9, true, None);
        assert_eq!((r.rounds, r.proposes, r.judges), (2, 2, 2));
        assert_eq!(r.final_exposure, 0.25, "the losing candidate must not survive");
    }

    /// The cost guardrail the consensus called for by name: the unattended
    /// surfaces cannot think, and cannot be made to by a default.
    ///
    /// `produce_recipe` calls the proposer BEFORE any judge gate
    /// (`openai.propose_planned(...)`, unconditional), so `judge: false` alone
    /// does not protect batch or eval from a thinking envelope — only the
    /// request they build does. Both build it here.
    #[test]
    fn the_unattended_surfaces_can_never_think() {
        assert!(!GradeRequest::default().think, "the struct default is the batch default");
        assert!(
            !GradeRequest::with_style(0.3).think,
            "`batch` builds its request through with_style — it has no way to reach the flag"
        );
        // eval's own literal (eval.rs): the calibration point plus struct defaults.
        let eval_req = GradeRequest {
            strength: GradeStrength::calibrated(),
            ..Default::default()
        };
        assert!(!eval_req.think, "eval must keep measuring the BARE proposal");
        assert_eq!(eval_req.strength.get(), GradeStrength::CALIBRATED);
        // And the judge loop cannot iterate for them either: eval/batch pass
        // judge=false, but even if that changed, the ungated cap is 1.
        assert_eq!(judge_convergence(eval_req.strength, eval_req.think).1, 1);
    }

    #[test]
    fn every_failure_keeps_the_reviewed_recipe() {
        let cur = EditRecipe::default();
        // Distinct settings, so the no-op short-circuit never masks the
        // failure arms under test.
        let changed = || Ok(EditRecipe { exposure_ev: 0.5, ..Default::default() });
        let boom = || AdvisorError::Transport("boom".into());
        assert!(matches!(
            visual_review_round("h", &cur, |_| Err(boom()), |_| Ok(accept()), |_| Ok(j(99.0)), 70.0),
            VisualRound::RoundFailed { .. }
        ));
        assert!(matches!(
            visual_review_round("h", &cur, |_| changed(), |_| Err(boom()), |_| Ok(j(99.0)), 70.0),
            VisualRound::RoundFailed { .. }
        ));
        // A revision that cannot be RE-JUDGED is discarded, not gambled on —
        // its own arm, so the disclosure can say the revision existed.
        assert!(matches!(
            visual_review_round("h", &cur, |_| changed(), |_| Ok(accept()), |_| Err(boom()), 70.0),
            VisualRound::RejudgeFailed { .. }
        ));
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn refine_keeps_what_the_ai_schema_cannot_return() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        // The photographer's edit: an AI-selected sky (bitmap, with recolour
        // gains), a hand-drawn radial, and manual lens corrections.
        let base = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Bitmap { path: "mask-sky.png".into() },
                    color_gains: Some([1.2, 1.0, 0.8]),
                    exposure_ev: -0.5,
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.2, left: 0.2, bottom: 0.8, right: 0.8,
                        feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                        midpoint: 50.0, mask_version: 2,
                    },
                    ..Default::default()
                },
            ],
            lens_distortion: -8.0,
            lens_vignette: 30.0,
            lens_vignette_mid: 40.0,
            lens_profile: crate::recipe::LensProfile {
                distortion: vec![1.0, 1.001, 1.002],
                distortion_on: false, // the user's UNSAVED toggle-off
                ..Default::default()
            },
            ..Default::default()
        };
        // What the model can return: parametric masks only, lens fields absent
        // from the schema so they deserialize to their defaults.
        let mut proposed = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear {
                    zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 0.4,
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        carry_over_unrepresentable(&mut proposed, &base, LensOpinion::default(), None);
        assert_eq!(proposed.masks.len(), 2, "the bitmap mask must survive Refine");
        // (the lens assertions below are the SILENT half of this test: with no
        // opinion stated, every manual correction is still carried back)
        let MaskGeometry::Bitmap { path } = &proposed.masks[0].mask else {
            panic!("the carried mask must come first, got {:?}", proposed.masks[0].mask)
        };
        assert_eq!(path, "mask-sky.png");
        assert_eq!(proposed.masks[0].color_gains, Some([1.2, 1.0, 0.8]), "gains too");
        assert_eq!(proposed.lens_distortion, -8.0, "manual lens correction kept");
        assert_eq!(proposed.lens_vignette, 30.0);
        assert_eq!(proposed.lens_vignette_mid, 40.0);
        assert_eq!(
            proposed.lens_profile, base.lens_profile,
            "the profile (with the user's unsaved toggles) must survive Refine"
        );
        // A STRIPPED-SENTINEL base (Default profile) must NOT overwrite the
        // stamped profile with emptiness.
        let mut stamped = EditRecipe::default();
        stamped.lens_profile.vignette = vec![1.0, 0.9];
        stamped.lens_profile.vignette_on = true;
        carry_over_unrepresentable(&mut stamped, &EditRecipe::default(), LensOpinion::default(), None);
        assert!(stamped.lens_profile.vignette_on, "sentinel base leaves the stamp alone");
        assert!(!stamped.lens_profile.vignette.is_empty());
        // The model's own proposal is still there, after the carried one.
        assert!(matches!(proposed.masks[1].mask, MaskGeometry::Linear { .. }));
    }

    /// R25 B2: every `Tier::CarriedOnly` global survives a Refine.
    ///
    /// These are the one class of recipe value with NO other way back: the
    /// schema never asks the model for them (they are `engine_only`), and
    /// unlike `base_curve` / `lens_profile` / the WB anchor nothing re-stamps
    /// them per photo. Since B2 also made the XMP writer OWN their keys, the
    /// merge strips them from the Lightroom sidecar before rewriting — so a
    /// dropped value is not "the app forgot", it is the photographer's own
    /// grain and post-crop vignette deleted from the file beside their RAW.
    ///
    /// The field list is re-derived from the registry through serde, not
    /// transcribed: the B3/B4 batches add more CarriedOnly rows, and this
    /// fails on the first one that `carry_over_unrepresentable` does not copy.
    #[test]
    fn carried_effects_survive_a_refine() {
        use crate::advisor::catalogue::{Shape, RECIPE_CONTROLS};
        // The CarriedOnly rows, PLUS the two engine-only rows that render
        // (`ca_r`/`ca_b`, R25 B3). The derivation cannot reach those from the
        // tier — they are `Rendered` — but they are in exactly the same
        // position: the schema never asks for them, so this block is their
        // only way home. Naming them here keeps the pair from being the one
        // field the widening test cannot see.
        //
        // R25 B4 widened the SIEVE, not just the list: the filter is now every
        // tier that renders nothing (`CarriedOnly` ∪ `PassThrough`) rather than
        // the one tier B2 happened to create. `passthrough` is the case that
        // proves the point — it is not `CarriedOnly`, it is exactly as
        // un-restatable, and a `CarriedOnly`-only sieve would have let B4 ship
        // with no carry line and no test to say so.
        let carried: Vec<&str> = RECIPE_CONTROLS
            .iter()
            .filter(|c| {
                c.tier.is_some_and(|t| !t.renders() && t.owns_crs_key())
                    || matches!(c.name, "ca_r" | "ca_b")
            })
            .map(|c| c.name)
            .collect();
        assert!(carried.len() > 9, "premise: B3 widened the tier past B2's nine");
        assert!(
            carried.contains(&"passthrough"),
            "premise: the B4 PassThrough row must reach this sieve, or the widening was cosmetic"
        );
        for pair in ["ca_r", "ca_b"] {
            assert!(carried.contains(&pair), "the rendered-but-engine-only pair must be probed");
        }
        // A base holding a non-neutral value for every one of them, built
        // through serde so a renamed field cannot slip past. Probed BY SHAPE:
        // B3 put a `Shape::Bool` on the list (`auto_lateral_ca`), and 3.0 is
        // not a value a bool deserialises from.
        let neutral = EditRecipe::default();
        let mut json = serde_json::to_value(&neutral).expect("recipe serialises");
        for name in &carried {
            let shape = RECIPE_CONTROLS
                .iter()
                .find(|c| c.name == *name)
                .map(|c| c.shape)
                .expect("a registry row");
            json[*name] = match shape {
                Shape::Bool => serde_json::json!(true),
                // B4's row is a key → verbatim-string MAP, and `3.0` is not a
                // value it deserialises from. One real Lightroom spelling,
                // first-hand from the reference sidecars.
                Shape::EngineCarrier => serde_json::json!({ "PerspectiveVertical": "-35" }),
                _ => serde_json::json!(3.0),
            };
        }
        let mut base: EditRecipe =
            serde_json::from_value(json).expect("the probe values are in range");
        base.clamp();
        // Read through SERDE, not through `global_value`: the latter answers
        // for the scalar shapes, and B4's carrier is a map. Comparing the
        // serialised value covers every shape the sieve can ever hand us — and
        // it is the same "against the DEFAULT, never against zero" rule
        // `global_value` was serving here (de-fringe's neutral is Adobe's own
        // 30/70/40/60, so "moved" cannot mean "non-zero").
        let field = |r: &EditRecipe, name: &str| {
            serde_json::to_value(r).ok().and_then(|v| v.get(name).cloned())
        };
        for name in &carried {
            assert!(
                field(&base, name).is_some(),
                "{name}: the registry names a field serde cannot see"
            );
            assert_ne!(
                field(&base, name),
                field(&neutral, name),
                "{name}: the probe must actually move the control"
            );
        }
        // What the model returns: a fresh recipe with no such fields at all.
        let mut proposed = EditRecipe { exposure_ev: 0.4, ..Default::default() };
        carry_over_unrepresentable(&mut proposed, &base, LensOpinion::default(), None);
        for name in &carried {
            assert_eq!(
                field(&proposed, name),
                field(&base, name),
                "{name}: a Refine deleted a carried value the model was never asked for"
            );
        }
        assert_eq!(proposed.exposure_ev, 0.4, "…without undoing the refine itself");
    }

    /// R23-1b: the lens trio is IN the strict schema now, so the carry-over
    /// stopped being unconditional — and both halves of that have to hold at
    /// once, which is exactly what made the change a trap.
    ///
    /// A silent overwrite (the old code) makes the schema addition a no-op on
    /// the Refine path: the model states a correction and it is discarded
    /// before the recipe is saved. Dropping the overwrite entirely re-opens the
    /// bug it was written for: the model says nothing (null, or no such field
    /// at all) and the photographer's hand-dialled correction defaults to 0,
    /// silently re-warping the frame.
    #[test]
    fn a_refine_keeps_the_lens_values_the_model_did_not_speak_about() {
        let base = EditRecipe {
            lens_distortion: -8.0,
            lens_vignette: 30.0,
            lens_vignette_mid: 40.0,
            ..Default::default()
        };
        // The model answered on distortion only; the two vignette fields came
        // back null, which `take_lens_opinion` turned into the engine defaults.
        let stated = LensOpinion { distortion: true, vignette: false, vignette_mid: false };
        let mut proposed =
            EditRecipe { lens_distortion: 12.0, lens_vignette: 0.0, lens_vignette_mid: 50.0, ..Default::default() };
        carry_over_unrepresentable(&mut proposed, &base, stated, None);
        assert_eq!(proposed.lens_distortion, 12.0, "a STATED correction is the model's answer");
        assert_eq!(proposed.lens_vignette, 30.0, "an unstated one keeps the photographer's");
        assert_eq!(proposed.lens_vignette_mid, 40.0);

        // The zero that MEANS zero: the model stated 0, which the catalogue
        // tells it is "clear the manual correction". Indistinguishable from a
        // null without the opinion flag — this is why the flag exists.
        let mut clearing = EditRecipe::default();
        carry_over_unrepresentable(
            &mut clearing,
            &base,
            LensOpinion { distortion: true, vignette: true, vignette_mid: true },
            None,
        );
        assert_eq!(
            (clearing.lens_distortion, clearing.lens_vignette, clearing.lens_vignette_mid),
            (0.0, 0.0, 50.0),
            "an explicit zero must not be read as 'no opinion'"
        );
    }

    /// R23-1b: a RADIAL mask's rotation entered the schema, so a refine must
    /// return the model's angle instead of having the base's re-imposed on it —
    /// and an angle is no longer a reason to treat the mask as state-bearing
    /// (which used to drag its whole geometry back).
    #[test]
    fn a_refine_takes_the_models_radial_rotation() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        let radial = |angle: f32, top: f32| MaskGeometry::Radial {
            top,
            left: 0.2,
            bottom: 0.8,
            right: 0.8,
            feather: 0.5,
            roundness: 0.0,
            flipped: false,
            angle,
            midpoint: 50.0,
            mask_version: 2,
        };
        let base = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "subject".into(),
                mask: radial(15.0, 0.2),
                ..Default::default()
            }],
            ..Default::default()
        };
        // The model rotated it further AND moved the top edge.
        let mut proposed = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "subject".into(),
                mask: radial(-40.0, 0.35),
                ..Default::default()
            }],
            ..Default::default()
        };
        carry_over_unrepresentable(&mut proposed, &base, LensOpinion::default(), None);
        assert_eq!(
            proposed.masks[0].mask,
            radial(-40.0, 0.35),
            "the rotation is the model's to set now — the whole geometry is its answer"
        );
        assert_eq!(proposed.masks.len(), 1, "a rotated ellipse is not 'unrepresentable' any more");
    }

    /// R27 L-09 (user ruling 2026-08-19 =「修」). `crs:Midpoint` and
    /// `crs:Version` are carried-only: the AI schema cannot ask for them, so
    /// every Refine used to hand back 50 / 2 and the next save wrote those
    /// numbers into the photographer's sidecar over whatever Lightroom had
    /// said. The library happens to hold no non-default Midpoint, which is why
    /// it was registered rather than fixed — a zero-instance defect is still a
    /// defect, and the fixture here is the instance.
    ///
    /// The SECOND half is the conservatism the ruling asked for: nothing but
    /// those two fields moves. The model's own answer for the rest of the
    /// geometry — a rotation, a re-drawn box — must survive untouched, or this
    /// pass would have quietly become the wholesale revert R25 declined to
    /// widen into.
    ///
    /// MUTATION THIS CATCHES: delete the `carry_radial_carried_attributes`
    /// call and the first pair of asserts fails at 50 / 2 (the pre-R27 state);
    /// copy the whole `original.mask` instead of the two fields and the angle
    /// assert fails, because the model's rotation would be thrown away.
    #[test]
    fn a_refine_keeps_the_photographers_radial_midpoint() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        let radial = |angle: f32, midpoint: f32, mask_version: u32| MaskGeometry::Radial {
            top: 0.2,
            left: 0.2,
            bottom: 0.8,
            right: 0.8,
            feather: 0.5,
            roundness: 0.0,
            flipped: false,
            angle,
            midpoint,
            mask_version,
        };
        // An imported Lightroom radial whose Midpoint the photographer moved.
        let base = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "subject".into(),
                mask: radial(15.0, 30.0, 3),
                ..Default::default()
            }],
            ..Default::default()
        };
        // What the model returns: the same mask by name, re-rotated, with the
        // two fields at the defaults its schema forces on them.
        let mut proposed = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "subject".into(),
                mask: radial(-40.0, 50.0, 2),
                exposure_ev: 0.3,
                ..Default::default()
            }],
            ..Default::default()
        };
        carry_over_unrepresentable(&mut proposed, &base, LensOpinion::default(), None);
        let MaskGeometry::Radial { midpoint, mask_version, angle, top, .. } = proposed.masks[0].mask
        else {
            panic!("expected a radial, got {:?}", proposed.masks[0].mask);
        };
        assert_eq!(midpoint, 30.0, "the photographer's Midpoint must survive a Refine");
        assert_eq!(mask_version, 3, "…and so must Lightroom's own geometry version");
        assert_eq!(angle, -40.0, "but the model's rotation is still the model's answer");
        assert_eq!(top, 0.2, "and nothing else in the geometry moved");
        assert_eq!(proposed.masks[0].exposure_ev, 0.3, "nor anything else in the adjustment");

        // A response that renamed the mask (or sent back two answering to the
        // same name) is not identifiable, so nothing is copied onto a shape
        // that may not be the same shape.
        let mut renamed = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "subject 2".into(),
                mask: radial(-40.0, 50.0, 2),
                ..Default::default()
            }],
            ..Default::default()
        };
        carry_over_unrepresentable(&mut renamed, &base, LensOpinion::default(), None);
        let MaskGeometry::Radial { midpoint, .. } = renamed.masks[0].mask else {
            panic!("expected a radial")
        };
        assert_eq!(midpoint, 50.0, "an unmatched name carries nothing");
    }

    #[test]
    fn guard_refuses_to_overwrite_a_photo_in_a_sibling_library_folder() {
        let base = std::env::temp_dir().join(format!("autoshade-guard-sibling-{}", std::process::id()));
        let (a, b) = (base.join("TripA"), base.join("TripB"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let raw = a.join("DSC1.arw");
        let victim = b.join("DSC2.jpg");
        std::fs::write(&raw, b"raw").unwrap();
        std::fs::write(&victim, b"an existing library photo").unwrap();
        // The pre-existing rule (same folder) still holds ...
        assert!(guard_readonly(&a.join("out.tif"), &raw).is_err(), "own folder");
        // ... and a SIBLING folder's existing photo is now protected too.
        assert!(guard_readonly(&victim, &raw).is_err(), "sibling photo must be refused");
        // A NEW file in that sibling folder stays allowed — nothing is lost.
        assert!(guard_readonly(&b.join("brand-new.tif"), &raw).is_ok(), "new file allowed");
        // A non-photo file is not ours to protect.
        let notes = b.join("notes.txt");
        std::fs::write(&notes, b"x").unwrap();
        assert!(guard_readonly(&notes, &raw).is_ok(), "non-photo allowed");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn write_xmp_merges_over_the_lightroom_sidecar_beside_the_raw() {
        let dir = std::env::temp_dir().join(format!("autoshade-pipe-xmp-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_pipe_xmp_merge.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        // Lightroom's sidecar beside the RAW, carrying a property we do not
        // model (PointColor) — the exact thing regeneration used to destroy.
        let lr = dir.join("_pipe_xmp_merge.xmp");
        std::fs::write(
            &lr,
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
             <rdf:Description rdf:about=\"\"\n    \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
             crs:PointColor=\"0\"\n    crs:Exposure2012=\"+1.00\"\n    \
             crs:HasSettings=\"True\">\n  </rdf:Description>\n \
             </rdf:RDF>\n</x:xmpmeta>\n",
        )
        .unwrap();
        let r = EditRecipe { exposure_ev: 0.75, ..Default::default() };
        let (out, _, _) = write_xmp(&raw, &r, crate::diag::stderr()).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("crs:PointColor=\"0\""), "LR-only property survives the save");
        assert_eq!(text.matches("crs:Exposure2012=").count(), 1, "ours replaces, never duplicates");
        assert!(text.contains("crs:Exposure2012=\"0.75\""), "…with OUR value");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// L05#4 end-to-end: a save whose recipe HAS masks over a sidecar
    /// carrying a foreign (brush) mask block publishes the develop's own
    /// masks and RETURNS the loss note — before, the projection showed the
    /// old pass's masks, suppressed ours, and reported plain success.
    #[test]
    fn a_save_with_own_masks_reports_the_foreign_mask_block_it_replaced() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        let dir = std::env::temp_dir().join(format!("autoshade-pipe-xmp-maskintent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_pipe_maskintent.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        let lr = dir.join("_pipe_maskintent.xmp");
        std::fs::write(
            &lr,
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
             <rdf:Description rdf:about=\"\"\n    \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
             crs:PointColor=\"0\" crs:HasSettings=\"True\">\n   \
             <crs:MaskGroupBasedCorrections><rdf:Seq>\n    \
             <rdf:li><rdf:Description crs:What=\"Correction\" \
             crs:CorrectionActive=\"true\">\n     \
             <crs:CorrectionMasks><rdf:Seq>\n      \
             <rdf:li crs:What=\"Mask/Brush\"/>\n     \
             </rdf:Seq></crs:CorrectionMasks>\n    \
             </rdf:Description></rdf:li>\n   \
             </rdf:Seq></crs:MaskGroupBasedCorrections>\n  \
             </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n",
        )
        .unwrap();
        let mut r = EditRecipe { exposure_ev: 0.75, ..Default::default() };
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: 0.2,
                left: 0.2,
                bottom: 0.8,
                right: 0.8,
                feather: 0.5,
                roundness: 0.0,
                flipped: false,
                angle: 0.0,
                midpoint: 50.0,
                mask_version: 2,
            },
            exposure_ev: 0.4,
            ..Default::default()
        });
        let (out, note, _) = write_xmp(&raw, &r, crate::diag::stderr()).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("Mask/CircularGradient"), "the develop's mask is published");
        assert!(!text.contains("Mask/Brush"), "the foreign block is not resurrected");
        assert!(text.contains("crs:PointColor=\"0\""), "LR-only globals still survive");
        let note = note.expect("the replaced block must be disclosed");
        assert!(note.contains("mask block carries"), "the note names the loss: {note}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// M6a end-to-end: the WRITE path hands its per-mask projection losses back
    /// to the caller, judged on the CLAMPED recipe it actually published. Every
    /// surface (GUI status line, web reply, CLI stderr) renders THIS list, so a
    /// boundary that dropped it would put every export-side disclosure back to
    /// the silence it was built to end — and no test of the writer alone would
    /// notice.
    #[test]
    fn the_write_path_hands_back_what_the_projection_could_not_carry() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        let dir = std::env::temp_dir().join(format!("autoshade-pipe-xmp-masklosses-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_pipe_masklosses.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        let r = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Bitmap { path: "out/sky.png".into() },
                    name: "sky".into(),
                    exposure_ev: -0.5,
                    ..Default::default()
                },
                // Exportable: nothing about it may reach the list.
                LocalAdjustment { name: "grad".into(), exposure_ev: 0.4, ..Default::default() },
            ],
            ..Default::default()
        };
        let (out, _, losses) = write_xmp(&raw, &r, crate::diag::stderr()).unwrap();
        assert_eq!(
            losses,
            vec![xmp::MaskLoss { name: "sky".into(), reason: xmp::MaskLossReason::Bitmap }],
            "the write path must report the raster mask it skipped, and only that"
        );
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            text.matches("crs:What=\"Correction\"").count(),
            1,
            "…which is what the published document says too"
        );
        // A faithful projection reports an EMPTY list (not a missing one): the
        // surfaces stay silent on that, and an always-non-empty list would make
        // every clean save shout.
        let clean = EditRecipe { masks: vec![r.masks[1].clone()], ..Default::default() };
        let (_, _, none) = write_xmp(&raw, &clean, crate::diag::stderr()).unwrap();
        assert!(none.is_empty(), "an exportable develop loses nothing: {none:?}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// The persistence boundary enforces the schema's size caps itself.
    /// `EditRecipe::clamp` names "a hostile POST to the local web server" as
    /// the reason those caps exist, yet the web route that WRITES skipped it:
    /// a 100 000-mask body became the photo's authoritative recipe.json,
    /// re-parsed by every later open and copied into the next version snapshot.
    #[test]
    fn a_persisted_recipe_cannot_exceed_the_schema_caps() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        let dir = std::env::temp_dir().join(format!("autoshade-pipe-recipe-caps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_pipe_caps.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);

        let hostile = EditRecipe {
            masks: (0..1000)
                .map(|_| LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.3,
                        left: 0.3,
                        bottom: 0.7,
                        right: 0.7,
                        feather: 0.5,
                        roundness: 0.0,
                        flipped: false,
                        angle: 0.0,
                        midpoint: 50.0,
                        mask_version: 2,
                    },
                    exposure_ev: 1.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let out = write_recipe(&raw, &hostile, None, crate::diag::stderr()).unwrap();
        let on_disk: EditRecipe =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(on_disk.masks.len(), 64, "the file on disk is within the caps");
        // The CALLER's recipe is not mutated — the clamp applies to the copy
        // that is written, so a render in flight keeps its own state.
        assert_eq!(hostile.masks.len(), 1000, "the caller's in-memory recipe is untouched");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// A sidecar the splicer cannot account for is REGENERATED rather than
    /// merged — which drops the user's Lightroom-only properties. The fallback
    /// itself is correct (never fail a save); doing it silently was the defect,
    /// because the loss only surfaced the next time they opened the catalog.
    #[test]
    fn an_unmergeable_sidecar_is_regenerated_and_says_so() {
        let dir = std::env::temp_dir().join(format!("autoshade-pipe-xmp-disclose-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_pipe_disclose.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        let lr = dir.join("_pipe_disclose.xmp");
        let r = EditRecipe { exposure_ev: 0.75, ..Default::default() };

        // A mergeable base: no note, and the LR-only property survives.
        std::fs::write(
            &lr,
            "<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:PointColor=\"0\"></rdf:Description>\n",
        )
        .unwrap();
        let (out, note, _) = write_xmp(&raw, &r, crate::diag::stderr()).unwrap();
        assert!(note.is_none(), "a merge that worked has nothing to disclose: {note:?}");
        assert!(std::fs::read_to_string(&out).unwrap().contains("crs:PointColor=\"0\""));

        // An UNTERMINATED description is markup the splicer cannot account
        // for: the save still succeeds, the PointColor is gone, and the caller is
        // told exactly that.
        std::fs::write(
            &lr,
            "<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:PointColor=\"0\">\n",
        )
        .unwrap();
        let (out, note, _) = write_xmp(&raw, &r, crate::diag::stderr()).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("crs:Exposure2012=\"0.75\""), "the save still happened");
        assert!(!text.contains("crs:PointColor"), "…by regenerating, so the LR property is gone");
        let note = note.expect("the loss must be disclosed, not silent");
        assert!(note.contains("regenerated"), "the note names what happened: {note}");
        // R25 B4: the example the note gives used to be "camera profile /
        // Look". `crs:CameraProfile` rides in `EditRecipe::passthrough` now,
        // so a REGENERATED file still carries it and naming it here would
        // make the disclosure false; the creative `Look` is a nested element
        // with no string spelling and is still genuinely lost.
        assert!(note.contains("creative Look"), "…and what it cost: {note}");
        assert!(!note.contains("camera profile"), "…which is no longer the profile: {note}");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    /// The disclosure is only worth having if it is TRUE. Three shapes made it
    /// false: a blank sidecar "lost" properties it never had (a note on every
    /// save, forever); a baked photo's neighbouring .xmp — someone else's
    /// file, which store::lightroom_sidecar refuses to interpret — was read
    /// and warned about anyway; and every note named the OUTPUT path as "the
    /// existing sidecar that could not be merged", pointing the user at a
    /// file that was never the problem.
    #[test]
    fn the_merge_note_fires_only_for_a_real_loss_and_names_the_base_file() {
        let dir = std::env::temp_dir().join(format!("autoshade-pipe-xmp-truthful-note-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = EditRecipe { exposure_ev: 0.75, ..Default::default() };

        // A blank (0-byte) sidecar beside the RAW: nothing to merge, nothing
        // to lose, no note.
        let raw = dir.join("_pipe_note_blank.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::write(dir.join("_pipe_note_blank.xmp"), b"").unwrap();
        let (_, note, _) = write_xmp(&raw, &r, crate::diag::stderr()).unwrap();
        assert!(note.is_none(), "an empty sidecar carries nothing to disclose: {note:?}");
        // Whitespace-only is the same emptiness.
        std::fs::write(dir.join("_pipe_note_blank.xmp"), b"  \n\t\n").unwrap();
        let (_, note, _) = write_xmp(&raw, &r, crate::diag::stderr()).unwrap();
        assert!(note.is_none(), "whitespace is not a merge base: {note:?}");
        let _ = std::fs::remove_dir_all(&dev);

        // A baked photo: its neighbouring .xmp is not ours to interpret, so
        // even an unmergeable one is no concern of the save.
        let png = dir.join("_pipe_note_baked.png");
        std::fs::write(&png, b"png").unwrap();
        let dev_png = crate::store::develop_dir(&png);
        let _ = std::fs::remove_dir_all(&dev_png);
        std::fs::write(
            dir.join("_pipe_note_baked.xmp"),
            "<rdf:Description rdf:about=\"\" xmlns:crs=\"c\" crs:PointColor=\"0\">\n",
        )
        .unwrap();
        let (_, note, _) = write_xmp(&png, &r, crate::diag::stderr()).unwrap();
        assert!(note.is_none(), "a baked photo's neighbour .xmp is not read: {note:?}");
        let _ = std::fs::remove_dir_all(&dev_png);

        // A well-formed RATINGS sidecar (no crs at all) beside the RAW — the
        // population the backlog actually named: exiftool / Bridge / Capture
        // One keywords and stars. There is nothing of ours to splice into, so
        // the old code regenerated and reported the loss on EVERY save,
        // forever, with no action the user could take. It is now a real merge:
        // the rating survives, our settings land, and there is nothing to
        // disclose.
        let raw2 = dir.join("_pipe_note_ratings.arw");
        std::fs::write(&raw2, b"raw").unwrap();
        let dev2 = crate::store::develop_dir(&raw2);
        let _ = std::fs::remove_dir_all(&dev2);
        let lr2 = dir.join("_pipe_note_ratings.xmp");
        let ratings = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
             <rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" \
             xmp:Rating=\"5\" xmp:Label=\"Green\"></rdf:Description>\n \
             </rdf:RDF>\n</x:xmpmeta>\n";
        std::fs::write(&lr2, ratings).unwrap();
        let (out2, note, _) = write_xmp(&raw2, &r, crate::diag::stderr()).unwrap();
        assert!(note.is_none(), "a foreign sidecar we CAN merge has no loss to report: {note:?}");
        let merged = std::fs::read_to_string(&out2).unwrap();
        assert!(merged.contains("xmp:Rating=\"5\""), "the user's rating survives: {merged}");
        assert!(merged.contains("xmp:Label=\"Green\""), "…and their label");
        assert!(merged.contains("crs:Exposure2012=\"0.75\""), "…and our settings landed");
        assert_eq!(
            std::fs::read_to_string(&lr2).unwrap(),
            ratings,
            "the sidecar beside the RAW is never written"
        );
        // Saving AGAIN must not stack a second Description: the merged store
        // copy now carries crs, so the ordinary merge path takes over.
        let (out2b, note, _) = write_xmp(&raw2, &r, crate::diag::stderr()).unwrap();
        let again = std::fs::read_to_string(&out2b).unwrap();
        assert!(note.is_none(), "the second save has nothing to disclose either: {note:?}");
        assert_eq!(again.matches("crs:Exposure2012=").count(), 1, "one settings block, not two");
        assert!(again.contains("xmp:Rating=\"5\""), "the rating still survives the re-save");
        let _ = std::fs::remove_dir_all(&dev2);

        // …and the splice point must be real MARKUP. A sidecar whose header
        // QUOTES the root tag in a comment used to get the settings block
        // spliced inside that comment: the merge then "succeeded", so no note
        // fired, and the file Lightroom reads carried none of the develop —
        // the same silence this test's sibling exists to end. (`xmp.rs`
        // already pins this property for the close scanner.)
        let raw4 = dir.join("_pipe_note_quoted.arw");
        std::fs::write(&raw4, b"raw").unwrap();
        let dev4 = crate::store::develop_dir(&raw4);
        let _ = std::fs::remove_dir_all(&dev4);
        for quoted in [
            "<!-- template root: <rdf:RDF xmlns:rdf=\"x\"> -->\n",
            "<![CDATA[ sample: <rdf:RDF xmlns:rdf=\"x\"> ]]>\n",
            "<?doc <rdf:RDF xmlns:rdf=\"x\"> ?>\n",
        ] {
            std::fs::write(
                dir.join("_pipe_note_quoted.xmp"),
                format!(
                    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n{quoted}\
                     <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
                     <rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" \
                     xmp:Rating=\"4\"></rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n"
                ),
            )
            .unwrap();
            let (out4, _, _) = write_xmp(&raw4, &r, crate::diag::stderr()).unwrap();
            let t = std::fs::read_to_string(&out4).unwrap();
            let settings = t.find("crs:Exposure2012").expect("our settings landed somewhere");
            let real_root = t.rfind("<rdf:RDF").expect("the real root survives");
            assert!(
                settings > real_root,
                "the settings block was spliced into quoted text, not the document: {t}"
            );
            assert!(t.contains("xmp:Rating=\"4\""), "the user's rating survives: {t}");
            let _ = std::fs::remove_dir_all(&dev4);
        }

        // A base we genuinely CANNOT account for still regenerates and still
        // says so — and the note names the SIDECAR, not the output.
        let raw3 = dir.join("_pipe_note_broken.arw");
        std::fs::write(&raw3, b"raw").unwrap();
        let dev3 = crate::store::develop_dir(&raw3);
        let _ = std::fs::remove_dir_all(&dev3);
        let lr3 = dir.join("_pipe_note_broken.xmp");
        std::fs::write(
            &lr3,
            "<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:PointColor=\"0\">\n",
        )
        .unwrap();
        let (_, note, _) = write_xmp(&raw3, &r, crate::diag::stderr()).unwrap();
        let note = note.expect("an unaccountable base is a real loss");
        assert!(
            note.contains(&lr3.display().to_string()),
            "the note names the base file, not only the output: {note}"
        );
        assert!(note.contains("untouched"), "…and is honest that it was not modified: {note}");
        let _ = std::fs::remove_dir_all(&dev3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Over the 16 MiB cap the Lightroom sidecar cannot BE the merge base —
    /// fine — but the one-Option reader turned that refusal into silence: the
    /// `.or_else` fell back to our own previous projection, the merge
    /// "succeeded", and the REAL loss (Lightroom's newer work not carried
    /// into the new file) produced no note at all. The disclosure mechanism
    /// round four built was defeated by round five's own size cap.
    #[test]
    fn an_oversized_lightroom_sidecar_is_disclosed_not_silently_skipped() {
        let dir = std::env::temp_dir().join(format!("autoshade-pipe-xmp-oversized-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_pipe_oversized.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        let lr = dir.join("_pipe_oversized.xmp");

        // First save: a mergeable Lightroom base whose PointColor lands in the
        // projection — the carried-forward property the second save must keep.
        std::fs::write(
            &lr,
            "<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:PointColor=\"0\"></rdf:Description>\n",
        )
        .unwrap();
        let r1 = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        let (_, note, _) = write_xmp(&raw, &r1, crate::diag::stderr()).unwrap();
        assert!(note.is_none(), "the mergeable base has nothing to disclose: {note:?}");

        // The sidecar balloons past the cap (Lightroom AI masks, or hostile).
        let mut big = String::with_capacity(16 * 1024 * 1024 + 64);
        big.push_str("<x:xmpmeta>");
        while big.len() <= 16 * 1024 * 1024 {
            big.push_str("<!-- pad -->");
        }
        std::fs::write(&lr, &big).unwrap();

        let r2 = EditRecipe { exposure_ev: 0.75, ..Default::default() };
        let (out, note, _) = write_xmp(&raw, &r2, crate::diag::stderr()).unwrap();
        // The fallback itself is CORRECT — the previous projection carries
        // forward what the first merge preserved, and the save must succeed.
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("crs:Exposure2012=\"0.75\""), "the save still happened");
        assert!(text.contains("crs:PointColor=\"0\""), "the projection base carried forward");
        // Doing it SILENTLY was the defect.
        let note = note.expect("an unreadable Lightroom sidecar must be disclosed");
        assert!(
            note.contains(&lr.display().to_string()) && note.contains("16 MiB"),
            "the note names the file and the reason: {note}"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    #[test]
    fn batch_names_keep_same_stem_deliverables_apart() {
        let mut names = BatchNames::default();
        let a = names.claim(Path::new("D:/roll1/DSC00001.ARW"), "developed", "tif");
        let b = names.claim(Path::new("D:/roll2/DSC00001.ARW"), "developed", "tif");
        // Case must fold: the filesystem collides DSC with dsc even though
        // the strings differ.
        let c = names.claim(Path::new("D:/roll3/dsc00001.arw"), "developed", "tif");
        assert_eq!(a, PathBuf::from("out").join("DSC00001.developed.tif"));
        assert_eq!(b, PathBuf::from("out").join("DSC00001 (2).developed.tif"));
        assert_eq!(c, PathBuf::from("out").join("dsc00001 (3).developed.tif"));
        // Exactly the deviations are disclosed; the first claimant is silent.
        assert_eq!(names.renamed.len(), 2);
        assert!(names.renamed[0].contains("DSC00001 (2).developed.tif"));
        assert!(names.renamed[0].contains("roll2"), "the SOURCE photo is named");
        // A distinct stem stays untouched — bare name, no disclosure.
        let d = names.claim(Path::new("D:/roll1/DSC00002.ARW"), "developed", "tif");
        assert_eq!(d, PathBuf::from("out").join("DSC00002.developed.tif"));
        assert_eq!(names.renamed.len(), 2);
    }

    /// R22-7: a rooted batch delivers into the root it was GIVEN — including
    /// the same-stem "(2)" arm, which had its own hardcoded `./out` and would
    /// otherwise scatter the second copy of a name back into ./out while its
    /// sibling went to the user's folder. The disclosure line names the real
    /// destination too, or the batch summary would point at a file that is not
    /// there.
    #[test]
    fn batch_names_deliver_into_the_root_they_were_given() {
        let root = PathBuf::from("D:/deliver/tripA");
        let mut names = BatchNames::rooted(root.clone());
        let a = names.claim(Path::new("D:/roll1/DSC00001.ARW"), "developed", "tif");
        let b = names.claim(Path::new("D:/roll2/DSC00001.ARW"), "developed", "tif");
        assert_eq!(a, root.join("DSC00001.developed.tif"));
        assert_eq!(b, root.join("DSC00001 (2).developed.tif"), "the dedup arm is rooted too");
        assert_eq!(names.renamed.len(), 1);
        assert!(
            names.renamed[0].contains(&root.join("DSC00001 (2).developed.tif").display().to_string()),
            "the disclosure names the real destination: {}",
            names.renamed[0]
        );
        // …and the CLI's default is byte-identical to what it always produced.
        let mut plain = BatchNames::default();
        assert_eq!(
            plain.claim(Path::new("D:/roll1/DSC00001.ARW"), "developed", "tif"),
            default_out(Path::new("D:/roll1/DSC00001.ARW"), "developed", "tif"),
            "an un-rooted batch is exactly default_out, as every CLI caller expects"
        );
    }
}

/// `<delivery root>/<stem>.<kind>.<ext>` — outputs never go beside the source
/// RAW. The root is [`crate::config::delivery_root`] (R24-5 M8), which
/// defaults to the historical `./out`, so an unset setting produces exactly
/// the path this function always produced. THE funnel: every deliverable and
/// every pixel master is named through here or through [`BatchNames`], which
/// itself calls it — so the setting has one consumption point, not five.
pub fn default_out(raw: &Path, kind: &str, ext: &str) -> PathBuf {
    crate::config::delivery_root().join(format!("{}.{kind}.{ext}", stem(raw)))
}

/// Batch-scope deliverable names. `default_out` keys deliverables by STEM
/// alone — deliberate (a re-export replaces its previous deliverable, and
/// user scripts key on the bare name) — but one batch can hold two
/// DSC00001.ARW from different folders (camera counter rollover), and both
/// would then write the SAME file: the second silently destroyed the first.
/// Four review units reported it independently. Within one batch the first
/// claimant keeps the bare name; later same-stem claimants get the import
/// convention `<stem> (2).<kind>.<ext>`, and every deviation is recorded in
/// `renamed` so the batch summary can say WHICH photo took WHICH name.
/// Claims fold case: the filesystem would collide `DSC` with `dsc` even
/// though the strings differ.
#[derive(Default)]
pub struct BatchNames {
    taken: std::collections::HashSet<String>,
    /// Disclosure lines, `"<final deliverable> ← <source photo>"`.
    pub renamed: Vec<String>,
    /// Delivery ROOT for every claim. `None` = whatever [`default_out`] gives
    /// — the CLI's shape, i.e. the configured delivery root (M8), `./out`
    /// unless the user moved it — and what `Default::default()` still
    /// produces byte for byte. The GUI passes the user's export destination
    /// setting here so a batch render lands where its single exports do
    /// (R22-7); the naming and dedup rules are untouched by it.
    root: Option<PathBuf>,
}

impl BatchNames {
    /// A batch that delivers into `dir` instead of the delivery root.
    pub fn rooted(dir: PathBuf) -> Self {
        Self { root: Some(dir), ..Default::default() }
    }

    /// The deliverable path for `raw`, unique within this batch.
    pub fn claim(&mut self, raw: &Path, kind: &str, ext: &str) -> PathBuf {
        let mut out = self.rooted_name(raw, kind, ext, 1);
        let mut n = 1u32;
        while !self.taken.insert(out.to_string_lossy().to_lowercase()) {
            n += 1;
            out = self.rooted_name(raw, kind, ext, n);
        }
        if n > 1 {
            self.renamed.push(format!("{} ← {}", out.display(), raw.display()));
        }
        out
    }

    /// `<root>/<stem>[ (n)].<kind>.<ext>`. `n == 1` is the bare name, and it
    /// comes from [`default_out`] so this type never re-spells the deliverable
    /// convention; only the parent directory is swapped when a root was set.
    fn rooted_name(&self, raw: &Path, kind: &str, ext: &str, n: u32) -> PathBuf {
        let base = if n == 1 {
            default_out(raw, kind, ext)
        } else {
            crate::config::delivery_root()
                .join(format!("{} ({n}).{kind}.{ext}", stem(raw)))
        };
        match (&self.root, base.file_name()) {
            (Some(root), Some(name)) => root.join(name),
            _ => base,
        }
    }
}

pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create output dir {}", parent.display()))?;
        }
    Ok(())
}

/// The output-path preflight every PAID command runs BEFORE its first API
/// call (L09#1): the read-only-library guard, a directory-target refusal,
/// and the parent directory. An `-o` mistake used to surface only AFTER
/// the analysis / image call had been billed — `autoshade analyze x.arw -o
/// <existing dir>` paid for propose+verify, then write_recipe bailed with
/// nothing saved. Trade-off, stated on purpose: creating the parent
/// up-front can leave an empty directory behind if the paid call then
/// fails — that beats burning the call (a REFUSAL leaving an empty dir is
/// the case main.rs's apply path guards against; here money was spent).
/// For the GUI/web surfaces this is a provable no-op: their `out` comes
/// from `unique_out`, which already ensure_parents and create_new-claims
/// the name.
pub fn preflight_out(out: &Path, src: &Path) -> Result<()> {
    guard_readonly(out, src)?;
    if out.is_dir() {
        anyhow::bail!(
            "output target {} is a directory — pass a file path (e.g. {}/result.png)",
            out.display(),
            out.display()
        );
    }
    ensure_parent(out)?;
    // An EXISTING destination must itself be replaceable (review R12-06):
    // the sibling probe below proves the DIRECTORY writes, not that a
    // read-only file already sitting at the name can be replaced. The GUI's
    // unique_out claims are its own 0-byte files, so this stays a no-op
    // there.
    if out.is_file()
        && let Err(e) = std::fs::OpenOptions::new().write(true).open(out)
    {
        anyhow::bail!(
            "output file {} exists and is not writable ({e}) — checked before the paid call",
            out.display()
        );
    }
    // An EXISTING parent never proved it was writable (L10-7): an ACL-denied
    // export dir used to surface only after the paid call. Probe with a
    // uniquely-named sibling (pid + process-wide seq — the sibling_tmp rule),
    // removed immediately; the refusal names the directory.
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        let probe = parent.join(format!(
            ".autoshade-write-probe.{}.{}",
            std::process::id(),
            crate::store::next_tmp_seq()
        ));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(f) => {
                drop(f);
                let _ = std::fs::remove_file(&probe);
            }
            Err(e) => {
                anyhow::bail!(
                    "output directory {} is not writable ({e}) — checked before the paid call",
                    parent.display()
                );
            }
        }
    }
    Ok(())
}

pub fn stem(p: &Path) -> &str {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("out")
}

/// The `"<stem>: "` the DEFAULT diagnostics sink puts in front of a
/// photograph's message ([`crate::diag::Subject::stamp`]) — a FORMATTING
/// helper, and the only thing left of what used to be the whole mechanism.
///
/// **Why the attribution exists at all.** `batch` defaults to `--jobs 3` and
/// the pool's ordering guarantee covers STDOUT only: workers write into their
/// own block and the sequencer releases blocks in index order, so the
/// transcript is byte-identical to a serial one. STDERR had no such thing —
/// `verbose` gates the progress chatter, not the warnings, so every ungated
/// `eprintln!` in the develop chain landed on a shared stream in COMPLETION
/// order and named no photograph at all. Three photos in flight, three
/// warnings, and nothing said which was which. `jobs`' module doc has always
/// disclosed the block channel; this is the identity the reordering costs,
/// bought back.
///
/// **What changed at R29-1.** R28 Batch-5 5c bought that identity back as a
/// PREFIX and registered the gap a prefix leaves: the deepest root the R27
/// adjudication names (F6) is that the pipeline had no caller-supplied
/// diagnostics channel at all, so the disclosure lines could not be routed,
/// suppressed or ORDERED by anyone who was not the process. [`crate::diag`] is
/// that channel, and it is no longer open: the develop chain's disclosures
/// travel as `diag::Line`s carrying their subject as DATA, and `batch` now
/// renders its workers' lines into the photo's own transcript block, so their
/// order is the sequencer's (index) rather than the scheduler's (completion).
/// Nothing but the default sink calls this function.
pub fn attribution(photo: &Path) -> String {
    format!("{}: ", stem(photo))
}

/// Whether a directory entry is a directory, WITHOUT an extra stat per file:
/// `DirEntry::file_type()` comes free with the directory listing (measurable on
/// large libraries / network shares), and only symlinks fall back to the
/// following `Path::is_dir` so junction/symlink traversal behaves as before.
fn entry_is_dir(entry: &std::fs::DirEntry) -> std::io::Result<bool> {
    let ft = entry.file_type()?;
    Ok(if ft.is_symlink() { entry.path().is_dir() } else { ft.is_dir() })
}

/// Recursively collect every camera RAW under `dir`, sorted.
///
/// "A RAW" has exactly ONE definition app-wide — [`decode::is_raw`] (arw, dng,
/// raw, raf, nef, cr2, cr3, orf, rw2), the same predicate [`find_sources`] and
/// the render/decode path use. This scanner used to accept `.arw` alone, which
/// left `batch`, `eval` and `style-index` blind to libraries the GUI and web
/// open fine.
pub fn find_raws(dir: &Path) -> Result<Vec<PathBuf>> {
    walk_photos(dir, decode::is_raw)
}

/// The shared scanner behind [`find_raws`] / [`find_sources`].
///
/// Cycle-proof by IDENTITY, not just depth: `entry_is_dir` deliberately
/// follows directory symlinks/junctions, and a link back into an ancestor
/// re-enters the same directory under a NEW spelling — the old depth cap
/// turned that into every RAW appearing up to 64× (batch then analyzed AND
/// BILLED each occurrence). Every directory is entered once per canonical
/// identity, and found files are deduped by canonical identity too (a file
/// symlink is the same photo). Resilient per entry: one unreadable
/// SUBDIRECTORY (AV lock, permissions) warns and is skipped instead of
/// aborting the whole scan — only an unreadable ROOT is a real failure. The
/// depth cap stays as a backstop for canonicalize-failing paths.
fn walk_photos(root: &Path, pred: fn(&Path) -> bool) -> Result<Vec<PathBuf>> {
    walk_photos_counted(root, pred).map(|(v, _)| v)
}

/// [`walk_photos`], plus how many entries the scan could not read and skipped.
/// Each skip already warns on stderr AS IT HAPPENS; the count exists so a
/// windowed caller can say so too (L08 — the GUI has no console).
fn walk_photos_counted(
    root: &Path,
    pred: fn(&Path) -> bool,
) -> Result<(Vec<PathBuf>, usize)> {
    fn walk(
        dir: &Path,
        pred: fn(&Path) -> bool,
        out: &mut Vec<PathBuf>,
        visited: &mut std::collections::HashSet<PathBuf>,
        depth: u32,
        is_root: bool,
        skipped: &mut usize,
    ) -> std::io::Result<()> {
        if let Ok(c) = std::fs::canonicalize(dir)
            && !visited.insert(c)
        {
            return Ok(()); // already scanned through another spelling
        }
        if depth > 64 {
            return Ok(());
        }
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if !is_root => {
                eprintln!("⚠ skipping unreadable folder {} ({e})", dir.display());
                *skipped += 1;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        for entry in rd {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("⚠ skipping unreadable entry under {} ({e})", dir.display());
                    *skipped += 1;
                    continue;
                }
            };
            let p = entry.path();
            match entry_is_dir(&entry) {
                Ok(true) => walk(&p, pred, out, visited, depth + 1, false, skipped)?,
                Ok(false) => {
                    if pred(&p) {
                        out.push(p);
                    }
                }
                Err(e) => {
                    eprintln!("⚠ skipping unreadable entry {} ({e})", p.display());
                    *skipped += 1;
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut skipped = 0usize;
    walk(root, pred, &mut out, &mut visited, 0, true, &mut skipped)
        .with_context(|| format!("scan {}", root.display()))?;
    out.sort();
    // Canonical-identity dedupe (first occurrence in sorted order wins): two
    // spellings of one file — a file symlink beside its target, or paths via
    // different directory links — are ONE photo, and each duplicate used to
    // bill its own analysis in batch.
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
    Ok((out, skipped))
}

/// Every ALREADY-BAKED raster extension the app opens — the counterpart to
/// [`decode::RAW_EXTS`], and the second half of "a photo". One list, because
/// there used to be four hand-typed copies of it (here, `serve::is_baked_ext`,
/// the GUI's `PHOTO_EXTS`, the web `accept` attribute) with nothing pinning
/// them together; the web copy had already drifted, silently refusing `.orf`,
/// `.rw2` and `.raw` uploads that every other surface accepted.
///
/// Bounded by what the `image` crate is compiled to DECODE (`Cargo.toml`'s
/// feature list) — adding an entry here without the matching codec feature
/// would let a file through the dialog and then fail at decode.
pub const BAKED_EXTS: [&str; 8] = ["png", "tif", "tiff", "jpg", "jpeg", "webp", "bmp", "gif"];

/// Does this path name a photo — a camera RAW or an already-baked raster?
/// Shared by the gallery scan and by [`guard_readonly`]'s never-clobber rule.
pub fn is_source(p: &Path) -> bool {
    crate::decode::is_raw(p) || is_baked(p)
}

/// The baked half of [`is_source`], on its own — what the web server's
/// add-path and upload gates ask, and what the GUI's file dialog lists
/// alongside the RAW set.
pub fn is_baked(p: &Path) -> bool {
    p.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| BAKED_EXTS.iter().any(|b| x.eq_ignore_ascii_case(b)))
}

/// Every extension the app opens, RAW and baked, for the surfaces that need a
/// LIST rather than a predicate (a file dialog's filter, an `<input accept>`).
/// Derived, never typed.
pub fn photo_exts() -> Vec<&'static str> {
    crate::decode::RAW_EXTS.iter().chain(BAKED_EXTS.iter()).copied().collect()
}

/// Like [`find_raws`] but also includes already-baked images (PNG/TIFF/JPEG), so
/// the web UI can browse and edit LR/PS-denoised exports alongside RAWs. Sorted.
pub fn find_sources(dir: &Path) -> Result<Vec<PathBuf>> {
    walk_photos(dir, is_source)
}

/// [`find_sources`], plus the number of unreadable entries the scan skipped —
/// the GUI folder scan consumes the count (stderr already names each one).
pub fn find_sources_counted(dir: &Path) -> Result<(Vec<PathBuf>, usize)> {
    walk_photos_counted(dir, is_source)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R29 Batch-3. `crs:LensProfileEnable="0"` is a REASON, not an absence:
    /// Lightroom drew no lens correction, so the frame it stored a mask in is
    /// the frame it exported that mask into and an identity warp is CORRECT.
    ///
    /// The distinction is the whole point of the variant — a photographer
    /// looking at `Absent` should go looking for a profile, and one looking at
    /// `DisabledInSidecar` should not.
    #[test]
    fn a_sidecar_that_switched_the_lens_profile_off_names_its_identity_warp() {
        use crate::recipe::MaskWarpSource;
        // No RAW behind it: `fresh_lens_profile` answers `Absent` for a path it
        // cannot read, which is the baseline this test moves off.
        let raw = std::env::temp_dir().join("r29b3-no-such-photo.arw");
        assert_eq!(fresh_lens_profile(&raw).mask_warp_src, MaskWarpSource::Absent);
        assert_eq!(
            fresh_lens_profile_for_sidecar(&raw, None).mask_warp_src,
            MaskWarpSource::Absent,
            "no document = no opinion"
        );
        let off = r#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:LensProfileEnable="0" crs:Version="15.0"></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let on = off.replace(r#"crs:LensProfileEnable="0""#, r#"crs:LensProfileEnable="1""#);
        // PREMISE: the reader really sees the switch, or the assertion below
        // would pass against a document it failed to parse.
        assert_eq!(crate::xmp::lens_profile_enabled(off), Some(false));
        assert_eq!(crate::xmp::lens_profile_enabled(&on), Some(true));
        assert_eq!(
            fresh_lens_profile_for_sidecar(&raw, Some(off)).mask_warp_src,
            MaskWarpSource::DisabledInSidecar
        );
        // ON does NOT overwrite the answer the photograph gave: the switch can
        // only say "the frames coincide", never "a warp exists".
        assert_eq!(
            fresh_lens_profile_for_sidecar(&raw, Some(&on)).mask_warp_src,
            MaskWarpSource::Absent
        );
        // The photographer's own toggles are untouched either way — reading
        // Lightroom's switch as an instruction would silently overwrite them.
        let p = fresh_lens_profile_for_sidecar(&raw, Some(off));
        assert!(p.mask_warp.is_empty() && !p.distortion_on && !p.vignette_on && !p.ca_on);
    }

    #[test]
    fn disabled_sidecar_retains_the_camera_map_only_for_linear_handles() {
        use crate::recipe::{LensProfile, MaskWarpSource, MaskWarpCenter};

        let solved = (0..crate::recipe::MASK_WARP_KNOTS)
            .map(|i| 0.975 + 0.06 * i as f32 / (crate::recipe::MASK_WARP_KNOTS - 1) as f32)
            .collect::<Vec<_>>();
        let centre = MaskWarpCenter {
            stored_px: [4768.0, 3168.0],
            stored_dims: [9504.0, 6336.0],
        };
        let mut profile = LensProfile {
            mask_warp: solved.clone(),
            mask_warp_src: MaskWarpSource::CameraMetadata,
            mask_warp_center: Some(centre),
            ..Default::default()
        };

        retain_disabled_linear_handle_warp(&mut profile);
        profile.clamp();

        assert!(profile.mask_warp.is_empty(), "disabled RADIAL must retain identity");
        assert_eq!(profile.mask_warp_src, MaskWarpSource::DisabledInSidecar);
        assert_eq!(profile.linear_handle_warp(), solved);
        assert_eq!(profile.mask_warp_center, Some(centre));

        // This is a persisted frame fact, not a transient import cache.
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("\"linear_handle_warp\""), "{json}");
        let reopened: LensProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(reopened, profile);

        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct PreLinearLensProfile {
            vignette: Vec<f32>,
            distortion: Vec<f32>,
            ca_r: Vec<f32>,
            ca_b: Vec<f32>,
            vignette_on: bool,
            distortion_on: bool,
            ca_on: bool,
            mask_warp: Vec<f32>,
            mask_warp_src: MaskWarpSource,
            mask_warp_center: Option<MaskWarpCenter>,
        }
        assert!(
            serde_json::from_str::<PreLinearLensProfile>(&json).is_err(),
            "an older strict reader must refuse the new frame fact"
        );

        let mut legacy_value: serde_json::Value = serde_json::from_str(&json).unwrap();
        legacy_value.as_object_mut().unwrap().remove("linear_handle_warp");
        let legacy: LensProfile = serde_json::from_value(legacy_value).unwrap();
        assert!(legacy.linear_handle_warp.is_empty(), "missing field defaults to legacy identity");
    }

    /// The PRODUCTION text of `rel`, with every `#[cfg(test)]` item removed.
    ///
    /// The cut is deliberately dumb, and reliable here for one stated reason:
    /// every `#[cfg(test)]` in the scanned files sits at column 0, and every
    /// top-level item in this codebase closes with a `}` at column 0 — so
    /// "from the attribute to the next line that is exactly `}`" is the item.
    /// A parser would be more general and would also have to know raw strings,
    /// nested block comments and lifetimes apart from char literals; the pinned
    /// counts below are what actually proves this cut landed where intended (a
    /// mis-cut moves them).
    ///
    /// It is also what lets this gate live inside one of the files it scans:
    /// its own registry table is `#[cfg(test)]`, so the scan cannot find it.
    fn production_text(rel: &str) -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        // LF-normalised: this repo has MIXED line endings by design.
        let text = std::fs::read_to_string(root.join(rel))
            .unwrap_or_else(|e| panic!("{rel}: source readable ({e})"))
            .replace("\r\n", "\n");
        let mut out = String::new();
        let (mut cursor, mut search) = (0usize, 0usize);
        while let Some(rel_at) = text[search..].find("\n#[cfg(test)]\n") {
            let at = search + rel_at + 1;
            let end = text[at..].find("\n}\n").map(|r| at + r + 3).unwrap_or(text.len());
            out.push_str(&text[cursor..at]);
            cursor = end;
            search = end;
        }
        out.push_str(&text[cursor..]);
        out
    }

    /// Every `<call>…;` statement in `text` opened by one of `openers`.
    ///
    /// Statement space, not raw offsets: the question is always "what does the
    /// call around this message look like", and a fragment can sit many lines
    /// below the call that raises it.
    fn statements(text: &str, openers: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for opener in openers {
            let mut from = 0usize;
            while let Some(rel) = text[from..].find(opener) {
                let start = from + rel;
                let end = start + text[start..].find(';').unwrap_or(text[start..].len() - 1);
                out.push(text[start..=end.min(text.len() - 1)].to_string());
                from = start + opener.len();
            }
        }
        out
    }

    /// R29-1 — THE DIAGNOSTICS-SINK GATE (successor of R28 Batch-5 5c's
    /// attribution gate, `every_worker_reachable_warning_names_its_photo`).
    ///
    /// `batch` defaults to `--jobs 3` and the pool orders STDOUT only, so every
    /// ungated `eprintln!` a worker could reach landed on a shared stderr in
    /// completion order. 5c made each of those lines NAME its photograph; that
    /// left the deeper half of adjudication F6 open — the caller could not
    /// route, suppress or order them, because they were hard-coded writes to
    /// the process's own stderr. R29-1 moved them onto [`crate::diag`], and
    /// this gate states the property that replaces "each line names a photo":
    ///
    /// 1. Each registered disclosure is raised through the SINK (`.warn(` /
    ///    `.emit(`), carrying its subject as data.
    /// 2. NONE of them is an `eprintln!` any more — the same fragment scanned
    ///    in `eprintln!` statement space must come back empty.
    /// 3. The develop chain's files hold no OTHER bare `eprintln!` than the
    ///    ones this table names and explains, so a new one cannot be added
    ///    without meeting the gate.
    ///
    /// A source scan, in the idiom this repo already uses for the store's
    /// capped-reader rule and the GUI font gate: the property is "no site
    /// forgot", which no behavioural test over one code path can state. The
    /// behavioural half lives beside it — `crate::diag`'s pool test proves the
    /// ORDER is the caller's, and `render`'s preview test proves the pure-pixel
    /// arm carries `Subject::PixelOnly`.
    ///
    /// Rows whose fragment is the CALL rather than a message ("warn(m)") are
    /// disclosures made of nothing but an interpolated value: there the count is
    /// the assertion, exactly as under 5c.
    #[test]
    fn every_worker_reachable_warning_flows_through_the_sink() {
        // (file, fragment, how many times it must be raised through the sink)
        const SITES: [(&str, &str, usize); 12] = [
            ("src/pipeline.rs", "GPT proposer failed", 1),
            ("src/pipeline.rs", "style embedding unavailable", 1),
            // The run-scoped `Once` line — 5c had to leave this one
            // un-stamped and say so in a comment; `Subject::Run` states it.
            ("src/pipeline.rs", "style reference unavailable", 1),
            ("src/pipeline.rs", "recipe limits discarded {}", 1),
            ("src/pipeline.rs", "warn(msg.clone())", 1),
            ("src/pipeline.rs", "warn(m)", 1),
            // The clamp disclosure 5c's sweep MISSED, because it re-walked
            // three files and this one lives in a fourth.
            ("src/recipe.rs", "truncated {} curve point(s)", 1),
            ("src/main.rs", "XMP failed: {e}\"", 1),
            ("src/render.rs", "could not embed the {space:?} ICC profile", 1),
            ("src/render.rs", "bitmap mask '{path}' could not be loaded", 1),
            ("src/render.rs", "bitmap mask '{path}' exceeds the", 1),
            // Three arms of the same aggregate-budget refusal, one loop.
            ("src/render.rs", "mask raster '{path}' skipped", 3),
        ];
        let mut checked = 0usize;
        let mut scanned = 0usize;
        for (rel, fragment, want) in SITES {
            let text = production_text(rel);
            let sunk = statements(&text, &[".warn(", ".emit("]);
            scanned += sunk.len();
            let hits: Vec<&String> = sunk.iter().filter(|s| s.contains(fragment)).collect();
            assert_eq!(
                hits.len(),
                want,
                "{rel}: expected {want} sink call(s) carrying {fragment:?}, found {} — this \
                 gate has gone stale, or a disclosure left the channel and was re-spelled",
                hits.len()
            );
            // …and it must not ALSO still be an eprintln (a half-migrated site
            // would otherwise satisfy the row above and keep printing twice).
            let bare = statements(&text, &["eprintln!("]);
            let leaks: Vec<&String> = bare.iter().filter(|s| s.contains(fragment)).collect();
            assert!(
                leaks.is_empty(),
                "{rel}: {fragment:?} is still raised by a bare eprintln!:\n{}",
                leaks[0]
            );
            checked += hits.len();
        }
        // PREMISE: a scanner that matched nothing would pass vacuously.
        assert_eq!(checked, 14, "every registered site must have been inspected");
        assert!(scanned > 40, "the statement scanner found almost nothing: {scanned}");

        // THE CENSUS. What is left of `eprintln!` in the develop chain's own
        // files, why each survivor is not a sink case, and a pinned count so a
        // new one cannot slip in silently.
        //
        // * `src/diag.rs` = 1: the DEFAULT SINK itself. Every routed line ends
        //   here unless the caller says otherwise, and it is the only write to
        //   the process stderr the develop chain performs.
        // * `src/pipeline.rs` = 8: five belong to the RESTORE/base-look
        //   helpers (`saved_recipe_snapshot`, `photo_base_knots_checked`) —
        //   library-wide functions with ~30 GUI/web/store call sites, each of
        //   which already names its photograph by stem or by path; threading a
        //   sink through them is its own sweep and is registered as such in
        //   `crate::diag`'s module doc. Three are the folder SCAN's, which run
        //   before any pool, in the caller's own thread, and are about a
        //   directory rather than a photograph.
        // * `src/render.rs` = 1: `disclose_approximate_demosaic`, which names
        //   its file by full path and belongs to the decoder rather than to
        //   this chain (same registered sweep).
        // * `src/main.rs` = 9: the SERIAL single-photo commands (`analyze`,
        //   `apply`, `auto`, `fit`) and their import-note siblings. One photo,
        //   one thread, a printed header directly above — nothing to order and
        //   nobody to route to. The pooled `batch` worker has none.
        // * `src/recipe.rs` = 0: `ValidatedRecipe::disclose` was the last one.
        const CENSUS: [(&str, usize); 5] = [
            ("src/diag.rs", 1),
            ("src/pipeline.rs", 8),
            ("src/render.rs", 1),
            ("src/main.rs", 9),
            ("src/recipe.rs", 0),
        ];
        for (rel, want) in CENSUS {
            let found = statements(&production_text(rel), &["eprintln!("]).len();
            assert_eq!(
                found,
                want,
                "{rel}: {found} bare eprintln!(s) in production code, registered {want} — a new \
                 one needs a row in this census explaining why it is not a sink case (or it \
                 needs to become one)"
            );
        }
    }

    /// L13#4: calibration comes from the NEWEST intent. A Lightroom sidecar
    /// that out-ranks the store vetoes the stored recipe's calibration
    /// (fresh stamp, like both restore surfaces); an older or neutral one
    /// leaves the stored develop in charge.
    #[test]
    fn saved_calibration_resolves_by_newest_intent() {
        let dir = std::env::temp_dir().join(format!("autoshade-pipeline-test-lr-calib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_pipe_lr_calib.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        std::fs::create_dir_all(&dev).unwrap();
        let stored = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        std::fs::write(crate::store::recipe_target(&raw), serde_json::to_string(&stored).unwrap())
            .unwrap();

        // No sidecar: the stored develop answers (the common case).
        assert!(saved_recipe_snapshot(&raw).is_some());

        // A NEWER Lightroom sidecar with a real edit vetoes it.
        let lr = raw.with_extension("xmp");
        std::fs::write(
            &lr,
            crate::xmp::recipe_to_xmp(&EditRecipe { contrast: 33.0, ..Default::default() }),
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
        assert!(
            saved_recipe_snapshot(&raw).is_none(),
            "a newer Lightroom edit supersedes the stored calibration"
        );

        // An OLDER sidecar leaves the store in charge.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap();
        let kept = saved_recipe_snapshot(&raw).expect("the older sidecar does not veto");
        assert_eq!(kept.exposure_ev, 0.5);

        // A NEUTRAL newer sidecar is not a save and does not veto either.
        std::fs::write(&lr, b"<x:xmpmeta/>").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lr)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
        assert!(saved_recipe_snapshot(&raw).is_some(), "a neutral sidecar is not intent");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    #[test]
    fn guard_refuses_writes_into_the_source_library() {
        // The library fixture is spelled from the host's own ROOT. `D:/…` is
        // an absolute, prefix-rooted path on Windows and a plain RELATIVE
        // directory literally named `D:` everywhere else — where the
        // root-level `..` case below folds to a cwd sibling instead of
        // bumping against a root, so the regression this test exists for
        // could not even be spelled and the guard (correctly) allowed the
        // write. That is how it failed on both Unix legs of CI run
        // 32398395462. The GUARD is portable as it stands: `normalize_lexical`
        // folds `..` against `Component::RootDir` and `Component::Prefix`
        // alike, which is `/` on Unix and `D:` on Windows — only the fixture
        // spelling was Windows-only.
        #[cfg(windows)]
        const ROOT: &str = "D:/";
        #[cfg(not(windows))]
        const ROOT: &str = "/";
        // A RAW living in the (read-only) photo library.
        let lib = PathBuf::from(ROOT).join("Photography/Raw/2024/Trip");
        let raw = lib.join("DSC0001.ARW");
        // Writing a sibling INTO that folder must be refused.
        let sibling = lib.join("DSC0001.developed.tif");
        assert!(guard_readonly(&sibling, &raw).is_err(), "must refuse a sibling write");
        // A subfolder under the RAW's folder is refused too.
        let under = lib.join("out/DSC0001.tif");
        assert!(guard_readonly(&under, &raw).is_err(), "must refuse a subfolder write");
        // The default ./out (outside the library) is allowed.
        let safe = default_out(&raw, "developed", "tif");
        assert!(guard_readonly(&safe, &raw).is_ok(), "./out must be allowed");
        // A source that itself lives in OUR ./out (e.g. `match` on an exported
        // preview) may be written beside — the guard protects the library only.
        let out_src = Path::new("out/DSC0001.preview.jpg");
        let out_dst = Path::new("out/DSC0001.matched.json");
        assert!(guard_readonly(out_dst, out_src).is_ok(), "our ./out is always writable");
        // A `..` bumping against the ROOT must fold like the filesystem does
        // ("D:/../Photography" = "D:/Photography", "/../Photography" =
        // "/Photography") — popping the root used to produce the
        // drive-relative "D:Photography", which dodged every starts_with
        // check and slipped a library write past the guard.
        //
        // The UNIX half of that fold is provable on any host, and is proven
        // here rather than only in CI: `Path` parses "/x" into RootDir +
        // Normal on Windows too, so `normalize_lexical`'s RootDir arm runs
        // everywhere. The reverse does NOT hold — "D:" is a `Prefix` only on
        // Windows and a plain directory name elsewhere — which is exactly why
        // the fixture above has to be spelled per platform.
        assert_eq!(
            normalize_lexical(Path::new("/../Photography/x")),
            PathBuf::from("/Photography/x"),
            "the root's parent is the root"
        );
        let root_dodge = PathBuf::from(ROOT).join("../Photography/Raw/2024/Trip/DSC0001.x.tif");
        assert!(
            guard_readonly(&root_dodge, &raw).is_err(),
            "a root-level .. must not bypass the library guard"
        );
    }

    /// find_raws must accept EVERY format decode::is_raw does (one definition of
    /// "a RAW" app-wide) and nothing else — a Canon/Nikon library used to scan
    /// as empty for batch/eval/style-index.
    ///
    /// R27: the RAW set went from 9 extensions to 24, so the fixture is
    /// GENERATED from `decode::RAW_EXTS` rather than re-typed. A hand-written
    /// list here would only have proven that the test agrees with itself.
    /// Case and recursion are still exercised explicitly.
    ///
    /// MUTATION THIS CATCHES: give `find_raws` its own extension `matches!`
    /// instead of `decode::is_raw` and every format the two disagree on shows
    /// up by name.
    #[test]
    fn find_raws_accepts_every_raw_format_the_app_can_decode() {
        let dir = std::env::temp_dir().join(format!("autoshade_find_raws_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).expect("temp dir");
        // One file per RAW extension, alternating case so a case-sensitive
        // predicate fails on half of them rather than none.
        let mut expected: Vec<String> = Vec::new();
        for (i, ext) in decode::RAW_EXTS.iter().enumerate() {
            let ext = if i % 2 == 0 { ext.to_ascii_uppercase() } else { (*ext).to_string() };
            let name = format!("shot{i}.{ext}");
            // Every other one goes in the subdirectory, so recursion is
            // exercised across the whole set and not just once.
            let at = if i % 3 == 0 { dir.join("sub").join(&name) } else { dir.join(&name) };
            std::fs::write(&at, b"").expect("write");
            expected.push(name.to_lowercase());
        }
        for name in ["note.txt", "baked.png", "export.jpg", "shot.webp", "DSC0001.xmp"] {
            std::fs::write(dir.join(name), b"").expect("write");
        }

        let found = find_raws(&dir).expect("scan");
        let mut names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_lowercase())
            .collect();
        names.sort();
        expected.sort();
        assert_eq!(
            names, expected,
            "find_raws must see every RAW format (case-insensitive, recursive) and no baked/sidecar files"
        );
        assert!(found.iter().all(|p| decode::is_raw(p)), "one RAW predicate, app-wide");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R27 A2, tier 2 — every extension list in the tree is the SAME two
    /// lists, and the two lists do not overlap.
    ///
    /// Four surfaces used to keep their own hand-typed copy (`is_source`
    /// here, `serve::is_baked_ext`, the GUI's `PHOTO_EXTS`, the web `accept`
    /// attribute) with no test relating any of them; the format-support map
    /// counted the duplication at four and found one already drifted. Three of
    /// the four are now derivations, which makes drift impossible rather than
    /// merely detectable; the fourth (the static HTML) is pinned by
    /// `serve::tests::the_web_accept_list_matches_the_formats_the_app_opens`.
    /// This test guards the properties the derivations rest on.
    ///
    /// MUTATION THIS CATCHES: add `"tif"` to `decode::RAW_EXTS` (the tempting
    /// "fix" for a `.tif`-named DNG — see `decode::raw_in_tiff_clothing` for
    /// why that is the wrong direction) and the disjointness assertion fires
    /// before a single baked TIFF is misrouted into the RAW engine.
    #[test]
    fn one_raw_list_and_one_baked_list_serve_every_surface() {
        // Disjoint: an extension that claimed to be both would make
        // `is_source` true by two routes and `is_raw` decide the dispatch on
        // its own, silently.
        for raw in decode::RAW_EXTS {
            assert!(
                !BAKED_EXTS.contains(&raw),
                "{raw} is in BOTH lists — the raw-vs-baked dispatch would be ambiguous"
            );
        }
        // Lower-case, no dots: every consumer folds case itself and adds its
        // own separator, so a stray "." or "PNG" here would break the derived
        // `accept` string and the dialog filter at once.
        for e in photo_exts() {
            assert!(
                !e.is_empty()
                    && e.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "{e:?} must be lower-case alphanumeric with no leading dot"
            );
        }
        // `photo_exts` IS the union — no third list.
        assert_eq!(photo_exts().len(), decode::RAW_EXTS.len() + BAKED_EXTS.len());
        for e in photo_exts() {
            let p = std::path::PathBuf::from(format!("x.{e}"));
            assert!(is_source(&p), "{e} is offered by the dialog but is_source refuses it");
        }
        // …and the predicates partition it.
        for e in decode::RAW_EXTS {
            let p = std::path::PathBuf::from(format!("x.{e}"));
            assert!(decode::is_raw(&p) && !is_baked(&p));
        }
        for e in BAKED_EXTS {
            let p = std::path::PathBuf::from(format!("x.{e}"));
            assert!(is_baked(&p) && !decode::is_raw(&p));
        }
    }

    /// Build a directory link `link` → `target` by ANY unprivileged means:
    /// a real symlink where the process may create one (unix always;
    /// Windows with Developer Mode), else an NTFS junction (`mklink /J`,
    /// no privilege needed — and std's `is_symlink()` is true for mount
    /// points too, so `walk` traverses a junction identically). Ok names
    /// the mechanism used; Err lists EVERY failure.
    fn link_dir_cycle(
        target: &std::path::Path,
        link: &std::path::Path,
    ) -> Result<&'static str, String> {
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link)
                .map(|()| "symlink")
                .map_err(|e| format!("symlink: {e}"))
        }
        #[cfg(windows)]
        {
            let sym = match std::os::windows::fs::symlink_dir(target, link) {
                Ok(()) => return Ok("symlink"),
                Err(e) => e,
            };
            let mut cmd = std::process::Command::new("cmd");
            cmd.arg("/C").arg("mklink").arg("/J").arg(link).arg(target);
            crate::hide_child_console(&mut cmd);
            match cmd.output() {
                Ok(o) if o.status.success() => Ok("junction"),
                Ok(o) => Err(format!(
                    "symlink: {sym}; mklink /J exit {:?}: {}",
                    o.status.code(),
                    String::from_utf8_lossy(&o.stderr).trim()
                )),
                Err(e) => Err(format!("symlink: {sym}; spawn cmd: {e}")),
            }
        }
    }

    #[test]
    fn photo_scan_survives_a_directory_link_cycle_and_finds_each_raw_once() {
        // Per-process name (Codex AL F9): the fixture is MANDATORY, and a
        // shared fixed path let a crashed prior run's junction — or a
        // concurrent test process — fail the setup of an unrelated run.
        let dir = crate::test_dir("scan-cycle");
        std::fs::write(dir.join("a.arw"), b"raw").unwrap();
        // A directory link back to its own parent — the classic cycle. The
        // fixture is MANDATORY: without a link every assertion below holds
        // vacuously, and the old silent-skip variant (an eprintln! cargo
        // swallows) reported green on exactly the machines — stock Windows,
        // unprivileged CI — where the cycle guard went untested. A junction
        // needs no privilege, so no legitimate silent skip remains; a
        // link-hostile filesystem must fix the fixture, not mute the test.
        let kind = link_dir_cycle(&dir, &dir.join("loop")).unwrap_or_else(|e| {
            panic!("cannot build a directory-link cycle, so the cycle guard is UNTESTED: {e}")
        });
        let found = find_raws(&dir).expect("scan");
        assert_eq!(
            found.len(),
            1,
            "one RAW, found ONCE — never once per traversal (cycle via {kind}): {found:?}"
        );
        // Not just deduped by count: the one reported path must be the real
        // spelling, never the alias — a scan that recursed once through the
        // link and deduped by filename would pass the count alone.
        assert!(
            found.iter().all(|p| p.components().all(|c| c.as_os_str() != "loop")),
            "the scan must not report a RAW through the link alias (cycle via {kind}): {found:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The link-free half of the cycle test, split out so a genuinely
    /// link-hostile filesystem still keeps the plain-scan coverage while
    /// the cycle test above fails loudly. Own fixture directory — sharing
    /// one would race under cargo's parallel test threads.
    #[test]
    fn photo_scan_finds_one_raw_once_without_any_link() {
        let dir = std::env::temp_dir().join(format!("autoshade-scan-nolink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.arw"), b"raw").unwrap();
        let found = find_raws(&dir).expect("scan");
        assert_eq!(found.len(), 1, "one RAW in a plain directory: {found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pre_era_base_curve_fingerprint_separates_washed_from_tuned() {
        use crate::recipe::CALIB_ERA;
        // A +0.5-biased develop puts EVERY sample in the top half, so every
        // interior knot's input does too. This is the shape that renders
        // several stops dark once batch 43 made the develop correct.
        let washed = vec![[0.0, 0.0], [0.55, 0.10], [0.72, 0.44], [0.96, 0.90], [1.0, 1.0]];
        assert!(base_curve_looks_pre_era(1, &washed), "the washed shape must be caught");
        // The user's own pre-bias develops: interior inputs 0.126..0.493 —
        // a real neutral is DARK, which is why the base curve exists.
        let tuned = vec![[0.0, 0.0], [0.126, 0.29], [0.31, 0.55], [0.493, 0.78], [1.0, 1.0]];
        assert!(!base_curve_looks_pre_era(1, &tuned), "a legitimate curve must be left alone");
        // A washed curve whose TOP interior knot clips on both sides: the
        // neutral saturates because of the bias, the camera rendition because
        // the frame really does hold blown highlights. The tie must not read
        // as a lift — one saturated frame used to disable the repair entirely.
        let washed_clipped = vec![[0.0, 0.0], [0.55, 0.10], [0.80, 0.55], [1.0, 1.0], [1.0, 1.0]];
        assert!(
            base_curve_looks_pre_era(1, &washed_clipped),
            "a clipped tie at the top is not a lift"
        );
        // ...and a curve that merely FLATTENS (never darkens measurably) is
        // not the bias either.
        let flat = vec![[0.0, 0.0], [0.55, 0.55], [0.80, 0.79], [1.0, 1.0], [1.0, 1.0]];
        assert!(!base_curve_looks_pre_era(1, &flat), "no measurable darkening is no fingerprint");
        // A legitimately HIGH-KEY photo: its darkest 2% really is above
        // mid-grey, so every interior input clears 0.5 — but the curve still
        // LIFTS, as a camera base look does. Re-estimating it would replace a
        // saved look for no reason (the estimator need not reproduce a curve
        // an older build, or the user, authored).
        let high_key = vec![[0.0, 0.0], [0.55, 0.68], [0.72, 0.84], [0.93, 0.97], [1.0, 1.0]];
        assert!(
            !base_curve_looks_pre_era(1, &high_key),
            "a bright scene is not a washed frame — the curve still lifts"
        );
        // ONE interior knot below the half is enough to clear it: quantiles
        // are non-decreasing, so a real neutral's toe always lands low.
        let mostly_high = vec![[0.0, 0.0], [0.49, 0.10], [0.80, 0.60], [1.0, 1.0]];
        assert!(!base_curve_looks_pre_era(1, &mostly_high));
        // Era-stamped recipes are never second-guessed, whatever they hold.
        assert!(!base_curve_looks_pre_era(CALIB_ERA, &washed), "an era stamp is trusted");
        // Nothing to judge: no curve, or endpoints only.
        assert!(!base_curve_looks_pre_era(1, &[]));
        assert!(!base_curve_looks_pre_era(1, &[[0.0, 0.0], [1.0, 1.0]]));
    }

    #[test]
    fn repair_adopts_a_cached_identity_answer_and_stamps_the_era() {
        use crate::recipe::CALIB_ERA;
        // Success-EMPTY is an ANSWER, not a failure: seed the process memo
        // exactly as a successful identity estimate would have (the fixture
        // path does not exist, so its identity is the deterministic
        // (0, None) metadata fallback), and the repair must CLEAR the washed
        // curve, stamp the era, and disclose. Before the tri-state split it
        // returned None here — the washed curve kept rendering stops-dark
        // forever, and every reader re-paid the full decode + develop.
        let raw = std::path::PathBuf::from("memo-seeded-identity-fixture.arw");
        let key: CurveMemoKey = (raw.clone(), (0, None));
        fresh_curve_memo().lock().unwrap().insert(key, Vec::new());
        let mut r = EditRecipe {
            version: 1,
            base_curve: vec![[0.0, 0.0], [0.55, 0.10], [0.80, 0.55], [1.0, 1.0]],
            ..Default::default()
        };
        let note = repair_pre_era_base_curve(&raw, &mut r);
        assert!(note.is_some(), "an identity answer must repair, and say so");
        assert!(r.base_curve.is_empty(), "the answer replaces the washed curve");
        assert_eq!(r.version, CALIB_ERA, "and the era stamp travels with it");
    }

    #[test]
    fn repair_leaves_curve_and_stamp_alone_when_no_estimate_can_be_produced() {
        // An unreadable RAW yields NO answer (tri-state None): the washed
        // curve AND the era-1 stamp must both survive untouched, so the next
        // reader retries instead of trusting a laundered stamp — the exact
        // launder the analyze funnel shipped by stamping a curve without its
        // version.
        let raw = std::path::PathBuf::from("no-such-file-ever.arw");
        let washed = vec![[0.0, 0.0], [0.55, 0.10], [0.80, 0.55], [1.0, 1.0]];
        let mut r =
            EditRecipe { version: 1, base_curve: washed.clone(), ..Default::default() };
        assert!(repair_pre_era_base_curve(&raw, &mut r).is_none());
        assert_eq!(r.base_curve, washed, "an inability must not touch the curve");
        assert_eq!(r.version, 1, "nor launder the stamp");
    }

    /// The coordinate-frame migration's three gates, none of which needs a
    /// readable RAW to exercise.
    #[test]
    fn coord_frame_migration_gates() {
        use crate::recipe::{Crop, COORD_ERA};

        // (1) An ERA-CURRENT recipe is never looked at again — not even to
        // stat the photo. This is what keeps a browser- or AI-authored crop,
        // and every recipe this build writes, from being turned twice.
        let mut current = EditRecipe {
            coord_era: COORD_ERA,
            crop: Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 }),
            ..Default::default()
        };
        let before = current.clone();
        assert!(migrate_recipe_coord_frame(Path::new("no-such-file-ever.arw"), &mut current)
            .is_none());
        assert_eq!(current, before, "an era-current recipe must be untouched");

        // (2) A BAKED image was oriented at decode long before this era, so
        // its saved coordinates are already display-frame: stamped, silently,
        // without asking rawler anything.
        let mut baked = EditRecipe { coord_era: 0, ..Default::default() };
        assert!(migrate_recipe_coord_frame(Path::new("photo.jpg"), &mut baked).is_none());
        assert_eq!(baked.coord_era, COORD_ERA, "a non-RAW is stamped, not left to retry");

        // (3) An UNREADABLE RAW is an inability, not an answer: nothing moves
        // AND nothing is stamped, so the next reader retries. Stamping here
        // would declare the coordinates migrated when they are not, and no
        // later reader could tell.
        let mut legacy = EditRecipe {
            coord_era: 0,
            crop: Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 }),
            ..Default::default()
        };
        let untouched = legacy.clone();
        assert!(migrate_recipe_coord_frame(Path::new("no-such-file-ever.arw"), &mut legacy)
            .is_none());
        assert_eq!(legacy, untouched, "an inability must not move or stamp anything");
    }

    /// A scratch photo path whose develop dir is wiped clean, so a raster
    /// claim starts from `<prefix>.png` every run.
    fn scratch_photo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("autoshade-rotate-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join(format!("{tag}.arw"));
        std::fs::write(&raw, b"raw").unwrap();
        let _ = std::fs::remove_dir_all(crate::store::develop_dir(&raw));
        raw
    }

    /// THE hazard the ROADMAP skeleton named: the turn handed to
    /// `orient_recipe_coords` is the DELTA, never the accumulated
    /// `quarter_turns`. Stored coordinates already live in the CURRENT display
    /// frame, so re-applying the total moves them a second time.
    ///
    /// Two single turns must land exactly where one double turn does, and
    /// four must return to the start — the group property, checked on real
    /// geometry rather than on the orientation enum.
    ///
    /// MUTATION THIS CATCHES: pass `r.quarter_turns + delta` (the composed
    /// value) to `orient_recipe_coords` instead of `delta`. The first turn
    /// still looks right; the second lands 180° out, which is precisely how a
    /// double-application bug hides.
    #[test]
    fn rotating_turns_the_geometry_by_the_delta_not_by_the_running_total() {
        use crate::recipe::{Crop, LocalAdjustment, MaskGeometry};
        let raw = scratch_photo("delta");
        let seed = EditRecipe {
            crop: Some(Crop { left: 0.10, top: 0.20, right: 0.80, bottom: 0.90 }),
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear {
                    zero_x: 0.25,
                    zero_y: 0.10,
                    full_x: 0.75,
                    full_y: 0.60,
                },
                ..Default::default()
            }],
            ..Default::default()
        };

        let mut once = seed.clone();
        rotate_recipe(&mut once, &raw, 2).unwrap();
        let mut twice = seed.clone();
        rotate_recipe(&mut twice, &raw, 1).unwrap();
        rotate_recipe(&mut twice, &raw, 1).unwrap();
        assert_eq!(twice, once, "two quarter turns must equal one half turn");
        assert_eq!(once.quarter_turns, 2);

        let mut full = seed.clone();
        for _ in 0..4 {
            rotate_recipe(&mut full, &raw, 1).unwrap();
        }
        assert_eq!(full.quarter_turns, 0, "four quarter turns is the identity");
        // The identity in EXACT arithmetic — `orient_point` restricted to the
        // unit square is an isometry — but `1.0 - v` is an f32 subtraction, so
        // four of them drift by an ulp or two (0.2 comes back 0.19999999).
        // That is rounding, not accumulation: the bound below is ~1e-7 and
        // does not grow with more circles. Stated rather than hidden behind a
        // loose epsilon, because "a turn is lossless" is the claim item 7.4
        // rests on and the honest version of it is "lossless in the raster,
        // exact-up-to-f32 in the coordinates".
        let fc = full.crop.unwrap();
        let sc = seed.crop.unwrap();
        for (got, want) in
            [(fc.left, sc.left), (fc.top, sc.top), (fc.right, sc.right), (fc.bottom, sc.bottom)]
        {
            assert!((got - want).abs() < 1e-6, "a full circle moved the crop: {got} != {want}");
        }
        let (crate::recipe::MaskGeometry::Linear { zero_x, zero_y, full_x, full_y }, _) =
            (&full.masks[0].mask, ())
        else {
            panic!("still linear")
        };
        for (got, want) in [(*zero_x, 0.25), (*zero_y, 0.10), (*full_x, 0.75), (*full_y, 0.60)] {
            assert!((got - want).abs() < 1e-6, "a full circle moved the mask: {got} != {want}");
        }
    }

    /// A scratch BAKED photo — a real PNG of `w × h`, whose header
    /// `decode::frame_size_turned` can actually read. [`scratch_photo`]'s
    /// `.arw` is three bytes of nonsense and no decoder will answer a frame out
    /// of it, which is deliberate for every test that does not need one and
    /// useless for the ones that do (R29 C1).
    fn scratch_baked_photo(tag: &str, w: u32, h: u32) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("autoshade-rotate-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{tag}.png"));
        image::GrayImage::new(w, h).save(&p).unwrap();
        let _ = std::fs::remove_dir_all(crate::store::develop_dir(&p));
        p
    }

    /// One brush group with a single stroke, `radius` and one dab token.
    fn brush_mask(radius: f32, dabs: &str) -> crate::recipe::MaskGeometry {
        crate::recipe::MaskGeometry::Brush {
            name: "Brush 1".into(),
            blend_mode: 0,
            value: 1.0,
            inverted: false,
            strokes: vec![crate::recipe::BrushStroke {
                radius,
                dabs: dabs.into(),
                ..Default::default()
            }],
        }
    }

    /// R29 C1, the rotate half — and the half that needed a NEW input. A brush
    /// dab is a circle in PIXELS while `crs:Radius` is in WIDTH units, so
    /// turning a develop means reading the photo's frame and rescaling by its
    /// aspect. `rotate_recipe` is the one caller that has to fetch that itself.
    ///
    /// 480 × 320 (aspect 1.5): one quarter turn takes the dab from (0.1, 0.8)
    /// to (0.2, 0.1) and the radius from 0.1 to 0.15. A SECOND quarter turn is
    /// what pins that the frame is re-read rather than cached — the photo is
    /// 320 × 480 by then, so the radius must come back to 0.1 and not go to
    /// 0.225.
    ///
    /// MUTATION THIS CATCHES: pass `r.quarter_turns + delta` (or a constant 0)
    /// to `frame_size_turned` — the first turn still looks right and the second
    /// rescales by 1.5 again.
    #[test]
    fn rotating_a_brush_develop_turns_its_dabs_with_the_frame() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        let src = scratch_baked_photo("brushturn", 480, 320);
        let mut r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: brush_mask(0.1, "r 0.100000\nd 0.100000 0.800000"),
                ..Default::default()
            }],
            ..Default::default()
        };
        rotate_recipe(&mut r, &src, 1).unwrap();
        let stream = |r: &EditRecipe| {
            let MaskGeometry::Brush { strokes, .. } = &r.masks[0].mask else { panic!("a brush") };
            (strokes[0].dabs.clone(), strokes[0].radius)
        };
        let (s, rad) = stream(&r);
        assert_eq!(s, "r 0.150000\nd 0.200000 0.100000", "the dab turned and the radius rescaled");
        assert!((rad - 0.15).abs() < 1e-6, "crs:Radius is {rad}, not 0.15");

        rotate_recipe(&mut r, &src, 1).unwrap();
        let (s, rad) = stream(&r);
        assert_eq!(
            s, "r 0.100000\nd 0.900000 0.200000",
            "the second turn must use the 320 × 480 frame, not the 480 × 320 one"
        );
        assert!((rad - 0.1).abs() < 1e-6, "crs:Radius is {rad}, not 0.1");
    }

    /// …and when the frame CANNOT be read, a develop carrying a brush is
    /// refused rather than half-turned. A brush turned without its rescale is a
    /// mask drawn in the wrong SHAPE, which is the outcome R29 C1 exists to
    /// end, so this joins `rotate_recipe`'s existing all-or-nothing rule.
    ///
    /// The same unreadable photo with NO brush still rotates, because nothing
    /// in that recipe needs an aspect — which is what keeps the read lazy and
    /// keeps every other test in this module on a three-byte `.arw`.
    ///
    /// MUTATION THIS CATCHES: drop the `recipe_has_brush_strokes` guard and
    /// read the frame unconditionally — the second half goes red, and every
    /// rotate in the app starts paying a `RawSource` slurp.
    #[test]
    fn a_brush_develop_whose_frame_cannot_be_read_is_refused_not_half_turned() {
        use crate::recipe::{Crop, LocalAdjustment};
        let raw = scratch_photo("brushnoframe");
        let mut r = EditRecipe {
            crop: Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 }),
            masks: vec![LocalAdjustment {
                mask: brush_mask(0.1, "d 0.100000 0.800000"),
                ..Default::default()
            }],
            ..Default::default()
        };
        let before = r.clone();
        assert!(rotate_recipe(&mut r, &raw, 1).is_err(), "no frame, no turn");
        assert_eq!(r, before, "and nothing moved at all");

        let mut plain = EditRecipe { crop: before.crop, ..Default::default() };
        rotate_recipe(&mut plain, &raw, 1).expect("a develop with no brush needs no frame");
        assert_eq!(plain.quarter_turns, 1);
    }

    /// R27 A4 — raster masks are REALLY turned, unlike the `coord_era`
    /// migration, which could only disclose them.
    ///
    /// The turned bytes land under a FRESH claimed name and the old file stays
    /// on disk: version snapshots freeze their own copies and another saved
    /// recipe may still point at the original, so an in-place rewrite would
    /// silently change what an old version renders.
    ///
    /// MUTATION THIS CATCHES: write the turned image back over `from` instead
    /// of `store::claim_raster`'s new name — the old-file assertion fails, and
    /// with it the promise every version snapshot depends on.
    #[test]
    fn rotating_really_turns_a_raster_mask_into_a_fresh_file() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        let raw = scratch_photo("raster");
        let original = crate::store::claim_raster(&raw, "mask-sky").unwrap();
        // 4 wide × 2 high, one white pixel at (3,0) — a frame whose turn is
        // unambiguous in both dims and in content.
        let mut g = image::GrayImage::new(4, 2);
        g.put_pixel(3, 0, image::Luma([255]));
        g.save(&original).unwrap();

        let mut r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap {
                    path: original.to_string_lossy().into_owned(),
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = rotate_recipe(&mut r, &raw, 1).unwrap();
        assert_eq!(out.rasters_turned, 1);

        let MaskGeometry::Bitmap { path } = &r.masks[0].mask else { panic!("still a bitmap") };
        assert_ne!(
            std::path::Path::new(path),
            original.as_path(),
            "the turned raster must claim a NEW name"
        );
        assert!(original.exists(), "the old raster stays — a version snapshot may share it");
        let turned = image::open(path).unwrap().to_luma8();
        assert_eq!(turned.dimensions(), (2, 4), "a quarter turn transposes the raster");
        // `rotate90` is clockwise: source (x, y) of a W×H frame lands at
        // (H−1−y, x), so (3, 0) → (1, 3).
        assert_eq!(turned.get_pixel(1, 3)[0], 255, "the white pixel moved with the frame");
        assert_eq!(turned.get_pixel(0, 0)[0], 0);
    }

    /// Every file in one develop directory, sorted — the shape a filesystem
    /// assertion needs. Missing directory = no files, so a test may take the
    /// "before" picture before anything has been created.
    fn develop_listing(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// R27 A4, the all-or-nothing half: a raster that cannot be read leaves
    /// the recipe COMPLETELY untouched. A half-turned develop — parametric
    /// masks moved, a painted mask left behind — is worse than a refusal,
    /// because nothing downstream can tell.
    ///
    /// IN MEMORY only, which was this test's blind spot until R28 Batch-3 3b:
    /// one mask cannot fail AFTER another has already been turned, so nothing
    /// here could see the orphan files a partial phase 1 left on disk. That is
    /// `a_failed_turn_leaves_nothing_behind_on_disk` below.
    ///
    /// MUTATION THIS CATCHES: move the raster loop after
    /// `orient_recipe_coords` (i.e. turn geometry first, rasters second) and
    /// the crop below comes back moved with the error.
    #[test]
    fn a_raster_that_cannot_be_turned_leaves_the_whole_recipe_untouched() {
        use crate::recipe::{Crop, LocalAdjustment, MaskGeometry};
        let raw = scratch_photo("refuse");
        let mut r = EditRecipe {
            crop: Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 }),
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: "no-such-raster-ever.png".to_string() },
                ..Default::default()
            }],
            ..Default::default()
        };
        let before = r.clone();
        assert!(rotate_recipe(&mut r, &raw, 1).is_err());
        assert_eq!(r, before, "a refused turn must move nothing at all");
    }

    /// R28 Batch-3 3b (adjudication F8-A) — all-or-nothing ON DISK.
    ///
    /// TWO rasters, the first sound and the second unreadable: phase 1 turns
    /// the first into a freshly claimed file and only then fails, which is the
    /// arrangement the single-mask test above cannot produce. Before the fix
    /// that finished file stayed in the photo's develop dir, referenced by
    /// nothing, one more per attempt — while the GUI toast said "nothing was
    /// changed" (`gui/actions.rs`). The assertion is deliberately the whole
    /// directory rather than one name: it also covers the 0-byte
    /// `store::claim_raster` slot, which is the other thing a failed turn can
    /// leave behind.
    ///
    /// MUTATION THIS CATCHES: delete the `rewritten` cleanup loop in
    /// `rotate_recipe`'s error path and `mask-a-2.png` appears in the listing.
    #[test]
    fn a_failed_turn_leaves_nothing_behind_on_disk() {
        use crate::recipe::{Crop, LocalAdjustment, MaskGeometry};
        let raw = scratch_photo("rollback");
        let dev = crate::store::develop_dir(&raw);

        // Mask A: a real 4×2 raster, turnable.
        let good = crate::store::claim_raster(&raw, "mask-a").unwrap();
        let mut g = image::GrayImage::new(4, 2);
        g.put_pixel(3, 0, image::Luma([255]));
        g.save(&good).unwrap();
        // Mask B: a file that EXISTS and is not an image, so the refusal lands
        // in `open_mask_bounded` — after A is already on disk, turned.
        let bad = crate::store::claim_raster(&raw, "mask-b").unwrap();
        std::fs::write(&bad, b"this is not a png").unwrap();

        let before = develop_listing(&dev);
        assert_eq!(before.len(), 2, "the fixture is two rasters: {before:?}");

        let mut r = EditRecipe {
            crop: Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 }),
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Bitmap { path: good.to_string_lossy().into_owned() },
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Bitmap { path: bad.to_string_lossy().into_owned() },
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let untouched = r.clone();
        assert!(rotate_recipe(&mut r, &raw, 1).is_err(), "an unreadable raster refuses the turn");
        assert_eq!(r, untouched, "the in-memory half, as before");
        assert_eq!(
            develop_listing(&dev),
            before,
            "a refused turn must leave the develop dir exactly as it found it — no turned copy \
             of the raster that DID succeed, no unfilled claim"
        );
    }

    /// R28 Batch-3 3b (adjudication F8-B) — an AI mask's alpha is a CACHE, and
    /// a turn invalidates it rather than migrating it.
    ///
    /// `orient_recipe_coords` moves the reference point and clears the raster
    /// so the next develop re-segments in the turned frame
    /// (`segment::resolve_ai_masks`, whose key now carries the frame). Phase 1
    /// used to turn the alpha anyway, because it walked the STORAGE set
    /// (`bitmap_paths_mut`) rather than the turnable one: the correctly-turned
    /// file was orphaned the moment phase 2 dropped the reference, and it was
    /// counted in `rasters_turned` as if the recipe still held it.
    ///
    /// MUTATION THIS CATCHES: put `MaskGeometry::AiMask { raster: Some(path) }`
    /// back into `turnable_raster_paths_mut` — a turned copy appears in the
    /// listing and `rasters_turned` reads 1.
    #[test]
    fn rotating_invalidates_an_ai_mask_alpha_instead_of_turning_it() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        let raw = scratch_photo("aicache");
        let dev = crate::store::develop_dir(&raw);
        // The name `segment::ai_cache_key` mints, so this is the real file the
        // real cache would have left there.
        let alpha = crate::store::claim_raster(&raw, "ai-mask-0123456789abcdef").unwrap();
        let mut g = image::GrayImage::new(4, 2);
        g.put_pixel(3, 0, image::Luma([255]));
        g.save(&alpha).unwrap();
        let before = develop_listing(&dev);
        let alpha_bytes = std::fs::read(&alpha).unwrap();

        let mut r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::AiMask {
                    name: "Sky 1".into(),
                    subtype: 2,
                    ref_x: 0.25,
                    ref_y: 0.10,
                    blend_mode: 0,
                    value: 1.0,
                    inverted: false,
                    mask_version: 1,
                    provenance: Vec::new(),
                    gesture: Vec::new(),
                    raster: Some(alpha.to_string_lossy().into_owned()),
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = rotate_recipe(&mut r, &raw, 1).unwrap();
        assert_eq!(out.quarter_turns, 1);
        assert_eq!(out.rasters_turned, 0, "a cache is not a raster mask this turn owns");

        let MaskGeometry::AiMask { raster, ref_x, ref_y, .. } = &r.masks[0].mask else {
            panic!("still an AI mask")
        };
        assert!(raster.is_none(), "the stale alpha is dropped, not carried into the new frame");
        // Clockwise: (u, v) → (1−v, u), so (0.25, 0.10) → (0.90, 0.25).
        assert!((*ref_x - 0.90).abs() < 1e-6 && (*ref_y - 0.25).abs() < 1e-6, "{ref_x} {ref_y}");
        assert_eq!(
            develop_listing(&dev),
            before,
            "no turned copy and no fresh claim — the alpha is re-derived, not migrated"
        );
        assert_eq!(std::fs::read(&alpha).unwrap(), alpha_bytes, "and the old file is untouched");
    }

    /// R27 A9 — the crop rectangle and the straighten angle under a quarter
    /// turn, which is where a double application would show up first.
    ///
    /// `Crop` is normalised against the STRAIGHTENED frame, and `inscribed_dims`
    /// is swap-equivariant, so a PURE rotation is exact and leaves the
    /// straighten angle alone (only a MIRROR reverses the sense of a rotation
    /// — R27 Batch-1a's fix, and no quarter turn is a mirror). What must hold:
    /// the rectangle's corners map through `orient_point` and the angle does
    /// NOT change sign.
    ///
    /// MUTATION THIS CATCHES: negate `straighten_deg` on a pure rotation (the
    /// natural over-generalisation of the mirror rule) and the tilt flips,
    /// re-cropping content that has been rotated out from under the box.
    #[test]
    fn a_quarter_turn_moves_the_crop_box_once_and_leaves_the_tilt_alone() {
        use crate::recipe::Crop;
        let raw = scratch_photo("crop");
        let mut r = EditRecipe {
            crop: Some(Crop { left: 0.10, top: 0.20, right: 0.80, bottom: 0.90 }),
            straighten_deg: -3.5,
            ..Default::default()
        };
        rotate_recipe(&mut r, &raw, 1).unwrap();
        // Clockwise: (u, v) → (1−v, u). Corners (0.10,0.20) and (0.80,0.90)
        // land at (0.80,0.10) and (0.10,0.80), re-ordered into a rectangle.
        let c = r.crop.unwrap();
        for (got, want) in [(c.left, 0.10), (c.top, 0.10), (c.right, 0.80), (c.bottom, 0.80)] {
            assert!((got - want).abs() < 1e-6, "crop {got} != {want} ({c:?})");
        }
        assert_eq!(r.straighten_deg, -3.5, "a pure rotation must not touch the tilt");
        assert_eq!(r.quarter_turns, 1);
        // …and the XMP writer's crop pair reads the TURNED rectangle back
        // unchanged, so a round trip cannot apply the turn a second time.
        let doc = crate::xmp::recipe_to_xmp(&r);
        assert!(doc.contains("crs:HasCrop=\"True\""), "{doc}");
        // R27: the sidecar's `crs:CropAngle` is the NEGATION of the engine's
        // clockwise straighten, at Lightroom's own six decimals
        // (`P3-cropangle-model.md` §4, §6.4).
        assert!(doc.contains("crs:CropAngle=\"3.500000\""), "{doc}");
        let back = crate::xmp::xmp_to_recipe(&doc);
        let bc = back.crop.expect("the crop survives the sidecar");
        for (got, want) in
            [(bc.left, c.left), (bc.top, c.top), (bc.right, c.right), (bc.bottom, c.bottom)]
        {
            assert!((got - want).abs() < 1e-6, "crop round trip {got} != {want}");
        }
        assert_eq!(
            back.quarter_turns, 0,
            "classic ACR has no place for a quarter turn (`tiff:Orientation` is R27 A8), so a \
             re-import must report NO turn rather than invent one — the crop it carries is \
             already in the turned frame"
        );
    }

    /// The two load-time migrations are INDEPENDENT facts and are disclosed
    /// as such — a photo can need one, the other, both or neither, and the
    /// GUI picks a different sentence for each.
    #[test]
    fn load_migration_reports_its_two_halves_separately() {
        let none = LoadMigration::default();
        assert!(!none.any() && none.note().is_none());
        let curve_only = LoadMigration { relook: Some("relooked".into()), reframe: None };
        assert!(curve_only.any());
        assert_eq!(curve_only.note().as_deref(), Some("relooked"));
        let frame = CoordMigration {
            orientation: rawler::Orientation::Rotate270,
            rasters_left: false,
        };
        let both = LoadMigration { relook: Some("relooked".into()), reframe: Some(frame) };
        let note = both.note().expect("a note");
        assert!(note.starts_with("relooked · "), "{note}");
        assert!(note.contains("EXIF orientation"), "{note}");
        // The raster gap is SAID, never folded into the success sentence.
        let with_rasters = CoordMigration { rasters_left: true, ..frame };
        assert!(coord_migration_note(with_rasters).contains("could NOT be turned"));
        assert!(!coord_migration_note(frame).contains("could NOT be turned"));
    }

    #[test]
    fn a_primed_answer_is_what_the_repair_consumes() {
        use crate::recipe::CALIB_ERA;
        // Pins the two contracts priming rests on: the primed key is
        // byte-identical to the repair's lookup key, and a primed answer is
        // consumed instead of a fresh estimate. A tuple-shape typo in
        // either would compile, pass clippy and every other test, and
        // silently restore the memo-never-hits behavior priming exists to
        // prevent. (Nonexistent fixture path: curve_ident falls back to the
        // deterministic (0, None).)
        let raw = std::path::PathBuf::from("primed-answer-fixture.arw");
        let primed = vec![[0.0, 0.0], [0.3, 0.55], [1.0, 1.0]];
        prime_curve_memo(&raw, curve_ident(&raw), primed.clone());
        let mut r = EditRecipe {
            version: 1,
            base_curve: vec![[0.0, 0.0], [0.55, 0.10], [0.80, 0.55], [1.0, 1.0]],
            ..Default::default()
        };
        assert!(repair_pre_era_base_curve(&raw, &mut r).is_some());
        assert_eq!(r.base_curve, primed, "the repair consumed the PRIMED answer");
        assert_eq!(r.version, CALIB_ERA);
        // First answer wins: a second prime must not overwrite a live key —
        // the GUI's working edge is a preference, and the memo's contract is
        // that the answer cannot change while the process runs.
        prime_curve_memo(&raw, curve_ident(&raw), Vec::new());
        let mut r2 = EditRecipe {
            version: 1,
            base_curve: vec![[0.0, 0.0], [0.55, 0.10], [0.80, 0.55], [1.0, 1.0]],
            ..Default::default()
        };
        assert!(repair_pre_era_base_curve(&raw, &mut r2).is_some());
        assert_eq!(
            r2.base_curve, primed,
            "prime-side or_insert: the first recorded answer stays (the repair-side \
             adopt arm needs a decodable RAW and is review-verified, not pinned here)"
        );
    }

    #[test]
    fn render_funnel_repairs_with_a_source_and_never_on_a_refusal() {
        use crate::recipe::CALIB_ERA;
        // Process-unique: debug and gui-config `cargo test` runs can overlap,
        // and a shared fixture path lets one process delete the other's probe
        // mid-test — the exact race a round-5 fixture caught.
        let dir = std::env::temp_dir()
            .join(format!("autoshade-pipeline-test-rsc-funnel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_rsc_funnel_probe.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        let washed = vec![[0.0, 0.0], [0.55, 0.10], [0.80, 0.55], [1.0, 1.0]];
        // Seed the process memo with the IDENTITY answer under this file's
        // real identity — exactly what a successful estimate would have left.
        let ident = std::fs::metadata(&raw)
            .ok()
            .map(|m| (m.len(), m.modified().ok()))
            .unwrap_or((0, None));
        fresh_curve_memo().lock().unwrap().insert((raw.clone(), ident), Vec::new());
        // No master recorded: the funnel hands back the RAW and must repair
        // AND return the disclosure with it.
        let mut r = EditRecipe { version: 1, base_curve: washed.clone(), ..Default::default() };
        let (src, note) =
            crate::store::render_source_checked(&raw, &mut r).expect("no master recorded");
        assert_eq!(src, raw);
        assert!(note.is_some(), "the disclosure rides the returned source");
        assert_eq!(r.version, CALIB_ERA);
        assert!(r.base_curve.is_empty(), "the identity answer replaced the washed curve");
        // A recorded-but-unloadable master: Err, and the refusal must leave
        // the recipe UNTOUCHED — every deliverable caller aborts on this arm,
        // and funding a RAW decode for a render that never runs is the tax
        // the Ok-only rule exists to avoid (the one caller that renders
        // anyway, the web preview's degraded fallback, repairs for itself).
        let gone = dir.join("gone-master.png");
        std::fs::write(&gone, b"png").unwrap();
        crate::store::write_pixel_source(&raw, &gone, false).unwrap();
        std::fs::remove_file(&gone).unwrap();
        let mut r2 = EditRecipe { version: 1, base_curve: washed.clone(), ..Default::default() };
        assert!(crate::store::render_source_checked(&raw, &mut r2).is_err());
        assert_eq!(r2.version, 1, "a refusal must not launder the stamp");
        assert_eq!(r2.base_curve, washed, "nor touch the curve");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dev);
    }

    #[test]
    fn an_era_stamp_is_provenance_not_an_edit() {
        // A recipe saved by an older build carries version 1. It must still
        // read as "no edits" — otherwise every legacy photo opens with a
        // permanent edited badge and Ctrl+S writes neutral files instead of
        // clearing (the badge trap R4-8 closed).
        let legacy = EditRecipe { version: 1, ..Default::default() };
        assert!(legacy.is_noop(), "an era-1 neutral recipe is still neutral");
        assert!(EditRecipe::default().is_noop());
        // ...while a real edit stays a real edit in either era.
        let edited = EditRecipe { version: 1, exposure_ev: 0.4, ..Default::default() };
        assert!(!edited.is_noop());
    }

    /// R24-5 M8: the delivery root is a SETTING, and every library-side
    /// deliverable name is claimed through the one funnel that reads it.
    ///
    /// This pins the WIRING rather than a folder: it re-derives each name from
    /// `config::delivery_root()` itself, so it holds whatever the setting says
    /// and fails the moment a caller re-introduces a literal `"out"` — which
    /// is exactly how the five hardcoded copies (CLI deliverable, batch
    /// dedup spelling, pixel masters, style prompt, web download) drifted into
    /// existence.
    #[test]
    fn every_deliverable_name_is_claimed_under_the_delivery_root() {
        let root = crate::config::delivery_root();
        let raw = Path::new("D:/Photography/Raw/2024/Trip/DSC0001.ARW");
        // The deliverable, the `match` report, the style prompt and the
        // preview all come off `default_out` — one funnel, one root.
        for (kind, ext) in
            [("developed", "tif"), ("matched", "json"), ("style", "txt"), ("preview", "jpg")]
        {
            assert_eq!(
                default_out(raw, kind, ext),
                root.join(format!("DSC0001.{kind}.{ext}")),
                "{kind}.{ext} must be claimed under the delivery root"
            );
        }
        // The batch dedup spelling is the one place that used to re-spell the
        // parent directory instead of borrowing it (`(2)` names had their own
        // `PathBuf::from("out")`), so a moved root would have split one batch
        // across two folders.
        let mut names = BatchNames::default();
        let a = names.claim(Path::new("D:/roll-a/DSC0001.ARW"), "developed", "tif");
        let b = names.claim(Path::new("D:/roll-b/DSC0001.ARW"), "developed", "tif");
        assert_eq!(a, root.join("DSC0001.developed.tif"));
        assert_eq!(b, root.join("DSC0001 (2).developed.tif"));
        // An explicitly ROOTED batch (the GUI's export destination, R22-7)
        // still overrides both — the setting is the DEFAULT, not a ceiling.
        let mut rooted = BatchNames::rooted(PathBuf::from("D:/deliver"));
        assert_eq!(
            rooted.claim(raw, "developed", "tif"),
            PathBuf::from("D:/deliver").join("DSC0001.developed.tif")
        );
        // And the read-only-library guard still counts the delivery root as
        // ours: a `match` run on a file that lives there may write beside it.
        let src = root.join("DSC0001.preview.jpg");
        let dst = root.join("DSC0001.matched.json");
        assert!(guard_readonly(&dst, &src).is_ok(), "the delivery root is always writable");
    }

    #[test]
    fn outputs_always_default_outside_the_library() {
        let raw = Path::new("D:/Photography/Raw/2024/Trip/DSC0001.ARW");
        // Exports (deliverable images) stay in ./out; develop STATE (recipe +
        // XMP sidecars) lives in the photo's central develop dir, keyed by the
        // absolute path. Neither ever lands beside the RAW — the library stays
        // read-only by construction.
        assert!(default_out(raw, "developed", "tif").starts_with(crate::config::delivery_root()));
        assert_eq!(xmp_target(raw), crate::store::develop_dir(raw).join("DSC0001.xmp"));
        assert!(guard_readonly(&xmp_target(raw), raw).is_ok(), "the store is always writable");
        // Same stem in a DIFFERENT folder → a different develop dir (the
        // cross-clobber the store exists to prevent).
        let other = Path::new("D:/Photography/Raw/2025/DSC0001.ARW");
        assert_ne!(xmp_target(raw), xmp_target(other));
    }

    #[test]
    fn refine_preserves_engine_only_mask_state_without_blocking_plain_refinement() {
        use crate::recipe::{
            LocalAdjustment, MaskCombine, MaskComponent, MaskGeometry,
        };

        let component = MaskComponent {
            geometry: MaskGeometry::Radial {
                top: 0.2,
                left: 0.2,
                bottom: 0.8,
                right: 0.8,
                feather: 0.4,
                roundness: 0.0,
                flipped: false,
                angle: 0.0,
                midpoint: 50.0,
                mask_version: 2,
            },
            mode: MaskCombine::Subtract,
        };
        let base = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "subject".into(),
                mask: MaskGeometry::Linear {
                    zero_x: 0.0,
                    zero_y: 0.5,
                    full_x: 1.0,
                    full_y: 0.5,
                },
                components: vec![component.clone()],
                enabled: false,
                exposure_ev: 0.25,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut proposed = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "subject".into(),
                mask: MaskGeometry::Linear {
                    zero_x: 0.1,
                    zero_y: 0.4,
                    full_x: 0.9,
                    full_y: 0.6,
                },
                exposure_ev: 0.75,
                ..Default::default()
            }],
            ..Default::default()
        };
        carry_over_unrepresentable(&mut proposed, &base, LensOpinion::default(), None);
        assert!(!proposed.masks[0].enabled);
        assert_eq!(proposed.masks[0].components, vec![component]);
        assert_eq!(
            proposed.masks[0].exposure_ev, 0.75,
            "the model's representable slider refinement must survive"
        );

        let plain_base = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "plain".into(),
                exposure_ev: 0.2,
                ..Default::default()
            }],
            ..Default::default()
        };
        let expected = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "plain".into(),
                mask: MaskGeometry::Linear {
                    zero_x: 0.2,
                    zero_y: 0.3,
                    full_x: 0.8,
                    full_y: 0.7,
                },
                exposure_ev: 0.9,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut refined = expected.clone();
        carry_over_unrepresentable(&mut refined, &plain_base, LensOpinion::default(), None);
        assert_eq!(refined, expected, "plain masks still take the model response exactly");
    }

    #[test]
    fn a_renamed_state_bearing_mask_falls_back_to_the_base_masks() {
        use crate::recipe::{
            LocalAdjustment, MaskCombine, MaskComponent, MaskGeometry,
        };

        // The user's mask carries engine-only state (a Subtract component).
        let base = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "subject".into(),
                components: vec![MaskComponent {
                    geometry: MaskGeometry::Radial {
                        top: 0.2,
                        left: 0.2,
                        bottom: 0.8,
                        right: 0.8,
                        feather: 0.4,
                        roundness: 0.0,
                        flipped: false,
                        angle: 0.0,
                        midpoint: 50.0,
                        mask_version: 2,
                    },
                    mode: MaskCombine::Subtract,
                }],
                exposure_ev: 0.25,
                ..Default::default()
            }],
            ..Default::default()
        };
        // The model returned its refined copy under a NEW name — the exact
        // case the old same-name discard sailed past: keeping it alongside
        // the carried base would apply the coverage twice.
        let mut proposed = EditRecipe {
            masks: vec![LocalAdjustment {
                name: "subject refined".into(),
                exposure_ev: 0.9,
                ..Default::default()
            }],
            ..Default::default()
        };
        carry_over_unrepresentable(&mut proposed, &base, LensOpinion::default(), None);
        assert_eq!(
            proposed.masks, base.masks,
            "identity lost ⇒ the base masks stand, the response's mask edits are discarded"
        );
        assert!(
            proposed.rationale.contains("did not preserve mask identities"),
            "the silent revert must be disclosed: {:?}",
            proposed.rationale
        );
    }

    /// R15: the fit calibration stamp copies ALL FIVE fields — era, curve,
    /// lens profile, both as-shot anchors. A partial stamp re-opens the
    /// dark-base disagreement the stamp exists to close.
    #[test]
    fn the_fit_stamp_carries_the_whole_calibration() {
        let cal = PhotoCalibration {
            version: 1,
            base_curve: vec![[0.0, 0.0], [0.4, 0.6], [1.0, 1.0]],
            lens_profile: crate::recipe::LensProfile {
                vignette: vec![1.0, 1.1],
                vignette_on: true,
                ..Default::default()
            },
            as_shot_k: Some(5476.0),
            as_shot_tint: Some(15.2),
        };
        let mut r = EditRecipe::default();
        stamp_fit_calibration(&mut r, cal);
        assert_eq!(r.version, 1, "the era stamp rides WITH the curve");
        assert_eq!(r.base_curve, vec![[0.0, 0.0], [0.4, 0.6], [1.0, 1.0]]);
        assert!(r.lens_profile.vignette_on && r.lens_profile.vignette == vec![1.0, 1.1]);
        assert_eq!((r.as_shot_k, r.as_shot_tint), (Some(5476.0), Some(15.2)));
    }

    /// R16: `calibration_recipe` carries every calibration half into the
    /// fit base and NOTHING else — a stray user field there would make the
    /// solve start from an edit instead of a calibration.
    #[test]
    fn the_calibration_recipe_is_calibration_only() {
        let cal = PhotoCalibration {
            version: 1,
            base_curve: vec![[0.0, 0.0], [0.25, 0.4], [0.6, 0.8], [1.0, 1.0]],
            lens_profile: crate::recipe::LensProfile {
                vignette: vec![1.0, 1.05],
                vignette_on: true,
                ..Default::default()
            },
            as_shot_k: Some(5476.0),
            as_shot_tint: Some(15.2),
        };
        let r = calibration_recipe(cal);
        assert_eq!(r.version, 1);
        assert_eq!(r.base_curve.len(), 4);
        assert!(r.lens_profile.vignette_active());
        assert_eq!((r.as_shot_k, r.as_shot_tint), (Some(5476.0), Some(15.2)));
        let stripped = EditRecipe {
            version: EditRecipe::default().version,
            base_curve: Vec::new(),
            lens_profile: Default::default(),
            as_shot_k: None,
            as_shot_tint: None,
            ..r
        };
        assert!(
            serde_json::to_string(&stripped).unwrap()
                == serde_json::to_string(&EditRecipe::default()).unwrap(),
            "beyond the five calibration halves the base must be a default recipe"
        );
    }

    /// R16 pin: the COMPOSED fit (`fit_recipe_from`) starts from the
    /// calibration base and must hand it back untouched — its solve stages
    /// own tone/saturation/curves only. On a target that IS a develop of
    /// the same base plus a representable grade, the solve must close most
    /// of the gap WITHOUT pegging the saturation cap (the R15 murk was
    /// exactly the cap burning on the base look), and the v0.24.0 two-pass
    /// clamp-order gap cannot exist: solve renders and the canvas render
    /// are the same one-pass `user(base(x))` call by construction.
    #[test]
    fn the_composed_fit_preserves_and_solves_on_its_calibration() {
        let src = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(96, 64, |x, y| {
            image::Rgb([
                (40 + x * 2).min(255) as u8,
                (30 + y * 3).min(255) as u8,
                (60 + x + y).min(255) as u8,
            ])
        }));
        let base = calibration_recipe(PhotoCalibration {
            version: crate::recipe::CALIB_ERA,
            base_curve: vec![[0.0, 0.0], [0.22, 0.26], [0.48, 0.74], [1.0, 1.0]],
            lens_profile: Default::default(),
            as_shot_k: Some(5476.0),
            as_shot_tint: Some(15.2),
        });
        let graded = EditRecipe {
            exposure_ev: 0.3,
            contrast: 20.0,
            saturation: 25.0,
            ..base.clone()
        };
        let target = crate::render::develop_preview(&src, &graded);
        let rep = crate::fit::fit_recipe_from(&src, &target, &base);
        assert_eq!(
            rep.recipe.base_curve, base.base_curve,
            "the solve must not touch the calibration curve"
        );
        assert_eq!(
            (rep.recipe.as_shot_k, rep.recipe.as_shot_tint),
            (Some(5476.0), Some(15.2)),
            "the as-shot anchors ride through the solve"
        );
        // The engine's own do-no-harm promise (an artificial ramp is exactly
        // the content the near-neutral tone gates were never tuned for, so
        // "closes most of the gap" belongs to the real-pair harness, not
        // here — the terminal reset ceiling is the hard contract).
        assert!(
            rep.err_after <= rep.err_before * 1.25 + 1.3e-3,
            "the composed fit broke its own do-no-harm ceiling ({:.4} -> {:.4})",
            rep.err_before,
            rep.err_after
        );
        assert!(
            rep.recipe.saturation.abs() < 60.0,
            "the saturation cap must have slack on a mild grade (got {:+.1})",
            rep.recipe.saturation
        );
        // THE R16 claim, pinned exactly: the reported residual IS the
        // residual of the canvas's DEVELOP pass (the tonal/colour chain;
        // the GUI's later lens-geometry resample is outside the fit's model
        // — pre-existing and second-order). Recompute err_after through the
        // engine's one-pass path at the fit's own analysis scale and demand
        // equality (the v0.24.0 two-pass mechanism failed this by up to
        // 18.7/255 of pixel drift on saturated content).
        let (s_thumb, t_thumb) = crate::fit::analysis_pair(&src, &target);
        let target_px = crate::fit::pixels_of(&t_thumb);
        let canvas_px = crate::fit::pixels_of(&crate::render::develop_preview(&s_thumb, &rep.recipe));
        let canvas_err =
            crate::fit::look_err_with_evidence(&canvas_px, &target_px, &rep.evidence);
        assert!(
            (canvas_err - rep.err_after).abs() < 1e-6,
            "the fit's number must describe the canvas render exactly \
             (reported {:.6}, canvas {:.6})",
            rep.err_after,
            canvas_err
        );
    }

    /// R16 pin: every refusal path still carries the calibration — a
    /// degenerate pair (flat frame) returns the base look, never the bare
    /// dark-neutral default the R15 murk came from. err_after == err_before
    /// by definition (the refusal renders AS the base).
    #[test]
    fn a_degenerate_pair_still_carries_the_calibration() {
        let flat = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            48,
            image::Rgb([128, 128, 128]),
        ));
        let base = calibration_recipe(PhotoCalibration {
            version: crate::recipe::CALIB_ERA,
            base_curve: vec![[0.0, 0.0], [0.25, 0.4], [0.6, 0.8], [1.0, 1.0]],
            lens_profile: Default::default(),
            as_shot_k: None,
            as_shot_tint: None,
        });
        let rep = crate::fit::fit_recipe_from(&flat, &flat, &base);
        assert_eq!(
            rep.recipe.base_curve, base.base_curve,
            "the degenerate refusal must keep the camera look"
        );
        assert!(
            (rep.err_after - rep.err_before).abs() < 1e-6,
            "a refusal reports the base look's own error on both sides"
        );
    }

    /// R16 real-pair harness (ignored): set AUTOSHADE_FIT_REPRO_RAW and
    /// AUTOSHADE_FIT_REPRO_TARGET to a photo and a rendition of it, then run
    /// with `-- --ignored r16 --nocapture`. Prints the OLD neutral-source
    /// fit next to the NEW composed-calibration fit. Pure lib calls — the
    /// develop store is read (calibration authority) but never written.
    ///
    /// R24 batch 2 ran this on six real pairs to retire the joint ladder's
    /// "provisional" label (the table is `fit::tests::
    /// joint_family_is_calibrated_on_the_fixture_set`'s doc). Composed-domain
    /// joint reading, base → fitted:
    ///   A1 astro composite   0.402 → 0.064     A4 low-sat  0.082 → 0.060
    ///   A2 vivid warm        0.332 → 0.064     A5 cropped  0.052 → 0.034
    ///   A3 monochrome        0.050 → 0.014     A6 portrait 0.089 → 0.028
    ///
    /// TWO findings that belong with the numbers, because the next person to
    /// run this harness will otherwise re-derive them the hard way:
    ///   * The reading is DOMAIN-DEPENDENT, and this harness's domain is not
    ///     the CLI's. `autoshade match` solves on the embedded preview and
    ///     stamps the calibration afterwards; this solves with the
    ///     calibration COMPOSED in. On A1 the composed solve is genuinely the
    ///     better fit (look error 0.032 vs the CLI's 0.061), and its joint
    ///     reading says so — 0.064 against the CLI's 0.141. The ladder is
    ///     calibrated on the CLI-domain numbers because that is the domain
    ///     the user's failure was reported in; a pair that reads far HERE is
    ///     therefore worse than the constant suggests, not better.
    ///   * Every one of the six improves by far more than
    ///     [`crate::fit_zoned::JOINT_DRIFT_TOL`], so no real pair has yet
    ///     come at that guard from the wrong side.
    #[test]
    #[ignore = "real-photo repro: needs AUTOSHADE_FIT_REPRO_RAW/_TARGET"]
    fn r16_composed_fit_on_a_real_pair() {
        let (Ok(raw), Ok(tgt)) = (
            std::env::var("AUTOSHADE_FIT_REPRO_RAW"),
            std::env::var("AUTOSHADE_FIT_REPRO_TARGET"),
        ) else {
            panic!("set AUTOSHADE_FIT_REPRO_RAW and AUTOSHADE_FIT_REPRO_TARGET");
        };
        let raw = std::path::PathBuf::from(raw);
        let neutral =
            crate::render::render_to_image(&raw, &EditRecipe::default(), None, Some(1280))
                .expect("neutral develop");
        let target =
            // baked-by-construction: the repro env var names a finished rendition.
            crate::decode::load_image(std::path::Path::new(&tgt)).expect("target loads");
        let old = crate::fit::fit_recipe(&neutral, &target);
        eprintln!(
            "OLD (neutral source):      err {:.4} -> {:.4}, ev {:+.2}, sat {:+.1}, cast {}",
            old.err_before,
            old.err_after,
            old.recipe.exposure_ev,
            old.recipe.saturation,
            if old.recipe.red_curve.is_empty() { "withheld/empty" } else { "attached" },
        );
        let fit_base = calibration_recipe(fit_calibration(&raw));
        // The tone gate's misprediction reading in THIS (composed) domain —
        // the anchor table at `NEUTRAL_MISPREDICTION_MAX` was measured in
        // the embedded-preview domain; this prints the composed-domain
        // counterpart per pair so both domains stay measured.
        {
            let edge = crate::fit::ANALYZE_EDGE;
            let s_base = crate::render::develop_preview(&neutral.thumbnail(edge, edge), &fit_base);
            let m = crate::fit::neutral_gate_misprediction(
                &crate::fit::pixels_of(&s_base),
                &crate::fit::pixels_of(&target.thumbnail(edge, edge)),
            );
            eprintln!("composed-domain misprediction: {m:.4}");
        }
        let new = crate::fit::fit_recipe_from(&neutral, &target, &fit_base);
        eprintln!(
            "NEW (composed calibration): err {:.4} -> {:.4}, ev {:+.2}, sat {:+.1}, cast {}, base {} pts",
            new.err_before,
            new.err_after,
            new.recipe.exposure_ev,
            new.recipe.saturation,
            if new.recipe.red_curve.is_empty() { "withheld/empty" } else { "attached" },
            new.recipe.base_curve.len(),
        );
        // Optional artifact for visual verification: the composed recipe as
        // JSON, renderable via `autoshade apply` (third repro round on this
        // pair — the eyes keep finding what the scalar cannot). Written
        // BEFORE the comparison assert: the artifact is diagnostics, not a
        // prize for passing.
        if let Ok(out) = std::env::var("AUTOSHADE_FIT_REPRO_OUT") {
            std::fs::write(&out, serde_json::to_string_pretty(&new.recipe).unwrap())
                .expect("write repro recipe");
            eprintln!("repro recipe -> {out}");
        }
        // The composed solve pays a small stiffness cost for carrying the
        // calibration (sliders act through the base curve's compression):
        // measured 0.0252 -> 0.0267 (+6% relative) on the live pair after
        // R17 un-poisoned both solves' tone evidence. The bound owed here is
        // "seeding must not MEANINGFULLY regress the fit", expressed as 10%
        // relative plus the additive floor the old absolute bound used —
        // diagnostics against the retired arm, not the contract.
        assert!(
            new.err_after <= old.err_after * 1.10 + 1e-3,
            "seeding must not meaningfully regress the fit ({:.4} vs {:.4})",
            new.err_after,
            old.err_after
        );
        // The contract R17 actually bought, standing on its own: the
        // composed render's luma quantiles sit ON the target's (the murk
        // this pair keeps re-teaching was ~13/255 of mid-band gap while the
        // scalar claimed victory; measured post-R17: 1.6/255 mean).
        let luma_q = |img: &image::DynamicImage| -> Vec<f32> {
            let mut l: Vec<f32> = crate::fit::pixels_of(img)
                .iter()
                .map(|p| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2])
                .collect();
            l.sort_by(f32::total_cmp);
            [0.05f32, 0.25, 0.50, 0.75, 0.95]
                .iter()
                .map(|&p| l[((l.len() - 1) as f32 * p) as usize])
                .collect()
        };
        let edge = crate::fit::ANALYZE_EDGE;
        let fitted = crate::render::develop_preview(&neutral.thumbnail(edge, edge), &new.recipe);
        let qf = luma_q(&fitted);
        let qt = luma_q(&target.thumbnail(edge, edge));
        let mean_gap: f32 =
            qf.iter().zip(&qt).map(|(a, b)| (a - b).abs()).sum::<f32>() / qf.len() as f32;
        assert!(
            mean_gap <= 6.0 / 255.0,
            "the composed render drifted off the target's tone distribution \
             (mean quantile gap {:.1}/255)",
            mean_gap * 255.0
        );
        // R23-6 E-14: the five global luma quantiles above are the whole
        // contract this harness had, and they are structurally blind to the
        // failure the user reports — a fit whose value ranges are wrong can
        // hold every global quantile. Print the JOINT family per bucket
        // (base vs finished, so the direction is visible, not just the
        // level), and assert the same bounded-drift rule the fit itself
        // applies. The per-bucket ASSERTION stays opt-in until real failure
        // pairs exist to calibrate a per-bucket ceiling against
        // (AUTOSHADE_FIT_REPRO_JOINT_MAX=<f32>) — a number invented here
        // would be a guess wearing a test's clothes.
        let src_px = crate::fit::pixels_of(&crate::render::develop_preview(
            &neutral.thumbnail(edge, edge),
            &fit_base,
        ));
        let tgt_px = crate::fit::pixels_of(&target.thumbnail(edge, edge));
        let fit_px = crate::fit::pixels_of(&fitted);
        for (side, px) in [("base", &src_px), ("fitted", &fit_px)] {
            for b in crate::fit_zoned::joint_buckets(px, &tgt_px) {
                eprintln!(
                    "joint {side:<6} {:<18} err {:.4}  share {:.3}",
                    b.label, b.err, b.share
                );
            }
        }
        let jb = crate::fit_zoned::joint_reading(&src_px, &tgt_px);
        let ja = crate::fit_zoned::joint_reading(&fit_px, &tgt_px);
        eprintln!("joint reading: base {jb:?}\njoint reading: fit  {ja:?}");
        // The LADDER's verdict on this pair, printed beside the raw reading
        // (R24 batch 2): the numbers above are what a calibration round
        // measures, but "does this pair warn, and what does it claim" is what
        // a calibration round DECIDES, and re-deriving it by hand from two
        // constants is how a review misses that the two disagree.
        eprintln!(
            "ladder: confidence {:.3}, joint FAR warning {} (line {}, reading {:.4})",
            new.recipe.confidence,
            if ja.is_some_and(|r| r.weighted >= crate::fit_zoned::JOINT_FAR_ERR) {
                "RAISED"
            } else {
                "silent"
            },
            crate::fit_zoned::JOINT_FAR_ERR,
            ja.map(|r| r.weighted).unwrap_or(f32::NAN),
        );
        eprintln!(
            "ladder: same-frame check {} (source {}x{}, target {}x{})",
            if crate::fit::same_frame_plausible(&neutral, &target) {
                "passed"
            } else {
                "WARNED — the reported confidence is capped"
            },
            neutral.width(),
            neutral.height(),
            target.width(),
            target.height(),
        );
        if let (Some(b), Some(a)) = (jb, ja) {
            assert!(
                a.weighted <= b.weighted + crate::fit_zoned::JOINT_DRIFT_TOL,
                "the fit pushed the joint value-range distributions further \
                 apart than leaving the photo alone ({:.4} -> {:.4})",
                b.weighted,
                a.weighted
            );
            if let Ok(max) = std::env::var("AUTOSHADE_FIT_REPRO_JOINT_MAX") {
                let max: f32 = max.trim().parse().expect("AUTOSHADE_FIT_REPRO_JOINT_MAX");
                for bucket in crate::fit_zoned::joint_buckets(&fit_px, &tgt_px) {
                    assert!(
                        bucket.err <= max,
                        "value range {} misses by {:.4} (ceiling {max})",
                        bucket.label,
                        bucket.err
                    );
                }
            }
        }
    }

    /// L09#1: the pre-pay output preflight — a directory target refuses
    /// with a message naming it (the case that used to bill the analysis
    /// first and bail at write_recipe after).
    #[test]
    fn preflight_out_refuses_a_directory_target() {
        let root =
            std::env::temp_dir().join(format!("autoshade-preflight-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // The raw lives in its OWN folder: targeting the raw's folder trips
        // the read-only-library guard first, which is correct but not what
        // this test pins — the directory-shape refusal is.
        let target_dir = root.join("exports");
        std::fs::create_dir_all(&target_dir).unwrap();
        let raw_dir = root.join("library");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let raw = raw_dir.join("photo.arw");
        let e = preflight_out(&target_dir, &raw).unwrap_err().to_string();
        assert!(e.contains("is a directory"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// L10-7: the write probe consumes itself — an export dir must not
    /// accumulate probe residue on every preflighted command.
    #[test]
    fn preflight_out_leaves_no_probe_residue() {
        let root =
            std::env::temp_dir().join(format!("autoshade-preflight-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let raw_dir = root.join("library");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let raw = raw_dir.join("photo.arw");
        let target = root.join("exports").join("x.png");
        preflight_out(&target, &raw).expect("a writable parent passes");
        let leftovers: Vec<_> = std::fs::read_dir(root.join("exports"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(leftovers.is_empty(), "the probe cleaned up after itself: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// L10-8: a missing analysis key refuses BEFORE the paid proposal (and
    /// before the decode — the fixture path does not even exist).
    #[test]
    fn a_missing_analysis_key_refuses_before_the_paid_proposal() {
        let cfg = crate::config::Config {
            openai_api_key: Some("test-key".into()),
            openai_model: "test-chat".into(),
            openai_base_url: "http://127.0.0.1:1".into(),
            openai_image_model: "test-image".into(),
            openai_image_quality: "auto".into(),
            openai_image_max_px: 4_000_000,
            image_provider: "api".into(),
            image_effort: None,
            analysis_provider: "api".into(),
            analysis_model: "gpt-5.5".into(),
            analysis_effort: None,
            claude_bin: "claude".into(),
            analysis_api_key: None,
            analysis_base_url: "http://127.0.0.1:1".into(),
            python_bin: "python".into(),
            denoise_model: "scunet_color_real_psnr".into(),
            denoise_script: String::new(),
            denoise_cache: String::new(),
            segment_script: String::new(),
            embed_script: String::new(),
            correspond_script: String::new(),
            describe_script: String::new(),
            style_strength: 0.5,
            send_reference_image: false,
        };
        let e = produce_recipe(
            Path::new("this-file-does-not-exist.arw"),
            &cfg,
            false,
            None,
            None,
            GradeRequest::default(),
            false,
            crate::diag::stderr(),
        )
        .expect_err("no analysis key + api provider must refuse up front")
        .to_string();
        assert!(
            e.contains("analysis") && e.contains("AFTER the paid proposal"),
            "the refusal names the reason, not a decode error: {e}"
        );
    }

    /// R23-2, feedback #6: the develop that ASKED for personal style and got
    /// none must SAY so — in all three arms. Before this, the condition was
    /// "an index file exists and failed to load", so two of the three were
    /// silent: a fresh install (no library) and a retrieval that matched
    /// nothing both left a Style slider that visibly did nothing.
    ///
    /// The arms are exercised at the seam they converge on, and the mapping
    /// from each arm to its triple is pinned by the loader's own test
    /// (`style::the_shared_status_read_reports_absent_built_and_unusable`)
    /// plus `render_reference([]) == None` (`style.rs`).
    #[test]
    fn every_silent_style_arm_now_discloses_itself() {
        use crate::rationale::{keys, render_one};
        let note = |strength, reference, err| style_gap_note(strength, reference, err);

        // Arm (a): no index file at all — `EffectiveIndex::Absent`, so no
        // loader error, and no reference.
        let a = note(0.65, None, None).expect("a fresh install must not stay silent");
        assert_eq!(a.key, keys::STYLE_NO_REFERENCE);
        let text = render_one(&a);
        assert!(text.contains("65%"), "the slider position the user set: {text}");
        assert!(
            text.contains("Style reference library"),
            "and where to fix it, in the GUI the user is looking at: {text}"
        );

        // Arm (b): the index LOADED, but `retrieve` matched nothing, so
        // `render_reference` returned None. Same user-visible fact, same note.
        let b = note(0.3, None, None).expect("a retrieval that matched nothing is not silent");
        assert_eq!(b.key, keys::STYLE_NO_REFERENCE);

        // Arm (c): a file exists and cannot be used — the loader's message
        // survives, because it names what to rebuild.
        let c = note(0.3, None, Some("style index … is version 2 (current 3)"))
            .expect("an unusable index still discloses");
        assert_eq!(c.key, keys::STYLE_UNAVAILABLE);
        assert!(render_one(&c).contains("is version 2"), "the cause rides along");

        // …and the two NEGATIVE controls, which is what stops this from
        // becoming a note on every develop: a reference that arrived, and a
        // slider the user turned off.
        assert!(
            note(0.65, Some("STYLE REFERENCE — …"), None).is_none(),
            "no complaint when the reference is right there"
        );
        assert!(
            note(0.0, None, None).is_none(),
            "style switched OFF is not a failure to disclose"
        );
    }

    /// R23-2, feedback #6's headline ("I have no idea which library it is
    /// referencing"): a develop that DID get a reference names the shots it
    /// leaned on. Driven through the same seam the pipeline uses, over stub
    /// exemplars — the production path needs an index and a paid call.
    #[test]
    fn a_develop_names_the_past_shots_it_referenced() {
        use crate::rationale::render_one;
        use crate::style::StyleExemplar;
        let ex = |stem: &str| StyleExemplar {
            stem: stem.into(),
            feat: vec![0.0; 14],
            tag: "wide/mid/midday/landscape".into(),
            settings: std::collections::BTreeMap::new(),
            curve: None,
            path: Some(format!("D:\\rolls\\{stem}.ARW")),
            families: None,
            embed: None,
            tags: Vec::new(), vocab_scores: None, desc: None, desc_embed: None,
            masks: None,
        };
        let all = [ex("DSC0001"), ex("DSC0002"), ex("DSC0003")];
        let refs: Vec<&StyleExemplar> = all.iter().collect();
        let stems = crate::style::neighbour_stems(&refs);
        let text = render_one(&style_neighbours_note(&stems).expect("a retrieval discloses itself"));
        assert!(text.contains("DSC0001, DSC0002, DSC0003"), "the shots by name: {text}");
        assert!(text.contains("the 3 most similar"), "…and how many answered: {text}");
        assert!(
            !text.contains("D:\\rolls"),
            "file names, not the folder layout (this string is persisted and displayed): {text}"
        );
        // Nothing retrieved ⇒ no claim (the gap note owns that case).
        assert!(style_neighbours_note(&[]).is_none());
    }

    /// L09#1: a missing parent is created up-front (the documented
    /// trade-off: an empty dir if the paid call then fails beats burning
    /// the call), and a parent that is a FILE refuses — the exact failure
    /// that used to land after payment.
    #[test]
    fn preflight_out_creates_a_missing_parent_and_refuses_a_file_parent() {
        let root =
            std::env::temp_dir().join(format!("autoshade-preflight-par-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // Separate library dir — a target under the raw's own folder is the
        // guard's case, not this test's (see the directory-target test).
        let raw_dir = root.join("library");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let raw = raw_dir.join("photo.arw");
        let target = root.join("exports").join("a").join("c.png");
        preflight_out(&target, &raw).expect("a missing parent chain is created");
        assert!(root.join("exports").join("a").is_dir(), "the parent now exists");
        let f = root.join("f.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(
            preflight_out(&f.join("x.png"), &raw).is_err(),
            "a file in the parent chain refuses (create_dir_all fails)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn look_reference_image_replaces_the_raw_neighbour_and_is_named() {
        let retrieved = StyleRetrieval {
            reference: Some("raw reference".into()),
            targets: std::collections::BTreeMap::new(),
            stems: vec!["raw-shot".into()],
            nearest: Some("D:\\raw\\raw-shot.arw".into()),
            look_reference: Some("finished look reference".into()),
            look_nearest: Some("D:\\looks\\finished.jpg".into()),
            look_stem: Some("finished".into()),
            look_tags: vec!["warm golden tones".into()],
            look_summary: None,
            looks_unreachable: false,
            looks_count: 1,
        };
        let (path, is_look) = reference_image_choice(&retrieved).expect("look answer");
        assert_eq!(path, "D:\\looks\\finished.jpg");
        assert!(is_look, "the selected image is explicitly from the look library");
        assert!(crate::rationale::keys::STYLE_LOOK_IMAGE.contains("look photo"));
    }

    /// B2: the style-look summary reaches EVERY reviewer of one analysis —
    /// the verifier, the first judgement, and the re-judge inside every
    /// hint-guided revision round.
    ///
    /// The last of those is the load-bearing one. The judge can BUY a revision,
    /// so a re-judge that did not know the look would spend the revision
    /// removing it — which is the observed behaviour this batch exists to fix.
    /// `produce_recipe` builds ONE `GradeIntent` through `grade_intent` and
    /// every reviewer closure closes over that one value, so the rounds cannot
    /// disagree; the second half of this test is what keeps it that way.
    ///
    /// MUTATION: drop the `style_look` argument from `grade_intent`'s
    /// initializer (the first assert fails), or hand the hint round a freshly
    /// built `GradeIntent` of its own (the source guard fails).
    #[test]
    fn the_style_look_summary_reaches_every_reviewer_including_the_revision_round() {
        let req = GradeRequest {
            strength: crate::recipe::GradeStrength::new(0.9),
            adherence: DirectionAdherence::new(0.2),
            ..GradeRequest::with_style(0.7)
        };
        let intent = grade_intent(&req, Some("moodier"), Some("warm golden tones"));
        assert_eq!(intent.style_look, Some("warm golden tones"), "the look must reach the intent");
        assert_eq!(intent.direction, Some("moodier"), "and it must not displace the direction");
        assert_eq!(intent.strength.get(), 0.9);
        assert_eq!(intent.adherence.get(), 0.2);
        // No library, no claim: the reviewers are told nothing rather than an
        // empty look.
        assert_eq!(grade_intent(&req, None, None).style_look, None);

        // …and there is exactly ONE place in this file that builds one. A
        // second literal is how the revision round would come to judge against
        // a different brief than the round it is revising — the needle is
        // assembled so this assertion cannot match itself.
        let needle = concat!("GradeIntent", " {");
        assert_eq!(
            include_str!("pipeline.rs").matches(needle).count(),
            1,
            "every reviewer in this file must share `grade_intent`'s one construction"
        );
    }

    /// B2, the retrieval half: a develop with a look library hands the
    /// reviewers the phrases, and one without hands them nothing.
    ///
    /// MUTATION: make the `look_summary` field of `StyleRetrieval` always
    /// `None` in `produce_recipe`'s retrieval closure and the first assert
    /// fails (it is the same `style::StyleIndex::look_summary` call).
    #[test]
    fn the_retrieval_carries_the_look_summary_the_reviewers_are_given() {
        let look = crate::style::LookExemplar {
            stem: "finished".into(),
            path: "D:\\looks\\finished.jpg".into(),
            embed: Vec::new(),
            tags: vec!["warm golden tones".into()],
            vocab_scores: None,
            desc: None,
            desc_embed: None,
        };
        let summary = crate::style::StyleIndex::look_summary(&[&look], &[]);
        assert!(
            summary.as_deref().is_some_and(|s| s.starts_with("warm golden tones")),
            "{summary:?}"
        );
        let retrieved = StyleRetrieval {
            reference: None,
            targets: Default::default(),
            stems: Vec::new(),
            nearest: None,
            look_reference: None,
            look_nearest: None,
            look_stem: None,
            look_tags: Vec::new(),
            look_summary: summary.clone(),
            looks_unreachable: false,
            looks_count: 1,
        };
        let req = GradeRequest::with_style(0.7);
        let intent = grade_intent(&req, None, retrieved.look_summary.as_deref());
        assert_eq!(intent.style_look, summary.as_deref());
        // An empty library answers None, and the reviewers are told nothing.
        assert_eq!(crate::style::StyleIndex::look_summary(&[], &[]), None);

        // …and `produce_recipe`'s retrieval closure is what FILLS that field.
        // A unit test cannot drive that closure — it needs a built index and a
        // real RAW — so the wiring is asserted at the source, the same way the
        // one-intent rule above is. Without this the field could be pinned to
        // `None` in production and every assertion above would stay green.
        let src = include_str!("pipeline.rs");
        assert!(
            src.contains(concat!("look_summary: crate::style::StyleIndex::", "look_summary(&looks, &ex)")),
            "the retrieval closure must fill `look_summary` from `style::StyleIndex::look_summary`"
        );
    }

    #[test]
    fn adherence_note_rides_only_with_a_direction() {
        assert_eq!(direction_adherence_tier(None, DirectionAdherence::default()), None);
        assert_eq!(direction_adherence_tier(Some("   "), DirectionAdherence::default()), None);
        assert_eq!(
            direction_adherence_tier(Some("warmer"), DirectionAdherence::new(0.2)),
            Some("hint")
        );
        assert_eq!(
            direction_adherence_tier(Some("warmer"), DirectionAdherence::default()),
            Some("direct")
        );
        assert_eq!(
            direction_adherence_tier(Some("warmer"), DirectionAdherence::new(0.9)),
            Some("brief")
        );
    }

    /// A look image that will not load costs the develop its LOOK, not its
    /// reference image.
    ///
    /// `reference_image_choice` prefers the look library, so a look file the
    /// user moved or deleted used to end the reference-image path entirely -
    /// the RAW neighbour that was sitting right there was never tried, and the
    /// user had paid for a two-image call and received one image plus a note.
    ///
    /// MUTATION: delete the `reference_image_fallback` arm and the fallback
    /// assertion fails.
    #[test]
    fn a_look_image_that_will_not_load_falls_back_to_the_raw_neighbour() {
        let both = StyleRetrieval {
            reference: None,
            targets: Default::default(),
            stems: vec!["raw-shot".into()],
            nearest: Some("D:\\raw\\raw-shot.arw".into()),
            look_reference: None,
            look_nearest: Some("D:\\looks\\gone.jpg".into()),
            look_stem: Some("gone".into()),
            look_tags: Vec::new(),
            look_summary: None,
            looks_unreachable: false,
            looks_count: 1,
        };
        // The chooser still prefers the look…
        let (chosen, is_look) = reference_image_choice(&both).expect("a look is preferred");
        assert_eq!(chosen, "D:\\looks\\gone.jpg");
        assert!(is_look);
        // …and the fallback is the RAW neighbour, which is what the failure arm
        // now reaches for.
        assert_eq!(reference_image_fallback(&both), Some("D:\\raw\\raw-shot.arw"));
        // With no RAW neighbour recorded there is nothing to fall back to, and
        // the arm must say None rather than invent a path.
        let look_only = StyleRetrieval { nearest: None, ..both };
        assert_eq!(reference_image_fallback(&look_only), None);
    }

    /// `style-query` and the develop rank through ONE helper, and the
    /// diagnostic prints the terms that helper produced.
    ///
    /// This used to be a grep for the string `"pipeline::retrieve_style("` in
    /// `main.rs`, which said nothing about the numbers: `distance_components`
    /// re-implemented the 14-dimension sum, so the diagnostic could print terms
    /// the ranking never used and both greps would still pass. The behavioural
    /// half now lives in `style::the_diagnostic_prints_the_terms_the_ranking_used`;
    /// what remains here is the one thing a behavioural test cannot see - that
    /// the CLI has not grown a SECOND retrieval path beside the shared one.
    ///
    /// MUTATION: call `ix.retrieve_with_embed` directly from `style_query_cmd`
    /// and the single-call-site assertion fails.
    #[test]
    fn style_query_uses_the_pipeline_retrieval_path() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let main = std::fs::read_to_string(root.join("src/main.rs")).expect("main source");
        assert_eq!(
            main.matches("pipeline::retrieve_style(").count(),
            1,
            "style-query must reach retrieval through the shared helper, once"
        );
        assert_eq!(
            main.matches(".retrieve_with_embed(").count(),
            0,
            "…and never around it"
        );
        // The PRODUCTION half only: this test spells the patterns it counts, so
        // reading its own module would count them twice (it did).
        // Line endings follow the checkout (LF on CI, CRLF under
        // autocrlf), so normalize before matching or the wrapped-call
        // pattern below is Windows-only.
        let pipeline = std::fs::read_to_string(root.join("src/pipeline.rs"))
            .expect("pipeline source")
            .replace("\r\n", "\n")
            .split("mod tests {")
            .next()
            .expect("pipeline production half")
            .to_string();
        assert!(
            pipeline.contains("let (ex, looks) =\n            retrieve_style(")
                || pipeline.contains("let (ex, looks) = retrieve_style("),
            "develop must use the shared helper"
        );
        // The helper itself is the only door to the index's scorers, on both
        // sides of the feature flag.
        assert_eq!(
            pipeline.matches("ix.retrieve_with_embed(").count()
                + pipeline.matches("ix.retrieve_looks(").count(),
            2,
            "retrieve_style is the single place the index is asked to rank"
        );
    }
}
