//! The one control-validity law (v1.18.0, rel1180 design-control-validity-1180):
//! a control class may move a population only as far as that population's own
//! evidence asks — read per CELL, and the verdict is that the class leaves no
//! cell WORSE on the class's own axes than it found it.
//!
//! * A **class** is the controls one physical operation covers, read between
//!   the candidate WITHOUT the class and the candidate's render
//!   ([`super::joint::without_class`], 2026-11-09): the tone (exposure, the five sliders,
//!   the master curve), the white balance (Kelvin, tint), the chroma dials
//!   (saturation, vibrance, the mixer, the four wheels), the channel curves.
//!   The detail controls (clarity, texture) belong to no class and are not
//!   judged; they sit in both frames of every class, so a texture's troughs
//!   in the cells' brightness medians cancel (they were the tone's until
//!   2026-11-08).
//! * A **cell** is the pixels a class cannot act on separately — the evidence
//!   model's own partition of the SOURCE pixels, brightness range
//!   ([`evidence_luma_bin`], 17) × hue band ([`evidence_hue_band`], 8, and an
//!   achromatic ninth), MERGED along the axis the class cannot tell apart
//!   (user ruling 2026-11-09, [`Cells`]): the tone on the brightness ranges,
//!   the white balance on the hue bands, the chroma dials on hue band × the
//!   grade wheels' three regions, the channel curves (which act per channel
//!   and do tell hues apart) on the fine grid. A cell has standing when it carries
//!   [`EVIDENCE_MIN_SHARE`] of the presence mass and [`SUPPORT_MIN_PIXELS`]
//!   pixels; the units of a grid short of that are POOLED along brightness
//!   until the pool has it (J145, [`Cells`]), so a population spread over
//!   brightness is still heard. The judge weights are the presence weights
//!   (R11).
//! * The **axes** are the class's own: brightness as ln linear luma (the tone,
//!   the chroma dials, the curves); the white balance's Kelvin and tint as
//!   `ln R − ln B` and `ln G − (ln R + ln B)/2` on linear RGB (the axes
//!   `wb_axes_by_range` reads); and for the chroma dials and the curves the
//!   sRGB chroma (max − min) as a Cartesian vector at the HSL hue. Polar
//!   (chroma, hue) coordinates were tried and rejected: at the achromatic axis
//!   a warming flips a near-neutral sky's hue by 150° while moving it by two
//!   codes, and the controls judged here act as vectors there (J47).
//! * The **ask** is what the cell's evidence wants on an axis. Where the pixel
//!   pairs ([`EvidenceModel::paired`]) it is the weighted median of
//!   target − before over the cell's paired pixels; where the frame does not
//!   pair it is the quantile-matched change between the before and target
//!   populations (brightness frame-wide; the colour axes per brightness
//!   range of the before frame, hue bands collapsed — the blind model's own
//!   doctrine, the split J* already makes). Its error is the median's own
//!   ([`median_se`]) with one output code's quantisation at the cell's level
//!   ([`code_step_variance`]) in quadrature — the white-balance estimator's
//!   own error, rel1180 RULING-1180-sc-r2.
//! * The **move** is the weighted median of the class's per-pixel change on
//!   the axis between its two frames, with its own error read the same way.
//! * The **verdict**: `|move − ask| ≤ |ask| + se` on a scalar axis, the
//!   Euclidean distance on a vector pair, `se` both errors in quadrature. A
//!   move no cell asked for (the violet sky under a tint fitted to the rocks,
//!   the sky warmed to 17 400 K while asking to cool, a slice fanned against
//!   its neighbours, a sky rotated into gold) leaves that cell farther from its
//!   evidence than it started; a compromise between cells that all end nearer
//!   (a band saturation that overshoots one brightness range by a quarter and
//!   undershoots the next) is the right control with an imperfect structure.
//!   Read on the NumPy prototype over the real pairs and the fixtures
//!   (rel1180 int/validity, 2026-11-07) before this was written. Two readings
//!   rejected there: the interval `[0, ask] ± se` (it refused every class of
//!   every real recipe — the coherent coast's curves on 37 cells — because no
//!   parametric control matches 17 × 9 medians to half a percent) and a hull
//!   over the cells' asks (it admits a white balance that warms a sky asking to
//!   cool because the ground asked the warming — the p39 sickness of
//!   2026-11-01).
//! * **No evidence**: a cell with standing but no ask (no paired counterpart
//!   and no target population in its brightness range) may not be moved by
//!   more than [`UNSUPPORTED_RANGE_MOVE`] under [`VetoPolicy::Withhold`]; under
//!   `Disclose` it is moved and named.
//! * **Contrast** (J154, 2026-11-13): a population ask speaks for where a
//!   brightness range sits, not for the range's contrast. Rank matching
//!   maps a dense range onto a spread one at a slope that is the ratio of
//!   the two densities, and where the content was replaced (a flat sky the
//!   target paints clouds over) that ratio is the new content's spread, not
//!   an edit: a tone curve that follows it amplifies the flat sky's own
//!   ripple. So the tone is also read on what its SHAPE — the contrast, the
//!   four range sliders, the master curve — does to a range whose brightness
//!   ask is the population's ([`Cells::stretched`]): the slope of the
//!   members' display luma on the candidate against their display luma on
//!   the candidate without the shape, its exposure kept. Above 1 by more
//!   than its error the range was stretched with no evidence for it:
//!   refused under `Withhold`, shipped and named under `Disclose`. The
//!   exposure is in both frames — what a level move does to a range's
//!   display contrast is its own consequence, either way. A range that
//!   pairs is not read here (its pixels' own distance reads its contrast
//!   in J*), and compressing a range is not read at all.
//!
//! This replaces, as code, the unsupported-range vetoes, the white balance's
//! foreign-hue and rotation gates and the 2026-11-01 shared-component rule,
//! and the four cast gates (ratio, foreign hue, rotation, fan): each was one
//! reading of this law on one class.

use super::evidence::{EVIDENCE_LUMA_BINS, EVIDENCE_MIN_SHARE, EvidenceModel, UNSUPPORTED_RANGE_MOVE, code_step_variance, evidence_hue_band, evidence_luma_bin, median_se, weighted_median};
use super::pixels::luma601;
use super::tone::SUPPORT_MIN_PIXELS;
use crate::render;
use rayon::prelude::*;

