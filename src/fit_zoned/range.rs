use super::*;
use super::field::FieldBandProposal;

/// Four bands is the existing measured stability ceiling for value-range
/// evidence; finer partitions routinely fall below the evidence floor.
pub(super) const RANGE_MAX_BANDS: usize = 4;
/// Corrected rank-mean calibration keeps `0.03` between supported neutral
/// bins (01-07 and 12, at most `0.025`) and the coherent 08-11 run (starting
/// at `0.036`); the isolated supported bin 13 measures `0.223`.
const RANGE_RESIDUAL_TRIGGER: f32 = 0.03;
/// One 17-bin evidence interval is the minimum transition width; the retained
/// opposite-half-EV probe measured a 5/255 step when that protection vanished.
const RANGE_MIN_RAMP: f32 = 1.0 / 17.0;
/// Two evidence intervals provide measured transition headroom before the
/// shared boundary shrink has to reduce correction differentials.
///
/// # The re-cut probe, and what the widest ramp actually does
///
/// v1.2.3 could not measure this width. Its synthetic probe was 512 rows and
/// the mask-free instrument (`scripts/rim_overshoot.py`) needs 180 px of
/// margin on each side of the boundary it locates, so every column of the 2/17
/// rows was rejected and the table carried "unmeasurable", not "measured
/// clean". That probe also pinned the band at ONE luminance position.
///
/// The probe is re-cut in
/// `tests::the_recut_ramp_probe_measures_every_ramp_the_producer_emits`: a
/// 64 x 1020 vertical grey ramp, a 4/17-wide band, the instrument's own
/// 60/60/60 geometry ported unchanged, and the band's position swept across
/// five values. Every cell now returns n = 64 of 64 columns, including the
/// 2/17 rows nothing could read before.
///
/// TWO RULERS, AND THEY DISAGREE BY CONSTRUCTION. The mask-free overshoot
/// reads mean 0.0000 and max 0.0000 in ALL 90 cells — every ramp, every EV,
/// every position — because the DIFFERENCE it ranks stays monotone even where
/// the delivered luma does not. The tone-order reading
/// ([`range_transfer_reversal`], the depth of the delivered transfer's
/// non-monotone excursion) is the ruler that sees the rest, and its control
/// (the same frame at 0.00 EV) is exactly 0.000000 in all 15 of its cells.
/// Depths below, in 8-bit codes, columns = band centre luma:
///
/// ```text
/// ramp 1/17    0.36 0.43 0.50 0.57 0.64     ramp 1.5/17   0.36 0.43 0.50 0.57 0.64
///  -0.35 EV       0    0    0    1    2      -0.35 EV        0    0    0    0    0
///  -0.56 EV       0    1    4    7    8      -0.56 EV        0    0    1    2    3
///  -0.80 EV       3    6    9   13   17      -0.80 EV        0    1    5    7   10
///  -1.10 EV       7   12   16   21   26      -1.10 EV        2    7   11   15   19
///  -1.50 EV      13   19   25   31   38      -1.50 EV        8   13   18   25   31
///
/// ramp 2/17 (THIS CONSTANT)   0.36 0.43 0.50 0.57 0.64
///  -0.35 EV                      0    0    0    0    0
///  -0.56 EV                      0    0    0    0    0
///  -0.80 EV                      0    0    1    3    6
///  -1.10 EV                      0    2    5    9   14
///  -1.50 EV                      3    7   13   19   24
/// ```
///
/// So the widest ramp is the SAFEST of the three at every cell — which is the
/// case for this ceiling and not against it — but it is not safe: at -0.80 EV
/// and above it inverts too, and the position sweep is what shows it (the same
/// EV reads 0 codes low in the frame and 6 high in it). The viaduct's real 2/17
/// band at -0.80 EV sits at codes 0-30, the bottom of that row, which is why it
/// measures clean. Nothing here asks for a different width: a wider ramp is
/// monotonically better on this table and the ceiling is already the widest the
/// producer emits, while the inversions are closed by the sign test on the
/// delivered transfer ([`RANGE_TRANSFER_REVERSAL_MAX`]) rather than by a re-tune.
const RANGE_MAX_RAMP: f32 = 2.0 / 17.0;
/// Native range transitions reuse the calibrated zoned signed-rim budget
/// — MEASURED as defensible in that role on 2026-09-01, no longer carried
/// over on the shared number alone.
///
/// THIS RULER IS NOT `boundary_step`. It never differences the scene away:
/// `range_transition_rim` admits a neighbouring pair only where the
/// REFERENCE crossing is already smooth (`|dl| <= 2.5/255`) and then
/// reports the RENDERED gradient there, so the scene's own gradient is
/// INSIDE the reading and is spent before the correction adds anything.
/// That argument is not new — the comment inside `range_transition_rim`
/// has said since v1.2.2 that a graded context here is capped near 2.5/255
/// by construction and so has no dynamic range. What this block adds is
/// the MEASUREMENT behind it and a test that pins the scale.
///
/// CHARGING HERE WOULD OVER-TIGHTEN, NOT LOOSEN. v1.2.2's charge is
/// `raw * (MAX / budget)` with `budget` clamped at or below `MAX`
/// (`fit_zoned.rs`, `boundary_step`), so the multiplier is >= 1 by
/// construction and can never weaken a gate. For THIS ruler the context
/// term is the admitted crossing itself, capped at 2.5/255 = 0.0098 —
/// always under this 0.012 ceiling — so every crossing would be charged at
/// least 1.22x and the pass condition would collapse to
/// `scene + correction <= scene`: no steepening admitted anywhere, which
/// refuses nearly every band. The tile seam v1.2.2 fixed is the opposite
/// failure mode: the step ruler CANCELS the scene and then handed clean
/// sky the same flat budget as texture, which is why that ruler had to be
/// charged per crossing and this one must not be.
///
/// WHAT THE BUDGET BUYS IS A p90, NOT A PER-CROSSING CAP. The reading is
/// the 90th percentile of |signed bow| (`ZONE_BOUNDARY_PERCENTILE`), so a
/// tenth of the crossings sit above it, ungoverned. At the ranked crossing:
/// the widest crossing the window admitted on either real pair measures
/// 0.00978 luma (2.49 codes) against this budget's 3.06, so where the scene
/// already fills the window a correction adds 0.57 of a code and no more,
/// and where it is flat the ceiling is the whole 3.06 — 1.22x the steepest
/// crossing the window is willing to call smooth. The measured MAXIMA are
/// above both, as a p90 gate permits: calibration 0.00978 -> 0.01217
/// (1.24x, +0.61 code), viaduct 0.00978 -> 0.01407 (1.44x, +1.09 codes,
/// past 0.012 itself).
///
/// First-party on the real pairs, segmentation and correspondence made
/// unavailable so `match --zoned` takes this fallback. TWO RASTERS are in
/// play and every number below says which. The ENGINE gates on the 384-px
/// thumbnail it develops itself (its crossing counts appear in the
/// rationale); the readings are a transcription of this same statistic
/// over a full-resolution develop downsampled to a 384-px long edge. The
/// reference the engine passes is `&global_px` — the TWIN, global
/// correction with no range masks — so the transcription uses the twin as
/// its basis too.
///
/// * calibration pair, band luma [0.471, 0.765] at -0.56 EV: gate k=1.000
///   over 18651 engine crossings. Transcribed over 18297 crossings, twin
///   basis: uncorrected p90 0.00392 (1.00 code, max 0.00978), delivered
///   p90 0.00230 (0.59, max 0.01217).
/// * stone-viaduct pair, band luma [0.118, 0.882] at -0.80 EV and
///   saturation +23: gate k=1.000 over 1214 engine crossings. Transcribed
///   over 1224, twin basis: uncorrected p90 0.00874 (2.23 codes, max
///   0.00978), delivered p90 0.00857 (2.19, max 0.01407) — the band moved
///   the ranked reading DOWN. Read against the no-masks base instead of
///   the twin the same pair gives 0.00874 over 1272; that is not the frame
///   the gate sees.
/// * the Cornwall pair attaches no band at all: one is dropped by the
///   do-no-harm arm and two abstain, so it carries no reading either way.
///
/// The delivered transitions are RAMPS, measured and not assumed, with the
/// instrument's own noise beside them. Over the populated 1-code bins of
/// the delivered transfer (twin luma to delivered luma, 1200 px) the
/// calibration band shows 0 reversals in 30 bins, minimum slope +0.019 and
/// maximum +0.964 — COMPRESSIVE: 30 input codes into 9.9 delivered ones,
/// with 9 of the 30 bins under slope 0.1 — and the viaduct 0 reversals in
/// 19 bins with +0.855. The same estimator over the zero-weight control
/// region, where the true slope is exactly 1.000, reads +0.942 to +1.039:
/// a +/-0.05 noise floor, so +0.019 is one noise unit from a reversal.
/// Both pairs therefore pass the sign test v1.2.4 added
/// ([`RANGE_TRANSFER_REVERSAL_MAX`]) — driven end to end, the calibration
/// band reads 0.0001 luma against the 0.0020 allowed — and the gate carries
/// that reading beside this one.
/// The mask-free `scripts/rim_overshoot.py` reads mean 0.0006 /
/// p90 0.0018 / max 0.0082 at the calibration transition against its own
/// control of exactly 0.0000; it is NOT applicable at the viaduct's
/// contour and says so in its own numbers — its 60 px plateau windows must
/// bracket the transition, and there the band's spread of 40.4 codes
/// exceeds the plateau gap of 22.1 on 201 of 231 columns.
///
/// THE SEAM-STYLE READING IS BASIS-DEPENDENT HERE, WHICH IS WHY IT DOES
/// NOT DECIDE. Stepping across the calibration band's contours in 8-bit
/// codes at 1200 px, the shape v1.2.2 used on the tile, all four measured
/// rows: lo_outer +9.05 codes at z +10.08 on the twin basis but +2.83 at
/// z +1.94 on the neutral one; lo +7.18 at z +6.99 twin and +8.28 at
/// z +8.92 neutral (p90 |.| 22.84 codes, 26.8x the control's sd of 0.853).
/// A 5x swing at ONE contour between two honest bases is itself the
/// evidence. A range edge IS an iso-luminance contour of the photograph,
/// so a difference across it is the correction doing its job, and the
/// transfer above shows the ramp COMPRESSING (dT max +0.964 inside it):
/// those 9 codes are the scene's own gradient being flattened, not an
/// induced step. A tile boundary is an arbitrary rectangle over continuous
/// sky, where any step is an artefact — which is why the same statistic
/// decided v1.2.2 and cannot decide this.
///
/// WHAT THIS RULER DOES NOT SAY, AND WHAT NOW SAYS IT. This ruler ranks
/// MAGNITUDE, so it cannot tell a preserved gradient from an inverted one
/// of the same size, and neither can the mask-free ruler, because the
/// DIFFERENCE it ranks stays monotone even where the delivered luma does
/// not. v1.2.3 measured that gap on a synthetic ramp and left it open;
/// v1.2.4 closes it with a second reading rather than a re-tune of this
/// number. [`RANGE_TRANSFER_REVERSAL_MAX`] carries the sign test, the
/// re-cut probe behind it carries the table (see [`RANGE_MAX_RAMP`], where
/// every ramp the producer emits is now measured at five band positions,
/// and the mask-free ruler reads 0.0000 in all 90 cells while the tone
/// order inverts by up to 38 codes), and `enforce_range_boundary_gate`
/// applies both readings as one verdict: a band that inverts the tone order
/// is shrunk to the largest amount that does not, and both readings ride
/// the disclosure. This ceiling is unchanged and is not what was wrong.
const RANGE_BOUNDARY_RIM_MAX: f32 = 0.012;
/// Ceiling on the depth of a TONE REVERSAL the delivered transfer may carry
/// across a band's edges, in luma. `RANGE_BOUNDARY_RIM_MAX` above ranks
/// MAGNITUDE, so it cannot tell a preserved gradient from an inverted one of
/// the same size; this is the ruler that can, and the two together are what
/// "the delivered transition is a ramp" means.
///
/// MEASURED, on the re-cut synthetic probe
/// (`the_recut_ramp_probe_measures_every_ramp_the_producer_emits`) and on the
/// two real pairs that attach a band:
///
/// * The estimator's own floor is EXACTLY 0. On the probe's zero-strength
///   control — the same frame, the same band, 0.00 EV — the delivered transfer
///   is the identity and this reading is 0.000000 on all 15 (ramp, position)
///   cells. It is a running-maximum depth, not a slope, so per-bin noise has to
///   accumulate before it registers, and on an unmoved render there is none.
/// * The real pairs sit a long way under the line. Driven end to end on
///   2026-09-02 — `match --zoned` on the calibration pair with segmentation
///   pointed at nothing, so the band path is the one that attaches — the
///   accepted band [0.471, 0.765] at -0.56 EV reads 0.0001 luma of fall-back,
///   0.026 of an 8-bit code, one twentieth of this ceiling, beside a rim of
///   0.002 over 18651 crossings; the band attaches unchanged at k = 1.000.
///   v1.2.3's slope table said the same thing with a different estimator: 0
///   reversals over the calibration band's 30 populated 1-code bins and the
///   viaduct's 19.
/// * The probe's smallest REAL inversion is 1 code = 0.0039 luma (a 1/17 band
///   at -0.35 EV, v1.2.3's table), and this batch's own sweep reproduces it.
///
/// Half a code is therefore the line: it is above a floor of exactly zero and
/// below the smallest inversion any configuration has produced, and half an
/// 8-bit code is under the quantisation of the render this gate measures, so
/// nothing it admits can be drawn.
const RANGE_TRANSFER_REVERSAL_MAX: f32 = 0.5 / 255.0;
/// How close two neighbouring REFERENCE pixels have to be, in the coordinate
/// a band's mask ramps along, before the crossing between them counts as a
/// smooth one a correction may be charged for. Two-and-a-half 8-bit levels
/// preserves the retained smooth-gradient stress while excluding real subject
/// edges. The number is unchanged: it was spelled inline inside
/// [`range_transition_rim`] while luma was the only coordinate, and it is a
/// constant now because a colour band asks the same question in its own.
const RANGE_SMOOTH_CROSSING: f32 = 2.5 / 255.0;
/// Native bands reuse the global evidence model's measured 1.5% population
/// floor; a smaller interval is not a two-sided measurement.
pub(super) const RANGE_MIN_EVIDENCE_SHARE: f32 = 0.015;
/// Range bands must pay for themselves on the composed evidence-weighted
/// frame: the live regression measured `0.018 -> 0.024` after two bands.
/// Equality is acceptable, but any worse frame restores the running recipe.
const RANGE_FRAME_REGRESSION_TOL: f32 = 0.0;

const RANGE_HOST: MaskGeometry = MaskGeometry::Linear {
    zero_x: 0.5,
    zero_y: -0.8,
    full_x: 0.5,
    full_y: -0.4,
};

/// The coordinate ONE range mask's weight ramps along.
///
/// A boundary reading only means something in that coordinate. A luminance
/// band ramps over an interval of luma, so a step it induces is a luma step.
/// A colour band ramps over a shell of chromaticity distance around its own
/// reference colour, so a step it induces is a chromaticity step, and the luma
/// ruler reads 0.000 across an edge that is plainly visible: a red pixel beside
/// a green one of the same luma differs by nothing the luma ruler measures.
///
/// So the family keeps ONE rule and reads it in each band's own units. Two
/// neighbours are admitted when the REFERENCE separates them by at most
/// [`RANGE_SMOOTH_CROSSING`] here; they belong to a transition when their
/// reference midpoint falls inside that transition's ramp; and the reading is
/// how far apart the RENDER puts the same two pixels. Instantiated on
/// [`Self::Luma`] this is, arithmetically, the reading the rim gate has taken
/// since v1.2.2 -- the p90 ranks magnitudes, so the luma arm's sign never
/// reached the verdict.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RangeSelector {
    /// Rec.601 display luma: the coordinate `RangeMask::Luminance` ramps in.
    Luma,
    /// Brightness-independent distance from a colour band's reference colour:
    /// the exact quantity `render::range_weight` ramps a `RangeMask::Color`
    /// over ([`render::chromaticity_distance`]).
    Chromaticity([f32; 3]),
}

impl RangeSelector {
    /// Where one pixel sits along this coordinate. Non-finite when the pixel
    /// carries no reading here, which for chromaticity is a pixel too dark to
    /// have one; callers skip those samples rather than inventing a position.
    fn position(self, px: &[f32; 3]) -> f32 {
        match self {
            Self::Luma => display_luma(px),
            Self::Chromaticity(reference) => render::chromaticity_distance(&reference, px),
        }
    }

    /// How far apart two pixels are along this coordinate. The luma arm keeps
    /// the signed difference the rim gate has always taken; the chromaticity
    /// arm is the same distance function applied BETWEEN the two pixels
    /// instead of between one pixel and the band's colour, and is never
    /// negative. Both are ranked by magnitude below.
    fn separation(self, a: &[f32; 3], b: &[f32; 3]) -> f32 {
        match self {
            Self::Luma => display_luma(b) - display_luma(a),
            Self::Chromaticity(_) => render::chromaticity_distance(a, b),
        }
    }
}

/// One ramp of one mask: the coordinate it ramps in, and the interval over
/// which its weight changes.
#[derive(Clone, Copy, Debug)]
struct RangeTransition {
    selector: RangeSelector,
    lo: f32,
    hi: f32,
}

/// Every ramp a range mask carries. A luminance trapezoid has two, and a hard
/// edge contributes none exactly as before; a colour ball has one, from the
/// distance where its weight starts falling to the one where it reaches zero.
fn range_transitions(range: &RangeMask) -> Vec<RangeTransition> {
    match *range {
        RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => [(lo_outer, lo), (hi, hi_outer)]
            .into_iter()
            .filter(|(a, b)| a.is_finite() && b.is_finite() && b > a)
            .map(|(lo, hi)| RangeTransition { selector: RangeSelector::Luma, lo, hi })
            .collect(),
        RangeMask::Color { r, g, b, amount, .. } => {
            let tolerance = render::colour_range_tolerance(amount);
            [(0.5 * tolerance, tolerance)]
                .into_iter()
                .filter(|(a, b)| a.is_finite() && b.is_finite() && b > a)
                .map(|(lo, hi)| RangeTransition {
                    selector: RangeSelector::Chromaticity([r, g, b]),
                    lo,
                    hi,
                })
                .collect()
        }
    }
}

