//! Structural divergence between the two renditions: the per-mode measure, its coarse form, the divergence pair and the pairing scale.

use super::*;

/// The smallest eroded core [`structure_divergence`] will read. The
/// translation search inside it already refuses any shift supported by fewer
/// than this many paired samples, so below this size the correlation half of
/// the statistic has no support at any offset and the reading would be the
/// constant its own guard produces rather than a measurement.
pub const STRUCTURE_MIN_CORE_PX: usize = 100;

/// Tone-invariant structural comparison shared by the global and zoned
/// solvers. Luma is rank-equalized through a mask-weighted 1024-bin CDF
/// under the one-code tie rule (see `rank_equalized` below). A reading is
/// made of its mask's own pixels at every scale (DEF-3): the five band
/// blurs are NORMALISED MASKED blurs, `blur(m·x) / blur(m)` with `m` the
/// weights, so a σ 16 band (reach 48 px) reads no pixel the mask leaves out;
/// a three-pixel erosion keeps the core's central differences inside the
/// mask. Central-difference rank-gradient maps are pooled at sigma 2 and
/// correlated over translations of +/-6 pixels. Five Gaussian bands (sigma
/// 1/2/4/8/16) contribute the RMS log2 energy-ratio error.
///
/// `None` is an ABSTENTION and never a matched reading. It is returned when
/// the rasters are not one geometry, or when the eroded core holds fewer than
/// [`STRUCTURE_MIN_CORE_PX`] pixels. Both used to return
/// `Divergence::matched()`, i.e. D = 0 — the value every consumer reads as
/// "the structure survived" — so a free-mask component of 90 core pixels
/// cleared the structural gate for free and [`crate::fit_field::local_support`]
/// handed an unmeasurable cell the full 1.0 support of a measured match.
/// A caller answers its own question without a reading: a gate that needs
/// "the structure survived" REFUSES, a decision that needs "the structure was
/// destroyed" does not fire, and a per-cell modulation drops the unread cell
/// — or, where no cell in the frame resolves, drops the modulation wholesale
/// rather than withholding evidence over a frame nothing measured.
pub fn structure_divergence(
    src_px: &[[f32; 3]],
    tgt_px: &[[f32; 3]],
    w: u32,
    h: u32,
    weights: &[f32],
) -> Option<Divergence> {
    let n = w as usize * h as usize;
    if n == 0 || src_px.len() != n || tgt_px.len() != n || weights.len() != n {
        return None;
    }

    let rank_equalized = |px: &[[f32; 3]]| -> Vec<f32> {
        let mut hist = [0.0f64; HIST_BINS];
        let mut bins = Vec::with_capacity(n);
        for (p, &weight) in px.iter().zip(weights) {
            let bin = (luma601(p).clamp(0.0, 1.0) * (HIST_BINS - 1) as f32).round() as usize;
            bins.push(bin);
            hist[bin] += weight.max(0.0) as f64;
        }
        let total = hist.iter().sum::<f64>();
        if total <= 1e-12 {
            return vec![0.0; n];
        }
        // THE TIE RULE (v1.18.0): two luma values within one output code are
        // a tie; a tie is charged in proportion to the fraction of a code
        // that separates them, never as a whole rank step. The render's
        // quantum is one 8-bit code, `CODE` = 1023/255 = 4.012 of the
        // histogram's bins, and below it two values are not told apart by
        // anything the fit ships. The rank of a bin is therefore its
        // MID-RANK (the mass below it plus half its own) averaged over a box
        // exactly one code wide centred on it — the expected rank of a value
        // known to one code. A flat block whose render varies by a fraction
        // of a code (a clipped sky, a crushed shadow, a posterised or
        // generated target) keeps one rank up to that fraction, where the
        // plain cumulative CDF split it by bin into ranks a whole block's
        // mass apart; continuous luma keeps a continuous rank, where giving
        // every value of a code ONE rank posterises the rank image into
        // code-wide contours the low-passed reading then sees as structure
        // (rel1171 msg10 §1: that form read the coarse Cornwall panel 0.183
        // against this form's 0.108).
        let mut below = 0.0f64;
        let mut mid = [0.0f64; HIST_BINS];
        for (m, &mass) in mid.iter_mut().zip(&hist) {
            *m = (below + 0.5 * mass) / total;
            below += mass;
        }
        const CODE: f64 = (HIST_BINS - 1) as f64 / 255.0;
        let reach = (CODE / 2.0).floor() as isize;
        // The two end taps carry what is left of the code's width, so the
        // box is exactly CODE bins wide and its taps sum to CODE.
        let edge = (CODE - (2 * reach - 1) as f64) / 2.0;
        let rank: Vec<f64> = (0..HIST_BINS as isize)
            .map(|b| {
                (-reach..=reach)
                    .map(|k| {
                        let tap = if k.abs() == reach { edge } else { 1.0 };
                        tap * mid[(b + k).clamp(0, HIST_BINS as isize - 1) as usize]
                    })
                    .sum::<f64>()
                    / CODE
            })
            .collect();
        bins.into_iter().map(|i| rank[i] as f32).collect()
    };

    let mut core: Vec<bool> = weights.iter().map(|&v| v > 0.8).collect();
    let (wu, hu) = (w as usize, h as usize);
    for _ in 0..3 {
        let mut next = core.clone();
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                next[i] = x > 0
                    && x + 1 < wu
                    && y > 0
                    && y + 1 < hu
                    && core[i]
                    && core[i - 1]
                    && core[i + 1]
                    && core[i - wu]
                    && core[i + wu];
            }
        }
        core = next;
    }
    if core.iter().filter(|&&v| v).count() < STRUCTURE_MIN_CORE_PX {
        return None;
    }

    let gaussian_blur = |input: &[f32], sigma: f32| -> Vec<f32> {
        let kernel = band_kernel(sigma);
        let radius = (kernel.len() / 2) as isize;
        let mut tmp = vec![0.0f32; n];
        let mut out = vec![0.0f32; n];
        for y in 0..hu {
            for x in 0..wu {
                let mut v = 0.0f32;
                for (ki, &kv) in kernel.iter().enumerate() {
                    let sx = x as isize + ki as isize - radius;
                    if (0..wu as isize).contains(&sx) {
                        v += input[y * wu + sx as usize] * kv;
                    }
                }
                tmp[y * wu + x] = v;
            }
        }
        for y in 0..hu {
            for x in 0..wu {
                let mut v = 0.0f32;
                for (ki, &kv) in kernel.iter().enumerate() {
                    let sy = y as isize + ki as isize - radius;
                    if (0..hu as isize).contains(&sy) {
                        v += tmp[sy as usize * wu + x] * kv;
                    }
                }
                out[y * wu + x] = v;
            }
        }
        out
    };

    // DEF-3: the bands read inside the mask. A truncated blur of the whole
    // rank image fed the core's coarse bands from up to 48 px past the mask
    // (a cell fully inside the kept two thirds of a frame read energy_error
    // 0.9589 from the replaced third beside it, 0.0000 without it); the
    // normalised masked blur weighs each tap by the mask, so outside pixels
    // contribute nothing and the image border needs no rule of its own.
    let mask: Vec<f32> = weights.iter().map(|v| v.max(0.0)).collect();
    let mask_blur: Vec<Vec<f32>> = BAND_SIGMAS.iter().map(|&s| gaussian_blur(&mask, s)).collect();
    let masked_blur = |input: &[f32], band: usize, sigma: f32| -> Vec<f32> {
        let weighted: Vec<f32> = input.iter().zip(&mask).map(|(v, m)| v * m).collect();
        gaussian_blur(&weighted, sigma)
            .into_iter()
            .zip(&mask_blur[band])
            .map(|(v, m)| if *m > 1e-12 { v / m } else { 0.0 })
            .collect()
    };
    let signature = |rank: &[f32]| -> (Vec<f32>, [f32; 5]) {
        let blurred: Vec<Vec<f32>> = BAND_SIGMAS
            .iter()
            .enumerate()
            .map(|(band, &s)| masked_blur(rank, band, s))
            .collect();
        let mut energy = [0.0f32; 5];
        let count = core.iter().filter(|&&v| v).count() as f64;
        for band in 0..5 {
            let mut sum = 0.0f64;
            for i in 0..n {
                if !core[i] {
                    continue;
                }
                let v = if band == 0 {
                    rank[i] - blurred[0][i]
                } else {
                    blurred[band - 1][i] - blurred[band][i]
                };
                sum += (v * v) as f64;
            }
            energy[band] = (sum / count).sqrt() as f32;
        }
        let mut gradient = vec![0.0f32; n];
        for y in 1..hu.saturating_sub(1) {
            for x in 1..wu.saturating_sub(1) {
                let i = y * wu + x;
                let dx = 0.5 * (rank[i + 1] - rank[i - 1]);
                let dy = 0.5 * (rank[i + wu] - rank[i - wu]);
                gradient[i] = (dx * dx + dy * dy).sqrt();
            }
        }
        (gaussian_blur(&gradient, 2.0), energy)
    };

    let src_rank = rank_equalized(src_px);
    let tgt_rank = rank_equalized(tgt_px);
    let (src_gradient, src_energy) = signature(&src_rank);
    let (tgt_gradient, tgt_energy) = signature(&tgt_rank);

    let mut best = -1.0f64;
    for dy in -6isize..=6 {
        for dx in -6isize..=6 {
            let mut count = 0usize;
            let (mut sx_sum, mut ty_sum) = (0.0f64, 0.0f64);
            for y in 0..hu {
                for x in 0..wu {
                    let i = y * wu + x;
                    let sy = y as isize - dy;
                    let sx = x as isize - dx;
                    if core[i]
                        && (0..hu as isize).contains(&sy)
                        && (0..wu as isize).contains(&sx)
                    {
                        sx_sum += src_gradient[sy as usize * wu + sx as usize] as f64;
                        ty_sum += tgt_gradient[i] as f64;
                        count += 1;
                    }
                }
            }
            if count < STRUCTURE_MIN_CORE_PX {
                continue;
            }
            let (sx_mean, ty_mean) = (sx_sum / count as f64, ty_sum / count as f64);
            let (mut cross, mut sa, mut sb) = (0.0f64, 0.0f64, 0.0f64);
            for y in 0..hu {
                for x in 0..wu {
                    let i = y * wu + x;
                    let sy = y as isize - dy;
                    let sx = x as isize - dx;
                    if core[i]
                        && (0..hu as isize).contains(&sy)
                        && (0..wu as isize).contains(&sx)
                    {
                        let a = src_gradient[sy as usize * wu + sx as usize] as f64 - sx_mean;
                        let b = tgt_gradient[i] as f64 - ty_mean;
                        cross += a * b;
                        sa += a * a;
                        sb += b * b;
                    }
                }
            }
            let den = (sa * sb).sqrt();
            if den > 0.0 {
                best = best.max(cross / den);
            }
        }
    }
    // No translation offered gradient variance on BOTH sides (one side, or
    // both, flat inside the core): the correlation term is not a
    // measurement and contributes NOTHING to D — 1.0 is the value that
    // zeroes it, not a claim that the structure survived. What D then reads
    // is the band-energy ratio alone, and that term is the whole structural
    // evidence such a pair offers: a uniform patch that stayed uniform reads
    // D = 0, a checkerboard that became flat reads far past DIVERGENCE_ZONE.
    // The tile stage depends on exactly this (`fit_zoned::spatial::tests::
    // depth_three_never_attaches`, `…cannot_become_a_tile`), which is why the
    // 2026-09-24 audit's proposal to abstain here was tried and withdrawn:
    // an abstention refuses the uniform tile and, through `is_some_and`,
    // lets a flattened target through the mode gate as "not divergent".
    let correlation = if best > -1.0 { best as f32 } else { 1.0 };
    let core_count = core.iter().filter(|&&v| v).count();
    let energy_error = band_energy_error(&src_energy, &tgt_energy, &band_energy_floors(core_count));
    let d = ((1.0 - correlation).powi(2) + energy_error.powi(2)).sqrt();
    Some(Divergence { correlation, energy_error, d })
}

