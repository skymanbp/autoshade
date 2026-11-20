//! What the band read keeps from one develop to the next (2026-11-20): the SHAPE half of a raster layer's band read (`mask_band::layer_band`) — each strip's block coverage, its 3 x 3 mean and the mean's largest step — which reads the layer's coverage and the strip's place and nothing of the pixels. Kept under every input it is computed from, compared exactly, so a kept plane is the plane a fresh computation gives or it is not served.

use super::mask_memo::{exact_text, CANVAS_EDGE};
use super::mask_raster::MaskBitmap;
use super::masks::Cover;
use super::*;
use std::sync::{Arc, Mutex, Weak};

/// The bytes the kept planes may hold together, their keys counted in: one
/// card's canvas and one draft of it, ten raster layers each, the rasters at
/// half the canvas's size (a 3:2 photo: 4096 x 2731 read in blocks of three
/// pixels, 1366 x 911 blocks, and 1024 x 683 in blocks of one), at three
/// planes of 4 bytes a block — 10 x 12 x (1 244 426 + 699 392) = 233 MB, of
/// 268 MB. Finer rasters on more layers do not all fit: the least recently
/// read go first, and a layer whose planes are not kept is read as it was
/// before anything was kept.
///
/// Fixed, like the kept frames beside it (`mask_memo::CHAIN_BYTES`): it does
/// not grow with the photo, so it is no row of
/// `decode::PIPELINE_BYTES_PER_PIXEL` and does not move
/// `jobs::PER_PHOTO_PEAK_COMMIT_MB`. The editor gives it back when its canvas
/// changes (`forget_kept_frames`).
const BAND_SHAPES_BYTES: usize = 256 * 1024 * 1024;

/// One strip's shape planes, as `layer_band` numbers them: each block's mean
/// coverage, its 3 x 3 mean, and the mean's largest step to a neighbour.
pub(super) struct Planes {
    pub(super) cover: Vec<f32>,
    pub(super) smooth: Vec<f32>,
    pub(super) slope: Vec<f32>,
}

impl Planes {
    fn bytes(&self) -> usize {
        std::mem::size_of::<f32>() * (self.cover.len() + self.smooth.len() + self.slope.len())
    }
}

/// What one raster layer's coverage at a pixel is computed from, besides the
/// frame's size: the closure `mask_owed::BandShares::read` hands the read is
/// `Layers::one(layer, Cover::centre(x, y))`, and every input of it is here.
///
/// - `Cover::centre` (`mask_owed.rs`): the pixel, the frame's `w` and `h`, and
///   `MASK_SAMPLE_CENTRE`, a constant. The frame's size is in the strip's key
///   ([`planes`]).
/// - `Layers::one` and `Layers::stored` (`mask_owed.rs`): the layer's own
///   GEOMETRY — the base's or component `k − 1`'s, nothing composed, no
///   inversion of the adjustment or the component, no amount — its RASTER,
///   the frame's LENS MAP (`Cover::unwarp`), and the frame's size as `dims`.
/// - `mask_weight_in`, `mask_weight_metric`, `mask_weight`
///   (`mask_weight.rs`): the geometry's every field, the lens map
///   (`source_point` through `Layers::stored`; `engine_at` for a raster drawn
///   in the corrected frame), the raster's pixels and whether it is drawn in
///   the corrected frame (`Bmp`), and `dims`. Nothing global.
///
/// So the key is:
/// - the geometry's exact text (`mask_memo::exact_text`: its `Debug`, which
///   prints every field and every float as itself; a geometry holding a NaN
///   is not kept). It is the geometry the pass reads — after
///   `MaskFrame::prepare_linear_handles`, `Cover::m`.
/// - WHICH decoded raster the layer reads (`Cover::new`, `masks.rs`), as the
///   kept frames know a raster (`mask_memo::MaskKey`): the snapshot's object,
///   held weakly, compared by allocation — while the handle lives no other
///   object is given that allocation, and nothing writes through an
///   `Arc<MaskBitmap>`, so the same allocation is the same pixels and the same
///   frame marker (`MaskBitmap::bmp`). A raster the snapshot does not hold is
///   a brush's (`Cover::new` takes `rasters.get(..)` or the brush), stamped by
///   `brush::brush_raster` from the geometry's own strokes and the frame's
///   size alone: the text and the size are its key. A raster that is neither
///   is not kept.
/// - the lens map's every number (`MaskUnwarp::same_map`), or that there is
///   none.
///
/// and, in the strip's key, the frame's `w` and `h`, the block edge
/// (`mask_owed::block_edge` of the raster and the frame; `layer_band` is handed
/// it and its planes are counted in it) and the strip's first block row: with
/// the frame's size and the block they fix the strip layout (`layer_band`'s
/// `strip`, `halo`, `lo`, `hi`, from constants).
#[derive(Clone)]
pub(super) struct LayerShape {
    text: String,
    raster: Option<Weak<MaskBitmap>>,
    unwarp: Option<MaskUnwarp>,
}

