//! The paired-CELL admission instrument (R33 §C).
//!
//! One question, asked by every stage that wants to move a control the
//! per-pixel evidence refuses: *did this edit take the frame's cells TOWARD
//! the target's cells, or away from them?*
//!
//! Why cells and not pixels. The fit's existing voucher
//! (`fit::converges_toward`) pairs source pixel `i` with target pixel `i`,
//! which is only valid where the texture survived — on a generative repaint
//! the sky's pixels are invented and the pairing is noise. A CELL mean over
//! the same rectangle is a different statistic: it needs the two frames to
//! show the same scene in the same place, not the same pixels, and that is
//! exactly what survives a repaint whose layout is intact.
//!
//! Why it is not a laundering machine. A stage that invents its own evidence
//! is the defect this crate guards hardest against (a one-sided band is
//! UNMEASURABLE, never "already equal"), and a convergence test is not
//! evidence THAT a control is right — it is the target's own verdict on what
//! the control DID. So this instrument can only lift a veto, never raise one,
//! never supply an estimate, and never name a value: its consumers solve from
//! population evidence exactly as before and ask this afterwards.
//!
//! TRUST is the frozen evidence model's own per-pixel structural confidence,
//! averaged over the cell — not a second divergence pass. The R33 design
//! called for a coarse-scale per-cell reading here; it was measured first and
//! refused (see `fit::COARSE_SIGMA_DIVISOR`): on the real pair the coarse
//! reading is not more forgiving at ANY scope, so a second instrument would
//! have bought a second 96-cell structural pass per admission and moved the
//! weights by 0.004. One structural ruler per solve, the one the value ranges
//! are already judged by.

use crate::fit::{self, EvidenceModel};
use crate::fit_field::unclipped;

/// Cell geometry: the local field's own grid, so a field attached later is
/// vouched over exactly the cells it is solved on.
pub(crate) const CELLS_X: usize = crate::fit_field::FIELD_X;
pub(crate) const CELLS_Y: usize = crate::fit_field::FIELD_Y;

/// A cell's mean must land strictly closer than it started, by this margin —
/// `fit::converges_toward`'s own margin, in the same display domain, so the
/// pixel voucher and the cell voucher mean the same thing by "closer".
const CONVERGE_MARGIN: f32 = 1e-3;
/// Trust-weighted mass share of the region whose cells must converge.
const VOUCH_MIN_CONVERGED: f32 = 0.70;
/// …and the share that may move AWAY from its target by more than
/// `fit::UNSUPPORTED_RANGE_MOVE` and still leave the verdict standing.
const VOUCH_MAX_DIVERGED: f32 = 0.10;
/// R34. The cosine floor of the DIRECTION test: a cell's move must point
/// within 60° of the direction to its own target mean.
///
/// "Closer" and "toward" are different questions, and only the second one can
/// admit a RECOLOUR. A Chebyshev distance shrinks for a move that crosses
/// colour space sideways — desaturating a warm region lands its mean nearer a
/// warm target's without being any part of the warming the target asks for —
/// and a region-scale gain whose channels are mis-proportioned does exactly
/// that on most of its cells. The angle refuses what the distance cannot.
const ALIGN_COS: f32 = 0.5;
/// Trust-weighted mass share of the region whose cells must be ALIGNED. The
/// same line as convergence, because the two are one verdict asked twice: a
/// region whose cells mostly moved closer but a third of them sideways is not
/// a region this edit recovered.
const VOUCH_MIN_ALIGNED: f32 = 0.70;

/// The target's cell statistics for one pair, plus the weight each cell
/// carries in a verdict. Built ONCE per solve from the frozen evidence model
/// and the target analysis raster; every later question is asked of the same
/// numbers, so two stages cannot vouch against two different targets.
#[derive(Clone, Debug)]
pub(crate) struct PairedCells {
    /// Structural confidence in [0, 1], the evidence model's per-pixel
    /// `spatial_weights` averaged over the whole cell — the frame-scope
    /// reading, kept for [`PairedCells::pixel_trust`]. A REGION averages the
    /// same numbers over its own membership instead.
    trust: Vec<f32>,
    /// Cell index of every analysis pixel.
    of_pixel: Vec<usize>,
    /// Per-pixel weight the means are taken with (evidence, clipping-aware).
    weight: Vec<f32>,
    /// The evidence model's per-pixel structural confidence, kept per PIXEL
    /// rather than only per cell: a region's verdict must be trusted by what
    /// the region actually covers.
    spatial: Vec<f32>,
    /// The TARGET raster these cells answer for. Per pixel, for the same
    /// reason: a cell's target mean is a statement about a population, and
    /// under a region the population is the region's part of the cell.
    tgt: Vec<[f32; 3]>,
}