/// A pixel darker than this on every channel does not testify on the white
/// balance's axes (the dark pixels' log gains are noise; [`super::evidence::WB_CHROMA_FLOOR`]).
const WB_AXES_FLOOR: f32 = 0.04;

/// The class a verdict is about, in the order the stages compose the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Tone,
    WhiteBalance,
    Chroma,
    Curves,
}

/// One axis the law reads a class on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Axis {
    /// ln linear luma.
    Luma,
    /// The chroma vector (a, b): sRGB chroma at the HSL hue.
    ChromaVector,
    /// The white balance's (Kelvin, tint) log-gain pair.
    WbVector,
}

impl Class {
    /// The class's slot in the per-class cell grids ([`Cells::grid`]).
    fn index(self) -> usize {
        match self {
            Class::Tone => 0,
            Class::WhiteBalance => 1,
            Class::Chroma => 2,
            Class::Curves => 3,
        }
    }

    fn axes(self) -> &'static [Axis] {
        match self {
            Class::Tone => &[Axis::Luma],
            Class::WhiteBalance => &[Axis::WbVector],
            Class::Chroma => &[Axis::Luma, Axis::ChromaVector],
            Class::Curves => &[Axis::Luma, Axis::ChromaVector],
        }
    }
}

/// A frame's coordinates per pixel: `l` ln linear luma, `(a, b)` the chroma
/// vector, `(k, t)` the white balance's axes (NaN under the floor), `level`
/// the display luma the code step is read at.
struct Coords {
    l: Vec<f32>,
    a: Vec<f32>,
    b: Vec<f32>,
    k: Vec<f32>,
    t: Vec<f32>,
    level: Vec<f32>,
}

impl Coords {
    /// The five coordinates as columns, in the slot order the axes read.
    fn columns(&self) -> [&Vec<f32>; 5] {
        [&self.l, &self.a, &self.b, &self.k, &self.t]
    }
}

fn coords_of(px: &[[f32; 3]]) -> Coords {
    let rows: Vec<[f32; 6]> = px
        .par_iter()
        .map(|p| {
            let lin = [p[0], p[1], p[2]].map(|c| render::srgb_to_linear(c.clamp(0.0, 1.0)).max(1e-5));
            let l = luma601(&lin).max(1e-5).ln();
            let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
            let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
            let h = h * std::f32::consts::TAU;
            let (k, t) = if p[0].max(p[1]).max(p[2]) < WB_AXES_FLOOR {
                (f32::NAN, f32::NAN)
            } else {
                let (lr, lg, lb) = (lin[0].ln(), lin[1].ln(), lin[2].ln());
                (lr - lb, lg - 0.5 * (lr + lb))
            };
            [l, chroma * h.cos(), chroma * h.sin(), k, t, luma601(p)]
        })
        .collect();
    Coords {
        l: rows.iter().map(|r| r[0]).collect(),
        a: rows.iter().map(|r| r[1]).collect(),
        b: rows.iter().map(|r| r[2]).collect(),
        k: rows.iter().map(|r| r[3]).collect(),
        t: rows.iter().map(|r| r[4]).collect(),
        level: rows.iter().map(|r| r[5]).collect(),
    }
}

/// One output code's variance on an axis at display level `level`: the log
/// step for the log axes, 1/255 of chroma for the chroma vector.
fn code_variance(axis: Axis, level: f32) -> f32 {
    match axis {
        Axis::Luma | Axis::WbVector => code_step_variance(level),
        Axis::ChromaVector => (1.0 / 255.0f32).powi(2) / 12.0,
    }
}

/// A weighted median and its error on one scalar coordinate: the median's
/// own ([`median_se`]) and one code's quantisation in quadrature. `None`
/// under [`SUPPORT_MIN_PIXELS`] samples.
fn reading(samples: &mut Vec<(f32, f32)>, axis: Axis, level: f32) -> Option<(f32, f32)> {
    samples.retain(|&(v, w)| v.is_finite() && w > 0.0);
    if (samples.len() as f32) < SUPPORT_MIN_PIXELS {
        return None;
    }
    let m = weighted_median(samples);
    let se = median_se(samples, m);
    Some((m, (se * se + code_variance(axis, level)).sqrt()))
}

/// The contrast a transfer gave a set of pixels: the least-squares slope of
/// their display luma on `after` against their display luma on `before`, at
/// the weights `w` — 1 where the range's contrast was kept, above 1 where it
/// was stretched — and its error: the regression's own, and the range's
/// extent read to one output code at each end, in quadrature (a rise of
/// under a code across the range is not one the frame shows). `None` under
/// [`SUPPORT_MIN_PIXELS`] weighted members, or where the range is flat
/// before (a spread under half a code: nothing a transfer could stretch).
fn contrast_gain(before: &[[f32; 3]], after: &[[f32; 3]], w: &[f32], members: &[u32]) -> Option<(f32, f32)> {
    let read = |i: u32| {
        let i = i as usize;
        (luma601(&before[i]) as f64, luma601(&after[i]) as f64, w[i] as f64)
    };
    let heard = |(x, y, wi): (f64, f64, f64)| wi > 0.0 && x.is_finite() && y.is_finite();
    let (mut n, mut sw, mut sww, mut sx, mut sy) = (0usize, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (x, y, wi) in members.iter().map(|&i| read(i)).filter(|&s| heard(s)) {
        n += 1;
        sw += wi;
        sww += wi * wi;
        sx += wi * x;
        sy += wi * y;
    }
    if (n as f32) < SUPPORT_MIN_PIXELS {
        return None;
    }
    let (mx, my) = (sx / sw, sy / sw);
    let (mut vxx, mut vxy, mut vyy) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y, wi) in members.iter().map(|&i| read(i)).filter(|&s| heard(s)) {
        vxx += wi * (x - mx) * (x - mx);
        vxy += wi * (x - mx) * (y - my);
        vyy += wi * (y - my) * (y - my);
    }
    let (vxx, vxy, vyy) = (vxx / sw, vxy / sw, vyy / sw);
    let code = 1.0f64 / 255.0;
    if vxx.sqrt() < 0.5 * code {
        return None;
    }
    let gain = vxy / vxx;
    // The regression's own error, at the weights' effective count.
    let effective = (sw * sw / sww).max(3.0);
    let regression = (vyy - gain * vxy).max(0.0) / (vxx * (effective - 2.0));
    // The extent — a uniform range of variance v is sqrt(12 v) wide — read
    // to one code at each end (a code's quantisation variance, 1/12 code^2).
    let ends = 2.0 * code * code / 12.0 / (12.0 * vxx);
    Some((gain as f32, (regression + ends).sqrt() as f32))
}

