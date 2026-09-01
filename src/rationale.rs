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
         pixel-aligned, so local masks and per-band hue rotation are not recovered): luma-CDF \
         → tone sliders + residual tone curve, chroma → saturation, per-band colour mixer, \
         per-channel cast curves. Residual look error {err_before} → {err_after}.";
    pub const FIT_SUMMARY_NO_CURVE: &str =
        "Reverse-fit from a target rendition (statistical match; the target is not \
         pixel-aligned, so local masks and per-band hue rotation are not recovered): luma-CDF \
         → tone sliders (no residual curve), chroma → saturation, per-band colour mixer, \
         per-channel cast curves. Residual look error {err_before} → {err_after}.";
    pub const FIT_SUMMARY_ATMOSPHERE: &str =
        "Reverse-fit Atmosphere mode (structural divergence D={d}): the target's \
         structure cannot be reconstructed by develop controls, so only its atmosphere \
         and overall tone/colour were matched with bounded robust controls. Residual look \
         error {err_before} → {err_after}.";
    pub const FIT_NOTE_FAR: &str =
        " NOTE: the fitted recipe still renders far from the target \
         (residual {err_after}) — this look exceeds what global \
         sliders can express; consider the AI variant itself or a zoned \
         edit.";
    pub const FIT_NOTE_SAT_PEGGED: &str = " Saturation demand exceeded the model cap (±60).";
    pub const FIT_NOTE_ATMOSPHERE_SAT_PEGGED: &str =
        " Atmosphere-mode saturation demand exceeded its conservative cap (±{cap}).";
    pub const FIT_NOTE_ATMOSPHERE_CONFIDENCE: &str =
        " Atmosphere-mode confidence is capped at {cap} because develop controls cannot \
         recreate the divergent structure.";
    pub const FIT_NOTE_ATMOSPHERE_POPULATION_EVIDENCE: &str =
        " Atmosphere controls were read on population evidence; structural withholding \
         of luma ranges [{luma_ranges}] and hue bands [{hue_bands}] applies to the \
         residual, zone and detail fits, not to the bounded atmosphere controls.";
    pub const FIT_NOTE_WB_CLAMPED: &str =
        " White balance was clamped into the strength budget (gain ratio {from} to {to}, rotated share {rotated_share} over {coverage} of the frame); the requested cast exceeded the honest range.";
    pub const FIT_NOTE_WB_WITHHELD_FOREIGN_HUE: &str =
        " White balance withheld: it would paint hues the target does not contain.";
    pub const FIT_NOTE_WB_WITHHELD_ROTATION: &str =
        " White balance withheld: rotated share {rotated_share} over {coverage} of the frame exceeded the strength budget.";
    pub const FIT_NOTE_WB_SEARCH_BOUND: &str =
        " White balance search reached the {k} K domain bound; the requested colour temperature may lie beyond the fitted range.";
    pub const FIT_NOTE_CAST_ADMITTED_BY_STRENGTH: &str =
        " Colour-cast curves were admitted by the strength budget (measured ratio {ratio}, budget {budget}).";
    pub const FIT_NOTE_GLOBAL_CAST: &str =
        " Global colour cast measured from consistent hue rotation across the populated frame (rotation {rotation} degrees, chroma ratio {ratio}); white balance and saturation were read from population evidence.";
    pub const FIT_NOTE_VETO_DISCLOSED: &str =
        " High-strength fit disclosed unsupported movement in {kind}: {ranges}. The controls were retained, but confidence is capped by the strength budget.";
    pub const FIT_NOTE_STRENGTH: &str =
        " Reverse-fit used panel Strength {pct}% to derive its honesty budget.";
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
    /// confidence still carries the shared-evidence identifiability cap, so
    /// the missing joint opinion cannot turn into an unsupported high claim.
    /// Silence here would make two different claims look identical.
    pub const FIT_NOTE_JOINT_NONE: &str =
        " (the joint distribution check found no value range with enough \
         evidence on both sides, so it has no opinion on this pair; confidence \
         remains capped by the pair's shared-evidence identifiability)";
    pub const FIT_NOTE_JOINT_FAR: &str =
        " That joint mismatch is large: the two images still differ inside \
         matching value ranges, which the single residual number above \
         cannot show — treat this fit as a starting point, not a match.";
    pub const FIT_NOTE_JOINT_MISS: &str =
        " The fit tried the supported controls but did not reach the target: \
         the two images still differ inside matching value ranges, which the \
         single residual number above cannot show; treat this fit as a starting \
         point, not a match.";
    pub const FIT_NOTE_JOINT_REFUSED: &str =
        " The fit withheld supported movement because the evidence was one-sided, \
         so this result is deliberately farther from the target; this is a \
         refusal, not a miss.";
    pub const FIT_NOTE_JOINT_REGRESSED: &str =
        " (the refusal came from the joint distribution check, not the \
         residual: the fitted recipe pushed the value ranges further apart \
         than leaving the photo alone)";
    pub const FIT_NOTE_EVIDENCE_CONTRADICTED: &str =
        " Fit refused because the evidence contradicted the proposed correction: \
         the measurable value ranges moved farther apart, so the recipe was reset \
         to the untouched calibration.";
    pub const FIT_NOTE_EVIDENCE_UNMEASURABLE: &str =
        " Fit withheld because nothing measurable was available here: no shared \
         value range had enough evidence on both sides, so no correction was fitted.";
    /// R23-6 A-5: which controls THIS pair seems to need that the solver
    /// never assigns.
    pub const FIT_NOTE_UNREPRESENTED: &str =
        " This target's look appears to use {controls}, which the reverse-fit \
         never solves for (its solution space is exposure/contrast/\
         highlights/shadows/whites/blacks, a tone curve, one global \
         saturation, clarity/texture, an evidence-gated per-band colour mixer that never \
         rotates hue, and the three channel curves) — that part of the look \
         cannot arrive through this route.";
    pub const FIT_NOTE_ATMOSPHERE_UNREPRESENTED: &str =
        " This target's remaining look appears to need {controls}; Atmosphere mode only \
         solves bounded exposure, white balance, a robust five-point tone curve, \
         saturation, an evidence-gated per-band colour mixer that never rotates hue, \
         and evidence-gated clarity/texture, so that part cannot arrive through this route.";
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
    ///
    /// R23 round review LOW-3: the VERDICT was right and the attribution was
    /// wrong. At a 2% aspect tolerance the commonest trigger by far is a crop
    /// of exactly the frame the user meant — a 3:2 photo exported at 16:9 is
    /// 18% out — and telling that user "two different pictures" sends them
    /// hunting a file mix-up they did not make. Name the crop first. The
    /// conclusion does not change: a crop moves which pixels the statistics are
    /// taken over, so the two distributions are incomparable either way.
    pub const FIT_NOTE_NOT_SAME_FRAME: &str =
        " WARNING: the reference's proportions do not match this photo's — it \
         was CROPPED, or it is not the same frame. Either way the two \
         distributions are not comparable, because a crop changes which pixels \
         the statistics are taken over. The fit matched them anyway, as it was \
         asked to — treat the result as unreliable.";

    /// The paired-path summaries: same solve family, but the sentence must
    /// not claim "the target is not pixel-aligned" on the pairs whose
    /// alignment the solve just used.
    pub const FIT_SUMMARY_WITH_CURVE_PAIRED: &str =
        "Reverse-fit from a target rendition (paired robust match on corresponding \
         pixels; local masks and per-band hue rotation are still not solved): robust paired \
         luma regression → tone sliders + residual tone curve, chroma → saturation, \
         per-band colour mixer, per-channel cast curves. Residual look error {err_before} → {err_after}.";
    pub const FIT_SUMMARY_NO_CURVE_PAIRED: &str =
        "Reverse-fit from a target rendition (paired robust match on corresponding \
         pixels; local masks and per-band hue rotation are still not solved): robust paired \
         luma regression → tone sliders (no residual curve), chroma → saturation, \
         per-band colour mixer, per-channel cast curves. Residual look error {err_before} → {err_after}.";
    pub const FIT_NOTE_ROBUST_REJECTED: &str =
        " Paired robust fit: {pct}% of the comparable pixels disagreed with any \
         single global develop of this source (concentrated in [{ranges}]) and \
         were down-weighted before the controls were solved.";
    pub const ZONE_ROBUST_REJECTED: &str =
        " Zoned {label} robust fit down-weighted {pct}% of the overlapping pixels \
         as content the two zones do not share (concentrated in [{ranges}]).";

    /// The paired-doctrine refinement's disclosure half: when vouched
    /// convergence carries movement through a one-sided hue band, the
    /// withheld-note's "vetoed movement" claim must not stand alone.
    pub const FIT_NOTE_VOUCHED_CONVERGENCE: &str =
        " Paired convergence carried movement through one-sided hue bands \
         [{bands}]: each moved pixel was individually vouched (robust \
         weight, hue-coherent with the global edit) and moved toward its own \
         paired target pixel; unvouched pixels kept the veto.";

    pub const FIT_NOTE_EVIDENCE_WITHHELD: &str =
        " Evidence gating withheld luma ranges [{luma_ranges}] and hue bands [{hue_bands}]. One-sided [{one_sided}] is UNMEASURABLE, not equal, so it vetoed movement. Sparse on both sides [{sparse}] was excluded from estimation but did not veto a move. Structurally divergent [{divergent}] also vetoed movement.";
    pub const FIT_NOTE_DETAIL: &str =
        " Detail evidence fitted clarity {clarity} and texture {texture} within the +/-20 budget; their high-frequency reading used only identifiable pixels.";
    pub const FIT_NOTE_DETAIL_WITHHELD: &str =
        " Detail controls were withheld: two-sided structural and luma-range evidence did not support a safe global detail move, so clarity and texture were not moved.";

    /// Stage 4a: the per-band colour mixer solved from population statistics.
    /// Both halves matter — what moved, and which bands were left alone
    /// because neither frame's population could speak for them.
    pub const FIT_NOTE_HSL_BANDS: &str =
        " Per-band colour mixer, solved from each band's own population: [{moved}]. Hue rotation is never solved, so every band's hue stays 0. Bands left neutral for want of two-sided population evidence: [{refused}].";
    pub const FIT_NOTE_HSL_WITHDRAWN_ERROR: &str =
        " The per-band colour move was given back: applying it did not leave the frame closer to the target, so every band returned to neutral.";
    pub const FIT_NOTE_HSL_WITHDRAWN_BLIND: &str =
        " The per-band colour move was given back: it would have carried pixels through hue bands no two-sided evidence covers, and blind movement is vetoed rather than shipped.";

    /// R30 batch 1 (R2-lite): what the Atmosphere global solve's two robust
    /// controls are actually READ FROM. Zero behaviour change — the
    /// assumption was always there, and was never stated.
    pub const FIT_ATMOSPHERE_REFERENCE_POPULATION: &str =
        " Atmosphere white balance and exposure were solved from WHOLE-FRAME per-channel \
         weighted medians of both sides. That pairs the two frames as distributions, which \
         assumes both describe the same content — the assumption this mode is selected \
         precisely because it does not hold.";
    /// …and, when a correspondence field exists, HOW MUCH of that reference
    /// population had no counterpart to be paired with.
    pub const FIT_ATMOSPHERE_REFERENCE_UNPAIRED: &str =
        " Of that reference population, {share}% of the target has no confident counterpart \
         in the source (cross-image correspondence below {tau}, read on the sidecar's {grid} \
         cell grid, so this is a coarse share) — and defined those two controls all the same.";
    /// R30 R2: …and once that unpaired share is EXCLUDED from the two
    /// controls rather than left to define them, the sentence above stops
    /// being true and this one takes its place. Same measurement, opposite
    /// consequence — which is why it is a second key and not an edit to the
    /// first (a persisted rationale must never change meaning under its own
    /// spelling).
    pub const FIT_ATMOSPHERE_REFERENCE_EXCLUDED: &str =
        " Of the whole-frame reference population, {share}% of the target has no confident \
         counterpart in the source (cross-image correspondence below {tau}, read on the \
         sidecar's {grid} cell grid, so this is a coarse share) — and was EXCLUDED from \
         those two controls rather than left to define them.";
    /// R30 R2: the whole-frame sentence's replacement, for the case where a
    /// correspondence field let the solve read those two controls over the
    /// content the two frames actually share.
    pub const FIT_ATMOSPHERE_REFERENCE_SHARED: &str =
        " Atmosphere white balance and exposure were solved from the SHARED-CONTENT \
         population of the two frames rather than from the whole frame: target pixels no \
         confident source pixel answers for are generated content that is not a rendition \
         of this frame, and source pixels whose content the target replaced have nothing \
         left to be compared with (cross-image correspondence below {tau} on either side), \
         so both were dropped before the per-channel weighted medians were read. That left \
         {src}% of the source's evidence mass and {tgt}% of the target's, and the two \
         distributions being paired now describe the same content.";
    /// R30 R2: …and when the shared population is too small to be read as a
    /// population, the whole-frame sentence stands and this one says why it
    /// had to.
    pub const FIT_ATMOSPHERE_REFERENCE_THIN: &str =
        " The shared-content population that would have replaced it retains only {src}% of \
         the source's evidence mass and {tgt}% of the target's, under the {floor}% a \
         population must keep to be read as one — so the whole-frame medians above stand, \
         and so does their pairing assumption.";
    /// …and when there is no field, the share is UNKNOWN, not zero.
    pub const FIT_ATMOSPHERE_REFERENCE_UNMEASURED: &str =
        " How much of that reference population has no counterpart in the source was not \
         measured: no cross-image correspondence field was available for this pair.";

    /// Step 7b: a content-divergent fit obtained a cross-image correspondence
    /// field (local DIFT sidecar) and the zone estimators use it.
    pub const FIT_CORRESPONDENCE: &str =
        " Cross-image correspondence measured (DIFT): {cov}% of the frame has a \
         confident counterpart in the target (median confidence {med}); full \
         zone fits weight pairs by it and read shifted content at its \
         corresponded position.";
    /// …and the sidecar failing must degrade with its reason, never silently.
    pub const FIT_CORRESPONDENCE_UNAVAILABLE: &str =
        " Cross-image correspondence unavailable ({e}) — the content-divergent \
         estimators ran without it.";

    // --- zoned fit (fit_zoned.rs) ---------------------------------------
    pub const ZONED_UNAVAILABLE: &str =
        " Zoned sky fit unavailable ({e}) — trying the automatic luminance-range fallback.";
    pub const ZONED_NO_PARTITION: &str =
        " Zoned fit skipped: no usable sky partition (sky covers {s}% \
         of the source frame, {t}% of the target's).";
    pub const ZONE_TOO_SMALL: &str =
        " The {label} zone covers too little of the frame (source {s}%, \
         target {t}%) — skipped.";
    pub const ZONE_MODE_FULL: &str =
        " The {label} structural divergence is D={d}; Full zone fit selected.";
    pub const ZONE_MODE_ATMOSPHERE: &str =
        " The {label} structure cannot be reconstructed by develop controls; \
         matching atmosphere only (D={d}).";
    pub const ZONE_QUALITY_PASSED: &str =
        " Local quality gate passed for {label}: texture ratio {texture}, clipped \
         share {clip_before}% → {clip_after}%.";
    pub const ZONE_QUALITY_TEXTURE_FAILED: &str =
        " Zoned {label} correction dropped by the local-quality texture gate: \
         ratio {ratio} is outside [{min}, {max}].";
    pub const ZONE_QUALITY_CLIPPING_FAILED: &str =
        " Zoned {label} correction dropped by the local-quality clipping gate: \
         clipped share {before}% → {after}% (allowed growth {growth} percentage point).";
    /// The zoned fit withholds control CLASSES, not whole corrections: a
    /// one-sided hue band silences colour movement while supported luma
    /// evidence still earns the zone its tone correction, and the reverse.
    /// One sentence claiming "the correction was withheld" therefore said
    /// something the code does not do, and said the SAME thing for three
    /// different outcomes (colour-only, tone-only, both). One key per claim;
    /// when both classes are refused BOTH notes ride and their conjunction
    /// IS the whole-correction refusal. What survived is stated positively
    /// by [`ZONE_ATTACHED`], which carries the values that were kept.
    pub const ZONE_EVIDENCE_WITHHELD_COLOUR: &str =
        " Zoned {label} colour controls withheld: they would move zero-evidence hue bands [{hue_bands}]. Those bands were not adjusted blindly.";
    pub const ZONE_EVIDENCE_WITHHELD_TONE: &str =
        " Zoned {label} tone controls withheld: they would move zero-evidence luma ranges [{luma_ranges}]. Those ranges were not adjusted blindly.";
    /// The share-mismatch exit attaches NO zone. It used to borrow the
    /// evidence-withheld sentence with both range lists empty, which read
    /// as "it would move zero-evidence luma ranges [none]" -- a claim about
    /// ranges, where the actual reason is that the two populations are not
    /// measurements of the same subject at all.
    pub const ZONE_SHARE_NO_CORRECTION: &str =
        " No zoned correction attached: the source and target zone shares differ by more than 2:1, so neither population is a comparable measurement of the same subject.";
    pub const ZONE_BOUNDARY_PASSED: &str =
        " Boundary-continuity gate kept {n} zoned correction(s): introduced transition \
         rim {before} to {after} luma after shared differential shrink k={k} \
         (budget {max}, {transitions} measured transitions).";
    /// Step 9. Distinct from [`ZONE_BOUNDARY_DROPPED`], which says the k=0
    /// render was itself over budget (an engine invariant failure). THIS one
    /// says the gate found a shrink inside the budget and that shrink moves
    /// no pixel at all, so there is nothing to attach.
    pub const ZONE_BOUNDARY_INERT: &str =
        " Zoned {n} correction(s) dropped by the boundary-continuity gate: \
         candidate introduced rim {before} luma, and the largest shrink inside \
         budget {max} was k={k}, whose render is byte-identical to the frame \
         without it — reading {after} over {transitions} measured transitions. \
         An inert attachment would occupy the correction budget and disclose a \
         change it did not make.";
    pub const ZONE_BOUNDARY_DROPPED: &str =
        " Zoned corrections dropped by the boundary-continuity gate: candidate \
         rim {before} luma, and even shared shrink k=0 left {after} \
         (budget {max}, {transitions} measured transitions).";
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
    pub const ZONE_ATMOSPHERE_DROPPED: &str =
        " Zoned {label} atmosphere correction dropped: zone residual {before} → \
         {after} did not satisfy do-no-harm, or frame-global drift {drift} exceeded \
         tolerance {tol}.";
    /// R30 batch 1 (R1): the third acceptance arm is the one that changes
    /// what ships, so a correction admitted by it says so — with both
    /// readings it was decided on, and with the ONE safety gate that is
    /// known not to discriminate named rather than counted.
    pub const ZONE_STRICTLY_BETTER: &str =
        " Zoned {label} correction kept by the strictly-better arm: zone residual {before} \
         → {after}, an absolute gain past the {gain} floor that the halve-or-land arms \
         would have dropped, at no cost to the frame ({frame_before} → {frame_after}). \
         Local quality read texture ratio {texture}; that texture floor is calibrated but \
         known not to separate every case, so this correction rests on the clipping gate, \
         the zero-regression frame reading and the boundary gate.";
    /// Which frame the residual above was measured on.
    ///
    /// The AI-proposal path solves and scores over the camera's EMBEDDED
    /// rendition -- deliberately, because the base look and the lens
    /// corrections are already in those pixels (pipeline.rs `produce_recipe`'s
    /// header) and the confidence ladder is calibrated on exactly those
    /// numbers. The photo's calibration is then stamped onto the finished
    /// recipe, so the delivered render is `user(base(x))`: a different frame,
    /// differing in chroma by construction because `camera_base_knots` matches
    /// luma only. Without this note the reader compares a printed residual
    /// against a picture it was never measured on.
    ///
    /// Emitted ONLY when the stamp actually introduces that difference. A
    /// source with no camera rendition and no lens profile is already the
    /// delivered frame, and claiming otherwise would be its own false note.
    pub const FIT_RESIDUAL_PRE_CALIBRATION: &str =
        " The residual above was measured on this camera's embedded rendition, \
         which is the frame the fit and the review both saw. The delivered \
         render additionally applies this photo's own calibration ({what}), so \
         it is a different frame — closer to the target in luma, and differing \
         in chroma, because the camera curve is matched on luma alone.";
    pub const ZONE_ALREADY_MATCHED: &str =
        " The {label} zone already matches the target (zone residual \
         {before}) — no correction needed.";
    /// The neutral-solution exit attaches NO zone, and it is NOT
    /// [`ZONE_ALREADY_MATCHED`]. That sentence claims the zone matches the
    /// target; this exit is reached when every dial the estimator produced
    /// came back within 1e-4 of neutral, which is a statement about the
    /// SOLUTION -- the evidence and quality gates left nothing they were
    /// willing to move. The two are independent: a zone can be far from its
    /// target and still solve to neutral, and the borrowed sentence then
    /// printed the contradicting residual inside its own claim ("already
    /// matches the target (zone residual 0.143)" against a 0.012 ceiling).
    /// Same rule, same fix as [`ZONE_SHARE_NO_CORRECTION`]: a terminal exit
    /// states its own reason.
    pub const ZONE_NO_MOVEMENT_SURVIVED: &str =
        " No zoned {label} correction attached: every control that survived \
         the evidence and quality gates solved to neutral, so the zone \
         residual {before} is left uncorrected.";
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
    pub const REGION_FRAME_REFUSED: &str =
        " Multi-region semantic corrections refused after the final comparison: \
         the multi-region frame residual {multi} was no better than the seeded \
         two-region residual {two}, so the byte-identical two-region result was kept. \
         Trialled regions: {regions}.";
    /// The multi-class layer failed (sidecar, manifest, budget, claim). The
    /// historical sky/land route ran INSTEAD and is what the report describes;
    /// its own `ZONED_UNAVAILABLE` would narrate a luminance-range fallback
    /// that did not happen here, hence a key of its own.
    pub const SEMANTIC_REGIONS_UNAVAILABLE: &str =
        " Multi-region semantic segmentation unavailable ({e}) — the historical \
         sky/land pass was used instead.";
    /// The multi-class layer ran but no class cleared the shared support floor
    /// on both frames: a typed hand-off to the historical route, which judges
    /// the sky partition on its own numbers.
    pub const SEMANTIC_REGIONS_NONE: &str =
        " No semantic region cleared the shared support floor on both frames \
         (up to {n} requested) — the historical sky/land pass was used instead.";
    /// A semantic region's own boundary gate refused it. The shared gate hands
    /// back only what it measured — the candidate rim and why — so no `after`
    /// reading or shrink factor is reported for a shrink it never accepted.
    pub const REGION_BOUNDARY_REFUSED: &str =
        " The {label} region was refused by its boundary-continuity gate \
         ({why}): candidate rim {before} luma against budget {max} \
         ({transitions} measured transitions).";

    // --- spatial residual tiles and bitmap-mask refinement -------------
    pub const TILE_ELIGIBLE: &str =
        " Spatial tile {id} eligible in derivation {generation}: frozen evidence \
         shares source {s}, target {t}, original D={d}, signed residual {residual} \
         (95% CI +/-{ci}, parent {parent}).";
    pub const TILE_ATTACHED: &str =
        " Spatial tile {id} attached as an engine bitmap: local residual \
         {before} -> {after}, composed frame {frame_before} -> {frame_after}, \
         boundary {boundary}. Classic XMP omits this correction with the named \
         bitmap-mask loss.";
    pub const TILE_ABSTAINED: &str =
        " Spatial tile {id} abstained in derivation {generation} ({reason}): \
         frozen evidence shares source \
         {s}, target {t}, original D={d}, signed residual {residual} (95% CI \
         +/-{ci}, parent {parent}).";
    pub const TILE_SWEEP: &str =
        " Spatial sweep {generation}: eligible parent nodes {eligible}; \
         abstentions by source share {s}, target share {t}, structural \
         divergence {d}, confidence interval {ci}, parent proximity {parent}, \
         other {other}.";
    pub const TILE_DEPTH_CAP: &str =
        " Spatial traversal stopped at depth {depth} with a {cap}-tile attachment \
         cap; {attached} tile(s) attached.";
    pub const TILE_BOUNDARY_PASSED: &str =
        " Spatial tile {id} passed the boundary gate: cross-boundary step \
         {before} -> {after} luma after direction-preserving shrink k={k} \
         (budget {max}, {transitions} measured crossings).";
    pub const TILE_BOUNDARY_REFUSED: &str =
        " Spatial tile {id} refused by its boundary/composed-frame gate: \
         candidate step {before}, final reading {after}, budget {max} \
         ({transitions} measured crossings, k={k}).";
    pub const MASK_REFINEMENT_KEPT: &str =
        " Guided mask refinement kept for {label}: coverage delta {coverage}, \
         guide-edge alignment {before} -> {after}, core pixels changed {core}.";
    pub const MASK_REFINEMENT_ABSTAINED: &str =
        " Guided mask refinement abstained for {label}: coverage delta {coverage}, \
         guide-edge alignment {before} -> {after}, core pixels changed {core}; \
         the original mask bytes were retained.";

    // --- native luminance-range fallback (fit_zoned.rs) -----------------
    pub const RANGE_ATTACHED: &str =
        " {label} attached for luminance [{lo}, {hi}] (local exposure {ev} EV, \
         colour gains [{g0} {g1} {g2}], saturation {sat}): band residual \
         {before} → {after}. The sentinel-hosted luminance range is native in \
         the Lightroom sidecar.";
    pub const RANGE_ABSTAINED: &str =
        " Luminance range [{lo}, {hi}] abstained: {reason}.";
    pub const RANGE_MERGED: &str =
        " Luminance range [{lo}, {hi}] merged into [{into_lo}, {into_hi}] \
         {why}; both runs have sign {sign}.";
    pub const RANGE_BOUNDARY_PASSED: &str =
        " Range boundary-continuity gate kept {n} correction(s): signed \
         transition rim {before} to {after} luma after shared \
         direction-preserving shrink k={k} (budget {max}, {transitions} \
         measured crossings).";
    pub const RANGE_BOUNDARY_REFUSED: &str =
        " Range corrections refused by the boundary-continuity gate: candidate \
         rim {before} luma, and even zero differential left {after} (budget \
         {max}, {transitions} measured crossings).";
    pub const RANGE_FRAME_REFUSED: &str =
        " Range corrections refused after the final boundary pass: the \
         composed frame residual {after} exceeded the global-only residual \
         {global} plus tolerance {tol}, so all {n} range correction(s) were removed.";
    pub const RANGE_CONFIDENCE: &str =
        " Confidence for this fit includes the {n} accepted luminance-range \
         correction(s) (worst band residual {worst}); the final frame residual \
         is {frame}.";

    // --- local-field analyzer (fit_zoned/field.rs) ----------------------
    pub const LOCAL_CEILING: &str =
        " Local-field ceiling: global {global}, ceiling {ceiling}, realized \
         {realized}, saturated vertices {saturated}, CG iterations {iterations}.";
    pub const LOCAL_SHAPE: &str =
        " Local-field shape: R2 tiles {r2_tiles}, R2 linear {r2_linear}, verdict \
         {shape}, effective tile cap {cap}, structured bins [{structured}].";
    pub const LOCAL_BAND_SKIPPED: &str =
        " Local-field band skipped: bin {bin}, dispersion {dispersion}/255, \
         maximum {max}/255.";
    pub const LOCAL_REALIZED: &str =
        " Local-field realized after {producer}: frame {err_after}, ceiling \
         {ceiling}, share {realized}.";
    pub const LOCAL_STOP: &str =
        " Local-field stop after {producer}: skipped [{skipped}], margin {margin}.";
    pub const FIELD_MASK_PROPOSED: &str =
        " Field mask {n} proposed: {sign} m={mass} s={share_src}/{share_tgt} D={d} p={pixels}.";
    pub const FIELD_MASK_ATTACHED: &str =
        " Field mask {n} attached: {err_before}->{err_after}, cross-boundary step {step} (bitmap/XMP loss).";
    pub const FIELD_MASK_REFUSED: &str =
        " Field mask component(s) {n} refused: {why}.";
    pub const FIELD_MASK_NONE: &str =
        " No field mask qualified: {why}.";

    // --- the propose/verify pipeline (pipeline.rs) ----------------------
    pub const REVISION_FAILED: &str =
        " [revision round {round} failed ({e}) — keeping the previous verified proposal]";
    pub const REVISION_VERIFY_FAILED: &str =
        " [verification of revision round {round} failed ({e}) — keeping the previous \
         verified proposal]";
    /// NAMES THE FIELDS (batch 2). The percentage alone said that something had
    /// been pulled and left "toward what, and what moved?" unanswered on a note
    /// that is persisted, re-rendered in three UIs, and sits beside a
    /// derivation it can contradict. `style::distilled_fields` measures the
    /// list from the two recipes and bounds it.
    pub const STYLE_DISTILLED: &str =
        " [style distillation then pulled this recipe toward this user's past \
         edits (effective strength {pct}%; moved: {fields}) — final values can \
         differ from the derivation above]";
    pub const STYLE_REVERIFY_FAILED: &str =
        " [re-verification after style distillation failed ({e}) — the verdict \
         above describes the PRE-distillation recipe]";
    pub const STYLE_UNAVAILABLE: &str =
        " [style reference unavailable ({e}) — the Style slider had no effect on this \
         develop; rebuild it with: autoshade style-index <folder>]";
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
    pub const STYLE_LOOK_REFERENCE: &str = " [look reference: finished photo {stem} from the photographer's look library; tags: {tags}]";
    pub const STYLE_LOOK_IMAGE: &str = " [finished look photo {stem} also went to the vision model as IMAGE 2]";
    pub const STYLE_LOOKS_UNREACHABLE: &str = " [look library unavailable for this develop ({n} finished photos): style embedding was off or no query vector was produced]";
    /// Step 14 / S2: how many exemplars of a freshly built library carry a
    /// LOCAL prose description of their grade. It is a build-path note, not a
    /// develop-path one: the count is a property of the library, and the thing
    /// a user needs to know after a build that took minutes is how much of it
    /// the optional pass actually covered.
    pub const STYLE_DESCRIBED: &str = " [look descriptions: {n} of {total} exemplars carry a local prose description]";
    pub const ADVISOR_NOTE_DIRECTION_ADHERENCE: &str = " [direction adherence tier: {tier}]";
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

