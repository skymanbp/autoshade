//! The `EditRecipe` is the contract between the *AI advisor* (which decides
//! *what* to do) and the *render engine* (which decides *how* to do it,
//! deterministically). The AI never touches pixels — it only emits one of
//! these. Every field maps to a well-understood, Lightroom/ACR-style develop
//! control so the same recipe can be (a) rendered by our own pipeline or
//! (b) serialised to an XMP sidecar that Lightroom / Camera Raw reads directly.
//!
//! Ranges follow Adobe conventions so the numbers are intuitive and portable:
//!   * sliders such as contrast/highlights/shadows: -100..=100, 0 = no change
//!   * `exposure_ev`: stops of exposure, typically -5.0..=5.0, 0.0 = no change
//!   * `temperature_k`: absolute white-balance target in Kelvin (None = as-shot)

use serde::{Deserialize, Serialize};

/// A complete, self-describing set of develop adjustments for one image.
///
/// `#[serde(default)]` means the AI may omit any field it doesn't want to
/// touch; the omitted control simply stays neutral. This keeps prompts small
/// and makes partial recipes valid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditRecipe {
    /// Schema version so we can evolve the contract without silently
    /// misreading old recipes — and, since [`CALIB_ERA`], the CALIBRATION
    /// era that produced `base_curve`. Version 1 recipes were written by
    /// builds whose working-resolution cap biased every sample, so a curve
    /// they stored may have been fitted against a washed frame; see
    /// `pipeline::repair_pre_era_base_curve`.
    pub version: u32,

    /// Which COORDINATE FRAME `crop` and every `masks` geometry are expressed
    /// in — see [`COORD_ERA`]. Absent in any recipe written before v0.30.0,
    /// which is exactly what `0` (the SENSOR frame) means.
    ///
    /// This is deliberately NOT folded into [`version`](Self::version).
    /// `version` is the CURVE's provenance and is transplanted between
    /// recipes on purpose — `paste_recipe_for` (gui/persist.rs), the Analyze
    /// writer (`pipeline::produce_recipe`), the quit-time generated-variant
    /// re-stamp (gui/actions.rs) and `photo_calibration` all copy the TARGET
    /// photo's `version` onto a recipe whose geometry came from somewhere
    /// else. A coordinate frame is intrinsic to the geometry and must never
    /// travel with a curve: stamping a saved era-2 `version` onto a recipe
    /// whose masks are already in the display frame would make the next load
    /// rotate them a second time. Two independent facts, two fields.
    #[serde(default = "coord_era_legacy")]
    pub coord_era: u32,

    /// Which SET OF CONTROLS this recipe was written against — see
    /// [`SCHEMA_ERA`]. Absent in any recipe written before v0.31.0, which is
    /// exactly what `0` means.
    ///
    /// The fact nothing else in the file could state: R25 gave the writer
    /// twenty-seven new `crs:` keys (global Texture, the carried effects, the
    /// detail axes, the manual CA pair, the de-fringe block), and every recipe
    /// written before them deserialises with those fields at their serde
    /// defaults. A default nobody read is NOT a photographer's decision, and
    /// the XMP merge cannot tell the two apart from the value alone: stripping
    /// `crs:Texture="-20"` out of someone's sidecar because a v0.30
    /// `recipe.json` "says" 0 is deleting an edit, not honouring one
    /// (`xmp::era_suppressed_attr_keys`).
    ///
    /// Deliberately NOT folded into [`version`](Self::version) or
    /// [`coord_era`](Self::coord_era), for the reason spelled out above: those
    /// two are the CURVE's provenance and the GEOMETRY's frame, and `version`
    /// in particular is transplanted between recipes on purpose. Which keys a
    /// recipe has ever seen is a third, independent fact — the four-fields-in-
    /// one-integer move is exactly what made the pre-`coord_era` migration
    /// unrecoverable.
    #[serde(default = "schema_era_legacy")]
    pub schema_era: u32,

    // --- Tone ---------------------------------------------------------------
    /// Global exposure in stops (EV). 0.0 = unchanged.
    pub exposure_ev: f32,
    /// -100..=100
    pub contrast: f32,
    /// -100..=100 (recover blown highlights when negative)
    pub highlights: f32,
    /// -100..=100 (lift shadows when positive)
    pub shadows: f32,
    /// -100..=100 (white point)
    pub whites: f32,
    /// -100..=100 (black point)
    pub blacks: f32,

    // --- White balance ------------------------------------------------------
    /// Absolute colour temperature target in Kelvin. `None` = keep as-shot.
    pub temperature_k: Option<f32>,
    /// Green/magenta tint, -100..=100. 0 = neutral.
    pub tint: f32,
    /// ENGINE-ONLY as-shot anchor (never in XMP): the camera's own white
    /// balance as an ABSOLUTE Kelvin CCT, derived from the RAW's WB
    /// coefficients through the camera colour matrix (`render::as_shot_wb`).
    /// Stamped like `base_curve`/`lens_profile` — fresh opens get it, a saved
    /// recipe.json owns its value verbatim. `None` = unknown → the engine
    /// keeps its historical 5500 K anchor, so legacy archives render
    /// byte-identically.
    pub as_shot_k: Option<f32>,
    /// The as-shot green/magenta companion, in the recipe's own tint scale
    /// (display + XMP honesty only — `tint` stays a RELATIVE shift from
    /// as-shot, so the engine never feeds this to `wb_gains`).
    pub as_shot_tint: Option<f32>,

    // --- Colour / presence --------------------------------------------------
    /// -100..=100, protects already-saturated colours.
    pub vibrance: f32,
    /// -100..=100, uniform saturation.
    pub saturation: f32,
    /// -100..=100, local contrast / midtone punch.
    pub clarity: f32,
    /// -100..=100, atmospheric haze removal.
    pub dehaze: f32,
    /// -100..=100, fine detail (skin, foliage, fabric) without clarity's
    /// midtone volume — Lightroom's Basic-panel Texture. The same unsharp
    /// operator as `clarity` at a SMALL radius and with no midtone mask; the
    /// per-mask `LocalAdjustment::texture` has rendered it since R22, and this
    /// is the global counterpart it never had (R25 B2). → `crs:Texture`.
    pub texture: f32,

    // --- Per-colour HSL (the 8 ACR colour bands) ----------------------------
    /// Lightroom's HSL / Color mixer. Default = all bands neutral (v1-compatible).
    pub hsl: Hsl,

    // --- Colour grading (3-wheel + global) ----------------------------------
    /// Lightroom's Color Grading wheels. Default = neutral (v1-compatible).
    pub color_grade: ColorGrade,

    // --- Detail -------------------------------------------------------------
    /// 0..=150, capture sharpening amount.
    pub sharpening: f32,
    /// 0..=100, luminance noise reduction.
    pub noise_reduction: f32,

    // --- Detail: the CARRIED shaping axes (R25 B3) ---------------------------
    // Eight more Adobe-only operators under policy SF4-C, same contract as the
    // effects block below: round-tripped through their own `crs:` keys, never
    // approximated. They shape the two sliders above — Lightroom's sharpening
    // has a radius / detail / edge-masking triple around its amount, its
    // luminance NR has a detail / contrast pair, and its COLOUR NR is a third
    // operator this engine does not have at all. Each carries its reason in
    // `advisor::catalogue::CARRIED_ONLY_GLOBAL`.
    //
    // NEUTRAL AT ZERO, like the effects: zero means "the sidecar said nothing"
    // and the writer emits a key only when it is non-zero, so an absent key
    // stays absent and Lightroom keeps its own default (Radius 1.0, Detail 25,
    // Luminance Detail 50, Colour NR 25/50/50) instead of one we invented.
    /// Sharpening radius, 0..=3.0 (0 = absent; ACR default 1.0 and its band is
    /// 0.5..3.0). The one DECIMAL key in this block — Lightroom writes
    /// `crs:SharpenRadius="+1.0"`, verified in all seven of the user's
    /// sidecars. → `crs:SharpenRadius`.
    pub sharpen_radius: f32,
    /// Sharpening detail / halo suppression, 0..=100 (ACR default 25).
    /// → `crs:SharpenDetail`.
    pub sharpen_detail: f32,
    /// Sharpening edge mask, 0..=100. → `crs:SharpenEdgeMasking`.
    pub sharpen_mask: f32,
    /// Luminance NR detail, 0..=100 (ACR default 50).
    /// → `crs:LuminanceNoiseReductionDetail`.
    pub nr_detail: f32,
    /// Luminance NR contrast, 0..=100. → `crs:LuminanceNoiseReductionContrast`.
    pub nr_contrast: f32,
    /// CHROMA noise reduction amount, 0..=100 (ACR default 25) — an operator
    /// this engine has none of. → `crs:ColorNoiseReduction`.
    pub color_nr: f32,
    /// Chroma NR detail, 0..=100 (ACR default 50).
    /// → `crs:ColorNoiseReductionDetail`.
    pub color_nr_detail: f32,
    /// Chroma NR smoothness, 0..=100 (ACR default 50).
    /// → `crs:ColorNoiseReductionSmoothness`.
    pub color_nr_smooth: f32,

    // --- Lens corrections (manual) -------------------------------------------
    /// Lens vignette compensation, -100..=100: positive brightens corners
    /// (compensates falloff), negative darkens. Applied as a radial gain in
    /// LINEAR light before any tonal work. → `crs:VignetteAmount`.
    pub lens_vignette: f32,
    /// Where the correction lands, 0..=100 (ACR Midpoint, default 50): lower
    /// reaches toward the centre, higher confines it to the corners.
    /// → `crs:VignetteMidpoint`.
    pub lens_vignette_mid: f32,
    /// Manual geometric distortion correction, -100..=100 (ACR convention):
    /// positive straightens BARREL distortion (wide-angle bulge), negative
    /// straightens PINCUSHION (tele pinch). A radial resample of the frame
    /// applied after develop, before straighten — see `render::distort_norm`
    /// for the model. → `crs:LensManualDistortionAmount`.
    pub lens_distortion: f32,
    /// Manual lateral CA, red/cyan axis, -100..=100 (R25 B3). RENDERED: the
    /// engine already resamples red and blue at their own radius for the
    /// in-camera profile's CA knots (`render::apply_lens_geometry`), and this
    /// pair is a CONSTANT factor folded onto that same per-channel LUT — a
    /// lateral CA scales the channel linearly with radius, so a constant
    /// factor is its exact shape, not an approximation of one. Positive
    /// magnifies the red channel relative to green. The slider→scale
    /// calibration is OURS (`render::MANUAL_CA_PER_UNIT`); Adobe's is
    /// unpublished. → `crs:ChromaticAberrationR`.
    pub ca_r: f32,
    /// Manual lateral CA, blue/yellow axis, -100..=100.
    /// → `crs:ChromaticAberrationB`.
    pub ca_b: f32,
    /// Lightroom's "Remove chromatic aberration" switch — an INSTRUCTION to
    /// Adobe's own lateral-CA solver, not a parameter, so it is carried and
    /// never interpreted (SF4-C). Verified in the user's library:
    /// `crs:AutoLateralCA="0"` on six sidecars and `="1"` on one.
    /// → `crs:AutoLateralCA`.
    pub auto_lateral_ca: bool,

    // --- De-fringe: CARRIED, and the one block with NON-ZERO neutrals -------
    // Lightroom writes all six keys on every sidecar (verified: 7 of 7 in the
    // user's library carry `DefringePurpleAmount="0"` with the hue windows at
    // 30/70 and 40/60), so "absent" is not a state this block has in the wild
    // and its honest neutral is ADOBE'S DEFAULT, not zero. Two consequences,
    // both deliberate:
    //
    //   * the READER falls back to these defaults, not to 0 — `crs_f32`
    //     answers `None` for an absent key and taking that as 0 would import
    //     a hue window of 0..0 from a document that never mentioned one, so
    //     an untouched photo would stop being a no-op;
    //   * the WRITER emits all six unconditionally, the shape Lightroom
    //     itself writes.
    //
    // `is_noop` is `*self == EditRecipe::default()`, so it stays correct by
    // construction — and it is pinned both ways, on a sidecar carrying the six
    // defaults and on one carrying no de-fringe keys at all
    // (`xmp::tests::a_real_defringe_block_imports_as_a_noop`).
    /// Purple de-fringe amount, 0..=20. → `crs:DefringePurpleAmount`.
    pub defringe_purple: f32,
    /// Purple hue window, low end, 0..=100 (ACR default 30).
    /// → `crs:DefringePurpleHueLo`.
    pub defringe_purple_lo: f32,
    /// Purple hue window, high end, 0..=100 (ACR default 70).
    /// → `crs:DefringePurpleHueHi`.
    pub defringe_purple_hi: f32,
    /// Green de-fringe amount, 0..=20. → `crs:DefringeGreenAmount`.
    pub defringe_green: f32,
    /// Green hue window, low end, 0..=100 (ACR default 40).
    /// → `crs:DefringeGreenHueLo`.
    pub defringe_green_lo: f32,
    /// Green hue window, high end, 0..=100 (ACR default 60).
    /// → `crs:DefringeGreenHueHi`.
    pub defringe_green_hi: f32,

    // --- Effects: CARRIED to Lightroom, rendered by nothing here (R25 B2) ----
    // Nine Adobe-only operators under the SF4-C policy: we round-trip them
    // through their own `crs:` keys so a Lightroom edit survives an Autoshop
    // save, and we deliberately do NOT approximate them. Each carries its
    // reason in `advisor::catalogue::CARRIED_ONLY_GLOBAL`, and
    // `Tier::CarriedOnly` is what makes the omission a declared fact rather
    // than a slider that quietly does nothing.
    //
    // Every one is NEUTRAL AT ZERO on purpose, including the three whose ACR
    // default is not zero (Midpoint/Feather 50, Style 1): zero here means
    // "the sidecar said nothing", and the writer emits a key only when its
    // value is non-zero — so an absent key stays absent and Lightroom keeps
    // its own default instead of one we invented. Verified against the user's
    // own library: Lightroom writes `PostCropVignetteAmount="0"` /
    // `GrainAmount="0"` on nearly every sidecar and the COMPANION keys only
    // when the amount is non-zero, so an untouched photo still imports as a
    // no-op (`is_noop`) while a real vignette imports all six.
    /// Post-crop vignette amount, -100..=100. → `crs:PostCropVignetteAmount`.
    pub post_crop_vignette: f32,
    /// Post-crop vignette midpoint, 0..=100 (ACR default 50).
    /// → `crs:PostCropVignetteMidpoint`.
    pub post_crop_vignette_mid: f32,
    /// Post-crop vignette feather, 0..=100 (ACR default 50).
    /// → `crs:PostCropVignetteFeather`.
    pub post_crop_vignette_feather: f32,
    /// Post-crop vignette roundness, -100..=100.
    /// → `crs:PostCropVignetteRoundness`.
    pub post_crop_vignette_round: f32,
    /// Which post-crop vignette OPERATOR Adobe applies: 1 = Highlight
    /// Priority, 2 = Colour Priority, 3 = Paint Overlay (0 = the sidecar named
    /// none). → `crs:PostCropVignetteStyle`.
    pub post_crop_vignette_style: f32,
    /// Post-crop vignette highlight contrast, 0..=100.
    /// → `crs:PostCropVignetteHighlightContrast`.
    pub post_crop_vignette_hl: f32,
    /// Film grain amount, 0..=100. → `crs:GrainAmount`.
    pub grain: f32,
    /// Film grain size, 0..=100 (ACR default 25). → `crs:GrainSize`.
    pub grain_size: f32,
    /// Film grain roughness/frequency, 0..=100 (ACR default 50).
    /// → `crs:GrainFrequency`.
    pub grain_rough: f32,

    // --- Transform + Calibration: PASSED THROUGH, never interpreted (R25 B4) -
    /// Lightroom's Transform (Upright / Perspective) and Camera Calibration
    /// blocks, carried between the sidecar and `recipe.json` as the VERBATIM
    /// strings Lightroom wrote — key → value, no parsing, no clamping, no
    /// meaning. The first member of `Tier::PassThrough`, and the one contract
    /// that separates it from `CarriedOnly`: a carried control is a NUMBER we
    /// read, bound and could render one day; a passed-through property is a
    /// string we have no opinion about at all. Out-of-range, oddly spelled or
    /// eight-decimal values ride out exactly as they rode in.
    ///
    /// The DOMAIN is a named key set (`xmp::PASSTHROUGH_CRS`, sixteen keys),
    /// not "everything unknown": `owned_attr_keys` — the merge's strip
    /// universe — is a static list, so a free-for-all map would write keys the
    /// merge does not remove and Lightroom would read our value beside its own
    /// duplicate. Everything outside those sixteen stays where it always was:
    /// preserved by the merge, and NAMED by `xmp::unmodelled_global_crs`.
    ///
    /// A `BTreeMap`, so `recipe.json` and the R21 structure fingerprint are
    /// deterministic. The container's `#[serde(default)]` covers absence (an
    /// empty map is `BTreeMap::default()`) — no field-level default is needed
    /// here, unlike `coord_era`, whose legacy default DIFFERS from the type's.
    pub passthrough: std::collections::BTreeMap<String, String>,

    // --- Geometry (optional) ------------------------------------------------
    /// Clockwise straighten angle in degrees, e.g. -2.5..=2.5 for horizons.
    pub straighten_deg: f32,
    /// How many CLOCKWISE quarter turns the PHOTOGRAPHER asked for, on top of
    /// whatever the camera's EXIF orientation already says. 0..=3, 0 = none.
    ///
    /// Composed with the EXIF value into ONE
    /// [`crate::render::compose_orientation`] result, which is what every
    /// orientation consumer reads — the engine's eight-state dihedral group is
    /// closed under composition, so a user turn on a `Transpose`-oriented file
    /// lands on a state `oriented`/`orient_point` already handle. The
    /// coordinates in [`crop`](Self::crop) and every `masks` geometry are
    /// stored in the frame this turn PRODUCES, exactly as they are stored in
    /// the frame the EXIF orientation produces (see [`COORD_ERA`]).
    ///
    /// Deliberately NOT folded into [`coord_era`](Self::coord_era) — the
    /// argument that field already carries, one step further: `coord_era` is
    /// which frame the stored numbers are IN (a storage epoch, migrated once
    /// and never again), `quarter_turns` is what the photographer ASKED FOR (a
    /// live edit, changed as often as any slider). Merging them would make an
    /// undo of a rotation look like a re-migration.
    ///
    /// **Serialisation: skipped when zero**, unlike `coord_era`/`schema_era`,
    /// which are always written. The difference is not style: an ABSENT era
    /// key means something DIFFERENT from the value a fresh recipe stamps
    /// (absent = legacy, so it must always be spelled out), whereas an absent
    /// `quarter_turns` means exactly 0, which is what it would have written.
    /// Skipping is therefore lossless, and it buys two things. (1) A recipe
    /// nobody rotated stays BYTE-IDENTICAL to what v0.32 wrote, so the R21
    /// deleted-version registry's structural arm (`store::recipe_struct_hash`
    /// re-serialises the whole recipe) keeps matching and needs no re-archive
    /// pass. (2) The class-① forward break that `deny_unknown_fields` above
    /// makes unavoidable is confined to recipes that actually carry a turn: a
    /// v0.32 exe still reads every un-rotated v0.33 `recipe.json`, and hard-
    /// rejects — by field name — exactly the ones it would have rendered
    /// sideways.
    #[serde(skip_serializing_if = "no_quarter_turn")]
    pub quarter_turns: u8,
    /// Optional crop as normalised [0,1] coordinates of the kept region.
    pub crop: Option<Crop>,

    // --- Free-form tone curve (optional) ------------------------------------
    /// Monotonic control points on the master tone curve, input/output in
    /// 0..=255. Empty = identity curve.
    pub tone_curve: Vec<CurvePoint>,

    /// Per-channel RGB tone curves (red/green/blue), input/output 0..=255. Empty =
    /// identity. The colour-shaping companion to `tone_curve`; emitted as
    /// `crs:ToneCurvePV2012Red/Green/Blue`.
    pub red_curve: Vec<CurvePoint>,
    pub green_curve: Vec<CurvePoint>,
    pub blue_curve: Vec<CurvePoint>,

    // --- Camera-matched base look (engine-only, NEVER exported to XMP) -------
    /// Base tone curve `[x, y]` knots (luma, 0..=1) mapping the NEUTRAL raw
    /// develop toward the camera's own rendition, estimated per photo by
    /// luma-CDF-matching the neutral develop against the embedded preview
    /// (`render::camera_base_knots`). The engine composes it UNDER the user
    /// tone controls (`build_tone_lut`), so sliders start from a camera-like
    /// base instead of the darker scene-referred develop. Empty = no base look
    /// — exactly the pre-0.14 rendering, which every legacy recipe.json
    /// deserialises to, so existing saved develops keep their appearance.
    /// Lightroom renders XMP over its own camera profile, so xmp.rs ignores it.
    pub base_curve: Vec<[f32; 2]>,

    // --- In-camera lens profile corrections (engine-only, NEVER in XMP) ------
    /// Per-photo lens corrections read from the RAW's own metadata (Sony
    /// 0x7031-0x7037 — see `lensmeta`): the manufacturer's exact profile for
    /// THIS lens at THIS focal/aperture, no lensfun database needed. The
    /// default (empty, everything off) is what every legacy recipe.json
    /// deserialises to, so existing saved develops render byte-identically.
    /// Lightroom applies its own profile db to XMP, so xmp.rs ignores this.
    pub lens_profile: LensProfile,

    // --- Local (masked) adjustments -----------------------------------------
    /// Local adjustments applied through gradient masks. Empty = global-only
    /// (v1-compatible). Emitted as `crs:MaskGroupBasedCorrections` in the XMP
    /// and composited by the render engine.
    pub masks: Vec<LocalAdjustment>,

    // --- Provenance (the AI explains itself) --------------------------------
    /// One or two sentences: why these adjustments, for the user to sanity-check.
    pub rationale: String,
    /// AI self-reported confidence, 0.0..=1.0. Used to gate auto-apply.
    pub confidence: f32,
}

/// The current calibration era, stamped into every recipe we write.
///
/// Era 1 = any build up to R12 batch 43. Its working-resolution cap added
/// +0.5 to every channel of every capped frame, and `photo_base_knots`
/// estimates the camera base curve from exactly such a capped develop — so an
/// era-1 `base_curve` may encode a washed frame. Bumping this is what lets a
/// later load tell the two apart; nothing else in the file could.
pub const CALIB_ERA: u32 = 2;

/// The current COORDINATE-FRAME era, stamped into every recipe we write.
///
/// Era 0 = every recipe written up to v0.29.x. rawler 0.7.2 reports
/// `Orientation::Normal` for every non-DNG RAW (`decode::raw_orientation_of`
/// has the crate-source citation), so a portrait ARW was displayed, developed
/// and exported in its SENSOR frame — and the crop rectangle and mask
/// geometries the user drew on it were stored against that sideways canvas.
/// Era 1 = the display frame the EXIF orientation actually asks for, which is
/// what `render::orient_f32` now produces and what the C2 coordinate contract
/// ("masks live in the ORIGINAL frame") has always claimed.
///
/// For a `Normal`-oriented photo the two frames are identical, so the era
/// only ever changes what a rotated/flipped RAW's saved geometry means.
pub const COORD_ERA: u32 = 1;

/// Serde default for [`EditRecipe::coord_era`] — deliberately NOT the
/// container's `Default::default()` value: a file with no `coord_era` key was
/// written before the field existed, and every one of those holds SENSOR-frame
/// coordinates. Pinned by `absent_coord_era_reads_as_the_legacy_frame`.
fn coord_era_legacy() -> u32 {
    0
}

/// The current CONTROL-SET era, stamped into every recipe this build writes.
///
/// Era 0 = every recipe written up to v0.30.x, whose JSON has no key for the
/// twenty-seven `crs:` properties R25 added to the writer — global `texture`,
/// the nine carried effects, the eight detail axes, the manual CA pair, the
/// auto-CA switch and the six de-fringe keys. Serde fills those fields from
/// [`EditRecipe::default`], so an era-0 recipe SAYS "texture 0, no grain,
/// Adobe's own de-fringe windows" without ever having read a document.
/// Era 1 = written by a build that has all twenty-seven, so its values for
/// them are statements.
///
/// The one consumer is [`crate::xmp::era_suppressed_attr_keys`]: on an era-0
/// recipe the merge neither strips nor re-emits a key still sitting at that
/// untouched default, so the base document's own bytes stand.
pub const SCHEMA_ERA: u32 = 1;

/// Serde default for [`EditRecipe::schema_era`] — deliberately NOT the
/// container's `Default::default()` value, the same field-level-beats-
/// container-level trap [`coord_era_legacy`] documents: a file with no
/// `schema_era` key was written before the field existed, and every one of
/// those predates the R25 keys. Pinned by
/// `absent_schema_era_reads_as_the_legacy_control_set`.
fn schema_era_legacy() -> u32 {
    0
}