/// Do the members with a population ask carry the MAJORITY of the weight of
/// the members the axis can be read on? `samples` are the cell's population
/// asks on one slot `(ask, weight)`, NaN where the member's brightness range
/// has no target population; a member is readable where its before
/// coordinate `before` is finite and its population weight `pw` is positive.
fn population_majority(samples: &[(f32, f32)], members: &[u32], before: &[f32], pw: &dyn Fn(usize) -> f32) -> bool {
    let asked: f64 = samples.iter().filter(|&&(v, w)| v.is_finite() && w > 0.0).map(|&(_, w)| w as f64).sum();
    let readable: f64 = members
        .iter()
        .map(|&i| i as usize)
        .filter(|&i| before[i].is_finite() && pw(i) > 0.0)
        .map(|i| pw(i) as f64)
        .sum();
    asked >= 0.5 * readable
}

/// Quantile-matched target value for every source sample (the population
/// ask): source sample `i`'s weighted quantile, read off the target's weighted
/// quantile function. Both populations sorted once.
fn quantile_map(src: &[(f32, f32)], tgt: &[(f32, f32)]) -> Vec<f32> {
    let mut so: Vec<usize> = (0..src.len()).collect();
    so.sort_by(|&i, &j| src[i].0.total_cmp(&src[j].0));
    let total_s: f64 = src.iter().map(|&(_, w)| w as f64).sum();
    let mut sq = vec![0.0f32; src.len()];
    let mut carried = 0.0f64;
    for &i in &so {
        let w = src[i].1 as f64;
        sq[i] = ((carried + 0.5 * w) / total_s.max(1e-12)) as f32;
        carried += w;
    }
    let mut to: Vec<(f32, f32)> = tgt.to_vec();
    to.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total_t: f64 = to.iter().map(|&(_, w)| w as f64).sum();
    let mut tq = Vec::with_capacity(to.len());
    let mut carried = 0.0f64;
    for &(_, w) in &to {
        tq.push(((carried + 0.5 * w as f64) / total_t.max(1e-12)) as f32);
        carried += w as f64;
    }
    sq.iter()
        .map(|&q| {
            let j = tq.partition_point(|&x| x < q);
            if j == 0 {
                to[0].0
            } else if j >= to.len() {
                to[to.len() - 1].0
            } else {
                let (q0, q1) = (tq[j - 1], tq[j]);
                let f = if q1 > q0 { (q - q0) / (q1 - q0) } else { 0.0 };
                to[j - 1].0 + f * (to[j].0 - to[j - 1].0)
            }
        })
        .collect()
}

/// One cell of a class's grid: the source pixels the class cannot act on
/// separately, its label for a trace or a disclosure, its members, its
/// presence mass.
#[derive(Clone, Debug)]
pub(super) struct Cell {
    label: String,
    members: Vec<u32>,
    pub(super) mass: f32,
}

impl Cell {
    /// The cell's name for a trace or a disclosure.
    pub(super) fn name(&self) -> String {
        self.label.clone()
    }
}

/// One refused (cell, axis): the class's move against the cell's ask.
#[derive(Clone, Debug)]
pub(super) struct Refused {
    pub(super) cell: usize,
    pub(super) axis: Axis,
    /// The move's size (a vector's length) and the ask's.
    pub(super) moved: f32,
    pub(super) ask: f32,
    /// Both readings' errors in quadrature.
    pub(super) se: f32,
    pub(super) paired: bool,
    /// The cosine between the move and the ask (±1 on a one-dimensional
    /// axis; 0 where either has no length): the trace's reading of
    /// DIRECTION, which the two lengths alone do not carry.
    pub(super) cos: f32,
}

/// A class's verdict between its two frames.
#[derive(Clone, Debug, Default)]
pub(super) struct Verdict {
    pub(super) refused: Vec<Refused>,
    /// Cells with standing and no ask that the class moved by more than
    /// [`UNSUPPORTED_RANGE_MOVE`].
    pub(super) blind: Vec<usize>,
    /// The tone's brightness ranges a population asks whose contrast its
    /// shape raised ([`Cells::stretched`]). Filled by the reader that has
    /// the frame without the shape (`joint::Joint::class_refused`);
    /// [`Cells::verdict`] leaves it empty.
    pub(super) stretched: Vec<Stretched>,
}

impl Verdict {
    pub(super) fn admits(&self, withhold: bool) -> bool {
        self.refused.is_empty() && (!withhold || (self.blind.is_empty() && self.stretched.is_empty()))
    }
}

/// One brightness range the tone's shape stretched with no evidence for it:
/// the contrast it gave the range ([`contrast_gain`]) and that reading's error.
#[derive(Clone, Debug)]
pub(super) struct Stretched {
    pub(super) cell: usize,
    pub(super) gain: f32,
    pub(super) se: f32,
}

/// One axis's ask in one cell: the ask per slot `(value, se)` and whether it
/// was read on the paired pixels.
struct AxisAsk {
    axis: Axis,
    asks: Vec<(f32, f32)>,
    paired: bool,
}

/// The asks of one class on one before frame ([`Cells::asks`]): the frame's
/// pixels and coordinates, and per cell the axes with an ask.
pub(super) struct ClassAsks {
    class: Class,
    before_px: Vec<[f32; 3]>,
    before: Coords,
    per_cell: Vec<Vec<AxisAsk>>,
}

/// The slots of the coordinate columns an axis reads.
fn slots(axis: Axis) -> &'static [usize] {
    match axis {
        Axis::Luma => &[0],
        Axis::ChromaVector => &[1, 2],
        Axis::WbVector => &[3, 4],
    }
}

