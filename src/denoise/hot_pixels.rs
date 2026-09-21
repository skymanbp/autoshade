//! Strong isolated hot sites, mapped on every supported Bayer develop before
//! anything else reads the mosaic. A hot site is a defect of the sensor, not
//! noise, so it does not wait for AI denoise to be asked for: until 2026-09-21
//! it did (the rule arrived with the denoise lane and stayed inside it), which
//! left the same photograph with its hot pixels in an ordinary develop and
//! without them in a denoised one. The user ruled that ordinary develops map too.
//! All decisions read the unchanged mosaic. A three-pixel border is left alone:
//! a neighbour's own same-colour median needs two more sensor pixels.
//!
//! The rule is a test against a null model: flat ground, unclipped, lit like
//! the rest of its tile. It had only ever been run on night sky, where that
//! model holds. Run on six ordinary city and dusk frames (2026-09-21) it mapped
//! 1 / 3866 / 1213 / 1246 / 4003 / 308 sites inside the picture, none at a
//! photosite any other frame shared: out-of-focus lights, sunlit walls, blown
//! signs, a red LED board. On three night frames 66 of the sites it mapped sat
//! inside stars. Each piece of evidence is therefore asked whether it CAN
//! testify where it stands, every doubt is read for the scene, and the same six
//! frames now map 0 / 0 / 0 / 6 / 0 / 4 (what is left is named in ARCHITECTURE):
//!
//!   * "20 sigma" is the noise WHERE THE SITE STANDS. The tile's residual MAD
//!     is the noise of the tile's majority. A bright structure in a mostly dark
//!     tile carries many times that in photon noise, and texture in one colour
//!     plane makes every bright sample of it an outlier of the same-colour
//!     median. One measurement answers both: the spread of the same-colour
//!     samples within four pixels of the site joins its scale at a half. Left
//!     out, four of the ordinary frames map 10 / 43 / 46 / 32 sites more; joined
//!     whole, the three night frames lose 23 / 7 / 24 of theirs. Flat Gaussian
//!     ground exceeds its tile's sigma at a half once in 133,000 sites
//!     (simulated), so flat ground is judged by its tile as it always was.
//!   * Clipped samples measure no noise. A tile that is part blown had its MAD
//!     collapse, and every ripple of its unblown part became a 20-sigma event.
//!     So the tile's statistics and the site's neighbourhood are taken over
//!     samples that can move, and a site whose background is clipped, or most
//!     of whose neighbourhood is, is not judged. (On the ten frames each of the
//!     three covers for the others: two sites on blown ground return only when
//!     the last two are both gone.)
//!   * A witness at white cannot show an excess, so it cannot vouch that the
//!     site stands alone; and the four DIAGONAL neighbours are asked as well as
//!     the four edge ones — for a green site they are the same colour, so no
//!     colour of light can hide from them. Asked of the edge four only, the
//!     night frames map three sites more.
//!
//! What stood here first, and why it went. A line through the frame's tiles
//! (variance against level, per CFA phase) carried the tile's sigma to the
//! site's brightness, beside the spread of the ring of eight at a third. Taken
//! out of that rule, the line changed three sites on the ten frames — and two
//! of them were lone photosites on a night sky that this rule maps. It had also
//! needed the median's own noise taken out of every background first, or it
//! declined 24 of the sites on the flat sky of two high-ISO night frames. The
//! neighbourhood measures at the site what the line inferred from the frame.

use anyhow::{bail, Result};
use rayon::prelude::*;
use rawler::rawimage::RawImageData;

const PLANE_TILE: usize = 64;
const HOT_SIGMA: f32 = 20.0;
const NEIGHBOUR_SIGMA: f32 = 3.0;
/// How much of the spread of a site's own neighbourhood joins its scale. At a
/// half, flat Gaussian ground exceeds its tile's sigma once in 133,000 sites
/// (simulated, 4e7 neighbourhoods) and then by a hair, so flat ground is judged
/// by its tile as it always was; a bright or textured neighbourhood is judged
/// by ten of its own sigmas.
const LOCAL_SHARE: f32 = 0.5;
/// A sample this far up from black toward white, or whose background is, has
/// part of its noise cut off by the clip and joins no tile's statistics.
const CLIP_SHARE: f32 = 0.9;