impl LayerShape {
    /// Layer `layer` of `cover` (0 the base, `k` component `k − 1`, as
    /// `Layers::one` counts), where its planes may be kept: on a frame whose
    /// longer side is the editor's canvas at most (`mask_memo::CANVAS_EDGE`,
    /// the condition the kept frames are under) — a larger frame is an export,
    /// developed once, and turned away before any text is made — and where
    /// every input is known by the key. `None` otherwise: the layer is read as
    /// it was before anything was kept.
    pub(super) fn of(cover: &Cover<'_>, layer: usize) -> Option<Self> {
        #[cfg(test)]
        if super::mask_memo::chain_off() {
            return None;
        }
        if cover.w.max(cover.h) > CANVAS_EDGE {
            return None;
        }
        let (geometry, bmp) = match layer.checked_sub(1) {
            None => (&cover.m.mask, cover.bmp),
            Some(k) => (&cover.m.components.get(k)?.geometry, *cover.comp_bmps.get(k)?),
        };
        let raster = cover.held.get(layer).copied()?.map(Arc::downgrade);
        if bmp.is_some() && raster.is_none() && !matches!(geometry, crate::recipe::MaskGeometry::Brush { .. }) {
            return None;
        }
        Some(Self { text: exact_text(geometry)?, raster, unwarp: cover.unwarp.cloned() })
    }

    fn same(&self, other: &Self) -> bool {
        let raster = match (&self.raster, &other.raster) {
            (Some(a), Some(b)) => Weak::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let unwarp = match (&self.unwarp, &other.unwarp) {
            (Some(a), Some(b)) => a.same_map(b),
            (None, None) => true,
            _ => false,
        };
        raster && unwarp && self.text == other.text
    }

    /// A layer known by its text alone, for the read's own tests.
    #[cfg(test)]
    pub(super) fn named(text: &str) -> Self {
        Self { text: text.into(), raster: None, unwarp: None }
    }
}

/// How many strips are noted as computed once, their planes not kept
/// ([`BandMemo::put`]): a note is a key — the layer's text, a raster handle,
/// a lens map's two shared tables — and four numbers. A card's canvas and
/// draft read one or a few strips for each raster layer at each size.
const SEEN_NOTES: usize = 128;

/// One kept strip: its layer, the frame's `w` and `h`, the block edge and the
/// strip's first block row, and its planes.
struct Kept {
    shape: LayerShape,
    frame: [usize; 4],
    planes: Arc<Planes>,
}

impl Kept {
    /// What the entry holds: its planes, its key's text and the lens map's
    /// tables its key holds a handle to (shared with the lens maps
    /// `mask_memo::unwarp_of` keeps, and counted here all the same).
    fn bytes(&self) -> usize {
        let tables = self.shape.unwarp.as_ref().map_or(0, MaskUnwarp::table_bytes);
        self.planes.bytes() + self.shape.text.len() + tables + std::mem::size_of::<Self>()
    }
}

impl LayerShape {
    /// Can it still be served: a layer whose raster object is gone is read
    /// from a raster that is another object (`MaskKey`'s rule), never again
    /// from this one.
    fn alive(&self) -> bool {
        self.raster.as_ref().is_none_or(|raster| raster.strong_count() > 0)
    }
}

/// What the memory holds: the kept strips, the most recently read first, and
/// the strips computed once, the most recently computed first.
struct Held {
    kept: Vec<Kept>,
    seen: Vec<(LayerShape, [usize; 4])>,
}

/// The kept strips within a byte budget, kept from the SECOND time a strip
/// is computed — the rule the kept frames follow (`MaskChain`: a frame is
/// kept from its second sight): a develop that happens once — an export of a
/// photo no larger than the canvas, a thumbnail — keeps no plane. The lock is
/// held to look up and to put, never while a plane is computed.
pub(super) struct BandMemo {
    budget: usize,
    held: Mutex<Held>,
}

impl BandMemo {
    pub(super) const fn new(budget: usize) -> Self {
        Self { budget, held: Mutex::new(Held { kept: Vec::new(), seen: Vec::new() }) }
    }

