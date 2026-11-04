//! The colour noise reduction of a RAW, in the camera's own linear light
//! (E3 of v1.18.0): one stage, on the mosaic, before the demosaic and before
//! every per-pixel nonlinear stage of the develop.
//!
//! **Why here.** A render's dark colour is biased by its own chroma noise:
//! the black clip after the white balance and the matrix, the transfer
//! encode, saturation and the final pack each read a dark pixel's noisy
//! colour, and the mean of a nonlinear function of a noisy value is not the
//! function of its mean (measured on the night pair: its dark deciles sit
//! 2.4–6.0 Lab off their binned version, mostly too blue). A colour noise
//! reduction that runs after those stages, as the display-frame pass does,
//! finds the bias already made. Before them, on the linear mosaic, it takes
//! the noise away before anything can rectify it.
//!
//! **The grid.** The mosaic's 2×2 cells: per cell its red, the mean of its two
//! greens and its blue, and the two colour differences `wR·R − G`,
//! `wB·B − G` (camera white balance, green = 1, so a neutral reads 0). Cells
//! share no site, so two cells' noise is independent and the mosaic's own
//! noise model ([`super::noise_model`]) states it exactly — the only grid of
//! the develop on which that is true (after a demosaic, neighbouring pixels
//! are interpolated from the same sites). The green sites, which carry half
//! the mosaic and the luminance a Bayer demosaic is built around, are never
//! written; the red and blue sites are rebuilt on their cell's green.
//!
//! **The filter.** A sigma filter: a cell's colour difference becomes the
//! mean of the differences in its window that lie within `t = k·σ_d` of its
//! own, `σ_d` the model's standard deviation of a DIFFERENCE of two cells at
//! that level (`√2` times one cell's, per colour difference, the red and blue
//! predicted from the green fit). A neighbour further away than the noise
//! can carry is scene, and does not vote.
//!
//! **γ = 1.** In linear light the noise is the only scale at a pixel; a
//! threshold that is a multiple of `σ(L)` is the only form that does not
//! introduce a scale of its own. (The `σ^0.5` that v1.17.1's round 3
//! predicted was measured on the display frame, where the encode bends the
//! scale; it does not apply in the linear domain.)
//!
//! **k follows the photo** ([`photo_k`]). It is read where the stop is
//! visible, on texture: over the photo's own textured patches (the flat
//! rule's complement, [`NoiseModel::is_flat`]) the filter removes the noise
//! and nothing more while the edge energy it takes away (the squared cell
//! differences of both colour differences) stays within the edge energy the
//! noise model attributes to noise there; `k` is where the two meet. A flat
//! patch cannot say where to stop — more smoothing is always closer to the
//! truth there — so the flat patches are the CHECK of `k`, never its source.
//! It is one law, not one constant: on the most textured 1000 px windows of
//! two ordinary scenes the meeting point is 2.62 and 3.69 (rel1180 msg-e3
//! round 4), further apart than the model's own 2–8 % error on the noise
//! can move it; over all their textured patches the law reads 3.42 and
//! 4.12, and 1.97 on a night sky. A photo with no texture has nothing to
//! protect, and its `k`
//! is unbounded (the window's mean).
//!
//! **The window** is the colour noise reduction's own reach at its default
//! strength (`detail::chroma_nr_reach_px`, 5.5 film pixels at 25), in cells.
//!
//! **The slider.** "Colour noise reduction" scales `k` linearly: 0 is off,
//! the default strength (Lightroom's own 25) is the photo's own `k`, 100 is
//! four times it. Detail and Smoothness belong to the display-frame pass,
//! which a RAW no longer runs.
//!
//! **One stage per develop.** A RAW with a 2×2 Bayer mosaic runs this stage
//! and no display-frame colour pass; a source with no mosaic (a JPEG, a TIFF,
//! a phone picture) keeps the display-frame pass; a master baked with this
//! stage is marked so that its card runs no second one, and a master baked
//! without it (every master before v1.18.0) keeps the display pass until it
//! is baked again.

use rayon::prelude::*;

use super::noise_model::{bayer_sites, patches, NoiseModel, Patch, PATCH};

/// The slider value at which the stage runs at the photo's own `k`:
/// Lightroom's own default for Color.
pub(crate) const DEFAULT_AMOUNT: f32 = 25.0;

/// The multiple of the photo's `k` the slider value `color_nr` (0..=100)
/// asks for: `None` when off (the colour-NR control's own rule,
/// `ChromaNrParams::global`).
pub(crate) fn scale_of(color_nr: f32) -> Option<f64> {
    (color_nr > 0.0).then(|| f64::from(color_nr.min(100.0)) / f64::from(DEFAULT_AMOUNT))
}

