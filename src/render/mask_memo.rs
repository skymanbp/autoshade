//! What the mask pass keeps from one develop of a frame to the next (2026-11-19): the lens map its masks are read through, and the frame as the first mask that changed finds it. Each is kept under every input it is computed from, compared exactly, so a kept value is the value a fresh computation would give or it is not served.

use super::mask_raster::MaskBitmap;
use super::*;
use std::sync::{Arc, Weak};

/// How many lens maps are kept: one is two tables of [`LUT_N`] numbers
/// (32 KiB), and a session asks for its canvas's and its draft's.
const UNWARP_KEPT: usize = 8;

/// The longest side of a frame the mask pass remembers: the editor's canvas
/// at its largest (the GUI's `preview_edge` is 1280, 2560 or 4096). A frame
/// with a longer side is no canvas — it is an export of a larger photo — and
/// [`MaskChain::open`] turns it away before anything is made of it.
pub(super) const CANVAS_EDGE: usize = 4096;

/// The longest side of the smallest draft the editor draws while a slider is
/// held (the GUI's `draft::DRAFT_EDGE_FLOOR`).
const DRAFT_EDGE: usize = 1024;

/// Room for the keys and the colour field's bands of the two frames below.
const KEYS_BYTES: usize = 8 * 1024 * 1024;

/// The bytes the kept frames of [`MaskChain`] may hold together, their keys
/// and the colour field's band counted in: what ONE canvas and ONE draft of
/// it need, each at its largest (a square photo) and each in the two copies
/// a frame is kept in (the frame the masks found, the frame the changed mask
/// found) at 12 bytes a pixel, and the room for their keys —
/// 2 × 12 × (4096² + 1024²) + 8 MiB = 384 MiB + 24 MiB + 8 MiB = 416 MiB.
/// A 3:2 photo's canvas and draft (4096×2731, 1024×683) take 272 MiB of it.
/// The canvas is in it (2026-11-20) so that the sharp frame drawn when a
/// slider is let go starts at the mask that moved, as the drafts before it
/// did; before, the bound was 96 MiB and held drafts alone.
///
/// Fixed, like the brush cache and the raster cache beside it
/// (`brush::BRUSH_RASTER_CACHE_BYTES`, `MASK_RASTER_BUDGET_BYTES`): it does
/// not grow with the photo, so it is no row of
/// `decode::PIPELINE_BYTES_PER_PIXEL` and does not move
/// `jobs::PER_PHOTO_PEAK_COMMIT_MB`. The editor gives it back when its
/// canvas changes ([`forget_kept_frames`]).
const CHAIN_BYTES: usize = 2 * std::mem::size_of::<[f32; 3]>() * (CANVAS_EDGE * CANVAS_EDGE + DRAFT_EDGE * DRAFT_EDGE) + KEYS_BYTES;

/// How many frames are remembered at all, kept or only seen.
const CHAIN_SLOTS: usize = 16;

/// The exact text of a value: what its `Debug` prints. Two values of one
/// type whose bits differ print differently — a float prints the shortest
/// decimal that reads back as itself, `-0.0` with its sign, and a derived
/// `Debug` prints every field — except two NaNs, so a value holding one has
/// no text and is never taken for another.
fn exact_text(value: &impl std::fmt::Debug) -> Option<String> {
    let text = format!("{value:?}");
    (!text.contains("NaN")).then_some(text)
}

#[cfg(test)]
thread_local! {
    static UNWARP_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CHAIN_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CHAIN_TRACE: std::cell::Cell<ChainTrace> = const { std::cell::Cell::new(ChainTrace::NONE) };
}

/// How many lens maps this thread has built ([`unwarp_of`]).
#[cfg(test)]
pub(super) fn unwarp_built() -> usize {
    UNWARP_BUILT.with(std::cell::Cell::get)
}

