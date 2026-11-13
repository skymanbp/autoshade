//! The READ of a raster mask's band on a content edge (J149, v1.18.0): the frame in blocks of the raster's own pixels, the two sides a band block lies between (a walk of the raster's slope to each plateau), the third beside them, the window a noisy frame is read through, and what each band pixel is owed between them. The law, and what is done with the answer, are `mask_owed`'s (`BandShares`).

use super::mask_owed::{between, light_fraction, linear_luminance, BandRow, Owed, MASK_BAND_REACH, MASK_BAND_STEP_FLOOR};
use super::*;

/// How many blocks a band block walks the raster's slope for each of its two
/// sides, and how many blocks across a band is no edge at all but a feather
/// its author drew: twice the reach.
const BAND_SIGHT: usize = 2 * MASK_BAND_REACH;
/// The blocks of the window a side is read in ([`Blocks::read`]).
const SIDE_WINDOW: usize = (2 * MASK_BAND_REACH + 1) * (2 * MASK_BAND_REACH + 1);
/// The widest window a band pixel's light is read in ([`Sides::light`]), this
/// many pixels each way: half the reach, so no window spans from one side's
/// plateau to the other's.
const BAND_CALM_REACH: usize = MASK_BAND_REACH / 2;
/// How many of its own spreads a side's content reaches from its level. A
/// pixel within that of a side's level is that side ([`Span::between`]). And
/// the step between two sides' levels must clear that many of the larger
/// side's spread, in encoded luma, before a pixel's own light is read for
/// its place between them, and twice that to be read whole
/// ([`Blocks::sides`]): a pure pixel is misplaced by `spread / step` of the
/// way — a quarter at four spreads, a coin toss at two, where the raster's
/// area weight is the better estimate. Encoded there, not linear: a few
/// stars in the window carry most of a night sky's linear variance and none
/// of its noise.
const BAND_STEP_SPREADS: f32 = 2.0;
/// The spread ramp is waived where the two sides' levels stand far apart in
/// LINEAR light, because there reading a pixel's own light cannot make the
/// band noisier than the frame beside it by much, whatever the mask does.
/// Placing a pixel between an inside level `I` (which the mask takes to
/// `t(I)`) and an outside level `O` (which it leaves) carries the frame's
/// noise through the slope `(t(I) − O) / (I − O)`, where the zones' own
/// slopes are `t(I) / I` and 1; for every `t` that slope is at most
/// `bright / (bright − dim)` times the larger of the zones' two. With the dim
/// level at half the bright one or under, the bound is this constant, 2, and
/// the pixel's light is read whole however noisy the frame; from there the
/// waiver ramps out to nothing at [`BAND_NOISE_GAIN_NONE`]. It is also where
/// the area weight does its harm: a bright pixel under a dim layer's ramp
/// lands over both sides only when the levels are well apart, and a mask that
/// lifts the dim side makes that side the noisier one for the mask after it.
/// (What the noise does to the band's MEAN there is [`Sides::light`]'s.)
const BAND_NOISE_GAIN_WHOLE: f32 = 2.0;
/// See [`BAND_NOISE_GAIN_WHOLE`]: a bound of four times a zone's own noise
/// gain (the dim level at three quarters of the bright one) waives nothing.
const BAND_NOISE_GAIN_NONE: f32 = 4.0;
/// A block of the raster is FLAT — on a side — while its coverage stands
/// within this of each of its eight neighbours', and not at all from
/// [`BAND_STEEP_STEP`]; the same two steps, taken on the raster's 3 x 3 mean,
/// say how far a block is on the BAND. Not "coverage near 0 or 1": the fit's
/// own zone rasters stand on plateaus either side of a silhouette (the night
/// pair's sky raster, 2026-11-13, read 9 to 14 px out on the sky's side of
/// its crossings at 4096 px: under 0.95 along 83 % of one start's silhouette
/// and 89 % of the other's, its median there 0.86 and 0.80). The first form
/// of this law took a side to be coverage at or over 0.95, or at or under
/// 0.05; it found no side along most of that silhouette and left the rim
/// there. A raster that moves a twentieth a pixel takes twenty pixels to
/// cross its range and is a feather its author drew; one that moves a tenth
/// crosses it inside a window ([`MASK_BAND_REACH`] each way) and is an edge.
const BAND_FLAT_STEP: f32 = 0.05;
/// See [`BAND_FLAT_STEP`].
const BAND_STEEP_STEP: f32 = 0.10;
/// The bytes one strip of [`layer_band`] may hold in block sums — a fixed
/// size, so the read costs the same on a 2 MP preview and a 61 MP export.
const BAND_STRIP_BYTES: usize = 64 * 1024 * 1024;

