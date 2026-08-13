//! Shared pipeline core used by both the CLI (`main.rs`) and the web UI
//! (`serve.rs`): run the advise chain for one RAW and write its outputs to the
//! right place. Keeping this in one module means the CLI and the server can
//! never drift apart.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::advisor::{
    Advisor, ClaudeProvider, Decision, HeuristicProposer, OpenAiProvider, OpenAiVerifier, Preview,
    Verdict,
};
use crate::config::Config;
use crate::decode;
use crate::recipe::EditRecipe;
use crate::xmp;

/// Run the full advise chain for one RAW: decode → propose (GPT or heuristic
/// fallback) → Claude verify → optional one revision round. `verbose` prints the
/// proposer/verifier lines (CLI uses true, the server uses false).
/// Run the advise chain for one RAW. `guidance` is an optional user direction
/// (a prompt steering the edit, e.g. "warmer and moodier") woven into the GPT
/// prompt.
pub fn produce_recipe(
    raw: &Path,
    cfg: &Config,
    verbose: bool,
    guidance: Option<&str>,
    base: Option<&EditRecipe>,
    style_strength: f32,
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
             AUTOSHOP_ANALYSIS_API_KEY), or switch the analysis provider to oauth"
        );
    }
    // decode_any: a camera RAW, or an already-baked PNG/TIFF/JPEG (PNG-source mode).
    let decoded = decode::decode_any(raw)?;

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

    let preview_img = decoded.preview_resized(1568);
    let mut jpeg = Vec::new();
    preview_img
        .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .context("encode preview JPEG for advisor")?;
    let preview = Preview { jpeg };
    // The full-resolution preview buffer is DEAD from here on — only meta and
    // histogram feed the advise chain, which can stall on the network for
    // minutes. Keeping `decoded` whole pinned hundreds of MB of 61MP pixels
    // for that entire window.
    let decode::Decoded { meta, histogram, .. } = decoded;

    // Style influence: retrieve the user's edits on the most SIMILAR past shots
    // (needs `autoshop style-index`). style_strength == 0 disables it entirely;
    // otherwise we inject a soft text reference AND, at higher strength, gently
    // pull the FINAL recipe toward those historical means (the blend below).
    // Central store first; the legacy cwd-relative file keeps an index built
    // before the store existed working unchanged.
    let mut style_err: Option<String> = None;
    let style_ix = if style_strength > 0.0 {
        let central = crate::store::style_index_path();
        match crate::style::StyleIndex::load(&central).or_else(|e| {
            crate::style::StyleIndex::load(std::path::Path::new("out/style-index.json"))
                // Keep the CENTRAL error — it carries the version-gate
                // message naming the rebuild command.
                .map_err(|_| e)
        }) {
            Ok(ix) => Some(ix),
            Err(e) => {
                // Surfaced ONCE on stderr, and only when an index file
                // exists: the old `.ok()` swallowed the version-gate message,
                // so a stale index silently disabled the style reference with
                // nothing to say why. (No file at all is the normal
                // fresh-install case — no noise there.) The windowed GUI has
                // no console, so the same fact also rides the rationale
                // below (L08 disclosure threading).
                if central.exists() {
                    static ONCE: std::sync::Once = std::sync::Once::new();
                    ONCE.call_once(|| {
                        eprintln!(
                            "⚠ style reference unavailable ({e:#}) — the Style slider has \
                             no effect until the index is rebuilt"
                        );
                    });
                    style_err = Some(format!("{e:#}"));
                }
                None
            }
        }
    } else {
        None
    };
    let style = style_ix.map(|ix| {
        let ex = ix.retrieve(&meta, &histogram, 4, raw);
        (ix.render_reference(&ex), crate::style::style_targets(&ex))
    });
    let reference: Option<String> = style.as_ref().and_then(|(r, _)| r.clone());
    let ref_str = reference.as_deref();
    if verbose && ref_str.is_some() {
        println!("style    : reference from similar past edits (strength {:.0}%)", style_strength * 100.0);
    }
    if verbose
        && let Some(g) = guidance {
            println!("direction: {g}");
        }

    let (meta, hist) = (&meta, &histogram);

    // GPT vision when a key is set; on failure (quota/network) warn and fall back
    // to the heuristic so we still produce a recipe (disclosure, not masking).
    let openai = OpenAiProvider::new(cfg);
    let mut det_notes: Vec<crate::rationale::Note> = Vec::new();
    let (mut recipe, can_revise) = if cfg.openai_api_key.is_some() {
        if verbose {
            println!("proposer : OpenAI ({})", cfg.openai_model);
        }
        match openai.propose(&preview, meta, hist, ref_str, guidance, None) {
            Ok(r) => (r, true),
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
                eprintln!("⚠ GPT proposer failed ({e})\n  → falling back to the heuristic baseline.");
                // Hand the REAL cause to the heuristic: this stderr line is
                // invisible in the windowed GUI, so the recipe's rationale is
                // the only place the user can learn why the AI didn't run.
                let heuristic = HeuristicProposer { fallback_reason: Some(e.to_string()) };
                let (r, note) = heuristic.propose_noted(hist)?;
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
        let (r, note) = heuristic.propose_noted(hist)?;
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
    let mut verdict = verifier.verify(&recipe, meta, hist)?;

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
        let revised = match openai.propose(&preview, meta, hist, ref_str, guidance, Some(&hint)) {
            Ok(r) => r,
            Err(e) => {
                crate::rationale::push_note(
                    &mut recipe.rationale,
                    &mut det_notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::REVISION_FAILED,
                        vec![("round", round.to_string()), ("e", e.to_string())],
                    ),
                );
                break;
            }
        };
        match verifier.verify(&revised, meta, hist) {
            Ok(v) => {
                recipe = revised;
                // Fresh model prose — the notes described the DISCARDED
                // recipe's tail, so they reset with it (the suffix contract).
                det_notes.clear();
                verdict = v;
            }
            Err(e) => {
                crate::rationale::push_note(
                    &mut recipe.rationale,
                    &mut det_notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::REVISION_VERIFY_FAILED,
                        vec![("round", round.to_string()), ("e", e.to_string())],
                    ),
                );
                break;
            }
        }
    }

    // Distill toward the user's historical style: a gentle, capped pull of the
    // global sliders toward similar past edits. Capped at 60% so even max
    // strength never fully overrides the AI's scene-specific proposal.
    if let Some((_, targets)) = &style {
        let pre_blend = recipe.clone();
        crate::style::blend_toward(&mut recipe, targets, style_strength.clamp(0.0, 1.0) * 0.6);
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
                    vec![("pct", format!("{:.0}", style_strength.clamp(0.0, 1.0) * 0.6 * 100.0))],
                ),
            );
            // Degrade like the revision loop above: a transient verifier
            // failure at this LAST step used to error out the whole call,
            // discarding the paid, already-verified proposal. Keep the pair
            // and disclose which recipe the verdict describes.
            match verifier.verify(&recipe, meta, hist) {
                Ok(v) => verdict = v,
                Err(e) => {
                    crate::rationale::push_note(
                        &mut recipe.rationale,
                        &mut det_notes,
                        crate::rationale::Note::new(
                            crate::rationale::keys::STYLE_REVERIFY_FAILED,
                            vec![("e", e.to_string())],
                        ),
                    );
                }
            }
        }
    }
    // L08: the stderr warning above is invisible in the windowed GUI — the
    // rationale is the one channel all three surfaces show. Every develop
    // that ASKED for style influence and silently got none says so here.
    if let Some(e) = &style_err {
        crate::rationale::push_note(
            &mut recipe.rationale,
            &mut det_notes,
            crate::rationale::Note::new(
                crate::rationale::keys::STYLE_UNAVAILABLE,
                vec![("e", e.clone())],
            ),
        );
    }
    // Base look, stamped in ONE place for every surface: the proposal and the
    // verification above both ran over the camera's embedded preview — the
    // very base the curve approximates — so the AI's JSON round-trip never
    // decides it. A saved recipe.json owns its curve verbatim (a legacy save
    // must keep rendering as it was tuned); otherwise a fresh estimate.
    // Without this, a CLI-written analyze recipe carried an empty curve and
    // the open-time "recipe.json keeps its saved curve" rule then pinned the
    // dark pre-base-look rendering onto that photo forever.
    // ONE read of the saved recipe for BOTH calibration fields: two separate
    // reads could straddle a concurrent writer's retire/publish window (GUI
    // and web are separate processes) and stamp a curve from the saved recipe
    // beside a lens profile from the fresh fallback.
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
            // The as-shot WB anchor is the third calibration half — same
            // saved-first rule (a legacy save keeps None → the 5500 K anchor
            // → byte-identical rendering of its tuned Kelvin).
            recipe.as_shot_k = saved.as_shot_k;
            recipe.as_shot_tint = saved.as_shot_tint;
        }
        None => {
            // A fresh estimate by THIS build's sampler — and the stamp is
            // authored here, never by the model: version is provenance, and
            // the response schema makes the model emit SOMETHING for it.
            recipe.version = crate::recipe::CALIB_ERA;
            recipe.base_curve = photo_base_knots(raw);
            recipe.lens_profile = fresh_lens_profile(raw);
            let (ask, ast) = fresh_as_shot_wb(raw);
            recipe.as_shot_k = ask;
            recipe.as_shot_tint = ast;
        }
    }
    // REFINE means "adjust MY edit", so it must not delete work the model was
    // never able to return. The strict response schema
    // (advisor::openai::edit_recipe_schema) can express only LINEAR and RADIAL
    // primary geometry; it carries no components, enabled toggle, radial angle,
    // colour gains or mask role, and it carries none of the
    // manual lens fields — so a round-trip silently dropped every bitmap mask
    // (AI-selected sky/subject, painted, reverse-fit zones) with its recolour
    // gains, plus any manual lens correction. The result then auto-saves.
    // Carry those back from the base the user actually had.
    if let Some(b) = base {
        carry_over_unrepresentable(&mut recipe, b, Some(&mut det_notes));
    }
    Ok((recipe, verdict, det_notes))
}