/// The lens map of a frame ([`MaskFrame::unwarp`]): `MaskUnwarp::corrected`
/// of `profile` and `dims`, or `MaskUnwarp::new` of them and `amount`.
///
/// Kept, because building one is a 256-step scan and 40 bisection steps at
/// each of [`LUT_N`] nodes — 11 ms on the machine this was measured on
/// (2026-11-19), whatever the frame's size — and one develop of a
/// lens-corrected source asked for it once for its own position passes
/// (`corrected_map`), once for the mask pass, once for every mask (its
/// LINEAR handles) and four times for the colour field's partition: 17
/// builds, 187 ms, on a card of eleven masks, at 1024 px as at 4096.
///
/// The key is everything the two builders read: the profile (its exact
/// text), the amount and the frame's size as bits, and which builder. A
/// profile holding a NaN is built every time.
pub(super) fn unwarp_of(profile: &crate::recipe::LensProfile, amount: f32, corrected: bool, dims: (f32, f32)) -> Option<MaskUnwarp> {
    let build = || {
        #[cfg(test)]
        UNWARP_BUILT.with(|n| n.set(n.get() + 1));
        if corrected { MaskUnwarp::corrected(profile, dims) } else { MaskUnwarp::new(profile, amount, dims) }
    };
    let Some(text) = exact_text(profile) else { return build() };
    let key = (text, corrected, [amount.to_bits(), dims.0.to_bits(), dims.1.to_bits()]);
    if let Some(map) = memory(|m| m.maps.iter().find(|(k, _)| *k == key).map(|(_, map)| map.clone())) {
        return map;
    }
    // Built outside the lock: two callers at once build it twice and keep one.
    let map = build();
    memory(|m| {
        m.maps.retain(|(k, _)| *k != key);
        m.maps.insert(0, (key, map.clone()));
        m.maps.truncate(UNWARP_KEPT);
    });
    map
}

/// The profile's exact text, which builder, and the amount and the frame's
/// two sides as bits.
type UnwarpKey = (String, bool, [u32; 3]);

/// Everything this module keeps.
struct Memory {
    maps: Vec<(UnwarpKey, Option<MaskUnwarp>)>,
    frames: Kept,
}

/// The process's one memory, locked for the length of `with` — a lookup or a
/// put, never a build or a develop. A poisoned lock is still whole: nothing
/// of a caller's runs under it.
#[cfg(not(test))]
fn memory<T>(with: impl FnOnce(&mut Memory) -> T) -> T {
    static MEMORY: std::sync::Mutex<Memory> = std::sync::Mutex::new(Memory { maps: Vec::new(), frames: Kept(Vec::new()) });
    with(&mut MEMORY.lock().unwrap_or_else(|p| p.into_inner()))
}

/// Under test each thread keeps its own, so what a test is served does not
/// depend on the tests running beside it.
#[cfg(test)]
fn memory<T>(with: impl FnOnce(&mut Memory) -> T) -> T {
    thread_local! {
        static MEMORY: std::cell::RefCell<Memory> = const { std::cell::RefCell::new(Memory { maps: Vec::new(), frames: Kept(Vec::new()) }) };
    }
    MEMORY.with(|m| with(&mut m.borrow_mut()))
}

/// Every kept frame is given back. The editor calls it when the photo, the
/// card or the resolution on its canvas changes: the frames kept of the one
/// before can be a quarter of a gigabyte and more, and would otherwise stay
/// until later frames pushed them out. The lens maps stay (eight of them are
/// 256 KiB).
pub fn forget_kept_frames() {
    memory(|m| m.frames.0.clear());
}

/// The bytes this thread's kept frames hold, and how many frames are
/// remembered.
#[cfg(test)]
pub(super) fn kept() -> (usize, usize) {
    memory(|m| (m.frames.0.iter().map(Slot::bytes).sum(), m.frames.0.len()))
}

/// One local adjustment as the mask pass reads it: every field of it (its
/// exact text), and WHICH decoded raster each of its layers reads — the
/// object's allocation, held weakly. While the handle lives no other object
/// is given that allocation, so the same allocation is the same object, and
/// nothing writes through an `Arc<MaskBitmap>`, so the same object is the
/// same bytes. A file repainted under its path is decoded anew by the loader
/// (`load_mask_bitmap`, by the file's time and size) and is another object.
/// The raster's pixels are not kept alive by it: the raster cache's budget
/// stays the only thing that holds them.
///
/// The loader hands out the same object while its cache holds it, and that
/// cache is emptied whole when a file is added to seventeen already held or
/// its byte budget is passed (`load_mask_bitmap`). After such a reset every
/// raster is decoded anew, so the next develop is a first sight from its
/// first raster-backed mask on — correct, only slower. A card whose masks
/// read eighteen or more raster files empties the cache within every
/// develop: it decodes its rasters anew on every develop, as it did before
/// this memory existed, and is served nothing past the masks before its
/// first raster.
#[derive(Clone)]
struct MaskKey {
    text: String,
    rasters: Vec<Option<Weak<MaskBitmap>>>,
}

