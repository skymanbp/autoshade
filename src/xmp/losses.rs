//! What a sidecar cannot carry, both ways: mask and global export losses, import losses, render gaps, and the passthrough and look-rendered key sets.

use super::*;

/// Why one mask does not reach a classic ACR sidecar intact. Produced by the
/// WRITER itself, one verdict per mask per defect ([`masks_xml`]) — so no
/// consumer has to re-derive the projection rules, and a disclosure can never
/// claim something different from what was actually emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaskLossReason {
    /// Raster geometry: classic XMP has no encoding for it, so the WHOLE
    /// correction is skipped ([`mask_geom_xml`] returns `None`).
    Bitmap,
    /// The eye toggle is off — the correction is skipped rather than exported
    /// as an active edit the app does not render.
    Disabled,
    /// A Bitmap component has no Lightroom geometry spelling and is omitted.
    /// Linear, Radial, Brush and AiMask components are composed in list order;
    /// this loss counts only corrections that still contain a Bitmap extra.
    ComponentsFlattened,
    /// A brush group (`Mask/Aggregate` + its `Mask/Paint` strokes) rides out
    /// into the sidecar COMPLETE — and the pixels AutoShade showed for it were
    /// drawn by **our** rasteriser from a measured model of Lightroom's brush,
    /// not by Adobe's. The XMP is exact; the alpha is an approximation.
    ///
    /// The odd one out in this enum, deliberately: every other variant names
    /// something the PROJECTION could not carry. This one names something the
    /// projection carries perfectly and the RENDERER reproduces only to within
    /// a measurement, which is the opposite direction of loss and needs saying
    /// in the same breath.
    ///
    /// **This variant used to be `BrushCarried`, and it meant「not drawn at
    /// all」** — weight 0 everywhere, from R27 Batch-4 until R29 Batch-6b. What
    /// changed is that the one missing input arrived: the alpha kernel is not
    /// in the sidecar (`recipe::BrushStroke::dabs` — the file stores the
    /// stroke, never the alpha), so it had to be MEASURED, and R29 Batch-6 did
    /// measure it on 29 controlled Lightroom exports —
    /// `k(ρ;h) = (1 − ρ^m(h))^n(h)` at rms 0.0109 held-out, a one-parameter
    /// flow odds law at κ = 0.1284, screen accumulation, density scaling the
    /// dab. `render::brush_raster` is the implementation. Renaming rather than
    /// keeping both was the honest option: nothing raises「carried, not drawn」
    /// any more, and a disclosure variant with no producer is a claim the code
    /// cannot make (this enum's own header: a disclosure must never say
    /// something other than what was actually emitted).
    ///
    /// What is still worth saying, and is what the label now says: our edges
    /// are not Adobe's. The model reproduces the measured ladder to ~0.01 in α
    /// — an order better than the AI-mask arm's re-derivation, and not zero.
    BrushRendered,
    /// An AI mask (`Mask/Image`) rides out into the sidecar COMPLETE — and the
    /// alpha this engine rendered was **recomputed by our own segmenter**, not
    /// Adobe's, so the XMP Lightroom reads and the pixels AutoShade showed do
    /// not describe the same coverage.
    ///
    /// The second member of [`BrushRendered`]'s odd-one-out class, and the one
    /// where the gap is largest. Both are drawn here by something that is not
    /// Adobe's code, but a brush is drawn from a MEASUREMENT of Adobe's own
    /// rasteriser (R29 Batch-6, ~0.01 in α on the ladder it was fitted to),
    /// while an AI mask is drawn by a different SEGMENTER whose edges have
    /// never been compared to Adobe's at all. The sidecar carries no raster
    /// (current corpus: 105 instances, longest attribute value 55 characters), so
    /// this one is structural and permanent, not a to-do.
    ///
    /// [`BrushRendered`]: MaskLossReason::BrushRendered
    AiMaskRecomputed,
    /// A rotated radial exports as its UNROTATED ellipse. v0.32.0 NARROWED
    /// this to one case: `crs:Angle`'s sign and pivot are measured now and the
    /// projection carries the tilt, so what is left is a document with no
    /// declared frame — the pixel↔normalised fold has no aspect to fold with
    /// ([`FrameAspect`], and see [`mask_geom_xml`]). The payload is the
    /// dropped angle in WHOLE DEGREES, so a disclosure can say how much
    /// rotation the sidecar is missing instead of only that some is (R25 P5).
    /// Rounding is the display's, not the model's: `recipe.json` keeps the
    /// exact `f32`, and a half-degree tilt no reader could act on rounds to
    /// `0` — which the prose channels read as "no angle worth naming" and
    /// answer with their plain phrasing.
    Rotation(i32),
    /// Per-channel recolour gains (`color_gains`) are engine-only: classic ACR
    /// has no counterpart, so the sidecar renders without them.
    Recolour,
    /// A raster this mask rides on — a bitmap tile, a zone alpha — could not
    /// be embedded in the sidecar's AutoShade payload (v1.3.1): the file was
    /// unreadable, or it would have pushed the document past
    /// [`payload::RASTER_BUDGET`]. The recipe still names it, so a store that
    /// has the file renders the mask; a store that does not renders it inert
    /// and is told why. Lightroom never saw either; this is about what the
    /// sidecar can give BACK to AutoShade.
    ///
    /// [`payload::RASTER_BUDGET`]: payload::RASTER_BUDGET
    RasterNotEmbedded,
}

impl MaskLossReason {
    /// Every reason, in the order the prose channels group them (skips before
    /// degradations) — the ONE list both disclosure surfaces iterate
    /// ([`describe_mask_losses`] and the GUI's `xmp_loss_line`).
    ///
    /// Those two used to carry a hand-written array each. [`en`]'s exhaustive
    /// match stops the BUILD when a variant is added, but an iteration array
    /// does not: the new reason would be raised by the writer, counted by
    /// nobody and printed by neither surface. Pinned by
    /// `mask_loss_reason_all_covers_every_variant`, whose own exhaustive match
    /// is where a new variant lands next.
    ///
    /// [`Rotation`] appears with a ZERO payload, exactly as the import twin's
    /// `InertLocal` appears with an empty one: the payload is one mask's
    /// angle, so no single value can stand for the variant. Grouping is by
    /// [`same_kind`], never by `==`.
    ///
    /// [`en`]: MaskLossReason::en
    /// [`Rotation`]: MaskLossReason::Rotation
    /// [`same_kind`]: MaskLossReason::same_kind
    pub const ALL: [MaskLossReason; 8] = [
        MaskLossReason::Bitmap,
        MaskLossReason::Disabled,
        MaskLossReason::ComponentsFlattened,
        MaskLossReason::BrushRendered,
        MaskLossReason::AiMaskRecomputed,
        MaskLossReason::Rotation(0),
        MaskLossReason::Recolour,
        MaskLossReason::RasterNotEmbedded,
    ];