/// What a block's pixels are summed in: encoded luma and its square, linear
/// luminance, the three linear channels, and linear luminance squared.
const LEVELS: usize = 7;
type Levels = [f32; LEVELS];

/// The largest difference between `plane` at block `(row, bx)` and at any of
/// its eight neighbours (the grid's own edge repeated past it).
fn largest_step(plane: &[f32], (rows, gw): (usize, usize), row: usize, bx: usize) -> f32 {
    let at = plane[row * gw + bx];
    let mut step = 0.0f32;
    for r in row.saturating_sub(1)..=(row + 1).min(rows - 1) {
        for b in bx.saturating_sub(1)..=(bx + 1).min(gw - 1) {
            step = step.max((plane[r * gw + b] - at).abs());
        }
    }
    step
}

/// `plane`'s mean over block `(row, bx)` and its eight neighbours (the grid's
/// own edge repeated past it).
fn mean_of_nine(plane: &[f32], (rows, gw): (usize, usize), row: usize, bx: usize) -> f32 {
    let mut sum = 0.0f32;
    for dy in 0..3 {
        for dx in 0..3 {
            sum += plane[(row + dy).saturating_sub(1).min(rows - 1) * gw + (bx + dx).saturating_sub(1).min(gw - 1)];
        }
    }
    sum / 9.0
}