impl MaskKey {
    fn of(m: &crate::recipe::LocalAdjustment, rasters: &MaskRasterSnapshot) -> Option<Self> {
        let layers = std::iter::once(&m.mask).chain(m.components.iter().map(|c| &c.geometry));
        Some(Self { text: exact_text(m)?, rasters: layers.map(|g| rasters.held(g).map(Arc::downgrade)).collect() })
    }

    fn bytes(&self) -> usize {
        self.text.len() + std::mem::size_of_val(self.rasters.as_slice())
    }
}

impl PartialEq for MaskKey {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.rasters.len() == other.rasters.len()
            && self.rasters.iter().zip(&other.rasters).all(|pair| match pair {
                (Some(a), Some(b)) => Weak::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            })
    }
}

/// What is remembered of one frame the mask pass was run on.
struct Slot {
    /// Where it is looked up: the frame's size and a hint of its pixels
    /// ([`hint`]). The hint picks the slot; it never says two frames are one.
    at: (usize, usize, u64),
    /// Everything the pass reads besides the pixels and the masks.
    context: String,
    /// The masks of the last develop of it, in order.
    masks: Vec<MaskKey>,
    /// The frame as the masks found it, from its second sight on.
    start: Option<Vec<[f32; 3]>>,
    /// The frame as mask `.0` found it — every mask before it applied.
    resume: Option<(usize, Vec<[f32; 3]>)>,
    /// The colour field's band on the frame the masks found
    /// (`masks::field_sky_band`), with the zone mask it was read from:
    /// which mask, and the mask.
    field: Option<FieldBand>,
}

type FieldBand = (usize, MaskKey, Vec<(u32, f32)>);

impl Slot {
    /// What it holds besides the frames: the keys, and the band.
    fn besides(&self) -> usize {
        let band = self.field.as_ref().map_or(0, |(_, _, band)| std::mem::size_of_val(band.as_slice()));
        band + self.context.len() + self.masks.iter().map(MaskKey::bytes).sum::<usize>()
    }

    fn bytes(&self) -> usize {
        let pixels = self.start.as_ref().map_or(0, Vec::len) + self.resume.as_ref().map_or(0, |(_, frame)| frame.len());
        pixels * std::mem::size_of::<[f32; 3]>() + self.besides()
    }

    /// The most it comes to hold: both copies of its frame. A frame is
    /// remembered only where this is under the bound ([`MaskChain::open`]),
    /// so a slot is never dropped by its own put — a frame too large to keep
    /// is not copied and thrown away on every develop.
    fn at_most(&self) -> usize {
        (self.at.0 * self.at.1).saturating_mul(2 * std::mem::size_of::<[f32; 3]>()).saturating_add(self.besides())
    }
}

/// The remembered frames, the most recently developed first.
struct Kept(Vec<Slot>);

impl Kept {
    fn take(&mut self, at: (usize, usize, u64)) -> Option<Slot> {
        let i = self.0.iter().position(|slot| slot.at == at)?;
        Some(self.0.remove(i))
    }

    /// `slot` in front, then the oldest dropped until what is left holds
    /// `bytes` at most and [`CHAIN_SLOTS`] frames.
    fn put(&mut self, slot: Slot, bytes: usize) {
        self.0.retain(|other| other.at != slot.at);
        self.0.insert(0, slot);
        self.0.truncate(CHAIN_SLOTS);
        let mut held = 0usize;
        let fits = self.0.iter().take_while(|slot| {
            held = held.saturating_add(slot.bytes());
            held <= bytes
        });
        let fits = fits.count();
        self.0.truncate(fits);
    }
}

/// A frame's pixels in 64 bits, from 4096 of them at most: which remembered
/// frame a develop is compared with, and whether a frame has been seen
/// before. Two frames with one hint are told apart by [`same_bits`].
fn hint(data: &[[f32; 3]]) -> u64 {
    let step = (data.len() / 4096).max(1);
    data.iter().step_by(step).flatten().fold(0xcbf2_9ce4_8422_2325u64, |h, v| (h ^ u64::from(v.to_bits())).wrapping_mul(0x0100_0000_01b3))
}

/// Are the two frames the same bits, pixel for pixel?
fn same_bits(a: &[[f32; 3]], b: &[[f32; 3]]) -> bool {
    const ROW: usize = 1 << 14;
    a.len() == b.len()
        && a.par_chunks(ROW).zip(b.par_chunks(ROW)).all(|(a, b)| a.iter().zip(b).all(|(p, q)| p.map(f32::to_bits) == q.map(f32::to_bits)))
}