/// Reduce an operational error to the single sanitized line safe for a
/// persisted rationale. Full diagnostics remain available to stderr/logging.
pub fn error_line(error: &anyhow::Error) -> String {
    let display = error.to_string();
    let first = display
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .unwrap_or("operation failed");
    // A Python traceback opens with "Traceback (most recent call last):" and
    // ends with the exception line; the exception line is the disclosure.
    let first = if first.to_ascii_lowercase().starts_with("traceback") {
        display
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("operation failed")
    } else {
        first
    };
    let mut line = sanitize_error_paths(first);
    if let Some(code) = error.chain().find_map(|cause| exit_code(&cause.to_string())) {
        let canonical = format!("exit {code}");
        if !line.contains(&canonical) {
            let suffix = format!("; {canonical}");
            let room = 160usize.saturating_sub(suffix.chars().count());
            line = format!("{}{}", truncate_chars(&line, room), suffix);
        }
    }
    truncate_chars(&line, 160)
}

fn exit_code(text: &str) -> Option<i32> {
    let lower = text.to_ascii_lowercase();
    for marker in ["exited", "exit"] {
        for (offset, _) in lower.match_indices(marker) {
            let tail: String = text[offset + marker.len()..].chars().take(32).collect();
            for token in tail.split(|c: char| !c.is_ascii_digit() && c != '-') {
                if token.is_empty() || token == "-" {
                    continue;
                }
                if let Ok(value) = token.parse::<i32>() {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn sanitize_error_paths(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        let drive = chars[i].is_ascii_alphabetic()
            && chars.get(i + 1) == Some(&':')
            && matches!(chars.get(i + 2), Some('/' | '\\'));
        let unix = chars[i] == '/'
            && chars.get(i + 1).is_some_and(|c| !c.is_whitespace())
            && (i == 0 || !chars[i - 1].is_ascii_alphanumeric());
        if drive || unix {
            let start = i;
            while i < chars.len()
                && !chars[i].is_whitespace()
                && !matches!(chars[i], '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '"' | '\'')
            {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            out.push_str(&path_basename(&token));
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn path_basename(token: &str) -> String {
    let mut value = token.trim_end_matches(|c: char| ",.;!?".contains(c)).to_string();
    if let Some((before, suffix)) = value.rsplit_once(':')
        && !suffix.is_empty()
        && suffix.chars().all(|c| c.is_ascii_digit())
    {
        value = before.to_string();
    }
    let parts: Vec<&str> = value.split(['/', '\\']).filter(|part| !part.is_empty()).collect();
    let lower: Vec<String> = parts.iter().map(|part| part.to_ascii_lowercase()).collect();
    if let Some(index) = lower.iter().position(|part| part == "users" || part == "home")
        && parts.len() <= index + 2
    {
        return "[path]".into();
    }
    parts.last().copied().unwrap_or("[path]").into()
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
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
                vec![
                    ("pct", format!("{:.0}", strength * 0.6 * 100.0)),
                    ("fields", "vibrance, hsl.saturation.blue".to_string()),
                ],
            )),
            format!(
                " [style distillation then pulled this recipe toward this user's past \
                 edits (effective strength {:.0}%; moved: {}) — final values can \
                 differ from the derivation above]",
                strength * 0.6 * 100.0,
                "vibrance, hsl.saturation.blue",
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
                 pixel-aligned, so local masks and per-band hue rotation are not recovered): luma-CDF \
                 → tone sliders {}, chroma → saturation, per-band colour mixer, per-channel cast \
                 curves. Residual look error {err_before:.3} → {err_after:.3}.",
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

    #[test]
    fn sidecar_failure_disclosure_has_no_traceback_or_home_path() {
        let error = anyhow::anyhow!("model sidecar exited Some(7)")
            .context("sidecar failed at C:\\Users\\alice\\Pictures\\run.py:17\nTraceback (most recent call last):\n  /home/alice/run.py");
        let safe = error_line(&error);
        assert!(!safe.contains("Traceback"), "{safe}");
        assert!(!safe.contains("alice"), "{safe}");
        assert!(!safe.contains("Users") && !safe.contains("home"), "{safe}");
        assert!(safe.contains("run.py") && safe.contains("exit 7"), "{safe}");
        assert!(safe.chars().count() <= 160, "{} chars", safe.chars().count());
        let unix = error_line(&anyhow::anyhow!("failed at /home/alice/run.py"));
        let traceback = error_line(&anyhow::anyhow!(
            r#"Traceback (most recent call last):
  File "/home/alice/run.py", line 3, in <module>
ValueError: boom at C:\Users\alice\x.py"#
        ));
        assert!(traceback.starts_with("ValueError: boom"), "{traceback}");
        assert!(!traceback.contains("Traceback") && !traceback.contains("alice"), "{traceback}");
        assert!(unix.contains("run.py") && !unix.contains("/home") && !unix.contains("alice"), "{unix}");
        assert!(error_line(&anyhow::anyhow!("sidecar exited with code 9")).contains("exit 9"));
        let long = anyhow::anyhow!("worker exited Some(12)").context("x".repeat(300));
        let long = error_line(&long);
        assert!(long.chars().count() <= 160 && long.ends_with("; exit 12"), "{long}");

        for key in [
            keys::FIT_CORRESPONDENCE_UNAVAILABLE,
            keys::ZONED_UNAVAILABLE,
            keys::REVISION_FAILED,
            keys::REVISION_VERIFY_FAILED,
            keys::STYLE_REVERIFY_FAILED,
            keys::STYLE_UNAVAILABLE,
            keys::STYLE_REF_IMAGE_FAILED,
            keys::JUDGE_ROUND_FAILED,
            keys::JUDGE_REJUDGE_FAILED,
            keys::JUDGE_UNAVAILABLE,
            keys::HEURISTIC_UNAVAILABLE,
            keys::HEAL_DETECT_FAILED,
        ] {
            let rendered = render_one(&Note::new(
                key,
                vec![
                    ("round", "1".into()),
                    ("score", "70".into()),
                    ("critique", "test".into()),
                    ("mean", "100".into()),
                    ("clip_b", "0".into()),
                    ("clip_w", "0".into()),
                    ("ev", "0".into()),
                    ("hl", "0".into()),
                    ("sh", "0".into()),
                    ("e", safe.clone()),
                ],
            ));
            assert!(!rendered.contains("Traceback"), "{key}: {rendered}");
            assert!(!rendered.contains("alice"), "{key}: {rendered}");
            assert!(!rendered.contains("C:\\Users") && !rendered.contains("/home/"), "{key}: {rendered}");
        }
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