/// Re-attach what the AI's response schema CANNOT express, from the refine
/// base the photographer actually had.
///
/// `advisor::openai::edit_recipe_schema` can encode only LINEAR and RADIAL
/// primary geometry; it carries no components, enabled toggle, radial angle,
/// colour gains or mask role, and it carries none of the
/// manual lens fields. A missing bitmap mask in the response therefore
/// carries NO intent — the model had no way to return one — yet the refined
/// recipe auto-saves, so every AI-selected sky/subject mask, painted mask,
/// reverse-fit zone (with its recolour gains) and hand-dialled lens
/// correction silently disappeared the moment the user clicked Refine.
pub(crate) fn carry_over_unrepresentable(
    recipe: &mut EditRecipe,
    base: &EditRecipe,
    notes: Option<&mut Vec<crate::rationale::Note>>,
) {
    use crate::recipe::{MaskGeometry, MaskRole};

    let schema_loses = |m: &crate::recipe::LocalAdjustment| {
        matches!(&m.mask, MaskGeometry::Bitmap { .. })
            || !m.components.is_empty()
            || !m.enabled
            || matches!(&m.mask, MaskGeometry::Radial { angle, .. } if *angle != 0.0)
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
            match (&original.mask, &mut refined.mask) {
                (MaskGeometry::Bitmap { .. }, returned) => {
                    *returned = original.mask.clone();
                }
                (
                    MaskGeometry::Radial { angle: base_angle, .. },
                    MaskGeometry::Radial { angle: returned_angle, .. },
                ) => {
                    *returned_angle = *base_angle;
                }
                (MaskGeometry::Radial { angle, .. }, returned) if *angle != 0.0 => {
                    *returned = original.mask.clone();
                }
                _ => {}
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
    // Manual lens corrections are geometry the photographer dialled in and the
    // model never saw; defaulting them silently re-warped the frame.
    recipe.lens_distortion = base.lens_distortion;
    recipe.lens_vignette = base.lens_vignette;
    recipe.lens_vignette_mid = base.lens_vignette_mid;
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
    recipe.clamp(); // the size caps still apply after re-attaching
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
            if !crate::xmp::xmp_to_recipe(&t).is_noop() =>
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

pub fn write_recipe(raw: &Path, recipe: &EditRecipe, out: Option<PathBuf>) -> Result<PathBuf> {
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
    let bytes = recipe_bytes_for(recipe, out.parent())?;
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
fn recipe_bytes_for(recipe: &EditRecipe, anchor: Option<&std::path::Path>) -> Result<Vec<u8>> {
    let mut on_disk = recipe.clone();
    let dropped = on_disk.clamp();
    if !dropped.is_empty() {
        // describe(): only the non-zero losses — curve/string truncation was
        // invisible behind a "0 mask(s)" line (16-lane scan L16).
        eprintln!("warning: recipe limits discarded {}", dropped.describe());
    }
    if let Some(parent) = anchor {
        crate::store::relativize_mask_paths(&mut on_disk, parent);
    }
    Ok(serde_json::to_vec_pretty(&on_disk)?)
}

/// The CENTRAL-STORE recipe bytes for `raw` — identical to what a plain
/// [`write_recipe`] publishes — for staging into a
/// [`crate::store::commit_develop`] single-generation save.
pub fn recipe_store_bytes(raw: &Path, recipe: &EditRecipe) -> Result<Vec<u8>> {
    let target = crate::store::recipe_target(raw);
    recipe_bytes_for(recipe, target.parent())
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

/// Write the develop's XMP projection: the published path PLUS the note when
/// the merge could not be performed and the sidecar was REGENERATED instead —
/// see [`write_xmp_doc`] for why that is a loss worth telling the user about.
/// Every caller receives the note (round-12 disclosure threading): the old
/// note-dropping `write_xmp` wrapper was how five of seven surfaces stayed
/// silent. stderr already hears every note via `write_xmp_doc`; UI surfaces
/// route what they are handed here.
pub fn write_xmp(raw: &Path, recipe: &EditRecipe) -> Result<(PathBuf, Option<String>)> {
    use crate::store::SidecarRead;
    let target = xmp_target(raw);
    // MERGE, never regenerate over Lightroom's work (A11): the base is the
    // sidecar Lightroom itself writes (beside the RAW) when one exists — its
    // LR-only properties (global Texture, camera profile / Look, LR
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
                 merged against Autoshop's own previous version instead — anything changed \
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
    write_xmp_doc(target, recipe, base, notes)
}

/// Write the XMP to an EXPLICIT path. Used when the recipe was redirected with
/// `-o`: the two halves of one develop must stay in the same folder, or the
/// GUI/web would keep restoring an older `out/<stem>.xmp` instead.
pub fn write_xmp_at(out: PathBuf, recipe: &EditRecipe) -> Result<(PathBuf, Option<String>)> {
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
    write_xmp_doc(out, recipe, base, notes)
}

/// Returns the published path and, when the merge could not run, a note naming
/// what that cost.
///
/// A base the splicer cannot account for falls back to a FRESH document rather
/// than failing the save — but that fallback DROPS exactly what the merge
/// exists to protect: `crs:Texture`, the camera profile / creative `Look`,
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
    mut notes: Vec<String>,
) -> Result<(PathBuf, Option<String>)> {
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
    let merged = merge_base.and_then(|(_, b)| xmp::merge_recipe_into_xmp(&b, recipe));
    // A merge that SUCCEEDED can still have losses it could not avoid (the
    // base's unrepresentable mask block giving way to the recipe's own
    // masks) — its notes ride the same channel as the regeneration note.
    let merged = merged.map(|mut o| {
        notes.append(&mut o.notes);
        o.doc
    });
    if merged.is_none()
        && let Some(bp) = base_path
    {
        // The note names the file whose properties are LOST — the base — and
        // is honest about what happened to it: a base at the output path is
        // genuinely replaced by the regeneration; a base beside the RAW is
        // not touched at all, only unrepresented in the new file. "Texture,
        // camera profile / Look, LR lens-profile data" are EXAMPLES of what a
        // Lightroom base carries, not a claim about this one — a ratings-only
        // sidecar loses its ratings the same way.
        notes.push(if bp == out {
            format!(
                "the existing sidecar at {} could not be merged — it was regenerated, \
                 so properties it carried (e.g. Lightroom's Texture, camera profile / \
                 Look, LR lens-profile data) are not in the new file",
                out.display()
            )
        } else {
            format!(
                "the sidecar at {} could not be merged — the new file at {} was \
                 regenerated without the properties it carried (e.g. Lightroom's \
                 Texture, camera profile / Look, LR lens-profile data, ratings); \
                 the sidecar itself is untouched",
                bp.display(),
                out.display()
            )
        });
    }
    let note = (!notes.is_empty()).then(|| {
        let msg = notes.join("; ");
        eprintln!("⚠ {msg}");
        msg
    });
    let doc = merged.unwrap_or_else(|| xmp::recipe_to_xmp(recipe));
    // Stage + rename, never truncate in place: `fs::write` opens the LIVE
    // sidecar with O_TRUNC, so a full disk, an interruption or a competing
    // writer left a truncated file where a valid Lightroom sidecar used to
    // be — the previous projection destroyed by the failed attempt to
    // replace it. (fs::rename replaces the destination on every platform.)
    crate::store::durable_write(&out, doc.as_bytes())
        .with_context(|| format!("publish xmp {}", out.display()))?;
    Ok((out, note))
}

/// Where the .xmp for `raw` goes — the photo's central develop dir (see
/// `store`; the photo library itself stays read-only). Kept here because every
/// surface already imports it from pipeline.
pub fn xmp_target(raw: &Path) -> PathBuf {
    crate::store::xmp_target(raw)
}

/// Guarantee the read-only library: refuse to write `out` if it lands inside the
/// source RAW's own folder (or below it). Outputs belong in ./out (exports) or
/// the central develop store (sidecars).
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
    if let Ok(own_out) = absolute(Path::new("out"))
        && out_abs.starts_with(resolve_existing(&normalize(&own_out))) {
            return Ok(());
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
        carry_over_unrepresentable(&mut proposed, &base, None);
        assert_eq!(proposed.masks.len(), 2, "the bitmap mask must survive Refine");
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
        carry_over_unrepresentable(&mut stamped, &EditRecipe::default(), None);
        assert!(stamped.lens_profile.vignette_on, "sentinel base leaves the stamp alone");
        assert!(!stamped.lens_profile.vignette.is_empty());
        // The model's own proposal is still there, after the carried one.
        assert!(matches!(proposed.masks[1].mask, MaskGeometry::Linear { .. }));
    }

    #[test]
    fn guard_refuses_to_overwrite_a_photo_in_a_sibling_library_folder() {
        let base = std::env::temp_dir().join("autoshop-guard-sibling");
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
        let dir = std::env::temp_dir().join("autoshop-pipe-xmp-merge");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_pipe_xmp_merge.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        // Lightroom's sidecar beside the RAW, carrying a property we do not
        // model (Texture) — the exact thing regeneration used to destroy.
        let lr = dir.join("_pipe_xmp_merge.xmp");
        std::fs::write(
            &lr,
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
             <rdf:Description rdf:about=\"\"\n    \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    \
             crs:Texture=\"+21\"\n    crs:Exposure2012=\"+1.00\"\n    \
             crs:HasSettings=\"True\">\n  </rdf:Description>\n \
             </rdf:RDF>\n</x:xmpmeta>\n",
        )
        .unwrap();
        let r = EditRecipe { exposure_ev: 0.75, ..Default::default() };
        let (out, _) = write_xmp(&raw, &r).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("crs:Texture=\"+21\""), "LR-only property survives the save");
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
        let dir = std::env::temp_dir().join("autoshop-pipe-xmp-maskintent");
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
             crs:Texture=\"+21\" crs:HasSettings=\"True\">\n   \
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
            },
            exposure_ev: 0.4,
            ..Default::default()
        });
        let (out, note) = write_xmp(&raw, &r).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("Mask/CircularGradient"), "the develop's mask is published");
        assert!(!text.contains("Mask/Brush"), "the foreign block is not resurrected");
        assert!(text.contains("crs:Texture=\"+21\""), "LR-only globals still survive");
        let note = note.expect("the replaced block must be disclosed");
        assert!(note.contains("mask correction(s)"), "the note names the loss: {note}");
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
        let dir = std::env::temp_dir().join("autoshop-pipe-recipe-caps");
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
                    },
                    exposure_ev: 1.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let out = write_recipe(&raw, &hostile, None).unwrap();
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
        let dir = std::env::temp_dir().join("autoshop-pipe-xmp-disclose");
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
             crs:Texture=\"+21\"></rdf:Description>\n",
        )
        .unwrap();
        let (out, note) = write_xmp(&raw, &r).unwrap();
        assert!(note.is_none(), "a merge that worked has nothing to disclose: {note:?}");
        assert!(std::fs::read_to_string(&out).unwrap().contains("crs:Texture=\"+21\""));

        // An UNTERMINATED description is markup the splicer cannot account
        // for: the save still succeeds, the Texture is gone, and the caller is
        // told exactly that.
        std::fs::write(
            &lr,
            "<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:Texture=\"+21\">\n",
        )
        .unwrap();
        let (out, note) = write_xmp(&raw, &r).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("crs:Exposure2012=\"0.75\""), "the save still happened");
        assert!(!text.contains("crs:Texture"), "…by regenerating, so the LR property is gone");
        let note = note.expect("the loss must be disclosed, not silent");
        assert!(note.contains("regenerated"), "the note names what happened: {note}");
        assert!(note.contains("Texture"), "…and what it cost: {note}");

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
        let dir = std::env::temp_dir().join("autoshop-pipe-xmp-truthful-note");
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
        let (_, note) = write_xmp(&raw, &r).unwrap();
        assert!(note.is_none(), "an empty sidecar carries nothing to disclose: {note:?}");
        // Whitespace-only is the same emptiness.
        std::fs::write(dir.join("_pipe_note_blank.xmp"), b"  \n\t\n").unwrap();
        let (_, note) = write_xmp(&raw, &r).unwrap();
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
            "<rdf:Description rdf:about=\"\" xmlns:crs=\"c\" crs:Texture=\"+21\">\n",
        )
        .unwrap();
        let (_, note) = write_xmp(&png, &r).unwrap();
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
        let (out2, note) = write_xmp(&raw2, &r).unwrap();
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
        let (out2b, note) = write_xmp(&raw2, &r).unwrap();
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
            let (out4, _) = write_xmp(&raw4, &r).unwrap();
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
             crs:Texture=\"+21\">\n",
        )
        .unwrap();
        let (_, note) = write_xmp(&raw3, &r).unwrap();
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
        let dir = std::env::temp_dir().join("autoshop-pipe-xmp-oversized");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("_pipe_oversized.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        let lr = dir.join("_pipe_oversized.xmp");

        // First save: a mergeable Lightroom base whose Texture lands in the
        // projection — the carried-forward property the second save must keep.
        std::fs::write(
            &lr,
            "<rdf:Description rdf:about=\"\" \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:Texture=\"+21\"></rdf:Description>\n",
        )
        .unwrap();
        let r1 = EditRecipe { exposure_ev: 0.5, ..Default::default() };
        let (_, note) = write_xmp(&raw, &r1).unwrap();
        assert!(note.is_none(), "the mergeable base has nothing to disclose: {note:?}");

        // The sidecar balloons past the cap (Lightroom AI masks, or hostile).
        let mut big = String::with_capacity(16 * 1024 * 1024 + 64);
        big.push_str("<x:xmpmeta>");
        while big.len() <= 16 * 1024 * 1024 {
            big.push_str("<!-- pad -->");
        }
        std::fs::write(&lr, &big).unwrap();

        let r2 = EditRecipe { exposure_ev: 0.75, ..Default::default() };
        let (out, note) = write_xmp(&raw, &r2).unwrap();
        // The fallback itself is CORRECT — the previous projection carries
        // forward what the first merge preserved, and the save must succeed.
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("crs:Exposure2012=\"0.75\""), "the save still happened");
        assert!(text.contains("crs:Texture=\"+21\""), "the projection base carried forward");
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
}