/// What [`MaskChain`] did on this thread's last mask pass.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ChainTrace {
    /// Whether the frame the masks found is kept after this pass.
    pub(super) kept_start: bool,
    /// The first mask that ran; the ones before it were not applied again.
    pub(super) resumed: usize,
    /// The mask whose frame is kept after this pass, if one is.
    pub(super) kept_at: Option<usize>,
    /// Whether the colour field's band was handed out as kept.
    pub(super) field_served: bool,
}

#[cfg(test)]
impl ChainTrace {
    /// A pass that kept nothing and was served nothing.
    pub(super) const NONE: Self = Self { kept_start: false, resumed: 0, kept_at: None, field_served: false };
}

#[cfg(test)]
pub(super) fn chain_trace() -> ChainTrace {
    CHAIN_TRACE.with(std::cell::Cell::get)
}

/// `run` with nothing kept and nothing served on this thread: the mask pass
/// as it is computed from nothing.
#[cfg(test)]
pub(super) fn without_chain<T>(run: impl FnOnce() -> T) -> T {
    CHAIN_OFF.with(|off| off.set(true));
    let out = run();
    CHAIN_OFF.with(|off| off.set(false));
    out
}

/// The mask pass's memory of the frame it is run on (`apply_masks`).
///
/// **What it removes.** A slider is dragged on ONE mask; the editor develops
/// the frame again on every move, and every mask before that one was applied
/// to the same pixels with the same values as the move before. On the card
/// this was measured on (eleven masks, ten of them rasters, 1024 px draft,
/// 2026-11-19) those ten cost 0.6 s of a 0.8 s develop, and the eleventh,
/// the one in hand, 13 ms.
///
/// **What is kept.** The frame as the first mask that differs from the last
/// develop's finds it. A later develop of the same frame whose masks before
/// that one are the same takes it and starts there. And the colour field's
/// band, which is read on the frame the masks find before any of them runs
/// ([`MaskChain::field_band`]): 29 ms of each develop on that card.
///
/// **Why a kept frame cannot be stale.** The frame mask `k` finds is computed
/// from the frame the masks found, the frame's size, the masks before `k`,
/// their rasters, and what the pass reads of the recipe, the lens frame and
/// the film scale — `apply_masks`' arguments and nothing else. It is served
/// only when all of them are the last develop's: the pixels bit for bit
/// ([`same_bits`], against a kept copy), each mask and the rest of the recipe
/// by exact text ([`exact_text`]; the WHOLE recipe less its masks, not the
/// fields the pass is known to read), each raster by being the same decoded
/// object ([`MaskKey`]). The masks from `k` on are not in it: they have not
/// run.
///
/// **What a develop that never comes again pays.** A sampled hint of its
/// pixels and the text of its masks, and no pixel is copied: a frame is kept
/// from its SECOND sight ([`hint`]) — a canvas-sized preview rendered once
/// copies nothing. A frame with a side over [`CANVAS_EDGE`], which is an
/// export of a photo larger than the canvas, pays nothing at all: it is
/// turned away before any text is made.
///
/// **What is given back.** The editor drops the kept frames when the photo,
/// the card or the resolution on its canvas changes
/// ([`forget_kept_frames`]); otherwise the least recently developed go
/// first under [`CHAIN_BYTES`].
///
/// **Two develops at once.** A develop takes its frame's slot out of the list
/// and puts its own back when it is done; one that finds the slot gone runs
/// as a first sight. The lock is held for the take and the put only.
pub(super) struct MaskChain(Option<Run>);

struct Run {
    slot: Slot,
    /// The last develop's band of the colour field, where the pixels and
    /// the context are that develop's.
    field_before: Option<FieldBand>,
    /// The mask whose frame is to be kept this time; `None` keeps what is held.
    keep_at: Option<usize>,
}

