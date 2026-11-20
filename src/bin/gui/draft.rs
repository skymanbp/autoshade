//! Draft frames: what the canvas shows while a slow photo is being edited.
//!
//! WHY (2026-11-19, user: 「曝光之类的有用，但是响应速度（从调整到预览出效果）
//! 很慢」). Every tick of a slider developed the whole photo from its base at
//! the canvas's own size, and a frame whose recipe had moved on by the time it
//! landed was dropped. On a card that carries ten masks and a colour field, at
//! the 4096 px canvas, one develop is seconds (measured on such a card: 3.8 s
//! in the test profile against 0.25 s with no mask and no field), so a drag
//! showed nothing until the pointer had rested for a whole develop.
//!
//! WHAT CHANGES:
//!
//!  1. **A draft while edits arrive** — only on a photo whose develop is slow.
//!     Once the last FULL develop of the canvas's base has taken longer than
//!     [`DRAFT_BUDGET_MS`], a tick during a gesture develops a reduced copy of
//!     that base: the same preview build, the same film edge, the same source
//!     frame, so the engine does at a smaller size exactly what it does at
//!     the full one. The copy is cut once per base, on the worker.
//!  2. **The newest finished frame is shown.** During a gesture a frame whose
//!     recipe has since moved on goes on the canvas (same photo, same card,
//!     same base, newer than what is there) instead of being dropped, and the
//!     develop of the live recipe still follows.
//!  3. **The full frame on settle.** With no pointer down, no draft in
//!     flight, and the canvas still for the settle time ([`settle_time`]: no
//!     edit, no new frame), the full base is developed and replaces the draft
//!     in place. That frame is the preview build on the full base — what the
//!     canvas showed before this module existed, byte for byte.
//!
//! A photo whose full develop is inside the budget never drafts: its ticks
//! are the full develops they always were.
//!
//! WHAT A DRAFT IS NOT. It is a picture, a histogram and a clipping layer on
//! the canvas while the hand moves, and nothing else: the card's thumbnail,
//! the accepted-frame counter and the mask overlay's refresh are the full
//! frame's alone (`show_passing_frame`), and nothing that is saved or exported
//! reads a canvas frame at all. `dirty` stays set from the first tick of a
//! gesture until the full develop is dispatched, so every reader of "has the
//! develop of this recipe landed" (the card switch in `actions.rs`) keeps its
//! answer.

use super::*;
use std::sync::Weak;

/// What one develop may cost before a gesture is drawn in drafts: about the
/// longest gap at which the picture still reads as following the hand (eight
/// frames a second). A full develop faster than this is its own live frame.
pub(crate) const DRAFT_BUDGET_MS: f32 = 120.0;

/// The shortest long edge a draft is cut to. A draft is stretched over the
/// canvas's rectangle, which at the fit view is 1100–1700 physical pixels
/// across on the displays this runs on: 1024 px is the last size that reads
/// as the photograph slightly out of focus and not as blocks. It is also the
/// size the mask overlay already judges coverage at (`refresh_mask_overlay`).
pub(crate) const DRAFT_EDGE_FLOOR: u32 = 1024;

/// Draft edges come in steps of this, so a full develop that takes 5 % longer
/// one time than the next does not have the reduced base cut again.
const DRAFT_EDGE_STEP: u32 = 256;

/// The shortest time the canvas must have been still, with no pointer down,
/// before the full develop is started. Longer than a key's auto-repeat gap
/// and a wheel notch's (30–50 ms), so a held arrow key or a scrolled value
/// box stays one gesture; short enough that the sharp frame is on its way
/// before the eye asks.
pub(crate) const DRAFT_SETTLE: Duration = Duration::from_millis(150);

/// …and the share of a photo's own full develop time the canvas must have
/// been still for, when that is longer. A full develop cannot be called back,
/// and a draft that shares the workers with one takes two to three times as
/// long (measured on a ten-mask card at 4096 px, release build: 0.7 s alone,
/// 1.2–2.5 s beside the 5 s full develop). So the full develop gives a hand
/// that looks at the draft and moves again the time to do it — and the sharp
/// frame comes at most a fifth later than it could have.
const DRAFT_SETTLE_SHARE: f32 = 0.2;

/// How long the canvas must have been still before the full develop of a
/// photo whose last full develop took `full_ms` is started.
pub(crate) fn settle_time(full_ms: Option<f32>) -> Duration {
    let share = Duration::from_millis((full_ms.unwrap_or(0.0).max(0.0) * DRAFT_SETTLE_SHARE).round() as u64);
    DRAFT_SETTLE.max(share)
}

