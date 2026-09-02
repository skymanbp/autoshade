//! Eval harness — how close is the AI's edit to the user's own?
//!
//! For RAWs that have a sibling `.xmp` (the user's ACR/Lightroom develop
//! settings = ground truth), run the AI advisor and compare global slider
//! values. Reports per-field **mean absolute error** (how far off) and **mean
//! signed bias** AI−user (which direction the AI leans). That bias is the
//! tuning signal: e.g. "AI contrast runs +8 hotter than you" → nudge the prompt.
//!
//! XMP is parsed by plain text scan of `crs:Key="value"` (the values are
//! attributes on rdf:Description; verified against the user's real P28.xmp).
//! No exiftool needed.
//!
//! **The ruler widened in R23-1** (feedback #12): the comparison table is now
//! DERIVED from `advisor::catalogue::RECIPE_CONTROLS` instead of being a
//! hand-kept list of 14 sliders, and it covers the 24 HSL cells, the 14
//! colour-grade fields, all four tone curves and the local-mask counts —
//! controls the old table was completely blind to (a `grep -c 'hsl'` over this
//! file used to return 0), which is why nothing could measure whether the AI
//! ever used them. A gap score from before that change is NOT comparable with
//! one from after it: more controls are measured, and a control the user moved
//! while the AI ignored it now counts as the miss it always was.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// `Context` is gone since R27 Batch-7: the one `?` in `run` (an unreadable
// sidecar) cannot propagate out of a worker closure any more, so that failure
// travels back as `PhotoOutcome::Aborted` carrying the SAME message the
// `with_context` produced — `read_sidecar` returns an `Option`, so the context
// string WAS the whole error and nothing is lost by formatting it directly.
use anyhow::Result;

use crate::advisor::catalogue::{self, Shape, COLOR_GRADE_CRS, RECIPE_CONTROLS};
use crate::config::Config;
use crate::pipeline;
use crate::recipe::EditRecipe;

// The `crs:` readers live beside the XMP writer they invert (xmp.rs, where the
// sidecar READER also uses them). Since R28 Batch-5 5d the scope is a TYPE:
// every read in this harness is against a `Scope` — a whole sidecar, narrowed
// to the crs Description's own span — which is what it always meant and now
// states. `style.rs` imports the same two names from `xmp` directly.
pub(crate) use crate::xmp::{CrsSource, Scope};

/// The provenance rule a row's USER value follows.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Rule {
    /// An ordinary slider: present = the photographer set it.
    Plain,
    /// White balance: under `WhiteBalance="As Shot"` the Temperature/Tint
    /// attributes record the CAMERA's values, not a user edit — counting them
    /// charged the AI a false "omission" for correctly leaving WB as-shot.
    Wb,
    /// `crs:CropAngle` is applied by Adobe only under `HasCrop="True"`, so a
    /// stale angle from a disabled crop is not a straighten the user asked for
    /// (the same gate `xmp::xmp_to_recipe` applies on import).
    CropGated,
}

/// Which recipe value a row reads.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// A top-level registry field, by the row's own `metric` name.
    Field,
    /// One cell of the 8-band mixer.
    HslCell { axis: usize, band: usize },
    /// One colour-grade wheel field.
    Grade(&'static str),
}

/// One comparable control: what to call it, which ACR attribute carries the
/// user's own value, and how to read the AI's side. Built by [`rows`] from the
/// control registry, so a control that becomes AI-visible later enters the
/// ruler without a second edit here.
///
/// There is NO unit conversion left on this struct. It carried a per-row
/// `scale` for exactly one control — `sharpening`, on the belief that
/// `crs:Sharpness` was ACR's 0..100 against the recipe's 0..150 — and that
/// belief was disproved in v0.31.1 (real sidecars carry `Sharpness="150"`; see
/// `xmp::xmp_to_recipe`). Every row is now 1:1 by construction, so the field is
/// gone rather than left as a column of 1.0s that invites the next scale guess.
#[derive(Debug, Clone)]
struct Row {
    /// Report label: a registry field name, or `hsl.<axis>.<band>` /
    /// `color_grade.<field>` for a family cell.
    metric: String,
    /// The `crs:` attribute the user's value is read from.
    crs: String,
    rule: Rule,
    source: Source,
    /// True for the four `*_hue` wheels: hue is CIRCULAR, so 350° vs 10° is a
    /// 20° disagreement, not 340°.
    circular: bool,
}

/// Rows below this sample size are visibly marked in the report. The paired
/// same-version analysis found n<20 rows 8.48x less stable per row than
/// n>=20 rows, so this is an annotation threshold, not a score threshold.
const LOW_N_ANNOTATION_THRESHOLD: usize = 20;

impl Row {
    /// Full scale this control's MAE is normalised by for the gap score. Kept
    /// as the pre-R23 normaliser (a "typical" full range, not the clamp band)
    /// so the aggregate stays comparable control-for-control; hue uses its
    /// circular half-turn.
    fn full_scale(&self) -> f64 {
        match self.metric.as_str() {
            _ if self.circular => 180.0,
            "exposure_ev" => 5.0,
            "sharpening" => 150.0,
            "temperature_k" => 2000.0,
            _ => 100.0,
        }
    }

    /// Deadband for "did anyone actually move this?" — exposure is in stops.
    fn eps(&self) -> f32 {
        eps_for(&self.metric)
    }

    /// AI − user, wrapped into ±180 for the circular hue wheels.
    fn diff(&self, ai: f32, user: f32) -> f32 {
        let d = ai - user;
        if self.circular {
            let w = d.rem_euclid(360.0);
            if w > 180.0 { w - 360.0 } else { w }
        } else {
            d
        }
    }
}

/// The ruler's "did anyone actually move this?" deadband, by metric name —
/// lifted out of [`Row::eps`] so a row that has to ask about ANOTHER control's
/// movement ([`hue_carries_colour`]) asks the same question with the same
/// number instead of restating it.
fn eps_for(metric: &str) -> f32 {
    if metric == "exposure_ev" { 0.05 } else { 0.5 }
}

/// Does a colour-grade HUE row describe colour that BOTH sides actually
/// applied? (R28 Batch-5 5b.)
///
/// A toning wheel's hue is an ANGLE, and the wheel paints nothing until its
/// SATURATION leaves zero — `render::apply_color_grade` multiplies the tint by
/// `sat/100`, so at zero saturation every hue from 0° to 359° renders the
/// identical (untinted) pixel. The old row counted a photo whenever EITHER side
/// moved the hue, saturation unasked: on the 147-photo baseline that made
/// `color_grade.shadow_hue` read `mean|Δ| = 141`, a number with no colour
/// anywhere behind it — two colourless wheels parked at opposite ends of a
/// circle. The real finding the artifact was burying is one row down and stands
/// unchanged: `shadow_sat` bias **+9.02**, i.e. the AI tints shadows the
/// photographer leaves neutral.
///
/// So a hue delta is measured only where both sides put colour on the wheel.
/// The threshold is not a new constant: it is the ruler's OWN movement deadband
/// ([`eps_for`], 0.5 on the wheels' 0..100 saturation axis) applied to the
/// companion `*_sat` control — "counted when both sides moved the saturation",
/// in exactly the sense this harness already means by "moved".
///
/// **What this costs, stated:** a photo where the photographer toned and the AI
/// did not no longer reaches the hue row at all, so that row's `AI-omit` count
/// falls. The omission is not lost — it is recorded where it is measurable, on
/// the `*_sat` row, which is the control that carries the decision. And the
/// four `*_hue` rows are NOT comparable across v0.34.0: the pre-R28 M-C
/// baseline's `shadow_hue` numbers count a different population.
///
/// Non-hue rows (every HSL cell included — those are ±100 shift sliders, not
/// wheel angles, and `Row::circular` is false for them) return `true` and are
/// untouched.
fn hue_carries_colour(row: &Row, scope: Scope<'_>, ai: &EditRecipe) -> bool {
    let Some(stem) = row
        .metric
        .strip_prefix("color_grade.")
        .and_then(|f| f.strip_suffix("_hue"))
    else {
        return true;
    };
    let sat_field = format!("{stem}_sat");
    // Derived from the registry, like every other reader here: a new wheel
    // inherits this gate instead of needing a second edit.
    let Some((_, sat_attr)) = COLOR_GRADE_CRS.iter().find(|(f, _)| *f == sat_field) else {
        return true;
    };
    let neutral = catalogue::color_grade_value(&Default::default(), &sat_field).unwrap_or(0.0);
    let eps = eps_for(&format!("color_grade.{sat_field}"));
    let moved = |v: Option<f32>| v.is_some_and(|v| (v - neutral).abs() > eps);
    // The user's side is read the same way `user_value` reads a `Rule::Plain`
    // row (an absent attribute = the neutral = not moved), and the AI's the
    // same way `ai_value` reads a `Source::Grade` row.
    moved(scope.crs_f32(sat_attr))
        && moved(catalogue::color_grade_value(&ai.color_grade, &sat_field))
}