/// The footprint the ORDER reading walks, in the coordinate that has an
/// order. `None` for a colour band, and that is a measured decision rather
/// than an omission.
///
/// [`range_transfer_reversal`] asks whether the delivered values stayed in
/// the order the reference put them in. Luma is a signed axis and the move a
/// luminance band makes — lift or lower a span — preserves that order when it
/// is doing its job, so a reversal there is always a defect. Chromaticity
/// distance from a band's own colour is RADIAL, and the move a colour band
/// exists to make — carry the band's population toward the target's colour —
/// changes every member's radius: the member that sat exactly on the band's
/// colour ends up a shift away from it while the member that sat a shift away
/// on the far side ends up on it. The radial order inverts by construction,
/// so the test would read the correction's success as its defect. Measured on
/// the synthetic scattered-hue pair: a modest desaturation of the blue band
/// was refused on this reading with a rim of 0.000, shrunk to k=0.104, and
/// then dropped by the composed-frame ceiling for want of any gain left.
///
/// A colour band's boundary is therefore judged by the rim, in ITS coordinate
/// — which is the reading that answers the question a colour ramp raises: did
/// two neighbours the photograph gave the same colour come out different.
fn range_footprint(range: &RangeMask) -> Option<(RangeSelector, (f32, f32))> {
    match *range {
        RangeMask::Luminance { lo_outer, hi_outer, .. } => {
            Some((RangeSelector::Luma, (lo_outer, hi_outer)))
        }
        RangeMask::Color { .. } => None,
    }
}

#[derive(Clone, Debug)]
struct ResidualRun {
    first: usize,
    last: usize,
    target_first: usize,
    target_last: usize,
    residual: f32,
    score: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct RangeBand {
    attachment: ZoneAttachment,
    source: RangeMask,
    target: RangeMask,
    divergence: fit::Divergence,
    /// The ACR hue band a COLOUR band was measured on; `None` for a luminance
    /// band. It names the population in the disclosure and decides which
    /// abstention sentence a refusal is written into.
    hue_band: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct RangeAbstention {
    lo: f32,
    hi: f32,
    reason: String,
}

#[derive(Clone, Debug, PartialEq)]
struct RangeMerge {
    lo: f32,
    hi: f32,
    into_lo: f32,
    into_hi: f32,
    sign: &'static str,
    why: &'static str,
}

#[derive(Debug, Default, PartialEq)]
struct RangeDerivation {
    bands: Vec<RangeBand>,
    abstentions: Vec<RangeAbstention>,
    merges: Vec<RangeMerge>,
}

#[cfg(test)]
thread_local! {
    static RANGE_DERIVATION_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RANGE_FRESH_RENDER_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RANGE_FINAL_FRAME_ERR_OVERRIDE: std::cell::Cell<Option<f32>> =
        const { std::cell::Cell::new(None) };
    /// The A/B arm the corpus test measures the colour family against: with
    /// it set the fallback is exactly the luminance-only producer that ran
    /// before this family existed, so two runs in one process differ by this
    /// feature and nothing else.
    static COLOUR_RANGE_SUPPRESSED: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

fn display_luma(p: &[f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

fn range_weights_for_pixels(range: &RangeMask, pixels: &[[f32; 3]]) -> Vec<f32> {
    pixels.iter().map(|p| render::range_weight(range, p)).collect()
}

/// Overlapping ramps of ONE partition may not count a pixel twice in one
/// estimator. The two families never meet here: each is a stage of its own
/// ([`attach_band_stage`]) and every band re-derives its population from a
/// fresh render of the stack so far, so a pixel a luminance band already
/// moved is measured as the luminance band left it.
fn normalize_partition_weights(weights: &mut [Vec<f32>]) {
    let n = weights.iter().map(Vec::len).min().unwrap_or(0);
    for i in 0..n {
        let total = weights.iter().map(|band| band[i]).sum::<f32>();
        if total > 1.0 {
            for band in &mut *weights {
                band[i] /= total;
            }
        }
    }
}

fn evidence_refusal(e: &fit::EvidenceRange) -> Option<String> {
    if !e.source_populated || e.source_share < RANGE_MIN_EVIDENCE_SHARE {
        Some(format!(
            "source population {:.1}% is below the {:.1}% evidence floor",
            e.source_share * 100.0,
            RANGE_MIN_EVIDENCE_SHARE * 100.0,
        ))
    } else if !e.target_populated || e.target_share < RANGE_MIN_EVIDENCE_SHARE {
        Some(format!(
            "target population {:.1}% is below the {:.1}% evidence floor",
            e.target_share * 100.0,
            RANGE_MIN_EVIDENCE_SHARE * 100.0,
        ))
    } else if e.two_sided_share <= 0.0 {
        Some("the paired population has zero two-sided evidence".to_string())
    } else if e.weight <= 0.0 {
        Some("the paired population has zero structural evidence".to_string())
    } else {
        None
    }
}

fn target_range_for_ranks(
    target_luma: &[f32],
    first: usize,
    last: usize,
) -> RangeMask {
    let n = target_luma.len();
    let first = first.min(n.saturating_sub(1));
    let last = last.max(first + 1).min(n);
    let lo = target_luma.get(first).copied().unwrap_or(0.0);
    let hi = target_luma.get(last - 1).copied().unwrap_or(lo);
    RangeMask::Luminance {
        lo_outer: (lo - RANGE_MAX_RAMP).max(0.0),
        lo,
        hi,
        hi_outer: (hi + RANGE_MAX_RAMP).min(1.0),
    }
}

fn source_ranges_for_runs(runs: &[ResidualRun]) -> Vec<RangeMask> {
    runs.iter()
        .enumerate()
        .map(|(i, run)| {
            let lo = run.first as f32 / 17.0;
            let hi = (run.last + 1) as f32 / 17.0;
            let left_gap = i
                .checked_sub(1)
                .map(|j| lo - (runs[j].last + 1) as f32 / 17.0)
                .unwrap_or(RANGE_MAX_RAMP);
            let right_gap = runs
                .get(i + 1)
                .map(|next| next.first as f32 / 17.0 - hi)
                .unwrap_or(RANGE_MAX_RAMP);
            let ramp = |gap: f32| gap.clamp(RANGE_MIN_RAMP, RANGE_MAX_RAMP);
            RangeMask::Luminance {
                lo_outer: (lo - ramp(left_gap)).max(0.0),
                lo,
                hi,
                hi_outer: (hi + ramp(right_gap)).min(1.0),
            }
        })
        .collect()
}

fn residual_run(
    first: usize,
    last: usize,
    bin_residual: &[f32; 17],
    target_bounds: &[(usize, usize); 17],
    evidence: &fit::EvidenceModel,
) -> ResidualRun {
    let verdict = fit::luma_evidence_for_bins(evidence, first, last);
    let residual_weight = verdict.two_sided_share.max(1e-6);
    let residual = (first..=last)
        .map(|bin| bin_residual[bin] * evidence.luma[bin].two_sided_share.max(1e-6))
        .sum::<f32>()
        / (first..=last)
            .map(|bin| evidence.luma[bin].two_sided_share.max(1e-6))
            .sum::<f32>();
    ResidualRun {
        first,
        last,
        target_first: target_bounds[first].0,
        target_last: target_bounds[last].1,
        residual,
        score: residual.abs() * residual_weight,
    }
}

fn bands_from_runs(
    source_px: &[[f32; 3]],
    target_px: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
    target_luma: &[f32],
    runs: Vec<ResidualRun>,
) -> Vec<RangeBand> {
    let source_ranges = source_ranges_for_runs(&runs);
    let target_ranges = runs
        .iter()
        .map(|run| target_range_for_ranks(target_luma, run.target_first, run.target_last))
        .collect::<Vec<_>>();
    let mut source_coverage = source_ranges
        .iter()
        .map(|range| range_weights_for_pixels(range, source_px))
        .collect::<Vec<_>>();
    let mut source_weights = source_coverage.clone();
    normalize_partition_weights(&mut source_weights);
    let mut bands = Vec::with_capacity(runs.len());
    for (i, run) in runs.into_iter().enumerate() {
        let verdict = fit::luma_evidence_for_bins(evidence, run.first, run.last);
        let d = if verdict.divergence.is_finite() { verdict.divergence } else { 1.0 };
        let source = source_ranges[i];
        let target = target_ranges[i];
        let name = format!("Luminance range {:02}", i + 1);
        bands.push(RangeBand {
            attachment: ZoneAttachment {
                source_weights: std::mem::take(&mut source_weights[i]),
                target_weights: Vec::new(),
                coverage: Some(ZoneCoverage {
                    source: std::mem::take(&mut source_coverage[i]),
                    target: range_weights_for_pixels(&target, target_px),
                }),
                mask: RANGE_HOST,
                range: Some(source),
                name: name.clone(),
                role: MaskRole::Custom,
                inverted: false,
                label: name,
                min_share: MIN_ZONE_SHARE,
                frame_regression_tol: RANGE_FRAME_REGRESSION_TOL,
            },
            source,
            target,
            divergence: fit::Divergence {
                correlation: (1.0 - d).clamp(-1.0, 1.0),
                energy_error: 0.0,
                d,
            },
            hue_band: None,
        });
    }
    bands
}

/// Maps a field proposal (a span of CURRENT-render luma) into the evidence-bin
/// domain (ORIGINAL source luma) through the pixels that occupy the span: the
/// weighted 10th..90th percentile of their original luma. The band then names
/// the population the field actually measured, not the same numbers read in a
/// domain a global tone move already shifted.
fn evidence_bins_for_span(
    proposal: &FieldBandProposal,
    source_px: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
) -> Option<(usize, usize)> {
    let inside = |luma: f32| {
        luma >= proposal.lo && (luma < proposal.hi || (proposal.hi >= 1.0 && luma <= 1.0))
    };
    let mut members = source_px
        .iter()
        .zip(&evidence.source_pixels)
        .zip(&evidence.source_weights)
        .filter(|((current, _), _)| inside(display_luma(current)))
        .map(|((_, base), weight)| (display_luma(base), weight.max(0.0)))
        .filter(|(_, weight)| *weight > 0.0)
        .collect::<Vec<_>>();
    members.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mass = members.iter().map(|(_, weight)| *weight as f64).sum::<f64>();
    if mass <= 0.0 {
        return None;
    }
    let quantile = |q: f64| {
        let mut acc = 0.0f64;
        members
            .iter()
            .find(|(_, weight)| {
                acc += *weight as f64;
                acc >= q * mass
            })
            .map(|(luma, _)| *luma)
    };
    let (lo, hi) = (quantile(0.10)?, quantile(0.90)?);
    Some((fit::evidence_luma_bin(lo), fit::evidence_luma_bin(hi)))
}

/// Derive coherent residual runs from the current rendered state. Target
/// populations are monotone rank partners, matching the fixed evidence model;
/// no spatial alignment is assumed.
fn derive_luminance_bands(
    source_px: &[[f32; 3]],
    target_px: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
    proposals: &[FieldBandProposal],
) -> RangeDerivation {
    #[cfg(test)]
    RANGE_DERIVATION_CALLS.with(|calls| calls.set(calls.get() + 1));

    if source_px.is_empty() || target_px.is_empty() || evidence.luma.len() != 17 {
        return RangeDerivation::default();
    }
    let mut target_luma = target_px.iter().map(display_luma).collect::<Vec<_>>();
    target_luma.sort_by(f32::total_cmp);

    let mut bin_residual = [0.0f32; 17];
    let mut bin_count = [0usize; 17];
    for p in &evidence.source_pixels {
        bin_count[fit::evidence_luma_bin(display_luma(p))] += 1;
    }
    let mut target_cursor = 0usize;
    let mut target_bounds = [(0usize, 0usize); 17];
    for bin in 0..17 {
        let current_values = evidence
            .source_pixels
            .iter()
            .zip(source_px)
            .filter(|(base, _)| fit::evidence_luma_bin(display_luma(base)) == bin)
            .map(|(_, current)| display_luma(current))
            .collect::<Vec<_>>();
        let last = (target_cursor + bin_count[bin]).min(target_luma.len());
        target_bounds[bin] = (target_cursor.min(target_luma.len()), last);
        let target_values = &target_luma[target_cursor.min(target_luma.len())..last];
        if !current_values.is_empty() && !target_values.is_empty() {
            let current_mean = current_values.iter().sum::<f32>() / current_values.len() as f32;
            let target_mean = target_values.iter().sum::<f32>() / target_values.len() as f32;
            bin_residual[bin] = target_mean - current_mean;
        }
        target_cursor = last;
    }

    let mut out = RangeDerivation::default();
    let mut candidate = [false; 17];
    for bin in 0..17 {
        if bin_count[bin] == 0 || bin_residual[bin].abs() < RANGE_RESIDUAL_TRIGGER {
            continue;
        }
        if let Some(reason) = evidence_refusal(&evidence.luma[bin]) {
            out.abstentions.push(RangeAbstention {
                lo: bin as f32 / 17.0,
                hi: (bin + 1) as f32 / 17.0,
                reason,
            });
        } else {
            candidate[bin] = true;
        }
    }

    let mut runs = Vec::<ResidualRun>::new();
    let mut bin = 0usize;
    while bin < 17 {
        if !candidate[bin] {
            bin += 1;
            continue;
        }
        let first = bin;
        let sign = bin_residual[bin].is_sign_positive();
        let mut last = bin;
        while last + 1 < 17
            && candidate[last + 1]
            && bin_residual[last + 1].is_sign_positive() == sign
        {
            let weighted = (first..=last + 1)
                .map(|b| {
                    let w = evidence.luma[b].two_sided_share.max(1e-6);
                    (bin_residual[b] * w, w)
                })
                .fold((0.0f32, 0.0f32), |(rs, ws), (r, w)| (rs + r, ws + w));
            if weighted.1 <= 0.0 || weighted.0.is_sign_positive() != sign {
                break;
            }
            last += 1;
        }
        let qualifies = last > first
            || (first..=last).any(|b| bin_residual[b].abs() >= 2.0 * RANGE_RESIDUAL_TRIGGER);
        if qualifies {
            runs.push(residual_run(
                first, last, &bin_residual, &target_bounds, evidence,
            ));
        }
        bin = last + 1;
    }

    let mut valid = Vec::new();
    for run in runs {
        let verdict = fit::luma_evidence_for_bins(evidence, run.first, run.last);
        if let Some(reason) = evidence_refusal(&verdict) {
            out.abstentions.push(RangeAbstention {
                lo: run.first as f32 / 17.0,
                hi: (run.last + 1) as f32 / 17.0,
                reason,
            });
        } else {
            valid.push(run);
        }
    }

    for proposal in proposals {
        let Some((first, last)) = evidence_bins_for_span(proposal, source_px, evidence) else {
            out.abstentions.push(RangeAbstention {
                lo: proposal.lo,
                hi: proposal.hi,
                reason: "field proposal covers no evidence population".to_string(),
            });
            continue;
        };
        let verdict = fit::luma_evidence_for_bins(evidence, first, last);
        if let Some(reason) = evidence_refusal(&verdict) {
            out.abstentions.push(RangeAbstention {
                lo: first as f32 / 17.0,
                hi: (last + 1) as f32 / 17.0,
                reason: format!("field proposal: {reason}"),
            });
            continue;
        }
        let proposed = residual_run(first, last, &bin_residual, &target_bounds, evidence);
        let overlaps = |run: &ResidualRun| run.first <= last && first <= run.last;
        if valid.iter().any(|run| overlaps(run)
            && run.residual.is_sign_positive() != proposal.sign.is_sign_positive())
        {
            out.abstentions.push(RangeAbstention {
                lo: first as f32 / 17.0,
                hi: (last + 1) as f32 / 17.0,
                reason: "field proposal conflicts with the rank-paired band".to_string(),
            });
            continue;
        }
        // The proposal's own rank-paired residual must agree with the field's
        // sign: a band the two estimators read in opposite directions is a
        // disagreement to disclose, not a correction to widen.
        if proposed.residual.is_sign_positive() != proposal.sign.is_sign_positive() {
            out.abstentions.push(RangeAbstention {
                lo: first as f32 / 17.0,
                hi: (last + 1) as f32 / 17.0,
                reason: "field proposal sign disagrees with its rank-paired residual".to_string(),
            });
            continue;
        }
        let touches = |run: &ResidualRun| {
            run.first <= last.saturating_add(1) && first <= run.last.saturating_add(1)
                && run.residual.is_sign_positive() == proposal.sign.is_sign_positive()
        };
        let merged = valid.iter().enumerate()
            .filter_map(|(i, run)| touches(run).then_some(i)).collect::<Vec<_>>();
        if let Some(&into) = merged.first() {
            let old = proposed.clone();
            valid[into].first = valid[into].first.min(proposed.first);
            valid[into].last = valid[into].last.max(proposed.last);
            valid[into].target_first = valid[into].target_first.min(proposed.target_first);
            valid[into].target_last = valid[into].target_last.max(proposed.target_last);
            for &index in merged.iter().skip(1).rev() {
                let joined = valid.remove(index);
                valid[into].first = valid[into].first.min(joined.first);
                valid[into].last = valid[into].last.max(joined.last);
                valid[into].target_first = valid[into].target_first.min(joined.target_first);
                valid[into].target_last = valid[into].target_last.max(joined.target_last);
            }
            out.merges.push(RangeMerge {
                lo: old.first as f32 / 17.0,
                hi: (old.last + 1) as f32 / 17.0,
                into_lo: valid[into].first as f32 / 17.0,
                into_hi: (valid[into].last + 1) as f32 / 17.0,
                sign: if proposal.sign.is_sign_positive() { "positive" } else { "negative" },
                why: "absorbed by the overlapping rank-paired run before the cap",
            });
        } else {
            valid.push(proposed);
        }
    }
    valid.sort_by_key(|run| run.first);
    debug_assert!(valid.windows(2).all(|pair| pair[0].last < pair[1].first));

    if valid.len() > RANGE_MAX_BANDS {
        let mut ranked = (0..valid.len()).collect::<Vec<_>>();
        ranked.sort_by(|&a, &b| {
            valid[b]
                .score
                .total_cmp(&valid[a].score)
                .then_with(|| valid[a].first.cmp(&valid[b].first))
        });
        let mut keep = ranked[..RANGE_MAX_BANDS].to_vec();
        for positive in [false, true] {
            if valid.iter().any(|r| r.residual.is_sign_positive() == positive)
                && !keep
                    .iter()
                    .any(|&i| valid[i].residual.is_sign_positive() == positive)
            {
                let replacement = ranked
                    .iter()
                    .copied()
                    .find(|&i| valid[i].residual.is_sign_positive() == positive)
                    .expect("sign was observed");
                keep.pop();
                keep.push(replacement);
            }
        }
        keep.sort_unstable();
        keep.dedup();
        let dropped = (0..valid.len()).filter(|i| !keep.contains(i)).collect::<Vec<_>>();
        for drop in dropped {
            let nearest = keep
                .iter()
                .copied()
                .filter(|&i| {
                    valid[i].residual.is_sign_positive()
                        == valid[drop].residual.is_sign_positive()
                })
                .filter(|&into| {
                    let lo = valid[into].first.min(valid[drop].first);
                    let hi = valid[into].last.max(valid[drop].last);
                    keep.iter().copied().filter(|&other| other != into).all(|other| {
                        valid[other].last < lo || valid[other].first > hi
                    })
                })
                .min_by_key(|&i| valid[i].first.abs_diff(valid[drop].first));
            let old = valid[drop].clone();
            if let Some(nearest) = nearest {
                valid[nearest].first = valid[nearest].first.min(old.first);
                valid[nearest].last = valid[nearest].last.max(old.last);
                valid[nearest].target_first = valid[nearest].target_first.min(old.target_first);
                valid[nearest].target_last = valid[nearest].target_last.max(old.target_last);
                out.merges.push(RangeMerge {
                    lo: old.first as f32 / 17.0,
                    hi: (old.last + 1) as f32 / 17.0,
                    into_lo: valid[nearest].first as f32 / 17.0,
                    into_hi: (valid[nearest].last + 1) as f32 / 17.0,
                    sign: if old.residual.is_sign_positive() { "positive" } else { "negative" },
                    why: "after the four-band evidence cap",
                });
            } else {
                out.abstentions.push(RangeAbstention {
                    lo: old.first as f32 / 17.0,
                    hi: (old.last + 1) as f32 / 17.0,
                    reason: "the four-band cap found no adjacent same-sign merge that preserved every retained core"
                        .to_string(),
                });
            }
        }
        valid = keep.into_iter().map(|i| valid[i].clone()).collect();
    }
    valid.sort_by_key(|run| run.first);
    debug_assert!(valid.windows(2).all(|pair| pair[0].last < pair[1].first));
    out.bands = bands_from_runs(source_px, target_px, evidence, &target_luma, valid);
    out
}

fn range_transition_rim(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    ranges: &[RangeMask],
    width: u32,
    height: u32,
) -> BoundaryReading {
    let (w, h) = (width as usize, height as usize);
    let transitions = ranges.iter().flat_map(range_transitions).collect::<Vec<_>>();
    let mut rims = vec![Vec::new(); transitions.len()];
    let mut sample_pair = |a: usize, b: usize| {
        let Some((ra, rb, pa, pb)) = reference
            .get(a)
            .zip(reference.get(b))
            .zip(rendered.get(a).zip(rendered.get(b)))
            .map(|((ra, rb), (pa, pb))| (ra, rb, pa, pb))
        else {
            return;
        };
        for (index, transition) in transitions.iter().enumerate() {
            let selector = transition.selector;
            // A range rim is a bow in a locally smooth crossing, not a
            // pre-existing subject edge -- and SMOOTH is a claim about the
            // coordinate this band selects on, which is why the test moved
            // inside the loop. A colour band's ramp lives exactly where the
            // luma ruler would have called every crossing smooth.
            let separation = selector.separation(ra, rb);
            if !separation.is_finite() || separation.abs() > RANGE_SMOOTH_CROSSING {
                continue;
            }
            let (sa, sb) = (selector.position(ra), selector.position(rb));
            if !sa.is_finite() || !sb.is_finite() {
                continue;
            }
            let middle = (sa + sb) * 0.5;
            if !(transition.lo..=transition.hi).contains(&middle) {
                continue;
            }
            let bow = selector.separation(pa, pb);
            if bow.is_finite() {
                rims[index].push(bow);
            }
        }
    };
    for y in 0..h {
        for x in 1..w {
            sample_pair(y * w + x - 1, y * w + x);
        }
    }
    for y in 1..h {
        for x in 0..w {
            sample_pair((y - 1) * w + x, y * w + x);
        }
    }
    let transition_count = rims.iter().map(Vec::len).sum();
    if transition_count == 0 {
        return BoundaryReading { rim: 0.0, transitions: 0, charged: 0.0 };
    }
    let rim = rims
        .into_iter()
        .filter(|values| !values.is_empty())
        .map(|mut values| {
            // Budget the signed samples by magnitude so bright and dark bows
            // cannot cancel or hide on opposite sides of a range transition.
            values.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
            let rank = ((values.len() as f32 * ZONE_BOUNDARY_PERCENTILE).ceil() as usize)
                .saturating_sub(1)
                .min(values.len() - 1);
            values[rank].abs()
        })
        .fold(0.0f32, f32::max);
    // This family declines to charge: its admission rule above (a bow in a
    // locally smooth crossing, never a pre-existing subject edge) is already
    // a binary form of the contextual test, and a graded context here is
    // capped at [`RANGE_SMOOTH_CROSSING`] by construction — no dynamic
    // range. See [`super::BoundaryReading::charged`].
    BoundaryReading { rim, transitions: transition_count, charged: rim }
}

/// The delivered transfer's worst ORDER REVERSAL across the bands' edges, in
/// each band's own coordinate.
///
/// The reading is taken in the same two frames the rim gate uses: the
/// REFERENCE (the globals-only twin the engine passes in) supplies the input
/// order, and the RENDERED frame supplies the delivered one. Pixels are binned
/// by their reference POSITION along the coordinate, into the render's own
/// 8-bit codes, and each populated bin holds the mean delivered position of its
/// members; walking the bins upward, the reading is the deepest fall below the
/// highest delivered position already reached. A monotone transfer of any
/// steepness — including one that compresses 30 input codes into 9.9, which is
/// what the calibration band measures — reads exactly 0. An inversion reads
/// its own depth.
///
/// ONLY BANDS WHOSE COORDINATE HAS AN ORDER are walked, which today means the
/// luminance family: [`range_footprint`] hands back nothing for a colour band,
/// and the measurement behind that is written there. Luma is a signed axis and
/// this is the tone order the gate has ranked since v1.2.4.
///
/// Two coordinates would never share a histogram — 0.30 of luma and 0.30 of
/// chromaticity are different facts about different pixels — so each footprint
/// is grouped by its own selector, given its own bins, and the reading is the
/// worst of them. A colour band contributes no group and is judged by the rim.
///
/// Only the bands' own footprints are read: outside them the correction has no
/// weight and its transfer is the identity, so including them would dilute the
/// bins that carry the question.
fn range_transfer_reversal(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    ranges: &[RangeMask],
) -> f32 {
    const BINS: usize = 256;
    let mut groups: Vec<(RangeSelector, Vec<(f32, f32)>)> = Vec::new();
    for range in ranges {
        let Some((selector, span)) = range_footprint(range) else { continue };
        match groups.iter_mut().find(|(known, _)| *known == selector) {
            Some((_, spans)) => spans.push(span),
            None => groups.push((selector, vec![span])),
        }
    }
    let mut worst = 0.0f32;
    for (selector, spans) in groups {
        let mut sum = [0.0f64; BINS];
        let mut count = [0u32; BINS];
        for (reference, delivered) in reference.iter().zip(rendered) {
            let l = selector.position(reference);
            if !l.is_finite() || !spans.iter().any(|(lo, hi)| l >= *lo && l <= *hi) {
                continue;
            }
            let after = selector.position(delivered);
            if !after.is_finite() {
                continue;
            }
            let bin = (l.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as usize;
            sum[bin] += after as f64;
            count[bin] += 1;
        }
        let mut highest = f32::NEG_INFINITY;
        let mut depth = 0.0f32;
        for bin in 0..BINS {
            if count[bin] == 0 {
                continue;
            }
            let delivered = (sum[bin] / count[bin] as f64) as f32;
            if highest > f32::NEG_INFINITY {
                depth = depth.max(highest - delivered);
            }
            highest = highest.max(delivered);
        }
        worst = worst.max(depth);
    }
    worst
}

fn range_boundary_note_args(
    n: usize,
    k: f32,
    before: BoundaryReading,
    after: BoundaryReading,
    reversal: f32,
) -> Vec<(&'static str, String)> {
    vec![
        ("n", n.to_string()),
        ("k", format!("{k:.3}")),
        ("before", format!("{:.3}", before.rim)),
        ("after", format!("{:.3}", after.rim)),
        ("max", format!("{RANGE_BOUNDARY_RIM_MAX:.3}")),
        ("transitions", after.transitions.to_string()),
        ("reversal", format!("{reversal:.4}")),
        ("rev_max", format!("{RANGE_TRANSFER_REVERSAL_MAX:.4}")),
    ]
}

fn enforce_range_boundary_gate(
    s_img: &DynamicImage,
    report: &mut FitReport,
    reference: &[[f32; 3]],
    ranges: &[RangeMask],
    correction_shares: &[f32],
    first_range: usize,
    initial_px: Vec<[f32; 3]>,
) -> BoundaryGateResult {
    let initial = range_transition_rim(
        reference,
        &initial_px,
        ranges,
        s_img.width(),
        s_img.height(),
    );
    let initial_reversal = range_transfer_reversal(reference, &initial_px, ranges);
    // TWO readings, one verdict. The rim ranks the SIZE of the bow a transition
    // carries and the reversal ranks its SIGN, and a band can fail either
    // alone: v1.2.3 measured a 1.5/17 band at -0.56 EV inverting the tone order
    // by 2 codes at the far corner while the rim ruler read 0.0000 over its
    // whole n, because the DIFFERENCE that ruler ranks stays monotone even
    // where the delivered luma does not.
    let passes = |reading: &BoundaryReading, reversal: f32| {
        reading.rim <= RANGE_BOUNDARY_RIM_MAX && reversal <= RANGE_TRANSFER_REVERSAL_MAX
    };
    let range_count = report.recipe.masks.len().saturating_sub(first_range);
    // A correction that survives this gate must MOVE something, and the test
    // is BYTE IDENTITY of the render against the reference rather than a
    // threshold on `k` — the zoned gate's own invariant since step 9 (see
    // [`crate::rationale::keys::ZONE_BOUNDARY_INERT`]), which this family
    // never had. The colour producer is what made it fire: on the p39
    // calibration pair the bisection returned k=0.000 and the stage kept a
    // mask whose every dial was zero and whose colour gains had been dropped —
    // a correction the sidecar would carry, the GUI would list, and nothing
    // would render. An inert attachment is strictly worse than a refusal.
    let refuse_inert = |report: &mut FitReport,
                        reading: BoundaryReading,
                        reversal: f32,
                        k: f32| {
        report.recipe.masks.truncate(first_range);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_BOUNDARY_INERT,
                range_boundary_note_args(range_count, k, initial, reading, reversal),
            ),
        );
    };
    if initial_px.as_slice() == reference {
        refuse_inert(report, initial, initial_reversal, 1.0);
        return BoundaryGateResult::Dropped;
    }
    if passes(&initial, initial_reversal) {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_BOUNDARY_PASSED,
                range_boundary_note_args(range_count, 1.0, initial, initial, initial_reversal),
            ),
        );
        return BoundaryGateResult::Kept {
            k: 1.0,
            before: initial,
            after: initial,
            pixels: initial_px,
        };
    }

    let originals = report.recipe.masks[first_range..].to_vec();
    debug_assert_eq!(originals.len(), correction_shares.len());
    let render_at = |report: &mut FitReport, k: f32| -> (BoundaryReading, f32, Vec<[f32; 3]>) {
        shrink_zone_corrections(
            &mut report.recipe.masks[first_range..],
            &originals,
            correction_shares,
            k,
        );
        let pixels = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let reading = range_transition_rim(
            reference,
            &pixels,
            ranges,
            s_img.width(),
            s_img.height(),
        );
        let reversal = range_transfer_reversal(reference, &pixels, ranges);
        (reading, reversal, pixels)
    };
    // `k = 0` is no local correction at all, so its delivered transfer IS the
    // reference's own and the reversal reading is 0 by construction; only the
    // rim can refuse here, and it does so for the reason it always did.
    let (zero, zero_reversal, zero_px) = render_at(report, 0.0);
    if !passes(&zero, zero_reversal) {
        report.recipe.masks.truncate(first_range);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_BOUNDARY_REFUSED,
                range_boundary_note_args(range_count, 0.0, initial, zero, zero_reversal),
            ),
        );
        return BoundaryGateResult::Dropped;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut best = (zero, zero_reversal, zero_px);
    for _ in 0..12 {
        let mid = (lo + hi) * 0.5;
        let measured = render_at(report, mid);
        if passes(&measured.0, measured.1) {
            lo = mid;
            best = measured;
        } else {
            hi = mid;
        }
    }
    if best.2.as_slice() == reference {
        refuse_inert(report, best.0, best.1, lo);
        return BoundaryGateResult::Dropped;
    }
    shrink_zone_corrections(
        &mut report.recipe.masks[first_range..],
        &originals,
        correction_shares,
        lo,
    );
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::RANGE_BOUNDARY_PASSED,
            range_boundary_note_args(range_count, lo, initial, best.0, best.1),
        ),
    );
    BoundaryGateResult::Kept { k: lo, before: initial, after: best.0, pixels: best.2 }
}