/// The five band sigmas of [`structure_divergence`].
const BAND_SIGMAS: [f32; 5] = [1.0, 2.0, 4.0, 8.0, 16.0];

/// The normalised, truncated (reach ceil(3 sigma)) Gaussian taps every blur
/// of [`structure_divergence`] uses — the same taps its band floor
/// ([`band_energy_floors`]) is derived from.
fn band_kernel(sigma: f32) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil().max(1.0) as isize;
    let mut kernel: Vec<f32> = (-radius..=radius).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    for v in &mut kernel {
        *v /= sum;
    }
    kernel
}

/// The band-energy ratio of two readings, each band read no lower than its
/// floor: `sqrt(mean_b log2(max(E_t,b, f_b) / max(E_s,b, f_b))^2)`.
///
/// A reading has a floor at its own resolution: a band the ranks cannot
/// resolve reads as empty, not as zero. Below `f_b` (see
/// [`band_energy_floors`]) a band's energy is what the rank's own
/// quantisation leaves in it, so two such bands are the same empty band
/// whatever their ratio, and one such band against a resolved one reads as
/// far from it as the floor, not as far as zero. The floor replaces the
/// 1e-6 offset the ratio used to carry, which was no reading's resolution:
/// before DEF-3 a truncated blur's border vignette had stood in for one.
pub(crate) fn band_energy_error(source: &[f32; 5], target: &[f32; 5], floor: &[f32; 5]) -> f32 {
    (source
        .iter()
        .zip(target)
        .zip(floor)
        .map(|((&a, &b), &f)| (b.max(f) / a.max(f)).log2().powi(2))
        .sum::<f32>()
        / 5.0)
        .sqrt()
}