/// `./out/<stem>.<kind>.<ext>` — outputs never go beside the source RAW.
pub fn default_out(raw: &Path, kind: &str, ext: &str) -> PathBuf {
    PathBuf::from("out").join(format!("{}.{kind}.{ext}", stem(raw)))
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
}

impl BatchNames {
    /// The deliverable path for `raw`, unique within this batch.
    pub fn claim(&mut self, raw: &Path, kind: &str, ext: &str) -> PathBuf {
        let mut out = default_out(raw, kind, ext);
        let mut n = 1u32;
        while !self.taken.insert(out.to_string_lossy().to_lowercase()) {
            n += 1;
            out = PathBuf::from("out").join(format!("{} ({n}).{kind}.{ext}", stem(raw)));
        }
        if n > 1 {
            self.renamed.push(format!("{} ← {}", out.display(), raw.display()));
        }
        out
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
/// the analysis / image call had been billed — `autoshop analyze x.arw -o
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
            ".autoshop-write-probe.{}.{}",
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

/// Does this path name a photo — a camera RAW or an already-baked raster?
/// Shared by the gallery scan and by [`guard_readonly`]'s never-clobber rule.
pub fn is_source(p: &Path) -> bool {
    crate::decode::is_raw(p)
        || p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
            matches!(x.to_ascii_lowercase().as_str(), "png" | "tif" | "tiff" | "jpg" | "jpeg")
        })
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