    /// Same VARIANT, payload ignored — the grouping key both prose channels
    /// use, mirroring [`MaskImportReason::same_kind`]. Two masks rotated by
    /// different amounts are one line in a sentence and two values under `==`,
    /// and `ALL`'s placeholder payload matches neither of them.
    pub fn same_kind(self, other: MaskLossReason) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }

    /// English label for the prose channel (CLI stderr / web reply). The GUI
    /// renders the same variants in the UI language instead.
    pub fn en(self) -> &'static str {
        match self {
            MaskLossReason::Bitmap => "bitmap mask(s) skipped",
            MaskLossReason::Disabled => "muted mask(s) skipped",
            MaskLossReason::ComponentsFlattened => "bitmap component(s) omitted",
            MaskLossReason::BrushRendered => {
                "brush mask(s) drawn from AutoShade's measured model of Lightroom's brush - \
                 not Adobe's own rasteriser"
            }
            MaskLossReason::AiMaskRecomputed => {
                "AI mask(s) re-derived by the local segmenter - not Adobe's own raster"
            }
            MaskLossReason::Rotation(_) => "radial rotation dropped",
            MaskLossReason::Recolour => "recolour gains dropped",
            MaskLossReason::RasterNotEmbedded => {
                "mask raster(s) not embedded in the sidecar's AutoShade payload (unreadable, or \
                 over the size budget)"
            }
        }
    }
}

/// One mask defect the XMP projection could not carry. A single mask can
/// appear more than once (a rotated radial with components and recolour gains
/// loses three separate things).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskLoss {
    /// The name the sidecar uses for this correction (`crs:CorrectionName`):
    /// the user's own label when set, else the generated `AutoShade <n>`.
    pub name: String,
    pub reason: MaskLossReason,
}