/// The resolution of [`structure_divergence`]'s band energies over a core of
/// `core` pixels, per band. DERIVATION — nothing in it is chosen:
///
/// * The rank image is read through a 1024-bin CDF: a rank is known to one
///   bin's width of rank, `1/HIST_BINS`, so it carries a uniform rounding
///   error of standard deviation `q = 1/(HIST_BINS·sqrt(12))` per sample,
///   independent from pixel to pixel.
/// * Band `b` is a linear filter `h_b` of the rank image: band 0 is
///   `δ − G_1`, band `k ≥ 1` is `G_σ(k−1) − G_σ(k)` with the taps of
///   [`band_kernel`] — separable, so `h = A⊗A − B⊗B` for 1-D taps `A`, `B`
///   (`A = δ` for band 0). White noise of variance `q²` leaves variance
///   `q²·Σh²` in it per sample, and `Σh² = (h⋆h)(0) = P₀² − 2Q₀² + R₀²`
///   with `P = A⋆A`, `Q = A⋆B`, `R = B⋆B` (1-D correlations, centre taps).
///   That is the energy² a band holding nothing but the quantum reads:
///   `E_q,b² = q²·Σh_b²`.
/// * The energy is the mean of `v²` over the core's `N` samples. They are
///   not independent — neighbours share the filter — so the mean's own
///   scatter is `E_q,b² · sqrt(2·Σρ_b²/N)` (Gaussian `v`; `ρ = h⋆h / Σh²`
///   the band's autocorrelation, `Σρ²` its correlation area in samples),
///   with `Σ(h⋆h)² = (P·P)² + 4(Q·Q)² + (R·R)² − 4(P·Q)² + 2(P·R)² − 4(Q·R)²`
///   for the separable form.
/// * The floor is the quantum's reading at one standard error of its own
///   estimate on this population: `f_b = E_q,b · sqrt(1 + sqrt(2·Σρ_b²/N))`.
///   A coarse band on a small core resolves less (larger `Σρ²/N`), so its
///   floor is higher there.
///
/// Read in the mask's interior: near a mask edge the normalised masked blur
/// has fewer taps, its `Σh²` is larger, and this floor is a lower bound.
pub(crate) fn band_energy_floors(core: usize) -> [f32; 5] {
    fn correlate(a: &[f64], b: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; a.len() + b.len() - 1];
        for (i, x) in a.iter().enumerate() {
            for (j, y) in b.iter().enumerate() {
                out[i + j] += x * y;
            }
        }
        out
    }
    // The dot product of two centred odd-length sequences.
    fn dot(a: &[f64], b: &[f64]) -> f64 {
        let (ca, cb) = ((a.len() / 2) as isize, (b.len() / 2) as isize);
        let reach = ca.min(cb);
        (-reach..=reach).map(|k| a[(ca + k) as usize] * b[(cb + k) as usize]).sum()
    }
    let centre = |v: &[f64]| v[v.len() / 2];
    let taps = |sigma: f32| band_kernel(sigma).into_iter().map(f64::from).collect::<Vec<f64>>();
    let quantum = 1.0 / (HIST_BINS as f64 * 12f64.sqrt());
    let n = core.max(1) as f64;
    let mut floor = [0.0f32; 5];
    for (band, f) in floor.iter_mut().enumerate() {
        let a = if band == 0 { vec![1.0] } else { taps(BAND_SIGMAS[band - 1]) };
        let b = taps(BAND_SIGMAS[band]);
        let (p, q, r) = (correlate(&a, &a), correlate(&a, &b), correlate(&b, &b));
        let sum_h2 = centre(&p).powi(2) - 2.0 * centre(&q).powi(2) + centre(&r).powi(2);
        let sum_hh2 = dot(&p, &p).powi(2) + 4.0 * dot(&q, &q).powi(2) + dot(&r, &r).powi(2)
            - 4.0 * dot(&p, &q).powi(2)
            + 2.0 * dot(&p, &r).powi(2)
            - 4.0 * dot(&q, &r).powi(2);
        let area = sum_hh2 / (sum_h2 * sum_h2);
        let e_q2 = quantum * quantum * sum_h2;
        *f = (e_q2 * (1.0 + (2.0 * area / n).sqrt())).sqrt() as f32;
    }
    floor
}

