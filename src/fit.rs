//! Reverse-fit ("match") — derive an editable [`EditRecipe`] from a LOOK.
//!
//! Given the same shot twice — the untouched source preview and a target
//! rendition of it (the gpt-image `reimagine` output, or any finished reference
//! of the SAME frame) — solve for the develop parameters that reproduce the
//! target's tonality and colour through OUR deterministic engine. No pixels are
//! copied: the output is sliders + curves, so it applies at full sensor
//! resolution and serialises to a Lightroom XMP sidecar. This is how a low-res
//! generative experiment becomes a real, adjustable, full-resolution develop.
//!
//! Method: STATISTICS, not per-pixel regression — a generative target is NOT
//! pixel-aligned with the source (the model re-renders the frame), so only
//! distribution-level evidence is trustworthy.
//!
//!   1. **Tone** — luminance CDF matching gives a monotone map `M`; sample it at
//!      the engine's own tone knots ([`render::TONE_KNOTS_X`]) and least-squares
//!      solve the sliders against the engine's OWN basis
//!      ([`render::tone_slider_basis`]), scanning exposure (it enters the model
//!      nonlinearly). The solve carries a REAL magnitude prior (ridge +
//!      penalised model selection): the knot system is near-collinear, so
//!      without it grotesque mutually-cancelling combos (Exposure +1.5 with
//!      Contrast −97 and Shadows −100) beat tasteful ones by numerical ε —
//!      the residual curve makes their total maps indistinguishable, but the
//!      slider semantics are ruined (real-photo failure, 2026-07-07). Whatever
//!      shape the penalised sliders don't express goes into `tone_curve`
//!      control points, which the engine composes exactly on top.
//!   2. **Saturation** — global mean-chroma ratio, secant-refined through real
//!      [`render::develop_preview`] renders (closed loop, not open-loop math).
//!      Chroma matching against a non-aligned target is a heuristic, so the
//!      pipeline ends with a DO-NO-HARM check: if the finished recipe renders
//!      farther from the target than the untouched source, saturation is
//!      halved (cast curves refit each step — they depend on the saturated
//!      state) until the end-to-end error stops objecting (the 2026-07-09
//!      golden-sky pair dragged the chroma chase to the cap and rendered
//!      worse than doing nothing). Saturation cannot be judged mid-pipeline:
//!      a correct value legitimately amplifies a latent cast that the curve
//!      stage then removes.
//!   3. **Colour cast** — per-channel CDF residuals → red/green/blue curves,
//!      last as the catch-all (cast-before-saturation measured worse on the
//!      haze regression — see the stage comments in [`fit_recipe`]). Accepted
//!      only through THREE gates: the aggregate look-error ratio, the
//!      foreign-hue veto (the curves must not paint a region of the frame in
//!      hues the target holds nowhere — the 2026-07-09 violet-sky failure was
//!      cross-band invisible to the aggregate) and the rotation budget (nor
//!      re-hue a region into hues the target DOES hold — the golden-sky
//!      failure passed both earlier gates; see the veto const blocks).
//!
//! There is deliberately NO per-band HSL stage — per-band statistics against
//! a non-pixel-aligned generative target conflate content with style and are
//! unidentifiable (see the note in [`fit_recipe`]; it caused the 2026-07-07
//! purple-sky failure).
//!
//! Every stage fits the RESIDUAL against a fresh render of the current recipe,
//! so stage interactions are absorbed instead of compounding; the report carries
//! the honest before/after distribution error (tonal + channel means + per-band
//! hue, so a hue disaster cannot hide behind matched luma quantiles). Local
//! masks and content changes are out of scope by construction (statistics
//! cannot localise them) — the AI style-prompt path covers intent the numbers
//! cannot.

use image::DynamicImage;

use crate::recipe::{CurvePoint, EditRecipe};
use crate::render;

/// Analysis resolution (long edge). CDFs and band means are stable well below
/// this; keeping it small keeps the closed-loop renders interactive-fast
/// (5 in the common path; up to ~20 if the do-no-harm loop shrinks
/// saturation, each a 384-px develop).
pub(crate) const ANALYZE_EDGE: u32 = 384;
const HIST_BINS: usize = 1024;
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
/// Anchors, all measured. Fall-back side: _DSC9608 × reimagine reads 0.021
/// (the pale sky, luma q50 ≈ 197/255, re-hued vivid blue out of the class;
/// share ratio 1.29× sailed under the 1.75× gate; the shipped map missed
/// the shared class by −22/255 right in the murk band); the archived
/// _DSC9621 pairs read 0.074 (× reimagine, the golden sky — share 2.65×,
/// both gates fire), 0.034 (× reimagine-4 — share 1.51×, UNDER the share
/// gate: this detector is the only defence) and 0.024 (× reimagine-2,
/// share 1.92×); the haze fixture reads 0.126 (its blue cast tints the
/// clean side's dark greys out of the class — under the R17 dense residual
/// knots the gated solve faithfully implements that broken map and
/// collapses to a do-no-harm reset, while the fallback lands
/// 0.0892 → 0.0229). Keep side: _DSC9621 × reimagine-3 — a REAL benign
/// pair — reads 0.0050 (share 1.12×), the identity / canyon fixtures read
/// ≈ 0 (matched members) and the synthetic uniform-inflation fixture reads
/// < 0.0075. The 0.015 ceiling thus has real pairs on BOTH flanks: 3.0×
/// clear below (0.0050), 1.4× above (0.021). The archive numbers above are
/// the embedded-preview domain (`decode` output vs reimagine target — the
/// camera-look source the CLI `match` feeds this gate); the GUI's
/// composed-calibration domain was measured separately for all five real
/// pairs (R19, the repro test prints it per pair) and each verdict lands
/// on the same side of the ceiling in both domains — composed readings:
/// _DSC9608 × re2 0.024, _DSC9621 × re 0.043, × re2 0.033, × re4 0.036
/// (fall-back side), × re3 0.0131 (keep side, a 1.15× margin against the
/// preview domain's 3.0×). The haze fixture is a recipe-render pair with
/// no RAW, so a composed domain does not exist for it.
const NEUTRAL_MISPREDICTION_MAX: f32 = 0.015;
/// Evidence floor for the SHARED class inside
/// [`neutral_gate_misprediction`] — the same absolute floor the per-side
/// `enough` bar uses (512 px), plus the same 5%-of-frame scaling, applied
/// to the one population the identification assumption is actually about.
/// Below it there is no identified population to score and the metric
/// reports infinite (fall back).
const NEUTRAL_SHARED_MIN: usize = 512;
/// Cast-curve acceptance: the fitted per-channel curves must cut the hue-aware
/// look error to ≤ this fraction of the without-curves error, else they are
/// rejected as a content mismatch masquerading as a cast (see the stage-4
/// comment in [`fit_recipe`]). A true global cast slashes the error far past
/// this; a content difference only nibbles at it while damaging regions.
const CAST_ACCEPT_RATIO: f32 = 0.85;

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
// pair (2026-07-09 #2, _DSC9621 × reimagine-5: the hazy pale-blue sky was
// re-hued ~170° into the target's own vivid orange; both earlier gates
// passed — the destination hue was target-native and the frame-dominant win
// carried the aggregate).

/// Target pixels feeding the hue-support bins must clear this chroma —
/// deliberately BELOW the 0.06 band-stats gate so a pale sky still testifies.
const VETO_SUPPORT_CHROMA: f32 = 0.03;
/// Below this many chromatic target pixels there is no reliable hue evidence
/// (e.g. a monochrome target) — the veto stands down.
const VETO_MIN_TARGET_CHROMATIC: usize = 500;
/// A pixel is "visibly tinted" at/above this chroma and enters the foreign
/// census. Below the renderer's HSL fade-in (0.05), but a contiguous 0.04
/// tint over a sky-sized region is visible.
const VETO_TINT_CHROMA: f32 = 0.04;
/// A 15° hue bin is "populated" when it holds ≥ this share of the target's
/// chromatic mass. Chroma noise spread over 24 bins stays well under this;
/// any hue region the target actually contains clears it severalfold.
const VETO_SUPPORT_BIN_MIN: f32 = 0.015;
/// Foreign radius in 15° bins: ±3 bins = ±45° = 1.5 ACR bands. The canyon
/// violet sits 60°+ from all target hues; the haze residuals sit ≤ 40° from
/// the target's own families — 45° splits them with margin on both sides.
const VETO_FAR_BINS: usize = 3;
/// Foreign frame-share the curves must CREATE (with − without) to be
/// rejected: 5% of the frame is a REGION (the canyon sky measures ~12-15%),
/// not boundary speckle (the haze pair measures ≈ 0.04%).
const VETO_CREATED_SHARE: f32 = 0.05;

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
const ROT_DEG: f32 = 75.0;
/// Frame share of re-hued pixels that constitutes a REGION (same region-vs-
/// speckle logic as [`VETO_CREATED_SHARE`]; the live wrecks measure 12.5%).
const ROT_SHARE: f32 = 0.05;
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
const ROT_VISIBLE_BEFORE: f32 = 0.05;
/// See [`ROT_VISIBLE_BEFORE`]: the after-side visibility floor — just above
/// the measured pass-through band (≤ 0.082), 3.8× under H17's 0.34, and
/// deliberately NOT higher: every step up widens the exempt band. Rotations
/// inside `cc ∈ [0.03, 0.05) × wc ∈ [0.04, 0.09)` — a faint tint re-hued
/// into another faint tint, invisible on BOTH ends — are exempt by design,
/// and the band's exact borders are pinned by the
/// `the_pass_through_exemption_borders_are_patrolled` fixture; if a real
/// pair ever wrecks a region at those chroma levels, this pair of floors
/// is the suspect.
const ROT_VISIBLE_AFTER: f32 = 0.09;
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
const ROT_HUE_MEASURABLE_CHROMA: f32 = 0.03;

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
// The slope stays 6.0: this round does not retune the look-error ladder (it
// would need the real failure pair that has not arrived). What changes is
// that this ladder is no longer the ONLY thing allowed to set the number —
// see `fit_zoned::JOINT_CONFIDENCE_SLOPE`, which can only lower it.

/// Confidence per unit of look error.
const CONFIDENCE_SLOPE: f32 = 6.0;
/// Never claim less than this — a fit that lands far is still a fit, and 0
/// would read as "broken" rather than "approximate".
const CONFIDENCE_FLOOR: f32 = 0.25;
/// Never claim more than this: a statistical match against a non-aligned
/// target is never certain, whatever the residual says.
const CONFIDENCE_CEIL: f32 = 0.95;
/// The FAR line — the residual at which [`CONFIDENCE_SLOPE`] has already
/// driven confidence onto [`CONFIDENCE_FLOOR`] ((1 − 0.25)/6 = 0.125),
/// rounded down to a legible number.
const FIT_FAR_ERR: f32 = 0.12;

/// The one clamp both confidence ladders (this module's and the zoned one's)
/// pass through, so the floor and ceiling are stated once.
pub(crate) fn clamp_confidence(v: f32) -> f32 {
    v.clamp(CONFIDENCE_FLOOR, CONFIDENCE_CEIL)
}

/// Confidence from the frame-global look error.
fn confidence_from_look_err(err: f32) -> f32 {
    clamp_confidence(1.0 - err * CONFIDENCE_SLOPE)
}

/// Aspect-ratio disagreement past which the two frames are unlikely to be
/// the same shot (R23-6 B-7). 2% mirrors the grid-comparability rule inside
/// [`neutral_gate_misprediction`] — a few rows of a 384-edge thumbnail, i.e.
/// beyond what aspect rounding and a sane crop explain. A WARNING, never a
/// refusal: the reference is a file the user chose on purpose, and a fit
/// between two shots of the same scene is unreliable, not illegal.
const SAME_FRAME_ASPECT_TOL: f32 = 0.02;

/// Do these two images plausibly show the SAME frame? `false` ⇒ warn.
///
/// Two cheap readings, in the order that costs least: the aspect ratios, and
/// then the grid comparability [`neutral_gate_misprediction`] already
/// computes (it returns infinity when the two analysis grids differ by more
/// than aspect rounding). Both are necessary conditions, neither is
/// sufficient — a different photograph of the same scene at the same aspect
/// passes, and nothing short of registration would catch it.
pub fn same_frame_plausible(src: &DynamicImage, target: &DynamicImage) -> bool {
    let ar = |w: u32, h: u32| w.max(1) as f32 / h.max(1) as f32;
    let (a, b) = (ar(src.width(), src.height()), ar(target.width(), target.height()));
    (a - b).abs() <= SAME_FRAME_ASPECT_TOL * a.max(b)
}

/// The fit outcome: the recipe plus the distribution error (mean |Δ| over luma
/// quantiles and channel means, 0 = identical look) before and after.
pub struct FitReport {
    pub recipe: EditRecipe,
    pub err_before: f32,
    pub err_after: f32,
    /// The rationale as typed notes (L12#2B): `render_en(&notes)` is the
    /// recipe's `rationale` byte-for-byte (empty prose prefix — the fit
    /// rationale is fully deterministic), so the GUI renders it localized
    /// while every persisted surface keeps the English string. In-process
    /// only, never serialized.
    pub notes: Vec<crate::rationale::Note>,
}

/// Fit an [`EditRecipe`] mapping `src` (untouched preview) onto the look of
/// `target` (a rendition of the same frame). Deterministic, no network.
pub fn fit_recipe(src: &DynamicImage, target: &DynamicImage) -> FitReport {
    fit_recipe_from(src, target, &EditRecipe::default())
}