/// Why one Lightroom correction does not reach this engine WHOLE — the
/// IMPORT-side twin of [`MaskLossReason`], produced by the READER itself
/// ([`classify_correction`]) so no consumer has to re-derive the import rules
/// and a disclosure can never claim something other than what was imported.
///
/// **The asymmetry this closes (R25 P1).** The export side has named its
/// losses per mask since M6a; the import side answered with a single integer
/// and SIX independent "if I do not recognise this, drop the whole correction"
/// gates. Every one of those gates fired on an ordinary Lightroom file:
/// `crs:Angle` is written on EVERY radial (as `"0"` when unrotated) and
/// `crs:MaskBlendMode` on EVERY component. The result was that every mask in
/// the user's catalog was refused on import, with a count as the only
/// explanation. A knob we do not model is not the same thing as a value we
/// cannot read, and only the second one is a reason to refuse a correction.
///
/// Two variants DROP the correction ([`is_drop`]); the other eight are notes
/// on a correction that DID import.
///
/// [`is_drop`]: MaskImportReason::is_drop
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaskImportReason {
    /// DROP. No geometry this engine can stand on: an AI / depth component
    /// (`Mask/Image`), a component nested somewhere this reader cannot account
    /// for, or no `crs:CorrectionMasks` at all.
    ///
    /// NARROWED TWICE, and the list it used to carry is worth stating
    /// correctly because two of its four members were never right:
    ///  * `Mask/Aggregate` and `Mask/Paint` LEFT in R27 Batch-4 — a brush
    ///    group is a first-class geometry now ([`MaskGeometry::Brush`]),
    ///    imported, carried and written back, with [`BrushRendered`] as its
    ///    note. It is not a drop and has not been one since.
    ///  * `Mask/Ellipse` was never a member in the first place. It is the
    ///    spot-healing shape and lives under `crs:RetouchAreas` ONLY — 84 of
    ///    84 instances in the reference library, never inside a correction —
    ///    so no correction has ever been refused for one.
    ///
    /// What genuinely lands here is `Mask/Image`: Lightroom stores the INTENT
    /// and recomputes the alpha from a model, so the sidecar holds no pixels
    /// and no geometry to take.
    ///
    /// [`BrushRendered`]: MaskImportReason::BrushRendered
    /// [`MaskGeometry::Brush`]: crate::recipe::MaskGeometry::Brush
    Unrepresentable,
    /// DROP. The values READ fine but land outside this engine's model (an
    /// exposure past ±5 EV, `crs:CorrectionActive="false"`, a component whose
    /// coordinates are not numbers, more masks than the recipe cap holds).
    OutOfModel,
    /// `crs:Angle` is non-zero and the radial still imports as its UNROTATED
    /// ellipse — mirrored from the export side's [`MaskLossReason::Rotation`].
    ///
    /// The REASON narrowed in v0.32.0 and this line moved with it: the sign,
    /// the pivot and the magnitude are all measured now (`ANGLE-MODEL.md`
    /// §6.1), and [`lr_to_engine`] carries the tilt through. The variant fires
    /// only when the DOCUMENT DECLARES NO FRAME — no `tiff:ImageWidth /
    /// ImageLength`, so the pixel→normalised fold has no aspect to fold with
    /// ([`FrameAspect`], [`RadialDecode::Unrotated`]) — or when the attribute
    /// is present but unreadable. The payload is the sidecar's angle in WHOLE
    /// DEGREES, or `0` when there was no angle to name (unreadable still
    /// counts as rotated: we cannot say it is zero), or it rounds away.
    Rotation(i32),
    /// `crs:MaskBlendMode` is not the plain composition we already do, so the
    /// component contributes its base geometry only.
    BlendMode,
    /// Historical import disclosure retained for stored diagnostics. Native
    /// parametric, brush and AI components now arrive in document order and
    /// no longer raise it; an unmodelled shape has its own refusal instead.
    MultiComponent,
    /// A `Mask/Aggregate` brush group imported WHOLE — strokes, dab streams,
    /// group blend mode and all — and **drawn here by our own rasteriser**,
    /// from a measured model of Lightroom's brush rather than Adobe's code.
    ///
    /// R27 Batch-4 (L-08) made it importable. Before that, a correction holding
    /// one of these was refused entire under [`Unrepresentable`], which cost
    /// the photographer not only the brush but every gradient standing beside
    /// it: 18 corrections and 14 already-drawable parametric shapes across the
    /// reference library, thrown away because a neighbouring component was a
    /// brush.
    ///
    /// **R29 Batch-6b then made it RENDER, and this variant was renamed from
    /// `BrushCarried` because「carried, not drawn」stopped being true.** The one
    /// input a renderer needs that no sidecar contains is the alpha kernel
    /// (`recipe::BrushStroke::dabs`), so it was measured instead of guessed:
    /// 29 controlled Lightroom exports, `k(ρ;h) = (1 − ρ^m(h))^n(h)` held-out
    /// at rms 0.0109, `D(f) = κf/(1−f+κf)` with κ = 0.1284, screen
    /// accumulation, density scaling the dab (R29 Batch-6; `render::brush_raster`).
    ///
    /// Raised on the IMPORT itself, exactly like [`AiMaskRecomputed`]: the
    /// photographer is being told what KIND of thing arrived, and「the alpha
    /// will be ours, from a measurement」is true the moment the group is read.
    /// A note, not a drop — the correction and its neighbours arrive.
    ///
    /// [`Unrepresentable`]: MaskImportReason::Unrepresentable
    /// [`AiMaskRecomputed`]: MaskImportReason::AiMaskRecomputed
    BrushRendered,
    /// A `Mask/Image` AI mask imported as INTENT and **re-derived on this
    /// machine by a different segmenter** — the dominant refusal before R27
    /// Batch-5, and the one arm that cannot be closed by any parser.
    ///
    /// The current corpus measures 105 instances: the component carries `MaskSubType` +
    /// `ReferencePoint` + `MaskName`, optional gesture region hints, provenance
    /// digests, and the proxy frame Adobe's model ran in — **no raster payload**.
    /// So there is no alpha to import in the sense the other variants mean.
    /// What lands is a RECOMPUTATION: our own subject / sky / point-prompted
    /// segmenter produces its own alpha, which will differ from Adobe's at every
    /// edge; subtype-0 gesture dabs now join its positive point prompt.
    ///
    /// A note, not a drop, and that is the whole gain: 78 corrections across 40
    /// files — 40 % of every file in the reference library that has a mask at
    /// all — were refused entire because of one of these, taking 52
    /// engine-drawable parametric shapes with them.
    ///
    /// When the segmenter has not run or declined, the mask renders INERT and
    /// [`AiMaskUnresolved`] says so instead — two different sentences for two
    /// different states, because "approximated" and "not drawn" are not the
    /// same news.
    ///
    /// [`AiMaskUnresolved`]: MaskImportReason::AiMaskUnresolved
    AiMaskRecomputed,
    /// An AI mask arrived but has NO alpha: the segmentation sidecar has not
    /// run for this photo yet, or it ran and declined. The mask contributes
    /// nothing and the correction's other shapes render normally.
    ///
    /// Separate from [`AiMaskRecomputed`] on purpose. That one says "these
    /// pixels are ours, not Adobe's"; this one says "there are no pixels".
    /// Collapsing them would let a failed model run read as a successful
    /// approximation, which is the one confusion this whole arm exists to
    /// avoid.
    ///
    /// [`AiMaskRecomputed`]: MaskImportReason::AiMaskRecomputed
    AiMaskUnresolved,
    /// A `Mask/RangeMask` we cannot honour (someone else's encoding, or more
    /// than one) — the geometry imports without the range refinement.
    ForeignRangeMask,
    /// A `crs:MainCurve` / `RedCurve` / `GreenCurve` / `BlueCurve` is present
    /// and UNREADABLE: the element never closes, or one of its points is not
    /// an `x,y` pair inside 0..255. The correction imports WITHOUT that curve
    /// — the geometry is still exactly what the file draws, so this costs the
    /// curve and not the mask (the same verdict [`ForeignRangeMask`] gets).
    ///
    /// R25 P1 raised this for every correction that carried a local curve at
    /// all, because the engine modelled none of them and they were vanishing
    /// with no note. R25 P6 models all four (`LocalAdjustment::main_curve` …),
    /// so what remains is the narrower, still-real case above: a knob we do
    /// not model and a value we cannot read are different things (see this
    /// enum's header), and only the second one is a loss once the knob exists.
    ///
    /// [`ForeignRangeMask`]: MaskImportReason::ForeignRangeMask
    LocalCurve,
    /// `crs:LocalCurveRefineSaturation` is off its 100 default.
    CurveRefineSaturation,
    /// A `crs:Local*` slider this engine has no model for carries a non-zero
    /// value; the payload is that slider's key.
    InertLocal(&'static str),
    /// A `crs:Local*` attribute name this build has never seen.
    UnknownLocalKey,
}

impl MaskImportReason {
    /// Every reason, drops before notes — the ONE list both disclosure
    /// surfaces iterate ([`describe_import_losses`] and the GUI's
    /// `xmp_import_line`), exactly as [`MaskLossReason::ALL`] serves the
    /// export half. [`en`]'s exhaustive match stops the BUILD when a variant
    /// is added; an iteration array does not, so a reason raised by the
    /// reader and missing here would be counted by nobody. Pinned by
    /// `mask_import_reason_all_covers_every_variant`.
    ///
    /// [`InertLocal`] appears with an EMPTY payload: the payload names one
    /// slider, so no single value can stand for the variant. Grouping is by
    /// [`same_kind`] (the discriminant), never by `==`.
    ///
    /// [`en`]: MaskImportReason::en
    /// [`InertLocal`]: MaskImportReason::InertLocal
    /// [`same_kind`]: MaskImportReason::same_kind
    pub const ALL: [MaskImportReason; 13] = [
        MaskImportReason::Unrepresentable,
        MaskImportReason::OutOfModel,
        MaskImportReason::Rotation(0),
        MaskImportReason::BlendMode,
        MaskImportReason::MultiComponent,
        MaskImportReason::BrushRendered,
        MaskImportReason::AiMaskRecomputed,
        MaskImportReason::AiMaskUnresolved,
        MaskImportReason::ForeignRangeMask,
        MaskImportReason::LocalCurve,
        MaskImportReason::CurveRefineSaturation,
        MaskImportReason::InertLocal(""),
        MaskImportReason::UnknownLocalKey,
    ];

    /// Did this verdict cost the whole correction? The two `true` answers are
    /// what [`unsupported_corrections`] counts, so "imported + refused" stays
    /// the size of the user's local work (`eval`'s reading) even now that a
    /// correction can import AND carry notes.
    pub fn is_drop(self) -> bool {
        matches!(self, MaskImportReason::Unrepresentable | MaskImportReason::OutOfModel)
    }

    /// Same VARIANT, payload ignored — the grouping key both prose channels
    /// use, because `InertLocal("LocalGrain")` and `InertLocal("LocalMoire")`
    /// are one line in a sentence and two values under `==`.
    pub fn same_kind(self, other: MaskImportReason) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }

    /// English label for the prose channel (CLI stderr / batch warnings). The
    /// GUI renders the same variants in the UI language instead.
    pub fn en(self) -> &'static str {
        match self {
            MaskImportReason::Unrepresentable => "AI / brush correction(s) skipped",
            MaskImportReason::OutOfModel => "correction(s) beyond this engine's model skipped",
            MaskImportReason::Rotation(_) => "radial rotation(s) read as 0",
            MaskImportReason::BlendMode => "non-default blend mode(s) ignored",
            MaskImportReason::MultiComponent => "extra shape component(s) dropped",
            MaskImportReason::BrushRendered => {
                "brush mask(s) drawn from AutoShade's measured model of Lightroom's brush - \
                 not Adobe's own rasteriser"
            }
            MaskImportReason::AiMaskRecomputed => {
                "AI mask(s) re-derived by the local segmenter - not Adobe's own raster"
            }
            MaskImportReason::AiMaskUnresolved => {
                "AI mask(s) carried but not yet re-derived - the local segmenter has not run"
            }
            MaskImportReason::ForeignRangeMask => "range mask(s) dropped",
            MaskImportReason::LocalCurve => "local point curve(s) unreadable",
            MaskImportReason::CurveRefineSaturation => "curve refine saturation not modelled",
            MaskImportReason::InertLocal(_) => "unmodelled local slider(s)",
            MaskImportReason::UnknownLocalKey => "unknown local setting(s)",
        }
    }
}

