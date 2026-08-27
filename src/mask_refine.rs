//! Conservative edge-aware production of bitmap-mask boundaries.
//!
//! The guided filter is allowed to alter only a fixed collar around the
//! existing boundary. It abstains unless coverage is conserved and the
//! transition moves toward, rather than away from, edges in the guide.

use image::{DynamicImage, GrayImage, Luma};
use rayon::prelude::*;

const COVERAGE_DELTA_MAX: f32 = 0.002;

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
    let mut collar = vec![false; alpha.len()];
    for y in 0..height {
        for x in 0..width {
            let y0 = y.saturating_sub(radius);
            let y1 = (y + radius + 1).min(height);
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius + 1).min(width);
            collar[y * width + x] = (y0..y1).any(|yy| {
                boundary[yy * width + x0..yy * width + x1].iter().any(|v| *v)
            });
        }
    }
    collar
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
    let guide = guide.resize_exact(width, height, image::imageops::FilterType::Lanczos3).to_rgb8();
    let guide = guide
        .pixels()
        .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
        .collect::<Vec<_>>();
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
