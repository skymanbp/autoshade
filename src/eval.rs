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

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::pipeline;
use crate::recipe::EditRecipe;

// The `crs:` attribute scanner lives beside the XMP writer it inverts (xmp.rs,
// where the sidecar READER also uses it); re-exported so this module's tests
// and `style.rs` keep their `eval::crs_f32` path.
pub(crate) use crate::xmp::crs_f32;

/// Comparable global develop values from a user XMP, mapped into our recipe's
/// units (e.g. crs Sharpness 0..100 → recipe sharpening 0..150).
struct UserEdit {
    fields: Vec<(&'static str, Option<f32>)>,
}

fn parse_user_xmp(xmp: &str) -> UserEdit {
    // The Description's OWN scope, like every other whole-document reader
    // (`xmp::crs_own_scope`): a nested creative Look's baked parameters are
    // the PROFILE's, not the photographer's, and scoring the AI against them
    // charged it for matching a look no slider in the sidecar states.
    let xmp = crate::xmp::crs_own_scope(xmp);
    let xmp = xmp.as_ref();
    // As-shot provenance: under WhiteBalance="As Shot", crs:Temperature/Tint
    // record the CAMERA's values, not a user edit — counting them charged the
    // AI a false "omission" for correctly leaving WB as-shot. Any OTHER value
    // (Custom, Daylight, …) is a user decision; an absent attribute (non-LR /
    // hand-trimmed sidecar) keeps the Kelvin as intentional.
    let user_wb = crate::xmp::crs_str(xmp, "WhiteBalance").as_deref() != Some("As Shot");
    let wb = |k: &str| if user_wb { crs_f32(xmp, k) } else { None };
    UserEdit {
        fields: vec![
            ("exposure_ev", crs_f32(xmp, "Exposure2012")),
            ("contrast", crs_f32(xmp, "Contrast2012")),
            ("highlights", crs_f32(xmp, "Highlights2012")),
            ("shadows", crs_f32(xmp, "Shadows2012")),
            ("whites", crs_f32(xmp, "Whites2012")),
            ("blacks", crs_f32(xmp, "Blacks2012")),
            ("tint", wb("Tint")),
            ("vibrance", crs_f32(xmp, "Vibrance")),
            ("saturation", crs_f32(xmp, "Saturation")),
            ("clarity", crs_f32(xmp, "Clarity2012")),
            ("dehaze", crs_f32(xmp, "Dehaze")),
            // crs Sharpness is 0..100; recipe sharpening is 0..150.
            ("sharpening", crs_f32(xmp, "Sharpness").map(|s| s * 1.5)),
            ("noise_reduction", crs_f32(xmp, "LuminanceSmoothing")),
            ("temperature_k", wb("Temperature")),
        ],
    }
}

/// The AI recipe's value for the same named field (None = field not set, e.g.
/// temperature_k left as-shot).
fn ai_field(r: &EditRecipe, name: &str) -> Option<f32> {
    Some(match name {
        "exposure_ev" => r.exposure_ev,
        "contrast" => r.contrast,
        "highlights" => r.highlights,
        "shadows" => r.shadows,
        "whites" => r.whites,
        "blacks" => r.blacks,
        // The engine's tint is a DELTA from the as-shot green balance
        // (render::apply_wb anchors at as_shot); LR's crs:Tint is absolute.
        // Compare absolutes on both sides.
        "tint" => r.as_shot_tint.unwrap_or(0.0) + r.tint,
        "vibrance" => r.vibrance,
        "saturation" => r.saturation,
        "clarity" => r.clarity,
        "dehaze" => r.dehaze,
        "sharpening" => r.sharpening,
        "noise_reduction" => r.noise_reduction,
        // As-shot resolves through the recipe's own stamp, with the engine's
        // historical 5500 K fallback (render.rs uses the same anchor).
        "temperature_k" => r
            .temperature_k
            .unwrap_or_else(|| r.as_shot_k.unwrap_or(5500.0)),
        _ => return None,
    })
}

/// The user's EFFECTIVE value for a field: an absent ordinary slider means
/// they left it at 0, an absent white balance means they kept as-shot — so a
/// one-sided disagreement still compares two real numbers instead of being
/// dropped from the aggregate (which understated the gap score).
fn effective_user_field(r: &EditRecipe, name: &str, value: Option<f32>) -> f32 {
    match name {
        "temperature_k" => value.unwrap_or_else(|| r.as_shot_k.unwrap_or(5500.0)),
        "tint" => value.unwrap_or_else(|| r.as_shot_tint.unwrap_or(0.0)),
        _ => value.unwrap_or(0.0),
    }
}

/// Parse an ACR tone-curve `<rdf:Seq>` of `<rdf:li>x, y</rdf:li>` points (each
/// 0..255 input,output) for the given crs tag (e.g. "ToneCurvePV2012"). Empty
/// vec if the tag is absent. The master tone curve is the single biggest "look"
/// control that the flat-slider comparison above was completely blind to.
fn parse_tone_curve(xmp: &str, tag: &str) -> Vec<(f32, f32)> {
    let open = format!("<crs:{tag}>");
    let close = format!("</crs:{tag}>");
    let Some(s) = xmp.find(&open) else { return Vec::new() };
    let body = &xmp[s + open.len()..];
    let Some(e) = body.find(&close) else { return Vec::new() };
    let body = &body[..e];
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

#[derive(Default, Clone, Copy)]
struct Acc {
    sum_abs: f64,
    sum_signed: f64,
    n: u32,
    /// Times the user used this control but the AI left it neutral/omitted it —
    /// a real miss the old both-set gate dropped silently.
    omit: u32,
}

pub fn run(dir: &Path, limit: usize) -> Result<()> {
    let cfg = Config::load();
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

    // Field order for the report (matches parse_user_xmp).
    let order = [
        "exposure_ev", "contrast", "highlights", "shadows", "whites", "blacks", "tint",
        "vibrance", "saturation", "clarity", "dehaze", "sharpening", "noise_reduction",
        "temperature_k",
    ];
    let mut acc: BTreeMap<&str, Acc> = BTreeMap::new();
    let mut evaluated = 0u32;
    // Master-tone-curve comparison (the look control the flat sliders miss),
    // accumulated only over photos where YOU drew a curve.
    let (mut curve_n, mut sum_curve_rmse) = (0u32, 0f64);
    let (mut sum_user_lift, mut sum_ai_lift) = (0f64, 0f64);
    let (mut sum_user_s, mut sum_ai_s) = (0f64, 0f64);

    for (i, raw) in pairs.iter().take(limit).enumerate() {
        print!("[{}/{}] {} ... ", i + 1, pairs.len().min(limit), pipeline::stem(raw));
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let xmp_text = crate::store::read_sidecar(&raw.with_extension("xmp"))
            .with_context(|| format!("read user xmp for {}", raw.display()))?;
        let user = parse_user_xmp(&xmp_text);
        // style_strength = 0: eval measures the raw AI proposal vs your edits, so
        // it must NOT pull toward your historical style (that would bias the gap).
        let (ai, _verdict) = match pipeline::produce_recipe(raw, &cfg, false, None, None, 0.0) {
            Ok(v) => v,
            Err(e) => {
                println!("FAILED: {e}");
                continue;
            }
        };
        for (name, user_val) in &user.fields {
            let eps = if *name == "exposure_ev" { 0.05 } else { 0.5 };
            let user_used = user_val.is_some_and(|u| u.abs() > eps);
            let ai_used = match *name {
                "temperature_k" => ai.temperature_k.is_some(),
                "tint" => ai.tint.abs() > eps,
                _ => ai_field(&ai, name).is_some_and(|a| a.abs() > eps),
            };
            let u = effective_user_field(&ai, name, *user_val);
            let ai_val = ai_field(&ai, name);
            // A both-neutral row is not a comparison: Lightroom writes EVERY
            // slider explicitly (Contrast2012="0"), so counting 0↔0 rows
            // inflated n and diluted each control's MAE toward zero — the gap
            // score understated real divergence. A row where EITHER side
            // moved still counts (that disagreement is the signal).
            if !user_used && !ai_used {
                continue;
            }
            let e = acc.entry(name).or_default();
            match ai_val {
                Some(a) => {
                    let d = (a - u) as f64;
                    e.sum_abs += d.abs();
                    e.sum_signed += d;
                    e.n += 1;
                    // You moved it; the AI parked it at neutral → a miss.
                    if user_used && !ai_used {
                        e.omit += 1;
                    }
                }
                // Unreachable for the 14 known fields now that WB resolves
                // through the as-shot stamp; kept for an unknown name.
                None => {
                    if u.abs() > eps {
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
            curve_n += 1;
            sum_curve_rmse += curve_rmse(&ulut, &alut);
            sum_user_lift += curve_black_lift(&ulut) as f64;
            sum_ai_lift += curve_black_lift(&alut) as f64;
            sum_user_s += curve_s_strength(&ulut) as f64;
            sum_ai_s += curve_s_strength(&alut) as f64;
        }

        evaluated += 1;
        println!("done");
    }

    if evaluated == 0 {
        println!("No photos evaluated.");
        return Ok(());
    }

    println!("\n=== AI vs your edits ({evaluated} photo(s)) ===");
    println!("{:<16} {:>4} {:>10} {:>13} {:>8}", "field", "n", "mean|Δ|", "bias(AI−you)", "AI-omit");
    for name in order {
        let Some(a) = acc.get(name) else { continue };
        if a.n == 0 && a.omit == 0 {
            continue;
        }
        if a.n > 0 {
            let mae = a.sum_abs / a.n as f64;
            let bias = a.sum_signed / a.n as f64;
            println!("{:<16} {:>4} {:>10.2} {:>+13.2} {:>8}", name, a.n, mae, bias, a.omit);
        } else {
            // You used it, the AI never engaged it — no Δ to report, just the miss.
            println!("{:<16} {:>4} {:>10} {:>13} {:>8}", name, a.n, "—", "—", a.omit);
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

    // --- aggregate gap score -------------------------------------------------
    // Mean fractional divergence across the controls you used, each MAE
    // normalised by a sensible full-scale, plus the tone-curve term. One number
    // to watch move as the advisor improves.
    let range_of = |name: &str| -> f64 {
        match name {
            "exposure_ev" => 5.0,
            "sharpening" => 150.0,
            "temperature_k" => 2000.0,
            _ => 100.0,
        }
    };
    let (mut frac_sum, mut frac_n) = (0f64, 0u32);
    for name in order {
        if let Some(a) = acc.get(name)
            && a.n > 0
        {
            frac_sum += (a.sum_abs / a.n as f64) / range_of(name);
            frac_n += 1;
        }
    }
    if curve_n > 0 {
        frac_sum += (sum_curve_rmse / curve_n as f64) / 255.0;
        frac_n += 1;
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

    // Values copied from the user's real DSC08724.xmp (read this session).
    const SAMPLE: &str = r#"<rdf:Description
        crs:Temperature="5650" crs:Tint="+13" crs:Exposure2012="0.00"
        crs:Contrast2012="+22" crs:Highlights2012="+7" crs:Shadows2012="-6"
        crs:Whites2012="0" crs:Blacks2012="0" crs:Clarity2012="0" crs:Dehaze="+18"
        crs:Vibrance="+5" crs:Saturation="+13" crs:Sharpness="40"
        crs:LuminanceSmoothing="0">"#;

    fn get(u: &UserEdit, k: &str) -> Option<f32> {
        u.fields.iter().find(|(n, _)| *n == k).and_then(|(_, v)| *v)
    }

    #[test]
    fn parses_real_crs_values() {
        let u = parse_user_xmp(SAMPLE);
        assert_eq!(get(&u, "exposure_ev"), Some(0.0));
        assert_eq!(get(&u, "contrast"), Some(22.0));
        assert_eq!(get(&u, "shadows"), Some(-6.0));
        assert_eq!(get(&u, "dehaze"), Some(18.0));
        assert_eq!(get(&u, "temperature_k"), Some(5650.0));
        assert_eq!(get(&u, "sharpening"), Some(60.0)); // 40 * 1.5
        assert_eq!(crs_f32(SAMPLE, "Nonexistent"), None);
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

        assert_eq!(effective_user_field(&ai, "contrast", None), 0.0);
        assert_eq!(ai_field(&ai, "contrast"), Some(25.0));
        assert_eq!(effective_user_field(&ai, "temperature_k", None), 4800.0);
        assert_eq!(ai_field(&ai, "temperature_k"), Some(4800.0));
        assert_eq!(effective_user_field(&ai, "tint", None), 7.0);
        assert_eq!(ai_field(&ai, "tint"), Some(10.0));
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