/// One import defect, NAMED. A single correction can appear more than once (a
/// rotated radial whose blend mode is Subtract loses two separate things) —
/// the import twin of [`MaskLoss`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskImportLoss {
    /// `crs:CorrectionName` when the sidecar sets one, else the positional
    /// `Correction <n>` — the same "say WHICH one" rule the export side's
    /// [`MaskLoss::name`] follows.
    pub name: String,
    pub reason: MaskImportReason,
}

/// What this sidecar's mask corrections cost on the way in, named — the
/// import-side counterpart of [`mask_export_losses`]. Empty = every
/// correction arrived whole (or there were none).
pub fn import_losses(xmp: &str) -> Vec<MaskImportLoss> {
    // The two gates the reader itself stands behind
    // (`xmp_to_recipe_clamped_impl`): under a foreign namespace binding it
    // imports NOTHING, and `unparsable_crs_numbers` says so in one sentence.
    // Naming per-correction defects on top of that — read through the very
    // prefix the gate declared unreliable — reported masks skipped for
    // reasons that were never the reason.
    if xmp.len() > MAX_XMP_BYTES || xmlns_conflict(xmp).is_some() {
        return Vec::new();
    }
    let authored_by_autoshade = is_autoshade_sidecar(xmp);
    let scope = crs_own_scope(xmp);
    // The FRAME is read off the whole document — `tiff:` properties live
    // outside the crs Description's own scope in principle, and the decode
    // needs them (see `FrameAspect`).
    mask_summary(scope.as_ref(), authored_by_autoshade, FrameAspect::from_xmp(xmp)).losses
}

/// [`import_losses`] with the photo identity needed to resolve a sibling ACR
/// MaskBrushTable — and, since 2026-09-24, to place an orientation-only
/// document's geometry in the frame it was written in. The structured loss
/// channel is already user-visible, so this re-read is silent; the recipe
/// import emits any named table refusal.
pub fn import_losses_for_photo(xmp: &str, photo: &std::path::Path) -> Vec<MaskImportLoss> {
    if xmp.len() > MAX_XMP_BYTES || xmlns_conflict(xmp).is_some() {
        return Vec::new();
    }
    let authored_by_autoshade = is_autoshade_sidecar(xmp);
    let scope = crs_own_scope(xmp);
    // The frame the recipe reader decodes in, photograph included
    // (`photo_frame_fallback`): an orientation-only document's rotated
    // radial is a rotation that imports, not a loss to report.
    mask_summary_with_source(
        scope.as_ref(),
        authored_by_autoshade,
        FrameAspect::from_xmp(xmp).or_else(|| photo_frame_fallback(xmp, Some(photo))),
        Some(photo),
        None,
    )
    .losses
}

/// Did this sidecar's document have Lightroom's LENS PROFILE CORRECTION
/// switched on? `None` when the document says nothing.
///
/// Read for exactly one purpose (R29 Batch-3): the mask-warp frame. The whole
/// difference between the frame a mask was STORED in and the frame it was
/// EXPORTED into is Lightroom's lens correction, so `crs:LensProfileEnable="0"`
/// means the two frames are the SAME and an identity warp is the right answer —
/// not a missing one. [`crate::recipe::MaskWarpSource::DisabledInSidecar`] is
/// where that distinction is recorded, and
/// [`crate::pipeline::fresh_lens_profile_for_sidecar`] is what applies it.
///
/// This does NOT touch what this engine renders. AutoShade's own geometry stage
/// is driven by the photographer's `lens_profile` toggles, which are theirs to
/// set; reading Lightroom's switch as an instruction would silently overwrite
/// them from a file they may have imported only for its masks.
///
/// Adobe writes `"0"` / `"1"`; `"False"` / `"True"` are accepted too because
/// the surrounding boolean keys in the same namespace use that spelling and a
/// reader that took only one of the two would be right by luck.
pub fn lens_profile_enabled(xmp: &str) -> Option<bool> {
    let scope = crs_own_scope(xmp);
    let raw = Scope::new(scope.as_ref()).crs_str("LensProfileEnable")?;
    match raw.trim() {
        "1" => Some(true),
        "0" => Some(false),
        s if s.eq_ignore_ascii_case("true") => Some(true),
        s if s.eq_ignore_ascii_case("false") => Some(false),
        // A value neither spelling covers is not a "no": saying nothing is
        // honest where guessing would decide a coordinate frame.
        _ => None,
    }
}