/// Which develop a tick is owed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DevelopPlan {
    /// The full base — the only plan of a photo that develops inside the budget.
    Full,
    /// A draft at this long edge.
    Draft(u32),
    /// Nothing to start yet: the gesture has not settled and the live
    /// recipe's draft is on its way or shown (or its full frame is), or it
    /// has settled and the full develop is waiting for the draft in flight.
    Wait,
}

/// What a dispatched frame carries back so its landing can place it: the card
/// it was developed for, and its place in the order of dispatch.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FrameTag {
    /// The photo and the card's own id (`Variant::id`); `None` with no photo.
    pub(crate) card: Option<(PathBuf, String)>,
    /// Later dispatches carry larger numbers; a frame built by hand carries 0.
    pub(crate) seq: u64,
}

/// What a draft's landing brings: the frame, the edge its base was cut to,
/// and that reduced base (kept for the next tick).
pub(crate) type DraftDone = (PreviewDone, u32, Arc<image::DynamicImage>);

/// A base, named without keeping its pixels alive. While the handle lives the
/// allocation's address is not given out again, so no other image can be
/// taken for this one.
type BaseId = Weak<image::DynamicImage>;

fn names(id: &BaseId, base: &Arc<image::DynamicImage>) -> bool {
    std::ptr::eq(id.as_ptr(), Arc::as_ptr(base))
}

/// The scheduler's memory between ticks.
#[derive(Default)]
pub(crate) struct Draft {
    /// How long the last full develop of a base took, in milliseconds.
    full_cost: Option<(BaseId, f32)>,
    /// The reduced copy of a base: which base, the long edge, the pixels.
    reduced: Option<(BaseId, u32, Arc<image::DynamicImage>)>,
    /// A draft develop is in flight (one at a time, beside at most one full).
    pub(crate) inflight: bool,
    /// The recipe as the scheduler last saw it.
    seen: Option<EditRecipe>,
    /// When the canvas last moved: the last edit, or the last passing frame
    /// put on it — a hand can only answer a picture it has seen.
    last_stir: Option<Instant>,
    /// The draft last dispatched: for which base and which recipe.
    drafted: Option<(BaseId, EditRecipe)>,
    /// The dispatch counter, and the number of the frame on the canvas.
    seq: u64,
    shown_seq: u64,
    /// What the canvas shows: a frame of which base and which recipe, and
    /// whether it is the full frame (`true`) or a draft.
    on_canvas: Option<(BaseId, EditRecipe, bool)>,
    /// The size the After picture is laid out at while the texture is a
    /// draft's: the size the full frame has. `None` with a full frame on the
    /// canvas, which is laid out at its texture's own size as it always was.
    pub(crate) stands_for: Option<egui::Vec2>,
    /// Frames shown that were NOT the full develop of the live recipe: drafts,
    /// and full frames a gesture had already moved past. The counterpart of
    /// `develop_count`, and like it a counter the tests read.
    pub(crate) passing_count: u64,
}

/// The long edge a draft of a `base_long` px base is cut to when the full
/// develop took `full_ms`, or `None` when the photo does not draft.
///
/// A develop's cost follows its pixel count, so the edge that lands on the
/// budget is `base_long · √(budget / full_ms)` — taken down to a step, and not
/// below the floor. A draft that would not at least halve the pixels is not
/// cut: the draft and the full develop that follows it would then cost more
/// together than the gesture gains.
pub(crate) fn draft_edge(base_long: u32, full_ms: f32) -> Option<u32> {
    if full_ms <= DRAFT_BUDGET_MS {
        return None;
    }
    let want = base_long as f32 * (DRAFT_BUDGET_MS / full_ms).sqrt();
    let edge = ((want / DRAFT_EDGE_STEP as f32).floor() as u32 * DRAFT_EDGE_STEP).max(DRAFT_EDGE_FLOOR);
    (u64::from(edge) * u64::from(edge) * 2 <= u64::from(base_long) * u64::from(base_long)).then_some(edge)
}

/// One draft, on the worker: the reduced copy of `full` at `edge` — cut here
/// when `cut` brings none — developed as the canvas's frame is, and handed
/// back as a frame OF `full`: its identity is the canvas's base, and it says
/// what size the full frame has so the canvas lays the picture out there.
/// Film edge and source frame are the full develop's own, so the Detail
/// passes land at the same scale in the photograph.
pub(crate) fn build_draft(
    full: Arc<image::DynamicImage>,
    cut: Option<Arc<image::DynamicImage>>,
    edge: u32,
    recipe: EditRecipe,
    show_clipping: bool,
    film_short_edge: Option<u32>,
    source: autoshade::render::SourceFrame,
) -> DraftDone {
    // The sampler every other reduced copy of a base is cut with: a float
    // base stays float, an 8-bit one 8-bit.
    let reduced = cut.unwrap_or_else(|| Arc::new(autoshade::render::thumbnail(&full, edge, edge)));
    let (rw, rh) = (reduced.width().max(1) as f32, reduced.height().max(1) as f32);
    let mut done = build_preview(reduced.clone(), recipe, show_clipping, film_short_edge, source);
    // The frame's own size against its base's, carried to the full base: the
    // full base's size exactly when the develop keeps its base's (every
    // recipe that does not straighten), within a pixel of the straightened
    // full frame's otherwise.
    let (dw, dh) = (done.rgb.width() as f32, done.rgb.height() as f32);
    done.stands_for = Some(egui::vec2(full.width() as f32 * (dw / rw), full.height() as f32 * (dh / rh)));
    done.base = full;
    (done, edge, reduced)
}

