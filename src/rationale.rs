//! Deterministic-rationale NOTES (L12#2B): the same fact carried twice —
//! once as the persisted English string (recipe.json, the XMP comment, the
//! `X-Heal-Rationale` header, the CLI printout and the verifier prompt are
//! all English by contract and must stay BYTE-STABLE), and once as typed
//! `(key, args)` pairs riding ALONGSIDE on in-process results only, so the
//! GUI can render them in the session language at draw time.
//!
//! NOT a field on `EditRecipe`: recipe.json is `deny_unknown_fields`
//! (a new field breaks downgrade compatibility) and the whole struct is
//! serialized into the verifier's LLM prompt (tokens + a wider untrusted
//! surface). The notes travel on `FitReport` / `HealReport` / the
//! `produce_recipe` return value instead — never to disk, never to a model.
//!
//! CONTRACT with consumers: the notes render, in order, the rationale
//! string's SUFFIX (`render_en(&notes)` == the tail of the rationale; the
//! prefix, possibly empty, is the AI model's own prose). A consumer strips
//! that suffix to recover the prose and renders the notes localized; when
//! the strip does not match (a byte cap truncated the string, an old build
//! produced it), the whole raw English string is shown instead — silent
//! English is the sanctioned fallback (user decision, 2026-08-11), never a
//! wrong or partial translation.

/// One deterministic rationale fragment: a catalogue key (its English
/// template, byte-for-byte what the persisted string carries) plus the
/// pre-formatted argument values.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub key: &'static str,
    pub args: Vec<(&'static str, String)>,
}

/// Bound on notes riding one result: the RENDERED string is capped
/// elsewhere (MAX_RATIONALE), but the vec itself must not grow without
/// limit if a producer loops. Past the bound, [`push_note`] plants ONE
/// [`TRUNCATED_SENTINEL`] note whose rendering no rationale string carries,
/// so the consumer's suffix strip MISSES and the whole raw English shows —
/// never a partial translation. (Without the pill, 64 retained copies of a
/// REPEATED note could still match the string's tail and localize a
/// truncated subset while the overflow rode as fake "prose" — Codex AL F7.)
pub const MAX_NOTES: usize = 64;

/// Renders to text no rationale string contains (a NUL never enters the
/// templates), so a truncated vec can never strip-match its string.
pub const TRUNCATED_SENTINEL: &str = "\u{0}notes-truncated";

impl Note {
    pub fn new(key: &'static str, args: Vec<(&'static str, String)>) -> Self {
        Self { key, args }
    }

    pub fn plain(key: &'static str) -> Self {
        Self { key, args: Vec::new() }
    }
}

