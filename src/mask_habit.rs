//! The photographer's LOCAL-WORK habit — what their masks DO, as summary
//! statistics a prompt can state (step 14 / S3).
//!
//! # Why the index learned this
//!
//! Before this module the style index read TWELVE GLOBAL SLIDERS off each
//! neighbour's `.xmp` (`style::REF_KEYS`), a curve shape and a colour family
//! summary, and nothing else: `mask` did not appear in `style.rs` once. So the
//! reference block could tell the proposer how this photographer exposes and
//! grades a whole frame, and nothing at all about how they work a sky, a
//! subject or a foreground — while the proposer is told, in generic prose it
//! has for every user alike, to "add 1-2 local masks"
//! (`advisor::openai`). The user's ruling (2026-08-30): *the mask data in the
//! xmp matters too, not only the document-level numbers.*
//!
//! # What this is NOT
//!
//! **No geometry is averaged, ever.** A mask's coordinates are a fact about
//! ONE frame — this sky ends at this horizon — and the mean of four
//! photographs' gradients is a gradient that belongs to none of them. Nothing
//! here interpolates, blends or emits a coordinate: the output is a COUNT per
//! use, a weighted MEAN of eight sliders, and English. The proposer places its
//! own masks on the frame in front of it.
//!
//! Retrieval never reads any of it — see
//! `retrieval_does_not_read_mask_habits`. A habit changes nothing about which
//! neighbours are chosen, which is why it could ship without an index-version
//! bump.
//!
//! What it DOES change is two things: the WORDS in the reference block, and —
//! since batch 2 — where `style::blend_toward` pulls a mask's SLIDERS. The
//! amplitudes only. The rule at the top of this file is unchanged and is
//! enforced from the other side too
//! (`style::distillation_never_moves_mask_geometry`): no coordinate is
//! averaged, read or written, and no mask is created, deleted, muted or
//! re-weighted. The proposer still places every mask itself.

use serde::{Deserialize, Serialize};

use crate::recipe::{LocalAdjustment, MaskGeometry};

/// The ten sliders a habit is summarised over, in canonical order, spelled
/// with the labels `style::REF_KEYS` already uses for the global half of the
/// block — so "shadows" means one thing in both sentences.
///
/// A curated subset, like `REF_KEYS` and for the same reason: these are the
/// moves a photographer makes HABITUALLY through a mask. The rest of
/// [`LocalAdjustment`]'s twenty-odd knobs (texture, sharpness, hue, noise
/// reduction) are per-photo repairs, and a mean over four neighbours would be
/// mush.
///
/// `temperature` and `tint` joined in B5 (user ruling 2026-08-30: *感觉还是要更加
/// 高效的使用蒙版吧？包括色温色调和曲线，都要学会使用*). They were in the
/// "per-photo repair" bucket by assumption and the assumption was wrong: a
/// photographer who cools a sky and leaves the foreground warm is doing it
/// through the mask, every time, and it is exactly the move the proposer never
/// made because nothing in the block or the prompt had ever named it. NOTE the
/// spelling: the LOCAL `temperature` is a RELATIVE ±100 shift, not the global
/// `temperature_K`'s absolute Kelvin — same word, different quantity, and this
/// is the one row where the "same label means the same thing" rule above had to
/// give way to the engine's own field name (`recipe::LocalAdjustment`).
///
/// A local tone CURVE is the third thing the ruling names and it is NOT here,
/// because it is not a slider: a curve is a point list, and the mean of four
/// photographs' curves is a curve none of them drew (the module header's rule).
/// It is counted instead — [`MaskHabit::curved`].
pub const HABIT_SLIDERS: [&str; 10] = [
    "exposure", "highlights", "shadows", "whites", "blacks", "clarity", "dehaze", "saturation",
    "temperature", "tint",
];

/// Width of [`BucketHabit::mean`], spelled once.
pub const N_HABIT_SLIDERS: usize = HABIT_SLIDERS.len();

/// At most this many sliders are named in ONE clause of the reference note.
///
/// Not a display nicety — a budget. The whole reference block is truncated at
/// 4,096 bytes by `advisor::BoundedUntrustedText`, and it is already the size
/// of four neighbour lines each carrying up to `style::MAX_DESC_CHARS` of
/// prose. Three sliders is the actionable core of a dodge-or-burn ("exposure,
/// and the two tone ends it was made with"); eight would triple the clause for
/// the two least-moved values in it. `local_work_note_fits_its_bound` measures
/// the worst case this produces.
pub const HABIT_SLIDERS_SHOWN: usize = 3;

/// Hard bound on the note this module contributes to the reference block,
/// PROVED rather than enforced: every byte of the note comes from constants
/// here and from numbers the index door has already clamped, so its length is
/// bounded by construction. `local_work_note_fits_its_bound` builds the worst
/// case and measures it, which is the check a runtime truncation would only
/// hide.
///
/// Re-derived in B5, which added THREE things to the note: two longer slider
/// names (`temperature`, `tint` — and the worst case is now the clause that
/// shows the three LONGEST names, not merely three names), a curve clause, and
/// the in-mask colour pointer. `local_work_note_fits_its_bound` measures the
/// widest note this can build at 679 characters (the longest-named clause;
/// the every-axis-moved one is 655). The bound goes 640 → 768 — 12 x 64,
/// the same 64-character grid 640 sat on — leaving 89 characters of
/// headroom: enough that rewording a clause is not silently a budget change,
/// small enough that growing the note again forces the measurement to be
/// re-run. It is spent against the same 4,096-byte
/// `advisor::REFERENCE_BUDGET_BYTES` the block as a whole must clear
/// (`style::the_local_work_note_fits_the_proposers_budget`).
pub const MAX_LOCAL_WORK_CHARS: usize = 768;

/// Below this a slider's mean does not survive its own rounding, and printing
/// it would claim a habit of `+0`.
const EXPOSURE_FLOOR: f32 = 0.05;
const SLIDER_FLOOR: f32 = 0.5;

/// Which USE a mask is put to — the only classification this module makes.
///
/// Five values, and `Other` is a real answer rather than a failure: a brush
/// group, a raster mask or an AI *Object/Background* selection is local work
/// whose PURPOSE the sidecar does not state, and inventing one would be the
/// roundness rule broken (`recipe::MaskGeometry::AiMask::subtype` — Object and
/// Background share the value 0 and Lightroom does not separate them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// The sky, held back from above.
    Sky,
    /// The subject, lifted out of its surroundings.
    Subject,
    /// The foreground / the surround, worked from below or from outside in.
    Ground,
    /// A mask whose only identity is a Range Mask refinement — see
    /// [`bucket_of`] for why this comes LAST and not first.
    Range,
    /// Local work whose purpose the sidecar does not state.
    Other,
}

impl Bucket {
    /// Every variant, so a `for` loop over the buckets cannot silently miss one.
    pub const ALL: [Bucket; 5] =
        [Bucket::Sky, Bucket::Subject, Bucket::Ground, Bucket::Range, Bucket::Other];
}

