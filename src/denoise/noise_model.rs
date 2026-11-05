//! The mosaic's own noise, measured on the photograph (E3 of v1.18.0): the
//! model a colour noise reduction that follows the noise reads its threshold
//! from, and the rule that says where a patch of the frame is flat.
//!
//! **The estimator.** The two green sites of one 2×2 Bayer cell sit one pixel
//! apart and see the same scene, so half the variance of their difference is
//! the green noise variance at that level with, to first order, no scene in
//! it. It is read per patch of [`PATCH`]×[`PATCH`] cells against the patch's
//! mean green.
//!
//! **The model.** Photon and read noise of the SENSOR signal `s` give
//! `a·s + b`. A camera may multiply the mosaic by a gain `g` before storing
//! it (shading compensation): then the stored level is `m = g·s` and its
//! variance `g²(a·s + b) = g·a·m + g²·b`. The gain is radial,
//! `g = 1 + c2·r² + c4·r⁴` (`r` = distance from the frame's centre over the
//! half diagonal), the first two terms of the DNG specification's radial
//! vignette polynomial (`FixVignetteRadial`); a camera that applies no gain
//! fits `c2 = c4 = 0`. Measured on the corpus (rel1180 msg-e3 round 3): the
//! night pair's mosaic is stored in 4 DN steps and its noise is HIGHER in the
//! corners where its level is LOWER (corner 143.6 DN / sd 62.9, centre
//! 205.7 / 54.5), which photon and read noise cannot do and the gain does;
//! with it, the residual has no trend left in level or radius (medians within
//! 0.025 per level decile and per ring on three photos) and its spread is the
//! variance estimate's own sampling spread plus 2–5 % (0.111 / 0.138 / 0.112
//! against `sqrt(2/255)` = 0.089, measured on 16-cell patches). The same model, fitted on green alone,
//! predicts the red and blue colour-difference noise to within 2–8 %.
//!
//! **The population, and the flat rule — no chosen factor.** Texture only
//! ever ADDS variance, so the residuals `ln(v / model)` of the noise-only
//! patches are symmetric about 0 and the textured ones sit on the positive
//! side. A patch is noise-only where the noise density beats the texture
//! density: with `f` the residual histogram, `f(u) − f(−u) < f(−u)`, i.e.
//! `f(u) < 2·f(−u)` (the maximum a posteriori decision between the two
//! populations; the 2 is that decision, not a tuning). The band `u*` is the
//! first `u` at which it fails (histogram bins of the Freedman–Diaconis
//! width). The fit runs on the noise-only patches and is iterated to a fixed
//! point. The SAME band then decides whether a patch's colour is flat: both
//! its colour-difference variances, against what the model predicts for
//! them, lie within `±u*`.
//!
//! Units: the normalised signed mosaic the develop demosaics
//! (`(v − black) / (white − black)`), so `a` is in full-scale units and `b`
//! in full-scale².
#![cfg_attr(not(test), allow(dead_code))] // the colour stage that reads it lands next (E3 commit b)

use rayon::prelude::*;

/// Cells per patch side (a patch is `2·PATCH` pixels square): the smallest
/// patch whose variance estimate is sharper than the flat band it is judged
/// by. The estimate's sampling error — measured, the robust spread of
/// `ln(v / model)` of the green and the two colour differences on the
/// noise-only patches, which carries the stored mosaic's own steps and the
/// model's misfit beside the chi-square spread — falls with the patch
/// (0.465 / 0.369 / 0.318 / 0.278 / 0.252 at 4 / 5 / 6 / 7 / 8 cells, the
/// worst of three photos, rel1180 msg-e3 round 4) and first drops under the
/// smallest band of those photos (0.256) at 8. A smaller patch is more local;
/// below 8 a patch's own sampling error is wider than the band that is to
/// tell its noise from its texture.
pub(crate) const PATCH: usize = 8;

