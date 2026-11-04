//! The cast gates' calibration: neutral misprediction, the foreign-hue veto, the rotation budget, the hue fan, and the reported-confidence family.

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
/// Cast-curve acceptance ANCHOR: the shipped-default value of
/// [`FitBudget::cast_ratio`], which the strength budget interpolates between
/// 1.5 and 3.0. The ratio arm compares the with-curves look error against
/// `budget.cast_ratio` × the without-curves error — but it is not a hard
/// admission threshold and never was: it rejects only when the evidence is
/// also unidentifiable (`identifiability < 0.25`), because a content mismatch
/// masquerading as a cast is a thing you can only diagnose when the pair is
/// too unidentifiable to trust the aggregate. So an ADMITTED cast's ratio may
/// legitimately exceed this number, and any prose that says "the curves cut
/// the error to at most X" is wrong.
///
/// Read straight only by [`cast_gate_outcome`]; the shipped path reads
/// `budget.cast_ratio`, so a test asserting against THIS constant is not
/// asserting against the gate the fit used at any strength but the default.
pub(super) const CAST_ACCEPT_RATIO: f32 = 2.0;

// --- cast foreign-hue veto (the second, pixel-aligned gate) -----------------
// The aggregate ratio above is structurally blind to a CROSS-BAND hue wreck:
// rotate a small pale sky from blue into violet and its band mass lands in
// Purple/Magenta (empty in the target → the hue term's two-sided weight gate
// skips it) while draining out of Blue (below the gate in the fitted render →
// also skipped) — the hue term sees nothing, and the tonal+colour win on the
// frame-dominant region sails the curves through (real-machine canyon
// failure, 2026-07-09; reproduced by
// `warm_rock_cast_must_not_violet_the_pale_sky`).
//
// The veto exploits what the aggregate cannot: the renders WITHOUT and WITH
// the curves come from the SAME source, so "what did the curves do" is
// exact, not statistical. The verdict is by HUE DISTANCE: a pixel the curves
// leave visibly tinted at a hue ≥ [`VETO_FAR_BINS`]·15° away from EVERY hue
// the target populates is FOREIGN — reject the curves when they grow the
// frame's foreign share by ≥ [`VETO_CREATED_SHARE`].
//
// Distance is the discriminator the failure data demanded (both probed on
// the live pairs): the canyon violet lands 60-100° from everything the
// target contains, while the haze correction's imperfect residuals scatter
// pixels only 5-40° off the target's own orange/green/blue mass — so
// family-membership rules at ANY granularity mis-classify one side or the
// other (a ±15° window scored the haze fix 15% "damage"; whole-band shares
// flagged its 35-45° orange-yellow skirt as a phantom yellow family), but a
// 45° = 1.5-ACR-band radius separates them with a 20°+ margin either way.
// Measuring the DELTA against the without-render keeps pre-existing content
// mismatch (already-foreign pixels the curves didn't create) out of the
// verdict.
//
// A cast that rotates a region into a hue the target DOES contain elsewhere
// (sky turned rock-gold) passes this veto BY DESIGN — that failure class is
// covered by the rotation budget below, added when it materialised on a real
// pair (2026-07-09 #2, P21 × reimagine-5: the hazy pale-blue sky was
// re-hued ~170° into the target's own vivid orange; both earlier gates
// passed — the destination hue was target-native and the frame-dominant win
// carried the aggregate).

