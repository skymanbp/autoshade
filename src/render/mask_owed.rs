//! A raster mask's band on a content edge: the weight each pixel there is OWED — each of its two contents' coverage, for the share of its light that is that content's — the layer fold that carries it and the list that holds it. The read that finds it is `mask_band`.

use super::mask_band::layer_band;
use super::mask_weight::mask_weight_in;
use super::masks::Cover;
use super::*;
use crate::recipe::{LocalAdjustment, MaskCombine};

/// How far a side is read round the place a band block's walk ends
/// (`mask_band::Blocks::walk`): a square window this many blocks each way —
/// the reverse fit's boundary ruler's own reach (`fit_zoned::boundary` takes
/// it from here) — and how many blocks a band may take to cross from one
/// side's coverage to the other's and still be trusted whole. Counted in
/// pixels of the layer's raster where the raster is coarser than the frame
/// ([`block_edge`]) and never in fewer frame pixels: past the step a keyed
/// silhouette leaves (2-3 raster pixels), and well inside one of the fit's
/// evidence cells, so a side is this edge's own.
pub(crate) const MASK_BAND_REACH: usize = 6;
/// One 8-bit code. Two sides whose levels are closer than this cannot be told
/// apart and the band keeps its area weight; at two codes the mixture is
/// trusted whole — the ruler's ramp (`fit_zoned` pins its own floor to this
/// one).
pub(crate) const MASK_BAND_STEP_FLOOR: f32 = 1.0 / 255.0;
/// One adjustment's owed weights may list at most one pixel in this many of
/// the frame ([`BAND_LIST_FLOOR`] aside); a layer that would take the list
/// past it keeps its area weight everywhere. An edge's band is a line a few
/// blocks wide; a raster that is band over an eighth of a large frame is a
/// texture, and what it would cost is the point: 32 bytes an entry, held in
/// each of two lists while they are merged — 8 bytes a frame pixel at the
/// cap — and the colour field's own band, 8 bytes an entry and so 1 a frame
/// pixel, beside the frame through the whole mask pass. Both are rows of
/// `decode::PIPELINE_BYTES_PER_PIXEL`'s accounting.
pub(super) const BAND_LIST_SHARE: usize = 8;
/// The list may always hold this many pixels, whatever the frame's size. A
/// band is a few blocks wide however small the frame, so on a thumbnail it is
/// a large share of the pixels, and a share alone would cut it off there: the
/// thumbnail would wear the rim its full-size frame does not. 2^20 entries
/// are 32 MiB — a fixed size, like the strip the read is made in
/// (`mask_band`'s `BAND_STRIP_BYTES`); the share takes over above 2^23
/// pixels, where the frame's size is what memory is counted in.
pub(super) const BAND_LIST_FLOOR: usize = 1 << 20;

/// The share of a pixel's light the content inside a layer supplies, the pixel
/// being `area` of that content at level `inside` and the rest at `outside`
/// (linear light): `a·I / (a·I + (1 − a)·O)`. Equal levels give `area` back,
/// and so does a pixel with no light at all.
pub(crate) fn light_fraction(area: f32, inside: f32, outside: f32) -> f32 {
    let (i, o) = (area * inside.max(0.0), (1.0 - area) * outside.max(0.0));
    if i + o > f32::EPSILON { i / (i + o) } else { area }
}

pub(super) fn linear_luminance(lin: &[f32; 3]) -> f32 {
    0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]
}

/// Where `v` stands from `none` (0) to `whole` (1), held to that range.
pub(super) fn between(v: f32, none: f32, whole: f32) -> f32 {
    ((v - none) / (whole - none)).clamp(0.0, 1.0)
}

/// One adjustment's layers as a point samples them: the base geometry and each
/// component, with the raster each reads and the frame they are read in.
#[derive(Clone, Copy)]
pub(super) struct Layers<'a> {
    pub(super) m: &'a LocalAdjustment,
    pub(super) base: Option<Bmp<'a>>,
    pub(super) comps: &'a [Option<Bmp<'a>>],
    pub(super) unwarp: Option<&'a MaskUnwarp>,
    pub(super) dims: (f32, f32),
}