/// [`fit_recipe`] with the photo's CALIBRATION composed into the solve
/// (R16). `base` is a calibration-only recipe — base curve, lens profile,
/// as-shot anchors, NO user edits: the returned recipe STARTS from it, so
/// every closed-loop candidate render develops source → candidate in the
/// same one-pass `user(base(x))` the canvas uses (the v0.24.0 two-pass
/// seed's clamp-order gap is gone by construction, and the residual
/// numbers describe exactly the render the user sees). Statistics are
/// measured against the BASE render, so the bounded stages solve only the
/// base-look → target delta; the tone stage solves its sliders in the
/// user domain (their input IS the base output) and the residual curve in
/// the full-LUT domain via the base LUT. With a default `base` this is
/// bit-for-bit the old fit.
pub fn fit_recipe_from(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
) -> FitReport {
    // CALLER CONTRACT: `base` must be calibration-only (build it with
    // `pipeline::calibration_recipe`, or pass the default). A base smuggling
    // user edits (curves/masks/sliders) breaks the residual algebra AND the
    // reset arm's `err_after = err_before` identity — debug-checked here;
    // release trusts the two in-crate callers, both correct by construction.
    debug_assert!(
        base.tone_curve.is_empty() && base.masks.is_empty() && base.red_curve.is_empty(),
        "the fit base must be a calibration-only recipe"
    );
    // R23-6 B-7: the reference no longer has to be an in-app generated
    // variant, so "is this even the same photograph?" is now a question the
    // fit can be asked. Measured BEFORE the thumbnails, which normalise the
    // long edge and would hide a shape mismatch. A warning only — see
    // [`same_frame_plausible`].
    let same_frame = same_frame_plausible(src, target);
    let s_img = src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
    let t_img = target.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
    // The base render IS the reference domain: err_before is "calibration
    // look vs target" and every statistic below describes the delta the
    // solve must close. All-default base ⇒ this is the raw thumbnail.
    let s_base = render::develop_preview(&s_img, base);
    let sp = pixels_of(&s_base);
    let tp = pixels_of(&t_img);
    let err_before = look_err(&sp, &tp);

    // A DEGENERATE pair carries no tone evidence: on a zero-variance source
    // or target (lens-cap frame, blank card, an empty crop) the inverse CDF
    // answers 1.0 everywhere, the fitted tone map collapses to a constant —
    // and look_err of the same frame against itself scores that garbage 0,
    // so it used to be ACCEPTED with no hint (L06-1/2). Refuse to fit: a
    // neutral recipe plus the reason, through the same rationale channel
    // sat_pegged uses.
    if luma_variance(&sp) < DEGENERATE_LUMA_VAR || luma_variance(&tp) < DEGENERATE_LUMA_VAR {
        let mut rationale = String::new();
        let mut notes: Vec<crate::rationale::Note> = Vec::new();
        crate::rationale::push_note(
            &mut rationale,
            &mut notes,
            crate::rationale::Note::plain(crate::rationale::keys::FIT_DEGENERATE),
        );
        // The refusal still carries the calibration: a degenerate pair must
        // not strip the camera look off the deliverable. Clamped like the
        // success path — the persisted JSON stays canonical (review R16 #2).
        let mut recipe = EditRecipe { rationale, ..base.clone() };
        recipe.clamp();
        return FitReport { recipe, err_before, err_after: err_before, notes };
    }

    let mut recipe = base.clone();

    // --- 1) tone: exposure scan × linear solve on the engine's knot basis ----
    // Tone evidence comes from NEAR-NEUTRAL pixels: saturated pixels clip
    // channels at the gamut ceiling under chroma scaling, so their luma lands
    // short of the tone map and would bias the solve (measured: one polluted
    // knot skews contrast by tens of points). Greys carry clean evidence.
    let (s_cdf, t_cdf) = tone_cdf_pair(&sp, &tp);
    let tone_map = |x: f32| quantile(&t_cdf, cdf_at(&s_cdf, x).clamp(P_CLIP, 1.0 - P_CLIP));
    let (ev, sliders) = fit_tone_sliders(&tone_map);
    recipe.exposure_ev = round2(ev);
    recipe.contrast = round1(sliders[0] * 100.0);
    recipe.highlights = round1(sliders[1] * 100.0);
    recipe.shadows = round1(sliders[2] * 100.0);
    recipe.whites = round1(sliders[3] * 100.0);
    recipe.blacks = round1(sliders[4] * 100.0);

    // --- 2) residual master curve (composed on top of the sliders) -----------
    // Domain care (R16): the sliders solved in the USER domain (their input
    // is the base curve's output — `tone_map` above maps base-render luma to
    // target luma), but `residual_tone_curve` samples the recipe's FULL LUT,
    // whose input is the NEUTRAL domain. Rebase the map through the base
    // LUT: full(x) = user_map(base(x)). An EMPTY base curve skips the rebase
    // outright — build_tone_lut's sRGB↔linear round trip is only ~1e-7 from
    // identity, but skipping keeps the default-base wrapper literally
    // bit-for-bit the old fit (review R16 #3).
    let base_lut = render::build_tone_lut(base);
    let full_map = |x: f32| {
        if base.base_curve.is_empty() { tone_map(x) } else { tone_map(render::sample_lut(&base_lut, x)) }
    };
    recipe.tone_curve = residual_tone_curve(&recipe, &full_map);

    // --- 3) global saturation, secant-refined through the real engine --------
    // Saturation stays BEFORE the cast curves: channel CDFs of a desaturated
    // render differ from the target's even with zero cast (each channel's
    // distribution is compressed toward luma), so fitting the cast first
    // would express chroma expansion through per-channel curves — and
    // per-channel curves rotate hue. Saturating first may amplify a latent
    // cast, but stage 5 fits the cast residual CLOSED-LOOP on the saturated
    // render, so it is measured and removed rather than compounded.
    let t_chroma = mean_chroma(&tp);
    let mut sat_pegged = false;
    for _ in 0..2 {
        let cur = pixels_of(&render::develop_preview(&s_img, &recipe));
        let c_chroma = mean_chroma(&cur);
        if c_chroma < 1e-4 {
            break;
        }
        let step = ((t_chroma / c_chroma - 1.0) * 100.0).clamp(-40.0, 40.0);
        if step.abs() < 1.0 {
            break;
        }
        let want = recipe.saturation + step;
        let clamped = want.clamp(-60.0, 60.0);
        // Hitting the model cap with demand to spare = the target's chroma is
        // out of the global model's reach — flagged into the rationale so the
        // user learns WHY the fit stays approximate.
        if (want - clamped).abs() > 0.5 {
            sat_pegged = true;
        }
        recipe.saturation = round1(clamped);
    }
    // NOTE deliberately NO validation here: a correct saturation legitimately
    // makes every colour metric worse at THIS point in the pipeline (it
    // amplifies a latent cast into the channel means and the hue bands; the
    // curve stage then measures and removes it — see the ordering comment
    // above). The only fair evaluation point is the finished recipe: the
    // do-no-harm check after stage 4 shrinks saturation if the END result
    // regressed. (A stage-local gate was tried first and it zeroed the haze
    // regression's saturation, degrading the whole fit.)

    // --- 4) per-channel colour-cast curves — the catch-all, LAST so its
    // closed-loop residual sees every earlier stage's composed output
    // (cast-before-saturation was tried and measured worse on the haze
    // regression: chroma expansion leaks into the curves, which rotate hue).
    //
    // The curves model a GLOBAL cast (one monotone map per channel). That
    // model is exactly right for uniform casts (haze tint, WB drift) and
    // exactly wrong when the colour residual is CONTENT (a generative
    // target's rocks simply ARE warmer than its sky): then the fitted map
    // drags every region — measured on the real pair, the red lift that
    // warmed the frame-dominant rocks turned the pale sky violet (and the
    // neutral-only-evidence variant, also tried, cooled the warm distance
    // haze instead). The two worlds are told apart by VALIDATION, not by
    // evidence filtering: accept the curves only if they improve the
    // hue-aware look error by a clear margin — a global map that truly
    // explains the residual slashes the error (the haze regression), while
    // a content mismatch yields a marginal "improvement" bought by regional
    // hue damage the metric's hue term partially sees. Marginal gain does
    // not earn regional risk: keep the recipe clean instead.
    //
    // Deliberately NO per-band HSL fitting. It was tried (centroid hue
    // deltas + sat/luma ratios per ACR band, correspondence-gated) and it is
    // what wrecked the real-photo fit (2026-07-07): against a generative,
    // non-pixel-aligned target, a band's centroid delta conflates CONTENT
    // difference with style, and an honest-looking 13° in-gate delta applied
    // as a whole-band rotation turns brown rock olive and a pale sky
    // lavender. Per-band intent is statistically unidentifiable here — like
    // local masks, it belongs to the AI style-prompt path, not to
    // distribution matching.
    let fit_cast_stage = |recipe: &mut EditRecipe| -> CastOutcome {
        recipe.red_curve = Vec::new();
        recipe.green_curve = Vec::new();
        recipe.blue_curve = Vec::new();
        let cur = pixels_of(&render::develop_preview(&s_img, recipe));
        let err_without = look_err(&cur, &tp);
        recipe.red_curve = residual_channel_curve(&cur, &tp, 0);
        recipe.green_curve = residual_channel_curve(&cur, &tp, 1);
        recipe.blue_curve = residual_channel_curve(&cur, &tp, 2);
        let mut out = CastOutcome::default();
        if !(recipe.red_curve.is_empty()
            && recipe.green_curve.is_empty()
            && recipe.blue_curve.is_empty())
        {
            let with_px = pixels_of(&render::develop_preview(&s_img, recipe));
            // Three gates, all must pass: the aggregate ratio (a marginal win
            // does not earn regional risk), the foreign-hue veto (a large
            // aggregate win does not earn a region painted in hues the target
            // holds nowhere) and the rotation budget (nor a region re-hued
            // into hues it does hold — golden-sky case). The vetoes only ever
            // reject, never rescue.
            out.ratio_rejected = look_err(&with_px, &tp) > err_without * CAST_ACCEPT_RATIO;
            out.rehue_blocked = cast_paints_foreign_hues(&cur, &with_px, &tp)
                || cast_rotates_a_region(&cur, &with_px);
            if out.ratio_rejected || out.rehue_blocked {
                recipe.red_curve = Vec::new();
                recipe.green_curve = Vec::new();
                recipe.blue_curve = Vec::new();
            }
        }
        out
    };
    let mut cast = fit_cast_stage(&mut recipe);

    // --- 4b) do-no-harm — the pipeline-END check ------------------------------
    // Goal: don't hand back a recipe that renders FARTHER from the target
    // than the untouched source. Saturation is the one dial fitted by
    // heuristic (mean-chroma chase) rather than by a validated residual, and
    // it cannot be judged mid-pipeline (see the stage-3 note), so when the
    // finished recipe regresses, halve saturation — refitting the cast curves
    // each step, they depend on the saturated state — until the end-to-end
    // error stops objecting. Saturation is the only shrinkable dial here: if
    // the regression persists at zero, it is reported honestly through
    // err_after/confidence rather than hidden. NOTE the case that motivated
    // this loop (golden-sky pair: a distorted tone map made the whole fit
    // regress) was root-fixed by `tone_cdf_pair`, and no current fixture
    // reaches the loop body — it stays as insurance for pair geometries we
    // have not seen, because the saturation heuristic remains unvalidated by
    // construction. If you find a triggering pair, pin it in the tests.
    let sat_fitted = recipe.saturation;
    let mut err_after = look_err(&pixels_of(&render::develop_preview(&s_img, &recipe)), &tp);
    while err_after > err_before + 1e-4 && recipe.saturation != 0.0 {
        let next = if recipe.saturation.abs() < 4.0 { 0.0 } else { recipe.saturation / 2.0 };
        recipe.saturation = round1(next);
        cast = fit_cast_stage(&mut recipe);
        err_after = look_err(&pixels_of(&render::develop_preview(&s_img, &recipe)), &tp);
    }
    let sat_reduced = recipe.saturation != sat_fitted;
    // TERMINAL do-no-harm: saturation is the loop's only shrinkable dial, so
    // it can exhaust at zero with the finished recipe STILL rendering farther
    // from the target than the untouched source (the tone/curve stages have
    // no shrink path). Handing that back violates the check's own promise —
    // return neutrality instead, with the honest numbers in the report.
    let mut fit_regressed = false;
    // Tolerate the fit's OWN quantisation, not a fixed error size. The
    // rounded sliders, the 8-bit residual tone curve and the f32 develop
    // round trip cost about 1e-3 of residual even on an IDENTICAL pair, where
    // err_before is exactly 0 — so a bare +1e-4 margin fired there, wiping a
    // perfectly good near-neutral solve and reporting "outside the global
    // model's reach" directly beneath a printed residual of 0.000 -> 0.000.
    //
    // A flat FLOOR is the wrong correction though: it would also wave through
    // a fit that is genuinely worse whenever both numbers are small
    // (err_before 0.0010 -> err_after 0.0029 is nearly 3x worse, and no
    // absolute floor below 0.003 catches it). Scale with the error instead
    // and add the quantisation budget once.
    // The SECOND reading, taken here because here is where "the finished
    // recipe against the untouched base" is the question (R23-6, feedback
    // #16). `joint_base` describes doing nothing; `joint_after` describes
    // shipping this recipe. Both are `None` when the family has no opinion —
    // fail-open, and every use below is written so `None` changes nothing.
    let joint_base = crate::fit_zoned::joint_reading(&sp, &tp);
    let mut after_px = pixels_of(&render::develop_preview(&s_img, &recipe));
    let mut joint_after = crate::fit_zoned::joint_reading(&after_px, &tp);
    let harm = terminal_harm(err_before, err_after, joint_base, joint_after);
    let joint_regressed = harm.joint;
    if harm.any() {
        // Reset to the BASE, not to a bare default (R16): "do no harm" means
        // degrading to the calibration look the canvas would show with no
        // fit at all — a bare default would re-introduce the dark neutral
        // the base exists to avoid. By definition that render IS the
        // err_before measurement, so no re-render is needed.
        recipe = base.clone();
        err_after = err_before;
        fit_regressed = true;
        // …and so is the render, and so is its joint reading.
        after_px = sp.clone();
        joint_after = joint_base;
    }

    // --- report ---------------------------------------------------------------
    // Honest-mismatch notes: the user reads WHY a fit stayed approximate
    // instead of wondering what went wrong (real-machine feedback,
    // 2026-07-09: a palette-transplant target produced a faithful-but-ugly
    // max-saturation fit with zero explanation).
    use crate::rationale::{keys, push_note, Note};
    let mut notes: Vec<Note> = Vec::new();
    let mut rationale = String::new();
    // The summary comes first; the note fragments append after it. Two full
    // summary keys instead of a nested English fragment argument — a
    // fragment inside an arg would stay English in the zh rendering.
    let summary_key = if recipe.tone_curve.is_empty() {
        keys::FIT_SUMMARY_NO_CURVE
    } else {
        keys::FIT_SUMMARY_WITH_CURVE
    };
    push_note(
        &mut rationale,
        &mut notes,
        Note::new(
            summary_key,
            vec![
                ("err_before", format!("{err_before:.3}")),
                ("err_after", format!("{err_after:.3}")),
            ],
        ),
    );
    // Keyed on the RESIDUAL, not the pre-fit distance: a large but perfectly
    // fittable tone gap (2 EV of exposure) starts far and ends near — only a
    // look the model cannot approach deserves the warning.
    if err_after > FIT_FAR_ERR {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(keys::FIT_NOTE_FAR, vec![("err_after", format!("{err_after:.2}"))]),
        );
    }
    // The joint value-range reading, ALWAYS reported when it has one: it is
    // the only number in this report that `look_err` did not produce, and
    // burying it behind a threshold would leave the user with a single
    // self-graded score again. Named "joint distribution", never "region" —
    // the buckets are value ranges whose pixels are scattered frame-wide.
    if let Some(j) = joint_after {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_JOINT,
                vec![
                    ("weighted", format!("{:.3}", j.weighted)),
                    ("worst", format!("{:.3}", j.worst)),
                    ("label", j.worst_label.to_string()),
                    ("n", j.buckets.to_string()),
                ],
            ),
        );
        if j.weighted >= crate::fit_zoned::JOINT_FAR_ERR {
            push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_JOINT_FAR));
        }
    } else {
        // FAIL-OPEN, disclosed. "No opinion" and "no problem" are different
        // claims and must not read the same (E-15): with no second reading
        // the confidence below is the look-error ladder on its own.
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_JOINT_NONE));
    }
    if sat_pegged {
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_SAT_PEGGED));
    }
    if fit_regressed {
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_REGRESSED));
        if joint_regressed {
            // WHICH check refused matters: the scalar arm and this one see
            // different damage, and "the value ranges drifted" is actionable
            // where "it rendered farther" is not.
            push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_JOINT_REGRESSED));
        }
    } else if sat_reduced {
        push_note(
            &mut rationale,
            &mut notes,
            Note::new(
                keys::FIT_NOTE_SAT_REDUCED,
                vec![
                    ("sat_fitted", format!("{sat_fitted:+.0}")),
                    ("sat_now", format!("{:+.0}", recipe.saturation)),
                ],
            ),
        );
    }
    if let Some(k) = cast.note_key() {
        push_note(&mut rationale, &mut notes, Note::plain(k));
    }
    // Which controls this target's look may need that the solver has no way
    // to reach — SPECIFIC to this pair, not the blanket sentence the summary
    // already carries (R23-6 A-5).
    if let Some(n) = unrepresented_note(&recipe, &after_px, &tp, err_after) {
        push_note(&mut rationale, &mut notes, n);
    }
    if !same_frame {
        push_note(&mut rationale, &mut notes, Note::plain(keys::FIT_NOTE_NOT_SAME_FRAME));
    }
    recipe.rationale = rationale;
    // Confidence: the look-error ladder, and never MORE than the joint
    // reading's own ladder allows. One-directional on purpose — a reading
    // that cannot see (`None`) must not raise a claim, and the two metrics
    // disagreeing means the honest answer is the lower one. On the fixture
    // set this is what finally separates a fit that reproduces the look from
    // one that only scores well: the unreachable-repaint pair reads 0.52 by
    // look error and 0.25 here.
    recipe.confidence = match joint_after {
        Some(j) => confidence_from_look_err(err_after).min(clamp_confidence(
            1.0 - j.weighted * crate::fit_zoned::JOINT_CONFIDENCE_SLOPE,
        )),
        None => confidence_from_look_err(err_after),
    };
    recipe.clamp();
    FitReport { recipe, err_before, err_after, notes }
}