/// Patches the model is fitted on (a uniform stride of the frame's patches,
/// [`NoiseModel::fit`]): the fewest whose fitted variance surface is sharp
/// enough not to undo [`PATCH`]. A patch's residual carries its own sampling
/// error (0.2523, the worst photo at 8 cells) plus the fit's error on the
/// surface, in quadrature; [`PATCH`] was set so that the first is under the
/// flat band (0.256), so the second may add at most
/// `sqrt(0.256² − 0.2523²)` = 0.0434 before the sum crosses it. Measured
/// (rel1180 msg-e3 round 6, the worst `|ln var|` over the frame's level ×
/// radius grid against the all-patch fit, worst of four stride offsets, three
/// photos of 238 800 patches): 0.0178 / 0.0345 / 0.0267 at stride 16, and
/// the first failure at 32 (0.0570 on the 61 MP frame). So 238 800 / 16 =
/// 14 925 patches; a frame with more is read at the stride that keeps at
/// least these many, a frame with fewer is read whole.
pub(crate) const FIT_PATCHES: usize = 14_925;

/// The uniform stride over `n` patches that keeps at least [`FIT_PATCHES`]
/// of them (1 when there are fewer): the one subsample rule of every reading
/// summed over a frame's patches (the fit, `linear_chroma::photo_k`).
pub(crate) fn fit_stride(n: usize) -> usize {
    (n / FIT_PATCHES).max(1)
}

/// The fitted model of one mosaic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NoiseModel {
    /// Photon term, full-scale units.
    pub a: f64,
    /// Read term, full-scale².
    pub b: f64,
    /// The gain's `r²` and `r⁴` coefficients.
    pub c2: f64,
    pub c4: f64,
    /// The MAP band `u*` of the residuals, in `ln` variance ratio.
    pub band: f64,
    /// The mosaic's size, for the radius.
    width: usize,
    height: usize,
}

/// One patch's statistics (all in full-scale units).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Patch {
    /// Mean green, red and blue.
    pub m: f64,
    pub m_r: f64,
    pub m_b: f64,
    /// `var(G1 − G2) / 2`.
    pub v_g: f64,
    /// Plane-removed variance of `wR·R − G` and `wB·B − G` (`G` the cell's mean green).
    pub v_dr: f64,
    pub v_db: f64,
    /// Distance of the patch's centre from the frame's centre over the half diagonal.
    pub r: f64,
    /// No site of the patch is clipped, and its level is above the black.
    pub usable: bool,
}

/// The four sites of a 2×2 Bayer cell as `(dy, dx)`: red, the two greens, blue.
/// `None` for a pattern that is not a 2×2 RGB Bayer.
pub(crate) fn bayer_sites(cfa: &rawler::cfa::CFA) -> Option<[(usize, usize); 4]> {
    if cfa.width != 2 || cfa.height != 2 || !cfa.is_rgb() {
        return None;
    }
    let mut greens = Vec::new();
    let (mut red, mut blue) = (None, None);
    for dy in 0..2 {
        for dx in 0..2 {
            match cfa.color_at(dy, dx) {
                0 => red = Some((dy, dx)),
                1 => greens.push((dy, dx)),
                2 => blue = Some((dy, dx)),
                _ => return None,
            }
        }
    }
    match (red, blue, greens.as_slice()) {
        (Some(r), Some(b), [g1, g2]) => Some([r, *g1, *g2, b]),
        _ => None,
    }
}