/// `skip_serializing_if` predicate for [`EditRecipe::quarter_turns`] — see
/// that field's doc for why THIS field is skipped when the era stamps beside
/// it never are. Pinned by
/// `an_unrotated_recipe_serialises_exactly_as_the_previous_build_wrote_it`.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde's predicate signature
fn no_quarter_turn(k: &u8) -> bool {
    *k == 0
}

impl Default for EditRecipe {
    fn default() -> Self {
        Self {
            version: CALIB_ERA,
            coord_era: COORD_ERA,
            schema_era: SCHEMA_ERA,
            exposure_ev: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            temperature_k: None,
            tint: 0.0,
            as_shot_k: None,
            as_shot_tint: None,
            vibrance: 0.0,
            saturation: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            texture: 0.0,
            hsl: Hsl::default(),
            color_grade: ColorGrade::default(),
            sharpening: 0.0,
            noise_reduction: 0.0,
            sharpen_radius: 0.0,
            sharpen_detail: 0.0,
            sharpen_mask: 0.0,
            nr_detail: 0.0,
            nr_contrast: 0.0,
            color_nr: 0.0,
            color_nr_detail: 0.0,
            color_nr_smooth: 0.0,
            lens_vignette: 0.0,
            lens_vignette_mid: 50.0,
            lens_distortion: 0.0,
            ca_r: 0.0,
            ca_b: 0.0,
            auto_lateral_ca: false,
            // The one NON-ZERO neutral block in this struct — Adobe's own
            // defaults, because Lightroom writes all six keys on every
            // sidecar and a hue window of 0..0 is not a state any real
            // document expresses (see the field docs).
            defringe_purple: 0.0,
            defringe_purple_lo: 30.0,
            defringe_purple_hi: 70.0,
            defringe_green: 0.0,
            defringe_green_lo: 40.0,
            defringe_green_hi: 60.0,
            post_crop_vignette: 0.0,
            post_crop_vignette_mid: 0.0,
            post_crop_vignette_feather: 0.0,
            post_crop_vignette_round: 0.0,
            post_crop_vignette_style: 0.0,
            post_crop_vignette_hl: 0.0,
            grain: 0.0,
            grain_size: 0.0,
            grain_rough: 0.0,
            // Empty = the document carried no Transform / Calibration block.
            // There is no "neutral Upright" to invent here: a key we never saw
            // must not be written into somebody's sidecar.
            passthrough: std::collections::BTreeMap::new(),
            straighten_deg: 0.0,
            quarter_turns: 0,
            crop: None,
            tone_curve: Vec::new(),
            red_curve: Vec::new(),
            green_curve: Vec::new(),
            blue_curve: Vec::new(),
            base_curve: Vec::new(),
            lens_profile: LensProfile::default(),
            masks: Vec::new(),
            rationale: String::new(),
            confidence: 0.0,
        }
    }
}

/// How many knots [`LensProfile::mask_warp`] carries when it is solved.
///
/// The map used to copy the source profile's sixteen samples. D2 measured up
/// to 0.3 px of interpolation error from that output table, so both producers
/// now publish the same dense canonical table.
pub const MASK_WARP_KNOTS: usize = 64;

/// Where [`LensProfile::mask_warp`] came from — or, when it is empty, WHY.
///
/// A typed answer rather than an empty vector, for the reason every disclosure
/// in this codebase is typed: "no warp" has six causes and they send the
/// photographer to six different places (install Camera Raw; this lens has no
/// profile; the sidecar switched the correction off; the profile is a fisheye
/// and we refuse to fake it; …). An empty `Vec` says none of that, and a
/// comment beside the call site is not a channel.
///
/// PERSISTED in `recipe.json` on purpose. The map is solved on the machine that
/// has the camera metadata and the Adobe profile pool; a `recipe.json` opened
/// somewhere else must still be able to say what was known when it was written
/// and what was not, without re-deriving anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MaskWarpSource {
    /// No map, and nothing was tried — the ordinary state for a photograph
    /// with no lens-correction data of any kind.
    #[default]
    Absent,
    /// Solved from the in-camera knots already in `distortion`
    /// (`render::mask_warp_from_camera_knots`). Source A: no external file, and
    /// exactly the polynomial the camera maker calibrated for this shot.
    CameraMetadata,
    /// Solved from an Adobe `.lcp` on this machine (`lcp::solve_mask_warp`).
    /// Source B, for bodies whose RAWs carry no knots.
    Lcp,
    /// The sidecar says `crs:LensProfileEnable="0"`: Lightroom drew NO lens
    /// correction. RADIAL therefore stays at stored coordinates; LINEAR uses
    /// its separately retained camera map to transport two handles into the raw
    /// frame. This is its own variant rather than [`Self::Absent`] because both
    /// the identity radial answer and the LINEAR transport are known facts.
    DisabledInSidecar,
    /// A profile was found and REFUSED because it is a fisheye model. Applying
    /// the rectilinear polynomial to one returns finite, plausible numbers and
    /// would move every mask by an invented amount (`lcp::Refusal::Fisheye`).
    FisheyeRefused,
    /// A profile was named or matched and could not be parsed or solved.
    Unparseable,
    /// This machine has no Adobe lens-profile directory — Camera Raw was never
    /// installed, or the build is not on Windows, where those roots live.
    NoProfileRoots,
}

impl MaskWarpSource {
    /// Every source — the ONE list the disclosure surfaces iterate, exactly as
    /// `xmp::MaskImportReason::ALL` serves the mask half. Pinned by
    /// `mask_warp_source_all_covers_every_variant`.
    pub const ALL: [MaskWarpSource; 7] = [
        MaskWarpSource::Absent,
        MaskWarpSource::CameraMetadata,
        MaskWarpSource::Lcp,
        MaskWarpSource::DisabledInSidecar,
        MaskWarpSource::FisheyeRefused,
        MaskWarpSource::Unparseable,
        MaskWarpSource::NoProfileRoots,
    ];

    /// Did a real map come out of this? The two `true` answers are the only
    /// ones that may be accompanied by a non-empty `mask_warp`, which
    /// `clamp` enforces rather than trusts.
    pub fn is_solved(self) -> bool {
        matches!(self, MaskWarpSource::CameraMetadata | MaskWarpSource::Lcp)
    }

    /// English label for the prose channel (CLI stderr / batch warnings).
    pub fn en(self) -> &'static str {
        match self {
            MaskWarpSource::Absent => "no lens-correction data for this photo",
            MaskWarpSource::CameraMetadata => "solved from the in-camera lens profile",
            MaskWarpSource::Lcp => "solved from an Adobe lens profile (.lcp)",
            MaskWarpSource::DisabledInSidecar => {
                "the sidecar turned the lens profile off - radial stays stored; linear transports handles"
            }
            MaskWarpSource::FisheyeRefused => {
                "the Adobe lens profile is a fisheye model - refused, not applied"
            }
            MaskWarpSource::Unparseable => "the Adobe lens profile could not be read",
            MaskWarpSource::NoProfileRoots => "no Adobe lens-profile directory on this machine",
        }
    }
}

/// In-camera lens profile corrections in ENGINE space: per-knot factors over
/// normalised radius (0 = centre, 1 = corner). The engine's established
/// interpolator places knot `i` at `(i + 0.5) / (n − 1)`. D2 established that
/// Sony's private distortion tag uses `(i+1)/16`; `lensmeta` converts that
/// native domain onto a dense canonical spline for the Lightroom mask solve.
/// Its ordinary render field deliberately remains on the established
/// calibration after the image-registration adjudication gate rejected a
/// render-path change. `vignette` holds linear-light GAINS;
/// `distortion` / `ca_r` / `ca_b` hold radius scale factors (CA multiplies the
/// distortion map per channel).
/// Conversion from the camera's raw integers lives in `lensmeta` — this
/// struct is camera-agnostic on purpose. Engine-only, never written to XMP.
///
/// **R29 Batch-3 adds `mask_warp` + `mask_warp_src`; D2 adds
/// `mask_warp_center`; D2 LINEAR adds `linear_handle_warp`. Each is a HARD
/// FORWARD BREAK.** This struct denies
/// unknown fields, so an older binary refuses a `recipe.json` carrying a frame
/// fact it cannot honour. Backwards is fine (all fields default), forwards is
/// deliberately not: silently dropping a coordinate-frame map would be the
/// one failure mode a mask frame cannot survive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct LensProfile {
    pub vignette: Vec<f32>,
    pub distortion: Vec<f32>,
    pub ca_r: Vec<f32>,
    pub ca_b: Vec<f32>,
    pub vignette_on: bool,
    pub distortion_on: bool,
    pub ca_on: bool,
    /// The RADIAL MASK WARP: `m(r) = r_exported / r_stored`, the radial
    /// magnification between the frame Lightroom STORED a radial point in and
    /// the frame it EXPORTED that point into. LINEAR uses the same solved camera
    /// map only as a two-handle forward transform when no geometry follows; it
    /// never applies this map pointwise. Same knot placement, same interpolator
    /// and same shape as `distortion` above — one convention, two producers
    /// (`render::mask_warp_from_camera_knots` and `lcp::solve_mask_warp`).
    ///
    /// Empty = identity, and `mask_warp_src` says why.
    ///
    /// **This is a LIGHTROOM-frame quantity, not a description of this
    /// engine's own render.** What each consumer must do with it depends on
    /// where that consumer evaluates masks; `render::lr_mask_warp_norm` and
    /// its inverse are the two primitive directions, and their block carries
    /// the frame table.
    ///
    /// NEVER written to XMP. The recipe keeps the sidecar's stored, plain-frame
    /// coordinates verbatim — `xmp::lr_to_engine` / `engine_to_lr` stay exact
    /// inverses of each other and this field is not part of that boundary.
    pub mask_warp: Vec<f32>,
    /// The camera map retained solely for LINEAR handle transport when the
    /// sidecar disabled lens correction. In that state [`Self::mask_warp`]
    /// remains empty, preserving RADIAL's measured stored-coordinate identity,
    /// while render maps a LINEAR component's two handles through this spline
    /// once and reconstructs one straight gradient in the raw pixel metric.
    ///
    /// Empty in every other state: when correction was not explicitly disabled,
    /// an inactive downstream geometry stage can use `mask_warp` itself for the
    /// same handle-only operation. Persisted because the camera/LCP solve may
    /// not be reproducible on the machine that later opens `recipe.json`.
    /// Never written to XMP.
    pub linear_handle_warp: Vec<f32>,
    /// Where `mask_warp` came from, or why there is none. See
    /// [`MaskWarpSource`].
    pub mask_warp_src: MaskWarpSource,
    /// Lightroom's radial-map centre in stored-frame pixels, paired with the
    /// stored-frame dimensions those pixels use. Camera-metadata profiles
    /// populate `raw_full_dims/2 - DefaultCropOrigin`; `None` is the legacy
    /// contract and means the current render's stored-frame centre.
    ///
    /// The dimensions are part of this ONE frame fact because previews are
    /// developed after a working-resolution downscale. They let render scale
    /// the pixel centre without guessing the source size.
    ///
    /// Persisted, never written to XMP. Adding it is a deliberate v1.0.0
    /// forward schema break: an older `deny_unknown_fields` reader refuses a
    /// recipe rather than silently discarding a coordinate-frame fact.
    pub mask_warp_center: Option<MaskWarpCenter>,
}

/// One stored-coordinate frame fact for Lightroom's radial mask transport.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaskWarpCenter {
    /// Full-raw-frame centre expressed in stored/default-crop pixels.
    pub stored_px: [f32; 2],
    /// Dimensions of that stored/default-crop pixel frame.
    pub stored_dims: [f32; 2],
}

impl LensProfile {
    /// Camera map available for LINEAR's corrections-off handle transport.
    /// The dedicated field wins only for the disabled-sidecar state; otherwise
    /// the ordinary solved map supplies the same forward transform.
    pub fn linear_handle_warp(&self) -> &[f32] {
        if self.mask_warp_src == MaskWarpSource::DisabledInSidecar {
            &self.linear_handle_warp
        } else {
            &self.mask_warp
        }
    }

    /// Vignette stage active? (toggle on AND data present)
    pub fn vignette_active(&self) -> bool {
        self.vignette_on && !self.vignette.is_empty()
    }
    /// Any geometric component (distortion / CA) active?
    pub fn geometry_active(&self) -> bool {
        (self.distortion_on && !self.distortion.is_empty())
            || (self.ca_on && !self.ca_r.is_empty() && !self.ca_b.is_empty())
    }
    /// Exactly the state stamping produces: every AVAILABLE component enabled
    /// (a toggle for absent data is meaningless either way). The default
    /// (all empty, all off) satisfies this too — so `is_noop` can treat the
    /// as-stamped profile as calibration-not-an-edit, while any user toggle
    /// AWAY from the stamp counts as an edit and survives saves.
    pub fn is_as_stamped(&self) -> bool {
        // Each toggle must equal "data present" (the != form is clippy's
        // spelling of a boolean-equality with a negation). CA needs the PAIR:
        // a half-damaged file (red knots without blue) renders no CA, so its
        // ca_on=true is NOT the stamped state and must read as an edit.
        let ca_present = !self.ca_r.is_empty() && !self.ca_b.is_empty();
        self.vignette_on != self.vignette.is_empty()
            && self.distortion_on != self.distortion.is_empty()
            && self.ca_on == ca_present
    }
    /// Defensive ranges for hand-edited files: knot counts capped, vignette
    /// gains and radius factors held to physically plausible bands (the real
    /// Sony data sits well inside: gains ≲ 1.5×, distortion within ±6%).
    pub fn clamp(&mut self) {
        for v in [&mut self.vignette, &mut self.distortion, &mut self.ca_r, &mut self.ca_b] {
            v.truncate(32);
            // A non-finite knot survives f32::clamp and poisons the whole
            // correction spline — drop it (the base_curve knot rule).
            v.retain(|k| k.is_finite());
        }
        self.mask_warp.truncate(MASK_WARP_KNOTS);
        self.mask_warp.retain(|k| k.is_finite());
        self.linear_handle_warp.truncate(MASK_WARP_KNOTS);
        self.linear_handle_warp.retain(|k| k.is_finite());
        for g in self.vignette.iter_mut() {
            *g = g.clamp(0.25, 4.0);
        }
        for f in self.distortion.iter_mut() {
            *f = f.clamp(0.7, 1.3);
        }
        for f in self.ca_r.iter_mut().chain(self.ca_b.iter_mut()) {
            *f = f.clamp(0.98, 1.02);
        }
        // The SAME band as `distortion`, because it is the same kind of
        // quantity — a radius factor — solved from the same polynomials. The
        // widest real value this batch measured is 1.0425 (105 mm centre) and
        // the narrowest 0.9734 (24 mm centre), so ±30 % is defensive rather
        // than restrictive.
        for f in self.mask_warp.iter_mut() {
            *f = f.clamp(0.7, 1.3);
        }
        for f in self.linear_handle_warp.iter_mut() {
            *f = f.clamp(0.7, 1.3);
        }
        if self
            .mask_warp_center
            .is_some_and(|c| {
                let [x, y] = c.stored_px;
                let [w, h] = c.stored_dims;
                !x.is_finite()
                    || !y.is_finite()
                    || !w.is_finite()
                    || !h.is_finite()
                    || x < 0.0
                    || y < 0.0
                    || w <= 0.0
                    || h <= 0.0
            })
        {
            self.mask_warp_center = None;
        }
        // The tag and the data must agree, and the tag is the one that carries
        // a REASON — so it wins. A hand-edited file claiming `fisheye_refused`
        // beside warp knots is claiming two contradictory things, and
        // honouring the knots would render a warp whose provenance line says it
        // was refused. Enforced, not trusted: `clamp` is what every reader runs.
        if !self.mask_warp_src.is_solved() {
            self.mask_warp.clear();
        } else if self.mask_warp.len() < 2 {
            // A solved source with no usable spline is not solved. One knot is
            // a constant and zero is nothing; either way the reason is gone, so
            // say the honest thing rather than interpolate over a stub.
            self.mask_warp.clear();
            self.mask_warp_src = MaskWarpSource::Unparseable;
        }
        // This second map has exactly one honest state: the sidecar disabled
        // correction, so RADIAL needs identity while LINEAR still needs the
        // camera map for its two raw-frame handles. In every solved state the
        // ordinary `mask_warp` is the source; in every refusal there is no map
        // to retain. Two or more knots are required for a real spline.
        if self.mask_warp_src != MaskWarpSource::DisabledInSidecar
            || self.linear_handle_warp.len() < 2
        {
            self.linear_handle_warp.clear();
        }
    }
}

/// Normalised crop rectangle. All values in [0.0, 1.0], with (0,0) at the
/// top-left of the full frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// A single point on the master tone curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurvePoint {
    /// Input value, 0..=255.
    pub input: u8,
    /// Output value, 0..=255.
    pub output: u8,
}

/// The 8 ACR colour bands, in the order the [`Hsl`] arrays are indexed. These
/// map 1:1 to Lightroom's `crs:{Hue,Saturation,Luminance}AdjustmentRed` … keys.
pub const HSL_BANDS: [&str; 8] =
    ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];

/// Per-colour HSL adjustments — Lightroom's HSL / Color mixer. Each array is
/// indexed by [`HSL_BANDS`] (red, orange, yellow, green, aqua, blue, purple,
/// magenta). Values -100..=100, 0 = no change: `hue` rotates a band, `saturation`
/// changes its intensity, `luminance` its brightness. This is the single biggest
/// "look" control the global sliders cannot express (e.g. teal-foliage / orange-skin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hsl {
    pub hue: [f32; 8],
    pub saturation: [f32; 8],
    pub luminance: [f32; 8],
}

impl Default for Hsl {
    fn default() -> Self {
        Self { hue: [0.0; 8], saturation: [0.0; 8], luminance: [0.0; 8] }
    }
}

impl Hsl {
    /// True when every band on every axis is neutral (lets the render + XMP skip it).
    pub fn is_neutral(&self) -> bool {
        self.hue.iter().chain(&self.saturation).chain(&self.luminance).all(|&v| v == 0.0)
    }

    /// Clamp every band to the documented -100..=100 range. Non-finite
    /// (foreign XMP text like HueAdjustmentRed="NaN") collapses to neutral 0:
    /// f32::clamp KEEPS NaN, which would poison the render arithmetic and
    /// later serialise as null into an unreadable recipe.
    pub fn clamp(&mut self) {
        for arr in [&mut self.hue, &mut self.saturation, &mut self.luminance] {
            for v in arr.iter_mut() {
                *v = if v.is_finite() { v.clamp(-100.0, 100.0) } else { 0.0 };
            }
        }
    }
}

/// Lightroom's Color Grading (the 3-wheel + global model that supersedes Split
/// Toning). Each tonal region (shadow/midtone/highlight) plus a global wheel gets
/// a `hue` (0..=360°), `sat` (0..=100), and `lum` (-100..=100). `blending` (0..=100,
/// default 50) sets how much the regions overlap; `balance` (-100..=100) shifts the
/// shadow/highlight split. Default = neutral. ACR XMP convention (verified against
/// the user's own sidecar): shadow/highlight hue+sat round-trip via the legacy
/// `crs:SplitToning*` keys, everything else via `crs:ColorGrade*`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ColorGrade {
    pub shadow_hue: f32,
    pub shadow_sat: f32,
    pub shadow_lum: f32,
    pub midtone_hue: f32,
    pub midtone_sat: f32,
    pub midtone_lum: f32,
    pub highlight_hue: f32,
    pub highlight_sat: f32,
    pub highlight_lum: f32,
    pub global_hue: f32,
    pub global_sat: f32,
    pub global_lum: f32,
    pub blending: f32,
    pub balance: f32,
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self {
            shadow_hue: 0.0, shadow_sat: 0.0, shadow_lum: 0.0,
            midtone_hue: 0.0, midtone_sat: 0.0, midtone_lum: 0.0,
            highlight_hue: 0.0, highlight_sat: 0.0, highlight_lum: 0.0,
            global_hue: 0.0, global_sat: 0.0, global_lum: 0.0,
            blending: 50.0, // ACR default
            balance: 0.0,
        }
    }
}

impl ColorGrade {
    /// True when no wheel tints or lifts (sat + lum all zero) — render + XMP skip it.
    /// `blending`/`balance` alone do nothing without a saturated or lifted wheel.
    pub fn is_neutral(&self) -> bool {
        [
            self.shadow_sat, self.shadow_lum, self.midtone_sat, self.midtone_lum,
            self.highlight_sat, self.highlight_lum, self.global_sat, self.global_lum,
        ]
        .iter()
        .all(|&v| v == 0.0)
    }

    /// Clamp every wheel to its documented range (hue 0..360, sat 0..100, lum/balance
    /// -100..100, blending 0..100).
    pub fn clamp(&mut self) {
        // Non-finite → the field's NEUTRAL (blending's is 50): clamp and
        // rem_euclid both pass NaN straight through (see Hsl::clamp).
        let fin = |v: f32, neutral: f32| if v.is_finite() { v } else { neutral };
        // A corrupt HUE makes the whole wheel meaningless — zeroing the hue
        // alone turned "NaN hue + sat 50" into a saturated RED grade; the
        // corrupt-component-goes-INERT rule needs the paired sat zeroed too.
        for (h, sat) in [
            (&mut self.shadow_hue, &mut self.shadow_sat),
            (&mut self.midtone_hue, &mut self.midtone_sat),
            (&mut self.highlight_hue, &mut self.highlight_sat),
            (&mut self.global_hue, &mut self.global_sat),
        ] {
            if !h.is_finite() {
                *h = 0.0;
                *sat = 0.0;
            }
        }
        for h in [&mut self.shadow_hue, &mut self.midtone_hue, &mut self.highlight_hue, &mut self.global_hue] {
            *h = fin(*h, 0.0).rem_euclid(360.0);
        }
        for s in [&mut self.shadow_sat, &mut self.midtone_sat, &mut self.highlight_sat, &mut self.global_sat] {
            *s = fin(*s, 0.0).clamp(0.0, 100.0);
        }
        for l in [&mut self.shadow_lum, &mut self.midtone_lum, &mut self.highlight_lum, &mut self.global_lum] {
            *l = fin(*l, 0.0).clamp(-100.0, 100.0);
        }
        self.blending = fin(self.blending, 50.0).clamp(0.0, 100.0);
        self.balance = fin(self.balance, 0.0).clamp(-100.0, 100.0);
    }
}