/// The comparison table, derived from the control registry: every AI-visible
/// control that lands on ONE `crs:` attribute becomes a row, and the two family
/// shapes expand into their per-band / per-wheel cells.
///
/// Engine-only rows are excluded on purpose — the AI cannot set them, so
/// scoring it against them would measure a schema gap, not a taste gap. The
/// lens trio entered the schema in R23-1b and therefore entered the ruler in
/// the same edit, exactly as this note promised: three more rows
/// (`VignetteAmount` / `VignetteMidpoint` / `LensManualDistortionAmount`),
/// derived, with no hand-kept list to update.
fn rows() -> Vec<Row> {
    let mut out = Vec::new();
    for c in RECIPE_CONTROLS.iter().filter(|c| !c.engine_only) {
        if let Some(attr) = c.crs.attr() {
            out.push(Row {
                metric: c.name.to_string(),
                crs: attr.to_string(),
                rule: match c.name {
                    "temperature_k" | "tint" => Rule::Wb,
                    "straighten_deg" => Rule::CropGated,
                    _ => Rule::Plain,
                },
                source: Source::Field,
                circular: false,
            });
        }
        match c.shape {
            Shape::Hsl => {
                for f in catalogue::hsl_expansion() {
                    out.push(Row {
                        metric: f.metric,
                        crs: f.crs,
                        rule: Rule::Plain,
                        source: Source::HslCell { axis: f.axis, band: f.band },
                        circular: false,
                    });
                }
            }
            Shape::ColorGrade => {
                for (field, attr) in COLOR_GRADE_CRS {
                    out.push(Row {
                        metric: format!("color_grade.{field}"),
                        crs: attr.to_string(),
                        rule: Rule::Plain,
                        source: Source::Grade(field),
                        circular: field.ends_with("_hue"),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// The AI recipe's value for a row (`None` only for a row whose reader is
/// missing — the registry test makes that unreachable).
fn ai_value(row: &Row, r: &EditRecipe) -> Option<f32> {
    match row.source {
        Source::Field => match row.metric.as_str() {
            // As-shot resolves through the recipe's own stamp, with the
            // engine's historical 5500 K fallback (render.rs uses the same
            // anchor): ACR records an ABSOLUTE Kelvin, so compare absolutes.
            "temperature_k" => Some(r.temperature_k.unwrap_or_else(|| r.as_shot_k.unwrap_or(5500.0))),
            // The engine's tint is a DELTA from the as-shot green balance
            // (render::apply_wb anchors at as_shot); LR's crs:Tint is absolute.
            "tint" => Some(r.as_shot_tint.unwrap_or(0.0) + r.tint),
            f => catalogue::global_value(r, f).and_then(|v| v.scalar()),
        },
        Source::HslCell { axis, band } => catalogue::hsl_value(&r.hsl, axis, band),
        Source::Grade(f) => catalogue::color_grade_value(&r.color_grade, f),
    }
}

/// The row's NEUTRAL value — READ from the defaults, never assumed to be 0:
/// `color_grade.blending` is neutral at 50, and the WB pair's neutral is the
/// photo's own as-shot anchor. (A hard-coded 0 counted every untouched
/// blending as a ±50 disagreement.)
fn neutral(row: &Row, r: &EditRecipe) -> f32 {
    match row.metric.as_str() {
        "temperature_k" => r.as_shot_k.unwrap_or(5500.0),
        "tint" => r.as_shot_tint.unwrap_or(0.0),
        _ => ai_value(row, &EditRecipe::default()).unwrap_or(0.0),
    }
}

/// The user's value for one row, or `None` when this sidecar does not state it.
/// Read STRAIGHT: every `crs:` attribute on the ruler carries the same number
/// the recipe field does (see [`Row`] for the one exception that used to exist).
fn user_value(row: &Row, scope: Scope<'_>, user_wb: bool, has_crop: bool) -> Option<f32> {
    match row.rule {
        Rule::Wb if !user_wb => None,
        Rule::CropGated if !has_crop => None,
        _ => scope.crs_f32(&row.crs),
    }
}

/// Parse an ACR tone-curve `<rdf:Seq>` of `<rdf:li>x, y</rdf:li>` points (each
/// 0..255 input,output) for the given crs tag (e.g. "ToneCurvePV2012"). Empty
/// vec if the tag is absent. The master tone curve is the single biggest "look"
/// control that the flat-slider comparison above was completely blind to.
fn parse_tone_curve(xmp: &str, tag: &str) -> Vec<(f32, f32)> {
    // The same name-matched finder the importer uses (xmp::owned_element_body):
    // the literal `<crs:Tag>` scan this replaced could not see the
    // attribute-carrying spelling, so the judge silently understated the gap
    // score on documents the importer now reads.
    let Ok(Some(body)) = crate::xmp::owned_element_body(xmp, &format!("crs:{tag}")) else {
        return Vec::new();
    };
    let mut pts = Vec::new();
    for chunk in body.split("<rdf:li>").skip(1) {
        let Some(end) = chunk.find("</rdf:li>") else { continue };
        let mut it = chunk[..end].split(',');
        if let (Some(xs), Some(ys)) = (it.next(), it.next())
            && let (Ok(x), Ok(y)) = (xs.trim().parse::<f32>(), ys.trim().parse::<f32>())
            // Same domain xmp::parse_curve enforces by clamping to u8: a
            // NaN/inf/out-of-range point from a foreign sidecar would poison
            // the LUT and print a NaN gap score.
            && x.is_finite()
            && y.is_finite()
            && (0.0..=255.0).contains(&x)
            && (0.0..=255.0).contains(&y)
        {
            pts.push((x, y));
        }
    }
    pts
}

/// The AI recipe's master tone curve as the same (input,output) point list.
fn ai_tone_curve_points(r: &EditRecipe) -> Vec<(f32, f32)> {
    r.tone_curve.iter().map(|p| (p.input as f32, p.output as f32)).collect()
}

/// Build a 256-entry [0..255]→[0..255] LUT from tone-curve control points
/// (piecewise-linear). Identity when empty. Endpoints are pinned to
/// (0,0)/(255,255) unless the curve places its own point at an end — the same
/// rule as `render::curve_lut`, so the judge scores the curve the engine
/// actually renders (an unpinned copy would report e.g. a black lift the
/// pinned render no longer produces).
fn curve_lut(points: &[(f32, f32)]) -> [f32; 256] {
    let mut lut = [0f32; 256];
    if points.is_empty() {
        for (i, v) in lut.iter_mut().enumerate() {
            *v = i as f32;
        }
        return lut;
    }
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // Duplicate inputs: FIRST wins — the same rule as render::curve_lut, so
    // the judge scores the curve the engine actually renders (interpolating
    // across a duplicated input stepped at a different height).
    pts.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-6);
    if pts[0].0 > 0.0 {
        pts.insert(0, (0.0, 0.0));
    }
    if pts[pts.len() - 1].0 < 255.0 {
        pts.push((255.0, 255.0));
    }
    for (i, v) in lut.iter_mut().enumerate() {
        *v = interp255(&pts, i as f32);
    }
    lut
}

fn interp255(pts: &[(f32, f32)], x: f32) -> f32 {
    if x <= pts[0].0 {
        return pts[0].1;
    }
    let last = pts[pts.len() - 1];
    if x >= last.0 {
        return last.1;
    }
    for w in pts.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x >= x0 && x <= x1 {
            let t = if (x1 - x0).abs() < 1e-6 { 0.0 } else { (x - x0) / (x1 - x0) };
            return y0 + (y1 - y0) * t;
        }
    }
    x
}

/// How much a curve lifts the black point (output at input 0; 0 = pinned black).
fn curve_black_lift(lut: &[f32; 256]) -> f32 {
    lut[0]
}

/// S-curve strength: how much the curve brightens the quarter-highlight (input
/// 191) AND darkens the quarter-shadow (input 64) relative to identity. >0 adds
/// contrast (an S); <0 flattens; ~0 is identity/linear.
fn curve_s_strength(lut: &[f32; 256]) -> f32 {
    (lut[191] - 191.0) - (lut[64] - 64.0)
}

/// RMS difference between two 0..255 LUTs (same scale as the curve values).
fn curve_rmse(a: &[f32; 256], b: &[f32; 256]) -> f64 {
    let mut s = 0f64;
    for i in 0..256 {
        let d = (a[i] - b[i]) as f64;
        s += d * d;
    }
    (s / 256.0).sqrt()
}

/// The user's master tone-curve SHAPE from an XMP, summarised as
/// `(black_lift, s_strength)` for the style library — `None` if they drew no
/// curve. Stores the shape, not the raw point list (averaging point lists is
/// mush). Reused by `style.rs` so the curve metric has one definition.
pub(crate) fn user_curve_shape(xmp: &str) -> Option<(f32, f32)> {
    // Scoped like every other whole-document read (`xmp::crs_own_scope`): a
    // creative profile bakes its OWN ToneCurvePV2012 inside `<crs:Look>`, and
    // a flat scan learned that curve as the photographer's habit.
    let pts = parse_tone_curve(&crate::xmp::crs_own_scope(xmp), "ToneCurvePV2012");
    // ANY point counts — even a one-point curve renders (pinned endpoints,
    // the same rule the comparison loop follows). `< 2` silently dropped
    // real one-point curve habits from the style library.
    if pts.is_empty() {
        return None;
    }
    let lut = curve_lut(&pts);
    Some((curve_black_lift(&lut), curve_s_strength(&lut)))
}

/// The 256-entry LUT of a RECIPE's own master tone curve.
///
/// Exposed (batch 2) because the style distillation pulls a proposal's curve
/// toward the library's curve habit, and it has to measure "before" with the
/// ruler that measured the library — [`user_curve_shape`] above. An empty
/// curve is the identity, which `curve_lut` already answers, so there is no
/// special case here.
pub(crate) fn recipe_curve_lut(r: &EditRecipe) -> [f32; 256] {
    curve_lut(&ai_tone_curve_points(r))
}

/// `(black_lift, s_strength)` off a LUT — the same pair
/// [`user_curve_shape`] learns from a sidecar, so "the library's habit" and
/// "this proposal's curve" are quantities on one scale.
pub(crate) fn curve_shape(lut: &[f32; 256]) -> (f32, f32) {
    (curve_black_lift(lut), curve_s_strength(lut))
}

/// Family SUMMARY statistics for one user sidecar — the colour-shaping
/// aggregates the style index stores beside its flat slider map (R23-1 / B6).
///
/// Aggregates on purpose, not the 38 individual keys: averaging per-band values
/// across four retrieved exemplars is mush (one photo's blue and another's
/// orange cancel), and 38 numbers per exemplar would swamp the reference block
/// the prompt can afford. Two or three numbers per family answer the question
/// the reference exists to answer — how hard does this photographer push
/// colour?
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FamilySummary {
    /// Mean |value| over the 8 bands, per axis: hue, saturation, luminance.
    pub hsl: [f32; 3],
    /// `[strongest wheel saturation, mean |wheel luminance|]`.
    pub grade: [f32; 2],
    /// How many of the three per-channel RGB curves they drew (0..=3).
    pub rgb_curves: u8,
}

impl Default for FamilySummary {
    fn default() -> Self {
        Self { hsl: [0.0; 3], grade: [0.0; 2], rgb_curves: 0 }
    }
}

impl FamilySummary {
    /// Every number finite (a NaN serialises as `null` and makes the whole
    /// index unloadable — the `exemplar_is_finite` rule).
    pub fn is_finite(&self) -> bool {
        self.hsl.iter().chain(&self.grade).all(|v| v.is_finite())
    }

    /// Bound every number to its band and the curve count to 3 — this reaches
    /// a paid prompt, so it gets invariants at the door like the rest of the
    /// index.
    pub fn clamp(&mut self) {
        for v in self.hsl.iter_mut().chain(self.grade.iter_mut()) {
            *v = if v.is_finite() { v.clamp(0.0, 100.0) } else { 0.0 };
        }
        self.rgb_curves = self.rgb_curves.min(3);
    }
}

/// The colour-family summary of a user sidecar, or `None` when they shaped no
/// colour at all (all bands neutral, no wheel, no channel curve) — an all-zero
/// summary in the index would tell the AI "this user pushes nothing", which is
/// only honest if we measured something.
///
/// Read through the SAME registry expansions the eval ruler uses, so the index
/// and the report can never disagree about what "uses the mixer" means.
pub(crate) fn user_family_summary(xmp: &str) -> Option<FamilySummary> {
    // Same scope rule as every other whole-document read: a creative Look's
    // baked mixer belongs to the PROFILE, not the photographer.
    let scope = crate::xmp::crs_own_scope(xmp);
    let scope = Scope::new(scope.as_ref());
    let mut sums = [0.0f32; 3];
    for f in catalogue::hsl_expansion() {
        sums[f.axis] += scope.crs_f32(&f.crs).unwrap_or(0.0).abs();
    }
    let hsl = sums.map(|s| s / crate::recipe::HSL_BANDS.len() as f32);
    let wheel = |suffix: &str| -> Vec<f32> {
        COLOR_GRADE_CRS
            .iter()
            .filter(|(field, _)| field.ends_with(suffix))
            .map(|(_, key)| scope.crs_f32(key).unwrap_or(0.0))
            .collect()
    };
    let max_sat = wheel("_sat").into_iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let lums = wheel("_lum");
    let mean_lum = if lums.is_empty() {
        0.0
    } else {
        lums.iter().map(|v| v.abs()).sum::<f32>() / lums.len() as f32
    };
    let rgb_curves = ["ToneCurvePV2012Red", "ToneCurvePV2012Green", "ToneCurvePV2012Blue"]
        .iter()
        .filter(|tag| !parse_tone_curve(scope.text(), tag).is_empty())
        .count() as u8;
    let mut out = FamilySummary { hsl, grade: [max_sat, mean_lum], rgb_curves };
    out.clamp();
    (out != FamilySummary::default()).then_some(out)
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Acc {
    sum_abs: f64,
    sum_signed: f64,
    n: u32,
    /// Times the user used this control but the AI left it neutral/omitted it —
    /// a real miss the old both-set gate dropped silently.
    omit: u32,
}

impl Acc {
    /// Fold one photograph's contribution in. Split out of the loop by R27
    /// Batch-7 so the per-photo work can run on a worker while the SUMS are
    /// still added in photo order — `f64` addition is not associative, so
    /// folding in completion order would move the reported gap score in the
    /// last ulp depending on which photo happened to finish first.
    fn merge(&mut self, other: &Acc) {
        self.sum_abs += other.sum_abs;
        self.sum_signed += other.sum_signed;
        self.n += other.n;
        self.omit += other.omit;
    }
}

fn low_n_marker(n: u32) -> &'static str {
    if (n as usize) < LOW_N_ANNOTATION_THRESHOLD {
        " [low n]"
    } else {
        ""
    }
}

/// Preserve the established scalar table formatting, adding only the new
/// low-sample marker to rows that need it.
fn format_scalar_report_line(metric: &str, a: &Acc) -> String {
    let label = format!("{metric}{}", low_n_marker(a.n));
    if a.n > 0 {
        format!(
            "{label:<22} {:>4} {:>10.2} {:>+13.2} {:>8}\n",
            a.n,
            a.sum_abs / a.n as f64,
            a.sum_signed / a.n as f64,
            a.omit
        )
    } else {
        format!("{label:<22} {:>4} {:>10} {:>13} {:>8}\n", a.n, "—", "—", a.omit)
    }
}

fn format_scalar_report_rows(ruler: &[Row], acc: &BTreeMap<String, Acc>) -> String {
    let mut out = format!(
        "Legend: [low n] marks rows with n < {LOW_N_ANNOTATION_THRESHOLD}; treat their per-control means as less stable.\n"
    );
    for row in ruler {
        let Some(a) = acc.get(&row.metric) else { continue };
        if a.n == 0 && a.omit == 0 {
            continue;
        }
        out.push_str(&format_scalar_report_line(&row.metric, a));
    }
    out
}

fn format_supplementary_gap_line(weighted: f64) -> String {
    format!(
        "Supplementary n-weighted gap score: {weighted:.1}%  (scalar block redistributed by n; term structure unchanged)\n"
    )
}

fn n_weighted_scalar_mean(rows: &[(f64, u32)]) -> Option<f64> {
    let total_n: u64 = rows.iter().map(|(_, n)| u64::from(*n)).sum();
    (total_n > 0).then(|| {
        rows.iter()
            .map(|(value, n)| *value * f64::from(*n) / total_n as f64)
            .sum()
    })
}

/// What became of one photograph. The three non-`Stats` arms exist because the
/// serial loop had three distinct exits (`continue` on a failed analysis, `?` on
/// an unreadable sidecar, and — new — "an earlier photo already aborted the
/// run"), and a pool cannot `?` out of a worker: the decision has to travel back
/// as a value and be acted on in index order.
enum PhotoOutcome {
    Stats(Box<PhotoStats>),
    /// A saved row already answers for this photograph, so nothing was
    /// decoded, asked or billed for it — its stats are folded from the state
    /// file at the same index, through the same [`Acc::merge`].
    Restored,
    /// `produce_recipe` failed; the FAILED line is already in this photo's
    /// block and the run continues, exactly as the serial `continue` did.
    Failed,
    /// The hard error the serial loop raised with `?`, carried out to be
    /// re-raised after the pool drains.
    Aborted(String),
    /// Dequeued after another photo aborted — never analyzed, never billed.
    Skipped,
}

/// Emit one output fragment.
///
/// At one job the fragment goes STRAIGHT to stdout and is flushed, so the
/// `[i/n] stem ... ` prefix still appears before the minute of network work the
/// photo is about to spend — the serial harness's progress indicator, kept
/// byte-for-byte. Above one job nothing may print from a worker: the fragment
/// joins the photo's block and the sequencer releases it in index order.
fn emit(live: bool, block: &mut String, text: &str) {
    if live {
        use std::io::Write;
        print!("{text}");
        let _ = std::io::stdout().flush();
    } else {
        block.push_str(text);
    }
}

/// Everything ONE photograph contributes to the report — produced on a worker,
/// folded on the main thread in index order (see [`Acc::merge`]).
///
/// Serialisable since the resume work below: this struct IS the state file's
/// payload, so a photograph measured in one run folds into another run's
/// report through the same [`Acc::merge`] it would have taken fresh. NO
/// `#[serde(default)]` on purpose — a truncated last line (the crash this
/// whole mechanism exists for) must fail to parse and be re-measured, not
/// deserialise into an all-zero contribution that claims the photograph was
/// measured.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct PhotoStats {
    /// This photo's per-control contributions, keyed exactly like the report.
    acc: BTreeMap<String, Acc>,
    /// `(rmse, user_lift, ai_lift, user_s, ai_s)` when either side drew a
    /// master curve.
    curve: Option<(f64, f64, f64, f64, f64)>,
    /// Per-channel curve RMSE, entered only where either side drew one.
    rgb: [Option<f64>; 3],
    /// `(imported, our importer refuses, AI proposed)` when any side had one.
    masks: Option<(usize, usize, usize)>,
}

// ── resumable progress ──────────────────────────────────────────────────────
//
// Two 147-photo PAID runs were thrown away whole (2026-08-20) because ONE
// upstream stream error poisoned each of them, and the transcript held no
// per-photo numbers to salvage: the report table below prints once, at the end,
// so a run that does not reach the end measured nothing anybody can use. This
// is the second arm of that one root cause — "a single mid-run failure forfeits
// the whole run"; the first is the transport retry in `advisor`.
//
// Every photograph whose measurement COMPLETED and came from the vision
// proposer is appended here as it lands, and a rerun folds those rows back in
// instead of re-buying them. The rows are the same `PhotoStats` the pool
// produces and take the same fold at the same index, so the arithmetic does not
// depend on where a run was interrupted.

/// The state file's FIRST line: what the rows below it were measured under.
///
/// Deliberately narrow, and deliberately not a machine description. A run is
/// resumable only against the same build and the same two models, so those are
/// what is recorded and compared. NO endpoint URL, NO key, and no path but the
/// folder the user themselves typed — this file lives in the per-user store,
/// but it is still a file a user may hand to somebody else when reporting a bad
/// baseline.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StateHeader {
    autoshade_version: String,
    /// The vision proposer's model id (`Config::openai_model`).
    proposer_model: String,
    /// The verifier's model id (`Config::analysis_model`).
    analysis_model: String,
    limit: usize,
    /// The folder AS THE USER GAVE IT.
    dir: String,
    /// Unix seconds. Provenance only — never compared.
    started: u64,
}

/// One measured photograph, as one line of the state file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StateRow {
    /// Readability of the file itself; `rel` is what a resume matches on.
    stem: String,
    /// The photograph's path RELATIVE to the eval folder, which is what
    /// actually identifies it: `pipeline::find_raws` RECURSES, so two shoot
    /// folders under one library can each hold a `DSC00001`, and a stem-keyed
    /// row would answer for the wrong photograph. Falls back to the stem when
    /// the path cannot be expressed under the folder — never an absolute path,
    /// which would put a machine path into a file the user may share.
    rel: String,
    /// SHA-256 of the sidecar this measurement was taken against: the
    /// photographer re-editing that photo invalidates the row.
    xmp_sha256: String,
    stats: PhotoStats,
}
/// SHA-256, lowercase hex — the shared implementation.
///
/// This module and `describe` each wrote their own until the two were folded
/// into [`crate::sha256`]: same digest, one known-answer test, one place a
/// mistyped constant can hide. The digest names a state file and decides
/// whether a saved measurement still describes the sidecar it was taken from,
/// so it must stay a fixed function of the bytes for ever.
use crate::sha256::sha256_hex;

/// A photograph's identity INSIDE the eval folder — see [`StateRow::rel`].
fn photo_rel(dir: &Path, raw: &Path) -> String {
    raw.strip_prefix(dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| pipeline::stem(raw).to_string())
}

/// Where a run's progress lives when the caller names no path.
///
/// Under the per-user store ([`crate::store::store_root`]), NEVER beside the
/// photographs: the library is read-only by settled project law. The name is
/// derived from the two things that decide the work list — the folder and
/// `--limit` — so re-running the same command finds its own file and no other
/// run's. Windows spellings fold case, the rule `store::photo_key` already
/// follows, so `d:\pics` and `D:\Pics` are one run.
fn default_state_path(dir: &Path, limit: usize) -> PathBuf {
    let abs = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let spelling = abs.to_string_lossy().into_owned();
    let spelling = if cfg!(windows) { spelling.to_lowercase() } else { spelling };
    let key = sha256_hex(format!("{spelling}|{limit}").as_bytes());
    crate::store::store_root().join("eval-state").join(format!("{}.jsonl", &key[..16]))
}

/// The saved rows at `path`, or an empty vec when there is nothing there.
///
/// `Err` is a REFUSAL, not a failure to read: a file whose header describes a
/// different build or different models must not be folded into this run's
/// table, because one table measured by two different things is not a
/// measurement. Both sides are printed so the reader can see WHICH moved.
fn load_state(path: &Path, want: &StateHeader) -> Result<Vec<StateRow>> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(Vec::new()) };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut lines = text.lines();
    let Some(Ok(head)) = lines.next().map(serde_json::from_str::<StateHeader>) else {
        anyhow::bail!(
            "the saved eval progress at {} does not begin with a readable header — re-run with \
             --fresh to discard it, or --state <path> to write this run somewhere else",
            path.display()
        );
    };
    if head.autoshade_version != want.autoshade_version
        || head.proposer_model != want.proposer_model
        || head.analysis_model != want.analysis_model
    {
        anyhow::bail!(
            "the saved eval progress at {p} was measured under autoshade {hv} (proposer {hp}, \
             analysis {ha}); this run is autoshade {wv} (proposer {wp}, analysis {wa}). Folding \
             them into one table would report a gap score measured by two different things — \
             re-run with --fresh to start over, or --state <path> to keep both.",
            p = path.display(),
            hv = head.autoshade_version,
            hp = head.proposer_model,
            ha = head.analysis_model,
            wv = want.autoshade_version,
            wp = want.proposer_model,
            wa = want.analysis_model,
        );
    }
    // A line that does not parse is a line that is not trusted — a crash
    // mid-write leaves exactly one of those at the end of the file, and the
    // photograph it half-described is simply measured again.
    Ok(lines.filter_map(|l| serde_json::from_str::<StateRow>(l).ok()).collect())
}