/// Map the frame's hot sites in place and say how many there were. The gate is
/// the one the AI path uses — a 2×2 Bayer mosaic of integer samples with its
/// levels declared — so the two paths can never disagree about which frames
/// are mapped; any other sensor is returned untouched.
pub(crate) fn map_for(raw: &mut rawler::RawImage) -> Result<usize> {
    let Ok(levels) = super::mosaic_args_for(raw) else { return Ok(0) };
    let RawImageData::Integer(data) = &mut raw.data else { return Ok(0) };
    if raw.width.checked_mul(raw.height) != Some(data.len()) {
        bail!("hot-pixel mapping: mosaic dimensions do not match its samples");
    }
    let corrections = find(data, raw.width, raw.height, &levels);
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

/// The eight same-colour neighbours of a sample, two pixels away.
#[inline(always)]
fn ring8(data: &[u16], w: usize, x: usize, y: usize) -> [u16; 8] {
    let (above, row, below) = ((y - 2) * w + x, y * w + x, (y + 2) * w + x);
    [
        data[above - 2], data[above], data[above + 2],
        data[row - 2], data[row + 2],
        data[below - 2], data[below], data[below + 2],
    ]
}

/// The median of the eight same-colour neighbours: half the sum of the 4th and
/// 5th of the sorted eight, exact in f32 (the sum stays below 2^17).
///
/// A fixed network on the integer samples rather than a general selection on
/// floats, because this runs once per sensor sample on EVERY develop since
/// 2026-09-21. The scan as first written — that selection, plus a record kept
/// for every sample until its tile's sigma was known — cost 0.75–0.78 s of a
/// 61 MP develop (two frames), close to a fifth of it; what it costs now is in
/// ARCHITECTURE beside the frames it was timed on.
fn median8(data: &[u16], w: usize, x: usize, y: usize) -> f32 {
    let mut v = ring8(data, w, x, y);
    // An optimal 19-comparator sorting network for eight inputs (checked over
    // all 256 zero-one inputs when it was written, and against a plain sort in
    // the tests), less its last comparator: that one only orders the middle
    // pair, and the pair is summed. Literal indices, so the eight values live
    // in registers.
    macro_rules! order {
        ($($a:literal $b:literal),+) => {$(
            let (lo, hi) = (v[$a].min(v[$b]), v[$a].max(v[$b]));
            v[$a] = lo;
            v[$b] = hi;
        )+};
    }
    order!(0 1, 2 3, 4 5, 6 7, 0 2, 1 3, 4 6, 5 7, 1 2, 5 6, 0 4, 3 7, 1 5, 2 6, 1 4, 3 6, 2 4, 3 5);
    (u32::from(v[3]) + u32::from(v[4])) as f32 * 0.5
}

/// The spread, as a sigma, of the same-colour samples within four pixels of a
/// site: the MAD about their median of those that can carry noise (below the
/// clip), the site itself left out. It is the noise AT THE SITE'S OWN
/// BRIGHTNESS and the texture of its plane at once, measured where the site
/// stands. `None` when fewer than half of them can carry noise: mostly clipped
/// ground measures none, so nothing on it is judged. Candidates only, so plain
/// sorts; in doubled integers, because a median of integers may be a half.
fn local_sigma(data: &[u16], w: usize, h: usize, x: usize, y: usize, clip: f32) -> Option<f32> {
    let mut twice = [0u32; 24];
    let (mut n, mut seen) = (0, 0);
    // Two steps back where the frame allows them, the site's own parity kept.
    let (x0, y0) = (x - 2 * (x / 2).min(2), y - 2 * (y / 2).min(2));
    for yy in (y0..h.min(y + 5)).step_by(2) {
        for xx in (x0..w.min(x + 5)).step_by(2) {
            if (xx, yy) == (x, y) {
                continue;
            }
            seen += 1;
            let value = data[yy * w + xx];
            if f32::from(value) < clip {
                twice[n] = 2 * u32::from(value);
                n += 1;
            }
        }
    }
    if n == 0 || 2 * n < seen {
        return None;
    }
    let near = &mut twice[..n];
    near.sort_unstable();
    let median = (near[(n - 1) / 2] + near[n / 2]) / 2;
    for value in near.iter_mut() {
        *value = value.abs_diff(median);
    }
    near.sort_unstable();
    Some(1.4826 * (near[(n - 1) / 2] + near[n / 2]) as f32 * 0.25)
}

struct Tile {
    /// The residual MAD of the samples that can carry noise (neither they nor
    /// their background near the clip). Zero when there were none.
    sigma: f32,
    /// (index, background, excess) of every sample above 20 TILE sigmas — a
    /// superset of the hot sites, since the scale that finally judges one is
    /// never smaller than its tile's.
    candidates: Vec<(usize, f32, f32)>,
}

/// One CFA phase of one tile, `id` counting phases, then rows, then columns of
/// tiles. Samples at the two-pixel boundary contribute to the statistics, but
/// cannot be candidates.
fn scan(data: &[u16], w: usize, h: usize, levels: &super::MosaicArgs, id: usize) -> Tile {
    let side = 2 * PLANE_TILE;
    let (nx, ny) = (w.div_ceil(side), h.div_ceil(side));
    let phase = id / (nx * ny);
    let tx = id % nx;
    let ty = id / nx % ny;
    let clip = levels.black[phase] + CLIP_SHARE * (levels.white - levels.black[phase]);
    // This phase's samples in the tile that have all eight same-colour
    // neighbours: the two-pixel frame border is cut from the ranges, not
    // tested per sample. `side` is even, so a start moved in by 2 keeps its phase.
    let inside = |start: usize| if start < 2 { start + 2 } else { start };
    let (y0, y1) = (inside(ty * side + phase / 2), ((ty + 1) * side).min(h - 2));
    let (x0, x1) = (inside(tx * side + phase % 2), ((tx + 1) * side).min(w - 2));
    let mut residuals = Vec::with_capacity(PLANE_TILE * PLANE_TILE);
    let mut backgrounds = Vec::with_capacity(PLANE_TILE * PLANE_TILE);
    // The samples that can carry noise, apart: the two selections below
    // reorder their input, and the scan's own columns keep their order for
    // the candidate sweep.
    let mut spread = Vec::with_capacity(PLANE_TILE * PLANE_TILE);
    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(2) {
            let (value, background) = (f32::from(data[y * w + x]), median8(data, w, x, y));
            residuals.push(value - background);
            backgrounds.push(background);
            if value < clip && background < clip {
                spread.push(value - background);
            }
        }
    }
    if spread.is_empty() {
        return Tile { sigma: 0.0, candidates: Vec::new() };
    }
    let centre = median(&mut spread);
    for r in &mut spread { *r = (*r - centre).abs(); }
    let sigma = 1.4826 * median(&mut spread);
    // Above 20 TILE sigmas: every hot site is among these. A tile without a
    // sigma (every measuring residual alike) has no scale to judge by.
    let mut candidates = Vec::new();
    let mut sample = residuals.iter().zip(&backgrounds);
    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(2) {
            let (&excess, &background) = sample.next().expect("one residual per scanned sample");
            // …on ground the statistics above would have read: over a clipped background an excess is
            // what is left under white, not a measurement.
            if sigma > 0.0 && background < clip && excess > HOT_SIGMA * sigma && x >= 3 && y >= 3 && x + 3 < w && y + 3 < h {
                candidates.push((y * w + x, background, excess));
            }
        }
    }
    Tile { sigma, candidates }
}

