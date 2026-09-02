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
/// ([`RANGE_TRANSFER_REVERSAL_MAX`]) with a reading of 0, and the gate
/// carries that reading beside this one.
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
/// * Both real pairs read 0: the calibration band shows 0 reversals over its 30
///   populated 1-code bins and the viaduct 0 over 19 (v1.2.3's table).
/// * The probe's smallest REAL inversion is 1 code = 0.0039 luma (a 1/17 band
///   at -0.35 EV, v1.2.3's table), and this batch's own sweep reproduces it.
///
/// Half a code is therefore the line: it is above a floor of exactly zero and
/// below the smallest inversion any configuration has produced, and half an
/// 8-bit code is under the quantisation of the render this gate measures, so
/// nothing it admits can be drawn.
const RANGE_TRANSFER_REVERSAL_MAX: f32 = 0.5 / 255.0;
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
}

fn display_luma(p: &[f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

fn range_weights_for_pixels(range: &RangeMask, pixels: &[[f32; 3]]) -> Vec<f32> {
    pixels.iter().map(|p| render::range_weight(range, p)).collect()
}

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
    let transitions = ranges
        .iter()
        .flat_map(|range| match *range {
            RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => {
                [(lo_outer, lo), (hi, hi_outer)]
            }
            RangeMask::Color { .. } => [(f32::NAN, f32::NAN); 2],
        })
        .filter(|(a, b)| a.is_finite() && b.is_finite() && b > a)
        .collect::<Vec<_>>();
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
        let (la, lb) = (display_luma(ra), display_luma(rb));
        // A range rim is a bow in a locally smooth value crossing, not a
        // pre-existing subject edge. Two-and-a-half 8-bit levels preserves
        // the retained smooth-gradient stress while excluding real edges.
        if (la - lb).abs() > 2.5 / 255.0 {
            return;
        }
        let middle = (la + lb) * 0.5;
        let rendered_a = display_luma(pa);
        let rendered_b = display_luma(pb);
        let signed_bow = if la <= lb {
            rendered_b - rendered_a
        } else {
            rendered_a - rendered_b
        };
        for (transition, &(outer, inner)) in transitions.iter().enumerate() {
            if (outer..=inner).contains(&middle) {
                rims[transition].push(signed_bow);
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
    // locally smooth value crossing, never a pre-existing subject edge) is
    // already a binary form of the contextual test, and a graded context
    // here is capped near 2.5/255 by construction — no dynamic range. See
    // [`super::BoundaryReading::charged`].
    BoundaryReading { rim, transitions: transition_count, charged: rim }
}

/// The delivered transfer's worst TONE REVERSAL across the bands' edges, in
/// luma.
///
/// The reading is taken in the same two frames the rim gate uses: the
/// REFERENCE (the globals-only twin the engine passes in) supplies the input
/// tone order, and the RENDERED frame supplies the delivered one. Pixels are
/// binned by reference luma into the render's own 8-bit codes and each
/// populated bin holds the mean delivered luma of its members; walking the bins
/// upward, the reading is the deepest fall below the highest delivered luma
/// already reached. A monotone transfer of any steepness — including one that
/// compresses 30 input codes into 9.9, which is what the calibration band
/// measures — reads exactly 0. An inversion reads its own depth.
///
/// Only the bands' own spans and their transitions are read: outside
/// `lo_outer..hi_outer` the correction has no weight and its transfer is the
/// identity, so including it would dilute the bins that carry the question.
fn range_transfer_reversal(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    ranges: &[RangeMask],
) -> f32 {
    const BINS: usize = 256;
    let spans = ranges
        .iter()
        .filter_map(|range| match *range {
            RangeMask::Luminance { lo_outer, hi_outer, .. } => Some((lo_outer, hi_outer)),
            RangeMask::Color { .. } => None,
        })
        .collect::<Vec<_>>();
    let mut sum = [0.0f64; BINS];
    let mut count = [0u32; BINS];
    for (reference, delivered) in reference.iter().zip(rendered) {
        let l = display_luma(reference);
        if !spans.iter().any(|(lo, hi)| l >= *lo && l <= *hi) {
            continue;
        }
        let bin = (l.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as usize;
        sum[bin] += display_luma(delivered) as f64;
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
    depth
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

/// Automatic pure-Rust fallback after the global fit. Bands are attempted in
/// ascending luma order, and every attempt derives its source population from
/// the current rendered stack rather than the untouched source.
pub(super) fn attach_luminance_ranges(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    proposals: &[FieldBandProposal],
) {
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let tgt_px = fit::pixels_of(&t_img);
    let global_px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    // Preserve the global stage's own reported frame metric as the ceiling;
    // every accepted range must be no worse than the recipe already handed
    // to this fallback.
    let global_frame_err = report.err_after;
    let mut derived = derive_luminance_bands(&global_px, &tgt_px, &report.evidence, proposals);
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
    if derived.bands.is_empty() {
        fit::append_finished_disclosure(report, &global_px, &tgt_px);
        return;
    }

    let all_ranges = derived.bands.iter().map(|band| band.source).collect::<Vec<_>>();
    let target_coverage = derived
        .bands
        .iter()
        .map(|band| range_weights_for_pixels(&band.target, &tgt_px))
        .collect::<Vec<_>>();
    let mut target_weights = target_coverage.clone();
    normalize_partition_weights(&mut target_weights);
    for (band, weights) in derived.bands.iter_mut().zip(target_weights) {
        band.attachment.target_weights = weights;
    }
    let first_range = report.recipe.masks.len();
    let mut frame_err = report.err_after;
    let corr = report.correspondence.take();
    let mut accepted = Vec::new();
    for i in 0..derived.bands.len() {
        let (mut current_weights, mut current_coverage) =
            range_weights_from_current_render(&s_img, &report.recipe, &all_ranges);
        derived.bands[i].attachment.source_weights = std::mem::take(&mut current_weights[i]);
        derived.bands[i].attachment.coverage = Some(ZoneCoverage {
            source: std::mem::take(&mut current_coverage[i]),
            target: target_coverage[i].clone(),
        });
        let accepted_band = attach_one_zone(
            &s_img,
            &tgt_px,
            report,
            &mut frame_err,
            &derived.bands[i].attachment,
            // A band's divergence is DERIVED from its own luma evidence rather
            // than read off the structural instrument, so it is always present:
            // there is nothing here for the instrument to abstain about.
            Some(derived.bands[i].divergence),
            corr.as_ref(),
        );
        match accepted_band {
            Some(zone) => accepted.push(zone),
            None => push_range_abstention(
                report,
                &RangeAbstention {
                    lo: match derived.bands[i].source {
                        RangeMask::Luminance { lo, .. } => lo,
                        RangeMask::Color { .. } => 0.0,
                    },
                    hi: match derived.bands[i].source {
                        RangeMask::Luminance { hi, .. } => hi,
                        RangeMask::Color { .. } => 1.0,
                    },
                    reason: "the shared estimator or do-no-harm gates refused the correction"
                        .to_string(),
                },
            ),
        }
    }
    report.correspondence = corr;
    if accepted.is_empty() {
        fit::append_finished_disclosure(report, &global_px, &tgt_px);
        return;
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
        &s_img,
        report,
        &global_px,
        &accepted_ranges,
        &shares,
        first_range,
        initial_px,
    ) {
        BoundaryGateResult::Kept { pixels, .. } => pixels,
        BoundaryGateResult::Dropped => {
            fit::append_finished_disclosure(report, &global_px, &tgt_px);
            return;
        }
    };
    let final_frame_err = final_range_frame_err(&final_px, &tgt_px, &report.evidence);
    if final_frame_err > global_frame_err + RANGE_FRAME_REGRESSION_TOL {
        report.recipe.masks.truncate(first_range);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_FRAME_REFUSED,
                vec![
                    ("n", accepted.len().to_string()),
                    ("global", format!("{global_frame_err:.3}")),
                    ("after", format!("{final_frame_err:.3}")),
                    ("tol", format!("{RANGE_FRAME_REGRESSION_TOL:+.3}")),
                ],
            ),
        );
        report.err_after = global_frame_err;
        fit::append_finished_disclosure(report, &global_px, &tgt_px);
        return;
    }
    for zone in &mut accepted {
        let after = zone_moments(&final_px, &zone.source_weights);
        let target = zone_moments(&tgt_px, &zone.target_weights);
        zone.after = zone_err(&after, &target);
        push_zone_attached_note(report, zone);
    }
    frame_err = final_frame_err;
    report.err_after = frame_err;
    let worst = accepted.iter().map(|zone| zone.after).fold(0.0f32, f32::max);
    let range_conf = fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE);
    report.recipe.confidence = report.recipe.confidence.min(range_conf);
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::RANGE_CONFIDENCE,
            vec![
                ("n", accepted.len().to_string()),
                ("worst", format!("{worst:.3}")),
                ("frame", format!("{frame_err:.3}")),
            ],
        ),
    );
    fit::append_finished_disclosure(report, &final_px, &tgt_px);
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
        attach_luminance_ranges(&source, &target, &mut report, &[]);
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
        attach_luminance_ranges(&source, &target, &mut report, &[]);
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
        attach_luminance_ranges(&source, &source, &mut deferred, &[]);
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
        attach_luminance_ranges(&fixture, &target, &mut report, &[]);
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
        attach_luminance_ranges(&source, &target, &mut report, &[]);
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
}