/// Re-measure a recipe the way [`fit_recipe_from`] measures its own output:
/// the frame-global look distance, and the confidence both ladders agree on.
/// Returns `(err, confidence)`.
///
/// Exposed for the ONE caller that legitimately hands back a recipe it did
/// not itself solve: the GUI's deep reverse-fit (R23-6 D) may adjust a
/// fitted recipe on the visual reviewer's say-so, and reporting the solve's
/// pre-adjustment numbers next to post-adjustment pixels would be exactly
/// the kind of stale claim this round is about. Deterministic and local —
/// the same two renders the fit already pays for.
pub fn rescore(src: &DynamicImage, target: &DynamicImage, recipe: &EditRecipe) -> (f32, f32) {
    let s = src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
    let t = pixels_of(&target.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE));
    let cand = pixels_of(&render::develop_preview(&s, recipe));
    let err = look_err(&cand, &t);
    let conf = match crate::fit_zoned::joint_reading(&cand, &t) {
        Some(j) => confidence_from_look_err(err).min(clamp_confidence(
            1.0 - j.weighted * crate::fit_zoned::JOINT_CONFIDENCE_SLOPE,
        )),
        None => confidence_from_look_err(err),
    };
    (err, conf)
}

/// Which of the two hue/ratio gates (if either) refused the colour stage —
/// both used to collapse into one boolean, and only one of them had a note.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct CastOutcome {
    /// A pixel-aligned hue gate fired (foreign hues, or a region re-hued).
    rehue_blocked: bool,
    /// The aggregate ratio refused: the curves did not buy enough.
    ratio_rejected: bool,
}

/// Did the finished recipe do HARM — and which check says so?
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct TerminalHarm {
    /// The frame-global look error regressed past the fit's own quantisation
    /// budget (R16's rule, unchanged).
    scalar: bool,
    /// The joint value-range distributions drifted apart past
    /// [`crate::fit_zoned::JOINT_DRIFT_TOL`] (R23-6's additional veto).
    joint: bool,
}

impl TerminalHarm {
    fn any(self) -> bool {
        self.scalar || self.joint
    }
}

/// Tolerance for the fit's OWN quantisation, not a fixed error size. The
/// rounded sliders, the 8-bit residual tone curve and the f32 develop round
/// trip cost about 1e-3 of residual even on an IDENTICAL pair, where
/// `err_before` is exactly 0 — so a bare +1e-4 margin fired there, wiping a
/// perfectly good near-neutral solve and reporting "outside the global
/// model's reach" directly beneath a printed residual of 0.000 -> 0.000.
///
/// A flat FLOOR is the wrong correction though: it would also wave through a
/// fit that is genuinely worse whenever both numbers are small (err_before
/// 0.0010 -> err_after 0.0029 is nearly 3× worse, and no absolute floor below
/// 0.003 catches it). Scale with the error instead and add the quantisation
/// budget once.
const FIT_QUANT: f32 = 1.2e-3;

/// The TERMINAL do-no-harm decision, pure so both of its arms are testable
/// without a fixture that can reach them end to end.
///
/// Two independent readings, OR-ed, because they see different damage. The
/// scalar arm is R16's and unchanged. The joint arm (R23-6) is the
/// ADDITIONAL veto in [`crate::fit_zoned::ZONE_GLOBAL_REGRESSION_TOL`]'s
/// shape: a fit that leaves the value ranges further apart than doing
/// nothing has done harm whatever the scalar says — and the scalar
/// structurally cannot say it, its colour term being three unconditional
/// channel means. It only ever REJECTS, never rescues (a joint reading that
/// improves cannot save a recipe the scalar convicts), and it is FAIL-OPEN:
/// either side missing means no opinion, never "no problem".
fn terminal_harm(
    err_before: f32,
    err_after: f32,
    joint_base: Option<crate::fit_zoned::JointReading>,
    joint_after: Option<crate::fit_zoned::JointReading>,
) -> TerminalHarm {
    TerminalHarm {
        scalar: err_after > err_before * 1.25 + FIT_QUANT,
        joint: match (joint_base, joint_after) {
            (Some(b), Some(a)) => {
                a.weighted > b.weighted + crate::fit_zoned::JOINT_DRIFT_TOL
            }
            _ => false,
        },
    }
}

impl CastOutcome {
    /// What the user is told about an EMPTY colour stage — pure, so the
    /// silent arm is testable without a fixture that reaches it.
    ///
    /// R23-6 A-2: `ratio_rejected` used to produce no note at all, so "the
    /// colour stage produced nothing" — the commonest outcome of the whole
    /// stage — reached the user as an unexplained absence, while the hue
    /// gates next to it did disclose. The hue note WINS a double rejection:
    /// it is the more specific statement, and a fit that would have re-hued
    /// a region is the thing worth saying.
    fn note_key(self) -> Option<&'static str> {
        if self.rehue_blocked {
            Some(crate::rationale::keys::FIT_NOTE_REHUE_BLOCKED)
        } else if self.ratio_rejected {
            Some(crate::rationale::keys::FIT_NOTE_CAST_REJECTED)
        } else {
            None
        }
    }
}

/// Name the develop controls THIS pair's residual points at that the fit has
/// no way to solve for (R23-6 A-5).
///
/// The summary note already says "local masks and per-band HSL are not
/// recovered" on every fit ever produced, which is true and useless: it does
/// not say whether THIS target needed them. The solve domain is a fact about
/// the code — the global arm writes exposure/contrast/highlights/shadows/
/// whites/blacks, a tone curve, one saturation and three channel curves, and
/// NOTHING in `advisor::catalogue::RECIPE_CONTROLS` else — so the honest
/// disclosure is the intersection of "the model can express it", "we never
/// solve it" and "the residual has evidence pointing at it".
///
/// The evidence tests are deliberately coarse and stated as SUSPICION, never
/// as measurement: the residual decomposition can say a gap is chromatic
/// rather than tonal, and it cannot say which control would close it. Naming
/// a control the residual gives no sign of would be inventing a diagnosis.
///
/// `after_px` is the FINISHED render — the residual is what the fit could
/// not close, so the evidence has to be read there and not on the base.
fn unrepresented_note(
    recipe: &EditRecipe,
    after_px: &[[f32; 3]],
    tp: &[[f32; 3]],
    err_after: f32,
) -> Option<crate::rationale::Note> {
    // Nothing left to explain.
    if err_after <= FIT_QUANT_CLEAN {
        return None;
    }
    let mut names: Vec<&str> = Vec::new();

    // --- is what is LEFT a colour difference, and is it conditioned on
    // brightness? That is exactly the question the joint family answers, and
    // exactly the shape `hsl` / `color_grade` have. Reading the CHROMATIC
    // buckets against the NEUTRAL ones at the same brightness separates "the
    // coloured pixels disagree" (a colour move) from "everything disagrees"
    // (a tone or exposure gap the fit does solve for) — a distinction no
    // single global statistic can make, which is why the band-centroid test
    // below cannot carry this on its own: a target that moves a whole region
    // to a hue the source has NOWHERE leaves both bands under the 1.5%
    // two-sided weight gate and is invisible to it (the cross-band blindness
    // `look_err`'s own hue term documents).
    let buckets = crate::fit_zoned::joint_buckets(after_px, tp);
    let worst_of = |chromatic: bool| -> f32 {
        buckets
            .iter()
            .filter(|b| b.chromatic == chromatic)
            .map(|b| b.err)
            .fold(0.0f32, f32::max)
    };
    let (chromatic_worst, neutral_worst) = (worst_of(true), worst_of(false));
    let colour_shaped = chromatic_worst >= UNREPRESENTED_CHROMATIC_ERR
        && chromatic_worst >= neutral_worst + UNREPRESENTED_CHROMATIC_LEAD;

    // …and the classic evidence for the same conclusion: a populated band
    // whose centroid hue is far off. Kept as a SECOND route because it fires
    // on frames where the residual is a rotation rather than a magnitude,
    // and the two routes miss different things.
    let (sa, ta) = band_stats(after_px);
    let (sb, tb) = band_stats(tp);
    let mut worst_band = 0.0f32;
    if ta >= 1.0 && tb >= 1.0 {
        for i in 0..8 {
            let (x, y) = (&sa[i], &sb[i]);
            if x.w / ta < 0.015 || y.w / tb < 0.015 {
                continue;
            }
            let mut d = y.sin.atan2(y.cos).to_degrees() - x.sin.atan2(x.cos).to_degrees();
            while d > 180.0 {
                d -= 360.0;
            }
            while d < -180.0 {
                d += 360.0;
            }
            worst_band = worst_band.max(d.abs() as f32);
        }
    }
    if colour_shaped || worst_band >= UNREPRESENTED_HUE_DEG {
        // `hsl` is the per-band colour mixer the solver bans outright (see
        // the stage-4 comment); `color_grade` is the tone-conditioned
        // version of the same move. Name the second only when the channel
        // curves — our one lever with that shape — are absent, which is
        // both the honest condition and the common one (they are refused by
        // the three gates far more often than they are kept).
        names.push("hsl");
        if recipe.red_curve.is_empty()
            && recipe.green_curve.is_empty()
            && recipe.blue_curve.is_empty()
        {
            names.push("color_grade");
        }
    }
    // A surviving UNIFORM channel-mean offset is the white-balance shape,
    // and `temperature_k` / `tint` are assigned NOWHERE in this module or
    // the zoned one — the one control family the user will look for first.
    let mean = |px: &[[f32; 3]], ch: usize| -> f32 {
        if px.is_empty() {
            0.0
        } else {
            px.iter().map(|p| p[ch]).sum::<f32>() / px.len() as f32
        }
    };
    let rb = (mean(after_px, 0) - mean(tp, 0)) - (mean(after_px, 2) - mean(tp, 2));
    if rb.abs() >= UNREPRESENTED_WB_RB {
        names.push("temperature_k/tint");
    }
    if names.is_empty() {
        return None;
    }
    Some(crate::rationale::Note::new(
        crate::rationale::keys::FIT_NOTE_UNREPRESENTED,
        vec![("controls", names.join(", "))],
    ))
}

/// Below this residual there is nothing to explain and the disclosure would
/// be noise — the same order as the fit's own quantisation budget.
const FIT_QUANT_CLEAN: f32 = 0.01;
/// Worst populated-band centroid disagreement (degrees) that counts as
/// "this target used a per-band colour move". 20° is well past the ±13.5°
/// the engine's own HSL hue axis can even express, so a gap this size cannot
/// be a rounding artefact of a band the fit did reach.
const UNREPRESENTED_HUE_DEG: f32 = 20.0;
/// A chromatic bucket must miss by at least this much before the residual
/// is called colour-shaped. On the fixture set the fits that LAND leave
/// every chromatic bucket under 0.05 (the haze pair's worst is 0.041), while
/// the region-graded canyon leaves 0.098 and the unreachable repaint 0.71.
const UNREPRESENTED_CHROMATIC_ERR: f32 = 0.06;
/// …and it must miss by this much MORE than the neutral buckets at the same
/// brightness, or the difference is a tone/exposure gap the solver does
/// address rather than a colour one it cannot.
const UNREPRESENTED_CHROMATIC_LEAD: f32 = 0.02;
/// Red-minus-blue mean offset (in 0..1 channel units) that counts as a
/// white-balance-shaped residual. 0.02 ≈ 5/255 across the whole frame —
/// visible as a cast, and an order above the fit's own rounding.
const UNREPRESENTED_WB_RB: f32 = 0.02;

// --------------------------------------------------------------------------
// tone solve
// --------------------------------------------------------------------------

/// Magnitude prior for the tone solve. The 5-slider knot system is
/// near-collinear (contrast vs shadows/highlights, whites vs the shoulder), so
/// unpenalised least squares happily returns huge mutually-cancelling sliders
/// whose TOTAL map ties a tasteful solution to within numerical ε — and the
/// residual curve erases even that difference. The prior makes slider
/// magnitude itself part of the cost, so "Exposure +1.5, Contrast −97,
/// Shadows −100" loses to the mild solve it was shadowing. Units: basis
/// authorities are O(0.2–0.34), knot residuals O(0.1); 0.02 prices a pegged
/// slider (s=1) like a ~0.14 luma miss at one knot — strong enough to kill
/// cancellation combos, weak enough that genuinely-needed big moves survive
/// (the roundtrip test pins recovery of a real ±25-point recipe).
const TONE_PRIOR: f64 = 0.02;

/// Scan exposure (nonlinear in the model) and, for each candidate, solve the 5
/// linear sliders (contrast/highlights/shadows/whites/blacks, in the basis
/// order of [`render::tone_slider_basis`]) by RIDGE least squares over the 8
/// knots; keep the (ev, sliders) minimising the PENALISED clamped-solution
/// score `SSE + TONE_PRIOR·Σs²` — the same prior in the solve and in the
/// model selection, so the exposure scan cannot smuggle the degeneracy back.
pub(crate) fn fit_tone_sliders(tone_map: &impl Fn(f32) -> f32) -> (f32, [f32; 5]) {
    let targets: Vec<f32> = render::TONE_KNOTS_X.iter().map(|&x| tone_map(x)).collect();
    let basis: Vec<[f32; 5]> =
        render::TONE_KNOTS_X.iter().map(|&x| render::tone_slider_basis(x)).collect();

    let mut best = (0.0f32, [0.0f32; 5], f32::INFINITY);
    let mut ev = -3.0f32;
    while ev <= 3.0 + 1e-6 {
        // Residual after the exposure component, then ridge normal equations.
        // Knot authority (`tone_knot_weights`) rides the basis rows: it
        // depends only on the candidate ev, so the system stays linear in the
        // sliders — and the solve models the SAME engine that will render the
        // result (an unweighted basis would ask saturated knots to explain
        // residual they can no longer move).
        let weights = render::tone_knot_weights(ev);
        let resid: Vec<f64> = render::TONE_KNOTS_X
            .iter()
            .zip(&targets)
            .map(|(&x, &t)| (t - render::tone_exposure_curve(x, ev)) as f64)
            .collect();
        let mut ata = [[0.0f64; 5]; 5];
        let mut atb = [0.0f64; 5];
        for ((b, r), &w) in basis.iter().zip(&resid).zip(&weights) {
            for i in 0..5 {
                let bi = (w * b[i]) as f64;
                for j in 0..5 {
                    ata[i][j] += bi * (w * b[j]) as f64;
                }
                atb[i] += bi * r;
            }
        }
        for (i, row) in ata.iter_mut().enumerate() {
            row[i] += TONE_PRIOR; // ridge = the magnitude prior (see const doc)
        }
        let sol = solve5(ata, atb);
        let s: [f32; 5] = std::array::from_fn(|i| (sol[i] as f32).clamp(-1.0, 1.0));
        let penalty: f64 = s.iter().map(|&v| TONE_PRIOR * v as f64 * v as f64).sum();
        let score: f64 = basis
            .iter()
            .zip(&resid)
            .zip(&weights)
            .map(|((b, r), &w)| {
                let fit: f64 = (0..5).map(|i| (w * b[i]) as f64 * s[i] as f64).sum();
                (r - fit) * (r - fit)
            })
            .sum::<f64>()
            + penalty;
        if (score as f32) < best.2 {
            best = (ev, s, score as f32);
        }
        ev += 0.05;
    }
    // NOT limited here on purpose. `render::limit_tone_sliders` saturates a
    // slider vector that would flatten a tonal band, and the engine applies it
    // at render time — but applying it to the PROPOSAL as well perturbs this
    // least-squares solve, and the acceptance test downstream is a knife edge:
    // on the hazy-to-clean fixture as it stood pre-R17 (gated evidence,
    // sparse residual knots) the solve was only 3 % better than neutral
    // (0.08625 against 0.08918), so a 0.34 % nudge to the sliders pushed it
    // over `err_before`, tripped the saturation do-no-harm loop, and ended at
    // 0.1286 — far worse than doing nothing. (R17's evidence fallback moved
    // that fixture to 0.0892 → 0.0229; the numbers above are kept as the
    // historical record of WHY the asymmetry exists — the knife-edge
    // geometry, not the exact figures, is the reason.) The fit does not need
    // to predict the limiter anyway: it scores candidates by RENDERING them
    // (`develop_preview` below), so it already measures whatever the engine
    // actually does.
    (best.0, best.1)
}