/// One raster layer's band on `data` (`w × h`): for each row, the band pixels
/// whose mixture can be read and the layer weight each is owed
/// (`mask_owed::BandShares`), `coverage` being the layer's own weight at a pixel. Empty
/// when there is none — or when there are more than `budget`, and the layer
/// keeps its area weight (`mask_owed::BAND_LIST_SHARE`, `BAND_LIST_FLOOR`).
///
/// Read in BLOCKS of `block` frame pixels on a side (`mask_owed::block_edge`), one
/// pixel of the layer's raster: a block's coverage is its pixels' mean, and
/// it is on the band where the raster's 3 x 3 mean steps to a neighbour by
/// more than [`BAND_FLAT_STEP`] — the mean, because a noisy raster's plateau
/// sets no mean moving. The frame is walked in strips of block rows whose
/// planes fit [`BAND_STRIP_BYTES`]; a strip reads one block row more than a
/// band block can (its walk's end, and that end's own window) past each of
/// its ends, so a pixel's answer does not depend on where the strips fall.
pub(super) fn layer_band(
    data: &[[f32; 3]],
    (w, h): (usize, usize),
    block: usize,
    budget: usize,
    coverage: &(dyn Fn(usize, usize) -> f32 + Sync),
) -> Vec<BandRow> {
    let (gw, gh) = (w.div_ceil(block), h.div_ceil(block));
    // The furthest block a band block reads: the end of a walk, and the window round it.
    let far = BAND_SIGHT + MASK_BAND_REACH;
    let halo = far + 1;
    let per_block = 3 * std::mem::size_of::<f32>() + std::mem::size_of::<Levels>() + 1;
    let strip = (BAND_STRIP_BYTES / (gw * per_block)).saturating_sub(2 * halo).max(1);
    let (dec, _) = transfer_luts();
    let span = |g: usize, edge: usize| g * block..((g + 1) * block).min(edge);
    let mut rows: Vec<BandRow> = vec![Vec::new(); h];
    let mut listed = 0usize;
    for first in (0..gh).step_by(strip) {
        let last = (first + strip).min(gh);
        // The block rows this strip reads.
        let (lo, hi) = (first.saturating_sub(halo), (last + halo).min(gh));
        let grid = (hi - lo, gw);
        let mut cover = vec![0.0f32; (hi - lo) * gw];
        cover.par_chunks_mut(gw).enumerate().for_each(|(i, row)| {
            let ys = span(lo + i, h);
            for (bx, mean) in row.iter_mut().enumerate() {
                let xs = span(bx, w);
                let sum: f32 = ys.clone().map(|y| xs.clone().map(|x| coverage(x, y)).sum::<f32>()).sum();
                *mean = sum / (ys.len() * xs.len()) as f32;
            }
        });
        let smooth: Vec<f32> = (0..cover.len()).into_par_iter().map(|i| mean_of_nine(&cover, grid, i / gw, i % gw)).collect();
        // How steeply the mean steps at each block: over `BAND_FLAT_STEP` the block is on the band.
        let slope: Vec<f32> = (0..cover.len()).into_par_iter().map(|i| largest_step(&smooth, grid, i / gw, i % gw)).collect();
        // This strip's own blocks, as the strip's planes number them.
        let own = (first - lo) * gw..(last - lo) * gw;
        if slope[own.clone()].iter().all(|s| *s <= BAND_FLAT_STEP) {
            continue;
        }
        // The blocks this strip's own band blocks read.
        let mut near = vec![false; (hi - lo) * gw];
        for i in own.filter(|i| slope[*i] > BAND_FLAT_STEP) {
            let (row, bx) = (i / gw, i % gw);
            let along = bx.saturating_sub(far)..=(bx + far).min(gw - 1);
            for r in row.saturating_sub(far)..=(row + far).min(hi - lo - 1) {
                near[r * gw + *along.start()..=r * gw + *along.end()].fill(true);
            }
        }
        let mut levels: Vec<Levels> = vec![[0.0; LEVELS]; (hi - lo) * gw];
        levels.par_chunks_mut(gw).enumerate().for_each(|(i, row)| {
            let ys = span(lo + i, h);
            for (bx, sums) in row.iter_mut().enumerate().filter(|(bx, _)| near[i * gw + *bx]) {
                for y in ys.clone() {
                    for p in &data[y * w + span(bx, w).start..y * w + span(bx, w).end] {
                        let e = luma601(p);
                        let lin = p.map(|c| decode_wide(dec, c));
                        let light = linear_luminance(&lin);
                        for (sum, term) in sums.iter_mut().zip([e, e * e, light, lin[0], lin[1], lin[2], light * light]) {
                            *sum += term;
                        }
                    }
                }
            }
        });
        let blocks = Blocks { lo, gw, gh, block, frame: (w, h), cover: &cover, smooth: &smooth, slope: &slope, levels: &levels };
        let found: Vec<Vec<BandRow>> = (first..last)
            .into_par_iter()
            .map(|gy| {
                let ys = span(gy, h);
                let mut out: Vec<BandRow> = vec![Vec::new(); ys.len()];
                for bx in 0..gw {
                    let Some(sides) = (blocks.slope[blocks.at(gy, bx)] > BAND_FLAT_STEP).then(|| blocks.sides(gy, bx)).flatten() else {
                        continue;
                    };
                    for (row, y) in out.iter_mut().zip(ys.clone()) {
                        row.extend(span(bx, w).map(|x| (x as u32, sides.owed(coverage(x, y), sides.light(data, (w, h), (x, y), dec)))));
                    }
                }
                out
            })
            .collect();
        for (y, row) in (first * block..).zip(found.into_iter().flatten()) {
            listed += row.len();
            rows[y] = row;
        }
        if listed > budget {
            return Vec::new();
        }
    }
    if listed == 0 { Vec::new() } else { rows }
}

/// One strip's blocks: rows `lo..` of the layer's `gw × gh` block grid.
struct Blocks<'a> {
    lo: usize,
    gw: usize,
    gh: usize,
    block: usize,
    frame: (usize, usize),
    /// Each block's mean coverage.
    cover: &'a [f32],
    /// Its 3 x 3 mean.
    smooth: &'a [f32],
    /// The mean's largest step from each block to a neighbour.
    slope: &'a [f32],
    /// Each block's level sums, over every pixel of it.
    levels: &'a [Levels],
}

