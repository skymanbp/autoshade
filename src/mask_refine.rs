//! Conservative edge-aware production of bitmap-mask boundaries.
//!
//! Two bounded operations on an existing mask, both allowed to alter only a
//! fixed collar around the existing boundary and both abstaining unless
//! coverage is conserved: [`guided_refine`] moves the transition TOWARD edges
//! in the guide, and [`widen_smooth_feather`] broadens it where the guide has
//! no edges to move toward at all.

use image::{DynamicImage, GrayImage, Luma};
use rayon::prelude::*;

const COVERAGE_DELTA_MAX: f32 = 0.002;

/// One code value of the 8-bit guide. The same quantity
/// `fit_zoned::BOUNDARY_STEP_FLOOR` is: below it an 8-bit instrument ranks
/// its own rounding, and unmasked step detection on a uniform field is ~0.6
/// code at mid-grey, so one code is where a seam starts being visible at all.
const ONE_CODE: f32 = 1.0 / 255.0;

/// Box radius of the widened ramp, as a share of the mask's OWN height — so
/// the ramp it delivers is about twice this, ~6% of the frame height.
///
/// A SHARE rather than a pixel count, so the rule describes the same ramp
/// whatever raster the sidecar emitted and the radius needs no rescaling when
/// the widened alpha is written back as the zone's PNG. It is a CAP, reached
/// only where the guide is perfectly flat.
///
/// The size is not free, and the boundary budget fixes both of its ends. The
/// transition-band ruler reads the correction's SHORTFALL at the 50% contour
/// — `(1 - ZONE_BOUNDARY_MID) = 0.5` of the settled height, a quantity a
/// wider ramp does not reduce — while a ramp `W` analysis pixels wide earns
/// `BOUNDARY_STEP_SHAPE * (3-px baseline / W)` of that same height back as
/// slope credit, and earns it only where the ramp persists past TWO
/// consecutive baselines. Those two rules bracket the useful width: below
/// ~14 analysis pixels the second baseline lands on the settled plateau and
/// the credit is refused, above ~18 (`3 * 3 / 0.5`) the credit falls below
/// the shortfall it has to pay for. The shipped analysis grid is 384 px on
/// its long edge, so a landscape frame is ~256 rows and this share puts the
/// delivered ramp at 17 of them — inside the bracket, and five times the
/// two-or-three-pixel band a segmentation model emits in haze. A portrait
/// frame sits a little above the bracket and is charged a little for it;
/// that is a budget decision, not a visible seam.
const FEATHER_CAP_SHARE: f32 = 0.03;

#[cfg(test)]
thread_local! {
    static GUIDED_REFINE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_guided_refine_calls() {
    GUIDED_REFINE_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
pub(crate) fn guided_refine_calls() -> usize {
    GUIDED_REFINE_CALLS.with(std::cell::Cell::get)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RefineReading {
    pub(crate) coverage_delta: f32,
    pub(crate) edge_before: f32,
    pub(crate) edge_after: f32,
    pub(crate) core_changed: usize,
}

#[derive(Debug)]
pub(crate) enum RefineOutcome {
    Kept { mask: GrayImage, reading: RefineReading },
    Abstained { reading: RefineReading },
}

fn box_mean(input: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let stride = width + 1;
    let mut integral = vec![0.0f64; (width + 1) * (height + 1)];
    for y in 0..height {
        let mut row_sum = 0.0f64;
        for x in 0..width {
            row_sum += input[y * width + x] as f64;
            integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row_sum;
        }
    }
    let mut output = vec![0.0f32; input.len()];
    output.par_chunks_mut(width.max(1)).enumerate().for_each(|(y, row)| {
        let y0 = y.saturating_sub(radius);
        let y1 = (y + radius + 1).min(height);
        for (x, value) in row.iter_mut().enumerate() {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius + 1).min(width);
            let sum = integral[y1 * stride + x1] - integral[y0 * stride + x1]
                - integral[y1 * stride + x0]
                + integral[y0 * stride + x0];
            *value = (sum / ((x1 - x0) * (y1 - y0)).max(1) as f64) as f32;
        }
    });
    output
}

/// Grow a seed mask by `radius` in the square metric. Factored out of
/// [`boundary_collar`] so the feather widener can dilate its own seed — the
/// 50% CONTOUR rather than every partially transparent pixel — with the same
/// collar machinery and the same semantics.
///
/// SEPARABLE, through per-row and per-column prefix counts: a square window
/// is the horizontal pass followed by the vertical one, and the result is
/// identical to the naive double loop. The naive form is
/// `O(n * (2r + 1)^2)`, which the guided refinement's `2 * 8` collar could
/// afford and the widener's `2 * 3% of the frame height` cannot — on a
/// 1024-row segmentation raster that is a 125x125 window at every pixel.
fn dilate(seed: &[bool], width: usize, height: usize, radius: usize) -> Vec<bool> {
    let mut rows = vec![false; seed.len()];
    let mut prefix = vec![0u32; width + 1];
    for y in 0..height {
        for x in 0..width {
            prefix[x + 1] = prefix[x] + u32::from(seed[y * width + x]);
        }
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius + 1).min(width);
            rows[y * width + x] = prefix[x1] > prefix[x0];
        }
    }
    let mut grown = vec![false; seed.len()];
    let mut column = vec![0u32; height + 1];
    for x in 0..width {
        for y in 0..height {
            column[y + 1] = column[y] + u32::from(rows[y * width + x]);
        }
        for y in 0..height {
            let y0 = y.saturating_sub(radius);
            let y1 = (y + radius + 1).min(height);
            grown[y * width + x] = column[y1] > column[y0];
        }
    }
    grown
}