/// Separable Gaussian low-pass of an analysis raster, tap weights RENORMALISED
/// at the border.
///
/// [`structure_divergence`]'s band blurs are renormalised too since DEF-3 —
/// by the reading's mask, of which the frame border is one edge — so no
/// blur in this file states a border fact. Truncated, a blur that is the
/// reading's input does not cancel its border deficit the way a difference
/// of two truncated bands can: a truncated sigma-8 pass darkens the outer 24 px of both frames by
/// the same vignette, both rank-equalise it into the same lowest ranks, and
/// the correlation half of the statistic is handed agreement it did not
/// measure. Renormalising costs one division per tap and states no border
/// fact at all.
pub(super) fn low_pass_pixels(px: &[[f32; 3]], w: usize, h: usize, sigma: f32) -> Vec<[f32; 3]> {
    let n = w * h;
    let radius = (3.0 * sigma).ceil().max(1.0) as isize;
    let kernel: Vec<f32> = (-radius..=radius)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let mut tmp = vec![[0.0f32; 3]; n];
    let mut out = vec![[0.0f32; 3]; n];
    for y in 0..h {
        for x in 0..w {
            let (mut acc, mut mass) = ([0.0f32; 3], 0.0f32);
            for (ki, &kv) in kernel.iter().enumerate() {
                let sx = x as isize + ki as isize - radius;
                if (0..w as isize).contains(&sx) {
                    let p = px[y * w + sx as usize];
                    for (slot, v) in acc.iter_mut().zip(p) {
                        *slot += v * kv;
                    }
                    mass += kv;
                }
            }
            tmp[y * w + x] = acc.map(|v| v / mass.max(1e-12));
        }
    }
    for y in 0..h {
        for x in 0..w {
            let (mut acc, mut mass) = ([0.0f32; 3], 0.0f32);
            for (ki, &kv) in kernel.iter().enumerate() {
                let sy = y as isize + ki as isize - radius;
                if (0..h as isize).contains(&sy) {
                    let p = tmp[sy as usize * w + x];
                    for (slot, v) in acc.iter_mut().zip(p) {
                        *slot += v * kv;
                    }
                    mass += kv;
                }
            }
            out[y * w + x] = acc.map(|v| v / mass.max(1e-12));
        }
    }
    out
}

