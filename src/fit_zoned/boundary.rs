//! The boundary ruler: readings, crossing samples and step frames, the line walks, cell sums and shares, the rims and the steps a zone's edge may show.

use super::*;

pub(super) const ZONE_BOUNDARY_LOW: f32 = 0.05;
pub(super) const ZONE_BOUNDARY_HIGH: f32 = 0.95;
const ZONE_BOUNDARY_MID: f32 = 0.5;
pub(super) const ZONE_BOUNDARY_PERCENTILE: f32 = 0.90;
const ZONE_BOUNDARY_INTERIOR_MIN: usize = 4;
/// Paired-sample stand-off for [`boundary_step`], in analysis pixels: each
/// sample of a pair sits `ZONE_STEP_OFFSET - 0.5` px from the 50% contour.
/// Two steps clear of the one-pixel ramp `render::sample_gray_norm` leaves on
/// a resampled 0/255 raster edge, while still reading each side's own plateau
/// rather than its neighbourhood.
const ZONE_STEP_OFFSET: usize = 2;
/// R40. How far past each far foot the hard family reads a crossing's TREND,
/// in multiples of the feet's own baseline (3 px): the flank sample sits
/// `STEP_FLANK_BASELINES * 3` px beyond the foot, on the foot's own side of
/// the contour, and the change from foot to flank divided by this number is
/// that side's change per baseline. Two, not one: at one the flank sits on
/// the resample-and-refine collar a hard raster wears (the first baseline
/// out, [`crossing_slope`] measured it at 0.005-0.008 luma in clean sky) and
/// the trend would carry the seam's own shoulder; and at one the reading's
/// rounding noise clears the one-code floor ([`BOUNDARY_STEP_FLOOR`]).
const STEP_FLANK_BASELINES: usize = 2;

#[derive(Clone, Copy, Debug)]
pub(crate) struct BoundaryReading {
    pub(super) rim: f32,
    pub(super) transitions: usize,
    /// The context-charged LUMA reading every boundary gate compares: each
    /// crossing's introduced step, scaled by `ceiling / budget` where its
    /// own per-crossing budget ([`crossing_budget`]) sits below the ceiling,
    /// ranked at the same 90th percentile. Since every charge >= its raw
    /// step, `charged >= rim` at every rank: the charged predicate subsumes
    /// the raw one, and nothing a gate refuses today can become acceptable.
    ///
    /// BOTH mask families charge, each the quantity its own ruler reads off
    /// the SAME three frames (since R40 the hard family reads a de-trended
    /// discontinuity and budgets it against the scene's own,
    /// [`discontinuity_budget`]). The soft family used to
    /// decline, on the argument that a feathered band is "a ramp by
    /// construction"; a desert-dusk pair falsified it. The OneFormer sky
    /// raster is the model's own soft class probability, so its transition
    /// in featureless haze is whatever the model emitted — two or three
    /// analysis pixels — and the flat ceiling let the shrink park the
    /// correction exactly on 0.012: a measured 0.013-0.016 luma step across
    /// the 50% contour in the finished 2048-px render (p50 0.018 with the
    /// mask against 0.005 without, 8-px stand-off, 154 columns), 3-4 codes
    /// of 255 in a smooth gradient. The hard family's own ruler would have
    /// budgeted that same haze at ONE code. One contract now: no seam larger
    /// than the scene's own local variation, floor one code, ceiling the
    /// family's constant.
    ///
    /// The luminance/colour-range family still declines, and for a reason
    /// that is not an assumption about shape: it admits only crossings whose
    /// REFERENCE is already smooth and then reports the RENDERED gradient
    /// there, so the scene's own variation sits inside the reading instead
    /// of being differenced away (`range.rs`). See
    /// [`BoundaryReading::uncharged`].
    pub(super) charged: f32,
    /// The raw per-CHANNEL reading, ranked exactly like `rim`: at each
    /// crossing the same difference in differences is taken on R, G and B
    /// and the crossing reports the largest magnitude of the three. A
    /// luma-only ruler reads a colour seam as nothing — gains that reproduce
    /// a target's mean colour can hold luma601 almost fixed while moving one
    /// channel several codes, which along a silhouette is a coloured halo.
    pub(super) colour: f32,
    /// `colour` after the same per-crossing charge, computed on the channel
    /// that produced the reading. The gate compares [`BoundaryReading::gated`].
    /// Since 2026-10-22 the step is read through a one-code dead band first
    /// ([`colour_dead_band`]), so unlike `charged >= rim` this rank may sit
    /// UNDER `colour` — by one code at a saturated budget.
    pub(super) colour_charged: f32,
    /// R37. The step the paired TARGET itself carries across this boundary,
    /// ranked like `rim`: the magnitude 90th percentile of the per-cell
    /// allowance the charged crossings were read against
    /// ([`cell_share`]). 0.0 when the ruler ran without a target.
    /// Disclosed so a kept rim above the ceiling reads as what it is — the
    /// target's own horizon, reproduced — and not as a gate that let a seam
    /// through.
    pub(super) asked: f32,
}

impl BoundaryReading {
    /// What every boundary gate compares: whichever charged rank is worse.
    /// ONE number, so a correction cannot pass by being quiet in the
    /// coordinate it did not move.
    pub(super) fn gated(self) -> f32 {
        self.charged.max(self.colour_charged)
    }

    /// A reading from the family that measures its band's OWN coordinate —
    /// luma for a luminance band, chromaticity for a colour band — and
    /// therefore neither charges nor runs the per-channel ruler
    /// (`range::range_transition_rim`). The colour ranks are 0.0 because
    /// that ruler was not run, not because a colour seam was measured and
    /// found absent; the reading it does return is already in the coordinate
    /// its band is made of.
    pub(super) fn uncharged(rim: f32, transitions: usize) -> Self {
        Self { rim, transitions, charged: rim, colour: 0.0, colour_charged: 0.0, asked: 0.0 }
    }

    pub(super) fn nothing_measured() -> Self {
        Self { rim: 0.0, transitions: 0, charged: 0.0, colour: 0.0, colour_charged: 0.0, asked: 0.0 }
    }
}

/// One measured crossing (hard family) or transition (soft family), in both
/// coordinates, with everything its charge needs. The raw and charged ranks
/// are taken over SEPARATE orderings ([`reading_of`]), so `rim` stays the
/// luma p90 every existing log line and pinned triple is comparable against
/// while `charged` is what the gate compares; a single re-ranked field would
/// return "that crossing's own luma step" at a silently moved position.
///
/// R37: the charge is no longer computed where the crossing is read. The
/// paired target's own step is pooled per evidence cell first, as a share of
/// the frozen candidate's own step ([`CellSums`], [`cell_share`]), and only
/// the part of the introduced step the target does not carry is charged
/// ([`unasked`]).
#[derive(Clone, Copy, Debug)]
struct CrossingSample {
    /// The signed luma step the correction introduced on the render under
    /// measurement.
    luma: f32,
    /// The same step on the FROZEN k=1 candidate — the correction's own
    /// shape, which the target's allowance is expressed as a share of.
    luma_frozen: f32,
    /// Its per-crossing context budget ([`crossing_budget`] for the soft
    /// family, [`discontinuity_budget`] for the hard).
    luma_budget: f32,
    /// The signed introduced step on `channel`, the channel that moved most.
    colour: f32,
    channel: usize,
    colour_budget: f32,
    /// The frozen candidate's own step per channel.
    colour_frozen: [f32; 3],
    /// The evidence cell (`fit_cells::CELLS_X` x `CELLS_Y`) the crossing falls
    /// in, whose share is the one honoured for it.
    cell: usize,
}

