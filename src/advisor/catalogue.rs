//! `RECIPE_CONTROLS` — the ONE registry of the develop controls the AI
//! contract carries (user feedback #12, R23-1).
//!
//! Before this table the [`EditRecipe`] was the de-facto source and every
//! downstream copy was hand-transcribed from it, with a single comment
//! ("Mirrors `src/recipe.rs` — keep in sync") as the only constraint: the
//! advisor's strict JSON schema (twice — the recipe and the nested local
//! adjustment), the proposer prompt's prose, the eval harness's comparison
//! table and its report order, and the style index's `REF_KEYS`/`MAP` pair.
//! A control added to the recipe therefore reached the renderer, the XMP
//! writer and the GUI while staying INVISIBLE to the AI and to every ruler
//! that measures the AI — silently, forever.
//!
//! So the registry is the source now, and it is enforced from two sides:
//!
//!   * **the compiler** — [`global_value`] and [`local_value`] destructure
//!     `EditRecipe` / `LocalAdjustment` with NO `..` rest pattern and consume
//!     every binding in a `match`, so a new field is a build error (an
//!     un-destructured field breaks the pattern; a destructured-but-unmapped
//!     one is an `unused_variables` deny) until its row exists here.
//!     [`color_grade_value`] and [`hsl_value`] do the same for the two nested
//!     objects the schema spells out field by field.
//!   * **the tests** — the registry, the serde key set and the strict
//!     schema's `required` array are compared THREE ways (both mirrors), and
//!     every numeric row's stated range is re-derived from
//!     `EditRecipe::clamp` itself, so the range the model is told can never
//!     be a range the engine does not enforce.
//!
//! Consumers: [`edit_recipe_schema`] (the advisor's strict response schema),
//! [`prompt_catalogue`] (the proposer prompt's control list), `eval.rs` (the
//! comparison ruler) and `style.rs` (the reference key set, asserted against
//! this table).

use serde_json::{json, Value};

use crate::recipe::{
    ColorGrade, Crop, CurvePoint, EditRecipe, Hsl, HSL_BANDS, LensProfile, LocalAdjustment,
};

/// The JSON shape one control takes in the advisor's strict response schema —
/// or, for the engine-only carriers, the shape it has in `recipe.json` alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A plain `number`.
    Number,
    /// `["number","null"]` — null is the control's "no opinion" state.
    ///
    /// Where that null LANDS differs by control, on purpose: `temperature_k`
    /// carries it all the way into the recipe (`Option<f32>`, "keep as-shot"),
    /// while the three manual LENS fields are plain `f32` the engine always
    /// renders — their null lives only in the advisor's parse layer
    /// (`openai::take_lens_opinion`), where it means "the photographer's own
    /// value stands" and 0 means "zero it". Adding an Option to the recipe
    /// instead would put a fourth spelling of "unset" into `recipe.json`, the
    /// XMP writer and R21's fingerprint for a value the renderer must always
    /// have (R23-1b).
    NullableNumber,
    /// An `integer` (the schema-era stamp).
    Integer,
    /// A `boolean`.
    Bool,
    /// A `string`.
    Text,
    /// An array of `{input,output}` curve points, both 0..255.
    Curve,
    /// The 8-band HSL mixer object (three arrays of 8 numbers).
    Hsl,
    /// The 3-wheel + global colour-grade object (14 flat numbers).
    ColorGrade,
    /// The normalised crop rectangle, or null for "keep the whole frame".
    NullableCrop,
    /// An array of local adjustments (the nested schema mirror).
    Masks,
    /// A mask geometry: the linear/radial tagged union.
    MaskGeometry,
    /// A Range-Mask refinement (luminance/colour tagged union), or null.
    NullableRangeMask,
    /// A structure only the engine reads: base-curve knots, the in-camera lens
    /// profile, extra mask components, linear-light colour gains, the mask
    /// role. These have no schema spelling at all — [`Shape::schema`] answers
    /// `None`, which is why every carrier row is also `engine_only`.
    EngineCarrier,
}

impl Shape {
    /// The strict-mode JSON-schema property for this shape, or `None` for the
    /// engine-only carriers (which the schema must never ask for).
    pub fn schema(self) -> Option<Value> {
        let num = || json!({"type": "number"});
        Some(match self {
            Shape::Number => num(),
            Shape::NullableNumber => json!({"type": ["number", "null"]}),
            Shape::Integer => json!({"type": "integer"}),
            Shape::Bool => json!({"type": "boolean"}),
            Shape::Text => json!({"type": "string"}),
            Shape::Curve => curve_arr(),
            Shape::Hsl => hsl_schema(),
            Shape::ColorGrade => color_grade_schema(),
            Shape::NullableCrop => crop_schema(),
            Shape::Masks => json!({"type": "array", "items": local_adjustment_schema()}),
            Shape::MaskGeometry => mask_geometry_schema(),
            Shape::NullableRangeMask => range_mask_schema(),
            Shape::EngineCarrier => return None,
        })
    }

    /// Is this shape a single number the eval harness / the range-honesty test
    /// can read and write through one JSON field?
    pub fn is_scalar(self) -> bool {
        matches!(self, Shape::Number | Shape::NullableNumber)
    }
}

/// Where a control lands in a Lightroom / Camera Raw sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrsKey {
    /// Exactly ONE `crs:` attribute on the sidecar's own `rdf:Description` —
    /// readable with `xmp::crs_f32`, which is what makes a control measurable
    /// against the user's real edits (the eval ruler is derived from these).
    Attr(&'static str),
    /// A FAMILY of keys (per band / wheel / channel), or a key that lives on a
    /// nested element rather than the Description's attributes. The descriptor
    /// is documentation — the exact spellings live in `xmp.rs`, and the
    /// per-band/per-wheel expansions this module needs are
    /// [`hsl_expansion`] and [`COLOR_GRADE_CRS`].
    Family(&'static str),
    /// No classic-ACR counterpart: engine calibration, or app-internal
    /// provenance (rationale/confidence).
    None,
}

impl CrsKey {
    /// The single attribute spelling, for the consumers that read one key.
    pub fn attr(self) -> Option<&'static str> {
        match self {
            CrsKey::Attr(k) => Some(k),
            _ => None,
        }
    }

    /// Does this control have a sidecar property of its OWN — one key (or key
    /// family) that carries it out and reads it back?
    pub fn is_owned(self) -> bool {
        !matches!(self, CrsKey::None)
    }
}

/// **The five-tier control registry** (R24-5 M0, feedback #15b).
///
/// Two independent facts decide what a control IS, and until now nothing wrote
/// them down together: does the ENGINE render it, and does it reach the
/// LIGHTROOM SIDECAR? [`Control::engine_only`] answers a third, different
/// question (may the AI set it), and `CrsKey` answered half of the second —
/// so "the GUI offers a slider the engine ignores" and "the save silently
/// drops what you are looking at" were both unrepresentable claims. Each tier
/// below names one (renders × exports) combination, and the disclosure
/// surfaces are DERIVED from it (`xmp::global_export_losses`,
/// `xmp::unmodelled_global_crs`) rather than restating a list.
///
/// Nothing here changes a control's behaviour: this round declares today's
/// truth and asserts it. Moving a control BETWEEN tiers (the LR-gap batches
/// B2–B5 — global Texture, Detail/Defringe, Transform/Calibration, mask
/// geometry) is the work this registry exists to make reviewable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// The engine renders it AND it round-trips through its own `crs:`
    /// property. What a user assumes every control is.
    Rendered,
    /// Round-trips through its own `crs:` property, and the engine renders
    /// NOTHING from it. Legitimate only for controls that carry meaning rather
    /// than pixels — every member must be listed in [`CARRIED_ONLY_GLOBAL`] /
    /// [`CARRIED_ONLY_LOCAL`] with the reason, or the inclusion test fails.
    CarriedOnly,
    /// Carried through the sidecar verbatim and never interpreted — neither
    /// engine nor reader gives it meaning. Reserved for the LR-only property
    /// blocks the XMP merge already preserves (Transform, Calibration, the
    /// camera Look): those are not recipe fields today, so the tier has NO
    /// registry member yet, and the B4 batch is what gives it one.
    PassThrough,
    /// The engine renders it and the sidecar cannot carry it — what you see
    /// here is NOT what Lightroom will show. The half of the disclosure story
    /// that had no name: `xmp::global_export_losses` is generated from exactly
    /// this tier.
    RenderedNotExported,
    /// Has no `crs:` property of its own; only a value DERIVED from it is
    /// written (the stamped as-shot white balance, which reaches the sidecar
    /// as `crs:Temperature`/`crs:Tint` when the user made no WB choice). It
    /// therefore never reads back as itself.
    DerivedWriteOnly,
}

impl Tier {
    /// Does the develop engine's output change when this control changes?
    pub fn renders(self) -> bool {
        match self {
            // DerivedWriteOnly is a rendering control too — its members are
            // the as-shot WB the engine develops with when the recipe states
            // no temperature of its own. The tier names its EXPORT shape,
            // which is the fact that had no other home.
            Tier::Rendered | Tier::RenderedNotExported | Tier::DerivedWriteOnly => true,
            Tier::CarriedOnly | Tier::PassThrough => false,
        }
    }

    /// Does this control own a `crs:` property that carries it out and back?
    /// Cross-checked against [`Control::crs`] by the registry's own test, so
    /// the tier can never claim an export shape the writer does not have.
    pub fn owns_crs_key(self) -> bool {
        match self {
            Tier::Rendered | Tier::CarriedOnly | Tier::PassThrough => true,
            Tier::RenderedNotExported | Tier::DerivedWriteOnly => false,
        }
    }
}

/// The `CarriedOnly` allow-list for [`RECIPE_CONTROLS`] — global controls a
/// surface may set although the engine renders nothing from them, each with
/// the reason. EMPTY today: every global with a `crs:` property of its own is
/// rendered. (The default policy for future entries is SF4-C — Adobe-only
/// operators we round-trip rather than approximate.)
pub const CARRIED_ONLY_GLOBAL: &[(&str, &str)] = &[];

/// The `CarriedOnly` allow-list for [`LOCAL_CONTROLS`].
pub const CARRIED_ONLY_LOCAL: &[(&str, &str)] = &[(
    "name",
    "a label, not an operator: it round-trips as crs:CorrectionName/MaskName so \
     a mask keeps its identity across Lightroom, and no pixel depends on it",
)];

/// One develop control: what it is called on the wire, what shape and band it
/// takes, whether the AI may set it, where it lands in an XMP, and the one
/// line the model is told about it.
#[derive(Debug, Clone, Copy)]
pub struct Control {
    /// The serde field name — the SAME string in `recipe.json`, in the strict
    /// schema and in the eval report.
    pub name: &'static str,
    pub shape: Shape,
    /// The documented legal band, exactly what `EditRecipe::clamp` enforces
    /// (`None` for the non-scalar shapes). Pinned by
    /// `stated_ranges_are_the_ranges_clamp_enforces`.
    pub range: Option<(f32, f32)>,
    /// The neutral / default value, spelled for the prompt.
    pub neutral: &'static str,
    /// ENGINE-ONLY: the advisor schema never asks the model for it, and
    /// `pipeline::carry_over_unrepresentable` re-attaches it after a refine.
    ///
    /// Since R23-1b every remaining one is a PERMANENT design marker, not
    /// staged work: the stamped calibration (`as_shot_k`, `as_shot_tint`,
    /// `base_curve`, `lens_profile`) is the engine's own measurement of this
    /// photo, and `components` / `color_gains` / `role` are mask state with no
    /// classic-ACR spelling (recipe.rs states the first as an explicit design
    /// decision). `enabled` stays here for a reason of its own — see
    /// [`LOCAL_CONTROLS`]' row for it.
    pub engine_only: bool,
    pub crs: CrsKey,
    /// WHAT this control is, on the (renders × exports) axes — see [`Tier`].
    ///
    /// `None` is not "unclassified": it means the row is NOT a develop control
    /// at all but part of the response ENVELOPE (the era stamp) or the AI's own
    /// provenance (`rationale`, `confidence`), or mask bookkeeping the solver
    /// owns (`role`). A `None` row may never own a `crs:` property — asserted,
    /// so the escape hatch cannot be used to dodge a classification.
    ///
    /// Adding a field to `EditRecipe` / `LocalAdjustment` already fails the
    /// build until it has a row here (the [`global_value`] / [`local_value`]
    /// destructures); this field makes the row itself incomplete until its
    /// tier is decided, because a struct literal cannot omit it.
    pub tier: Option<Tier>,
    /// One line for the model: what this control does.
    pub purpose: &'static str,
}