/// Target pixels feeding the hue-support bins must clear this chroma —
/// deliberately BELOW the 0.06 band-stats gate so a pale sky still testifies.
pub(super) const VETO_SUPPORT_CHROMA: f32 = 0.03;
/// Below this many chromatic target pixels there is no reliable hue evidence
/// (e.g. a monochrome target) — the veto stands down.
pub(super) const VETO_MIN_TARGET_CHROMATIC: usize = 500;
/// A pixel is "visibly tinted" at/above this chroma and enters the foreign
/// census. Below the renderer's HSL fade-in (0.05), but a contiguous 0.04
/// tint over a sky-sized region is visible.
pub(super) const VETO_TINT_CHROMA: f32 = 0.04;
/// A 15° hue bin is "populated" when it holds ≥ this share of the target's
/// chromatic mass. Chroma noise spread over 24 bins stays well under this;
/// any hue region the target actually contains clears it severalfold.
pub(super) const VETO_SUPPORT_BIN_MIN: f32 = 0.015;
/// Foreign radius in 15° bins: ±3 bins = ±45° = 1.5 ACR bands. The canyon
/// violet sits 60°+ from all target hues; the haze residuals sit ≤ 40° from
/// the target's own families — 45° splits them with margin on both sides.
pub(super) const VETO_FAR_BINS: usize = 3;
/// Foreign frame-share the curves must CREATE (with − without) to be
/// rejected: 5% of the frame is a REGION (the canyon sky measures ~12-15%),
/// not boundary speckle (the haze pair measures ≈ 0.04%).
pub(super) const VETO_CREATED_SHARE: f32 = 0.05;

// --- cast rotation budget (the third gate) ----------------------------------
// The foreign-hue veto cannot see a rotation whose DESTINATION the target
// populates (see above). The rotation budget closes that hole from the
// pixel-aligned side alone: a pixel visibly tinted BOTH before and after the
// curves that lands ≥ [`ROT_DEG`] away has been RE-HUED, not corrected; when
// a region-sized share of the frame is re-hued, the curves are a regional
// regrade masquerading as a global cast — reject. Measured on the live pairs
// (calibration probe, 2026-07-09): the accepted haze correction moves 0.01%
// of the frame past 60° (its cast-dominated pixels stay under); the violet
// canyon rotates 12.5% of the frame by 112°; the golden-sky canyon by ~170°.
// 75° sits 15° above the measured-legit ceiling and 37° below the smallest
// observed wreck.
//
// Deliberate cost: a HEAVY global cast (strong tungsten drift) whose honest
// correction would rotate still-tinted pixels past 75° is refused too — the
// fit then under-corrects (tone + saturation only) rather than risk a
// regional re-hue it cannot statistically tell apart. A conservative miss is
// recoverable in the develop panel; a re-hued region is not. True regional
// regrades (a sky genuinely gone gold) belong to the zoned fit, not to
// global curves.