/// Carry a value out of every seeded pixel to everything within `radius` in
/// the square metric, keeping the SMALLEST value in reach. `f32::INFINITY`
/// is the empty seed, and stays wherever nothing reaches.
///
/// The graded twin of [`dilate`]: where that one answers "is any seed in
/// reach", this one answers "what is the least any seed in reach will
/// allow". The feather widener needs the second, because a credit read on
/// the contour has to veto the widening of every pixel the broadened ramp
/// would have touched, and one edgy crossing must outrank the flat ones
/// beside it.
///
/// SEPARABLE, like [`dilate`] and for the same reason — a square window's
/// minimum is the horizontal pass followed by the vertical one — but by a
/// running scan rather than a prefix count, because a minimum does not
/// subtract. That is `O(n * (2r + 1))` per axis against the square window's
/// `O(n * (2r + 1)^2)`: at the widener's radius on a 1024-row raster, 126
/// comparisons per pixel instead of 3969.
fn spread_min(seed: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let least = |values: &[f32]| values.iter().fold(f32::INFINITY, |a, b| a.min(*b));
    let mut rows = vec![f32::INFINITY; seed.len()];
    for y in 0..height {
        let row = &seed[y * width..(y + 1) * width];
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius + 1).min(width);
            rows[y * width + x] = least(&row[x0..x1]);
        }
    }
    let mut spread = vec![f32::INFINITY; seed.len()];
    let mut column = vec![0.0f32; height];
    for x in 0..width {
        for (y, value) in column.iter_mut().enumerate() {
            *value = rows[y * width + x];
        }
        for y in 0..height {
            let y0 = y.saturating_sub(radius);
            let y1 = (y + radius + 1).min(height);
            spread[y * width + x] = least(&column[y0..y1]);
        }
    }
    spread
}

fn boundary_collar(alpha: &[u8], width: usize, height: usize, radius: usize) -> Vec<bool> {
    let mut boundary = vec![false; alpha.len()];
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let here = alpha[i];
            boundary[i] = (here > 0 && here < 255)
                || (x > 0 && alpha[i - 1] != here)
                || (x + 1 < width && alpha[i + 1] != here)
                || (y > 0 && alpha[i - width] != here)
                || (y + 1 < height && alpha[i + width] != here);
        }
    }
    dilate(&boundary, width, height, radius)
}