/// A cell's display level: the weighted mean of its members' display luma
/// on the before frame (0.5 where the cell has no mass).
fn cell_level(before: &Coords, w: &[f32], members: &[u32]) -> f32 {
    let (mut s, mut m) = (0.0f64, 0.0f64);
    for &i in members {
        let i = i as usize;
        s += (before.level[i] * w[i]) as f64;
        m += w[i] as f64;
    }
    if m > 0.0 { (s / m) as f32 } else { 0.5 }
}

/// The cells of one pair and the target's coordinates, built once per solve.
/// The cells are PER CLASS (user ruling 2026-11-09): a class's cell is the
/// set of source pixels that class cannot act on separately, so the fine
/// partition — brightness range ([`evidence_luma_bin`], 17) × hue band
/// ([`evidence_hue_band`], 8, and an achromatic ninth) — is MERGED along the
/// axis the class cannot tell apart. The tone (exposure, the sliders, the
/// master curve) acts by brightness and cannot tell hues apart at one
/// brightness: its cells are the 17 brightness ranges. The white balance is
/// one gain pair: it cannot tell brightnesses apart, and its cells are the
/// nine hue bands. The chroma dials act per hue band (the mixer) and per
/// tonal region (the three grade wheels): their cells are hue band × the
/// render's own three grade regions (`render::color_grade_regions` at the
/// default grade). The channel curves act per channel and so DO tell hues
/// apart at one brightness: their cells stay the fine grid. Until then every
/// class was read on the fine grid, and a tone was refused because the
/// aqua, green and neutral pixels of ONE brightness range asked 0 / +0.05 /
/// +0.44 of a control that cannot tell them apart (the haze pair, gates/j97);
/// the hue-dependent remainder is the mixer's to explain, on its own cells.
pub(super) struct Cells {
    /// One grid per class, in [`Class::index`] order.
    grids: [Vec<Cell>; 4],
    /// The judge weights (presence) and the pairing, per pixel.
    w: Vec<f32>,
    paired: Vec<bool>,
    /// Does any pixel with mass not pair? Then the population asks are read.
    any_unpaired: bool,
    /// The pairing's reliability r as J* reads it ([`super::look`], J91): a
    /// paired pixel's per-pixel term weighs r, its population term 1 − r.
    /// The law decides each cell by the same weights.
    reliability: f32,
    target: Coords,
    /// The target pixels' brightness ranges, for the population asks.
    target_bin: Vec<u8>,
}

impl Cells {
    /// The cells of a pair: every pixel the evidence model holds PRESENT has
    /// standing, at its presence weight, and the pairing is the evidence
    /// model's. A scoped model's presence is its population's membership, so
    /// on an Atmosphere solve judged on a readable shared-content population
    /// (R30 R2, `evidence::shared_content_pairing`: `tp` the
    /// correspondence-remapped target) the pixels outside it are SILENT — no
    /// standing, no ask — the replaced content the robust controls were
    /// never solved from (an invented half asks for a scene, not an edit,
    /// and no global control can be judged on it). ONE construction for both
    /// (2026-11-10): the restricted cells used to take membership for
    /// pairing while J* read the model's flags, and a field that dropped
    /// nothing moved a dial.
    pub(super) fn new(sp: &[[f32; 3]], tp: &[[f32; 3]], evidence: &EvidenceModel) -> Self {
        Self::build(sp, tp, evidence)
    }

    /// The class's cells.
    pub(super) fn grid(&self, class: Class) -> &[Cell] {
        &self.grids[class.index()]
    }

    /// The cells of every class, counted (a reading for the probe).
    #[cfg(test)]
    pub(super) fn count(&self) -> usize {
        self.grids.iter().map(Vec::len).sum()
    }