/// One English sentence for what an import carried and what it left behind, or
/// `None` when the sidecar's masks arrived whole. Groups by reason in
/// [`MaskImportReason::ALL`] order and names the corrections, so the line is
/// actionable ("which of my 12 masks?") — the import twin of
/// [`describe_mask_losses`].
///
/// BOTH FRONT-ENDS consume this (R27, closing the R25 P9 registration). The
/// GUI reads it through `bin/gui/export.rs` and `bin/gui/persist.rs`; the CLI
/// reads it through `main::lightroom_import_note`, which prints the sentence
/// on stderr beside the `xmp -> …` line of every single-photo command that
/// publishes a projection (`analyze`, `auto`, `match`). Until R27 the CLI had
/// no mask disclosure at all: the losses were computable and nobody computed
/// them.
///
/// STILL NARROWER, and named rather than left to be rediscovered: `eval.rs`
/// uses [`unsupported_corrections`], which counts DROPS only
/// ([`MaskImportReason::is_drop`]) and so cannot see a degradation like
/// `MultiComponent`. That is the eval RULER's own definition — "imported +
/// refused" has to stay the size of the user's local work — not a missing
/// channel, so it is a difference to know about, not a gap to close.
///
/// `batch` prints nothing here on purpose: its work list is
/// `store::has_develop_or_sidecar`-filtered, so a photo that HAS a Lightroom
/// sidecar is never in it.
pub fn describe_import_losses(imported: usize, losses: &[MaskImportLoss]) -> Option<String> {
    if losses.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for reason in MaskImportReason::ALL {
        let names: Vec<&str> = losses
            .iter()
            .filter(|l| l.reason.same_kind(reason))
            .map(|l| l.name.as_str())
            .collect();
        if names.is_empty() {
            continue;
        }
        // Capped like the export sentence, for the same reason: the count is
        // the fact, the first few names are the pointer.
        let shown = names.len().min(4);
        let more = names.len() - shown;
        let list = names[..shown].join(", ");
        let tail = if more > 0 { format!(", +{more} more") } else { String::new() };
        parts.push(format!("{} {} ({list}{tail})", names.len(), reason.en()));
    }
    Some(format!(
        "imported {imported} Lightroom mask(s); {} — the sidecar itself is not modified",
        parts.join("; ")
    ))
}

/// The masks `r` cannot project into a classic ACR sidecar, exactly as the
/// writer judges them while emitting the XML (see [`masks_xml`]) — the ONE
/// source for every surface's export-side disclosure. Empty = a faithful
/// projection.
///
/// For a caller that is also WRITING the document, this is the wrong door: it
/// builds the whole mask block to return the verdict list. Take the pair from
/// [`recipe_to_xmp_with_losses`] / [`MergeOutcome::losses`] instead (R22 NIT-1)
/// — `pipeline::write_xmp` did exactly this and ran the projection twice per
/// save. What is left here is the standalone question ("what would this recipe
/// lose?") with no document wanted; the crate's own tests are its only callers
/// today.
///
/// Judged with NO frame ([`FrameAspect`]), which is the honest answer to a
/// question asked without a photo: a rotated radial counts as a rotation loss
/// here, and the writer that IS given the frame does not lose it. The two
/// cannot disagree about a document, because this one produces none.
pub fn mask_export_losses(r: &EditRecipe) -> Vec<MaskLoss> {
    masks_xml(r, None).1
}

/// One English sentence naming what the sidecar left behind, or `None` when
/// nothing was lost. Groups by reason in [`MaskLossReason`] order and names
/// the masks, so the line is actionable ("which of my 12 masks?").
pub fn describe_mask_losses(losses: &[MaskLoss]) -> Option<String> {
    if losses.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for reason in MaskLossReason::ALL {
        let names: Vec<&str> = losses
            .iter()
            .filter(|l| l.reason.same_kind(reason))
            .map(|l| l.name.as_str())
            .collect();
        if names.is_empty() {
            continue;
        }
        // Names are already length-capped by `EditRecipe::clamp`, but a
        // 64-mask recipe would still make an unreadable line — the count is
        // the fact, the first few names are the pointer.
        let shown = names.len().min(4);
        let more = names.len() - shown;
        let list = names[..shown].join(", ");
        let tail = if more > 0 { format!(", +{more} more") } else { String::new() };
        parts.push(format!("{} {} ({list}{tail})", names.len(), reason.en()));
    }
    Some(format!(
        "the Lightroom XMP does not carry: {} — recipe.json keeps all of it",
        parts.join("; ")
    ))
}

/// **Export-side disclosure, GLOBAL half** (R24-5 M0): the develop controls
/// this recipe is CARRYING that the sidecar cannot express — an active
/// control the engine renders and `owned_attrs` has no `crs:` property for.
///
/// The mask half of this story has existed since M6a ([`MaskLoss`]); the
/// global half never did, so a photo whose look depends on its camera base
/// curve or its lens-profile correction exported a sidecar that renders
/// visibly differently in Lightroom, silently. Reopened in AutoShade it is
/// fine — recipe.json keeps everything — which is exactly why the loss went
/// unnoticed.
///
/// DERIVED, not listed: the members are the registry's `RenderedNotExported`
/// rows ([`crate::advisor::catalogue::Tier`]), and "is it active" is a serde
/// comparison against a default recipe — so moving a control between tiers
/// (the B2–B5 batches) updates this disclosure by itself, and a new
/// unexportable control cannot be added without appearing here.
pub fn global_export_losses(r: &EditRecipe) -> Vec<&'static str> {
    use crate::advisor::catalogue::{Tier, RECIPE_CONTROLS};
    let (Ok(live), Ok(neutral)) =
        (serde_json::to_value(r), serde_json::to_value(EditRecipe::default()))
    else {
        return Vec::new();
    };
    RECIPE_CONTROLS
        .iter()
        .filter(|c| c.tier == Some(Tier::RenderedNotExported))
        .filter(|c| live.get(c.name) != neutral.get(c.name))
        .map(|c| c.name)
        .collect()
}

/// One English sentence naming the GLOBAL controls in `globals` (what
/// [`global_export_losses`] returned) that the sidecar leaves behind, or
/// `None` when there are none — the CLI and web counterpart of the GUI's
/// localised loss line, with the same labels. A control without a label here
/// is named by its registry name rather than dropped.
pub fn describe_global_losses(globals: &[&str]) -> Option<String> {
    if globals.is_empty() {
        return None;
    }
    let names: Vec<&str> = globals
        .iter()
        .map(|g| match *g {
            "base_curve" => "camera base curve",
            "lens_profile" => "lens profile correction",
            "colour_field" => "colour field",
            "pixel_layers" => "fill layers",
            "lens_blur" => "lens blur",
            "retouch" => "spot removal",
            other => other,
        })
        .collect();
    Some(format!(
        "the Lightroom XMP does not carry: {} — recipe.json keeps all of it",
        names.join(", ")
    ))
}