/// A local (masked) adjustment: *where* it applies (`mask`) plus the slider
/// deltas to apply inside that mask. Sliders use the SAME UI scale as the global
/// [`EditRecipe`] fields; the XMP writer converts to ACR's local scale (exposure
/// stops/4, other sliders /100). `temperature` here is a *relative* shift, not
/// Kelvin (maps to `crs:LocalTemperature`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LocalAdjustment {
    pub mask: MaskGeometry,
    /// Extra shapes composed onto `mask` in order (Add / Subtract /
    /// Intersect — Lightroom's mask-component grammar). Empty = the base
    /// geometry alone (v1-compatible). ENGINE-ONLY: see [`MaskComponent`].
    pub components: Vec<MaskComponent>,
    /// Lightroom's per-mask eye toggle: `false` mutes the adjustment without
    /// touching its tuned Amount (dragging Amount to 0 as a mute DESTROYED
    /// the tuned value). Disabled masks render nothing, show no coverage,
    /// don't gate exports, and are SKIPPED by the XMP projection (the recipe
    /// keeps them; a re-enable is one click — same lossy-projection stance
    /// as Bitmap masks).
    pub enabled: bool,
    /// Optional Range Mask refinement intersected with the COMBINED coverage
    /// (final weight = combined geometry × range). `None` = pure geometry
    /// (v1-compatible).
    pub range: Option<RangeMask>,
    /// Human label → `crs:CorrectionName` / `crs:MaskName`.
    pub name: String,
    /// Master opacity 0.0..=1.0 → `crs:CorrectionAmount` + `crs:MaskValue`.
    pub amount: f32,
    /// Invert the mask region → `crs:MaskInverted`.
    pub inverted: bool,
    pub exposure_ev: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    /// Local midtone local-contrast → `crs:LocalClarity2012`. ENGINE-RENDERED
    /// since R22 (before that these three were carried and exported but never
    /// drawn, so a mask that moved only them did nothing in-app — user feedback
    /// #15a/#10B); see `render::apply_masks` for the pass order.
    pub clarity: f32,
    /// Local atmospheric-veil removal → `crs:LocalDehaze`. Shares the global
    /// haze model (`render::dehaze_airlight` + `dehaze_px`); the airlight is
    /// estimated once per frame, so mask order cannot change it.
    pub dehaze: f32,
    /// Local fine-detail contrast → `crs:LocalTexture`. Rendered as a
    /// small-radius unsharp mask; unlike clarity there is no global Texture
    /// stage to align its radius with, so the scaling is our own calibration
    /// (Adobe's model is not published) — Lightroom re-renders from the raw
    /// slider value in the XMP.
    pub texture: f32,
    /// Local capture sharpening → `crs:LocalSharpness`, -100..=100 and SIGNED
    /// (ACR's local Sharpness band): positive sharpens, negative SOFTENS. The
    /// radius is the global `sharpening` stage's own σ model, so "Sharpness 40"
    /// means the same structure globally and inside a mask. Added in R23-1b —
    /// the writer emitted a literal `"0"` for this key from the first sidecar
    /// on, so a photographer could soften a background in Lightroom but never
    /// in this app.
    pub sharpness: f32,
    pub saturation: f32,
    /// Local HUE ROTATION → `crs:LocalHue`, -100..=100 mapped to ±30° (the
    /// same scale as the global 8-band mixer's hue axis, `render::apply_hsl`).
    /// Rotates every hue inside the mask instead of one band across the whole
    /// frame — the one shape of colour move a global mixer cannot make. Added
    /// in R23-1b beside `sharpness`, for the same reason.
    pub hue: f32,
    /// Relative warm/cool shift (NOT Kelvin) → `crs:LocalTemperature`.
    pub temperature: f32,
    pub tint: f32,
    /// Local luminance noise reduction, 0..=100 → `crs:LocalLuminanceNoise`.
    /// For "this region is noisy" requests; smooths only inside the mask.
    pub noise_reduction: f32,
    /// This mask's own master point curve → `crs:MainCurve`. Same `{input,
    /// output}` 0..=255 points as the global [`EditRecipe::tone_curve`], and
    /// composed through the very same LUT builder (`render::build_tone_lut`),
    /// so "a curve" means one thing everywhere in this engine.
    ///
    /// **The key is the BARE name.** Lightroom writes the global curves as
    /// `crs:ToneCurvePV2012{,Red,Green,Blue}` but the local ones as
    /// `crs:MainCurve` / `RedCurve` / `GreenCurve` / `BlueCurve` — no
    /// `PV2012` suffix — as child elements of the Correction, between
    /// `crs:LocalCurveRefineSaturation` and `crs:CorrectionMasks`. Their point
    /// payload is spelled `x,y` with NO SPACE after the comma, where the
    /// global curves' is `x, y` WITH one; `xmp::local_curve_elem` is a
    /// separate writer for exactly that reason.
    ///
    /// The four are SPARSE and INDEPENDENT — a real sidecar carries Red and
    /// Green with no Main and no Blue — so this is four plain vectors, not one
    /// four-curve struct. Empty = identity, and the two ways a vector can
    /// arrive empty (absent from the JSON, or written as `[]`) MEAN THE SAME
    /// THING: unlike `coord_era` / `Radial::midpoint`, whose type default is a
    /// legitimate value and therefore needs a field-level `#[serde(default =
    /// …)]`, an empty curve is "no curve" from either source. Do not copy that
    /// pattern here.
    pub main_curve: Vec<CurvePoint>,
    /// This mask's RED channel curve → `crs:RedCurve`. See [`main_curve`].
    ///
    /// [`main_curve`]: LocalAdjustment::main_curve
    pub red_curve: Vec<CurvePoint>,
    /// This mask's GREEN channel curve → `crs:GreenCurve`. See [`main_curve`].
    ///
    /// [`main_curve`]: LocalAdjustment::main_curve
    pub green_curve: Vec<CurvePoint>,
    /// This mask's BLUE channel curve → `crs:BlueCurve`. See [`main_curve`].
    ///
    /// [`main_curve`]: LocalAdjustment::main_curve
    pub blue_curve: Vec<CurvePoint>,
    /// Per-channel LINEAR-light gains for zone RECOLOURING, `1.0` = neutral.
    /// Produced by the zoned reverse-fit (fit_zoned.rs): a palette-transplant
    /// target (pale-blue sky → gold) demands channel ratios far beyond what
    /// any white-balance parametrisation can express (measured: blue→gold
    /// needs r/b ≈ 5.3×; the full 2000–40000 K blackbody range caps at
    /// ≈ 1.9×), so the fit writes the exact gains instead. ENGINE-ONLY: no
    /// classic-ACR counterpart exists, so the XMP writer cannot carry it —
    /// the fit only attaches it to Bitmap-masked corrections, which classic
    /// XMP skips anyway (see [`MaskGeometry::Bitmap`]). Composes
    /// multiplicatively with the temperature/tint gains in the engine.
    pub color_gains: Option<[f32; 3]>,
    /// Which semantic zone produced this mask, a STABLE identity independent of
    /// the free-text `name` (which the GUI now translates, so it can no longer
    /// be an equality key). `Custom` for every user/AI mask; the zoned
    /// reverse-fit tags its two masks `ZoneSky`/`ZoneLand` (fit_zoned.rs). Like
    /// `color_gains`, ENGINE-ONLY — the fit only sets it on Bitmap masks, which
    /// the XMP writer skips, so it never reaches a sidecar (round-trips only in
    /// the app-internal `recipe.json`).
    pub role: MaskRole,
}

impl Default for LocalAdjustment {
    fn default() -> Self {
        Self {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 0.5 },
            components: Vec::new(),
            enabled: true,
            range: None,
            name: String::new(),
            amount: 1.0,
            inverted: false,
            exposure_ev: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            texture: 0.0,
            sharpness: 0.0,
            saturation: 0.0,
            hue: 0.0,
            temperature: 0.0,
            tint: 0.0,
            noise_reduction: 0.0,
            main_curve: Vec::new(),
            red_curve: Vec::new(),
            green_curve: Vec::new(),
            blue_curve: Vec::new(),
            color_gains: None,
            role: MaskRole::Custom,
        }
    }
}

/// Which semantic zone a local adjustment came from — a STABLE mask identity
/// that survives UI translation and user renaming (the display `name` is free
/// text and, since i18n, no longer a reliable key). Set by the zoned
/// reverse-fit (fit_zoned.rs); `Custom` for every user-placed or AI-segmented
/// mask. Serialised in `recipe.json` as `"custom"` / `"zone_sky"` /
/// `"zone_land"`; never written to XMP (see [`LocalAdjustment::role`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskRole {
    /// A user-placed or AI-segmented mask — identity carried by `name` only.
    #[default]
    Custom,
    /// Sky zone from the zoned reverse-fit (rides the upright sky raster).
    ZoneSky,
    /// Land zone from the zoned reverse-fit (rides the INVERTED sky raster).
    ZoneLand,
}

impl MaskRole {
    /// ASCII tag for the fit's rationale text (`"custom"` / `"sky"` / `"land"`).
    pub fn tag(self) -> &'static str {
        match self {
            MaskRole::Custom => "custom",
            MaskRole::ZoneSky => "sky",
            MaskRole::ZoneLand => "land",
        }
    }

    /// Canonical English display name for a zone mask, or `None` for `Custom`
    /// (whose label is the user's own `name`). The GUI runs this through i18n
    /// so the mask row localises alongside the rest of the UI.
    pub fn en_name(self) -> Option<&'static str> {
        match self {
            MaskRole::Custom => None,
            MaskRole::ZoneSky => Some("Sky (reverse-fit)"),
            MaskRole::ZoneLand => Some("Land (reverse-fit)"),
        }
    }
}

/// Where a local adjustment applies. Coordinates are normalised to the frame and
/// MAY fall outside [0,1] for gradients (matching ACR's geometry).
//
// FORWARD COMPATIBILITY, class 3 (R27 L-17, closing the registration at
// docs/ROADMAP.md's v0.31.0 entry). The old comment here read "serde does not
// support `deny_unknown_fields` on internally-tagged enums" — that is wrong,
// and the cost of believing it was the only SILENT forward break this project
// ever shipped: v0.31.0 added `midpoint` + `mask_version` to `Radial`, and a
// v0.30 binary reading such a recipe dropped both without a word and wrote the
// truncated geometry back, quietly editing the user's file. Classes 1 and 2
// (a new top-level `EditRecipe` field, a new `LocalAdjustment` curve) are LOUD
// because those containers deny; this one was not, purely for want of the
// attribute.
//
// The attribute placed on the CONTAINER covers every variant and exempts the
// tag itself (probed against serde 1.0.228 + serde_json 1.0.150 before landing;
// pinned by `an_unknown_radial_field_is_a_loud_refusal`). Consequence, stated
// so it is a decision rather than a surprise: a build that meets a recipe from
// a NEWER build now REFUSES it by name instead of silently truncating it —
// same posture as `EditRecipe`'s own `deny_unknown_fields`, and the reason
// downgrades are not supported. The AI's own schema is a strict SUBSET of
// these fields (`advisor::catalogue::mask_geometry_schema` /
// `range_mask_schema`, both `additionalProperties: false`), so no proposal
// shape changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaskGeometry {
    /// Linear gradient — the zero→full vector sets direction + falloff width.
    /// Maps to ACR `What="Mask/Gradient"`.
    Linear { zero_x: f32, zero_y: f32, full_x: f32, full_y: f32 },
    /// Radial/elliptical gradient. Maps to ACR `What="Mask/CircularGradient"`.
    /// `feather` IS engine-rendered (clamped to 0..1). `roundness` is **carried
    /// only** — persisted, XMP round-tripped and accepted from the AI, but NOT
    /// rendered: the engine draws a pure ellipse. Since R29 B7 (2026-08-20)
    /// that is Lightroom's MEASURED behaviour at `Roundness = +100` with
    /// `Feather = 0` — a hand-authored probe rendered pixel-identically to
    /// its Roundness=0 reference — not just a refusal to guess; see
    /// `mask_weight` in render.rs. Negatives and the Roundness×Feather cross
    /// term are still unmeasured, so the value stays carried verbatim.
    ///
    /// Its DOMAIN is no longer a guess either: `crs:Roundness` is Lightroom's
    /// ±100 integer slider (all 24 radials in the harvested real-sidecar corpus
    /// write a bare signed integer, every one of them at the default `0`), not
    /// the 0..1 aspect ratio this field was once clamped to. v0.31.1 widened
    /// the clamp and the importer's gate to ±100 so a user who moved that
    /// slider keeps their mask (docs/V2_PLAN.md §7 item 11).
    ///
    /// `angle` (degrees, ENGINE convention: CLOCKWISE on screen about the bbox
    /// centre, 0 = axis-aligned — normalised frame coords are y-DOWN, so the
    /// very same rotation reads counter-clockwise in a y-UP maths frame, which
    /// is the reading this line used to print while claiming the screen one.
    /// Measured on the released binary's own output, not derived: E1-verdict
    /// §4a, R25 P9) IS rendered and GUI-editable, and is still OUR OWN field
    /// rather than `crs:Angle` — but v0.32.0 mapped the two onto each other in
    /// both directions. They are not the same number: `crs:Angle` is a tilt in
    /// PIXEL space and this one is a rotation applied in the NORMALISED frame,
    /// which differ by up to 11.2° of rendered tilt over the ±44° range real
    /// sidecars use. The fold between them is `xmp::lr_to_engine` /
    /// `xmp::engine_to_lr`, and it needs the frame's aspect ratio; a document
    /// that declares none still exports the UNROTATED ellipse and discloses the
    /// angle it could not write.
    ///
    /// The bbox is likewise no longer the sidecar's bbox: Lightroom's stored
    /// corners are the ROTATED corners of the ellipse's box, and the whole
    /// normalised frame is scaled by `xmp::LR_MASK_FRAME_SCALE` between the two
    /// conventions. What is stored HERE is the engine's own ellipse; the
    /// projection to and from Lightroom's is `xmp.rs`'s alone.
    ///
    /// `midpoint` and `mask_version` are the two attributes Lightroom writes on
    /// EVERY radial that this engine could not even read until R25 P5 — see
    /// their own docs. Both are CARRIED: read from the sidecar, kept in
    /// `recipe.json`, written back unchanged, and never consulted by
    /// `mask_weight`.
    Radial {
        top: f32,
        left: f32,
        bottom: f32,
        right: f32,
        feather: f32,
        roundness: f32,
        flipped: bool,
        #[serde(default)]
        angle: f32,
        /// Lightroom's `crs:Midpoint` (0..100, ACR default 50): where along the
        /// radial falloff the half-strength point sits — a second shaping knob
        /// beside `feather`. CARRIED ONLY: the engine's falloff is `feather`'s
        /// alone, so honouring this would reshape every imported mask on a
        /// guess (the roundness rule). Round-trips so a sidecar we rewrite
        /// still says what Lightroom said.
        ///
        /// FRAME-INDEPENDENT: a ratio along the radial axis, not a coordinate,
        /// so the `coord_era` migration (`render::orient_recipe_coords`) does
        /// not touch it — the same reason a curve point is not migrated.
        ///
        /// KNOWN BOUNDARY: this field and `mask_version` are deliberately NOT
        /// in the AI's mask-geometry schema (`advisor::catalogue`) — a model
        /// cannot know a value it never saw, and a required property is a
        /// property it would invent. So a REFINE returns the geometry with
        /// both at their serde defaults.
        ///
        /// R25 left that as a permanent cost; R27 (L-09, user ruling
        /// 2026-08-19 =「修」) closed it WITHOUT the widening R25 feared:
        /// `pipeline::carry_radial_carried_attributes` re-attaches the pair
        /// from the refine base in a pass of its own, so `schema_loses` — the
        /// state-bearing predicate that decides which masks take the
        /// wholesale-revert path — is untouched. The schema stays silent about
        /// both fields, which is still the right shape: the model is not asked
        /// for a number it cannot know, and the pipeline puts the
        /// photographer's own back. The GUI's ↻ Redraw preserved them all
        /// along (canvas.rs).
        #[serde(default = "radial_midpoint_centre")]
        midpoint: f32,
        /// Lightroom's `crs:Version` on the mask component (2 in every
        /// reference sidecar) — the geometry SCHEMA version Lightroom stamps,
        /// not our recipe's. CARRIED verbatim and never interpreted: dropping
        /// it made a rewritten sidecar claim a version Lightroom had not
        /// written, and inventing a meaning for it would be worse. Not
        /// clamped for the same reason — a value we do not read has no range
        /// to enforce.
        #[serde(default = "lightroom_mask_version")]
        mask_version: u32,
    },
    /// Free-form raster mask — the carrier for AI subject/sky segmentation and
    /// any painted selection. `path` names an 8-bit image whose LUMINANCE is
    /// the weight (white = full effect), sampled bilinearly in normalised
    /// coordinates so one file drives both the small preview and the full-res
    /// export. Lives in the ORIGINAL frame like the parametric geometries.
    /// Classic ACR XMP cannot express raster masks — the sidecar writer skips
    /// these (parametric masks still round-trip; positioning tradeoff like the
    /// §B retouch-master limitation). A missing/unreadable file renders the
    /// mask inert (weight 0) with a stderr warning rather than failing.
    Bitmap { path: String },
    /// Lightroom's BRUSH group — one `crs:What="Mask/Aggregate"` component and
    /// the `Mask/Paint` strokes it contains (R27 Batch-4, L-08).
    ///
    /// **RENDERED since R29 Batch-6b, from a measured model** — and until then
    /// it was carried and NOT rendered, which is a hard behaviour change for
    /// every recipe that holds one. `render::mask_weight` answered a literal
    /// `0.0` for this geometry from R27 Batch-4 until that batch, because the
    /// one input a rasteriser needs is the alpha kernel and the sidecar does
    /// not contain it (see [`BrushStroke::dabs`]). R29 Batch-6 MEASURED the
    /// kernel instead of guessing at it — 29 controlled Lightroom exports, a
    /// nine-rung hardness ladder and a 5 × 2 × 2 flow × radius × hardness grid
    /// of drags — so `render::brush_raster` now stamps the dab stream into an
    /// alpha and the geometry draws. **Nothing in this file changed for it:**
    /// the raster is a render-time artefact, so there is no new field, no
    /// schema bump and no `schema_era` gate. Both disclosures moved with the
    /// behaviour and now name the approximation rather than the absence
    /// (`MaskImportReason::BrushRendered` / `MaskLossReason::BrushRendered`).
    ///
    /// **The encoding, as re-derived** (current corpus: 177 sidecars of
    /// the user's own library, real XML parser, 0 parse failures; 42
    /// `Mask/Aggregate` and 398 `Mask/Paint` instances):
    ///
    ///  * an Aggregate is a ONE-LEVEL group. Its children are `Mask/Paint`,
    ///    398/398 — never a Gradient, a Radial, a RangeMask, an Image or
    ///    another Aggregate — and the maximum component nesting depth in the
    ///    whole library is exactly 2. `Mask/Paint` is NEVER a top-level
    ///    correction component (398/398 of the in-correction Paints are
    ///    children of an Aggregate).
    ///  * the composition mode relative to the rest of the correction sits on
    ///    the GROUP: `(blend_mode, value, inverted)` measured as `(1, 0, false)`
    ///    ×22, `(0, 1, false)` ×16, `(1, 0, true)` ×1 — Lightroom's subtract
    ///    pair and its plain add, the same encoding v0.31.1 taught the reader
    ///    for parametric components.
    ///  * the strokes INSIDE only ever union with each other: `MaskBlendMode`
    ///    is `"0"` on 398/398 Paints and `MaskInverted` `"false"` on 398/398.
    ///    Anything else is someone else's writer and is refused, not guessed
    ///    (the roundness rule).
    ///
    /// The TOTALS above (177 sidecars, 42 Aggregates, 398 Paints, 398
    /// in-correction Paints) are corpus measurements, not schema fields; the
    /// invariants they describe are enforced independently by the parser.
    ///
    /// **NEWER LIGHTROOM MAY STORE THESE STROKES IN A COMPANION `.acr`.**
    /// Lightroom 18.4 can replace text `Mask/Paint` children with a
    /// `MaskBrushTable` reference on the Aggregate. Before R30 B2, the two
    /// rewritten specimens failed loudly as `OutOfModel` (one owning
    /// correction in `_DSC9206`, two in `_DSC8904`); their table strokes did
    /// not render. The path-aware XMP reader now imports the adjudicated ACR
    /// grammar under strict bounds and reports one of nine named refusal
    /// classes when any unestablished form is encountered. Within an
    /// Aggregate, table records precede its surviving text Paint children in
    /// document order; gesture Paints remain with their gesture owner.
    ///
    /// The binary table is deliberately not recipe state. An unchanged
    /// path-aware merge reparses the base sidecar, recognizes that the recipe
    /// mask is unchanged, and preserves the original Aggregate block verbatim.
    /// Its table attributes and residual Paints therefore ride through without
    /// synthesizing a second text representation of the table-derived strokes.
    ///
    /// All four numbers ride out into the sidecar exactly as they rode in, so
    /// a brush mask this app imports and republishes still says what Lightroom
    /// said. `blend_mode` is therefore stored HERE as well as being projected
    /// onto the owning [`MaskComponent`]'s [`MaskCombine`] on import: the
    /// component's mode is what the renderer composes with, and this is what
    /// the WRITER re-emits. They are two spellings of one fact and the import
    /// is the only place that maps between them.
    Brush {
        /// `crs:MaskName` on the Aggregate — always "Brush *n*" in some UI
        /// language (`画笔 1` ×23, `Brush 1` ×7, `画笔 2` ×6, …). Carried so a
        /// republished sidecar keeps the photographer's own label.
        name: String,
        /// `crs:MaskBlendMode` on the Aggregate, VERBATIM. 0 = union onto the
        /// correction's coverage, 1 = subtract (paired with `value` 0).
        blend_mode: u32,
        /// `crs:MaskValue` on the Aggregate, VERBATIM — the other half of the
        /// subtract pair, never a strength (see `component_import_reasons` in
        /// xmp.rs for why reading it as one neutralises real masks).
        value: f32,
        /// `crs:MaskInverted` on the Aggregate.
        inverted: bool,
        /// The group's `Mask/Paint` children, in DOCUMENT ORDER. Order is
        /// load-bearing for the eventual raster (dabs accumulate), so it is
        /// preserved rather than sorted.
        strokes: Vec<BrushStroke>,
    },
    /// Lightroom's AI mask — one `crs:What="Mask/Image"` component (R27
    /// Batch-5, L-08 Arm C). The INTENT is imported and carried; the alpha is
    /// **RECOMPUTED on this machine by our own segmenter**, and that is a
    /// different thing from an import.
    ///
    /// **Why this cannot be an import.** F2's anatomy enumerated the full
    /// attribute vocabulary over 105 real instances (the current corpus count,
    /// the wider library for this batch, 21 attribute names, one child element)
    /// and found **no raster payload and no geometry payload** — the longest
    /// attribute value anywhere on one is 55 characters. The sidecar names
    /// WHICH segmentation to run (`MaskSubType` + `MaskName` +
    /// `ReferencePoint`), WHICH model produced the original (`ModelVersion`,
    /// three digests), and the proxy frame it ran in (`FullMaskSize`,
    /// `WholeImageArea`, `Origin`). Nothing else. Adobe recomputes the alpha on
    /// apply; so must we, with a different model.
    ///
    /// **So the disclosure is mandatory and it runs in both directions.** What
    /// this engine renders is OUR segmenter's alpha, not Adobe's raster: the
    /// coverage will differ at every edge and can differ grossly on a hard
    /// scene. `MaskImportReason::AiMaskRecomputed` says so on the way in and
    /// `MaskLossReason::AiMaskRecomputed` on the way out, in both languages.
    /// A sidecar/model failure leaves `raster` at `None`, which renders the
    /// mask INERT (weight 0, exactly like a `Bitmap` whose file is missing) and
    /// is disclosed as carried-not-rendered — never a silent zero, never a
    /// crash.
    ///
    /// **What rides back out** is the component Lightroom wrote, verbatim: the
    /// intent fields below plus `provenance`, which holds the attributes this
    /// engine never interprets. A republished sidecar therefore still says what
    /// Lightroom said, and Lightroom recomputes its own alpha from it — which
    /// is the one round trip that IS lossless here.
    AiMask {
        /// `crs:MaskName` — a localised user string (`天空 1`, `Sky 1`,
        /// `背景1`, `水源`). Carried so a republished sidecar keeps the
        /// photographer's own label; also the only hint that a `subtype == 0`
        /// mask meant *Background* rather than *Object*, which is why that
        /// guess is NOT made here (see `subtype`).
        name: String,
        /// `crs:MaskSubType` — **0 = Object/Background, 1 = Subject, 2 = Sky**
        /// (decoded by cross-tabulating against `MaskName` over 105 instances;
        /// still exactly three values).
        ///
        /// Object and Background share the value 0 and the sidecar does not
        /// separate them: `crs:MaskSubCategoryID` appears on 8 of 105 with two
        /// values, far too few to call an enum. So the segmenter is pointed at
        /// the OBJECT under the click point in both cases, and the possibility
        /// that the photographer meant its complement is disclosed rather than
        /// guessed — the roundness rule.
        subtype: u32,
        /// `crs:ReferencePoint` — the user's own click, normalised to the
        /// frame, present on 105/105. This is the ONLY spatial information the
        /// component carries, and it is what a point-promptable segmenter
        /// (SAM 2.1) consumes directly.
        ref_x: f32,
        ref_y: f32,
        /// `crs:MaskBlendMode` on the component, VERBATIM — 0 = union onto the
        /// correction's coverage, 1 = subtract (paired with `value` 0). Same
        /// encoding, same reading and same "carried for the writer, projected
        /// onto `MaskCombine` for the render" split as
        /// [`MaskGeometry::Brush::blend_mode`].
        blend_mode: u32,
        /// `crs:MaskValue`, VERBATIM — the other half of the subtract pair,
        /// never a strength.
        value: f32,
        /// `crs:MaskInverted`.
        inverted: bool,
        /// `crs:MaskVersion` — `"1"` on 105/105. Lightroom's own schema stamp
        /// for the component, carried and never interpreted, exactly like
        /// [`MaskGeometry::Radial::mask_version`].
        mask_version: u32,
        /// Every remaining `crs:` attribute the file carried, in DOCUMENT
        /// ORDER, as `(local name, value)` — the three digests and their
        /// version tags, `WholeImageArea`, `FullMaskSize`, `Origin`,
        /// `ModelVersion`, `ErrorReason`, `MaskSubCategoryID`.
        ///
        /// **PROVENANCE ONLY. Nothing here is read.** `FullMaskSize`
        /// (`"2880,1920"`) is the resolution Adobe's segmenter worked at,
        /// `Origin` is the mask raster's bbox origin in those proxy pixels, and
        /// `WholeImageArea` is the proxy frame's EFFECTIVE IMAGE AREA — four
        /// rationals in `(top, left, bottom, right)` order. R29 me3-c
        /// (2026-08-21) measured both halves of that sentence, because the
        /// version it replaces called `WholeImageArea` the crop rect and it is
        /// not one: the specimen carries `HasCrop="True"` with a visible crop
        /// and still writes `"0/1,0/1,1920/1,2880/1"` = the whole proxy frame,
        /// and across the library 85 of 94 values are the full frame while the
        /// 9 inset ones pair with a SHORTER `FullMaskSize` (`"2880,1913"`) —
        /// the signature of a lens-geometry inset, not of a user crop. The
        /// frame those numbers live in is the UNCROPPED one: projecting the
        /// decoded alpha through `FullMaskSize` as an uncropped frame and then
        /// applying `crs:Crop*` lands it on the subject including flyaway hair,
        /// while reading `FullMaskSize` as the cropped frame lands it visibly
        /// small and displaced. They still describe a raster this engine does
        /// not have and would still mislead a renderer that consulted them.
        /// They are kept so the sidecar round-trips and so a future measurement
        /// has the numbers, not so anything can act on them.
        ///
        /// A key outside the measured vocabulary is REFUSED by the parser
        /// rather than carried ([`xmp`]'s `AI_MASK_PROVENANCE_KEYS`): an
        /// open-ended attribute bag read off disk and written into XML is an
        /// injection surface, and a name we have never seen means a writer we
        /// have not measured.
        ///
        /// [`xmp`]: crate::xmp
        provenance: Vec<(String, String)>,
        /// The `crs:Gesture` child's `Mask/Paint` strokes — the user's brush
        /// REFINEMENT of the AI mask (present on 40 of 105 components, exactly
        /// one Paint each in the current corpus census; the count is
        /// gate-checked by `scripts/check_docs.py` (the shape it describes is
        /// not). Re-measured on the
        /// library as it stands
        /// 2026-08-21: 40 gesture blocks over 10 files, one Paint each (40/40),
        /// every one of them on `MaskSubType="0"`.
        ///
        /// Carried, not composited by the renderer — and since R29 me3-c / me5
        /// (2026-08-21) that is MEASURED in both subtypes rather than registered
        /// as unknown. There is no render-time overlay to miss: a gesture is a
        /// segmentation input, not paint added to a finished alpha.
        ///
        ///  * `MaskSubType="1"` (Subject): a hand-authored `crs:Gesture` is
        ///    accepted as a first-class object — Lightroom re-serialises it,
        ///    keeps `BrushGestureInterpretation`, and silently recomputes past
        ///    the stale digests — and contributes EXACTLY ZERO to the render.
        ///    The two full-size exports are pixel-identical (39 M px,
        ///    max|Δ| = 0) and their entropy-coded segments are byte-identical.
        ///  * `MaskSubType="0"` (Object): the gesture IS read, but as an INPUT
        ///    to the segmentation — the region prompt that says which object.
        ///    Deleting it changed 511,020 px (max|Δ| = 74 DN) inside a bbox
        ///    that contains all 12 dabs, while the same frame's two Sky masks
        ///    (subtype 2, no gesture) stayed bit-identical outside it. The
        ///    component was not dropped; Lightroom recomputed a DIFFERENT alpha
        ///    from the remaining inputs.
        ///
        /// R30 B3 now uses the measured subtype-0 meaning: the ReferencePoint is
        /// the first positive SAM point, followed by every ordered `d x y` dab
        /// from every Paint, with duplicates preserved. `r/f/h`, MaskValue,
        /// Radius, Flow and hardness remain state, not points or weights; there
        /// are no negative labels, boxes, centroids, sampling or dense prompts.
        /// Subtype 1/2 gestures remain transport-only and never steer their
        /// backends. This is a segmentation-INPUT fidelity improvement, not a
        /// render gap — an AI alpha is declared a local re-derivation either way.
        /// Carrying it is still what makes the corrections whose ONLY brush
        /// content is a gesture importable at all.
        ///
        /// Carried, and since R29 C1 also TURNED: these are `BrushStroke`s and
        /// they ride `render::orient_recipe_coords`' rewrite with the rest.
        /// They are written back into the sidecar, so leaving them in the old
        /// frame would hand Lightroom a refinement stroke beside a
        /// `ReferencePoint` that had moved.
        gesture: Vec<BrushStroke>,
        /// The RECOMPUTED alpha, once our segmenter has produced one — a path
        /// to an 8-bit grey PNG inside the photo's develop dir, keyed by
        /// (photo, subtype, reference point, frame, backend generation), plus a
        /// scoped hash of the exact sent point list for subtype-0 gestures, so a
        /// re-render reuses it instead of re-running the model
        /// (`segment::resolve_ai_masks`).
        ///
        /// Machine-local, like every other raster path in this file: it joins
        /// [`LocalAdjustment::bitmap_paths_mut`], so the store relativizes,
        /// resolves, detaches and snapshots it with the rest. `None` = not
        /// resolved yet, or the model declined — which renders inert and is
        /// disclosed, never silently zero.
        ///
        /// NOT written to the sidecar: what Lightroom gets back is the intent.
        #[serde(default)]
        raster: Option<String>,
    },
}