/// Every note template, byte-for-byte the English the persisted rationale
/// carries (leading separators/spaces included — notes concatenate with NO
/// joiner). In ONE module so the i18n audit extracts the full key set
/// mechanically; a new note key without a zh pair fails the gate.
pub mod keys {
    // --- reverse fit (fit.rs) -------------------------------------------
    pub const FIT_SUMMARY_WITH_CURVE: &str =
        "Reverse-fit from a target rendition (statistical match; the target is not \
         pixel-aligned, so local masks and per-band HSL are not recovered): luma-CDF \
         → tone sliders + residual tone curve, chroma → saturation, per-channel cast \
         curves. Residual look error {err_before} → {err_after}.";
    pub const FIT_SUMMARY_NO_CURVE: &str =
        "Reverse-fit from a target rendition (statistical match; the target is not \
         pixel-aligned, so local masks and per-band HSL are not recovered): luma-CDF \
         → tone sliders (no residual curve), chroma → saturation, per-channel cast \
         curves. Residual look error {err_before} → {err_after}.";
    pub const FIT_NOTE_FAR: &str =
        " NOTE: the fitted recipe still renders far from the target \
         (residual {err_after}) — this look exceeds what global \
         sliders can express; consider the AI variant itself or a zoned \
         edit.";
    pub const FIT_NOTE_SAT_PEGGED: &str = " Saturation demand exceeded the model cap (±60).";
    pub const FIT_DEGENERATE: &str = " Fit refused: the source or target frame has no tonal variation (blank or single-tone), so a statistical match would produce a constant tone map — no recipe was fitted.";
    pub const FIT_NOTE_REGRESSED: &str =
        " The full fit rendered farther from the target than the untouched \
         source at every saturation level — returning a NEUTRAL recipe \
         (do-no-harm terminal case); this look is outside the global \
         model's reach.";
    pub const FIT_NOTE_SAT_REDUCED: &str =
        " Saturation was pulled back from the chroma-matched {sat_fitted} \
         to {sat_now} after the full-strength fit rendered farther from the \
         target than the untouched source (do-no-harm check).";
    pub const FIT_NOTE_REHUE_BLOCKED: &str =
        " Colour-cast curves were withheld: they would have re-hued a \
         region of the frame (pixel-aligned hue-damage gates).";
    /// R23-6 A-2: the colour stage's SILENT rejection arm. `ratio_fail`
    /// empties all three channel curves and used to push no note at all, so
    /// "the colour stage produced nothing" — its commonest outcome — reached
    /// the user as an unexplained absence.
    pub const FIT_NOTE_CAST_REJECTED: &str =
        " The colour stage produced NOTHING: the per-channel cast curves it \
         fitted did not improve the look by a clear enough margin to earn \
         the risk of dragging every region, so they were rejected and this \
         fit carries tone and saturation only.";
    /// R23-6 C: the second reading, independent of the look error above.
    pub const FIT_NOTE_JOINT: &str =
        " Joint distribution check (luminance × chroma value ranges, {n} of \
         them carried evidence on both sides): area-weighted mismatch \
         {weighted}, worst range {worst} in {label}. These are VALUE ranges, \
         not areas of the picture — their pixels are spread over the whole \
         frame.";
    /// The fail-open case, said out loud. When the joint family finds no
    /// value range with evidence on both sides it has NO opinion — and the
    /// confidence above is then the look-error ladder alone, which is the
    /// self-graded number this round exists to stop presenting as a verdict.
    /// Silence here would make two different claims look identical.
    pub const FIT_NOTE_JOINT_NONE: &str =
        " (the joint distribution check found no value range with enough \
         evidence on both sides, so it has no opinion on this pair — the \
         confidence above rests on the single residual number alone)";
    pub const FIT_NOTE_JOINT_FAR: &str =
        " That joint mismatch is large: the two images still differ inside \
         matching value ranges, which the single residual number above \
         cannot show — treat this fit as a starting point, not a match.";
    pub const FIT_NOTE_JOINT_REGRESSED: &str =
        " (the refusal came from the joint distribution check, not the \
         residual: the fitted recipe pushed the value ranges further apart \
         than leaving the photo alone)";
    /// R23-6 A-5: which controls THIS pair seems to need that the solver
    /// never assigns.
    pub const FIT_NOTE_UNREPRESENTED: &str =
        " This target's look appears to use {controls}, which the reverse-fit \
         never solves for (its whole solution space is exposure/contrast/\
         highlights/shadows/whites/blacks, a tone curve, one global \
         saturation and the three channel curves) — that part of the look \
         cannot arrive through this route.";
    /// R23-6 D: the deep reverse-fit adopted a guided retry. The persisted
    /// record has to say the shipped recipe is not the plain solve — the
    /// status line is transient, the rationale is what reopening the photo
    /// shows.
    pub const FIT_NOTE_DEEP_ADOPTED: &str =
        " Deep reverse-fit: the visual reviewer scored the plain solve \
         {score1}/100, so one guided retry was bought ({action}); it \
         re-scored {score2}/100 and was kept (a lower score would have been \
         discarded).";
    /// R23-6 B-7: the reference may not be the same frame.
    pub const FIT_NOTE_NOT_SAME_FRAME: &str =
        " WARNING: the reference does not look like the same frame as this \
         photo (their proportions disagree). The fit matched distributions \
         anyway, as it was asked to, but a match between two different \
         pictures describes neither — treat the result as unreliable.";