/// [`structure_divergence`] of the same pair at LAYOUT scale: both sides are
/// low-passed at [`COARSE_SIGMA_DIVISOR`] first, then handed to the unchanged
/// instrument with the same weights and the same abstention.
///
/// The two readings answer two different questions and the crate needs both.
/// PIXEL pairing (a per-pixel target, a per-pixel voucher, a paired tone
/// regression) is only valid where the fine reading says the texture survived.
/// CELL and population statistics need only the layout: a generative repaint
/// that re-synthesises every cloud but leaves the horizon, the ridge and the
/// building where they were has replaced the pixels and KEPT the frame, and
/// reading the fine number as "this is a different scene" is the wrong
/// question asked of tone and colour evidence.
pub(crate) fn structure_divergence_coarse(
    src_px: &[[f32; 3]],
    tgt_px: &[[f32; 3]],
    w: u32,
    h: u32,
    weights: &[f32],
) -> Option<Divergence> {
    let n = w as usize * h as usize;
    if n == 0 || src_px.len() != n || tgt_px.len() != n || weights.len() != n {
        return None;
    }
    let sigma = (w.max(h) as f32 / COARSE_SIGMA_DIVISOR).max(1.0);
    let (wu, hu) = (w as usize, h as usize);
    structure_divergence(
        &low_pass_pixels(src_px, wu, hu, sigma),
        &low_pass_pixels(tgt_px, wu, hu, sigma),
        w,
        h,
        weights,
    )
}