/// What a window of blocks reads as one content: a side of a band block (the
/// blocks round the place a walk of the raster's slope ended that stand at
/// that place's own coverage), or the third side beside it.
struct Side {
    /// The blocks' weight in pixels, against one line of the window they are read in.
    seen: f64,
    /// Their coverage beside the window's middle: each block weighted by a
    /// Gaussian of one block on its distance from it, so a plateau that slopes
    /// toward the step is read where it meets it.
    beside: f32,
    /// Their encoded luma: its mean and its spread.
    luma: f64,
    luma_spread: f64,
    /// Their level in linear light: luminance, red, green, blue.
    level: [f32; 4],
    /// The spread of their linear luminance.
    light_spread: f32,
    /// Their GRAIN in linear luminance: what a pixel of theirs stands off
    /// their level by, the few that are something else left out — the median
    /// over the blocks of each one's mean square about the blocks' middle
    /// level, put on a variance's scale (the median of a mean of `n` squares
    /// is `(1 − 2/(9n))³` of it). The spread above takes a lamp, a handful of
    /// stars or a corner of the other side for the side's own content; this
    /// does not (modelled, 2026-11-13: lamps on a noisy sky took its spread
    /// from 0.0089 to 0.0217 and left its grain at 0.0089).
    grain: f32,
}

/// A stretch of light between two contents, the DIMMER first: where the
/// first one's own content ends and the second's begins, in linear luminance
/// (each level moved toward the other by [`BAND_STEP_SPREADS`] of what is its
/// own, [`Span::between`]), the layer weight each content is owed, and the
/// two levels — luminance, red, green, blue.
#[derive(Clone, Copy)]
struct Span {
    edge: [f32; 2],
    cover: [f32; 2],
    level: [[f32; 4]; 2],
}

/// A band block's two sides and the third content beside them, in order of
/// light: how far the third is heeded, which of the three it is (0 the
/// dimmest), the middle one's level in linear luminance, and the two
/// stretches — up to the middle one, and from it.
struct Chain {
    heed: f32,
    third: usize,
    middle: f32,
    spans: [Span; 2],
}

/// What a band block's surroundings say of the contents its pixels may be.
struct Sides {
    trust: f32,
    /// How many pixels each way the window reaches that a pixel of the block
    /// is read in ([`Sides::light`]); none where its own light tells.
    calm: usize,
    /// The block's two sides.
    pair: Span,
    /// The chain of three, where a third content is heeded.
    chain: Option<Chain>,
}

/// How much of two sides' grain is the FRAME's noise, the dimmer side given
/// first. A sensor's noise grows with the light and no faster than its root,
/// so of two sides the dimmer is no noisier than the brighter, and the
/// brighter at most `√(bright / dim)` times as noisy as the dimmer. Grain past
/// that is the side's own content — a lamp's glare, a rock's texture — and no
/// reason to read its neighbours through a window ([`Sides::light`]): on a
/// frame with no noise a lamp on the ground would otherwise count as a very
/// noisy side, and the ground beside it be read as lamp.
fn frame_noise(dim: &Side, bright: &Side) -> [f32; 2] {
    let root = if dim.level[0] > 0.0 { (bright.level[0] / dim.level[0]).sqrt() } else { f32::INFINITY };
    [dim.grain.min(bright.grain), bright.grain.min(dim.grain * root)]
}

impl Span {
    /// From `from` to `to`, the dimmer of the two given first, for a pixel
    /// read in a window `calm` pixels each way ([`Sides::light`]). Read by
    /// its own light (`calm` 0), a side's own content is its spread, and the
    /// two leave half the way between the levels to the mixture. Read by its
    /// window's median, it is the side's noise ([`frame_noise`]) — the median has
    /// dropped the rest — and the two may take the whole way: where they
    /// meet, a pixel is one side or the other, which is all that noise lets
    /// it be told.
    fn between(from: &Side, to: &Side, calm: usize) -> Self {
        let way = to.level[0] - from.level[0];
        let (own, room) = if calm == 0 { ([from.light_spread, to.light_spread], 0.5) } else { (frame_noise(from, to), 1.0) };
        let reach = BAND_STEP_SPREADS * (own[0] + own[1]);
        let spreads = if reach <= room * way { BAND_STEP_SPREADS } else { BAND_STEP_SPREADS * room * way / reach };
        Self {
            edge: [from.level[0] + spreads * own[0], to.level[0] - spreads * own[1]],
            cover: [from.beside, to.beside],
            level: [from.level, to.level],
        }
    }