    // --- zoned fit (fit_zoned.rs) ---------------------------------------
    pub const ZONED_UNAVAILABLE: &str = " Zoned sky fit unavailable ({e}) — global fit only.";
    pub const ZONED_NO_PARTITION: &str =
        " Zoned fit skipped: no usable sky partition (sky covers {s}% \
         of the source frame, {t}% of the target's).";
    pub const ZONE_TOO_SMALL: &str =
        " The {label} zone covers too little of the frame (source {s}%, \
         target {t}%) — skipped.";
    pub const ZONE_ATTACHED: &str =
        " Zoned {label} correction attached ({label}-to-{label} moments → \
         local exposure {ev} EV, colour gains [{g0} {g1} {g2}], \
         saturation {sat}): zone residual {before} → {after}. The correction \
         is a BITMAP mask — rendered in-app; the Lightroom sidecar \
         carries the global fit only (classic XMP cannot hold raster \
         masks).";
    pub const ZONE_SHARE_MISMATCH: &str =
        " Note: the {label} zone covers {s}% of the source frame \
         but {t}% of the target's — the compositions differ, so the \
         overall distribution residual stays where the global fit \
         left it.";
    pub const ZONE_DROPPED: &str =
        " Zoned {label} correction dropped: zone residual {before} → {after} \
         (needs ≤ {ratio}% of the original, or ≤ {floor} with a ≥ {gain}% \
         gain) with frame-global drift {drift} (tolerance {tol}).";
    pub const ZONE_ALREADY_MATCHED: &str =
        " The {label} zone already matches the target (zone residual \
         {before}) — no correction needed.";
    /// R23-6 A-4: what the reported confidence of a ZONED fit now means.
    /// Before this round the zone stage overwrote both `err_after` and
    /// `confidence` with the frame-global look error of the zoned render —
    /// the very number this module's own acceptance doc proves cannot judge
    /// a zone (0.507 → 0.015 zone-local while the frame moved 0.1768 →
    /// 0.1792). The frame number is still reported, because it is what
    /// `err_before` is comparable to; it is simply no longer the verdict.
    pub const ZONE_CONFIDENCE: &str =
        " Confidence for this fit comes from the {n} zone correction(s) that \
         were actually accepted (worst zone residual {worst}), not from the \
         frame-wide residual {frame} — a frame-wide distribution cannot \
         judge a zone whose share of the two frames differs.";

    // --- the propose/verify pipeline (pipeline.rs) ----------------------
    pub const REVISION_FAILED: &str =
        " [revision round {round} failed ({e}) — keeping the previous verified proposal]";
    pub const REVISION_VERIFY_FAILED: &str =
        " [verification of revision round {round} failed ({e}) — keeping the previous \
         verified proposal]";
    pub const STYLE_DISTILLED: &str =
        " [style distillation then pulled the global sliders toward this user's past \
         edits (effective strength {pct}%) — final values can differ from the \
         derivation above]";
    pub const STYLE_REVERIFY_FAILED: &str =
        " [re-verification after style distillation failed ({e}) — the verdict \
         above describes the PRE-distillation recipe]";
    pub const STYLE_UNAVAILABLE: &str =
        " [style reference unavailable ({e}) — the Style slider had no effect on this \
         develop; rebuild it with: autoshop style-index <folder>]";
    /// R23-2, feedback #6: the OTHER two silent arms. `STYLE_UNAVAILABLE`
    /// above only ever fired when an index FILE existed and failed to load, so
    /// a fresh install (no library at all) and a retrieval that matched
    /// nothing both produced a Style slider that did nothing, silently, on
    /// every surface. One note covers both — the condition is "asked for
    /// style, ended up with no reference" — and it names the GUI entry point,
    /// because the windowed app is where this is most often read.
    /// (The arrow is `→`, not `›`: this template is RENDERED BY THE GUI, whose
    /// embedded font subset is generated from the GUI sources — a character
    /// only this module uses would draw as a tofu box there.)
    pub const STYLE_NO_REFERENCE: &str =
        " [no style reference was available for this photo — the Style slider ({pct}%) had \
         no effect on this develop. Build your style library in the AI panel → Style \
         reference library: a folder of your own RAWs with their Lightroom .xmp sidecars \
         beside them]";
    /// Which past shots this develop actually leaned on (R23-2 transparency).
    pub const STYLE_NEIGHBOURS: &str =
        " [style reference: your own edits on {files} — the {n} most similar shots in your \
         style library]";
    /// The opt-in reference IMAGE (off by default — it is an extra image on a
    /// paid vision call).
    pub const STYLE_REF_IMAGE: &str =
        " [{file} also went to the vision model as a reference IMAGE — one extra image on \
         each call of this analysis]";
    pub const STYLE_REF_IMAGE_FAILED: &str =
        " [the reference photo could not be prepared ({e}) — this develop used the text \
         reference only]";
    pub const MASKS_NOT_PRESERVED: &str =
        "\n⚠ the response did not preserve mask identities (a mask was renamed or \
         duplicated) — your masks were kept unchanged and the model's mask edits were \
         discarded";

