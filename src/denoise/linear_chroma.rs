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
//! **The pixels** ([`CellChroma::rebuild`]): the sensor measures colour once
//! per cell; pixel-scale chroma is the demosaic's, so the stage's cell chroma
//! is the pixel's. After the demosaic every pixel keeps its green — the
//! demosaic's luminance, `Y` of the colour differences above, untouched — and
//! its red and blue are rebuilt on it from the filtered cell chroma read at
//! that pixel (bilinear in the cell grid, the coordinate of the display
//! pass's own upsample, `render::upsample_at`). Without it each green site's
//! own noise, which no cell pass writes, becomes pixel-scale colour after the
//! demosaic (26–32 % of the night sky's residual grain, rel1180 msg-e3 round
//! 7); with it that noise is luminance. No pixel pass: one read per pixel.
//! (The filtered red and blue samples are still written to the mosaic, so
//! the demosaic's own direction choices read the filtered colour.)
//!
//! **The filter.** A sigma filter: a neighbour votes when the evidence says
//! it shares the cell's TRUE colour — it lies within `k` noise sds of an
//! estimate of that colour. The cell's own sample is not that colour: a test
//! anchored on it keeps the neighbours on the sample's side, and with them a
//! share of the sample's own noise (measured on the night sky's flat patches,
//! rel1180 msg-e3 round 8: 0.18–0.22 of it at its `k`, where the plain
//! window mean keeps 1/N = 0.02). So the sample anchors only the first
//! estimate (within `k·σ_d`, `σ_d` the model's sd of a DIFFERENCE of two
//! cells at that level, `√2` times one cell's `σ`, per colour difference, the
//! red and blue predicted from the green fit); each next pass is anchored on
//! the last estimate, a mean of `n` cells, against which a sample differs by
//! `σ·√(1 + 1/n)`, until an estimate repeats. That fixed point is the mean of
//! the window's samples within `t` of itself — the skipped mean, whose share
//! of one sample's noise is `1 / (N·(Z − 2t·φ(t)))` (`t` in `σ`, `Z` the
//! normal mass within `±t`; its influence function), which falls to the
//! window mean's `1/N` as `k` grows. A neighbour further from the estimate than
//! the noise can carry is scene, and does not vote.
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
//! can move it; over all their textured patches, with the estimate-anchored
//! test and the window below, the law reads 6.21 and 13.61, and 1.43 on a
//! night sky (round 9; at round 8's 3-cell window 5.86, 10.36, 1.63: the
//! crossing moves with the window, the law is the same). A photo with no texture has nothing to
//! protect, and its `k` is unbounded: the filter is then the plain mean of
//! its window, and the window is the bound.
//!
//! **The window** ([`window`]): the slider keeps its meaning — at each
//! strength the stage averages as many samples as the pass it replaces, so a
//! flat sky is as clean as before and the texture is now kept by the sigma
//! test, not by a blur. The display pass (`render::detail::chroma_nr`) leaves
//! on a flat region the share `Σw²` of white noise its kernel's weights give
//! (`render::chroma_nr_flat_noise_share`, from its own constants); a plain
//! mean leaves `1/N` of it, so a plain mean's window holds `N = 1/Σw²`
//! pixels, `N/4` cells ([`plain_window`]): at 25, `Σw² = 0.000995`, 1005
//! pixels, a half side of `(√251 − 1)/2 = 7.4`, so 7 cells (rel1180 msg-e3
//! round 9). The stage is not a plain mean: the skipped mean at its test
//! width `t` keeps `1/(N·(Z − 2t·φ(t)))` of a sample's noise (above), so its
//! window holds the `N` cells at which `N·(Z − 2t·φ(t))` is the display
//! pass's `N/4` ([`window`]; `t` the width the stage runs at, so it follows
//! the photo): the same on daylight, where `t` is wide and `Z − 2t·φ(t)` is 1
//! (7 cells at 25), wider on a night sky (11 cells at its `k` 1.43, rel1180
//! msg-e3 round 10). The display pass's reach (its radius law) stays that
//! pass's own, for the sources it still runs on. `k` itself is read over the
//! plain mean's window at the default strength ([`DEFAULT_AMOUNT`]) — the one
//! window that does not depend on the reading — so it is one reading per
//! photo.
//!
//! **The slider.** "Colour noise reduction" scales `k` linearly and sets the
//! window as above: 0 is off, the default strength (Lightroom's own 25) is the
//! photo's own `k` over (on daylight) 7 cells, 100 is four times it over 23. Below 25 the
//! display pass blends its result with the input and keeps more than one
//! cell's worth of noise; the stage's window is then its least, one cell.
//! Where `k` is unbounded (no texture) the scale cannot weaken it, and the
//! window does: at 10 the mean runs over 1 cell, at 25 over 7, so the slider
//! is never dead. Detail and Smoothness belong to the display-frame pass,
//! which a RAW no longer runs; the window reads that pass at Lightroom's own
//! Smoothness, the value an absent one resolves to.
//!
//! **One stage per develop.** The linear stage where a Bayer mosaic the
//! stage can read exists ([`reads`]: one sample per site on a 2×2 RGB
//! pattern); the display-frame pass otherwise — a JPEG, a TIFF, a phone
//! picture, a RAW whose mosaic is not a 2×2 Bayer (X-Trans, a linear DNG),
//! and a master baked without this stage (every master before v1.18.0, until
//! it is baked again). The develop reports which one ran: a master baked
//! with this stage is marked (`render::BakedColourNr`) so that its card runs
//! no second one, and only then. A readable mosaic whose noise cannot be
//! measured (fewer than 16 usable patches: a frame clipped or black all
//! over) has no noise the stage can see, and passes through it unchanged.

