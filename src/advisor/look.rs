//! The numbers a LOOK STATEMENT is made of, and the move of a frame to them.
//!
//! The statement path of the analysis (see [`super::statement`]) shows the
//! vision model the neutral frame and, per AREA of it (sky and ground, or the
//! whole frame), what the area measures now in CIELAB:
//!   * L* at five tonal anchors — the 2 / 25 / 50 / 75 / 98 % weighted
//!     quantiles of the area's L*;
//!   * the mean (a*, b*) of its shadows, midtones and highlights — the area's
//!     three L* terciles.
//!
//! The model answers the finished appearance of each area in the same numbers
//! ([`AreaStatement`]), and [`moved`] moves the frame to them: a monotone tone
//! curve per area through the anchors ([`ToneCurve`]), the three bands' (a*, b*)
//! offsets blended along L*, the areas blended by their soft weights. The
//! result is a synthetic target that is the same photograph pixel for pixel,
//! which the ordinary same-scene reverse fit then reproduces with a recipe.
//!
//! Pure: no I/O, no model. Every computation is done in `f64` over the `f32`
//! Lab of [`crate::fit::cie_lab`] (the objective's own conversion), so the sums
//! over a whole plate do not drift.

/// The tonal anchors: the weighted quantiles of an area's L* the statement
/// speaks in.
pub const ANCHOR_QUANTILES: [f64; 5] = [0.02, 0.25, 0.50, 0.75, 0.98];

/// An area whose mean share of the frame is under this has no look of its own
/// to state: the whole frame is then ONE area.
pub const SMALLEST_AREA: f64 = 0.03;

/// Anchors nearer than this (in L*) are one point of the tone curve.
const TOGETHER: f64 = 0.25;

/// The tercile edges that split an area into shadows / midtones / highlights.
const BAND_EDGES: [f64; 2] = [1.0 / 3.0, 2.0 / 3.0];

/// The name of one stated area. The wire spelling is [`Self::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AreaName {
    Sky,
    Ground,
    /// The whole frame, when sky or ground is too small to state on its own.
    Frame,
}

impl AreaName {
    pub fn as_str(self) -> &'static str {
        match self {
            AreaName::Sky => "sky",
            AreaName::Ground => "ground",
            AreaName::Frame => "frame",
        }
    }
}

/// One area of a frame: its name and its per-pixel weight in 0..=1.
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub name: AreaName,
    pub weight: Vec<f32>,
}

/// The areas a frame is stated in, from its sky weight (mask / 255, one value
/// per pixel): sky = the weight, ground = 1 − the weight; when either's mean
/// share is under [`SMALLEST_AREA`] (or there are no pixels) the whole frame is
/// ONE area named [`AreaName::Frame`] with weight 1.
pub fn areas_of(sky: &[f32]) -> Vec<Area> {
    let mean = if sky.is_empty() {
        0.0
    } else {
        sky.iter().map(|&w| f64::from(w)).sum::<f64>() / sky.len() as f64
    };
    if mean.min(1.0 - mean) < SMALLEST_AREA {
        return vec![Area { name: AreaName::Frame, weight: vec![1.0; sky.len()] }];
    }
    vec![
        Area { name: AreaName::Sky, weight: sky.to_vec() },
        Area { name: AreaName::Ground, weight: sky.iter().map(|&w| 1.0 - w).collect() },
    ]
}

/// What one area measures: the statement's own unit.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AreaLook {
    /// The area's mean weight: its share of the frame, 0..=1.
    pub share: f32,
    /// L* at the five [`ANCHOR_QUANTILES`].
    pub l: [f32; 5],
    /// The weighted mean L* of the shadows, midtones and highlights.
    pub centres: [f32; 3],
    /// The weighted mean (a*, b*) of the shadows, midtones and highlights.
    pub ab: [[f32; 2]; 3],
}

/// The finished appearance stated for one area, in [`AreaLook`]'s numbers.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AreaStatement {
    /// L* at the five anchors, non-decreasing.
    pub l: [f32; 5],
    /// (a*, b*) of the shadows, midtones and highlights.
    pub ab: [[f32; 2]; 3],
}

/// CIELAB of 8-bit sRGB pixels ([`crate::fit::cie_lab`], D65).
pub fn lab_of_rgb8(pixels: &[[u8; 3]]) -> Vec<[f32; 3]> {
    pixels
        .iter()
        .map(|p| crate::fit::cie_lab(&p.map(|c| f32::from(c) / 255.0)))
        .collect()
}