impl MaskChain {
    pub(super) fn open(
        data: &[[f32; 3]],
        (w, h): (usize, usize),
        r: &EditRecipe,
        rasters: &MaskRasterSnapshot,
        frame: MaskFrame<'_>,
        film: FilmScale,
    ) -> Self {
        #[cfg(test)]
        CHAIN_TRACE.with(|t| t.set(ChainTrace::NONE));
        #[cfg(test)]
        if CHAIN_OFF.with(std::cell::Cell::get) {
            return Self(None);
        }
        if r.masks.is_empty() || data.is_empty() || data.len() != w * h || w.max(h) > CANVAS_EDGE {
            return Self(None);
        }
        let rest = EditRecipe { masks: Vec::new(), ..r.clone() };
        let Some(context) = exact_text(&(&rest, frame, film)) else { return Self(None) };
        let Some(masks) = r.masks.iter().map(|m| MaskKey::of(m, rasters)).collect::<Option<Vec<_>>>() else {
            return Self(None);
        };
        let at = (w, h, hint(data));
        let first = Slot { at, context, masks, start: None, resume: None, field: None };
        if first.at_most() > CHAIN_BYTES {
            return Self(None);
        }
        let before = memory(|m| m.frames.take(at));
        let Some(before) = before else {
            return Self(Some(Run { slot: first, field_before: None, keep_at: None }));
        };
        let Slot { context, masks, .. } = first;
        // Seen before: kept from here on. What was kept of it holds where the
        // pixels and the context are the same.
        let (start, same) = match before.start {
            Some(start) if same_bits(&start, data) => (start, before.context == context),
            _ => (data.to_vec(), false),
        };
        let shared = if before.context == context { before.masks.iter().zip(&masks).take_while(|(a, b)| a == b).count() } else { 0 };
        let resume = before.resume.filter(|(index, _)| same && *index <= shared);
        let keep_at = (shared > 0 && resume.as_ref().map(|(index, _)| *index) != Some(shared)).then_some(shared);
        let field_before = before.field.filter(|_| same);
        Self(Some(Run { slot: Slot { at, context, masks, start: Some(start), resume, field: None }, field_before, keep_at }))
    }

    /// The colour field's band on the frame the masks find: `read`'s
    /// answer, or the last develop's where it is the same answer. `zone` is
    /// the mask the band is read from (`masks::field_zone_kept`).
    ///
    /// The band is computed from the frame the masks find, the zone mask's
    /// base geometry and raster, the lens frame and the frame's size
    /// (`masks::field_sky_band`). It is served when the pixels and the
    /// context are the last develop's ([`MaskChain`]), the zone is the same
    /// mask of the list, and that mask is the last develop's in every
    /// field and raster ([`MaskKey`]) — its sliders too, which the band
    /// does not read: a zone whose own slider is in hand reads its band
    /// every time. With no `zone` it is read every time.
    pub(super) fn field_band(&mut self, zone: Option<usize>, read: impl FnOnce() -> Vec<(u32, f32)>) -> Vec<(u32, f32)> {
        let (Some(run), Some(zone)) = (&mut self.0, zone) else { return read() };
        let Some(key) = run.slot.masks.get(zone) else { return read() };
        let band = match run.field_before.take() {
            Some((index, mask, band)) if index == zone && mask == *key => {
                #[cfg(test)]
                CHAIN_TRACE.with(|t| t.set(ChainTrace { field_served: true, ..t.get() }));
                band
            }
            _ => read(),
        };
        // Kept with the frame it was read on: from the frame's second sight,
        // and where the slot has the room.
        if run.slot.start.is_some() {
            run.slot.field = Some((zone, key.clone(), band.clone()));
            if run.slot.at_most() > CHAIN_BYTES {
                run.slot.field = None;
            }
        }
        band
    }

    /// The first mask to run: `data` is made the frame that mask finds.
    pub(super) fn resume(&mut self, data: &mut [[f32; 3]]) -> usize {
        let Some(run) = &mut self.0 else { return 0 };
        let Some((index, frame)) = &run.slot.resume else { return 0 };
        let index = *index;
        data.copy_from_slice(frame);
        if run.keep_at.is_some() {
            // A later mask's frame is kept this time: this one goes first.
            run.slot.resume = None;
        }
        #[cfg(test)]
        CHAIN_TRACE.with(|t| t.set(ChainTrace { resumed: index, ..t.get() }));
        index
    }

    /// Mask `index` is about to run on `data` (or, at the masks' count, the
    /// pass is done).
    pub(super) fn reached(&mut self, index: usize, data: &[[f32; 3]]) {
        if let Some(run) = &mut self.0
            && run.keep_at == Some(index)
        {
            run.slot.resume = Some((index, data.to_vec()));
        }
    }