use rayon::prelude::*;

use super::noise_model::{bayer_sites, patches, NoiseModel, Patch, PATCH};

/// Can the stage read `raw`'s mosaic — one sample per site on a 2×2 RGB
/// Bayer pattern? The rule the develop runs the stage by and every surface
/// that asks whether a RAW's pixels carry it (`render::SourceFrame::of`,
/// off a metadata-only decode) reads, so the two cannot disagree.
pub(crate) fn reads(raw: &rawler::RawImage) -> bool {
    use rawler::rawimage::RawPhotometricInterpretation as Photo;
    matches!(&raw.photometric, Photo::Cfa(c) if raw.cpp == 1 && bayer_sites(&c.cfa).is_some())
}

/// The slider value at which the stage runs at the photo's own `k`:
/// Lightroom's own default for Color.
pub(crate) const DEFAULT_AMOUNT: f32 = 25.0;

/// The multiple of the photo's `k` the slider value `color_nr` (0..=100)
/// asks for: `None` when off (the colour-NR control's own rule,
/// `ChromaNrParams::global`).
pub(crate) fn scale_of(color_nr: f32) -> Option<f64> {
    (color_nr > 0.0).then(|| f64::from(color_nr.min(100.0)) / f64::from(DEFAULT_AMOUNT))
}

/// The number of pixels the display pass averages on a flat region at slider
/// value `color_nr` (0..=100), Smoothness at Lightroom's own default: the
/// plain mean that leaves the same share of white noise,
/// `1 / render::chroma_nr_flat_noise_share`.
pub(crate) fn display_samples(color_nr: f32) -> f64 {
    let smooth = crate::recipe::LR_COMPANION_DEFAULTS.iter().find(|(n, _)| *n == "color_nr_smooth").expect("Smoothness is a companion").1;
    1.0 / crate::render::chroma_nr_flat_noise_share(color_nr.clamp(0.0, 100.0) / 100.0, smooth / 100.0)
}

/// The half side in cells of the square holding `n` cells, `(√n − 1)/2`
/// rounded, at least one cell.
fn half_side(n: f64) -> usize {
    ((n.sqrt() - 1.0) / 2.0).round().max(1.0) as usize
}

/// The plain mean's window at slider value `color_nr` (0..=100): the square
/// of cells that holds as many samples as the display pass averages
/// ([`display_samples`] pixels, four to a cell). The window `k` is read over.
pub(crate) fn plain_window(color_nr: f32) -> usize {
    half_side(display_samples(color_nr) / 4.0)
}