impl Layers<'_> {
    /// A corrected frame's pixel asks about the original-frame point it shows
    /// (`MaskFrame::Corrected`); every geometry, raster and brush included, is
    /// stored in that frame.
    fn stored(&self, nx: f32, ny: f32) -> (f32, f32) {
        self.unwarp.map_or((nx, ny), |u| u.source_point(nx, ny))
    }

    /// Layer `layer`'s own coverage at normalised `(nx, ny)` — 0 the base, `k`
    /// component `k − 1` — its geometry's own inversion applied, nothing
    /// composed.
    pub(super) fn one(&self, layer: usize, nx: f32, ny: f32) -> f32 {
        let (nx, ny) = self.stored(nx, ny);
        match layer.checked_sub(1) {
            None => mask_weight_in(&self.m.mask, nx, ny, self.base, self.unwarp, self.dims),
            Some(k) => mask_weight_in(&self.m.components[k].geometry, nx, ny, self.comps[k], self.unwarp, self.dims),
        }
    }

    /// The adjustment's COMBINED coverage at normalised `(nx, ny)`: the base
    /// geometry's weight folded with each component in list order (Lightroom's
    /// Add / Subtract / Intersect grammar — the algebra is documented on
    /// [`MaskCombine`]), each layer's own weight first passed through `lanes`
    /// (the layer's number as [`Layers::one`] counts it, its weight) into `N`
    /// values folded side by side. One lane handed its weight back is the
    /// plain coverage ([`combined_mask_weight`]); four lanes carry a band
    /// pixel's luminance and channel shares through the same composition
    /// ([`BandShares`]) — every mode is linear in each layer, so a share composes as
    /// the coverage it stands for does.
    ///
    /// `unwarp` is the frame adaptation ([`MaskFrame`]) — applied PER LAYER,
    /// through [`mask_weight_in`], because a correction can hold a
    /// post-correction radial and a pre-correction brush side by side and each
    /// must be asked about in its own frame.
    #[inline]
    pub(super) fn fold<const N: usize>(&self, nx: f32, ny: f32, lanes: impl Fn(usize, f32) -> [f32; N]) -> [f32; N] {
        let (nx, ny) = self.stored(nx, ny);
        let mut w = lanes(0, mask_weight_in(&self.m.mask, nx, ny, self.base, self.unwarp, self.dims));
        for (k, (c, bmp)) in self.m.components.iter().zip(self.comps).enumerate() {
            // A raster-backed component with NO raster carries no coverage, and
            // its inversion must not turn that into the whole frame (an inverted
            // Add would cover everything at full strength, an inverted Subtract
            // would wipe the base). The develop already skips the whole
            // adjustment for a dead raster (the inert contract, recipe.rs
            // `MaskGeometry::Bitmap`); this is the same rule for every caller
            // that reaches the combination directly, and until 2026-09-24 the
            // component's own `inverted` was applied to the missing raster.
            if bmp.is_none() && is_raster_backed(&c.geometry) {
                continue;
            }
            let cw = lanes(k + 1, mask_weight_in(&c.geometry, nx, ny, *bmp, self.unwarp, self.dims));
            for (w, cw) in w.iter_mut().zip(cw) {
                let cw = if c.inverted { 1.0 - cw } else { cw };
                *w = match c.mode {
                    MaskCombine::Add => 1.0 - (1.0 - *w) * (1.0 - cw),
                    MaskCombine::Subtract => *w * (1.0 - cw),
                    MaskCombine::Intersect => *w * cw,
                };
            }
        }
        w
    }
}

/// One pixel's weight in a per-pixel mask pass. Off a band it is one number.
/// On one the pixel is a mixture of two contents ([`BandShares`]): it has the
/// weight each of them is owed, and the share of its light that is the second
/// one's — luminance, red, green, blue.
#[derive(Clone, Copy)]
pub(super) struct PixelWeight {
    /// The weight of the pixel's luminance — what a pass with one strength
    /// for the whole pixel (the Color tint) acts by.
    pub(super) luma: f32,
    cover: [f32; 2],
    share: [f32; 4],
}

impl PixelWeight {
    fn flat(w: f32) -> Self {
        Self { luma: w, cover: [w; 2], share: [0.0; 4] }
    }

    fn mixed(cover: [f32; 2], share: [f32; 4]) -> Self {
        Self { luma: cover[0] + (cover[1] - cover[0]) * share[0], cover, share }
    }