    /// The share of a pixel's AREA that is the second content's, the pixel
    /// standing at linear luminance `light`: none of it at the first
    /// content's own edge and below, all of it from the second's, and between
    /// them where its light stands between the two.
    fn area(&self, light: f32) -> f32 {
        let [from, to] = self.edge;
        if to > from { ((light - from) / (to - from)).clamp(0.0, 1.0) } else { f32::from(light > from) }
    }

    /// The share of the pixel's LIGHT that is the second content's, `area` of
    /// the pixel being that content — luminance, red, green, blue, each by
    /// its own two levels ([`light_fraction`]).
    fn shares(&self, area: f32) -> [f32; 4] {
        std::array::from_fn(|lane| light_fraction(area, self.level[1][lane], self.level[0][lane]))
    }
}

impl Blocks<'_> {
    /// Where block `(gy, bx)` is in the strip's planes.
    fn at(&self, gy: usize, bx: usize) -> usize {
        (gy - self.lo) * self.gw + bx
    }

    /// Where the walk from band block `from` ends: up the raster's 3 x 3 mean
    /// (`sign` 1) or down it (−1), each step the steepest one — to the
    /// neighbour standing furthest beyond for the length of the step, a
    /// diagonal one being root two long — until the block it stands on is not
    /// band, or no neighbour stands beyond it (a crest, a floor), or it has
    /// gone [`BAND_SIGHT`] blocks. A silhouette's band block ends on the
    /// plateau either side; the flank of a thin spire walks down the spire's
    /// own furrow to the butte it rises from, a notch's up its ridge to open
    /// sky. (By the bare difference a walk across a straight edge took the
    /// diagonal every time — three neighbours stand equally beyond — and four
    /// rows below a corner it ended on the far side of the corner.)
    fn walk(&self, from: (usize, usize), sign: f32) -> (usize, usize) {
        let (mut y, mut x) = from;
        for _ in 0..BAND_SIGHT {
            let here = self.at(y, x);
            if self.slope[here] <= BAND_FLAT_STEP {
                break;
            }
            let (mut steepest, mut to) = (0.0f32, (y, x));
            for ny in y.saturating_sub(1)..=(y + 1).min(self.gh - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(self.gw - 1) {
                    let length = if ny != y && nx != x { std::f32::consts::SQRT_2 } else { 1.0 };
                    let rise = sign * (self.smooth[self.at(ny, nx)] - self.smooth[here]) / length;
                    if rise > steepest {
                        (steepest, to) = (rise, (ny, nx));
                    }
                }
            }
            if to == (y, x) {
                break;
            }
            (y, x) = to;
        }
        (y, x)
    }

    /// The blocks within [`MASK_BAND_REACH`] of `place` read as one content,
    /// each for the `share` of it that is that content's (its place in the
    /// strip's planes given). `None` where no block has a share.
    fn read(&self, (ey, ex): (usize, usize), share: impl Fn(usize) -> f32) -> Option<Side> {
        let reach = MASK_BAND_REACH;
        let count = |g: usize, edge: usize| (((g + 1) * self.block).min(edge) - g * self.block) as f64;
        let across = |g: usize, blocks: usize| g.saturating_sub(reach)..=(g + reach).min(blocks - 1);
        let (down, along) = (across(ey, self.gh), across(ex, self.gw));
        // One line of the window as the frame's edge leaves it: its shorter way across.
        let line = down.clone().count().min(along.clone().count());
        let (mut weight, mut beside, mut nearness) = (0.0f64, 0.0f64, 0.0f64);
        let mut sums = [0.0f64; LEVELS];
        // The blocks read, and each one's pixels.
        let mut held = [(0usize, 0.0f64); SIDE_WINDOW];
        let mut blocks = 0;
        for wy in down {
            for wx in along.clone() {
                let at = self.at(wy, wx);
                let share = f64::from(share(at));
                if share > 0.0 {
                    held[blocks] = (at, count(wy, self.frame.1) * count(wx, self.frame.0));
                    blocks += 1;
                    let pixels = share * held[blocks - 1].1;
                    let distance = (wy.abs_diff(ey).pow(2) + wx.abs_diff(ex).pow(2)) as f64;
                    let near = pixels * (-distance / 2.0).exp();
                    weight += pixels;
                    beside += near * f64::from(self.cover[at]);
                    nearness += near;
                    for (sum, term) in sums.iter_mut().zip(&self.levels[at]) {
                        *sum += share * f64::from(*term);
                    }
                }
            }
        }
        let mean = |sum: f64| sum / weight;
        let spread = |square: f64, of: f64| (mean(square) - mean(of).powi(2)).max(0.0).sqrt();
        // The blocks' middle level, each block's mean square about it, and the middle one of those.
        let middle = |of: &mut [f64]| if of.is_empty() { 0.0 } else { *of.select_nth_unstable_by(of.len() / 2, f64::total_cmp).1 };
        let mut each = [0.0f64; SIDE_WINDOW];
        for (mean, (at, pixels)) in each.iter_mut().zip(&held[..blocks]) {
            *mean = f64::from(self.levels[*at][2]) / pixels;
        }
        let level = middle(&mut each[..blocks]);
        for (square, (at, pixels)) in each.iter_mut().zip(&held[..blocks]) {
            let sums = &self.levels[*at];
            *square = ((f64::from(sums[6]) - 2.0 * level * f64::from(sums[2])) / pixels + level * level).max(0.0);
        }
        let grain = middle(&mut each[..blocks]) / (1.0 - 2.0 / (9.0 * (self.block * self.block) as f64)).powi(3);
        (weight > 0.0).then(|| Side {
            seen: weight / (line * self.block * self.block) as f64,
            beside: (beside / nearness) as f32,
            luma: mean(sums[0]),
            luma_spread: spread(sums[1], sums[0]),
            level: std::array::from_fn(|lane| mean(sums[2 + lane]) as f32),
            light_spread: spread(sums[6], sums[2]) as f32,
            grain: grain.sqrt() as f32,
        })
    }

    /// The side at `place`, where a walk ended: the blocks round it, each as
    /// far as its coverage is the place's own (whole within
    /// [`BAND_FLAT_STEP`] of it, not at all from [`BAND_STEEP_STEP`]) — the
    /// plateau the place is on and nothing of the step beside it or of a
    /// level beyond.
    fn side(&self, place: (usize, usize)) -> Option<Side> {
        let stands = self.cover[self.at(place.0, place.1)];
        self.read(place, |at| 1.0 - between((self.cover[at] - stands).abs(), BAND_FLAT_STEP, BAND_STEEP_STEP))
    }

    /// The two sides band block `(gy, bx)` lies between, the third beside it
    /// if there is one, and how far their reading of the block is trusted
    /// (`mask_owed::BandShares`). `None` where the block's pixels keep their coverage:
    /// both walks ending at one coverage, or no trust.
    fn sides(&self, gy: usize, bx: usize) -> Option<Sides> {
        let slope = self.slope[self.at(gy, bx)];
        let (low, high) = (self.side(self.walk((gy, bx), -1.0))?, self.side(self.walk((gy, bx), 1.0))?);
        if high.beside <= low.beside {
            return None;
        }
        let steep = f64::from(between(slope, BAND_FLAT_STEP, BAND_STEEP_STEP));
        // In sight: the scarcer side's pixels against one line's worth.
        let seen = low.seen.min(high.seen).min(1.0);
        // The blocks this band would take, at its slope here, to cross from one side's coverage to the other's.
        let across = (high.beside - low.beside) / slope;
        let close = f64::from(1.0 - between(across, MASK_BAND_REACH as f32, BAND_SIGHT as f32));
        let step = (high.luma - low.luma).abs();
        let floor = f64::from(MASK_BAND_STEP_FLOOR);
        let told = ((step - floor) / floor).clamp(0.0, 1.0);
        let noise = f64::from(BAND_STEP_SPREADS) * low.luma_spread.max(high.luma_spread);
        let clear = if noise > 0.0 { ((step - noise) / noise).clamp(0.0, 1.0) } else { 1.0 };
        // The two as contents: the dimmer, the brighter.
        let (dim, bright) = if low.level[0] <= high.level[0] { (&low, &high) } else { (&high, &low) };
        // `1 / bound` of the noise gain: (bright − dim) / bright.
        let separation = if bright.level[0] > 0.0 { 1.0 - dim.level[0].max(0.0) / bright.level[0] } else { 0.0 };
        let apart = between(separation, 1.0 / BAND_NOISE_GAIN_NONE, 1.0 / BAND_NOISE_GAIN_WHOLE);
        let trust = (steep * seen * close * told * clear.max(f64::from(apart))) as f32;
        if !(trust > 0.0 && bright.level[0] - dim.level[0] > f32::EPSILON) {
            return None;
        }
        // How many half-ways between the levels the two sides' noise takes, pixel by pixel.
        let need = BAND_STEP_SPREADS * frame_noise(dim, bright).iter().sum::<f32>() / (0.5 * (bright.level[0] - dim.level[0]));
        let calm = if need > 1.0 { ((need - 0.5).ceil() as usize).min(BAND_CALM_REACH) } else { 0 };
        Some(Sides { trust, calm, pair: Span::between(dim, bright, calm), chain: self.third((gy, bx), dim, bright, calm) })
    }

    /// The THIRD side beside band block `at`, where three levels meet: the
    /// blocks within reach of the block itself where the raster is flat and
    /// stands at neither of the two sides' coverages — a staircase's middle
    /// step, which neither walk ends on because the raster's slope leads past
    /// it both ways. It is heeded as far as it is in sight (its pixels against
    /// one line's worth), its level can be told from each of the two sides'
    /// (the ruler's ramp the sides themselves are held to: not at all within
    /// one code, whole at two) and stands outside that side's own content —
    /// not at all at the side's own edge, whole at twice it. A pixel is then
    /// read along the chain of the three levels in order of light, between
    /// the two it stands between. (A plateau that slopes is flat blocks at
    /// another coverage too, and of the side's own content: the two ramps are
    /// what keep it from being a third.)
    fn third(&self, at: (usize, usize), dim: &Side, bright: &Side, calm: usize) -> Option<Chain> {
        let neither = |c: f32, side: &Side| between((c - side.beside).abs(), BAND_FLAT_STEP, BAND_STEEP_STEP);
        let third = self.read(at, |at| {
            if self.slope[at] <= BAND_FLAT_STEP { neither(self.cover[at], dim) * neither(self.cover[at], bright) } else { 0.0 }
        })?;
        let floor = f64::from(MASK_BAND_STEP_FLOOR);
        let mut heed = third.seen.min(1.0);
        for side in [dim, bright] {
            let told = (((third.luma - side.luma).abs() - floor) / floor).clamp(0.0, 1.0);
            let (gap, own) = ((third.level[0] - side.level[0]).abs(), BAND_STEP_SPREADS * side.light_spread);
            heed *= told * if own > 0.0 { f64::from(((gap - own) / own).clamp(0.0, 1.0)) } else { 1.0 };
        }
        let heed = heed as f32;
        (heed > 0.0).then(|| {
            // Where the third stands among the three: before the dimmer side, between the two, or after the brighter.
            let place = usize::from(third.level[0] >= dim.level[0]) + usize::from(third.level[0] > bright.level[0]);
            let mut by_light = [dim, bright, bright];
            by_light.copy_within(place..2, place + 1);
            by_light[place] = &third;
            Chain {
                heed,
                third: place,
                middle: by_light[1].level[0],
                spans: [Span::between(by_light[0], by_light[1], calm), Span::between(by_light[1], by_light[2], calm)],
            }
        })
    }
}