/// One `crs:What="Mask/Paint"` stroke inside a [`MaskGeometry::Brush`] group.
///
/// Exactly nine attributes on 398/398 in-correction instances, no optional
/// fields and no variation: `What`, `MaskActive`, `MaskBlendMode`,
/// `MaskInverted`, `MaskSyncID`, `MaskValue`, `Radius`, `Flow`, `CenterWeight`,
/// plus exactly one child element, `crs:Dabs`. The three whose value is an
/// invariant (`MaskActive="true"`, `MaskBlendMode="0"`, `MaskInverted="false"`)
/// are NOT fields here: a constant is not data, and the reader refuses a
/// component that breaks one rather than storing the surprise.
///
/// Denies unknown fields for the same reason [`MaskGeometry`] does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrushStroke {
    /// `crs:MaskValue` — a genuine 0..1 stroke DENSITY here, not the subtract
    /// half-pair: `MaskBlendMode` is 0 on every Paint, so the pairing rule
    /// cannot apply. Measured `"1"` ×305 and 10 distinct fractional values on
    /// the rest. This is the missing member of the "`BlendMode=0` with
    /// `MaskValue≠1`" bucket the R25 census counted and could not explain.
    pub value: f32,
    /// `crs:Radius` — the brush size in WIDTH units (a dab is a circle in
    /// PIXELS, hence semi-axes `(r, r·W/H)` in the normalised frame). The
    /// component attribute is the stream's INITIAL state; an `r` token
    /// overrides it. 102 components carry no `r` token at all and every one of
    /// them has a non-zero radius here, so the value is never undefined.
    pub radius: f32,
    /// `crs:Flow` — initial flow state. Exactly redundant with the stream's
    /// first `f` token wherever one exists (72/72).
    pub flow: f32,
    /// `crs:CenterWeight` — HARDNESS on 0..1, the same quantity the stream's
    /// `h` token sets (the two observed `h` values, 1.0000 and 0.0006, are
    /// members of this attribute's own value set to 4 dp).
    pub center_weight: f32,
    /// `crs:MaskSyncID` as the file spelled it. Carried so `recipe.json`
    /// remembers Lightroom's own identity for the stroke; the WRITER mints a
    /// fresh one exactly like it does for every other component it emits, so
    /// this never travels back out (see `masks_xml`).
    pub sync_id: String,
    /// The `crs:Dabs` token stream, VERBATIM — one token per line, in the
    /// order the file lists its `<rdf:li>` items.
    ///
    /// **Historical grammar measurement** (22,966 tokens over 382 components, zero
    /// malformed): four forms and no others —
    ///
    /// | token | meaning |
    /// |---|---|
    /// | `r <f>` | set the current radius |
    /// | `f <f>` | set the current flow |
    /// | `h <f>` | set the current hardness |
    /// | `d <x> <y>` | stamp a dab at (x, y) with the current state |
    ///
    /// Initial state comes from the attributes above; tokens override in
    /// stream order. The polyline is ALREADY DENSIFIED by Lightroom at
    /// 0.2000 · r (15,582 consecutive-dab steps, IQR [0.1998, 0.2001], and
    /// **zero steps exceed 1.0 r — there are no pen-lifts inside a Paint**), so
    /// a renderer interpolates nothing: it stamps the dabs it is given. The
    /// coordinate frame is the PLAIN image frame, `k = 1.00001 ± 2×10⁻⁵` —
    /// measured HERE first, and since the 2026-08-19 ruling also what
    /// `xmp::LR_MASK_FRAME_SCALE` says of radial corners (1.0: the old 1.032
    /// was one frame's lens-profile warp, not a frame constant — the
    /// constant's own comment carries the evidence). Both facts are calibrated
    /// against Lightroom's own `crs:pm_*` pixel boxes on 79 spots, sub-pixel.
    ///
    /// **Why this is a STRING and not a parsed `Vec<Dab>`.** The one thing the
    /// sidecar does not carry is the alpha KERNEL — the falloff as a function
    /// of hardness, and the per-dab accumulation law. The file stores the
    /// stroke, never the alpha, so no amount of parsing recovers it, and the
    /// only published model for it is a third-party decompile reconstruction
    /// whose `Density` field does not exist in any of those historical components.
    /// Structuring the stream before that measurement existed would freeze a
    /// shape around a renderer nobody has written; carrying it verbatim kept
    /// the round trip exact meanwhile. R27 Batches 8-10 then MADE the
    /// measurement (screen accumulation; density scales each dab BEFORE the
    /// screen; a one-parameter flow odds law; an 11-rung hardness kernel TABLE
    /// with no closed form).
    ///
    /// **Verbatim is no longer unconditional** (R29 C1, the 2026-08-21 ruling).
    /// A TURN rewrites this stream numerically — every `d` coordinate through
    /// `render::orient_point`, every `r` token and [`radius`](Self::radius)
    /// rescaled by the frame aspect, because a dab is a circle in PIXELS while
    /// the radius is in width units. The alternative was a mask that renders in
    /// the wrong place beside every parametric shape that moved, and the render
    /// is what the ruling protected. The re-emitted numbers keep Lightroom's
    /// own six decimals (`render::LR_DAB_DECIMALS`), so the file stays legal
    /// and reads back identically — but a ROTATED photo's republished stream is
    /// no longer byte-identical to what Lightroom wrote, and neither is a
    /// PORTRAIT capture's, whose sensor→display→sensor projection passes
    /// through the same rewrite twice. An unrotated landscape photo is
    /// untouched: the identity orientations return before the rewrite.
    ///
    /// ~~What still gates RENDERING, never carrying, is the kernel's missing
    /// closed form~~ — **CLOSED 2026-08-21 (R29 Batch-6), and the stream is
    /// still a String for the reason above: this type is the sidecar's shape,
    /// not the renderer's.** The closed form is
    /// `k(ρ;h) = (1 − ρ^m(h))^n(h)` with `ln m` and `ln n` cubic in the
    /// hardness — 8 numbers, pooled rms 0.0102 and held-out 0.0109 against a
    /// nine-rung measured table that the table's own interpolation only
    /// reaches 0.0180 on — and the flow law is re-measured at κ = 0.1284 ±
    /// 0.0029 (universal to 2.24 % across a 3× radius change and both hardness
    /// ends; batch-10's single-cell 0.12189 sits 5.3 % low and the two must not
    /// be quoted as agreeing better than that). `render::brush_dabs` runs this
    /// state machine at develop time and `render::brush_raster` stamps it;
    /// neither writes anything back here. (docs/V2_PLAN.md §7 item 13;
    /// `~/.claude/plans/r29-materials/b6-analysis.md`.)
    ///
    /// ~~plus Lightroom rasterising the mask in its PRE-lens-correction
    /// frame~~ — that half is CLOSED (R29 Batch-3). Lightroom does rasterise
    /// a brush pre-correction, and so does this engine: `render::apply_masks`
    /// runs before `render::apply_lens_geometry`, so the geometry stage
    /// carries a brush mask exactly as it carries the pixels. These
    /// coordinates are therefore already in the frame a renderer would want
    /// them in, and warping them would apply the lens field twice. The
    /// measurements, the per-mask-type frame table and the two regression
    /// pins are in `render.rs`'s mask-warp block header.
    pub dabs: String,
}

impl Default for BrushStroke {
    fn default() -> Self {
        // Lightroom's own neutrals for a stroke that says nothing: full
        // density, full flow, a soft edge, no dabs. Not the TYPE's defaults —
        // `flow: 0.0` would be a stroke that paints nothing, which is a
        // legitimate value and therefore the wrong reading of "absent" (the
        // same trap `radial_midpoint_centre` documents).
        Self {
            value: 1.0,
            radius: 0.0,
            flow: 1.0,
            center_weight: 0.0,
            sync_id: String::new(),
            dabs: String::new(),
        }
    }
}

/// Serde default for [`MaskGeometry::Radial::midpoint`] — deliberately NOT the
/// TYPE's default: `f32::default()` is 0.0, which on Lightroom's 0..100 scale
/// is a legitimate, extreme value ("the falloff's half point sits at the very
/// centre"), not "unset". A radial written before this field existed carries
/// ACR's neutral 50, so that is what its absence has to read as — the same
/// field-level-default-beats-container-default trap `coord_era_legacy`
/// documents, and the reason `#[serde(default)]` alone is wrong here.
/// Pinned by `radial_extras_are_dropped_by_an_older_reader_but_never_corrupt`.
fn radial_midpoint_centre() -> f32 {
    50.0
}

/// Serde default for [`MaskGeometry::Radial::mask_version`] — 2, the value
/// every reference Lightroom sidecar carries, and again not the type's 0
/// (which would claim a schema version nobody has ever written). Same
/// rationale as [`radial_midpoint_centre`].
fn lightroom_mask_version() -> u32 {
    2
}

/// How one extra [`MaskComponent`] composes onto the coverage built so far
/// (Lightroom's Add / Subtract / Intersect grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskCombine {
    /// Union: `w = 1 − (1−w)(1−c)` — smooth screen-union, so two feathered
    /// shapes overlap without a hard seam (plain `max` creases the gradient
    /// where the shapes cross).
    #[default]
    Add,
    /// Carve out: `w = w · (1 − c)` — "everything so far, minus this shape".
    Subtract,
    /// Restrict: `w = w · c` — "everything so far, only where this shape is".
    Intersect,
}

/// One extra shape composed onto a [`LocalAdjustment`]'s base `mask`, in list
/// order: coverage starts at the base geometry's weight and each component
/// folds in via its [`MaskCombine`] mode. ENGINE-ONLY like `color_gains`: the
/// classic-ACR XMP writer projects only the base geometry (a multi-component
/// `crs:CorrectionMasks` group needs `crs:MaskBlendMode` semantics we have no
/// verified reference sidecar for — the roundness rule: never reshape masks
/// on a guess), so combinations round-trip in recipe.json only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MaskComponent {
    pub geometry: MaskGeometry,
    pub mode: MaskCombine,
}

impl Default for MaskComponent {
    fn default() -> Self {
        Self {
            geometry: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 0.5 },
            mode: MaskCombine::Add,
        }
    }
}

/// Every finite-coordinate check the sanitizer needs for one geometry —
/// shared by the base `mask` and each component.
fn geometry_is_finite(g: &MaskGeometry) -> bool {
    match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            [zero_x, zero_y, full_x, full_y].iter().all(|v| v.is_finite())
        }
        MaskGeometry::Radial { top, left, bottom, right, feather, roundness, angle, .. } => {
            [top, left, bottom, right, feather, roundness, angle].iter().all(|v| v.is_finite())
        }
        MaskGeometry::Bitmap { .. } => true,
        // Every number a stroke carries, including the ones nothing renders
        // yet: a NaN that survives here reaches the sidecar writer, and "NaN"
        // is not something Lightroom can parse back.
        MaskGeometry::Brush { value, strokes, .. } => {
            value.is_finite()
                && strokes.iter().all(|s| {
                    [s.value, s.radius, s.flow, s.center_weight].iter().all(|v| v.is_finite())
                })
        }
        // The reference point is a real COORDINATE — it decides where the
        // segmenter clicks — so a NaN here would put the prompt nowhere. The
        // gesture strokes get the same treatment as a brush group's for the
        // same reason: they reach the sidecar writer, and "NaN" is not
        // something Lightroom can parse back.
        MaskGeometry::AiMask { ref_x, ref_y, value, gesture, .. } => {
            [ref_x, ref_y, value].iter().all(|v| v.is_finite())
                && gesture.iter().all(|s| {
                    [s.value, s.radius, s.flow, s.center_weight].iter().all(|v| v.is_finite())
                })
        }
    }
}

fn range_is_finite(range: &Option<RangeMask>) -> bool {
    match range {
        Some(RangeMask::Luminance { lo_outer, lo, hi, hi_outer }) => {
            lo_outer.is_finite() && lo.is_finite() && hi.is_finite() && hi_outer.is_finite()
        }
        Some(RangeMask::Color { r, g, b, amount, px, py }) => {
            [r, g, b, amount, px, py].iter().all(|v| v.is_finite())
        }
        None => true,
    }
}

/// Coordinate/fraction clamp for one geometry — shared by the base `mask`
/// and each component (see the COORD_LIMIT rationale at the call site).
fn clamp_geometry(g: &mut MaskGeometry) {
    const COORD_LIMIT: f32 = 8.0;
    match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            for v in [zero_x, zero_y, full_x, full_y] {
                *v = v.clamp(-COORD_LIMIT, COORD_LIMIT);
            }
        }
        MaskGeometry::Radial { top, left, bottom, right, feather, roundness, angle, midpoint, .. } => {
            for v in [top, left, bottom, right] {
                *v = v.clamp(-COORD_LIMIT, COORD_LIMIT);
            }
            // Lightroom's Midpoint is 0..100. A corrupt one goes to its
            // NEUTRAL rather than through `clamp` (which passes NaN straight
            // through — see `Hsl::clamp`): this value is written back into a
            // sidecar Lightroom reads, and "NaN" there is not a number it can
            // parse. `mask_version` is deliberately NOT clamped — a value we
            // never interpret has no range to enforce, and rewriting it would
            // make our sidecar claim a version Lightroom never wrote.
            *midpoint = if midpoint.is_finite() { midpoint.clamp(0.0, 100.0) } else { 50.0 };
            // `feather` is a 0..1 fraction everywhere that reads it (render,
            // XMP); a stored 2.0 was a value the engine silently treated as
            // 1.0 but persistence kept.
            *feather = feather.clamp(0.0, 1.0);
            // `roundness` is NOT on that scale. It is Lightroom's ±100 slider,
            // carried and never interpreted (see `MaskGeometry::Radial`), so
            // the clamp is the SIDECAR's band — the old 0..1 clamp would have
            // crushed a real `crs:Roundness="-30"` to 0 on the way back out,
            // which is how a carried value stops being carried. v0.31.1.
            *roundness = roundness.clamp(-100.0, 100.0);
            // Rotation wraps — ±180° covers every ellipse orientation twice
            // over (the shape has 180° symmetry), and an unbounded stored
            // angle would feed sin/cos a huge argument for no extra shapes.
            *angle = angle.rem_euclid(360.0);
            if *angle > 180.0 {
                *angle -= 360.0;
            }
        }
        MaskGeometry::Bitmap { .. } => {}
        // A brush group is CARRIED, so the bands here are the SIDECAR's, not a
        // render's: every one of the 382 measured Paints already sits inside
        // them, which makes this a guard against a hand-edited recipe.json and
        // nothing else. `blend_mode` is deliberately not touched — it is a
        // Lightroom enum we re-emit verbatim, and there is no "nearest legal
        // value" for an enum (same rule `mask_version` is left alone under).
        MaskGeometry::Brush { value, strokes, .. } => {
            let unit = |v: &mut f32, neutral: f32| {
                *v = if v.is_finite() { v.clamp(0.0, 1.0) } else { neutral };
            };
            unit(value, 1.0);
            for s in strokes.iter_mut() {
                unit(&mut s.value, 1.0);
                unit(&mut s.flow, 1.0);
                unit(&mut s.center_weight, 0.0);
                // Radius is a normalised length, so it takes the coordinate
                // limit rather than the unit band — the measured maximum is
                // 0.582, but a brush wider than the frame is a shape, not a
                // corruption.
                s.radius = if s.radius.is_finite() { s.radius.clamp(0.0, COORD_LIMIT) } else { 0.0 };
            }
        }
        // An AI mask's bands are the SIDECAR's plus one that is the RENDER's:
        // the reference point is a normalised click that becomes a pixel
        // coordinate the segmenter is prompted at, so it takes the coordinate
        // limit rather than a unit band (a click just outside the frame is a
        // thing Lightroom writes — the sidecar's own values run to 3 decimal
        // places inside [0,1], but the segment bridge is what decides what is
        // reachable, not this clamp). `blend_mode`, `mask_version` and every
        // `provenance` string are left alone for `MaskGeometry::Brush`'s
        // reason: there is no "nearest legal value" for an enum, a schema stamp
        // or an opaque digest.
        MaskGeometry::AiMask { ref_x, ref_y, value, gesture, provenance, .. } => {
            for v in [ref_x, ref_y] {
                *v = if v.is_finite() { v.clamp(-COORD_LIMIT, COORD_LIMIT) } else { 0.5 };
            }
            *value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 1.0 };
            for s in gesture.iter_mut() {
                for (v, neutral) in
                    [(&mut s.value, 1.0f32), (&mut s.flow, 1.0), (&mut s.center_weight, 0.0)]
                {
                    *v = if v.is_finite() { v.clamp(0.0, 1.0) } else { neutral };
                }
                s.radius = if s.radius.is_finite() { s.radius.clamp(0.0, COORD_LIMIT) } else { 0.0 };
            }
            // A hand-edited recipe.json must not be able to make one component
            // cost an unbounded allocation, or to smuggle a novel attribute
            // into a sidecar. The COUNT is bounded here; the KEYS are an
            // allowlist in the parser and again in the writer.
            provenance.truncate(MAX_AI_PROVENANCE_ENTRIES);
            for (k, v) in provenance.iter_mut() {
                truncate_chars(k, MAX_AI_PROVENANCE_KEY_CHARS);
                truncate_chars(v, MAX_AI_PROVENANCE_VALUE_CHARS);
            }
        }
    }
}

/// Bounds for [`MaskGeometry::AiMask::provenance`] — the measured vocabulary is
/// 11 optional attributes, the longest value in the whole reference library is
/// 55 characters (`crs:WholeImageArea`), and the longest name is
/// `LocalInputDigestVersion` at 23. Each cap leaves room without leaving the
/// door open.
const MAX_AI_PROVENANCE_ENTRIES: usize = 16;
const MAX_AI_PROVENANCE_KEY_CHARS: usize = 48;
const MAX_AI_PROVENANCE_VALUE_CHARS: usize = 128;

/// Truncate on a CHARACTER boundary — `String::truncate` panics mid-codepoint,
/// and these strings come off disk where a multi-byte value is legal.
fn truncate_chars(s: &mut String, max: usize) {
    if s.chars().count() > max {
        let cut = s.char_indices().nth(max).map_or(s.len(), |(i, _)| i);
        s.truncate(cut);
    }
}

impl LocalAdjustment {
    /// Every geometry this adjustment carries — the base mask and each
    /// component — as ONE walk.
    ///
    /// The two path walks below differ only in which geometries they accept, so
    /// the walk itself lives here once: a new geometry carrier is added in a
    /// single place and neither membership rule can silently miss it.
    fn geometries_mut(&mut self) -> impl Iterator<Item = &mut MaskGeometry> {
        std::iter::once(&mut self.mask).chain(self.components.iter_mut().map(|c| &mut c.geometry))
    }

    /// Mutable references to every Bitmap raster path this adjustment holds
    /// (base geometry + components) — the ONE walk the store's path
    /// relativize/resolve/detach/snapshot helpers share, so a new geometry
    /// carrier can never be forgotten by one of them.
    pub fn bitmap_paths_mut(&mut self) -> Vec<&mut String> {
        // TWO carriers now (R27 Batch-5): the explicit `Bitmap` raster and the
        // RECOMPUTED alpha an `AiMask` caches. They are the same kind of thing
        // to every consumer of this walk — a machine-local PNG path that must
        // be relativized on save, resolved on load, copied when a version
        // snapshot becomes live, and swept with a deleted version — so the
        // AiMask one goes through the same door rather than growing a second.
        self.geometries_mut()
            .filter_map(|g| match g {
                MaskGeometry::Bitmap { path } => Some(path),
                MaskGeometry::AiMask { raster: Some(path), .. } => Some(path),
                _ => None,
            })
            .collect()
    }