/// Every patch of the mosaic (`width × height`, normalised, signed), its sites
/// at `sites` ([`bayer_sites`]), the colour differences white-balanced by `wb`
/// (green = 1).
pub(crate) fn patches(mosaic: &[f32], width: usize, height: usize, sites: [(usize, usize); 4], wb: [f64; 3]) -> Vec<Patch> {
    let (pw, ph) = (width / 2 / PATCH, height / 2 / PATCH);
    let half_diag = ((width as f64 / 2.0).powi(2) + (height as f64 / 2.0).powi(2)).sqrt();
    let n = (PATCH * PATCH) as f64;
    // Plane removal on the cell grid: the grid is symmetric about its centre,
    // so the x and y slopes are independent regressions.
    let centre = (PATCH as f64 - 1.0) / 2.0;
    let sxx: f64 = (0..PATCH).map(|i| (i as f64 - centre).powi(2)).sum::<f64>() * PATCH as f64;
    (0..pw * ph)
        .into_par_iter()
        .map(|k| {
            let (cx0, cy0) = ((k % pw) * PATCH, (k / pw) * PATCH);
            let site = |cx: usize, cy: usize, s: usize| {
                let (dy, dx) = sites[s];
                mosaic[(2 * cy + dy) * width + 2 * cx + dx] as f64
            };
            let (mut sg, mut sr, mut sb, mut sd, mut sdd) = (0.0, 0.0, 0.0, 0.0, 0.0);
            let mut chroma = [[0.0f64; 3]; 2]; // per colour difference: sum, x-moment, y-moment
            let mut chroma2 = [0.0f64; 2];
            let mut usable = true;
            for j in 0..PATCH {
                for i in 0..PATCH {
                    let (cx, cy) = (cx0 + i, cy0 + j);
                    let (r, g1, g2, b) = (site(cx, cy, 0), site(cx, cy, 1), site(cx, cy, 2), site(cx, cy, 3));
                    usable &= r < 1.0 && g1 < 1.0 && g2 < 1.0 && b < 1.0;
                    let g = (g1 + g2) / 2.0;
                    sg += g;
                    sr += r;
                    sb += b;
                    sd += g1 - g2;
                    sdd += (g1 - g2) * (g1 - g2);
                    for (c, d) in [wb[0] * r - wb[1] * g, wb[2] * b - wb[1] * g].into_iter().enumerate() {
                        chroma[c][0] += d;
                        chroma[c][1] += (i as f64 - centre) * d;
                        chroma[c][2] += (j as f64 - centre) * d;
                        chroma2[c] += d * d;
                    }
                }
            }
            let m = sg / n;
            let v_g = (sdd - sd * sd / n) / (n - 1.0) / 2.0;
            let deplaned = |c: usize| {
                let [s, sx, sy] = chroma[c];
                (chroma2[c] - s * s / n - sx * sx / sxx - sy * sy / sxx) / (n - 3.0)
            };
            let (px, py) = ((2 * cx0 + PATCH) as f64 - width as f64 / 2.0, (2 * cy0 + PATCH) as f64 - height as f64 / 2.0);
            Patch {
                m,
                m_r: sr / n,
                m_b: sb / n,
                v_g,
                v_dr: deplaned(0),
                v_db: deplaned(1),
                r: (px * px + py * py).sqrt() / half_diag,
                usable: usable && m > 0.0 && v_g > 0.0,
            }
        })
        .collect()
}

impl NoiseModel {
    /// The gain at radius `r`.
    pub(crate) fn gain(&self, r: f64) -> f64 {
        1.0 + self.c2 * r * r + self.c4 * r.powi(4)
    }

    /// The noise variance of one site at stored level `m`, radius `r`.
    pub(crate) fn var(&self, m: f64, r: f64) -> f64 {
        let g = self.gain(r);
        g * self.a * m.max(0.0) + g * g * self.b
    }

    /// The radius of mosaic pixel `(x, y)`.
    pub(crate) fn radius(&self, x: f64, y: f64) -> f64 {
        let (cx, cy) = (self.width as f64 / 2.0, self.height as f64 / 2.0);
        ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() / (cx * cx + cy * cy).sqrt()
    }

    /// `ln(measured / model)` of a patch's green variance.
    pub(crate) fn residual(&self, p: &Patch) -> f64 {
        (p.v_g / self.var(p.m, p.r)).ln()
    }