fn edge_alignment(guide: &[f32], alpha: &[f32], width: usize, height: usize) -> f32 {
    if width < 3 || height < 3 {
        return 0.0;
    }
    let mut weighted = 0.0f64;
    let mut weights = 0.0f64;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = y * width + x;
            let dx = (
                guide[i + 1 - width]
                    + 2.0 * guide[i + 1]
                    + guide[i + 1 + width]
                    - guide[i - 1 - width]
                    - 2.0 * guide[i - 1]
                    - guide[i - 1 + width]
            ) / 8.0;
            let dy = (
                guide[i - 1 + width]
                    + 2.0 * guide[i + width]
                    + guide[i + 1 + width]
                    - guide[i - 1 - width]
                    - 2.0 * guide[i - width]
                    - guide[i + 1 - width]
            ) / 8.0;
            let gradient = (dx * dx + dy * dy).sqrt() as f64;
            let boundary = (4.0 * alpha[i] * (1.0 - alpha[i])) as f64;
            weighted += gradient * boundary;
            weights += boundary;
        }
    }
    if weights > 0.0 { (weighted / weights) as f32 } else { 0.0 }
}

/// The guide, at the mask's own size, as luma in [0, 1] — the one basis both
/// operations in this module judge a mask against.
fn guide_luma(guide: &DynamicImage, width: u32, height: u32) -> Vec<f32> {
    guide
        .resize_exact(width, height, image::imageops::FilterType::Lanczos3)
        .to_rgb8()
        .pixels()
        .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
        .collect()
}

fn refinement_passes(reading: RefineReading) -> bool {
    reading.core_changed == 0
        && reading.coverage_delta <= COVERAGE_DELTA_MAX
        && reading.edge_before.is_finite()
        && reading.edge_after.is_finite()
        && reading.edge_after >= reading.edge_before
}

