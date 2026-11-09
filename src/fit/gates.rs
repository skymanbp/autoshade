//! The fit's calibration constants: the CDF clip, neutral misprediction, the zoned vetoes' region share, and the reported-confidence family.

/// Quantile clip for CDF inversion — the extreme tails of a generative render
/// are noise (a few blown/crushed pixels would otherwise own the end knots).
pub(crate) const P_CLIP: f32 = 0.002;
/// Ceiling on the gated tone evidence's MISPREDICTION of its own identified
/// population (see [`neutral_gate_misprediction`]) before the assumption is
/// declared dead and the solve falls back to full-pixel CDFs. Membership
/// counts and one-sided CDF-shift proxies were both tried and both mis-rank
/// the live pairs (each fires HARDER on the haze pair than on the pair that
/// actually shipped a murky fit), so the gate is judged by the harm itself:
/// how far its map misses the pixels it claims to identify, in luma units.
/// Anchors, all measured. Fall-back side: P20 × reimagine reads 0.021
/// (the pale sky, luma q50 ≈ 197/255, re-hued vivid blue out of the class;
/// share ratio 1.29× sailed under the 1.75× gate; the shipped map missed
/// the shared class by −22/255 right in the murk band); the archived
/// P21 pairs read 0.074 (× reimagine, the golden sky — share 2.65×,
/// both gates fire), 0.034 (× reimagine-4 — share 1.51×, UNDER the share
/// gate: this detector is the only defence) and 0.024 (× reimagine-2,
/// share 1.92×); the haze fixture reads 0.126 (its blue cast tints the
/// clean side's dark greys out of the class — under the R17 dense residual
/// knots the gated solve faithfully implements that broken map and
/// collapses to a do-no-harm reset, while the fallback lands
/// 0.0892 → 0.0229). Keep side: P21 × reimagine-3 — a REAL benign
/// pair — reads 0.0050 (share 1.12×), the identity / canyon fixtures read
/// ≈ 0 (matched members) and the synthetic uniform-inflation fixture reads
/// < 0.0075. The 0.015 ceiling thus has real pairs on BOTH flanks: 3.0×
/// clear below (0.0050), 1.4× above (0.021). The archive numbers above are
/// the embedded-preview domain (`decode` output vs reimagine target — the
/// camera-look source the CLI `match` feeds this gate); the GUI's
/// composed-calibration domain was measured separately for all five real
/// pairs (R19, the repro test prints it per pair) and each verdict lands
/// on the same side of the ceiling in both domains — composed readings:
/// P20 × re2 0.024, P21 × re 0.043, × re2 0.033, × re4 0.036
/// (fall-back side), × re3 0.0131 (keep side, a 1.15× margin against the
/// preview domain's 3.0×). The haze fixture is a recipe-render pair with
/// no RAW, so a composed domain does not exist for it.
#[cfg(test)]
pub(super) const NEUTRAL_MISPREDICTION_MAX: f32 = 0.015;
/// Evidence floor for the SHARED class inside
/// [`neutral_gate_misprediction`] — the same absolute floor the per-side
/// `enough` bar uses (512 px), plus the same 5%-of-frame scaling, applied
/// to the one population the identification assumption is actually about.
/// Below it there is no identified population to score and the metric
/// reports infinite (fall back).
pub(super) const NEUTRAL_SHARED_MIN: usize = 512;
/// Share of blindly moved pixels that constitutes a REGION of the population
/// a correction moves -- a zone's coverage for the zoned solve's
/// unsupported-range vetoes (`hue_gates`; the live wrecks measured 12.5%).
/// The global solve reads the one validity law instead (`fit::validity`).
pub(super) const ROT_SHARE: f32 = 0.05;
// --- the REPORTED-CONFIDENCE family (R23-6) ---------------------------------
// One calibration with two ends, named as one so they can never be retuned
// apart again. They were two bare literals — `(1.0 - err * 6.0).clamp(0.25,
// 0.95)` at the bottom of `fit_recipe_from` and `err_after > 0.12` on the FAR
// warning — and their relationship was invisible: the slope drives confidence
// onto its floor at err = (1 − 0.25)/6 = 0.125, so the FAR line is the same
// point, rounded down. That coincidence is not decoration; it means "the
// number bottomed out" and "the warning fires" are ONE decision expressed
// twice, and R17's real pair sits under BOTH (err_before 0.0947 → 0.0267),
// which is exactly why neither ever fired on the fit the user called
// nonsense. `the_confidence_family_is_one_calibration` pins the relation.
//
// The slope stays 6.0, and since 2026-09-02 that is a MEASURED calibration
// rather than a postponement. This ladder is not the only thing allowed to
// set the number: `recipe.confidence` starts here and is then `min`-ed with
// the joint ladder (`fit_zoned::JOINT_CONFIDENCE_SLOPE`), the evidence
// identifiability cap and the not-same-frame cap, each of which can only
// LOWER it. So the question "is 6.0 right" is only asked on a pair where
// none of the three binds.
//
// Read on every real pair in the corpus, each loaded the way CLI `match`
// loads a RAW (the frame developed at the default recipe, the calibration
// recipe as the base), delivered residual against what the report carries:
//
// | pair    | residual | this ladder | reported |
// |---------|----------|-------------|----------|
// | p40     | 0.033481 | 0.799       | 0.611693 |
// | p41     | 0.041265 | 0.752       | 0.437316 |
// | p38     | 0.053713 | 0.678       | 0.250000 |
// | p39     | 0.054478 | 0.673       | 0.478439 |
// | neutral | 0.099869 | 0.401       | 0.250000 |
// | p36     | 0.105034 | 0.370       | 0.369798 |
// | p37     | 0.161220 | 0.250       | 0.250000 |
//
// Two pairs are where the ladder decides. On `p36` nothing else binds and it
// IS the number, to the last digit the report prints: 1 - 6 x 0.105034 =
// 0.369796 against a reported 0.369798, on a fit that took 0.111648 to
// 0.105034 and claims 0.37 for it — which is the right claim for a fit that
// closed 6% of its gap. On `p37` the residual is past [`FIT_FAR_ERR`], the
// ladder is already on [`CONFIDENCE_FLOOR`], and the reported number is that
// floor. On the other five the ladder is an upper bound one of the three caps
// takes further down, which is also what makes reported confidence
// non-monotone in the residual ACROSS pairs — `p39` reports 0.478 at a WORSE
// residual than `p41`'s 0.437 — and that is the family working as designed
// rather than a slope that needs moving: this ladder proposes, the others can
// only dispose downward.
//
// Retuning would need a pair on which THIS term is the number and the number
// is wrong. On the frame above the corpus had exactly one pair on which this
// term is the number, and it was right. So 6.0 stands, and nothing is waiting
// on it.
//
// THE TABLE IS THE 2026-09-02 FRAME and no longer the corpus's. Two fixes of
// 2026-09-21 moved every residual in it — the base look estimated against the
// picture the camera drew, and the develop window moved onto the DefaultCrop
// rectangle — and re-read on the frame those leave, one of the three caps
// binds on all six `p` pairs, `p36` included: the ladder proposes 0.63 there
// and 0.51 is reported. So the corpus as it stands measures this slope on NO
// pair. That is a fact about the corpus, not an argument for a different
// number, and 6.0 is unchanged: the relation this comment derives is the one
// `the_confidence_family_is_one_calibration` pins, and it holds frame by
// frame. The current residuals are in the ledger (Part 13).