    /// A poisoned lock is still whole: nothing of a caller's runs under it.
    fn with<T>(&self, with: impl FnOnce(&mut Held) -> T) -> T {
        with(&mut self.held.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// The kept planes of this strip, moved to the front; `None` where none are kept.
    fn find(&self, shape: &LayerShape, frame: [usize; 4]) -> Option<Arc<Planes>> {
        self.with(|held| {
            let i = held.kept.iter().position(|k| k.frame == frame && k.shape.same(shape))?;
            let hit = held.kept.remove(i);
            let planes = hit.planes.clone();
            held.kept.insert(0, hit);
            Some(planes)
        })
    }

    /// A strip just computed. The first time, only its key is noted (at most
    /// [`SEEN_NOTES`], the oldest dropped). The second, its planes are kept in
    /// front, then the least recently read dropped until what is left is
    /// within the budget; a strip larger than the whole budget is not kept, so
    /// it never pushes out everything else for nothing. Entries and notes whose
    /// raster object is gone are dropped first: they can never be served.
    fn put(&self, shape: &LayerShape, frame: [usize; 4], planes: &Arc<Planes>) {
        self.with(|held| {
            held.kept.retain(|k| k.shape.alive() && !(k.frame == frame && k.shape.same(shape)));
            held.seen.retain(|(seen, _)| seen.alive());
            let Some(noted) = held.seen.iter().position(|(seen, at)| *at == frame && seen.same(shape)) else {
                held.seen.insert(0, (shape.clone(), frame));
                held.seen.truncate(SEEN_NOTES);
                return;
            };
            held.seen.remove(noted);
            let new = Kept { shape: shape.clone(), frame, planes: planes.clone() };
            if new.bytes() > self.budget {
                return;
            }
            held.kept.insert(0, new);
            let mut bytes = 0usize;
            let fits = held
                .kept
                .iter()
                .take_while(|k| {
                    bytes = bytes.saturating_add(k.bytes());
                    bytes <= self.budget
                })
                .count();
            held.kept.truncate(fits);
        });
    }

    fn forget(&self) {
        self.with(|held| {
            held.kept.clear();
            held.seen.clear();
        });
    }

    /// The bytes the kept strips hold and how many strips.
    #[cfg(test)]
    pub(super) fn held(&self) -> (usize, usize) {
        self.with(|held| (held.kept.iter().map(Kept::bytes).sum(), held.kept.len()))
    }
}

/// The process's one memory.
#[cfg(not(test))]
fn memo<T>(with: impl FnOnce(&BandMemo) -> T) -> T {
    static MEMO: BandMemo = BandMemo::new(BAND_SHAPES_BYTES);
    with(&MEMO)
}

#[cfg(test)]
thread_local! {
    static MEMO: std::cell::RefCell<Arc<BandMemo>> = std::cell::RefCell::new(Arc::new(BandMemo::new(BAND_SHAPES_BYTES)));
    static SERVED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Under test each thread keeps its own, so what a test is served does not
/// depend on the tests running beside it ([`using`] hands one to several).
#[cfg(test)]
fn memo<T>(with: impl FnOnce(&BandMemo) -> T) -> T {
    let memo = MEMO.with(|m| m.borrow().clone());
    with(&memo)
}

/// `run` with `memo` as this thread's memory, the thread's own put back after.
#[cfg(test)]
pub(super) fn using<T>(memo: Arc<BandMemo>, run: impl FnOnce() -> T) -> T {
    let own = MEMO.with(|m| m.replace(memo));
    let out = run();
    MEMO.with(|m| m.replace(own));
    out
}

/// How many strips this thread has been handed as kept.
#[cfg(test)]
pub(super) fn served() -> usize {
    SERVED.with(std::cell::Cell::get)
}

/// The bytes this thread's memory holds and how many strips.
#[cfg(test)]
pub(super) fn held() -> (usize, usize) {
    memo(BandMemo::held)
}

/// Every kept strip is given back (`mask_memo::forget_kept_frames`).
pub(super) fn forget() {
    memo(BandMemo::forget);
}

/// The shape planes of one strip of a layer's band read: the kept ones where
/// `shape` knows the layer and they are kept under it, the frame's `w` and
/// `h`, the block edge and the strip's first block row (`frame`); `compute`'s
/// otherwise, kept from its second computation on where `shape` knows the
/// layer ([`BandMemo::put`]). With no `shape` nothing is looked up or kept and
/// `compute` runs as it did before.
pub(super) fn planes(shape: Option<&LayerShape>, frame: [usize; 4], compute: impl FnOnce() -> Planes) -> Arc<Planes> {
    let Some(shape) = shape else { return Arc::new(compute()) };
    if let Some(kept) = memo(|m| m.find(shape, frame)) {
        #[cfg(test)]
        SERVED.with(|n| n.set(n.get() + 1));
        return kept;
    }
    // Computed outside the lock: two callers at once compute it twice and keep one.
    let planes = Arc::new(compute());
    memo(|m| m.put(shape, frame, &planes));
    planes
}