    /// The larger of its weights: at or under a pass's floor the pixel is
    /// left alone.
    pub(super) fn most(&self) -> f32 {
        self.cover[0].max(self.cover[1])
    }

    /// Every weight times `k` (a Range Mask's intersection).
    pub(super) fn scaled(self, k: f32) -> Self {
        Self { luma: self.luma * k, cover: self.cover.map(|c| c * k), share: self.share }
    }

    /// `p` blended toward its fully adjusted `t`. Off a band, by the one
    /// weight. On one, the pixel is blended as each of its two contents is —
    /// by that content's weight — and the two results are mixed IN LIGHT,
    /// each channel in the share of the pixel's light that is that content's
    /// ([`BandShares`], "In light"). A pixel that is all one content is that
    /// content's blend to the bit.
    pub(super) fn blend(&self, p: &[f32; 3], t: &[f32; 3]) -> [f32; 3] {
        let toward = |w: f32, c: usize| p[c] * (1.0 - w) + t[c] * w;
        let [first, second] = self.cover;
        if first == second {
            return std::array::from_fn(|c| toward(first, c));
        }
        let (dec, enc) = transfer_luts();
        std::array::from_fn(|c| match self.share[c + 1] {
            share if share <= 0.0 => toward(first, c),
            share if share >= 1.0 => toward(second, c),
            share => {
                let (as_first, as_second) = (decode_wide(dec, toward(first, c)), decode_wide(dec, toward(second, c)));
                encode_wide(enc, as_first + share * (as_second - as_first))
            }
        })
    }
}

/// What a band pixel is owed ([`BandShares`]): the layer weight of each of
/// the two contents it is a mixture of — the dimmer first — the share of its
/// area that is the brighter one's, and the share of its light that is:
/// luminance, red, green, blue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Owed {
    pub(super) cover: [f32; 2],
    pub(super) area: f32,
    pub(super) share: [f32; 4],
}

impl Owed {
    /// The pixel's coverage as a reader that weighs by AREA takes it (the
    /// colour field's partition): each content's weight in the share of the
    /// pixel that is that content.
    pub(super) fn by_area(&self) -> f32 {
        self.cover[0] + (self.cover[1] - self.cover[0]) * self.area
    }

    /// The one weight a lane has where the two contents are told as one:
    /// each content's weight in the share of the pixel's light that is that
    /// content's — luminance, red, green, blue.
    #[cfg(test)]
    pub(super) fn weights(&self) -> [f32; 4] {
        self.share.map(|share| self.cover[0] + (self.cover[1] - self.cover[0]) * share)
    }
}

/// One row's band pixels: each one's column and what it is owed, ascending by
/// column.
pub(super) type BandRow = Vec<(u32, Owed)>;