/// Which of `work`'s photographs the saved rows still answer for, by index,
/// plus the stems whose row was thrown away as stale.
///
/// A row is honoured only when it names this photograph AND the sidecar on disk
/// still hashes to what the measurement was taken against: the photographer
/// re-editing a photo invalidates the measurement OF that photo, and silently
/// reusing the row would score the AI against an edit that no longer exists.
/// The sidecar is read through the same capped reader [`run`] measures with
/// (`store::read_sidecar`), so the hash covers exactly the text that was read.
fn resume_plan(
    dir: &Path,
    work: &[(&Path, &Path)],
    rows: Vec<StateRow>,
) -> (Vec<Option<Box<PhotoStats>>>, Vec<String>) {
    // Last row wins: a photograph re-measured after a stale row was discarded
    // has both lines in the file, and the newer one is the live measurement.
    let mut by_rel: BTreeMap<String, StateRow> = BTreeMap::new();
    for row in rows {
        by_rel.insert(row.rel.clone(), row);
    }
    let mut restored = Vec::with_capacity(work.len());
    let mut stale = Vec::new();
    for (raw, sidecar) in work {
        let Some(row) = by_rel.remove(&photo_rel(dir, raw)) else {
            restored.push(None);
            continue;
        };
        let now = crate::store::read_sidecar(sidecar).map(|t| sha256_hex(t.as_bytes()));
        if now.as_deref() == Some(row.xmp_sha256.as_str()) {
            restored.push(Some(Box::new(row.stats)));
        } else {
            stale.push(row.stem.clone());
            restored.push(None);
        }
    }
    (restored, stale)
}

/// Appends one row per completed photograph, as it completes.
///
/// Flushed per row on purpose: the whole point is that a run killed by the next
/// upstream stream error keeps everything measured before it.
struct StateLog {
    path: PathBuf,
    /// `None` = writing is off, because it failed. A state file that cannot be
    /// written DEGRADES the run to un-resumable and says so; it does not fail a
    /// run whose expensive half is the API spend.
    sink: std::sync::Mutex<Option<std::fs::File>>,
}

impl StateLog {
    /// The file itself: appended to when this is a resume, created with its
    /// header line when it is not.
    fn open_sink(
        path: &Path,
        header: &StateHeader,
        resuming: bool,
    ) -> std::io::Result<std::fs::File> {
        use std::io::Write as _;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if resuming {
            return std::fs::OpenOptions::new().append(true).open(path);
        }
        let mut f = std::fs::File::create(path)?;
        let head = serde_json::to_string(header).map_err(std::io::Error::other)?;
        writeln!(f, "{head}")?;
        Ok(f)
    }

    /// Open the file, writing the header when this is not a resume.
    fn open(path: &Path, header: &StateHeader, resuming: bool) -> Self {
        let sink = match Self::open_sink(path, header, resuming) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!(
                    "⚠ eval progress cannot be saved to {} ({e}) — this run still measures, but \
                     a rerun will have to re-buy every photograph.",
                    path.display()
                );
                None
            }
        };
        Self { path: path.to_path_buf(), sink: std::sync::Mutex::new(sink) }
    }

    fn record(&self, row: &StateRow) {
        use std::io::Write as _;
        let mut guard = self.sink.lock().unwrap_or_else(|p| p.into_inner());
        let Some(file) = guard.as_mut() else { return };
        let wrote = serde_json::to_string(row)
            .map_err(std::io::Error::other)
            .and_then(|line| writeln!(file, "{line}").and_then(|()| file.flush()));
        if let Err(e) = wrote {
            eprintln!(
                "⚠ eval progress stopped being saved to {} ({e}) — the rows already written are \
                 still good.",
                self.path.display()
            );
            *guard = None;
        }
    }
}

/// Did the VISION proposer answer for this photograph, or did the run fall back
/// to the deterministic baseline?
///
/// Read off the typed note channel `produce_recipe` already returns — no new
/// field, no new global, because the fact was already on the wire. The
/// heuristic stands in under exactly two keys: `HEURISTIC_UNAVAILABLE` (the
/// proposer WAS tried and failed — the mid-run stream error that cost two paid
/// 147-photo runs) and `HEURISTIC_NO_KEY` (no proposer was configured at all).
/// Both mean the numbers came from arithmetic over the histogram rather than
/// from the model this measurement is about, so neither may be saved as
/// progress: a rerun that folded them back in would print a baseline built
/// partly out of the fallback, which is the poisoning this file exists to
/// prevent.
///
/// The note cannot be lost on THIS path: it is pushed first, and the two places
/// that clear the note vec (an adopted verifier revision, an adopted visual
/// round) are both unreachable here — a fallback proposal sets
/// `can_revise = false`, and `eval` passes `judge = false`.
fn proposer_answered(notes: &[crate::rationale::Note]) -> bool {
    use crate::rationale::keys;
    !notes
        .iter()
        .any(|n| n.key == keys::HEURISTIC_UNAVAILABLE || n.key == keys::HEURISTIC_NO_KEY)
}

/// Append this photograph's row unless the proposer fell back. Returns whether
/// the row was persisted — the caller discloses the `false` case in the
/// transcript and in the report's provenance line.
fn record_measured(log: &StateLog, notes: &[crate::rationale::Note], row: &StateRow) -> bool {
    if !proposer_answered(notes) {
        return false;
    }
    log.record(row);
    true
}

/// The report line that says what the transport's one-repeat funnel did this
/// run (R29 ruling 拍板一: 524 and 529 are each posted a second time).
///
/// `None` when nothing repeated, because a row of zeros would bury the
/// interesting case — a run that hit no transient at all — under a line that
/// looks the same as one that hit four. When something DID repeat, both halves
/// are named, and they are not the same fact: a repeat that then answered is
/// the photograph the ruling BOUGHT (it is in the table and in saved
/// progress), while a repeat that failed again is a photograph that still fell
/// back, so it is inside the fallback count on the line above and deliberately
/// not persisted. The 524 sentence is not decoration either: the ruling
/// accepted at most one duplicate charge per photograph as the price of not
/// scrapping a 147-photo run, and a transcript that never says so hides the
/// cost that was accepted. It names both transient classes rather than a count
/// because this layer is given counts, not codes — the per-occurrence stderr
/// disclosure, which does know the code, is where an operator reads which one
/// it was.
fn retry_disclosure(t: crate::advisor::RetryTally) -> Option<String> {
    (t.repeated > 0).then(|| {
        format!(
            "{} upstream call(s) failed transiently and were posted a second time: \
             {} recovered (the repeated call's outer response succeeded), {} failed again \
             (those photographs fell back). A repeated relay timeout (HTTP 524) or relay \
             overload response (HTTP 529) may have been billed twice.",
            t.repeated,
            t.recovered,
            t.exhausted()
        )
    })
}

/// Supplementary gap view: keep the headline's active-term structure and
/// redistribute only the scalar block by observed sample counts (n / total
/// scalar n). With equal n, this is exactly the headline over the same rows;
/// the curve terms and their one-term-per-curve normalization are unchanged.
/// This intentionally does not replace the headline or alter the state file.
fn supplementary_weighted_gap(
    ruler: &[Row],
    acc: &BTreeMap<String, Acc>,
    curve_n: u32,
    sum_curve_rmse: f64,
    rgb_acc: &[(u32, f64); 3],
) -> Option<f64> {
    let scalar: Vec<(f64, u32)> = ruler
        .iter()
        .filter_map(|row| {
            let a = acc.get(&row.metric)?;
            (a.n > 0).then(|| (((a.sum_abs / a.n as f64) / row.full_scale()), a.n))
        })
        .collect();
    let scalar_terms = scalar.len() as u32;
    let mut sum = if scalar_terms > 0 {
        f64::from(scalar_terms) * n_weighted_scalar_mean(&scalar).unwrap()
    } else {
        0.0
    };
    let mut terms = scalar_terms;
    if curve_n > 0 {
        sum += (sum_curve_rmse / curve_n as f64) / 255.0;
        terms += 1;
    }
    for (n, total) in rgb_acc {
        if *n > 0 {
            sum += (*total / *n as f64) / 255.0;
            terms += 1;
        }
    }
    (terms > 0).then(|| 100.0 * sum / terms as f64)
}

/// The pool body's FIRST decision: a photograph a valid saved row already
/// answers for is not measured — no decode, no API call, no spend.
///
/// Separated from the body so that promise is testable without a network: the
/// test counts how often `measure` runs.
fn restore_or_measure<M>(
    live: bool,
    block: &mut String,
    restored: bool,
    measure: M,
) -> PhotoOutcome
where
    M: FnOnce(&mut String) -> PhotoOutcome,
{
    if restored {
        emit(live, block, "from saved progress\n");
        return PhotoOutcome::Restored;
    }
    measure(block)
}