fn push_range_abstention(report: &mut FitReport, abstention: &RangeAbstention) {
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::RANGE_ABSTAINED,
            vec![
                ("lo", format!("{:.3}", abstention.lo)),
                ("hi", format!("{:.3}", abstention.hi)),
                ("reason", abstention.reason.clone()),
            ],
        ),
    );
}

fn range_weights_from_current_render(
    s_img: &DynamicImage,
    recipe: &crate::recipe::EditRecipe,
    ranges: &[RangeMask],
) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    #[cfg(test)]
    RANGE_FRESH_RENDER_CALLS.with(|calls| calls.set(calls.get() + 1));

    let current = fit::pixels_of(&render::develop_preview(s_img, recipe));
    let coverage = ranges
        .iter()
        .map(|range| range_weights_for_pixels(range, &current))
        .collect::<Vec<_>>();
    let mut weights = coverage.clone();
    normalize_partition_weights(&mut weights);
    (weights, coverage)
}

fn final_range_frame_err(
    pixels: &[[f32; 3]],
    target: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
) -> f32 {
    let measured = fit::look_err_with_evidence(pixels, target, evidence);
    #[cfg(test)]
    {
        RANGE_FINAL_FRAME_ERR_OVERRIDE.with(|value| value.take().unwrap_or(measured))
    }
    #[cfg(not(test))]
    {
        measured
    }
}

/// A hue band this producer declined, and the reading that declined it.
/// Colour bands are named by the ACR band they were measured on rather than
/// by an interval, because that band is the population the frozen evidence
/// model already carries a verdict for.
#[derive(Clone, Debug, PartialEq)]
struct ColourAbstention {
    band: usize,
    reason: String,
}

/// A colour band that cleared every proposal gate, carrying the numbers its
/// propose disclosure prints.
#[derive(Clone, Debug, PartialEq)]
struct ColourProposal {
    band: RangeBand,
    hue_band: usize,
    /// Weighted share of the analysis frame the emitted mask selects.
    share: f32,
    /// The chromaticity tolerance the emitted `crs:ColorAmount` really buys,
    /// after the schema's 0..=1 clamp — not the radius that was measured.
    tolerance: f32,
    residual: f32,
    score: f32,
}

#[derive(Debug, Default, PartialEq)]
struct ColourDerivation {
    proposals: Vec<ColourProposal>,
    abstentions: Vec<ColourAbstention>,
}

/// Hard membership of one ACR hue band on whichever frame is passed in.
/// [`fit::evidence_hue_band`] is the SAME classifier the evidence model bins
/// with (a 0.06 chroma floor, then the nearest of the eight HSL centres), so
/// the population this producer measures and the population the frozen
/// verdicts are about are one population and not two that resemble each other.
fn hue_band_members(px: &[[f32; 3]], band: usize) -> Vec<f32> {
    px.iter()
        .map(|p| if fit::evidence_hue_band(p) == Some(band) { 1.0 } else { 0.0 })
        .collect()
}

/// One hue band's own colour and the chromaticity radius that holds nine of
/// its members in ten, plus the member that sits closest to that colour.
///
/// The radius is the band's [`ZONE_BOUNDARY_PERCENTILE`] distance from its own
/// weighted mean: DERIVED from the population, not tuned, and taken at the
/// same percentile the boundary readings are budgeted at, so the mask and the
/// gate agree on which members are the body of the band and which are its
/// edge.
///
/// `None` when the band has no members, or when its mean colour is too dark to
/// carry a chromaticity at all — the same rule `render::range_weight` applies
/// to a near-black reference, asked before a mask is built rather than after.
fn colour_band_centre(px: &[[f32; 3]], members: &[f32]) -> Option<([f32; 3], f32, usize)> {
    let mut mass = 0.0f64;
    let mut mean = [0.0f64; 3];
    for (p, &w) in px.iter().zip(members) {
        if w <= 0.0 {
            continue;
        }
        mass += w as f64;
        for (channel, value) in mean.iter_mut().zip(p) {
            *channel += w as f64 * *value as f64;
        }
    }
    if mass <= 0.0 {
        return None;
    }
    let reference = [
        (mean[0] / mass) as f32,
        (mean[1] / mass) as f32,
        (mean[2] / mass) as f32,
    ];
    let mut distances = Vec::new();
    let mut nearest: Option<(f32, usize)> = None;
    for (i, (p, &w)) in px.iter().zip(members).enumerate() {
        if w <= 0.0 {
            continue;
        }
        let d = render::chromaticity_distance(&reference, p);
        if !d.is_finite() {
            continue;
        }
        distances.push(d);
        if nearest.is_none_or(|(best, _)| d < best) {
            nearest = Some((d, i));
        }
    }
    let (_, sample) = nearest?;
    distances.sort_by(f32::total_cmp);
    let rank = ((distances.len() as f32 * ZONE_BOUNDARY_PERCENTILE).ceil() as usize)
        .saturating_sub(1)
        .min(distances.len() - 1);
    Some((reference, distances[rank], sample))
}

/// The colour Range Mask keyed to one band's colour at that band's radius.
///
/// The tolerance IS the radius. Measured on the synthetic scattered-hue pair:
/// at the radius, every unit of the ball's weight lands on a member of the
/// band it was derived from (purity 1.000 on both of that frame's bands); at
/// twice the radius — which would put the p90 member in the full-weight core
/// rather than on the ramp — purity falls to 0.55 and 0.78, because the ball
/// starts selecting the very pixels the band excluded.
///
/// AND IT STOPS BEFORE NEUTRAL. A ball wide enough to reach the achromatic
/// axis is not a colour range: it would select grey, which belongs to no hue
/// band at all, and a correction through it is a global move that walked past
/// the global stage's gates. The reference colour's own distance from neutral
/// is that limit, measured rather than chosen, and at it grey lands exactly on
/// the tolerance where the weight reaches zero.
///
/// Over 1.05 the sidecar's grammar cannot go, so a very wide band gets a mask
/// narrower than its measurement asked for and the proposal disclosure prints
/// the tolerance that was actually bought.
///
/// `(px, py)` is the member closest to the reference colour. Lightroom treats
/// it as a cosmetic sample marker, and this way it is a real sample of the
/// band instead of an invented point.
fn colour_mask(
    reference: [f32; 3],
    radius: f32,
    sample: usize,
    width: u32,
    height: u32,
) -> RangeMask {
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    let neutral = render::chromaticity_distance(&reference, &[0.5; 3]);
    let tolerance = if neutral.is_finite() { radius.min(neutral) } else { radius };
    RangeMask::Color {
        r: reference[0],
        g: reference[1],
        b: reference[2],
        amount: render::colour_range_amount(tolerance),
        px: ((sample % w) as f32 + 0.5) / w as f32,
        py: ((sample / w).min(h - 1) as f32 + 0.5) / h as f32,
    }
}