    fn build(sp: &[[f32; 3]], tp: &[[f32; 3]], evidence: &EvidenceModel) -> Self {
        let n = sp.len().min(tp.len());
        let w: Vec<f32> =
            (0..n).map(|i| evidence.source_presence_weights.get(i).copied().unwrap_or(0.0).max(0.0)).collect();
        let paired: Vec<bool> = (0..n).map(|i| evidence.paired.get(i).copied().unwrap_or(false)).collect();
        let total: f64 = w.iter().map(|&x| x as f64).sum();
        // The fine partition: brightness range × (achromatic + 8 hue bands).
        let mut fine: Vec<Vec<u32>> = vec![Vec::new(); EVIDENCE_LUMA_BINS * 9];
        for i in 0..n {
            if w[i] <= 0.0 {
                continue;
            }
            let bin = evidence_luma_bin(luma601(&sp[i]));
            let band = evidence_hue_band(&sp[i]).map(|b| b + 1).unwrap_or(0);
            fine[bin * 9 + band].push(i as u32);
        }
        let range_label = |b0: usize, b1: usize| {
            format!("luma[{:.2}-{:.2}]", b0 as f32 / EVIDENCE_LUMA_BINS as f32, (b1 + 1) as f32 / EVIDENCE_LUMA_BINS as f32)
        };
        let band_label = |band: usize| if band == 0 { "neutral".to_string() } else { crate::recipe::HSL_BANDS[band - 1].to_string() };
        let mass_of = |members: &[u32]| -> f32 { members.iter().map(|&i| w[i as usize]).sum() };
        let standing = |members: &[u32]| -> bool {
            (members.len() as f32) >= SUPPORT_MIN_PIXELS
                && total > 0.0
                && (mass_of(members) as f64) / total >= EVIDENCE_MIN_SHARE as f64
        };
        // The units of ONE population along brightness, pooled where they
        // have no standing alone (J145, 2026-11-12): a unit with standing is
        // its own cell; the units without are pooled in brightness order —
        // neighbours first — and each pool is a cell as soon as it has
        // standing, a last pool short of it joining the pool before it. Only
        // a population whose units without standing stay under it TOGETHER
        // has no cell for them. Until then a unit without standing was not
        // read at all, and a population SPREAD over brightness had no voice
        // on the finest grid: the canyon toy's grey ramp is an eighth of the
        // frame, 0.8 % in each of fifteen ranges; the channel curves that
        // carried the rocks and the sky stained it by 19.6 codes of 255 (it
        // matched the target to begin with; v1.17.1 left it at 9.2), and the
        // law admitted them on every cell it could hear (gates/j145).
        // `units`: (first range, last range, members), in brightness order.
        let pooled = |units: Vec<(usize, usize, Vec<u32>)>| -> Vec<(usize, usize, Vec<u32>)> {
            let mut out: Vec<(usize, usize, Vec<u32>)> = Vec::new();
            let mut pool: Option<(usize, usize, Vec<u32>)> = None;
            let mut last_pool: Option<usize> = None;
            for (b0, b1, members) in units {
                if standing(&members) {
                    out.push((b0, b1, members));
                    continue;
                }
                if members.is_empty() {
                    continue;
                }
                let open = pool.get_or_insert_with(|| (b0, b1, Vec::new()));
                open.1 = b1;
                open.2.extend(members);
                if standing(&open.2) {
                    last_pool = Some(out.len());
                    out.extend(pool.take());
                }
            }
            if let (Some((_, b1, members)), Some(at)) = (pool, last_pool) {
                out[at].1 = out[at].1.max(b1);
                out[at].2.extend(members);
            }
            out.sort_by_key(|unit| unit.0);
            out
        };
        let cell = |label: String, members: Vec<u32>| -> Cell {
            let mass = mass_of(&members);
            Cell { label, members, mass }
        };
        // The grade wheels' region of a brightness range: the render's own
        // split at the default grade, read at the range's centre.
        let region_of = |bin: usize| -> usize {
            let centre = (bin as f32 + 0.5) / EVIDENCE_LUMA_BINS as f32;
            let w3 = render::color_grade_regions(&crate::recipe::ColorGrade::default(), centre);
            (0..3).max_by(|&a, &b| w3[a].total_cmp(&w3[b])).unwrap_or(1)
        };
        let region_name = ["shadows", "midtones", "highlights"];
        // Tone: per brightness range, the hue bands merged.
        let by_range: Vec<Cell> = pooled(
            (0..EVIDENCE_LUMA_BINS)
                .map(|bin| (bin, bin, (0..9).flat_map(|band| fine[bin * 9 + band].iter().copied()).collect()))
                .collect(),
        )
        .into_iter()
        .map(|(b0, b1, members)| cell(range_label(b0, b1), members))
        .collect();
        // White balance: per hue band, the brightness ranges merged (one unit
        // per population: nothing to pool along).
        let by_band: Vec<Cell> = (0..9)
            .map(|band| (band, (0..EVIDENCE_LUMA_BINS).flat_map(|bin| fine[bin * 9 + band].iter().copied()).collect::<Vec<u32>>()))
            .filter(|(_, members)| standing(members))
            .map(|(band, members)| cell(band_label(band), members))
            .collect();
        // Chroma dials: hue band × grade region, a band's regions pooled.
        let region_bins: Vec<Vec<usize>> =
            (0..3).map(|region| (0..EVIDENCE_LUMA_BINS).filter(|&bin| region_of(bin) == region).collect()).collect();
        let region_label = |b0: usize, b1: usize| {
            let (r0, r1) = (region_of(b0), region_of(b1));
            if r0 == r1 { region_name[r0].to_string() } else { format!("{}+{}", region_name[r0], region_name[r1]) }
        };
        let mut by_region_band: Vec<(usize, usize, Cell)> = Vec::new();
        for band in 0..9 {
            let units: Vec<(usize, usize, Vec<u32>)> = region_bins
                .iter()
                .filter_map(|bins| {
                    let (b0, b1) = (*bins.first()?, *bins.last()?);
                    Some((b0, b1, bins.iter().flat_map(|&bin| fine[bin * 9 + band].iter().copied()).collect()))
                })
                .collect();
            for (b0, b1, members) in pooled(units) {
                let label = format!("{} {}/{}", region_label(b0, b1), range_label(b0, b1), band_label(band));
                by_region_band.push((b0, band, cell(label, members)));
            }
        }
        by_region_band.sort_by_key(|(b0, band, _)| (*b0, *band));
        let by_region_band: Vec<Cell> = by_region_band.into_iter().map(|(_, _, c)| c).collect();
        // Channel curves: the fine grid — a curve acts per channel, so at one
        // brightness it does tell hues apart (a red lift warms the rocks and
        // turns a pale sky violet, the sickness the law refuses on the sky's
        // own cell) — a hue band's ranges pooled.
        let mut fine_cells: Vec<(usize, usize, Cell)> = Vec::new();
        for band in 0..9 {
            let units: Vec<(usize, usize, Vec<u32>)> =
                (0..EVIDENCE_LUMA_BINS).map(|bin| (bin, bin, fine[bin * 9 + band].clone())).collect();
            for (b0, b1, members) in pooled(units) {
                fine_cells.push((b0, band, cell(format!("{}/{}", range_label(b0, b1), band_label(band)), members)));
            }
        }
        fine_cells.sort_by_key(|(b0, band, _)| (*b0, *band));
        let fine_cells: Vec<Cell> = fine_cells.into_iter().map(|(_, _, c)| c).collect();
        let grids = [by_range, by_band, by_region_band, fine_cells];
        let target = coords_of(&tp[..n]);
        let target_bin = tp[..n].iter().map(|p| evidence_luma_bin(luma601(p)) as u8).collect();
        let any_unpaired = (0..n).any(|i| w[i] > 0.0 && !paired[i]);
        // The pairing's reliability as J* reads it (J91): the presence-weighted
        // ρ² of L* over the paired pixels of the base frame.
        let lab = super::look::lab_of(&tp[..n]);
        let reliability = super::look::pairing_reliability(&sp[..n], &lab, n, &|i| w[i], &|i| paired[i]);
        Cells { grids, w, paired, any_unpaired, reliability, target, target_bin }
    }