/// WHICH USE, from one local adjustment — a pure function of the mask alone.
///
/// # The rules, and the order they are applied in
///
/// 1. An **AI mask** answers directly: `MaskSubType` 2 is Sky and 1 is Subject
///    (`recipe::MaskGeometry::AiMask::subtype`, decoded over 105 real
///    instances). Subtype 0 falls through to [`Bucket::Other`] on purpose —
///    it means *Object* OR *Background*, the sidecar does not separate them,
///    and a mask over the background is the opposite of a mask over the
///    subject.
/// 2. A **linear gradient** is read by which END OF THE FRAME it covers.
///    `mask_weight` gives weight 0 at `zero` and 1 at `full` (render.rs), and
///    the normalised frame is y-DOWN, so a gradient whose `full` end is ABOVE
///    its `zero` end covers the top: a sky. Below it: the ground. Purely
///    horizontal (`full_y == zero_y`) is neither and lands in `Other`.
///    [`LocalAdjustment::inverted`] REVERSES the covered end (render.rs applies
///    `1 - w` to the whole combined coverage), so it is XORed in rather than
///    ignored — 8 of the 192 gradient components in the calibration library's
///    applied corrections set it (`target/style-s3/corpus-census.txt`).
/// 3. A **radial** is a subject when it covers its own ellipse and a
///    foreground/surround when it is inverted — a vignette holds back
///    everything the ellipse does not cover. 41 of 186 radial components in
///    that library carry `MaskInverted="true"`.
/// 4. Only then a **Range Mask**. It is last, not first, and that is the one
///    ordering decision in this file worth stating: Lightroom's Range Mask is
///    always a REFINEMENT intersected with a geometry
///    ([`LocalAdjustment::range`] sits BESIDE `mask`, it does not replace it),
///    and the advisor prompt itself recommends exactly that pairing. Ranking
///    it first would take the 14 most carefully refined corrections in the
///    calibration library out of `sky`/`subject`/`ground` and file them under
///    a bucket that says nothing about WHERE they were placed — losing the
///    actionable half. So `Range` catches only the masks whose geometry says
///    nothing, and the "do they use range masks" question is answered by
///    [`MaskHabit::refined`], which counts EVERY refined mask.
pub fn bucket_of(a: &LocalAdjustment) -> Bucket {
    match &a.mask {
        MaskGeometry::AiMask { subtype: 2, .. } => return Bucket::Sky,
        MaskGeometry::AiMask { subtype: 1, .. } => return Bucket::Subject,
        MaskGeometry::Linear { zero_y, full_y, .. } => {
            if full_y != zero_y {
                // XOR: `inverted` swaps which end of the gradient is covered.
                return if (full_y < zero_y) != a.inverted { Bucket::Sky } else { Bucket::Ground };
            }
        }
        MaskGeometry::Radial { .. } => {
            return if a.inverted { Bucket::Ground } else { Bucket::Subject };
        }
        _ => {}
    }
    if a.range.is_some() { Bucket::Range } else { Bucket::Other }
}

/// One use's share of a photograph's local work.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct BucketHabit {
    /// How many of this photograph's counted masks are put to this use. Summed
    /// over the five buckets this is exactly [`MaskHabit::count`].
    #[serde(default)]
    pub n: u8,
    /// Total `amount` of the masks that contributed to [`mean`], i.e. the
    /// weight the mean was taken with.
    ///
    /// Carried so the reference block's cross-neighbour mean is EXACT rather
    /// than a mean of means: `sum(w*mean)/sum(w)` is the amount-weighted mean
    /// over every contributing mask of every neighbour, which averaging the
    /// per-photograph means would only approximate (and would get wrong
    /// whenever one neighbour placed four sky masks and another placed one).
    ///
    /// [`mean`]: BucketHabit::mean
    #[serde(default)]
    pub w: f32,
    /// Amount-weighted mean of [`HABIT_SLIDERS`], over the masks in this bucket
    /// that moved at least one of them. `w == 0.0` means the bucket has masks
    /// but none of them is described by these ten sliders, and the note then
    /// states the placement without numbers rather than claiming zeros.
    #[serde(default, deserialize_with = "habit_mean")]
    pub mean: [f32; N_HABIT_SLIDERS],
}

/// [`BucketHabit::mean`] read from an index written before [`HABIT_SLIDERS`]
/// last grew.
///
/// The array is fixed-width in the type and a plain JSON array on disk, so
/// widening the slider set changes the WIRE SHAPE of every stored habit — and
/// `style::StyleIndex::load` parses the whole file BEFORE it reaches the
/// version gate, so a bare `[f32; N]` would turn every S3-era index into
/// `parse style index …: invalid length 8` instead of the actionable "rebuild
/// it" the gate exists to print. That is a worse failure than the one the gate
/// was designed for, and it would arrive without anyone deciding to ship it.
///
/// A SHORT array zero-fills, and zero-filling claims nothing: `local_work_note`
/// prints a slider only when its magnitude clears [`SLIDER_FLOOR`], so an axis
/// that index never measured stays silent rather than reporting a habit of 0.
/// A LONG array is REFUSED — that is an index from a newer build, and silently
/// dropping its tail would be reading the wrong columns.
fn habit_mean<'de, D>(d: D) -> Result<[f32; N_HABIT_SLIDERS], D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Vec::<f32>::deserialize(d)?;
    if v.len() > N_HABIT_SLIDERS {
        return Err(serde::de::Error::invalid_length(
            v.len(),
            &"at most one value per HABIT_SLIDERS entry — this index was written by a newer build",
        ));
    }
    let mut out = [0.0f32; N_HABIT_SLIDERS];
    out[..v.len()].copy_from_slice(&v);
    Ok(out)
}

impl BucketHabit {
    /// Nothing measured — the form that is left out of the serialised index.
    pub fn is_empty(&self) -> bool {
        self.n == 0 && self.w == 0.0 && self.mean.iter().all(|v| *v == 0.0)
    }
}