    /// L13#4: calibration comes from the NEWEST intent. A Lightroom sidecar
    /// that out-ranks the store vetoes the stored recipe's calibration
    /// (fresh stamp, like both restore surfaces); an older or neutral one
    /// leaves the stored develop in charge.
    #[test]
    fn saved_calibration_resolves_by_newest_intent() {
        let dir = std::env::temp_dir().join("autoshop-pipeline-test-lr-calib");
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
        // A RAW living in the (read-only) photo library.
        let raw = Path::new("D:/Photography/Raw/2024/Trip/DSC0001.ARW");
        // Writing a sibling INTO that folder must be refused.
        let sibling = Path::new("D:/Photography/Raw/2024/Trip/DSC0001.developed.tif");
        assert!(guard_readonly(sibling, raw).is_err(), "must refuse a sibling write");
        // A subfolder under the RAW's folder is refused too.
        let under = Path::new("D:/Photography/Raw/2024/Trip/out/DSC0001.tif");
        assert!(guard_readonly(under, raw).is_err(), "must refuse a subfolder write");
        // The default ./out (outside the library) is allowed.
        let safe = default_out(raw, "developed", "tif");
        assert!(guard_readonly(&safe, raw).is_ok(), "./out must be allowed");
        // A source that itself lives in OUR ./out (e.g. `match` on an exported
        // preview) may be written beside — the guard protects the library only.
        let out_src = Path::new("out/DSC0001.preview.jpg");
        let out_dst = Path::new("out/DSC0001.matched.json");
        assert!(guard_readonly(out_dst, out_src).is_ok(), "our ./out is always writable");
        // A `..` bumping against the ROOT must fold like the filesystem does
        // ("D:/../Photography" = "D:/Photography") — popping the root used to
        // produce the drive-relative "D:Photography", which dodged every
        // starts_with check and slipped a library write past the guard.
        let root_dodge = Path::new("D:/../Photography/Raw/2024/Trip/DSC0001.x.tif");
        assert!(
            guard_readonly(root_dodge, raw).is_err(),
            "a root-level .. must not bypass the library guard"
        );
    }