/// What one [`PairedCells::vouch`] call measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CellVouch {
    /// Trust-weighted share of the region whose cell mean moved CLOSER.
    pub(crate) converged: f32,
    /// …and the share that moved further away than the blind-move line.
    pub(crate) diverged: f32,
    /// R34. Trust-weighted share of the region whose cell moved in a direction
    /// within [`ALIGN_COS`] of the direction to its OWN target mean.
    pub(crate) aligned: f32,
    /// Cells that carried any trust-weighted mass at all. Zero is an
    /// ABSTENTION: nothing was measured, so nothing is vouched.
    pub(crate) read: usize,
}

impl CellVouch {
    /// The verdict. An abstention is never a vouch.
    pub(crate) fn vouched(self) -> bool {
        self.read > 0
            && self.converged >= VOUCH_MIN_CONVERGED
            && self.diverged <= VOUCH_MAX_DIVERGED
            && self.aligned >= VOUCH_MIN_ALIGNED
    }

    /// R36. The DIRECTION half of the verdict on its own: the region's cells
    /// point where their own targets lie (the `aligned` share clears its
    /// line) even though the verdict as a whole fails. That is the one case a
    /// search over the SIZE of the move may answer — the cells asked for this
    /// move, only for less of it — and the case it may never answer is the
    /// one where `aligned` itself fails: cells asking for opposite moves are
    /// not asking for a smaller one.
    pub(crate) fn direction_agrees(self) -> bool {
        self.read > 0 && self.aligned >= VOUCH_MIN_ALIGNED
    }

    /// The three shares as the disclosures print them.
    pub(crate) fn shares(self) -> (String, String, String) {
        (
            format!("{:.3}", self.converged),
            format!("{:.3}", self.diverged),
            format!("{:.3}", self.aligned),
        )
    }
}

/// Chebyshev distance between two pixels — `fit::converges_toward`'s metric.
fn distance(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0f32, f32::max)
}

/// Did this cell move TOWARD its target, and not merely nearer to it?
///
/// Read in LINEAR light, because that is the domain a colour move is a
/// physical quantity in: the same three means the distance test reads,
/// transformed through the engine's own transfer function rather than
/// accumulated a second time. Two accumulators over one cell would be two
/// statistics, and two statistics can disagree about one edit.
///
/// A cell that is ALREADY where its target is asks for no move, so leaving it
/// alone is aligned and moving it is not — without that arm every matched cell
/// in a region would vote against an edit that correctly did nothing to it.
fn aligned(before: &[f32; 3], after: &[f32; 3], target: &[f32; 3]) -> bool {
    let linear = |p: &[f32; 3]| p.map(crate::render::srgb_to_linear);
    let (b, a, t) = (linear(before), linear(after), linear(target));
    let moved: [f32; 3] = std::array::from_fn(|c| a[c] - b[c]);
    let wanted: [f32; 3] = std::array::from_fn(|c| t[c] - b[c]);
    let reach = |v: &[f32; 3]| v.iter().fold(0.0f32, |m, c| m.max(c.abs()));
    if reach(&wanted) < CONVERGE_MARGIN {
        return reach(&moved) < CONVERGE_MARGIN;
    }
    if reach(&moved) < CONVERGE_MARGIN {
        return false;
    }
    let dot = (0..3).map(|c| moved[c] * wanted[c]).sum::<f32>();
    let length = |v: &[f32; 3]| v.iter().map(|c| c * c).sum::<f32>().sqrt();
    dot / (length(&moved) * length(&wanted)).max(1e-12) >= ALIGN_COS
}

/// One cell's reading in a single measurement pass. `None` at a cell the
/// region did not reach or the evidence never weighted — an abstention at cell
/// scale, which is what keeps a region of only unsupported pixels from
/// vouching anything.
#[derive(Clone, Copy, Debug)]
struct CellReading {
    vote: f64,
    converged: bool,
    diverged: bool,
    aligned: bool,
}