/// Derive colour bands from the current rendered stack.
///
/// THE CASE THIS PRODUCER EXISTS FOR is a look that differs by COLOUR over a
/// population no rectangle and no silhouette can hold: the same hue wherever it
/// appears in the frame. The luminance producer above cannot see it, because a
/// hue move need not move luma at all; the spatial producers cannot hold it,
/// because the population is scattered. Until this existed such a look was
/// fitted globally or not at all.
///
/// THE EVIDENCE IS TWO-SIDED OR THERE IS NO BAND, and that is not a
/// precaution: a hue-shifting edit DEPOPULATES its own band on the target
/// side, so a band only the source populates is exactly the shape a circular
/// argument takes here — "the target has no blues, therefore correct the
/// blues" is the edit talking about itself. Two independent arms answer it.
/// The FROZEN original-pair verdict `evidence.hue[band]` is asked through the
/// same [`evidence_refusal`] the luminance producer uses, so both sides must
/// clear the 1.5% population floor and the band must carry structural weight.
/// Then the DELIVERED populations are asked the same question again, on the
/// two frames the mask will actually be applied to, because the global stage
/// may have moved hues since the evidence was frozen. A band that fails either
/// arm abstains with the reading that refused it. A third arm sits downstream
/// and is not optional either: [`attach_one_zone`] re-scopes the evidence over
/// the mask's OWN population and withholds the colour class outright when the
/// hue bands that population moves are unsupported.
///
/// A GLOBAL COLOUR MOVE PRODUCES NO BAND HERE, for the same reason a global
/// tone move produces no luminance band: the residual is read on the CURRENT
/// render, so whatever the global stage already matched is gone before this
/// function sees it, and the bands read under [`ZONE_MATCHED_ERR`] — the
/// observed matched domain, which is the line a band has to clear to be
/// proposed at all. A global move the global stage was REFUSED is caught by
/// the two-sidedness arms instead, because refusing it does not put the
/// target's hues back into the source's bands.
fn derive_colour_bands(
    source_px: &[[f32; 3]],
    target_px: &[[f32; 3]],
    evidence: &fit::EvidenceModel,
    width: u32,
    height: u32,
) -> ColourDerivation {
    #[cfg(test)]
    if COLOUR_RANGE_SUPPRESSED.with(std::cell::Cell::get) {
        return ColourDerivation::default();
    }
    let mut out = ColourDerivation::default();
    let n = source_px.len().min(target_px.len());
    if n == 0 {
        return out;
    }
    let (source_px, target_px) = (&source_px[..n], &target_px[..n]);
    for band in 0..crate::recipe::HSL_BANDS.len().min(evidence.hue.len()) {
        let source_members = hue_band_members(source_px, band);
        let target_members = hue_band_members(target_px, band);
        let residual = zone_err(
            &zone_moments(source_px, &source_members),
            &zone_moments(target_px, &target_members),
        );
        // Silent under the observed matched domain, exactly as a luminance bin
        // under the residual trigger is silent. A quiet band is not a refusal
        // anyone needs explained, and eight of them per fit would bury the
        // bands that were refused for a reason.
        if residual < ZONE_MATCHED_ERR {
            continue;
        }
        if let Some(reason) = evidence_refusal(&evidence.hue[band]) {
            out.abstentions.push(ColourAbstention { band, reason });
            continue;
        }
        let share_of = |members: &[f32]| members.iter().sum::<f32>() / n as f32;
        let (source_share, target_share) =
            (share_of(&source_members), share_of(&target_members));
        if source_share < RANGE_MIN_EVIDENCE_SHARE || target_share < RANGE_MIN_EVIDENCE_SHARE {
            out.abstentions.push(ColourAbstention {
                band,
                reason: format!(
                    "the rendered band holds {:.1}% and the target band {:.1}% \
                     of the frame, against the {:.1}% two-sided floor",
                    source_share * 100.0,
                    target_share * 100.0,
                    RANGE_MIN_EVIDENCE_SHARE * 100.0,
                ),
            });
            continue;
        }
        let Some((reference, radius, sample)) = colour_band_centre(source_px, &source_members)
        else {
            out.abstentions.push(ColourAbstention {
                band,
                reason: "the band's mean colour is too dark to carry a chromaticity"
                    .to_string(),
            });
            continue;
        };
        // ONE MASK, READ ON BOTH FRAMES. The target population has to be
        // defined the same way as the source one or the share gate reads a
        // sizing artefact as a composition mismatch: on the synthetic pair the
        // target band's OWN ball covered 4.7% of the frame against the source
        // ball's 18.2%, for populations that are 26.9% and 26.8% of it. The
        // emitted mask is the ruler for both sides, so what the share gate
        // compares is one question asked twice — which is why `target` here is
        // the same mask and not a second one keyed to the target's colour.
        //
        // That also makes the gate a circularity guard in its own right, and
        // the last of four: a colour move large enough to carry the target's
        // pixels out of the mask leaves the target share far under the source's
        // and the shared 2:1 composition gate refuses the band, rather than
        // fitting a correction to a population that is no longer there.
        let source = colour_mask(reference, radius, sample, width, height);
        let target = source;
        let coverage = ZoneCoverage {
            source: range_weights_for_pixels(&source, source_px),
            target: range_weights_for_pixels(&target, target_px),
        };
        let share = coverage.source.iter().sum::<f32>() / n as f32;
        let verdict = &evidence.hue[band];
        let d = if verdict.divergence.is_finite() { verdict.divergence } else { 1.0 };
        out.proposals.push(ColourProposal {
            band: RangeBand {
                attachment: ZoneAttachment {
                    // Both weight vectors are placeholders the entry point
                    // replaces: the source population is re-derived from the
                    // current rendered stack before every attempt, and the
                    // target population is normalised against every OTHER band
                    // in the same fit. They start as this band's own raw ramp so
                    // the structure is meaningful when a test builds one alone.
                    source_weights: coverage.source.clone(),
                    target_weights: coverage.target.clone(),
                    coverage: Some(coverage),
                    mask: RANGE_HOST,
                    range: Some(source),
                    name: String::new(),
                    role: MaskRole::Custom,
                    inverted: false,
                    label: String::new(),
                    min_share: MIN_ZONE_SHARE,
                    frame_regression_tol: RANGE_FRAME_REGRESSION_TOL,
                },
                source,
                target,
                divergence: fit::Divergence {
                    correlation: (1.0 - d).clamp(-1.0, 1.0),
                    energy_error: 0.0,
                    d,
                },
                hue_band: Some(band),
            },
            hue_band: band,
            share,
            tolerance: match source {
                RangeMask::Color { amount, .. } => render::colour_range_tolerance(amount),
                RangeMask::Luminance { .. } => 0.0,
            },
            residual,
            score: residual * verdict.two_sided_share.max(1e-6),
        });
    }
    // The same cap the luminance family answers to, applied to this family's
    // own set: four bands is the measured stability ceiling for value-range
    // evidence and a hue band is no better identified than a luma one. The
    // ranking is the luminance family's too — residual weighted by the band's
    // two-sided share — so a large move in a band barely anyone populates does
    // not displace a real one. Ties go to the lower band index, so the outcome
    // does not depend on iteration order.
    out.proposals.sort_by(|a, b| {
        b.score.total_cmp(&a.score).then_with(|| a.hue_band.cmp(&b.hue_band))
    });
    for dropped in out.proposals.split_off(out.proposals.len().min(RANGE_MAX_BANDS)) {
        out.abstentions.push(ColourAbstention {
            band: dropped.hue_band,
            reason: format!(
                "band residual {:.3} ranked below the {} bands the evidence cap keeps",
                dropped.residual, RANGE_MAX_BANDS,
            ),
        });
    }
    // Attach in band order, not in score order: the rationale then reads red to
    // magenta the way the HSL mixer does, and the mask names follow it.
    out.proposals.sort_by_key(|proposal| proposal.hue_band);
    out.abstentions.sort_by_key(|abstention| abstention.band);
    for (index, proposal) in out.proposals.iter_mut().enumerate() {
        proposal.band.attachment.name = format!("Colour range {:02}", index + 1);
        proposal.band.attachment.label = format!(
            "Colour range {:02} ({})",
            index + 1,
            crate::recipe::HSL_BANDS[proposal.hue_band],
        );
    }
    out
}

fn push_colour_proposal(report: &mut FitReport, proposal: &ColourProposal) {
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::COLOUR_RANGE_PROPOSED,
            vec![
                ("label", proposal.band.attachment.label.clone()),
                ("band", crate::recipe::HSL_BANDS[proposal.hue_band].to_string()),
                ("share", format!("{:.1}", proposal.share * 100.0)),
                ("tol", format!("{:.3}", proposal.tolerance)),
                ("residual", format!("{:.3}", proposal.residual)),
            ],
        ),
    );
}

fn push_colour_abstention(report: &mut FitReport, abstention: &ColourAbstention) {
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::COLOUR_RANGE_ABSTAINED,
            vec![
                ("band", crate::recipe::HSL_BANDS[abstention.band].to_string()),
                ("reason", abstention.reason.clone()),
            ],
        ),
    );
}

/// One family's bands, attached, gated and shrunk as a stage of its own.
///
/// TWO FAMILIES, TWO STAGES, and that is not tidiness. The boundary gate
/// shrinks every correction it holds by ONE shared `k`, so a colour band whose
/// edge is over budget would crush the luminance bands beside it: measured on
/// the p37 corpus pair, one gate over both families took `k` to 0.003 and the
/// frame from 0.15915 — where the luminance bands alone left it — to 0.16474.
/// The composed-frame ceiling could not see that, because it compares against
/// the residual the fallback was handed and not against the stage before it.
/// Staged, each family carries its own shrink and its own ceiling, and the
/// colour family is measured against the frame the luminance family actually
/// left behind. It is the same layering the producers above already use.
struct RangeStage {
    accepted: usize,
    worst: f32,
    /// The render this stage leaves behind; `None` when it attached nothing
    /// and the frame the caller already had still stands.
    pixels: Option<Vec<[f32; 3]>>,
}

fn attach_band_stage(
    s_img: &DynamicImage,
    tgt_px: &[[f32; 3]],
    report: &mut FitReport,
    mut bands: Vec<RangeBand>,
    reference_px: &[[f32; 3]],
) -> RangeStage {
    let nothing = RangeStage { accepted: 0, worst: 0.0, pixels: None };
    if bands.is_empty() {
        return nothing;
    }
    // The frame metric this stage was HANDED is its ceiling; every band it
    // accepts must leave the composed frame no worse than it found it.
    let entry_frame_err = report.err_after;
    let all_ranges = bands.iter().map(|band| band.source).collect::<Vec<_>>();
    let target_coverage = bands
        .iter()
        .map(|band| range_weights_for_pixels(&band.target, tgt_px))
        .collect::<Vec<_>>();
    let mut target_weights = target_coverage.clone();
    normalize_partition_weights(&mut target_weights);
    for (band, weights) in bands.iter_mut().zip(target_weights) {
        band.attachment.target_weights = weights;
    }
    let first_range = report.recipe.masks.len();
    let mut frame_err = report.err_after;
    let corr = report.correspondence.take();
    let mut accepted = Vec::new();
    for i in 0..bands.len() {
        let (mut current_weights, mut current_coverage) =
            range_weights_from_current_render(s_img, &report.recipe, &all_ranges);
        bands[i].attachment.source_weights = std::mem::take(&mut current_weights[i]);
        bands[i].attachment.coverage = Some(ZoneCoverage {
            source: std::mem::take(&mut current_coverage[i]),
            target: target_coverage[i].clone(),
        });
        let accepted_band = attach_one_zone(
            s_img,
            tgt_px,
            report,
            &mut frame_err,
            &bands[i].attachment,
            // A band's divergence is DERIVED from its own evidence range
            // rather than read off the structural instrument, so it is always
            // present: there is nothing here for the instrument to abstain
            // about.
            Some(bands[i].divergence),
            corr.as_ref(),
        );
        let refused = "the shared estimator or do-no-harm gates refused the correction";
        match (accepted_band, bands[i].hue_band) {
            (Some(zone), _) => accepted.push(zone),
            (None, Some(band)) => push_colour_abstention(
                report,
                &ColourAbstention { band, reason: refused.to_string() },
            ),
            (None, None) => push_range_abstention(
                report,
                &RangeAbstention {
                    lo: match bands[i].source {
                        RangeMask::Luminance { lo, .. } => lo,
                        RangeMask::Color { .. } => 0.0,
                    },
                    hi: match bands[i].source {
                        RangeMask::Luminance { hi, .. } => hi,
                        RangeMask::Color { .. } => 1.0,
                    },
                    reason: refused.to_string(),
                },
            ),
        }
    }
    report.correspondence = corr;
    if accepted.is_empty() {
        return nothing;
    }
    let initial_px = accepted.last().expect("accepted range exists").rendered.clone();
    let accepted_ranges = accepted
        .iter()
        .filter_map(|zone| zone.range)
        .collect::<Vec<_>>();
    let shares = accepted
        .iter()
        .map(|zone| {
            zone.source_weights.iter().sum::<f32>()
                / zone.source_weights.len().max(1) as f32
        })
        .collect::<Vec<_>>();
    let final_px = match enforce_range_boundary_gate(
        s_img,
        report,
        reference_px,
        &accepted_ranges,
        &shares,
        first_range,
        initial_px,
    ) {
        BoundaryGateResult::Kept { pixels, .. } => pixels,
        BoundaryGateResult::Dropped => return nothing,
    };
    let final_frame_err = final_range_frame_err(&final_px, tgt_px, &report.evidence);
    if final_frame_err > entry_frame_err + RANGE_FRAME_REGRESSION_TOL {
        report.recipe.masks.truncate(first_range);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_FRAME_REFUSED,
                vec![
                    ("n", accepted.len().to_string()),
                    ("global", format!("{entry_frame_err:.3}")),
                    ("after", format!("{final_frame_err:.3}")),
                    ("tol", format!("{RANGE_FRAME_REGRESSION_TOL:+.3}")),
                ],
            ),
        );
        report.err_after = entry_frame_err;
        return nothing;
    }
    for zone in &mut accepted {
        let after = zone_moments(&final_px, &zone.source_weights);
        let target = zone_moments(tgt_px, &zone.target_weights);
        zone.after = zone_err(&after, &target);
        push_zone_attached_note(report, zone);
    }
    report.err_after = final_frame_err;
    RangeStage {
        accepted: accepted.len(),
        worst: accepted.iter().map(|zone| zone.after).fold(0.0f32, f32::max),
        pixels: Some(final_px),
    }
}