/// CIELAB of an RGB image, row-major.
pub fn lab_of_image(img: &image::RgbImage) -> Vec<[f32; 3]> {
    img.pixels().map(|p| crate::fit::cie_lab(&p.0.map(|c| f32::from(c) / 255.0))).collect()
}

/// sRGB in 0..=1 of a CIELAB colour (D65): the inverse of
/// [`crate::fit::cie_lab`], clipped to the gamut. As `skimage.color.lab2rgb`
/// does, a negative Z is held at zero before the matrix.
pub fn srgb_of_lab(lab: [f64; 3]) -> [f32; 3] {
    let [l, a, b] = lab;
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let inv = |t: f64| if t > 6.0 / 29.0 { t * t * t } else { 3.0 * (6.0f64 / 29.0).powi(2) * (t - 4.0 / 29.0) };
    let x = 0.95047 * inv(fx);
    let y = inv(fy);
    let z = (1.08883 * inv(fz)).max(0.0);
    let lin = [
        3.2404542 * x - 1.5371385 * y - 0.4985314 * z,
        -0.9692660 * x + 1.8760108 * y + 0.0415560 * z,
        0.0556434 * x - 0.2040259 * y + 1.0572252 * z,
    ];
    lin.map(|c| crate::render::linear_to_srgb(c as f32).clamp(0.0, 1.0))
}

/// `numpy.interp(q, xp, fp)` for a non-decreasing `xp`, ties included: below
/// the first point the first value, at or above the last the last value, and
/// in between the line through the last point at or below `q` and the next.
fn interp(q: f64, xp: &[f64], fp: &[f64]) -> f64 {
    let n = xp.len();
    let j = xp.partition_point(|&c| c <= q);
    if j == 0 {
        return fp[0];
    }
    if j == n {
        return fp[n - 1];
    }
    let j = j - 1;
    if xp[j] == q {
        return fp[j];
    }
    fp[j] + (fp[j + 1] - fp[j]) * (q - xp[j]) / (xp[j + 1] - xp[j])
}

/// Weighted quantiles: the values sorted, their cumulative weight normalised
/// to end at 1, and the values linearly interpolated over that cumulative
/// weight at each `q` (`numpy.interp(q, cw, xs)`; below the first cumulative
/// weight, the first value). A zero total weight reads every pixel alike
/// (an area is never weightless: [`areas_of`] guarantees a share).
pub fn weighted_quantiles(values: &[f32], weights: &[f32], qs: &[f64]) -> Vec<f64> {
    if values.is_empty() {
        return vec![0.0; qs.len()];
    }
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&i, &j| values[i].total_cmp(&values[j]));
    let total: f64 = weights.iter().map(|&w| f64::from(w)).sum();
    let uniform = total <= 0.0;
    let mut cw = Vec::with_capacity(order.len());
    let mut acc = 0.0f64;
    for &i in &order {
        acc += if uniform { 1.0 } else { f64::from(weights[i]) };
        cw.push(acc);
    }
    let last = acc;
    for c in &mut cw {
        *c /= last;
    }
    let xs: Vec<f64> = order.iter().map(|&i| f64::from(values[i])).collect();
    qs.iter().map(|&q| interp(q, &cw, &xs)).collect()
}

/// An area's statistics, with each band's total weight (zero = an empty band).
struct Measured {
    look: AreaLook,
    band_weight: [f64; 3],
}

fn measure_full(lab: &[[f32; 3]], weight: &[f32]) -> Measured {
    let l: Vec<f32> = lab.iter().map(|p| p[0]).collect();
    let mut qs = ANCHOR_QUANTILES.to_vec();
    qs.extend_from_slice(&BAND_EDGES);
    let q = weighted_quantiles(&l, weight, &qs);
    let (e0, e1) = (q[5], q[6]);
    let mut sums = [[0.0f64; 4]; 3]; // weight, L*, a*, b*
    for (p, &w) in lab.iter().zip(weight) {
        let lv = f64::from(p[0]);
        let band = if lv <= e0 {
            0
        } else if lv <= e1 {
            1
        } else {
            2
        };
        let w = f64::from(w);
        let s = &mut sums[band];
        s[0] += w;
        s[1] += lv * w;
        s[2] += f64::from(p[1]) * w;
        s[3] += f64::from(p[2]) * w;
    }
    let mut centres = [0.0f32; 3];
    let mut ab = [[0.0f32; 2]; 3];
    for (k, s) in sums.iter().enumerate() {
        let d = s[0].max(1e-9);
        centres[k] = (s[1] / d) as f32;
        ab[k] = [(s[2] / d) as f32, (s[3] / d) as f32];
    }
    let share = if weight.is_empty() {
        0.0
    } else {
        (weight.iter().map(|&w| f64::from(w)).sum::<f64>() / weight.len() as f64) as f32
    };
    Measured {
        look: AreaLook { share, l: std::array::from_fn(|k| q[k] as f32), centres, ab },
        band_weight: [sums[0][0], sums[1][0], sums[2][0]],
    }
}