impl PairedCells {
    /// Build the target's cell statistics for one pair. `None` when the
    /// geometry does not resolve or nothing carries weight — an abstention,
    /// never an all-zero grid that would read as "measured and empty".
    pub(crate) fn build(
        tp: &[[f32; 3]],
        width: u32,
        height: u32,
        evidence: &EvidenceModel,
    ) -> Option<Self> {
        let n = (width as usize).checked_mul(height as usize)?;
        if n == 0 || tp.len() < n || evidence.source_weights.len() < n {
            return None;
        }
        let cells = CELLS_X * CELLS_Y;
        let of_pixel: Vec<usize> = (0..n)
            .map(|i| {
                let (x, y) = (i % width as usize, i / width as usize);
                (y * CELLS_Y / height as usize) * CELLS_X + x * CELLS_X / width as usize
            })
            .collect();
        let weight: Vec<f32> = (0..n)
            .map(|i| {
                if unclipped(&tp[i]) { evidence.source_weights[i].max(0.0) } else { 0.0 }
            })
            .collect();
        let spatial: Vec<f32> = (0..n)
            .map(|i| evidence.spatial_weights.get(i).copied().unwrap_or(0.0))
            .collect();
        let mut mass = vec![0.0f32; cells];
        let mut trust_sum = vec![0.0f64; cells];
        let mut member = vec![0.0f64; cells];
        for (i, &cell) in of_pixel.iter().enumerate() {
            mass[cell] += weight[i];
            member[cell] += 1.0;
            trust_sum[cell] += spatial[i] as f64;
        }
        if mass.iter().sum::<f32>() <= 0.0 {
            return None;
        }
        let trust: Vec<f32> = (0..cells)
            .map(|c| if member[c] > 0.0 { (trust_sum[c] / member[c]) as f32 } else { 0.0 })
            .collect();
        Some(Self { trust, of_pixel, weight, spatial, tgt: tp[..n].to_vec() })
    }

    /// How much this pixel's CELL is trusted to hold its target's counterpart.
    /// The per-pixel weight a stage uses when the texture did not survive and
    /// the pixel's own robust weight is therefore not a statement about it.
    pub(crate) fn pixel_trust(&self, i: usize) -> f32 {
        self.of_pixel.get(i).map_or(0.0, |&cell| self.trust[cell])
    }

    /// Which cell an analysis pixel sits in — the index into
    /// [`Self::verdicts`], so a per-pixel consumer and the region verdict
    /// cannot disagree about which cell answered for a pixel.
    pub(crate) fn cell_of(&self, i: usize) -> Option<usize> {
        self.of_pixel.get(i).copied()
    }

    /// Did `after` take the region's cells toward this target, or away?
    ///
    /// `region` is an optional soft membership over the same analysis raster
    /// (a zone's mask) and it restricts the WHOLE reading — each cell's before
    /// and after means, the target mean they are put to, the evidence mass the
    /// cell votes with and the trust that mass is scaled by. Restricting only
    /// some of those is not a smaller error but a different measurement: a
    /// cell a zone's feather merely clips would answer for a population it
    /// does not hold, in a voice sized for the population it does.
    pub(crate) fn vouch(
        &self,
        before: &[[f32; 3]],
        after: &[[f32; 3]],
        region: Option<&[f32]>,
    ) -> CellVouch {
        Self::fold(&self.measure(before, after, region))
    }

    /// R34. The per-CELL verdicts behind [`Self::vouch`]'s shares, for the
    /// consumers that admit pixel by pixel rather than region by region: one
    /// entry per cell, `None` where the cell was not read, `Some(converged &&
    /// aligned)` where it was.
    ///
    /// Same pass, same numbers: a pixel can never be admitted by a cell the
    /// region verdict counted the other way, because there is only one
    /// measurement and these are two views of it.
    pub(crate) fn verdicts(
        &self,
        before: &[[f32; 3]],
        after: &[[f32; 3]],
        region: Option<&[f32]>,
    ) -> Vec<Option<bool>> {
        Self::cellwise(self.measure(before, after, region))
    }

    /// R34 §D3. The per-cell verdicts for one edit — but ONLY when the region
    /// as a whole vouches it — together with the verdict it was admitted on,
    /// which its disclosure has to print.
    ///
    /// A cell's verdict is a LOCAL statement; a hue-band veto is a REGIONAL
    /// one. Without this scope one converged cell inside a region the cells
    /// overall REFUSE carries its own pixels through, and because the veto is
    /// a SHARE of the moved population, a converged third can hold enough of
    /// the moved pixels to drop the band under `fit::ROT_SHARE` and dissolve
    /// the veto entirely. Measured, before this gate existed: a frame whose
    /// cells read 0.337 converged / 0.663 diverged lifted a one-sided Blue
    /// band's veto. The region's own verdict is not overruled by its minority.
    ///
    /// This is deliberately NOT how `fit_zoned::field` reads the same numbers:
    /// there each cell keeps or replaces its OWN eight vertices and nothing
    /// else, so a good cell beside a bad one is simply a good cell, and the
    /// do-no-harm checks judge the result. Lifting a veto and choosing a
    /// vertex are different acts and take different scopes.
    pub(crate) fn region_arm(
        &self,
        before: &[[f32; 3]],
        after: &[[f32; 3]],
    ) -> Option<(CellVouch, Vec<Option<bool>>)> {
        let readings = self.measure(before, after, None);
        let vouch = Self::fold(&readings);
        vouch.vouched().then(|| (vouch, Self::cellwise(readings)))
    }