fn find(data: &[u16], w: usize, h: usize, levels: &super::MosaicArgs) -> Vec<(usize, u16)> {
    if w < 7 || h < 7 {
        return Vec::new();
    }
    let side = 2 * PLANE_TILE;
    let nx = w.div_ceil(side);
    let ny = h.div_ceil(side);
    // One tile's residuals per worker, not a full-frame f32 sigma map.
    let tiles: Vec<Tile> = (0..4 * nx * ny).into_par_iter().map(|id| scan(data, w, h, levels, id)).collect();
    let per_phase = nx * ny;
    let tile_at = |x: usize, y: usize| &tiles[((y % 2) * 2 + x % 2) * per_phase + (y / side) * nx + x / side];
    tiles.par_iter().flat_map_iter(|tile| tile.candidates.iter().filter_map(|&(i, background, excess)| {
        let (x, y) = (i % w, i / w);
        let phase = (y % 2) * 2 + x % 2;
        let clip = levels.black[phase] + CLIP_SHARE * (levels.white - levels.black[phase]);
        // The tile's sigma is the noise of the tile's majority. Where the site
        // stands the ground may be brighter (many times the photon noise) or
        // textured in its own plane (every bright sample an outlier of the
        // same-colour median): its own neighbourhood says so, and the larger of
        // the two scales judges it.
        let scale = tile.sigma.max(LOCAL_SHARE * local_sigma(data, w, h, x, y, clip)?);
        if excess <= HOT_SIGMA * scale {
            return None;
        }
        // Light reaches every photosite around the one it lands on; a defect
        // reaches none. A witness vouches only if it could have shown light.
        // One thing else puts a loud sample beside a hot one: the camera's own
        // long-exposure filter levels a sample down to its brightest same-colour
        // neighbour, so what survives it comes in same-colour PAIRS that repeat
        // each other to the count (3649 beside 3649, 1597 beside 1597, 5469
        // beside 5468 on two night frames; none of them at a photosite the
        // other frame shares, none with a lit neighbour of another colour —
        // particle hits, not stars). For red and blue the partner is two pixels
        // off, outside these eight; for green it is a DIAGONAL neighbour. Light
        // does not repeat itself to the count.
        let alone = [
            (x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1),
            (x - 1, y - 1), (x + 1, y - 1), (x - 1, y + 1), (x + 1, y + 1),
        ].iter().enumerate().all(|(k, &(xx, yy))| {
            if k >= 4 && data[yy * w + xx].abs_diff(data[i]) <= 1 {
                return true;
            }
            // A witness is heard at its TILE's sigma, never a larger one: every
            // doubt is read for the scene. (Heard at the sigma its own brightness
            // would earn, the tip of a trailed star lost a photosite on the star
            // frame — its neighbours, lit by the trail, passed as quiet.)
            let around = median8(data, w, xx, yy);
            let quiet = NEIGHBOUR_SIGMA * tile_at(xx, yy).sigma;
            quiet > 0.0 && f32::from(data[yy * w + xx]) - around <= quiet && levels.white - around >= quiet
        });
        alone.then_some((i, background.round() as u16))
    })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::MosaicArgs;

    fn levels() -> MosaicArgs {
        MosaicArgs { pattern: "RGGB".into(), black: [512.0; 4], white: 16383.0 }
    }

    /// Unit normal deviates from a fixed seed (twelve uniforms summed).
    fn deviates(seed: u32) -> impl FnMut() -> f32 {
        let mut state = seed;
        move || {
            let mut sum = 0.0;
            for _ in 0..12 {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                sum += (state >> 8) as f32 / (1u32 << 24) as f32;
            }
            sum - 6.0
        }
    }

    /// A sensor's law for the scene tests: variance 0.4 per count above black, plus 4.
    fn sensor_sigma(level: f32) -> f32 {
        (0.4 * (level - 512.0) + 4.0).sqrt()
    }

    /// `level` lit through a sensor of that gain (variance per count above black, plus 4), one
    /// deviate per sample.
    fn expose_at(level: &[f32], seed: u32, gain: f32) -> Vec<u16> {
        let mut next = deviates(seed);
        level.iter().map(|&l| (l + (gain * (l - 512.0) + 4.0).sqrt() * next()).round().clamp(0.0, 16383.0) as u16).collect()
    }

    fn expose(level: &[f32], seed: u32) -> Vec<u16> {
        expose_at(level, seed, 0.4)
    }

    /// The eight photosites around a sample; doubled, its same-colour ring.
    const AROUND: [(isize, isize); 8] = [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)];
    fn set(data: &mut [u16], w: usize, x: usize, y: usize, step: isize, value: u16) {
        for (dx, dy) in AROUND {
            data[(y as isize + step * dy) as usize * w + (x as isize + step * dx) as usize] = value;
        }
    }

    fn noise() -> Vec<u16> {
        let mut next = deviates(21);
        (0..256 * 256).map(|_| (2000.0 + 20.0 * next()).round() as u16).collect()
    }

    #[test]
    fn the_network_median_is_the_sorted_middle_pair_everywhere() {
        let mut data = noise();
        // Ties, a saturated run and an extreme pair, so equal keys and both ends are exercised.
        for (i, v) in [(70 * 256 + 70, 65535), (70 * 256 + 72, 65535), (72 * 256 + 70, 0), (68 * 256 + 68, 0)] {
            data[i] = v;
        }
        for y in 2..254 {
            for x in 2..254 {
                let mut eight = ring8(&data, 256, x, y);
                eight.sort_unstable();
                let expected = (f32::from(eight[3]) + f32::from(eight[4])) * 0.5;
                assert_eq!(median8(&data, 256, x, y), expected, "at ({x},{y})");
            }
        }
        // The ring itself: the eight same-colour neighbours and nothing else.
        let mut ring = ring8(&data, 256, 70, 70);
        ring.sort_unstable();
        let mut by_hand: Vec<u16> = [(68, 68), (70, 68), (72, 68), (68, 70), (72, 70), (68, 72), (70, 72), (72, 72)]
            .iter().map(|&(x, y)| data[y * 256 + x]).collect();
        by_hand.sort_unstable();
        assert_eq!(ring.to_vec(), by_hand);
    }

    #[test]
    fn the_local_spread_is_the_mad_of_the_neighbourhood_that_can_move() {
        const N: usize = 13;
        let clip = 14_000.0;
        // Every other-colour sample and the site itself carry values the measurement must not read.
        let mut data = vec![9999u16; N * N];
        let near: Vec<(usize, usize)> = (2..=10).step_by(2).flat_map(|y| (2..=10).step_by(2).map(move |x| (x, y)))
            .filter(|&p| p != (6, 6)).collect();
        assert_eq!(near.len(), 24);
        // Twelve at 1000 and twelve at 1010, 1020, … 1120: median 1005, thirteen deviations of 5 → sigma 1.4826 × 5.
        for (k, &(x, y)) in near.iter().enumerate() {
            data[y * N + x] = if k < 12 { 1000 } else { 1010 + 10 * (k as u16 - 12) };
        }
        data[6 * N + 6] = 60_000;
        let sigma = local_sigma(&data, N, N, 6, 6, clip).expect("an unclipped neighbourhood");
        assert!((sigma - 1.4826 * 5.0).abs() < 1e-4, "{sigma}");
        // Eleven of them clipped: the thirteen that can move are measured alone (1000 ×12 and 1010 → 0).
        for &(x, y) in &near[13..] { data[y * N + x] = 16_000; }
        assert_eq!(local_sigma(&data, N, N, 6, 6, clip), Some(0.0));
        // Twelve clipped is still half that can move; thirteen is not, and the ground is not judged.
        data[near[12].1 * N + near[12].0] = 16_000;
        assert_eq!(local_sigma(&data, N, N, 6, 6, clip), Some(0.0));
        data[near[11].1 * N + near[11].0] = 16_000;
        assert_eq!(local_sigma(&data, N, N, 6, 6, clip), None);
        // Three pixels from two edges the window keeps the site's parity and what the frame holds: fifteen samples.
        let mut next = deviates(4);
        let noisy: Vec<u16> = (0..N * N).map(|_| (3000.0 + 50.0 * next()).round() as u16).collect();
        let mut held: Vec<f32> = (1..=7).step_by(2).flat_map(|y| (1..=7).step_by(2).map(move |x| (x, y)))
            .filter(|&p| p != (3, 3)).map(|(x, y): (usize, usize)| f32::from(noisy[y * N + x])).collect();
        assert_eq!(held.len(), 15);
        let centre = median(&mut held);
        for v in &mut held { *v = (*v - centre).abs(); }
        let expected = 1.4826 * median(&mut held);
        assert!((local_sigma(&noisy, N, N, 3, 3, clip).unwrap() - expected).abs() < 1e-3);
    }

    #[test]
    fn strong_isolated_sites_on_all_phases_are_mapped_but_border_and_faint_are_not() {
        let mut data = noise();
        let sites = [(80, 80), (121, 80), (80, 121), (121, 121)];
        for (x, y) in sites { data[y * 256 + x] = 2880; }
        data[180 * 256 + 180] = 2330; // about 15 residual sigmas
        data[2 * 256 + 80] = 5000; // cannot inspect its edge neighbour's median
        let found = find(&data, 256, 256, &levels());
        assert_eq!(found.len(), 4);
        for (x, y) in sites {
            let value = found.iter().find(|(i, _)| *i == y * 256 + x).unwrap().1;
            assert_eq!(value, median8(&data, 256, x, y).round() as u16);
        }
    }

    /// The scene the brightness tests share: 1536 px square (144 tiles a phase), four level bands,
    /// and 32 px patches at 12,000 counts inside the darkest band's tiles — a minority of every
    /// tile they touch.
    const SCENE: usize = 1536;
    fn scene(darkest: f32) -> Vec<f32> {
        let mut level = vec![0.0f32; SCENE * SCENE];
        for y in 0..SCENE {
            for x in 0..SCENE {
                let band = [darkest, 1400.0, 2800.0, 5600.0][x * 4 / SCENE];
                let patch = x < SCENE / 4 && x % 128 >= 40 && x % 128 < 72 && y % 128 >= 40 && y % 128 < 72;
                level[y * SCENE + x] = if patch { 12_000.0 } else { band };
            }
        }
        level
    }

    #[test]
    fn a_bright_structure_in_dark_tiles_is_judged_by_its_own_brightness() {
        const N: usize = SCENE;
        let level = scene(600.0);
        let mut data = expose(&level, 77);
        // A real defect on the dark band, 30 of ITS sigmas, and the same excess inside a bright patch where it
        // is under three sigmas of the light there. The patch sample is given every other excuse — its nearest
        // ring flat to the count, eight neighbours below their own surroundings — so only the noise of the
        // light around it speaks for it.
        let excess = (30.0 * sensor_sigma(600.0)).round() as u16;
        let (dark, bright) = (300 * N + 20, (128 * 3 + 56) * N + 128 * 2 + 56);
        assert_eq!((level[dark], level[bright]), (600.0, 12_000.0));
        data[dark] = 600 + excess;
        data[bright] = 12_000 + excess;
        set(&mut data, N, 128 * 2 + 56, 128 * 3 + 56, 2, 12_000);
        set(&mut data, N, 128 * 2 + 56, 128 * 3 + 56, 1, 11_000);
        let found = find(&data, N, N, &levels());
        assert!(found.iter().any(|(i, _)| *i == dark), "the defect on the dark band is mapped");
        let in_patches = found.iter().filter(|(i, _)| level[*i] == 12_000.0).count();
        assert_eq!(in_patches, 0, "nothing inside the bright patches is a defect");
        assert_eq!(found.len(), 1, "and nothing else in the frame is");
        // What this protects against, said of the DATA: by the dark band's own sigma — what a mostly dark
        // tile's MAD reports — the patches' ordinary photon noise is full of 20-sigma events.
        let dark_sigma = 1.09 * sensor_sigma(600.0);
        let events = (0..N * N).filter(|&i| {
            let (x, y) = (i % N, i / N);
            level[i] == 12_000.0 && x % 128 >= 44 && x % 128 < 68 && y % 128 >= 44 && y % 128 < 68
                && f32::from(data[i]) - median8(&data, N, x, y) > HOT_SIGMA * dark_sigma
        }).count();
        assert!(events >= 400, "only {events} patch samples stand 20 dark sigmas clear");
    }

    #[test]
    fn a_part_blown_tile_is_measured_on_the_samples_that_can_move() {
        // Every tile: its left 60 px blown to white, the rest lit at 3000 — 47 % of each tile clipped. Counted in,
        // the clipped residuals (all zero) drag the tile's MAD to a fraction of the light's noise.
        const N: usize = 1024;
        let level: Vec<f32> = (0..N * N).map(|i| if (i % N) % 128 < 60 { 40_000.0 } else { 3000.0 }).collect();
        let data = expose(&level, 5);
        let tile = scan(&data, N, N, &levels(), 8 + 1);
        let truth = 1.081 * sensor_sigma(3000.0);
        assert!((tile.sigma / truth - 1.0).abs() < 0.1, "{} against {truth}", tile.sigma);
        assert!(find(&data, N, N, &levels()).is_empty(), "ordinary noise beside a blown area is not a field of defects");
        // The hazard, said of the DATA: the same tile's MAD over every sample it holds.
        let mut all: Vec<f32> = (128..256).step_by(2).flat_map(|y| (128..256).step_by(2).map(move |x| (x, y)))
            .map(|(x, y)| (f32::from(data[y * N + x]) - median8(&data, N, x, y)).abs()).collect();
        assert!(1.4826 * median(&mut all) < 0.35 * truth);
    }

    #[test]
    fn a_rough_neighbourhood_raises_the_bar_and_a_flat_one_does_not() {
        // One plane (phase 0) carries texture in a 32 px patch: every other sample of it lifted by 12 sigmas, in a
        // checker of its own lattice, so every neighbourhood there is half lifted. The patch is 6 % of its tile —
        // the tile's MAD does not see it — and the other planes are flat, so every witness is quiet: only the
        // site's own neighbourhood can speak.
        const N: usize = 512;
        let mut data = expose(&vec![2000.0f32; N * N], 9);
        let sigma = sensor_sigma(2000.0);
        let lift = (12.0 * sigma).round() as u16;
        for y in (84..116).step_by(2) {
            for x in (384..416).step_by(2) {
                if (x / 2 + y / 2) % 2 == 0 { data[y * N + x] += lift; }
            }
        }
        let (flat, rough) = (100 * N + 100, 100 * N + 400);
        let excess = (40.0 * sigma).round() as u16;
        data[flat] = 2000 + excess;
        data[rough] = 2000 + excess;
        let found = find(&data, N, N, &levels());
        assert_eq!(found.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [flat], "forty sigmas is a defect on flat ground only");
        let local = |x, y| local_sigma(&data, N, N, x, y, 14_000.0).unwrap();
        assert!(local(400, 100) > 5.0 * sigma && local(100, 100) < 2.0 * sigma);
        // The same excess clears twenty TILE sigmas in both places: without the neighbourhood, both are mapped.
        let clear = |x, y| f32::from(data[y * N + x]) - median8(&data, N, x, y);
        assert!(clear(400, 100) > HOT_SIGMA * 1.2 * sigma && clear(100, 100) > HOT_SIGMA * 1.2 * sigma);
    }

    #[test]
    fn flat_ground_is_judged_by_its_tile() {
        // Two hundred sites of 20.8 tile sigmas on flat, steeply noisy ground (gain 4, 18 counts above black — a
        // high-ISO night sky). A neighbourhood's spread is a noisy reading of the same noise, above it as often as
        // below: joined at its full size it would put one site in four under twenty. At a half it never speaks
        // here, and what is lost is the hundredth whose witnesses were loud by chance.
        const N: usize = 1024;
        let mut data = expose_at(&vec![530.0f32; N * N], 31, 4.0);
        let sites: Vec<(usize, usize)> = (0..200).map(|k| (40 + 47 * (k % 20), 40 + 93 * (k / 20))).collect();
        for &(x, y) in &sites {
            let tile = scan(&data, N, N, &levels(), ((y % 2) * 2 + x % 2) * 64 + (y / 128) * 8 + x / 128);
            data[y * N + x] = (median8(&data, N, x, y) + 20.8 * tile.sigma).round() as u16;
        }
        let found = find(&data, N, N, &levels());
        let kept = sites.iter().filter(|&&(x, y)| found.iter().any(|(i, _)| *i == y * N + x)).count();
        assert!(kept >= 190, "{kept} of 200");
    }

    #[test]
    fn a_site_on_clipped_ground_is_not_judged() {
        // A sample at white on own-colour ground a few hundred counts under it: blown, or nearly — nothing there
        // can show its noise, so the flat neighbourhood is no evidence of flat ground. The other planes are dark
        // and quiet, with all the headroom in the world, so every witness would vouch.
        const N: usize = 256;
        let mut data = noise();
        let (x, y) = (128usize, 128usize);
        for yy in (y - 8..=y + 8).step_by(2) {
            for xx in (x - 8..=x + 8).step_by(2) { data[yy * N + xx] = 15_900; }
        }
        set(&mut data, N, x, y, 1, 1990);
        data[y * N + x] = 16_383;
        let tile = scan(&data, N, N, &levels(), (y / 128) * 2 + x / 128);
        assert!(f32::from(data[y * N + x]) - median8(&data, N, x, y) > HOT_SIGMA * tile.sigma, "a candidate by its tile");
        assert_eq!(local_sigma(&data, N, N, x, y, 14_796.0), None);
        assert!(find(&data, N, N, &levels()).iter().all(|(i, _)| *i != y * N + x));
        // Only the nearest ring that bright, the rest of the neighbourhood dark: the neighbourhood can be measured
        // and is quiet, but the background the excess is counted from is still a clipped one.
        let mut data = noise();
        set(&mut data, N, x, y, 2, 15_900);
        set(&mut data, N, x, y, 1, 1990);
        data[y * N + x] = 16_383;
        assert!(local_sigma(&data, N, N, x, y, 14_796.0).is_some_and(|s| s < 2.0 * tile.sigma));
        assert!(find(&data, N, N, &levels()).iter().all(|(i, _)| *i != y * N + x));
    }

    #[test]
    fn a_witness_at_white_cannot_vouch_and_a_lit_diagonal_speaks() {
        const N: usize = 256;
        let base = noise();
        let site = 128 * N + 128;
        // Alone on flat ground: mapped.
        let mut data = base.clone();
        data[site] = 2880;
        assert!(find(&data, N, N, &levels()).iter().any(|(i, _)| *i == site));
        // The four edge neighbours and everything of their colours around them blown: they show no excess over
        // their own (blown) medians, and used to vouch by that silence.
        let mut data = base.clone();
        data[site] = 2880;
        for y in 120..137 {
            for x in 120..137 {
                if (x + y) % 2 == 1 { data[y * N + x] = 16383; }
            }
        }
        assert!(find(&data, N, N, &levels()).iter().all(|(i, _)| *i != site), "blown witnesses cannot vouch");
        // One diagonal neighbour lit well above its own ring: light fell here, whatever its colour.
        let mut data = base.clone();
        data[site] = 2880;
        data[127 * N + 127] = 2000 + 200;
        assert!(find(&data, N, N, &levels()).iter().all(|(i, _)| *i != site), "a lit diagonal speaks for the scene");
        // …unless it repeats the site to the count: the camera's own copy of a green defect. Both go.
        let mut data = base.clone();
        let (green, copy) = (128 * N + 129, 129 * N + 128);
        data[green] = 2880;
        data[copy] = 2880;
        let found: Vec<usize> = find(&data, N, N, &levels()).iter().map(|(i, _)| *i).collect();
        assert!(found.contains(&green) && found.contains(&copy), "a defect and its diagonal copy: {found:?}");
        data[copy] = 2879;
        assert_eq!(find(&data, N, N, &levels()).len(), 2, "the filter's rounding leaves a count between them");
        data[copy] = 2870;
        assert!(find(&data, N, N, &levels()).is_empty(), "ten counts apart is two lit samples, not a pair");
    }

    #[test]
    fn a_witness_is_heard_at_its_tile_sigma() {
        // A site far above a rough neighbourhood of its own plane (every other sample lifted 300 counts: its scale
        // is some five times the tile's) still clears twenty of that scale. Its eight neighbours sit just under
        // their surroundings — alone, it is a defect. One neighbour five TILE sigmas above its surroundings is
        // light; heard at the site's larger scale it would pass as quiet, as the lit neighbours of a trailed
        // star's tip once did.
        const N: usize = 256;
        let mut data = noise();
        let (x, y) = (128usize, 128usize);
        for yy in (y - 8..=y + 8).step_by(2) {
            for xx in (x - 8..=x + 8).step_by(2) {
                if (xx / 2 + yy / 2) % 2 == 0 { data[yy * N + xx] += 300; }
            }
        }
        set(&mut data, N, x, y, 1, 1990);
        data[y * N + x] = 8000;
        let tile = scan(&data, N, N, &levels(), (y / 128) * 2 + x / 128);
        let scale = LOCAL_SHARE * local_sigma(&data, N, N, x, y, 14_796.0).unwrap();
        let excess = f32::from(data[y * N + x]) - median8(&data, N, x, y);
        assert!(scale > 4.0 * tile.sigma && excess > HOT_SIGMA * scale, "scale {scale} against tile {}", tile.sigma);
        assert!(find(&data, N, N, &levels()).iter().any(|(i, _)| *i == y * N + x), "alone among quiet neighbours: a defect");
        data[y * N + x + 1] = (median8(&data, N, x + 1, y) + 5.0 * tile.sigma).round() as u16;
        assert!(find(&data, N, N, &levels()).iter().all(|(i, _)| *i != y * N + x), "five tile sigmas beside it is light");
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
        assert!(find(&data, 256, 256, &levels()).iter().all(|(i, _)| *i != 128 * 256 + 128),
            "a star's other-colour neighbours must protect its centre");
    }

    #[test]
    fn a_bayer_frame_is_mapped_without_being_asked_and_other_sensors_are_left_alone() {
        use rawler::rawimage::RawPhotometricInterpretation as Photo;
        let mut data = noise(); data[80 * 256 + 80] = 2880;
        // Demosaiced (linear) data has no CFA phase to take a same-colour median over.
        let mut linear = super::super::tests::bayer_fixture("RGGB", 256, 256, &[512], 16383);
        linear.data = RawImageData::Integer(data.clone());
        linear.photometric = Photo::LinearRaw;
        assert_eq!(map_for(&mut linear).unwrap(), 0);
        let RawImageData::Integer(after) = &linear.data else { panic!("integer fixture") };
        assert_eq!(after, &data);
        // The Bayer frame: one site replaced, every other sample as it was.
        let mut raw = super::super::tests::bayer_fixture("RGGB", 256, 256, &[512], 16383);
        raw.data = RawImageData::Integer(data.clone());
        assert_eq!(map_for(&mut raw).unwrap(), 1);
        let RawImageData::Integer(after) = &raw.data else { panic!("integer fixture") };
        let changed: Vec<usize> = (0..data.len()).filter(|&i| after[i] != data[i]).collect();
        assert_eq!(changed, [80 * 256 + 80]);
        assert_eq!(after[80 * 256 + 80], median8(&data, 256, 80, 80).round() as u16);
    }

    #[test]
    fn hot_mapping_is_unconditional_and_precedes_every_reader_of_the_mosaic() {
        let source = include_str!("../render.rs");
        let start = source.find("let strength = denoise.map_or").unwrap();
        let body = &source[start..source.find("fn develop_raw_buffer(").unwrap()];
        // The hook takes the frame and nothing else: no denoise option can switch it off.
        let hook = "hot_pixels::map_for(&mut rawimage)?";
        assert_eq!(body.matches("hot_pixels::map_for(").count(), 1);
        let mapping = body.find(hook).expect("hot-site mapping hook, called with the frame alone");
        assert!(mapping < body.find("denoise_grain::capture_original(").unwrap());
        assert!(mapping < body.find("crate::denoise::denoise_mosaic(").unwrap());
        assert!(mapping < body.find("develop_raw_buffer(&rawimage").unwrap());
        // …and it is not nested under the denoise request, nor under any other condition: the statement stands
        // at the function body's own indentation.
        assert!(mapping < body.find("if let Some(opts) = denoise").unwrap());
        assert!(source.lines().any(|l| l == "    let hot_sites = crate::denoise::hot_pixels::map_for(&mut rawimage)?;"));
    }
}