/// J149 (v1.18.0). What one adjustment's raster layers OWE the pixels on
/// their bands, read once on the frame as the adjustment finds it.
///
/// **The defect.** A mask pass blends a pixel toward its adjusted self by the
/// mask's coverage, `p·(1 − w) + t(p)·w`, and across a raster's step `w` is
/// the AREA of the pixel the raster covers. Along a silhouette the raster's
/// step is wider than the content's edge, so PURE pixels of either side wear
/// a fractional weight: a sky pixel under 30 % of a land layer takes 30 % of
/// the land's lift and lands brighter than both sides — a white rim, seen by
/// the owner on the night pair (2026-11-12). The coarser the raster against
/// the frame, the wider the band of pure pixels and the wider the rim. On
/// that pair, both starts rendered at 4096 px and read with no threshold on
/// the raster (groups of 32 crossings, the profile's peak within 7 px against
/// the two sides 9 to 14 px out), a rim of more than six codes stood on 72 %
/// and 71 % of the silhouette under the area weight, on 13 % and 29 % under
/// this law, and on 12 % and 14 % of the frames with no mask at all (stars,
/// and lamps on the horizon). The released v1.17.1 drew the same rim from
/// the same recipe (70 % of the master start's silhouette); its own fit had
/// lifted the land 0.9 EV where this one lifts it 2.5.
///
/// **The law** is the one the reverse fit's boundary ruler already held the
/// render to (R21 (2), `fit_zoned::boundary`'s `ContentShare`): each
/// content's light through its own treatment. A raster has SIDES, where it
/// is flat, and a BAND, where it steps between them. A band block walks the
/// raster's slope up and down to the two plateaus it lies between
/// (`mask_band::Blocks::walk`); each is a side, with its own coverage and its
/// own level in linear light. A band pixel's light places it between the two
/// levels — none of the brighter side at the dimmer side's own edge, all of
/// it from the brighter's — and it is owed each side's coverage for the
/// share of it that is that side's content ([`Owed`]). The frame says where
/// the edge is; the raster only says it is near. A pixel of one side's
/// content gets what that side's flat neighbours get, however much of the
/// raster's step lies over it — and no more than they get: a sky standing at
/// 0.9 beside a silhouette stays at 0.9 right up to it.
///
/// **In light.** A mixed pixel is blended as each of its two contents is — by
/// that content's weight — and the two results are mixed in LINEAR light,
/// each channel in the share of the pixel's light that is that content's
/// ([`PixelWeight::blend`], [`light_fraction`]). One weight laid on the
/// encoded values under-delivers the dimmer content's treatment.
///
/// **Three contents.** Where a third level stands beside the block — a
/// staircase's middle step, which neither walk ends on — the pixel is read
/// along the chain of the three in order of light, between the two it stands
/// between (`mask_band::Blocks::third`).
///
/// **A noisy frame.** Where the frame's noise leaves a mixture no room
/// between the two levels, a pixel's own light cannot say what it is, and it
/// is read by the median of its window (`mask_band::Sides::light`).
///
/// **How far it is trusted** — a product of ramps: the block's own steepness;
/// both sides in sight; the band crossing from one side's coverage to the
/// other's within the walk's reach (a feather wider than that is its
/// author's); and the two levels told apart ([`MASK_BAND_STEP_FLOOR`]) and
/// clear of the sides' own spread, or far apart in light
/// (`mask_band::Blocks::sides`). `weight = coverage + trust·(owed −
/// coverage)`. Every other pixel keeps its coverage, to the bit: one on a
/// side, one over content of a single level, one inside a drawn feather.
///
/// **Per layer, before the fold.** The band is a raster's — a bitmap, a
/// recomputed AI alpha, a brush's stamped dabs. A zone the fit split in bands
/// is its raster INTERSECTED with a ramp, so its combined coverage along the
/// silhouette is the ramp's value times the raster's; read on the raster
/// alone the band is found whatever multiplies it afterwards
/// ([`Layers::fold`]). Parametric layers have no band: a gradient's feather
/// is its author's.
///
/// **Which passes.** The two per-pixel ones — dehaze and the fused
/// tone / colour pass with its Color tint — where a level is moved and a
/// misplaced weight is a rim. The spatial passes after them (clarity,
/// texture, sharpness, noise) keep the coverage: they weigh a neighbourhood's
/// detail, and the list is dropped before their planes are allocated, so the
/// develop's peak is what it was. And the colour field's sky / land
/// partition, which weighs by area (`masks::field_sky_band`,
/// [`Owed::by_area`]): read by the raster's coverage it ramped the land's
/// correction into the sky's last rows above every silhouette.
///
/// **Lightroom** renders an exported mask by its own law; what a sidecar
/// carries is the mask and its sliders, as before.
pub(super) struct BandShares {
    rows: Vec<BandRow>,
}