/// The standard normal's mass within `±t` and its density at `t` (Simpson's
/// rule on `0..t` in 2000 steps, exact to 1e-12 at any `t` the stage reads;
/// `(1, 0)` at an unbounded `t`).
pub(crate) fn normal_mass(t: f64) -> (f64, f64) {
    let phi = |x: f64| (-x * x / 2.0).exp() / std::f64::consts::TAU.sqrt();
    if !t.is_finite() {
        return (1.0, 0.0);
    }
    let steps = 2000;
    let h = t / steps as f64;
    let inner: f64 = (1..steps).map(|i| phi(i as f64 * h) * if i % 2 == 1 { 4.0 } else { 2.0 }).sum();
    (2.0 * (phi(0.0) + phi(t) + inner) * h / 3.0, phi(t))
}

/// The window's half side in cells at slider value `color_nr` (0..=100) for
/// the stage running at `k` (in `σ_d`): the `N` cells at which the skipped
/// mean at its test width `t = k·√(1 + 1/n)` (in one cell's `σ`, against an
/// estimate of `n = N·Z(t)` samples) leaves the display pass's share,
/// `N·(Z(t) − 2t·φ(t)) = display_samples / 4` (see the module); `t` and `N`
/// depend on each other, and are read in turn until the half side repeats.
/// Unbounded `k` is the plain mean's window.
pub(crate) fn window(color_nr: f32, k: f64) -> usize {
    let plain = display_samples(color_nr) / 4.0;
    if !k.is_finite() {
        return half_side(plain);
    }
    let (mut t, mut half) = (k, 0);
    loop {
        let (z, phi) = normal_mass(t);
        let n = plain / (z - 2.0 * t * phi);
        t = k * (1.0 + 1.0 / (n * z)).sqrt();
        let next = half_side(n);
        if next == half {
            return half;
        }
        half = next;
    }
}

/// Run the stage on the normalised signed `mosaic` (`width × height`, sites
/// of `cfa`), white-balanced by `wb` (any scale; green is divided out), at
/// `scale` times the photo's own `k` ([`photo_k`], read over the plain
/// mean's window at the default strength, [`plain_window`]) over a window of
/// half side `rho(k)` cells for the `k` it runs at ([`window`]). Returns the fitted model, the `k` it ran at and
/// the filtered cell chroma the demosaiced pixels are rebuilt on
/// ([`CellChroma::rebuild`]), or `None` — and the mosaic untouched — when the
/// pattern is not a 2×2 RGB Bayer or the frame has too few usable patches for
/// a model.
pub(crate) fn filter_mosaic(
    mosaic: &mut [f32],
    width: usize,
    height: usize,
    cfa: &rawler::cfa::CFA,
    wb: [f64; 3],
    scale: f64,
    rho: impl Fn(f64) -> usize,
) -> Option<(NoiseModel, f64, CellChroma)> {
    let sites = bayer_sites(cfa)?;
    if mosaic.len() != width * height || wb[1].is_nan() || wb[1] <= 0.0 || scale.is_nan() || scale <= 0.0 {
        return None;
    }
    let wb = [wb[0] / wb[1], 1.0, wb[2] / wb[1]];
    let pts = patches(mosaic, width, height, sites, wb);
    let model = NoiseModel::fit(&pts, width, height)?;
    let cells = Cells::of(mosaic, width, height, sites, wb, &model, &pts)?;
    let k = photo_k(&cells, &model, &pts, wb, plain_window(DEFAULT_AMOUNT)).0 * scale;
    let rho = rho(k);
    let filtered: Vec<[f32; 2]> = (0..cells.cw * cells.ch).into_par_iter().map(|i| cells.filtered(i, k, rho)).collect();
    cells.write(mosaic, width, sites, wb, &filtered);
    let chroma = CellChroma {
        cw: cells.cw,
        ch: cells.ch,
        wb: [wb[0] as f32, wb[2] as f32],
        planes: [0, 1].map(|j| filtered.iter().map(|f| f[j]).collect()),
    };
    Some((model, k, chroma))
}

/// The stage's filtered cell chroma, `wR·R − G` and `wB·B − G` per cell of
/// the mosaic, the planes the demosaiced pixels are rebuilt on.
pub(crate) struct CellChroma {
    cw: usize,
    ch: usize,
    /// `[wR, wB]`, green = 1.
    wb: [f32; 2],
    planes: [Vec<f32>; 2],
}

