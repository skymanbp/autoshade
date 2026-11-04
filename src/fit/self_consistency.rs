//! The self-consistency gate (v1.18.0): every control the global fit can
//! solve, set to a known value on a corpus RAW, rendered by the app itself,
//! and reverse-fitted from the photo's straight conversion. A control that
//! never fires on the calibration pairs has no real-photo evidence of what
//! it does when it fires; this is the evidence the project's own standard
//! allows — the target is the app's own render, so every pixel of it is
//! reachable by the recipe that made it.
//!
//! Each test asserts what a self-consistent fit owes such a target:
//! * the finished frame lands within the fit's own quantisation
//!   ([`FIT_QUANT`] — the residual the rounded sliders, the 8-bit curves and
//!   the f32 develop leave on an IDENTICAL pair), and
//! * the control that was set carries the move: its fitted value has the
//!   set value's sign and reaches at least half of it, and
//! * every control that was NOT set stays at the straight conversion's
//!   value — the move is attributed to the control that made it.
//!
//! Gated on the calibration corpus (`AUTOSHADE_FIT_CALIBRATION_DIR`, the
//! `pNN.arw` RAWs beside it); with the corpus absent each test is skipped.
//!
//! Every test here failed when the gate was built (rel1180 msg2 lists the
//! readings); each is `#[ignore]`d until the fix that makes it pass lands
//! with it, so the list of ignores IS the list of open defects.

use super::*;
use crate::recipe::{CurvePoint, GradeStrength};

fn corpus_raw(name: &str) -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR")?);
    let raw = dir.join(name);
    if !raw.is_file() {
        crate::test_skipped("self-consistency gate", &format!("{} has no {name}", dir.display()));
        return None;
    }
    Some(raw)
}

/// The fitted recipe's user controls that differ from the straight
/// conversion's, as `key: base -> fitted` lines (calibration fields, the
/// rationale and the confidence are not controls).
fn moved_controls(base: &EditRecipe, fitted: &EditRecipe) -> Vec<String> {
    let skip = ["rationale", "confidence", "version", "coord_era", "schema_era", "base_curve", "lens_profile",
        "as_shot_k", "as_shot_tint", "camera_profile", "look"];
    let (a, b) = (serde_json::to_value(base).unwrap(), serde_json::to_value(fitted).unwrap());
    let mut out = Vec::new();
    fn walk(path: &str, a: &serde_json::Value, b: &serde_json::Value, out: &mut Vec<String>) {
        match (a, b) {
            (serde_json::Value::Object(x), serde_json::Value::Object(y)) => {
                for (k, v) in y {
                    let null = serde_json::Value::Null;
                    walk(&format!("{path}.{k}"), x.get(k).unwrap_or(&null), v, out);
                }
            }
            (serde_json::Value::Array(x), serde_json::Value::Array(y)) if x.len() == y.len() && !x.is_empty()
                && x.iter().all(|v| v.is_number()) =>
            {
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    walk(&format!("{path}[{i}]"), p, q, out);
                }
            }
            (p, q) if p != q => out.push(format!("{}: {} -> {}", path.trim_start_matches('.'), p, q)),
            _ => {}
        }
    }
    if let (serde_json::Value::Object(x), serde_json::Value::Object(y)) = (&a, &b) {
        for (k, v) in y {
            if skip.contains(&k.as_str()) {
                continue;
            }
            let null = serde_json::Value::Null;
            walk(k, x.get(k).unwrap_or(&null), v, &mut out);
        }
    }
    out
}

