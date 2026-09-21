//! Strong isolated hot sites, mapped only on the positive-strength Bayer AI path.
//! All decisions read the unchanged mosaic. A three-pixel border is left alone:
//! the edge neighbour's own same-colour median needs two more sensor pixels.

use anyhow::{bail, Result};
use rayon::prelude::*;
use rawler::rawimage::RawImageData;

const PLANE_TILE: usize = 64;
const HOT_SIGMA: f32 = 20.0;
const NEIGHBOUR_SIGMA: f32 = 3.0;

pub(crate) fn map_for(raw: &mut rawler::RawImage, strength: Option<f32>) -> Result<usize> {
    if !strength.is_some_and(|s| s > 0.0) || super::mosaic_args_for(raw).is_err() {
        return Ok(0);
    }
    let RawImageData::Integer(data) = &mut raw.data else { return Ok(0) };
    if raw.width.checked_mul(raw.height) != Some(data.len()) {
        bail!("hot-pixel mapping: mosaic dimensions do not match its samples");
    }
    let corrections = find(data, raw.width, raw.height);
    for &(i, value) in &corrections {
        data[i] = value;
    }
    Ok(corrections.len())
}

fn median(values: &mut [f32]) -> f32 {
    let n = values.len();
    let (lo, mid, _) = values.select_nth_unstable_by(n / 2, f32::total_cmp);
    if n.is_multiple_of(2) {
        (lo.iter().copied().fold(f32::NEG_INFINITY, f32::max) + *mid) * 0.5
    } else {
        *mid
    }
}

fn median8(data: &[u16], w: usize, x: usize, y: usize) -> f32 {
    let mut neighbours = [0.0; 8];
    let mut k = 0;
    for yy in [y - 2, y, y + 2] {
        for xx in [x - 2, x, x + 2] {
            if xx != x || yy != y {
                neighbours[k] = f32::from(data[yy * w + xx]);
                k += 1;
            }
        }
    }
    median(&mut neighbours)
}

struct Tile {
    sigma: f32,
    candidates: Vec<(usize, f32)>,
}

fn find(data: &[u16], w: usize, h: usize) -> Vec<(usize, u16)> {
    if w < 7 || h < 7 {
        return Vec::new();
    }
    let side = 2 * PLANE_TILE;
    let nx = w.div_ceil(side);
    let ny = h.div_ceil(side);
    // One tile's residuals per worker, not a full-frame f32 sigma map. Samples
    // at the two-pixel boundary contribute to the fit, but cannot be replaced.
    let tiles: Vec<Tile> = (0..4 * nx * ny).into_par_iter().map(|id| {
        let phase = id / (nx * ny);
        let tx = id % nx;
        let ty = id / nx % ny;
        let mut samples = Vec::with_capacity(PLANE_TILE * PLANE_TILE);
        let mut residuals = Vec::with_capacity(PLANE_TILE * PLANE_TILE);
        for y in (ty * side + phase / 2..((ty + 1) * side).min(h)).step_by(2) {
            for x in (tx * side + phase % 2..((tx + 1) * side).min(w)).step_by(2) {
                if x < 2 || y < 2 || x + 2 >= w || y + 2 >= h { continue; }
                let background = median8(data, w, x, y);
                let residual = f32::from(data[y * w + x]) - background;
                residuals.push(residual);
                samples.push((y * w + x, background, residual));
            }
        }
        if residuals.is_empty() { return Tile { sigma: 0.0, candidates: Vec::new() }; }
        let centre = median(&mut residuals);
        for r in &mut residuals { *r = (*r - centre).abs(); }
        let sigma = 1.4826 * median(&mut residuals);
        let candidates = samples.into_iter().filter_map(|(i, background, excess)| {
            let (x, y) = (i % w, i / w);
            (sigma > 0.0 && excess > HOT_SIGMA * sigma
                && x >= 3 && y >= 3 && x + 3 < w && y + 3 < h).then_some((i, background))
        }).collect();
        Tile { sigma, candidates }
    }).collect();
    let sigma_at = |x: usize, y: usize| {
        tiles[((y % 2) * 2 + x % 2) * nx * ny + (y / side) * nx + x / side].sigma
    };
    tiles.par_iter().flat_map_iter(|tile| tile.candidates.iter().filter_map(|&(i, background)| {
        let (x, y) = (i % w, i / w);
        let isolated = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)].iter().all(|&(xx, yy)| {
            let sigma = sigma_at(xx, yy);
            let excess = f32::from(data[yy * w + xx]) - median8(data, w, xx, yy);
            sigma > 0.0 && excess <= NEIGHBOUR_SIGMA * sigma
        });
        isolated.then_some((i, background.round() as u16))
    })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise() -> Vec<u16> {
        let mut state = 21u32;
        (0..256 * 256).map(|_| {
            let mut sum = 0.0;
            for _ in 0..12 {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                sum += (state >> 8) as f32 / (1u32 << 24) as f32;
            }
            (2000.0 + 20.0 * (sum - 6.0)).round() as u16
        }).collect()
    }

    #[test]
    fn strong_isolated_sites_on_all_phases_are_mapped_but_border_and_faint_are_not() {
        let mut data = noise();
        let sites = [(80, 80), (121, 80), (80, 121), (121, 121)];
        for (x, y) in sites { data[y * 256 + x] = 2880; }
        data[180 * 256 + 180] = 2330; // about 15 residual sigmas
        data[2 * 256 + 80] = 5000; // cannot inspect its edge neighbour's median
        let found = find(&data, 256, 256);
        assert_eq!(found.len(), 4);
        for (x, y) in sites {
            let value = found.iter().find(|(i, _)| *i == y * 256 + x).unwrap().1;
            assert_eq!(value, median8(&data, 256, x, y).round() as u16);
        }
    }

    #[test]
    fn a_star_with_bright_edge_neighbours_is_not_a_hot_site() {
        let mut data = noise();
        for y in 126..=130 {
            for x in 126..=130 {
                let r2 = (x as f32 - 128.0).powi(2) + (y as f32 - 128.0).powi(2);
                data[y * 256 + x] = (f32::from(data[y * 256 + x]) + 1100.0 * (-r2 / 1.28).exp()).round() as u16;
            }
        }
        assert!(find(&data, 256, 256).iter().all(|(i, _)| *i != 128 * 256 + 128),
            "a star's other-colour neighbours must protect its centre");
    }

    #[test]
    fn ordinary_and_zero_strength_renders_never_touch_the_mosaic() {
        let mut raw = super::super::tests::bayer_fixture("RGGB", 256, 256, &[512], 16383);
        let mut data = noise(); data[80 * 256 + 80] = 2880;
        raw.data = RawImageData::Integer(data.clone());
        for strength in [None, Some(0.0)] {
            assert_eq!(map_for(&mut raw, strength).unwrap(), 0);
            let RawImageData::Integer(after) = &raw.data else { panic!("integer fixture") };
            assert_eq!(after, &data);
        }
        assert_eq!(map_for(&mut raw, Some(0.71)).unwrap(), 1);
    }

    #[test]
    fn hot_mapping_precedes_both_original_capture_and_cleaning() {
        let source = include_str!("../render.rs");
        let start = source.find("let strength = denoise.map_or").unwrap();
        let body = &source[start..source.find("fn develop_raw_buffer(").unwrap()];
        let mapping = body.find("hot_pixels::map_for(").expect("hot-site mapping hook");
        assert!(mapping < body.find("denoise_grain::capture_original(").unwrap());
        assert!(mapping < body.find("crate::denoise::denoise_mosaic(").unwrap());
    }
}