    // --- the vision proposer's own repairs (advisor/openai.rs) -----------
    /// R23-1: OpenAI strict mode cannot bound an array's LENGTH, so a
    /// miscounted `hsl` axis used to fail the whole recipe deserialize and
    /// throw away a paid high-detail vision call. The axis is repaired and
    /// the repair disclosed. (Written by the provider, which has no notes
    /// vec of its own — so this one renders as English in the rationale's
    /// PROSE prefix until `propose` grows a notes channel; the zh pair is
    /// registered so the localized rendering lands the moment it does.)
    pub const HSL_AXIS_LENGTH_REPAIRED: &str =
        " [the proposal's 8-band colour mixer arrived with the wrong number of values \
         ({axes}) — the missing bands were read as neutral 0 and any extra ones dropped, \
         so the rest of the proposal was kept]";

    // --- visual judge closed loop (pipeline.rs, R20) ---------------------
    pub const JUDGE_SCORE: &str =
        " [AI visual review: {score}/100 — {critique}]";
    pub const JUDGE_ADOPTED: &str =
        " [AI visual review: {score1}/100 first; a guided revision re-scored \
         {score2}/100 and was adopted — {critique}]";
    pub const JUDGE_KEPT: &str =
        " [AI visual review: {score1}/100 — {critique}; the guided revision \
         re-scored lower ({score2}/100) and was discarded (do-no-harm)]";
    pub const JUDGE_UNCHANGED: &str =
        " [AI visual review: {score}/100 — {critique}; the guided revision \
         returned the same recipe — keeping it]";
    pub const JUDGE_ROUND_FAILED: &str =
        " [AI visual review: {score}/100 — {critique}; the guided revision \
         round failed ({e}) — keeping the reviewed develop]";
    pub const JUDGE_REJUDGE_FAILED: &str =
        " [AI visual review: {score}/100 — {critique}; the guided revision \
         could not be re-judged ({e}) and was discarded (do-no-harm)]";
    pub const JUDGE_UNAVAILABLE: &str =
        " [AI visual review unavailable ({e}) — the develop was not visually \
         checked]";
    /// R23-4: ONE intermediate round of the multi-round convergence loop. The
    /// terminal round keeps `JUDGE_ADOPTED` (so a single-round analysis — every
    /// default-path one — writes exactly what it wrote before this round), and
    /// each earlier adoption logs here with its number and both scores.
    pub const JUDGE_ROUND: &str =
        " [AI visual review round {round}: {score1}/100 → a guided revision \
         re-scored {score2}/100 and was adopted; still under the {target}/100 \
         target, so another round was bought]";

    // --- deep thinking mode (pipeline.rs, R23-4) -------------------------
    /// The proposer's own reading of the photograph, one sentence. Model prose
    /// rides the arg verbatim (the `{e}`/`{critique}` convention).
    pub const THINK_SCENE: &str = " [deep thinking — what it saw: {scene}]";
    pub const THINK_LOOK: &str = " [deep thinking — the look it aimed for: {look}]";
    pub const THINK_CRITIQUE: &str =
        " [deep thinking — its own critique against your strength target: {critique}]";
    /// R23-1b: the PIXEL tools the model thinks this photo needs — advice
    /// only, and named as such. No develop control can express them, so this
    /// note is the whole channel: the app never runs one on the model's say-so
    /// (several are paid and destructive — R20's "the fee is the explicit
    /// caller's decision"). `{tools}` is our own joined list of enum names +
    /// bounded model clauses.
    pub const PIXEL_TOOLS: &str =
        " [it also suggests the pixel tools (nothing was run — these are for you to \
         choose): {tools}]";

