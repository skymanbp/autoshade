//! Return the frame's own luminance residual after demosaic, in linear light.
//! One scalar goes along RGB grey; channel differences cannot acquire noise.
use anyhow::Result;
use rayon::prelude::*;

use super::{ExportColorSpace, linear_to_srgb, srgb_to_linear};

pub(super) fn weights(space: ExportColorSpace) -> [f32; 3] {
    let row = super::rgb_to_xyz(super::space_primaries(space), super::D65_XY)[1];
    let sum = row.iter().sum::<f32>();
    row.map(|v| v / sum)
}

fn luminance(rgb: [f32; 3], weights: [f32; 3]) -> f32 {
    rgb[0] * weights[0] + rgb[1] * weights[1] + rgb[2] * weights[2]
}

/// The closure owns the expensive develop. Endpoints and fallback never call
/// it, and its RGB allocation dies here: only one f32 Y plane survives.
pub(super) fn capture_original(
    strength: f32,
    applicable: bool,
    weights: [f32; 3],
    develop: impl FnOnce() -> Result<Vec<[f32; 3]>>,
) -> Result<Option<Vec<f32>>> {
    if !(strength > 0.0 && strength < 1.0 && applicable) {
        return Ok(None);
    }
    let original = develop()?;
    Ok(Some(original.par_iter().map(|p| luminance(p.map(srgb_to_linear), weights)).collect()))
}

/// Buffers in every working space use the sRGB transfer. Do not clip here:
/// negative grain is real; the final output container owns its gamut boundary.
pub(super) fn return_luminance(
    clean: &mut [[f32; 3]],
    original_y: &[f32],
    share: f32,
    weights: [f32; 3],
) {
    if share == 0.0 {
        return;
    }
    assert_eq!(clean.len(), original_y.len(), "grain return must use the identical developed frame");
    clean.par_iter_mut().zip(original_y.par_iter()).for_each(|(px, original)| {
        let linear = px.map(srgb_to_linear);
        let delta = share * (original - luminance(linear, weights));
        *px = linear.map(|c| linear_to_srgb(c + delta));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 3e-6, "{a} != {b}");
    }

    #[test]
    fn neutral_return_preserves_chroma_and_obeys_the_luminance_law() {
        let mut state = 17u32;
        let mut random = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1u32 << 24) as f32
        };
        for space in [ExportColorSpace::Srgb, ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb] {
            let w = weights(space);
            for _ in 0..100 {
                let clean = [random(), random(), random()];
                let original = random();
                for k in 0..=10 {
                    let share = k as f32 / 10.0;
                    let mut out = [clean.map(linear_to_srgb)];
                    return_luminance(&mut out, &[original], share, w);
                    let out = out[0].map(srgb_to_linear);
                    close(out[0] - out[1], clean[0] - clean[1]);
                    close(out[2] - out[1], clean[2] - clean[1]);
                    close(luminance(out, w),
                        luminance(clean, w) + share * (original - luminance(clean, w)));
                }
            }
        }
    }

    #[test]
    fn full_clean_is_bit_exact_and_does_not_develop_the_original() {
        use std::cell::Cell;
        let count = Cell::new(0);
        let w = weights(ExportColorSpace::Srgb);
        for (strength, applicable, want) in [(0.0, true, 0), (1.0, true, 0), (0.7, false, 0), (0.7, true, 1)] {
            let result = capture_original(strength, applicable, w, || {
                count.set(count.get() + 1);
                Ok(vec![[0.5; 3]])
            }).unwrap();
            assert_eq!(count.get(), want);
            assert_eq!(result.is_some(), want == 1);
        }
        let before = [[-0.03, 0.34987654, 1.234567]];
        let mut out = before;
        return_luminance(&mut out, &[], 0.0, w);
        assert_eq!(out.map(|p| p.map(f32::to_bits)), before.map(|p| p.map(f32::to_bits)));
    }

    #[test]
    fn grey_noise_returns_at_the_exact_share_and_pure_chroma_returns_nothing() {
        let w = weights(ExportColorSpace::Srgb);
        for k in 0..=10 {
            let share = k as f32 / 10.0;
            let clean = [0.2, 0.35, 0.1];
            for residual in [-0.03, 0.0, 0.03] {
                let original_y = luminance(clean, w) + residual;
                let mut out = [clean.map(linear_to_srgb)];
                return_luminance(&mut out, &[original_y], share, w);
                for (c, d) in out[0].map(srgb_to_linear).iter().zip(clean) {
                    close(c - d, share * residual);
                }
            }
            // A genuine nonzero chroma perturbation whose Y is zero.
            let original = [clean[0] + 0.02, clean[1] - 0.02*w[0]/w[1], clean[2]];
            let mut out = [clean.map(linear_to_srgb)];
            return_luminance(&mut out, &[luminance(original, w)], share, w);
            for (c, d) in out[0].map(srgb_to_linear).iter().zip(clean) { close(*c, d); }
        }
    }

    #[test]
    fn srgb_and_wide_exports_carry_the_same_linear_grain() {
        let original = [0.42, 0.24, 0.15];
        let clean = [0.38, 0.22, 0.17];
        let mut reference = [clean.map(linear_to_srgb)];
        return_luminance(&mut reference, &[luminance(original, weights(ExportColorSpace::Srgb))],
            0.3, weights(ExportColorSpace::Srgb));
        let reference = reference[0].map(srgb_to_linear);
        for space in [ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb] {
            let matrix = super::super::srgb_to_space_matrix(space).unwrap();
            let o = super::super::mat_vec3(&matrix, &original);
            let d = super::super::mat_vec3(&matrix, &clean);
            let mut out = [d.map(linear_to_srgb)];
            return_luminance(&mut out, &[luminance(o, weights(space))], 0.3, weights(space));
            let expected = super::super::mat_vec3(&matrix, &reference);
            for (got, want) in out[0].map(srgb_to_linear).iter().zip(expected) {
                close(*got, want);
            }
        }
    }
}