    /// The subset of [`bitmap_paths_mut`](Self::bitmap_paths_mut) a FRAME TURN
    /// owns: the explicit `Bitmap` rasters, and NOT an `AiMask`'s cached alpha.
    ///
    /// The distinction is the root of R28 Batch-3 3b (adjudication F8-B). The
    /// two walks answer different questions and the rotate was asking the wrong
    /// one: `bitmap_paths_mut` means "machine-local PNGs this recipe owns and
    /// the STORE must carry along", while a rotate needs "rasters that must be
    /// turned or the photo comes back with its masks somewhere else". A
    /// `Bitmap` qualifies — its pixels are the mask, and nothing can re-derive
    /// them. An `AiMask`'s raster does not: it is a CACHE of a recomputation,
    /// and the rotation's own semantics already discard it in both of the
    /// places that decide (`render::orient_recipe_coords` clears it so the next
    /// develop re-segments at the turned point, and `render::
    /// recipe_has_raster_masks` refuses to count it as something the migration
    /// could not turn). Turning it too produced a correctly-turned file nothing
    /// ever pointed at, plus a `rasters_turned` count that promised work the
    /// recipe did not keep.
    pub fn turnable_raster_paths_mut(&mut self) -> Vec<&mut String> {
        self.geometries_mut()
            .filter_map(|g| match g {
                MaskGeometry::Bitmap { path } => Some(path),
                _ => None,
            })
            .collect()
    }
}

/// Lightroom's Range Mask: a per-pixel refinement INTERSECTED with the mask's
/// geometry (final weight = geometry × range), so a sky gradient can affect only
/// the bright pixels, or only the blues. Serialised to XMP as a second
/// `Mask/RangeMask` component inside `crs:CorrectionMasks` — structure verified
/// against the user's own Lightroom sidecars (e.g. `_DSC9245.xmp` LumRange,
/// `_DSC9303.xmp` PointModels).
// Denies unknown fields for the same reason [`MaskGeometry`] does, and in the
// same breath: the two are this crate's only internally-tagged enums, so a
// fix that covered one of them would have left the identical silent-drop hole
// open in the other.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RangeMask {
    /// Select by luminance: full weight inside [lo, hi], smooth ramps over
    /// lo_outer→lo and hi→hi_outer. All four in 0..=1, non-decreasing —
    /// exactly ACR's `crs:LumRange="lo_outer lo hi hi_outer"`.
    Luminance { lo_outer: f32, lo: f32, hi: f32, hi_outer: f32 },
    /// Select pixels whose chromaticity (brightness-independent colour) is near
    /// the reference `r,g,b` (0..=1 sRGB). `amount` 0..=1 widens the tolerance
    /// (ACR `crs:ColorAmount`, LR default 0.5). `(px, py)` is the normalised
    /// sample point in the ORIGINAL frame — cosmetic, for LR's sample marker.
    Color { r: f32, g: f32, b: f32, amount: f32, px: f32, py: f32 },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClampSummary {
    pub dropped_masks: usize,
    pub dropped_components: usize,
    /// Points cut from the tone/RGB/base curves past their caps — a curve
    /// truncation changes rendered TONE exactly as a dropped mask changes
    /// coverage, and it used to vanish without a count.
    pub truncated_curve_points: usize,
    /// Bytes cut from rationale / mask names / raster paths. A truncated
    /// PATH is load-bearing (the raster stops resolving); the count keeps
    /// the disclosure honest even for the cosmetic strings.
    pub truncated_string_bytes: usize,
}

impl ClampSummary {
    pub fn is_empty(self) -> bool {
        self == ClampSummary::default()
    }

    /// Fold another summary in — the accumulator restore paths use. Field-by-
    /// field copies at call sites silently dropped the curve/string counts the
    /// moment those fields were added (16-lane scan, L14/L15/L16).
    pub fn absorb(&mut self, other: ClampSummary) {
        self.dropped_masks += other.dropped_masks;
        self.dropped_components += other.dropped_components;
        self.truncated_curve_points += other.truncated_curve_points;
        self.truncated_string_bytes += other.truncated_string_bytes;
    }

    /// English stderr rendering listing only the non-zero losses — "0 mask(s)
    /// and 0 component(s)" for a curve-only truncation was a false all-clear.
    /// GUI surfaces use their own localized 4-placeholder message instead.
    pub fn describe(self) -> String {
        let mut parts = Vec::new();
        if self.dropped_masks > 0 {
            parts.push(format!("{} mask(s)", self.dropped_masks));
        }
        if self.dropped_components > 0 {
            parts.push(format!("{} mask component(s)", self.dropped_components));
        }
        if self.truncated_curve_points > 0 {
            parts.push(format!("{} curve point(s)", self.truncated_curve_points));
        }
        if self.truncated_string_bytes > 0 {
            parts.push(format!("{} string byte(s)", self.truncated_string_bytes));
        }
        parts.join(", ")
    }
}

/// Proof-of-sanitisation token (arch item c): constructing it is the ONE
/// place entry-point clamping happens, so the public render surfaces stop
/// hand-rolling clone+clamp+disclose triplets that had already drifted.
/// Derefs to the recipe — internal render functions keep their signatures.
pub struct ValidatedRecipe {
    recipe: EditRecipe,
    pub dropped: ClampSummary,
}

impl ValidatedRecipe {
    pub fn new(r: &EditRecipe) -> Self {
        let mut recipe = r.clone();
        let dropped = recipe.clamp();
        ValidatedRecipe { recipe, dropped }
    }

    /// Say what sanitisation cost, on the caller's channel — entry points call
    /// this once; silence is reserved for the recipe that lost nothing.
    ///
    /// **R29-1 found this one.** It is raised by all four render entry points,
    /// so a `batch --jobs 3` worker reaches it per photo — and the R28 Batch-5
    /// 5c attribution sweep missed it entirely, because that sweep re-walked
    /// `pipeline.rs`, `render.rs` and `main.rs` and this line lives here. It
    /// therefore printed with no photograph on it at all while its twin in
    /// `pipeline::recipe_bytes_for` carried a stem. Taking the channel fixes
    /// both halves at once: the subject rides with it, and the caller decides
    /// where it lands.
    pub fn disclose(&self, diag: &crate::diag::Diag<'_>) {
        let d = self.dropped;
        if !d.is_empty() {
            diag.emit(
                crate::diag::Mark::WarningWord,
                format!(
                    "recipe limits discarded {} mask(s) and {} mask component(s), \
                     truncated {} curve point(s) and {} string byte(s) before rendering",
                    d.dropped_masks,
                    d.dropped_components,
                    d.truncated_curve_points,
                    d.truncated_string_bytes
                ),
            );
        }
    }
}

impl std::ops::Deref for ValidatedRecipe {
    type Target = EditRecipe;
    fn deref(&self) -> &EditRecipe {
        &self.recipe
    }
}

impl EditRecipe {
    /// Clamp every slider into its documented legal range. The AI is
    /// instructed to stay in range, but we never trust the input blindly —
    /// an out-of-range value would otherwise corrupt the render downstream.
    pub fn clamp(&mut self) -> ClampSummary {
        let mut summary = ClampSummary::default();
        // f32::clamp passes a NaN receiver STRAIGHT THROUGH — a non-finite
        // slider from malformed input (hand-edited JSON, foreign XMP) must
        // collapse to the neutral 0.0 instead, the same corrupt-component-
        // goes-inert rule the crop and range-mask guards below follow.
        let c = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
        // SIZE limits, not just value limits. A recipe is also a set of
        // VECTORS, and a crafted one (hand-edited JSON, a hostile POST to the
        // local web server) could carry thousands of masks or curve points:
        // each active mask costs a full-frame pass and each curve is cloned +
        // sorted per render, so the render thread can be monopolised without a
        // single out-of-range number. The caps sit far above any real edit
        // (the GUI offers a handful of masks; a tone curve has at most a few
        // dozen points), so they can only ever truncate abuse.
        //
        // …and STRINGS, which this comment used to claim were covered ("every
        // field below is bounded") while bounding none of them. That was not
        // theoretical. `rationale` is filled from the advisor's failure text,
        // and an upstream HTTP error body flows into it verbatim
        // (`AdvisorError::Http{body}` -> `fallback_reason` -> the heuristic's
        // rationale), so a misbehaving endpoint needed no malice to write a
        // multi-megabyte string into recipe.json AND into the XMP comment
        // beside the RAW. Past 16 MiB `store::read_sidecar` then refuses to
        // read that sidecar back, which silently costs the Lightroom merge its
        // base. The web route's own 256 MiB body cap is the only other ceiling.
        const MAX_MASKS: usize = 64;
        const MAX_CURVE_POINTS: usize = 256;
        const MAX_BASE_KNOTS: usize = 256;
        /// An abuse bound, not a disclosure budget. The layered zoned
        /// fit's typed notes (17-bin range refusals, per-generation tile
        /// sweeps, refinement readings) legitimately reach ~6 KB on the
        /// calibration pair; 4096 truncated the tile ATTACHMENT disclosure
        /// off the persisted recipe while the masks stayed. Sized so honest
        /// writer output never loses its tail, while a hand-edited foreign
        /// recipe.json still cannot smuggle a payload.
        const MAX_RATIONALE: usize = 16 * 1024;
        /// A mask label is a UI affordance; Lightroom's own are far shorter.
        const MAX_NAME: usize = 256;
        /// A path, not a payload — comfortably past Windows' extended limit.
        const MAX_PATH: usize = 4096;
        /// Strokes in ONE brush group. The largest Aggregate in the reference
        /// library holds 14; 256 is abuse-only headroom, sized like MAX_MASKS.
        const MAX_BRUSH_STROKES: usize = 256;
        /// One stroke's `crs:Dabs` stream. The largest real stroke is 645 dabs
        /// / 1,267 tokens ≈ 13 KB, and the WHOLE library is 15,964 dabs — so
        /// 256 KiB is twenty times the worst case and still a bound.
        const MAX_DABS: usize = 256 * 1024;
        /// Truncate on a char boundary: `String::truncate` panics off one, and
        /// this runs on input nobody validated. Returns the bytes cut so the
        /// summary can report string loss instead of hiding it.
        fn cap(s: &mut String, max: usize) -> usize {
            let before = s.len();
            if s.len() > max {
                let mut end = max;
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                s.truncate(end);
            }
            before - s.len()
        }
        /// [`cap`] for a `'\n'`-separated TOKEN stream — today only
        /// [`BrushStroke::dabs`]. Cuts at the last separator at or before the
        /// byte cap, so every survivor is a WHOLE token.
        ///
        /// Why `cap` is the wrong tool here (R28 2b, adjudication F5). A char
        /// boundary is not a token boundary: 65,536 `"d 0 0"` tokens are
        /// 393,215 bytes, `MAX_DABS` is 262,144, and `cap` cut that stream
        /// mid-token (`"d 0 "`). The writer then emitted the fragment
        /// unchanged — its `split('\n')` is documented as the EXACT INVERSE of
        /// the reader's join (xmp.rs `brush_mask_xml`) — and our own next read
        /// refused the fragment, taking the whole Aggregate with it: every
        /// brush mask in the group disappeared. Structured payload, byte
        /// truncator; the units never matched.
        ///
        /// The cap is therefore a CEILING, not an exact size. The result is
        /// the longest whole-token prefix that fits, which is at most `max`
        /// bytes and can be shorter by up to one token. A stream whose FIRST
        /// token already exceeds `max` truncates to empty — the caller drops
        /// such a stroke rather than emit a dab-less Paint (a Paint with no
        /// dabs is not a stroke; the reader says so by refusing one).
        ///
        /// Returns the bytes cut, like `cap`.
        fn cap_tokens(s: &mut String, max: usize) -> usize {
            let before = s.len();
            if s.len() > max {
                // `max < s.len()`, so `max` is a valid byte index. '\n' is
                // ASCII, hence always a char boundary — truncating AT the
                // separator drops it too, which is the storage form (tokens
                // joined by '\n', no trailing one).
                match s.as_bytes()[..=max].iter().rposition(|&b| b == b'\n') {
                    Some(nl) => s.truncate(nl),
                    None => s.clear(),
                }
            }
            before - s.len()
        }
        /// One brush group's strokes, bounded — sync IDs and each stroke's
        /// token stream. Shared by [`cap_geometry`]'s `Brush` and `AiMask`
        /// arms, because a carrier bounded in one and trusted in the other is
        /// the hole that function exists to keep shut. Returns
        /// `(bytes cut, strokes dropped)`.
        ///
        /// A stroke whose stream truncates to EMPTY is dropped, not kept: the
        /// writer would otherwise emit `<rdf:li></rdf:li>` and our own reader
        /// refuses a Paint with zero tokens (`xmp::parse_dabs`), which loses
        /// the whole Aggregate — the identical failure `cap_tokens` exists to
        /// prevent, arriving one step later. Only a hand-edited `recipe.json`
        /// can reach it: on the import path no single token survives 256 bytes
        /// (`xmp::dab_token_is_known`), so a 256 KiB budget always fits
        /// hundreds of them.
        fn cap_strokes(strokes: &mut Vec<BrushStroke>, name_max: usize) -> (usize, usize) {
            let mut bytes = 0;
            for s in strokes.iter_mut() {
                bytes += cap(&mut s.sync_id, name_max);
                bytes += cap_tokens(&mut s.dabs, MAX_DABS);
            }
            let before = strokes.len();
            strokes.retain(|s| !s.dabs.is_empty());
            (bytes, before - strokes.len())
        }
        /// Every STRING and every VECTOR one geometry carries, bounded —
        /// shared by the base `mask` and by each component, because a carrier
        /// bounded in one of the two loops and trusted in the other is exactly
        /// the hole `bitmap_paths_mut` exists to stop reopening. Returns
        /// `(bytes cut, strokes dropped)`.
        fn cap_geometry(g: &mut MaskGeometry, name_max: usize) -> (usize, usize) {
            match g {
                MaskGeometry::Bitmap { path } => (cap(path, MAX_PATH), 0),
                MaskGeometry::Brush { name, strokes, .. } => {
                    let bytes = cap(name, name_max);
                    let over = strokes.len().saturating_sub(MAX_BRUSH_STROKES);
                    strokes.truncate(MAX_BRUSH_STROKES);
                    let (stroke_bytes, emptied) = cap_strokes(strokes, name_max);
                    (bytes + stroke_bytes, over + emptied)
                }
                // Every string an AI mask carries, including the ones only the
                // WRITER reads. `provenance` is capped by ENTRY COUNT in
                // `clamp_geometry` and by LENGTH here, so a hand-edited
                // recipe.json cannot make one component's XML unbounded from
                // either direction.
                MaskGeometry::AiMask { name, gesture, provenance, raster, .. } => {
                    let mut bytes = cap(name, name_max);
                    if let Some(p) = raster.as_mut() {
                        bytes += cap(p, MAX_PATH);
                    }
                    let over = gesture.len().saturating_sub(MAX_BRUSH_STROKES);
                    gesture.truncate(MAX_BRUSH_STROKES);
                    let (stroke_bytes, emptied) = cap_strokes(gesture, name_max);
                    bytes += stroke_bytes;
                    for (k, v) in provenance.iter_mut() {
                        bytes += cap(k, name_max);
                        bytes += cap(v, name_max);
                    }
                    (bytes, over + emptied)
                }
                MaskGeometry::Linear { .. } | MaskGeometry::Radial { .. } => (0, 0),
            }
        }
        summary.truncated_string_bytes += cap(&mut self.rationale, MAX_RATIONALE);
        // The pass-through block is STRINGS from a foreign document, so it
        // falls under the same rule as `rationale` and the mask names above:
        // the reader fills it from a fixed sixteen keys, but a hand-edited
        // recipe.json is not obliged to. Bounded here rather than trusted —
        // and generously, because the whole promise of these values is that
        // they ride out byte-for-byte (`crs:CameraProfile` is a profile NAME,
        // and Adobe's are long).
        const MAX_PASSTHROUGH: usize = 64;
        const MAX_PASSTHROUGH_VALUE: usize = 512;
        while self.passthrough.len() > MAX_PASSTHROUGH {
            if let Some((_, v)) = self.passthrough.pop_last() {
                summary.truncated_string_bytes += v.len();
            }
        }
        for v in self.passthrough.values_mut() {
            summary.truncated_string_bytes += cap(v, MAX_PASSTHROUGH_VALUE);
        }
        summary.dropped_masks = self.masks.len().saturating_sub(MAX_MASKS);
        self.masks.truncate(MAX_MASKS);
        for m in &mut self.masks {
            summary.truncated_string_bytes += cap(&mut m.name, MAX_NAME);
            // Strokes cut from an over-long brush group count as COMPONENTS:
            // `Mask/Paint` is a component in Lightroom's own vocabulary (it
            // lives in the group's `<crs:Masks>` list), and the existing word
            // says what happened without inventing a fifth counter no
            // disclosure surface would print.
            let (bytes, strokes) = cap_geometry(&mut m.mask, MAX_NAME);
            summary.truncated_string_bytes += bytes;
            summary.dropped_components += strokes;
        }
        summary.truncated_curve_points +=
            self.base_curve.len().saturating_sub(MAX_BASE_KNOTS);
        self.base_curve.truncate(MAX_BASE_KNOTS);
        for curve in [
            &mut self.tone_curve,
            &mut self.red_curve,
            &mut self.green_curve,
            &mut self.blue_curve,
        ] {
            summary.truncated_curve_points += curve.len().saturating_sub(MAX_CURVE_POINTS);
            curve.truncate(MAX_CURVE_POINTS);
        }
        // Base-curve knots are (x, y) in 0..1 and are composed UNDER the user
        // curve — a non-finite or wild pair renders as a black/blown band.
        self.base_curve.retain(|k| k[0].is_finite() && k[1].is_finite());
        for k in self.base_curve.iter_mut() {
            k[0] = k[0].clamp(0.0, 1.0);
            k[1] = k[1].clamp(0.0, 1.0);
        }
        self.exposure_ev = c(self.exposure_ev, -5.0, 5.0);
        self.contrast = c(self.contrast, -100.0, 100.0);
        self.highlights = c(self.highlights, -100.0, 100.0);
        self.shadows = c(self.shadows, -100.0, 100.0);
        self.whites = c(self.whites, -100.0, 100.0);
        self.blacks = c(self.blacks, -100.0, 100.0);
        self.tint = c(self.tint, -100.0, 100.0);
        self.vibrance = c(self.vibrance, -100.0, 100.0);
        self.saturation = c(self.saturation, -100.0, 100.0);
        self.clarity = c(self.clarity, -100.0, 100.0);
        self.dehaze = c(self.dehaze, -100.0, 100.0);
        self.texture = c(self.texture, -100.0, 100.0);
        self.hsl.clamp();
        self.color_grade.clamp();
        self.sharpening = c(self.sharpening, 0.0, 150.0);
        self.noise_reduction = c(self.noise_reduction, 0.0, 100.0);
        // The carried detail axes (R25 B3). The radius keeps its DECIMAL —
        // Lightroom's own band is 0.5..3.0 in tenths and `crs:SharpenRadius`
        // is written with one, so rounding it here would quantise the value
        // the sidecar carries. The other seven are integer sliders.
        // ONE decimal, the grid `crs:SharpenRadius="+1.0"` is written on —
        // rounded here rather than only at the writer, so what the panel
        // shows and what the sidecar carries are the same number (the same
        // rule `post_crop_vignette_style` states below).
        self.sharpen_radius = (c(self.sharpen_radius, 0.0, 3.0) * 10.0).round() / 10.0;
        for v in [
            &mut self.sharpen_detail,
            &mut self.sharpen_mask,
            &mut self.nr_detail,
            &mut self.nr_contrast,
            &mut self.color_nr,
            &mut self.color_nr_detail,
            &mut self.color_nr_smooth,
        ] {
            *v = c(*v, 0.0, 100.0);
        }
        self.lens_vignette = c(self.lens_vignette, -100.0, 100.0);
        self.lens_vignette_mid = c(self.lens_vignette_mid, 0.0, 100.0);
        self.lens_distortion = c(self.lens_distortion, -100.0, 100.0);
        self.ca_r = c(self.ca_r, -100.0, 100.0);
        self.ca_b = c(self.ca_b, -100.0, 100.0);
        // De-fringe: Adobe's own bands (amount 0..20, hue windows 0..100).
        // A hand-edited file cannot push a value outside them and reach the
        // sidecar, exactly as for the effects below.
        self.defringe_purple = c(self.defringe_purple, 0.0, 20.0);
        self.defringe_green = c(self.defringe_green, 0.0, 20.0);
        for v in [
            &mut self.defringe_purple_lo,
            &mut self.defringe_purple_hi,
            &mut self.defringe_green_lo,
            &mut self.defringe_green_hi,
        ] {
            *v = c(*v, 0.0, 100.0);
        }
        // The carried effects (R25 B2): bands are Adobe's own, so a
        // hand-edited value can never reach the sidecar outside them.
        self.post_crop_vignette = c(self.post_crop_vignette, -100.0, 100.0);
        self.post_crop_vignette_mid = c(self.post_crop_vignette_mid, 0.0, 100.0);
        self.post_crop_vignette_feather = c(self.post_crop_vignette_feather, 0.0, 100.0);
        self.post_crop_vignette_round = c(self.post_crop_vignette_round, -100.0, 100.0);
        // An operator INDEX, not a band: 1/2/3 are Adobe's three vignette
        // styles and 0 is "none named". Rounded here rather than only at the
        // writer so what the panel shows and what the sidecar carries are the
        // same number.
        self.post_crop_vignette_style = c(self.post_crop_vignette_style, 0.0, 3.0).round();
        self.post_crop_vignette_hl = c(self.post_crop_vignette_hl, 0.0, 100.0);
        self.grain = c(self.grain, 0.0, 100.0);
        self.grain_size = c(self.grain_size, 0.0, 100.0);
        self.grain_rough = c(self.grain_rough, 0.0, 100.0);
        self.lens_profile.clamp();
        self.straighten_deg = c(self.straighten_deg, -45.0, 45.0);
        // A quarter turn is CYCLIC, so out-of-domain folds rather than
        // saturating: 4 is not "the largest legal turn", it is no turn at all,
        // and clamping it to 3 would invent a three-quarter rotation nobody
        // asked for. The rest of this function clamps because its ranges are
        // intervals; this one is a residue class.
        self.quarter_turns %= 4;
        self.confidence = c(self.confidence, 0.0, 1.0);
        self.temperature_k = match self.temperature_k {
            // Ceiling matches the render engine's blackbody fit validity (kelvin_to_rgb
            // is documented + re-clamped to 1000..40000); keep one source of truth so
            // the recipe never carries a Kelvin the engine would silently re-clamp.
            Some(k) if k.is_finite() => Some(k.clamp(2000.0, 40000.0)),
            // A non-finite Kelvin cannot fall back to the closure's 0.0 (out
            // of range) — corrupt WB goes back to as-shot.
            _ => None,
        };
        // The as-shot anchor obeys the same band (one source of truth with
        // the engine's blackbody fit); corrupt values fall back to "unknown"
        // = the historical 5500 K anchor, never a nonsense anchor.
        self.as_shot_k = match self.as_shot_k {
            Some(k) if k.is_finite() => Some(k.clamp(2000.0, 40000.0)),
            _ => None,
        };
        self.as_shot_tint = match self.as_shot_tint {
            Some(t) if t.is_finite() => Some(t.clamp(-100.0, 100.0)),
            _ => None,
        };
        // Base-look knots are luma coordinates — anything outside [0,1] is
        // corrupt input (monotonisation is the LUT builder's job, values here).
        // A non-finite knot survives f32::clamp: drop it outright.
        self.base_curve.retain(|p| p[0].is_finite() && p[1].is_finite());
        for p in self.base_curve.iter_mut() {
            p[0] = p[0].clamp(0.0, 1.0);
            p[1] = p[1].clamp(0.0, 1.0);
        }
        // Crop is a normalized [0,1] view-frame rectangle; untrusted input
        // (AI JSON, foreign XMP, a hand-edited recipe) can carry values
        // outside the frame or an inverted/empty rectangle. Clamp into the
        // frame, order the edges, and drop a degenerate crop entirely — the
        // geometry stage must never see an empty rectangle.
        if let Some(cr) = &mut self.crop {
            // NaN SURVIVES f32::clamp and defeats every comparison below — a
            // non-finite coordinate (malformed foreign XMP) drops the crop.
            if !(cr.left.is_finite()
                && cr.top.is_finite()
                && cr.right.is_finite()
                && cr.bottom.is_finite())
            {
                self.crop = None;
            } else {
                cr.left = cr.left.clamp(0.0, 1.0);
                cr.top = cr.top.clamp(0.0, 1.0);
                cr.right = cr.right.clamp(0.0, 1.0);
                cr.bottom = cr.bottom.clamp(0.0, 1.0);
                if cr.left > cr.right {
                    std::mem::swap(&mut cr.left, &mut cr.right);
                }
                if cr.top > cr.bottom {
                    std::mem::swap(&mut cr.top, &mut cr.bottom);
                }
                if cr.right - cr.left < 1e-3 || cr.bottom - cr.top < 1e-3 {
                    self.crop = None;
                }
            }
        }
        // Mask GEOMETRY first: a non-finite coordinate (hand-edited or
        // foreign input) makes mask_weight return NaN, which defeats the
        // weight early-out and paints corrupt pixels — drop the whole mask,
        // the same policy as a NaN crop (an unusable geometry has no
        // sensible neutral to collapse to). A corrupt COMPONENT drops the
        // whole mask too: dropping only the component silently changes what
        // the mask covers (a lost Subtract WIDENS the effect area), which is
        // worse than losing the mask outright.
        let masks_before_retain = self.masks.len();
        self.masks.retain(|m| {
            geometry_is_finite(&m.mask) && m.components.iter().all(|c| geometry_is_finite(&c.geometry))
        });
        // A corrupt range changes the combined coverage just as surely as corrupt
        // geometry. Dropping only the range would widen the adjustment.
        self.masks.retain(|m| range_is_finite(&m.range));
        // Size cap for components, same reasoning as MAX_MASKS: each is a
        // per-pixel weight evaluation inside the render's hot loop.
        summary.dropped_masks = summary
            .dropped_masks
            .saturating_add(masks_before_retain.saturating_sub(self.masks.len()));
        const MAX_MASK_COMPONENTS: usize = 16;
        for m in self.masks.iter_mut() {
            summary.dropped_components = summary
                .dropped_components
                .saturating_add(m.components.len().saturating_sub(MAX_MASK_COMPONENTS));
            m.components.truncate(MAX_MASK_COMPONENTS);
            for c in m.components.iter_mut() {
                let (bytes, strokes) = cap_geometry(&mut c.geometry, MAX_NAME);
                summary.truncated_string_bytes += bytes;
                summary.dropped_components += strokes;
            }
        }
        // Finite is NOT sufficient. Mask coordinates are NORMALISED (0..1
        // over the frame, a little outside for handles dragged past the
        // edge), and the engine squares differences: a legal-looking 1e30
        // overflows to Inf, Inf/Inf yields NaN weights, and those pixels
        // quantise to black. Bound the magnitude to the range the UI can
        // actually produce.
        for m in self.masks.iter_mut() {
            clamp_geometry(&mut m.mask);
            for c in m.components.iter_mut() {
                clamp_geometry(&mut c.geometry);
            }
        }
        // Clamp each local adjustment to the same UI ranges as the globals.
        for m in self.masks.iter_mut() {
            // Same finite-or-neutral closure as the globals: a NaN amount
            // used to survive and poison every weighted blend downstream.
            m.amount = c(m.amount, 0.0, 1.0);
            m.exposure_ev = c(m.exposure_ev, -5.0, 5.0);
            for v in [
                &mut m.contrast, &mut m.highlights, &mut m.shadows, &mut m.whites,
                &mut m.blacks, &mut m.clarity, &mut m.dehaze, &mut m.texture,
                &mut m.sharpness, &mut m.saturation, &mut m.hue,
                &mut m.temperature, &mut m.tint,
            ] {
                *v = c(*v, -100.0, 100.0);
            }
            m.noise_reduction = c(m.noise_reduction, 0.0, 100.0);
            // The four local point curves (R25 P6) take the GLOBAL curves'
            // own cap, spelled once above: each is cloned and sorted per
            // render exactly like `tone_curve`, and 64 masks × 4 curves is
            // already the widest surface a crafted recipe has here. Point
            // VALUES need no clamp — `CurvePoint` is a pair of `u8`, so
            // 0..=255 is the type.
            for curve in [
                &mut m.main_curve,
                &mut m.red_curve,
                &mut m.green_curve,
                &mut m.blue_curve,
            ] {
                summary.truncated_curve_points += curve.len().saturating_sub(MAX_CURVE_POINTS);
                curve.truncate(MAX_CURVE_POINTS);
            }
            // Recolour gains: keep each channel strictly positive and inside
            // a generous-but-sane range (0 would kill a channel outright; the
            // real repaint demands measure ≲ 3×). Neutral-ish gains collapse
            // back to None so a hand-rounded recipe stays clean.
            if let Some(g) = &mut m.color_gains {
                for ch in g.iter_mut() {
                    // Neutral gain is 1.0 — the right inert value for a
                    // non-finite channel (0.0 would kill the channel).
                    *ch = if ch.is_finite() { ch.clamp(0.05, 8.0) } else { 1.0 };
                }
                if g.iter().all(|ch| (ch - 1.0).abs() < 1e-3) {
                    m.color_gains = None;
                }
            }
            // Range mask invariants: everything in 0..=1, and the luminance
            // trapezoid non-decreasing (lo_outer ≤ lo ≤ hi ≤ hi_outer) so the
            // render's ramps and ACR's LumRange both stay well-formed.
            // Non-finite ranges were removed with their whole mask above, so
            // every remaining value is safe to clamp without widening coverage.

            match &mut m.range {
                Some(RangeMask::Luminance { lo_outer, lo, hi, hi_outer }) => {
                    let a = lo.clamp(0.0, 1.0);
                    let b = hi.clamp(0.0, 1.0);
                    let (a, b) = if a <= b { (a, b) } else { (b, a) };
                    *lo = a;
                    *hi = b;
                    *lo_outer = lo_outer.clamp(0.0, a);
                    *hi_outer = hi_outer.clamp(b, 1.0);
                }
                Some(RangeMask::Color { r, g, b, amount, px, py }) => {
                    for v in [r, g, b, amount, px, py] {
                        *v = v.clamp(0.0, 1.0);
                    }
                }
                None => {}
            }
        }
        summary
    }