/// Automatic pure-Rust fallback after the global fit, and the one entry point
/// both range families share.
///
/// Luminance bands run first, in ascending luma order, then colour bands in
/// ACR band order on the frame the luminance stage left behind. Each family is
/// a stage of its own — see [`attach_band_stage`] for the measurement that
/// made that necessary — and every attempt inside a stage derives its source
/// population from the current rendered stack rather than the untouched
/// source. The confidence the two stages earn is reported once, at the end,
/// on the worst band either of them accepted.
pub(super) fn attach_ranges(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    proposals: &[FieldBandProposal],
) {
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let tgt_px = fit::pixels_of(&t_img);
    let mut current_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    let derived = derive_luminance_bands(&current_px, &tgt_px, &report.evidence, proposals);
    for merged in &derived.merges {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_MERGED,
                vec![
                    ("lo", format!("{:.3}", merged.lo)),
                    ("hi", format!("{:.3}", merged.hi)),
                    ("into_lo", format!("{:.3}", merged.into_lo)),
                    ("into_hi", format!("{:.3}", merged.into_hi)),
                    ("sign", merged.sign.to_string()),
                    ("why", merged.why.to_string()),
                ],
            ),
        );
    }
    for abstention in &derived.abstentions {
        push_range_abstention(report, abstention);
    }
    let luminance = attach_band_stage(&s_img, &tgt_px, report, derived.bands, &current_px);
    if let Some(pixels) = luminance.pixels {
        current_px = pixels;
    }
    let colour = derive_colour_bands(
        &current_px,
        &tgt_px,
        &report.evidence,
        s_img.width(),
        s_img.height(),
    );
    for proposal in &colour.proposals {
        push_colour_proposal(report, proposal);
    }
    for abstention in &colour.abstentions {
        push_colour_abstention(report, abstention);
    }
    let bands = colour.proposals.into_iter().map(|proposal| proposal.band).collect();
    let colour = attach_band_stage(&s_img, &tgt_px, report, bands, &current_px);
    if let Some(pixels) = colour.pixels {
        current_px = pixels;
    }
    let accepted = luminance.accepted + colour.accepted;
    if accepted > 0 {
        let worst = luminance.worst.max(colour.worst);
        report.recipe.confidence = report
            .recipe
            .confidence
            .min(fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE));
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_CONFIDENCE,
                vec![
                    ("n", accepted.to_string()),
                    ("worst", format!("{worst:.3}")),
                    ("frame", format!("{:.3}", report.err_after)),
                ],
            ),
        );
    }
    fit::append_finished_disclosure(report, &current_px, &tgt_px);
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::{fixture_mask_path, neutral_report, zoned_pair};
    use image::RgbImage;

    fn synthetic_range_case(residuals: [f32; 17]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, fit::EvidenceModel) {
        const PER_BIN: usize = 8;
        let mut base = Vec::with_capacity(17 * PER_BIN);
        let mut source = Vec::with_capacity(17 * PER_BIN);
        let mut target = Vec::with_capacity(17 * PER_BIN);
        for (bin, residual) in residuals.into_iter().enumerate() {
            let v = (bin as f32 + 0.5) / 17.0;
            for _ in 0..PER_BIN {
                base.push([v; 3]);
                source.push([(v - residual).clamp(0.0, 1.0); 3]);
                target.push([v; 3]);
            }
        }
        let share = 1.0 / 17.0;
        let luma = (0..17)
            .map(|bin| fit::EvidenceRange {
                label: format!("luma-{bin:02}"),
                source_share: share,
                target_share: share,
                source_evidence_share: share,
                target_evidence_share: share,
                two_sided_share: share,
                divergence: 0.1,
                weight: share,
                source_populated: true,
                target_populated: true,
            })
            .collect();
        let n = source.len();
        let evidence = fit::EvidenceModel {
            source_pixels: base,
            source_membership: vec![1.0; n],
            width: n as u32,
            height: 1,
            spatial_supported: vec![true; n],
            source_weights: vec![1.0; n],
            target_weights: vec![1.0; n],
            source_hue_weights: vec![0.0; n],
            target_hue_weights: vec![0.0; n],
            luma,
            hue: Vec::new(),
            source_chroma_vector: [0.0; 2],
            target_chroma_vector: [0.0; 2],
            global_cast: None,
            identifiability: 1.0,
            spatial_weights: vec![1.0; n],
            spatial_divergence: vec![0.0; n],
            globally_same_content: true,
            population: n as f32,
        };
        (source, target, evidence)
    }

    #[test]
    fn field_proposals_enter_before_range_sort_and_cap() {
        let cores_of = |derived: &RangeDerivation| derived.bands.iter().map(|band| {
            let (_, lo, hi, _) = luminance_bounds(band.source);
            (lo, hi)
        }).collect::<Vec<_>>();
        let is_field_band = |(lo, hi): &(f32, f32)| {
            (lo - 14.0 / 17.0).abs() < 1e-6 && (hi - 15.0 / 17.0).abs() < 1e-6
        };
        // Three rank-paired runs plus a field-only band: four bands, the field
        // band among them. Deleting the union loop leaves three.
        let mut residuals = [0.0; 17];
        for (bin, residual) in [(1, 0.08), (4, -0.08), (7, 0.08)] {
            residuals[bin] = residual;
        }
        residuals[14] = 0.02; // below rank trigger: field-only candidate
        let (source, target, evidence) = synthetic_range_case(residuals);
        let proposal = FieldBandProposal { lo: 14.0 / 17.0, hi: 15.0 / 17.0, sign: 0.02 };
        let derived = derive_luminance_bands(&source, &target, &evidence, std::slice::from_ref(&proposal));
        let cores = cores_of(&derived);
        assert_eq!(cores.len(), 4, "field band did not enter: {derived:?}");
        assert!(cores.iter().any(is_field_band), "{cores:?}");
        assert!(cores.windows(2).all(|pair| pair[0].1 <= pair[1].0), "{cores:?}");
        // Four rank-paired runs already fill the cap: the weaker field band is
        // ranked WITH them and cut by the cap, never appended past it.
        residuals[10] = -0.08;
        let (source, target, evidence) = synthetic_range_case(residuals);
        let capped = derive_luminance_bands(&source, &target, &evidence, &[proposal]);
        let cores = cores_of(&capped);
        assert_eq!(cores.len(), RANGE_MAX_BANDS, "field union bypassed cap: {capped:?}");
        assert!(!cores.iter().any(is_field_band), "{cores:?}");
        assert!(cores.windows(2).all(|pair| pair[0].1 <= pair[1].0), "{cores:?}");
    }

    #[test]
    fn field_proposal_union_merges_same_sign_and_refuses_opposite_overlap() {
        let mut residuals = [0.0; 17];
        residuals[4] = 0.08;
        let (source, target, evidence) = synthetic_range_case(residuals);
        let same = FieldBandProposal { lo: 3.0 / 17.0, hi: 6.0 / 17.0, sign: 0.1 };
        let merged = derive_luminance_bands(&source, &target, &evidence, &[same]);
        assert_eq!(merged.bands.len(), 1, "{merged:?}");
        let (_, lo, hi, _) = luminance_bounds(merged.bands[0].source);
        assert!((lo - 3.0 / 17.0).abs() < 1e-6 && (hi - 6.0 / 17.0).abs() < 1e-6);
        assert_eq!(merged.merges.len(), 1);
        assert_eq!(merged.merges[0].why, "absorbed by the overlapping rank-paired run before the cap");

        let opposite = FieldBandProposal { lo: 3.0 / 17.0, hi: 6.0 / 17.0, sign: -0.1 };
        let refused = derive_luminance_bands(&source, &target, &evidence, &[opposite]);
        assert_eq!(refused.bands.len(), 1);
        assert!(refused.abstentions.iter().any(|a|
            a.reason == "field proposal conflicts with the rank-paired band"));

        // No rank-paired run at bin 8 (its residual is below the trigger) and
        // the field reads the opposite direction: a disagreement, not a band.
        let mut disagreeing = [0.0; 17];
        disagreeing[8] = -0.02;
        let (source, target, evidence) = synthetic_range_case(disagreeing);
        let wrong_way = FieldBandProposal { lo: 8.0 / 17.0, hi: 9.0 / 17.0, sign: 0.1 };
        let refused = derive_luminance_bands(&source, &target, &evidence, &[wrong_way]);
        assert!(refused.bands.is_empty(), "{refused:?}");
        assert!(refused.abstentions.iter().any(|a|
            a.reason == "field proposal sign disagrees with its rank-paired residual"), "{refused:?}");
    }

    #[test]
    fn field_proposal_spans_are_mapped_through_the_pixels_that_occupy_them() {
        // Every bin is rendered 0.10 darker than its original: a span of
        // CURRENT luma names ORIGINAL bins about 1.7 bins brighter, so a
        // proposal read in the field's domain must not be reused as an index.
        let (source, _target, evidence) = synthetic_range_case([0.10; 17]);
        let proposal = FieldBandProposal { lo: 0.20, hi: 0.30, sign: 0.1 };
        let mapped = evidence_bins_for_span(&proposal, &source, &evidence);
        // Current luma in [0.20, 0.30) <=> original centres 0.324 (bin 5) and
        // 0.382 (bin 6); the naive index of the span would have been bins 3..5.
        assert_eq!(mapped, Some((5, 6)), "{mapped:?}");
        let empty = FieldBandProposal { lo: 0.98, hi: 1.0, sign: 0.1 };
        assert_eq!(evidence_bins_for_span(&empty, &source, &evidence), None);
    }

    fn luminance_bounds(range: RangeMask) -> (f32, f32, f32, f32) {
        match range {
            RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => {
                (lo_outer, lo, hi, hi_outer)
            }
            RangeMask::Color { .. } => panic!("range derivation emitted a colour range"),
        }
    }

    #[test]
    fn range_band_derivation_follows_signed_residual_runs_and_caps_at_four() {
        let mut residuals = [0.0; 17];
        residuals[1] = 0.04;
        residuals[2] = 0.05;
        residuals[8] = 0.04;
        residuals[9] = 0.10;
        residuals[10] = 0.12;
        residuals[12] = -0.105;
        residuals[13] = -0.04;
        let (source, target, evidence) = synthetic_range_case(residuals);
        let derived = derive_luminance_bands(&source, &target, &evidence, &[]);
        let cores = derived
            .bands
            .iter()
            .map(|band| {
                let (_, lo, hi, _) = luminance_bounds(band.source);
                (lo, hi)
            })
            .collect::<Vec<_>>();
        assert_eq!(cores.len(), 3, "coherent neutral gaps must remain unmasked: {cores:?}");
        assert!((cores[0].0 - 1.0 / 17.0).abs() < 1e-6);
        assert!((cores[0].1 - 3.0 / 17.0).abs() < 1e-6);
        assert!((cores[1].0 - 8.0 / 17.0).abs() < 1e-6);
        assert!((cores[2].0 - 12.0 / 17.0).abs() < 1e-6);

        let mut five = [0.0; 17];
        for (bin, residual) in [(1, 0.07), (3, -0.07), (5, 0.07), (7, -0.07), (9, 0.07)] {
            five[bin] = residual;
        }
        let (source, target, evidence) = synthetic_range_case(five);
        let capped = derive_luminance_bands(&source, &target, &evidence, &[]);
        assert_eq!(capped.bands.len(), RANGE_MAX_BANDS);
        assert!(capped.merges.is_empty(), "a merge may not cross the retained opposite-sign core");
        assert!(capped.abstentions.iter().any(|a| a.reason.contains("no adjacent same-sign")));
        let cores = capped
            .bands
            .iter()
            .map(|band| {
                let (_, lo, hi, _) = luminance_bounds(band.source);
                (lo, hi)
            })
            .collect::<Vec<_>>();
        assert!(cores.windows(2).all(|pair| pair[0].1 < pair[1].0));

        let mut mergeable = [0.0; 17];
        for (bin, residual) in [(1, 0.061), (3, 0.07), (5, -0.10), (7, 0.10), (9, -0.10)] {
            mergeable[bin] = residual;
        }
        let (source, target, evidence) = synthetic_range_case(mergeable);
        let merged = derive_luminance_bands(&source, &target, &evidence, &[]);
        assert_eq!(merged.bands.len(), RANGE_MAX_BANDS);
        assert_eq!(merged.merges.len(), 1, "an adjacent same-sign run remains mergeable");
        let cores = merged
            .bands
            .iter()
            .map(|band| {
                let (_, lo, hi, _) = luminance_bounds(band.source);
                (lo, hi)
            })
            .collect::<Vec<_>>();
        assert!(cores.windows(2).all(|pair| pair[0].1 < pair[1].0));
    }

    #[test]
    fn range_band_derivation_is_invariant_to_target_pixel_positions() {
        let mut residuals = [0.0; 17];
        residuals[2] = 0.07;
        residuals[8] = -0.08;
        residuals[13] = 0.09;
        let (source, target, evidence) = synthetic_range_case(residuals);
        let expected = derive_luminance_bands(&source, &target, &evidence, &[]);
        let mut shuffled = target;
        shuffled.rotate_left(37);
        shuffled.reverse();
        let actual = derive_luminance_bands(&source, &shuffled, &evidence, &[]);
        let canonical = |mut derived: RangeDerivation| {
            for band in &mut derived.bands {
                band
                    .attachment
                    .coverage
                    .as_mut()
                    .expect("a range owns target coverage")
                    .target
                    .sort_by(f32::total_cmp);
            }
            derived
        };
        assert_eq!(
            canonical(actual),
            canonical(expected),
            "target positions must not influence rank-derived bands or the target ramp's mass",
        );
    }

    #[test]
    fn range_band_abstains_with_one_sided_or_zero_structural_evidence() {
        let mut residuals = [0.0; 17];
        residuals[1] = 0.07;
        let (source, target, mut evidence) = synthetic_range_case(residuals);
        evidence.luma[1].target_populated = false;
        evidence.luma[1].target_share = 0.0;
        evidence.luma[1].target_evidence_share = 0.0;
        evidence.luma[1].two_sided_share = 0.0;
        evidence.luma[1].weight = 0.0;
        let one_sided = derive_luminance_bands(&source, &target, &evidence, &[]);
        assert!(one_sided.bands.is_empty());
        assert_eq!(one_sided.abstentions.len(), 1);
        assert!(one_sided.abstentions[0].reason.contains("target population"));

        evidence.luma[1].target_populated = true;
        evidence.luma[1].target_evidence_share = 1.0 / 17.0;
        evidence.luma[1].two_sided_share = 1.0 / 17.0;
        evidence.luma[1].weight = 1.0 / 17.0;
        let population_only = derive_luminance_bands(&source, &target, &evidence, &[]);
        assert!(population_only.bands.is_empty());
        assert_eq!(population_only.abstentions.len(), 1);
        assert!(population_only.abstentions[0].reason.contains("target population"));

        evidence.luma[1].target_share = 1.0 / 17.0;
        evidence.luma[1].weight = 0.0;
        let structural = derive_luminance_bands(&source, &target, &evidence, &[]);
        assert!(structural.bands.is_empty());
        assert!(structural.abstentions[0].reason.contains("zero structural evidence"));

        let mut adjacent = [0.0; 17];
        adjacent[1] = 0.07;
        adjacent[2] = 0.07;
        let (source, target, mut evidence) = synthetic_range_case(adjacent);
        evidence.luma[2].weight = 0.0;
        let no_hitchhike = derive_luminance_bands(&source, &target, &evidence, &[]);
        assert_eq!(no_hitchhike.bands.len(), 1);
        let (_, lo, hi, _) = luminance_bounds(no_hitchhike.bands[0].source);
        assert!((lo - 1.0 / 17.0).abs() < 1e-6 && (hi - 2.0 / 17.0).abs() < 1e-6);
        assert!(no_hitchhike.abstentions.iter().any(|a| {
            (a.lo - 2.0 / 17.0).abs() < 1e-6 && a.reason.contains("zero structural evidence")
        }));

        // The SOURCE population arm is its own gate: a bin can hold a few
        // actual pixels (bin_count > 0) while sitting under the 1.5% evidence
        // floor, and only this arm refuses it.
        let mut src_gap = [0.0; 17];
        src_gap[1] = 0.07;
        let (source, target, mut evidence) = synthetic_range_case(src_gap);
        evidence.luma[1].source_populated = false;
        evidence.luma[1].source_share = 0.0;
        let source_side = derive_luminance_bands(&source, &target, &evidence, &[]);
        assert!(source_side.bands.is_empty());
        assert!(source_side.abstentions[0].reason.contains("source population"));
    }

    #[test]
    fn range_band_ramps_are_ordered_and_partition_weights_do_not_exceed_one() {
        let mut residuals = [0.0; 17];
        residuals[5] = 0.07;
        residuals[6] = -0.07;
        let (mut source, target, evidence) = synthetic_range_case(residuals);
        let overlap_i = 5 * 8;
        let compensate_i = overlap_i + 1;
        let original = source[overlap_i][0];
        let overlap_value = 6.25 / 17.0;
        source[overlap_i] = [overlap_value; 3];
        source[compensate_i] = [original - (overlap_value - original); 3];
        let derived = derive_luminance_bands(&source, &target, &evidence, &[]);
        assert_eq!(derived.bands.len(), 2);
        for band in &derived.bands {
            let (lo_outer, lo, hi, hi_outer) = luminance_bounds(band.source);
            assert!(lo_outer <= lo && lo <= hi && hi <= hi_outer);
            assert!(lo - lo_outer > 0.0 || lo == 0.0, "interior lower ramp is hard");
            assert!(hi_outer - hi > 0.0 || hi == 1.0, "interior upper ramp is hard");
            assert!(lo - lo_outer <= RANGE_MAX_RAMP + 1e-6);
            assert!(hi_outer - hi <= RANGE_MAX_RAMP + 1e-6);
            let coverage = band.attachment.coverage.as_ref().expect("a range owns its raw ramp");
            assert_eq!(coverage.source, range_weights_for_pixels(&band.source, &source));
            assert_eq!(coverage.target, range_weights_for_pixels(&band.target, &target));
        }
        let overlap = (0..source.len()).find(|&i| {
            derived
                .bands
                .iter()
                .filter(|band| band.attachment.coverage.as_ref().unwrap().source[i] > 0.0)
                .count()
                > 1
                && derived.bands.iter().any(|band| {
                    band.attachment.coverage.as_ref().unwrap().source[i]
                        > band.attachment.source_weights[i]
                })
        });
        assert!(overlap.is_some(), "adjacent raw feathers must exceed their normalized estimator");
        for i in 0..source.len() {
            let sum = derived
                .bands
                .iter()
                .map(|band| band.attachment.source_weights[i])
                .sum::<f32>();
            assert!(sum <= 1.0 + 1e-6, "source partition overlaps at {i}: {sum}");
        }
        let mut target_weights = derived
            .bands
            .iter()
            .map(|band| range_weights_for_pixels(&band.target, &target))
            .collect::<Vec<_>>();
        normalize_partition_weights(&mut target_weights);
        for i in 0..target.len() {
            let sum = target_weights.iter().map(|band| band[i]).sum::<f32>();
            assert!(sum <= 1.0 + 1e-6, "target partition overlaps at {i}: {sum}");
        }
    }

    fn range_boundary_fixture() -> (DynamicImage, Vec<[f32; 3]>, Vec<RangeMask>) {
        let (w, h) = (512u32, 8u32);
        let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(w, h, |x, _| {
            let v = (x as f32 / (w - 1) as f32 * 255.0).round() as u8;
            image::Rgb([v; 3])
        }));
        let ranges = vec![
            RangeMask::Luminance {
                lo_outer: 0.0,
                lo: 0.45,
                hi: 0.50,
                hi_outer: 0.55,
            },
            RangeMask::Luminance {
                lo_outer: 0.45,
                lo: 0.50,
                hi: 0.55,
                hi_outer: 0.60,
            },
        ];
        let reference = fit::pixels_of(&render::develop_preview(
            &source,
            &crate::recipe::EditRecipe::default(),
        ));
        (source, reference, ranges)
    }

    fn opposing_range_recipe(ranges: &[RangeMask]) -> crate::recipe::EditRecipe {
        crate::recipe::EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: RANGE_HOST,
                    range: Some(ranges[0]),
                    exposure_ev: 0.5,
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: RANGE_HOST,
                    range: Some(ranges[1]),
                    exposure_ev: -0.5,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    fn rank_wave_target(source: &DynamicImage) -> DynamicImage {
        let (w, h) = (source.width(), source.height());
        DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(w, h, |x, _| {
            let v = x as f32 / (w - 1) as f32;
            let target = v + 0.15 * (std::f32::consts::TAU * v).sin();
            let value = (target.clamp(0.0, 1.0) * 255.0).round() as u8;
            image::Rgb([value; 3])
        }))
    }

    fn compact_rank_wave_fixture() -> (DynamicImage, DynamicImage) {
        let image = |target: bool| {
            DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(512, 16, |x, _| {
                let v = (x % 128) as f32 / 127.0;
                let value = if target {
                    v + 0.15 * (std::f32::consts::TAU * v).sin()
                } else {
                    v
                };
                let value = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                image::Rgb([value; 3])
            }))
        };
        (image(false), image(true))
    }

    #[test]
    fn range_boundary_rim_rejects_the_measured_opposite_half_ev_stress() {
        let (source, reference, ranges) = range_boundary_fixture();
        let rendered = fit::pixels_of(&render::develop_preview(
            &source,
            &opposing_range_recipe(&ranges),
        ));
        let reading = range_transition_rim(
            &reference,
            &rendered,
            &ranges,
            source.width(),
            source.height(),
        );
        assert!(
            reading.rim > RANGE_BOUNDARY_RIM_MAX && reading.rim < 0.05,
            "the retained 5/255-class stress must cross 0.012 but not 0.05: {reading:?}"
        );
    }

    /// A ramp whose columns advance by 2 and 4 codes in turn: the 2-code
    /// steps sit inside `range_transition_rim`'s smooth-crossing window and
    /// the 4-code ones are the real edges it exists to exclude. Vertical
    /// neighbours are identical, exactly as a real smooth region's are along
    /// its iso-luminance contours.
    fn mixed_gradient_fixture() -> (Vec<[f32; 3]>, Vec<RangeMask>) {
        let (w, h) = (80u32, 8u32);
        let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(w, h, |x, _| {
            let steps = (x / 2) * 6 + (x % 2) * 2;
            image::Rgb([(5 + steps).min(255) as u8; 3])
        }));
        // One transition spanning the whole ramp, so the reading is the p90
        // of the fixture's own gradient and nothing else.
        let ranges = vec![RangeMask::Luminance {
            lo_outer: 0.0,
            lo: 0.98,
            hi: 0.99,
            hi_outer: 1.0,
        }];
        let reference = fit::pixels_of(&render::develop_preview(
            &source,
            &crate::recipe::EditRecipe::default(),
        ));
        (reference, ranges)
    }

    /// THE SCALE OF THIS RULER, pinned (2026-09-01). Unlike [`boundary_step`],
    /// `range_transition_rim` never differences the scene away: it reports the
    /// RENDERED gradient at crossings whose REFERENCE gradient is already
    /// smooth (<= 2.5/255). Two consequences hold the budget in place and
    /// neither had a test.
    ///
    /// 1. The admission window BOUNDS an untouched render's own reading, so
    ///    [`RANGE_BOUNDARY_RIM_MAX`] has to clear 2.5/255 or the gate would
    ///    shrink corrections to pay for gradients they never touched. At the
    ///    p90 the gate ranks, what is left where the scene already fills the
    ///    window is 0.012 - 2.5/255 = 0.0022 luma, 0.56 of a code — and a
    ///    tenth of the crossings rank above it and are not bounded at all.
    /// 2. This family declines the contextual charge v1.2.2 gave the hard
    ///    raster ruler, which is what `charged == rim` states here.
    ///
    /// Measured first-party on the real pairs the same day, on the TWIN basis
    /// the engine itself passes (`&global_px` — the globals-only render, no
    /// range masks): with segmentation unavailable a range band attaches on
    /// the calibration pair (p90 0.00392 max 0.00978 uncorrected, 0.00230 max
    /// 0.01217 delivered, over 18297 transcribed crossings) and on the
    /// stone-viaduct pair (p90 0.00874 max 0.00978 to 0.00857 max 0.01407
    /// over 1224) — the correction moved the RANKED reading down on both,
    /// while the maxima it does not rank rose to 1.24x and 1.44x of the
    /// steepest crossing the window admitted. The engine's own gate counts,
    /// on the 384-px thumbnail it develops itself, are 18651 and 1214.
    #[test]
    fn the_range_rim_budget_clears_the_window_that_bounds_an_untouched_render() {
        let (reference, ranges) = mixed_gradient_fixture();
        let reading = range_transition_rim(&reference, &reference, &ranges, 80, 8);
        eprintln!("RANGE_WINDOW_CALIBRATION untouched={reading:?}");
        assert!(reading.transitions > 0, "premise: the ruler sampled: {reading:?}");
        assert!(
            reading.rim <= 2.5 / 255.0,
            "the smooth-crossing window must bound an untouched render's own \
             reading, or the gate charges a correction for the scene: {reading:?}"
        );
        // Derived from the render, not from two literals: the subtrahend is
        // this fixture's OWN untouched reading, so what is pinned is the
        // budget's scale against a measurement. At the p90 the gate ranks, a
        // correction on a scene that FILLS the window has 0.012 - 2.5/255 =
        // 0.56 of a code left; this fixture reads 2/255, so it leaves 1.06.
        let allowance_codes = (RANGE_BOUNDARY_RIM_MAX - reading.rim) * 255.0;
        assert!(
            (1.0..=1.1).contains(&allowance_codes),
            "the budget must clear the reading an untouched render produces \
             here, by the measured margin: {allowance_codes:.3} codes from \
             {reading:?}"
        );
        assert!(
            reading.rim <= RANGE_BOUNDARY_RIM_MAX,
            "and an untouched render must therefore pass its own gate: {reading:?}"
        );
        assert_eq!(
            reading.charged, reading.rim,
            "the luminance-range family declines the contextual charge: {reading:?}"
        );
    }

    #[test]
    fn range_boundary_gate_helper_shrinks_differentials_and_discloses_k() {
        let (source, reference, ranges) = range_boundary_fixture();
        let mut report = neutral_report(&source, &source);
        report.recipe = opposing_range_recipe(&ranges);
        let initial = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
        let weights = ranges
            .iter()
            .map(|range| range_weights_for_pixels(range, &reference))
            .collect::<Vec<_>>();
        let shares = weights
            .iter()
            .map(|band| band.iter().sum::<f32>() / band.len() as f32)
            .collect::<Vec<_>>();
        let verdict = enforce_range_boundary_gate(
            &source,
            &mut report,
            &reference,
            &ranges,
            &shares,
            0,
            initial,
        );
        let BoundaryGateResult::Kept { k, before, after, .. } = verdict else {
            panic!("zero differential must give the bisection a passing endpoint")
        };
        assert!(before.rim > RANGE_BOUNDARY_RIM_MAX);
        assert!(after.rim <= RANGE_BOUNDARY_RIM_MAX);
        assert!((0.0..1.0).contains(&k), "stress must require a real shrink: {k}");
        assert!(report.notes.iter().any(|n| {
            n.key == crate::rationale::keys::RANGE_BOUNDARY_PASSED
                && n.args.iter().any(|(key, value)| *key == "k" && value != "1.000")
        }));
    }

    #[test]
    fn range_boundary_gate_shrinks_differentials_and_discloses_k() {
        let (source, target) = compact_rank_wave_fixture();
        let mut report = neutral_report(&source, &target);
        let global_err = report.err_after;
        attach_ranges(&source, &target, &mut report, &[]);
        let note = report
            .notes
            .iter()
            .find(|note| {
                note.key == crate::rationale::keys::RANGE_BOUNDARY_PASSED
                    && note.args.iter().any(|(key, value)| *key == "k" && value != "1.000")
            })
            .unwrap_or_else(|| {
                panic!("the fallback orchestration did not call the shrinking boundary gate: {:?}", report.notes)
            });
        assert!(note.args.iter().any(|(key, _)| *key == "before"));
        assert!(
            report.err_after <= global_err + RANGE_FRAME_REGRESSION_TOL,
            "the post-shrink stack escaped the global-only frame ceiling"
        );
    }

    #[test]
    fn range_final_frame_ceiling_refuses_a_post_shrink_regression() {
        let (source, target) = compact_rank_wave_fixture();
        let mut report = neutral_report(&source, &target);
        let global_frame_err = report.err_after;
        RANGE_FINAL_FRAME_ERR_OVERRIDE.with(|value| value.set(Some(global_frame_err + 0.001)));
        attach_ranges(&source, &target, &mut report, &[]);
        assert!(report.recipe.masks.is_empty(), "the complete range stack must be reverted");
        assert_eq!(report.err_after, global_frame_err);
        assert!(report.notes.iter().any(|note| {
            note.key == crate::rationale::keys::RANGE_FRAME_REFUSED
                && note.args.iter().any(|(key, value)| *key == "tol" && value == "+0.000")
        }));
    }

    #[test]
    fn range_conservation_all_frame_band_reproduces_global_fit() {
        let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(64, 8, |x, _| {
            let v = (32 + x * 3).min(255) as u8;
            image::Rgb([v, v.saturating_add(3), v.saturating_add(7)])
        }));
        let mut without_range = crate::recipe::EditRecipe::default();
        without_range.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            exposure_ev: 0.35,
            ..Default::default()
        });
        let mut with_range = without_range.clone();
        with_range.masks[0].range = Some(RangeMask::Luminance {
            lo_outer: 0.0,
            lo: 0.0,
            hi: 1.0,
            hi_outer: 1.0,
        });
        assert_eq!(
            render::develop_preview(&source, &without_range).to_rgb8(),
            render::develop_preview(&source, &with_range).to_rgb8(),
            "the sentinel plus an all-frame luminance range must conserve every pixel"
        );
    }

    #[test]
    fn range_xmp_round_trip_preserves_host_and_luminance_range() {
        let range = RangeMask::Luminance {
            lo_outer: 0.10,
            lo: 0.20,
            hi: 0.70,
            hi_outer: 0.80,
        };
        let mut recipe = crate::recipe::EditRecipe::default();
        recipe.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(range),
            name: "Luminance range 01".to_string(),
            exposure_ev: 0.25,
            role: MaskRole::Custom,
            ..Default::default()
        });
        let xmp = crate::xmp::recipe_to_xmp(&recipe);
        assert_eq!(xmp.matches("Mask/RangeMask").count(), 1);
        assert!(!xmp.contains("Mask/Bitmap"));
        let round_trip = crate::xmp::xmp_to_recipe(&xmp);
        assert_eq!(round_trip.masks.len(), 1);
        assert_eq!(round_trip.masks[0].mask, RANGE_HOST);
        assert_eq!(round_trip.masks[0].range, Some(range));
    }

    #[test]
    fn range_serde_old_recipe_reads_and_new_range_recipe_is_explicitly_scoped() {
        let old = r#"{"version":2,"masks":[{"name":"legacy"}]}"#;
        let read: crate::recipe::EditRecipe = serde_json::from_str(old).expect("old recipe");
        assert_eq!(read.masks[0].range, None);
        assert_eq!(read.masks[0].role, MaskRole::Custom);

        let mut new = crate::recipe::EditRecipe::default();
        new.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(RangeMask::Luminance {
                lo_outer: 0.1,
                lo: 0.2,
                hi: 0.7,
                hi_outer: 0.8,
            }),
            role: MaskRole::Custom,
            ..Default::default()
        });
        let value = serde_json::to_value(&new).unwrap();
        assert_eq!(value["schema_era"], 1);
        assert_eq!(value["masks"][0]["role"], "custom");
        assert_eq!(value["masks"][0]["range"]["kind"], "luminance");
        serde_json::from_value::<crate::recipe::EditRecipe>(value).expect("explicit range scope");
    }

    #[test]
    fn range_abstention_preserves_global_recipe_byte_for_byte() {
        let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(96, 12, |x, _| {
            let v = (x * 255 / 95) as u8;
            image::Rgb([v; 3])
        }));
        let global = fit::fit_recipe(&source, &source);
        let mut deferred = fit::fit_recipe_from_promoted_with_disclosure(
            &source,
            &source,
            &crate::recipe::EditRecipe::default(),
            false,
            true,
            None,
        );
        attach_ranges(&source, &source, &mut deferred, &[]);
        assert_eq!(
            serde_json::to_vec(&deferred.recipe).unwrap(),
            serde_json::to_vec(&global.recipe).unwrap(),
            "an entirely abstaining range pass must be indistinguishable from global-only"
        );
        assert_eq!(deferred.err_after, global.err_after);
    }

    #[test]
    fn range_band_composed_frame_regression_is_dropped_while_neutral_or_better_is_kept() {
        let (w, h) = (64u32, 64u32);
        let build = |target: bool| DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v = 0.30 + 0.30 * x as f32 / (w - 1) as f32;
            let p = if y < h / 2 {
                if target { [v * 1.5, v * 1.5, v * 1.5] } else { [v * 0.72, v * 0.86, v] }
            } else { [0.45, 0.38, 0.28] };
            image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
        }));
        let source = build(false);
        let target = build(true);
        let mask =
            GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
        let path = fixture_mask_path("range-frame-regression");
        mask.save(path.path()).unwrap();
        let (s_img, t_img) = fit::analysis_pair(&source, &target);
        let tgt_px = fit::pixels_of(&t_img);
        let divergence = measure_zone_divergence(
            &source,
            &target,
            &crate::recipe::EditRecipe::default(),
            &mask,
        )
        .sky
        .divergence;
        let attachment = ZoneAttachment {
            source_weights: mask_weights(&mask, s_img.width(), s_img.height()),
            target_weights: mask_weights(&mask, t_img.width(), t_img.height()),
            coverage: None,
            mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
            range: Some(RangeMask::Luminance {
                lo_outer: 0.0,
                lo: 0.0,
                hi: 1.0,
                hi_outer: 1.0,
            }),
            name: "Luminance range 01".to_string(),
            role: MaskRole::Custom,
            inverted: false,
            label: "Luminance range 01".to_string(),
            min_share: MIN_ZONE_SHARE,
            frame_regression_tol: RANGE_FRAME_REGRESSION_TOL,
        };

        // Obtain the deterministic candidate frame without making its
        // composed-frame arm binding; both assertions replay this attachment
        // against either side of the zero-tolerance line.
        let mut probe = neutral_report(&source, &target);
        let mut loose_frame_err = f32::MAX;
        let candidate = attach_one_zone(
            &s_img,
            &tgt_px,
            &mut probe,
            &mut loose_frame_err,
            &attachment,
            divergence,
            None,
        )
        .expect("the fixture must earn its band on local evidence");
        let candidate_err =
            fit::look_err_with_evidence(&candidate.rendered, &tgt_px, &probe.evidence);

        let mut regressing = neutral_report(&source, &target);
        let mut better_running_frame = candidate_err - 0.001;
        let dropped = attach_one_zone(
            &s_img,
            &tgt_px,
            &mut regressing,
            &mut better_running_frame,
            &attachment,
            divergence,
            None,
        );
        assert!(dropped.is_none(), "a +0.001 composed-frame regression must drop the band");
        assert!(regressing.recipe.masks.is_empty(), "the dropped band must be removed");
        let note = regressing
            .notes
            .iter()
            .find(|note| {
                note.key == crate::rationale::keys::ZONE_DROPPED
                    || note.key == crate::rationale::keys::ZONE_ATMOSPHERE_DROPPED
            })
            .expect("the dropped band must disclose its measured frame drift");
        assert!(note.args.iter().any(|(key, value)| *key == "drift" && value == "+0.00100"));
        assert!(note.args.iter().any(|(key, value)| *key == "tol" && value == "+0.00000"));

        let mut nonregressing = neutral_report(&source, &target);
        let mut no_worse_running_frame = candidate_err;
        let kept = attach_one_zone(
            &s_img,
            &tgt_px,
            &mut nonregressing,
            &mut no_worse_running_frame,
            &attachment,
            divergence,
            None,
        );
        assert!(kept.is_some(), "a neutral-or-better composed frame must keep the band");
        assert_eq!(nonregressing.recipe.masks.len(), 1);
        path.remove();
    }

    fn assert_semantic_success_does_not_derive_ranges(name: &str) {
        RANGE_DERIVATION_CALLS.with(|calls| calls.set(0));
        let (source, target, sky) = zoned_pair();
        let path = fixture_mask_path(name);
        sky.save(path.path()).unwrap();
        SEGMENT_BOTH_OVERRIDE.with(|value| *value.borrow_mut() = Some((sky.clone(), sky)));
        let seg = SegmentOpts {
            python_bin: "unused-by-segmentation-test-override".into(),
            script: "unused-by-segmentation-test-override".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let report = fit_recipe_zoned_inner(
            &source,
            &target,
            &seg,
            &path,
            &crate::recipe::EditRecipe::default(),
            None,
            ZonedLayerOpts {
                field: false, spatial: false, free_masks: false, refine_masks: false,
            },
        );
        let calls = RANGE_DERIVATION_CALLS.with(std::cell::Cell::get);
        assert_eq!(calls, 0, "semantic success must not even derive range candidates");
        assert!(report.recipe.masks.iter().any(|mask| mask.range.is_none()));
        path.remove();
    }

    #[test]
    fn segmentation_success_does_not_derive_range_bands() {
        assert_semantic_success_does_not_derive_ranges("range-no-work-segmentation-success");
    }

    #[test]
    fn semantic_success_still_does_not_derive_ranges() {
        assert_semantic_success_does_not_derive_ranges("semantic-no-range-derivation");
    }

    #[test]
    fn range_weights_are_unchanged_when_refinement_is_enabled() {
        let mut residuals = [0.0; 17];
        residuals[4] = 0.08;
        residuals[5] = 0.07;
        let (range_source, range_target, evidence) = synthetic_range_case(residuals);
        let before = derive_luminance_bands(&range_source, &range_target, &evidence, &[]);
        assert!(!before.bands.is_empty(), "premise: candidate range weights exist");

        let (source, target) = compact_rank_wave_fixture();
        let seg = SegmentOpts {
            python_bin: "autoshade-test-no-such-python".into(),
            script: "Cargo.toml".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        };
        let path = fixture_mask_path("range-refinement-conservation");
        crate::mask_refine::reset_guided_refine_calls();
        let _ = fit_recipe_zoned_inner(
            &source,
            &target,
            &seg,
            &path,
            &crate::recipe::EditRecipe::default(),
            None,
            ZonedLayerOpts {
                field: false, spatial: false, free_masks: false, refine_masks: true,
            },
        );
        assert_eq!(
            crate::mask_refine::guided_refine_calls(),
            0,
            "luminance ranges must never enter the spatial mask refiner",
        );
        let after = derive_luminance_bands(&range_source, &range_target, &evidence, &[]);
        assert_eq!(
            after.bands,
            before.bands,
            "enabling semantic refinement changed observed-domain range weights",
        );
        path.remove();
    }

    #[test]
    fn range_bands_compose_in_current_render_order() {
        RANGE_FRESH_RENDER_CALLS.with(|calls| calls.set(0));
        let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
            32,
            8,
            image::Rgb([118, 118, 118]),
        ));
        let first = RangeMask::Luminance {
            lo_outer: 0.35,
            lo: 0.40,
            hi: 0.50,
            hi_outer: 0.55,
        };
        let second = RangeMask::Luminance {
            lo_outer: 0.50,
            lo: 0.55,
            hi: 0.70,
            hi_outer: 0.75,
        };
        let mut recipe = crate::recipe::EditRecipe::default();
        recipe.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(first),
            exposure_ev: 0.5,
            ..Default::default()
        });
        recipe.masks.push(LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(second),
            exposure_ev: -0.5,
            ..Default::default()
        });
        let first_only = {
            let mut r = recipe.clone();
            r.masks.pop();
            render::develop_preview(&source, &r).to_rgb8()
        };
        let composed = render::develop_preview(&source, &recipe).to_rgb8();
        assert_ne!(
            composed, first_only,
            "the first band must move pixels into the later band's current-render range"
        );
        let current = fit::pixels_of(&DynamicImage::ImageRgb8(first_only));
        assert!(
            range_weights_for_pixels(&second, &current).iter().any(|&weight| weight > 0.0),
            "later estimator weights must be derivable from the current render"
        );

        let (fixture, _, _ranges) = range_boundary_fixture();
        let target = rank_wave_target(&fixture);
        let mut report = neutral_report(&fixture, &target);
        attach_ranges(&fixture, &target, &mut report, &[]);
        assert!(
            RANGE_FRESH_RENDER_CALLS.with(std::cell::Cell::get) >= 2,
            "the fallback loop must freshly render each candidate's current stack"
        );
    }

    /// The fit base is calibration-only by caller contract (debug-asserted in
    /// `fit_recipe_from_promoted_with_disclosure`), so a recipe that already
    /// carries a `Custom` range reaches the range producer below that line:
    /// the producer must keep it in place and never dispatch on its role.
    #[test]
    fn refit_with_existing_custom_range_never_dispatches_on_mask_role() {
        let (source, _, _) = range_boundary_fixture();
        let target = rank_wave_target(&source);
        let existing = LocalAdjustment {
            mask: RANGE_HOST,
            range: Some(RangeMask::Luminance {
                lo_outer: 0.05,
                lo: 0.10,
                hi: 0.25,
                hi_outer: 0.30,
            }),
            name: "Existing custom range".to_string(),
            exposure_ev: 0.0,
            role: MaskRole::Custom,
            ..Default::default()
        };
        let mut report = neutral_report(&source, &target);
        report.recipe.masks.push(existing.clone());
        attach_ranges(&source, &target, &mut report, &[]);
        assert_eq!(report.recipe.masks.first(), Some(&existing));
        assert!(report.notes.iter().any(|note| note.key == crate::rationale::keys::RANGE_ATTACHED));
    }

    // ---------------------------------------------------------------------
    // The RE-CUT synthetic ramp probe (v1.2.4)
    // ---------------------------------------------------------------------

    /// Height of the probe frame, in rows, chosen so the SUPERVISOR'S OWN
    /// mask-free instrument fits on it unchanged.
    ///
    /// `scripts/rim_overshoot.py` needs 180 px of margin on each side of the
    /// boundary it locates (60 px of transition + 60 px of gap + a 60 px
    /// plateau window) and its 121-row transition window must COVER the
    /// transition. On a frame whose luma runs 0 to 1 over `PROBE_ROWS` rows
    /// those two demands pull opposite ways: a taller frame buys margin and
    /// spends it again by stretching the transition. `RANGE_MAX_RAMP` (2/17 of
    /// luma) lands on 120 rows here, just inside the 121-row window, and
    /// 180 rows is 0.176 of luma, which is what fixes the position sweep to
    /// [0.36, 0.64]. v1.2.3's probe was 512 rows: its 2/17 transition was only
    /// 60 rows wide but every window that had to bracket it ran off the top of
    /// the frame, so that instrument returned n = 0 and the widest ramp this
    /// producer emits went unmeasured rather than measured clean.
    const PROBE_ROWS: u32 = 1020;
    /// Every column of a vertical ramp is the same column; the instrument
    /// reads one number per column, so 64 of them is 64 independent readings
    /// of a deterministic frame.
    const PROBE_COLS: u32 = 64;
    /// The band's own width in luma, wide enough that the plateau window on
    /// the inside of the lo transition (60 px of gap plus 60 px of window,
    /// starting 60 px past the crossing) still lands inside the band.
    const PROBE_BAND: f32 = 4.0 / 17.0;

    fn probe_frame() -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(PROBE_COLS, PROBE_ROWS, |_, y| {
            let value = (y as f32 / (PROBE_ROWS - 1) as f32 * 255.0).round() as u8;
            image::Rgb([value; 3])
        }))
    }

    fn probe_range(position: f32, ramp: f32) -> RangeMask {
        let (lo, hi) = (position - PROBE_BAND * 0.5, position + PROBE_BAND * 0.5);
        RangeMask::Luminance {
            lo_outer: (lo - ramp).max(0.0),
            lo,
            hi,
            hi_outer: (hi + ramp).min(1.0),
        }
    }

    fn probe_recipe(range: RangeMask, ev: f32) -> crate::recipe::EditRecipe {
        crate::recipe::EditRecipe {
            masks: vec![LocalAdjustment {
                mask: RANGE_HOST,
                range: Some(range),
                name: "probe band".to_string(),
                exposure_ev: ev,
                role: MaskRole::Custom,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// `scripts/rim_overshoot.py`, ported: same 60/60/60 geometry, the same
    /// per-column first-0.5-crossing locator, the same median plateaus, the
    /// same zero-padded 3-tap smoothing of the transition window, and the same
    /// "distance outside the interval the two plateaus bound" reading. A
    /// monotone transition of any steepness scores 0 and an identical pair of
    /// renders scores exactly 0.0000, which is the control this table quotes.
    fn mask_free_overshoot(
        zoned: &[[f32; 3]],
        twin: &[[f32; 3]],
        weights: &[f32],
        width: usize,
        height: usize,
    ) -> (usize, f32, f32) {
        const HALF: i32 = 60;
        const PLATEAU_GAP: i32 = 60;
        const PLATEAU_WIDTH: i32 = 60;
        let median = |values: &mut Vec<f32>| -> f32 {
            values.sort_by(f32::total_cmp);
            let n = values.len();
            if n == 0 {
                0.0
            } else if n % 2 == 1 {
                values[n / 2]
            } else {
                (values[n / 2 - 1] + values[n / 2]) * 0.5
            }
        };
        let mut readings = Vec::new();
        for x in 0..width {
            let column = |y: usize| display_luma(&zoned[y * width + x])
                - display_luma(&twin[y * width + x]);
            let mut centre = None;
            for y in 0..height - 1 {
                let (a, b) = (weights[y * width + x], weights[(y + 1) * width + x]);
                if (a - 0.5) * (b - 0.5) <= 0.0 && a != b {
                    centre = Some(y as i32);
                    break;
                }
            }
            let Some(centre) = centre else { continue };
            let sky0 = centre - HALF - PLATEAU_GAP - PLATEAU_WIDTH;
            let sky1 = centre - HALF - PLATEAU_GAP;
            let land0 = centre + HALF + PLATEAU_GAP;
            let land1 = centre + HALF + PLATEAU_GAP + PLATEAU_WIDTH;
            if sky0 < 0 || land1 > height as i32 {
                continue;
            }
            let mut sky = (sky0..sky1).map(|y| column(y as usize)).collect::<Vec<_>>();
            let mut land = (land0..land1).map(|y| column(y as usize)).collect::<Vec<_>>();
            let (sky, land) = (median(&mut sky), median(&mut land));
            let (low, high) = (sky.min(land), sky.max(land));
            let first = (centre - HALF).max(0);
            let last = (centre + HALF).min(height as i32 - 1);
            let raw = (first..=last).map(|y| column(y as usize)).collect::<Vec<_>>();
            let smoothed = (0..raw.len())
                .map(|i| {
                    let left = if i == 0 { 0.0 } else { raw[i - 1] };
                    let right = if i + 1 == raw.len() { 0.0 } else { raw[i + 1] };
                    (left + raw[i] + right) / 3.0
                })
                .collect::<Vec<_>>();
            readings.push(
                smoothed
                    .iter()
                    .map(|value| (value - high).max(low - value).max(0.0))
                    .fold(0.0f32, f32::max),
            );
        }
        let n = readings.len();
        let mean = if n == 0 { 0.0 } else { readings.iter().sum::<f32>() / n as f32 };
        let max = readings.iter().copied().fold(0.0f32, f32::max);
        (n, mean, max)
    }

    struct ProbeCell {
        n: usize,
        overshoot_mean: f32,
        overshoot_max: f32,
        reversal: f32,
    }

    fn probe_cell(
        frame: &DynamicImage,
        twin: &[[f32; 3]],
        position: f32,
        ramp: f32,
        ev: f32,
    ) -> ProbeCell {
        let range = probe_range(position, ramp);
        let zoned = fit::pixels_of(&render::develop_preview(frame, &probe_recipe(range, ev)));
        let weights = twin.iter().map(|p| render::range_weight(&range, p)).collect::<Vec<_>>();
        let (n, overshoot_mean, overshoot_max) = mask_free_overshoot(
            &zoned,
            twin,
            &weights,
            PROBE_COLS as usize,
            PROBE_ROWS as usize,
        );
        ProbeCell {
            n,
            overshoot_mean,
            overshoot_max,
            reversal: range_transfer_reversal(twin, &zoned, &[range]),
        }
    }

    /// THE RE-CUT PROBE. Two claims are pinned here and nowhere else.
    ///
    /// 1. `RANGE_MAX_RAMP` is MEASURABLE. v1.2.3 could not read it: every
    ///    column of its 512-row probe was rejected by the mask-free
    ///    instrument's margin test, so the widest ramp the producer emits was
    ///    absent from the table on purpose. On this frame every cell returns
    ///    n = 64 of 64 columns.
    /// 2. The luminance POSITION of the band is swept, which v1.2.3's probe
    ///    never did — it pinned the band at the calibration position and said
    ///    so.
    ///
    /// The numbers this prints are transcribed into `RANGE_MAX_RAMP`'s own
    /// rustdoc; the assertions below are the pins.
    ///
    /// MUTATION: `RANGE_MAX_RAMP` 2/17 -> 3/17 makes the widest ramp 180 rows
    /// wide, the instrument's 121-row window can no longer bracket it, and the
    /// n = 64 assertion fails.
    #[test]
    fn the_recut_ramp_probe_measures_every_ramp_the_producer_emits() {
        let frame = probe_frame();
        let twin = fit::pixels_of(&render::develop_preview(
            &frame,
            &crate::recipe::EditRecipe::default(),
        ));
        let positions = [0.36f32, 0.43, 0.50, 0.57, 0.64];
        let ramps = [
            ("1/17", RANGE_MIN_RAMP),
            ("1.5/17", 1.5 / 17.0),
            ("2/17 (RANGE_MAX_RAMP)", RANGE_MAX_RAMP),
        ];
        let evs = [0.0f32, -0.35, -0.56, -0.80, -1.10, -1.50];
        for (label, ramp) in ramps {
            for ev in evs {
                for position in positions {
                    let cell = probe_cell(&frame, &twin, position, ramp, ev);
                    eprintln!(
                        "RAMP_PROBE ramp={label} ev={ev:+.2} position={position:.2} \
                         n={} overshoot_mean={:.4} overshoot_max={:.4} reversal={:.6}",
                        cell.n, cell.overshoot_mean, cell.overshoot_max, cell.reversal,
                    );
                    assert_eq!(
                        cell.n, PROBE_COLS as usize,
                        "the re-cut probe must be MEASURABLE at ramp {label}, \
                         position {position}: the instrument rejected columns",
                    );
                    if ev == 0.0 {
                        // The control: the same frame twice. Both instruments
                        // must read exactly zero, or neither number below means
                        // anything.
                        assert_eq!(cell.overshoot_max, 0.0, "control overshoot must be exact 0");
                        assert_eq!(cell.reversal, 0.0, "control reversal must be exact 0");
                    }
                }
            }
        }
    }

    /// A3. The sign test refuses what the magnitude ruler cannot see.
    ///
    /// The 1.5/17 band at -0.56 EV is v1.2.3's own counter-example: it inverts
    /// the delivered tone order while `rim_overshoot.py` reads 0.0000 over its
    /// whole n, because the DIFFERENCE that ruler ranks stays monotone. The
    /// probe reproduces the inversion, and the gate now shrinks it away.
    ///
    /// MUTATION: drop `reversal <= RANGE_TRANSFER_REVERSAL_MAX` from the gate's
    /// `passes` predicate and the k assertion below fails (the band is kept
    /// whole at k = 1 with the inversion delivered).
    #[test]
    fn the_tone_reversal_gate_shrinks_a_band_the_rim_ruler_cannot_see() {
        let frame = probe_frame();
        let twin = fit::pixels_of(&render::develop_preview(
            &frame,
            &crate::recipe::EditRecipe::default(),
        ));
        let range = probe_range(0.50, 1.5 / 17.0);
        let mut worst = 0.0f32;
        for ev in [-0.56f32, -0.80, -1.10, -1.50] {
            let cell = probe_cell(&frame, &twin, 0.50, 1.5 / 17.0, ev);
            eprintln!(
                "SIGN_TEST ev={ev:+.2} overshoot_max={:.4} reversal={:.6}",
                cell.overshoot_max, cell.reversal,
            );
            worst = worst.max(cell.reversal);
        }
        assert!(
            worst > RANGE_TRANSFER_REVERSAL_MAX,
            "premise: this probe must invert the tone order somewhere, worst {worst}",
        );

        // Now through the gate itself, on the configuration that inverts.
        let mut report = neutral_report(&frame, &frame);
        report.recipe = probe_recipe(range, -1.50);
        let initial_px =
            fit::pixels_of(&render::develop_preview(&frame, &report.recipe));
        let before = range_transfer_reversal(&twin, &initial_px, &[range]);
        assert!(before > RANGE_TRANSFER_REVERSAL_MAX, "premise: {before}");
        let result = enforce_range_boundary_gate(
            &frame,
            &mut report,
            &twin,
            &[range],
            &[1.0],
            0,
            initial_px,
        );
        let BoundaryGateResult::Kept { k, pixels, .. } = result else {
            panic!("the gate must shrink this band, not drop it")
        };
        let after = range_transfer_reversal(&twin, &pixels, &[range]);
        eprintln!("SIGN_TEST_GATE k={k:.4} reversal {before:.6} -> {after:.6}");
        assert!(k < 1.0, "an inverting band must not be kept whole: k={k}");
        assert!(
            after <= RANGE_TRANSFER_REVERSAL_MAX,
            "the shrink must land inside the budget: {after}",
        );
        assert!(
            report.notes.iter().any(|note| {
                note.key == crate::rationale::keys::RANGE_BOUNDARY_PASSED
                    && note.args.iter().any(|(key, _)| *key == "reversal")
            }),
            "the gate must disclose the reading it acted on: {}",
            report.recipe.rationale,
        );
    }

    /// The statistic itself, on hand-built transfers rather than on a render:
    /// a compressive monotone transfer reads exactly 0 no matter how flat it
    /// gets, and an inversion reads its own depth.
    #[test]
    fn the_transfer_reversal_reads_depth_and_ignores_compression() {
        let range = RangeMask::Luminance { lo_outer: 0.0, lo: 0.0, hi: 1.0, hi_outer: 1.0 };
        let build = |map: &dyn Fn(f32) -> f32| -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
            let reference =
                (0..256).map(|i| [i as f32 / 255.0; 3]).collect::<Vec<_>>();
            let rendered = reference
                .iter()
                .map(|p| [map(p[0]).clamp(0.0, 1.0); 3])
                .collect::<Vec<_>>();
            (reference, rendered)
        };
        let (reference, flat) = build(&|x| 0.5 + 0.02 * x);
        assert_eq!(
            range_transfer_reversal(&reference, &flat, &[range]),
            0.0,
            "a 50x compression is still monotone",
        );
        // An 8-code step down, sampled one input code past the step: the
        // highest delivered luma before the drop is the value at x = 0.5 and
        // the first value after it is (0.5 + 1/255) - 8/255, so the depth this
        // frame CARRIES is seven codes, not eight. The reading is the depth of
        // the excursion, which is the quantity a viewer sees, and not the size
        // of the step that caused it.
        let (reference, inverted) = build(&|x| if x > 0.5 { x - 8.0 / 255.0 } else { x });
        let depth = range_transfer_reversal(&reference, &inverted, &[range]);
        assert!(
            (depth * 255.0 - 7.0).abs() < 0.51,
            "an 8-code step down one code past the peak must read 7 codes of \
             depth, read {}",
            depth * 255.0,
        );
    }

    /// A frame whose colour lives in two scattered populations of DIFFERENT
    /// ACR bands, only one of which the target moves.
    ///
    /// The moved population is the case this producer exists for: one hue,
    /// everywhere it appears, in 4-pixel blocks no rectangle and no silhouette
    /// can enclose. The UNMOVED population is why a global answer will not do
    /// -- anything applied to the whole frame that fixes the blues moves the
    /// oranges that already match.
    ///
    /// Measured on this fixture's own 384x384 analysis frame, with the mask
    /// the producer derives for the blue band (radius 0.675, and every unit of
    /// its weight on a member of that band on BOTH frames):
    ///
    /// ```text
    /// target blue        mask covers src / tgt   ratio   band residual
    /// [0.24 0.34 0.72]      18.2% / 21.1%        1.16       0.068
    /// [0.18 0.29 0.78]      18.2% /  9.7%        1.88       0.052
    /// [0.16 0.28 0.80]      18.2% /  9.6%        1.89       0.093
    /// [0.14 0.26 0.83]      18.2% / 10.1%        1.79       0.148
    /// [0.10 0.22 0.88]      18.2% /  8.5%        2.14       0.240
    /// ```
    ///
    /// The first row is the case this producer is for — the look pulls the
    /// blues back and the two populations stay comparable. The last is past
    /// the shared 2:1 composition gate: that move carried the target's pixels
    /// out of the mask, so there is nothing left to fit them against.
    fn scattered_hue_pair(target_blue: [f32; 3]) -> (DynamicImage, DynamicImage) {
        const EDGE: u32 = 64;
        let build = |blue: [f32; 3]| {
            DynamicImage::ImageRgb8(RgbImage::from_fn(EDGE, EDGE, |x, y| {
                let pixel = match ((x / 4) * 7 + (y / 4) * 13) % 5 {
                    0 => blue,
                    1 => [0.80f32, 0.45, 0.15],
                    _ => [0.50f32, 0.50, 0.50],
                };
                image::Rgb(pixel.map(|c| (c * 255.0).round() as u8))
            }))
        };
        (build([0.20, 0.30, 0.75]), build(target_blue))
    }

    /// ACR band 5. `fit::evidence_hue_band` puts both the source blue
    /// ([0.20 0.30 0.75], hue 229 degrees) and the target blue here.
    const BLUE_BAND: usize = 5;

    /// A target blue the look pulled BACK: the two populations stay comparable
    /// through the mask, so the band is measurable (see the fixture's table).
    const MEASURABLE_BLUE: [f32; 3] = [0.24, 0.34, 0.72];
    /// A target blue the look pushed past the mask: 18.2% of the frame against
    /// 8.5%, which the shared composition gate refuses.
    const UNREACHABLE_BLUE: [f32; 3] = [0.10, 0.22, 0.88];

    /// A luminance range over the whole 0..=1 domain: the tone ruler asked
    /// about every pixel there is, so it cannot be said to have missed
    /// something for want of a place to look.
    fn full_luma_range() -> RangeMask {
        RangeMask::Luminance { lo_outer: 0.0, lo: 0.0, hi: 1.0, hi_outer: 1.0 }
    }

    fn colour_masks(recipe: &crate::recipe::EditRecipe) -> usize {
        recipe
            .masks
            .iter()
            .filter(|mask| matches!(mask.range, Some(RangeMask::Color { .. })))
            .count()
    }

    fn range_only_layers() -> ZonedLayerOpts {
        ZonedLayerOpts { field: false, spatial: false, free_masks: false, refine_masks: false }
    }

    fn segmentation_off() -> SegmentOpts {
        SegmentOpts {
            python_bin: "autoshade-test-no-such-python".into(),
            script: "Cargo.toml".into(),
            target: "sky".into(),
            reference_point: None,
            prompt_points: None,
        }
    }

    #[test]
    fn a_scattered_hue_band_becomes_a_native_colour_range_correction() {
        let (source, target) = scattered_hue_pair(MEASURABLE_BLUE);
        let mut report = neutral_report(&source, &target);
        let before = report.err_after;
        attach_ranges(&source, &target, &mut report, &[]);
        assert_eq!(colour_masks(&report.recipe), 1, "{}", report.recipe.rationale);
        assert!(
            report.err_after < before,
            "the colour band did not pay for itself: {before} -> {}\n{}",
            report.err_after,
            report.recipe.rationale,
        );
        let key_present = |key: &str| report.notes.iter().any(|note| note.key == key);
        assert!(key_present(crate::rationale::keys::COLOUR_RANGE_PROPOSED));
        assert!(key_present(crate::rationale::keys::COLOUR_RANGE_ATTACHED));
        // Native in the sidecar: the sentinel host and the colour range are
        // both classic ACR grammar, so nothing is lost on the way out.
        let xmp = crate::xmp::recipe_to_xmp(&report.recipe);
        assert!(!xmp.contains("Mask/Bitmap"), "a colour band must not need a raster");
        assert!(xmp.contains("crs:ColorAmount"));
        assert_eq!(colour_masks(&crate::xmp::xmp_to_recipe(&xmp)), 1);
    }

    #[test]
    fn a_colour_move_that_leaves_the_mask_is_refused_not_fitted() {
        // The FOURTH circularity guard, and the one that needs no evidence
        // model at all: the mask is the ruler for both frames, so a move big
        // enough to carry the target's pixels out of it leaves the two shares
        // past 2:1 and the shared composition gate refuses the band. Fitting it
        // anyway would be solving a correction against a population that is no
        // longer there.
        let (source, target) = scattered_hue_pair(UNREACHABLE_BLUE);
        let mut report = neutral_report(&source, &target);
        attach_ranges(&source, &target, &mut report, &[]);
        assert_eq!(colour_masks(&report.recipe), 0, "{}", report.recipe.rationale);
        assert!(
            report.notes.iter().any(|note| {
                note.key == crate::rationale::keys::ZONE_SHARE_MISMATCH
            }),
            "the refusal must name the composition, not go unexplained: {}",
            report.recipe.rationale,
        );
        assert!(
            report.notes.iter().any(|note| {
                note.key == crate::rationale::keys::COLOUR_RANGE_ABSTAINED
            }),
            "{}",
            report.recipe.rationale,
        );
    }

    #[test]
    fn a_global_colour_move_leaves_no_colour_range_behind() {
        // The target IS the source under ONE global saturation move, so no
        // colour band may survive: every pixel of every band moved by the
        // same rule, and a band that claimed the move would be claiming a
        // population it does not own.
        //
        // Measured on this fixture, the refusal is not the quiet one. The
        // global model cannot reproduce this move on these synthetic
        // colours — it returns a neutral recipe under its own do-no-harm —
        // so the residual is still on the frame when the producer runs and
        // two bands ARE proposed. Neither survives: the Orange band's own
        // populations are 22% against 11%, which is the shared 2:1
        // composition gate, and the Blue band's correction costs the
        // composed frame +0.00198 against a tolerance of exactly zero.
        let (source, _) = scattered_hue_pair([0.20, 0.30, 0.75]);
        let global = crate::recipe::EditRecipe { saturation: 22.0, ..Default::default() };
        let target = render::develop_preview(&source, &global);
        let path = fixture_mask_path("colour-range-global-move");
        let report = fit_recipe_zoned_inner(
            &source,
            &target,
            &segmentation_off(),
            &path,
            &crate::recipe::EditRecipe::default(),
            None,
            range_only_layers(),
        );
        assert_eq!(colour_masks(&report.recipe), 0, "{}", report.recipe.rationale);
        // AND EVERY PROPOSAL IS ANSWERED BY NAME. A band that vanished
        // between the proposal and the recipe without a refusal naming it
        // would be a correction the fit dropped in silence, which is the
        // one outcome the disclosure channels exist to prevent.
        let bands_named = |key: &'static str| {
            report
                .notes
                .iter()
                .filter(|note| note.key == key)
                .filter_map(|note| {
                    note.args
                        .iter()
                        .find(|(name, _)| *name == "band")
                        .map(|(_, value)| value.clone())
                })
                .collect::<std::collections::BTreeSet<_>>()
        };
        let proposed = bands_named(crate::rationale::keys::COLOUR_RANGE_PROPOSED);
        let refused = bands_named(crate::rationale::keys::COLOUR_RANGE_ABSTAINED);
        assert!(
            !proposed.is_empty() && proposed.is_subset(&refused),
            "proposed {proposed:?} but refused only {refused:?}\n{}",
            report.recipe.rationale,
        );
        path.remove();
    }

    #[test]
    fn a_one_sided_hue_band_abstains_instead_of_arguing_with_itself() {
        let (source, target) = scattered_hue_pair(MEASURABLE_BLUE);
        let (s_img, t_img) = fit::analysis_pair(&source, &target);
        let (sp, tp) = (fit::pixels_of(&s_img), fit::pixels_of(&t_img));
        let (w, h) = (s_img.width(), s_img.height());
        let evidence = fit::evidence_model_for(&sp, &tp, w, h);
        let two_sided = derive_colour_bands(&sp, &tp, &evidence, w, h);
        assert!(
            two_sided.proposals.iter().any(|p| p.hue_band == BLUE_BAND),
            "premise: the moved band is proposable while both sides hold it: {two_sided:?}",
        );

        // ARM ONE, the frozen verdict. A hue-shifting edit empties the very
        // band it moved, which is what makes "the target has no blues, so
        // correct the blues" circular. The producer must refuse to be the
        // second half of that sentence.
        let mut one_sided_evidence = evidence.clone();
        one_sided_evidence.hue[BLUE_BAND].target_populated = false;
        one_sided_evidence.hue[BLUE_BAND].target_share = 0.0;
        one_sided_evidence.hue[BLUE_BAND].target_evidence_share = 0.0;
        one_sided_evidence.hue[BLUE_BAND].two_sided_share = 0.0;
        one_sided_evidence.hue[BLUE_BAND].weight = 0.0;
        let one_sided = derive_colour_bands(&sp, &tp, &one_sided_evidence, w, h);
        assert!(!one_sided.proposals.iter().any(|p| p.hue_band == BLUE_BAND));
        assert!(
            one_sided.abstentions.iter().any(|a| {
                a.band == BLUE_BAND && a.reason.contains("target population")
            }),
            "{one_sided:?}",
        );

        // ARM TWO, the delivered populations. The frozen verdict is held OPEN
        // here on purpose: the frames are the ones the mask would be applied
        // to, and a target the edit emptied has to refuse on its own reading
        // rather than relying on the frozen model to have noticed.
        let (_, emptied) = scattered_hue_pair([0.62, 0.60, 0.58]);
        let (_, e_img) = fit::analysis_pair(&source, &emptied);
        let delivered = derive_colour_bands(&sp, &fit::pixels_of(&e_img), &evidence, w, h);
        assert!(!delivered.proposals.iter().any(|p| p.hue_band == BLUE_BAND));
        assert!(
            delivered.abstentions.iter().any(|a| {
                a.band == BLUE_BAND && a.reason.contains("two-sided floor")
            }),
            "{delivered:?}",
        );
    }

    /// A frame that sweeps from a saturated colour to neutral at CONSTANT
    /// display luma. Rec.601 luma is linear in the channels, so interpolating
    /// between two colours of equal luma holds the luma exactly: the luma
    /// ruler is blind across the whole frame by construction, and anything a
    /// reading picks up here it picked up in chromaticity.
    fn equal_luma_chromaticity_ramp() -> (DynamicImage, [f32; 3]) {
        const W: u32 = 80;
        const H: u32 = 8;
        let saturated = [0.30f32, 0.50, 0.70];
        let luma = display_luma(&saturated);
        let image = DynamicImage::ImageRgb8(RgbImage::from_fn(W, H, |x, _| {
            let t = x as f32 / (W - 1) as f32;
            let pixel = saturated.map(|c| c + t * (luma - c));
            image::Rgb(pixel.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
        }));
        (image, saturated)
    }

    #[test]
    fn the_colour_rim_reads_a_step_the_luma_ruler_is_blind_to() {
        let (frame, keyed) = equal_luma_chromaticity_ramp();
        let colour = RangeMask::Color {
            r: keyed[0],
            g: keyed[1],
            b: keyed[2],
            amount: 0.0,
            px: 0.0,
            py: 0.5,
        };
        // One luminance transition spanning the whole frame, so the luma
        // ruler is asked about exactly the same pixels and cannot be said to
        // have missed the step for want of a place to look.
        let luma_probe = RangeMask::Luminance { lo_outer: 0.0, lo: 0.9, hi: 0.95, hi_outer: 1.0 };
        let recipe = crate::recipe::EditRecipe {
            masks: vec![LocalAdjustment {
                mask: RANGE_HOST,
                range: Some(colour),
                color_gains: Some([1.60, 1.00, 0.35]),
                ..Default::default()
            }],
            ..Default::default()
        };
        let reference = fit::pixels_of(&render::develop_preview(
            &frame,
            &crate::recipe::EditRecipe::default(),
        ));
        let rendered = fit::pixels_of(&render::develop_preview(&frame, &recipe));
        let (w, h) = (frame.width(), frame.height());
        let seen = range_transition_rim(&reference, &rendered, &[colour], w, h);
        let blind = range_transition_rim(&reference, &rendered, &[luma_probe], w, h);
        assert!(seen.transitions > 0 && blind.transitions > 0, "{seen:?} {blind:?}");
        assert!(
            seen.rim > RANGE_BOUNDARY_RIM_MAX,
            "the colour band's own edge must be visible to its own ruler: {seen:?}",
        );
        assert!(
            blind.rim < RANGE_BOUNDARY_RIM_MAX,
            "the luma ruler is supposed to be blind here: {blind:?}",
        );
    }

    #[test]
    fn the_order_test_is_not_asked_of_a_radial_coordinate() {
        let keyed = [0.30f32, 0.50, 0.70];
        let range = RangeMask::Color {
            r: keyed[0],
            g: keyed[1],
            b: keyed[2],
            amount: 1.0,
            px: 0.5,
            py: 0.5,
        };
        let luma = display_luma(&keyed);
        // A sweep from the keyed colour to neutral at constant luma:
        // chromaticity distance from the key rises monotonically along it, and
        // the tone ruler is blind to the whole frame by construction.
        let reference = (0..256)
            .map(|i| {
                let t = i as f32 / 255.0;
                keyed.map(|c| c + t * (luma - c))
            })
            .collect::<Vec<_>>();
        // THE MOVE A COLOUR BAND EXISTS TO MAKE: carry the whole population
        // toward a different colour, rigidly. Every neighbour keeps the
        // difference it had, so nothing steps and nothing folds — but every
        // member's RADIUS from the band's own colour changes, and the member
        // that sat exactly on that colour is now half the sweep away from it.
        let shifted = reference
            .iter()
            .map(|p| {
                [
                    p[0] - 0.5 * (luma - keyed[0]),
                    p[1] - 0.5 * (luma - keyed[1]),
                    p[2] - 0.5 * (luma - keyed[2]),
                ]
            })
            .collect::<Vec<_>>();
        let selector = RangeSelector::Chromaticity(keyed);
        assert!(
            selector.position(&reference[0]) < selector.position(&reference[128])
                && selector.position(&shifted[0]) > selector.position(&shifted[128]),
            "premise: a rigid colour shift inverts the RADIAL order it is read \
             on, {} -> {} against {} -> {}",
            selector.position(&reference[0]),
            selector.position(&reference[128]),
            selector.position(&shifted[0]),
            selector.position(&shifted[128]),
        );
        // The ruler a colour band IS judged by reads this frame correctly:
        // neighbours the photograph gave the same colour still have it.
        let rim = range_transition_rim(&reference, &shifted, &[range], 256, 1);
        assert!(
            rim.transitions > 0 && rim.rim <= RANGE_BOUNDARY_RIM_MAX,
            "a rigid shift steps nothing: {rim:?}",
        );
        // So the order reading is not asked of this coordinate at all. Were it
        // asked, it would refuse the correction above for succeeding.
        assert_eq!(range_transfer_reversal(&reference, &shifted, &[range]), 0.0);
        // And the luminance coordinate's own order test is untouched by the
        // same frames: nothing moved in luma, so it reads exactly zero.
        assert_eq!(
            range_transfer_reversal(&reference, &shifted, &[full_luma_range()]),
            0.0,
        );
    }

    /// The luminance readings this batch measured on the fixtures that
    /// already existed, before and after the selector split: crossings and
    /// p90 rim on two fixtures, and the reversal depth of an eight-code step.
    const PINNED_OPPOSING: (usize, f32, f32) = (4620, 0.015686244, 0.015686244);
    const PINNED_MIXED: (usize, f32) = (880, 0.007843144);
    const PINNED_STEP_REVERSAL: f32 = 0.027450979;

    #[test]
    fn the_luminance_boundary_readings_are_unchanged_by_the_selector_split() {
        // The two readings a luminance band is judged on, on the fixtures
        // that already existed, pinned to the values they carried before the
        // rim and reversal gates learned a second coordinate. A luminance
        // band's arithmetic is the same arithmetic; only its spelling moved.
        let (source, reference, ranges) = range_boundary_fixture();
        let rendered = fit::pixels_of(&render::develop_preview(
            &source,
            &opposing_range_recipe(&ranges),
        ));
        let reading = range_transition_rim(
            &reference,
            &rendered,
            &ranges,
            source.width(),
            source.height(),
        );
        let (mixed, wide) = mixed_gradient_fixture();
        let untouched = range_transition_rim(&mixed, &mixed, &wide, 80, 8);
        let ramp = (0..256).map(|i| [i as f32 / 255.0; 3]).collect::<Vec<_>>();
        let stepped = ramp
            .iter()
            .map(|p| [if p[0] > 0.5 { p[0] - 8.0 / 255.0 } else { p[0] }; 3])
            .collect::<Vec<_>>();
        // One assertion, so one run reports every pin that moved.
        assert_eq!(
            (
                (reading.transitions, reading.rim, reading.charged),
                (untouched.transitions, untouched.rim),
                range_transfer_reversal(&ramp, &stepped, &[full_luma_range()]),
            ),
            (PINNED_OPPOSING, PINNED_MIXED, PINNED_STEP_REVERSAL),
        );
    }

    #[test]
    fn colour_range_round_trips_through_the_lightroom_sidecar() {
        // Values that survive the sidecar's six decimals exactly, so the
        // round trip is an equality and not a tolerance.
        let range = RangeMask::Color {
            r: 0.25,
            g: 0.5,
            b: 0.75,
            amount: 0.375,
            px: 0.125,
            py: 0.625,
        };
        let recipe = crate::recipe::EditRecipe {
            masks: vec![LocalAdjustment {
                mask: RANGE_HOST,
                range: Some(range),
                name: "Colour range 01".to_string(),
                role: MaskRole::Custom,
                saturation: -12.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let xmp = crate::xmp::recipe_to_xmp(&recipe);
        assert_eq!(xmp.matches("Mask/RangeMask").count(), 1);
        assert!(!xmp.contains("Mask/Bitmap"));
        // The element Lightroom itself writes for a colour range: a
        // CorrectionRangeMask carrying Type 1, a ColorAmount and one
        // PointModels entry of "r g b px py 0".
        assert!(xmp.contains("crs:Type=\"1\""));
        assert!(xmp.contains("crs:ColorAmount=\"0.375000\""));
        assert!(xmp.contains(
            "<rdf:li>0.250000 0.500000 0.750000 0.125000 0.625000 0</rdf:li>",
        ));
        let round_trip = crate::xmp::xmp_to_recipe(&xmp);
        assert_eq!(round_trip.masks.len(), 1);
        assert_eq!(round_trip.masks[0].mask, RANGE_HOST);
        assert_eq!(round_trip.masks[0].range, Some(range));
    }

    /// The corpus arm: the colour family may not cost any real pair anything.
    ///
    /// Both arms run in this process, so the difference between them is this
    /// feature and nothing else. A pair where no colour band attaches must be
    /// byte-identical, and a pair where one does must be no worse on the
    /// composed frame -- the ceiling the range family's own frame gate
    /// enforces, checked here on the pairs rather than trusted.
    #[test]
    fn calibration_colour_ranges_never_cost_a_corpus_pair() {
        let Some(root) = fit::calibration_corpus() else { return };
        let mut pairs = vec![(
            "neutral".to_string(),
            image::open(root.join("neutral.jpg")).unwrap(),
            image::open(root.join("target.jpg")).unwrap(),
        )];
        for code in ["p36", "p37", "p38", "p39"] {
            let raw = root.join(format!("{code}.arw"));
            let target = root.join(format!("{code}-target.jpg"));
            if !raw.exists() || !target.exists() {
                continue;
            }
            pairs.push((
                code.to_string(),
                crate::decode::preview_only(&raw).unwrap(),
                image::open(target).unwrap(),
            ));
        }
        for (code, source, target) in pairs {
            let run = |suppressed: bool, tag: &str| {
                COLOUR_RANGE_SUPPRESSED.with(|cell| cell.set(suppressed));
                let path = fixture_mask_path(&format!("colour-corpus-{code}-{tag}"));
                let report = fit_recipe_zoned_inner(
                    &source,
                    &target,
                    &segmentation_off(),
                    &path,
                    &crate::recipe::EditRecipe::default(),
                    None,
                    range_only_layers(),
                );
                COLOUR_RANGE_SUPPRESSED.with(|cell| cell.set(false));
                path.remove();
                report
            };
            let without = run(true, "without");
            let with = run(false, "with");
            let bands = colour_masks(&with.recipe);
            assert!(
                with.err_after <= without.err_after + 1e-6,
                "{code}: {bands} colour band(s) cost the frame {} -> {}\n{}",
                without.err_after,
                with.err_after,
                with.recipe.rationale,
            );
            if bands == 0 {
                // The rationale is EXPECTED to differ: a proposal and its
                // refusal are disclosures, and making them is the point. What
                // may not differ is anything that renders or exports.
                assert_eq!(
                    serde_json::to_string(&with.recipe.masks).unwrap(),
                    serde_json::to_string(&without.recipe.masks).unwrap(),
                    "{code}: no colour band attached, so the correction stack \
                     must be byte-identical",
                );
                assert_eq!(
                    (with.err_after, with.recipe.confidence),
                    (without.err_after, without.recipe.confidence),
                    "{code}: a refused colour band moved the frame or the claim",
                );
            }
            let attached = with
                .notes
                .iter()
                .filter(|note| note.key == crate::rationale::keys::COLOUR_RANGE_ATTACHED)
                .map(|note| {
                    let arg = |name: &str| {
                        note.args
                            .iter()
                            .find(|(key, _)| *key == name)
                            .map(|(_, value)| value.clone())
                            .unwrap_or_default()
                    };
                    format!(
                        "[{} {} -> {}, ev {}, gains {} {} {}, sat {}]",
                        arg("label"), arg("before"), arg("after"), arg("ev"),
                        arg("g0"), arg("g1"), arg("g2"), arg("sat"),
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "COLOUR_RANGE_CORPUS {code}: bands {bands}, frame {:.8} -> {:.8} {attached}",
                without.err_after, with.err_after,
            );
        }
    }
}