    /// The population asks per pixel for the unpaired pixels (NaN where the
    /// pixel's brightness range has no target population): brightness
    /// frame-wide, the other coordinates per brightness range of `before`.
    fn population_asks(&self, before: &Coords, axes: &[Axis]) -> [Vec<f32>; 5] {
        let n = self.w.len();
        let nan = || vec![f32::NAN; n];
        let mut out = [nan(), nan(), nan(), nan(), nan()];
        // The population J* reads: every pixel at its distribution weight —
        // w(1 − r) where it pairs, w where it does not (`look::dist_weights`).
        let pw = |i: usize| if self.paired[i] { self.w[i] * (1.0 - self.reliability) } else { self.w[i] };
        let unpaired: Vec<usize> = (0..n).filter(|&i| pw(i) > 0.0).collect();
        if unpaired.is_empty() {
            return out;
        }
        if axes.contains(&Axis::Luma) {
            let src: Vec<(f32, f32)> = unpaired.iter().map(|&i| (before.l[i], pw(i))).collect();
            let tgt: Vec<(f32, f32)> = unpaired.iter().map(|&i| (self.target.l[i], pw(i))).collect();
            for (k, v) in quantile_map(&src, &tgt).into_iter().enumerate() {
                out[0][unpaired[k]] = v - before.l[unpaired[k]];
            }
        }
        let colour: Vec<(usize, &Vec<f32>, &Vec<f32>)> = match (axes.contains(&Axis::ChromaVector), axes.contains(&Axis::WbVector)) {
            (true, false) => vec![(1, &before.a, &self.target.a), (2, &before.b, &self.target.b)],
            (false, true) => vec![(3, &before.k, &self.target.k), (4, &before.t, &self.target.t)],
            _ => Vec::new(),
        };
        if colour.is_empty() {
            return out;
        }
        let bin_before: Vec<usize> = unpaired.iter().map(|&i| evidence_luma_bin(before.level[i])).collect();
        for bin in 0..EVIDENCE_LUMA_BINS {
            let s: Vec<usize> = unpaired.iter().zip(&bin_before).filter(|&(_, &b)| b == bin).map(|(&i, _)| i).collect();
            let t: Vec<usize> = unpaired.iter().copied().filter(|&i| self.target_bin[i] as usize == bin).collect();
            if (s.len() as f32) < SUPPORT_MIN_PIXELS || (t.len() as f32) < SUPPORT_MIN_PIXELS {
                continue;
            }
            for &(slot, bv, tv) in &colour {
                let src: Vec<(f32, f32)> = s.iter().filter(|&&i| bv[i].is_finite()).map(|&i| (bv[i], pw(i))).collect();
                let tgt: Vec<(f32, f32)> = t.iter().filter(|&&i| tv[i].is_finite()).map(|&i| (tv[i], pw(i))).collect();
                if (src.len() as f32) < SUPPORT_MIN_PIXELS || (tgt.len() as f32) < SUPPORT_MIN_PIXELS {
                    continue;
                }
                let mapped = quantile_map(&src, &tgt);
                let mut k = 0;
                for &i in &s {
                    if bv[i].is_finite() {
                        out[slot][i] = mapped[k] - bv[i];
                        k += 1;
                    }
                }
            }
        }
        out
    }

    /// The class's verdict between its frames `before` and `after`: the asks
    /// read on `before` ([`Cells::asks`]), the moves read against `after`
    /// ([`Cells::verdict`]).
    pub(super) fn judge(&self, class: Class, before: &[[f32; 3]], after: &[[f32; 3]]) -> Verdict {
        self.verdict(&self.asks(class, before), after)
    }