impl BandShares {
    /// The owed weights of `cover`'s adjustment on `data`, the frame of
    /// `cover`'s own size as the adjustment finds it. Empty — every pixel
    /// keeps its coverage — for an adjustment with no raster layer, or none
    /// with a band on a content edge.
    pub(super) fn read(cover: &Cover<'_>, data: &[[f32; 3]]) -> Self {
        let (w, h) = (cover.w, cover.h);
        let none = Self { rows: Vec::new() };
        if w == 0 || h == 0 || data.len() < w * h {
            return none;
        }
        let layers = cover.layers();
        let mut budget = (w * h / BAND_LIST_SHARE).max(BAND_LIST_FLOOR);
        let bands: Vec<Vec<BandRow>> = cover
            .rasters()
            .enumerate()
            .map(|(layer, raster)| {
                let Some(raster) = raster else { return Vec::new() };
                let coverage = |x: usize, y: usize| {
                    let (nx, ny) = cover.centre(x, y);
                    layers.one(layer, nx, ny)
                };
                let rows = layer_band(data, (w, h), block_edge(raster.img, w, h), budget, &coverage);
                budget = budget.saturating_sub(rows.iter().map(Vec::len).sum());
                rows
            })
            .collect();
        if bands.iter().all(Vec::is_empty) {
            return none;
        }
        let owed = |layer: usize, x: u32, y: usize| -> Option<Owed> {
            let row = bands.get(layer)?.get(y)?;
            row.binary_search_by_key(&x, |entry| entry.0).ok().map(|i| row[i].1)
        };
        let rows = (0..h)
            .into_par_iter()
            .map(|y| {
                let mut columns: Vec<u32> =
                    bands.iter().filter_map(|rows| rows.get(y)).flatten().map(|entry| entry.0).collect();
                columns.sort_unstable();
                columns.dedup();
                columns
                    .into_iter()
                    .map(|x| {
                        let (nx, ny) = cover.centre(x as usize, y);
                        // Each content's weight folds through the layers as a coverage does; the shares are
                        // the layer's whose two weights stand furthest apart — the edge the pixel is read on.
                        let folded = layers.fold(nx, ny, |layer, weight| owed(layer, x, y).map_or([weight; 2], |owed| owed.cover));
                        let read = (0..bands.len())
                            .filter_map(|layer| owed(layer, x, y))
                            .max_by(|a, b| (a.cover[1] - a.cover[0]).abs().total_cmp(&(b.cover[1] - b.cover[0]).abs()));
                        let (area, share) = read.map_or((0.0, [0.0; 4]), |owed| (owed.area, owed.share));
                        (x, Owed { cover: if cover.m.inverted { folded.map(|v| 1.0 - v) } else { folded }, area, share })
                    })
                    .collect()
            })
            .collect();
        Self { rows }
    }

    /// Row `y`'s band pixels and what each is owed, ascending by column; empty
    /// for a row with none.
    pub(super) fn row(&self, y: usize) -> &[(u32, Owed)] {
        self.rows.get(y).map_or(&[], Vec::as_slice)
    }
}

impl<'a> Cover<'a> {
    fn layers(&self) -> Layers<'_> {
        Layers { m: self.m, base: self.bmp, comps: &self.comp_bmps, unwarp: self.unwarp, dims: (self.w as f32, self.h as f32) }
    }

    /// The raster each layer reads, in [`Layers::one`]'s order; `None` for a
    /// layer drawn from parameters.
    fn rasters(&self) -> impl Iterator<Item = Option<Bmp<'a>>> + '_ {
        std::iter::once(self.bmp).chain(self.comp_bmps.iter().copied())
    }

    /// Pixel `(x, y)`'s CENTRE in normalised frame coordinates, where
    /// [`Cover::geometry_at`] samples.
    fn centre(&self, x: usize, y: usize) -> (f32, f32) {
        ((x as f32 + MASK_SAMPLE_CENTRE) / self.w as f32, (y as f32 + MASK_SAMPLE_CENTRE) / self.h as f32)
    }

    /// Pixel `(x, y)`'s weight in a per-pixel pass: `owed`'s entry for it
    /// where it has one ([`BandShares::row`]), times the master amount; its
    /// coverage ([`Cover::at`]) where it has none.
    pub(super) fn weight_of(&self, x: usize, y: usize, owed: Option<&(u32, Owed)>) -> PixelWeight {
        match owed {
            Some((_, owed)) => PixelWeight::mixed(owed.cover.map(|c| c * self.amount), owed.share),
            None => PixelWeight::flat(self.at(x, y)),
        }
    }
}

/// The edge, in frame pixels, of the blocks a layer's band is read in: one
/// raster pixel's worth where the raster is coarser than the frame (rounded
/// up), one frame pixel otherwise — so the 13 x 13 blocks a band block reads
/// are [`MASK_BAND_REACH`] raster pixels each way and never fewer frame
/// pixels, and a block's step to its neighbour is the raster's own.
pub(super) fn block_edge(raster: &image::GrayImage, w: usize, h: usize) -> usize {
    let across = w as f32 / raster.width().max(1) as f32;
    let down = h as f32 / raster.height().max(1) as f32;
    (across.max(down).ceil() as usize).max(1)
}

