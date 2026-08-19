//! Eval harness — how close is the AI's edit to the user's own?
//!
//! For RAWs that have a sibling `.xmp` (the user's ACR/Lightroom develop
//! settings = ground truth), run the AI advisor and compare global slider
//! values. Reports per-field **mean absolute error** (how far off) and **mean
//! signed bias** AI−user (which direction the AI leans). That bias is the
//! tuning signal: e.g. "AI contrast runs +8 hotter than you" → nudge the prompt.
//!
//! XMP is parsed by plain text scan of `crs:Key="value"` (the values are
//! attributes on rdf:Description; verified against the user's real DSC08724.xmp).
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
use std::path::Path;

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

// The `crs:` attribute scanner lives beside the XMP writer it inverts (xmp.rs,
// where the sidecar READER also uses it); re-exported so this module's tests
// and `style.rs` keep their `eval::crs_f32` path.
pub(crate) use crate::xmp::crs_f32;

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
        if self.metric == "exposure_ev" { 0.05 } else { 0.5 }
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
fn user_value(row: &Row, scope: &str, user_wb: bool, has_crop: bool) -> Option<f32> {
    match row.rule {
        Rule::Wb if !user_wb => None,
        Rule::CropGated if !has_crop => None,
        _ => crs_f32(scope, &row.crs),
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
    let scope = scope.as_ref();
    let mut sums = [0.0f32; 3];
    for f in catalogue::hsl_expansion() {
        sums[f.axis] += crs_f32(scope, &f.crs).unwrap_or(0.0).abs();
    }
    let hsl = sums.map(|s| s / crate::recipe::HSL_BANDS.len() as f32);
    let wheel = |suffix: &str| -> Vec<f32> {
        COLOR_GRADE_CRS
            .iter()
            .filter(|(field, _)| field.ends_with(suffix))
            .map(|(_, key)| crs_f32(scope, key).unwrap_or(0.0))
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
        .filter(|tag| !parse_tone_curve(scope, tag).is_empty())
        .count() as u8;
    let mut out = FamilySummary { hsl, grade: [max_sat, mean_lum], rgb_curves };
    out.clamp();
    (out != FamilySummary::default()).then_some(out)
}

#[derive(Default, Clone, Copy)]
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

/// What became of one photograph. The three non-`Stats` arms exist because the
/// serial loop had three distinct exits (`continue` on a failed analysis, `?` on
/// an unreadable sidecar, and — new — "an earlier photo already aborted the
/// run"), and a pool cannot `?` out of a worker: the decision has to travel back
/// as a value and be acted on in index order.
enum PhotoOutcome {
    Stats(Box<PhotoStats>),
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
#[derive(Default)]
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

pub fn run(dir: &Path, limit: usize, jobs: usize) -> Result<()> {
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
    let pairs: Vec<_> = raws
        .iter()
        .filter(|r| r.with_extension("xmp").exists())
        .collect();
    println!(
        "found {} RAW(s); {} have a sibling .xmp (your edits). Evaluating {}.",
        raws.len(),
        pairs.len(),
        pairs.len().min(limit)
    );
    if pairs.is_empty() {
        println!("Nothing to evaluate — no .xmp sidecars next to the RAWs in this folder.");
        return Ok(());
    }

    // The ruler, in registry order (which is also the report order — the two
    // used to be separate hand-kept lists with a comment claiming they matched).
    let ruler = rows();
    // The work list is fixed up front — exactly the first `limit` pairs the old
    // sequential `.take(limit)` selected — then a bounded, memory-budgeted pool
    // works through it (R27 Batch-7). At `--jobs 1` this is the serial loop it
    // replaces, line for line and sum for sum.
    let work: Vec<&Path> = pairs.iter().take(limit).map(|p| p.as_path()).collect();
    let n = work.len();
    let plan = crate::jobs::plan(jobs, n);
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
        let raw: &Path = work[i];
        emit(live, block, &format!("[{}/{}] {} ... ", i + 1, n, pipeline::stem(raw)));
        let Some(xmp_text) = crate::store::read_sidecar(&raw.with_extension("xmp")) else {
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
        let scope = scope.as_ref();
        // Any value OTHER than "As Shot" is a user WB decision; an absent
        // attribute (non-LR / hand-trimmed sidecar) keeps the Kelvin as
        // intentional.
        let user_wb = crate::xmp::crs_str(scope, "WhiteBalance").as_deref() != Some("As Shot");
        let has_crop = crate::xmp::crs_str(scope, "HasCrop").as_deref() == Some("True");
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
            let user_curve = parse_tone_curve(scope, tag);
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
        let imported = crate::xmp::xmp_to_recipe(&xmp_text).masks.len();
        let refused = crate::xmp::unsupported_corrections(&xmp_text);
        if imported + refused + ai.masks.len() > 0 {
            stats.masks = Some((imported, refused, ai.masks.len()));
        }

        emit(live, block, "done\n");
        // Above one job the per-photo ⚠ lines the pipeline raises on stderr
        // (the GPT-proposer fallback at pipeline.rs:390) arrive in COMPLETION
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
        PhotoOutcome::Stats(Box::new(stats))
    });

    // ── FOLD, strictly in photo order ────────────────────────────────────────
    // `f64` addition is not associative, so summing in completion order would
    // move the printed gap score in the last ulp depending on which photo
    // finished first. Folding the positional results reproduces the serial
    // arithmetic exactly, at any `--jobs`.
    let mut abort: Option<String> = None;
    for outcome in outcomes.into_iter().flatten() {
        match outcome {
            PhotoOutcome::Stats(s) => {
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
            // Reported in its own block; the run continues (unchanged).
            PhotoOutcome::Failed | PhotoOutcome::Skipped => {}
            // The FIRST hard error wins, so the message a `--jobs 1` run would
            // have raised is the message this run raises.
            PhotoOutcome::Aborted(why) => {
                if abort.is_none() {
                    abort = Some(why);
                }
            }
        }
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
    println!("{:<22} {:>4} {:>10} {:>13} {:>8}", "field", "n", "mean|Δ|", "bias(AI−you)", "AI-omit");
    for row in &ruler {
        let Some(a) = acc.get(&row.metric) else { continue };
        if a.n == 0 && a.omit == 0 {
            continue;
        }
        if a.n > 0 {
            let mae = a.sum_abs / a.n as f64;
            let bias = a.sum_signed / a.n as f64;
            println!("{:<22} {:>4} {:>10.2} {:>+13.2} {:>8}", row.metric, a.n, mae, bias, a.omit);
        } else {
            // You used it, the AI never engaged it — no Δ to report, just the miss.
            println!("{:<22} {:>4} {:>10} {:>13} {:>8}", row.metric, a.n, "—", "—", a.omit);
        }
    }

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
    /// Point `AUTOSHOP_ESTIMATOR_PROBE` at a folder scanned recursively for
    /// RAW files with a sibling `.xmp` (the same pair rule [`run`] uses, via
    /// the same `pipeline::find_raws`), and run:
    ///
    /// ```text
    /// AUTOSHOP_ESTIMATOR_PROBE=<dir> cargo test --lib -- --ignored --nocapture estimator
    /// ```
    ///
    /// `AUTOSHOP_ESTIMATOR_PROBE_LIMIT` caps the pair count for a quick pass.
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
    #[ignore = "forensic probe: needs AUTOSHOP_ESTIMATOR_PROBE=<dir of RAW+.xmp pairs>; decodes every pair (minutes)"]
    fn estimators_against_the_photographers_own_sliders() {
        let Ok(dir) = std::env::var("AUTOSHOP_ESTIMATOR_PROBE") else {
            panic!(
                "set AUTOSHOP_ESTIMATOR_PROBE to a folder of RAW files with sibling .xmp \
                 sidecars (the eval corpus)"
            );
        };
        let root = std::path::PathBuf::from(&dir);
        assert!(root.is_dir(), "AUTOSHOP_ESTIMATOR_PROBE is not a directory: {dir}");
        let limit: usize = std::env::var("AUTOSHOP_ESTIMATOR_PROBE_LIMIT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(usize::MAX);
        let raws = pipeline::find_raws(&root).expect("scan the probe directory");
        let pairs: Vec<_> = raws
            .iter()
            .filter(|r| r.with_extension("xmp").exists())
            .take(limit)
            .collect();
        assert!(
            !pairs.is_empty(),
            "AUTOSHOP_ESTIMATOR_PROBE ({dir}) holds no RAW with a sibling .xmp"
        );

        let (mut ev, mut wb_k, mut wb_t) = (Scatter::default(), Scatter::default(), Scatter::default());
        let (mut gw_k_all, mut gw_t_all) = (Scatter::default(), Scatter::default());
        let (mut gw_k_custom, mut gw_t_custom) = (Scatter::default(), Scatter::default());
        let (mut as_shot_photos, mut custom_wb_photos, mut no_anchor) = (0u32, 0u32, 0u32);

        for raw in &pairs {
            let stem = pipeline::stem(raw);
            let xmp_text = crate::store::read_sidecar(&raw.with_extension("xmp"))
                .unwrap_or_else(|| panic!("sidecar for {stem} is missing or unreadable"));
            // The Description's OWN scope — the same rule `run` applies, and
            // for the same reason: a creative Look bakes its own crs values.
            let scope = crate::xmp::crs_own_scope(&xmp_text);
            let scope = scope.as_ref();
            let user_wb = crate::xmp::crs_str(scope, "WhiteBalance").as_deref() != Some("As Shot");
            let decoded = crate::decode::decode_any(raw)
                .unwrap_or_else(|e| panic!("decode {stem}: {e:#}"));
            let hist = &decoded.histogram;

            // --- E1: EV offset vs the photographer's exposure slider -------
            // An absent Exposure2012 means they left it at 0, the same
            // neutral rule the ruler above uses.
            let est_ev = crate::advisor::HeuristicProposer::ev_offset_estimate(hist);
            ev.push(stem, est_ev as f64, crs_f32(scope, "Exposure2012").unwrap_or(0.0) as f64);

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
                    if let Some(t) = crs_f32(scope, "Temperature") {
                        wb_k.push(stem, k as f64, t as f64);
                    }
                    if let Some(t) = crs_f32(scope, "Tint") {
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
            let (uk, ut) = (crs_f32(scope, "Temperature"), crs_f32(scope, "Tint"));
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

    // Values copied from the user's real DSC08724.xmp (read this session).
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
        let user_wb = crate::xmp::crs_str(xmp, "WhiteBalance").as_deref() != Some("As Shot");
        let has_crop = crate::xmp::crs_str(xmp, "HasCrop").as_deref() == Some("True");
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
        assert_eq!(crs_f32(SAMPLE, "Nonexistent"), None);
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
}