    /// find_raws must accept EVERY format decode::is_raw does (one definition of
    /// "a RAW" app-wide) and nothing else — a Canon/Nikon library used to scan
    /// as empty for batch/eval/style-index.
    #[test]
    fn find_raws_accepts_every_raw_format_the_app_can_decode() {
        let dir = std::env::temp_dir().join(format!("autoshop_find_raws_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).expect("temp dir");
        let raws = ["a.ARW", "b.dng", "c.NEF", "d.cr2", "e.cr3", "f.orf", "g.rw2", "h.raw"];
        for name in raws {
            std::fs::write(dir.join(name), b"").expect("write");
        }
        std::fs::write(dir.join("sub").join("i.raf"), b"").expect("write"); // recursion
        for name in ["note.txt", "baked.png", "export.jpg", "DSC0001.xmp"] {
            std::fs::write(dir.join(name), b"").expect("write");
        }

        let found = find_raws(&dir).expect("scan");
        let mut names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_lowercase())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["a.arw", "b.dng", "c.nef", "d.cr2", "e.cr3", "f.orf", "g.rw2", "h.raw", "i.raf"],
            "find_raws must see every RAW format (case-insensitive, recursive) and no baked/sidecar files"
        );
        assert!(found.iter().all(|p| decode::is_raw(p)), "one RAW predicate, app-wide");
        let _ = std::fs::remove_dir_all(&dir);
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
        let dir = std::env::temp_dir()
            .join(format!("autoshop-scan-cycle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
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
        let dir = std::env::temp_dir().join("autoshop-scan-nolink");
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
            .join(format!("autoshop-pipeline-test-rsc-funnel-{}", std::process::id()));
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

    #[test]
    fn outputs_always_default_outside_the_library() {
        let raw = Path::new("D:/Photography/Raw/2024/Trip/DSC0001.ARW");
        // Exports (deliverable images) stay in ./out; develop STATE (recipe +
        // XMP sidecars) lives in the photo's central develop dir, keyed by the
        // absolute path. Neither ever lands beside the RAW — the library stays
        // read-only by construction.
        assert!(default_out(raw, "developed", "tif").starts_with("out"));
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
        carry_over_unrepresentable(&mut proposed, &base, None);
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
        carry_over_unrepresentable(&mut refined, &plain_base, None);
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
        carry_over_unrepresentable(&mut proposed, &base, None);
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
        let s_thumb = src.thumbnail(crate::fit::ANALYZE_EDGE, crate::fit::ANALYZE_EDGE);
        let t_thumb = target.thumbnail(crate::fit::ANALYZE_EDGE, crate::fit::ANALYZE_EDGE);
        let canvas_err = crate::fit::look_err(
            &crate::fit::pixels_of(&crate::render::develop_preview(&s_thumb, &rep.recipe)),
            &crate::fit::pixels_of(&t_thumb),
        );
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

    /// R16 real-pair harness (ignored): set AUTOSHOP_FIT_REPRO_RAW and
    /// AUTOSHOP_FIT_REPRO_TARGET to a photo and a rendition of it, then run
    /// with `-- --ignored r16 --nocapture`. Prints the OLD neutral-source
    /// fit next to the NEW composed-calibration fit. Pure lib calls — the
    /// develop store is read (calibration authority) but never written.
    #[test]
    #[ignore = "real-photo repro: needs AUTOSHOP_FIT_REPRO_RAW/_TARGET"]
    fn r16_composed_fit_on_a_real_pair() {
        let (Ok(raw), Ok(tgt)) = (
            std::env::var("AUTOSHOP_FIT_REPRO_RAW"),
            std::env::var("AUTOSHOP_FIT_REPRO_TARGET"),
        ) else {
            panic!("set AUTOSHOP_FIT_REPRO_RAW and AUTOSHOP_FIT_REPRO_TARGET");
        };
        let raw = std::path::PathBuf::from(raw);
        let neutral =
            crate::render::render_to_image(&raw, &EditRecipe::default(), None, Some(1280))
                .expect("neutral develop");
        let target =
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
        assert!(
            new.err_after <= old.err_after + 1e-3,
            "seeding must not regress the fit ({:.4} vs {:.4})",
            new.err_after,
            old.err_after
        );
    }

    /// L09#1: the pre-pay output preflight — a directory target refuses
    /// with a message naming it (the case that used to bill the analysis
    /// first and bail at write_recipe after).
    #[test]
    fn preflight_out_refuses_a_directory_target() {
        let root =
            std::env::temp_dir().join(format!("autoshop-preflight-dir-{}", std::process::id()));
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
            std::env::temp_dir().join(format!("autoshop-preflight-probe-{}", std::process::id()));
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
            style_strength: 0.5,
        };
        let e = produce_recipe(
            Path::new("this-file-does-not-exist.arw"),
            &cfg,
            false,
            None,
            None,
            0.0,
        )
        .expect_err("no analysis key + api provider must refuse up front")
        .to_string();
        assert!(
            e.contains("analysis") && e.contains("AFTER the paid proposal"),
            "the refusal names the reason, not a decode error: {e}"
        );
    }

    /// L09#1: a missing parent is created up-front (the documented
    /// trade-off: an empty dir if the paid call then fails beats burning
    /// the call), and a parent that is a FILE refuses — the exact failure
    /// that used to land after payment.
    #[test]
    fn preflight_out_creates_a_missing_parent_and_refuses_a_file_parent() {
        let root =
            std::env::temp_dir().join(format!("autoshop-preflight-par-{}", std::process::id()));
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
}