/// The window's half side in cells: the colour NR's reach at its default
/// strength (`render::chroma_nr_reach_px`, film pixels) over the cell's two
/// pixels.
pub(crate) fn window() -> usize {
    ((crate::render::chroma_nr_reach_px(DEFAULT_AMOUNT / 100.0) / 2.0).round() as usize).max(1)
}

/// Run the stage on the normalised signed `mosaic` (`width × height`, sites
/// of `cfa`), white-balanced by `wb` (any scale; green is divided out), at
/// `scale` times the photo's own `k` ([`photo_k`]) over a window of half side
/// `rho` cells. Returns the fitted model and the `k` it ran at, or `None` —
/// and the mosaic untouched — when the pattern is not a 2×2 RGB Bayer or the
/// frame has too few usable patches for a model.
pub(crate) fn filter_mosaic(
    mosaic: &mut [f32],
    width: usize,
    height: usize,
    cfa: &rawler::cfa::CFA,
    wb: [f64; 3],
    scale: f64,
    rho: usize,
) -> Option<(NoiseModel, f64)> {
    let sites = bayer_sites(cfa)?;
    if mosaic.len() != width * height || !(wb[1] > 0.0) || !(scale > 0.0) {
        return None;
    }
    let wb = [wb[0] / wb[1], 1.0, wb[2] / wb[1]];
    let pts = patches(mosaic, width, height, sites, wb);
    let model = NoiseModel::fit(&pts, width, height)?;
    let cells = Cells::of(mosaic, width, height, sites, wb, &model, &pts)?;
    let k = photo_k(&cells, &model, &pts, wb, rho).0 * scale;
    let filtered: Vec<[f32; 2]> = (0..cells.cw * cells.ch).into_par_iter().map(|i| cells.filtered(i, k, rho)).collect();
    cells.write(mosaic, width, sites, wb, &filtered);
    Some((model, k))
}

/// The rungs `photo_k` reads the removed edge energy on: `2^(j/2)`,
/// `j = 0..LADDER` (1 to 64, half an octave apart — a resolution: the
/// crossing is interpolated between rungs).
const LADDER: usize = 13;

/// The photo's own `k`: the threshold (in `σ_d`) at which the edge energy the
/// filter removes from the photo's TEXTURED patches meets the noise model's
/// share of that energy (see the module), linear between the rungs of
/// [`LADDER`]; `f64::INFINITY` when no textured patch exists or the removed
/// energy never reaches the share. Also returns, per rung read,
/// `(k, removed / share)` — the reading itself.
pub(crate) fn photo_k(cells: &Cells, model: &NoiseModel, pts: &[Patch], wb: [f64; 3], rho: usize) -> (f64, Vec<(f64, f64)>) {
    let textured: Vec<usize> = (0..pts.len()).filter(|&i| pts[i].usable && !model.is_flat(&pts[i], wb)).collect();
    if textured.is_empty() {
        return (f64::INFINITY, Vec::new());
    }
    let (cw, ch, pw) = (cells.cw, cells.ch, cells.pw);
    let origin = |p: usize| ((p % pw) * PATCH, (p / pw) * PATCH);
    // A patch's forward differences (to the right and below, inside the
    // frame): each pairs two independent cells.
    let neighbours = move |cx: usize, cy: usize| {
        [(cx + 1 < cw).then(|| cy * cw + cx + 1), (cy + 1 < ch).then(|| (cy + 1) * cw + cx)].into_iter().flatten()
    };
    let energy = |value: &(dyn Fn(usize) -> [f32; 2] + Sync)| -> f64 {
        textured
            .par_iter()
            .map(|&p| {
                let (x0, y0) = origin(p);
                let mut e = 0.0f64;
                for cy in y0..y0 + PATCH {
                    for cx in x0..x0 + PATCH {
                        let c = value(cy * cw + cx);
                        for n in neighbours(cx, cy) {
                            let q = value(n);
                            e += f64::from(q[0] - c[0]).powi(2) + f64::from(q[1] - c[1]).powi(2);
                        }
                    }
                }
                e
            })
            .sum()
    };
    // The noise's share: each difference carries both cells' variance, at the
    // patch's level (`2·var` per difference and colour difference).
    let share: f64 = textured
        .iter()
        .map(|&p| {
            let [vr, vb] = model.chroma_var(&pts[p], wb);
            let (x0, y0) = origin(p);
            let diffs: usize =
                (y0..y0 + PATCH).flat_map(|cy| (x0..x0 + PATCH).map(move |cx| neighbours(cx, cy).count())).sum();
            2.0 * (vr + vb) * diffs as f64
        })
        .sum();
    let e_in = energy(&|i| [cells.d[i][0], cells.d[i][1]]);
    let mut read: Vec<(f64, f64)> = Vec::with_capacity(LADDER);
    for j in 0..LADDER {
        let k = 2f64.powf(j as f64 / 2.0);
        let removed = (e_in - energy(&|i| cells.filtered(i, k, rho))) / share;
        read.push((k, removed));
        if removed >= 1.0 {
            let k = match read.len() {
                1 => k,
                n => {
                    let ((k0, r0), (k1, r1)) = (read[n - 2], read[n - 1]);
                    k0 + (1.0 - r0) * (k1 - k0) / (r1 - r0)
                }
            };
            return (k, read);
        }
    }
    (f64::INFINITY, read)
}