    // --- heuristic baseline (advisor/heuristic.rs) ----------------------
    pub const HEURISTIC_UNAVAILABLE: &str =
        "Heuristic baseline (AI vision unavailable (untrusted provider diagnostic): {e}). \
         mean_luma={mean}/255, clip black/white={clip_b}%/{clip_w}% → exposure {ev}EV, \
         highlights {hl}, shadows {sh}.";
    pub const HEURISTIC_NO_KEY: &str =
        "Heuristic baseline (no AI vision; OPENAI_API_KEY unset). \
         mean_luma={mean}/255, clip black/white={clip_b}%/{clip_w}% → exposure {ev}EV, \
         highlights {hl}, shadows {sh}.";

    // --- heal / retouch (retouch.rs) ------------------------------------
    pub const HEAL_DETECT_FAILED: &str =
        "AI spot-detection failed ({e}); healed the painted mask only.";
    pub const HEAL_NOTE_SEP: &str = "; ";
    pub const HEAL_BUDGET: &str =
        "healed {n} of {total} painted region(s) — the rest exceeded the retouch budget \
         ({max_spots} regions / {max_bbox}x bbox / {max_disk}x \
         heal coverage) and were left UNTOUCHED; paint fewer or smaller regions";
}

/// Render ONE note's English text: substitute each `{name}` with its arg,
/// in a single pass over the TEMPLATE only (the GUI trf rule — a value that
/// happens to contain brace syntax is never reinterpreted as markup). An
/// unmatched placeholder stays visible; a bare `{` with no closer is text.
pub fn render_one(n: &Note) -> String {
    let mut out = String::with_capacity(n.key.len());
    let mut rest = n.key;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match n.args.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Render a note sequence to the English string the persisted rationale
/// carries — plain concatenation, each template owns its leading separator.
pub fn render_en(notes: &[Note]) -> String {
    notes.iter().map(render_one).collect()
}

/// Append a note to BOTH carriers at once: the English rendering onto the
/// rationale string (the persisted truth) and the typed note onto the vec
/// (the GUI's localizable copy) — one call site per fact, so the two can
/// never drift. Past [`MAX_NOTES`] the string still grows (it stays the
/// complete record) and the vec stops; the consumer-side strip rule then
/// falls back to raw English rather than rendering a partial translation.
pub fn push_note(rationale: &mut String, notes: &mut Vec<Note>, note: Note) {
    rationale.push_str(&render_one(&note));
    if notes.len() < MAX_NOTES {
        notes.push(note);
    } else if notes.len() == MAX_NOTES {
        // Poison pill (see MAX_NOTES): a truncated vec must never
        // strip-match the (complete) string again.
        notes.push(Note::plain(TRUNCATED_SENTINEL));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refactor moved every deterministic rationale from `format!` to a
    /// template + args. recipe.json, the XMP comment, the HTTP header, the
    /// CLI printout and the verifier prompt all carry the STRING, so its
    /// bytes must be exactly what the old `format!` produced — pinned here
    /// against the original expressions for representative producers.
    #[test]
    fn deterministic_rationale_renders_byte_identical_english_from_notes() {
        let round = 2usize;
        let e = "boom {braces} stay";
        assert_eq!(
            render_one(&Note::new(
                keys::REVISION_FAILED,
                vec![("round", round.to_string()), ("e", e.to_string())],
            )),
            format!(" [revision round {round} failed ({e}) — keeping the previous verified proposal]"),
        );
        let strength: f32 = 0.7;
        assert_eq!(
            render_one(&Note::new(
                keys::STYLE_DISTILLED,
                vec![("pct", format!("{:.0}", strength * 0.6 * 100.0))],
            )),
            format!(
                " [style distillation then pulled the global sliders toward this user's past \
                 edits (effective strength {:.0}%) — final values can differ from the \
                 derivation above]",
                strength * 0.6 * 100.0
            ),
        );
        let (err_before, err_after) = (0.2013_f32, 0.0567_f32);
        assert_eq!(
            render_one(&Note::new(
                keys::FIT_SUMMARY_NO_CURVE,
                vec![
                    ("err_before", format!("{err_before:.3}")),
                    ("err_after", format!("{err_after:.3}")),
                ],
            )),
            format!(
                "Reverse-fit from a target rendition (statistical match; the target is not \
                 pixel-aligned, so local masks and per-band HSL are not recovered): luma-CDF \
                 → tone sliders {}, chroma → saturation, per-channel cast curves. Residual \
                 look error {err_before:.3} → {err_after:.3}.",
                "(no residual curve)",
            ),
        );
        let (s_share, t_share) = (0.075_f32, 0.226_f32);
        assert_eq!(
            render_one(&Note::new(
                keys::ZONE_SHARE_MISMATCH,
                vec![
                    ("label", "sky".to_string()),
                    ("s", format!("{:.0}", s_share * 100.0)),
                    ("t", format!("{:.0}", t_share * 100.0)),
                ],
            )),
            format!(
                " Note: the {} zone covers {:.0}% of the source frame \
                 but {:.0}% of the target's — the compositions differ, so the \
                 overall distribution residual stays where the global fit \
                 left it.",
                "sky",
                s_share * 100.0,
                t_share * 100.0,
            ),
        );
        assert_eq!(
            render_one(&Note::plain(keys::MASKS_NOT_PRESERVED)),
            "\n⚠ the response did not preserve mask identities (a mask was renamed or \
             duplicated) — your masks were kept unchanged and the model's mask edits were \
             discarded",
        );
    }

    /// The substitution is single-pass over the template: values carrying
    /// brace syntax are inserted verbatim, an unknown placeholder stays
    /// visible, and a bare `{` is plain text (the GUI trf contract).
    #[test]
    fn note_rendering_is_single_pass_and_total() {
        let n = Note::new(
            keys::REVISION_FAILED,
            vec![("round", "1".into()), ("e", "err with {round} inside".into())],
        );
        let s = render_one(&n);
        assert!(s.contains("(err with {round} inside)"), "{s}");
        assert!(!s.contains("(err with 1 inside)"), "value braces must not re-expand: {s}");

        let missing = Note::new(keys::REVISION_FAILED, vec![("round", "1".into())]);
        assert!(render_one(&missing).contains("({e})"), "unmatched placeholder stays visible");

        let both = render_en(&[Note::plain(keys::FIT_NOTE_SAT_PEGGED), Note::plain(keys::FIT_NOTE_REHUE_BLOCKED)]);
        assert!(both.starts_with(" Saturation demand"));
        assert!(both.contains("cap (±60). Colour-cast curves"), "plain concatenation: {both}");
    }

    /// push_note keeps string and vec in lockstep, and the vec stops at the
    /// bound while the string keeps the complete record.
    #[test]
    fn push_note_binds_the_string_and_the_vec_together() {
        let mut r = String::new();
        let mut v = Vec::new();
        for _ in 0..(MAX_NOTES + 3) {
            push_note(&mut r, &mut v, Note::plain(keys::FIT_NOTE_SAT_PEGGED));
        }
        assert_eq!(v.len(), MAX_NOTES + 1, "the bounded vec plus ONE poison pill");
        assert_eq!(v.last().unwrap().key, TRUNCATED_SENTINEL);
        assert_eq!(
            r.len(),
            keys::FIT_NOTE_SAT_PEGGED.len() * (MAX_NOTES + 3),
            "the string keeps the complete record"
        );
        // Codex AL F7: with REPEATED notes the retained 64 renderings match
        // the string's tail byte-for-byte — without the pill the consumer
        // would localize a truncated subset and present the overflow as
        // prose. The pill renders to text the string cannot contain, so the
        // strip misses and the raw-English fallback engages.
        assert!(
            !r.ends_with(render_en(&v).as_str()),
            "a truncated vec must never strip-match its string"
        );
        // Below the bound the two carriers are byte-lockstep — the suffix
        // rule that consumers strip on.
        let mut r2 = String::from("model prose.");
        let mut v2 = Vec::new();
        push_note(&mut r2, &mut v2, Note::plain(keys::FIT_NOTE_SAT_PEGGED));
        push_note(&mut r2, &mut v2, Note::plain(keys::FIT_NOTE_REHUE_BLOCKED));
        assert_eq!(r2.strip_suffix(render_en(&v2).as_str()), Some("model prose."));
    }
}