/// Circular hue distance (degrees) beyond which a still-tinted pixel counts
/// as re-hued rather than corrected.
pub(super) const ROT_DEG: f32 = 75.0;
/// Share of re-hued (or blindly moved) pixels that constitutes a REGION of
/// the population a correction moves -- the frame for the global fit, a
/// zone's coverage for a zone (same region-vs-speckle logic as
/// [`VETO_CREATED_SHARE`]; the live wrecks measure 12.5%).
pub(super) const ROT_SHARE: f32 = 0.05;
/// A rotation only counts as a re-hue when it is VISIBLE on at least one
/// end: before-chroma ≥ this (a tinted pixel moved) or after-chroma ≥
/// [`ROT_VISIBLE_AFTER`] (a faint pixel painted vivid — the H17 class). A
/// cast INVERSION passing through neutral flips the hue of a sub-visible
/// tint into another sub-visible tint — measured on the haze pair after the
/// R17 tone-evidence fallback strengthened its correction: 4.5% of the
/// frame, before-chroma < 0.05 to the last pixel, after-chroma ≤ 0.082 —
/// an invisible "rotation" on both ends that ate the veto's whole margin
/// while wrecking nothing. The real wrecks stay above the exemption on the
/// end that matters: H17 paints 0.34 after-chroma from a 0.035 tint; the
/// canyon rotations start from ≥ 0.05 before-chroma.
#[cfg(test)]
pub(super) const ROT_VISIBLE_BEFORE: f32 = 0.05;
/// See [`ROT_VISIBLE_BEFORE`]: the after-side visibility floor — just above
/// the measured pass-through band (≤ 0.082), 3.8× under H17's 0.34, and
/// deliberately NOT higher: every step up widens the exempt band. Rotations
/// inside `cc ∈ [0.03, 0.05) × wc ∈ [0.04, 0.09)` — a faint tint re-hued
/// into another faint tint, invisible on BOTH ends — are exempt by design,
/// and the band's exact borders are pinned by the
/// `the_pass_through_exemption_borders_are_patrolled` fixture.
///
/// How much the exemption actually forgives is measured rather than
/// supposed. Across every calibration pair, on the cast candidate the
/// rotation census reads (2026-09-02), counted the unweighted way
/// [`rehued_share`] counts: the share of analysed pixels that falls inside
/// the band at all, and the share that falls inside it AND turns
/// [`ROT_DEG`] or more —
///
/// | pair    | in band | in band and turned |
/// |---------|---------|--------------------|
/// | neutral | 17.53% | 3.41% |
/// | p36     | 1.32% | 0.26% |
/// | p37     | 3.36% | 3.14% |
/// | p38     | 4.87% | 0.33% |
/// | p39     | 1.22% | 0.39% |
/// | p40     | 10.73% | 0.01% |
/// | p41     | 0.61% | 0.22% |
///
/// The band is not empty and never was: `neutral` hides
/// 3.41% of the frame turning past 75°, which is
/// 0.68× [`ROT_SHARE`] — real forgiveness, at the
/// scale the census would otherwise convict on. That is the trade this pair
/// of floors makes, stated in numbers: those pixels are invisible on both
/// ends, so forgiving them costs a viewer nothing, and the alternative
/// measured in R17 was a veto that ate its whole margin on a haze correction
/// that wrecked nothing. If a pair ever wrecks a region at these chroma
/// levels these floors are where to look — and no pair in this corpus does,
/// because a wreck needs a visible end and the band has none by
/// construction.
#[cfg(test)]
pub(super) const ROT_VISIBLE_AFTER: f32 = 0.09;
/// The BEFORE side of the rotation census needs only a MEASURABLE hue, not a
/// visible tint: requiring [`VETO_TINT_CHROMA`] on both sides let the curves
/// rotate a region whose chroma sat just UNDER that gate (a barely-blue sky
/// at 0.035) into a strong target-native colour without the census ever
/// seeing it — the golden-sky class again, one threshold to the left.
/// CALIBRATED at 0.03 (the [`VETO_SUPPORT_CHROMA`] "a pale sky still
/// testifies" level) by the haze regression itself: at 0.015 the haze
/// correction's legitimately-restored faint pixels measured a 0.0414 census
/// share — 0.83× the firing threshold, margin gone — because hue is
/// genuinely unstable that close to neutral. Below 0.03 chroma the census
/// abstains BY DESIGN, not by omission: colourising near-neutrals is
/// exactly what a corrective cast legitimately does (the haze pair), and
/// at the measured 0.015 level a rotation verdict is demonstrably noise
/// convicting that feature (the 0.83× margin collapse above).
pub(super) const ROT_HUE_MEASURABLE_CHROMA: f32 = 0.03;

// --- the hue-FAN gate (the fourth gate) -------------------------------------
// The three gates above all ask about a pixel's DESTINATION: is it far from
// where it started (rotation budget), is it somewhere the target holds no
// colour (foreign-hue veto), did the aggregate improve (ratio). None of them
// can see the one thing three INDEPENDENT monotone channel maps do that no
// hue-preserving control can: they sort a single-hued region into several
// hues BY LUMINANCE. Each pixel's own rotation stays small, every
// destination is target-native, and the region's mean hue barely moves —
// because the slices rotate in OPPOSITE directions and the circular mean
// cancels them.
//
// Measured on the Cornwall reverse-fit pair (2026-09-01, the defect v1.2.2
// shipped — closed by this gate in v1.2.3 and made structural by the
// terminal re-read in v1.2.4): the admitted curves leave the sky's mean hue at
// 218.3° → 217.6° (0.7°, invisible), rotate no pixel past 75° at all
// (`rehued_share_weighted` = 0.000000, unweighted 0.0058), create 0.000000
// foreign share and cut the look error nearly in half (0.0576 → 0.0334,
// ratio 0.580) — and split the sky's hue across luminance from a 1.6° spread
// to 33.1° in the delivered render: the dark half lands at 226.8° (violet),
// the bright clouds at 193.8° (green-cyan). That is the tint the showcase
// caption reports, and every existing gate reads clean on it.

