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

/// The target's cell statistics for one pair, plus the weight each cell
/// carries in a verdict. Built ONCE per solve from the frozen evidence model
/// and the target analysis raster; every later question is asked of the same
/// numbers, so two stages cannot vouch against two different targets.
#[derive(Clone, Debug)]
pub(crate) struct PairedCells {
    /// `evidence.source_weights` summed over each cell: what the cell is worth
    /// as evidence.
    mass: Vec<f32>,
    /// Structural confidence in [0, 1], the evidence model's per-pixel
    /// `spatial_weights` averaged over the cell.
    trust: Vec<f32>,
    /// Weighted mean of the TARGET over each cell, channel by channel.
    target: Vec<[f32; 3]>,
    /// Cell index of every analysis pixel.
    of_pixel: Vec<usize>,
    /// Per-pixel weight the means are taken with (evidence, clipping-aware).
    weight: Vec<f32>,
}

/// What one [`PairedCells::vouch`] call measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CellVouch {
    /// Trust-weighted share of the region whose cell mean moved CLOSER.
    pub(crate) converged: f32,
    /// …and the share that moved further away than the blind-move line.
    pub(crate) diverged: f32,
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
    }

    /// The two shares as the disclosures print them.
    pub(crate) fn shares(self) -> (String, String) {
        (format!("{:.3}", self.converged), format!("{:.3}", self.diverged))
    }
}

/// Chebyshev distance between two pixels — `fit::converges_toward`'s metric.
fn distance(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0f32, f32::max)
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
        let mut mass = vec![0.0f32; cells];
        let mut trust_sum = vec![0.0f64; cells];
        let mut member = vec![0.0f64; cells];
        let mut sums = vec![[0.0f64; 3]; cells];
        for (i, &cell) in of_pixel.iter().enumerate() {
            mass[cell] += weight[i];
            member[cell] += 1.0;
            trust_sum[cell] += evidence.spatial_weights.get(i).copied().unwrap_or(0.0) as f64;
            for (slot, v) in sums[cell].iter_mut().zip(tp[i]) {
                *slot += weight[i] as f64 * v as f64;
            }
        }
        if mass.iter().sum::<f32>() <= 0.0 {
            return None;
        }
        let trust: Vec<f32> = (0..cells)
            .map(|c| if member[c] > 0.0 { (trust_sum[c] / member[c]) as f32 } else { 0.0 })
            .collect();
        let target: Vec<[f32; 3]> = (0..cells)
            .map(|c| {
                let w = (mass[c] as f64).max(1e-12);
                std::array::from_fn(|ch| (sums[c][ch] / w) as f32)
            })
            .collect();
        Some(Self { mass, trust, target, of_pixel, weight })
    }

    /// How much this pixel's CELL is trusted to hold its target's counterpart.
    /// The per-pixel weight a stage uses when the texture did not survive and
    /// the pixel's own robust weight is therefore not a statement about it.
    pub(crate) fn pixel_trust(&self, i: usize) -> f32 {
        self.of_pixel.get(i).map_or(0.0, |&cell| self.trust[cell])
    }

    /// Did `after` take the region's cells toward this target, or away?
    ///
    /// `region` is an optional soft membership over the same analysis raster
    /// (a zone's mask): it restricts BOTH the cell means and the shares, so a
    /// sky edit is judged on the sky's cells and nothing else.
    pub(crate) fn vouch(
        &self,
        before: &[[f32; 3]],
        after: &[[f32; 3]],
        region: Option<&[f32]>,
    ) -> CellVouch {
        let cells = CELLS_X * CELLS_Y;
        let n = self.of_pixel.len().min(before.len()).min(after.len());
        let mut weight = vec![0.0f64; cells];
        let mut before_sum = vec![[0.0f64; 3]; cells];
        let mut after_sum = vec![[0.0f64; 3]; cells];
        for i in 0..n {
            let member = region.map_or(1.0, |r| r.get(i).copied().unwrap_or(0.0).max(0.0));
            let w = self.weight[i] as f64 * member as f64;
            if w <= 0.0 {
                continue;
            }
            let cell = self.of_pixel[i];
            weight[cell] += w;
            for ch in 0..3 {
                before_sum[cell][ch] += w * before[i][ch] as f64;
                after_sum[cell][ch] += w * after[i][ch] as f64;
            }
        }
        let (mut total, mut converged, mut diverged, mut read) = (0.0f64, 0.0f64, 0.0f64, 0usize);
        for cell in 0..cells {
            // The verdict's weight is the cell's EVIDENCE mass scaled by how
            // much structure survived in it — a cell nothing measured is not
            // asked, and a cell the instrument distrusts is asked quietly.
            let vote = self.mass[cell] as f64 * self.trust[cell] as f64;
            if weight[cell] <= 0.0 || vote <= 0.0 {
                continue;
            }
            read += 1;
            total += vote;
            let mean = |sum: &[f64; 3]| -> [f32; 3] {
                std::array::from_fn(|ch| (sum[ch] / weight[cell]) as f32)
            };
            let (b, a) = (mean(&before_sum[cell]), mean(&after_sum[cell]));
            let (db, da) = (distance(&b, &self.target[cell]), distance(&a, &self.target[cell]));
            if da + CONVERGE_MARGIN < db {
                converged += vote;
            } else if da > db + fit::UNSUPPORTED_RANGE_MOVE {
                diverged += vote;
            }
        }
        let share = |v: f64| if total > 0.0 { (v / total) as f32 } else { 0.0 };
        CellVouch { converged: share(converged), diverged: share(diverged), read }
    }
}

#[cfg(test)]
mod tests;