/// The pair a reverse fit of `raw` against the app's own render of `made`
/// reads, built by the entry points' own steps (`main.rs` `match`, the
/// desktop app's fit): the fit source and the calibration it composes
/// ([`crate::pipeline::fit_source`]), the target rendered at the source's
/// working edge, and the source put in the target's frame
/// ([`crate::pipeline::fit_target`] — the export runs the lens profile, the
/// fit source is in the sensor frame). Returns (source, target, the base the
/// fit composes after `fit_target`).
pub(crate) fn straight_pair(raw: &std::path::Path, set: impl Fn(&mut EditRecipe)) -> (render::Plate, DynamicImage, EditRecipe) {
    let (mut src, mut base) = crate::pipeline::fit_source(raw).expect("the fit source develops");
    let mut made = base.clone();
    set(&mut made);
    let target = render::render_to_image(raw, &made, None, Some(crate::pipeline::FIT_SOURCE_EDGE)).expect("the app's own render");
    let fitted = crate::pipeline::fit_target(&mut src, Some(raw), &target, &mut base).expect("the target's frame");
    (src, fitted.image, base)
}

/// Render `raw` with the control `set` applies on top of its straight
/// conversion, reverse-fit from the straight conversion at `strength`, and
/// return (the base the fit composed, the report).
fn round_trip(raw: &std::path::Path, strength: f32, set: impl Fn(&mut EditRecipe)) -> (EditRecipe, FitReport) {
    let (src, target, base) = straight_pair(raw, set);
    let report = fit_recipe_from_with(
        &src,
        &target,
        &base,
        FitOptions { strength: GradeStrength::new(strength), provider: None },
    );
    (base, report)
}

/// The gate's three assertions, collected so a failing control names every
/// way it failed. `carried` is the set control's fitted value, `wanted` the
/// value set; `own` the key prefixes the set control is allowed to move.
fn gate(name: &str, base: &EditRecipe, rep: &FitReport, carried: f32, wanted: f32, own: &[&str]) {
    let moved = moved_controls(base, &rep.recipe);
    let foreign: Vec<&String> = moved.iter().filter(|m| !own.iter().any(|o| m.starts_with(o))).collect();
    let mut failures = Vec::new();
    if rep.err_after > FIT_QUANT {
        failures.push(format!("the finished frame lands at {:.5} > FIT_QUANT {FIT_QUANT}", rep.err_after));
    }
    if carried * wanted <= 0.0 || carried.abs() < 0.5 * wanted.abs() {
        failures.push(format!("the control carries {carried:+.2} of the {wanted:+.2} set"));
    }
    if !foreign.is_empty() {
        failures.push(format!("controls not set moved: {foreign:?}"));
    }
    eprintln!(
        "GATE {name}: err {:.5} -> {:.5}; D {:?}; carried {carried:+.2} of {wanted:+.2}; moved {moved:?}",
        rep.err_before,
        rep.err_after,
        rep.divergence.map(|r| r.d)
    );
    assert!(failures.is_empty(), "{name}: {}\n{}", failures.join("; "), rep.recipe.rationale);
}

#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_fit_recovers_a_white_balance_it_rendered() {
    let Some(raw) = corpus_raw("p39.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.temperature_k = Some(4300.0));
    let anchor = base.as_shot_k.unwrap_or(5500.0);
    let fitted = rep.recipe.temperature_k.unwrap_or(anchor);
    gate("temperature", &base, &rep, fitted - anchor, 4300.0 - anchor, &["temperature_k", "tint"]);
}

#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_fit_recovers_a_tint_it_rendered() {
    let Some(raw) = corpus_raw("p39.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.tint = 15.0);
    gate("tint", &base, &rep, rep.recipe.tint - base.tint, 15.0, &["temperature_k", "tint"]);
}

#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_fit_recovers_a_band_hue_it_rendered() {
    let Some(raw) = corpus_raw("p39.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.hsl.hue[5] = 20.0);
    gate("hsl hue (blue)", &base, &rep, rep.recipe.hsl.hue[5], 20.0, &["hsl.hue"]);
}

#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_fit_recovers_a_band_saturation_it_rendered() {
    let Some(raw) = corpus_raw("p39.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.hsl.saturation[5] = 30.0);
    gate("hsl saturation (blue)", &base, &rep, rep.recipe.hsl.saturation[5], 30.0, &["hsl.saturation"]);
}