/// The mosaic's cells: the two colour differences and the green of each, and
/// each patch's `σ_d` per colour difference.
pub(crate) struct Cells {
    cw: usize,
    ch: usize,
    pw: usize,
    ph: usize,
    /// `[wR·R − G, wB·B − G, G]` per cell.
    d: Vec<[f32; 3]>,
    /// `σ_d` per patch and colour difference: `√(2 · chroma_var)`.
    sd: Vec<[f32; 2]>,
}

impl Cells {
    /// `None` when the frame has no whole patch or `pts` is not its patches.
    pub(crate) fn of(
        mosaic: &[f32],
        width: usize,
        height: usize,
        sites: [(usize, usize); 4],
        wb: [f64; 3],
        model: &NoiseModel,
        pts: &[Patch],
    ) -> Option<Self> {
        let (cw, ch) = (width / 2, height / 2);
        let (pw, ph) = (cw / PATCH, ch / PATCH);
        if pw == 0 || ph == 0 || pts.len() != pw * ph {
            return None;
        }
        let at = |cx: usize, cy: usize, s: usize| {
            let (dy, dx) = sites[s];
            f64::from(mosaic[(2 * cy + dy) * width + 2 * cx + dx])
        };
        let d = (0..cw * ch)
            .into_par_iter()
            .map(|i| {
                let (cx, cy) = (i % cw, i / cw);
                let g = (at(cx, cy, 1) + at(cx, cy, 2)) / 2.0;
                [(wb[0] * at(cx, cy, 0) - g) as f32, (wb[2] * at(cx, cy, 3) - g) as f32, g as f32]
            })
            .collect();
        let sd = pts.iter().map(|p| model.chroma_var(p, wb).map(|v| (2.0 * v).sqrt() as f32)).collect();
        Some(Self { cw, ch, pw, ph, d, sd })
    }

    /// Cell `i`'s two colour differences through the sigma filter at `k`.
    fn filtered(&self, i: usize, k: f64, rho: usize) -> [f32; 2] {
        let (cw, ch) = (self.cw, self.ch);
        let (cx, cy) = (i % cw, i / cw);
        let sd = self.sd[(cy / PATCH).min(self.ph - 1) * self.pw + (cx / PATCH).min(self.pw - 1)];
        let t = [(k * f64::from(sd[0])) as f32, (k * f64::from(sd[1])) as f32];
        let c = self.d[i];
        let (mut sum, mut n) = ([0.0f64; 2], [0u32; 2]);
        for y in cy.saturating_sub(rho)..(cy + rho + 1).min(ch) {
            for q in &self.d[y * cw + cx.saturating_sub(rho)..y * cw + (cx + rho + 1).min(cw)] {
                for j in 0..2 {
                    if (q[j] - c[j]).abs() <= t[j] {
                        sum[j] += f64::from(q[j]);
                        n[j] += 1;
                    }
                }
            }
        }
        // The centre always votes (|0| ≤ t), so n ≥ 1.
        [(sum[0] / f64::from(n[0])) as f32, (sum[1] / f64::from(n[1])) as f32]
    }

    /// Write `filtered` back: red and blue rebuilt on the cell's own green,
    /// the greens untouched.
    fn write(&self, mosaic: &mut [f32], width: usize, sites: [(usize, usize); 4], wb: [f64; 3], filtered: &[[f32; 2]]) {
        let cw = self.cw;
        let (wr, wbl) = (wb[0] as f32, wb[2] as f32);
        let (r_at, b_at) = (sites[0], sites[3]);
        mosaic.par_chunks_mut(2 * width).take(self.ch).enumerate().for_each(|(cy, lines)| {
            for cx in 0..cw {
                let [dr, db] = filtered[cy * cw + cx];
                let g = self.d[cy * cw + cx][2];
                lines[r_at.0 * width + 2 * cx + r_at.1] = (g + dr) / wr;
                lines[b_at.0 * width + 2 * cx + b_at.1] = (g + db) / wbl;
            }
        });
    }
}

#[cfg(test)]
mod tests;