/// Hue classes for the fan census: the foreign-hue veto's 15° bins, so the
/// two colour gates partition hue on one grid.
///
/// The grid has a fixed PHASE, and that is a stated sensitivity, not an
/// oversight: a coherent region whose hue straddles a class edge splits
/// across two classes, each holding half its mass, and can fall under
/// [`FAN_SHARE`] and go unjudged. The precedent is the foreign-hue veto,
/// which has read hue on this same fixed grid since it was written; a
/// phase-free alternative (sliding windows, or clustering the hue histogram)
/// would buy edge-invariance at the cost of a census whose population is no
/// longer identical to the rotation budget's — and that identity is what
/// stops the two gates drifting into disagreeing about WHICH pixels they
/// read. Cornwall's convicted class sits at 0.917 of the population, nowhere
/// near an edge, so the wreck is not a marginal case of this.
pub(super) const FAN_HUE_CLASSES: usize = 24;

/// A hue class — and a luma slice inside it — must be a REGION of its own
/// population before its mean hue counts as evidence. Deliberately the same
/// number as [`ROT_SHARE`] and [`VETO_CREATED_SHARE`] (0.05 = region, not
/// speckle) and deliberately its own constant: those three share-gated
/// censuses (there are FOUR cast gates; the aggregate-ratio one has no share
/// term) answer different questions, and a retune of one must not silently
/// move the others.
///
/// CALIBRATED by sweep (2026-09-01, `hue_fan_weighted` on the four
/// calibration pairs plus Cornwall). The slice floor is on a plateau here:
/// at 0.02 the readings are Cornwall 37.6° / canyon-warm 9.6° / haze 7.8°,
/// at 0.05 Cornwall 37.6° / canyon-warm 7.5° / haze 7.8°, at 0.10 Cornwall
/// 24.5° / canyon-warm 2.9° / haze 7.8°. 0.05 keeps the wreck's full
/// reading (0.10 loses a third of it to slices that fall under the floor)
/// while reading the same 7.8° on the accepted haze correction — the widest
/// separation of the three.
pub(super) const FAN_SHARE: f32 = 0.05;

/// Added hue spread (degrees) inside one class that convicts the curves.
///
/// CALIBRATED, not chosen, and exactly one hue class wide: the slices have
/// to land in DIFFERENT bins of the census's own 15° grid before a fan is
/// resolvable at all, so anything under this is inside the grid's
/// quantisation and must not convict.
///
/// `hue_fan_weighted` measures, on the analysis raster the fit itself uses:
/// Cornwall (the wreck) 37.6°, the synthetic Cornwall-shape fixture 44.6°,
/// and on the pairs whose curves the other three gates legitimately accept
/// or reject — the haze regression (ACCEPTED, and the one that matters:
/// this gate must not touch it) 7.8°, canyon-warm 7.5°, canyon-gold 5.2°,
/// hazy→vivid 2.7°, an identical pair 0.0°. 15° sits 1.9× above the largest
/// legitimate reading and 2.5× below the wreck.
///
/// The threshold is verified END TO END, not just on the census: at 20° the
/// Cornwall solve does not refuse outright — the mixer's do-no-harm loop
/// halves Aqua/Blue and refits until a milder cast measures 19°, which
/// ships and still leaves a 20.6° fan in the delivered sky (the violet is
/// gone, a pale green in the bright cloud is not). At 15° the refusal
/// stands: the delivered sky's hue spread across luminance octiles is 1.6°,
/// the same coherence the TARGET's sky has, at a look error of 0.058
/// instead of 0.033.
///
/// Deliberate cost, the same shape as the rotation budget's: a cast whose
/// honest correction genuinely needs different hue movement at different
/// luminances (a scene lit by two sources of different colour temperature)
/// is refused too. The fit then under-corrects — tone, saturation and the
/// per-band mixer only — rather than ship a region sorted into a hue fan a
/// user cannot undo with any single develop control.
///
/// WORST CASE, stated because the gate judges the ADDED spread and a reader
/// will otherwise take this for the delivered one. The class is a 15° bin of
/// the BEFORE hue, so the baseline the census subtracts is itself bounded by
/// one class width — and one class width IS this number. An admitted cast
/// can therefore leave up to `2 × FAN_DEG` ≈ 30° of ABSOLUTE hue spread
/// inside the class, when the class arrived already spread across its own
/// bin and the curves add just under the limit on top. That is the gate's
/// tolerance, and it is asserted rather than promised: see
/// `an_admitted_cast_delivers_at_most_two_class_widths_of_hue_fan`, which
/// pins the admitted haze pair's DELIVERED in-class spread under 2 ×
/// FAN_DEG.
///
/// It is also a calibrated threshold and not a structural guarantee: the
/// mixer's do-no-harm loop re-fits after every shrink, so the solve can
/// search for a cast that clears the limit rather than give the cast up —
/// which is exactly what the 20° experiment above shows it doing.
pub(super) const FAN_DEG: f32 = 15.0;