/// Refine a mask with the local-linear guided filter, retaining the result
/// only when all mask-production conservation laws pass.
pub(crate) fn guided_refine(
    guide: &DynamicImage,
    mask: &GrayImage,
    radius: u32,
    epsilon: f32,
) -> RefineOutcome {
    #[cfg(test)]
    GUIDED_REFINE_CALLS.with(|calls| calls.set(calls.get() + 1));
    let (width, height) = mask.dimensions();
    let n = width as usize * height as usize;
    let empty = RefineReading {
        coverage_delta: 0.0,
        edge_before: 0.0,
        edge_after: 0.0,
        core_changed: 0,
    };
    if n == 0 || !epsilon.is_finite() || epsilon <= 0.0 {
        return RefineOutcome::Abstained { reading: empty };
    }
    let guide = guide_luma(guide, width, height);
    let original = mask.as_raw();
    let alpha = original.iter().map(|v| *v as f32 / 255.0).collect::<Vec<_>>();
    let radius = radius as usize;
    let mean_i = box_mean(&guide, width as usize, height as usize, radius);
    let mean_p = box_mean(&alpha, width as usize, height as usize, radius);
    let ii = guide.iter().map(|v| v * v).collect::<Vec<_>>();
    let ip = guide.iter().zip(&alpha).map(|(i, p)| i * p).collect::<Vec<_>>();
    let corr_i = box_mean(&ii, width as usize, height as usize, radius);
    let corr_ip = box_mean(&ip, width as usize, height as usize, radius);
    let coefficients = corr_i
        .iter()
        .zip(&mean_i)
        .zip(&corr_ip)
        .zip(&mean_p)
        .map(|(((corr_i, mean_i), corr_ip), mean_p)| {
            let variance = corr_i - mean_i * mean_i;
            let a = (corr_ip - mean_i * mean_p) / (variance + epsilon);
            (a, mean_p - a * mean_i)
        })
        .collect::<Vec<_>>();
    if coefficients.iter().any(|(a, b)| !a.is_finite() || !b.is_finite()) {
        return RefineOutcome::Abstained { reading: empty };
    }
    let a = coefficients.iter().map(|v| v.0).collect::<Vec<_>>();
    let b = coefficients.iter().map(|v| v.1).collect::<Vec<_>>();
    let mean_a = box_mean(&a, width as usize, height as usize, radius);
    let mean_b = box_mean(&b, width as usize, height as usize, radius);
    let collar = boundary_collar(
        original,
        width as usize,
        height as usize,
        radius.saturating_mul(2),
    );
    let mut refined = vec![0u8; n];
    refined.par_iter_mut().enumerate().for_each(|(i, value)| {
        if collar[i] {
            let filtered = (mean_a[i] * guide[i] + mean_b[i]).clamp(0.0, 1.0);
            *value = (filtered * 255.0).round() as u8;
        } else {
            *value = original[i];
        }
    });
    let core_changed = refined
        .iter()
        .zip(original)
        .zip(&collar)
        .filter(|((after, before), in_collar)| !**in_collar && after != before)
        .count();
    let refined_f32 = refined.iter().map(|v| *v as f32 / 255.0).collect::<Vec<_>>();
    let coverage = |values: &[f32]| values.iter().map(|v| *v as f64).sum::<f64>() / n as f64;
    let coverage_delta = (coverage(&refined_f32) - coverage(&alpha)).abs() as f32;
    let edge_before = edge_alignment(&guide, &alpha, width as usize, height as usize);
    let edge_after = edge_alignment(&guide, &refined_f32, width as usize, height as usize);
    let reading = RefineReading { coverage_delta, edge_before, edge_after, core_changed };
    if !refinement_passes(reading) {
        return RefineOutcome::Abstained { reading };
    }
    let mut output = GrayImage::new(width, height);
    for (pixel, value) in output.pixels_mut().zip(refined) {
        *pixel = Luma([value]);
    }
    RefineOutcome::Kept { mask: output, reading }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WidenReading {
    /// Share of the mask's own 50% contour that ran through guide smooth
    /// enough to widen, in [0, 1]. Zero is the abstention.
    pub(crate) widened_share: f32,
    /// The widest ramp radius the rule applied anywhere, in MASK pixels —
    /// the cap, reached where the guide is perfectly flat.
    pub(crate) max_radius: u32,
    pub(crate) coverage_delta: f32,
    /// Alpha changed outside the collar. Zero by construction; carried
    /// because a conservation law nobody measures is a comment.
    pub(crate) core_changed: usize,
}

#[derive(Debug)]
pub(crate) enum WidenOutcome {
    Widened { mask: GrayImage, reading: WidenReading },
    Abstained { reading: WidenReading },
}

/// Broaden a SOFT mask's alpha transition where the guide is too smooth to
/// hide a seam, and leave it exactly as it is everywhere else.
///
/// WHY this exists, measured: a semantic sky raster is the segmentation
/// model's own class probability, so in featureless haze its 5%-95% band is
/// whatever the model emitted — two or three analysis pixels. The whole
/// height of the correction is then delivered across those two or three
/// pixels, and on a real desert-dusk pair that shipped a 0.013-0.016 luma
/// step across the 50% contour in the finished 2048-px render: 3-4 codes of
/// 255 in a smooth gradient, plainly visible. Spreading the SAME height over
/// a ramp five times wider divides the per-pixel step by five without
/// touching the correction's strength — which is the half of the fix the
/// boundary budget cannot do, because a budget can only take strength away.
///
/// The rule is deterministic and has no per-image sweep:
///
///   * the collar is `2 x cap` px around the mask's 50% CONTOUR, twice the
///     widest ramp, so the broadened alpha has decayed back onto the original
///     plateau before the collar ends and the collar's own edge cannot become
///     a second seam;
///   * "smooth" is the ONE-CODE rule this engine's boundary budget is already
///     built on, read on the GUIDE rather than on the render, and read the
///     same way the budget reads the scene: the guide is box-averaged over a
///     `probe` window and the variation is the largest step between that
///     average here and `probe` px away along either axis — the guide's own
///     change over the probe distance, against [`ONE_CODE`]. A
///     neighbourhood the guide crosses by less than one code cannot mask a
///     seam of even one code. A DIFFERENCE of smoothed values rather than an
///     average of |gradient|, because the second does not cancel: an 8-bit
///     gradient IS a staircase of one-code steps, whose |gradient| averages
///     to something near the threshold however wide the window, and a rule
///     built on it would refuse to widen exactly the featureless haze it
///     exists for;
///   * the credit falls linearly from 1 at a perfectly flat guide to 0 at
///     one code, is read ON the 50% contour, and is then carried out to
///     every pixel the broadening kernel can reach by taking the SMALLEST
///     credit in reach ([`spread_min`]) — because smoothness is a property
///     of the CROSSING, not of the pixel being written. Nine pixels to the
///     flat side of a mesa silhouette the guide is perfectly smooth, and a
///     rule that read the guide where it writes would widen that flank while
///     the silhouette itself stayed pinned: a stepped profile, and a worse
///     boundary than the one it started from;
///   * the delivered alpha is `alpha + credit * (broadened - alpha)`, so the
///     ramp widens exactly as far as the guide is featureless and NOT AT ALL
///     within reach of an edge, which is what keeps a silhouette crisp.
///
/// It abstains — and says so — when nothing is smooth, when the widened alpha
/// would be byte-identical anyway, when coverage moves by more than
/// [`COVERAGE_DELTA_MAX`], or when anything outside the collar moved.
///
/// SOFT masks only. Spatial tiles and free masks are hard 0/255 by
/// construction and are measured by the cross-boundary-step ruler; widening
/// one would change what that ruler is reading. The call sites are the two
/// semantic-zone producers in `fit_zoned.rs`, beside the guided refinement.
pub(crate) fn widen_smooth_feather(guide: &DynamicImage, mask: &GrayImage) -> WidenOutcome {
    let (width, height) = mask.dimensions();
    let (w, h) = (width as usize, height as usize);
    let nothing = WidenReading {
        widened_share: 0.0,
        max_radius: 0,
        coverage_delta: 0.0,
        core_changed: 0,
    };
    if w < 3 || h < 3 {
        return WidenOutcome::Abstained { reading: nothing };
    }
    let cap = ((height as f32 * FEATHER_CAP_SHARE).round() as usize).max(1);
    // A quarter of the cap: wide enough that one texture pixel does not read
    // as an edge, narrow enough that the verdict is local to the crossing.
    let probe = (cap / 4).max(1);
    let original = mask.as_raw();
    let alpha = original.iter().map(|v| *v as f32 / 255.0).collect::<Vec<_>>();
    let mut contour = vec![false; alpha.len()];
    let mut contour_len = 0usize;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let inside = alpha[i] >= 0.5;
            let crosses = (x + 1 < w && (alpha[i + 1] >= 0.5) != inside)
                || (x > 0 && (alpha[i - 1] >= 0.5) != inside)
                || (y + 1 < h && (alpha[i + w] >= 0.5) != inside)
                || (y > 0 && (alpha[i - w] >= 0.5) != inside);
            contour[i] = crosses;
            contour_len += usize::from(crosses);
        }
    }
    if contour_len == 0 {
        return WidenOutcome::Abstained { reading: nothing };
    }
    let guide = guide_luma(guide, width, height);
    let smoothed = box_mean(&guide, w, h, probe);
    let variation = |x: usize, y: usize| -> f32 {
        let here = smoothed[y * w + x];
        let reach = |xx: usize, yy: usize| (smoothed[yy * w + xx] - here).abs();
        reach(x.saturating_sub(probe), y)
            .max(reach((x + probe).min(w - 1), y))
            .max(reach(x, y.saturating_sub(probe)))
            .max(reach(x, (y + probe).min(h - 1)))
    };
    let collar = dilate(&contour, w, h, 2 * cap);
    let broadened = box_mean(&alpha, w, h, cap);
    let mut on_contour = vec![f32::INFINITY; alpha.len()];
    for (i, earned) in on_contour.iter_mut().enumerate() {
        if contour[i] {
            *earned = (1.0 - variation(i % w, i / w) / ONE_CODE).clamp(0.0, 1.0);
        }
    }
    // Out to `cap`, which is exactly how far the broadening kernel reaches.
    let carried = spread_min(&on_contour, w, h, cap);
    let credit = |i: usize| -> f32 {
        if collar[i] && carried[i].is_finite() {
            carried[i]
        } else {
            0.0
        }
    };
    let widened = (0..alpha.len())
        .map(|i| {
            let earned = credit(i);
            alpha[i] + earned * (broadened[i] - alpha[i])
        })
        .collect::<Vec<_>>();
    let bytes = widened
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect::<Vec<u8>>();
    let smooth = (0..alpha.len()).filter(|i| contour[*i] && credit(*i) > 0.0).count();
    let core_changed = bytes
        .iter()
        .zip(original)
        .zip(&collar)
        .filter(|((after, before), in_collar)| !**in_collar && after != before)
        .count();
    let delivered = bytes.iter().map(|v| *v as f32 / 255.0).collect::<Vec<_>>();
    let coverage = |values: &[f32]| {
        values.iter().map(|v| *v as f64).sum::<f64>() / alpha.len() as f64
    };
    let reading = WidenReading {
        widened_share: smooth as f32 / contour_len as f32,
        max_radius: cap as u32,
        coverage_delta: (coverage(&delivered) - coverage(&alpha)).abs() as f32,
        core_changed,
    };
    if smooth == 0
        || bytes.as_slice() == original.as_slice()
        || core_changed > 0
        || reading.coverage_delta > COVERAGE_DELTA_MAX
    {
        return WidenOutcome::Abstained { reading };
    }
    let mut output = GrayImage::new(width, height);
    for (pixel, value) in output.pixels_mut().zip(bytes) {
        *pixel = Luma([value]);
    }
    WidenOutcome::Widened { mask: output, reading }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn aligned_fixture(aligned: bool) -> (DynamicImage, GrayImage) {
        step_fixture(if aligned { 32 } else { 16 })
    }

    fn step_fixture(guide_edge: u32) -> (DynamicImage, GrayImage) {
        let mut guide = RgbImage::new(64, 32);
        let mut mask = GrayImage::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                let value = if x < guide_edge { 20 } else { 235 };
                guide.put_pixel(x, y, Rgb([value, value, value]));
                let alpha = if x < 32 { 0 } else { 255 };
                mask.put_pixel(x, y, Luma([alpha]));
            }
        }
        (DynamicImage::ImageRgb8(guide), mask)
    }

    #[test]
    fn guided_refinement_restores_every_core_pixel() {
        let (guide, mask) = aligned_fixture(true);
        let outcome = guided_refine(&guide, &mask, 4, (4.0f32 / 255.0).powi(2));
        let (refined, reading) = match outcome {
            RefineOutcome::Kept { mask, reading } => (mask, reading),
            RefineOutcome::Abstained { reading } => {
                assert_eq!(reading.core_changed, 0);
                return;
            }
        };
        assert_eq!(reading.core_changed, 0);
        for y in 0..32 {
            for x in 0..8 {
                assert_eq!(refined.get_pixel(x, y), mask.get_pixel(x, y));
            }
            for x in 56..64 {
                assert_eq!(refined.get_pixel(x, y), mask.get_pixel(x, y));
            }
        }

        // The 2*radius collar width is itself load-bearing: the filter's
        // coverage-compensating tail lives in (radius, 2*radius]. With the
        // guide edge offset 4px from the mask edge, the full collar keeps
        // whole-frame coverage drift at ~0.0001 and the outcome Kept;
        // truncating the collar to `radius` cuts the tail and inflates the
        // drift past the 0.002 conservation gate (measured 0.0041).
        let (guide, mask) = step_fixture(28);
        match guided_refine(&guide, &mask, 8, (4.0f32 / 255.0).powi(2)) {
            RefineOutcome::Kept { reading, .. } => {
                assert_eq!(reading.core_changed, 0);
                assert!(reading.coverage_delta <= 0.001, "{reading:?}");
            }
            RefineOutcome::Abstained { reading } => {
                panic!("offset-guide refinement lost its compensating tail: {reading:?}")
            }
        }
    }

    /// A 64x256 guide whose TOP half is featureless haze and whose bottom
    /// half carries a 40-code vertical edge exactly under the mask contour,
    /// plus a `ramp`-px alpha transition at x=32. One fixture, two verdicts:
    /// the widener must find the haze and leave the silhouette alone.
    ///
    /// 256 ROWS on purpose: the cap is a share of the HEIGHT, and the widths
    /// it is derived from are stated in analysis pixels (see
    /// [`FEATHER_CAP_SHARE`]), so only a frame at the analysis grid's own
    /// scale exercises the rule. A 128-row fixture would test a 4-px radius
    /// the shipped geometry never asks for.
    fn split_guide_fixture(ramp: f32, all_edges: bool) -> (DynamicImage, GrayImage) {
        let (w, h) = (64u32, 256u32);
        let guide = RgbImage::from_fn(w, h, |x, y| {
            let haze = 100.0 + y as f32 * 8.0 / (h - 1) as f32;
            let edge = if x >= 32 { 40.0 } else { 0.0 };
            let value = if all_edges || y >= h / 2 { haze + edge } else { haze };
            let v = value.round().clamp(0.0, 255.0) as u8;
            Rgb([v, v, v])
        });
        let mask = GrayImage::from_fn(w, h, |x, _| {
            let t = ((x as f32 - (32.0 - ramp * 0.5)) / ramp).clamp(0.0, 1.0);
            Luma([(t * 255.0).round() as u8])
        });
        (DynamicImage::ImageRgb8(guide), mask)
    }

    /// The ramp's own width, in pixels that are neither fully in nor fully
    /// out — the quantity the widening exists to change.
    fn ramp_span(image: &GrayImage, y: u32) -> usize {
        (0..image.width())
            .filter(|x| (1..=254).contains(&image.get_pixel(*x, y).0[0]))
            .count()
    }

    #[test]
    fn the_feather_widens_over_haze_and_leaves_a_silhouette_alone() {
        let (guide, mask) = split_guide_fixture(3.0, false);
        let (widened, reading) = match widen_smooth_feather(&guide, &mask) {
            WidenOutcome::Widened { mask, reading } => (mask, reading),
            WidenOutcome::Abstained { reading } => {
                panic!("half of this contour is featureless haze: {reading:?}")
            }
        };
        assert_eq!(reading.core_changed, 0, "nothing outside the collar may move: {reading:?}");
        assert!(
            reading.coverage_delta <= COVERAGE_DELTA_MAX,
            "the ramp is symmetric about the contour, so coverage is conserved: {reading:?}"
        );
        assert_eq!(reading.max_radius, 8, "3% of 256 rows, rounded: {reading:?}");
        assert!(
            (0.35..=0.65).contains(&reading.widened_share),
            "exactly the haze half of the contour may be widened: {reading:?}"
        );
        assert!(
            ramp_span(&widened, 32) >= 3 * ramp_span(&mask, 32),
            "the haze row's ramp must really widen: {} from {}",
            ramp_span(&widened, 32),
            ramp_span(&mask, 32)
        );
        for x in 0..64u32 {
            assert_eq!(
                widened.get_pixel(x, 224),
                mask.get_pixel(x, 224),
                "the silhouette row must survive byte for byte at x={x}"
            );
        }
    }

    #[test]
    fn an_all_edges_guide_gets_no_widening_at_all() {
        let (guide, mask) = split_guide_fixture(3.0, true);
        match widen_smooth_feather(&guide, &mask) {
            WidenOutcome::Abstained { reading } => {
                assert_eq!(
                    reading.widened_share, 0.0,
                    "nothing was smooth, and the abstention has to say so: {reading:?}"
                );
            }
            WidenOutcome::Widened { reading, .. } => {
                panic!("a guide that is edge everywhere earns no ramp: {reading:?}")
            }
        }
    }

    #[test]
    fn refinement_that_lowers_edge_alignment_is_rejected() {
        if let Some(root) = crate::fit::calibration_corpus() {
            let guide = image::open(root.join("neutral.jpg")).unwrap();
            let mask = image::open(root.join("sky-mask.png")).unwrap().to_luma8();
            match guided_refine(&guide, &mask, 8, (4.0f32 / 255.0).powi(2)) {
                RefineOutcome::Abstained { reading } => {
                    assert!(reading.edge_after < reading.edge_before, "{reading:?}");
                }
                RefineOutcome::Kept { reading, .. } => {
                    panic!("calibration refinement with worse alignment kept: {reading:?}")
                }
            }
        } else {
            let reading = RefineReading {
                coverage_delta: 0.000673,
                edge_before: 0.046444,
                edge_after: 0.023914,
                core_changed: 0,
            };
            assert!(!refinement_passes(reading), "worse measured alignment passed");
        }
    }
}