    /// The model's prediction of a patch's two colour-difference variances
    /// (`wR·R − G`, `wB·B − G`, `G` the mean of two greens).
    pub(crate) fn chroma_var(&self, p: &Patch, wb: [f64; 3]) -> [f64; 2] {
        let g = self.var(p.m, p.r) / 2.0 * wb[1] * wb[1];
        [wb[0] * wb[0] * self.var(p.m_r, p.r) + g, wb[2] * wb[2] * self.var(p.m_b, p.r) + g]
    }

    /// Is the patch's colour flat — both colour-difference variances explained
    /// by the noise model within its own band?
    pub(crate) fn is_flat(&self, p: &Patch, wb: [f64; 3]) -> bool {
        let [pr, pb] = self.chroma_var(p, wb);
        p.usable
            && p.v_dr > 0.0
            && p.v_db > 0.0
            && (p.v_dr / pr).ln().abs() <= self.band
            && (p.v_db / pb).ln().abs() <= self.band
    }

    /// Fit the model on the noise-only population of `patches` (see the
    /// module), read at the uniform stride that keeps at least
    /// [`FIT_PATCHES`] of them (every patch on a smaller frame). `None` when
    /// fewer than 16 of those are usable or the fit has no noise-only
    /// population.
    pub(crate) fn fit(patches: &[Patch], width: usize, height: usize) -> Option<Self> {
        let stride = fit_stride(patches.len());
        let sampled: Vec<Patch> = patches.iter().step_by(stride).copied().collect();
        Self::fit_all(&sampled, width, height)
    }

    /// [`NoiseModel::fit`] on every one of `patches`.
    pub(crate) fn fit_all(patches: &[Patch], width: usize, height: usize) -> Option<Self> {
        let usable: Vec<&Patch> = patches.iter().filter(|p| p.usable).collect();
        if usable.len() < 16 {
            return None;
        }
        // Start: a line through all usable patches, gain 1.
        let (n, sm, sv, smm, smv) = usable.iter().fold((0.0, 0.0, 0.0, 0.0, 0.0), |s, p| {
            (s.0 + 1.0, s.1 + p.m, s.2 + p.v_g, s.3 + p.m * p.m, s.4 + p.m * p.v_g)
        });
        let slope = (n * smv - sm * sv) / (n * smm - sm * sm);
        let floor = usable.iter().map(|p| p.v_g).fold(f64::INFINITY, f64::min);
        let mut model = NoiseModel {
            a: slope.max(floor),
            b: ((sv - slope * sm) / n).max(floor),
            c2: 0.0,
            c4: 0.0,
            band: f64::INFINITY,
            width,
            height,
        };
        let mut keep: Vec<bool> = vec![true; usable.len()];
        for _ in 0..32 {
            let kept: Vec<&Patch> = usable.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect();
            if kept.len() < 16 {
                return None;
            }
            model = model.refined(&kept);
            let res: Vec<f64> = usable.iter().map(|p| model.residual(p)).collect();
            model.band = map_band(&res)?;
            let next: Vec<bool> = res.iter().map(|u| u.abs() <= model.band).collect();
            if next == keep {
                return Some(model);
            }
            keep = next;
        }
        Some(model)
    }