/// The three frames one crossing is read from.
#[derive(Clone, Copy)]
struct StepFrames<'a> {
    /// This same frame rendered WITHOUT the correction under test.
    reference: &'a [[f32; 3]],
    /// The render under measurement — the `k` the bisection is trying.
    rendered: &'a [[f32; 3]],
    /// The k=1 candidate, FROZEN through the bisection; the correction's
    /// own slope is read from it and never from `rendered`. See
    /// [`boundary_line_steps`].
    frozen: &'a [[f32; 3]],
    /// R37. The paired target at the same geometry, when the gate has one:
    /// the step IT carries at each crossing is what the correction is allowed
    /// to reproduce there ([`unasked`]). `None` charges every introduced
    /// step, which is the rule as it stood — what the fixtures pinned on the
    /// context budget alone still measure.
    target: Option<&'a [[f32; 3]]>,
}

/// One scan line's addressing: `(start, step, len)`, exactly the triple both
/// line rulers already walk.
type LineWalk = (usize, usize, usize);

/// One scan line of the two boundary walkers — a row or a column of the
/// analysis frame, `start + p * step` for `p < len`, with the frame `grid`
/// its evidence cells are laid on — so a walker takes the line as ONE
/// argument beside its frames, its alpha and its two tallies.
#[derive(Clone, Copy)]
struct ScanLine {
    start: usize,
    step: usize,
    len: usize,
    grid: (usize, usize),
}

impl ScanLine {
    fn row(y: usize, (w, h): (usize, usize)) -> Self {
        Self { start: y * w, step: 1, len: w, grid: (w, h) }
    }

    fn column(x: usize, (w, h): (usize, usize)) -> Self {
        Self { start: x, step: w, len: h, grid: (w, h) }
    }
}

/// ONE crossing's contextual budget for the SOFT family (the hard family's
/// is [`discontinuity_budget`] since R40): a
/// correction may introduce a discontinuity no larger than the largest
/// smooth variation the neighbourhood already carries — the scene's own
/// change across the crossing (`context`, read off the frame rendered
/// WITHOUT the correction) or [`BOUNDARY_STEP_SHAPE`] times the correction's
/// own same-side slope beside it ([`crossing_slope`]) — never more than the
/// family's `ceiling`, never less than one code value.
fn crossing_budget(context: f32, slope: f32, ceiling: f32) -> f32 {
    context.abs().max(BOUNDARY_STEP_SHAPE * slope).clamp(BOUNDARY_STEP_FLOOR, ceiling)
}

/// R40. ONE crossing's budget for the HARD family. A hard 0/255 raster can
/// only introduce a DISCONTINUITY at its contour, so the only thing that can
/// mask one is a discontinuity the scene already has there — a roof line the
/// mask follows, a horizon a tile edge grazes — read off the frame rendered
/// WITHOUT the correction as the same de-trended jump the crossing itself is
/// read as ([`boundary_line_steps`]). A smooth gradient is not one and buys
/// nothing. The reference pair's sky (2026-09-22, per-crossing dump of the
/// accepted r1c0 reading at 0.85) is why: the context term read the sky's
/// own 3-px gradient as an edge, 0.005-0.007 luma; the R37 allowance read
/// the target's gradient DIFFERENCE as a step to reproduce, 0.0055; and the
/// slope credit added its share — enough for a 2.3-code tile step to pass
/// at k=0.134 in featureless sky, a visible pale block. Three ways of
/// calling a gradient a discontinuity, one reading that stops all three:
/// de-trended, the sky's context is ~0, the target asks ~0, and the tile
/// step is charged at the floor's rate. No slope credit either — a
/// correction that ramps is not a discontinuity in the first place and
/// reads ~0 under the same de-trending, so it needs no budget to stay whole.
/// Never less than one code, never more than the family's `ceiling`.
fn discontinuity_budget(context: f32, ceiling: f32) -> f32 {
    context.abs().clamp(BOUNDARY_STEP_FLOOR, ceiling)
}

/// A BRANCH, not a multiply by a ratio that happens to be one: at or above
/// the ceiling the charge IS the raw reading, bit for bit, so a fully
/// textured or genuinely ramped border is governed by exactly the constant it
/// was governed by before the charge existed. (The colour coordinate hands
/// this its step after the one-code dead band, [`colour_dead_band`].)
fn crossing_charge(introduced: f32, budget: f32, ceiling: f32) -> f32 {
    if budget >= ceiling {
        introduced.abs()
    } else {
        introduced.abs() * (ceiling / budget)
    }
}

/// R37. The part of an introduced step the paired target does NOT carry at
/// that crossing: nothing while the correction moves the boundary the way
/// the target's own boundary goes and no further than it; the overshoot past
/// it; the whole step when it moves the other way. With no target (`wanted`
/// 0) every introduced step is unasked, which is the rule as it stood.
///
/// Why the gate needed this: the ceiling is absolute. On the reference pair
/// the target's own horizon steps +8.4 codes at the median (+15.3 at the 90th
/// percentile) against the source's +3.0, so 101 of 154 contour columns need
/// the masks to introduce MORE than the whole 3-code ceiling to sit within
/// three codes of the target — and the sky zone's solved move was shrunk to
/// k=0.105 for reproducing exactly that horizon. A step the picture itself
/// has at that place is fidelity, not a seam; only the excess is one.
/// Monotone in the introduced step for a fixed allowance, so the shrink
/// bisection keeps its invariant, and exactly 0.0 for an introduced 0.0, so
/// the k=0 render still reads 0.0.
pub(super) fn unasked(introduced: f32, wanted: f32) -> f32 {
    if introduced * wanted <= 0.0 {
        introduced.abs()
    } else {
        (introduced.abs() - wanted.abs()).max(0.0)
    }
}

/// R37. What one evidence cell's boundary pixels add up to, per coordinate
/// (`[luma, R, G, B]`), so the target's own step is read as a MEAN over the
/// cell. The target's texture is re-synthesised where the zoned fit runs
/// (the reference sky pairs at layout scale only: D 0.617 at pixel scale),
/// so at one pixel its step is that texture's noise — twelve codes on the
/// reference pair's horizon, the size of the correction's own step. Noise
/// averages out of a mean and does not out of a rank or a vote: measured on
/// that pair, the per-crossing sign vote this replaced read a coin flip
/// (385 of 718 crossings agreeing) in cells whose mean stood thirty
/// standard errors from zero. Soft family: every band pixel of every FOOTED
/// band (one with settled feet on opposite sides, [`boundary_line_rims`]),
/// each in its own cell; hard family: every crossing.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CellSums {
    /// Band pixels (soft) or crossings (hard) summed.
    pixels: u32,
    /// Lines whose crossing sample was assigned to this cell — the quorum.
    pub(super) crossings: u32,
    /// The render under measurement's step against `reference`.
    rendered: [f32; 4],
    /// The frozen k=1 candidate's own step — what the correction makes.
    frozen: [f32; 4],
    /// The paired target's own step — what the picture asks — and its
    /// square, for the standard error of the mean. Zero without a target.
    target: [f32; 4],
    target_sq: [f32; 4],
    /// The rendered step's Lab a*/b* part, for the sub-zone trial rulers.
    ab: [f32; 2],
}

impl CellSums {
    pub(super) fn add(&mut self, rendered: [f32; 4], frozen: [f32; 4], target: Option<[f32; 4]>, ab: [f32; 2]) {
        self.pixels += 1;
        for k in 0..4 {
            self.rendered[k] += rendered[k];
            self.frozen[k] += frozen[k];
            if let Some(t) = target {
                self.target[k] += t[k];
                self.target_sq[k] += t[k] * t[k];
            }
        }
        self.ab[0] += ab[0];
        self.ab[1] += ab[1];
    }
}

fn cells_grid() -> Vec<CellSums> {
    vec![CellSums::default(); crate::fit_cells::CELLS_X * crate::fit_cells::CELLS_Y]
}

/// How many standard errors from zero a cell's mean target step must stand
/// before it is an allowance: the cell must be able to tell the step from
/// its own texture noise. Two is the same line the tile eligibility draws on
/// its 95% residual interval, stated separately because the two instruments
/// must stay re-derivable on their own evidence.
const TARGET_STEP_SIGMAS: f32 = 2.0;
/// Fewer lines than this crossing a cell and it reads no allowance: a mean
/// of a handful of pixels has a standard error the size of the step. At the
/// analysis grid a horizontal horizon puts ~32 crossings in each 32-px cell.
pub(super) const TARGET_STEP_MIN_CROSSINGS: usize = 8;