/// What this photograph's local work looks like, as numbers.
///
/// `Option<MaskHabit>` on the exemplar distinguishes three states, and the
/// third is why the type is not just a count:
///
/// * `None` — NOT MEASURED. A pre-S3 index, or a sidecar that could not be
///   read.
/// * `Some` with `count == 0` — measured, and the photographer worked
///   GLOBALLY on this frame. That is a positive finding and the reference
///   block states it ("they mostly work globally"), which it could not do if
///   the two cases shared a spelling.
/// * `Some` with `count > 0` — measured local work.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct MaskHabit {
    /// Enabled masks carrying a non-zero `amount` — the population every other
    /// number here is taken over. A muted mask (Lightroom's eye toggle) and a
    /// mask at Amount 0 are both work the photographer chose NOT to apply.
    pub count: u8,
    /// How many of those masks carry a Range Mask refinement, whatever bucket
    /// they landed in. See [`bucket_of`] rule 4.
    ///
    /// Counted from BOTH sides of the import, which is not fussiness: a
    /// `Mask/RangeMask` this engine cannot honour is DROPPED on the way in and
    /// reported as `xmp::MaskImportReason::ForeignRangeMask` — the geometry
    /// survives, the refinement does not. On the calibration library the
    /// importer refuses TWELVE and carries NONE, so a count taken from the
    /// surviving recipe alone would have told the proposer "none use range
    /// masks" about a photographer who uses them on twelve corrections. See
    /// [`MaskHabit::of_with_refused_ranges`].
    #[serde(default)]
    pub refined: u8,
    /// How many of the counted masks draw a CURVE inside themselves — any of
    /// `main_curve` / `red_curve` / `green_curve` / `blue_curve`
    /// (`recipe::LocalAdjustment`), whatever bucket they landed in.
    ///
    /// A COUNT and not a mean, for the reason the module header states: a
    /// curve is a point list and the average of four photographs' curves is a
    /// shape none of them drew. What the proposer can act on is that this
    /// photographer reaches for a curve inside a mask at all — the free-form
    /// tonal shape the local Highlights/Shadows sliders cannot draw — which is
    /// the third of the three tools the B5 ruling named.
    ///
    /// Counted from the SURVIVING recipe alone, and that is not the oversight
    /// [`MaskHabit::refined`] had to fix: a local curve ROUND-TRIPS through the
    /// importer (`xmp::xmp_to_recipe` parses all four, `xmp::local_curve_elem`
    /// writes them), so there is no loss channel to consult. The one exception
    /// is a curve whose points fall outside 0..=255, which the importer drops
    /// with a disclosure of its own — that under-states this count, which is
    /// the same conservative direction [`MaskHabit::clamp`] already takes.
    #[serde(default)]
    pub curved: u8,
    #[serde(default, skip_serializing_if = "BucketHabit::is_empty")]
    pub sky: BucketHabit,
    #[serde(default, skip_serializing_if = "BucketHabit::is_empty")]
    pub subject: BucketHabit,
    #[serde(default, skip_serializing_if = "BucketHabit::is_empty")]
    pub ground: BucketHabit,
    #[serde(default, skip_serializing_if = "BucketHabit::is_empty")]
    pub range: BucketHabit,
    #[serde(default, skip_serializing_if = "BucketHabit::is_empty")]
    pub other: BucketHabit,
}

impl MaskHabit {
    /// Summarise one photograph's local adjustments.
    ///
    /// Two populations, deliberately different:
    ///
    /// * [`MaskHabit::count`] and every bucket's `n` count the masks the
    ///   photographer APPLIED (`enabled && amount != 0`), whatever they move —
    ///   a mask that only softens noise is still a mask on the sky.
    /// * `w` and `mean` are taken over the subset that moved one of
    ///   [`HABIT_SLIDERS`], weighted by `amount`. A mask at all eight zeros
    ///   would otherwise drag every mean toward 0 and report a restraint we
    ///   never measured — the same rule `eval::user_family_summary` follows
    ///   when it answers `None` for a photographer who shaped no colour.
    pub fn of(masks: &[LocalAdjustment]) -> MaskHabit {
        MaskHabit::of_with_refused_ranges(masks, 0)
    }

    /// [`MaskHabit::of`] plus the refinements the IMPORTER itself dropped.
    ///
    /// `refused` is how many of this photograph's corrections raised
    /// `xmp::MaskImportReason::ForeignRangeMask` — a Range Mask in an encoding
    /// this engine does not model, dropped explicitly rather than imported as
    /// something we had understood. Those corrections still arrive with their
    /// geometry, so they are already in the right bucket; what is missing is
    /// only the fact that they were refined, and the loss channel is where
    /// that fact still exists.
    ///
    /// KNOWN IMPRECISION: the loss channel does not say whether the correction
    /// was ENABLED, so a refusal on a muted mask can push `refined` past
    /// [`MaskHabit::count`]. [`MaskHabit::clamp`] caps it there, which is the
    /// conservative direction — the count can under-state, never over-state.
    pub fn of_with_refused_ranges(masks: &[LocalAdjustment], refused: usize) -> MaskHabit {
        let mut h =
            MaskHabit { refined: refused.min(u8::MAX as usize) as u8, ..Default::default() };
        let mut acc = [[0.0f64; N_HABIT_SLIDERS]; Bucket::ALL.len()];
        for m in masks.iter().filter(|m| m.enabled && m.amount != 0.0) {
            h.count = h.count.saturating_add(1);
            if m.range.is_some() {
                h.refined = h.refined.saturating_add(1);
            }
            if !m.main_curve.is_empty()
                || !m.red_curve.is_empty()
                || !m.green_curve.is_empty()
                || !m.blue_curve.is_empty()
            {
                h.curved = h.curved.saturating_add(1);
            }
            let b = bucket_of(m);
            let slot = Bucket::ALL.iter().position(|x| *x == b).expect("ALL covers every bucket");
            let bucket = h.bucket_mut(b);
            bucket.n = bucket.n.saturating_add(1);
            let v = sliders_of(m);
            if v.iter().all(|x| *x == 0.0) {
                continue;
            }
            bucket.w += m.amount;
            for (i, x) in v.iter().enumerate() {
                acc[slot][i] += (*x as f64) * (m.amount as f64);
            }
        }
        for (slot, b) in Bucket::ALL.iter().enumerate() {
            let bucket = h.bucket_mut(*b);
            if bucket.w > 0.0 {
                let w = bucket.w as f64;
                for (mean, sum) in bucket.mean.iter_mut().zip(acc[slot].iter()) {
                    *mean = (sum / w) as f32;
                }
            }
        }
        h
    }

    /// The bucket, by value — the read side of `bucket_mut`.
    pub fn bucket(&self, b: Bucket) -> BucketHabit {
        match b {
            Bucket::Sky => self.sky,
            Bucket::Subject => self.subject,
            Bucket::Ground => self.ground,
            Bucket::Range => self.range,
            Bucket::Other => self.other,
        }
    }

    fn bucket_mut(&mut self, b: Bucket) -> &mut BucketHabit {
        match b {
            Bucket::Sky => &mut self.sky,
            Bucket::Subject => &mut self.subject,
            Bucket::Ground => &mut self.ground,
            Bucket::Range => &mut self.range,
            Bucket::Other => &mut self.other,
        }
    }

    /// Every number finite — the `exemplar_is_finite` rule: a NaN serialises as
    /// `null` and makes the whole index unloadable.
    pub fn is_finite(&self) -> bool {
        Bucket::ALL.iter().all(|b| {
            let x = self.bucket(*b);
            x.w.is_finite() && x.mean.iter().all(|v| v.is_finite())
        })
    }

    /// Bound every number to its band at the index door, exactly as
    /// `eval::FamilySummary::clamp` does: this reaches a paid prompt, and an
    /// index file is disk input.
    ///
    /// The bands are the ENGINE's own (`recipe::EditRecipe::clamp`): exposure
    /// ±5 EV, the other seven ±100, `amount` 0..=1 so a bucket's weight cannot
    /// exceed its mask count. A bucket with no masks is emptied outright — a
    /// mean with no population behind it is a claim, not a measurement — and
    /// no bucket may claim more masks than the exemplar counted.
    pub fn clamp(&mut self) {
        let cap = self.count;
        self.refined = self.refined.min(cap);
        self.curved = self.curved.min(cap);
        for b in Bucket::ALL {
            let x = self.bucket_mut(b);
            x.n = x.n.min(cap);
            if x.n == 0 {
                *x = BucketHabit::default();
                continue;
            }
            x.w = if x.w.is_finite() { x.w.clamp(0.0, x.n as f32) } else { 0.0 };
            for (i, v) in x.mean.iter_mut().enumerate() {
                let (lo, hi) = if i == 0 { (-5.0, 5.0) } else { (-100.0, 100.0) };
                *v = if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
            }
            if x.w == 0.0 {
                x.mean = [0.0; N_HABIT_SLIDERS];
            }
        }
    }
}