    fn fold(readings: &[Option<CellReading>]) -> CellVouch {
        let (mut total, mut converged, mut diverged, mut aligned, mut read) =
            (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0usize);
        for reading in readings.iter().flatten() {
            read += 1;
            total += reading.vote;
            if reading.converged {
                converged += reading.vote;
            } else if reading.diverged {
                diverged += reading.vote;
            }
            if reading.aligned {
                aligned += reading.vote;
            }
        }
        let share = |v: f64| if total > 0.0 { (v / total) as f32 } else { 0.0 };
        CellVouch {
            converged: share(converged),
            diverged: share(diverged),
            aligned: share(aligned),
            read,
        }
    }

    fn cellwise(readings: Vec<Option<CellReading>>) -> Vec<Option<bool>> {
        readings.into_iter().map(|r| r.map(|r| r.converged && r.aligned)).collect()
    }

    /// The one pass both public verdicts are taken from.
    fn measure(
        &self,
        before: &[[f32; 3]],
        after: &[[f32; 3]],
        region: Option<&[f32]>,
    ) -> Vec<Option<CellReading>> {
        let cells = CELLS_X * CELLS_Y;
        let n = self.of_pixel.len().min(before.len()).min(after.len());
        let mut weight = vec![0.0f64; cells];
        let mut mass = vec![0.0f32; cells];
        let mut trust_sum = vec![0.0f64; cells];
        let mut covered = vec![0.0f64; cells];
        let mut before_sum = vec![[0.0f64; 3]; cells];
        let mut after_sum = vec![[0.0f64; 3]; cells];
        let mut target_sum = vec![[0.0f64; 3]; cells];
        for i in 0..n {
            let member = region.map_or(1.0, |r| r.get(i).copied().unwrap_or(0.0).max(0.0));
            if member <= 0.0 {
                continue;
            }
            let cell = self.of_pixel[i];
            // Mass and trust accumulate over every pixel the region covers,
            // weighted by HOW MUCH it covers it — including pixels the
            // evidence gave no weight, exactly as the frame-scope build
            // counts them. A zero-weight pixel adds nothing to the means
            // below, so the two accumulations stay one population.
            mass[cell] += self.weight[i] * member;
            covered[cell] += member as f64;
            trust_sum[cell] += self.spatial[i] as f64 * member as f64;
            let w = self.weight[i] as f64 * member as f64;
            weight[cell] += w;
            for ch in 0..3 {
                before_sum[cell][ch] += w * before[i][ch] as f64;
                after_sum[cell][ch] += w * after[i][ch] as f64;
                target_sum[cell][ch] += w * self.tgt[i][ch] as f64;
            }
        }
        (0..cells)
            .map(|cell| {
                // The verdict's weight is the cell's EVIDENCE mass scaled by
                // how much structure survived in it — a cell nothing measured
                // is not asked, and a cell the instrument distrusts is asked
                // quietly. Both are read over the region, so a cell a zone's
                // feather merely clips votes in proportion to the sliver it
                // actually contributed.
                let trust =
                    if covered[cell] > 0.0 { (trust_sum[cell] / covered[cell]) as f32 } else { 0.0 };
                let vote = mass[cell] as f64 * trust as f64;
                if weight[cell] <= 0.0 || vote <= 0.0 {
                    return None;
                }
                let mean = |sum: &[f64; 3]| -> [f32; 3] {
                    std::array::from_fn(|ch| (sum[ch] / weight[cell]) as f32)
                };
                let (b, a) = (mean(&before_sum[cell]), mean(&after_sum[cell]));
                let t = &mean(&target_sum[cell]);
                let (db, da) = (distance(&b, t), distance(&a, t));
                Some(CellReading {
                    vote,
                    converged: da + CONVERGE_MARGIN < db,
                    diverged: da > db + fit::UNSUPPORTED_RANGE_MOVE,
                    aligned: aligned(&b, &a, t),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