/// **The MIRROR of [`global_export_losses`]** (R25 B4): the controls this
/// document carries that LIGHTROOM renders and this engine does not.
///
/// The disclosure story had one half. `RenderedNotExported` — we render it,
/// the sidecar cannot carry it — has been named since R24-5 M0. The opposite
/// corner had no surface at all, and R25's B2/B3 batches filled it with
/// twenty-four members: a photographer who imports a Lightroom grain, a
/// post-crop vignette or a colour-noise setting sees a canvas that is missing
/// them, and until now nothing on screen said so. `ARCHITECTURE.md` calls a
/// slider that moves a number and no pixel "the worst kind of bug here"; this
/// is the sentence that keeps the SF4-C whitelist from being exactly that.
///
/// DERIVED from the registry's `CarriedOnly` rows, so a control promoted to
/// `Rendered` leaves this disclosure by itself and a new carried one arrives
/// already named.
///
/// "Active" is measured against [`EditRecipe::default`], NEVER against zero.
/// The de-fringe block's neutral is Adobe's own 30/70/40/60 (R25 B3), so a
/// zero comparison would report every untouched photo as carrying a de-fringe
/// this app does not render — a disclosure that cries wolf on every save is a
/// disclosure nobody reads.
///
/// **`PassThrough` is deliberately NOT here**, though its tier renders nothing
/// either — and the reason is the tier's own definition rather than an
/// oversight. Every other row on this list has a KNOWN neutral, so "active"
/// is a decidable question; a pass-through value is never interpreted, which
/// means we cannot tell `crs:PerspectiveUpright="0"` (a block Lightroom
/// stamps on every file it touches, changing nothing anywhere) from a real
/// Upright correction. Including it would put this sentence on essentially
/// every Lightroom photo, permanently and unactionably, and drown the members
/// that ARE actionable — the same judgement `xmp_loss_interrupts` makes about
/// the base curve. The pass-through disclosure is its own develop-panel
/// section, which shows the nine values themselves and says we never read
/// them. Pinned in both directions by
/// `the_render_gaps_name_what_lightroom_renders_and_this_engine_does_not`.
pub fn global_render_gaps(r: &EditRecipe) -> Vec<&'static str> {
    render_gaps_in(crate::advisor::catalogue::RECIPE_CONTROLS.as_slice(), r)
}

/// The derivation itself, over a GIVEN registry slice.
///
/// Split out in v1.5.0 for one reason: Track F emptied `CARRIED_ONLY_GLOBAL`,
/// so the real registry can no longer exercise this at all — every call now
/// answers with an empty vector, and a test over the real slice would pass
/// however the comparison were broken. The test feeds a synthetic carried row
/// instead, which keeps the MECHANISM proven against the day a real one
/// arrives, rather than leaving a live disclosure with no evidence behind it.
pub(super) fn render_gaps_in(
    controls: &[crate::advisor::catalogue::Control],
    r: &EditRecipe,
) -> Vec<&'static str> {
    use crate::advisor::catalogue::Tier;
    let (Ok(live), Ok(neutral)) =
        (serde_json::to_value(r), serde_json::to_value(EditRecipe::default()))
    else {
        return Vec::new();
    };
    controls
        .iter()
        .filter(|c| c.tier == Some(Tier::CarriedOnly))
        .filter(|c| live.get(c.name) != neutral.get(c.name))
        .map(|c| c.name)
        .collect()
}

/// **Import-side disclosure, GLOBAL half** (R24-5 M0): the `crs:` properties
/// this sidecar carries on its own `rdf:Description` that AutoShade does not
/// model at all — the camera Look, `CameraProfileDigest`,
/// `UprightTransform`. (Global Texture and Grain headed that list until R25
/// B2 modelled them, the whole Defringe block left it in B3, the Transform
/// block and the profile name left in B4 as [`PASSTHROUGH_CRS`], and the
/// Calibration panel, the B&W mixer and the point colours left in v1.5.0 —
/// the list SHRINKING by itself, see the paragraph
/// on the complement below, and the fixture note in
/// `an_imported_sidecar_names_the_globals_the_engine_does_not_render`.)
///
/// The merge PRESERVES all of them (that is what `graft_into` is for), so
/// nothing is destroyed; what was missing is the sentence saying they exist.
/// Until now the only import-side global check was
/// [`unparsable_crs_numbers`], whose universe IS [`owned_attr_keys`] — so a
/// perfectly valid property we simply do not render was invisible to every
/// disclosure surface, and the photo just looked different from Lightroom's
/// render with no explanation on screen.
///
/// The universe is the COMPLEMENT of what we own, so it needs no catalogue of
/// Adobe's property names to rot: the day a batch teaches the engine
/// `crs:Texture`, the key joins `owned_attr_keys` and leaves this list. That
/// day was R25 B2, and it happened with no edit to this function — only to
/// the tests, whose fixtures had used Texture as their unmodelled sample.
///
/// BOTH SPELLINGS, because Lightroom really writes both: the Description's own
/// open-tag ATTRIBUTES, and its top-level PROPERTY-ELEMENT children
/// (`<crs:Texture>+30</crs:Texture>` — the form [`crs_str`] reads and
/// [`merge_recipe_into_xmp`]'s element strip removes, "in plenty of real
/// sidecars"). An attribute-only scan answered EMPTY for an element-form
/// sidecar, which is the shape of a Lightroom catalog export.
///
/// Only THIS Description's own attributes and own top-level children are
/// scanned — mask corrections live inside a child element and have their own
/// disclosure ([`unsupported_corrections`]), a creative Look nests someone
/// else's settings block, and mixing either in would report every
/// `crs:Local*` / baked profile key as an unmodelled global. Quote-aware: a
/// mask name or a `crs:RawFileName` value may contain anything, `crs:Foo=`
/// included.
pub fn unmodelled_global_crs(xmp: &str) -> Vec<String> {
    // Under a foreign `crs:` binding these are not camera-raw properties at
    // all, and the merge that would keep them refuses the document
    // ([`merge_recipe_into_xmp_in_frame_for_photo`]) — the conflict sentence
    // on [`unparsable_crs_numbers`] is the whole disclosure, as for the masks.
    if xmp.len() > MAX_XMP_BYTES || xmlns_conflict(xmp).is_some() {
        return Vec::new();
    }
    let Some(start) = find_crs_description(xmp) else { return Vec::new() };
    let Some((gt, self_closing)) = scan_tag_end(xmp, start) else { return Vec::new() };
    let tag = &xmp[start..=gt];
    // The universe is the complement of what we OWN, and ownership has two
    // halves: the attribute keys the writer emits and the element-only
    // properties that have no attribute spelling at all
    // ([`OWNED_ELEMENT_ONLY`]). Leaving the second half out would have this
    // disclosure name our own tone curves as Lightroom-only properties.
    let owned: std::collections::BTreeSet<String> = owned_attr_keys()
        .into_iter()
        .chain(OWNED_ELEMENT_ONLY.iter().map(|k| (*k).to_string()))
        .collect();
    let mut found: std::collections::BTreeSet<String> = Default::default();
    let b: Vec<char> = tag.chars().collect();
    let mut quote: Option<char> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                i += 1;
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                i += 1;
            }
            None => {
                // `crs:` must start a NAME, i.e. not be preceded by an
                // identifier character (`xcrs:Foo` is a different prefix).
                let name_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.');
                let starts = b[i..].starts_with(&['c', 'r', 's', ':'])
                    && (i == 0 || !name_char(b[i - 1]));
                if starts {
                    let mut j = i + 4;
                    // `-` and `.` are legal XML name characters (NCName). No
                    // Adobe key uses either TODAY, so this changes no current
                    // output — it is here so a future `crs:Foo-Bar` is named
                    // in full instead of being reported as `Foo`.
                    while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], '_' | '-' | '.')) {
                        j += 1;
                    }
                    let name: String = b[i + 4..j].iter().collect();
                    // An ATTRIBUTE, so `=` must follow — after any XML
                    // whitespace (`Eq ::= S? '=' S?`; S is space, tab, CR or
                    // LF), the same class `next_xml_attribute` skips. Skipping
                    // spaces alone left a key wrapped as `crs:Foo\n="1"` read
                    // by the merge and missing from this disclosure.
                    let mut k = j;
                    while k < b.len() && b[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    if !name.is_empty() && b.get(k) == Some(&'=') && !owned.contains(&name) {
                        found.insert(name);
                    }
                    i = j.max(i + 1);
                } else {
                    i += 1;
                }
            }
        }
    }

    // The ELEMENT half, walked with the same primitives `crs_str` uses
    // (`next_xml_tag` + `element_close_start`) rather than a second XML
    // scanner. Every non-self-closing child is JUMPED OVER whole, so nothing
    // nested inside one is ever read as this Description's own property.
    if !self_closing
        && let Some(close) = find_matching_close(xmp, gt + 1)
    {
        let mut from = gt + 1;
        while let Some((s, e, child_self_closing)) = next_xml_tag(xmp, from) {
            if s >= close {
                break; // past this Description's own body
            }
            let child = &xmp[s..=e];
            if child.starts_with("</") {
                // Only reachable on markup this walk cannot account for (the
                // opens above are all skipped past their own close).
                from = e + 1;
                continue;
            }
            let name = tag_name(child);
            if let Some(bare) = name.strip_prefix("crs:")
                && !bare.is_empty()
                && !owned.contains(bare)
            {
                found.insert(bare.to_string());
            }
            from = if child_self_closing {
                e + 1
            } else {
                match element_close_start(xmp, name, e).and_then(|c| scan_tag_end(xmp, c)) {
                    Some((close_gt, _)) => close_gt + 1,
                    // A child that never closes: the rest of the body is not
                    // accountable, and guessing would report a mask's own
                    // items as globals.
                    None => break,
                }
            };
        }
    }
    found.into_iter().collect()
}