    /// The pass is done on `data`, `count` masks in all.
    pub(super) fn close(mut self, count: usize, data: &[[f32; 3]]) {
        self.reached(count, data);
        let Some(run) = self.0 else { return };
        #[cfg(test)]
        CHAIN_TRACE.with(|t| {
            t.set(ChainTrace { kept_start: run.slot.start.is_some(), kept_at: run.slot.resume.as_ref().map(|(index, _)| *index), ..t.get() })
        });
        memory(|m| m.frames.put(run.slot, CHAIN_BYTES));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(at: u64, pixels: usize) -> Slot {
        Slot { at: (4, 4, at), context: String::new(), masks: Vec::new(), start: Some(vec![[0.0; 3]; pixels]), resume: None, field: None }
    }

    /// A slot holding all it can hold — both copies of its frame, its keys,
    /// its band — holds what it was admitted for, and not a byte more.
    #[test]
    fn a_slot_holds_no_more_than_it_was_admitted_for() {
        let key = MaskKey { text: "mask".into(), rasters: vec![None, None] };
        let mut full = Slot { at: (5, 4, 0), context: "context".into(), masks: vec![key.clone()], start: None, resume: None, field: None };
        let keys = 7 + 4 + 2 * std::mem::size_of::<Option<Weak<MaskBitmap>>>();
        assert_eq!((full.bytes(), full.at_most()), (keys, 2 * 20 * 12 + keys), "seen once: the keys alone, and room asked for two copies");
        full.start = Some(vec![[0.0; 3]; 20]);
        full.resume = Some((1, vec![[0.0; 3]; 20]));
        full.field = Some((0, key, vec![(0, 0.0); 3]));
        assert_eq!(full.bytes(), 2 * 20 * 12 + keys + 3 * 8);
        assert_eq!(full.bytes(), full.at_most());
    }

    /// The list holds its bytes: the oldest frames go first, and a frame put
    /// again replaces itself.
    ///
    /// MUTATION: drop the byte walk in `Kept::put`.
    #[test]
    fn the_kept_frames_hold_their_bytes_and_drop_the_oldest() {
        let mut kept = Kept(Vec::new());
        let bytes = 3 * 100 * 12;
        for at in 0..5 {
            kept.put(slot(at, 100), bytes);
        }
        assert_eq!(kept.0.iter().map(|s| s.at.2).collect::<Vec<_>>(), [4, 3, 2], "three fit; the two oldest went");
        kept.put(slot(3, 100), bytes);
        assert_eq!(kept.0.iter().map(|s| s.at.2).collect::<Vec<_>>(), [3, 4, 2], "put again, it moves to the front");
        kept.put(slot(9, 250), bytes);
        assert_eq!(kept.0.iter().map(|s| s.at.2).collect::<Vec<_>>(), [9], "a large one leaves room for itself alone");
        assert!(kept.take((4, 4, 9)).is_some() && kept.0.is_empty());
        for at in 0..2 * CHAIN_SLOTS as u64 {
            kept.put(slot(at, 0), bytes);
        }
        assert_eq!(kept.0.len(), CHAIN_SLOTS, "and the list itself is bounded");
    }

    /// The bound is what one canvas and one draft of it need at their
    /// largest, each in its two copies: 384 MiB and 24 MiB, and 8 MiB for
    /// their keys. A 3:2 photo's canvas and draft take 272 MiB of it.
    #[test]
    fn the_bound_is_one_canvas_and_one_draft_at_their_largest() {
        let frame = |w: usize, h: usize| Slot { at: (w, h, 0), context: String::new(), masks: Vec::new(), start: None, resume: None, field: None }.at_most();
        assert_eq!((frame(4096, 4096), frame(1024, 1024)), (384 << 20, 24 << 20));
        assert_eq!(CHAIN_BYTES, frame(CANVAS_EDGE, CANVAS_EDGE) + frame(DRAFT_EDGE, DRAFT_EDGE) + KEYS_BYTES);
        assert_eq!(CHAIN_BYTES, 416 << 20);
        let three_two = frame(4096, 2731) + frame(1024, 683);
        assert!(three_two > (272 << 20) && three_two < (273 << 20), "{three_two}");
    }

    /// The exact text tells apart what `==` does not, and refuses a NaN.
    #[test]
    fn the_exact_text_tells_a_negative_zero_and_refuses_a_nan() {
        assert_ne!(exact_text(&0.0f32), exact_text(&-0.0f32));
        assert_ne!(exact_text(&0.1f32), exact_text(&0.100_000_01f32));
        assert_eq!(exact_text(&f32::NAN), None);
        assert!(same_bits(&[[0.5, 0.25, 1.0]], &[[0.5, 0.25, 1.0]]));
        assert!(!same_bits(&[[0.0, 0.25, 1.0]], &[[-0.0, 0.25, 1.0]]), "bits, not values");
        assert!(!same_bits(&[[0.5; 3]], &[[0.5; 3], [0.5; 3]]));
    }
}