#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_fit_recovers_a_band_luminance_it_rendered() {
    let Some(raw) = corpus_raw("p40.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.hsl.luminance[1] = -25.0);
    gate("hsl luminance (orange)", &base, &rep, rep.recipe.hsl.luminance[1], -25.0, &["hsl.luminance"]);
}

#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_fit_recovers_a_grade_wheel_it_rendered() {
    let Some(raw) = corpus_raw("p41.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| {
        r.color_grade.midtone_hue = 40.0;
        r.color_grade.midtone_sat = 25.0;
    });
    gate("grade midtone wheel", &base, &rep, rep.recipe.color_grade.midtone_sat, 25.0, &["color_grade"]);
}

#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_fit_recovers_a_cast_curve_it_rendered() {
    let Some(raw) = corpus_raw("p38.arw") else { return };
    let curve = vec![
        CurvePoint { input: 0, output: 0 },
        CurvePoint { input: 128, output: 146 },
        CurvePoint { input: 255, output: 255 },
    ];
    let (base, rep) = round_trip(&raw, 0.85, |r| r.red_curve = curve.clone());
    let lift = |c: &[CurvePoint]| {
        if c.is_empty() { return 0.0 }
        // The curve's output at 128, read by its own linear interpolation.
        let w = c.windows(2).find(|w| w[0].input <= 128 && w[1].input >= 128).unwrap_or(&c[..2]);
        let t = (128.0 - w[0].input as f32) / (w[1].input as f32 - w[0].input as f32).max(1.0);
        w[0].output as f32 + t * (w[1].output as f32 - w[0].output as f32) - 128.0
    };
    gate("red cast curve", &base, &rep, lift(&rep.recipe.red_curve), 18.0, &["red_curve", "green_curve", "blue_curve"]);
}

/// The harness itself, and the first gate: with nothing set, the app's
/// render of the recipe the fit starts from must read as the straight
/// conversion the fit is handed — within [`FIT_QUANT`], with no control
/// moved — or every gate below measures the gap between the export and the
/// fit's own preview develop, not the control.
#[test]
fn the_straight_conversion_round_trips_to_itself() {
    let mut failures = Vec::new();
    for name in ["p38.arw", "p39.arw", "p40.arw", "p41.arw"] {
        let Some(raw) = corpus_raw(name) else { return };
        let (base, rep) = round_trip(&raw, 0.85, |_| {});
        let moved = moved_controls(&base, &rep.recipe);
        eprintln!("GATE identity {name}: err {:.5} -> {:.5}; moved {moved:?}", rep.err_before, rep.err_after);
        if rep.err_before > FIT_QUANT || !moved.is_empty() {
            failures.push(format!("{name}: the identical pair reads {:.5} (FIT_QUANT {FIT_QUANT}) and moved {} controls", rep.err_before, moved.len()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

/// Item 5's case in the gate's form: a target that differs from the straight
/// conversion by EXPOSURE ALONE is recovered as exposure alone — no mixer, no
/// tint, no cast curves (no fan), nothing else — whether the exposure clips
/// the sky (+1.0 EV on p39: green and blue clip over the sky) or stays just
/// under the clip (-0.4 EV). In v1.17.1 the clipped target pegged the mixer,
/// tinted the sky and fanned the cast curves (rel1180 msg1, item 5).
#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_fit_recovers_an_exposure_alone_with_a_clipped_sky() {
    let Some(raw) = corpus_raw("p39.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.exposure_ev = 1.0);
    gate("exposure +1.0 EV (sky clipped)", &base, &rep, rep.recipe.exposure_ev - base.exposure_ev, 1.0, &["exposure_ev"]);
}

#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_fit_recovers_an_exposure_alone_just_under_the_clip() {
    let Some(raw) = corpus_raw("p39.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.exposure_ev = -0.4);
    gate("exposure -0.4 EV (under the clip)", &base, &rep, rep.recipe.exposure_ev - base.exposure_ev, -0.4, &["exposure_ev"]);
}