/// R37. The share of the correction's own boundary step the paired target
/// carries in one cell and coordinate `k` (`0` luma, `1..=3` a channel): the
/// cell's mean target step over its mean frozen-candidate step, honoured
/// only where the cell holds at least [`TARGET_STEP_MIN_CROSSINGS`]
/// crossings, both means are at least one code ([`BOUNDARY_STEP_FLOOR`]:
/// below one code the ruler reads its own rounding), the target moves the
/// CORRECTION's way, and the target's mean stands at least
/// [`TARGET_STEP_SIGMAS`] standard errors from zero; 0 otherwise, and 0
/// everywhere without a target. Above 1 the target asks more than the
/// candidate makes and the whole step is asked.
///
/// A SHARE rather than a pooled step, because a zone dial is multiplicative
/// in linear light: its step varies with the base level along one cell, and
/// a pooled absolute step left the brighter half of every cell charged for a
/// horizon the target carries at exactly the candidate's height. The share
/// is a property of the correction's shape, so the allowance it yields
/// (`share × frozen step`) holds still through the shrink bisection exactly
/// as the slope credit does.
fn cell_share(cell: &CellSums, k: usize) -> f32 {
    if (cell.crossings as usize) < TARGET_STEP_MIN_CROSSINGS || cell.pixels == 0 {
        return 0.0;
    }
    let n = cell.pixels as f32;
    let asked = cell.target[k] / n;
    let made = cell.frozen[k] / n;
    let variance = (cell.target_sq[k] / n - asked * asked).max(0.0);
    let error = (variance / n).sqrt();
    if asked.abs() < BOUNDARY_STEP_FLOOR
        || made.abs() < BOUNDARY_STEP_FLOOR
        || asked * made <= 0.0
        || asked.abs() < TARGET_STEP_SIGMAS * error
    {
        0.0
    } else {
        asked / made
    }
}

/// [`cell_share`] for every cell, luma first and then the three channels.
pub(super) fn cell_shares(cells: &[CellSums]) -> [Vec<f32>; 4] {
    std::array::from_fn(|k| cells.iter().map(|cell| cell_share(cell, k)).collect())
}

/// The sub-zone trial rulers' reading of the same sums: the render's mean
/// step against `reference` per cell — `[|luma|, widest channel, Lab chroma
/// on the 100-unit scale / 100]`, 0 where a cell has no band pixel.
fn cell_gaps(cells: &[CellSums]) -> Vec<[f32; 3]> {
    cells
        .iter()
        .map(|cell| {
            if cell.pixels == 0 {
                return [0.0; 3];
            }
            let n = cell.pixels as f32;
            let mean = cell.rendered.map(|v| v / n);
            let chroma = (cell.ab[0] / n).hypot(cell.ab[1] / n) / 100.0;
            [mean[0].abs(), mean[1..].iter().fold(0.0f32, |m, v| m.max(v.abs())), chroma]
        })
        .collect()
}

