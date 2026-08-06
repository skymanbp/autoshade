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

    // --- Geometry (optional) ------------------------------------------------
    /// Clockwise straighten angle in degrees, e.g. -2.5..=2.5 for horizons.
    pub straighten_deg: f32,
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

impl Default for EditRecipe {
    fn default() -> Self {
        Self {
            version: CALIB_ERA,
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
            hsl: Hsl::default(),
            color_grade: ColorGrade::default(),
            sharpening: 0.0,
            noise_reduction: 0.0,
            lens_vignette: 0.0,
            lens_vignette_mid: 50.0,
            lens_distortion: 0.0,
            straighten_deg: 0.0,
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

/// In-camera lens profile corrections in ENGINE space: per-knot factors over
/// the normalised radius (0 = centre, 1 = corner; knot `i` sits at
/// `(i + 0.5) / (n − 1)`, the placement RawTherapee's metadata path uses).
/// `vignette` holds linear-light GAINS; `distortion` / `ca_r` / `ca_b` hold
/// radius scale factors (CA multiplies the distortion map per channel).
/// Conversion from the camera's raw integers lives in `lensmeta` — this
/// struct is camera-agnostic on purpose. Engine-only, never written to XMP.
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
}

impl LensProfile {
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
        for g in self.vignette.iter_mut() {
            *g = g.clamp(0.25, 4.0);
        }
        for f in self.distortion.iter_mut() {
            *f = f.clamp(0.7, 1.3);
        }
        for f in self.ca_r.iter_mut().chain(self.ca_b.iter_mut()) {
            *f = f.clamp(0.98, 1.02);
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
    /// Optional Range Mask refinement intersected with `mask` (final weight =
    /// geometry × range). `None` = pure geometry (v1-compatible).
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
    pub clarity: f32,
    pub dehaze: f32,
    pub texture: f32,
    pub saturation: f32,
    /// Relative warm/cool shift (NOT Kelvin) → `crs:LocalTemperature`.
    pub temperature: f32,
    pub tint: f32,
    /// Local luminance noise reduction, 0..=100 → `crs:LocalLuminanceNoise`.
    /// For "this region is noisy" requests; smooths only inside the mask.
    pub noise_reduction: f32,
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
            saturation: 0.0,
            temperature: 0.0,
            tint: 0.0,
            noise_reduction: 0.0,
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
// KNOWN BOUNDARY: serde does not support `deny_unknown_fields` on
// internally-tagged enums (this one and RangeMask) — an unknown field inside
// a variant deserializes silently and is dropped on the next rewrite. The
// top-level recipe and every plain struct DO deny.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaskGeometry {
    /// Linear gradient — the zero→full vector sets direction + falloff width.
    /// Maps to ACR `What="Mask/Gradient"`.
    Linear { zero_x: f32, zero_y: f32, full_x: f32, full_y: f32 },
    /// Radial/elliptical gradient. Maps to ACR `What="Mask/CircularGradient"`.
    /// `feather` IS engine-rendered (clamped to 0..1). `roundness` is **carried
    /// only** — persisted, XMP round-tripped and accepted from the AI, but NOT
    /// rendered: the engine draws a pure ellipse. Its scale and sign are
    /// unverified (docs/V2_PLAN.md §7 item 1), so honouring it would reshape
    /// imported and AI-authored masks on a guess; see `mask_weight` in render.rs.
    Radial {
        top: f32,
        left: f32,
        bottom: f32,
        right: f32,
        feather: f32,
        roundness: f32,
        flipped: bool,
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
}

/// Lightroom's Range Mask: a per-pixel refinement INTERSECTED with the mask's
/// geometry (final weight = geometry × range), so a sky gradient can affect only
/// the bright pixels, or only the blues. Serialised to XMP as a second
/// `Mask/RangeMask` component inside `crs:CorrectionMasks` — structure verified
/// against the user's own Lightroom sidecars (e.g. `_DSC9245.xmp` LumRange,
/// `_DSC9303.xmp` PointModels).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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

impl EditRecipe {
    /// Clamp every slider into its documented legal range. The AI is
    /// instructed to stay in range, but we never trust the input blindly —
    /// an out-of-range value would otherwise corrupt the render downstream.
    pub fn clamp(&mut self) {
        // f32::clamp passes a NaN receiver STRAIGHT THROUGH — a non-finite
        // slider from malformed input (hand-edited JSON, foreign XMP) must
        // collapse to the neutral 0.0 instead, the same corrupt-component-
        // goes-inert rule the crop and range-mask guards below follow.
        let c = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
        // SIZE limits, not just value limits. Every field below is bounded,
        // but a recipe is also a set of VECTORS, and a crafted one (hand-
        // edited JSON, a hostile POST to the local web server) could carry
        // thousands of masks or curve points: each active mask costs a
        // full-frame pass and each curve is cloned + sorted per render, so
        // the render thread can be monopolised without a single
        // out-of-range number. The caps sit far above any real edit (the GUI
        // offers a handful of masks; a tone curve has at most a few dozen
        // points), so they can only ever truncate abuse.
        const MAX_MASKS: usize = 64;
        const MAX_CURVE_POINTS: usize = 256;
        const MAX_BASE_KNOTS: usize = 256;
        self.masks.truncate(MAX_MASKS);
        self.base_curve.truncate(MAX_BASE_KNOTS);
        for curve in [
            &mut self.tone_curve,
            &mut self.red_curve,
            &mut self.green_curve,
            &mut self.blue_curve,
        ] {
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
        self.hsl.clamp();
        self.color_grade.clamp();
        self.sharpening = c(self.sharpening, 0.0, 150.0);
        self.noise_reduction = c(self.noise_reduction, 0.0, 100.0);
        self.lens_vignette = c(self.lens_vignette, -100.0, 100.0);
        self.lens_vignette_mid = c(self.lens_vignette_mid, 0.0, 100.0);
        self.lens_distortion = c(self.lens_distortion, -100.0, 100.0);
        self.lens_profile.clamp();
        self.straighten_deg = c(self.straighten_deg, -45.0, 45.0);
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
        // sensible neutral to collapse to).
        self.masks.retain(|m| match &m.mask {
            MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
                [zero_x, zero_y, full_x, full_y].iter().all(|v| v.is_finite())
            }
            MaskGeometry::Radial { top, left, bottom, right, feather, roundness, .. } => {
                [top, left, bottom, right, feather, roundness].iter().all(|v| v.is_finite())
            }
            MaskGeometry::Bitmap { .. } => true,
        });
        // Finite is NOT sufficient. Mask coordinates are NORMALISED (0..1
        // over the frame, a little outside for handles dragged past the
        // edge), and the engine squares differences: a legal-looking 1e30
        // overflows to Inf, Inf/Inf yields NaN weights, and those pixels
        // quantise to black. Bound the magnitude to the range the UI can
        // actually produce.
        const COORD_LIMIT: f32 = 8.0;
        for m in self.masks.iter_mut() {
            match &mut m.mask {
                MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
                    for v in [zero_x, zero_y, full_x, full_y] {
                        *v = v.clamp(-COORD_LIMIT, COORD_LIMIT);
                    }
                }
                MaskGeometry::Radial { top, left, bottom, right, feather, roundness, .. } => {
                    for v in [top, left, bottom, right] {
                        *v = v.clamp(-COORD_LIMIT, COORD_LIMIT);
                    }
                    // feather/roundness are 0..1 fractions everywhere that
                    // reads them (render, XMP); a stored 2.0 was a value the
                    // engine silently treated as 1.0 but persistence kept.
                    *feather = feather.clamp(0.0, 1.0);
                    *roundness = roundness.clamp(0.0, 1.0);
                }
                MaskGeometry::Bitmap { .. } => {}
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
                &mut m.saturation, &mut m.temperature, &mut m.tint,
            ] {
                *v = c(*v, -100.0, 100.0);
            }
            m.noise_reduction = c(m.noise_reduction, 0.0, 100.0);
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
            // Non-finite values first (hand-edited/foreign JSON): a NaN
            // luminance bound would PANIC f32::clamp below (its min ≤ max
            // assert sees NaN as the max), and a NaN colour reference feeds
            // NaN weights into the render — drop the range entirely, the same
            // policy as a NaN crop.
            let range_finite = match &m.range {
                Some(RangeMask::Luminance { lo_outer, lo, hi, hi_outer }) => {
                    lo_outer.is_finite() && lo.is_finite() && hi.is_finite() && hi_outer.is_finite()
                }
                Some(RangeMask::Color { r, g, b, amount, px, py }) => {
                    [r, g, b, amount, px, py].iter().all(|v| v.is_finite())
                }
                None => true,
            };
            if !range_finite {
                m.range = None;
            }
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
    }

    /// Taste guardrail for **AI-proposed** recipes (never manual edits): keep a
    /// finished develop from over-cooking the tone. Two rules:
    ///  1. **Couple highlight recovery to the white point.** Pulling Highlights
    ///     negative without raising Whites drags specular whites (sea foam, clouds,
    ///     sun glints) to grey — so recovery lifts Whites proportionally. This is the
    ///     principled "keep whites white", at the recipe layer (the renderer stays
    ///     faithful; it does not override the recipe).
    ///  2. **Soft-cap over-aggressive tone moves** toward a tasteful ceiling with a
    ///     smooth knee (not a hard clip), so Highlights/Shadows asymptote near ±70
    ///     and Whites/Blacks near ±45 instead of slamming to the ±100 schema bound.
    pub fn temper(&mut self) {
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
        // Couple recovery to whites BEFORE soft-capping (uses the original strength).
        if self.highlights < 0.0 {
            self.whites = self.whites.max((-self.highlights * 0.3).min(50.0));
        }
        self.highlights = soft_cap(self.highlights, 50.0, 70.0);
        self.shadows = soft_cap(self.shadows, 50.0, 70.0);
        self.whites = soft_cap(self.whites, 30.0, 45.0);
        self.blacks = soft_cap(self.blacks, 30.0, 45.0);
        // Same restraint on each local mask's tone sliders.
        for m in self.masks.iter_mut() {
            if m.highlights < 0.0 {
                m.whites = m.whites.max((-m.highlights * 0.3).min(50.0));
            }
            m.highlights = soft_cap(m.highlights, 50.0, 70.0);
            m.shadows = soft_cap(m.shadows, 50.0, 70.0);
            m.whites = soft_cap(m.whites, 30.0, 45.0);
            m.blacks = soft_cap(m.blacks, 30.0, 45.0);
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
                ..self.clone()
            } == EditRecipe::default()
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(r.masks[0].range, None, "NaN luminance bound drops the range");
        assert_eq!(r.masks[1].range, None, "NaN colour reference drops the range");
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
            },
            ..Default::default()
        };
        assert!(stamped.is_noop(), "as-stamped profile must stay a no-op");
        // Turning one component OFF is a user edit that must survive.
        let mut toggled = stamped.clone();
        toggled.lens_profile.distortion_on = false;
        assert!(!toggled.is_noop(), "a toggle away from the stamp is an edit");
        // Round-trip.
        let json = serde_json::to_string(&toggled).unwrap();
        let back: EditRecipe = serde_json::from_str(&json).unwrap();
        assert_eq!(back.lens_profile, toggled.lens_profile);
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
        let mut hot = EditRecipe { highlights: -78.81, whites: 10.27, shadows: 95.0, ..Default::default() };
        hot.temper();
        assert!(hot.whites >= 23.0, "recovery must lift the white point: whites={}", hot.whites);
        assert!(hot.highlights >= -65.0, "highlights not tempered: {}", hot.highlights);
        assert!(hot.highlights <= -55.0, "highlights over-tempered (lost commitment): {}", hot.highlights);
        assert!(hot.shadows >= 55.0 && hot.shadows < 70.0, "shadows soft-cap off: {}", hot.shadows);

        // A modest recipe keeps its tone moves; recovery still nudges whites a touch.
        let mut mild = EditRecipe { highlights: -30.0, shadows: 20.0, whites: 5.0, ..Default::default() };
        mild.temper();
        assert_eq!(mild.highlights, -30.0, "modest highlights must pass through");
        assert_eq!(mild.shadows, 20.0, "modest shadows must pass through");
        assert!(mild.whites >= 9.0, "modest recovery still protects speculars: {}", mild.whites);
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
                        feather: 0.5, roundness: 0.0, flipped: false,
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
        // A mask WITHOUT a "range" key (pre-range recipes) defaults to None.
        let old_mask = r#"{ "masks": [ { "name": "sky" } ] }"#;
        assert_eq!(serde_json::from_str::<EditRecipe>(old_mask).unwrap().masks[0].range, None);
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
}