/// What one area measures: its share, L* at the five anchors, and the centre
/// L* and mean (a*, b*) of each of its three L* terciles (the bands are
/// `L ≤ e₁`, `e₁ < L ≤ e₂`, `L > e₂` for the weighted 1/3 and 2/3 quantiles).
pub fn measure(lab: &[[f32; 3]], weight: &[f32]) -> AreaLook {
    measure_full(lab, weight).look
}

/// The monotone tone curve that moves an area's measured anchors onto the
/// stated ones, keeping the tails.
///
/// The stated L* is made non-decreasing; anchors nearer than 0.25 L* are one
/// point at their means; (0, 0) is prepended unless the first point is already
/// within 0.25 of black (then it sits AT black with its stated value — a black
/// the statement lifts stays lifted), (100, 100) appended likewise; a monotone
/// cubic (PCHIP, Fritsch–Carlson, with SciPy's end slopes) runs through the
/// points.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneCurve {
    xs: Vec<f64>,
    ys: Vec<f64>,
    slopes: Vec<f64>,
}

impl ToneCurve {
    pub fn through(anchors: &[f32; 5], stated: &[f32; 5]) -> Self {
        let mut target = [0.0f64; 5];
        let mut run = f64::NEG_INFINITY;
        for (t, &s) in target.iter_mut().zip(stated) {
            run = run.max(f64::from(s));
            *t = run;
        }
        // Groups of anchors chained closer than TOGETHER, each (xs, ys).
        let mut groups: Vec<(Vec<f64>, Vec<f64>)> = Vec::new();
        for (&x, &y) in anchors.iter().zip(&target) {
            let (x, y) = (f64::from(x).clamp(0.0, 100.0), y.clamp(0.0, 100.0));
            match groups.last_mut() {
                Some(g) if x - g.0[g.0.len() - 1] < TOGETHER => {
                    g.0.push(x);
                    g.1.push(y);
                }
                _ => groups.push((vec![x], vec![y])),
            }
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let mut xs: Vec<f64> = groups.iter().map(|g| mean(&g.0)).collect();
        let mut ys: Vec<f64> = Vec::with_capacity(xs.len() + 2);
        let mut run = f64::NEG_INFINITY;
        for g in &groups {
            run = run.max(mean(&g.1));
            ys.push(run);
        }
        if xs[0] < TOGETHER {
            xs[0] = 0.0;
        } else {
            xs.insert(0, 0.0);
            ys.insert(0, 0.0);
        }
        let last = xs.len() - 1;
        if xs[last] > 100.0 - TOGETHER {
            xs[last] = 100.0;
        } else {
            xs.push(100.0);
            ys.push(100.0);
        }
        let slopes = pchip_slopes(&xs, &ys);
        ToneCurve { xs, ys, slopes }
    }

    /// The curve at `l` (clipped to 0..=100 first).
    pub fn at(&self, l: f64) -> f64 {
        let x = l.clamp(0.0, 100.0);
        let n = self.xs.len();
        let k = self.xs.partition_point(|&v| v <= x).clamp(1, n - 1) - 1;
        let h = self.xs[k + 1] - self.xs[k];
        let t = (x - self.xs[k]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[k]
            + (t3 - 2.0 * t2 + t) * h * self.slopes[k]
            + (-2.0 * t3 + 3.0 * t2) * self.ys[k + 1]
            + (t3 - t2) * h * self.slopes[k + 1]
    }
}

/// The node slopes of a monotone piecewise-cubic Hermite interpolant, as
/// `scipy.interpolate.PchipInterpolator` computes them: interior slopes are
/// the weighted harmonic mean of the neighbouring secants (zero where the
/// secants change sign or one is flat); the end slopes are the one-sided
/// three-point estimate, zeroed when its sign differs from the end secant's
/// and limited to three times it when the first two secants differ in sign.
fn pchip_slopes(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = xs.len();
    let h: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).collect();
    let m: Vec<f64> = (0..n - 1).map(|k| (ys[k + 1] - ys[k]) / h[k]).collect();
    if n == 2 {
        return vec![m[0], m[0]];
    }
    let mut d = vec![0.0; n];
    for k in 1..n - 1 {
        let (m0, m1) = (m[k - 1], m[k]);
        if m0 == 0.0 || m1 == 0.0 || m0.signum() != m1.signum() {
            continue;
        }
        let w1 = 2.0 * h[k] + h[k - 1];
        let w2 = h[k] + 2.0 * h[k - 1];
        d[k] = (w1 + w2) / (w1 / m0 + w2 / m1);
    }
    // np.sign: zero is its own sign (f64::signum calls 0.0 positive).
    let sign = |v: f64| if v > 0.0 { 1 } else if v < 0.0 { -1 } else { 0 };
    let edge = |h0: f64, h1: f64, m0: f64, m1: f64| {
        let d = ((2.0 * h0 + h1) * m0 - h0 * m1) / (h0 + h1);
        if sign(d) != sign(m0) {
            0.0
        } else if sign(m0) != sign(m1) && d.abs() > 3.0 * m0.abs() {
            3.0 * m0
        } else {
            d
        }
    };
    d[0] = edge(h[0], h[1], m[0], m[1]);
    d[n - 1] = edge(h[n - 2], h[n - 3], m[n - 2], m[n - 3]);
    d
}

/// Why a frame could not be moved: the statement names no numbers for this
/// area of the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnstatedArea(pub AreaName);

impl std::fmt::Display for UnstatedArea {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the statement has no numbers for the area {:?}", self.0.as_str())
    }
}