    /// Taste guardrail for **AI-proposed** recipes (never manual edits): keep a
    /// finished develop from over-cooking the tone. Two rules:
    ///  1. **Couple highlight recovery to the white point.** Pulling Highlights
    ///     negative without raising Whites drags specular whites (sea foam, clouds,
    ///     sun glints) to grey — so recovery lifts Whites proportionally. This is the
    ///     principled "keep whites white", at the recipe layer (the renderer stays
    ///     faithful; it does not override the recipe). It is applied as a FLOOR
    ///     *after* rule 2, so rule 2 can never shave it — see the ordering note
    ///     in the body.
    ///  2. **Soft-cap over-aggressive tone moves** toward a tasteful ceiling with a
    ///     smooth knee (not a hard clip), so Highlights/Shadows asymptote near ±70
    ///     and Whites/Blacks near ±45 instead of slamming to the ±100 schema bound.
    ///
    /// `strength` scales rule 2 only — see [`GradeStrength::soft_cap_factor`] for
    /// the formula and [`GradeStrength::CALIBRATED`] for the point at which this
    /// function reproduces the shipped knees exactly. Rule 1 is **not** scaled:
    /// it is not a taste knob but the fix for a measured defect (bd3f9d4 —
    /// Highlights −78.81 with Whites +10.27 dragged sea foam to grey), so scaling
    /// it by strength would re-open that defect at low strength and over-apply it
    /// at high strength.
    pub fn temper(&mut self, strength: GradeStrength) {
        // Smoothly compress a magnitude past `knee` toward `ceil` (C1-continuous at
        // the knee; asymptotes to `ceil`, so |out| < ceil always). Identity below knee.
        fn soft_cap(v: f32, knee: f32, ceil: f32) -> f32 {
            let a = v.abs();
            if a <= knee {
                return v;
            }
            let span = ceil - knee;
            let excess = a - knee;
            v.signum() * (knee + span * (excess / (excess + span)))
        }
        // ONE factor for all four knees, so the calibrated RATIO between the
        // tone pair (50→70) and the point pair (30→45) survives the scaling.
        let f = strength.soft_cap_factor();
        let (tone_knee, tone_ceil) = (TEMPER_TONE_KNEE * f, TEMPER_TONE_CEIL * f);
        let (point_knee, point_ceil) = (TEMPER_POINT_KNEE * f, TEMPER_POINT_CEIL * f);
        // Couple recovery to whites. Read off the ORIGINAL highlights (the
        // recovery the model asked for), and applied as a FLOOR *after* the
        // point soft-cap. NOT scaled by `strength` — see the doc comment:
        // white-point protection is a measured defect fix, not a taste dial.
        //
        // The order is the rule, not an accident (R23 review NIT-2). Running the
        // coupling BEFORE the cap let the cap eat into it at low strength: at
        // s = 0 (knee 19.5, ceiling 29.25) a Highlights of −100 asks for whites
        // 30 and came back 24.56 — the one rule this axis promises never to
        // touch, scaled after all, and invisible to a test that only probed
        // −40. As a floor the promise holds at every strength, and nothing moves
        // at or above the calibration point: the guard's own ceiling is 30 (the
        // ±100 Highlights range caps −h·0.3 there, below the `.min(50)`), which
        // is exactly `TEMPER_POINT_KNEE` at f = 1, and `soft_cap` is the
        // identity below its knee — so max(cap(w), g) == cap(max(w, g)) for
        // every f ≥ 1. `clamp` runs before `temper` at both production call
        // sites (advisor::openai, advisor::heuristic), which is what keeps
        // −h·0.3 inside that 30.
        fn white_point_floor(highlights: f32) -> Option<f32> {
            (highlights < 0.0).then(|| (-highlights * 0.3).min(50.0))
        }
        let guard = white_point_floor(self.highlights);
        self.highlights = soft_cap(self.highlights, tone_knee, tone_ceil);
        self.shadows = soft_cap(self.shadows, tone_knee, tone_ceil);
        self.whites = soft_cap(self.whites, point_knee, point_ceil);
        if let Some(g) = guard {
            self.whites = self.whites.max(g);
        }
        self.blacks = soft_cap(self.blacks, point_knee, point_ceil);
        // Same restraint on each local mask's tone sliders.
        for m in self.masks.iter_mut() {
            // Mask side of the SAME white-point protection — also unscaled, and
            // floored after the cap for the same reason.
            let guard = white_point_floor(m.highlights);
            m.highlights = soft_cap(m.highlights, tone_knee, tone_ceil);
            m.shadows = soft_cap(m.shadows, tone_knee, tone_ceil);
            m.whites = soft_cap(m.whites, point_knee, point_ceil);
            if let Some(g) = guard {
                m.whites = m.whites.max(g);
            }
            m.blacks = soft_cap(m.blacks, point_knee, point_ceil);
        }
    }

    /// Returns true if the recipe leaves the image essentially untouched —
    /// useful to detect a "the AI declined to edit" no-op result.
    pub fn is_noop(&self) -> bool {
        *self == EditRecipe::default()
            // Ignore provenance fields when judging "did it actually edit?".
            // base_curve is the photo's camera-matched BASE (stamped on open),
            // not a user edit — a fresh-open recipe must still count as no-op.
            // The lens profile is the same kind of calibration WHILE it stays
            // exactly as stamped (every available component on); a user toggle
            // away from that is a real edit and must keep the recipe non-noop.
            || EditRecipe {
                rationale: String::new(),
                confidence: 0.0,
                // The era stamp is provenance, not an edit: a recipe saved by
                // an older build is still "no edits" and must still clear
                // rather than pin a permanent edited badge.
                version: EditRecipe::default().version,
                // Same rule for the coordinate-frame stamp: a legacy recipe
                // whose only difference from neutral is "written before the
                // orientation fix" holds no user edit, and counting it as one
                // would make a neutral legacy recipe.json outrank the XMP
                // beside it (`SavedDevelop::NoopOnly` precedence).
                coord_era: EditRecipe::default().coord_era,
                // …and for the control-set stamp. "This recipe predates the
                // R25 keys" is provenance about the SCHEMA, not an edit: a
                // neutral v0.30 recipe.json must still clear the ● and must
                // still lose the `SavedDevelop::NoopOnly` precedence contest
                // to a sidecar that holds real edits.
                schema_era: EditRecipe::default().schema_era,
                base_curve: Vec::new(),
                // The as-shot WB anchor is the same kind of stamped
                // calibration — a fresh-open stamp must still count as no-op.
                as_shot_k: None,
                as_shot_tint: None,
                lens_profile: if self.lens_profile.is_as_stamped() {
                    LensProfile::default()
                } else {
                    self.lens_profile.clone()
                },
                // The pass-through blocks are PROVENANCE too (R25 B4), and
                // this exclusion is load-bearing rather than tidy. Lightroom
                // writes the whole Perspective block plus `crs:CameraProfile`
                // onto every file it has so much as looked at, whether or not
                // the photographer ever opened Transform — all seven reference
                // sidecars carry nine of these keys and six of the seven carry
                // the Perspective block entirely at rest — so counting them as
                // an edit would make an UNTOUCHED Lightroom sidecar out-rank the
                // photographer's own stored develop (`SavedDevelop`
                // precedence, gui/persist.rs) and blank their canvas on
                // open. Nothing in this app can set the map either: it is
                // filled by reading a document and by nothing else, so it can
                // never be the thing the user changed. The sidecar keeps its
                // block regardless — `xmp::merge_strip_keys` never strips a
                // key the recipe in hand does not carry.
                passthrough: std::collections::BTreeMap::new(),
                ..self.clone()
            } == EditRecipe::default()
    }
}

/// [`EditRecipe::temper`]'s shipped soft-cap knees and ceilings — the values
/// bd3f9d4 tuned, named so the strength axis scales ONE definition instead of
/// eight literals. `GradeStrength::CALIBRATED` reproduces exactly these.
pub const TEMPER_TONE_KNEE: f32 = 50.0; // Highlights / Shadows
pub const TEMPER_TONE_CEIL: f32 = 70.0;
pub const TEMPER_POINT_KNEE: f32 = 30.0; // Whites / Blacks
pub const TEMPER_POINT_CEIL: f32 = 45.0;

/// Which of the three strength BANDS a [`GradeStrength`] falls in — the coarse
/// dial the prompt/verifier/judge templates switch on.
///
/// Prose cannot be interpolated the way a number can, so the wording is banded
/// while every NUMBER on the axis stays continuous. Consequence worth knowing:
/// the calibration point (0.50) and the shipped default (0.65) share the
/// `Balanced` band, so they differ in the guardrail NUMBERS the prompt quotes
/// and in `temper`'s knees, not in the adjectives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrengthTier {
    /// ≤ 0.4 — the shipped-since-f944ef3 restraint prose, verbatim.
    Restrained,
    /// 0.4 … 0.7 — confident: the model must decide about each look control
    /// rather than default it to neutral.
    Balanced,
    /// Above 0.7 — committed: the reference becomes a floor rather than a
    /// ceiling, and only BROKEN data (not strength) counts as over-cooked.
    Committed,
}

/// How closely the proposer should follow the photographer's free-text
/// direction. This is prompt intent only; it never changes render bounds.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct DirectionAdherence(f32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdherenceTier {
    Hint,
    Direct,
    Brief,
}

impl DirectionAdherence {
    pub const DEFAULT: f32 = 0.65;
    pub const TIER_LOW_MAX: f32 = GradeStrength::TIER_LOW_MAX;
    pub const TIER_MID_MAX: f32 = GradeStrength::TIER_MID_MAX;

    pub fn new(v: f32) -> Self {
        Self(if v.is_finite() { v.clamp(0.0, 1.0) } else { Self::DEFAULT })
    }

    pub fn get(self) -> f32 { self.0 }
    pub fn pct(self) -> f32 { self.0 * 100.0 }
    pub fn tier(self) -> AdherenceTier {
        if self.0 <= Self::TIER_LOW_MAX {
            AdherenceTier::Hint
        } else if self.0 <= Self::TIER_MID_MAX {
            AdherenceTier::Direct
        } else {
            AdherenceTier::Brief
        }
    }
}

impl Default for DirectionAdherence {
    fn default() -> Self { Self(Self::DEFAULT) }
}

/// How COMMITTED an AI-proposed develop should be — the app's strength axis
/// (R23-3, feedback #5: "the AI is too timid, and I want a strength slider").
///
/// Deliberately **not** a field of [`EditRecipe`]: strength is the user's
/// INTENT for one analysis, not a develop parameter. In the recipe it would
/// (a) have to be projected into the Lightroom XMP contract, which has no such
/// notion, and (b) change `store::recipe_norm`'s structural fingerprint — the
/// R21 deleted-version registry documents that a schema drift there fails OPEN,
/// so every already-recorded deletion would silently lose its structure arm.
///
/// SIX gates in this app decide "how hard to push", and every one of them was a
/// hard-coded constant. This value drives all six — the proposer prompt
/// (`advisor::openai`), [`EditRecipe::temper`], the verifier's two-sided band
/// (`advisor::build_verify_prompt`), the visual judge's rubric
/// (`advisor::judge`), the style-reference wording (`style::render_reference`)
/// and the no-AI fallback (`advisor::heuristic`). Missing ONE gate cancels the
/// axis: a bolder proposal that the verifier then calls over-cooked comes back
/// exactly as timid as before, which is why each module carries its own gate
/// test.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct GradeStrength(f32);

impl GradeStrength {
    /// The point every restraint NUMBER in the app was tuned at (the 147-photo
    /// eval of f944ef3, plus bd3f9d4's highlight-integrity cases). At this value
    /// the prompt quotes ±50/±35 and [`EditRecipe::temper`] uses the shipped
    /// 50→70 / 30→45 knees, bit for bit.
    ///
    /// NOT a "behave like the last release" switch, and calling it one was this
    /// axis's single dishonest claim. Two different axes run through here. The
    /// NUMBERS are reproduced exactly at 0.5. The restraint PROSE every pre-R23
    /// request carried verbatim is now the [`StrengthTier::Restrained`] wording,
    /// i.e. the ≤ 0.4 band — while 0.5 lands in [`StrengthTier::Balanced`]. So no
    /// single value on this dial reproduces a pre-R23 request in full: 0.5 buys
    /// the calibrated numbers, 0.4 the old words (at `soft_cap_factor` 0.93, i.e.
    /// with the numbers already tightened).
    pub const CALIBRATED: f32 = 0.5;
    /// What every surface starts at (user decision 2026-08-17 ⑦: "a bit braver
    /// than today, with one click back to the calibration point").
    pub const DEFAULT: f32 = 0.65;
    /// Upper edge of [`StrengthTier::Restrained`] (inclusive).
    pub const TIER_LOW_MAX: f32 = 0.4;
    /// Upper edge of [`StrengthTier::Balanced`] (inclusive).
    pub const TIER_MID_MAX: f32 = 0.7;
    /// Slope of the [`soft_cap_factor`](Self::soft_cap_factor) line.
    pub const SOFT_CAP_SPREAD: f32 = 0.7;

    /// Clamp into 0..=1. A non-finite value is the shipped default, never 0 —
    /// a NaN slider must not silently mean "maximum restraint".
    pub fn new(v: f32) -> Self {
        Self(if v.is_finite() { v.clamp(0.0, 1.0) } else { Self::DEFAULT })
    }

    /// The calibration point — the pre-R23 guardrail NUMBERS, not the whole
    /// pre-R23 request (see [`Self::CALIBRATED`] for what does and does not come
    /// back at this value).
    pub fn calibrated() -> Self {
        Self(Self::CALIBRATED)
    }

    /// Resolve a dial a surface may not have sent: the CLI's omitted
    /// `--strength`, the web body's absent `grade_strength`. ONE definition of
    /// that decision, because getting it wrong is silent — `Option::unwrap_or`
    /// with a bare `0.0` would make "the client said nothing" mean "be as timid
    /// as possible", on the very dial that exists because the AI was too timid.
    pub fn from_optional(v: Option<f32>) -> Self {
        v.map(Self::new).unwrap_or_default()
    }

    pub fn get(self) -> f32 {
        self.0
    }

    /// 0..=100, for the one-decimal-free `{:.0}%` the prompts quote.
    pub fn pct(self) -> f32 {
        self.0 * 100.0
    }

    pub fn tier(self) -> StrengthTier {
        if self.0 <= Self::TIER_LOW_MAX {
            StrengthTier::Restrained
        } else if self.0 <= Self::TIER_MID_MAX {
            StrengthTier::Balanced
        } else {
            StrengthTier::Committed
        }
    }

    /// How far ABOVE the calibration point, as 0..=1 (0 at or below it). The
    /// ramp every "open the numbers up" formula shares, so the axis can never
    /// tighten a number below its calibrated value: the guardrails were measured
    /// there, and only the bold half of the axis is new.
    pub fn above_calibration(self) -> f32 {
        ((self.0 - Self::CALIBRATED) / (1.0 - Self::CALIBRATED)).clamp(0.0, 1.0)
    }

    /// The multiplier on [`EditRecipe::temper`]'s four knees and ceilings.
    ///
    /// `1 + (s − 0.5) · 0.7`, so:
    ///   * s = 0.5 → 1.0 — the shipped 50→70 / 30→45 knees, bit for bit;
    ///   * s = 1.0 → 1.35 — the widest ceiling (70) becomes 94.5, and `soft_cap`
    ///     ASYMPTOTES to its ceiling, so the output stays ≥ 5.5 inside the ±100
    ///     hard `clamp`. The clamp is a safety bound, never a target, and this
    ///     axis does not touch it;
    ///   * s = 0.0 → 0.65 — knees at 32.5/19.5, i.e. more restrained than any
    ///     release so far, which is what a 0 % strength dial should mean.
    pub fn soft_cap_factor(self) -> f32 {
        1.0 + (self.0 - Self::CALIBRATED) * Self::SOFT_CAP_SPREAD
    }
}

impl Default for GradeStrength {
    /// [`Self::DEFAULT`] — the product default, so a call site that forgets to
    /// thread the axis gets the shipped behaviour rather than the timid one this
    /// round exists to fix. Measurement runs that need the fixed baseline say
    /// [`Self::calibrated`] explicitly (see `eval.rs`).
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    /// The strings, and the fact that clamping can NEUTRALISE a recipe.
    ///
    /// Two defects in one place. `clamp`'s own comment claimed every field was
    /// bounded while no string was, and `rationale` is fed from the advisor's
    /// failure text — an upstream HTTP error body reaches it verbatim, so a
    /// merely broken endpoint could write megabytes into recipe.json and into
    /// the XMP beside the RAW.
    ///
    /// And `serve.rs`'s XMP route asserted that "clamping cannot flip the
    /// is_noop branch". It can, and that branch DELETES the photo's saved
    /// edits: a body carrying only a zero-area crop is a real edit on arrival
    /// and `EditRecipe::default()` afterwards. The route now decides on the
    /// pre-clamp recipe; this pins the property that makes the order matter,
    /// so the assertion cannot quietly come back.
    #[test]
    fn clamp_bounds_strings_and_can_neutralise_a_recipe() {
        use super::*;
        let mut r = EditRecipe {
            rationale: "r".repeat(50_000),
            masks: vec![LocalAdjustment {
                name: "n".repeat(10_000),
                mask: MaskGeometry::Bitmap { path: "p".repeat(50_000) },
                ..Default::default()
            }],
            ..Default::default()
        };
        r.clamp();
        assert!(r.rationale.len() <= 16384, "rationale unbounded: {}", r.rationale.len());
        assert!(r.masks[0].name.len() <= 256, "mask name unbounded: {}", r.masks[0].name.len());
        match &r.masks[0].mask {
            MaskGeometry::Bitmap { path } => {
                assert!(path.len() <= 4096, "bitmap path unbounded: {}", path.len())
            }
            other => panic!("the bitmap mask was replaced by {other:?}"),
        }

        // A multi-byte rationale must not be cut mid-character — `String::
        // truncate` panics off a char boundary, and this input is unvalidated.
        let mut multi = EditRecipe { rationale: "é".repeat(50_000), ..Default::default() };
        multi.clamp(); // would panic if the cut ignored char boundaries
        assert!(multi.rationale.len() <= 16384);
        assert!(multi.rationale.chars().all(|c| c == 'é'), "cut mid-character");

        // The neutralisation itself: a zero-area crop is an edit on arrival…
        let mut degenerate = EditRecipe {
            crop: Some(Crop { left: 0.5, top: 0.5, right: 0.5, bottom: 0.5 }),
            ..Default::default()
        };
        assert!(!degenerate.is_noop(), "a crop-bearing recipe is not a no-op as sent");
        degenerate.clamp();
        // …and exactly the recipe that means "clear my saved edits" afterwards.
        assert!(
            degenerate.is_noop(),
            "clamp no longer neutralises a degenerate crop — if this changed on \
             purpose, re-check serve.rs's api_xmp clear branch, which is ordered \
             around it"
        );
    }