pub fn run(
    dir: &Path,
    xmp_dir: Option<&Path>,
    limit: usize,
    jobs: usize,
    fresh: bool,
    state: Option<&Path>,
) -> Result<()> {
    let cfg = Config::load();
    // R27 P1, deliberate NON-action: this stays RAW-only, and not for want of
    // a flag. `eval` scores the AI against the user's OWN edit, which it reads
    // from the sibling `.xmp`. For a baked photo that sidecar is off limits by
    // a settled, thrice-cited policy — `store::lightroom_sidecar` returns None
    // for non-RAW ("a baked PNG/TIFF's neighbouring `.xmp` is not ours to
    // interpret"), `pipeline`'s merge base skips it, and the GUI disables
    // "Export XMP beside photo". Including baked sources here would reopen
    // that decision through the back door, and would score the AI against a
    // sidecar written by some other program about some other image.
    let raws = pipeline::find_raws(dir)?;
    // The SAME pairing rule the style index builds with ([`crate::xmp_pair`]),
    // resolved once here: `run` and `resume_plan` must agree about which file a
    // measurement was taken against, and two derivations of one name is exactly
    // how they would stop agreeing.
    let pairing = crate::xmp_pair::XmpPairing::new(dir, xmp_dir);
    let pairs: Vec<(&Path, PathBuf)> = raws
        .iter()
        .filter_map(|r| pairing.find(r).map(|x| (r.as_path(), x)))
        .collect();
    println!(
        "found {} RAW(s); {} have an .xmp sidecar (your edits). Evaluating {}.",
        raws.len(),
        pairs.len(),
        pairs.len().min(limit)
    );
    if pairs.is_empty() {
        println!(
            "Nothing to evaluate — no .xmp sidecars were found beside these RAWs or under \
             --xmp-dir."
        );
        return Ok(());
    }

    // The ruler, in registry order (which is also the report order — the two
    // used to be separate hand-kept lists with a comment claiming they matched).
    let ruler = rows();
    // The work list is fixed up front — exactly the first `limit` pairs the old
    // sequential `.take(limit)` selected — then a bounded, memory-budgeted pool
    // works through it (R27 Batch-7). At `--jobs 1` this is the serial loop it
    // replaces, line for line and sum for sum.
    let work: Vec<(&Path, &Path)> =
        pairs.iter().take(limit).map(|(r, x)| (*r, x.as_path())).collect();
    let n = work.len();

    // ── resumable progress ───────────────────────────────────────────────
    let state_path = match state {
        Some(p) => p.to_path_buf(),
        None => default_state_path(dir, limit),
    };
    let header = StateHeader {
        autoshade_version: env!("CARGO_PKG_VERSION").to_string(),
        proposer_model: cfg.openai_model.clone(),
        analysis_model: cfg.analysis_model.clone(),
        limit,
        dir: dir.display().to_string(),
        started: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    println!("progress: {}", state_path.display());
    if fresh && state_path.exists() {
        std::fs::remove_file(&state_path).map_err(|e| {
            anyhow::anyhow!("--fresh could not discard {}: {e}", state_path.display())
        })?;
        println!("  --fresh: saved progress discarded; every photograph is measured again.");
    }
    let saved = load_state(&state_path, &header)?;
    // Append to a file that already carries a valid header; write a new one
    // otherwise. An existing file whose rows all turned out stale is still
    // appended to — its stale lines stay, and the fresh row for the same
    // photograph, written later, is the one a later resume reads.
    let resuming = !saved.is_empty();
    let (mut restored, stale) = resume_plan(dir, &work, saved);
    for stem in &stale {
        println!("  {stem}: its .xmp changed since it was measured — measuring it again.");
    }
    let loaded = restored.iter().filter(|r| r.is_some()).count();
    if loaded > 0 {
        println!("resuming: {loaded} of {n} already measured, {} to go.", n - loaded);
    }
    let log = StateLog::open(&state_path, &header, resuming);
    // How many photographs this run measured with the deterministic fallback
    // standing in for the proposer. A COUNT, order-independent, so an atomic
    // is honest here where an f64 sum would not be.
    let fallbacks = std::sync::atomic::AtomicU32::new(0);
    // The transport's one-repeat funnel is counted process-wide, so THIS run's
    // numbers are a difference across the work below (see `advisor::RetryTally`
    // for why the counters do not travel through `produce_recipe`).
    let retries_before = crate::advisor::retry_tally();
    // `plan_for` for the same reason `batch` uses it (R28 Batch-4 4a), even
    // though `eval`'s work list is RAW+.xmp pairs by construction and the
    // survey therefore finds nothing to raise: the door a caller takes should
    // not depend on a filter two functions away staying RAW-only.
    let photos: Vec<&Path> = work.iter().map(|(r, _)| *r).collect();
    let plan = crate::jobs::plan_for(jobs, &photos);
    if let Some(note) = &plan.note {
        println!("{note}");
    }
    if plan.jobs > 1 {
        println!(
            "  {} photo(s) in flight; each photo's lines are held and printed in order.",
            plan.jobs
        );
    }
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    let mut evaluated = 0u32;
    // Master-tone-curve comparison (the look control the flat sliders miss),
    // accumulated only over photos where YOU drew a curve.
    let (mut curve_n, mut sum_curve_rmse) = (0u32, 0f64);
    let (mut sum_user_lift, mut sum_ai_lift) = (0f64, 0f64);
    let (mut sum_user_s, mut sum_ai_s) = (0f64, 0f64);
    // The three per-channel RGB curves (R23-1): same RMSE metric as the master,
    // one accumulator per channel, entered only when either side drew one.
    let rgb_curves = [("red", "ToneCurvePV2012Red"), ("green", "ToneCurvePV2012Green"),
        ("blue", "ToneCurvePV2012Blue")];
    let mut rgb_acc = [(0u32, 0f64); 3];
    // Local masks: how many the photographer used (importable + the ones our
    // importer refuses) against how many the AI proposed. A count, not a
    // divergence — deliberately NOT folded into the gap score below.
    let (mut mask_photos, mut user_masks, mut user_masks_unsupported, mut ai_masks) =
        (0u32, 0usize, 0usize, 0usize);

    // Set the moment a photo raises the HARD error the serial loop raised with
    // `?` (an unreadable sidecar). Checked at dequeue so the remaining photos
    // are skipped instead of billed — at `--jobs 1` that is exactly the old
    // stop-at-once behaviour; above it, the photos already in flight finish
    // first and the run then fails with the same message.
    let aborted = std::sync::atomic::AtomicBool::new(false);
    let live = plan.jobs == 1;
    let outcomes = crate::jobs::for_each_indexed(plan.jobs, n, |i, block| {
        if aborted.load(std::sync::atomic::Ordering::Relaxed) {
            return PhotoOutcome::Skipped;
        }
        let (raw, sidecar) = work[i];
        emit(live, block, &format!("[{}/{}] {} ... ", i + 1, n, pipeline::stem(raw)));
        restore_or_measure(live, block, restored[i].is_some(), |block| {
            let Some(xmp_text) = crate::store::read_sidecar(sidecar) else {
                aborted.store(true, std::sync::atomic::Ordering::Relaxed);
                emit(live, block, "FAILED\n");
                return PhotoOutcome::Aborted(format!("read user xmp for {}", raw.display()));
            };
            let mut stats = PhotoStats::default();
            // The Description's OWN scope, like every other whole-document reader
            // (`xmp::crs_own_scope`): a nested creative Look's baked parameters are
            // the PROFILE's, not the photographer's, and scoring the AI against
            // them charged it for matching a look no slider in the sidecar states.
            let scope = crate::xmp::crs_own_scope(&xmp_text);
            let scope = Scope::new(scope.as_ref());
            // Any value OTHER than "As Shot" is a user WB decision; an absent
            // attribute (non-LR / hand-trimmed sidecar) keeps the Kelvin as
            // intentional.
            let user_wb = scope.crs_str("WhiteBalance").as_deref() != Some("As Shot");
            let has_crop = scope.crs_str("HasCrop").as_deref() == Some("True");
            // style_strength = 0 AND judge = false: eval measures the RAW AI
            // proposal vs your edits, so it must NOT pull toward your historical
            // style and must NOT let the visual closed loop revise the proposal
            // (either would bias the gap — and the judge is a paid vision call
            // per photo, unasked-for in a measurement run).
            let (ai, _verdict, notes) =
                match pipeline::produce_recipe(
                    raw,
                    &cfg,
                    false,
                    None,
                    None,
                    // The CALIBRATION point, named: eval is the measurement
                    // baseline R23-5's wider ruler is calibrated against, so it must
                    // not drift when the product default moves (R23-3).
                    //
                    // What this pins is the NUMBER axis — the prompt's ±50/±35 pair
                    // and temper's knees are bit-for-bit pre-R23 here. It does NOT
                    // freeze the request: R23-1 rewrote the proposer prose at every
                    // strength (and the verbatim old wording now lives in the ≤ 0.4
                    // band, where soft_cap_factor is 0.93 and the numbers would drift
                    // instead). So scores from this harness are not comparable across
                    // R23 whatever value stands here — the same break the module doc
                    // already records for the widened ruler — and the 147-photo set
                    // has to be re-measured to give the new baseline a number.
                    pipeline::GradeRequest {
                        strength: crate::recipe::GradeStrength::calibrated(),
                        ..Default::default()
                    },
                    false,
                    // The DEFAULT channel, deliberately (R29-1). `batch`
                    // collects its workers' diagnostics and renders them into
                    // the photo's block; eval does not, because the same
                    // disclosures already ride the typed `rationale::Note`
                    // channel it renders below by construction, and routing
                    // both would print each fallback twice in the measurement
                    // transcript. What eval loses is the ORDER of the stderr
                    // copy, which is where it was before this batch.
                    crate::diag::stderr(),
                ) {
                Ok(v) => v,
                Err(e) => {
                    emit(live, block, &format!("FAILED: {e}\n"));
                    return PhotoOutcome::Failed;
                }
            };
            for row in ruler.iter() {
                let eps = row.eps();
                let n0 = neutral(row, &ai);
                let user_val = user_value(row, scope, user_wb, has_crop);
                // "Used" is measured against the control's NEUTRAL, not against
                // zero: `color_grade.blending` sits at 50 untouched, so a
                // zero-based test called every neutral wheel a ±50 disagreement.
                let user_used = user_val.is_some_and(|u| (u - n0).abs() > eps);
                let ai_used = match row.metric.as_str() {
                    // Stating an absolute Kelvin AT ALL is a WB decision, even one
                    // that lands on the as-shot value.
                    "temperature_k" => ai.temperature_k.is_some(),
                    _ => ai_value(row, &ai).is_some_and(|a| (a - n0).abs() > eps),
                };
                // An absent slider means the photographer left it at its neutral,
                // an absent white balance means they kept as-shot — so a one-sided
                // disagreement still compares two real numbers instead of being
                // dropped from the aggregate (which understated the gap score).
                let u = user_val.unwrap_or(n0);
                let ai_val = ai_value(row, &ai);
                // A both-neutral row is not a comparison: Lightroom writes EVERY
                // slider explicitly (Contrast2012="0"), so counting 0↔0 rows
                // inflated n and diluted each control's MAE toward zero — the gap
                // score understated real divergence. A row where EITHER side
                // moved still counts (that disagreement is the signal).
                if !user_used && !ai_used {
                    continue;
                }
                // …with ONE exception, and it is a different question (R28 Batch-5
                // 5b): a toning wheel's hue is only a value while the wheel has
                // saturation. "Either side moved it" is the right rule for a slider
                // whose every position renders something; it is the wrong rule for
                // an angle that renders nothing at zero saturation, and it is what
                // produced the 141° `shadow_hue` artifact. See `hue_carries_colour`
                // for the threshold, what it costs, and why that row's history is
                // not comparable with what this run prints.
                if row.circular && !hue_carries_colour(row, scope, &ai) {
                    continue;
                }
                let e = stats.acc.entry(row.metric.clone()).or_default();
                match ai_val {
                    Some(a) => {
                        let d = row.diff(a, u) as f64;
                        e.sum_abs += d.abs();
                        e.sum_signed += d;
                        e.n += 1;
                        // You moved it; the AI parked it at neutral → a miss.
                        if user_used && !ai_used {
                            e.omit += 1;
                        }
                    }
                    // Unreachable while every row has a reader (pinned by the
                    // catalogue tests); kept so a future row without one still
                    // shows up as a miss instead of vanishing.
                    None => {
                        if (u - n0).abs() > eps {
                            e.omit += 1;
                        }
                    }
                }
            }

            // --- master tone curve: did the AI commit to a curve like you did? ---
            // Entered when EITHER side has a real curve: gating on the user's
            // alone made an AI-only curve invisible to the metric (an identity
            // user curve is a valid comparison baseline — curve_lut of no points
            // is the identity LUT). ANY point counts: even a one-point curve
            // renders (pinned endpoints), so `>= 2` still hid real curves.
            // Scoped like `user_curve_shape` and `parse_user_xmp` — the SAME rule
            // at every read of a whole sidecar, or the report scores the AI
            // against a curve the profile baked, not the one the user drew.
            let user_curve =
                parse_tone_curve(&crate::xmp::crs_own_scope(&xmp_text), "ToneCurvePV2012");
            let ai_curve = ai_tone_curve_points(&ai);
            if !user_curve.is_empty() || !ai_curve.is_empty() {
                let ulut = curve_lut(&user_curve);
                let alut = curve_lut(&ai_curve);
                stats.curve = Some((
                    curve_rmse(&ulut, &alut),
                    curve_black_lift(&ulut) as f64,
                    curve_black_lift(&alut) as f64,
                    curve_s_strength(&ulut) as f64,
                    curve_s_strength(&alut) as f64,
                ));
            }

            // --- the three per-channel RGB curves (R23-1) ---------------------
            // The colour-shaping companion to the master curve, and the other half
            // of the "did the AI use the look controls at all?" question. Same
            // either-side rule and same RMSE metric; the SHAPE metrics (black lift
            // / S-strength) are master-curve vocabulary and are not repeated here.
            for (ch, (_, tag)) in rgb_curves.iter().enumerate() {
                let user_curve = parse_tone_curve(scope.text(), tag);
                let ai_curve: Vec<(f32, f32)> = match catalogue::global_value(&ai, &format!("{}_curve", rgb_curves[ch].0)) {
                    Some(catalogue::GlobalValue::Curve(pts)) => {
                        pts.iter().map(|p| (p.input as f32, p.output as f32)).collect()
                    }
                    _ => Vec::new(),
                };
                if user_curve.is_empty() && ai_curve.is_empty() {
                    continue;
                }
                stats.rgb[ch] =
                    Some(curve_rmse(&curve_lut(&user_curve), &curve_lut(&ai_curve)));
            }

            // --- local masks: counts, not divergence --------------------------
            // The user's side comes from the same importer the app uses, plus the
            // corrections it REFUSES (foreign brush/AI masks): reporting only the
            // importable ones would understate how much local work the photograph
            // actually had.
            let imported = crate::xmp::xmp_to_recipe_for_photo(&xmp_text, raw).masks.len();
            let refused = crate::xmp::unsupported_corrections_for_photo(&xmp_text, raw);
            if imported + refused + ai.masks.len() > 0 {
                stats.masks = Some((imported, refused, ai.masks.len()));
            }

            emit(live, block, "done\n");
            // Above one job the per-photo ⚠ lines the pipeline raises on stderr
            // (the GPT-proposer fallback warn in `pipeline::produce_recipe`) arrive in COMPLETION
            // order and can no longer be read as belonging to the line above them.
            // The same disclosures ride the typed note channel by construction, so
            // attach them to THIS photo's block — the attribution the reordering
            // costs, bought back on the channel that already carries it. At one job
            // the transcript stays exactly what it was.
            if !live && !notes.is_empty() {
                let text = crate::rationale::render_en(&notes);
                let text = text.trim();
                if !text.is_empty() {
                    block.push_str("       ");
                    block.push_str(text);
                    block.push('\n');
                }
            }
            // The measurement is complete: save it, so the NEXT upstream stream
            // error costs this photograph nothing. A photograph the proposer
            // fell back on completes the run exactly as it does today — it is
            // in the table below — but it is not saved, and the disclosure says
            // so on the photo's own block rather than only in the summary.
            let row = StateRow {
                stem: pipeline::stem(raw).to_string(),
                rel: photo_rel(dir, raw),
                xmp_sha256: sha256_hex(xmp_text.as_bytes()),
                stats,
            };
            if !record_measured(&log, &notes, &row) {
                fallbacks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                emit(
                    live,
                    block,
                    "       (the proposer fell back to the deterministic baseline — NOT saved as \
                     progress; a rerun measures this photograph again)\n",
                );
            }
            PhotoOutcome::Stats(Box::new(row.stats))
        })
    });

    // ── FOLD, strictly in photo order ────────────────────────────────────────
    // `f64` addition is not associative, so summing in completion order would
    // move the printed gap score in the last ulp depending on which photo
    // finished first. Folding the positional results reproduces the serial
    // arithmetic exactly, at any `--jobs`.
    let mut abort: Option<String> = None;
    // Provenance of the table below, counted while it is folded.
    let (mut loaded_rows, mut measured_now) = (0u32, 0u32);
    for (i, outcome) in outcomes.into_iter().enumerate() {
        let Some(outcome) = outcome else { continue };
        // A restored photograph and a freshly measured one reach the fold the
        // SAME way, at the SAME index — that is what makes a resumed run's
        // arithmetic identical to an uninterrupted one's, whatever the split.
        let stats = match outcome {
            PhotoOutcome::Stats(s) => Some((s, false)),
            PhotoOutcome::Restored => restored[i].take().map(|s| (s, true)),
            // Reported in its own block; the run continues (unchanged).
            PhotoOutcome::Failed | PhotoOutcome::Skipped => None,
            // The FIRST hard error wins, so the message a `--jobs 1` run would
            // have raised is the message this run raises.
            PhotoOutcome::Aborted(why) => {
                if abort.is_none() {
                    abort = Some(why);
                }
                None
            }
        };
        let Some((s, from_state)) = stats else { continue };
        if from_state {
            loaded_rows += 1;
        } else {
            measured_now += 1;
        }
        for (metric, one) in &s.acc {
            acc.entry(metric.clone()).or_default().merge(one);
        }
        if let Some((rmse, ul, al, us, as_)) = s.curve {
            curve_n += 1;
            sum_curve_rmse += rmse;
            sum_user_lift += ul;
            sum_ai_lift += al;
            sum_user_s += us;
            sum_ai_s += as_;
        }
        for (ch, r) in s.rgb.iter().enumerate() {
            if let Some(rmse) = r {
                rgb_acc[ch].0 += 1;
                rgb_acc[ch].1 += rmse;
            }
        }
        if let Some((imported, refused, ai_n)) = s.masks {
            mask_photos += 1;
            user_masks += imported;
            user_masks_unsupported += refused;
            ai_masks += ai_n;
        }
        evaluated += 1;
    }
    if let Some(why) = abort {
        anyhow::bail!("{why}");
    }

    if evaluated == 0 {
        // Attempted pairs that ALL failed must not exit 0 — broken
        // credentials looked like a clean run to CI (16-lane scan L09).
        if n > 0 {
            anyhow::bail!("no photo evaluated — every attempted pair failed (see the FAILED lines above)");
        }
        println!("No photos evaluated.");
        return Ok(());
    }

    println!("\n=== AI vs your edits ({evaluated} photo(s)) ===");
    // WHERE the rows came from, before the numbers they produced: a table
    // built partly out of saved progress must say so, and a fallback row —
    // counted here, deliberately NOT saved — is the reason a rerun's photo
    // count can move.
    println!(
        "{evaluated} row(s) total: {loaded_rows} loaded from saved progress, {measured_now} \
         measured this run, {} fallback row(s) NOT persisted.",
        fallbacks.load(std::sync::atomic::Ordering::Relaxed)
    );
    // …and WHY that count is as low as it is: a run that spent repeats is not
    // the same run as one that never needed them, and the fallback number
    // above is the number those repeats were bought to hold down.
    if let Some(line) = retry_disclosure(crate::advisor::retry_tally().since(retries_before)) {
        println!("{line}");
    }
    println!("progress file: {}", state_path.display());
    println!("{:<22} {:>4} {:>10} {:>13} {:>8}", "field", "n", "mean|Δ|", "bias(AI−you)", "AI-omit");
    print!("{}", format_scalar_report_rows(&ruler, &acc));

    // --- master tone curve summary -------------------------------------------
    if curve_n > 0 {
        let avg = |s: f64| s / curve_n as f64;
        println!("\n=== Master tone curve ({curve_n} photo(s) where you or the AI drew one) ===");
        println!("  black lift (output@0):  you {:>6.1}   AI {:>6.1}", avg(sum_user_lift), avg(sum_ai_lift));
        println!("  S-strength (contrast):  you {:>6.1}   AI {:>6.1}", avg(sum_user_s), avg(sum_ai_s));
        println!("  curve RMSE (AI vs you): {:>6.1}   (0 = identical, on the 0..255 scale)", avg(sum_curve_rmse));
        if avg(sum_user_s).abs() > 4.0 && avg(sum_ai_s).abs() < avg(sum_user_s).abs() * 0.5 {
            println!("  → the AI's curve is much flatter than yours: it is omitting the S-curve that gives your photos their contrast.");
        }
    } else {
        println!("\n(no master tone curve in your XMPs to compare)");
    }

    // --- per-channel RGB curves + local masks (R23-1 ruler expansion) --------
    if rgb_acc.iter().any(|(n, _)| *n > 0) {
        println!("\n=== Per-channel RGB curves (photo(s) where you or the AI drew one) ===");
        for (ch, (name, _)) in rgb_curves.iter().enumerate() {
            let (n, sum) = rgb_acc[ch];
            if n > 0 {
                println!("  {name:<6} n {n:>3}   curve RMSE (AI vs you): {:>6.1}", sum / n as f64);
            }
        }
    } else {
        println!("\n(no per-channel RGB curves on either side)");
    }
    if mask_photos > 0 {
        let per = |v: usize| v as f64 / mask_photos as f64;
        println!("\n=== Local masks ({mask_photos} photo(s) with masks on either side) ===");
        println!(
            "  per photo: you {:.2} (+{:.2} our importer refuses)   AI {:.2}",
            per(user_masks),
            per(user_masks_unsupported),
            per(ai_masks)
        );
        println!("  (a COUNT, not a divergence — deliberately not folded into the gap score)");
    } else {
        println!("\n(no local masks on either side)");
    }

    // --- aggregate gap score -------------------------------------------------
    // Mean fractional divergence across the controls you used, each MAE
    // normalised by a sensible full-scale, plus the tone-curve terms. One number
    // to watch move as the advisor improves.
    //
    // NOT comparable with a pre-R23-1 score: the ruler now covers the HSL
    // mixer, the colour-grade wheels and the three channel curves, so controls
    // the AI never touched (and that were invisible before) now weigh in.
    let (mut frac_sum, mut frac_n) = (0f64, 0u32);
    for row in &ruler {
        if let Some(a) = acc.get(&row.metric)
            && a.n > 0
        {
            frac_sum += (a.sum_abs / a.n as f64) / row.full_scale();
            frac_n += 1;
        }
    }
    if curve_n > 0 {
        frac_sum += (sum_curve_rmse / curve_n as f64) / 255.0;
        frac_n += 1;
    }
    for (n, sum) in rgb_acc {
        if n > 0 {
            frac_sum += (sum / n as f64) / 255.0;
            frac_n += 1;
        }
    }
    // n/a, not 0.0%: with nothing measured, a perfect score claimed the AI
    // matched a look that was never compared.
    if frac_n > 0 {
        let gap = 100.0 * frac_sum / frac_n as f64;
        println!(
            "\nOverall gap score: {gap:.1}%  (mean per-control divergence incl. tone curve; lower = closer to your look)"
        );
        if let Some(weighted) = supplementary_weighted_gap(&ruler, &acc, curve_n, sum_curve_rmse, &rgb_acc) {
            print!("{}", format_supplementary_gap_line(weighted));
        }
    } else {
        println!(
            "\nOverall gap score: n/a — no comparable controls were measured (the XMPs may hold \
             no readable crs settings)"
        );
    }
    println!(
        "Interpretation: positive bias = AI sets this higher than you do; large mean|Δ| = you \
         disagree a lot on that control; AI-omit = times you used a control the AI ignored. Use \
         these to calibrate the advisor prompt."
    );
    // The one row whose MEANING changed, disclosed in the report itself rather
    // than only in the ledger — a reader comparing this transcript against the
    // R27 147-photo baseline has to know the population moved (R28 Batch-5 5b,
    // `hue_carries_colour`).
    println!(
        "Note: the four color_grade.*_hue rows count a photo only when BOTH sides put saturation \
         on that wheel (a hue at zero saturation renders nothing). Their n / mean|Δ| / AI-omit are \
         NOT comparable with a pre-v0.34.0 run; every other row is unchanged."
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // The ESTIMATOR PROBE (M1_PLAN §8 #12)
    //
    // Three numbers this codebase computes about a photograph without asking
    // any model — and which M1 labelled "estimates, not measurements" and
    // never checked against anything:
    //
    //   E1  EV-offset       `advisor::HeuristicProposer::ev_offset_estimate`
    //   E2  as-shot WB      `render::as_shot_wb` (McCamy CCT + Krystek Duv)
    //   E3  gray-world WB   `render::solve_wb_from_neutral`, fed the frame's
    //                       own mean RGB — the "gray-world-on-neutrals"
    //                       heuristic of the same row, assembled from the
    //                       shipped solver rather than reimplemented
    //
    // The ground truth is the photographer's real sidecar. The three do NOT
    // share a truth standard, and the report says so per estimator, because
    // the comparisons are not equally clean:
    //
    //   * E2 is a MEASUREMENT. Under `WhiteBalance="As Shot"` Adobe writes
    //     its OWN reading of the camera's as-shot illuminant into
    //     `crs:Temperature`/`crs:Tint`. Two programs, one file, one physical
    //     quantity — a disagreement is our estimator being wrong, full stop.
    //   * E1 and E3 are ESTIMATOR-vs-TASTE. The photographer's
    //     `crs:Exposure2012` is not "the correct EV offset", it is what they
    //     wanted; and their custom WB is a choice, not the illuminant. A weak
    //     correlation there is a statement about how much of the user's
    //     intent a content-blind heuristic can predict — which is precisely
    //     what a fallback baseline is allowed to be — not a defect.
    // ---------------------------------------------------------------------

    /// One estimator's scatter against the photographer's own values.
    #[derive(Default)]
    struct Scatter {
        /// `(stem, estimate, user value)`, one row per photograph.
        pts: Vec<(String, f64, f64)>,
    }

    /// Mean of a sample, or 0 for an empty one.
    fn mean(v: &[f64]) -> f64 {
        if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
    }

    /// POPULATION standard deviation (the scatter is the whole cohort, not a
    /// sample drawn from a larger one).
    fn sd(v: &[f64]) -> f64 {
        let m = mean(v);
        (mean(&v.iter().map(|x| (x - m) * (x - m)).collect::<Vec<_>>())).sqrt()
    }

    impl Scatter {
        fn push(&mut self, stem: &str, est: f64, user: f64) {
            self.pts.push((stem.to_string(), est, user));
        }

        /// Print `n`, Pearson r, OLS slope/intercept (estimate regressed ON
        /// the user's value), signed bias, MAE, both spreads, and the worst
        /// rows by |estimate − user|.
        ///
        /// r and the slope are printed as `n/a` when either side has no
        /// spread — a correlation against a constant column is 0/0, and
        /// printing a number there would invent a finding.
        fn report(&self, title: &str, truth: &str, unit: &str) {
            let (est, user): (Vec<f64>, Vec<f64>) =
                self.pts.iter().map(|(_, e, u)| (*e, *u)).unzip();
            println!("\n--- {title}\n    truth = {truth}");
            if est.is_empty() {
                println!("    n=0 — no photograph in this cohort");
                return;
            }
            let (se, su) = (sd(&est), sd(&user));
            let (me, mu) = (mean(&est), mean(&user));
            let cov = mean(
                &self
                    .pts
                    .iter()
                    .map(|(_, e, u)| (e - me) * (u - mu))
                    .collect::<Vec<_>>(),
            );
            let fmt = |v: f64| {
                if v.is_finite() { format!("{v:+.3}") } else { "n/a".to_string() }
            };
            let (r, slope, intercept) = if se > 1e-9 && su > 1e-9 {
                let sl = cov / (su * su);
                (cov / (se * su), sl, me - sl * mu)
            } else {
                (f64::NAN, f64::NAN, f64::NAN)
            };
            let errs: Vec<f64> = self.pts.iter().map(|(_, e, u)| e - u).collect();
            println!(
                "    n={:<4} r={:<8} slope={:<8} intercept={:<8}  [{unit}]",
                self.pts.len(),
                fmt(r),
                fmt(slope),
                fmt(intercept)
            );
            println!(
                "    bias(est−user)={:<8} MAE={:<8.3} mean: est={:<8} user={:<8}  sd: est={:<8.3} user={:.3}",
                fmt(mean(&errs)),
                mean(&errs.iter().map(|e| e.abs()).collect::<Vec<_>>()),
                fmt(me),
                fmt(mu),
                se,
                su
            );
            let mut worst = self.pts.clone();
            worst.sort_by(|a, b| {
                (b.1 - b.2).abs().partial_cmp(&(a.1 - a.2).abs()).unwrap_or(std::cmp::Ordering::Equal)
            });
            let names: Vec<String> = worst
                .iter()
                .take(5)
                .map(|(s, e, u)| format!("{s} (est {e:+.2} vs {u:+.2})"))
                .collect();
            println!("    worst 5 by |est−user|: {}", names.join(" · "));
        }
    }

    /// Mean of one 256-bin channel histogram, on the 0..1 scale
    /// `render::solve_wb_from_neutral` reads sRGB pixels in.
    fn channel_mean_01(bins: &[u32]) -> f32 {
        let total: u64 = bins.iter().map(|&v| v as u64).sum::<u64>().max(1);
        let weighted: u64 = bins.iter().enumerate().map(|(i, &v)| i as u64 * v as u64).sum();
        (weighted as f32 / total as f32) / 255.0
    }

    /// **M1_PLAN §8 #12** — measure the three no-model estimators against the
    /// photographer's own sliders over a real RAW + `.xmp` library.
    ///
    /// Point `AUTOSHADE_ESTIMATOR_PROBE` at a folder scanned recursively for
    /// RAW files with a sibling `.xmp` (the same pair rule [`run`] uses, via
    /// the same `pipeline::find_raws`), and run:
    ///
    /// ```text
    /// AUTOSHADE_ESTIMATOR_PROBE=<dir> cargo test --lib -- --ignored --nocapture estimator
    /// ```
    ///
    /// `AUTOSHADE_ESTIMATOR_PROBE_LIMIT` caps the pair count for a quick pass.
    ///
    /// It costs NO API call and writes nothing — it decodes each RAW and
    /// reads each sidecar, both read-only. It is `#[ignore]`d rather than
    /// silently no-op because an unset probe that returns green measures
    /// nothing while looking like it measured something (the L-29 risk), and
    /// it PANICS on a decode failure rather than skipping the file: a
    /// forensic probe whose specimens quietly stop arriving is worse than no
    /// probe.
    ///
    /// It asserts only what a probe honestly can — every pair decodes, every
    /// cohort is non-empty, every printed statistic is finite. Deliberately
    /// NO correlation threshold: nobody has set an acceptance bar for these
    /// heuristics, and inventing one here would turn a measurement into a
    /// gate the user never agreed to.
    ///
    /// MUTATION THIS CATCHES: it is a report, and the estimator identities it
    /// reports on are pinned by unit tests next to each estimator
    /// (`ev_offset_estimator_is_signed_stops_and_feeds_the_recipe`,
    /// `wb_eyedropper_neutralizes_a_synthetic_cast`). What THIS body catches
    /// is the plumbing: point E2 at `crs:Tint` instead of `crs:Temperature`
    /// and the Kelvin cohort's finiteness assertion still passes but `n`
    /// collapses to the handful of sidecars carrying one and not the other;
    /// drop the `crs_own_scope` call and a creative Look's baked temperature
    /// is scored as the photographer's own, exactly the defect that scoping
    /// exists to prevent.
    #[test]
    #[ignore = "forensic probe: needs AUTOSHADE_ESTIMATOR_PROBE=<dir of RAW+.xmp pairs>; decodes every pair (minutes)"]
    fn estimators_against_the_photographers_own_sliders() {
        let Some(dir) = crate::config::live_env("AUTOSHADE_ESTIMATOR_PROBE") else {
            panic!(
                "set AUTOSHADE_ESTIMATOR_PROBE to a folder of RAW files with sibling .xmp \
                 sidecars (the eval corpus)"
            );
        };
        let root = std::path::PathBuf::from(&dir);
        assert!(root.is_dir(), "AUTOSHADE_ESTIMATOR_PROBE is not a directory: {dir}");
        let limit: usize = crate::config::live_env("AUTOSHADE_ESTIMATOR_PROBE_LIMIT")
            .and_then(|s| s.parse().ok())
            .unwrap_or(usize::MAX);
        let raws = pipeline::find_raws(&root).expect("scan the probe directory");
        let pairing = crate::xmp_pair::XmpPairing::new(&root, None);
        let pairs: Vec<(&std::path::PathBuf, std::path::PathBuf)> = raws
            .iter()
            .filter_map(|r| pairing.find(r).map(|x| (r, x)))
            .take(limit)
            .collect();
        assert!(
            !pairs.is_empty(),
            "AUTOSHADE_ESTIMATOR_PROBE ({dir}) holds no RAW with a sibling .xmp"
        );

        let (mut ev, mut wb_k, mut wb_t) = (Scatter::default(), Scatter::default(), Scatter::default());
        let (mut gw_k_all, mut gw_t_all) = (Scatter::default(), Scatter::default());
        let (mut gw_k_custom, mut gw_t_custom) = (Scatter::default(), Scatter::default());
        let (mut as_shot_photos, mut custom_wb_photos, mut no_anchor) = (0u32, 0u32, 0u32);

        for (raw, sidecar) in &pairs {
            let stem = pipeline::stem(raw);
            let xmp_text = crate::store::read_sidecar(sidecar)
                .unwrap_or_else(|| panic!("sidecar for {stem} is missing or unreadable"));
            // The Description's OWN scope — the same rule `run` applies, and
            // for the same reason: a creative Look bakes its own crs values.
            let scope = crate::xmp::crs_own_scope(&xmp_text);
            let scope = Scope::new(scope.as_ref());
            let user_wb = scope.crs_str("WhiteBalance").as_deref() != Some("As Shot");
            let decoded = crate::decode::decode_any(raw)
                .unwrap_or_else(|e| panic!("decode {stem}: {e:#}"));
            let hist = &decoded.histogram;

            // --- E1: EV offset vs the photographer's exposure slider -------
            // An absent Exposure2012 means they left it at 0, the same
            // neutral rule the ruler above uses.
            let est_ev = crate::advisor::HeuristicProposer::ev_offset_estimate(hist);
            ev.push(stem, est_ev as f64, scope.crs_f32("Exposure2012").unwrap_or(0.0) as f64);

            // --- E2: as-shot WB vs Adobe's own as-shot reading -------------
            let as_shot = crate::render::as_shot_wb(raw);
            if let Some((k, tint)) = as_shot {
                if user_wb {
                    custom_wb_photos += 1;
                } else {
                    // Adobe's reading of the SAME camera metadata. Only
                    // meaningful while the photographer left WB as-shot;
                    // once they drag Temp, crs:Temperature is their taste.
                    as_shot_photos += 1;
                    if let Some(t) = scope.crs_f32("Temperature") {
                        wb_k.push(stem, k as f64, t as f64);
                    }
                    if let Some(t) = scope.crs_f32("Tint") {
                        wb_t.push(stem, tint as f64, t as f64);
                    }
                }
            } else {
                no_anchor += 1;
            }

            // --- E3: gray-world on the frame's own mean --------------------
            // "This frame averages to neutral" is the classic gray-world
            // assumption; the shipped eyedropper solver turns that assumed
            // neutral into the (K, tint) the render would need. Anchored on
            // the photo's own as-shot Kelvin (the engine's 5500 K fallback
            // when the file has none), exactly like the eyedropper.
            let anchor = as_shot.map(|(k, _)| k).unwrap_or(5500.0);
            let px = [
                channel_mean_01(&hist.r),
                channel_mean_01(&hist.g),
                channel_mean_01(&hist.b),
            ];
            let (gk, gt) = crate::render::solve_wb_from_neutral(px, anchor);
            let (uk, ut) = (scope.crs_f32("Temperature"), scope.crs_f32("Tint"));
            if let Some(t) = uk {
                gw_k_all.push(stem, gk as f64, t as f64);
                if user_wb {
                    gw_k_custom.push(stem, gk as f64, t as f64);
                }
            }
            if let Some(t) = ut {
                gw_t_all.push(stem, gt as f64, t as f64);
                if user_wb {
                    gw_t_custom.push(stem, gt as f64, t as f64);
                }
            }
        }

        println!(
            "\n=== Estimator probe (M1_PLAN §8 #12) — {} RAW+.xmp pair(s) under {dir} ===\n\
             as-shot WB kept: {as_shot_photos} · custom WB set: {custom_wb_photos} · \
             no as-shot anchor readable: {no_anchor}",
            pairs.len()
        );
        ev.report(
            "E1 EV-offset — advisor::HeuristicProposer::ev_offset_estimate (raw, pre-clamp)",
            "crs:Exposure2012 (the photographer's TASTE, not a correct answer)",
            "stops",
        );
        wb_k.report(
            "E2 as-shot Kelvin — render::as_shot_wb (McCamy CCT)",
            "crs:Temperature under WhiteBalance=\"As Shot\" (Adobe's own reading — a MEASUREMENT)",
            "K",
        );
        wb_t.report(
            "E2 as-shot tint — render::as_shot_wb (Krystek Duv × 3000)",
            "crs:Tint under WhiteBalance=\"As Shot\" (Adobe's own reading — a MEASUREMENT)",
            "tint units",
        );
        gw_k_all.report(
            "E3 gray-world Kelvin — render::solve_wb_from_neutral(frame mean)",
            "crs:Temperature, every photograph",
            "K",
        );
        gw_t_all.report(
            "E3 gray-world tint — render::solve_wb_from_neutral(frame mean)",
            "crs:Tint, every photograph",
            "tint units",
        );
        gw_k_custom.report(
            "E3 gray-world Kelvin — custom-WB cohort only",
            "crs:Temperature where the photographer moved WB off As Shot",
            "K",
        );
        gw_t_custom.report(
            "E3 gray-world tint — custom-WB cohort only",
            "crs:Tint where the photographer moved WB off As Shot",
            "tint units",
        );

        // Every cohort measured something, and every number printed above is
        // real: a NaN estimate would print as a plausible-looking row.
        assert!(!ev.pts.is_empty(), "E1 measured nothing");
        assert!(!wb_k.pts.is_empty(), "E2 measured nothing — no as-shot sidecar in the corpus");
        assert!(!gw_k_all.pts.is_empty(), "E3 measured nothing");
        for s in [&ev, &wb_k, &wb_t, &gw_k_all, &gw_t_all, &gw_k_custom, &gw_t_custom] {
            for (stem, e, u) in &s.pts {
                assert!(e.is_finite() && u.is_finite(), "non-finite row for {stem}: {e} vs {u}");
            }
        }
    }

    // Values copied from the user's real P28.xmp (read this session).
    const SAMPLE: &str = r#"<rdf:Description
        crs:Temperature="5650" crs:Tint="+13" crs:Exposure2012="0.00"
        crs:Contrast2012="+22" crs:Highlights2012="+7" crs:Shadows2012="-6"
        crs:Whites2012="0" crs:Blacks2012="0" crs:Clarity2012="0" crs:Dehaze="+18"
        crs:Vibrance="+5" crs:Saturation="+13" crs:Sharpness="40"
        crs:LuminanceSmoothing="0">"#;

    /// One row's user value out of a sidecar, by metric name (the ruler is
    /// derived, so the test looks rows up instead of indexing a fixed list).
    fn get(xmp: &str, metric: &str) -> Option<f32> {
        let ruler = rows();
        let row = ruler
            .iter()
            .find(|r| r.metric == metric)
            .unwrap_or_else(|| panic!("{metric} is not a ruler row"));
        let xmp = Scope::new(xmp);
        let user_wb = xmp.crs_str("WhiteBalance").as_deref() != Some("As Shot");
        let has_crop = xmp.crs_str("HasCrop").as_deref() == Some("True");
        user_value(row, xmp, user_wb, has_crop)
    }

    #[test]
    fn parses_real_crs_values() {
        assert_eq!(get(SAMPLE, "exposure_ev"), Some(0.0));
        assert_eq!(get(SAMPLE, "contrast"), Some(22.0));
        assert_eq!(get(SAMPLE, "shadows"), Some(-6.0));
        assert_eq!(get(SAMPLE, "dehaze"), Some(18.0));
        assert_eq!(get(SAMPLE, "temperature_k"), Some(5650.0));
        // v0.31.1: 40, not 60. The ruler used to multiply the user's
        // `crs:Sharpness` by 1.5 to reach "recipe units"; `crs:Sharpness` IS
        // recipe units. Every earlier `sharpening` row in an eval report —
        // the R25 M-C 147-photo baseline included — read the user 1.5× high.
        assert_eq!(get(SAMPLE, "sharpening"), Some(40.0));
        assert_eq!(Scope::new(SAMPLE).crs_f32("Nonexistent"), None);
    }

    /// R23-1: the ruler is DERIVED from the control registry, so the two
    /// hand-kept 14-row tables (the parse list and the report order, whose
    /// comment claimed they matched) are gone — and the controls the old ruler
    /// was blind to are in.
    #[test]
    fn the_ruler_covers_every_ai_visible_attribute_and_both_families() {
        let ruler = rows();
        let metric = |m: &str| ruler.iter().any(|r| r.metric == m);
        // Every AI-visible registry row that lands on one crs attribute.
        for c in RECIPE_CONTROLS.iter().filter(|c| !c.engine_only) {
            if c.crs.attr().is_some() {
                assert!(metric(c.name), "{} has a crs attribute but no ruler row", c.name);
            }
        }
        // Engine-only controls stay OUT: the AI cannot set them, so scoring it
        // against them would measure the schema gap, not the taste gap. (Read
        // off the registry, not a name — R23-1b moved the lens trio to the
        // AI-visible side and this assertion had to move with it or be a lie.)
        for c in RECIPE_CONTROLS.iter().filter(|c| c.engine_only) {
            assert!(!metric(c.name), "{} is engine-only and must not be scored", c.name);
        }
        // …and the trio that CROSSED that line is measured now: the ruler was
        // blind to the manual lens corrections for exactly as long as the
        // schema was.
        for m in ["lens_vignette", "lens_vignette_mid", "lens_distortion"] {
            assert!(metric(m), "{m} is in the schema now and must be scored");
        }
        // The two families expand (24 + 14) — the blind spot #12 reported.
        assert_eq!(ruler.iter().filter(|r| r.metric.starts_with("hsl.")).count(), 24);
        assert_eq!(ruler.iter().filter(|r| r.metric.starts_with("color_grade.")).count(), 14);
        assert!(metric("hsl.saturation.blue") && metric("color_grade.shadow_sat"));
        // straighten_deg entered the ruler with its Adobe gate: an angle from a
        // DISABLED crop is not a straighten the photographer asked for.
        let angle = r#"<rdf:Description crs:CropAngle="-2.5" crs:HasCrop="False">"#;
        assert_eq!(get(angle, "straighten_deg"), None);
        let angle_on = r#"<rdf:Description crs:CropAngle="-2.5" crs:HasCrop="True">"#;
        assert_eq!(get(angle_on, "straighten_deg"), Some(-2.5));

        // Family cells read the recipe through the registry's own accessors.
        let mut ai = EditRecipe::default();
        ai.hsl.saturation[5] = -30.0; // blue
        ai.color_grade.shadow_sat = 20.0;
        ai.color_grade.shadow_hue = 210.0;
        let row = |m: &str| ruler.iter().find(|r| r.metric == m).unwrap().clone();
        assert_eq!(ai_value(&row("hsl.saturation.blue"), &ai), Some(-30.0));
        assert_eq!(ai_value(&row("color_grade.shadow_sat"), &ai), Some(20.0));

        // Hue is CIRCULAR: 350° vs 10° is 20° apart, not 340°.
        let hue = row("color_grade.shadow_hue");
        assert!(hue.circular, "a hue wheel must be circular");
        assert_eq!(hue.diff(10.0, 350.0), 20.0);
        assert_eq!(hue.diff(350.0, 10.0), -20.0);
        assert_eq!(hue.full_scale(), 180.0);

        // Neutral is READ, not assumed: blending sits at 50 untouched, so a
        // zero-based "used" test called every neutral wheel a ±50 divergence.
        assert_eq!(neutral(&row("color_grade.blending"), &EditRecipe::default()), 50.0);
        assert_eq!(neutral(&row("contrast"), &EditRecipe::default()), 0.0);
    }

    /// R28 Batch-5 5b — THE 141° ARTIFACT, reproduced and closed.
    ///
    /// The inspection pack's reading of the R27 147-photo baseline:
    /// `color_grade.shadow_hue  mean|Δ| = 141`. This is that number's
    /// mechanism, in the smallest form that produces it — a photographer's
    /// sidecar and an AI recipe that BOTH leave the shadow wheel colourless and
    /// merely park its angle at opposite ends of the circle. 210° against 351°
    /// is 141° of "disagreement" about a hue neither side rendered.
    ///
    /// Revert `hue_carries_colour` to `true` (or drop the `row.circular` gate in
    /// `run`) and the first assertion fails: that is the mutation proof.
    #[test]
    fn a_hue_on_a_colourless_wheel_is_not_a_measurement() {
        let ruler = rows();
        let row = |m: &str| ruler.iter().find(|r| r.metric == m).unwrap().clone();
        let hue = row("color_grade.shadow_hue");

        // The artifact itself: the delta the old row counted, in full. The
        // circular wrap is doing its job here — this is not a 141° bug in
        // `diff`, it is a real angular distance between two angles that mean
        // nothing, which is exactly why the fix is upstream of the subtraction.
        assert_eq!(hue.diff(351.0, 210.0).abs(), 141.0, "the ledger'd 141° is this subtraction");

        // Both wheels colourless — the sidecar states a hue, the AI states
        // another, and NOTHING is painted by either.
        let colourless = Scope::new(r#"<rdf:Description crs:SplitToningShadowHue="210" crs:SplitToningShadowSaturation="0">"#);
        let mut ai = EditRecipe::default();
        ai.color_grade.shadow_hue = 351.0;
        assert!(
            !hue_carries_colour(&hue, colourless, &ai),
            "a hue delta between two zero-saturation wheels is not a comparison"
        );

        // ONE side toning is still not a hue comparison — the omission belongs
        // to (and is counted on) the saturation row, which is where the
        // photographer's decision actually lives.
        let user_tones = Scope::new(r#"<rdf:Description crs:SplitToningShadowHue="210" crs:SplitToningShadowSaturation="35">"#);
        assert!(!hue_carries_colour(&hue, user_tones, &ai), "user-only toning: no hue to compare");
        ai.color_grade.shadow_sat = 22.0;
        assert!(
            !hue_carries_colour(&hue, colourless, &ai),
            "AI-only toning: still no hue to compare"
        );

        // BOTH sides painting → the row measures a real disagreement again.
        assert!(
            hue_carries_colour(&hue, user_tones, &ai),
            "two saturated wheels DO disagree about hue, and that must still count"
        );

        // The deadband is the ruler's own, not a second number: a wheel moved
        // by less than `eps_for` has not been moved.
        let dust = Scope::new(r#"<rdf:Description crs:SplitToningShadowHue="210" crs:SplitToningShadowSaturation="0.4">"#);
        assert!(!hue_carries_colour(&hue, dust, &ai), "0.4 is inside the ruler's own deadband");

        // Every non-hue row is untouched — including the HSL "hue" cells, which
        // are ±100 shift sliders and not wheel angles.
        assert!(hue_carries_colour(&row("hsl.hue.blue"), colourless, &ai));
        assert!(hue_carries_colour(&row("contrast"), colourless, &ai));
        assert!(hue_carries_colour(&row("color_grade.shadow_sat"), colourless, &ai));
    }

    // A user S-curve: black lifted to 12, quarter-shadow pulled down (64→50),
    // quarter-highlight pushed up (191→210), white pinned.
    const CURVE_XMP: &str = r#"<crs:ToneCurvePV2012>
 <rdf:Seq>
  <rdf:li>0, 12</rdf:li>
  <rdf:li>64, 50</rdf:li>
  <rdf:li>191, 210</rdf:li>
  <rdf:li>255, 255</rdf:li>
 </rdf:Seq>
</crs:ToneCurvePV2012>"#;

    #[test]
    fn curve_helpers_follow_the_renderers_rules() {
        // Duplicate input: FIRST point wins (render::curve_lut's rule) — no
        // cliff to the later twin's height just past the duplicate.
        let lut = curve_lut(&[(0.0, 0.0), (128.0, 200.0), (128.0, 60.0), (255.0, 255.0)]);
        assert!((lut[128] - 200.0).abs() < 0.5, "first duplicate wins: {}", lut[128]);
        assert!(lut[129] > 190.0, "no cliff after the duplicate: {}", lut[129]);
        // A ONE-point user curve is a real habit, not "no curve".
        let one = r#"<crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 30</rdf:li></rdf:Seq></crs:ToneCurvePV2012>"#;
        let (lift, _s) = user_curve_shape(one).expect("one-point curve counts");
        assert!(lift > 25.0, "black lift read from the single point: {lift}");
    }

    #[test]
    fn parses_tone_curve_and_measures_shape() {
        let pts = parse_tone_curve(CURVE_XMP, "ToneCurvePV2012");
        assert_eq!(pts.len(), 4);
        assert_eq!(pts[0], (0.0, 12.0));
        assert_eq!(pts[2], (191.0, 210.0));

        let lut = curve_lut(&pts);
        assert!((lut[0] - 12.0).abs() < 0.5, "black lifted to 12: {}", lut[0]);
        assert!((lut[255] - 255.0).abs() < 0.5, "white pinned: {}", lut[255]);
        assert!(curve_black_lift(&lut) > 10.0, "reads the black lift");
        // S-strength = (210-191) - (50-64) = 19 - (-14) = 33 > 0 (an S).
        assert!(curve_s_strength(&lut) > 20.0, "reads as an S: {}", curve_s_strength(&lut));

        // Identity curve has ~0 lift and ~0 strength, and differs from the S.
        let id = curve_lut(&[]);
        assert!(curve_s_strength(&id).abs() < 0.5);
        assert!(curve_rmse(&lut, &id) > 5.0, "an S-curve is far from identity");

        // Absent tag → empty (so the eval simply skips the curve comparison).
        assert!(parse_tone_curve("<x:y/>", "ToneCurvePV2012").is_empty());
    }

    #[test]
    fn one_sided_controls_use_neutral_and_stamped_effective_values() {
        let ai = EditRecipe {
            contrast: 25.0,
            as_shot_k: Some(4800.0),
            as_shot_tint: Some(7.0),
            tint: 3.0,
            ..Default::default()
        };
        let ruler = rows();
        let row = |m: &str| ruler.iter().find(|r| r.metric == m).unwrap().clone();

        // An absent slider means the photographer left it at its neutral; an
        // absent white balance means they kept as-shot (through the recipe's
        // own stamp, with the engine's historical 5500 K fallback).
        assert_eq!(neutral(&row("contrast"), &ai), 0.0);
        assert_eq!(ai_value(&row("contrast"), &ai), Some(25.0));
        assert_eq!(neutral(&row("temperature_k"), &ai), 4800.0);
        assert_eq!(ai_value(&row("temperature_k"), &ai), Some(4800.0));
        assert_eq!(neutral(&row("tint"), &ai), 7.0);
        // The engine's tint is a DELTA from as-shot; ACR's is absolute.
        assert_eq!(ai_value(&row("tint"), &ai), Some(10.0));
    }

    // ── resumable progress ────────────────────────────────────────────────
    //
    // The arm-2 tests of "a single mid-run failure forfeits the whole run".
    // None of them touch a network: the state file, its header guard, the
    // resume decision and the fallback rule are all measurable without one,
    // which is the point of keeping them out of the pool body.

    /// A scratch folder of this test's own, removed on the way out.
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir()
            .join(format!("autoshade-eval-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch dir");
        d
    }

    /// The premise of the stale-sidecar guard, asserted where the guard lives.
    ///
    /// The published FIPS 180-4 vectors moved to `crate::sha256` with the
    /// implementation — one known-answer test for one hash. What is eval's own
    /// is the property those vectors do not state: that the digest DISCRIMINATES,
    /// so a saved measurement stops matching the moment its sidecar changes by
    /// one byte.
    ///
    /// MUTATION THIS CATCHES: a digest that ignores its input (a constant, or a
    /// length-only hash) — which would leave every stale measurement looking
    /// current for ever, while `sha256_matches_the_fips_180_4_vectors` in the
    /// hash's own module catches the mistyped-constant class.
    #[test]
    fn a_one_byte_edit_changes_the_state_files_digest() {
        // The published vectors moved with the implementation to
        // `crate::sha256`; what stays here is the property THIS module's
        // stale-sidecar guard actually rests on — that a one-byte edit
        // produces a different digest, so a saved measurement stops matching
        // the sidecar it was taken from the moment that sidecar changes.
        assert_ne!(sha256_hex(b"abc"), sha256_hex(b"abd"));
    }

    /// R28 item 5: the report must not be able to tell which side a
    /// photograph came from. Aggregates are sums over per-photo stats, so this
    /// is a statement about the ROUND TRIP being exact, and it is asserted bit
    /// for bit rather than approximately — the whole reason the fold runs in
    /// index order is that the last ulp is allowed to matter here.
    ///
    /// MUTATION THIS CATCHES: putting `#[serde(default)]` on `PhotoStats` (the
    /// last two assertions go green-then-red — a truncated row would silently
    /// deserialise into an all-zero contribution that claims the photograph was
    /// measured), or serialising the sums through `f32`.
    #[test]
    fn a_saved_row_folds_exactly_like_the_measurement_it_replaces() {
        let mut fresh = PhotoStats::default();
        fresh.acc.insert(
            "contrast".into(),
            Acc { sum_abs: 12.5, sum_signed: -12.5, n: 1, omit: 0 },
        );
        // Deliberately a value with no short decimal form.
        fresh.acc.insert(
            "exposure_ev".into(),
            Acc { sum_abs: 0.1 + 0.2, sum_signed: -(0.1 + 0.2), n: 1, omit: 1 },
        );
        fresh.curve = Some((3.5, 12.25, 0.0, 33.0, 1.0));
        fresh.rgb = [Some(1.5), None, Some(2.5)];
        fresh.masks = Some((2, 1, 0));

        let row = StateRow {
            stem: "P28".into(),
            rel: "shoot/P28.ARW".into(),
            xmp_sha256: sha256_hex(b"<x/>"),
            stats: fresh.clone(),
        };
        let line = serde_json::to_string(&row).expect("a row serialises");
        assert!(!line.contains('\n'), "one photograph is one line: {line}");
        let loaded: StateRow = serde_json::from_str(&line).expect("a row round-trips");

        let fold = |s: &PhotoStats| {
            let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
            for (m, one) in &s.acc {
                acc.entry(m.clone()).or_default().merge(one);
            }
            acc
        };
        let (a, b) = (fold(&fresh), fold(&loaded.stats));
        assert_eq!(a.len(), b.len(), "the same controls");
        for ((ka, va), (kb, vb)) in a.iter().zip(b.iter()) {
            assert_eq!(ka, kb);
            assert_eq!(va.sum_abs.to_bits(), vb.sum_abs.to_bits(), "{ka} mean|Δ|");
            assert_eq!(va.sum_signed.to_bits(), vb.sum_signed.to_bits(), "{ka} bias");
            assert_eq!((va.n, va.omit), (vb.n, vb.omit), "{ka} counts");
        }
        assert_eq!(fresh.curve, loaded.stats.curve);
        assert_eq!(fresh.rgb, loaded.stats.rgb);
        assert_eq!(fresh.masks, loaded.stats.masks);

        // The crash this whole mechanism exists for leaves exactly ONE
        // half-written line. It must not parse.
        assert!(serde_json::from_str::<StateRow>(&line[..line.len() / 2]).is_err());
        assert!(serde_json::from_str::<StateRow>("{}").is_err());
    }

    /// A photograph a valid row answers for is not measured — and "not
    /// measured" is asserted by COUNTING, not by reading the code.
    ///
    /// MUTATION THIS CATCHES: make `resume_plan` always return `None` (or
    /// `restore_or_measure` always call `measure`) and the run re-buys every
    /// photograph a previous run already paid for — exactly the loss this
    /// change exists to stop. Both go red here.
    #[test]
    fn a_photograph_a_saved_row_answers_for_is_not_measured_again() {
        let dir = scratch("resume-skip");
        for stem in ["a", "b"] {
            std::fs::write(
                dir.join(format!("{stem}.xmp")),
                format!("<rdf:Description crs:Contrast2012=\"+{}\">", stem.len()),
            )
            .unwrap();
        }
        let raws = [dir.join("a.arw"), dir.join("b.arw")];
        let xmps = [dir.join("a.xmp"), dir.join("b.xmp")];
        let work: Vec<(&Path, &Path)> =
            raws.iter().zip(&xmps).map(|(r, x)| (r.as_path(), x.as_path())).collect();
        let mut stats = PhotoStats::default();
        stats.acc.insert("contrast".into(), Acc { sum_abs: 7.0, sum_signed: 7.0, n: 1, omit: 0 });
        let saved = StateRow {
            stem: "a".into(),
            rel: "a.arw".into(),
            xmp_sha256: sha256_hex(&std::fs::read(dir.join("a.xmp")).unwrap()),
            stats,
        };

        let (restored, stale) = resume_plan(&dir, &work, vec![saved]);
        assert!(stale.is_empty(), "an untouched sidecar is not stale: {stale:?}");
        assert!(restored[0].is_some(), "a's saved row must answer for a");
        assert!(restored[1].is_none(), "b was never measured");

        let calls = std::cell::Cell::new(0u32);
        let mut block = String::new();
        let out = restore_or_measure(false, &mut block, restored[0].is_some(), |_| {
            calls.set(calls.get() + 1);
            PhotoOutcome::Failed
        });
        assert!(matches!(out, PhotoOutcome::Restored), "a is restored, not measured");
        assert_eq!(calls.get(), 0, "a restored photograph must cost NOTHING");
        assert!(block.contains("from saved progress"), "and the transcript says so: {block}");

        let out = restore_or_measure(false, &mut block, restored[1].is_some(), |_| {
            calls.set(calls.get() + 1);
            PhotoOutcome::Failed
        });
        assert!(matches!(out, PhotoOutcome::Failed), "b takes the measurement path");
        assert_eq!(calls.get(), 1, "an unanswered photograph must be measured");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The photographer re-editing a photo invalidates the measurement OF that
    /// photo: the saved row scored the AI against an edit that no longer
    /// exists, so it is thrown away and the photograph is measured again — out
    /// loud, by stem.
    #[test]
    fn an_edited_sidecar_invalidates_its_saved_row() {
        let dir = scratch("resume-stale");
        let xmp = dir.join("a.xmp");
        std::fs::write(&xmp, "<rdf:Description crs:Contrast2012=\"+10\">").unwrap();
        let raws = [dir.join("a.arw")];
        let work: Vec<(&Path, &Path)> =
            raws.iter().map(|p| (p.as_path(), xmp.as_path())).collect();
        let row = StateRow {
            stem: "a".into(),
            rel: "a.arw".into(),
            xmp_sha256: sha256_hex(&std::fs::read(&xmp).unwrap()),
            stats: PhotoStats::default(),
        };

        let (restored, stale) = resume_plan(&dir, &work, vec![row.clone()]);
        assert!(restored[0].is_some() && stale.is_empty(), "untouched: honoured");

        std::fs::write(&xmp, "<rdf:Description crs:Contrast2012=\"+40\">").unwrap();
        let (restored, stale) = resume_plan(&dir, &work, vec![row.clone()]);
        assert!(restored[0].is_none(), "a re-edited photograph must be measured again");
        assert_eq!(stale, vec!["a".to_string()], "and the run must name which");

        // A sidecar that is gone is equally not a match (the run then fails on
        // it the way it always has — it must not be silently "restored").
        std::fs::remove_file(&xmp).unwrap();
        let (restored, _) = resume_plan(&dir, &work, vec![row]);
        assert!(restored[0].is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A header describing a different build or a different model is a
    /// REFUSAL, printed with both sides — one table measured by two different
    /// things is not a measurement, and silently mixing them would be the
    /// quietest possible way to publish a wrong baseline.
    #[test]
    fn saved_progress_from_another_build_or_model_is_refused() {
        let dir = scratch("resume-header");
        let path = dir.join("state.jsonl");
        let want = StateHeader {
            autoshade_version: "0.34.0".into(),
            proposer_model: "gpt-image-2".into(),
            analysis_model: "opus".into(),
            limit: 147,
            dir: "library".into(),
            started: 0,
        };
        let write = |h: &StateHeader| {
            let row = StateRow {
                stem: "a".into(),
                rel: "a.arw".into(),
                xmp_sha256: sha256_hex(b"<x/>"),
                stats: PhotoStats::default(),
            };
            let text = format!(
                "{}\n{}\n",
                serde_json::to_string(h).unwrap(),
                serde_json::to_string(&row).unwrap()
            );
            std::fs::write(&path, text).unwrap();
        };

        write(&want);
        assert_eq!(load_state(&path, &want).expect("same build loads").len(), 1);

        let refusal = |other: &StateHeader, saved_value: &str| {
            write(other);
            let e = load_state(&path, &want).unwrap_err().to_string();
            assert!(e.contains(saved_value), "the refusal must name the SAVED value: {e}");
            assert!(e.contains("--fresh"), "…and how to get past it: {e}");
            e
        };
        let mut other = want.clone();
        other.autoshade_version = "0.33.0".into();
        let e = refusal(&other, "0.33.0");
        assert!(e.contains("0.34.0"), "…and this run's value beside it: {e}");
        let mut other = want.clone();
        other.proposer_model = "gpt-4o".into();
        let e = refusal(&other, "gpt-4o");
        assert!(e.contains("gpt-image-2"), "…and this run's proposer beside it: {e}");
        let mut other = want.clone();
        other.analysis_model = "sonnet".into();
        let e = refusal(&other, "sonnet");
        assert!(e.contains("opus"), "…and this run's verifier beside it: {e}");

        // Not a header at all → still a refusal, never a silent empty resume.
        std::fs::write(&path, "not json\n").unwrap();
        assert!(load_state(&path, &want).is_err());
        // No file → simply "nothing measured yet".
        std::fs::remove_file(&path).unwrap();
        assert!(load_state(&path, &want).expect("absent = empty").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE poisoned row. A photograph whose recipe came from the deterministic
    /// baseline instead of the vision proposer completes the run — it is in the
    /// table, visibly — but it is never saved, so a rerun measures it again
    /// rather than folding the fallback into a baseline that claims to be the
    /// model's.
    ///
    /// MUTATION THIS CATCHES: drop either key from `proposer_answered` (a
    /// stream-error fallback, or a whole key-less run, becomes permanently
    /// baked into the saved progress), or record the row before the check.
    #[test]
    fn a_fallback_photograph_is_never_saved_as_progress() {
        use crate::rationale::{keys, Note};
        let dir = scratch("resume-fallback");
        let path = dir.join("state.jsonl");
        let header = StateHeader {
            autoshade_version: "0.34.0".into(),
            proposer_model: "gpt-image-2".into(),
            analysis_model: "opus".into(),
            limit: 3,
            dir: "library".into(),
            started: 0,
        };
        let log = StateLog::open(&path, &header, false);
        let row = |stem: &str| StateRow {
            stem: stem.to_string(),
            rel: format!("{stem}.arw"),
            xmp_sha256: sha256_hex(stem.as_bytes()),
            stats: PhotoStats::default(),
        };

        assert!(record_measured(&log, &[], &row("proposer_answered")));
        // The field failure: the proposer WAS tried and the stream broke.
        assert!(!record_measured(
            &log,
            &[Note::plain(keys::HEURISTIC_UNAVAILABLE)],
            &row("stream_error")
        ));
        // …and a run with no proposer configured is not a measurement of one.
        assert!(!record_measured(&log, &[Note::plain(keys::HEURISTIC_NO_KEY)], &row("no_key")));

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2, "header + exactly one row:\n{text}");
        assert!(text.contains("proposer_answered"), "{text}");
        assert!(!text.contains("stream_error"), "a fallback row leaked in:\n{text}");
        assert!(!text.contains("no_key"), "a key-less row leaked in:\n{text}");

        let rows = load_state(&path, &header).expect("reload");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].stem, "proposer_answered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A repeat that rescued a photograph and a repeat that failed again are
    /// NOT the same fact, and the provenance block has to keep them apart: the
    /// first is a row in the table that the R29 ruling paid to keep, the second
    /// is a photograph that still fell back and is therefore inside the
    /// fallback count on the line above it. A clean run says nothing at all —
    /// zeros here would read exactly like a run that repeated four times.
    ///
    /// MUTATION THIS CATCHES: printing unconditionally (a clean run grows a
    /// noise line), collapsing the two numbers into one total, or dropping the
    /// 524 double-billing sentence the ruling accepted out loud.
    #[test]
    fn the_report_separates_a_rescued_repeat_from_one_that_still_fell_back() {
        use crate::advisor::RetryTally;
        assert_eq!(retry_disclosure(RetryTally::default()), None, "a clean run stays quiet");

        let line = retry_disclosure(RetryTally { repeated: 5, recovered: 4 })
            .expect("a run that repeated says so");
        assert!(line.contains("5 upstream call(s)"), "{line}");
        assert!(line.contains("4 recovered (the repeated call's outer response succeeded)"), "{line}");
        assert!(line.contains("1 failed again"), "the exhausted half is derived, not dropped: {line}");
        assert!(line.contains("fell back"), "…and named as what it costs the table: {line}");
        assert!(line.contains("HTTP 524"), "the accepted 524 double-billing risk is disclosed: {line}");
        assert!(line.contains("HTTP 529"), "the relay 529 double-billing risk is disclosed: {line}");

        // Every repeat failing is the attempt-3 shape, and it must not read as
        // a success: nothing recovered, everything fell back.
        let all_lost = retry_disclosure(RetryTally { repeated: 5, recovered: 0 })
            .expect("a run whose repeats all failed says so");
        assert!(all_lost.contains("0 recovered (the repeated call's outer response succeeded)"), "{all_lost}");
        assert!(all_lost.contains("5 failed again"), "{all_lost}");
    }

    /// The progress file lands in the per-user store and nowhere near the
    /// photographs — the library is READ-ONLY by settled project law — and it
    /// is keyed by the two things that decide the work list.
    #[test]
    fn the_default_progress_file_is_in_the_store_and_keyed_by_the_run() {
        let lib = if cfg!(windows) { "D:/pics" } else { "/pics" };
        let a = default_state_path(Path::new(lib), 147);
        assert_ne!(a, default_state_path(Path::new(lib), 10), "--limit picks the work list");
        let other = if cfg!(windows) { "D:/other" } else { "/other" };
        assert_ne!(a, default_state_path(Path::new(other), 147), "another folder, another run");
        assert!(a.starts_with(crate::store::store_root()), "{}", a.display());
        assert!(!a.starts_with(lib), "never inside the library: {}", a.display());
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("jsonl"));
        if cfg!(windows) {
            // The store's own case-fold rule: one folder is one run however it
            // was typed.
            assert_eq!(a, default_state_path(Path::new("d:/PICS"), 147));
        }
    }

    #[test]
    fn tone_curve_parser_rejects_non_finite_and_out_of_range_points() {
        let xmp = r#"<crs:ToneCurvePV2012><rdf:Seq>
            <rdf:li>NaN, 1</rdf:li>
            <rdf:li>1, inf</rdf:li>
            <rdf:li>-1, 20</rdf:li>
            <rdf:li>20, 256</rdf:li>
            <rdf:li>64, 64</rdf:li>
        </rdf:Seq></crs:ToneCurvePV2012>"#;
        assert_eq!(
            parse_tone_curve(xmp, "ToneCurvePV2012"),
            vec![(64.0, 64.0)]
        );
    }

    #[test]
    fn low_sample_annotation_threshold_is_exactly_twenty() {
        assert_eq!(low_n_marker(19), " [low n]");
        assert_eq!(low_n_marker(20), "");
    }

    #[test]
    fn n_weighted_scalar_arithmetic_matches_a_hand_computed_fixture() {
        // (1 * 1 + 3 * 3) / (1 + 3) = 2.5.
        assert_eq!(n_weighted_scalar_mean(&[(1.0, 1), (3.0, 3)]), Some(2.5));
    }

    #[test]
    fn supplementary_gap_weights_scalar_rows_by_observed_n() {
        let ruler = vec![
            Row { metric: "contrast".into(), crs: String::new(), rule: Rule::Plain, source: Source::Field, circular: false },
            Row { metric: "exposure_ev".into(), crs: String::new(), rule: Rule::Plain, source: Source::Field, circular: false },
        ];
        let acc = BTreeMap::from([
            ("contrast".to_string(), Acc { sum_abs: 10.0, n: 1, ..Default::default() }),
            ("exposure_ev".to_string(), Acc { sum_abs: 10.0, n: 3, ..Default::default() }),
        ]);
        let score = supplementary_weighted_gap(&ruler, &acc, 1, 63.75, &[(0, 0.0); 3]).unwrap();
        // The scalar block is redistributed by n but retains its two headline
        // terms; the curve term remains a third term: 100 * (2 *
        // ((0.1 * 1 + (2/3) * 3) / 4) + 0.25) / 3 = 43.333...%.
        assert!((score - 43.3333333333).abs() < 1e-5, "{score}");
    }

    #[test]
    fn supplementary_gap_equals_headline_when_scalar_sample_counts_are_equal() {
        let ruler = vec![
            Row { metric: "contrast".into(), crs: String::new(), rule: Rule::Plain, source: Source::Field, circular: false },
            Row { metric: "exposure_ev".into(), crs: String::new(), rule: Rule::Plain, source: Source::Field, circular: false },
        ];
        let acc = BTreeMap::from([
            ("contrast".to_string(), Acc { sum_abs: 20.0, n: 2, ..Default::default() }),
            ("exposure_ev".to_string(), Acc { sum_abs: 1.0, n: 2, ..Default::default() }),
        ]);
        let headline = 100.0 * (0.1 + 0.1 + 0.1) / 3.0;
        let weighted = supplementary_weighted_gap(&ruler, &acc, 1, 25.5, &[(0, 0.0); 3]).unwrap();
        assert_eq!(weighted, headline);
    }

    #[test]
    fn report_printer_keeps_old_lines_and_adds_low_sample_marker() {
        let regular = Acc { sum_abs: 50.0, sum_signed: -10.0, n: 20, omit: 2 };
        let legacy = format!(
            "{:<22} {:>4} {:>10.2} {:>+13.2} {:>8}\n",
            "contrast", regular.n, regular.sum_abs / regular.n as f64,
            regular.sum_signed / regular.n as f64, regular.omit
        );
        assert_eq!(format_scalar_report_line("contrast", &regular), legacy);

        let ruler = vec![
            Row { metric: "contrast".into(), crs: String::new(), rule: Rule::Plain, source: Source::Field, circular: false },
            Row { metric: "exposure_ev".into(), crs: String::new(), rule: Rule::Plain, source: Source::Field, circular: false },
        ];
        let acc = BTreeMap::from([
            ("contrast".to_string(), regular),
            ("exposure_ev".to_string(), Acc { sum_abs: 19.0, sum_signed: 1.0, n: 19, omit: 0 }),
        ]);
        let report = format_scalar_report_rows(&ruler, &acc);
        assert!(report.contains(&legacy), "the established row stays byte-stable");
        assert!(report.contains("Legend: [low n]"), "the report carries its marker legend");
        assert!(format_supplementary_gap_line(52.5).contains("52.5%"), "the new line is printable");

        let low = Acc { sum_abs: 19.0, sum_signed: 1.0, n: 19, omit: 0 };
        let low_line = format_scalar_report_line("contrast", &low);
        assert!(low_line.starts_with("contrast [low n]"), "{low_line}");
        assert!(low_line.contains("  19 "), "the original n column remains: {low_line}");
    }
}
