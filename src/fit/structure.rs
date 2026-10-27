//! Structural divergence between the two renditions: the per-mode measure, its coarse form, the divergence pair and the pairing scale.

use super::*;

/// The smallest eroded core [`structure_divergence`] will read. The
/// translation search inside it already refuses any shift supported by fewer
/// than this many paired samples, so below this size the correlation half of
/// the statistic has no support at any offset and the reading would be the
/// constant its own guard produces rather than a measurement.
pub const STRUCTURE_MIN_CORE_PX: usize = 100;

/// Tone-invariant structural comparison shared by the global and zoned
/// solvers. Luma is rank-equalized through a mask-weighted 1024-bin CDF; a
/// three-pixel erosion keeps semantic-mask boundaries out of the reading.
/// Central-difference rank-gradient maps are pooled at sigma 2 and correlated
/// over translations of +/-6 pixels. Five Gaussian bands (sigma 1/2/4/8/16)
/// contribute the RMS log2 energy-ratio error.
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
        let mut acc = 0.0f64;
        for v in &mut hist {
            acc += *v;
            *v = acc / total;
        }
        bins.into_iter().map(|i| hist[i] as f32).collect()
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
        let radius = (3.0 * sigma).ceil().max(1.0) as isize;
        let mut kernel = Vec::with_capacity((2 * radius + 1) as usize);
        for i in -radius..=radius {
            kernel.push((-(i * i) as f32 / (2.0 * sigma * sigma)).exp());
        }
        let sum: f32 = kernel.iter().sum();
        for v in &mut kernel {
            *v /= sum;
        }
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

    let signature = |rank: &[f32]| -> (Vec<f32>, [f32; 5]) {
        let blurred: Vec<Vec<f32>> =
            [1.0, 2.0, 4.0, 8.0, 16.0].iter().map(|&s| gaussian_blur(rank, s)).collect();
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
    let energy_error = (src_energy
        .iter()
        .zip(tgt_energy)
        .map(|(&a, b)| ((b + 1e-6) / (a + 1e-6)).log2().powi(2))
        .sum::<f32>()
        / 5.0)
        .sqrt();
    let d = ((1.0 - correlation).powi(2) + energy_error.powi(2)).sqrt();
    Some(Divergence { correlation, energy_error, d })
}

/// Separable Gaussian low-pass of an analysis raster, tap weights RENORMALISED
/// at the border.
///
/// [`structure_divergence`]'s own band blurs truncate instead, which is
/// correct there — the bands are differences of two truncated blurs, so the
/// border deficit cancels. It does NOT cancel when the blur is the reading's
/// input: a truncated sigma-8 pass darkens the outer 24 px of both frames by
/// the same vignette, both rank-equalise it into the same lowest ranks, and
/// the correlation half of the statistic is handed agreement it did not
/// measure. Renormalising costs one division per tap and states no border
/// fact at all.
fn low_pass_pixels(px: &[[f32; 3]], w: usize, h: usize, sigma: f32) -> Vec<[f32; 3]> {
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
    let presence = target.color().has_alpha().then(|| presence_of(&target.thumbnail_exact(w, h))).flatten();
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