    /// Least squares of `ln v_g` on `ln var(m, r)` over `kept` (Levenberg–Marquardt
    /// in the four parameters, `a, b ≥ 0`, the gain positive on the frame).
    fn refined(self, kept: &[&Patch]) -> Self {
        let cost = |q: &NoiseModel| kept.iter().map(|p| q.residual(p).powi(2)).sum::<f64>();
        let mut cur = self;
        let mut c0 = cost(&cur);
        let mut lambda = 1e-3;
        for _ in 0..200 {
            let mut jtj = [[0.0f64; 4]; 4];
            let mut jtr = [0.0f64; 4];
            for p in kept {
                let g = cur.gain(p.r);
                let model = cur.var(p.m, p.r);
                let dg = cur.a * p.m.max(0.0) + 2.0 * g * cur.b;
                let j = [g * p.m.max(0.0) / model, g * g / model, dg * p.r * p.r / model, dg * p.r.powi(4) / model];
                let res = -cur.residual(p); // ln(model) − ln(v)
                for a in 0..4 {
                    jtr[a] += j[a] * res;
                    for b in 0..4 {
                        jtj[a][b] += j[a] * j[b];
                    }
                }
            }
            let mut stepped = false;
            for _ in 0..16 {
                let mut m = jtj;
                for (a, row) in m.iter_mut().enumerate() {
                    row[a] *= 1.0 + lambda;
                }
                let Some(d) = solve4(m, jtr) else { break };
                let next = NoiseModel {
                    a: (cur.a - d[0]).max(0.0),
                    b: (cur.b - d[1]).max(0.0),
                    c2: cur.c2 - d[2],
                    c4: cur.c4 - d[3],
                    ..cur
                };
                let positive = (0..=16).all(|k| next.gain(k as f64 / 16.0) > 0.0) && next.a + next.b > 0.0;
                let c1 = if positive { cost(&next) } else { f64::INFINITY };
                if c1 < c0 {
                    let done = (c0 - c1) <= 1e-12 * c0;
                    cur = next;
                    c0 = c1;
                    lambda = (lambda / 4.0).max(1e-12);
                    stepped = !done;
                    break;
                }
                lambda *= 8.0;
            }
            if !stepped {
                break;
            }
        }
        cur
    }
}

/// The MAP band `u*` of the residuals `res`: the first `u > 0` where the
/// positive side's bin holds at least twice the mirrored negative bin's count
/// (`f(u) ≥ 2·f(−u)`), bins of the Freedman–Diaconis width. `None` when the
/// residuals have no spread.
pub(crate) fn map_band(res: &[f64]) -> Option<f64> {
    let mut s: Vec<f64> = res.iter().copied().filter(|u| u.is_finite()).collect();
    if s.len() < 16 {
        return None;
    }
    s.sort_unstable_by(f64::total_cmp);
    let q = |p: f64| s[((s.len() - 1) as f64 * p).round() as usize];
    let h = 2.0 * (q(0.75) - q(0.25)) / (s.len() as f64).cbrt();
    if h <= 0.0 {
        return None;
    }
    let count = |lo: f64, hi: f64| s.iter().filter(|u| **u > lo && **u <= hi).count();
    let neg = |lo: f64, hi: f64| s.iter().filter(|u| **u >= -hi && **u < -lo).count();
    let mut k = 1usize;
    loop {
        let (lo, hi) = ((k - 1) as f64 * h, k as f64 * h);
        if count(lo, hi) >= 2 * neg(lo, hi) {
            return Some(lo);
        }
        k += 1;
    }
}

/// Solve the 4×4 system `m · x = v` (Gaussian elimination, partial pivoting).
fn solve4(mut m: [[f64; 4]; 4], mut v: [f64; 4]) -> Option<[f64; 4]> {
    for c in 0..4 {
        let p = (c..4).max_by(|a, b| m[*a][c].abs().total_cmp(&m[*b][c].abs()))?;
        if m[p][c].abs() < 1e-300 {
            return None;
        }
        m.swap(c, p);
        v.swap(c, p);
        for r in c + 1..4 {
            let (f, pivot) = (m[r][c] / m[c][c], m[c]);
            for (x, p) in m[r].iter_mut().zip(pivot).skip(c) {
                *x -= f * p;
            }
            v[r] -= f * v[c];
        }
    }
    let mut x = [0.0f64; 4];
    for c in (0..4).rev() {
        x[c] = (v[c] - (c + 1..4).map(|k| m[c][k] * x[k]).sum::<f64>()) / m[c][c];
    }
    Some(x)
}

#[cfg(test)]
mod tests;