/// The ten tracked sliders of one adjustment, in [`HABIT_SLIDERS`] order.
fn sliders_of(a: &LocalAdjustment) -> [f32; N_HABIT_SLIDERS] {
    [
        a.exposure_ev,
        a.highlights,
        a.shadows,
        a.whites,
        a.blacks,
        a.clarity,
        a.dehaze,
        a.saturation,
        a.temperature,
        a.tint,
    ]
}

/// Index of `temperature` in [`HABIT_SLIDERS`]; `tint` is the one after it.
/// Named so the in-mask colour pointer below and the array cannot drift apart.
const I_TEMPERATURE: usize = 8;

/// How each bucket is DESCRIBED to the proposer: the placement it should make,
/// and the verb for what that placement does. Constants, never a file name and
/// never a coordinate — see the module header.
fn phrasing(b: Bucket) -> Option<(&'static str, &'static str)> {
    match b {
        Bucket::Sky => Some(("mask the sky", "linear from the top, or an AI sky selection")),
        Bucket::Subject => Some(("lift the subject", "radial, or an AI subject selection")),
        Bucket::Ground => {
            Some(("work the foreground", "linear from the bottom, or an inverted radial"))
        }
        // Range is answered by the refinement sentence, and Other is local work
        // whose purpose the sidecar never stated — neither can be turned into
        // "place a mask HERE", which is the only thing a clause is for.
        Bucket::Range | Bucket::Other => None,
    }
}