impl Sides {
    /// The light pixel `(x, y)` of the band block is read by, in linear
    /// luminance, on `data` (`w × h`). Its OWN, where the two sides' noise —
    /// [`BAND_STEP_SPREADS`] of each one's ([`frame_noise`]) — leaves half the
    /// way between their levels to the mixture: there a pixel's light
    /// tells a mixture from either side. On a noisy frame it does not. A
    /// pixel of one side is as bright as a mixture often enough; read by its
    /// own light, the dim side's brighter pixels lose the dim side's
    /// treatment and the bright side's dimmer ones gain it, all across the
    /// band, and the band stands off both plateaus. But a pixel is a mixture
    /// only where the two contents MEET, and its surround says whether they
    /// do: there the pixel is read by the MEDIAN of its window, a square `r`
    /// pixels each way. A median keeps an edge where it is — along a
    /// straight one it is the light of the pixels at the middle one's own
    /// distance from it — and drops what a few pixels hold that the rest do
    /// not, the frame's noise and a star alike. `r` is the least whose window
    /// brings the noise inside that half-way, a window's median taken to
    /// spread half its width less than a pixel does (what is left of the
    /// square-root law by a grain neighbours share, as a demosaic's is):
    /// `⌈need − ½⌉`, [`BAND_CALM_REACH`] at most. A feature thinner than
    /// `r + 1` pixels is read as its surround — at that noise it is not told
    /// from it, and a pixel the median leaves between the sides' own noise
    /// is a mixture ([`Span::between`]). Modelled before it was written
    /// (2026-11-13, numpy, a slanted edge at the night pair's RAW-start
    /// levels, 44 codes against 7 with noise of 8.5, the dim side lifted
    /// 2.5 EV): read by its own light the dim side stood 3.7 to 3.9 codes
    /// under its plateau from 1.5 to 12 px off the edge and the bright side
    /// 0.5 over; read this way, `r` 2, within 0.3 and 0.1.
    fn light(&self, data: &[[f32; 3]], (w, h): (usize, usize), (x, y): (usize, usize), dec: &[f32]) -> f32 {
        let of = |x: usize, y: usize| linear_luminance(&data[y * w + x].map(|c| decode_wide(dec, c)));
        let r = self.calm;
        if r == 0 {
            return of(x, y);
        }
        // The frame's own edge repeated past it.
        let line = |at: usize, edge: usize| (0..=2 * r).map(move |d| (at + d).saturating_sub(r).min(edge - 1));
        let mut window = [0.0f32; (2 * BAND_CALM_REACH + 1) * (2 * BAND_CALM_REACH + 1)];
        let mut held = 0;
        for wy in line(y, h) {
            for wx in line(x, w) {
                window[held] = of(wx, wy);
                held += 1;
            }
        }
        *window[..held].select_nth_unstable_by(held / 2, f32::total_cmp).1
    }