impl std::error::Error for UnstatedArea {}

/// The frame moved to the stated look, as sRGB in 0..=1 per pixel.
///
/// Per area: L' = the area's [`ToneCurve`] at L; a', b' = a, b + the band
/// offsets (stated − measured) linearly interpolated along L* between the
/// band centres and held outside them (an empty band carries no offset); the
/// areas blended by weight, any weight left over keeping the frame's own Lab;
/// back to sRGB, clipped.
pub fn moved(
    lab: &[[f32; 3]],
    areas: &[Area],
    stated: &[(AreaName, AreaStatement)],
) -> Result<Vec<[f32; 3]>, UnstatedArea> {
    let mut acc = vec![[0.0f64; 4]; lab.len()]; // weighted L, a, b, and the weight
    for area in areas {
        let want = stated.iter().find(|(n, _)| *n == area.name).map(|(_, s)| s).ok_or(UnstatedArea(area.name))?;
        let m = measure_full(lab, &area.weight);
        let curve = ToneCurve::through(&m.look.l, &want.l);
        let mut centres = Vec::with_capacity(3);
        let mut offsets: [Vec<f64>; 2] = [Vec::with_capacity(3), Vec::with_capacity(3)];
        for k in 0..3 {
            if m.band_weight[k] > 0.0 {
                centres.push(f64::from(m.look.centres[k]));
                for (c, off) in offsets.iter_mut().enumerate() {
                    off.push(f64::from(want.ab[k][c]) - f64::from(m.look.ab[k][c]));
                }
            }
        }
        for ((p, &w), out) in lab.iter().zip(&area.weight).zip(acc.iter_mut()) {
            let w = f64::from(w);
            let l = f64::from(p[0]);
            let shift = |c: usize| if centres.is_empty() { 0.0 } else { interp(l, &centres, &offsets[c]) };
            out[0] += w * curve.at(l);
            out[1] += w * (f64::from(p[1]) + shift(0));
            out[2] += w * (f64::from(p[2]) + shift(1));
            out[3] += w;
        }
    }
    Ok(lab
        .iter()
        .zip(&acc)
        .map(|(p, s)| {
            let rest = 1.0 - s[3];
            srgb_of_lab([
                s[0] + rest * f64::from(p[0]),
                s[1] + rest * f64::from(p[1]),
                s[2] + rest * f64::from(p[2]),
            ])
        })
        .collect())
}

/// 8-bit sRGB of a 0..=1 pixel, rounded.
pub fn rgb8_of(p: [f32; 3]) -> [u8; 3] {
    p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[cfg(test)]
#[path = "look_tests.rs"]
mod tests;