    #[test]
    fn clamp_bounds_sizes_and_coordinates_not_just_values() {
        use super::*;
        // A crafted recipe: absurd counts and finite-but-overflowing coords.
        let mut r = EditRecipe {
            masks: (0..500)
                .map(|_| LocalAdjustment {
                    mask: MaskGeometry::Linear {
                        zero_x: 1e30,
                        zero_y: -1e30,
                        full_x: 1e30,
                        full_y: 0.0,
                    },
                    ..Default::default()
                })
                .collect(),
            tone_curve: (0..5000)
                .map(|i| CurvePoint { input: (i % 256) as u8, output: 0 })
                .collect(),
            base_curve: (0..5000).map(|_| [5.0f32, -3.0]).collect(),
            ..Default::default()
        };
        r.clamp();
        assert!(r.masks.len() <= 64, "mask count uncapped: {}", r.masks.len());
        assert!(r.tone_curve.len() <= 256, "curve uncapped: {}", r.tone_curve.len());
        assert!(r.base_curve.len() <= 256, "base curve uncapped: {}", r.base_curve.len());
        for k in &r.base_curve {
            assert!((0.0..=1.0).contains(&k[0]) && (0.0..=1.0).contains(&k[1]), "knot {k:?}");
        }
        let MaskGeometry::Linear { zero_x, .. } = &r.masks[0].mask else { panic!() };
        assert!(zero_x.abs() <= 8.0, "coordinate magnitude uncapped: {zero_x}");
    }

    use super::*;

    #[test]
    fn clamp_normalizes_untrusted_crop() {
        // Out-of-range + inverted edges: clamp into the frame and reorder.
        let mut r = EditRecipe {
            crop: Some(Crop { left: 1.2, top: 0.8, right: -0.1, bottom: 0.2 }),
            ..Default::default()
        };
        r.clamp();
        let c = r.crop.expect("a real rectangle survives");
        assert!((c.left, c.top, c.right, c.bottom) == (0.0, 0.2, 1.0, 0.8), "{c:?}");
        // A degenerate rectangle is dropped — the geometry stage must never
        // see an empty crop.
        let mut r = EditRecipe {
            crop: Some(Crop { left: 0.5, top: 0.1, right: 0.5, bottom: 0.9 }),
            ..Default::default()
        };
        r.clamp();
        assert_eq!(r.crop, None);
        // NaN survives f32::clamp — a non-finite coordinate drops the crop.
        let mut r = EditRecipe {
            crop: Some(Crop { left: f32::NAN, top: 0.0, right: 1.0, bottom: 1.0 }),
            ..Default::default()
        };
        r.clamp();
        assert_eq!(r.crop, None);
        // A NaN luminance range bound used to PANIC clamp() outright
        // (f32::clamp's min ≤ max assert saw NaN as the max) — it must drop
        // the range, same policy as the crop. A NaN colour reference drops too.
        let mut r = EditRecipe::default();
        r.masks.push(LocalAdjustment {
            range: Some(RangeMask::Luminance {
                lo_outer: 0.0,
                lo: f32::NAN,
                hi: 0.8,
                hi_outer: 1.0,
            }),
            ..Default::default()
        });
        r.masks.push(LocalAdjustment {
            range: Some(RangeMask::Color {
                r: f32::NAN,
                g: 0.5,
                b: 0.5,
                amount: 0.5,
                px: 0.5,
                py: 0.5,
            }),
            ..Default::default()
        });
        r.clamp();
        assert!(r.masks.is_empty(), "a non-finite range drops its whole mask");
    }