impl PreviewDone {
    /// This frame with the tag of its dispatch.
    pub(crate) fn tagged(mut self, tag: FrameTag) -> Self {
        self.tag = tag;
        self
    }
}

impl Draft {
    /// Note the recipe: a change since the last look is an edit, now.
    fn observe(&mut self, recipe: &EditRecipe, now: Instant) {
        if self.seen.as_ref() != Some(recipe) {
            self.seen = Some(recipe.clone());
            self.last_stir = Some(now);
        }
        // The reduced copy of a base that is gone is let go with it.
        if self.reduced.as_ref().is_some_and(|(of, ..)| of.strong_count() == 0) {
            self.reduced = None;
        }
    }

    /// Is an edit gesture running at `now`: a pointer held down, or the
    /// canvas moved inside the settle time?
    fn in_gesture(&self, now: Instant, pointer_down: bool, settle: Duration) -> bool {
        pointer_down || self.last_stir.is_some_and(|t| now.saturating_duration_since(t) < settle)
    }

    /// The time left at `now` until the canvas has been still for `settle`.
    fn until_settled(&self, now: Instant, settle: Duration) -> Option<Duration> {
        self.last_stir.map(|t| settle.saturating_sub(now.saturating_duration_since(t)))
    }

    /// Is a draft of this base in flight?
    fn inflight_on(&self, base: &Arc<image::DynamicImage>) -> bool {
        self.inflight && self.drafted.as_ref().is_some_and(|(of, _)| names(of, base))
    }

    /// What the last full develop of `base` cost, in milliseconds.
    pub(crate) fn full_cost_of(&self, base: &Arc<image::DynamicImage>) -> Option<f32> {
        self.full_cost.as_ref().filter(|(of, _)| names(of, base)).map(|(_, ms)| *ms)
    }

    /// A full develop of `base` took this long.
    pub(crate) fn note_full_cost(&mut self, base: &Arc<image::DynamicImage>, took: Duration) {
        self.full_cost = Some((Arc::downgrade(base), took.as_secs_f32() * 1000.0));
    }

    /// A draft of `recipe` on `base` at `edge` is being dispatched: returns
    /// the reduced copy when it has been cut already.
    pub(crate) fn dispatching(
        &mut self,
        base: &Arc<image::DynamicImage>,
        edge: u32,
        recipe: &EditRecipe,
    ) -> Option<Arc<image::DynamicImage>> {
        self.inflight = true;
        self.drafted = Some((Arc::downgrade(base), recipe.clone()));
        self.reduced.as_ref().filter(|(of, at, _)| *at == edge && names(of, base)).map(|(_, _, pixels)| pixels.clone())
    }

    /// Was the last draft dispatched for this base and this recipe?
    fn drafted_is(&self, base: &Arc<image::DynamicImage>, recipe: &EditRecipe) -> bool {
        self.drafted.as_ref().is_some_and(|(of, r)| names(of, base) && r == recipe)
    }

    /// Does the canvas show the FULL frame of this recipe on this base?
    fn shows_full_of(&self, base: &Arc<image::DynamicImage>, recipe: &EditRecipe) -> bool {
        self.on_canvas.as_ref().is_some_and(|(of, r, full)| *full && names(of, base) && r == recipe)
    }

    /// A frame went on the canvas (`show_frame`).
    pub(crate) fn frame_shown(
        &mut self,
        base: &Arc<image::DynamicImage>,
        recipe: EditRecipe,
        tag: &FrameTag,
        stands_for: Option<egui::Vec2>,
    ) {
        self.shown_seq = self.shown_seq.max(tag.seq);
        self.on_canvas = Some((Arc::downgrade(base), recipe, stands_for.is_none()));
        self.stands_for = stands_for;
    }
}

impl AutoShadeApp {
    /// The card a frame dispatched now is for.
    fn card_tag(&self) -> Option<(PathBuf, String)> {
        let photo = self.src_path.clone()?;
        Some((photo, self.active_variant().map(|v| v.id.clone()).unwrap_or_default()))
    }