impl Control {
    /// `(-100..100, neutral 0)` — the range/neutral parenthetical the prompt
    /// catalogue prints. Integers and structures print their shape instead of
    /// a numeric band.
    pub fn spec(&self) -> String {
        match self.range {
            Some((lo, hi)) => format!("{}..{}, neutral {}", trim_num(lo), trim_num(hi), self.neutral),
            None => format!("neutral {}", self.neutral),
        }
    }
}

/// `-100` not `-100.0`, `0.65` still `0.65` — the prompt reads like Lightroom's
/// own labels.
fn trim_num(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// Every field of [`EditRecipe`], in DECLARATION order — which is also the
/// order the strict schema's `required` array takes, so the generated schema
/// is byte-identical to the hand-written mirror it replaced.
pub const RECIPE_CONTROLS: [Control; 34] = [
    Control {
        name: "version",
        shape: Shape::Integer,
        range: None,
        neutral: "2",
        engine_only: false,
        crs: CrsKey::None,
        tier: None,
        purpose: "schema + calibration-era stamp; return 2",
    },
    Control {
        name: "coord_era",
        shape: Shape::Integer,
        range: None,
        neutral: "1",
        // ENGINE-ONLY on purpose: this says which FRAME `crop` and the mask
        // geometries are drawn in, and the model only ever sees the display
        // frame — asking it would invite a wrong answer about a fact it
        // cannot observe. Stamped where the model's JSON becomes a recipe
        // (`advisor::openai`), exactly like the other measured stamps.
        engine_only: true,
        crs: CrsKey::None,
        tier: None,
        purpose: "engine bookkeeping: which coordinate frame crop/masks are stored in \
                  (0 = the pre-v0.30 sensor frame, 1 = the EXIF display frame)",
    },
    Control {
        name: "exposure_ev",
        shape: Shape::Number,
        range: Some((-5.0, 5.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Exposure2012"),
        tier: Some(Tier::Rendered),
        purpose: "global exposure in stops (EV)",
    },
    Control {
        name: "contrast",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Contrast2012"),
        tier: Some(Tier::Rendered),
        purpose: "global contrast about the midtone",
    },
    Control {
        name: "highlights",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Highlights2012"),
        tier: Some(Tier::Rendered),
        purpose: "recover (negative) or open up (positive) the BRIGHT tones",
    },
    Control {
        name: "shadows",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Shadows2012"),
        tier: Some(Tier::Rendered),
        purpose: "lift (positive) or deepen (negative) the DARK tones",
    },
    Control {
        name: "whites",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Whites2012"),
        tier: Some(Tier::Rendered),
        purpose: "the WHITE point — how bright the brightest tones sit",
    },
    Control {
        name: "blacks",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Blacks2012"),
        tier: Some(Tier::Rendered),
        purpose: "the BLACK point — how deep the darkest tones sit",
    },
    Control {
        name: "temperature_k",
        shape: Shape::NullableNumber,
        range: Some((2000.0, 40000.0)),
        neutral: "null = keep as-shot",
        engine_only: false,
        crs: CrsKey::Attr("Temperature"),
        tier: Some(Tier::Rendered),
        purpose: "white balance as an ABSOLUTE colour temperature in Kelvin (a TARGET, not a \
                  shift): lower = cooler/bluer result, higher = warmer. null keeps the camera's \
                  as-shot WB",
    },
    Control {
        name: "tint",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Tint"),
        tier: Some(Tier::Rendered),
        purpose: "green (negative) / magenta (positive) shift RELATIVE to the as-shot white \
                  balance — unlike temperature_k this one is a delta, not an absolute value",
    },
    Control {
        name: "as_shot_k",
        shape: Shape::NullableNumber,
        range: Some((2000.0, 40000.0)),
        neutral: "null = unknown",
        engine_only: true,
        crs: CrsKey::None,
        tier: Some(Tier::DerivedWriteOnly),
        purpose: "the camera's OWN white balance in absolute Kelvin, stamped per photo by the \
                  engine — the anchor temperature_k is measured against",
    },
    Control {
        name: "as_shot_tint",
        shape: Shape::NullableNumber,
        range: Some((-100.0, 100.0)),
        neutral: "null = unknown",
        engine_only: true,
        crs: CrsKey::None,
        tier: Some(Tier::DerivedWriteOnly),
        purpose: "the as-shot green/magenta companion of as_shot_k (display + XMP honesty only)",
    },
    Control {
        name: "vibrance",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Vibrance"),
        tier: Some(Tier::Rendered),
        purpose: "saturation that protects the already-saturated colours",
    },
    Control {
        name: "saturation",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Saturation"),
        tier: Some(Tier::Rendered),
        purpose: "uniform saturation of every colour",
    },
    Control {
        name: "clarity",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Clarity2012"),
        tier: Some(Tier::Rendered),
        purpose: "midtone local contrast / punch",
    },
    Control {
        name: "dehaze",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Dehaze"),
        tier: Some(Tier::Rendered),
        purpose: "atmospheric haze removal (negative ADDS haze)",
    },
    Control {
        name: "hsl",
        shape: Shape::Hsl,
        range: None,
        neutral: "every band 0",
        engine_only: false,
        crs: CrsKey::Family("{Hue,Saturation,Luminance}Adjustment<Band> (8 bands)"),
        tier: Some(Tier::Rendered),
        purpose: "the 8-band colour mixer: `hue`, `saturation` and `luminance` are each an array \
                  of EXACTLY 8 numbers (-100..100) in the FIXED band order red, orange, yellow, \
                  green, aqua, blue, purple, magenta",
    },
    Control {
        name: "color_grade",
        shape: Shape::ColorGrade,
        range: None,
        neutral: "every wheel 0, blending 50",
        engine_only: false,
        crs: CrsKey::Family("SplitToning* + ColorGrade*"),
        tier: Some(Tier::Rendered),
        purpose: "3-wheel + global colour grading: per region (shadow/midtone/highlight/global) a \
                  `*_hue` 0..360, `*_sat` 0..100 and `*_lum` -100..100, plus `blending` 0..100 \
                  (region overlap) and `balance` -100..100 (shadow/highlight split)",
    },
    Control {
        name: "sharpening",
        shape: Shape::Number,
        range: Some((0.0, 150.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("Sharpness"),
        tier: Some(Tier::Rendered),
        purpose: "capture sharpening amount (0..150 here; a sidecar's 0..100 Sharpness × 1.5)",
    },
    Control {
        name: "noise_reduction",
        shape: Shape::Number,
        range: Some((0.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("LuminanceSmoothing"),
        tier: Some(Tier::Rendered),
        purpose: "global luminance noise reduction",
    },
    Control {
        name: "lens_vignette",
        shape: Shape::NullableNumber,
        range: Some((-100.0, 100.0)),
        neutral: "null = no opinion (0 = zero the photographer's own value)",
        engine_only: false,
        crs: CrsKey::Attr("VignetteAmount"),
        tier: Some(Tier::Rendered),
        purpose: "manual LENS vignette compensation — a radial gain in LINEAR light before any \
                  tonal work (a falloff correction, NOT a creative corner darkening); null means \
                  you have NO opinion and the photographer's own value stands",
    },
    Control {
        name: "lens_vignette_mid",
        shape: Shape::NullableNumber,
        range: Some((0.0, 100.0)),
        neutral: "null = no opinion (the engine's own neutral is 50)",
        engine_only: false,
        crs: CrsKey::Attr("VignetteMidpoint"),
        tier: Some(Tier::Rendered),
        purpose: "where the vignette compensation lands (lower reaches toward the centre); null \
                  means no opinion",
    },
    Control {
        name: "lens_distortion",
        shape: Shape::NullableNumber,
        range: Some((-100.0, 100.0)),
        neutral: "null = no opinion (0 = zero the photographer's own value)",
        engine_only: false,
        crs: CrsKey::Attr("LensManualDistortionAmount"),
        tier: Some(Tier::Rendered),
        purpose: "manual geometric distortion correction (positive straightens BARREL, negative \
                  PINCUSHION); null means you have NO opinion and the photographer's own value \
                  stands",
    },
    Control {
        name: "straighten_deg",
        shape: Shape::Number,
        range: Some((-45.0, 45.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Attr("CropAngle"),
        tier: Some(Tier::Rendered),
        purpose: "clockwise straighten angle in degrees, for a tilted horizon",
    },
    Control {
        name: "crop",
        shape: Shape::NullableCrop,
        range: None,
        neutral: "null = the whole frame",
        engine_only: false,
        crs: CrsKey::Family("HasCrop + Crop{Left,Top,Right,Bottom}"),
        tier: Some(Tier::Rendered),
        purpose: "the kept rectangle in normalised 0..1 frame coordinates; null keeps the full frame",
    },
    Control {
        name: "tone_curve",
        shape: Shape::Curve,
        range: None,
        neutral: "empty = identity",
        engine_only: false,
        crs: CrsKey::Family("ToneCurvePV2012"),
        tier: Some(Tier::Rendered),
        purpose: "master tone curve as {input,output} points, both 0..255 — the free-form \
                  alternative to the Contrast slider",
    },
    Control {
        name: "red_curve",
        shape: Shape::Curve,
        range: None,
        neutral: "empty = identity",
        engine_only: false,
        crs: CrsKey::Family("ToneCurvePV2012Red"),
        tier: Some(Tier::Rendered),
        purpose: "per-channel RED curve (same point form as tone_curve) for a deliberate colour \
                  cast in specific tones",
    },
    Control {
        name: "green_curve",
        shape: Shape::Curve,
        range: None,
        neutral: "empty = identity",
        engine_only: false,
        crs: CrsKey::Family("ToneCurvePV2012Green"),
        tier: Some(Tier::Rendered),
        purpose: "per-channel GREEN curve",
    },
    Control {
        name: "blue_curve",
        shape: Shape::Curve,
        range: None,
        neutral: "empty = identity",
        engine_only: false,
        crs: CrsKey::Family("ToneCurvePV2012Blue"),
        tier: Some(Tier::Rendered),
        purpose: "per-channel BLUE curve",
    },
    Control {
        name: "base_curve",
        shape: Shape::EngineCarrier,
        range: None,
        neutral: "empty = no base look",
        engine_only: true,
        crs: CrsKey::None,
        tier: Some(Tier::RenderedNotExported),
        purpose: "camera-matched base-look knots estimated per photo by the engine (calibration, \
                  never exported to XMP)",
    },
    Control {
        name: "lens_profile",
        shape: Shape::EngineCarrier,
        range: None,
        neutral: "empty = every component off",
        engine_only: true,
        crs: CrsKey::None,
        tier: Some(Tier::RenderedNotExported),
        purpose: "the in-camera lens profile read from the RAW's own metadata (calibration, never \
                  exported to XMP)",
    },
    Control {
        name: "masks",
        shape: Shape::Masks,
        range: None,
        neutral: "empty = global only",
        engine_only: false,
        crs: CrsKey::Family("MaskGroupBasedCorrections"),
        tier: Some(Tier::Rendered),
        purpose: "local (masked) adjustments — one entry per mask, controls listed below",
    },
    Control {
        name: "rationale",
        shape: Shape::Text,
        range: None,
        neutral: "\"\"",
        engine_only: false,
        crs: CrsKey::None,
        tier: None,
        purpose: "one or two sentences explaining these choices, for the photographer to \
                  sanity-check",
    },
    Control {
        name: "confidence",
        shape: Shape::Number,
        range: Some((0.0, 1.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::None,
        tier: None,
        purpose: "your own confidence in this develop, 0..1",
    },
];

/// Every field of [`LocalAdjustment`], in DECLARATION order — the nested
/// schema mirror. Its field set is NOT a subset of the global one (`texture`
/// exists only here; `temperature` is a relative shift, not Kelvin), which is
/// exactly why the drift had two independent surfaces.
pub const LOCAL_CONTROLS: [Control; 24] = [
    Control {
        name: "mask",
        shape: Shape::MaskGeometry,
        range: None,
        neutral: "a linear gradient",
        engine_only: false,
        crs: CrsKey::Family("CorrectionMasks/Mask"),
        tier: Some(Tier::Rendered),
        purpose: "WHERE this adjustment applies: kind=linear (zero_* = the no-effect edge, \
                  full_* = the full-effect edge, in 0..1 frame coords) for skies/horizons, or \
                  kind=radial (top/left/bottom/right + feather 0..1, roundness 0..1, flipped, \
                  and angle in DEGREES counter-clockwise about the ellipse centre, 0 = \
                  axis-aligned — rotate it to follow an oblique subject) for subjects and \
                  vignettes",
    },
    Control {
        name: "components",
        shape: Shape::EngineCarrier,
        range: None,
        neutral: "empty = the base geometry alone",
        engine_only: true,
        crs: CrsKey::None,
        tier: Some(Tier::RenderedNotExported),
        purpose: "extra Add/Subtract/Intersect shapes composed onto `mask` — engine-only by \
                  design: keep an existing mask's `name` byte-exact and they are preserved for you",
    },
    Control {
        name: "enabled",
        shape: Shape::Bool,
        range: None,
        neutral: "true",
        engine_only: true,
        crs: CrsKey::None,
        tier: Some(Tier::RenderedNotExported),
        purpose: "the per-mask eye toggle (mutes the mask without destroying its tuned Amount)",
    },
    // WHY `enabled` STAYS ENGINE-ONLY (R23-1b, weighed and declined).
    //
    // Every value the model could put in this field is either discarded or
    // meaningless, while one of them is destructive:
    //   * a NEW mask — `LocalAdjustment::default()` is already `enabled: true`,
    //     and a model that authors a mask it immediately mutes has authored
    //     nothing. Zero gain.
    //   * an EXISTING mask — the photographer's mute is theirs. A muted mask is
    //     already "state-bearing" for `carry_over_unrepresentable`
    //     (`schema_loses` tests `!m.enabled`), so a refine restores it by name;
    //     letting the model set the field instead would hand it the power to
    //     silence a mask the user deliberately kept, and any guard that stops it
    //     (restore the user's value for every name-matched mask) makes the
    //     field's value discarded anyway.
    // Deleting a mask, the request this field might look like it serves, is
    // already expressible: the model omits it from `masks`.
    Control {
        name: "range",
        shape: Shape::NullableRangeMask,
        range: None,
        neutral: "null = pure geometry",
        engine_only: false,
        crs: CrsKey::Family("CorrectionMasks/Mask/RangeMask"),
        tier: Some(Tier::Rendered),
        purpose: "optional Range-Mask refinement INTERSECTED with the geometry: \
                  {kind:\"luminance\", lo_outer<=lo<=hi<=hi_outer in 0..1} keeps only that \
                  brightness band, {kind:\"color\", r,g,b 0..1 reference, amount 0..1 tolerance, \
                  px,py sample point} keeps only similar colours",
    },
    Control {
        name: "name",
        shape: Shape::Text,
        range: None,
        neutral: "\"\"",
        engine_only: false,
        crs: CrsKey::Family("CorrectionName + MaskName"),
        tier: Some(Tier::CarriedOnly),
        purpose: "the mask's IDENTITY, not decoration — when refining an edit that already has \
                  masks, return each existing name byte-exact or your mask edits are discarded",
    },
    Control {
        name: "amount",
        shape: Shape::Number,
        range: Some((0.0, 1.0)),
        neutral: "1",
        engine_only: false,
        crs: CrsKey::Family("CorrectionAmount"),
        tier: Some(Tier::Rendered),
        purpose: "master opacity of this mask's whole adjustment",
    },
    Control {
        name: "inverted",
        shape: Shape::Bool,
        range: None,
        neutral: "false",
        engine_only: false,
        crs: CrsKey::Family("MaskInverted"),
        tier: Some(Tier::Rendered),
        purpose: "invert the mask region",
    },
    Control {
        name: "exposure_ev",
        shape: Shape::Number,
        range: Some((-5.0, 5.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalExposure2012"),
        tier: Some(Tier::Rendered),
        purpose: "local exposure in stops",
    },
    Control {
        name: "contrast",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalContrast2012"),
        tier: Some(Tier::Rendered),
        purpose: "local contrast",
    },
    Control {
        name: "highlights",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalHighlights2012"),
        tier: Some(Tier::Rendered),
        purpose: "local highlights",
    },
    Control {
        name: "shadows",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalShadows2012"),
        tier: Some(Tier::Rendered),
        purpose: "local shadows",
    },
    Control {
        name: "whites",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalWhites2012"),
        tier: Some(Tier::Rendered),
        purpose: "local white point",
    },
    Control {
        name: "blacks",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalBlacks2012"),
        tier: Some(Tier::Rendered),
        purpose: "local black point",
    },
    Control {
        name: "clarity",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalClarity2012"),
        tier: Some(Tier::Rendered),
        purpose: "local midtone contrast",
    },
    Control {
        name: "dehaze",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalDehaze"),
        tier: Some(Tier::Rendered),
        purpose: "local haze removal",
    },
    Control {
        name: "texture",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalTexture"),
        tier: Some(Tier::Rendered),
        purpose: "local FINE-detail contrast (smaller radius than clarity) — this control exists \
                  only per mask, there is no global Texture",
    },
    Control {
        name: "sharpness",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalSharpness"),
        tier: Some(Tier::Rendered),
        purpose: "local capture sharpening at the same radius as the global `sharpening` — but on \
                  ACR's own LOCAL scale, where the band is SIGNED: positive sharpens, NEGATIVE \
                  SOFTENS (blurs) inside the mask, which is how a background is thrown back",
    },
    Control {
        name: "saturation",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalSaturation"),
        tier: Some(Tier::Rendered),
        purpose: "local saturation",
    },
    Control {
        name: "hue",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalHue"),
        tier: Some(Tier::Rendered),
        purpose: "local HUE ROTATION of every colour inside the mask (±100 = ±30°, the same scale \
                  as the global mixer's hue axis) — for turning one region's colour without \
                  touching the same colour elsewhere in the frame",
    },
    Control {
        name: "temperature",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalTemperature"),
        tier: Some(Tier::Rendered),
        purpose: "local warm/cool shift — a RELATIVE ±100 shift, NOT Kelvin (the global \
                  temperature_k is the only absolute one)",
    },
    Control {
        name: "tint",
        shape: Shape::Number,
        range: Some((-100.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalTint"),
        tier: Some(Tier::Rendered),
        purpose: "local green/magenta shift",
    },
    Control {
        name: "noise_reduction",
        shape: Shape::Number,
        range: Some((0.0, 100.0)),
        neutral: "0",
        engine_only: false,
        crs: CrsKey::Family("LocalLuminanceNoise"),
        tier: Some(Tier::Rendered),
        purpose: "local luminance noise reduction — for a \"this region is noisy\" request",
    },
    Control {
        name: "color_gains",
        shape: Shape::EngineCarrier,
        range: None,
        neutral: "null = neutral",
        engine_only: true,
        crs: CrsKey::None,
        tier: Some(Tier::RenderedNotExported),
        purpose: "per-channel LINEAR-light gains for zone recolouring, written by the reverse-fit \
                  (engine-only: no classic-ACR counterpart exists)",
    },
    Control {
        name: "role",
        shape: Shape::EngineCarrier,
        range: None,
        neutral: "custom",
        engine_only: true,
        crs: CrsKey::None,
        tier: None,
        purpose: "which semantic zone produced this mask (engine-only stable identity)",
    },
];

// ── the nested object shapes ────────────────────────────────────────────────
// These spell out types the recipe composes (Hsl, ColorGrade, Crop,
// CurvePoint, MaskGeometry, RangeMask) rather than fields of the recipe
// itself, so they stay shape definitions instead of registry rows. Their
// field sets are pinned to serde in this module's tests, and
// `color_grade_value` / `hsl_value` give the two field-bearing ones the same
// exhaustive-destructure treatment the top-level structs get.

/// MaskGeometry tagged enum (`#[serde(tag="kind")]`) → anyOf of the two
/// variants the AI may author; each is strict (all props required,
/// `additionalProperties:false`). `Bitmap` is deliberately ABSENT: a raster
/// mask is a file the engine writes (AI segmentation, painting), never
/// something a text response can name.
///
/// `Radial::angle` IS here since R23-1b. Its absence used to be justified by
/// the XMP omission ("our own field, unverified against `crs:Angle`"), but the
/// two questions are separate: the sidecar omits the rotation because mapping
/// it onto `crs:Angle` would rotate Lightroom's masks on a guess (and
/// `xmp::MaskLossReason::Rotation` discloses that projection loss), while THIS
/// schema is the engine's own contract — `render::mask_weight` rotates the
/// ellipse by exactly this field, the GUI edits it, and `recipe.json`
/// round-trips it. Withholding it meant an oblique subject could only ever get
/// an axis-aligned ellipse from the model, and every refine of a rotated mask
/// had to be repaired by `carry_over_unrepresentable`.
fn mask_geometry_schema() -> Value {
    let num = || json!({"type": "number"});
    json!({
        "anyOf": [
            {"type": "object", "additionalProperties": false,
             "required": ["kind","zero_x","zero_y","full_x","full_y"],
             "properties": {"kind": {"type": "string", "enum": ["linear"]},
                "zero_x": num(), "zero_y": num(), "full_x": num(), "full_y": num()}},
            {"type": "object", "additionalProperties": false,
             "required": ["kind","top","left","bottom","right","feather","roundness","flipped",
                "angle"],
             "properties": {"kind": {"type": "string", "enum": ["radial"]},
                "top": num(), "left": num(), "bottom": num(), "right": num(),
                "feather": num(), "roundness": num(), "flipped": {"type": "boolean"},
                "angle": num()}}
        ]
    })
}

/// RangeMask tagged enum (`#[serde(tag="kind")]`) → anyOf of the two variants
/// + null (strict mode requires the field to be PRESENT, so "no range" = null).
fn range_mask_schema() -> Value {
    let num = || json!({"type": "number"});
    json!({
        "anyOf": [
            {"type": "object", "additionalProperties": false,
             "required": ["kind","lo_outer","lo","hi","hi_outer"],
             "properties": {"kind": {"type": "string", "enum": ["luminance"]},
                "lo_outer": num(), "lo": num(), "hi": num(), "hi_outer": num()}},
            {"type": "object", "additionalProperties": false,
             "required": ["kind","r","g","b","amount","px","py"],
             "properties": {"kind": {"type": "string", "enum": ["color"]},
                "r": num(), "g": num(), "b": num(), "amount": num(),
                "px": num(), "py": num()}},
            {"type": "null"}
        ]
    })
}

/// HSL: three numeric arrays (red..magenta). Length is pinned at 8 by
/// `recipe::Hsl`'s `[f32;8]` at DESERIALIZE time; OpenAI strict mode cannot
/// pin array length (minItems/maxItems are unsupported and 400 the request),
/// so the proposer prompt enforces "exactly 8, in band order" — and
/// `openai::repair_hsl_axis_lengths` repairs a miscounted axis instead of
/// throwing the whole paid proposal away.
fn hsl_schema() -> Value {
    let hsl_axis = || json!({"type": "array", "items": {"type": "number"}});
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["hue", "saturation", "luminance"],
        "properties": {"hue": hsl_axis(), "saturation": hsl_axis(), "luminance": hsl_axis()}
    })
}

/// Colour grading wheels (flat scalar object), per `recipe::ColorGrade`.
fn color_grade_schema() -> Value {
    let num = || json!({"type": "number"});
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["shadow_hue","shadow_sat","shadow_lum","midtone_hue","midtone_sat","midtone_lum",
            "highlight_hue","highlight_sat","highlight_lum","global_hue","global_sat","global_lum",
            "blending","balance"],
        "properties": {
            "shadow_hue": num(), "shadow_sat": num(), "shadow_lum": num(),
            "midtone_hue": num(), "midtone_sat": num(), "midtone_lum": num(),
            "highlight_hue": num(), "highlight_sat": num(), "highlight_lum": num(),
            "global_hue": num(), "global_sat": num(), "global_lum": num(),
            "blending": num(), "balance": num()
        }
    })
}

fn crop_schema() -> Value {
    let num = || json!({"type": "number"});
    json!({"type": ["object","null"], "additionalProperties": false,
        "required": ["left","top","right","bottom"],
        "properties": {"left": num(), "top": num(), "right": num(), "bottom": num()}})
}

/// An array of `{input,output}` curve points (master + the three RGB
/// channels). The integers are bounded to 0..255 (`recipe::CurvePoint` is u8)
/// so the model can't emit an out-of-range value that fails the whole-recipe
/// deserialize.
fn curve_arr() -> Value {
    let int255 = || json!({"type": "integer", "minimum": 0, "maximum": 255});
    json!({"type": "array", "items": {"type": "object",
        "additionalProperties": false, "required": ["input", "output"],
        "properties": {"input": int255(), "output": int255()}}})
}

/// Build one strict-mode object schema from a registry: every AI-visible row
/// becomes a property AND a `required` entry (OpenAI strict mode requires
/// both), engine-only rows appear nowhere.
fn strict_object(controls: &[Control]) -> Value {
    let mut required = Vec::new();
    let mut properties = serde_json::Map::new();
    for c in controls.iter().filter(|c| !c.engine_only) {
        let prop = c.shape.schema().unwrap_or_else(|| {
            // Unreachable by construction (every EngineCarrier row is
            // engine_only, pinned by `engine_carriers_never_reach_the_schema`)
            // — but a future row that got the pair wrong would otherwise emit
            // a property-less `required` entry, which 400s the request with a
            // schema error instead of naming the mistake.
            panic!("control `{}` is in the schema but has no schema shape", c.name)
        });
        required.push(Value::String(c.name.to_string()));
        properties.insert(c.name.to_string(), prop);
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": Value::Array(required),
        "properties": Value::Object(properties),
    })
}

/// The nested local-adjustment mirror, derived from [`LOCAL_CONTROLS`].
fn local_adjustment_schema() -> Value {
    strict_object(&LOCAL_CONTROLS)
}

// ── control FAMILIES + the thinking envelope (R23-4, feedback #13) ──────────

/// One FAMILY of develop controls — the vocabulary the thinking mode writes
/// its `tool_plan` in.
///
/// NOT one row per field: 33 global controls × `{use, why}` would multiply the
/// response (and its cost) for no gain, while the reported defect is that whole
/// CLASSES of tool go unconsidered ("it never touches hsl or color_grade"),
/// which is a question about families. `members` are [`RECIPE_CONTROLS`] names
/// and the partition is pinned BOTH ways by
/// `every_ai_visible_control_belongs_to_exactly_one_family`, so a control added
/// to the registry cannot quietly fall outside the plan the model is asked to
/// make.
#[derive(Debug, Clone, Copy)]
pub struct Family {
    /// The wire spelling — one value of the `control` enum in the envelope
    /// schema, and the word the prompt uses.
    pub name: &'static str,
    /// What the model is told this family covers.
    pub covers: &'static str,
    /// The registry rows it owns.
    pub members: &'static [&'static str],
}

/// The 10 families the AI-visible global controls partition into.
pub const CONTROL_FAMILIES: [Family; 10] = [
    Family {
        name: "tone",
        covers: "exposure, contrast and the four tonal bands (highlights / shadows / whites / blacks)",
        members: &["exposure_ev", "contrast", "highlights", "shadows", "whites", "blacks"],
    },
    Family {
        name: "white_balance",
        covers: "the absolute Kelvin target and the relative green/magenta tint",
        members: &["temperature_k", "tint"],
    },
    Family {
        name: "presence",
        covers: "vibrance, saturation, clarity and dehaze",
        members: &["vibrance", "saturation", "clarity", "dehaze"],
    },
    Family {
        name: "hsl",
        covers: "the 8-band hue / saturation / luminance colour mixer",
        members: &["hsl"],
    },
    Family {
        name: "color_grade",
        covers: "the shadow / midtone / highlight / global colour-grading wheels",
        members: &["color_grade"],
    },
    Family {
        name: "curves",
        covers: "the master tone curve and the three per-channel RGB curves",
        members: &["tone_curve", "red_curve", "green_curve", "blue_curve"],
    },
    Family {
        name: "detail",
        covers: "capture sharpening and global luminance noise reduction",
        members: &["sharpening", "noise_reduction"],
    },
    Family {
        name: "framing",
        covers: "the crop rectangle and the straighten angle",
        members: &["straighten_deg", "crop"],
    },
    Family {
        name: "lens",
        covers: "the manual optical corrections — vignette falloff and geometric distortion \
                 (physical fixes, not mood; leave them null unless you SEE a defect)",
        members: &["lens_vignette", "lens_vignette_mid", "lens_distortion"],
    },
    Family {
        name: "masks",
        covers: "local (masked) adjustments — dodging, burning, holding a sky back",
        members: &["masks"],
    },
];

/// The AI-visible rows that are BOOKKEEPING rather than develop tools, so they
/// belong to no family. Spelled out (not inferred) so a future non-tool field
/// is a deliberate edit here rather than a silent hole in the partition.
pub const NOT_A_TOOL: [&str; 3] = ["version", "rationale", "confidence"];

/// The controls [`family_is_active`] does NOT count — a non-neutral value that
/// changes no pixel anywhere, so lighting the develop panel's ● for it would
/// promise an edit the preview, the export and Lightroom all render
/// identically. One entry, with the reason, on the [`CARRIED_ONLY_GLOBAL`]
/// pattern: an exemption without a stated reason is indistinguishable from an
/// oversight.
pub const DOT_EXEMPT: &[(&str, &str)] = &[(
    "lens_vignette_mid",
    "the engine builds the manual falloff LUT only when the AMOUNT is non-zero \
     — `(r.lens_vignette != 0.0).then(|| manual_vignette_lut(…))`, render.rs — \
     and the XMP carries VignetteMidpoint under the same zero amount, so a \
     moved Midpoint alone changes no pixel in the preview, the export or \
     Lightroom. Whenever it DOES matter, `lens_vignette != 0.0` has already lit \
     the dot (R22 #16, re-checked R25)",
)];

/// Is any control in this family standing away from its neutral state?
///
/// The develop panel's ● per section — "a collapsed active adjustment is never
/// invisible" — DERIVED from the family table instead of a hand-written field
/// tuple per section. Four of those tuples had already drifted one field short
/// of their section by R22 #16, and every LR-gap batch that adds a control adds
/// another chance to forget one; a family that gains a member now gains the dot
/// with it.
///
/// Neutral means "as [`EditRecipe::default`] has it", read through
/// [`global_value`] so a renamed or retyped field cannot slip past — with the
/// two structured look objects answering for THEMSELVES (see
/// [`value_is_active`]) and [`DOT_EXEMPT`] left out.
pub fn family_is_active(f: &Family, r: &EditRecipe) -> bool {
    let neutral = EditRecipe::default();
    f.members
        .iter()
        .filter(|m| !DOT_EXEMPT.iter().any(|(n, _)| n == *m))
        .any(|m| match (global_value(r, m), global_value(&neutral, m)) {
            (Some(live), Some(base)) => value_is_active(live, base),
            // Unreachable: `every_ai_visible_control_belongs_to_exactly_one_
            // family` proves the members ARE registry rows. A typo'd member
            // must not claim activity it cannot read.
            _ => false,
        })
}

/// "Away from neutral", per shape: plain inequality against the default recipe,
/// except for the two structured look objects, which are asked their OWN
/// question.
///
/// `Hsl::is_neutral` / `ColorGrade::is_neutral` are the gates the renderer and
/// the XMP writer skip on, and the grade's is deliberately NOT inequality: a
/// wheel turned to a hue at zero saturation, or Blending moved on its own,
/// changes no pixel (recipe.rs states it). Comparing the whole object would
/// therefore light a ● over a section that renders identically — the same
/// mistake [`DOT_EXEMPT`] exists to prevent for the lens midpoint.
fn value_is_active(live: GlobalValue<'_>, neutral: GlobalValue<'_>) -> bool {
    match live {
        GlobalValue::Hsl(h) => !h.is_neutral(),
        GlobalValue::Grade(cg) => !cg.is_neutral(),
        other => other != neutral,
    }
}

/// The THINKING ENVELOPE (R23-4, six fields since R23-1b): the model states
/// what it sees, which tool families it will use and why, and the look it is
/// aiming for BEFORE it writes the numbers — then critiques its own answer
/// after, and may point at the PIXEL tools no develop control can reach.
///
/// `recipe` is the SAME [`edit_recipe_schema`] the default path sends, nested
/// verbatim: the thinking fields are not develop controls and must never enter
/// [`RECIPE_CONTROLS`] (they would reach `recipe.json`, the XMP comment,
/// `deny_unknown_fields` downgrades and R21's `recipe_norm` structure
/// fingerprint). One envelope, one paid call, zero new roles.
///
/// ORDER, honestly: `required` lists the fields in thinking order, and the
/// prompt states that order explicitly. Whether OpenAI's strict structured
/// output GENERATES fields in the declared order is UNVERIFIED — it cannot be
/// tested without a real paid call, so the probe lives as an `#[ignore]`d,
/// env-gated test (`openai::tests::think_envelope_field_order_probe`). The
/// envelope's structural benefits (an explicit plan, an auditable self-critique,
/// a per-family use/skip decision) do not depend on generation order; only the
/// "think before you write" strength does. `properties` is alphabetical on the
/// wire regardless — this tree's `serde_json` has no `preserve_order` feature,
/// so a JSON object's keys serialize sorted, exactly as the recipe schema's
/// own properties already do.
pub fn think_envelope_schema() -> Value {
    let families: Vec<Value> =
        CONTROL_FAMILIES.iter().map(|f| Value::String(f.name.to_string())).collect();
    let tools: Vec<Value> = crate::advisor::PixelTool::ALL
        .iter()
        .map(|(n, _)| Value::String((*n).to_string()))
        .collect();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "scene", "tool_plan", "intended_look", "recipe", "self_critique",
            "pixel_tool_suggestions"
        ],
        "properties": {
            "scene": {"type": "string"},
            "tool_plan": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["control", "use", "why"],
                "properties": {
                    "control": {"type": "string", "enum": Value::Array(families)},
                    "use": {"type": "boolean"},
                    "why": {"type": "string"}
                }
            }},
            "intended_look": {"type": "string"},
            "recipe": edit_recipe_schema(),
            "self_critique": {"type": "string"},
            // The PIXEL tools, which no recipe field can reach (R23-1b). Last
            // on purpose: it is an aside about work the develop cannot do, and
            // the four thinking fields around the recipe are the ones whose
            // order carries the "think first" claim. Strict mode has no
            // optional property — every property must be `required` — so the
            // "nothing to suggest" answer is an EMPTY ARRAY, which the prompt
            // states and which is the common case.
            "pixel_tool_suggestions": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["tool", "why"],
                "properties": {
                    "tool": {"type": "string", "enum": Value::Array(tools)},
                    "why": {"type": "string"}
                }
            }}
        }
    })
}

/// The prompt half of the envelope: what each field means, in the order the
/// response must take. Generated from [`CONTROL_FAMILIES`] so the plan's
/// candidate list can never advertise a family the schema's enum rejects.
///
/// The ORDER SENTENCE below is the one line that must be re-read whenever the
/// envelope grows a field — R23-1b added `pixel_tool_suggestions` to the schema
/// and this sentence still listed five, which asks the model for a shape
/// nothing described. `the_think_envelope_nests_the_recipe_schema_unchanged`
/// now parses this very sentence against the schema's `required` array.
pub fn think_prompt() -> String {
    let list: String = CONTROL_FAMILIES
        .iter()
        .map(|f| format!("  • {} — {}\n", f.name, f.covers))
        .collect();
    format!(
        "THINK FIRST, THEN ANSWER. Your reply is an ENVELOPE whose keys must be emitted in \
THIS ORDER: `scene`, `tool_plan`, `intended_look`, `recipe`, `self_critique`, \
`pixel_tool_suggestions`. Write the first \
three BEFORE the numbers — they are your working, not a summary of it.\n\
  1. `scene` — ONE sentence on what this photograph is and what its light is doing.\n\
  2. `tool_plan` — ONE entry for EACH tool family below, in this order, with `use` true or \
false and ONE short clause of `why`. \"this photo does not need it\" is a valid reason; \
\"I did not consider it\" is not. Deciding a family is unnecessary is a real decision — make it \
explicitly rather than by omission.\n\
{list}\
  3. `intended_look` — ONE sentence naming the finished look you are going for.\n\
  4. `recipe` — the EditRecipe itself, which must EXECUTE the plan above: a family you marked \
`use: true` has to show up in the numbers, and one you marked `use: false` stays at its neutral \
value.\n\
  5. `self_critique` — ONE sentence judging your own answer against the TARGET STRENGTH \
above: is this a finished photograph at that strength, or did you play it safe? Name the \
weakest part.\n\
  6. `pixel_tool_suggestions` — AT MOST 3 entries, and `[]` (empty) is the normal answer. \
These are the app's PIXEL tools, which NO develop control can reach: {tools}. Name one only \
when this photograph visibly needs it (a sensor spot or a distracting object → Heal / \
CloneStamp / GenerativeFill; heavy noise a slider cannot fix → Denoise; a selection the \
gradient masks above cannot draw → SelectSubject / SelectSky; a re-imagined frame → \
Reimagine), with ONE clause of `why`. You are ADVISING the photographer, not running these \
tools: nothing here is executed, several cost money, and the develop you return must stand on \
its own without them.\n",
        tools = crate::advisor::PixelTool::ALL
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(" / "),
    )
}

/// JSON Schema for [`EditRecipe`] in OpenAI strict mode: every property listed
/// in `required`, `additionalProperties:false`, optionals expressed as
/// nullable — DERIVED from [`RECIPE_CONTROLS`] (R23-1), so a new recipe field
/// can no longer be invisible to the AI.
pub fn edit_recipe_schema() -> Value {
    strict_object(&RECIPE_CONTROLS)
}

/// The proposer prompt's control list: name, band, neutral value and the one
/// line of purpose, for every control the model may set. Generated, so the
/// prompt can never advertise a control the schema rejects (or omit one it
/// accepts) — the prompt's own blind spot before R23-1 (it never mentioned
/// white balance, dehaze, sharpening, crop or straighten at all).
pub fn prompt_catalogue() -> String {
    let render = |controls: &[Control]| -> String {
        controls
            .iter()
            .filter(|c| !c.engine_only)
            .map(|c| format!("  • {} ({}) — {}\n", c.name, c.spec(), c.purpose))
            .collect::<String>()
    };
    format!(
        "CONTROL CATALOGUE — the COMPLETE set of controls you may return, each with the range it \
accepts (ranges are SAFETY bounds, not targets), its neutral value, and what it is for. Leave a \
control at its neutral value when the photograph does not call for it; anything not listed here \
you cannot set.\nGLOBAL:\n{}PER MASK (one object per entry of `masks`; the slider scales match \
the globals):\n{}",
        render(&RECIPE_CONTROLS),
        render(&LOCAL_CONTROLS),
    )
}

// ── typed value access (the compiler-enforced coverage) ─────────────────────

/// One control's value read off a recipe — the shape-preserving view the eval
/// ruler and the tests read through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GlobalValue<'a> {
    Num(f32),
    OptNum(Option<f32>),
    Int(u32),
    Text(&'a str),
    Curve(&'a [CurvePoint]),
    Hsl(&'a Hsl),
    Grade(&'a ColorGrade),
    Crop(Option<&'a Crop>),
    Masks(&'a [LocalAdjustment]),
    Knots(&'a [[f32; 2]]),
    Lens(&'a LensProfile),
}

impl GlobalValue<'_> {
    /// The value as a single number, for the scalar consumers (`None` for the
    /// structured shapes and for an unset nullable).
    pub fn scalar(&self) -> Option<f32> {
        match self {
            GlobalValue::Num(v) => Some(*v),
            GlobalValue::OptNum(v) => *v,
            _ => None,
        }
    }
}

/// The value of the named [`RECIPE_CONTROLS`] row in `r`, or `None` for a name
/// that is not a control.
///
/// **This is one of the two compiler tripwires this module exists for.** The
/// `let EditRecipe { .. }` below has NO rest pattern, so a new recipe field
/// fails to compile here; and because every binding is consumed by a match
/// arm, a field that IS destructured but never mapped fails as an
/// `unused_variables` deny. Either way the build stops until the registry
/// gains the row.
pub fn global_value<'a>(r: &'a EditRecipe, name: &str) -> Option<GlobalValue<'a>> {
    let EditRecipe {
        version,
        coord_era,
        exposure_ev,
        contrast,
        highlights,
        shadows,
        whites,
        blacks,
        temperature_k,
        tint,
        as_shot_k,
        as_shot_tint,
        vibrance,
        saturation,
        clarity,
        dehaze,
        hsl,
        color_grade,
        sharpening,
        noise_reduction,
        lens_vignette,
        lens_vignette_mid,
        lens_distortion,
        straighten_deg,
        crop,
        tone_curve,
        red_curve,
        green_curve,
        blue_curve,
        base_curve,
        lens_profile,
        masks,
        rationale,
        confidence,
    } = r;
    Some(match name {
        "version" => GlobalValue::Int(*version),
        "coord_era" => GlobalValue::Int(*coord_era),
        "exposure_ev" => GlobalValue::Num(*exposure_ev),
        "contrast" => GlobalValue::Num(*contrast),
        "highlights" => GlobalValue::Num(*highlights),
        "shadows" => GlobalValue::Num(*shadows),
        "whites" => GlobalValue::Num(*whites),
        "blacks" => GlobalValue::Num(*blacks),
        "temperature_k" => GlobalValue::OptNum(*temperature_k),
        "tint" => GlobalValue::Num(*tint),
        "as_shot_k" => GlobalValue::OptNum(*as_shot_k),
        "as_shot_tint" => GlobalValue::OptNum(*as_shot_tint),
        "vibrance" => GlobalValue::Num(*vibrance),
        "saturation" => GlobalValue::Num(*saturation),
        "clarity" => GlobalValue::Num(*clarity),
        "dehaze" => GlobalValue::Num(*dehaze),
        "hsl" => GlobalValue::Hsl(hsl),
        "color_grade" => GlobalValue::Grade(color_grade),
        "sharpening" => GlobalValue::Num(*sharpening),
        "noise_reduction" => GlobalValue::Num(*noise_reduction),
        "lens_vignette" => GlobalValue::Num(*lens_vignette),
        "lens_vignette_mid" => GlobalValue::Num(*lens_vignette_mid),
        "lens_distortion" => GlobalValue::Num(*lens_distortion),
        "straighten_deg" => GlobalValue::Num(*straighten_deg),
        "crop" => GlobalValue::Crop(crop.as_ref()),
        "tone_curve" => GlobalValue::Curve(tone_curve),
        "red_curve" => GlobalValue::Curve(red_curve),
        "green_curve" => GlobalValue::Curve(green_curve),
        "blue_curve" => GlobalValue::Curve(blue_curve),
        "base_curve" => GlobalValue::Knots(base_curve),
        "lens_profile" => GlobalValue::Lens(lens_profile),
        "masks" => GlobalValue::Masks(masks),
        "rationale" => GlobalValue::Text(rationale),
        "confidence" => GlobalValue::Num(*confidence),
        _ => return None,
    })
}

/// The local-adjustment counterpart of [`GlobalValue`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LocalValue<'a> {
    Num(f32),
    OptGains(Option<[f32; 3]>),
    Bool(bool),
    Text(&'a str),
    Geometry(&'a crate::recipe::MaskGeometry),
    Components(&'a [crate::recipe::MaskComponent]),
    Range(Option<&'a crate::recipe::RangeMask>),
    Role(crate::recipe::MaskRole),
}

impl LocalValue<'_> {
    pub fn scalar(&self) -> Option<f32> {
        match self {
            LocalValue::Num(v) => Some(*v),
            _ => None,
        }
    }
}

/// The value of the named [`LOCAL_CONTROLS`] row in `m` — the second compiler
/// tripwire (see [`global_value`]); `LocalAdjustment` is destructured with no
/// rest pattern and every binding consumed.
pub fn local_value<'a>(m: &'a LocalAdjustment, name: &str) -> Option<LocalValue<'a>> {
    let LocalAdjustment {
        mask,
        components,
        enabled,
        range,
        name: label,
        amount,
        inverted,
        exposure_ev,
        contrast,
        highlights,
        shadows,
        whites,
        blacks,
        clarity,
        dehaze,
        texture,
        sharpness,
        saturation,
        hue,
        temperature,
        tint,
        noise_reduction,
        color_gains,
        role,
    } = m;
    Some(match name {
        "mask" => LocalValue::Geometry(mask),
        "components" => LocalValue::Components(components),
        "enabled" => LocalValue::Bool(*enabled),
        "range" => LocalValue::Range(range.as_ref()),
        "name" => LocalValue::Text(label),
        "amount" => LocalValue::Num(*amount),
        "inverted" => LocalValue::Bool(*inverted),
        "exposure_ev" => LocalValue::Num(*exposure_ev),
        "contrast" => LocalValue::Num(*contrast),
        "highlights" => LocalValue::Num(*highlights),
        "shadows" => LocalValue::Num(*shadows),
        "whites" => LocalValue::Num(*whites),
        "blacks" => LocalValue::Num(*blacks),
        "clarity" => LocalValue::Num(*clarity),
        "dehaze" => LocalValue::Num(*dehaze),
        "texture" => LocalValue::Num(*texture),
        "sharpness" => LocalValue::Num(*sharpness),
        "saturation" => LocalValue::Num(*saturation),
        "hue" => LocalValue::Num(*hue),
        "temperature" => LocalValue::Num(*temperature),
        "tint" => LocalValue::Num(*tint),
        "noise_reduction" => LocalValue::Num(*noise_reduction),
        "color_gains" => LocalValue::OptGains(*color_gains),
        "role" => LocalValue::Role(*role),
        _ => return None,
    })
}

// ── family expansions (the per-band / per-wheel rulers) ─────────────────────

/// The three HSL axes: the recipe's array name, and the `crs` attribute stem
/// each band spelling is built on (`SaturationAdjustmentBlue`, …).
pub const HSL_AXES: [(&str, &str); 3] = [
    ("hue", "HueAdjustment"),
    ("saturation", "SaturationAdjustment"),
    ("luminance", "LuminanceAdjustment"),
];

/// One band of one HSL axis, as a measurable field.
#[derive(Debug, Clone)]
pub struct HslField {
    /// Report/metric name, e.g. `hsl.saturation.blue`.
    pub metric: String,
    /// The ACR attribute, e.g. `SaturationAdjustmentBlue`.
    pub crs: String,
    /// Index into [`HSL_AXES`].
    pub axis: usize,
    /// Index into `recipe::HSL_BANDS`.
    pub band: usize,
}

/// The 24 attributes the 8-band mixer occupies — the expansion the registry's
/// `CrsKey::Family` stands for. Pinned against `xmp::owned_attr_keys` (the
/// writer's own universe) by this module's tests, so the ruler can only ever
/// read keys we also write.
pub fn hsl_expansion() -> Vec<HslField> {
    let mut out = Vec::with_capacity(24);
    for (axis, (axis_name, stem)) in HSL_AXES.iter().enumerate() {
        for (band, band_name) in HSL_BANDS.iter().enumerate() {
            out.push(HslField {
                metric: format!("hsl.{axis_name}.{}", band_name.to_lowercase()),
                crs: format!("{stem}{band_name}"),
                axis,
                band,
            });
        }
    }
    out
}

/// One HSL cell by (axis, band) index — the exhaustive-destructure tripwire
/// for `recipe::Hsl` (a fourth axis stops the build here).
pub fn hsl_value(h: &Hsl, axis: usize, band: usize) -> Option<f32> {
    let Hsl { hue, saturation, luminance } = h;
    let arr = match axis {
        0 => hue,
        1 => saturation,
        2 => luminance,
        _ => return None,
    };
    arr.get(band).copied()
}

/// The 14 colour-grade fields paired with their ACR attributes. ACR's own
/// convention (verified against the user's sidecars): shadow/highlight hue+sat
/// round-trip through the legacy `SplitToning*` keys, everything else through
/// `ColorGrade*`.
pub const COLOR_GRADE_CRS: [(&str, &str); 14] = [
    ("shadow_hue", "SplitToningShadowHue"),
    ("shadow_sat", "SplitToningShadowSaturation"),
    ("shadow_lum", "ColorGradeShadowLum"),
    ("midtone_hue", "ColorGradeMidtoneHue"),
    ("midtone_sat", "ColorGradeMidtoneSat"),
    ("midtone_lum", "ColorGradeMidtoneLum"),
    ("highlight_hue", "SplitToningHighlightHue"),
    ("highlight_sat", "SplitToningHighlightSaturation"),
    ("highlight_lum", "ColorGradeHighlightLum"),
    ("global_hue", "ColorGradeGlobalHue"),
    ("global_sat", "ColorGradeGlobalSat"),
    ("global_lum", "ColorGradeGlobalLum"),
    ("blending", "ColorGradeBlending"),
    ("balance", "SplitToningBalance"),
];

/// One colour-grade wheel field by name — the exhaustive-destructure tripwire
/// for `recipe::ColorGrade` (a fifth wheel stops the build here).
pub fn color_grade_value(cg: &ColorGrade, field: &str) -> Option<f32> {
    let ColorGrade {
        shadow_hue,
        shadow_sat,
        shadow_lum,
        midtone_hue,
        midtone_sat,
        midtone_lum,
        highlight_hue,
        highlight_sat,
        highlight_lum,
        global_hue,
        global_sat,
        global_lum,
        blending,
        balance,
    } = cg;
    Some(match field {
        "shadow_hue" => *shadow_hue,
        "shadow_sat" => *shadow_sat,
        "shadow_lum" => *shadow_lum,
        "midtone_hue" => *midtone_hue,
        "midtone_sat" => *midtone_sat,
        "midtone_lum" => *midtone_lum,
        "highlight_hue" => *highlight_hue,
        "highlight_sat" => *highlight_sat,
        "highlight_lum" => *highlight_lum,
        "global_hue" => *global_hue,
        "global_sat" => *global_sat,
        "global_lum" => *global_lum,
        "blending" => *blending,
        "balance" => *balance,
        _ => return None,
    })
}

/// The registry row for a global control name.
pub fn global_control(name: &str) -> Option<&'static Control> {
    RECIPE_CONTROLS.iter().find(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Serialised top-level keys of a value, as a set.
    fn serde_keys(v: &Value) -> BTreeSet<String> {
        v.as_object().expect("struct serialises to an object").keys().cloned().collect()
    }

    /// The `required` array of a strict-mode object schema, as a set.
    fn required_keys(schema: &Value) -> BTreeSet<String> {
        schema["required"]
            .as_array()
            .expect("strict schema lists every property in `required`")
            .iter()
            .map(|k| k.as_str().expect("required entries are strings").to_string())
            .collect()
    }

    /// Compare two key sets BOTH ways and report the difference itself — a
    /// count check passes on a rename, and "the schema is one short" never
    /// says which field the model can no longer set.
    fn assert_same(what: &str, left: (&str, &BTreeSet<String>), right: (&str, &BTreeSet<String>)) {
        let missing: Vec<&str> = left.1.difference(right.1).map(String::as_str).collect();
        let extra: Vec<&str> = right.1.difference(left.1).map(String::as_str).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{what}: in {} but not in {}: {missing:?}; in {} but not in {}: {extra:?}",
            left.0,
            right.0,
            right.0,
            left.0,
        );
    }

    fn names(controls: &[Control], engine_only: Option<bool>) -> BTreeSet<String> {
        controls
            .iter()
            .filter(|c| engine_only.is_none_or(|e| c.engine_only == e))
            .map(|c| c.name.to_string())
            .collect()
    }

    /// R22-0a, widened to the THREE-way form R23-1 needs: the registry, the
    /// serde shape and the strict schema's `required` array must agree, on
    /// both mirrors.
    ///
    ///   registry (all rows)      == serde keys        → no field escapes the table
    ///   registry (AI-visible)    == schema required   → the schema IS the table
    ///
    /// A field added to the recipe without its row is a build error in
    /// `global_value`/`local_value` before it ever reaches this test; this
    /// pins the other two edges, which no pattern can enforce.
    #[test]
    fn registry_serde_and_schema_agree_on_both_mirrors() {
        let schema = edit_recipe_schema();
        let recipe = serde_keys(&serde_json::to_value(EditRecipe::default()).unwrap());
        assert_same(
            "EditRecipe",
            ("the registry", &names(&RECIPE_CONTROLS, None)),
            ("the recipe's serde shape", &recipe),
        );
        assert_same(
            "EditRecipe",
            ("the registry's AI-visible rows", &names(&RECIPE_CONTROLS, Some(false))),
            ("the schema's `required`", &required_keys(&schema)),
        );

        // Reach the local-adjustment mirror THROUGH the top-level schema: an
        // orphaned second copy would pass a test that rebuilt it directly.
        let local_schema = &schema["properties"]["masks"]["items"];
        let local = serde_keys(&serde_json::to_value(LocalAdjustment::default()).unwrap());
        assert_same(
            "LocalAdjustment",
            ("the registry", &names(&LOCAL_CONTROLS, None)),
            ("the adjustment's serde shape", &local),
        );
        assert_same(
            "LocalAdjustment",
            ("the registry's AI-visible rows", &names(&LOCAL_CONTROLS, Some(false))),
            ("the nested schema's `required`", &required_keys(local_schema)),
        );

        // Order, not just membership: `required` follows declaration order, so
        // the generated schema stays byte-identical to the hand-written mirror
        // it replaced (a reordering is harmless to the API but would make the
        // R23-1 diff impossible to verify).
        let ai_order: Vec<&str> = RECIPE_CONTROLS
            .iter()
            .filter(|c| !c.engine_only)
            .map(|c| c.name)
            .collect();
        let schema_order: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(ai_order, schema_order, "required order follows the registry order");
    }

    /// R24-5 M0 — the tier a row CLAIMS must match the export shape the XMP
    /// writer actually gives it.
    ///
    /// [`Control::crs`] is already pinned against `xmp::owned_attr_keys` by
    /// `every_attr_the_registry_names_is_one_the_writer_writes`, so anchoring
    /// the tier's export half to `crs` anchors it to the writer itself: a row
    /// cannot claim `Rendered` while owning no sidecar property, and it cannot
    /// claim `RenderedNotExported` while the writer emits a key for it. Only
    /// the RENDER half is a free declaration — and the locals' half of that is
    /// re-derived from the engine below.
    #[test]
    fn every_tier_matches_the_sidecar_shape_the_writer_gives_it() {
        for (label, rows, allow) in [
            ("EditRecipe", RECIPE_CONTROLS.as_slice(), CARRIED_ONLY_GLOBAL),
            ("LocalAdjustment", LOCAL_CONTROLS.as_slice(), CARRIED_ONLY_LOCAL),
        ] {
            for c in rows {
                match c.tier {
                    Some(t) => assert_eq!(
                        t.owns_crs_key(),
                        c.crs.is_owned(),
                        "{label}.{}: tier {t:?} and crs {:?} disagree about whether it \
                         has a sidecar property of its own",
                        c.name,
                        c.crs
                    ),
                    // The escape hatch is for rows that are not controls
                    // (the era stamp, the AI's provenance, the solver's mask
                    // role). Anything the sidecar carries IS a control and
                    // owes a tier — so `None` may never own a key.
                    None => assert!(
                        !c.crs.is_owned(),
                        "{label}.{} has a sidecar property but no tier — a carried \
                         control cannot opt out of being classified",
                        c.name
                    ),
                }
                // A CarriedOnly control renders nothing, so the only thing
                // stopping it from being a slider that does nothing is this
                // list. Membership is the agreement, spelled with its reason.
                if c.tier == Some(Tier::CarriedOnly) {
                    assert!(
                        allow.iter().any(|(n, _)| *n == c.name),
                        "{label}.{} is CarriedOnly but not on the allow-list — add it \
                         with the reason it renders nothing, or give it a real tier",
                        c.name
                    );
                }
            }
            // …and the allow-list may not name a control that is not (or no
            // longer) CarriedOnly: a stale entry silently blesses whatever
            // takes that name next.
            for (n, why) in allow {
                let c = rows
                    .iter()
                    .find(|c| c.name == *n)
                    .unwrap_or_else(|| panic!("{label}: allow-list names unknown control {n}"));
                assert_eq!(c.tier, Some(Tier::CarriedOnly), "{label}.{n} is not CarriedOnly");
                assert!(!why.trim().is_empty(), "{label}.{n} has no stated reason");
            }
        }
        // PassThrough is declared and deliberately unpopulated: the LR-only
        // property blocks it describes (Transform, Calibration, the camera
        // Look) are preserved by the XMP merge WITHOUT being recipe fields.
        // The B4 batch is what gives this tier its first member; until then
        // an accidental one would be a control nobody renders and nobody
        // interprets.
        assert!(
            RECIPE_CONTROLS
                .iter()
                .chain(LOCAL_CONTROLS.iter())
                .all(|c| c.tier != Some(Tier::PassThrough)),
            "PassThrough gained a member — that is a B4 decision, not a refactor"
        );
    }

    /// The LOCAL tiers' RENDER half, re-derived from the engine rather than
    /// declared.
    ///
    /// The renderer's own answer to "does this mask touch a pixel" is the gate
    /// at `render.rs`'s mask loop — `enabled && amount != 0 &&
    /// engine_active(m)` — whose middle and inner terms mirror every `!= 0.0`
    /// check inside `apply_masks`. So a control renders iff that gate's answer
    /// DEPENDS on it, probed two ways because the gate is two-layered:
    ///
    ///   * one-hot: neutral everywhere but this control ⇒ the mask wakes up.
    ///     Catches every slider `engine_active` itself weighs.
    ///   * zeroing: an already-active mask with this control zeroed ⇒ the mask
    ///     goes back to sleep. Catches the MULTIPLIERS, which change nothing on
    ///     their own — `amount` is the case, and one-hot alone called it inert.
    ///
    /// For the SCALARS both directions are asserted: a slider the gate cannot
    /// notice at all is a slider that does nothing, which is precisely what
    /// the inclusion law below exists to forbid. Everything goes through
    /// serde, so the probe cannot drift from the field names the registry is
    /// keyed by.
    ///
    /// WHAT THIS TEST COVERS, exactly (R24 round-end LOW-3 — the earlier
    /// `is_scalar` filter left three of the four local `RenderedNotExported`
    /// rows unprobed and their render half a free declaration):
    ///
    ///   * `Shape::Number` / `NullableNumber` — probed both ways, above.
    ///   * `Shape::Bool` — probed ONE way. `enabled` is a term of the gate
    ///     itself, so flipping it is decisive; `inverted` moves WHERE the
    ///     adjustment lands and never whether the mask is awake, so the gate
    ///     is structurally blind to it and its silence proves nothing. Hence:
    ///     a flag the gate DOES weigh must be claimed as rendering, and one it
    ///     cannot see is left to its own test (`inverted`:
    ///     `render::tests::mask_coverage_reports_the_engine_weight`, which
    ///     asserts the coverage map flips end for end).
    ///   * `Shape::EngineCarrier` (`components`, `color_gains`) — NOT probed
    ///     here: neither is a JSON scalar the one-hot/zeroing probe can write,
    ///     and `components` needs a raster on disk to compose. Their render
    ///     half is backed by their own tests —
    ///     `render::tests::mask_components_compose_add_subtract_intersect` and
    ///     `render::tests::a_missing_component_raster_makes_the_whole_adjustment_inert`
    ///     for `components`; for `color_gains`,
    ///     `fit_zoned::tests::zone_dials_turn_a_pale_sky_golden_through_the_engine`,
    ///     which renders a recipe whose ONLY colour term is `color_gains`
    ///     through `develop_preview` and asserts the masked pixels move (there
    ///     is no test named for `color_gains` inside `render.rs` itself).
    #[test]
    fn local_tiers_agree_with_the_engines_own_activity_gate() {
        // The render-side gate, spelled once (render.rs's mask loop).
        fn gate(m: &LocalAdjustment) -> bool {
            m.enabled && m.amount != 0.0 && crate::render::engine_active(m)
        }
        // An active baseline held up by TWO independent terms, so zeroing
        // either one leaves the mask awake and only a real multiplier can put
        // it back to sleep.
        let baseline = LocalAdjustment {
            enabled: true,
            amount: 1.0,
            exposure_ev: 1.0,
            contrast: 1.0,
            ..Default::default()
        };
        assert!(gate(&baseline), "premise: the baseline mask is active");
        assert!(!gate(&LocalAdjustment::default()), "premise: a default mask is not");

        for c in LOCAL_CONTROLS.iter() {
            // The two probe values for this shape: what "set" and "neutral"
            // mean as JSON. Anything else is out of this probe's reach and
            // says so in the doc above.
            let (set, neutral) = match c.shape {
                Shape::Number | Shape::NullableNumber => (json!(1.0), json!(0.0)),
                Shape::Bool => (json!(true), json!(false)),
                _ => continue,
            };
            let with = |m: &LocalAdjustment, v: &serde_json::Value| -> LocalAdjustment {
                let mut j = serde_json::to_value(m).expect("LocalAdjustment serializes");
                j[c.name] = v.clone();
                serde_json::from_value(j).unwrap_or_else(|e| {
                    panic!("{} does not accept its own {:?} probe value in serde: {e}", c.name, c.shape)
                })
            };
            let wakes_it = gate(&with(&LocalAdjustment::default(), &set));
            let sleeps_without_it = !gate(&with(&baseline, &neutral));
            let renders = wakes_it || sleeps_without_it;
            let claimed = c.tier.is_some_and(Tier::renders);
            if c.shape == Shape::Bool {
                // ONE-WAY (see the doc): the gate weighs `enabled` and cannot
                // see `inverted`, so a flag it DOES notice must be claimed as
                // rendering, while its silence is not evidence of inertness.
                assert!(
                    !renders || claimed,
                    "LocalAdjustment.{}: the engine's gate answers differently when this flag \
                     flips, but the registry does not claim it renders",
                    c.name
                );
                continue;
            }
            assert_eq!(
                claimed, renders,
                "LocalAdjustment.{}: the registry says renders={claimed}, but the engine's \
                 own gate says {renders} (one-hot wakes it: {wakes_it}; zeroing it puts an \
                 active mask to sleep: {sleeps_without_it})",
                c.name
            );
        }
        // The Bool arm above must really have PROBED `enabled` — an empty or
        // shape-filtered loop would make it vacuous, which is exactly the
        // defect being fixed. `enabled` is the one flag the gate reads.
        assert!(
            !gate(&LocalAdjustment { enabled: false, ..baseline.clone() }),
            "premise for the Bool arm: muting an active mask must put it to sleep"
        );
        assert!(
            LOCAL_CONTROLS.iter().any(|c| c.name == "enabled" && c.shape == Shape::Bool),
            "`enabled` must stay a Bool row, or the Bool arm probes nothing"
        );
    }

    /// **Inclusion law, AI half** (R24-5 M0): a control the model is ASKED for
    /// must be one the engine renders — or an agreed CarriedOnly.
    ///
    /// Otherwise the schema spends a paid request soliciting a number that
    /// changes no pixel, the eval ruler scores it, and the rationale explains
    /// it — the exact failure the whole registry exists to make visible. Both
    /// mirrors, derived from the registry on both sides (no transcribed list).
    #[test]
    fn every_control_the_ai_may_set_is_one_the_engine_renders() {
        for (label, rows, allow) in [
            ("EditRecipe", RECIPE_CONTROLS.as_slice(), CARRIED_ONLY_GLOBAL),
            ("LocalAdjustment", LOCAL_CONTROLS.as_slice(), CARRIED_ONLY_LOCAL),
        ] {
            for c in rows.iter().filter(|c| !c.engine_only) {
                let Some(t) = c.tier else {
                    // Envelope rows (version / rationale / confidence) are in
                    // the schema by design — they carry the response, not a
                    // develop value. They may not own a sidecar key (asserted
                    // above), which is what keeps this exemption narrow.
                    continue;
                };
                assert!(
                    t.renders() || allow.iter().any(|(n, _)| *n == c.name),
                    "{label}.{}: the schema asks the model for a {t:?} control the engine \
                     renders nothing from",
                    c.name
                );
            }
        }
    }

    /// The registry's stated bands ARE the bands `EditRecipe::clamp` enforces
    /// — re-derived from clamp itself rather than trusted, because these are
    /// the numbers the prompt catalogue tells the model, and a range the
    /// engine does not honour is a lie in a paid request.
    #[test]
    fn stated_ranges_are_the_ranges_clamp_enforces() {
        for c in RECIPE_CONTROLS.iter().filter(|c| c.shape.is_scalar()) {
            let (lo, hi) = c.range.unwrap_or_else(|| panic!("{} has no stated range", c.name));
            // A full span OUTSIDE each end (not a multiple of the bound: for a
            // 2000..40000 band, `lo * 10` lands INSIDE the range).
            let span = hi - lo;
            for (probe, want) in [(hi + span + 1.0, hi), (lo - span - 1.0, lo)] {
                let json = format!("{{\"{}\": {probe}}}", c.name);
                let mut r: EditRecipe = serde_json::from_str(&json)
                    .unwrap_or_else(|e| panic!("{} probe {probe} did not deserialize: {e}", c.name));
                r.clamp();
                let got = global_value(&r, c.name).and_then(|v| v.scalar());
                assert_eq!(
                    got,
                    Some(want),
                    "{}: the registry states {lo}..{hi}, but clamp() maps {probe} to {got:?}",
                    c.name
                );
            }
        }
        // The local mirror, through a one-mask recipe (clamp walks masks).
        for c in LOCAL_CONTROLS.iter().filter(|c| c.shape.is_scalar()) {
            let (lo, hi) = c.range.unwrap_or_else(|| panic!("{} has no stated range", c.name));
            // A full span OUTSIDE each end (not a multiple of the bound: for a
            // 2000..40000 band, `lo * 10` lands INSIDE the range).
            let span = hi - lo;
            for (probe, want) in [(hi + span + 1.0, hi), (lo - span - 1.0, lo)] {
                let json = format!("{{\"masks\": [{{\"{}\": {probe}}}]}}", c.name);
                let mut r: EditRecipe = serde_json::from_str(&json)
                    .unwrap_or_else(|e| panic!("{} probe {probe} did not deserialize: {e}", c.name));
                r.clamp();
                let got = r.masks.first().and_then(|m| local_value(m, c.name)).and_then(|v| v.scalar());
                assert_eq!(
                    got,
                    Some(want),
                    "mask {}: the registry states {lo}..{hi}, but clamp() maps {probe} to {got:?}",
                    c.name
                );
            }
        }
    }

    /// Every engine-only row is a row the schema cannot express OR one staged
    /// for a later round — but in BOTH cases it must be absent from the
    /// schema, and every carrier shape must be engine-only (a carrier in the
    /// schema would panic in `strict_object`).
    #[test]
    fn engine_carriers_never_reach_the_schema() {
        for c in RECIPE_CONTROLS.iter().chain(LOCAL_CONTROLS.iter()) {
            if c.shape == Shape::EngineCarrier {
                assert!(c.engine_only, "{} is a carrier shape but not engine_only", c.name);
            }
            if !c.engine_only {
                assert!(c.shape.schema().is_some(), "{} is in the schema with no shape", c.name);
            }
        }
        // The engine-only set, spelled out so flipping one is a deliberate
        // edit with a test to update. R23-1b emptied it of STAGED work: what
        // is left is the engine's own per-photo measurement (the four global
        // rows) and mask state with no ACR spelling. The lens trio left this
        // list when `Shape::NullableNumber` gave it an "I have no opinion"
        // expression and `carry_over_unrepresentable` learned to honour it.
        assert_eq!(
            names(&RECIPE_CONTROLS, Some(true)).into_iter().collect::<Vec<_>>(),
            // `coord_era` (v0.30.0) joined for a reason of its own: it is not
            // a measurement and not staged work but a FRAME declaration the
            // model cannot observe — it only ever sees the display frame.
            vec!["as_shot_k", "as_shot_tint", "base_curve", "coord_era", "lens_profile"]
        );
        assert_eq!(
            names(&LOCAL_CONTROLS, Some(true)).into_iter().collect::<Vec<_>>(),
            vec!["color_gains", "components", "enabled", "role"]
        );
    }

    /// The nested object shapes the registry composes are pinned to serde the
    /// same way the two top-level mirrors are (no pattern can watch them:
    /// they are shape definitions, not registry rows).
    #[test]
    fn nested_object_shapes_match_their_serde_keys() {
        let hsl = serde_keys(&serde_json::to_value(Hsl::default()).unwrap());
        assert_same("Hsl", ("serde", &hsl), ("the schema", &required_keys(&hsl_schema())));
        assert_same(
            "Hsl",
            ("serde", &hsl),
            ("HSL_AXES", &HSL_AXES.iter().map(|(a, _)| a.to_string()).collect()),
        );

        let grade = serde_keys(&serde_json::to_value(ColorGrade::default()).unwrap());
        assert_same(
            "ColorGrade",
            ("serde", &grade),
            ("the schema", &required_keys(&color_grade_schema())),
        );
        assert_same(
            "ColorGrade",
            ("serde", &grade),
            ("COLOR_GRADE_CRS", &COLOR_GRADE_CRS.iter().map(|(f, _)| f.to_string()).collect()),
        );

        let crop = serde_keys(
            &serde_json::to_value(Crop { left: 0.0, top: 0.0, right: 1.0, bottom: 1.0 }).unwrap(),
        );
        assert_same("Crop", ("serde", &crop), ("the schema", &required_keys(&crop_schema())));

        let point = serde_keys(&serde_json::to_value(CurvePoint { input: 0, output: 0 }).unwrap());
        assert_same(
            "CurvePoint",
            ("serde", &point),
            ("the schema", &required_keys(&curve_arr()["items"])),
        );

        // Every colour-grade field resolves through the destructuring reader
        // (a renamed wheel field would break the build there, but a MISSING
        // match arm for a new one is what this catches at test time).
        for (field, _) in COLOR_GRADE_CRS {
            assert!(
                color_grade_value(&ColorGrade::default(), field).is_some(),
                "colour-grade field {field} has no reader"
            );
        }
    }

    /// The ruler may only read keys the writer can write: every attribute the
    /// registry and its family expansions name must be in `xmp`'s own owned-key
    /// universe. Without this, an eval row could measure the user against a
    /// key spelling that exists nowhere in the app (silently scoring 0).
    #[test]
    fn every_crs_attribute_is_one_the_xmp_writer_owns() {
        let owned: BTreeSet<String> = crate::xmp::owned_attr_keys().into_iter().collect();
        for c in RECIPE_CONTROLS.iter() {
            if let Some(k) = c.crs.attr() {
                assert!(owned.contains(k), "{}: crs:{k} is not a key the writer owns", c.name);
            }
        }
        for f in hsl_expansion() {
            assert!(owned.contains(&f.crs), "{}: crs:{} is not owned", f.metric, f.crs);
        }
        for (field, key) in COLOR_GRADE_CRS {
            assert!(owned.contains(key), "color_grade.{field}: crs:{key} is not owned");
        }
        // The expansion covers all 24 cells and each resolves to a value.
        let ex = hsl_expansion();
        assert_eq!(ex.len(), 24);
        for f in &ex {
            assert!(hsl_value(&Hsl::default(), f.axis, f.band).is_some(), "{} unreadable", f.metric);
        }
    }

    /// The generated prompt section names every AI-visible control and no
    /// engine-only one — the blind spot #12 reported (white balance, dehaze,
    /// sharpening, crop and straighten were absent from the prompt entirely).
    #[test]
    fn prompt_catalogue_lists_every_ai_visible_control() {
        let text = prompt_catalogue();
        for c in RECIPE_CONTROLS.iter().chain(LOCAL_CONTROLS.iter()) {
            let bullet = format!("• {} (", c.name);
            if c.engine_only {
                // `name`/`tint`/`saturation`… exist on both mirrors, so only
                // the names unique to the engine-only set can be asserted absent.
                let shared = LOCAL_CONTROLS
                    .iter()
                    .chain(RECIPE_CONTROLS.iter())
                    .any(|o| o.name == c.name && !o.engine_only);
                assert!(
                    shared || !text.contains(&bullet),
                    "{} is engine-only but the prompt advertises it",
                    c.name
                );
            } else {
                assert!(text.contains(&bullet), "{} is missing from the prompt catalogue", c.name);
                assert!(text.contains(c.purpose), "{}'s purpose line is missing", c.name);
            }
        }
        // The two white-balance semantics are a PAIR (#12): absolute Kelvin
        // and a relative tint. Naming only one leaves the model treating the
        // other as absolute too.
        assert!(text.contains("ABSOLUTE colour temperature in Kelvin"), "{text}");
        assert!(text.contains("RELATIVE to the as-shot white balance"), "{text}");
    }

    /// R23-4: the `tool_plan`'s candidate list is a PARTITION of the registry's
    /// AI-visible global rows — every tool belongs to exactly one family, and no
    /// family names a control that does not exist.
    ///
    /// This is the same guarantee `prompt_catalogue` gets from the registry, one
    /// level up: without it a control added in a later round would be in the
    /// schema and in the prompt catalogue while being absent from the plan the
    /// model is asked to make about its own tools — the exact blind spot #13
    /// reports, reintroduced through the back door.
    #[test]
    fn every_ai_visible_control_belongs_to_exactly_one_family() {
        let mut owned: Vec<&str> = Vec::new();
        for f in &CONTROL_FAMILIES {
            for m in f.members {
                assert!(
                    !owned.contains(m),
                    "{m} is claimed by two families — the plan would ask about it twice"
                );
                owned.push(m);
            }
        }
        let owned: BTreeSet<String> = owned.into_iter().map(str::to_string).collect();
        let tools: BTreeSet<String> = names(&RECIPE_CONTROLS, Some(false))
            .into_iter()
            .filter(|n| !NOT_A_TOOL.contains(&n.as_str()))
            .collect();
        assert_same(
            "the tool families",
            ("the registry's AI-visible develop tools", &tools),
            ("the families' members", &owned),
        );
        // The bookkeeping rows really are AI-visible rows (an obsolete
        // exclusion would silently shrink the left side above).
        for n in NOT_A_TOOL {
            assert!(
                global_control(n).is_some_and(|c| !c.engine_only),
                "{n} is excluded from the plan but is not an AI-visible control"
            );
        }
        // The schema's enum IS the family list, and the prompt names each one.
        let schema = think_envelope_schema();
        let enum_values: Vec<&str> = schema["properties"]["tool_plan"]["items"]["properties"]
            ["control"]["enum"]
            .as_array()
            .expect("the control field is an enum")
            .iter()
            .map(|v| v.as_str().expect("enum values are strings"))
            .collect();
        assert_eq!(
            enum_values,
            CONTROL_FAMILIES.iter().map(|f| f.name).collect::<Vec<_>>(),
            "the schema enum must be the family table, in its order"
        );
        let prompt = think_prompt();
        for f in &CONTROL_FAMILIES {
            assert!(prompt.contains(f.name), "{} is missing from the think prompt", f.name);
            assert!(prompt.contains(f.covers), "{}'s coverage line is missing", f.name);
        }
    }

    /// R25 P0-0.3: the ● exemption is one named control with one stated
    /// reason — the same shape [`CARRIED_ONLY_GLOBAL`] uses, and for the same
    /// reason: an exemption that names a control nobody has, or gives no
    /// account of itself, is indistinguishable from a control that was
    /// forgotten.
    #[test]
    fn dot_exempt_names_exist_and_state_a_reason() {
        for (name, why) in DOT_EXEMPT {
            let c = global_control(name)
                .unwrap_or_else(|| panic!("DOT_EXEMPT names unknown control {name}"));
            assert!(!why.trim().is_empty(), "{name} is exempt with no stated reason");
            // Exempting a control no family claims would be an exemption from
            // nothing — `family_is_active` only ever walks family members.
            assert!(
                CONTROL_FAMILIES.iter().any(|f| f.members.contains(&c.name)),
                "{name} belongs to no family, so nothing was exempting it"
            );
            // …and it really is left out of its family's verdict.
            let f = CONTROL_FAMILIES
                .iter()
                .find(|f| f.members.contains(&c.name))
                .expect("checked above");
            let mut moved = EditRecipe::default();
            let neutral = EditRecipe::default();
            match name {
                &"lens_vignette_mid" => moved.lens_vignette_mid = 12.0,
                other => panic!("{other} has no exemption probe — add one with the entry"),
            }
            assert_ne!(
                global_value(&moved, name),
                global_value(&neutral, name),
                "{name}: the probe must actually move the control"
            );
            assert!(!family_is_active(f, &moved), "{name} must not light the {} dot", f.name);
        }
    }

    /// R25 P0 risk gate: [`family_is_active`] REPLACED five hand-written field
    /// tuples in the develop panel, so it has to be the same predicate, not a
    /// better one. The oracle here is those tuples verbatim (GUI HEAD before
    /// R25, `panels/develop.rs:240-252` + `:969-972`), run against 200
    /// pseudo-random recipes that move every field each section reads —
    /// including the `lens_vignette_mid` exemption, which is the one place the
    /// two algorithms COULD differ if the exemption were dropped.
    ///
    /// It stays in the tree deliberately: when a family gains a member (the
    /// B2–B5 batches do exactly that), this test fails and names the section
    /// whose dot just widened — which is a decision worth stating, not drift.
    #[test]
    fn the_family_dots_match_the_hand_written_predicates_they_replaced() {
        // splitmix64, so the sequence is deterministic and the low bits are
        // not the counter's.
        fn next(seed: &mut u64) -> u64 {
            *seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *seed;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let fam = |name: &str| {
            CONTROL_FAMILIES.iter().find(|f| f.name == name).expect("a declared family")
        };
        let (mut lit, mut exempt_probes) = ([0usize; 6], 0usize);
        for i in 0..200 {
            // A sparse draw: mostly neutral, so the interesting cases (exactly
            // one field off zero) actually occur instead of every section
            // being active on every round.
            fn pick(seed: &mut u64) -> f32 {
                let v = next(seed);
                if v.is_multiple_of(3) { ((v >> 8) % 201) as f32 - 100.0 } else { 0.0 }
            }
            let s = &mut s;
            let mut r = EditRecipe {
                clarity: pick(s),
                dehaze: pick(s),
                vibrance: pick(s),
                saturation: pick(s),
                sharpening: pick(s).abs(),
                noise_reduction: pick(s).abs(),
                lens_vignette: pick(s),
                lens_vignette_mid: if next(s).is_multiple_of(2) { 50.0 } else { (next(s) % 101) as f32 },
                lens_distortion: pick(s),
                ..Default::default()
            };
            r.hsl.hue[(next(s) % 8) as usize] = pick(s);
            r.color_grade.shadow_sat = pick(s).abs();
            r.color_grade.shadow_hue = (next(s) % 361) as f32;
            r.color_grade.blending = (next(s) % 101) as f32;
            r.color_grade.global_lum = pick(s);
            if next(s).is_multiple_of(3) {
                r.tone_curve = vec![CurvePoint { input: 0, output: 0 }];
            }
            if next(s).is_multiple_of(5) {
                r.blue_curve = vec![CurvePoint { input: 255, output: 200 }];
            }
            // The predicates as the panel wrote them, verbatim.
            let presence =
                r.clarity != 0.0 || r.dehaze != 0.0 || r.vibrance != 0.0 || r.saturation != 0.0;
            let detail = r.sharpening != 0.0 || r.noise_reduction != 0.0;
            let hsl = !r.hsl.is_neutral();
            let grade = !r.color_grade.is_neutral();
            let curves = !r.tone_curve.is_empty()
                || !r.red_curve.is_empty()
                || !r.green_curve.is_empty()
                || !r.blue_curve.is_empty();
            let lens = r.lens_vignette != 0.0 || r.lens_distortion != 0.0;
            for (k, (label, old, new)) in [
                ("presence", presence, family_is_active(fam("presence"), &r)),
                ("detail", detail, family_is_active(fam("detail"), &r)),
                ("hsl", hsl, family_is_active(fam("hsl"), &r)),
                ("color_grade", grade, family_is_active(fam("color_grade"), &r)),
                ("curves", curves, family_is_active(fam("curves"), &r)),
                ("lens", lens, family_is_active(fam("lens"), &r)),
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(old, new, "recipe #{i}: the {label} dot changed meaning — {r:?}");
                lit[k] += usize::from(new);
            }
            // The midpoint exemption is the one case the two algorithms could
            // differ on, so it has to OCCUR: a moved midpoint with the amount
            // at rest.
            if r.lens_vignette == 0.0 && r.lens_distortion == 0.0 && r.lens_vignette_mid != 50.0 {
                exempt_probes += 1;
                assert!(!lens, "premise: the panel's own predicate ignores the midpoint");
            }
        }
        // A draw that never lit (or never left dark) a section would agree with
        // anything — the equality above would prove nothing about it.
        for (k, label) in
            ["presence", "detail", "hsl", "color_grade", "curves", "lens"].iter().enumerate()
        {
            assert!(lit[k] > 0 && lit[k] < 200, "{label}: the draw was degenerate ({} lit)", lit[k]);
        }
        assert!(exempt_probes > 0, "no recipe moved the midpoint alone — the exemption is untested");
    }

    /// The envelope wraps the recipe schema VERBATIM and adds four thinking
    /// fields — it must not fork a second copy of the recipe contract, and the
    /// thinking fields must not leak into the recipe (they would reach
    /// recipe.json, the XMP comment and R21's structure fingerprint).
    #[test]
    fn the_think_envelope_nests_the_recipe_schema_unchanged() {
        let env = think_envelope_schema();
        assert_eq!(
            env["properties"]["recipe"],
            edit_recipe_schema(),
            "the nested recipe must be the SAME schema the default path sends"
        );
        assert_eq!(
            env["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            vec![
                "scene",
                "tool_plan",
                "intended_look",
                "recipe",
                "self_critique",
                "pixel_tool_suggestions"
            ],
            "the required array is the declared thinking ORDER (see the fn's note on \
             generation order — the probe test measures reality)"
        );
        assert_eq!(env["additionalProperties"], false, "strict mode");
        // R23-1b: the pixel-tool enum IS `PixelTool::ALL`, so a suggestion can
        // only ever name a tool this app has (the parser reads the same list).
        assert_eq!(
            env["properties"]["pixel_tool_suggestions"]["items"]["properties"]["tool"]["enum"]
                .as_array()
                .expect("the tool field is an enum")
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>(),
            crate::advisor::PixelTool::ALL.iter().map(|(n, _)| *n).collect::<Vec<_>>()
        );
        // The thinking fields are NOT registry rows — the recipe's own key set
        // is untouched by the envelope.
        let recipe_keys = required_keys(&edit_recipe_schema());
        for f in [
            "scene",
            "tool_plan",
            "intended_look",
            "self_critique",
            "pixel_tool_suggestions",
        ] {
            assert!(
                !recipe_keys.contains(f),
                "{f} reached the RECIPE schema — it would land in recipe.json and the XMP"
            );
        }
        // The PROMPT's ORDER SENTENCE states the same field list, in the same
        // order. Checked against that SENTENCE, not the whole prompt: every
        // field is also described in its own numbered paragraph below, so a
        // whole-prompt `contains` passes while the sentence that actually tells
        // the model the response SHAPE names one fewer — which is exactly the
        // state R23-1b's sixth field first landed in.
        let prompt = think_prompt();
        let sentence = prompt
            .split_once("THIS ORDER:")
            .and_then(|(_, rest)| rest.split_once('.'))
            .map(|(s, _)| s.to_string())
            .expect("the think prompt opens by naming the envelope's field order");
        let mut at = 0usize;
        for f in env["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()) {
            let needle = format!("`{f}`");
            let found = sentence[at..]
                .find(&needle)
                .unwrap_or_else(|| panic!("the order sentence omits (or misorders) `{f}`: {sentence}"));
            at += found + needle.len();
        }
    }
}