    #[test]
    fn lens_profile_legacy_json_noop_and_toggle_semantics() {
        // A pre-profile recipe.json has no lens_profile → default (all off):
        // the byte-identical legacy render contract.
        let legacy: EditRecipe = serde_json::from_str(r#"{"exposure_ev":0.5}"#).unwrap();
        assert_eq!(legacy.lens_profile, LensProfile::default());
        // As-stamped (every available component on) is calibration, not an edit.
        let stamped = EditRecipe {
            lens_profile: LensProfile {
                vignette: vec![1.0, 1.2],
                distortion: vec![1.0, 0.95],
                ca_r: vec![1.0],
                ca_b: vec![1.0],
                vignette_on: true,
                distortion_on: true,
                ca_on: true,
                // The mask warp is stamped calibration like everything else
                // here, and the as-stamped no-op rule must survive it: it is
                // not a control the photographer moved.
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(stamped.is_noop(), "as-stamped profile must stay a no-op");
        // Turning one component OFF is a user edit that must survive.
        let mut toggled = stamped.clone();
        toggled.lens_profile.distortion_on = false;
        assert!(!toggled.is_noop(), "a toggle away from the stamp is an edit");
        // R25 B4: the pass-through blocks are the same kind of provenance,
        // and the consequence of getting this wrong is bigger than a badge.
        // Lightroom stamps `crs:PerspectiveUpright="0" … crs:CameraProfile=
        // "Adobe Standard"` onto every file it has touched (nine keys on all
        // seven reference sidecars), so if that counted as an edit an
        // UNTOUCHED Lightroom sidecar would out-rank the photographer's own
        // stored develop on open (`SavedDevelop` precedence) and blank the
        // canvas. Nothing in the app can set this map, so it can never be the
        // thing the user changed.
        let carried = EditRecipe {
            passthrough: [
                ("PerspectiveUpright".to_string(), "0".to_string()),
                ("CameraProfile".to_string(), "Adobe Standard".to_string()),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        assert!(carried.is_noop(), "a Lightroom Transform block is not an edit");
        let edited = EditRecipe { exposure_ev: 0.5, ..carried.clone() };
        assert!(!edited.is_noop(), "…and it does not mask a real one either");
        // Round-trip.
        let json = serde_json::to_string(&toggled).unwrap();
        let back: EditRecipe = serde_json::from_str(&json).unwrap();
        assert_eq!(back.lens_profile, toggled.lens_profile);
    }

    /// R29 Batch-3: the mask-warp field's schema contract, in the four parts
    /// that can each break silently.
    #[test]
    fn mask_warp_source_all_covers_every_variant_and_the_schema_reads_both_ways() {
        // (a) The iteration array is complete and has prose for every member —
        // the `MaskImportReason::ALL` guard, for the same reason: `en`'s match
        // stops the build when a variant is added, an array does not.
        let mut seen = MaskWarpSource::ALL.to_vec();
        seen.sort_by_key(|s| format!("{s:?}"));
        seen.dedup();
        assert_eq!(seen.len(), MaskWarpSource::ALL.len(), "ALL repeats a variant");
        for s in MaskWarpSource::ALL {
            assert!(!s.en().is_empty(), "{s:?} has no prose");
        }
        assert_eq!(MaskWarpSource::default(), MaskWarpSource::Absent);

        // (b) BACKWARDS compatible: a recipe.json written before this field
        // existed reads as "no warp, nobody looked", not as an error.
        let legacy: EditRecipe = serde_json::from_str(r#"{"exposure_ev":0.5}"#).unwrap();
        assert!(legacy.lens_profile.mask_warp.is_empty());
        assert_eq!(legacy.lens_profile.mask_warp_src, MaskWarpSource::Absent);
        assert_eq!(legacy.lens_profile.mask_warp_center, None);

        // (c) FORWARDS it is a HARD BREAK, and that is the point: `LensProfile`
        // denies unknown fields, so a build without these frame keys refuses a
        // recipe that has them rather than dropping a coordinate frame on the
        // floor. Asserted through the same door a v0.34 binary would use.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct PreR29LensProfile {
            #[serde(default)]
            vignette: Vec<f32>,
            #[serde(default)]
            distortion: Vec<f32>,
            #[serde(default)]
            ca_r: Vec<f32>,
            #[serde(default)]
            ca_b: Vec<f32>,
            #[serde(default)]
            vignette_on: bool,
            #[serde(default)]
            distortion_on: bool,
            #[serde(default)]
            ca_on: bool,
        }
        let current = serde_json::to_string(&LensProfile {
            distortion: vec![1.0, 0.95],
            distortion_on: true,
            mask_warp: vec![0.98, 1.02],
            mask_warp_src: MaskWarpSource::CameraMetadata,
            mask_warp_center: Some(MaskWarpCenter {
                stored_px: [4768.0, 3168.0],
                stored_dims: [9504.0, 6336.0],
            }),
            ..Default::default()
        })
        .unwrap();
        assert!(current.contains("\"mask_warp\""), "{current}");
        assert!(
            serde_json::from_str::<PreR29LensProfile>(&current).is_err(),
            "a pre-R29 reader must REFUSE this recipe, not silently drop the frame"
        );
        // …and the current reader round-trips it exactly.
        let back: LensProfile = serde_json::from_str(&current).unwrap();
        assert_eq!(back.mask_warp, vec![0.98, 1.02]);
        assert_eq!(back.mask_warp_src, MaskWarpSource::CameraMetadata);
        assert_eq!(
            back.mask_warp_center,
            Some(MaskWarpCenter {
                stored_px: [4768.0, 3168.0],
                stored_dims: [9504.0, 6336.0],
            })
        );

        // (d) `clamp` holds the band AND the tag/data invariant. A hand-edited
        // file cannot claim a refusal and carry knots, and cannot smuggle a
        // 4× radius factor or a NaN past the interpolator.
        let mut wild = LensProfile {
            mask_warp: vec![4.0, f32::NAN, 0.1, 1.02],
            mask_warp_src: MaskWarpSource::Lcp,
            ..Default::default()
        };
        wild.clamp();
        assert_eq!(wild.mask_warp, vec![1.3, 0.7, 1.02], "band and NaN drop");
        let mut lying = LensProfile {
            mask_warp: vec![1.02; 16],
            mask_warp_src: MaskWarpSource::FisheyeRefused,
            ..Default::default()
        };
        lying.clamp();
        assert!(lying.mask_warp.is_empty(), "a refusal cannot carry a warp");
        assert_eq!(lying.mask_warp_src, MaskWarpSource::FisheyeRefused, "…and keeps its reason");
        // A solved tag with no usable spline is not solved, and says so.
        let mut stub = LensProfile {
            mask_warp: vec![1.02],
            mask_warp_src: MaskWarpSource::Lcp,
            ..Default::default()
        };
        stub.clamp();
        assert!(stub.mask_warp.is_empty());
        assert_eq!(stub.mask_warp_src, MaskWarpSource::Unparseable);
    }

    #[test]
    fn base_curve_legacy_json_noop_and_round_trip() {
        // A pre-0.14 recipe.json has no base_curve → empty (renders as before).
        let legacy: EditRecipe = serde_json::from_str(r#"{"exposure_ev":0.5}"#).unwrap();
        assert!(legacy.base_curve.is_empty(), "legacy files must deserialise to NO base look");
        // A stamped-but-untouched recipe is still a no-op: the base look is the
        // photo's calibration, not a user edit (fresh opens must stay neutral).
        let stamped = EditRecipe {
            base_curve: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
            ..Default::default()
        };
        assert!(stamped.is_noop());
        // And the curve survives the JSON round trip byte-exactly.
        let back: EditRecipe =
            serde_json::from_str(&serde_json::to_string(&stamped).unwrap()).unwrap();
        assert_eq!(back.base_curve, stamped.base_curve);
        // clamp() sanitises corrupt knot coordinates into [0,1].
        let mut wild = EditRecipe {
            base_curve: vec![[-0.5, 2.0], [0.5, 0.7]],
            ..Default::default()
        };
        wild.clamp();
        assert_eq!(wild.base_curve, vec![[0.0, 1.0], [0.5, 0.7]]);
    }

    #[test]
    fn as_shot_anchor_is_calibration_not_an_edit() {
        // Legacy JSON (no as-shot fields) deserialises to None — the engine
        // then keeps its historical 5500 K anchor, byte-identical.
        let legacy: EditRecipe = serde_json::from_str(r#"{"exposure_ev":0.5}"#).unwrap();
        assert_eq!((legacy.as_shot_k, legacy.as_shot_tint), (None, None));
        // A stamped-but-untouched recipe is still a no-op (fresh opens stay
        // neutral), and the stamp survives the JSON round trip.
        let stamped = EditRecipe {
            as_shot_k: Some(4830.0),
            as_shot_tint: Some(6.0),
            ..Default::default()
        };
        assert!(stamped.is_noop(), "an as-shot stamp must stay a no-op");
        let back: EditRecipe =
            serde_json::from_str(&serde_json::to_string(&stamped).unwrap()).unwrap();
        assert_eq!((back.as_shot_k, back.as_shot_tint), (Some(4830.0), Some(6.0)));
        // clamp() sanitises: NaN → unknown, out-of-band → the legal band.
        let mut wild = EditRecipe {
            as_shot_k: Some(f32::NAN),
            as_shot_tint: Some(900.0),
            ..Default::default()
        };
        wild.clamp();
        assert_eq!((wild.as_shot_k, wild.as_shot_tint), (None, Some(100.0)));
    }

    #[test]
    fn temper_lifts_white_point_and_soft_caps_extremes() {
        // The reported over-cooked recipe: strong −Highlights with low Whites greys
        // the foam. temper() lifts the white point and softens the extremes, WITHOUT
        // touching a modest recipe's committed tone moves.
        // At the CALIBRATION point — the assertions below are the shipped
        // behaviour, so they double as the "0.5 changes nothing" pin.
        let mut hot = EditRecipe { highlights: -78.81, whites: 10.27, shadows: 95.0, ..Default::default() };
        hot.temper(GradeStrength::calibrated());
        assert!(hot.whites >= 23.0, "recovery must lift the white point: whites={}", hot.whites);
        assert!(hot.highlights >= -65.0, "highlights not tempered: {}", hot.highlights);
        assert!(hot.highlights <= -55.0, "highlights over-tempered (lost commitment): {}", hot.highlights);
        assert!(hot.shadows >= 55.0 && hot.shadows < 70.0, "shadows soft-cap off: {}", hot.shadows);

        // A modest recipe keeps its tone moves; recovery still nudges whites a touch.
        let mut mild = EditRecipe { highlights: -30.0, shadows: 20.0, whites: 5.0, ..Default::default() };
        mild.temper(GradeStrength::calibrated());
        assert_eq!(mild.highlights, -30.0, "modest highlights must pass through");
        assert_eq!(mild.shadows, 20.0, "modest shadows must pass through");
        assert!(mild.whites >= 9.0, "modest recovery still protects speculars: {}", mild.whites);
    }

    /// The strength axis's own arithmetic: the two NAMED points, the three
    /// bands, and the door every surface's number comes through (R23-3).
    ///
    /// The named points are a promise made in the UI ("50% is the calibrated
    /// baseline", "double-click resets to 65%"), so they are pinned here rather
    /// than only in the GUI: the CLI's omitted `--strength`, the web body's
    /// absent field and the desktop default all resolve through these.
    #[test]
    fn the_strength_axis_has_two_named_points_three_bands_and_one_door() {
        use super::*;
        assert_eq!(GradeStrength::CALIBRATED, 0.5);
        assert_eq!(GradeStrength::DEFAULT, 0.65, "user decision 2026-08-17 ⑦");
        assert_eq!(GradeStrength::default().get(), GradeStrength::DEFAULT);
        assert_eq!(GradeStrength::calibrated().get(), GradeStrength::CALIBRATED);

        // Bands, on their edges (inclusive upper bounds, so 0.4 and 0.7 are the
        // last member of their band — the boundary a retune would move).
        for (v, want) in [
            (0.0, StrengthTier::Restrained),
            (0.4, StrengthTier::Restrained),
            (0.401, StrengthTier::Balanced),
            (0.5, StrengthTier::Balanced),
            (0.65, StrengthTier::Balanced),
            (0.7, StrengthTier::Balanced),
            (0.701, StrengthTier::Committed),
            (1.0, StrengthTier::Committed),
        ] {
            assert_eq!(GradeStrength::new(v).tier(), want, "band edge moved at {v}");
        }

        // The door: clamped, and a non-finite dial is the DEFAULT — never 0.0,
        // which would be the most timid setting on the dial that exists because
        // the AI was too timid.
        assert_eq!(GradeStrength::new(-1.0).get(), 0.0);
        assert_eq!(GradeStrength::new(4.0).get(), 1.0);
        assert_eq!(GradeStrength::new(f32::NAN).get(), GradeStrength::DEFAULT);
        assert_eq!(GradeStrength::new(f32::INFINITY).get(), GradeStrength::DEFAULT);

        // The one-sided ramp the prompt's numbers ride: 0 at and below the
        // calibration point (those numbers were MEASURED — the axis may open
        // them up, never tighten them), 1 at full strength.
        assert_eq!(GradeStrength::new(0.0).above_calibration(), 0.0);
        assert_eq!(GradeStrength::calibrated().above_calibration(), 0.0);
        assert_eq!(GradeStrength::new(0.75).above_calibration(), 0.5);
        assert_eq!(GradeStrength::new(1.0).above_calibration(), 1.0);
    }

    /// GATE 2 of the six the strength axis has to pass (R23-3, feedback #5).
    ///
    /// Three properties, each of which the axis is worthless without:
    ///  1. the four soft-cap knees MOVE with strength, monotonically — a bolder
    ///     proposal must survive `temper` instead of being compressed back;
    ///  2. 0.5 is the calibration point *exactly* (the knees are the literals
    ///     bd3f9d4 tuned), so "one click back to 0.5" is a real promise;
    ///  3. the white-point coupling does NOT scale — that rule is the fix for a
    ///     measured defect (foam dragged to grey), and scaling it would re-open
    ///     the defect at low strength.
    #[test]
    fn temper_knees_scale_with_strength_but_the_white_point_guard_never_does() {
        // (1) + (2): the same over-cooked recipe at three strengths.
        let hot = |s: f32| {
            let mut r = EditRecipe { highlights: -78.81, shadows: 95.0, blacks: -60.0, ..Default::default() };
            r.temper(GradeStrength::new(s));
            r
        };
        let (timid, calib, bold) = (hot(0.2), hot(0.5), hot(0.9));
        assert!(
            timid.shadows < calib.shadows && calib.shadows < bold.shadows,
            "shadows must open up monotonically: {} < {} < {}",
            timid.shadows, calib.shadows, bold.shadows
        );
        assert!(
            timid.highlights > calib.highlights && calib.highlights > bold.highlights,
            "negative highlights must open up monotonically: {} > {} > {}",
            timid.highlights, calib.highlights, bold.highlights
        );
        assert!(
            timid.blacks > calib.blacks && calib.blacks > bold.blacks,
            "blacks must open up monotonically: {} > {} > {}",
            timid.blacks, calib.blacks, bold.blacks
        );
        // The calibration point reproduces the pre-R23 arithmetic to the bit:
        // shadows 95 through knee 50 / ceil 70 → 50 + 20·45/65.
        assert_eq!(GradeStrength::calibrated().soft_cap_factor(), 1.0);
        assert_eq!(calib.shadows, 50.0 + 20.0 * (45.0 / 65.0));
        // …and even at full strength the asymptote stays inside the ±100 clamp.
        assert!(
            TEMPER_TONE_CEIL * GradeStrength::new(1.0).soft_cap_factor() < 100.0,
            "the soft-cap ceiling must never reach the hard clamp"
        );

        // (3) The white-point guard is strength-INVARIANT, globally and per mask.
        // Highlights −40 sits below every knee at every strength, so the ONLY
        // thing that can move `whites` here is the coupling.
        for s in [0.0_f32, 0.5, 1.0] {
            let mut r = EditRecipe {
                highlights: -40.0,
                whites: 0.0,
                masks: vec![LocalAdjustment {
                    highlights: -40.0,
                    whites: 0.0,
                    ..Default::default()
                }],
                ..Default::default()
            };
            r.temper(GradeStrength::new(s));
            assert_eq!(r.whites, 12.0, "global white-point guard drifted at strength {s}");
            assert_eq!(r.masks[0].whites, 12.0, "mask white-point guard drifted at strength {s}");
        }
        // …and at the guard's MAXIMUM, which is where the old ordering broke it
        // (R23 review NIT-2). Highlights −100 asks for whites 30 — above the
        // point knee at every s < 0.5 (19.5 at s = 0), so the pre-cap coupling
        // came back 24.56 and the invariant above was only true because −40
        // never left the identity region of the cap. The floor makes it true.
        for s in [0.0_f32, 0.5, 1.0] {
            let mut r = EditRecipe {
                highlights: -100.0,
                whites: 0.0,
                masks: vec![LocalAdjustment {
                    highlights: -100.0,
                    whites: 0.0,
                    ..Default::default()
                }],
                ..Default::default()
            };
            r.temper(GradeStrength::new(s));
            // 30 to within f32 rounding — `100 · 0.3` is 30.000002 in f32, and
            // the point of the assertion is the guard's VALUE surviving, not
            // the last mantissa bit of a multiply the shipped code also does.
            assert!(
                (r.whites - 30.0).abs() < 1e-4,
                "global white point was shaved by the soft cap at strength {s}: {}",
                r.whites
            );
            assert!(
                (r.masks[0].whites - 30.0).abs() < 1e-4,
                "mask white point was shaved by the soft cap at strength {s}: {}",
                r.masks[0].whites
            );
        }
        // The floor only ever RAISES: a recipe that already asks for more white
        // point than the coupling wants keeps the soft-capped value, and that
        // value is exactly what the pre-floor ordering produced (the algebraic
        // identity the body's comment states, pinned rather than asserted in
        // prose).
        for s in [0.5_f32, 0.65, 1.0] {
            let mut r = EditRecipe { highlights: -100.0, whites: 90.0, ..Default::default() };
            r.temper(GradeStrength::new(s));
            let f = GradeStrength::new(s).soft_cap_factor();
            let (knee, ceil) = (TEMPER_POINT_KNEE * f, TEMPER_POINT_CEIL * f);
            let span = ceil - knee;
            let excess = 90.0 - knee;
            assert_eq!(
                r.whites,
                knee + span * (excess / (excess + span)),
                "a whites the user set above the guard must still be soft-capped at strength {s}"
            );
        }
    }

    #[test]
    fn masks_round_trip_and_v1_compatible() {
        // Default has no masks (v1-compatible).
        assert!(EditRecipe::default().masks.is_empty());

        let mut recipe = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.35, full_x: 0.5, full_y: 0.0 },
                    // Luminance range with a deliberately ill-formed trapezoid:
                    // clamp must sort lo/hi and pin the outers around them.
                    range: Some(RangeMask::Luminance { lo_outer: 0.9, lo: 0.8, hi: 0.5, hi_outer: 0.2 }),
                    name: "sky".into(),
                    exposure_ev: -0.4,
                    highlights: -200.0, // out of range → clamp pulls to -100
                    ..Default::default()
                },
                LocalAdjustment {
                    mask: MaskGeometry::Radial {
                        top: 0.3, left: 0.35, bottom: 0.7, right: 0.65,
                        feather: 0.5, roundness: 0.0, flipped: false, angle: 0.0,
                        midpoint: 50.0, mask_version: 2,
                    },
                    range: Some(RangeMask::Color { r: 0.9, g: 0.6, b: 0.2, amount: 1.7, px: 0.5, py: 0.5 }),
                    name: "subject".into(),
                    shadows: 15.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        recipe.clamp();
        assert_eq!(recipe.masks[0].highlights, -100.0); // clamped
        // Luminance trapezoid re-ordered to lo_outer ≤ lo ≤ hi ≤ hi_outer.
        assert_eq!(
            recipe.masks[0].range,
            Some(RangeMask::Luminance { lo_outer: 0.5, lo: 0.5, hi: 0.8, hi_outer: 0.8 })
        );
        // Color amount clamped into 0..=1.
        match recipe.masks[1].range {
            Some(RangeMask::Color { amount, .. }) => assert_eq!(amount, 1.0),
            other => panic!("color range lost in clamp: {other:?}"),
        }

        let json = serde_json::to_string_pretty(&recipe).unwrap();
        let back: EditRecipe = serde_json::from_str(&json).unwrap();
        assert_eq!(recipe, back);
        assert!(!recipe.is_noop()); // masks present ⇒ not a no-op

        // A v1 recipe JSON (no "masks" key) still deserializes, masks default empty.
        let v1 = r#"{ "exposure_ev": 0.5, "rationale": "x", "confidence": 0.9 }"#;
        assert!(serde_json::from_str::<EditRecipe>(v1).unwrap().masks.is_empty());
    }

    /// The SAME serde subtlety as `absent_coord_era_reads_as_the_legacy_frame`
    /// above, for the control-set stamp — and it earns its own test rather than
    /// a line in that one, because it gates a different consequence: the XMP
    /// merge's decision to strip a key or leave it (`xmp::era_suppressed_
    /// attr_keys`). Get this default backwards and every v0.30 `recipe.json`
    /// decodes as "written against the current control set", so an ordinary
    /// Ctrl+S deletes `crs:Texture`, the Grain block and the detail axes out
    /// of the photographer's own Lightroom sidecar.
    #[test]
    fn absent_schema_era_reads_as_the_legacy_control_set() {
        let legacy = r#"{"version":2,"contrast":7.0}"#;
        assert_eq!(
            serde_json::from_str::<EditRecipe>(legacy).unwrap().schema_era,
            0,
            "a file with no schema_era key predates the R25 keys"
        );
        // The FIELD-level default must beat the CONTAINER-level one, which is
        // `Default::default()` — i.e. the current era. This is the assertion
        // that fails if the `#[serde(default = "schema_era_legacy")]`
        // attribute is ever dropped, and nothing else in the tree would.
        assert_eq!(
            EditRecipe::default().schema_era,
            SCHEMA_ERA,
            "a freshly built recipe is authored against the current control set"
        );
        assert_ne!(schema_era_legacy(), EditRecipe::default().schema_era);
        // …and it round-trips: what we write, we read back unchanged.
        let json = serde_json::to_string(&EditRecipe::default()).unwrap();
        assert!(json.contains("\"schema_era\":1"), "{json}");
        assert_eq!(serde_json::from_str::<EditRecipe>(&json).unwrap().schema_era, SCHEMA_ERA);
        // PROVENANCE, not an edit: a neutral v0.30 recipe.json must still read
        // as "no edits", or it would out-rank the XMP beside it
        // (`SavedDevelop::NoopOnly` precedence) and blank a canvas on open.
        let neutral_legacy: EditRecipe = serde_json::from_str(r#"{"version":2}"#).unwrap();
        assert_eq!(neutral_legacy.schema_era, 0, "premise: it really is era 0");
        assert!(neutral_legacy.is_noop(), "the era stamp is not an edit");
    }

    /// THE CONTRACT the whole coordinate migration is gated on, and the one
    /// serde subtlety in it: `EditRecipe` carries a CONTAINER-level
    /// `#[serde(default)]`, which fills every missing field from
    /// `Default::default()` — and `Default::default().coord_era` is
    /// [`COORD_ERA`], the CURRENT frame. The field-level
    /// `#[serde(default = "coord_era_legacy")]` must OVERRIDE that, or every
    /// pre-v0.30 recipe would decode as "already migrated" and a portrait
    /// RAW's saved masks would stay on the wrong axis forever.
    #[test]
    fn absent_coord_era_reads_as_the_legacy_frame() {
        let legacy = r#"{"version":2,"contrast":7.0}"#;
        assert_eq!(
            serde_json::from_str::<EditRecipe>(legacy).unwrap().coord_era,
            0,
            "a file with no coord_era key predates the field: SENSOR frame"
        );
        assert_eq!(
            EditRecipe::default().coord_era,
            COORD_ERA,
            "a freshly built recipe is authored in the display frame"
        );
        // And it round-trips: what we write, we read back unchanged.
        let json = serde_json::to_string(&EditRecipe::default()).unwrap();
        assert!(json.contains("\"coord_era\":1"), "{json}");
        assert_eq!(serde_json::from_str::<EditRecipe>(&json).unwrap().coord_era, COORD_ERA);
        // A legacy recipe whose only difference from neutral is the missing
        // stamp is still "no edits" — otherwise a neutral legacy recipe.json
        // would outrank the XMP beside it.
        assert!(serde_json::from_str::<EditRecipe>(r#"{"version":2}"#).unwrap().is_noop());
        // A mask WITHOUT a "range" key (pre-range recipes) defaults to None.
        let old_mask = r#"{ "masks": [ { "name": "sky" } ] }"#;
        assert_eq!(serde_json::from_str::<EditRecipe>(old_mask).unwrap().masks[0].range, None);
    }

    /// The serialisation choice `quarter_turns` makes and the two things it
    /// buys (R27 A1) — see the field's doc for the argument.
    ///
    /// 1. An UN-ROTATED recipe's JSON carries no `quarter_turns` key at all,
    ///    so `store::recipe_struct_hash` (which re-serialises the whole
    ///    recipe) still answers exactly what the previous build's bytes
    ///    answered. The R21 deleted-version registry's STRUCTURAL arm
    ///    therefore keeps matching, and no re-archive pass is needed — unlike
    ///    v0.31.0, which had to describe one.
    /// 2. A ROTATED one does carry it, which is precisely the recipe an
    ///    older exe must refuse (`deny_unknown_fields`) rather than render
    ///    sideways.
    ///
    /// MUTATION THIS CATCHES: delete the `skip_serializing_if` attribute and
    /// assertion (1) fails — every v0.33 recipe.json would then differ from
    /// its v0.32 self, breaking the fingerprint for photos nobody rotated.
    #[test]
    fn an_unrotated_recipe_serialises_exactly_as_the_previous_build_wrote_it() {
        let neutral = serde_json::to_string(&EditRecipe::default()).unwrap();
        assert!(
            !neutral.contains("quarter_turns"),
            "an un-rotated recipe must be byte-identical to what v0.32 wrote: {neutral}"
        );
        let turned = serde_json::to_string(&EditRecipe { quarter_turns: 1, ..Default::default() })
            .unwrap();
        assert!(turned.contains("\"quarter_turns\":1"), "{turned}");
        // …and reads back, so the skip is lossless in both directions.
        assert_eq!(serde_json::from_str::<EditRecipe>(&turned).unwrap().quarter_turns, 1);
        // Absent = 0. The container's `#[serde(default)]` is enough here (the
        // legacy meaning and the type's default AGREE), which is exactly why
        // this field carries no `coord_era`-style field-level default.
        assert_eq!(
            serde_json::from_str::<EditRecipe>(r#"{"version":2}"#).unwrap().quarter_turns,
            0
        );
        // A turn is an EDIT, not bookkeeping: it must light ● and outrank a
        // sidecar, or a rotated photo would look clean and never be saved.
        assert!(!EditRecipe { quarter_turns: 1, ..Default::default() }.is_noop());
    }

    /// A quarter turn is a RESIDUE CLASS, so `clamp` folds it instead of
    /// saturating — 4 means "no turn", not "the largest legal turn".
    ///
    /// MUTATION THIS CATCHES: replace `%= 4` with `= self.quarter_turns.min(3)`
    /// and a recipe carrying 4 (a hand edit, a foreign writer) renders a
    /// three-quarter rotation nobody asked for, with the crop and every mask
    /// left where they were.
    #[test]
    fn an_out_of_domain_quarter_turn_folds_rather_than_saturating() {
        for (raw, want) in [(0u8, 0u8), (3, 3), (4, 0), (7, 3), (255, 3)] {
            let mut r = EditRecipe { quarter_turns: raw, ..Default::default() };
            r.clamp();
            assert_eq!(r.quarter_turns, want, "clamp({raw})");
        }
    }

    /// R25 P5's forward-compatibility contract — the THIRD shape this
    /// round produced, and the one that behaves least like the other two.
    ///
    ///   * a new `EditRecipe` field (P2–P4) → the top level denies unknown
    ///     fields, so an older exe REFUSES the whole recipe.json, loudly;
    ///   * a new `LocalAdjustment` field (P6) → same, for any recipe that
    ///     has masks;
    ///   * a new `MaskGeometry::Radial` field (here) → in R25 this was the
    ///     SILENT one: an older exe read the mask fine, dropped the two keys
    ///     and wrote the file back without them. R27 closed that (the type
    ///     now denies unknown fields — see `an_unknown_radial_field_is_a_
    ///     loud_refusal`), so from v0.33 on the third shape behaves like the
    ///     first two.
    ///
    /// What this test still pins is the OTHER direction, which no attribute
    /// changes: a recipe MISSING the two keys — every file written before
    /// v0.31.0 — must read whole. The drop must not corrupt anything else,
    /// and what comes back must be Lightroom's neutrals, which is also the
    /// pin on the FIELD-level serde defaults, since the type's own defaults
    /// (0.0 / 0) would claim a midpoint at the extreme and a schema version
    /// nobody has ever written.
    ///
    /// MUTATION THIS CATCHES: replace either `#[serde(default = "…")]` with
    /// a bare `#[serde(default)]` and the two neutral asserts fail.
    #[test]
    fn radial_extras_are_dropped_by_an_older_reader_but_never_corrupt() {
        let mine = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Radial {
                    top: 0.3,
                    left: 0.35,
                    bottom: 0.7,
                    right: 0.65,
                    feather: 0.5,
                    roundness: 0.25,
                    flipped: true,
                    angle: -12.0,
                    midpoint: 37.0,
                    mask_version: 3,
                },
                name: "subject".into(),
                exposure_ev: 0.5,
                ..Default::default()
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&mine).unwrap();
        let mut doc: serde_json::Value = serde_json::from_str(&json).unwrap();

        // The older reader's view of the same file: the two keys it has never
        // heard of are simply not there.
        let geom = doc["masks"][0]["mask"].as_object_mut().expect("a radial object");
        assert!(geom.remove("midpoint").is_some(), "the writer emitted midpoint: {json}");
        assert!(geom.remove("mask_version").is_some(), "the writer emitted mask_version: {json}");
        let older: EditRecipe =
            serde_json::from_value(doc).expect("an older exe must still READ the recipe");

        let MaskGeometry::Radial {
            top, left, bottom, right, feather, roundness, flipped, angle, midpoint, mask_version,
        } = older.masks[0].mask
        else {
            panic!("expected a radial, got {:?}", older.masks[0].mask);
        };
        // Nothing else moved: this is a DROP, not a corruption.
        assert_eq!(
            (top, left, bottom, right, feather, roundness, flipped, angle),
            (0.3, 0.35, 0.7, 0.65, 0.5, 0.25, true, -12.0),
            "the rest of the geometry is untouched"
        );
        assert_eq!(older.masks[0].exposure_ev, 0.5, "and so is the adjustment");
        // …and the dropped pair reads as LIGHTROOM's neutral, not the type's.
        assert_eq!(midpoint, 50.0, "an absent Midpoint is ACR's 50, not f32::default()");
        assert_eq!(mask_version, 2, "an absent Version is Lightroom's 2, not u32::default()");
    }

    /// R27 L-17 — the class-3 forward break stops being silent.
    ///
    /// v0.31.0 added two fields to `Radial` and a v0.30 binary reading such a
    /// recipe dropped them WITHOUT A WORD and wrote the truncated geometry
    /// back, editing the user's file on their behalf. The reason given at the
    /// time was that serde cannot `deny_unknown_fields` on an internally
    /// tagged enum. It can — on the CONTAINER, where it covers every variant
    /// and exempts the tag — and this is the pin.
    ///
    /// Three shapes, because the boundary has three edges: the tag must still
    /// be accepted, a KNOWN field set must still parse, and the same object
    /// plus one key from the future must be refused BY NAME so a downgrading
    /// user reads a sentence instead of losing a value. `RangeMask` is the
    /// only other internally-tagged enum in the crate and gets the same
    /// treatment in the same breath.
    ///
    /// MUTATION THIS CATCHES: drop `deny_unknown_fields` from either enum and
    /// the corresponding `unwrap_err` panics — which is exactly the state
    /// v0.31.0 and v0.32.0 shipped in.
    #[test]
    fn an_unknown_radial_field_is_a_loud_refusal() {
        let radial = |extra: &str| {
            format!(
                r#"{{"masks":[{{"name":"subject","mask":{{"kind":"radial","top":0.3,"left":0.35,
                   "bottom":0.7,"right":0.65,"feather":0.5,"roundness":0.0,"flipped":false,
                   "angle":0.0,"midpoint":50.0,"mask_version":2{extra}}}}}]}}"#
            )
        };
        // Premise: the known shape parses, tag and all.
        let ok: EditRecipe = serde_json::from_str(&radial("")).expect("the known shape parses");
        assert!(matches!(ok.masks[0].mask, MaskGeometry::Radial { .. }));
        // …and one key from a future build is a refusal that NAMES it.
        let e = serde_json::from_str::<EditRecipe>(&radial(r#","quarter_turns":1"#))
            .expect_err("an unknown radial field must refuse the recipe, not be dropped");
        assert!(
            e.to_string().contains("quarter_turns"),
            "the refusal must name the field: {e}"
        );
        // The same edge on the crate's other internally-tagged enum.
        let range = |extra: &str| {
            format!(
                r#"{{"masks":[{{"name":"sky","range":{{"kind":"luminance","lo_outer":0.0,
                   "lo":0.2,"hi":0.8,"hi_outer":1.0{extra}}}}}]}}"#
            )
        };
        assert!(serde_json::from_str::<EditRecipe>(&range("")).is_ok(), "premise: it parses");
        let e = serde_json::from_str::<EditRecipe>(&range(r#","depth_lo":0.1"#))
            .expect_err("an unknown range-mask field must refuse too");
        assert!(e.to_string().contains("depth_lo"), "the refusal must name the field: {e}");
    }

    #[test]
    fn round_trips_through_json() {
        let mut recipe = EditRecipe {
            exposure_ev: 0.35,
            contrast: 12.0,
            highlights: -40.0,
            shadows: 25.0,
            temperature_k: Some(5600.0),
            vibrance: 18.0,
            crop: Some(Crop { left: 0.05, top: 0.0, right: 0.95, bottom: 1.0 }),
            tone_curve: vec![
                CurvePoint { input: 0, output: 8 },
                CurvePoint { input: 255, output: 247 },
            ],
            rationale: "Slightly underexposed; recovered sky, lifted shadows.".into(),
            confidence: 0.82,
            ..Default::default()
        };
        recipe.clamp();

        let json = serde_json::to_string_pretty(&recipe).unwrap();
        let back: EditRecipe = serde_json::from_str(&json).unwrap();
        assert_eq!(recipe, back);
    }

    #[test]
    fn omitted_fields_default_to_neutral() {
        // The AI emits only the controls it cares about.
        let json = r#"{ "exposure_ev": 0.5, "rationale": "brighten", "confidence": 0.9 }"#;
        let recipe: EditRecipe = serde_json::from_str(json).unwrap();
        assert_eq!(recipe.exposure_ev, 0.5);
        assert_eq!(recipe.contrast, 0.0); // defaulted
        assert_eq!(recipe.temperature_k, None);
        assert_eq!(recipe.version, CALIB_ERA);
    }

    #[test]
    fn clamp_pulls_out_of_range_values_back() {
        let mut recipe = EditRecipe {
            contrast: 999.0,
            exposure_ev: -42.0,
            lens_distortion: -250.0,
            ..Default::default()
        };
        recipe.clamp();
        assert_eq!(recipe.contrast, 100.0);
        assert_eq!(recipe.exposure_ev, -5.0);
        assert_eq!(recipe.lens_distortion, -100.0);
    }

    /// R23-1b, the T4 schema-change policy in one test: what the two new
    /// `LocalAdjustment` fields do and do NOT change on disk.
    ///
    /// BACKWARD (must keep working): a `recipe.json` written before this round
    /// has neither key, and the container-level `#[serde(default)]` fills the
    /// engine's neutral — every archived develop still loads and renders as it
    /// did.
    ///
    /// BLAST RADIUS (the fingerprint consequence): the keys appear in EVERY
    /// serialization of a mask-bearing recipe, because `EditRecipe` has no
    /// `skip_serializing_if` anywhere. That moves the compact re-serialization
    /// R21's deleted-version registry hashes (`store::recipe_struct_hash`), so a
    /// version deleted by an older build no longer matches structurally and is
    /// preserved anew under a FRESH number — fail-open, exactly as that arm's
    /// contract says ("schema drift changes this hash"). The number itself is
    /// never reissued (the `hwm` arm is untouched by any schema change), which
    /// is what `tests/repro_deleted_version_resurrection.rs` pins.
    ///
    /// A recipe with NO masks is unaffected in both directions — the drift is
    /// bounded to mask-bearing develops, and this test states that bound rather
    /// than leaving the release note to guess it.
    #[test]
    fn the_new_local_fields_default_in_and_only_widen_a_mask_bearing_recipe() {
        // An OLD file: every key this round added is absent.
        let old = r#"{"version":2,"masks":[{"mask":{"kind":"linear","zero_x":0.5,
            "zero_y":0.0,"full_x":0.5,"full_y":0.4},"name":"sky","exposure_ev":-0.4}]}"#;
        let back: EditRecipe = serde_json::from_str(old).expect("an old recipe.json still loads");
        assert_eq!(back.masks[0].hue, 0.0);
        assert_eq!(back.masks[0].sharpness, 0.0);
        assert_eq!(back.masks[0].exposure_ev, -0.4, "…with its own values intact");

        // A maskless recipe's bytes cannot have moved: the new fields live on
        // `LocalAdjustment`, which such a recipe never serializes.
        // (`"hue":` alone would match the global mixer's own axis, which is an
        // ARRAY — the local one is a scalar, hence the `:0.0`.)
        let bare = serde_json::to_string(&EditRecipe::default()).unwrap();
        assert!(!bare.contains("\"hue\":0.0"), "no mask ⇒ no new key: {bare}");
        assert!(!bare.contains("\"sharpness\""), "no mask ⇒ no new key: {bare}");
        // A mask-bearing one always carries them, zero or not (no
        // skip_serializing_if) — the drift is real and total on this side.
        let with_mask = serde_json::to_string(&back).unwrap();
        assert!(with_mask.contains("\"hue\":0.0") && with_mask.contains("\"sharpness\":0.0"));
    }

    #[test]
    fn bitmap_mask_geometry_round_trips_via_json() {
        // The raster variant must serialise under the same "kind" tag family
        // as linear/radial and survive a JSON round-trip byte-faithfully —
        // it is the carrier for AI segmentation masks (gap batch A②).
        let mut r = EditRecipe::default();
        r.masks.push(LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: "out/photo.mask1.png".into() },
            exposure_ev: -0.8,
            ..Default::default()
        });
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains(r#""kind":"bitmap""#), "tagged form: {j}");
        assert!(j.contains("out/photo.mask1.png"));
        let back: EditRecipe = serde_json::from_str(&j).unwrap();
        assert_eq!(r, back);
        // clamp() must pass through untouched (no numeric fields to clamp).
        let mut c = back.clone();
        c.clamp();
        assert_eq!(c.masks[0].mask, r.masks[0].mask);
    }

    #[test]
    fn clamp_reports_discarded_components_and_zeroes_for_a_clean_recipe() {
        let mut crowded = EditRecipe {
            masks: vec![LocalAdjustment {
                components: vec![MaskComponent::default(); 17],
                ..Default::default()
            }],
            ..Default::default()
        };
        let dropped = crowded.clamp();
        assert_eq!(dropped.dropped_masks, 0);
        assert_eq!(dropped.dropped_components, 1);
        assert_eq!(crowded.masks[0].components.len(), 16);

        let mut clean = EditRecipe::default();
        assert_eq!(clean.clamp(), ClampSummary::default());
    }

    #[test]
    fn clamp_counts_curve_and_string_truncation() {
        let mut r = EditRecipe {
            tone_curve: (0..300u32)
                .map(|i| CurvePoint { input: (i % 256) as u8, output: 0 })
                .collect(),
            rationale: "x".repeat(20_000),
            ..Default::default()
        };
        let d = r.clamp();
        assert_eq!(d.truncated_curve_points, 44, "300 points over the 256 cap");
        assert_eq!(d.truncated_string_bytes, 20_000 - 16_384, "rationale past its cap");
        assert!(!d.is_empty(), "curve/string loss alone must flip is_empty");
    }

    /// The dab cap cuts on a TOKEN boundary, not a byte one (R28 2b,
    /// adjudication F5).
    ///
    /// `BrushStroke::dabs` is a `'\n'`-separated token stream that the writer
    /// splits back apart as "the exact inverse" of the reader's join, so a cut
    /// inside a token (`"d 0 "`) rides back out into the sidecar and our own
    /// next read refuses the whole Aggregate — every brush mask in the group
    /// gone, over a size cap.
    ///
    /// MUTATION THIS KILLS: put `cap(&mut s.dabs, MAX_DABS)` back into
    /// `cap_strokes` (a byte truncator on a structured stream). The last
    /// surviving line is then the fragment `"d 0 "` and the whole-token
    /// assertion below fails.
    #[test]
    fn an_over_cap_dab_stream_is_cut_between_tokens_never_inside_one() {
        // 65,536 x "d 0 0" joined by '\n' = 393,215 bytes against the 256 KiB
        // cap — the exact construction the adjudication measured.
        let dabs = std::iter::repeat_n("d 0 0", 65_536).collect::<Vec<_>>().join("\n");
        assert_eq!(dabs.len(), 393_215, "premise: the payload really is over the cap");
        let mut r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Brush {
                    name: "brush".into(),
                    blend_mode: 0,
                    value: 1.0,
                    inverted: false,
                    strokes: vec![BrushStroke { dabs: dabs.clone(), ..Default::default() }],
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let dropped = r.clamp();
        let MaskGeometry::Brush { strokes, .. } = &r.masks[0].mask else { panic!() };
        let kept = &strokes[0].dabs;

        assert!(kept.len() <= 256 * 1024, "the cap is still a bound: {}", kept.len());
        // The property that matters: EVERY surviving line is a whole token of
        // the grammar, and the stream carries no trailing separator.
        assert!(!kept.ends_with('\n'), "storage form joins, never terminates");
        for (i, tok) in kept.split('\n').enumerate() {
            assert_eq!(tok, "d 0 0", "token {i} was cut apart: {tok:?}");
        }
        // A ceiling, not an exact size: the cut lands on the last separator at
        // or before the cap, so up to one token's worth is given up.
        let n = kept.split('\n').count();
        assert_eq!(n, 43_690, "43,690 x 6 - 1 = 262,139 bytes, the largest whole-token fit");
        assert_eq!(dropped.truncated_string_bytes, 393_215 - 262_139);
        // Truncating a stream is not dropping a stroke.
        assert_eq!(dropped.dropped_components, 0);
    }

    /// The other end of the ceiling: a stream whose FIRST token is already
    /// over the cap truncates to nothing, and a Paint with no dabs is not a
    /// stroke — the reader refuses one (`xmp::parse_dabs` errors on zero
    /// tokens), so keeping it would hand the writer an empty `<rdf:li>` and
    /// lose the whole Aggregate at the next read anyway. Dropped and COUNTED
    /// instead of silently emitted.
    ///
    /// Only a hand-edited `recipe.json` reaches this: the import path bounds
    /// one token at 256 bytes (`xmp::dab_token_is_known`).
    #[test]
    fn a_single_token_larger_than_the_cap_drops_the_stroke_and_says_so() {
        let mut r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Brush {
                    name: "brush".into(),
                    blend_mode: 0,
                    value: 1.0,
                    inverted: false,
                    strokes: vec![
                        BrushStroke { dabs: format!("d 0 {}", "9".repeat(300_000)), ..Default::default() },
                        BrushStroke { dabs: "r 0.5\nd 0 0".into(), ..Default::default() },
                    ],
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let dropped = r.clamp();
        let MaskGeometry::Brush { strokes, .. } = &r.masks[0].mask else { panic!() };
        assert_eq!(strokes.len(), 1, "the dab-less stroke is gone");
        assert_eq!(strokes[0].dabs, "r 0.5\nd 0 0", "the healthy stroke is untouched");
        assert_eq!(dropped.dropped_components, 1, "and it was disclosed, not swallowed");
    }

    /// 16-lane scan L14/L15/L16: field-by-field accumulator copies silently
    /// dropped the two new counts, and the shared formatter printed
    /// "0 mask(s) and 0 component(s)" for a curve-only loss.
    #[test]
    fn absorb_folds_all_four_fields_and_describe_names_only_real_losses() {
        let mut acc = ClampSummary::default();
        acc.absorb(ClampSummary {
            dropped_masks: 1,
            dropped_components: 0,
            truncated_curve_points: 44,
            truncated_string_bytes: 0,
        });
        acc.absorb(ClampSummary {
            dropped_masks: 0,
            dropped_components: 2,
            truncated_curve_points: 1,
            truncated_string_bytes: 7,
        });
        assert_eq!(
            acc,
            ClampSummary {
                dropped_masks: 1,
                dropped_components: 2,
                truncated_curve_points: 45,
                truncated_string_bytes: 7,
            }
        );
        assert_eq!(
            acc.describe(),
            "1 mask(s), 2 mask component(s), 45 curve point(s), 7 string byte(s)"
        );
        let curve_only = ClampSummary { truncated_curve_points: 44, ..Default::default() };
        assert_eq!(
            curve_only.describe(),
            "44 curve point(s)",
            "zero categories stay OUT of the line — '0 mask(s)' was a false all-clear"
        );
    }

    #[test]
    fn a_non_finite_range_drops_the_whole_mask_on_clamp() {
        let mut recipe = EditRecipe {
            masks: vec![LocalAdjustment {
                range: Some(RangeMask::Luminance {
                    lo_outer: 0.0,
                    lo: f32::NAN,
                    hi: 0.8,
                    hi_outer: 1.0,
                }),
                exposure_ev: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        recipe.clamp();
        assert!(recipe.masks.is_empty());
    }
}