/// The strongest sliders of an aggregate mean, as the note prints them.
fn slider_phrase(mean: &[f32; N_HABIT_SLIDERS]) -> String {
    let mut shown: Vec<usize> = (0..N_HABIT_SLIDERS)
        .filter(|&i| mean[i].abs() >= if i == 0 { EXPOSURE_FLOOR } else { SLIDER_FLOOR })
        .collect();
    // Strongest first, ties keeping the canonical order (`sort_by` is stable),
    // then back into canonical order so the sentence reads the same way every
    // time regardless of which three survived.
    shown.sort_by(|a, b| mean[*b].abs().total_cmp(&mean[*a].abs()).then_with(|| a.cmp(b)));
    shown.truncate(HABIT_SLIDERS_SHOWN);
    shown.sort_unstable();
    shown
        .iter()
        .map(|&i| {
            if i == 0 {
                format!("{} {:+.1} EV", HABIT_SLIDERS[i], mean[i])
            } else {
                format!("{} {:+.0}", HABIT_SLIDERS[i], mean[i])
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `THEIR TYPICAL LOCAL WORK` — the reference block's local-work note, or the
/// EMPTY STRING when no retrieved neighbour was measured.
///
/// `habits` is one entry per retrieved neighbour, in the block's own order;
/// `None` entries are neighbours from a pre-S3 index and are excluded from
/// every denominator, so an old index produces the S2 block byte for byte
/// (`reference_local_work_note_is_absent_when_no_neighbour_carries_masks`).
///
/// `bold` is the Style >= 0.85 axis the rest of the block already switches on:
/// at that strength the retrieved shots are the TARGET, so the note reads as a
/// floor rather than a ceiling.
pub fn local_work_note(habits: &[Option<MaskHabit>], bold: bool) -> String {
    let measured: Vec<&MaskHabit> = habits.iter().flatten().collect();
    let m = measured.len();
    if m == 0 {
        return String::new();
    }
    let aim = if bold {
        "treat this as your FLOOR — place at least these masks."
    } else {
        "place similar masks, not stronger."
    };
    let worked = measured.iter().filter(|h| h.count > 0).count();
    if worked == 0 {
        return format!(
            "  THEIR TYPICAL LOCAL WORK: none of the {m} similar shots carries a mask — they \
             work globally; leave `masks` empty unless the scene needs it."
        );
    }
    let mut clauses: Vec<String> = Vec::new();
    for b in Bucket::ALL {
        let Some((verb, shape)) = phrasing(b) else { continue };
        let k = measured.iter().filter(|h| h.bucket(b).n > 0).count();
        if k == 0 {
            continue;
        }
        // The EXACT amount-weighted mean over every contributing mask of every
        // measured neighbour — see `BucketHabit::w`.
        let w: f64 = measured.iter().map(|h| h.bucket(b).w as f64).sum();
        let numbers = if w > 0.0 {
            let mut mean = [0.0f32; N_HABIT_SLIDERS];
            for (i, slot) in mean.iter_mut().enumerate() {
                let s: f64 = measured
                    .iter()
                    .map(|h| {
                        let x = h.bucket(b);
                        x.w as f64 * x.mean[i] as f64
                    })
                    .sum();
                *slot = (s / w) as f32;
            }
            slider_phrase(&mean)
        } else {
            String::new()
        };
        clauses.push(if numbers.is_empty() {
            format!("{k} of {m} {verb} ({shape})")
        } else {
            format!("{k} of {m} {verb} ({shape}: {numbers})")
        });
    }
    let refined = measured.iter().filter(|h| h.refined > 0).count();
    clauses.push(if refined == 0 {
        "none use range masks".to_string()
    } else {
        format!("{refined} of {m} refine a mask with a range mask")
    });
    // B5's third tool. Stated only when it happened — "none draw a curve" is a
    // sentence about a photographer who may simply have had no reason to, and
    // the block already spends its budget on things the proposer can act on.
    let curved = measured.iter().filter(|h| h.curved > 0).count();
    if curved > 0 {
        clauses.push(format!("{curved} of {m} draw a tone curve INSIDE the mask"));
    }
    // Stated only when it is the MAJORITY reading: a single global neighbour
    // among four is not "they mostly work globally".
    let global = if (m - worked) * 2 > m {
        " They mostly work globally — leave `masks` empty unless the scene needs it."
    } else {
        ""
    };
    // B5: when this photographer's own masks carry COLOUR or a curve, say so as
    // an instruction and not only as a number. The measured mean already prints
    // `temperature -18` when it is among the strongest three, but a number is
    // evidence and this is the guidance — and the whole point of the ruling is
    // that the proposer had never been told these live inside a mask at all.
    let shapes_colour = Bucket::ALL.iter().any(|b| {
        measured.iter().any(|h| {
            let x = h.bucket(*b);
            x.w > 0.0
                && x.mean[I_TEMPERATURE..].iter().any(|v| v.abs() >= SLIDER_FLOOR)
        })
    });
    // A POINTER, not a tutorial. What these controls ARE and how they
    // behave is `advisor::openai::mask_colour_clause`, which the proposer
    // reads unconditionally and at length. Saying it twice spent the
    // block's 4,096-byte budget on a duplicate, and it showed: with this
    // sentence at its first length an adversarial maximal block cleared
    // `advisor::REFERENCE_BUDGET_BYTES` by nothing. This states the one
    // thing only the HABIT knows, which is that this photographer does it.
    let inside = match (shapes_colour, curved > 0) {
        (false, false) => "",
        (true, false) => {
            " They work COLOUR inside the mask, not just brightness — use its own \
`temperature` / `tint`."
        }
        (false, true) => {
            " They shape TONE inside the mask with its own `main_curve`, not sliders alone."
        }
        (true, true) => {
            " They work COLOUR and TONE inside the mask, not just brightness — use its own \
`temperature` / `tint` and `main_curve`."
        }
    };
    format!(
        "  THEIR TYPICAL LOCAL WORK ({worked} of {m} similar shots carry masks): {} — {aim}{global}{inside}",
        clauses.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::RangeMask;

    /// A gradient with `zero` at `zero_y` and `full` at `full_y`, both on the
    /// frame's vertical centre line — the y-DOWN normalised frame, so a smaller
    /// y is HIGHER in the picture.
    fn gradient(zero_y: f32, full_y: f32) -> MaskGeometry {
        MaskGeometry::Linear { zero_x: 0.5, zero_y, full_x: 0.5, full_y }
    }

    fn radial() -> MaskGeometry {
        MaskGeometry::Radial {
            top: 0.3,
            left: 0.3,
            bottom: 0.7,
            right: 0.7,
            feather: 0.5,
            roundness: 0.0,
            flipped: false,
            angle: 0.0,
            midpoint: 50.0,
            mask_version: 2,
        }
    }

    fn ai(subtype: u32) -> MaskGeometry {
        MaskGeometry::AiMask {
            name: String::new(),
            subtype,
            ref_x: 0.5,
            ref_y: 0.5,
            blend_mode: 0,
            value: 1.0,
            inverted: false,
            mask_version: 1,
            provenance: Vec::new(),
            gesture: Vec::new(),
            raster: None,
        }
    }

    fn brush() -> MaskGeometry {
        MaskGeometry::Brush {
            name: "Brush 1".into(),
            blend_mode: 0,
            value: 1.0,
            inverted: false,
            strokes: Vec::new(),
        }
    }

    /// One adjustment carrying `geometry`, with a real exposure move so it is
    /// never mistaken for an all-zero mask.
    fn mask(geometry: MaskGeometry) -> LocalAdjustment {
        LocalAdjustment { mask: geometry, exposure_ev: -0.5, ..Default::default() }
    }

    fn luminance_range() -> RangeMask {
        RangeMask::Luminance { lo_outer: 0.4, lo: 0.6, hi: 1.0, hi_outer: 1.0 }
    }

    // ---- the bucket rules, one named test each ------------------------------

    /// Rule 1a: `MaskSubType="2"` is Lightroom's Sky selection.
    #[test]
    fn bucket_rules_send_an_ai_sky_selection_to_sky() {
        assert_eq!(bucket_of(&mask(ai(2))), Bucket::Sky);
    }

    /// Rule 1b: `MaskSubType="1"` is Subject.
    #[test]
    fn bucket_rules_send_an_ai_subject_selection_to_subject() {
        assert_eq!(bucket_of(&mask(ai(1))), Bucket::Subject);
    }

    /// Rule 1c: subtype 0 means Object OR Background and the sidecar does not
    /// separate them (`recipe::MaskGeometry::AiMask::subtype`). A mask over the
    /// BACKGROUND is the opposite of a mask over the subject, so guessing would
    /// tell the proposer to place the wrong mask half the time.
    #[test]
    fn bucket_rules_leave_an_ai_object_selection_unclassified() {
        assert_eq!(bucket_of(&mask(ai(0))), Bucket::Other);
    }

    /// Rule 2a: `full` above `zero` covers the TOP of the frame — a sky.
    #[test]
    fn bucket_rules_send_a_linear_from_the_top_to_sky() {
        assert_eq!(bucket_of(&mask(gradient(0.81, -0.02))), Bucket::Sky);
    }

    /// Rule 2b: the mirror image is the ground.
    #[test]
    fn bucket_rules_send_a_linear_from_the_bottom_to_ground() {
        assert_eq!(bucket_of(&mask(gradient(0.2, 1.02))), Bucket::Ground);
    }

    /// Rule 2c: `inverted` swaps which end is covered (render.rs applies
    /// `1 - w` to the whole combined coverage), so the SAME coordinates read as
    /// the opposite use. Reading the coordinates alone would file 8 of the 203
    /// gradients in the calibration library upside down.
    #[test]
    fn bucket_rules_read_a_linear_through_its_inversion() {
        let sky = mask(gradient(0.81, -0.02));
        assert_eq!(bucket_of(&sky), Bucket::Sky);
        let inverted = LocalAdjustment { inverted: true, ..sky.clone() };
        assert_eq!(bucket_of(&inverted), Bucket::Ground, "inverting a sky gradient covers the ground");
        let ground = mask(gradient(0.2, 1.02));
        assert_eq!(bucket_of(&LocalAdjustment { inverted: true, ..ground }), Bucket::Sky);
    }

    /// Rule 2d: a purely horizontal gradient is neither end of the frame.
    #[test]
    fn bucket_rules_leave_a_horizontal_linear_unclassified() {
        assert_eq!(bucket_of(&mask(gradient(0.5, 0.5))), Bucket::Other);
    }

    /// Rule 3a: an upright radial covers its own ellipse — the subject.
    #[test]
    fn bucket_rules_send_an_upright_radial_to_subject() {
        assert_eq!(bucket_of(&mask(radial())), Bucket::Subject);
    }

    /// Rule 3b: an inverted radial is a vignette — everything the ellipse does
    /// NOT cover, i.e. the surround.
    #[test]
    fn bucket_rules_send_an_inverted_radial_to_ground() {
        let m = LocalAdjustment { inverted: true, ..mask(radial()) };
        assert_eq!(bucket_of(&m), Bucket::Ground);
    }

    /// Rule 4: a Range Mask on a geometry that says nothing is the one case
    /// `Range` catches.
    #[test]
    fn bucket_rules_send_a_range_only_mask_to_range() {
        let m = LocalAdjustment { range: Some(luminance_range()), ..mask(brush()) };
        assert_eq!(bucket_of(&m), Bucket::Range);
    }

    /// …and rule 4's ORDER, which is the one ordering decision in this file: a
    /// refined sky gradient is still a sky gradient. Ranking `Range` first would
    /// take the most carefully refined corrections in the library out of the
    /// buckets that say WHERE they were placed.
    #[test]
    fn bucket_rules_keep_a_refined_gradient_in_its_geometric_bucket() {
        let m = LocalAdjustment { range: Some(luminance_range()), ..mask(gradient(0.81, -0.02)) };
        assert_eq!(bucket_of(&m), Bucket::Sky);
        // …and the refinement is still COUNTED, by the field that answers the
        // "do they use range masks" question for every bucket at once.
        let h = MaskHabit::of(&[m]);
        assert_eq!((h.count, h.refined, h.sky.n), (1, 1, 1));
    }

    /// Rule 5: local work whose purpose the sidecar never stated.
    #[test]
    fn bucket_rules_send_a_brush_group_to_other() {
        assert_eq!(bucket_of(&mask(brush())), Bucket::Other);
        assert_eq!(bucket_of(&mask(MaskGeometry::Bitmap { path: "a.png".into() })), Bucket::Other);
    }

    // ---- the statistics -----------------------------------------------------

    /// The mean is weighted by `amount`, and a mask that moved none of the
    /// eight sliders contributes nothing to it — while still being COUNTED as
    /// a mask, because it is one.
    ///
    /// MUTATION THIS KILLS: dropping the `amount` factor (the mean becomes the
    /// unweighted -1.0), or letting the all-zero mask into the weight (the mean
    /// is diluted toward 0 and the index claims a restraint nobody measured).
    #[test]
    fn mask_habit_means_are_amount_weighted_and_ignore_zero_masks() {
        let strong = LocalAdjustment {
            mask: gradient(0.8, 0.0),
            amount: 1.0,
            exposure_ev: -1.5,
            ..Default::default()
        };
        let faint = LocalAdjustment {
            mask: gradient(0.8, 0.0),
            amount: 0.25,
            exposure_ev: -0.5,
            ..Default::default()
        };
        // Enabled, non-zero amount, and every tracked slider at zero: a mask
        // that (say) only softens noise. Counted, never averaged.
        let inert = LocalAdjustment {
            mask: gradient(0.8, 0.0),
            amount: 1.0,
            noise_reduction: 40.0,
            ..Default::default()
        };
        let h = MaskHabit::of(&[strong, faint, inert]);
        assert_eq!(h.count, 3, "three masks were applied");
        assert_eq!(h.sky.n, 3, "all three are sky gradients");
        assert_eq!(h.sky.w, 1.25, "only the two that moved a slider carry weight");
        // (-1.5*1.0 + -0.5*0.25) / 1.25 = -1.3
        assert!(
            (h.sky.mean[0] - (-1.3)).abs() < 1e-5,
            "amount-weighted, zero-mask-free: {}",
            h.sky.mean[0]
        );
        // The unweighted mean of the two would be -1.0 and the mean of all
        // three -0.666…; neither is what this reports.
        assert!((h.sky.mean[0] - (-1.0)).abs() > 0.2);
    }

    /// `count` is the population the photographer APPLIED: a muted mask
    /// (Lightroom's eye toggle) and a mask at Amount 0 are work they chose not
    /// to apply, and every bucket's `n` sums back to it.
    #[test]
    fn mask_habit_counts_only_the_masks_that_were_applied() {
        let masks = vec![
            LocalAdjustment { mask: gradient(0.8, 0.0), exposure_ev: -1.0, ..Default::default() },
            LocalAdjustment { mask: radial(), enabled: false, exposure_ev: 1.0, ..Default::default() },
            LocalAdjustment { mask: radial(), amount: 0.0, exposure_ev: 1.0, ..Default::default() },
            LocalAdjustment { mask: brush(), exposure_ev: 0.5, ..Default::default() },
        ];
        let h = MaskHabit::of(&masks);
        assert_eq!(h.count, 2, "the muted mask and the Amount-0 mask are not applied work");
        assert_eq!(h.subject.n, 0, "…so neither radial reaches a bucket");
        let summed: u32 = Bucket::ALL.iter().map(|b| h.bucket(*b).n as u32).sum();
        assert_eq!(summed, h.count as u32, "every counted mask lands in exactly one bucket");
    }

    /// A photograph the build MEASURED and found no local work on is
    /// `count == 0`, not `None`: the two are different facts and the reference
    /// block states only the first one.
    #[test]
    fn a_measured_photograph_with_no_masks_is_not_the_same_as_unmeasured() {
        let h = MaskHabit::of(&[]);
        assert_eq!(h.count, 0);
        assert_eq!(h, MaskHabit::default());
        assert_eq!(local_work_note(&[None, None], false), "", "unmeasured says nothing");
        let note = local_work_note(&[Some(h), Some(h)], false);
        assert!(note.contains("none of the 2 similar shots carries a mask"), "{note}");
    }

    /// The index door: every number bounded to the ENGINE's own bands, a bucket
    /// with no masks emptied, and no bucket allowed to claim more masks than the
    /// exemplar counted. An index file is disk input that reaches a paid prompt.
    #[test]
    fn a_hand_edited_habit_is_bounded_at_the_door() {
        let mut h = MaskHabit {
            count: 2,
            refined: 9,
            sky: BucketHabit { n: 200, w: 500.0, mean: [900.0; N_HABIT_SLIDERS] },
            subject: BucketHabit { n: 0, w: 7.0, mean: [42.0; N_HABIT_SLIDERS] },
            ..Default::default()
        };
        assert!(h.is_finite());
        h.clamp();
        assert_eq!(h.refined, 2, "no more refined masks than masks");
        assert_eq!(h.sky.n, 2, "no bucket claims more masks than the exemplar has");
        assert_eq!(h.sky.w, 2.0, "amount is 0..=1, so the weight cannot exceed the count");
        assert_eq!(h.sky.mean[0], 5.0, "exposure is the engine's own ±5 EV");
        assert_eq!(h.sky.mean[1], 100.0, "the other seven are ±100");
        assert!(h.subject.is_empty(), "a mean with no masks behind it is a claim, not a measurement");
        let nan = MaskHabit { count: 1, sky: BucketHabit { n: 1, w: f32::NAN, ..Default::default() }, ..Default::default() };
        assert!(!nan.is_finite(), "a NaN serialises as null and makes the whole index unloadable");
    }

    // ---- the note -----------------------------------------------------------

    fn sky_habit(exposure: f32) -> MaskHabit {
        MaskHabit::of(&[LocalAdjustment {
            mask: gradient(0.8, 0.0),
            exposure_ev: exposure,
            highlights: -25.0,
            dehaze: 10.0,
            ..Default::default()
        }])
    }

    /// The strength axis the rest of the block already switches on: at
    /// Style >= 0.85 the retrieved shots are the TARGET, so the note is a floor;
    /// below it they are a reference and the note is a ceiling.
    ///
    /// MUTATION THIS KILLS: one wording for both tiers.
    #[test]
    fn reference_local_work_note_wording_follows_strength() {
        let habits = [Some(sky_habit(-0.6))];
        let gentle = local_work_note(&habits, false);
        let bold = local_work_note(&habits, true);
        assert!(gentle.contains("place similar masks, not stronger."), "{gentle}");
        assert!(!gentle.contains("FLOOR"), "{gentle}");
        assert!(bold.contains("treat this as your FLOOR — place at least these masks."), "{bold}");
        assert!(!bold.contains("not stronger"), "{bold}");
        // …and the MEASUREMENT is the same on both sides of the axis: only the
        // aim changes, never the numbers.
        assert!(gentle.contains("1 of 1 mask the sky"), "{gentle}");
        assert!(bold.contains("1 of 1 mask the sky"), "{bold}");
    }

    /// The numbers reach the sentence: placement, then the strongest sliders,
    /// exposure to a tenth of a stop and the rest as integers.
    #[test]
    fn the_local_work_note_states_the_placement_and_its_numbers() {
        let note = local_work_note(&[Some(sky_habit(-0.6)), None, Some(MaskHabit::of(&[]))], false);
        assert!(
            note.contains("1 of 2 mask the sky (linear from the top, or an AI sky selection: \
                           exposure -0.6 EV, highlights -25, dehaze +10)"),
            "{note}"
        );
        assert!(note.contains("(1 of 2 similar shots carry masks)"), "the unmeasured neighbour is in no denominator: {note}");
        assert!(note.contains("none use range masks"), "the negative is stated too: {note}");
        // One of two is not a majority, so the global reading is NOT claimed.
        assert!(!note.contains("mostly work globally"), "{note}");
        let mostly = local_work_note(
            &[Some(sky_habit(-0.6)), Some(MaskHabit::of(&[])), Some(MaskHabit::of(&[]))],
            false,
        );
        assert!(mostly.contains("They mostly work globally"), "{mostly}");
    }

    /// The cross-neighbour mean is taken over MASKS, not over photographs: a
    /// neighbour who placed four sky gradients weighs four times one who placed
    /// one. That is what `BucketHabit::w` is carried for.
    #[test]
    fn the_note_weights_neighbours_by_the_masks_they_placed() {
        let one = MaskHabit::of(&[LocalAdjustment {
            mask: gradient(0.8, 0.0),
            exposure_ev: -2.0,
            ..Default::default()
        }]);
        let three = MaskHabit::of(&vec![
            LocalAdjustment { mask: gradient(0.8, 0.0), exposure_ev: 0.0, highlights: -30.0, ..Default::default() };
            3
        ]);
        let note = local_work_note(&[Some(one), Some(three)], false);
        // (-2.0*1 + 0.0*3) / 4 = -0.5, not the -1.0 a mean of means would give.
        assert!(note.contains("exposure -0.5 EV"), "{note}");
    }

    /// A bucket with masks but no numbers states the PLACEMENT and stops: a
    /// mask that only softens noise is real local work, and printing `+0`s for
    /// it would claim a restraint nobody measured.
    #[test]
    fn a_bucket_with_no_slider_moves_states_the_placement_only() {
        let h = MaskHabit::of(&[LocalAdjustment {
            mask: gradient(0.8, 0.0),
            noise_reduction: 40.0,
            ..Default::default()
        }]);
        let note = local_work_note(&[Some(h)], false);
        assert!(note.contains("1 of 1 mask the sky (linear from the top, or an AI sky selection)"), "{note}");
        assert!(!note.contains("exposure +0.0"), "{note}");
    }

    /// The refinement sentence counts EVERY refined mask, in whatever bucket —
    /// which is the whole reason `refined` exists beside the `Range` bucket.
    ///
    /// MUTATION THIS KILLS: reading the sentence off `Bucket::Range`'s `n`
    /// instead, which reports "none use range masks" for a photographer whose
    /// every sky gradient carries one.
    ///
    /// The second half is the live finding this batch turned up: on the
    /// calibration library the IMPORTER refuses TWELVE Range Masks
    /// (`MaskImportReason::ForeignRangeMask`) and carries none, so a count
    /// taken from the surviving recipe alone is 0 for a photographer who
    /// plainly uses them.
    #[test]
    fn the_range_clause_counts_every_refined_mask() {
        let refined_sky = MaskHabit::of(&[LocalAdjustment {
            range: Some(luminance_range()),
            ..mask(gradient(0.8, 0.0))
        }]);
        assert_eq!(refined_sky.range.n, 0, "premise: it is filed under sky, not range");
        assert_eq!(refined_sky.refined, 1);
        let note = local_work_note(&[Some(refined_sky), Some(sky_habit(-0.6))], false);
        assert!(note.contains("1 of 2 refine a mask with a range mask"), "{note}");
        assert!(!note.contains("none use range masks"), "{note}");
        // …and a refinement the IMPORTER dropped counts too. The recipe below
        // carries no `range` at all — that is exactly what a correction whose
        // Range Mask was refused looks like after import — and the habit must
        // still say the photographer refined it.
        let dropped = MaskHabit::of_with_refused_ranges(&[mask(gradient(0.8, 0.0))], 1);
        assert_eq!(dropped.refined, 1, "the loss channel is the surviving evidence");
        let note = local_work_note(&[Some(dropped)], false);
        assert!(note.contains("1 of 1 refine a mask with a range mask"), "{note}");
        // The count can never exceed the mask count, so it under-states rather
        // than over-states when a refusal lands on a muted correction.
        let mut over = MaskHabit::of_with_refused_ranges(&[mask(gradient(0.8, 0.0))], 9);
        over.clamp();
        assert_eq!(over.refined, 1);
    }

    /// The note is built from constants and clamped numbers ONLY. A mask name is
    /// free text a photographer types — and on this machine it is routinely a
    /// file name — so it must never reach a prompt through this door.
    ///
    /// MUTATION THIS KILLS: naming the mask in its clause.
    #[test]
    fn local_work_note_never_names_a_file() {
        let hostile = MaskHabit::of(&[LocalAdjustment {
            name: "D:/Photography/Raw/2026/ROLL-0042.ARW".into(),
            ..mask(gradient(0.8, 0.0))
        }]);
        let note = local_work_note(&[Some(hostile)], true);
        for needle in ["D:/", "Photography", "ROLL-0042", ".ARW", "\\"] {
            assert!(!note.contains(needle), "{needle:?} reached the prompt: {note}");
        }
        // The habit itself carries no string at all — that is what makes the
        // assertion above structural rather than a spot check.
        let json = serde_json::to_string(&hostile).expect("serialise");
        assert!(!json.contains("ROLL-0042"), "{json}");
    }

    /// B5: the note reports the two things the ruling named and the block had
    /// never carried — white balance INSIDE a mask, and a curve inside a mask.
    ///
    /// User ruling 2026-08-30: *感觉还是要更加高效的使用蒙版吧？包括色温色调和曲线，
    /// 都要学会使用*. Before this batch `HABIT_SLIDERS` held eight tonal
    /// sliders and nothing else, so a photographer who cools every sky through
    /// the mask produced a note that said only "3 of 4 mask the sky (exposure
    /// -0.6 EV, highlights -25)" — and the proposer, reading a prompt that
    /// talked only about dodging and burning, never set a local `temperature`
    /// in its life.
    ///
    /// MUTATION: remove `a.temperature`/`a.tint` from `sliders_of` (the WB
    /// assertions fail), stop counting `curved` (the curve clause vanishes), or
    /// make the in-mask pointer unconditional (the tonal-only case fails).
    #[test]
    fn the_local_work_note_names_the_colour_and_curve_tools_inside_a_mask() {
        use crate::recipe::CurvePoint;
        let sky = |a: LocalAdjustment| LocalAdjustment { mask: gradient(0.8, 0.0), ..a };
        // A photographer who cools the sky and shapes it with a curve.
        let colour_and_curve = MaskHabit::of(&[sky(LocalAdjustment {
            exposure_ev: -0.4,
            temperature: -30.0,
            tint: 8.0,
            main_curve: vec![CurvePoint { input: 0, output: 12 }],
            amount: 1.0,
            enabled: true,
            ..Default::default()
        })]);
        assert_eq!(colour_and_curve.curved, 1, "the local curve is counted");
        let note = local_work_note(&[Some(colour_and_curve)], true);
        // The MEASUREMENT: `temperature` is now one of the sliders a bucket's
        // mean is taken over, and it is the strongest one here.
        assert!(note.contains("temperature -30"), "{note}");
        // The CURVE, as a count — a curve is a point list and a mean of four
        // photographs' curves is a shape none of them drew.
        assert!(note.contains("1 of 1 draw a tone curve INSIDE the mask"), "{note}");
        // …and the GUIDANCE, which is the half a number cannot carry.
        assert!(note.contains("They work COLOUR and TONE inside the mask"), "{note}");
        assert!(note.contains("`temperature` / `tint`"), "{note}");
        assert!(note.contains("`main_curve`"), "{note}");

        // COLOUR alone, and TONE alone, say only what is true.
        let colour_only = MaskHabit::of(&[sky(LocalAdjustment {
            temperature: -30.0,
            amount: 1.0,
            enabled: true,
            ..Default::default()
        })]);
        let n = local_work_note(&[Some(colour_only)], true);
        assert!(n.contains("They work COLOUR inside the mask"), "{n}");
        assert!(!n.contains("draw a tone curve"), "no curve was measured: {n}");
        let curve_only = MaskHabit::of(&[sky(LocalAdjustment {
            exposure_ev: -0.4,
            main_curve: vec![CurvePoint { input: 0, output: 12 }],
            amount: 1.0,
            enabled: true,
            ..Default::default()
        })]);
        let n = local_work_note(&[Some(curve_only)], true);
        assert!(n.contains("They shape TONE inside the mask with its own `main_curve`"), "{n}");
        assert!(!n.contains("They work COLOUR"), "no WB was measured: {n}");

        // A purely TONAL habit is the pre-B5 sentence, with no pointer at all —
        // the note states what was measured and invents no habit.
        let tonal = MaskHabit::of(&[sky(LocalAdjustment {
            exposure_ev: -0.6,
            highlights: -25.0,
            amount: 1.0,
            enabled: true,
            ..Default::default()
        })]);
        let n = local_work_note(&[Some(tonal)], true);
        assert!(!n.contains("inside the mask"), "nothing measured, nothing claimed: {n}");
        assert!(!n.contains("temperature"), "{n}");
        assert_eq!(MaskHabit::of(&[]).curved, 0, "no masks, no curves");
    }

    /// An index written before `HABIT_SLIDERS` grew must still LOAD.
    ///
    /// `style::StyleIndex::load` parses the whole file before it reaches the
    /// version gate, so a bare `[f32; N]` would turn every S3-era index into
    /// `invalid length 8` — a parse error, not the actionable "rebuild it" the
    /// gate prints. The short array zero-fills, and zero claims nothing: the
    /// note prints a slider only above `SLIDER_FLOOR`, so an axis that index
    /// never measured stays silent instead of reporting a habit of 0.
    ///
    /// MUTATION: drop `deserialize_with = "habit_mean"` and the 8-wide record
    /// fails to deserialize; make `habit_mean` accept a LONG array and the last
    /// assertion fails.
    #[test]
    fn a_pre_widening_habit_loads_and_claims_no_white_balance() {
        let json = r#"{"count":2,"refined":0,"sky":{"n":2,"w":1.5,
            "mean":[-0.6,-25.0,0.0,0.0,0.0,0.0,0.0,0.0]}}"#;
        let h: MaskHabit = serde_json::from_str(json).expect("an S3-era habit still loads");
        assert_eq!(h.count, 2);
        assert_eq!(h.curved, 0, "a pre-B5 record measured no curves and claims none");
        assert_eq!(h.sky.mean[I_TEMPERATURE], 0.0, "the unmeasured axes zero-fill");
        assert_eq!(h.sky.mean[0], -0.6, "and the measured ones keep their column");
        let note = local_work_note(&[Some(h)], true);
        assert!(note.contains("exposure -0.6 EV"), "{note}");
        assert!(!note.contains("temperature"), "an unmeasured axis is silent: {note}");
        assert!(!note.contains("inside the mask"), "{note}");

        // A record from a NEWER build is refused rather than read short: the
        // columns would not line up, and reading the wrong ones silently is
        // worse than saying so.
        let wide = r#"{"count":1,"sky":{"n":1,"w":1.0,
            "mean":[0,0,0,0,0,0,0,0,0,0,0]}}"#;
        let e = serde_json::from_str::<MaskHabit>(wide).unwrap_err().to_string();
        assert!(e.contains("newer build"), "{e}");
    }

    /// The BUDGET. `advisor::BoundedUntrustedText` truncates the whole style
    /// reference at 4,096 bytes, and it already carries four neighbour lines
    /// each with up to `style::MAX_DESC_CHARS` of prose. This measures the
    /// worst note this module can produce — every bucket populated on every one
    /// of `style::RETRIEVE_K` neighbours, every slider at its clamped extreme —
    /// against the bound the constant states.
    #[test]
    fn local_work_note_fits_its_bound() {
        let extreme = BucketHabit {
            n: u8::MAX,
            w: 1.0,
            // The widest each field prints: -5.0 EV, and -100 for the nine.
            // The three the clause SHOWS are picked by magnitude with ties
            // keeping canonical order, so with every non-exposure axis tied at
            // -100 the winners are `highlights, shadows, whites` — which is NOT
            // the widest sentence this can build now that `temperature` (11
            // characters) is in the set. `widest` below is the fixture that
            // actually maximises the clause; `extreme` stays as the
            // every-axis-moved case, because the two answer different questions
            // and only measuring one of them is how the bound drifts.
            mean: [-5.0, -100.0, -100.0, -100.0, -100.0, -100.0, -100.0, -100.0, -100.0, -100.0],
        };
        // The LONGEST-NAMED three, forced into the clause by giving only them a
        // magnitude: `highlights` (10) + `saturation` (10) + `temperature` (11)
        // is the widest slider phrase `slider_phrase` can emit. This is the
        // fixture the bound is actually derived from.
        let widest = BucketHabit {
            n: u8::MAX,
            w: 1.0,
            mean: [0.0, -100.0, 0.0, 0.0, 0.0, 0.0, 0.0, -100.0, -100.0, 0.0],
        };
        let build = |b: BucketHabit| MaskHabit {
            count: u8::MAX,
            refined: u8::MAX,
            curved: u8::MAX,
            sky: b,
            subject: b,
            ground: b,
            range: b,
            other: b,
        };
        for (name, note) in [
            ("every axis moved", local_work_note(&vec![Some(build(extreme)); crate::style::RETRIEVE_K], true)),
            ("longest names", local_work_note(&vec![Some(build(widest)); crate::style::RETRIEVE_K], true)),
        ] {
            println!("{name} local-work note: {} chars, bound {MAX_LOCAL_WORK_CHARS}", note.chars().count());
            assert!(
                note.chars().count() <= MAX_LOCAL_WORK_CHARS,
                "the {name} note is {} chars, over its {MAX_LOCAL_WORK_CHARS}-char bound:\n{note}",
                note.chars().count()
            );
        }
        let note = local_work_note(&vec![Some(build(widest)); crate::style::RETRIEVE_K], true);
        // At most three sliders per clause is what makes that bound hold — a
        // ten-slider clause would be ~3x this.
        assert_eq!(HABIT_SLIDERS_SHOWN, 3);
        assert!(note.contains("temperature -100"), "the WB pair reaches the clause: {note}");
        assert!(
            note.matches("saturation").count() <= 3,
            "one clause per placement bucket: {note}"
        );
    }
}