/// Gaussian elimination with partial pivoting for the 5×5 normal equations.
fn solve5(mut a: [[f64; 5]; 5], mut b: [f64; 5]) -> [f64; 5] {
    for c in 0..5 {
        let mut p = c;
        for r in c + 1..5 {
            if a[r][c].abs() > a[p][c].abs() {
                p = r;
            }
        }
        a.swap(c, p);
        b.swap(c, p);
        if a[c][c].abs() < 1e-12 {
            continue;
        }
        let pivot = a[c]; // copy of the pivot row ([f64; 5] is Copy)
        for r in c + 1..5 {
            let f = a[r][c] / pivot[c];
            for k in c..5 {
                a[r][k] -= f * pivot[k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = [0.0f64; 5];
    for c in (0..5).rev() {
        let mut acc = b[c];
        for k in c + 1..5 {
            acc -= a[c][k] * x[k];
        }
        x[c] = if a[c][c].abs() < 1e-12 { 0.0 } else { acc / a[c][c] };
    }
    x
}

/// Whatever tonal shape the sliders could not express, as `tone_curve` control
/// points. The engine composes `tone_curve` AFTER the knot spline `S`, so the
/// exact residual curve is `M ∘ S⁻¹` — i.e. points `(S(x), M(x))`. Monotone by
/// construction (both `S` and `M` are monotone); skipped when the residual is
/// within tolerance everywhere.
fn residual_tone_curve(recipe: &EditRecipe, tone_map: &impl Fn(f32) -> f32) -> Vec<CurvePoint> {
    debug_assert!(recipe.tone_curve.is_empty(), "fit the residual before setting a curve");
    let lut = render::build_tone_lut(recipe);
    // Knot placement (R17): uniform in the LUT's OUTPUT domain, inverted
    // back through the LUT — the curve's input axis IS the engine's output
    // (`sx` below), so sampling uniform in raw x inherits the base curve's
    // compression. On the real camera base the old fixed 9 xs left a single
    // 38-u8 input gap right across the band holding the frame's tonal mass,
    // and the curve's PIECEWISE-LINEAR rendering (`render::curve_lut` →
    // `interp` — not the monotone cubic the knot spline uses) chords
    // ~10/255 below the concave desired map inside it (measured, _DSC9608
    // × reimagine). 13 output levels bound the inter-knot input gap to
    // ~21 u8 wherever the LUT moves; where it is flat the levels collapse
    // onto one x and the `prev_in` dedup keeps the point list minimal —
    // which also means a flat plateau's interior is no longer sampled by
    // `max_dev` (the old fixed xs could land mid-plateau): deliberate, a
    // many-to-one plateau is beyond any input-side curve's reach anyway.
    // The trade's cost side: 21-u8 spacing doubles the density of u8-rounded
    // control points, ~±0.5/255 of quantisation ripple bought against the
    // ~10/255 of chord sag removed — a 20:1 win.
    const LEVELS: usize = 13;
    let xs = (0..LEVELS).map(|i| {
        let o = i as f32 / (LEVELS - 1) as f32;
        let idx = lut.partition_point(|&v| v < o).min(lut.len() - 1);
        idx as f32 / (lut.len() - 1) as f32
    });
    let mut max_dev = 0.0f32;
    let mut pts: Vec<CurvePoint> = Vec::with_capacity(LEVELS);
    let (mut prev_in, mut prev_out) = (-1i32, 0i32);
    for x in xs {
        let sx = render::sample_lut(&lut, x); // engine output before the residual curve
        let y = tone_map(x).clamp(0.0, 1.0); // desired output
        max_dev = max_dev.max((y - sx).abs());
        let input = (sx * 255.0).round() as i32;
        let output = ((y * 255.0).round() as i32).max(prev_out); // keep monotone
        if input <= prev_in {
            continue; // spline outputs can quantise together at the ends
        }
        pts.push(CurvePoint { input: input as u8, output: output as u8 });
        (prev_in, prev_out) = (input, output);
    }
    if max_dev < 0.015 {
        Vec::new() // the sliders already express the map — keep the recipe clean
    } else {
        pts
    }
}

// --------------------------------------------------------------------------
// colour residuals
// --------------------------------------------------------------------------

/// Per-band accumulator: weight, circular hue (sin/cos), HSL sat + luma.
#[derive(Clone, Copy, Default)]
struct BandStat {
    w: f64,
    sin: f64,
    cos: f64,
    s: f64,
    l: f64,
}

/// Accumulate chroma-gated band statistics with the SAME partition of unity the
/// renderer uses ([`render::bracket_bands`]), so the fit and the engine agree on
/// what "the blue band" is. Returns the per-band stats and the chromatic total.
fn band_stats(px: &[[f32; 3]]) -> ([BandStat; 8], f64) {
    let mut bands = [BandStat::default(); 8];
    let mut total = 0.0f64;
    for p in px {
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < 0.06 {
            continue; // matches the renderer's chroma gate: near-grey carries no hue evidence
        }
        let (h, s, l) = render::rgb_to_hsl(p[0], p[1], p[2]);
        let (b0, b1, w1) = render::bracket_bands(h * 360.0, &render::HSL_CENTERS);
        let ang = (h * std::f32::consts::TAU) as f64;
        for (bi, w) in [(b0, 1.0 - w1 as f64), (b1, w1 as f64)] {
            let b = &mut bands[bi];
            b.w += w;
            b.sin += w * ang.sin();
            b.cos += w * ang.cos();
            b.s += w * s as f64;
            b.l += w * l as f64;
        }
        total += 1.0;
    }
    (bands, total)
}

/// Residual per-channel CDF map (current render → target) as a channel curve —
/// the colour-cast catch-all (white balance shift, split toning the wheels/HSL
/// didn't express). Skipped when the channel already matches within tolerance.
fn residual_channel_curve(cur: &[[f32; 3]], tgt: &[[f32; 3]], ch: usize) -> Vec<CurvePoint> {
    let c_cdf = channel_cdf(cur, ch);
    let t_cdf = channel_cdf(tgt, ch);
    const XS: [f32; 5] = [0.0, 0.25, 0.50, 0.75, 1.0];
    // The keep/skip decision is judged at the SOURCE DISTRIBUTION's own
    // quantiles — |Q_t(q) − Q_c(q)| — where the pixel mass actually sits.
    // Judging at the fixed intensity knots had two failure modes: identical
    // low-dynamic-range channels tripped the gate on their clipped endpoint
    // samples (emitting a non-identity curve through real pixels), and a
    // genuine narrow-band shift (0.30-0.40 → 0.40-0.50) fell BETWEEN the
    // knots and was suppressed entirely.
    let mut max_dev = 0.0f32;
    // Down to the 2nd/98th percentile — the P_CLIP band the curve itself
    // preserves: a 5% cluster shift left every 10-90 quantile untouched and
    // was suppressed. Identical distributions still read 0 everywhere.
    for &q in &[0.02f32, 0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95, 0.98] {
        let xc = quantile(&c_cdf, q);
        let xt = quantile(&t_cdf, q);
        max_dev = max_dev.max((xt - xc).abs());
    }
    if max_dev < 0.012 {
        return Vec::new();
    }
    let mut pts: Vec<CurvePoint> = Vec::with_capacity(XS.len());
    let (mut prev_in, mut prev_out) = (-1i32, 0i32);
    for &x in &XS {
        let f = cdf_at(&c_cdf, x);
        let y = quantile(&t_cdf, f.clamp(P_CLIP, 1.0 - P_CLIP)).clamp(0.0, 1.0);
        let input = (x * 255.0).round() as i32;
        let output = ((y * 255.0).round() as i32).max(prev_out);
        if input <= prev_in {
            continue;
        }
        pts.push(CurvePoint { input: input as u8, output: output as u8 });
        (prev_in, prev_out) = (input, output);
    }
    pts
}

/// The set of 15°-hue bins FOREIGN to the target: farther than
/// [`VETO_FAR_BINS`] bins (circularly) from every bin holding ≥
/// [`VETO_SUPPORT_BIN_MIN`] of the target's chromatic mass. `None` when the
/// target has fewer than [`VETO_MIN_TARGET_CHROMATIC`] chromatic pixels — no
/// reliable hue testimony, the veto stands down.
fn foreign_hue_bins(tp: &[[f32; 3]]) -> Option<[bool; 24]> {
    let mut mass = [0.0f32; 24];
    let mut n = 0usize;
    for p in tp {
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < VETO_SUPPORT_CHROMA {
            continue;
        }
        let (h, _s, _l) = render::rgb_to_hsl(p[0], p[1], p[2]);
        mass[((h * 24.0) as usize).min(23)] += 1.0;
        n += 1;
    }
    if n < VETO_MIN_TARGET_CHROMATIC {
        return None;
    }
    let populated: Vec<usize> =
        (0..24).filter(|&k| mass[k] / n as f32 >= VETO_SUPPORT_BIN_MIN).collect();
    let mut foreign = [true; 24];
    for (k, f) in foreign.iter_mut().enumerate() {
        for &p in &populated {
            let fwd = (k as isize - p as isize).rem_euclid(24) as usize;
            if fwd.min(24 - fwd) <= VETO_FAR_BINS {
                *f = false;
                break;
            }
        }
    }
    Some(foreign)
}

/// Fraction of the frame visibly tinted at a hue foreign to the target
/// (chroma ≥ [`VETO_TINT_CHROMA`], hue in a foreign bin).
fn foreign_share(px: &[[f32; 3]], foreign: &[bool; 24]) -> f32 {
    let mut cnt = 0usize;
    for p in px {
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < VETO_TINT_CHROMA {
            continue;
        }
        let (h, _s, _l) = render::rgb_to_hsl(p[0], p[1], p[2]);
        if foreign[((h * 24.0) as usize).min(23)] {
            cnt += 1;
        }
    }
    cnt as f32 / px.len().max(1) as f32
}

/// Did the cast curves paint a REGION of the frame in hues the target holds
/// nowhere (≥ [`VETO_FAR_BINS`]·15° from all its populated hue mass)?
/// `cur`/`with_px` render the SAME source, so the share DELTA is exactly the
/// curves' own work — pre-existing content mismatch cancels out.
fn cast_paints_foreign_hues(cur: &[[f32; 3]], with_px: &[[f32; 3]], tp: &[[f32; 3]]) -> bool {
    let Some(foreign) = foreign_hue_bins(tp) else {
        return false;
    };
    foreign_share(with_px, &foreign) - foreign_share(cur, &foreign) >= VETO_CREATED_SHARE
}

/// Frame share of RE-HUED pixels: a MEASURABLE hue before (chroma ≥
/// [`ROT_HUE_MEASURABLE_CHROMA`]), a visible tint after (chroma ≥
/// [`VETO_TINT_CHROMA`]), landing ≥ [`ROT_DEG`] of circular hue away — and
/// VISIBLE on at least one end (see [`ROT_VISIBLE_BEFORE`]): a sub-visible
/// tint flipped into another sub-visible tint is cast-inversion
/// pass-through, not a re-hue. Pixel-aligned: `cur`/`with_px` render the
/// SAME source, so per-pixel hue movement is exact. De-tinting (end chroma
/// under the gate) is exempt — removing colour is what a corrective cast
/// does. Exposed separately from the boolean gate so the pin test measures
/// the same census the gate uses.
fn rehued_share(cur: &[[f32; 3]], with_px: &[[f32; 3]]) -> f32 {
    let mut cnt = 0usize;
    for (c, w) in cur.iter().zip(with_px) {
        let cc = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        let wc = w[0].max(w[1]).max(w[2]) - w[0].min(w[1]).min(w[2]);
        if cc < ROT_HUE_MEASURABLE_CHROMA || wc < VETO_TINT_CHROMA {
            continue;
        }
        if cc < ROT_VISIBLE_BEFORE && wc < ROT_VISIBLE_AFTER {
            continue; // invisible on both ends: pass-through, not a re-hue
        }
        let h0 = render::rgb_to_hsl(c[0], c[1], c[2]).0 * 360.0;
        let h1 = render::rgb_to_hsl(w[0], w[1], w[2]).0 * 360.0;
        let mut d = (h1 - h0).abs() % 360.0;
        if d > 180.0 {
            d = 360.0 - d;
        }
        if d >= ROT_DEG {
            cnt += 1;
        }
    }
    cnt as f32 / cur.len().max(1) as f32
}

/// Did the curves re-hue a REGION ([`ROT_SHARE`] of the frame)? See
/// [`rehued_share`] and the rotation-budget const block.
fn cast_rotates_a_region(cur: &[[f32; 3]], with_px: &[[f32; 3]]) -> bool {
    rehued_share(cur, with_px) >= ROT_SHARE
}

// --------------------------------------------------------------------------
// statistics primitives
// --------------------------------------------------------------------------

pub(crate) fn pixels_of(img: &DynamicImage) -> Vec<[f32; 3]> {
    img.to_rgb8()
        .pixels()
        .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
        .collect()
}

fn luma601(p: &[f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

fn cdf_from_values(values: impl Iterator<Item = f32>, n_hint: usize) -> Vec<f32> {
    let mut hist = vec![0.0f32; HIST_BINS];
    let mut n = 0usize;
    for v in values {
        let i = ((v.clamp(0.0, 1.0)) * (HIST_BINS - 1) as f32).round() as usize;
        hist[i] += 1.0;
        n += 1;
    }
    let total = (n.max(n_hint.min(1)) as f32).max(1.0);
    let mut acc = 0.0f32;
    for h in hist.iter_mut() {
        acc += *h;
        *h = acc / total;
    }
    hist
}

fn luma_cdf(px: &[[f32; 3]]) -> Vec<f32> {
    cdf_from_values(px.iter().map(luma601), px.len())
}

/// Near-neutral gate for the TONE evidence (the cast catch-all fits on
/// ungated per-channel CDFs — see `residual_channel_curve`). Gated on HSV
/// saturation ((max−min)/max), which is INVARIANT under pure luminance
/// scaling — so the same pixels qualify in the source and in its tone-mapped
/// target (an absolute-chroma gate is not: dark colours slip under it in the
/// source and leave it once brightened, skewing the two CDFs against each
/// other). Near-black counts as neutral.
fn is_neutralish(p: &[f32; 3]) -> bool {
    let mx = p[0].max(p[1]).max(p[2]);
    let mn = p[0].min(p[1]).min(p[2]);
    mx < 0.04 || (mx - mn) / mx < 0.15
}

/// Tone-evidence CDF pair. Near-neutral gating only carries clean evidence
/// when the SAME population is neutral on BOTH sides — the tone map is
/// quantile-to-quantile, so the gate is an identification assumption about
/// pixel correspondence, not a per-image preference. Three observed
/// breakages: a side's neutral sample is too small (< 5% or < 512 px —
/// noise); the neutral SHARES diverge, meaning the target re-hued (or
/// de-hued) part of the population — golden-sky pair, 2026-07-09: the
/// source's pale sky is neutralish ((max−min)/max ≈ 0.12), the target's
/// vivid gold one is not (≈ 0.37), and an asymmetric gate mapped the sky's
/// luma cluster across a ramp it doesn't belong to, distorting the whole
/// tone solve; or the shares stay COMPARABLE while a luma-CONCENTRATED band
/// churns out of one side's class — _DSC9608 × reimagine, 2026-08-12: the
/// target re-hued 24% of the base's neutral class (the pale sky, base-luma
/// q50 ≈ 197/255, → vivid blue), the share ratio read a passing 1.29×, and
/// the base's bright grey ranks paired against target ranks the sky no
/// longer belongs to — every upper-mid darkened and the render shipped
/// murky. Shares are a SIZE proxy, blind to composition (and the haze pair
/// proves one-sided CDF-shift proxies rank harm no better), so the gate is
/// judged by the harm itself: [`neutral_gate_misprediction`] scores the
/// gated evidence map against the shared class's own observable pairing.
/// Either way the assumption is dead: fall back to full-pixel CDFs on BOTH
/// sides (deciding per side, as the original code did, can even compare a
/// neutral-gated CDF against a full one). 1.75× keeps the
/// matched-population regressions (identity / roundtrip / violet canyon
/// ≈ 1.0×) while catching the golden-sky asymmetry (2.0×); the
/// misprediction ceiling is anchored in [`NEUTRAL_MISPREDICTION_MAX`]'s
/// doc. The two detectors are COMPLEMENTARY, not redundant (R18, measured
/// on the archive): the share ratio is alignment-free — it still works
/// when misregistration slides the misprediction metric toward 0 (its
/// fail-open direction) — while the misprediction gate catches membership
/// churn the ratio cannot see (_DSC9621 × reimagine-4: share 1.51×,
/// misprediction 0.034 — only this gate fires). A >1.75× share asymmetry
/// falls back UNCONDITIONALLY, by design: a low misprediction reading must
/// never override it, because misregistration fakes exactly that reading
/// (the fail-open direction), and the fallback itself is the safe arm —
/// on the two live pairs whose evidence failed a gate, the fallback solve
/// measured better than the gated one both times (a benign uniform >1.75×
/// inflation remains synthetic-only; no real pair has produced one). Gate order: cheap counts and shares first, the
/// misprediction pass (a full-frame scan plus four CDFs) last, so
/// under-evidenced pairs never pay for it.
fn tone_cdf_pair(sp: &[[f32; 3]], tp: &[[f32; 3]]) -> (Vec<f32>, Vec<f32>) {
    let s_n: Vec<f32> = sp.iter().filter(|p| is_neutralish(p)).map(luma601).collect();
    let t_n: Vec<f32> = tp.iter().filter(|p| is_neutralish(p)).map(luma601).collect();
    let share_s = s_n.len() as f32 / sp.len().max(1) as f32;
    let share_t = t_n.len() as f32 / tp.len().max(1) as f32;
    let gated = enough_evidence(s_n.len(), sp.len())
        && enough_evidence(t_n.len(), tp.len())
        && share_s.max(share_t) <= 1.75 * share_s.min(share_t)
        && neutral_gate_misprediction(sp, tp) <= NEUTRAL_MISPREDICTION_MAX;
    if gated {
        let (ns, nt) = (s_n.len(), t_n.len());
        (cdf_from_values(s_n.into_iter(), ns), cdf_from_values(t_n.into_iter(), nt))
    } else {
        (luma_cdf(sp), luma_cdf(tp))
    }
}

/// The tone-evidence sample floor: at least 5% of the frame and never fewer
/// than 512 px. Shared between the per-side gate and the shared-class floor
/// inside [`neutral_gate_misprediction`], so "enough to trust" means one
/// thing.
fn enough_evidence(n: usize, total: usize) -> bool {
    n >= (total / 20).max(NEUTRAL_SHARED_MIN)
}

/// How badly the gated tone evidence MISPREDICTS the population it claims
/// to identify. The SHARED class — pixels neutral at the same position on
/// both sides — is the one population whose (source-luma, target-luma)
/// pairing is observable without any modelling: under a monotone tone map,
/// its own quantiles ARE the map. So build the production evidence map
/// exactly as the tone solve would (each side's whole neutral class,
/// quantile-paired) and score it against the shared class's empirical map:
/// mean |Δ| over the 21 look_err quantiles. Asymmetric members that merely
/// inflate a class along the shared luma ramp leave the pairing intact
/// (the synthetic uniform-inflation fixture reads < 0.0075); members that
/// churn in a luma-concentrated band bend the ranks and the misprediction
/// shows it directly — this is the murk, measured at its source.
///
/// POSITIONAL-CORRESPONDENCE ASSUMPTION, stated plainly because the rest of
/// this module deliberately avoids one (a generative target is not
/// pixel-aligned): co-membership at equal row-major index is read as "same
/// coarse region", which holds for same-frame pairs on the shared 384-edge
/// thumbnail grid and degrades with misregistration. The failure direction
/// is OPEN: under broken alignment target-membership decorrelates from
/// source-membership, the shared class becomes an unbiased thinning of both
/// sides, and the metric slides toward 0 — the gate is KEPT, not dropped,
/// and this detector goes vacuous (its sensitivity is proportional to
/// registration quality; the older share/size gates still stand in front
/// of it). Grids that disagree by more than aspect rounding (~a row) are
/// not comparable at all — that case returns infinite (fall back) as an
/// explicit decision rather than an emergent prefix artifact, and so does
/// a shared class too small to clear the same evidence floor the sides
/// must clear.
pub(crate) fn neutral_gate_misprediction(sp: &[[f32; 3]], tp: &[[f32; 3]]) -> f32 {
    let n = sp.len().min(tp.len());
    // 2% ≈ several rows of a 384-edge thumb: beyond aspect rounding, the
    // pairs come from different geometry and co-membership is meaningless.
    if sp.len().abs_diff(tp.len()) > n / 50 {
        return f32::INFINITY;
    }
    let (mut s_all, mut t_all) = (Vec::with_capacity(n / 2), Vec::with_capacity(n / 2));
    let (mut sh_s, mut sh_t) = (Vec::with_capacity(n / 2), Vec::with_capacity(n / 2));
    for i in 0..n {
        let (a, b) = (is_neutralish(&sp[i]), is_neutralish(&tp[i]));
        if a {
            s_all.push(luma601(&sp[i]));
        }
        if b {
            t_all.push(luma601(&tp[i]));
        }
        if a && b {
            sh_s.push(luma601(&sp[i]));
            sh_t.push(luma601(&tp[i]));
        }
    }
    if !enough_evidence(sh_s.len(), n) {
        return f32::INFINITY;
    }
    let (ns, nt, nsh) = (s_all.len(), t_all.len(), sh_s.len());
    let s_cdf = cdf_from_values(s_all.into_iter(), ns);
    let t_cdf = cdf_from_values(t_all.into_iter(), nt);
    let sh_s_cdf = cdf_from_values(sh_s.into_iter(), nsh);
    let sh_t_cdf = cdf_from_values(sh_t.into_iter(), nsh);
    let mut acc = 0.0f32;
    let mut cnt = 0.0f32;
    for i in 0..=20 {
        let p = (i as f32 / 20.0).clamp(P_CLIP, 1.0 - P_CLIP);
        let x = quantile(&sh_s_cdf, p);
        // The same formula the tone solve uses for its map (see `tone_map`).
        let predicted = quantile(&t_cdf, cdf_at(&s_cdf, x).clamp(P_CLIP, 1.0 - P_CLIP));
        let actual = quantile(&sh_t_cdf, p);
        acc += (predicted - actual).abs();
        cnt += 1.0;
    }
    acc / cnt
}

fn channel_cdf(px: &[[f32; 3]], ch: usize) -> Vec<f32> {
    cdf_from_values(px.iter().map(|p| p[ch]), px.len())
}

/// F(x): fraction of pixels ≤ x (linear interp between bins).
pub(crate) fn cdf_at(cdf: &[f32], x: f32) -> f32 {
    let pos = x.clamp(0.0, 1.0) * (cdf.len() - 1) as f32;
    let i = pos.floor() as usize;
    if i >= cdf.len() - 1 {
        return cdf[cdf.len() - 1];
    }
    let t = pos - i as f32;
    cdf[i] * (1.0 - t) + cdf[i + 1] * t
}

/// Q(p): the value at quantile `p` (inverse CDF, linear interp within the bin).
pub(crate) fn quantile(cdf: &[f32], p: f32) -> f32 {
    let n = cdf.len();
    let mut lo = 0usize;
    let mut hi = n - 1;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if cdf[mid] < p {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    // Interpolate within the step from the previous bin for a smooth inverse.
    if lo == 0 {
        return 0.0;
    }
    let (c0, c1) = (cdf[lo - 1], cdf[lo]);
    let t = if c1 > c0 { ((p - c0) / (c1 - c0)).clamp(0.0, 1.0) } else { 1.0 };
    ((lo - 1) as f32 + t) / (n - 1) as f32
}

/// Variance of the per-pixel channel mean — the degenerate-input probe
/// (see the refusal at the top of [`fit_recipe`]). Zero for an empty slice.
const DEGENERATE_LUMA_VAR: f32 = 1e-6;
fn luma_variance(px: &[[f32; 3]]) -> f32 {
    if px.is_empty() {
        return 0.0;
    }
    let n = px.len() as f32;
    let lum = |p: &[f32; 3]| (p[0] + p[1] + p[2]) / 3.0;
    let mean: f32 = px.iter().map(lum).sum::<f32>() / n;
    px.iter().map(|p| (lum(p) - mean).powi(2)).sum::<f32>() / n
}

fn mean_chroma(px: &[[f32; 3]]) -> f32 {
    if px.is_empty() {
        return 0.0;
    }
    let sum: f32 =
        px.iter().map(|p| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])).sum();
    sum / px.len() as f32
}

/// One scalar "how different do these look" — mean |Δ| over 21 luma quantiles
/// (60 %), the 3 channel means (20 %), and the worst per-band centroid hue
/// disagreement (20 %). 0 = identical distributions. The hue term exists
/// because matched luma quantiles + channel MEANS can hide a full-blown hue
/// disaster (a purple sky and a blue one can share all four global numbers —
/// exactly how the 2026-07-07 real-photo failure reported err 0.034 /
/// confidence 0.80 for an unusable render).
pub(crate) fn look_err(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    let (ca, cb) = (luma_cdf(a), luma_cdf(b));
    let mut tonal = 0.0f32;
    let mut n = 0.0f32;
    for i in 0..=20 {
        let p = (i as f32 / 20.0).clamp(P_CLIP, 1.0 - P_CLIP);
        tonal += (quantile(&ca, p) - quantile(&cb, p)).abs();
        n += 1.0;
    }
    tonal /= n;
    let mean = |px: &[[f32; 3]], ch: usize| -> f32 {
        if px.is_empty() {
            return 0.0;
        }
        px.iter().map(|p| p[ch]).sum::<f32>() / px.len() as f32
    };
    let colour = (0..3).map(|ch| (mean(a, ch) - mean(b, ch)).abs()).sum::<f32>() / 3.0;
    let base = 0.6 * tonal + 0.2 * colour;
    // Per-band centroid hue disagreement — the WORST qualifying band, not a
    // weighted mean: one region with wrecked hue ruins a photo no matter how
    // small its area share (a lavender sky over perfect rocks), and an
    // area-weighted mean lets exactly that hide (measured: the violet-sky
    // curves slipped through the cast-acceptance gate on the mean variant).
    // |Δ| saturates at 60° so a fully-wrecked band reads 1.
    let (sa, ta) = band_stats(a);
    let (sb, tb) = band_stats(b);
    let mut hue = 0.0f32;
    if ta >= 1.0 && tb >= 1.0 {
        for i in 0..8 {
            let (x, y) = (&sa[i], &sb[i]);
            if x.w / ta < 0.015 || y.w / tb < 0.015 {
                continue;
            }
            let mut d = y.sin.atan2(y.cos).to_degrees() - x.sin.atan2(x.cos).to_degrees();
            while d > 180.0 {
                d -= 360.0;
            }
            while d < -180.0 {
                d += 360.0;
            }
            hue = hue.max((d.abs().min(60.0) / 60.0) as f32);
        }
    }
    base + 0.2 * hue
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}
fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    /// L06-1/2: a zero-variance pair (lens-cap frame against itself) must
    /// refuse to fit — the CDF inverse would produce a constant tone map and
    /// err==0 would accept it silently.
    #[test]
    fn a_degenerate_pair_refuses_to_fit_and_says_why() {
        use image::{DynamicImage, RgbImage};
        let black = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0])));
        let report = fit_recipe(&black, &black);
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_DEGENERATE),
            "the refusal is disclosed through the rationale channel: {:?}",
            report.recipe.rationale
        );
        assert!(report.recipe.tone_curve.is_empty(), "no constant tone map is produced");
        assert_eq!(report.recipe.exposure_ev, 0.0, "the recipe stays neutral");
    }

    use super::*;
    use image::RgbImage;

    /// M-F1: `fit_tone_sliders` degraded to return neutral (or any solver
    /// regression that stops beating the ground truth under the engine's own
    /// penalised objective) — on a look the weighted model can represent
    /// exactly, the solve must score at least as well as the generating
    /// parameters themselves.
    ///
    /// Recorded honestly: the fit-side knot WEIGHTING itself has no
    /// test-observable effect — an unweighted inner solve is a worse proposer
    /// in the saturated regime, but the outer acceptance loop re-scores every
    /// candidate by real rendering and masks it (verified by running the full
    /// suite under that mutant). The weights stay in the solve for model
    /// consistency — three sites, one definition — not because a test pins
    /// them.
    #[test]
    fn the_fit_models_the_same_weighted_engine_it_renders_against() {
        let ev = 1.5f32;
        let truth = [-0.6f32, 0.0, 0.35, 0.0, 0.0]; // contrast −60, shadows +35
        let weights = render::tone_knot_weights(ev);
        let tone = |x: f32| -> f32 {
            let i = render::TONE_KNOTS_X
                .iter()
                .position(|&k| (k - x).abs() < 1e-6)
                .expect("fit samples the tone map at the knots only");
            let b = render::tone_slider_basis(x);
            render::tone_exposure_curve(x, ev)
                + weights[i] * (0..5).map(|k| b[k] * truth[k]).sum::<f32>()
        };
        let (got_ev, got) = fit_tone_sliders(&tone);
        // The saturated regime is deliberately non-identifiable (several
        // (ev, sliders) pairs render the same 8 knots, and the ridge prior
        // picks the smallest sliders), so the property is NOT parameter
        // recovery. It is: under the engine's OWN penalised objective — knot
        // error through the weighted model, plus the magnitude prior — the
        // solution must be at least as good as the ground truth itself. The
        // pristine solve minimises exactly this, so it passes structurally;
        // a solve that dropped the weights optimises a different forward
        // model and lands on parameters this objective scores worse.
        let true_score = |cand_ev: f32, s: &[f32; 5]| -> f64 {
            let w = render::tone_knot_weights(cand_ev);
            let sse: f64 = render::TONE_KNOTS_X
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    let b = render::tone_slider_basis(x);
                    let rendered = render::tone_exposure_curve(x, cand_ev)
                        + w[i] * (0..5).map(|k| b[k] * s[k]).sum::<f32>();
                    let e = (rendered - tone(x)) as f64;
                    e * e
                })
                .sum();
            sse + s.iter().map(|&v| TONE_PRIOR * v as f64 * v as f64).sum::<f64>()
        };
        let (got_score, truth_score) = (true_score(got_ev, &got), true_score(ev, &truth));
        assert!(
            got_score <= truth_score + 1e-6,
            "the fit landed on (ev {got_ev}, {got:?}) scoring {got_score:.6} — WORSE under \
             the engine's own model than the ground truth's {truth_score:.6}: the solve is \
             not modelling the engine it renders against"
        );
    }

    /// Synthetic frame with real tonal + chromatic coverage: a neutral luma ramp
    /// plus orange / blue / green ramps (192×128 — analysis-sized already).
    fn synth() -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = x as f32 / (w - 1) as f32;
                let p = match y * 4 / h {
                    0 => [l, l, l],
                    1 => [l, l * 0.6, l * 0.2],
                    2 => [l * 0.2, l * 0.7, l],
                    _ => [l * 0.3, l, l * 0.4],
                };
                img.put_pixel(x, y, image::Rgb(p.map(|c| (c * 255.0).round() as u8)));
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn identity_fit_is_near_neutral() {
        let img = synth();
        let rep = fit_recipe(&img, &img);
        let r = &rep.recipe;
        // The terminal do-no-harm reset would satisfy EVERY assertion below
        // by wiping the recipe to default, so a broken solve (exposure +3,
        // contrast +90) would look identical to a correct one. Demand that
        // the safety net did NOT fire: on an identity pair the honest solve
        // is already near-neutral, so there is nothing for it to catch.
        assert!(
            !rep.recipe.rationale.contains("do-no-harm terminal case"),
            "the identity solve must stand on its own, not on the reset: {}",
            rep.recipe.rationale
        );
        assert!(r.exposure_ev.abs() < 0.06, "exposure {}", r.exposure_ev);
        for (name, v) in [
            ("contrast", r.contrast),
            ("highlights", r.highlights),
            ("shadows", r.shadows),
            ("whites", r.whites),
            ("blacks", r.blacks),
            ("saturation", r.saturation),
        ] {
            assert!(v.abs() < 6.0, "{name} should stay near 0, got {v}");
        }
        assert!(rep.err_after < 0.02, "identity residual {}", rep.err_after);
    }

    #[test]
    fn roundtrip_recovers_tone_and_saturation() {
        // Render a KNOWN recipe through the real engine, then fit it back.
        let src = synth();
        let mut truth = EditRecipe {
            exposure_ev: 0.35,
            contrast: 18.0,
            highlights: -25.0,
            whites: 12.0,
            saturation: 15.0,
            ..Default::default()
        };
        truth.clamp();
        let target = render::develop_preview(&src, &truth);
        let rep = fit_recipe(&src, &target);
        let r = &rep.recipe;
        // The luma CDF of the target IS the engine's own tone map of the source,
        // so the solve must land close (exposure/slider trade-offs allowed).
        assert!((r.exposure_ev - 0.35).abs() < 0.20, "exposure {}", r.exposure_ev);
        assert!(r.contrast > 3.0 && r.contrast < 45.0, "contrast {}", r.contrast);
        assert!(r.highlights < -8.0 && r.highlights > -50.0, "highlights {}", r.highlights);
        assert!(r.saturation > 5.0 && r.saturation < 30.0, "saturation {}", r.saturation);
        // And the fitted recipe must actually reproduce the look through the engine.
        assert!(
            rep.err_after < (rep.err_before * 0.5).max(0.012),
            "residual {} vs before {}",
            rep.err_after,
            rep.err_before
        );
    }

    #[test]
    fn hazy_to_clean_fit_stays_sane() {
        // Regression for the 2026-07-07 real-photo failure: fitting a
        // low-contrast, low-chroma, blue-cast base toward a clean punchy
        // target produced mutually-cancelling pegged tone sliders
        // (Exposure +1.5 / Contrast −97 / Shadows −100), pegged per-band hue
        // rotations (+45) and a purple sky — while the old metric reported
        // "improved". The prior, the stage order and the correspondence gate
        // must keep every fitted control in its sane regime.
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            // a shadow-weighted blue cast at realistic haze strength (the
            // midpoint pin keeps it out of the highlights, like real haze)
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        let rep = fit_recipe(&base, &clean);
        let r = &rep.recipe;
        assert!(
            r.contrast > -20.0 && r.contrast.abs() < 90.0,
            "degenerate contrast {}",
            r.contrast
        );
        assert!(
            r.shadows.abs() < 90.0 && r.whites.abs() < 90.0 && r.blacks.abs() < 90.0,
            "pegged tone sliders: sh {} wh {} bl {}",
            r.shadows,
            r.whites,
            r.blacks
        );
        assert!(r.exposure_ev.abs() <= 1.0, "runaway exposure {}", r.exposure_ev);
        // NOTE deliberately no "slider not pegged" assertion for hue: the
        // correspondence gate already rejects mismatched populations, and a
        // genuine in-gate rotation larger than the engine's ±13.5° range
        // legitimately clamps. What must hold is the RESULT (below): no band
        // of the fitted render lands tens of degrees off the target.
        assert!(
            rep.err_after < rep.err_before,
            "fit made the look worse: {} -> {}",
            rep.err_before,
            rep.err_after
        );
        // The decisive invariant: render the fitted recipe and check every
        // populated band's centroid hue against the target — the purple-sky
        // failure class means some band lands tens of degrees off.
        let fitted = pixels_of(&render::develop_preview(&base, &rep.recipe));
        let (fb, ftot) = band_stats(&fitted);
        let (tb, ttot) = band_stats(&pixels_of(&clean));
        let mut worst = 0.0f64;
        for i in 0..8 {
            let (x, y) = (&fb[i], &tb[i]);
            if x.w / ftot < 0.015 || y.w / ttot < 0.015 {
                continue;
            }
            let mut d = y.sin.atan2(y.cos).to_degrees() - x.sin.atan2(x.cos).to_degrees();
            while d > 180.0 {
                d -= 360.0;
            }
            while d < -180.0 {
                d += 360.0;
            }
            worst = worst.max(d.abs());
        }
        assert!(worst < 15.0, "a band's hue is still {worst:.1}° off after the fit");
    }

    /// 192×128 canyon: 15.6% neutral ramp (tone evidence — without it the
    /// pale sky is the only `is_neutralish` population and the tone solve
    /// degenerates), 68.8% warm-rock ramp, 15.6% pale-blue sky. `warm` = the
    /// region-graded target: rocks red-lifted (`l^0.7`), ramp + sky IDENTICAL
    /// to the source — the grade a global cast cannot express without
    /// collateral damage.
    fn canyon(warm: bool) -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = 0.15 + 0.80 * x as f32 / (w - 1) as f32;
                let p = if y < 16 {
                    [l, l, l] // neutral ramp
                } else if y < 112 {
                    // Rocks: the warm grade is a pure RED OFFSET — a hue move
                    // toward orange that symmetric chroma expansion (the
                    // saturation stage) cannot express, so the residual lands
                    // squarely on the red channel curve, as in the real photo.
                    let r = if warm { (0.85 * l + 0.18).min(1.0) } else { 0.85 * l };
                    [r, 0.52 * l, 0.30 * l]
                } else {
                    [0.64, 0.68, 0.73] // pale blue sky, hue ≈ 213°, chroma 0.09
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// R17: the _DSC9608 × reimagine murk, distilled — the target re-hues a
    /// luma-CONCENTRATED bright band out of the source's neutral class. The
    /// share ratio stays under 1.75× (the old gate passed and the murky fit
    /// shipped), but the leavers bend the source evidence CDF far past the
    /// contamination ceiling; the solve must fall back to full-pixel CDFs.
    #[test]
    fn a_rehued_bright_grey_band_falls_back_to_full_cdfs() {
        let n = 64 * 64;
        let mut sp: Vec<[f32; 3]> = Vec::with_capacity(n);
        let mut tp: Vec<[f32; 3]> = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / n as f32;
            if t < 0.4 {
                let l = 0.30 + 0.5 * t; // mid grey, neutral on BOTH sides
                sp.push([l, l, l]);
                tp.push([l, l, l]);
            } else if t < 0.6 {
                let l = 0.75 + 0.5 * (t - 0.4); // bright grey → re-hued vivid blue
                sp.push([l, l, l]);
                tp.push([0.3 * l, 0.5 * l, l]);
            } else {
                let l = 0.2 + 0.5 * t; // chromatic on both sides
                sp.push([l, 0.6 * l, 0.3 * l]);
                tp.push([l, 0.6 * l, 0.3 * l]);
            }
        }
        // Premise: this is the case the SHARE gate cannot see (0.6 vs 0.4 =
        // 1.5×, under 1.75×) — only the contamination measure convicts it.
        let s_share = sp.iter().filter(|p| is_neutralish(p)).count() as f32 / n as f32;
        let t_share = tp.iter().filter(|p| is_neutralish(p)).count() as f32 / n as f32;
        assert!(
            s_share.max(t_share) <= 1.75 * s_share.min(t_share),
            "premise broken: the share gate would already catch this ({s_share} vs {t_share})"
        );
        let c = neutral_gate_misprediction(&sp, &tp);
        assert!(
            c > 2.0 * NEUTRAL_MISPREDICTION_MAX,
            "the concentrated band must contaminate with margin: {c}"
        );
        let (s_cdf, t_cdf) = tone_cdf_pair(&sp, &tp);
        assert_eq!(s_cdf, luma_cdf(&sp), "source side must fall back to the full CDF");
        assert_eq!(t_cdf, luma_cdf(&tp), "target side must fall back to the full CDF");
    }

    /// R17 counterpart #1: benign one-sided inflation keeps the gate. The
    /// source's extra neutrals (a uniform desaturation — the haze-pair
    /// geometry) span the same luma ramp as the shared class, so the
    /// evidence CDF barely moves even though a sixth of the frame is
    /// neutral on the source side only.
    #[test]
    fn a_uniformly_inflated_neutral_class_keeps_the_gate() {
        let n = 64 * 64;
        let mut sp: Vec<[f32; 3]> = Vec::with_capacity(n);
        let mut tp: Vec<[f32; 3]> = Vec::with_capacity(n);
        for i in 0..n {
            let l = 0.2 + 0.6 * (i as f32 / n as f32);
            match i % 6 {
                0 => {
                    sp.push([l, l, l]); // neutral in the source only…
                    tp.push([l, 0.8 * l, 0.6 * l]); // …chromatic in the target
                }
                1..=3 => {
                    sp.push([l, l, l]); // the shared class
                    tp.push([l, l, l]);
                }
                _ => {
                    sp.push([l, 0.6 * l, 0.3 * l]); // chromatic on both sides
                    tp.push([l, 0.6 * l, 0.3 * l]);
                }
            }
        }
        let c = neutral_gate_misprediction(&sp, &tp);
        assert!(
            c < 0.5 * NEUTRAL_MISPREDICTION_MAX,
            "uniform inflation must stay clear of the ceiling: {c}"
        );
        let (s_cdf, _) = tone_cdf_pair(&sp, &tp);
        assert_ne!(s_cdf, luma_cdf(&sp), "the benign pair must stay neutral-gated");
    }

    /// R17 anchor on the LIVE haze pair: its neutral identification is
    /// genuinely broken — the haze recipe's blue cast tints the clean
    /// frame's dark greys OUT of the source-side class while the global
    /// desaturation pulls colours IN, and the gated evidence map misses the
    /// shared class by 0.13 mean luma (measured; worst at the dark ranks).
    /// The misprediction gate must fall back to full-pixel CDFs — and the
    /// fit, now solving on honest evidence, must land far below its
    /// starting error (0.0892 -> 0.0229 measured; the GATED solve under the
    /// R17 dense residual knots collapses to a do-no-harm reset, because
    /// faithful sampling faithfully implements a broken map).
    #[test]
    fn the_haze_pairs_broken_identification_falls_back_and_still_fits() {
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        let sp = pixels_of(&base.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE));
        let tp = pixels_of(&clean.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE));
        let m = neutral_gate_misprediction(&sp, &tp);
        assert!(
            m > NEUTRAL_MISPREDICTION_MAX,
            "premise broken: the haze pair's neutral evidence reads clean ({m:.4})"
        );
        let rep = fit_recipe(&base, &clean);
        // Measured 0.0892 -> 0.0229 (0.26×); 0.35× keeps real margin without
        // letting the win quietly rot.
        assert!(
            rep.err_after < 0.35 * rep.err_before,
            "the fallback solve must still close most of the gap ({:.4} -> {:.4})",
            rep.err_before,
            rep.err_after
        );
    }

    /// R17 counterpart #2: matched populations read ZERO contamination. The
    /// canyon pair's neutral members (ramp + pale sky) are IDENTICAL on
    /// both sides, and the returned CDFs stay neutral-gated (≠ the
    /// full-pixel CDFs, which include the rocks).
    #[test]
    fn matched_neutral_members_keep_the_gate() {
        let sp = pixels_of(&canyon(false).thumbnail(ANALYZE_EDGE, ANALYZE_EDGE));
        let tp = pixels_of(&canyon(true).thumbnail(ANALYZE_EDGE, ANALYZE_EDGE));
        let c = neutral_gate_misprediction(&sp, &tp);
        assert!(
            c < 0.5 * NEUTRAL_MISPREDICTION_MAX,
            "premise broken: the canyon pair's neutral members diverged ({c})"
        );
        let (s_cdf, _) = tone_cdf_pair(&sp, &tp);
        assert_ne!(s_cdf, luma_cdf(&sp), "the canyon pair must stay neutral-gated");
    }

    /// R17: residual-curve knots follow the LUT's OUTPUT spacing. On a steep
    /// camera base the old fixed-x placement left a 38-u8 input gap right
    /// across the band holding a real frame's tonal mass, and the curve's
    /// piecewise-linear rendering chorded ~10/255 below the promised map
    /// inside it. The base curve here is the _DSC9608 camera calibration
    /// verbatim.
    #[test]
    fn residual_knots_stay_dense_in_the_curve_input_space() {
        let recipe = EditRecipe {
            base_curve: vec![
                [0.0, 0.0],
                [0.22091886, 0.25904202],
                [0.23851417, 0.29325512],
                [0.25317693, 0.32551318],
                [0.28152493, 0.38514173],
                [0.34115347, 0.49266863],
                [0.39687195, 0.6060606],
                [0.42033234, 0.6539589],
                [0.4848485, 0.74486804],
                [0.51808405, 0.77614856],
                [0.5474096, 0.8005865],
                [0.60117304, 0.8445748],
                [1.0, 1.0],
            ],
            ..EditRecipe::default()
        };
        let curve = residual_tone_curve(&recipe, &|x: f32| (0.9 * x + 0.02).clamp(0.0, 1.0));
        assert!(curve.len() >= 8, "a nontrivial map earns a dense curve: {} pts", curve.len());
        for w in curve.windows(2) {
            let gap = w[1].input as i32 - w[0].input as i32;
            assert!(
                gap <= 32,
                "knot gap {gap} u8 between inputs {} and {} — interpolation sag territory",
                w[0].input,
                w[1].input
            );
        }
    }

    /// The veto's discriminator, pinned on both live cases: it must NOT fire
    /// on the haze pair (whose accepted correction rotates pixels only INTO
    /// the target's own hue families — measured foreign-share delta ≈ 0.000)
    /// and MUST fire on the canyon cast (which paints ~12% of the frame in
    /// hues ≥ 45° from everything the target contains). The end-to-end
    /// verdicts live in `hazy_to_clean_fit_stays_sane` and
    /// `warm_rock_cast_must_not_violet_the_pale_sky`; this pins the primitive
    /// so a threshold tweak that flips one side fails HERE with numbers.
    #[test]
    fn foreign_hue_veto_separates_haze_from_canyon() {
        // Canyon: rebuild stage 4's exact inputs (fit minus its cast curves →
        // `cur`; curves re-derived and rendered → `with`).
        let src = canyon(false);
        let tgt = canyon(true);
        let s2 = src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
        let tp2 = pixels_of(&tgt.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE));
        let mut pre = fit_recipe(&src, &tgt).recipe;
        pre.red_curve = Vec::new();
        pre.green_curve = Vec::new();
        pre.blue_curve = Vec::new();
        let cur2 = pixels_of(&render::develop_preview(&s2, &pre));
        let mut with = pre.clone();
        with.red_curve = residual_channel_curve(&cur2, &tp2, 0);
        with.green_curve = residual_channel_curve(&cur2, &tp2, 1);
        with.blue_curve = residual_channel_curve(&cur2, &tp2, 2);
        assert!(
            !with.red_curve.is_empty(),
            "premise broken: the canyon pair no longer provokes a red cast curve"
        );
        let with2 = pixels_of(&render::develop_preview(&s2, &with));
        let cf = foreign_hue_bins(&tp2).expect("canyon target has chromatic mass");
        let created = foreign_share(&with2, &cf) - foreign_share(&cur2, &cf);
        assert!(
            cast_paints_foreign_hues(&cur2, &with2, &tp2),
            "veto must fire on the canyon cast (foreign share created {created:.4})"
        );
        assert!(created > 2.0 * VETO_CREATED_SHARE, "margin eroded: created {created:.4}");

        // Haze: same reconstruction on the accepted correction.
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        let s_img = base.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
        let tp = pixels_of(&clean.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE));
        let rec = fit_recipe(&base, &clean).recipe; // veto active: curves survive
        assert!(
            !(rec.red_curve.is_empty() && rec.green_curve.is_empty() && rec.blue_curve.is_empty()),
            "the haze correction's cast curves must survive the veto"
        );
        let mut pre_h = rec.clone();
        pre_h.red_curve = Vec::new();
        pre_h.green_curve = Vec::new();
        pre_h.blue_curve = Vec::new();
        let cur_h = pixels_of(&render::develop_preview(&s_img, &pre_h));
        let with_h = pixels_of(&render::develop_preview(&s_img, &rec));
        assert!(
            !cast_paints_foreign_hues(&cur_h, &with_h, &tp),
            "veto must NOT fire on the haze correction"
        );
    }

    #[test]
    fn warm_rock_cast_must_not_violet_the_pale_sky() {
        // Regression for the 2026-07-09 real-machine canyon failure: the target
        // warms the frame-dominant rocks and keeps the small pale sky blue. The
        // channel-CDF cast stage answers the rocks' demand with a global red
        // lift whose 5-knot interpolation drags the sky's red up too → violet
        // sky. The aggregate acceptance gate passed it because the rotated sky
        // is CROSS-BAND invisible to the hue term (mass lands in Purple/Magenta
        // — empty in the target — and drains out of Blue: the two-sided band
        // gate skips both). The pixel-aligned hue-damage veto must reject the
        // curves; saturation alone (hue-preserving) then matches the chroma.
        let src = canyon(false);
        let tgt = canyon(true);
        let rep = fit_recipe(&src, &tgt);
        // Render the fitted recipe and audit the sky region (rows y ≥ 108).
        let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
        let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
            for x in 0..192 {
                let p = out.get_pixel(x, y);
                let (r, g, b) =
                    (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
                if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                    continue; // desaturated sky pixels carry no hue verdict
                }
                let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
                sin += hue.sin();
                cos += hue.cos();
                n += 1.0;
            }
        }
        assert!(n > 0.0, "no chromatic sky pixels to audit — the fixture or a stage broke");
        {
            let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
            let d = (mean - 213.0 + 540.0).rem_euclid(360.0) - 180.0;
            assert!(
                d.abs() < 30.0,
                "sky hue drifted to {mean:.0}° (Δ{d:.0}°) — a violet/purple cast leaked through"
            );
        }
        // And rejecting the cast must not have made the overall fit worse.
        assert!(
            rep.err_after <= rep.err_before + 0.01,
            "fit made the look worse: {} -> {}",
            rep.err_before,
            rep.err_after
        );
    }

    /// Same geometry as [`canyon`], but the target regrades the WHOLE scene
    /// warm: rocks red-lifted AND the pale-blue sky replaced by a pale gold
    /// one ([0.92, 0.78, 0.58]: hue ≈ 35°, chroma ≈ 0.34, luma ≈ 0.80 vs the
    /// source sky's 0.67 — brighter, like the real golden-hour target). The
    /// destination hue is TARGET-NATIVE, so the foreign-hue veto stays silent
    /// — this models the 2026-07-09 real-machine failure #2 (reimagine-5):
    /// the hazy pale sky was rotated ~170° into the target's own orange.
    fn canyon_gold_target() -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = 0.15 + 0.80 * x as f32 / (w - 1) as f32;
                let p = if y < 16 {
                    [l, l, l]
                } else if y < 112 {
                    [(0.85 * l + 0.18).min(1.0), 0.52 * l, 0.30 * l]
                } else {
                    // PALE gold sky: bright golden-hour skies keep a HIGH blue
                    // channel (b ≈ 0.6) — the demanded blue curve is a gentle
                    // top-end dip (like the real pair's 255→188), not a global
                    // crush that would wake the aggregate gate on rock damage.
                    [0.92, 0.78, 0.58]
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    /// POLICY regression for real-machine failure #2 (2026-07-09, _DSC9621 ×
    /// reimagine-5): when the target's statistics demand rotating a large
    /// coherent chromatic region into a hue the target DOES populate (blue
    /// hazy sky → vivid target-native orange, ~170°), both existing gates
    /// pass — the foreign-hue veto by design (fit.rs "Known non-goal"), the
    /// aggregate ratio because the frame-dominant demand is genuine. From
    /// non-pixel-aligned statistics such a rotation is INDISTINGUISHABLE from
    /// content mismatch, so the policy is to refuse it: hue-preserving stages
    /// (tone + saturation) may chase the look, the cast curves may not
    /// re-hue a region. Deliberate cost: a true whole-scene regrade (sky
    /// genuinely gone gold) is not chased either — that expressiveness
    /// belongs to the zoned fit, not to global curves.
    #[test]
    fn cast_must_not_rotate_the_sky_into_a_target_native_hue() {
        let src = canyon(false);
        let tgt = canyon_gold_target();
        let rep = fit_recipe(&src, &tgt);
        let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
        let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
            for x in 0..192 {
                let p = out.get_pixel(x, y);
                let (r, g, b) =
                    (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
                if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                    continue;
                }
                let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
                sin += hue.sin();
                cos += hue.cos();
                n += 1.0;
            }
        }
        assert!(n > 0.0, "no chromatic sky pixels to audit — the fixture or a stage broke");
        {
            let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
            let d = (mean - 213.0 + 540.0).rem_euclid(360.0) - 180.0;
            assert!(
                d.abs() < 30.0,
                "sky hue rotated to {mean:.0}° (Δ{d:.0}°) — a target-native re-hue leaked through"
            );
        }
        assert!(
            rep.recipe.red_curve.is_empty()
                && rep.recipe.green_curve.is_empty()
                && rep.recipe.blue_curve.is_empty(),
            "the whole-scene regrade's cast curves must be withheld"
        );
        assert!(
            rep.err_after <= rep.err_before + 0.01,
            "fit made the look worse: {} -> {}",
            rep.err_before,
            rep.err_after
        );
    }

    /// The REAL-pair geometry (2026-07-09 #2, _DSC9621 × reimagine-5), where
    /// the rotation gate is the UNIQUE rejector — measured on this fixture:
    /// stage-4 ratio 0.450 (aggregate gate PASSES: crushing blue genuinely
    /// fixes the channel means frame-wide), foreign-hue veto false (the
    /// destination orange is target-native), rotation gate true (the hazy
    /// pale-blue sky re-hues ~170°). Unwiring the rotation gate from
    /// `fit_cast_stage` flips this test (curves accepted → orange sky),
    /// which the synthetic `canyon` pairs cannot detect: there the re-hue
    /// also damages the aggregate, so the ratio gate rejects redundantly.
    fn hazy_canyon_source() -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = 0.25 + 0.60 * x as f32 / (w - 1) as f32;
                let p = if y < 16 {
                    [l, l, l]
                } else if y < 112 {
                    // hazy desaturated rocks: warm but muted
                    [0.95 * l + 0.03, 0.88 * l + 0.03, 0.80 * l + 0.04]
                } else {
                    [0.60, 0.63, 0.67] // hazy pale-blue sky, hue ≈ 214°
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    fn vivid_warm_target() -> DynamicImage {
        let (w, h) = (192u32, 128u32);
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let l = 0.25 + 0.60 * x as f32 / (w - 1) as f32;
                let p = if y < 16 {
                    [l, l, l]
                } else if y < 112 {
                    [(1.05 * l + 0.15).min(1.0), 0.55 * l, 0.30 * l] // vivid warm rocks
                } else {
                    [0.92, 0.72, 0.48] // vivid gold sky
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn rotation_gate_is_the_unique_rejector_on_the_real_pair_geometry() {
        let src = hazy_canyon_source();
        let tgt = vivid_warm_target();
        // Pin the gate DECISIONS at stage 4 so this test keeps meaning "only
        // the rotation gate stands here" — if a fixture drift makes the ratio
        // gate reject too, the premise asserts below fail with numbers.
        let s2 = src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
        let tp2 = pixels_of(&tgt.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE));
        let rep = fit_recipe(&src, &tgt);
        let mut pre = rep.recipe.clone();
        pre.red_curve = Vec::new();
        pre.green_curve = Vec::new();
        pre.blue_curve = Vec::new();
        let cur = pixels_of(&render::develop_preview(&s2, &pre));
        let mut with = pre.clone();
        with.red_curve = residual_channel_curve(&cur, &tp2, 0);
        with.green_curve = residual_channel_curve(&cur, &tp2, 1);
        with.blue_curve = residual_channel_curve(&cur, &tp2, 2);
        assert!(!with.blue_curve.is_empty(), "premise broken: no blue crush demanded");
        let with_px = pixels_of(&render::develop_preview(&s2, &with));
        let ratio = look_err(&with_px, &tp2) / look_err(&cur, &tp2);
        assert!(
            ratio < CAST_ACCEPT_RATIO,
            "premise broken: the aggregate gate rejects too (ratio {ratio:.3}) — the \
             rotation gate is no longer uniquely load-bearing on this fixture"
        );
        assert!(
            !cast_paints_foreign_hues(&cur, &with_px, &tp2),
            "premise broken: the foreign-hue veto fires — destination should be target-native"
        );
        assert!(
            cast_rotates_a_region(&cur, &with_px),
            "the rotation gate must fire on the real-pair geometry"
        );
        // End-to-end: the fit must have withheld the curves (rotation gate is
        // the only rejector, per the premises above) and kept the sky blue.
        assert!(
            rep.recipe.red_curve.is_empty()
                && rep.recipe.green_curve.is_empty()
                && rep.recipe.blue_curve.is_empty(),
            "cast curves must be withheld on the real-pair geometry"
        );
        let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
        let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
            for x in 0..192 {
                let p = out.get_pixel(x, y);
                let (r, g, b) =
                    (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
                if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                    continue;
                }
                let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
                sin += hue.sin();
                cos += hue.cos();
                n += 1.0;
            }
        }
        assert!(n > 0.0, "no chromatic sky pixels to audit");
        let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
        let d = (mean - 214.0 + 540.0).rem_euclid(360.0) - 180.0;
        assert!(
            d.abs() < 30.0,
            "sky hue rotated to {mean:.0}° (Δ{d:.0}°) — the rotation gate is unwired"
        );
    }

    /// The rotation budget's discriminator, pinned on the three live pairs
    /// (calibration-probe numbers, 2026-07-09): the golden-sky regrade
    /// rotates 12.5% of the frame ~170° (must fire — both earlier gates are
    /// blind to a target-native destination), the violet cast 12.5% at 112°
    /// (must fire), the haze correction ≈0.01% past 60° and ~0 past 75°
    /// (must NOT fire). End-to-end verdicts live in
    /// `cast_must_not_rotate_the_sky_into_a_target_native_hue` /
    /// `warm_rock_cast_must_not_violet_the_pale_sky`; this pins the primitive
    /// so a threshold tweak that flips one side fails HERE with numbers.
    #[test]
    fn rotation_gate_separates_regrade_from_haze() {
        // Reconstruct stage-4's exact inputs for each pair, like the veto pin
        // test. Also reports whether the re-derived curves are non-empty, so
        // each leg can assert its premise (an empty-curve pair would make the
        // share trivially 0 and the leg vacuous).
        let stage4 = |src: &DynamicImage, tgt: &DynamicImage| {
            let s2 = src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
            let tp2 = pixels_of(&tgt.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE));
            let mut pre = fit_recipe(src, tgt).recipe;
            pre.red_curve = Vec::new();
            pre.green_curve = Vec::new();
            pre.blue_curve = Vec::new();
            let cur = pixels_of(&render::develop_preview(&s2, &pre));
            let mut with = pre.clone();
            with.red_curve = residual_channel_curve(&cur, &tp2, 0);
            with.green_curve = residual_channel_curve(&cur, &tp2, 1);
            with.blue_curve = residual_channel_curve(&cur, &tp2, 2);
            let nonempty = !(with.red_curve.is_empty()
                && with.green_curve.is_empty()
                && with.blue_curve.is_empty());
            let with_px = pixels_of(&render::develop_preview(&s2, &with));
            (cur, with_px, nonempty)
        };
        // Golden-sky regrade: destination hue is target-native, so neither
        // earlier hue veto sees it.
        let (c1, w1, ne1) = stage4(&canyon(false), &canyon_gold_target());
        assert!(ne1, "premise broken: the golden pair no longer provokes cast curves");
        let s1 = rehued_share(&c1, &w1);
        assert!(
            cast_rotates_a_region(&c1, &w1),
            "rotation gate must fire on the golden-sky regrade (share {s1:.4})"
        );
        assert!(s1 > 2.0 * ROT_SHARE, "margin eroded: golden share {s1:.4}");
        // Violet canyon: also caught here (112° ≥ 75°) — an independent net
        // under the foreign-hue veto.
        let (c2, w2, ne2) = stage4(&canyon(false), &canyon(true));
        assert!(ne2, "premise broken: the violet pair no longer provokes cast curves");
        let s2 = rehued_share(&c2, &w2);
        assert!(
            cast_rotates_a_region(&c2, &w2),
            "rotation gate must fire on the violet cast (share {s2:.4})"
        );
        assert!(s2 > 2.0 * ROT_SHARE, "margin eroded: violet share {s2:.4}");
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        let (c3, w3, ne3) = stage4(&base, &clean);
        assert!(ne3, "premise broken: the haze pair no longer provokes cast curves");
        let s3 = rehued_share(&c3, &w3);
        assert!(
            !cast_rotates_a_region(&c3, &w3),
            "rotation gate must NOT fire on the haze correction (share {s3:.4})"
        );
        // 0.1× also pins ROT_DEG from BELOW: at 45° the haze pair's share is
        // 0.0134 (0.27× ROT_SHARE) and would fail here; at 75° it measures
        // ≈ 0.0001.
        assert!(s3 < 0.1 * ROT_SHARE, "margin eroded: haze share {s3:.4} (measured ≈ 0)");
    }

    /// R18: the pass-through exemption's exact borders, patrolled (the R17
    /// disclosure left the band unmonitored). A rotation invisible on both
    /// ends (cc < 0.05 ∧ wc < 0.09) is exempt; crossing EITHER visibility
    /// floor puts it straight back in the census. If a real pair ever
    /// wrecks a region inside the exempt band, ROT_VISIBLE_* is the
    /// suspect (see the const doc).
    #[test]
    fn the_pass_through_exemption_borders_are_patrolled() {
        let share = |c: [f32; 3], w: [f32; 3]| rehued_share(&vec![c; 100], &vec![w; 100]);
        // Inside the blind band: a faint blue (cc 0.045) flipped ~174° to a
        // faint warm (wc 0.085) — pass-through, exempt.
        assert_eq!(share([0.655, 0.68, 0.70], [0.735, 0.68, 0.65]), 0.0);
        // After side crosses 0.09: the same faint blue painted a VISIBLE
        // warm — back in the census.
        assert!(share([0.655, 0.68, 0.70], [0.745, 0.68, 0.65]) > 0.99);
        // Before side crosses 0.05: a visible tint re-hued — counted even
        // though the destination stays faint.
        assert!(share([0.645, 0.68, 0.70], [0.70, 0.68, 0.65]) > 0.99);
    }

    #[test]
    fn rotation_census_sees_a_barely_tinted_before_side() {
        // H17: a faint blue (chroma 0.035 — UNDER the visible-tint gate,
        // above the measurable-hue floor) painted strong gold is a ~180°
        // re-hue; the old both-sides-visible census skipped it entirely,
        // reopening the golden-sky class one threshold to the left.
        let cur = vec![[0.665f32, 0.68, 0.70]; 1000];
        let with = vec![[0.92f32, 0.78, 0.58]; 1000];
        assert!(rehued_share(&cur, &with) > 0.99, "the whole faint-blue field re-hued");
        assert!(cast_rotates_a_region(&cur, &with));
        // A truly NEUTRAL before side has no hue to rotate — colourising
        // neutrals is what a corrective cast legitimately does.
        let neutral = vec![[0.68f32, 0.68, 0.68]; 1000];
        assert_eq!(rehued_share(&neutral, &with), 0.0);
    }

    /// The haze pair, whose cast curves are ACCEPTED — reused by several
    /// tests below, so the fixture is built once.
    fn haze_pair() -> (DynamicImage, DynamicImage) {
        let clean = synth();
        let mut haze = EditRecipe {
            exposure_ev: -0.3,
            contrast: -45.0,
            blacks: 40.0,
            saturation: -40.0,
            blue_curve: vec![
                CurvePoint { input: 0, output: 25 },
                CurvePoint { input: 128, output: 132 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        };
        haze.clamp();
        let base = render::develop_preview(&clean, &haze);
        (base, clean)
    }

    /// THE CALIBRATION RECORD for the joint value-range family (R23-6).
    ///
    /// Every threshold in `fit_zoned`'s joint block was set from this table
    /// and nothing else, so the table lives here as an executable
    /// assertion — a fixture drift that moves the numbers must fail loudly
    /// rather than quietly invalidate the constants.
    ///
    /// Measured (weighted reading, base → finished fit):
    ///   identity                     0.0000 → 0.0009   (pure quantisation)
    ///   roundtrip (known recipe)     0.0592 → 0.0044
    ///   haze → clean (cast kept)     0.1802 → 0.0446
    ///   canyon warm (violet class)   0.1767 → 0.0607
    ///   canyon gold (rotation class) 0.2428 → 0.0925
    ///   hazy canyon → vivid warm     0.5874 → 0.5813
    ///
    /// Two facts the constants rest on:
    ///   * SEPARATION — every honest fit lands at 0.004-0.093, while the one
    ///     pair whose target is a repaint no global model can reach (the
    ///     real-pair geometry of 2026-07-09 #2) stays at 0.58. That is what
    ///     [`fit_zoned::JOINT_FAR_ERR`] = 0.25 sits between, and it is the
    ///     whole point of the second reading: `look_err` scores that same
    ///     fit 0.080, i.e. confidence 0.52, a number the user reads as "it
    ///     mostly worked".
    ///   * MONOTONICITY — every pair improves or holds, the single exception
    ///     being the identity pair's +0.0009 of rounding. That is what
    ///     [`fit_zoned::JOINT_DRIFT_TOL`] = 0.05 has 56× of headroom over.
    ///
    /// HONEST STATUS: synthetic fixtures plus the two archived real-pair
    /// geometries they distil. The new real "the reverse-fit is nonsense"
    /// pair this round asked for did not arrive, so these constants are
    /// provisional pending a real-pair review.
    #[test]
    fn joint_family_is_calibrated_on_the_fixture_set() {
        let edge = ANALYZE_EDGE;
        let read = |src: &DynamicImage, tgt: &DynamicImage| -> (f32, f32, f32) {
            let s2 = src.thumbnail(edge, edge);
            let tp2 = pixels_of(&tgt.thumbnail(edge, edge));
            let rep = fit_recipe(src, tgt);
            let base_px = pixels_of(&render::develop_preview(&s2, &EditRecipe::default()));
            let fit_px = pixels_of(&render::develop_preview(&s2, &rep.recipe));
            let b = crate::fit_zoned::joint_reading(&base_px, &tp2).expect("base reading");
            let a = crate::fit_zoned::joint_reading(&fit_px, &tp2).expect("fit reading");
            (b.weighted, a.weighted, rep.err_after)
        };
        let mut honest_max = 0.0f32;
        for (name, src, tgt) in [
            ("identity", synth(), synth()),
            ("canyon warm", canyon(false), canyon(true)),
            ("canyon gold", canyon(false), canyon_gold_target()),
        ] {
            let (before, after, _) = read(&src, &tgt);
            assert!(
                after <= before + crate::fit_zoned::JOINT_DRIFT_TOL,
                "{name}: the fit must not push the joint reading past the drift \
                 tolerance ({before:.4} -> {after:.4})"
            );
            honest_max = honest_max.max(after);
        }
        {
            let (base, clean) = haze_pair();
            let (before, after, _) = read(&base, &clean);
            assert!(after < before * 0.5, "haze: {before:.4} -> {after:.4}");
            honest_max = honest_max.max(after);
        }
        {
            let src = synth();
            let mut truth = EditRecipe {
                exposure_ev: 0.35,
                contrast: 18.0,
                highlights: -25.0,
                whites: 12.0,
                saturation: 15.0,
                ..Default::default()
            };
            truth.clamp();
            let tgt = render::develop_preview(&src, &truth);
            let (before, after, _) = read(&src, &tgt);
            assert!(after < before * 0.5, "roundtrip: {before:.4} -> {after:.4}");
            honest_max = honest_max.max(after);
        }
        // The separation the FAR line lives in. Both sides asserted, so a
        // drift that closes the gap from either end fails here rather than
        // silently making the reading useless.
        let (_, unreachable, look) = read(&hazy_canyon_source(), &vivid_warm_target());
        assert!(
            honest_max < crate::fit_zoned::JOINT_FAR_ERR * 0.6,
            "the honest fits must stay well under the FAR line (worst {honest_max:.4})"
        );
        assert!(
            unreachable > crate::fit_zoned::JOINT_FAR_ERR * 2.0,
            "the unreachable repaint must stay well over the FAR line ({unreachable:.4})"
        );
        // …and the reason the second reading exists: the scalar calls that
        // same fit a partial success.
        assert!(
            look < 0.12,
            "premise: the scalar reports this unreachable fit as a modest \
             residual ({look:.4}) — if it ever reports it as far, this pair \
             no longer demonstrates the gap"
        );
    }

    /// The joint family may REPORT the worst bucket but must never gate on
    /// it — measured here, because the temptation is obvious and the data
    /// says the opposite. A change that fixes a bucket moves its members
    /// out of it, so a per-stage worst-bucket comparison inverts: the one
    /// correct cast in the fixture set gets worse by it, and the two casts
    /// that must be refused get better.
    #[test]
    fn the_worst_bucket_cannot_gate_a_stage() {
        let edge = ANALYZE_EDGE;
        let cast_pair = |src: &DynamicImage, tgt: &DynamicImage| -> (f32, f32) {
            let s2 = src.thumbnail(edge, edge);
            let tp2 = pixels_of(&tgt.thumbnail(edge, edge));
            let rep = fit_recipe(src, tgt);
            let mut pre = rep.recipe.clone();
            pre.red_curve = Vec::new();
            pre.green_curve = Vec::new();
            pre.blue_curve = Vec::new();
            let cur = pixels_of(&render::develop_preview(&s2, &pre));
            let mut with = pre.clone();
            with.red_curve = residual_channel_curve(&cur, &tp2, 0);
            with.green_curve = residual_channel_curve(&cur, &tp2, 1);
            with.blue_curve = residual_channel_curve(&cur, &tp2, 2);
            let with_px = pixels_of(&render::develop_preview(&s2, &with));
            (
                crate::fit_zoned::joint_reading(&cur, &tp2).expect("without").worst,
                crate::fit_zoned::joint_reading(&with_px, &tp2).expect("with").worst,
            )
        };
        let (base, clean) = haze_pair();
        let (haze_without, haze_with) = cast_pair(&base, &clean);
        assert!(
            haze_with > haze_without,
            "premise: the CORRECT cast makes the worst bucket read worse \
             ({haze_without:.4} -> {haze_with:.4})"
        );
        let (gold_without, gold_with) = cast_pair(&canyon(false), &canyon_gold_target());
        assert!(
            gold_with < gold_without,
            "premise: the cast that MUST be refused makes the worst bucket \
             read better ({gold_without:.4} -> {gold_with:.4})"
        );
        // The conclusion, stated as an assertion so it cannot rot: any
        // "worst bucket must not get worse" rule ranks these two the wrong
        // way round.
        assert!(
            haze_with - haze_without > gold_with - gold_without,
            "a worst-bucket drift gate would reject the correct cast before \
             the wrecking one"
        );
    }

    /// The confidence family is ONE calibration: the FAR warning fires
    /// exactly where the slope has already bottomed the number out. Two
    /// literals could drift apart silently; this is why they are named.
    #[test]
    fn the_confidence_family_is_one_calibration() {
        let bottom = (1.0 - CONFIDENCE_FLOOR) / CONFIDENCE_SLOPE;
        assert!(
            (bottom - FIT_FAR_ERR).abs() <= 0.01,
            "the FAR line ({FIT_FAR_ERR}) must be the residual at which the \
             slope reaches the floor ({bottom})"
        );
        assert_eq!(confidence_from_look_err(bottom + 0.001), CONFIDENCE_FLOOR);
        assert_eq!(confidence_from_look_err(0.0), CONFIDENCE_CEIL);
        // The joint ladder is its own calibration with the same shape.
        let jb = (1.0 - CONFIDENCE_FLOOR) / crate::fit_zoned::JOINT_CONFIDENCE_SLOPE;
        assert!(
            (jb - crate::fit_zoned::JOINT_FAR_ERR).abs() <= 0.01,
            "the joint FAR line must be the weighted reading at which its own \
             slope reaches the floor ({jb})"
        );
    }

    /// R23-6 A-2: the colour stage's SILENT arm. `ratio_fail` empties all
    /// three channel curves and used to push no note, while the hue gates
    /// beside it did disclose — so the commonest way for the colour stage to
    /// produce nothing was also the only one the user could not read about.
    ///
    /// The decision is pinned as a PURE function because no fixture in this
    /// repo reaches the ratio arm without a hue gate also firing (measured:
    /// of the six fixture pairs, 13 stage runs accept, 8 are hue-only
    /// rejections and 5 are both) — an end-to-end test would therefore pass
    /// on the hue note and prove nothing about the arm it is named for.
    #[test]
    fn a_silently_rejected_colour_stage_now_says_so() {
        use crate::rationale::keys;
        // The arm this test exists for: no hue damage, the aggregate simply
        // did not earn the risk.
        assert_eq!(
            CastOutcome { rehue_blocked: false, ratio_rejected: true }.note_key(),
            Some(keys::FIT_NOTE_CAST_REJECTED)
        );
        // The hue note wins a double rejection — more specific, and the
        // thing worth saying.
        assert_eq!(
            CastOutcome { rehue_blocked: true, ratio_rejected: true }.note_key(),
            Some(keys::FIT_NOTE_REHUE_BLOCKED)
        );
        assert_eq!(
            CastOutcome { rehue_blocked: true, ratio_rejected: false }.note_key(),
            Some(keys::FIT_NOTE_REHUE_BLOCKED)
        );
        // An ACCEPTED stage says nothing — a note on every fit is noise.
        assert_eq!(CastOutcome::default().note_key(), None);

        // …and the end-to-end property that follows from it: whenever the
        // colour stage ships nothing, SOMETHING explains it.
        for (name, src, tgt) in [
            ("canyon warm", canyon(false), canyon(true)),
            ("canyon gold", canyon(false), canyon_gold_target()),
            ("hazy canyon", hazy_canyon_source(), vivid_warm_target()),
        ] {
            let rep = fit_recipe(&src, &tgt);
            let empty = rep.recipe.red_curve.is_empty()
                && rep.recipe.green_curve.is_empty()
                && rep.recipe.blue_curve.is_empty();
            if !empty {
                continue;
            }
            assert!(
                rep.notes.iter().any(|n| {
                    n.key == keys::FIT_NOTE_CAST_REJECTED
                        || n.key == keys::FIT_NOTE_REHUE_BLOCKED
                        || n.key == keys::FIT_NOTE_REGRESSED
                }),
                "{name}: an empty colour stage must disclose WHY: {}",
                rep.recipe.rationale
            );
        }
    }

    /// R23-6 A-5: the disclosure names controls for THIS pair, not the
    /// blanket sentence every fit carries. The canyon-gold pair's residual
    /// is a per-band hue move the solver has no dial for.
    #[test]
    fn the_unsolvable_controls_are_named_for_this_pair() {
        let rep = fit_recipe(&canyon(false), &canyon_gold_target());
        let note = rep
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED)
            .unwrap_or_else(|| panic!("no specific disclosure: {}", rep.recipe.rationale));
        let controls = &note.args.iter().find(|(k, _)| *k == "controls").expect("arg").1;
        assert!(controls.contains("hsl"), "expected the colour mixer named, got {controls}");
        // And it must NOT fire on a pair the solver actually reproduces —
        // a disclosure that always fires is the blanket sentence again.
        let (base, clean) = haze_pair();
        let good = fit_recipe(&base, &clean);
        assert!(
            !good
                .notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED),
            "a fit that lands must not claim a missing control: {}",
            good.recipe.rationale
        );
    }

    /// R23-6 B-7: any file may now be the reverse-fit target, so a
    /// reference that is not this frame must be WARNED about — and not
    /// refused: the user chose the file.
    #[test]
    fn a_differently_shaped_reference_is_warned_about_not_refused() {
        let src = synth(); // 192x128, aspect 1.5
        // A genuinely different shape — `thumbnail` PRESERVES aspect, so it
        // cannot build this case; cropping can.
        let tall = synth().crop_imm(0, 0, 96, 128); // aspect 0.75
        assert!(!same_frame_plausible(&src, &tall));
        assert!(same_frame_plausible(&src, &synth()));
        // A resize of the SAME frame must not trip it: aspect survives
        // `thumbnail`, and its integer rounding is what the tolerance is for.
        assert!(same_frame_plausible(&src, &synth().thumbnail(97, 97)));
        let rep = fit_recipe(&src, &tall);
        assert!(
            rep.notes
                .iter()
                .any(|n| n.key == crate::rationale::keys::FIT_NOTE_NOT_SAME_FRAME),
            "the doubt must be disclosed: {}",
            rep.recipe.rationale
        );
        // Refused would be wrong: a recipe still comes back.
        assert!(rep.err_after.is_finite());
    }

    /// The joint family's ADDITIONAL terminal veto (R23-6 C, role 3). No
    /// fixture in this repo reaches it — by design, since it has 56× headroom
    /// over the largest non-improvement measured — so the decision itself is
    /// pinned here, both arms and the fail-open direction.
    #[test]
    fn the_terminal_check_reads_both_metrics_and_fails_open() {
        use crate::fit_zoned::{JointReading, JOINT_DRIFT_TOL};
        let j = |w: f32| {
            Some(JointReading { worst: w, worst_label: "shadows/colour", weighted: w, buckets: 6 })
        };
        // A clean fit: neither arm objects.
        assert_eq!(
            terminal_harm(0.09, 0.03, j(0.18), j(0.04)),
            TerminalHarm { scalar: false, joint: false }
        );
        // The scalar arm alone (R16's rule, untouched).
        assert!(terminal_harm(0.010, 0.030, j(0.10), j(0.02)).scalar);
        // The JOINT arm alone — the case that motivated it: the frame-global
        // number improves while the value ranges are driven apart.
        let joint_only = terminal_harm(0.09, 0.03, j(0.10), j(0.10 + JOINT_DRIFT_TOL + 0.01));
        assert!(joint_only.joint && !joint_only.scalar);
        assert!(joint_only.any(), "the two arms are OR-ed, not AND-ed");
        // …and it is BOUNDED: drift inside the tolerance is not harm.
        assert!(!terminal_harm(0.09, 0.03, j(0.10), j(0.10 + JOINT_DRIFT_TOL - 0.01)).joint);
        // FAIL-OPEN in both directions: no reading ⇒ no verdict from this
        // arm, never a silent pass dressed as approval.
        assert!(!terminal_harm(0.09, 0.03, None, j(0.9)).joint);
        assert!(!terminal_harm(0.09, 0.03, j(0.0), None).joint);
        // …but the scalar arm still stands on its own when it does.
        assert!(terminal_harm(0.01, 0.9, None, None).any());
        // It can only REJECT: a joint reading that improves cannot rescue a
        // recipe the scalar convicts.
        assert!(terminal_harm(0.010, 0.030, j(0.90), j(0.001)).any());
    }

    /// The joint reading may only LOWER the reported confidence, and the
    /// case it exists for is the one the scalar over-reports.
    #[test]
    fn the_joint_reading_can_only_lower_confidence() {
        let rep = fit_recipe(&hazy_canyon_source(), &vivid_warm_target());
        let scalar_alone = confidence_from_look_err(rep.err_after);
        assert!(
            rep.recipe.confidence < scalar_alone - 0.1,
            "the unreachable repaint must not keep the scalar's claim \
             ({scalar_alone:.2} vs reported {:.2})",
            rep.recipe.confidence
        );
        assert!(rep.recipe.confidence >= CONFIDENCE_FLOOR);
        // …and on a fit that genuinely lands, it must not invent doubt.
        let (base, clean) = haze_pair();
        let good = fit_recipe(&base, &clean);
        assert!(
            good.recipe.confidence >= 0.75,
            "a landed fit must keep its confidence, got {}",
            good.recipe.confidence
        );
    }

    #[test]
    fn quantile_and_cdf_are_inverse_on_a_ramp() {
        let px: Vec<[f32; 3]> = (0..4096)
            .map(|i| {
                let v = i as f32 / 4095.0;
                [v, v, v]
            })
            .collect();
        let cdf = luma_cdf(&px);
        for &x in &[0.1f32, 0.25, 0.5, 0.75, 0.9] {
            let p = cdf_at(&cdf, x);
            let back = quantile(&cdf, p);
            assert!((back - x).abs() < 0.01, "x={x} → p={p} → {back}");
        }
    }
}
