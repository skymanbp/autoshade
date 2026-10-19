//! The zoned fit's limits: the layer options, zone share and pixel floors, the per-zone budgets, the boundary maxima and the acceptance ratios.

pub(super) const MASK_REFINE_RADIUS: u32 = 8;
pub(super) const MASK_REFINE_EPSILON: f32 = (4.0 / 255.0) * (4.0 / 255.0);

#[derive(Clone, Copy)]
pub(super) struct ZonedLayerOpts {
    pub(super) field: bool,
    pub(super) spatial: bool,
    pub(super) free_masks: bool,
    pub(super) refine_masks: bool,
}

pub(super) const SHIPPED_LAYERS: ZonedLayerOpts = ZonedLayerOpts {
    field: true,
    spatial: true,
    free_masks: true,
    refine_masks: true,
};

/// A zone must cover at least this weighted share of ITS frame on BOTH sides
/// to carry trustworthy moments (a real sky measures 10–40%; segmentation
/// misses and boundary slivers sit far below).
pub(crate) const MIN_ZONE_SHARE: f32 = 0.03;
/// ONE footprint floor for every hard-raster mask family — spatial tiles and
/// free-form components alike.  A component under 64 analysis pixels is an 8x8
/// patch of the 384-edge grid: below the structural instrument's own resolvable
/// core ([`fit::STRUCTURE_MIN_CORE_PX`], which its three-pixel erosion puts out
/// of reach for anything this small), and under a quarter of the 3-px baseline
/// the boundary-step ruler needs on each side of a crossing.  The free-mask
/// producer carried this floor from its first batch while the tile producer had
/// none, which is the asymmetry v1.2.4 closes: the two families are gated by
/// the same rulers downstream, so they are admitted by the same floor.
pub(crate) const MIN_MASK_PIXELS: usize = 64;
/// Conservative local-exposure budget: ±2.5 EV covers any real sky-to-sky
/// brightness gap; a larger demand means the zones do not correspond.
pub(super) const ZONE_EV_LIMIT: f32 = 2.5;
/// Atmosphere zones keep only a restrained local brightness move.
pub(super) const ZONE_ATMOS_EV_LIMIT: f32 = 0.75;
/// Local saturation shares the global fit's model cap (fit.rs stage 3).
pub(super) const ZONE_SAT_LIMIT: f32 = 60.0;
pub(super) const ZONE_ATMOS_SAT_LIMIT: f32 = 20.0;
/// The window an Atmosphere zone's recolour is shrunk into at the SHIPPED
/// DEFAULT strength. Since R33 §F it is one point on
/// [`fit::FitBudget::zone_gain`]'s ladder rather than a fixed pair: these two
/// numbers are what `FitBudget::for_strength(DEFAULT)` returns, and they are
/// kept named so the default stays readable at its definition and a test can
/// pin the two against each other.
pub(crate) const ZONE_ATMOS_GAIN_MIN: f32 = 0.85;
pub(crate) const ZONE_ATMOS_GAIN_MAX: f32 = 1.18;
/// Mask-weighted mean-gradient energy PER UNIT of mean luma
/// ([`super::zone_fit::local_quality`]) may not fall below this ratio. The
/// window was calibrated on the ABSOLUTE gradient, where accepted repository
/// zones measured 0.730, 0.980, 1.084, 1.330, 1.684 and 1.918 and the saved
/// generated-cloud correction 0.961 — inseparable from them, a contradiction
/// pinned in the fixture test rather than hidden by a false cap. Since
/// 2026-10-19 the reading divides each side's brightness out, so an exposure
/// move no longer counts as texture; the window is kept, and the same
/// generated-cloud correction now reads 1.024.
pub(super) const ZONE_TEXTURE_MIN: f32 = 0.70;
/// The ceiling of the same per-unit ratio: 1.95, headroom over the absolute
/// calibration's accepted maximum of 1.918 (the list above).
pub(super) const ZONE_TEXTURE_MAX: f32 = 1.95;
/// Weighted clipped-luma share may grow by at most one percentage point.
pub(super) const ZONE_CLIP_GROWTH: f32 = 0.01;
/// Maximum luma rim the correction may INTRODUCE across the 5%-95% mask
/// feather. The statistic is the 90th percentile, BY MAGNITUDE, of
/// `rendered - transported(reference)` at each transition run's largest
/// deviation, on rows/columns carrying both settled interiors — see
/// [`boundary_line_rims`] for why the reference is transported through the
/// settled sky's own linear multiplier and why the rank is a magnitude.
///
/// EVERY PROBE VALUE BELOW WAS RE-MEASURED FOR STEP 9, because the statistic
/// changed: what preceded it read the corrected render against its own
/// settled sky with no reference at all, so it carried a content floor and
/// ranked its samples signed. At the 384-edge analysis size, first-party this
/// batch:
///
/// * synthetic bright-half probe +0.120, just-over-budget opposite-sign probe
///   +0.013 — unchanged, and unchanged BY ARITHMETIC rather than by luck: the
///   fixture's settled sky is 0.20 on every row, so the reference reading is
///   exactly 0 and the transport multiplier is exactly 1.
/// * same-sign probe +0.020, which was -0.020. The shape did not move; the
///   RANK did (`|-0.020| == 0.0200`), and the gate still keeps that fixture at
///   k=1 because the bow is in the reference too.
/// * the four accepted repository fixture entries +0.012 each, reached by a
///   shrink instead of passing free: candidates +0.046 (k=0.244) and +0.019
///   (k=0.852). The move is the REFERENCE, not the rank — measured on that
///   fixture the previous statistic's signed 90th percentile is -0.0044 and
///   its MAGNITUDE 90th percentile 0.0044, against an introduced rim of
///   0.0461. The zone was shipping a rim it had itself made, invisible
///   because the scene's own bow under the feather cancelled it.
/// * the real calibration pair +0.030 before the gate and +0.012 after its
///   largest passing shrink k=0.371, over 731 measured transitions; its sky
///   zone consequently keeps -0.184 EV where the absolute ruler had shrunk
///   it into -0.12..-0.15 EV to pay for a bow it had not introduced.
///
/// NOT re-measured, and therefore NOT carried forward: the "no-zone
/// calibration +0.013", "previous fitted pair -0.009" and "HEAD's
/// opposite-sign pair +0.054" probes this block used to quote. They were
/// bespoke measurements of the replaced statistic and no instrument in the
/// tree reproduces them; quoting them against this ruler would be a number
/// with no source.
///
/// THE CONSTANT ITSELF STAYS AT +0.012, deliberately and not by inertia. It
/// was calibrated where neighbourhood contrast can mask a discontinuity of
/// this size — real textured and feathered borders — and it is NOT a
/// visibility threshold that holds everywhere: v1.2.2 measured a tile seam
/// in clean sky at 7.8 sigma over a mask-free neutral control while sitting
/// exactly on this number. It is therefore a CEILING here too, exactly as
/// [`ZONE_BOUNDARY_STEP_MAX`] has been since v1.2.2, and no longer a flat
/// budget. THIS ruler used to keep the scalar, on the argument that it only
/// ever samples INSIDE a feathered transition band "where the correction is
/// a ramp by construction". A desert-dusk pair falsified that argument: a
/// semantic sky raster is the segmentation model's own soft class
/// probability, so in featureless haze its band is the two or three analysis
/// pixels the model happened to emit, and the flat ceiling let the shared
/// shrink park the correction exactly on it — a measured 0.013-0.016 luma
/// step across the 50% contour in the finished 2048-px render (p50 0.018
/// with the mask against 0.005 without, 8-px stand-off, 154 columns), 3-4
/// codes of 255 in a smooth gradient, where the hard family's own ruler
/// would have budgeted one code. Every transition is now charged per
/// crossing against its own context ([`crossing_budget`],
/// [`BOUNDARY_STEP_FLOOR`]), in luma AND per channel, and this constant is
/// the CAP on that exchange, not a promise about bare-sky visibility.
/// No perceptual study is owed for this number, and none is planned: it is a
/// calibrated exchange rate, not a visibility threshold, and the batch that
/// questioned it measured what the rate actually buys — the calibration
/// island's seam fell from +3.15 to +0.92 code values under this budget and
/// is not visible at 1:1. A number re-derived from a fresh threshold study
/// would replace a measurement that exists with a preference that does not. It is its own constant: [`ZONE_BOUNDARY_STEP_MAX`] carries the
/// hard-raster ruler's ceiling, so moving either one can no longer
/// silently re-tune the other. The supervisor's independent RAW rim metric
/// remains the final regression check, because it samples a 40px crossing
/// neighbourhood rather than this analysis-grid statistic.
///
/// THE "RAMP BY CONSTRUCTION" CLAIM ABOVE IS NOW MEASURED FOR ONE OF THE
/// TWO FAMILIES THAT MAKE IT (2026-09-01). The luminance-range ruler's own
/// scalar (`range::RANGE_BOUNDARY_RIM_MAX`, which carries the table) was
/// driven on the two real pairs where a band attaches with segmentation
/// unavailable: the delivered transitions are monotone ramps (0 reversals
/// over 30 and 19 populated 1-code bins of the delivered transfer, minimum
/// slope +0.019 and +0.855, against that estimator's own +/-0.05 noise
/// floor), the mask-free ruler reads p90 0.0018 luma against a control of
/// exactly 0.0000, and on the basis the engine actually gates against —
/// the globals-only twin — the correction moved that gate's own p90 DOWN
/// on both pairs (0.00392 -> 0.00230 and 0.00874 -> 0.00857). That ruler
/// reports the RENDERED gradient on already-smooth crossings, so the
/// scene's own gradient is inside the reading rather than differenced
/// away, and the budget is spent on the scene first. It is a p90 like this
/// one, so the maxima neither ruler ranks moved the other way (0.01217 and
/// 0.01407 on those pairs) while the ranked reading fell.
///
/// THIS ruler's half was measured on 2026-09-02, with the segmentation
/// sidecar running under the shared GPU lock. `match --zoned` over the six
/// corpus pairs attaches a feathered semantic zone on three of them, and this
/// ruler reads 745 / 691 / 793 transitions inside their transition bands —
/// it is LIVE on the family it was written for, not returning the empty
/// `0 transitions` that let every hard raster past it. The ceiling binds
/// where the correction is large and does not where it is not: introduced
/// rims of 0.029 and 0.040 luma shrink to exactly +0.012 at k=0.385 and
/// k=0.271, while a mild zone introducing 0.006 is kept whole at k=1.000.
/// The constant therefore STANDS on its own family's evidence: reached from
/// above on real pairs, and not a tax on a zone that stays under it.
/// What those runs do not carry is the range family's second instrument (a
/// monotone delivered transfer plus a mask-free control), which reads a
/// rendered frame this path never writes to disk; the gate readings above
/// are what this ruler is held to.
pub(crate) const ZONE_BOUNDARY_RIM_MAX: f32 = 0.012;
/// Maximum induced cross-boundary step for a HARD 0/255 raster — spatial
/// tiles and free masks, read by [`boundary_step`] rather than
/// [`boundary_rim`]. Split out of [`ZONE_BOUNDARY_RIM_MAX`] because the two
/// rulers measure two different shapes on two different mask families, and
/// while ONE constant served both, re-deriving either budget silently
/// re-tuned the other. That coupling is not theoretical: the accepted tiles
/// of the calibration island park ON this ceiling — 0.1063 → 0.0118 after
/// k=0.114, 0.0732 → 0.0116 after k=0.168, 0.0675 → 0.0118 after k=0.187,
/// i.e. 0.0002/0.0004/0.0002 luma of slack (1.7%/3.3%/1.7% of the budget) —
/// so any downward move re-shrinks real tiles and must be argued on the step
/// ruler's own evidence. Carried over at the rim's calibrated +0.012 — but
/// as a CEILING, not a flat budget: each crossing is charged against its own
/// per-crossing budget — since R40 the scene's own DISCONTINUITY at that
/// crossing ([`discontinuity_budget`]), clamped to `[BOUNDARY_STEP_FLOOR,
/// this constant]` — and the gate compares the charged 90th percentile. A
/// crossing on a scene edge that can mask the whole ceiling is charged its
/// raw step — bit-identical to the scalar rule — while one in smooth sky,
/// gradient or not, is charged at the one-code floor's exchange rate, which
/// is what stops a tile seam from hiding inside a budget calibrated on
/// texture. (The island numbers above are the pre-R40 ruler's: it also
/// credited the scene's gradient and the correction's slope, which R40
/// removed — the R40 note on [`discontinuity_budget`] says why.)
pub(crate) const ZONE_BOUNDARY_STEP_MAX: f32 = 0.012;
/// Per-crossing floor of the contextual step budget: ONE code value of the
/// 8-bit analysis render. Three derivations agree. (1) Instrument:
/// `fit::pixels_of` is `to_rgb8()/255`, so below one code the ruler ranks
/// its own rounding, not seams. (2) Perception: unmasked step detection on
/// a uniform field is ~0.5-1% Weber, which at mid-grey in a gamma-2.2
/// encoding is ~0.6 code — one code sits at-or-just-above bare threshold on
/// a flat patch, which is why 8-bit banding is visible at all. (3) Ruler
/// noise: each sample is a difference of two differences of ROUNDED values
/// — four quantisations — whose p90 on a gently graded patch is ~0.95
/// code; a floor below that would gate quantisation noise. (R40: the hard
/// family's de-trended reading adds the two flank samples at quarter weight
/// on each of its two frames — per channel 6.5/12 code² of rounding variance
/// against the plain step's 4/12, a p90 of ~0.94 code against ~0.74, still
/// under this floor; three-pixel flanks would put it at ~1.17 code, over it,
/// which is why [`STEP_FLANK_BASELINES`] is two.)
pub(crate) const BOUNDARY_STEP_FLOOR: f32 = 1.0 / 255.0;
/// Exchange rate of the SOFT family's slope term (the hard family stopped
/// reading a slope at R40: a ramp is not a discontinuity and needs no credit
/// to stay whole): a correction whose own same-side
/// variation over the 3-px baseline is `s` earns a budget of `3 * s`
/// before the clamp, so a genuine ramp saturates the ceiling and stays
/// whole. The slope is the MINIMUM over two consecutive same-side
/// baselines, because a hard raster's resample-and-refine collar spans
/// exactly the first baseline out and reads as a spurious inner slope —
/// the seam's own soft shoulder must never buy the seam its budget; a
/// true ramp shows the same slope on both baselines and keeps full
/// credit. Calibrated on `feathered_fixture` (+0.55 EV over a 32-px alpha
/// ramp): the measured same-side slope is ~0.0089 luma on either
/// baseline, and 3 x 0.0089 = 0.0267 clamps to the ceiling with a 2.2x
/// margin — it saturates even if quantisation reads the slope a full
/// code low (3 x 1 code = 0.0118 ~ ceiling). A value of 2 sits on that
/// coin flip; 3 does not.
pub(crate) const BOUNDARY_STEP_SHAPE: f32 = 3.0;
/// Acceptance: the zone-local error ([`zone_err`]) must fall to ≤ this
/// fraction of its pre-correction value. The correction is judged on ITS
/// zone, not on the frame-global `look_err` — measured on the real pair
/// (2026-07-09, P21 × reimagine-5): the sky correction landed the zone
/// moments almost exactly on the target's (zone error 0.507 → 0.015) while
/// the FRAME-global metric moved 0.1768 → 0.1792, because the generative
/// target holds ~3× more sky area than the source (the composition differs —
/// no zone repaint can reconcile frame-level distributions) and a correct
/// blue→gold repaint migrates band mass, which the worst-band hue term can
/// only read as damage. A frame-global gate therefore vetoes exactly the
/// correction this module exists to make.
pub(super) const ZONE_ACCEPT_RATIO: f32 = 0.5;
/// The relative gate above has no absolute yardstick, and that produced the
/// R17-era complaint: a zone that was ALREADY matched (sky 0.012 on the
/// murk-era pair) was "corrected", barely moved, and reported "dropped:
/// needs ≤ 50%" — reading like a discarded improvement when there was
/// nothing to improve. Two absolute yardsticks fix that, SPLIT on purpose
/// (R19 — one shared number either skipped fixable zones or dialled
/// matched ones): corrections that genuinely work LAND at 0.007–0.015
/// (this pair's sky 0.076 → 0.007; golden-sky 0.507 → 0.015), so this
/// figure — just above that landing range — is the ACCEPTANCE floor: a
/// correction ending at/below it with a real gain is accepted even when
/// the relative arm alone would refuse (started close, ended matched).
pub(super) const ZONE_MATCHED_ERR: f32 = 0.02;
/// …while the observed already-matched zones read 0.009–0.012 and every
/// attempt at them regressed (land 0.009 → 0.029 on the live pair), so
/// THIS figure — the ceiling of that observed matched domain — is the
/// SKIP line: at/below it the zone is left alone with an honest "already
/// matches" note. Zones between the two figures are attempted and judged
/// by [`zone_accepts`]; nothing is declined untried above the matched
/// domain.
pub(super) const ZONE_SKIP_ERR: f32 = 0.012;
/// `zone_err` lives in LINEAR light, so an absolute line alone means
/// different things at different zone levels — 0.012 of linear mean is a
/// hundredth of a stop on a bright sky but most of a stop in deep shadow
/// (sRGB ≈ 0.12 vs 0.173 zones score zone_err ≈ 0.012 while sitting
/// 0.9 EV apart; that zone must be FITTED, not declared matched). Both
/// absolute yardsticks therefore carry this quarter-stop EV companion —
/// the skip line refuses to declare such a zone matched, and the
/// acceptance floor refuses to call such a landing matched; the relative
/// acceptance arm needs no companion because ratios are scale-free.
pub(super) const ZONE_MATCHED_EV: f32 = 0.25;
/// The floor-landing acceptance arm must still MOVE the zone — without a
/// minimum gain, a hairline 0.0201 → 0.0200 "landing" would buy the full
/// [`ZONE_GLOBAL_REGRESSION_TOL`] drift budget (200× the zone gain) and
/// overwrite `err_after` with the worse frame number. One fifth of the
/// starting error is the smallest move worth a mask.
pub(super) const ZONE_FLOOR_MIN_GAIN: f32 = 0.8;
/// Insurance bound: the mask cannot touch pixels outside its raster (engine
/// guarantee, pinned by the rocks-bit-equal test), so the only frame-global
/// drift a correct zone repaint can cause is metric-visible band migration
/// inside its own region. Allow that small, measured drift (+0.0024 on the
/// real pair) but refuse anything larger — a big global regression means the
/// mask is NOT the region we thought it was.
pub(super) const ZONE_GLOBAL_REGRESSION_TOL: f32 = 0.02;
/// Smallest ABSOLUTE zone gain the strictly-better arm will buy a mask for
/// (R30 batch 1). Derivation, in two independent legs that meet at the same
/// figure:
///
/// 1. The one measured instance the arm exists for is the calibration land
///    zone: 0.078 -> 0.054, an absolute gain of 0.024, with the frame moving
///    -0.00004 and the quality gates clear. The task book names that 0.024 as
///    the calibration ANCHOR and asks for the floor to be derived below it and
///    the derivation disclosed; it is the smallest gain yet observed to be
///    worth buying, NOT the threshold itself. Reading it as the threshold
///    would make this arm's strict `<` refuse the very zone it exists for.
///    n = 1, so the floor is set a factor of two under the anchor rather than
///    on it -- one sample cannot justify a threshold its own value sits on.
/// 2. 0.012 is already this module's [`ZONE_SKIP_ERR`], the ceiling of the
///    observed already-matched domain: a zone anywhere at or below it is
///    declared matched and left alone. A gain SMALLER than that whole domain
///    is therefore not worth a bitmap mask (engine-only, a named XMP loss,
///    a raster per correction) by the module's own yardstick.
///
/// It is deliberately a separate constant rather than a reference to
/// `ZONE_SKIP_ERR`: the two are calibrated from different measurements and
/// only happen to agree today.
pub(super) const ZONE_MIN_ABS_GAIN: f32 = 0.012;