/// Confidence per unit of look error in J* (R5, v1.18.0), derived from the one
/// row of the table above whose value is fixed by definition: `p37`, the pair
/// whose target is ANOTHER photograph. No recipe can fit it, so its confidence
/// must sit on [`CONFIDENCE_FLOOR`]: `1 − s·J*(p37) ≤ 0.25`, i.e.
/// `s ≥ 0.75 / 0.157756 = 4.754`, and 4.8 is the smallest slope at two figures
/// that meets it (`p37` reads 1 − 4.8 × 0.157756 = 0.243 → the floor). The
/// residual is the finished global one, loaded the way CLI `match` loads the
/// pair, at the shipped default strength, read after R8.
///
/// The other rows are the record, not the derivation: their "this ladder"
/// column above was the OLD formula's own output (1 − 6 × display residual),
/// not a measurement of what a fit is worth, so fitting a slope to reproduce
/// it would be circular. Read in J* on the same tree (each row through
/// `pipeline::fit_source`; the neutral row is a calibration JPEG, a baked
/// source):
///
/// | row     | J* residual | at 4.8 |
/// |---------|-------------|--------|
/// | p40     | 0.053340    | 0.744  |
/// | p41     | 0.061116    | 0.707  |
/// | p38     | 0.055669    | 0.733  |
/// | p39     | 0.042468    | 0.796  |
/// | neutral | 0.087423    | 0.580  |
/// | p36     | 0.043867    | 0.789  |
/// | p37     | 0.157756    | 0.250 (floor) |
pub(super) const CONFIDENCE_SLOPE: f32 = 4.8;
/// Never claim less than this — a fit that lands far is still a fit, and 0
/// would read as "broken" rather than "approximate".
pub(super) const CONFIDENCE_FLOOR: f32 = 0.25;
/// Never claim more than this: a statistical match against a non-aligned
/// target is never certain, whatever the residual says.
pub(super) const CONFIDENCE_CEIL: f32 = 0.95;
/// Exponential confidence penalty per mean RGB unit moved where no evidence
/// survived. Calibrated on the generated-sky pair: 0.1186 unsupported motion
/// versus 0.0413 for the preferred saved render.
pub(super) const UNSUPPORTED_MOVEMENT_CONFIDENCE_SLOPE: f32 = 8.0;
/// The FAR line — the residual at which [`CONFIDENCE_SLOPE`] has already
/// driven confidence onto [`CONFIDENCE_FLOOR`] ((1 − 0.25)/4.8 = 0.156 in J*
/// since R5; (1 − 0.25)/6 = 0.125 of the display objective before v1.18.0),
/// rounded down to two figures.
pub(super) const FIT_FAR_ERR: f32 = 0.15;

/// The one clamp both confidence ladders (this module's and the zoned one's)
/// pass through, so the floor and ceiling are stated once.
pub(crate) fn clamp_confidence(v: f32) -> f32 {
    v.clamp(CONFIDENCE_FLOOR, CONFIDENCE_CEIL)
}

/// Confidence from the frame-global look error.
pub(super) fn confidence_from_look_err(err: f32) -> f32 {
    clamp_confidence(1.0 - err * CONFIDENCE_SLOPE)
}