impl CellChroma {
    /// Rebuild the demosaiced camera-native `rgb` (`width` wide, its pixel
    /// `(0, 0)` at mosaic site `(ox, oy)`): the sensor measures colour once
    /// per cell; pixel-scale chroma is the demosaic's, so the stage's cell
    /// chroma is the pixel's. Each pixel keeps its green and takes
    /// `R = (G + c_R)/wR`, `B = (G + c_B)/wB`, `c` the cell chroma read at the
    /// pixel's centre bilinearly in the cell grid (`render::upsample_at` at
    /// 2×, the display pass's own coordinate).
    pub(crate) fn rebuild(&self, rgb: &mut [[f32; 3]], width: usize, ox: usize, oy: usize) {
        let (cw, ch) = (self.cw, self.ch);
        let [wr, wbl] = self.wb;
        rgb.par_chunks_mut(width.max(1)).enumerate().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let (sx, sy) = (x + ox, y + oy);
                let cr = crate::render::upsample_at(&self.planes[0], cw, ch, 2, sx, sy);
                let cb = crate::render::upsample_at(&self.planes[1], cw, ch, 2, sx, sy);
                px[0] = (px[1] + cr) / wr;
                px[2] = (px[1] + cb) / wbl;
            }
        });
    }
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

/// The sigma filter's estimate of the true value of `centre` (one sample of
/// noise sd `sigma`) over `window` (its samples, the centre among them):
/// starting from the sample itself, a mean of `n = 1`, the mean of the window's
/// samples within `k·σ·√(1 + 1/n)` of the current estimate — a sample against
/// a mean of `n` samples differs by that sd — until an estimate repeats (a
/// fixed point, or a cycle: from there the passes only repeat; the estimates
/// are means of subsets of a finite window, so a repeat comes). Returns the
/// estimate and the `n` of the last pass. The first pass accepts the centre
/// (|0| ≤ t), so `n ≥ 1`; a later pass that accepts no sample leaves the
/// estimate it was anchored on, itself a mean of accepted samples.
pub(crate) fn skipped_mean(window: &[f32], centre: f32, sigma: f64, k: f64) -> (f32, u32) {
    let (mut estimate, mut n) = (centre, 1u32);
    let mut seen: Vec<u32> = Vec::new();
    loop {
        let t = (k * sigma * (1.0 + 1.0 / f64::from(n)).sqrt()) as f32;
        let (mut sum, mut m) = (0.0f64, 0u32);
        for &q in window {
            if (q - estimate).abs() <= t {
                sum += f64::from(q);
                m += 1;
            }
        }
        if m == 0 {
            return (estimate, n);
        }
        let next = (sum / f64::from(m)) as f32;
        n = m;
        if seen.contains(&next.to_bits()) {
            return (next, n);
        }
        seen.push(next.to_bits());
        estimate = next;
    }
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

    /// Cell `i`'s two colour differences through the sigma filter at `k`:
    /// the test is anchored on an estimate of the cell's true colour, not on
    /// its own noisy sample (see the module).
    fn filtered(&self, i: usize, k: f64, rho: usize) -> [f32; 2] {
        self.filtered_counted(i, k, rho).0
    }

    /// [`Self::filtered`] with the count of window samples the last pass accepted.
    fn filtered_counted(&self, i: usize, k: f64, rho: usize) -> ([f32; 2], [u32; 2]) {
        let (cw, ch) = (self.cw, self.ch);
        let (cx, cy) = (i % cw, i / cw);
        let sd = self.sd[(cy / PATCH).min(self.ph - 1) * self.pw + (cx / PATCH).min(self.pw - 1)];
        let (y0, y1, x0, x1) = (cy.saturating_sub(rho), (cy + rho + 1).min(ch), cx.saturating_sub(rho), (cx + rho + 1).min(cw));
        let mut window = Vec::with_capacity((y1 - y0) * (x1 - x0));
        let (mut out, mut count) = ([0.0f32; 2], [0u32; 2]);
        for j in 0..2 {
            window.clear();
            window.extend((y0..y1).flat_map(|y| self.d[y * cw + x0..y * cw + x1].iter().map(move |q| q[j])));
            // One cell's sd is `σ_d / √2`.
            (out[j], count[j]) = skipped_mean(&window, self.d[i][j], f64::from(sd[j]) / 2f64.sqrt(), k);
        }
        (out, count)
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