    /// The ASKS of `class` on the frame `before`: per cell with standing, per
    /// axis the class reads, the ask per slot with its error — the paired
    /// pixels' weighted median of target − before where the cell pairs, the
    /// population's quantile-matched change otherwise. Built once per before
    /// frame; [`Cells::verdict`] and [`Cells::misfit`] read it against any
    /// after frame (the class's before frame is fixed while the class's own
    /// coordinates move, `Joint::ask_fit`).
    pub(super) fn asks(&self, class: Class, before: &[[f32; 3]]) -> ClassAsks {
        let axes = class.axes();
        let cb = coords_of(before);
        // The population asks are read whenever a cell may need them: where
        // pixels do not pair, or where the pairing's reliability leaves a
        // cell's paired weight short of the majority.
        let pop = if self.any_unpaired || self.reliability < 1.0 { Some(self.population_asks(&cb, axes)) } else { None };
        let pw = |i: usize| if self.paired[i] { self.w[i] * (1.0 - self.reliability) } else { self.w[i] };
        let (bcol, tcol) = (cb.columns(), self.target.columns());
        let per_cell: Vec<Vec<AxisAsk>> = self
            .grid(class)
            .par_iter()
            .map(|cell| {
                let members = &cell.members;
                let level = cell_level(&cb, &self.w, members);
                let paired_mass: f32 = members.iter().filter(|&&i| self.paired[i as usize]).map(|&i| self.w[i as usize]).sum();
                let paired_n = members.iter().filter(|&&i| self.paired[i as usize]).count();
                // Paired asks where the paired weight J* trusts — the paired
                // mass at the pairing's reliability — is the majority of the
                // cell; population asks otherwise (the night master's cells
                // pair by mass and J* reads them as a population, legs/J9).
                let use_paired = (paired_n as f32) >= SUPPORT_MIN_PIXELS && paired_mass * self.reliability >= 0.5 * cell.mass;
                let mut out = Vec::new();
                for &axis in axes {
                    let mut asks = Vec::new();
                    let mut complete = true;
                    for &slot in slots(axis) {
                        let mut ask_samples: Vec<(f32, f32)> = if use_paired {
                            members
                                .iter()
                                .filter(|&&i| self.paired[i as usize])
                                .map(|&i| (tcol[slot][i as usize] - bcol[slot][i as usize], self.w[i as usize]))
                                .collect()
                        } else if let Some(pop) = &pop {
                            members
                                .iter()
                                .map(|&i| i as usize)
                                .filter(|&i| pw(i) > 0.0)
                                .map(|i| (pop[slot][i], pw(i)))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        // A population ask speaks for the cell only where the
                        // members that HAVE one are the majority of those the
                        // axis can be read on — the paired side's own rule
                        // (`use_paired`), on this side. A colour ask exists
                        // per brightness range and a cell of the white
                        // balance or the chroma dials spans ranges, so
                        // without it a sliver in a populated range answered
                        // for a cell whose bulk has no target population
                        // (the strip fixture: 3.4 % of the Blue cell), and
                        // the cell the law calls blind never was.
                        if !use_paired && !population_majority(&ask_samples, members, bcol[slot], &pw) {
                            complete = false;
                            break;
                        }
                        match reading(&mut ask_samples, axis, level) {
                            Some(a) => asks.push(a),
                            None => {
                                complete = false;
                                break;
                            }
                        }
                    }
                    if complete {
                        out.push(AxisAsk { axis, asks, paired: use_paired });
                    }
                }
                out
            })
            .collect();
        ClassAsks { class, before_px: before.to_vec(), before: cb, per_cell }
    }

    /// The brightness asks of `asks`' cells, for the stage that states them as
    /// a curve (`asks_curve::asks_tone_curve`): per cell with a brightness
    /// ask, its display level on the asks' frame, the ask (ln linear luma)
    /// and the cell's mass.
    pub(super) fn luma_asks(&self, asks: &ClassAsks) -> Vec<(f32, f32, f32)> {
        self.grid(asks.class)
            .iter()
            .zip(&asks.per_cell)
            .filter_map(|(cell, axes)| {
                let luma = axes.iter().find(|a| a.axis == Axis::Luma)?;
                Some((cell_level(&asks.before, &self.w, &cell.members), luma.asks[0].0, cell.mass))
            })
            .collect()
    }

    /// The moves of `after` from the asks' before frame, per slot of `axis` in
    /// cell `ci`: the weighted median of after − before over the cell's
    /// members and its error, `None` where a slot cannot be read.
    fn moves_of(&self, asks: &ClassAsks, acol: &[&Vec<f32>; 5], ci: usize, axis: Axis, level: f32) -> Option<Vec<(f32, f32)>> {
        let bcol = asks.before.columns();
        let members = &self.grid(asks.class)[ci].members;
        let mut moves = Vec::new();
        for &slot in slots(axis) {
            let mut move_samples: Vec<(f32, f32)> =
                members.iter().map(|&i| (acol[slot][i as usize] - bcol[slot][i as usize], self.w[i as usize])).collect();
            moves.push(reading(&mut move_samples, axis, level)?);
        }
        Some(moves)
    }

    /// The verdict of `after` against the asks read on their before frame:
    /// each cell's move on each asked axis against its ask,
    /// `|move − ask| ≤ |ask| + se`, and the blind cells moved.
    pub(super) fn verdict(&self, asks: &ClassAsks, after: &[[f32; 3]]) -> Verdict {
        let ca = coords_of(after);
        let acol = ca.columns();
        let verdicts: Vec<(Vec<Refused>, bool)> = self
            .grid(asks.class)
            .par_iter()
            .enumerate()
            .map(|(ci, cell)| {
                let members = &cell.members;
                let level = cell_level(&asks.before, &self.w, members);
                let mut refused = Vec::new();
                let mut any_ask = false;
                for a in &asks.per_cell[ci] {
                    let Some(moves) = self.moves_of(asks, &acol, ci, a.axis, level) else { continue };
                    any_ask = true;
                    let dist: f32 = a.asks.iter().zip(&moves).map(|(x, m)| (m.0 - x.0).powi(2)).sum::<f32>().sqrt();
                    let ask_len: f32 = a.asks.iter().map(|x| x.0 * x.0).sum::<f32>().sqrt();
                    let move_len: f32 = moves.iter().map(|m| m.0 * m.0).sum::<f32>().sqrt();
                    let se: f32 = a.asks.iter().zip(&moves).map(|(x, m)| x.1 * x.1 + m.1 * m.1).sum::<f32>().sqrt();
                    if dist > ask_len + se {
                        let dot: f32 = a.asks.iter().zip(&moves).map(|(x, m)| x.0 * m.0).sum();
                        let cos = if ask_len > 0.0 && move_len > 0.0 { dot / (ask_len * move_len) } else { 0.0 };
                        refused.push(Refused { cell: ci, axis: a.axis, moved: move_len, ask: ask_len, se, paired: a.paired, cos });
                    }
                }
                // No ask on any axis: the cell is blind, and its displacement
                // is read in display codes.
                let blind = !any_ask && {
                    let mut d: Vec<(f32, f32)> = members
                        .iter()
                        .map(|&i| {
                            let i = i as usize;
                            let (p, q) = (asks.before_px[i], after[i]);
                            ((p[0] - q[0]).abs().max((p[1] - q[1]).abs()).max((p[2] - q[2]).abs()), self.w[i])
                        })
                        .collect();
                    weighted_median(&mut d) > UNSUPPORTED_RANGE_MOVE
                };
                (refused, blind)
            })
            .collect();
        let mut out = Verdict::default();
        for (ci, (refused, blind)) in verdicts.into_iter().enumerate() {
            out.refused.extend(refused);
            if blind {
                out.blind.push(ci);
            }
        }
        out
    }

    /// Does a population ask any of `asks`' cells its brightness? Where every
    /// asked cell pairs, [`Cells::stretched`] has nothing to read.
    pub(super) fn asked_by_population(&self, asks: &ClassAsks) -> bool {
        asks.per_cell.iter().any(|axes| axes.iter().any(|a| a.axis == Axis::Luma && !a.paired))
    }

    /// The tone's brightness ranges that a POPULATION asks and whose contrast
    /// the tone's shape RAISED (module docs, Contrast): per such cell of
    /// `asks` — the tone's — the slope of the members' display luma on
    /// `after` against their display luma on `exposed`, the candidate without
    /// the tone's shape and with its exposure, where it exceeds 1 by more
    /// than its error ([`contrast_gain`]). Empty for another class's asks.
    pub(super) fn stretched(&self, asks: &ClassAsks, exposed: &[[f32; 3]], after: &[[f32; 3]]) -> Vec<Stretched> {
        if asks.class != Class::Tone {
            return Vec::new();
        }
        self.grid(Class::Tone)
            .par_iter()
            .zip(&asks.per_cell)
            .enumerate()
            .filter(|(_, (_, axes))| axes.iter().any(|a| a.axis == Axis::Luma && !a.paired))
            .filter_map(|(ci, (cell, _))| {
                let (gain, se) = contrast_gain(exposed, after, &self.w, &cell.members)?;
                (gain - 1.0 > se).then_some(Stretched { cell: ci, gain, se })
            })
            .collect()
    }

    /// The misfit's own quantum: one output code on every cell with an ask
    /// (the unit [`Cells::misfit`] sums in), the resolution the
    /// re-estimation's probe reads a coordinate's slope at.
    pub(super) fn misfit_unit(&self, asks: &ClassAsks) -> f32 {
        self.grid(asks.class).iter().zip(&asks.per_cell).filter(|(_, a)| !a.is_empty()).map(|(c, _)| c.mass).sum()
    }

    /// The ask MISFIT of `after`: over the cells and axes with an ask, the
    /// cell's mass times the squared gap between its move and its ask, each
    /// slot in units of one output code on its axis ([`code_variance`], the
    /// law's own quantisation unit, so brightness, chroma and the white
    /// balance's axes add in one unit). The quantity a refused class is
    /// re-fitted on (`Joint::ask_fit`); zero where no axis asks.
    pub(super) fn misfit(&self, asks: &ClassAsks, after: &[[f32; 3]]) -> f32 {
        let ca = coords_of(after);
        let acol = ca.columns();
        self.grid(asks.class)
            .par_iter()
            .enumerate()
            .map(|(ci, cell)| {
                let level = cell_level(&asks.before, &self.w, &cell.members);
                let mut sum = 0.0f32;
                for a in &asks.per_cell[ci] {
                    let Some(moves) = self.moves_of(asks, &acol, ci, a.axis, level) else { continue };
                    let unit = code_variance(a.axis, level).max(1e-12);
                    let d2: f32 = a.asks.iter().zip(&moves).map(|(x, m)| (m.0 - x.0).powi(2)).sum();
                    sum += cell.mass * d2 / unit;
                }
                sum
            })
            .sum()
    }

    /// The cells with standing and no ask on a class's axes that `after`
    /// moved from `before` by more than [`UNSUPPORTED_RANGE_MOVE`], named
    /// (every class's blind cells, read on the whole recipe's work): what a
    /// `Disclose` solve discloses (`FIT_NOTE_VETO_DISCLOSED`) where a
    /// `Withhold` solve refuses the candidate ([`Verdict::admits`]). The
    /// chroma dials and the curves share their axes, so three readings
    /// cover the four classes. With `exposed` — the finished recipe without
    /// the tone's shape, rendered ([`disclosed_cells`]) — the tone's ranges a
    /// population asks that the shape stretched are named beside them,
    /// marked `(contrast)` ([`Cells::stretched`]).
    pub(super) fn blind_cell_names(&self, before: &[[f32; 3]], after: &[[f32; 3]], exposed: Option<&[[f32; 3]]>) -> Option<String> {
        let mut blind: Vec<String> = [Class::Tone, Class::WhiteBalance, Class::Chroma]
            .into_iter()
            .flat_map(|class| {
                self.judge(class, before, after).blind.into_iter().map(move |c| self.grid(class)[c].name()).collect::<Vec<_>>()
            })
            .collect();
        if let Some(exposed) = exposed {
            let asks = self.asks(Class::Tone, before);
            let tone = self.grid(Class::Tone);
            blind.extend(self.stretched(&asks, exposed, after).iter().map(|s| format!("{} (contrast)", tone[s.cell].name())));
        }
        blind.sort_unstable();
        blind.dedup();
        (!blind.is_empty()).then(|| blind.join(", "))
    }

    /// The trace line of a verdict: the class, each refused cell with its
    /// axis, move, ask and error, and the blind cells moved.
    pub(super) fn describe(&self, class: Class, v: &Verdict) -> String {
        let mut parts: Vec<String> = v
            .refused
            .iter()
            .map(|r| {
                format!(
                    "{} {:?} move {:.4} ask {:.4} ±{:.4} cos {:+.2} ({})",
                    self.grid(class)[r.cell].name(),
                    r.axis,
                    r.moved,
                    r.ask,
                    r.se,
                    r.cos,
                    if r.paired { "paired" } else { "population" }
                )
            })
            .collect();
        parts.extend(v.blind.iter().map(|&c| format!("{} moved blind", self.grid(class)[c].name())));
        parts.extend(
            v.stretched
                .iter()
                .map(|s| format!("{} contrast x{:.3} ±{:.3} on a population's ask", self.grid(class)[s.cell].name(), s.gain, s.se)),
        );
        format!("{class:?}: {}", if parts.is_empty() { "admitted".to_string() } else { parts.join("; ") })
    }

    /// A trace of a class's asks, every cell with its axes (the ask per slot
    /// and its error, paired or population).
    pub(super) fn describe_asks(&self, asks: &ClassAsks) -> String {
        let cells: Vec<String> = self
            .grid(asks.class)
            .iter()
            .zip(&asks.per_cell)
            .filter(|(_, a)| !a.is_empty())
            .map(|(cell, a)| {
                let axes: Vec<String> = a
                    .iter()
                    .map(|x| {
                        let slots: Vec<String> = x.asks.iter().map(|(m, se)| format!("{m:+.4}±{se:.4}")).collect();
                        format!("{:?} {} ({})", x.axis, slots.join(","), if x.paired { "paired" } else { "population" })
                    })
                    .collect();
                format!("{} {}", cell.name(), axes.join(" "))
            })
            .collect();
        format!("{:?}: {}", asks.class, cells.join("; "))
    }
}

/// The cells a `Disclose` solve names for its finished `recipe`
/// ([`Cells::blind_cell_names`]): the blind cells it moved and the tone's
/// population-asked ranges its shape stretched — the latter read from the
/// recipe without the tone's shape ([`super::solve::without_tone_shape`]),
/// rendered here and only where the recipe has one. `before` is the base's
/// render of `s_img`, `after` the recipe's.
pub(super) fn disclosed_cells(
    cells: &Cells,
    s_img: &image::DynamicImage,
    base: &crate::recipe::EditRecipe,
    recipe: &crate::recipe::EditRecipe,
    before: &[[f32; 3]],
    after: &[[f32; 3]],
) -> Option<String> {
    let unshaped = super::solve::without_tone_shape(base, recipe);
    let exposed = (unshaped != *recipe).then(|| super::pixels::pixels_of(&render::develop_preview(s_img, &unshaped)));
    cells.blind_cell_names(before, after, exposed.as_deref())
}