/// **The PASS-THROUGH key set** (R25 B4): Lightroom's Transform (Upright /
/// Perspective) block and the camera profile's name, carried verbatim between
/// the sidecar and `EditRecipe::passthrough` and NEVER interpreted — the first
/// real payload `Tier::PassThrough` has ever had.
///
/// A NAMED SET, not "everything unknown", and that is the whole design.
/// [`owned_attr_keys`] is a static list and it is the merge's REMOVAL
/// universe: a free-form map would emit keys the merge never strips, so our
/// value would land beside Lightroom's original as a duplicate attribute.
/// Six keys we can name are six keys the strip can name too.
///
/// The complement is not abandoned — it is DISCLOSED. `crs:CameraProfileDigest`
/// and the rest stay outside this list, are preserved by the merge exactly as
/// before, and go on being named by [`unmodelled_global_crs`]. That is the
/// feature, not the omission.
///
/// FIRST-HAND (all seven reference sidecars, re-measured for F6): one of the
/// seven carries all six of these keys
/// (`crs:UprightCenterNormX="0.507694218"`,
/// `crs:UprightFocalLength35mm="15.491801514"` — nine significant digits that
/// no `f32` round trip returns, which is precisely why these are strings) and
/// the other six carry none of them.
///
/// The eight `crs:Perspective*` keys were this list's founding members and
/// left it in v1.5.0 F6: they are present on all seven files too, but they are
/// OWNED controls now, parsed into `EditRecipe`'s own fields and rendered by
/// [`crate::render::perspective`]. `crs:CameraProfile` left for the same reason
/// in F7 — it is on all seven, and [`crate::dcp`] now renders the profile it
/// names instead of carrying the name past a render that ignored it.
///
/// R25 listed seven `CameraCalibration*` keys here as well. Lightroom writes
/// none of them: its Calibration panel is the unprefixed `crs:ShadowTint` /
/// `crs:RedHue` / … block (on all seven reference sidecars and 162 of the
/// operator's 175), so the seven never captured a value, and v1.5.0 renders the
/// real ones ([`CALIBRATION_CRS`]).
pub const PASSTHROUGH_CRS: [&str; 6] = [
    // The ASSUMPTIONS Lightroom's own Upright solver worked from: a version
    // stamp, and the centre and focal length it took the photograph to have.
    // None of them changes a pixel here and none is a control a photographer
    // moves, so they are carried and never interpreted.
    //
    // Its cached ANSWER is deliberately not on this list — not the matrices
    // (`recipe::EditRecipe::upright_transform` reads them and never writes
    // them), and so also not `UprightTransformCount`, `UprightPreview` or
    // `UprightDependentDigest`, which describe an answer a fresh sidecar of
    // ours would no longer contain. A digest whose subject is missing is worse
    // than no digest: it is exactly the staleness marker Lightroom checks. On
    // the usual path — a MERGE into Lightroom's own file — all six survive
    // untouched anyway, because a key we neither own nor carry is a key the
    // strip never names.
    "UprightVersion",
    "UprightCenterMode",
    "UprightCenterNormX",
    "UprightCenterNormY",
    "UprightFocalMode",
    "UprightFocalLength35mm",
];

/// The crs keys a creative Look can bake that this engine RENDERS, plus the two
/// that are bookkeeping. Everything else a Look carries is named in
/// [`crate::recipe::CreativeLook::unrendered`].
///
/// `LookTable` is deliberately NOT here: it is the creative colour table, it is
/// the one thing the Look carries that cannot be rendered, and leaving it out
/// is what makes the disclosure name it.
const LOOK_RENDERED_CRS: [&str; 7] = [
    "Version",
    "ProcessVersion",
    "CameraProfile",
    "ConvertToGrayscale",
    "Clarity2012",
    "Highlights2012",
    "Shadows2012",
];