    /// The tag of the next dispatch.
    pub(crate) fn frame_tag(&mut self) -> FrameTag {
        self.draft.seq += 1;
        FrameTag { card: self.card_tag(), seq: self.draft.seq }
    }

    /// The settle time of the photo on the canvas ([`settle_time`]).
    fn settle(&self) -> Duration {
        settle_time(self.base_preview.as_ref().and_then(|base| self.draft.full_cost_of(base)))
    }

    /// Is an edit gesture running on the canvas at `now`?
    fn gesture_running(&self, now: Instant) -> bool {
        let pointer_down = self.egui_ctx.input(|i| i.pointer.any_down());
        self.draft.in_gesture(now, pointer_down, self.settle())
    }

    /// Which develop the canvas is owed at `now` (module head). Also where an
    /// edit is noticed, and where the wake-up for the settle is asked for: a
    /// gesture that ends with no further input would otherwise leave its
    /// draft on the canvas until the next mouse move.
    pub(crate) fn plan_develop(&mut self, now: Instant) -> DevelopPlan {
        self.draft.observe(&self.recipe, now);
        let Some(base) = self.base_preview.as_ref() else { return DevelopPlan::Full };
        let long = base.width().max(base.height());
        let Some(edge) = self.draft.full_cost_of(base).and_then(|ms| draft_edge(long, ms)) else {
            return DevelopPlan::Full;
        };
        let pointer_down = self.egui_ctx.input(|i| i.pointer.any_down());
        let settle = self.settle();
        if !self.draft.in_gesture(now, pointer_down, settle) {
            // The full develop waits for this photo's draft in flight: beside
            // each other both are slow, and the draft is the frame the hand
            // is waiting for. Its landing asks again.
            return if self.draft.inflight_on(base) { DevelopPlan::Wait } else { DevelopPlan::Full };
        }
        if !self.draft.drafted_is(base, &self.recipe) && !self.draft.shows_full_of(base, &self.recipe) {
            return DevelopPlan::Draft(edge);
        }
        // A held pointer wakes the window itself when it is let go.
        if !pointer_down && let Some(left) = self.draft.until_settled(now, settle) {
            self.egui_ctx.request_repaint_after(left);
        }
        DevelopPlan::Wait
    }

    /// May a frame that is not the full develop of the live recipe go on the
    /// canvas at `now`? (That it is a frame of the canvas's base is the
    /// caller's check.) It must be this card's and newer than what is shown;
    /// be the live recipe's own draft, or a frame a running gesture has moved
    /// past; and not be a draft of the very recipe whose full frame is there.
    pub(crate) fn passing_frame_shows(&self, frame: &PreviewDone, now: Instant) -> bool {
        frame.tag.card == self.card_tag()
            && frame.tag.seq > self.draft.shown_seq
            && (frame.recipe == self.recipe || self.gesture_running(now))
            && !self.draft.shows_full_of(&frame.base, &frame.recipe)
    }

    /// Put a passing frame on the canvas: the picture, its histogram and its
    /// clipping layer. `develop_count` and the mask overlay's refresh wait
    /// for the full frame of the live recipe, and so does the card's
    /// thumbnail — this frame's is dropped with it. The canvas moved at `now`:
    /// the settle is counted from here, so the full develop leaves the hand
    /// the time to answer what it has just been shown.
    pub(crate) fn show_passing_frame(&mut self, ctx: &egui::Context, frame: PreviewDone, now: Instant) {
        self.draft.passing_count += 1;
        self.draft.last_stir = Some(now);
        let _ = self.show_frame(ctx, frame);
        ctx.request_repaint();
    }

    /// A draft landed at `now`. It never touches `develop_inflight` and never
    /// clears `dirty`: the full develop keeps its own guard and its own debt.
    pub(crate) fn finish_draft_at(&mut self, ctx: &egui::Context, done: anyhow::Result<DraftDone>, now: Instant) {
        self.draft.inflight = false;
        let (frame, edge, reduced) = match done {
            Ok(done) => done,
            // Only a caught panic lands here; the full develop still follows.
            Err(e) => return self.preview_develop_failed(&e),
        };
        ctx.request_repaint();
        // Another photo, another card's pixels, another resolution: nothing
        // of it is shown and nothing of it is kept.
        if !self.base_preview.as_ref().is_some_and(|base| Arc::ptr_eq(base, &frame.base)) {
            return;
        }
        self.draft.reduced = Some((Arc::downgrade(&frame.base), edge, reduced));
        if self.recipe != frame.recipe {
            // Ask the scheduler again even when the edit that moved the
            // recipe never set `dirty` (`finish_redevelop_at`'s own rule).
            self.dirty = true;
        }
        if self.passing_frame_shows(&frame, now) {
            self.show_passing_frame(ctx, frame, now);
        }
    }
}