/// CIE Lab, D65, from the analysis raster's display sRGB. Keep signed a/b:
/// equally strong warm/cool residuals have equal deltaE and opposite intent.
pub(super) fn lab(rgb: &[f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(render::srgb_to_linear);
    let f = |t: f32| if t > 216.0 / 24389.0 {
        t.cbrt()
    } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let x = f((0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047);
    let y = f(0.2126729 * r + 0.7151522 * g + 0.0721750 * b);
    let z = f((0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883);
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

/// The evidence cell a pixel index falls in, on the same grid the target's
/// cell means are read on (`fit_cells`).
fn cell_of(i: usize, (width, height): (usize, usize)) -> usize {
    let (nx, ny) = (crate::fit_cells::CELLS_X, crate::fit_cells::CELLS_Y);
    let (x, y) = (i % width.max(1), i / width.max(1));
    (y * ny / height.max(1)).min(ny - 1) * nx + (x * nx / width.max(1)).min(nx - 1)
}

/// The magnitude 90th percentile both rulers rank at. A correction that
/// darkens its side of a border is as visible a seam as one that brightens
/// it, and a signed percentile would let a dark edge hide behind a bright
/// one.
fn magnitude_rank(values: &mut [f32]) -> f32 {
    values.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
    let rank = ((values.len() as f32 * ZONE_BOUNDARY_PERCENTILE).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    values[rank].abs()
}

/// The slope credit one crossing's COLOUR reading earns.
///
/// A single channel's `u1` is a difference of two 8-BIT renders, so its slope
/// over the 3-px baseline quantises to whole code values — and the MIN over
/// two consecutive baselines then reads 0 whenever either baseline happens not
/// to cross a code, on a ramp that is plainly there. Luma is the same
/// measurement with three independent quantisations averaged, so it resolves
/// the SAME shape sub-code (it is why [`BOUNDARY_STEP_SHAPE`] could be
/// calibrated at all), and the correction's alpha ramp is shared by all three
/// channels: the luma reading is therefore a valid lower bound on any one
/// channel's slope, never an invented credit.
///
/// Without this floor the colour ruler charges a seam that is not a colour
/// seam. Measured, before R40 took the slope term off the hard family, on
/// `shoulder_fixture(32.0, 0.37)` — a pure EXPOSURE dial
/// over a warm field, where every channel moves together and the budget is
/// supposed to cancel the channel ratio exactly — the R channel's slope
/// quantised to zero, its budget fell to the one-code floor, and the accepted
/// shrink went 0.24536133 -> 0.14794922 with the kept step falling from two
/// code values to one. That is the instrument's rounding, not a halo.
fn colour_slope_credit(channel_slope: f32, luma_slope: f32) -> f32 {
    channel_slope.max(luma_slope)
}

/// The correction's OWN same-side slope beside one crossing, in `channel`
/// (`None` reads luma), read by the soft family (the hard family stopped
/// consulting it at R40).
///
/// `feet` are the two far feet the crossing was measured on, as positions
/// along the line; each side's slope walks further out in its own direction
/// on `2 * (ZONE_STEP_OFFSET - 1) + 1`-px baselines and is the MINIMUM over
/// two consecutive ones, because a ramp earns credit only where it PERSISTS
/// past the guided-refine collar: a hard raster's resample-and-refine collar
/// spans exactly the first baseline out and reads as a spurious inner slope
/// — measured on the real seam, inner |u1| slope ~0.005-0.008 in CLEAN sky,
/// which times [`BOUNDARY_STEP_SHAPE`] had bought the whole ceiling back. The
/// seam's own soft shoulder is part of the seam, never a masker. A true ramp
/// shows the same slope on both baselines and keeps full credit. The larger
/// of the two sides is returned; an unavailable or side-crossing extended
/// foot contributes zero slope (no credit).
///
/// `u1` is read off the FROZEN k=1 candidate, never off the render under
/// bisection: the shape of a correction is a property of the correction, and
/// shrinking scales it without changing its shape; measuring it live would
/// let the budget chase the bisection.
fn crossing_slope(
    frames: StepFrames<'_>,
    alpha: &[f32],
    line: LineWalk,
    feet: (usize, usize),
    forward: bool,
    channel: Option<usize>,
) -> f32 {
    let StepFrames { reference, rendered, frozen, .. } = frames;
    let (start, step, len) = line;
    let (far_in, far_out) = feet;
    let read = |p: &[f32; 3]| match channel {
        Some(c) => p[c],
        None => 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2],
    };
    let index = |p: usize| -> Option<usize> {
        let i = start + p * step;
        (i < alpha.len() && i < rendered.len() && i < reference.len() && i < frozen.len())
            .then_some(i)
    };
    let inside = |i: usize| alpha[i] >= ZONE_BOUNDARY_MID;
    let (Some(i_in), Some(i_out)) = (index(far_in), index(far_out)) else {
        return 0.0;
    };
    let u1 = |i: usize| read(&frozen[i]) - read(&reference[i]);
    let baseline = 2 * ZONE_STEP_OFFSET.saturating_sub(1) + 1;
    let probe = |foot: usize, inward: bool, hops: usize| -> Option<usize> {
        let away = baseline * hops;
        let p = if forward == inward { foot.checked_add(away) } else { foot.checked_sub(away) }?;
        if p >= len {
            return None;
        }
        let i = index(p)?;
        (inside(i) == inward).then_some(i)
    };
    let slope_in = match (probe(far_in, true, 1), probe(far_in, true, 2)) {
        (Some(e1), Some(e2)) => (u1(e1) - u1(i_in)).abs().min((u1(e2) - u1(e1)).abs()),
        _ => 0.0,
    };
    let slope_out = match (probe(far_out, false, 1), probe(far_out, false, 2)) {
        (Some(e1), Some(e2)) => (u1(i_out) - u1(e1)).abs().min((u1(e1) - u1(e2)).abs()),
        _ => 0.0,
    };
    slope_in.max(slope_out)
}

fn median(mut values: Vec<f32>) -> f32 {
    values.sort_by(f32::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    }
}

/// One frame's settled treatment over the reference, as linear multipliers.
#[derive(Clone, Copy)]
struct Multipliers {
    luma: f32,
    channels: [f32; 3],
}

/// [`Multipliers`] on each side of a boundary.
#[derive(Clone, Copy)]
struct SideMultipliers {
    sky: Multipliers,
    land: Multipliers,
}

/// How far, in analysis pixels, [`ContentShare`] reaches for the settled sky
/// and settled land a band pixel's content is read between. Past the widest
/// soft band the night pair's keyed horizon leaves (2-3 px) and inside one
/// evidence cell (32 px), so the two levels are this boundary's own.
const CONTENT_REACH: usize = 6;

/// How much of each band pixel is SKY CONTENT, and how surely that can be
/// told, read once per frame off the reference (the frame rendered WITHOUT the
/// correction) — the treatment a band pixel is owed is that mixture of the
/// settled sky's and the settled land's ([`boundary_line_rims`]).
///
/// Read in TWO dimensions, from the settled pixels around the pixel, and not
/// from the two feet of the scan line it was reached on. The feet version
/// could only see a step ACROSS its own line: a row scan running along a
/// near-level horizon walks a band of half-sky, half-mesa pixels with no
/// settled foot on the far side, found none, and owed every one of them the
/// whole sky's treatment. On the night pair (2026-10-19) those tangent runs
/// were what still held the sky at k=0.003 after the mask had been keyed onto
/// the skyline and the edge crossings traded.
///
/// The mixture is trusted along the same ramp the feet used: not at all while
/// the two local levels are within one code ([`BOUNDARY_STEP_FLOOR`]) — haze,
/// where the content cannot be told apart and the pixel is read as sky, the
/// rule as it stood — fully at two codes.
struct ContentShare {
    share: Vec<f32>,
}

impl ContentShare {
    fn read(reference: &[[f32; 3]], weights: &[f32], width: usize, height: usize) -> Self {
        let n = reference.len().min(weights.len()).min(width * height);
        let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        let linear = |p: &[f32; 3]| {
            0.2126 * render::srgb_to_linear(p[0])
                + 0.7152 * render::srgb_to_linear(p[1])
                + 0.0722 * render::srgb_to_linear(p[2])
        };
        let side = |settled: &dyn Fn(f32) -> bool| {
            let member = (0..width * height)
                .map(|i| f32::from(i < n && settled(weights[i])))
                .collect::<Vec<_>>();
            let weighted = |value: &dyn Fn(&[f32; 3]) -> f32| {
                let product = (0..width * height)
                    .map(|i| if member[i] > 0.0 { value(&reference[i]) } else { 0.0 })
                    .collect::<Vec<_>>();
                box_sum(&product, width, height, CONTENT_REACH)
            };
            (box_sum(&member, width, height, CONTENT_REACH), weighted(&luma), weighted(&linear))
        };
        let (sky_n, sky_luma, sky_linear) = side(&|a| a >= ZONE_BOUNDARY_HIGH);
        let (land_n, land_luma, land_linear) = side(&|a| a <= ZONE_BOUNDARY_LOW);
        let mut share = vec![1.0f32; width * height];
        for i in 0..n {
            if sky_n[i] <= 0.0 || land_n[i] <= 0.0 {
                continue;
            }
            let step = (sky_luma[i] / sky_n[i] - land_luma[i] / land_n[i]).abs();
            let c = ((step - BOUNDARY_STEP_FLOOR) / BOUNDARY_STEP_FLOOR).clamp(0.0, 1.0);
            let (y_sky, y_land) = (sky_linear[i] / sky_n[i], land_linear[i] / land_n[i]);
            if c <= 0.0 || (y_sky - y_land).abs() <= f32::EPSILON {
                continue;
            }
            let mix = ((linear(&reference[i]) - y_land) / (y_sky - y_land)).clamp(0.0, 1.0);
            share[i] = 1.0 - c * (1.0 - mix);
        }
        Self { share }
    }
}

/// Sum over the `(2r + 1)`-square window around each pixel, clipped at the
/// frame — two separable running sums.
fn box_sum(values: &[f32], width: usize, height: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; values.len()];
    for y in 0..height {
        let mut prefix = vec![0.0f32; width + 1];
        for x in 0..width {
            prefix[x + 1] = prefix[x] + values[y * width + x];
        }
        for x in 0..width {
            rows[y * width + x] = prefix[(x + r + 1).min(width)] - prefix[x.saturating_sub(r)];
        }
    }
    let mut out = vec![0.0f32; values.len()];
    for x in 0..width {
        let mut prefix = vec![0.0f32; height + 1];
        for y in 0..height {
            prefix[y + 1] = prefix[y] + rows[y * width + x];
        }
        for y in 0..height {
            out[y * width + x] = prefix[(y + r + 1).min(height)] - prefix[y.saturating_sub(r)];
        }
    }
    out
}

/// Every row and every column of the frame through [`boundary_line_rims`],
/// with the content share read once ([`ContentShare`]).
fn rim_samples(frames: StepFrames<'_>, weights: &[f32], (w, h): (usize, usize)) -> (Vec<CrossingSample>, Vec<CellSums>) {
    let content = ContentShare::read(frames.reference, weights, w, h);
    let (mut rims, mut cells) = (Vec::new(), cells_grid());
    for y in 0..h {
        boundary_line_rims(frames, weights, &content, ScanLine::row(y, (w, h)), &mut rims, &mut cells);
    }
    for x in 0..w {
        boundary_line_rims(frames, weights, &content, ScanLine::column(x, (w, h)), &mut rims, &mut cells);
    }
    (rims, cells)
}

/// Add one row's or column's INTRODUCED transition readings to `out`.
///
/// A transition contributes only when that SAME scan line reaches settled sky
/// (>=95%) and settled land (<=5%); this keeps a soft but one-sided mask edge
/// from inventing an interior. Within the 5%-95% run, the sky half is tested
/// against `reference` — this same frame rendered WITHOUT the correction
/// under test — exactly as [`boundary_line_steps`] already tests its own
/// crossings, so scene content under the feather cannot false-positive.
///
/// The reference is TRANSPORTED through the settled sky's own linear
/// multiplier before it is subtracted:
///
/// ```text
///     M            = linear(median settled sky on `rendered`)
///                  / linear(median settled sky on `reference`)
///     transported(L) = encode(M * linear(L))
///     d(i)         = luma(rendered[i]) - transported(luma(reference[i]))
/// ```
///
/// A plain difference of differences is NOT enough here, and the reason is
/// that every zone dial is MULTIPLICATIVE in linear light: a band pixel
/// carrying a content rim `r` above the settled sky moves MORE in absolute
/// luma than the settled sky does under the same multiplier, so the additive
/// residual keeps a term proportional to `r * (m - 1)` and charges the
/// correction for a bow it did not introduce. Measured on a 12x4 hazy fixture
/// under ONE multiply applied to the WHOLE frame (no seam exists by
/// construction, truth 0.0000): additive reads -0.0083 / +0.0069 / +0.0110 /
/// +0.0201 at g = 0.70 / 1.30 / 1.50 / 2.00, i.e. 1.68x the budget at a +1 EV
/// dial; the transported form reads exactly +0.0000 in every cell.
/// `transported` is exact because it asks "what would this pixel look like if
/// it had received the settled sky's OWN treatment", so the pixel's own
/// content cancels identically whatever the transfer function does.
///
/// The `M == 1.0` short circuit is load bearing rather than decoration: the
/// sRGB round trip is accurate to 8.9e-08 but not bit exact, and only bit
/// exactness makes `rendered == reference` read 0.0 EXACTLY, which is what
/// keeps the `k=0` verdict an invariant instead of a float coin flip.
///
/// ONE argmax, taken on the difference and ranked by MAGNITUDE — two
/// independent maxima would pair the brightest rendered pixel with a
/// reference pixel somewhere else on the line, which is a comparison of two
/// different places rather than one pixel's own change.
///
/// COLOUR rides the identical form, one transport per channel
/// (`M_c` from the settled sky's own median in that channel), and the
/// transition reports the largest of the three magnitudes together with the
/// channel that produced it, so the charge below is read on that channel's
/// own context.
///
/// Each transition is CHARGED against the same per-crossing budget the
/// cross-boundary-step ruler uses ([`crossing_budget`]), computed on the same
/// three frames: the scene's own luma change across the whole transition band
/// on the reference — the span the introduced bow itself is measured over —
/// or [`BOUNDARY_STEP_SHAPE`] times the correction's own same-side slope read
/// outward from the 50% contour ([`crossing_slope`]), whichever is larger.
/// The contour, not the band edge, is where the slope is read, for the same
/// reason the hard ruler reads it there: a WIDE alpha ramp keeps the probes
/// inside its own ramp and earns credit, a two-pixel one puts them on the
/// settled plateaus where the correction is flat and earns none. That is what
/// makes the feather widening upstream ([`crate::mask_refine::widen_smooth_feather`])
/// buy back the strength the charge takes away, instead of the two fighting.
fn boundary_line_rims(
    frames: StepFrames<'_>,
    weights: &[f32],
    content: &ContentShare,
    line: ScanLine,
    out: &mut Vec<CrossingSample>,
    cells: &mut [CellSums],
) {
    let StepFrames { reference, rendered, frozen, target } = frames;
    let ScanLine { start, step, len, grid } = line;
    let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let index = |p: usize| -> Option<usize> {
        let i = start + p * step;
        (i < rendered.len() && i < reference.len() && i < frozen.len() && i < weights.len())
            .then_some(i)
    };
    let mut sky_index = Vec::new();
    let mut land_index = Vec::new();
    for p in 0..len {
        let Some(i) = index(p) else {
            break;
        };
        if weights[i] >= ZONE_BOUNDARY_HIGH {
            sky_index.push(i);
        } else if weights[i] <= ZONE_BOUNDARY_LOW {
            land_index.push(i);
        }
    }
    if sky_index.len() < ZONE_BOUNDARY_INTERIOR_MIN || land_index.len() < ZONE_BOUNDARY_INTERIOR_MIN {
        return;
    }
    // Each frame's settled treatment over the reference's, on BOTH sides of
    // the boundary: the multiplier that frame gives the settled sky and the
    // settled land, in luma and per channel. The render under measurement,
    // the FROZEN k=1 candidate (the correction's own shape, R37) and the
    // paired target (its own shortfall in the band is the treatment the
    // picture itself gives this transition, R37) each get their own, all
    // read off the same settled pixels.
    let settled = |side: &[usize], frame: &[[f32; 3]]| -> (f32, [f32; 3]) {
        let l = median(side.iter().map(|&i| luma(&frame[i])).collect());
        let c = std::array::from_fn(|c| median(side.iter().map(|&i| frame[i][c]).collect()));
        (l, c)
    };
    let reference_sky = settled(&sky_index, reference);
    let reference_land = settled(&land_index, reference);
    let over = |(l, c): (f32, [f32; 3]), (base_l, base_c): (f32, [f32; 3])| -> Multipliers {
        Multipliers {
            luma: render::srgb_to_linear(l) / render::srgb_to_linear(base_l).max(1e-5),
            channels: std::array::from_fn(|k| {
                render::srgb_to_linear(c[k]) / render::srgb_to_linear(base_c[k]).max(1e-5)
            }),
        }
    };
    let sides = |frame: &[[f32; 3]]| -> SideMultipliers {
        SideMultipliers {
            sky: over(settled(&sky_index, frame), reference_sky),
            land: over(settled(&land_index, frame), reference_land),
        }
    };
    let rendered_side = sides(rendered);
    let frozen_side = sides(frozen);
    let target_side = target.map(sides);
    let transport = |m: f32, v: f32| -> f32 {
        if m == 1.0 {
            v
        } else {
            render::linear_to_srgb(m * render::srgb_to_linear(v))
        }
    };
    // The treatment a band pixel is OWED: the settled sky's multiplier on the
    // share of it that is sky, the settled land's on the rest (`owed`). A
    // pixel that is all sky — every pixel of a boundary drawn through haze —
    // is owed exactly the settled sky's treatment, the rule as it stood.
    let treat = |side: &SideMultipliers, sky_share: f32, channel: Option<usize>, v: f32| -> f32 {
        let pick = |m: &Multipliers| channel.map_or(m.luma, |c| m.channels[c]);
        let (sky, land) = (pick(&side.sky), pick(&side.land));
        if sky_share >= 1.0 || sky == land {
            transport(sky, v)
        } else {
            transport(sky_share * sky + (1.0 - sky_share) * land, v)
        }
    };
    let inside = |i: usize| weights[i] >= ZONE_BOUNDARY_MID;
    let stand_off = ZONE_STEP_OFFSET.saturating_sub(1);
    let mut p = 0usize;
    while p < len {
        let Some(i) = index(p) else {
            p += 1;
            continue;
        };
        if !(ZONE_BOUNDARY_LOW..ZONE_BOUNDARY_HIGH).contains(&weights[i]) {
            p += 1;
            continue;
        }
        let band_begin = p;
        while p < len {
            let Some(i) = index(p) else {
                break;
            };
            if !(ZONE_BOUNDARY_LOW..ZONE_BOUNDARY_HIGH).contains(&weights[i]) {
                break;
            }
            p += 1;
        }
        let band_end = p - 1;
        // The SCENE's own variation over the span the bow was measured on:
        // the settled pixel just outside each end of the transition band.
        // Not the 3-px baseline the hard ruler uses, because the quantity
        // being budgeted is not a 3-px step — it is how far the treatment
        // inside the band falls short of the treatment it is owed, which the
        // whole band's width carries.
        //
        // A band with no FEET — both ends on the same side of the contour
        // (an alpha dip inside the zone, or a bump outside it) or one end on
        // the frame's edge — is not a transition BETWEEN the two sides, so
        // this ruler cannot read its context, and reading that context as 0
        // was the harshest reading there is. Such a band is skipped whole:
        // no crossing sample and no contribution to its cell's sums, so the
        // R37 cell means are read from footed transitions only. Measured
        // (2026-10-22) on the two zones every verdict of the 11-pair battery
        // bound on: a building whose mask has alpha holes on its sign
        // letters — 62% of its 4224 crossings unfooted, 93% of those at the
        // one-code floor budget, 196 of the top-5% 212 unfooted — and a sky
        // whose top 5% were ALL unfooted (63% of 1655), while on its footed
        // tree line the target carried the same red step as the candidate
        // (-0.0995 against -0.106) and every cell still asked 0, its mean
        // diluted by the unfooted band pixels. An interior alpha dip is the
        // mask stage's question (`mask_refine`'s confidence-hole filling),
        // and the hard ruler still reads the tile contours.
        let settled_at = |q: usize| if q < len { index(q) } else { None };
        let band_feet = match (band_begin.checked_sub(1).and_then(settled_at), settled_at(band_end + 1)) {
            (Some(a), Some(b)) if inside(a) != inside(b) => {
                Some(if inside(a) { (a, b) } else { (b, a) })
            }
            _ => None,
        };
        let Some((foot_in, foot_out)) = band_feet else {
            continue;
        };
        // How much of band pixel `i` is SKY CONTENT ([`ContentShare`]): where
        // the scene itself steps across the boundary — a horizon, a mesa
        // against the night — a band pixel's level between the settled sky
        // and the settled land around it is the mixture of the two contents
        // it holds, and that mixture is the treatment it is owed. The ruler
        // used to owe every band pixel the WHOLE settled sky's treatment,
        // which is right in haze and wrong on a silhouette: a pixel that is
        // 40% mesa carries 40% of the land's treatment however exact the
        // mask, so the shortfall it read grew with the correction itself and
        // the shrink took the strength away. Measured on a night pair
        // (2026-10-19) whose horizon band needed blue x3.7: 662 of 758
        // crossings charged past the ceiling at full strength, all on the
        // blue channel, all in the two cell rows on the horizon, and the sky
        // shipped at k=0.066. In haze the share is 1, the rule as it stood.
        let owed = |i: usize| -> f32 { content.share[i] };
        // Each argmax remembers WHERE it was read, so the target's own
        // shortfall is read at that same pixel and nowhere else.
        let mut introduced: Option<(f32, usize)> = None;
        let mut coloured: Option<(f32, usize, usize)> = None;
        for q in band_begin..=band_end {
            let Some(i) = index(q) else {
                continue;
            };
            if weights[i] < ZONE_BOUNDARY_MID {
                continue;
            }
            let share = owed(i);
            let here = luma(&rendered[i]) - treat(&rendered_side, share, None, luma(&reference[i]));
            introduced = Some(match introduced {
                Some((v, at)) if v.abs() >= here.abs() => (v, at),
                _ => (here, i),
            });
            let transported: [f32; 3] =
                std::array::from_fn(|c| treat(&rendered_side, share, Some(c), reference[i][c]));
            for c in 0..3 {
                let moved = rendered[i][c] - transported[c];
                coloured = Some(match coloured {
                    Some((v, channel, at)) if v.abs() >= moved.abs() => (v, channel, at),
                    _ => (moved, c, i),
                });
            }
            // R37: the cell sums — this pixel's own step on each frame, each
            // through ITS settled treatment, in its own cell.
            let shortfall = |side: &SideMultipliers, frame: &[[f32; 3]]| -> [f32; 4] {
                [
                    luma(&frame[i]) - treat(side, share, None, luma(&reference[i])),
                    frame[i][0] - treat(side, share, Some(0), reference[i][0]),
                    frame[i][1] - treat(side, share, Some(1), reference[i][1]),
                    frame[i][2] - treat(side, share, Some(2), reference[i][2]),
                ]
            };
            let (own, moved) = (lab(&rendered[i]), lab(&transported));
            cells[cell_of(i, grid)].add(
                shortfall(&rendered_side, rendered),
                shortfall(&frozen_side, frozen),
                target.zip(target_side.as_ref()).map(|(t, side)| shortfall(side, t)),
                [own[1] - moved[1], own[2] - moved[2]],
            );
        }
        let Some((here, at)) = introduced else {
            continue;
        };
        // The 50% contour inside this band, and the same two far feet the
        // hard ruler stands on, so the slope credit is the same measurement.
        let contour = (band_begin.saturating_sub(1)..(band_end + 1).min(len.saturating_sub(1)))
            .find_map(|q| {
                let (Some(a), Some(b)) = (index(q), index(q + 1)) else {
                    return None;
                };
                (inside(a) != inside(b)).then_some((q, q + 1))
            });
        let slope_feet = contour.and_then(|(low, high)| {
            let high_inside = index(high).map(inside)?;
            let (near_in, near_out) = if high_inside { (high, low) } else { (low, high) };
            let forward = near_in > near_out;
            let (far_in, far_out) = (
                if forward { near_in.checked_add(stand_off)? } else { near_in.checked_sub(stand_off)? },
                if forward { near_out.checked_sub(stand_off)? } else { near_out.checked_add(stand_off)? },
            );
            if far_in >= len || far_out >= len {
                return None;
            }
            let (i_in, i_out) = (index(far_in)?, index(far_out)?);
            (inside(i_in) && !inside(i_out)).then_some(((far_in, far_out), forward))
        });
        let slope_of = |channel: Option<usize>| match slope_feet {
            Some((feet, forward)) => crossing_slope(
                frames,
                weights,
                (start, step, len),
                feet,
                forward,
                channel,
            ),
            None => 0.0,
        };
        let context = |channel: Option<usize>| match channel {
            Some(c) => reference[foot_in][c] - reference[foot_out][c],
            None => luma(&reference[foot_in]) - luma(&reference[foot_out]),
        };
        let slope = slope_of(None);
        let budget = crossing_budget(context(None), slope, ZONE_BOUNDARY_RIM_MAX);
        let (colour_here, channel, colour_at) = coloured.unwrap_or((0.0, 0, at));
        let colour_budget = crossing_budget(
            context(Some(channel)),
            colour_slope_credit(slope_of(Some(channel)), slope),
            ZONE_BOUNDARY_RIM_MAX,
        );
        // The frozen candidate's own shortfall at the same pixels, through
        // ITS settled sky's transport (R37): what the cell's share is of.
        let luma_frozen =
            luma(&frozen[at]) - treat(&frozen_side, owed(at), None, luma(&reference[at]));
        let colour_frozen = std::array::from_fn(|c| {
            frozen[colour_at][c]
                - treat(&frozen_side, owed(colour_at), Some(c), reference[colour_at][c])
        });
        let cell = cell_of(at, grid);
        cells[cell].crossings += 1;
        out.push(CrossingSample {
            luma: here,
            luma_frozen,
            luma_budget: budget,
            colour: colour_here,
            channel,
            colour_budget,
            colour_frozen,
            cell,
        });
    }
}

/// Boundary-continuity reading beside [`local_quality`]. Unlike that
/// mask-weighted in-zone average, this samples ONLY the transition band and
/// compares it with the same band on the UNCORRECTED render, transported
/// through the settled sky's own multiplier ([`boundary_line_rims`]).
///
/// `frozen` is the k=1 candidate held constant through the shrink bisection —
/// the frame the correction's own slope is read from, exactly as
/// [`boundary_step`] reads it.
///
/// The result is a MAGNITUDE at the 90th percentile, ranked exactly as
/// [`boundary_step`] already ranks its own samples and for the reason stated
/// there. Robust to an isolated silhouette highlight, while retaining the
/// systematic bow that repeats along an edge.
///
/// Since R37 the production gates read [`boundary_rim_toward`] and the
/// sub-zone trial rulers [`rim_cell_gaps`]; this no-target form is what the
/// context-budget pins measure.
#[cfg(test)]
pub(super) fn boundary_rim(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    frozen: &[[f32; 3]],
    weights: &[f32],
    width: u32,
    height: u32,
) -> BoundaryReading {
    boundary_rim_toward(None, reference, rendered, frozen, weights, width, height)
}

/// [`boundary_rim`] read toward a paired `target` at the same geometry (R37):
/// what the target itself does at each transition is pooled per evidence
/// cell and the correction is charged only for what it introduces beyond
/// that ([`unasked`], [`cell_share`]). `None` is the rule as it stood.
pub(super) fn boundary_rim_toward(
    target: Option<&[[f32; 3]]>,
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    frozen: &[[f32; 3]],
    weights: &[f32],
    width: u32,
    height: u32,
) -> BoundaryReading {
    let (w, h) = (width as usize, height as usize);
    debug_assert!(target.is_none_or(|t| t.len() == rendered.len()), "one geometry");
    let frames = StepFrames { reference, rendered, frozen, target };
    let (rims, cells) = rim_samples(frames, weights, (w, h));
    reading_of(&rims, &cells, ZONE_BOUNDARY_RIM_MAX)
}

/// Rank one ruler's samples into the numbers a gate and its disclosure need.
/// Separate ORDERINGS on purpose — see [`CrossingSample`]. `ceiling` is the
/// family's own constant, handed in rather than read from a shared one so the
/// two rulers cannot re-tune each other.
///
/// R37: the charge is taken here, after the target's own step has been pooled
/// per evidence cell as a share of the frozen candidate's ([`cell_share`]);
/// each crossing pays for the part of its introduced step beyond `share ×
/// its own frozen step` ([`unasked`]) at its own context rate
/// ([`crossing_charge`]). `asked` is that allowance ranked like `rim`, for
/// the disclosure.
///
/// The COLOUR charge reads the introduced step through a one-code dead band
/// first ([`colour_dead_band`]); luma and the raw `colour` rank do not.
fn reading_of(samples: &[CrossingSample], cells: &[CellSums], ceiling: f32) -> BoundaryReading {
    if samples.is_empty() {
        return BoundaryReading::nothing_measured();
    }
    let share = cell_shares(cells);
    let allowed: Vec<f32> = samples.iter().map(|s| share[0][s.cell] * s.luma_frozen).collect();
    let mut raw: Vec<f32> = samples.iter().map(|s| s.luma).collect();
    let mut charges: Vec<f32> = samples
        .iter()
        .zip(&allowed)
        .map(|(s, &a)| crossing_charge(unasked(s.luma, a), s.luma_budget, ceiling))
        .collect();
    let mut colours: Vec<f32> = samples.iter().map(|s| s.colour.abs()).collect();
    let mut colour_charges: Vec<f32> = samples
        .iter()
        .map(|s| {
            let a = share[s.channel + 1][s.cell] * s.colour_frozen[s.channel];
            crossing_charge(unasked(colour_dead_band(s.colour), a), s.colour_budget, ceiling)
        })
        .collect();
    let mut asked = allowed;
    BoundaryReading {
        rim: magnitude_rank(&mut raw),
        transitions: samples.len(),
        charged: magnitude_rank(&mut charges),
        colour: magnitude_rank(&mut colours),
        colour_charged: magnitude_rank(&mut colour_charges),
        asked: magnitude_rank(&mut asked),
    }
}

/// A single channel's introduced step less one code, toward zero:
/// `sign(v) · max(|v| − BOUNDARY_STEP_FLOOR, 0)`. That step is the difference
/// of two 8-BIT renders, so ±1 code of it is rounding, and charged at the
/// floor budget's rate (ceiling / floor, x3) one code of rounding is the
/// whole ceiling. Measured on the 11-pair battery (fc-v8, 2026-10-22): all 22
/// zone verdicts bound on the colour charge, every one parked exactly at the
/// 0.012 ceiling, by raw colour steps of 1-1.5 codes charged x3 at the floor
/// budget. Luma is the mean of three independent quantisations and resolves
/// sub-code, so it gets no dead band; the raw `colour` rank the disclosure
/// reports is not touched either. Monotone and exactly 0.0 at 0.0, so the
/// shrink bisection keeps its invariant and the k=0 render still reads 0.0.
fn colour_dead_band(v: f32) -> f32 {
    v.signum() * (v.abs() - BOUNDARY_STEP_FLOOR).max(0.0)
}

/// Test window onto [`reading_of`]: one crossing with no target, its colour
/// step `colour` on channel 0 against `colour_budget`, read at the soft
/// family's ceiling — the colour charge it is ranked at.
#[cfg(test)]
pub(super) fn colour_charge_of_one_crossing(colour: f32, colour_budget: f32) -> f32 {
    let sample = CrossingSample {
        luma: 0.0,
        luma_frozen: 0.0,
        luma_budget: ZONE_BOUNDARY_RIM_MAX,
        colour,
        channel: 0,
        colour_budget,
        colour_frozen: [colour; 3],
        cell: 0,
    };
    reading_of(&[sample], &cells_grid(), ZONE_BOUNDARY_RIM_MAX).colour_charged
}

/// Add one scan line's cross-boundary steps to `out`.
///
/// Where [`boundary_line_rims`] reads INSIDE a feathered transition band,
/// this reads ACROSS the mask's 50% contour, so it has something to say about
/// a hard 0/255 raster — the shape `spatial::tile_mask` and the free-mask
/// producer write, whose transition band is empty by construction. That is
/// why the rim ruler returned `rim 0.0` from `0` transitions for every
/// spatial tile ever measured, and why a rectangular seam passed a gate that
/// was reporting a budget it had never been able to test.
///
/// A crossing is a neighbouring pair straddling [`ZONE_BOUNDARY_MID`]. Each
/// contributes ONE difference of DISCONTINUITIES (R40):
///
/// ```text
///     disc(F) = (inside - outside) on F  -  F's own trend over the flanks
///     reading = disc(rendered) - disc(reference)
/// ```
///
/// where the trend is the mean change per 3-px baseline read from each far
/// foot to a flank [`STEP_FLANK_BASELINES`] baselines further out on the
/// same side (a flank the line cannot offer contributes nothing; with
/// neither, the plain step). A gradient, however steep, reads ~0 on every
/// frame; a jump reads its height. `reference` is this same frame rendered
/// WITHOUT the correction under test. A luma step the subject already had
/// at that border — a roof line the mask follows, a horizon a tile edge
/// grazes — appears in both terms and cancels, so scene content cannot
/// false-positive; and because the trend is taken on every frame the same
/// way, the correction's own smooth variation cancels with it, exactly as
/// the plain steps' did. What survives is only the discontinuity the
/// correction introduced, which is the seam itself. The identical reading is
/// taken PER CHANNEL, and the crossing reports the largest of the three
/// magnitudes: a gain set that reproduces a target's mean colour can leave
/// luma601 nearly still while moving one channel several codes across the
/// contour, and a luma-only ruler reads that halo as 0.
///
/// "Inside" is the `>= mid` side, decided by the mask and never by the
/// direction of the scan, so the left and right edges of one brightened tile
/// report the SAME sign instead of cancelling. Both samples must still be on
/// their own side of the contour, which drops a pair straddling a sliver
/// thinner than the stand-off rather than reading a plateau that is not there.
///
/// Each crossing is charged against its own per-crossing budget: the scene's
/// own discontinuity there, `disc(reference)`, clamped between one code and
/// the family's ceiling ([`discontinuity_budget`]). No slope term: `frozen`
/// is still read, but only for the correction's own discontinuity, which the
/// R37 allowance is a share of.
fn boundary_line_steps(
    frames: StepFrames<'_>,
    geometry: &[f32],
    line: ScanLine,
    out: &mut Vec<CrossingSample>,
    cells: &mut [CellSums],
) {
    let StepFrames { reference, rendered, frozen, target } = frames;
    let ScanLine { start, step, len, grid } = line;
    let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let index = |p: usize| -> Option<usize> {
        let i = start + p * step;
        (i < geometry.len() && i < rendered.len() && i < reference.len() && i < frozen.len())
            .then_some(i)
    };
    let stand_off = ZONE_STEP_OFFSET.saturating_sub(1);
    for p in 1..len {
        let (Some(low), Some(high)) = (index(p - 1), index(p)) else {
            continue;
        };
        let inside = |i: usize| geometry[i] >= ZONE_BOUNDARY_MID;
        if inside(low) == inside(high) {
            continue;
        }
        // Walk each foot away from the contour, `stand_off` px in its own
        // direction, then require it to have stayed on its own side.
        let (near_in, near_out) = if inside(high) { (p, p - 1) } else { (p - 1, p) };
        let forward = near_in > near_out;
        let (Some(far_in), Some(far_out)) = (
            if forward { near_in.checked_add(stand_off) } else { near_in.checked_sub(stand_off) },
            if forward { near_out.checked_sub(stand_off) } else { near_out.checked_add(stand_off) },
        ) else {
            continue;
        };
        if far_in >= len || far_out >= len {
            continue;
        }
        let (Some(i_in), Some(i_out)) = (index(far_in), index(far_out)) else {
            continue;
        };
        if !inside(i_in) || inside(i_out) {
            continue;
        }
        // R40. The reading is a DISCONTINUITY: the step across the two feet
        // less the frame's own trend there, the trend being the mean change
        // per baseline over the flanks beyond each far foot
        // ([`STEP_FLANK_BASELINES`] baselines out, each flank still on its own
        // side of the contour; a flank the line cannot offer contributes
        // nothing, and with neither the reading is the plain step). Taken on
        // every frame the same way, so the scene's trend and the correction's
        // cancel in the difference exactly as the plain steps did.
        let flank_len = far_in.abs_diff(far_out) * STEP_FLANK_BASELINES;
        let flank = |foot: usize, inward: bool| -> Option<usize> {
            let p = if forward == inward { foot.checked_add(flank_len) } else { foot.checked_sub(flank_len) }?;
            if p >= len {
                return None;
            }
            let i = index(p)?;
            (inside(i) == inward).then_some(i)
        };
        let flanks = (flank(far_in, true), flank(far_out, false));
        let disc = |frame: &[[f32; 3]], read: &dyn Fn(&[f32; 3]) -> f32| -> f32 {
            let step = read(&frame[i_in]) - read(&frame[i_out]);
            let trends = [
                flanks.0.map(|e| (read(&frame[e]) - read(&frame[i_in])) / STEP_FLANK_BASELINES as f32),
                flanks.1.map(|e| (read(&frame[i_out]) - read(&frame[e])) / STEP_FLANK_BASELINES as f32),
            ];
            let (sum, n) = trends.iter().flatten().fold((0.0f32, 0usize), |(s, n), t| (s + t, n + 1));
            if n == 0 { step } else { step - sum / n as f32 }
        };
        let channel = |c: usize| move |p: &[f32; 3]| p[c];
        let reference_disc = disc(reference, &luma);
        let introduced = disc(rendered, &luma) - reference_disc;
        let budget = discontinuity_budget(reference_disc, ZONE_BOUNDARY_STEP_MAX);
        // The same crossing in colour: the channel that moved most decides
        // the reading, and is charged against ITS OWN context.
        let reference_channels: [f32; 3] = std::array::from_fn(|c| disc(reference, &channel(c)));
        let mut worst = (0.0f32, 0usize);
        for (c, reference_c) in reference_channels.iter().enumerate() {
            let step_c = disc(rendered, &channel(c)) - reference_c;
            if step_c.abs() > worst.0.abs() {
                worst = (step_c, c);
            }
        }
        let (colour_introduced, colour_channel) = worst;
        let colour_budget = discontinuity_budget(reference_channels[colour_channel], ZONE_BOUNDARY_STEP_MAX);
        // Each frame's own discontinuity across the same feet, less the
        // scene's (R37): what that frame carries. The target's is what the
        // cell's share is read from.
        let across = |frame: &[[f32; 3]]| -> [f32; 4] {
            [
                disc(frame, &luma) - reference_disc,
                disc(frame, &channel(0)) - reference_channels[0],
                disc(frame, &channel(1)) - reference_channels[1],
                disc(frame, &channel(2)) - reference_channels[2],
            ]
        };
        let own = across(frozen);
        let (luma_frozen, colour_frozen) = (own[0], [own[1], own[2], own[3]]);
        let lab_a = |p: &[f32; 3]| lab(p)[1];
        let lab_b = |p: &[f32; 3]| lab(p)[2];
        let cell = cell_of(i_in, grid);
        cells[cell].add(
            across(rendered),
            own,
            target.map(across),
            [disc(rendered, &lab_a) - disc(reference, &lab_a), disc(rendered, &lab_b) - disc(reference, &lab_b)],
        );
        cells[cell].crossings += 1;
        out.push(CrossingSample {
            luma: introduced,
            luma_frozen,
            luma_budget: budget,
            colour: colour_introduced,
            channel: colour_channel,
            colour_budget,
            colour_frozen,
            cell,
        });
    }
}

/// Cross-boundary step reading — [`boundary_rim`]'s counterpart for masks
/// with no transition band to read into.
///
/// `geometry` is the mask's OWN alpha at analysis size, i.e. what the
/// renderer applies, and never the estimator weights: those are that alpha
/// times the zone's per-bin evidence verdicts, so their 50% contour is
/// punched full of interior holes that are not boundaries at all.
///
/// The result is a MAGNITUDE, ranked exactly as `range::range_transition_rim`
/// already ranks its own signed samples ([`magnitude_rank`]).
///
/// Since R37 the production gates read [`boundary_step_toward`]; this no-target
/// form is what the context-budget pins measure.
#[cfg(test)]
pub(super) fn boundary_step(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    frozen: &[[f32; 3]],
    geometry: &[f32],
    width: u32,
    height: u32,
) -> BoundaryReading {
    boundary_step_toward(None, reference, rendered, frozen, geometry, width, height)
}

/// [`boundary_step`] read toward a paired `target` (R37) — see
/// [`boundary_rim_toward`]; the same allowance, pooled on the same cells.
pub(super) fn boundary_step_toward(
    target: Option<&[[f32; 3]]>,
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    frozen: &[[f32; 3]],
    geometry: &[f32],
    width: u32,
    height: u32,
) -> BoundaryReading {
    let (w, h) = (width as usize, height as usize);
    debug_assert!(target.is_none_or(|t| t.len() == rendered.len()), "one geometry");
    let frames = StepFrames { reference, rendered, frozen, target };
    let (mut steps, mut cells) = (Vec::new(), cells_grid());
    for y in 0..h {
        boundary_line_steps(frames, geometry, ScanLine::row(y, (w, h)), &mut steps, &mut cells);
    }
    for x in 0..w {
        boundary_line_steps(frames, geometry, ScanLine::column(x, (w, h)), &mut steps, &mut cells);
    }
    reading_of(&steps, &cells, ZONE_BOUNDARY_STEP_MAX)
}

/// R37. The sub-zone trial rulers' reading: the render's transition-band
/// gap against `reference` — the paired target, for those rulers — as a
/// MEAN per evidence cell (`CELLS_X` x `CELLS_Y` entries, 0 where a cell has
/// no band pixel): `[|luma|, widest channel, Lab chroma / 100]`. The same
/// scan as [`boundary_rim_toward`]; means where that ranks, because the
/// target's re-synthesised texture is zero-mean noise per pixel and a rank
/// of it is a coin flip ([`CellSums`]).
pub(crate) fn rim_cell_gaps(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    weights: &[f32],
    width: u32,
    height: u32,
) -> Vec<[f32; 3]> {
    let (w, h) = (width as usize, height as usize);
    let frames = StepFrames { reference, rendered, frozen: rendered, target: None };
    let (_, cells) = rim_samples(frames, weights, (w, h));
    cell_gaps(&cells)
}

/// [`rim_cell_gaps`]' hard-family counterpart: the render's cross-contour
/// step against `reference`'s, per cell ([`boundary_line_steps`]).
pub(crate) fn step_cell_gaps(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    geometry: &[f32],
    width: u32,
    height: u32,
) -> Vec<[f32; 3]> {
    let (w, h) = (width as usize, height as usize);
    let frames = StepFrames { reference, rendered, frozen: rendered, target: None };
    let (mut steps, mut cells) = (Vec::new(), cells_grid());
    for y in 0..h {
        boundary_line_steps(frames, geometry, ScanLine::row(y, (w, h)), &mut steps, &mut cells);
    }
    for x in 0..w {
        boundary_line_steps(frames, geometry, ScanLine::column(x, (w, h)), &mut steps, &mut cells);
    }
    cell_gaps(&cells)
}