/// The creative profile `crs:Look` names, as far as this engine can render it.
///
/// Lightroom writes the creative profile's WHOLE baked half into the sidecar —
/// on 161 of 161 Looks in the library this was measured against — so the file
/// in front of us is self-sufficient and nothing has to be looked up on disk.
/// What it does NOT write out is the colour table: that is a 32-hex reference
/// (`crs:LookTable`), and [`crate::dcp`] records what was measured about the
/// payload it refers to and why it is not decodable.
///
/// Read from the `<crs:Look>` element itself rather than through the
/// Description's scope, which is the whole point: those baked properties LOOK
/// like the photographer's own settings and are not. The engine's global reader
/// is scoped to exclude them precisely so this function can claim them.
pub(super) fn read_creative_look(xmp: &str) -> Option<crate::recipe::CreativeLook> {
    let body = owned_element_body(xmp, "crs:Look").ok().flatten()?;
    let head = Scope::new(body);
    // A Look with no Parameters block states nothing to render; it is still a
    // Look, and its name and amount are still worth carrying.
    let params = owned_element_body(body, "crs:Parameters").ok().flatten().unwrap_or("");
    let p = Scope::new(params);
    let text = |s: Option<std::borrow::Cow<'_, str>>| s.map(|v| v.into_owned()).unwrap_or_default();
    let curve = |tag: &str| parse_curve_checked(params, tag).unwrap_or_default();
    Some(crate::recipe::CreativeLook {
        name: text(head.crs_str("Name")),
        // Absent means the whole Look, which is what `crs:Amount="1"` says on
        // every one of the 161 measured — not zero, which would silently
        // switch the profile off.
        amount: head.crs_f32("Amount").filter(|v| v.is_finite()).unwrap_or(1.0),
        base_profile: text(p.crs_str("CameraProfile")),
        table: text(p.crs_str("LookTable")),
        grayscale: p.crs_str("ConvertToGrayscale").as_deref().map(str::trim) == Some("True"),
        clarity: p.crs_f32("Clarity2012").unwrap_or(0.0),
        highlights: p.crs_f32("Highlights2012").unwrap_or(0.0),
        shadows: p.crs_f32("Shadows2012").unwrap_or(0.0),
        tone_curve: curve("ToneCurvePV2012"),
        red_curve: curve("ToneCurvePV2012Red"),
        green_curve: curve("ToneCurvePV2012Green"),
        blue_curve: curve("ToneCurvePV2012Blue"),
        unrendered: look_unrendered(params),
    })
}

/// Which of the Look's baked crs properties this engine does not act on, by
/// name, sorted and de-duplicated.
///
/// Scans the ATTRIBUTES of every tag inside the Parameters block, which is
/// where Lightroom writes them, and subtracts [`LOOK_RENDERED_CRS`]. The four
/// `ToneCurvePV2012*` children are elements rather than attributes, so they
/// never appear here in the first place.
fn look_unrendered(params: &str) -> Vec<String> {
    let mut found: std::collections::BTreeSet<String> = Default::default();
    let mut from = 0usize;
    while let Some((start, end, _)) = next_xml_tag(params, from) {
        let tag = &params[start..=end];
        if !tag.starts_with("</") {
            let mut cursor = 0usize;
            while let Some(a) = next_xml_attribute(tag, &mut cursor) {
                if let Some(key) = a.name.strip_prefix("crs:")
                    && !LOOK_RENDERED_CRS.contains(&key)
                {
                    found.insert(key.to_string());
                }
            }
        }
        from = end + 1;
    }
    found.into_iter().collect()
}

/// Adobe's solved Upright matrices, read off `crs:UprightTransform_0…N`.
///
/// Each value is nine comma-separated numbers, a row-major 3×3 projective map
/// in [0,1] FRAME coordinates (see `recipe::EditRecipe::upright_transform` for
/// the measurement that settled the coordinate system). The list is INDEXED BY
/// MODE: `_0` is the identity Lightroom writes for "off" and `_5` the one it
/// writes for a Guided correction with no guides drawn, so index 3 really is
/// what `crs:PerspectiveUpright="3"` selects.
///
/// Stops at the first index the document does not carry, rather than trusting
/// `crs:UprightTransformCount`, because the count is bookkeeping and the
/// indices are the thing being indexed. A malformed entry ends the list for the
/// same reason the recipe's clamp drops the whole list: keeping the later
/// matrices would shift every mode's meaning by one.
pub(super) fn upright_matrices<'a>(scope: impl CrsSource<'a>) -> Vec<[f32; 9]> {
    let mut out = Vec::new();
    for i in 0.. {
        let Some(raw) = scope.crs_str(&format!("UprightTransform_{i}")) else {
            break;
        };
        let mut m = [0.0f32; 9];
        let mut n = 0usize;
        for (slot, field) in m.iter_mut().zip(raw.split(',')) {
            match field.trim().parse::<f32>() {
                Ok(v) if v.is_finite() => {
                    *slot = v;
                    n += 1;
                }
                _ => break,
            }
        }
        if n != 9 || raw.split(',').count() != 9 {
            break;
        }
        out.push(m);
    }
    out
}

/// Lightroom's Calibration panel (v1.5.0) in the order Lightroom writes it and
/// `EditRecipe::calibration` returns it: shadows tint, then the red, green and
/// blue primaries' hue and saturation. Signed integers, all seven or none.
pub(crate) const CALIBRATION_CRS: [&str; 7] =
    ["ShadowTint", "RedHue", "RedSaturation", "GreenHue", "GreenSaturation", "BlueHue", "BlueSaturation"];

/// The seven `crs:SDR*` keys of Lightroom's SDR-rendition panel (v1.5.0 F8),
/// in [`crate::recipe::EditRecipe::sdr_controls`]'s order.
///
/// Unlike the calibration block above these are written INDEPENDENTLY, one key
/// per moved slider: Lightroom does not stamp the block onto every file (it is
/// not on one of the 175 sidecars measured), so there is no "all seven or
/// none" shape in the wild to match, and a key we never saw stays absent.
pub(crate) const SDR_CRS: [&str; 7] = [
    "SDRBlend",
    "SDRBrightness",
    "SDRContrast",
    "SDRHighlights",
    "SDRShadows",
    "SDRWhites",
    "SDRClarity",
];

/// The `crs:` properties this writer owns that have NO attribute spelling —
/// the four tone curves, the point colours and the mask block, which reach
/// the sidecar only as child elements. [`owned_attr_keys`] is the ATTRIBUTE
/// writer's universe and names none of them, so every scanner asking "is this
/// ELEMENT ours?" must union the two lists (the merge's own element strip
/// builds exactly this union — see [`merge_recipe_into_xmp`], which also
/// decides per recipe whether the base's point colours are its to replace).
pub(crate) const OWNED_ELEMENT_ONLY: [&str; 6] = [
    "ToneCurvePV2012",
    "ToneCurvePV2012Red",
    "ToneCurvePV2012Green",
    "ToneCurvePV2012Blue",
    "PointColors",
    "MaskGroupBasedCorrections",
];