/// The two structural readings of ONE pair, measured together on one raster.
///
/// Threaded rather than re-measured: the mode line, the evidence model's range
/// survival, the pairing-scale choice and the disclosure all read the same two
/// numbers, and a second `structure_divergence_for` call on a re-resampled
/// frame is how the crate ended up with two independent readings of the same
/// statistic disagreeing about the same pair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DivergencePair {
    /// Pixel-scale agreement: did the texture survive?
    pub fine: Option<Divergence>,
    /// Layout-scale agreement: is the scene still in the same place?
    pub coarse: Option<Divergence>,
}

impl DivergencePair {
    /// The pairing scale this reading admits. `Pixel` needs the FINE reading
    /// to hold; nothing else does. An ABSTENTION is not a match: an unread
    /// fine reading cannot authorise per-pixel pairing.
    pub(crate) fn scale(&self) -> PairingScale {
        if self.fine.is_some_and(|r| r.d < DIVERGENCE_GLOBAL) {
            PairingScale::Pixel
        } else {
            PairingScale::Cell
        }
    }
}

/// At which scale this solve may pair a source sample with a target sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingScale {
    /// Per-pixel: index `i` on one side IS index `i` on the other.
    Pixel,
    /// Per-cell: only cell statistics correspond, never individual pixels.
    Cell,
}

impl PairingScale {
    /// The one word the CLI prints for this scale.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pixel => "pixel",
            Self::Cell => "cell",
        }
    }
}

/// One pair on the common analysis grid, as every structure reading takes it.
pub(crate) struct DivergenceRaster {
    /// The source, developed under the base recipe, row-major.
    pub source: Vec<[f32; 3]>,
    /// The target as it was handed in, row-major on the same grid.
    pub target: Vec<[f32; 3]>,
    pub width: u32,
    pub height: u32,
    /// Which cells the target's field covers ([`presence_of`]); `None` when the
    /// target carries no alpha, i.e. it is present everywhere.
    pub presence: Option<Vec<f32>>,
}

/// The common, pixel-aligned 384×256 analysis raster used by both scopes and
/// by the calibration prototype. Both sides are Lanczos-resampled onto that
/// one grid and the source is placed in the calibration/base domain before
/// ranks are measured.
///
/// Presence is read off a BOX thumbnail of the target rather than the Lanczos
/// grid: the kernel's lobes would carry the clear ring a pixel or two into the
/// field and back out, and presence is a fact about which cells the field
/// covers.
pub(crate) fn divergence_raster(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
) -> DivergenceRaster {
    let (w, h) = (ANALYZE_EDGE, ANALYZE_EDGE * 2 / 3);
    let src_grid = src.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    let tgt_grid = target.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    let src_base = render::develop_preview(&src_grid, base);
    let presence = target.color().has_alpha().then(|| presence_of(&crate::render::thumbnail_exact(target, w, h))).flatten();
    DivergenceRaster { source: pixels_of(&src_base), target: pixels_of(&tgt_grid), width: w, height: h, presence }
}

/// [`structure_divergence`] over the pair's own analysis raster, carrying that
/// function's abstention: `None` is "not measured", never "matched".
pub(crate) fn structure_divergence_for(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
    weights: Option<&[f32]>,
) -> Option<Divergence> {
    let DivergenceRaster { source: sp, target: tp, width: w, height: h, presence } = divergence_raster(src, target, base);
    let all;
    let weights = match weights {
        Some(v) => v,
        None => {
            all = vec![1.0; sp.len()];
            &all
        }
    };
    structure_divergence(&sp, &tp, w, h, &with_presence(weights, presence.as_deref()))
}

/// Both readings of one pair off ONE [`divergence_raster`] build.
pub(crate) fn divergence_pair_for(
    src: &DynamicImage,
    target: &DynamicImage,
    base: &EditRecipe,
) -> DivergencePair {
    let DivergenceRaster { source: sp, target: tp, width: w, height: h, presence } = divergence_raster(src, target, base);
    let weights = presence.unwrap_or_else(|| vec![1.0; sp.len()]);
    DivergencePair {
        fine: structure_divergence(&sp, &tp, w, h, &weights),
        coarse: structure_divergence_coarse(&sp, &tp, w, h, &weights),
    }
}