/// The fan a PROJECTED cast must clear — half [`FAN_DEG`], deliberately not
/// [`FAN_DEG`] itself.
///
/// [`FAN_DEG`] is the REFUSAL line and it sits at the visibility edge: the
/// FAN_DEG = 20 experiment shipped a cast measuring 19° that left 20.6° of
/// fan in the delivered sky (the violet gone, a pale green in the bright
/// cloud not), and the widest reading the gate admits on its own merits is
/// the haze correction's 7.8°. A cast the fit CHOOSES to keep by shrinking
/// it is not entitled to sit on the edge the gate merely tolerates — it has
/// to be no worse than what the gate already passes unprojected. Half the
/// refusal line is 7.5°, just under that 7.8°: THAT is the calibration.
///
/// Both candidate targets were measured end to end on the Cornwall pair
/// before this number was fixed (2026-09-02, global stage, `match` without
/// `--zoned`), because "shrink until the fan clears 15°" is the obvious
/// alternative and it had to be refuted with numbers rather than taste:
///
/// | target | t | census fan after | look error | confidence | DELIVERED sky spread |
/// |--------|------|------|---------------|--------|--------|
/// | 7.5° (shipped) | 0.363 | +7° | 0.137 → 0.030 | 0.664 | 10.5° |
/// | 15° | 0.483 | +14° | 0.137 → 0.026 | 0.680 | 15.3° |
///
/// against the target's own 1.6°, the refuse-outright branch's 1.6° at look
/// error 0.058, and v1.2.2's 33.1° at 0.033. The looser target buys 0.004 of
/// look error by delivering half again as much fan as the visibility
/// calibration allows, so the conservative constant stands.
///
/// See [`projected_cast_curves`] for the path and [`search_cast_projection`]
/// for the search.
pub(super) const FAN_PROJECT_DEG: f32 = FAN_DEG / 2.0;
/// Cells the projection's gain sweep divides the admissible interval into.
/// The gain wiggles over `t` — 0.00104 / 0.00190 / 0.00169 / 0.00187 at
/// `t` 0.25 / 0.35 / 0.40 / 0.50 on the coast candidate — so the grid has to
/// be fine enough to bracket an interior peak, and every probe costs a full
/// render, so it is not made finer than the structure it has to see. Eight
/// cells put each probe 0.05 of `t` apart on a frontier at 0.4, half the
/// spacing of that measured wiggle.
pub(super) const PROJECT_GRID: usize = 8;
/// Golden-section iterations on the winning cell. Each iteration multiplies
/// the bracket by 0.618, so eight of them take a cell 0.05 wide down to
/// 0.0011 of `t` — just past the third decimal the projection note prints,
/// which is as far as refining can change anything the user reads.
pub(super) const PROJECT_REFINE: usize = 8;

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