    /// What a pixel of the band block is owed, the layer's `coverage` of it
    /// and the `light` it is read by given ([`Sides::light`], [`Owed`],
    /// `mask_owed::BandShares`): the two contents it lies between — the block's two
    /// sides, or, as far as a third is heeded, the two of the chain its
    /// light stands between — each with the weight it is owed as far as the
    /// block's reading is trusted, its coverage for the rest.
    fn owed(&self, coverage: f32, light: f32) -> Owed {
        let (mut cover, mut area) = (self.pair.cover, self.pair.area(light));
        let mut share = self.pair.shares(area);
        if let Some(chain) = &self.chain {
            let stretch = usize::from(light > chain.middle);
            let span = &chain.spans[stretch];
            let on = span.area(light);
            let along = span.shares(on);
            // The two sides' own reading, told in this stretch's two contents. With the third between the
            // sides it is the reading above (one content is common, the other is the far side for the
            // third). With the third beyond a side, the stretch out to it holds, by the two sides alone,
            // nothing but that side; the other stretch is the two sides themselves.
            if chain.third != 1 && stretch == chain.third / 2 {
                (cover, area, share) = ([self.pair.cover[stretch]; 2], on, along);
            }
            for (cover, along) in cover.iter_mut().zip(span.cover) {
                *cover += chain.heed * (along - *cover);
            }
            area += chain.heed * (on - area);
            for (share, along) in share.iter_mut().zip(along) {
                *share += chain.heed * (along - *share);
            }
        }
        Owed { cover: cover.map(|c| coverage + self.trust * (c - coverage)), area, share }
    }
}
