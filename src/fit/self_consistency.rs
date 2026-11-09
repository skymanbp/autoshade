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

/// One case of the validity prototype (design-control-validity-1180.md §5),
/// written under `dir/tag/` as f32 little-endian arrays (`scripts/field_check.py`'s
/// layout) and a `meta.json`: the base render (`source.bin`), the target
/// (`target.bin`), the recipe without each class and the recipe's render
/// (`without_tone.bin`, `without_wb.bin`, `without_chroma.bin`,
/// `without_curves.bin`, `full.bin`: `joint::without_class` + the recipe
/// itself), and the
/// evidence the law reads — `source_weights`, `target_weights`,
/// `source_presence_weights`, `spatial_weights` and `paired` (0/1) — plus the
/// recipe as JSON. Pixels are `w × h` rows, three channels interleaved.
pub(crate) fn dump_validity_probe(
    dir: &std::path::Path,
    tag: &str,
    s_img: &DynamicImage,
    base: &EditRecipe,
    recipe: &EditRecipe,
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) {
    let out = dir.join(tag);
    std::fs::create_dir_all(&out).expect("create the validity probe directory");
    let write = |name: &str, values: &[f32]| {
        let mut bytes = Vec::with_capacity(values.len() * 4);
        for v in values {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(out.join(name), bytes).unwrap_or_else(|e| panic!("write {name}: {e}"));
    };
    let flat = |px: &[[f32; 3]]| px.iter().flatten().copied().collect::<Vec<f32>>();
    let frame = |r: &EditRecipe| pixels_of(&render::develop_preview(s_img, r));
    write("source.bin", &flat(&frame(base)));
    write("target.bin", &flat(tp));
    for (class, name) in super::joint::CLASSES.iter().zip(["without_tone", "without_wb", "without_chroma", "without_curves"]) {
        write(&format!("{name}.bin"), &flat(&frame(&super::joint::without_class(base, recipe, *class))));
    }
    write("full.bin", &flat(&frame(recipe)));
    write("source_weights.bin", &evidence.source_weights);
    write("target_weights.bin", &evidence.target_weights);
    write("source_presence_weights.bin", &evidence.source_presence_weights);
    write("spatial_weights.bin", &evidence.spatial_weights);
    write("paired.bin", &evidence.paired.iter().map(|&p| if p { 1.0 } else { 0.0 }).collect::<Vec<f32>>());
    std::fs::write(out.join("recipe.json"), serde_json::to_vec_pretty(recipe).expect("recipe json")).expect("write recipe");
    let meta = serde_json::json!({
        "tag": tag,
        "width": s_img.width(),
        "height": s_img.height(),
        "identifiability": evidence.identifiability,
        "paired_share": evidence.paired.iter().filter(|&&p| p).count() as f32 / evidence.paired.len().max(1) as f32,
        "globally_same_content": evidence.globally_same_content,
        "strength_default": crate::recipe::GradeStrength::DEFAULT,
    });
    std::fs::write(out.join("meta.json"), serde_json::to_vec_pretty(&meta).expect("meta")).expect("write meta");
    eprintln!("VALIDITY_DUMP {tag}: {} pixels to {}", tp.len(), out.display());
}

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
/// conversion, reverse-fit from the straight conversion at `strength`, re-read
/// the finished recipe at full size as the entry points do
/// ([`crate::pipeline::reread_at_full_size`], R13b), and return (the base the
/// fit composed, the report). The report's residual is then the terminal
/// full-size reading — what ships (R14); the analysis-size residual the search
/// ended on is printed beside it, and their gap is the measured commutation
/// error of that recipe.
fn round_trip(raw: &std::path::Path, strength: f32, set: impl Fn(&mut EditRecipe)) -> (EditRecipe, FitReport) {
    let (src, target, base) = straight_pair(raw, set);
    let mut report = fit_recipe_from_with(
        &src,
        &target,
        &base,
        FitOptions { strength: GradeStrength::new(strength), provider: None },
    );
    let analysis = report.err_after;
    crate::pipeline::reread_at_full_size(&mut report, Some(raw), &src, &target, &base, crate::diag::silent());
    eprintln!(
        "TERMINAL {}: full-size residual {:.5}; analysis-size residual {analysis:.5} (commutation {:+.5})",
        raw.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        report.err_after,
        report.err_after - analysis
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

/// The white balance's demand is read closed-loop through the engine and
/// a region reading a quantised zero bounds the common ask by an interval
/// of at least one code (`paired_wb_closed_loop`, `wb_common_shared`): one
/// open-loop reading asked 4800 K of this 4300 K render, the re-reads
/// stopped at 4400 K where the shadows' 0.0 vetoed the other regions' ask.
#[test]
fn the_fit_recovers_a_white_balance_it_rendered() {
    let Some(raw) = corpus_raw("p39.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.temperature_k = Some(4300.0));
    let anchor = base.as_shot_k.unwrap_or(5500.0);
    let fitted = rep.recipe.temperature_k.unwrap_or(anchor);
    gate("temperature", &base, &rep, fitted - anchor, 4300.0 - anchor, &["temperature_k", "tint"]);
}

#[test]
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

/// The mixer's amounts are solved jointly on each band's population reach
/// (`fit_band_populations`, `band_population_reach`) and attributed to the
/// band before the frame-domain tone sliders and saturation (the attribution
/// law at the mixer, `solve.rs` 4a): the tone stage read this Orange-only
/// move as exposure −0.2 / saturation +6.6, and the ratio chase split what
/// the mixer kept into Red −10.7 / Orange −14.5 (rel1180 msg-sc case 2).
#[test]
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
        eprintln!("GATE identity {name}: err {:.5} -> {:.5} (terminal); moved {moved:?}", rep.err_before, rep.err_after);
        if rep.err_before > FIT_QUANT || rep.err_after > FIT_QUANT || !moved.is_empty() {
            failures.push(format!(
                "{name}: the identical pair reads {:.5} at the search's zero point and {:.5} at full size (FIT_QUANT {FIT_QUANT}) and moved {} controls",
                rep.err_before, rep.err_after, moved.len()
            ));
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
fn the_fit_recovers_an_exposure_alone_with_a_clipped_sky() {
    let Some(raw) = corpus_raw("p39.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.exposure_ev = 1.0);
    gate("exposure +1.0 EV (sky clipped)", &base, &rep, rep.recipe.exposure_ev - base.exposure_ev, 1.0, &["exposure_ev"]);
}

#[test]
fn the_fit_recovers_an_exposure_alone_just_under_the_clip() {
    let Some(raw) = corpus_raw("p39.arw") else { return };
    let (base, rep) = round_trip(&raw, 0.85, |r| r.exposure_ev = -0.4);
    gate("exposure -0.4 EV (under the clip)", &base, &rep, rep.recipe.exposure_ev - base.exposure_ev, -0.4, &["exposure_ev"]);
}

/// FIT_QUANT's own fixture, read by its definition: the objective of an
/// IDENTICAL pair after the fit's own rounding. The target is the app's
/// render of the straight conversion with every control the global fit
/// stores displaced by half its storage step, all in one direction (the
/// worst a rounded solve of that very target can be left with): exposure
/// 0.01 EV -> 0.005, the tone sliders, saturation, vibrance, tint and the 24
/// mixer values 0.1 -> 0.05, the white balance 50 K -> 25 K. The fit reads it
/// against the straight conversion through its own preview develop (the f32
/// develop is in the reading), and `err_before` IS that objective. The 8-bit
/// curves' half code cannot be rendered (a curve stores integer codes), so it
/// is not in the reading; one code of luma at mid-grey is ~0.4 L*, a curve
/// stores to half of that, and the readings below are dominated by the
/// exposure's half step (0.0036 L* stops ~ 0.1-0.2 L*) at the same order.
/// The constant is the largest reading over the four corpus RAWs and both
/// directions.
#[test]
#[ignore = "measurement: prints the readings FIT_QUANT is taken from; run with --ignored"]
fn fit_quant_is_the_objective_of_the_fits_own_rounding() {
    let mut worst = 0.0f32;
    for name in ["p38.arw", "p39.arw", "p40.arw", "p41.arw"] {
        let Some(raw) = corpus_raw(name) else { return };
        for sign in [1.0f32, -1.0] {
            let (_, rep) = round_trip(&raw, 0.85, |r| {
                r.exposure_ev += sign * 0.005;
                for v in [&mut r.contrast, &mut r.highlights, &mut r.shadows, &mut r.whites, &mut r.blacks,
                    &mut r.saturation, &mut r.vibrance, &mut r.tint]
                {
                    *v += sign * 0.05;
                }
                for band in [&mut r.hsl.hue, &mut r.hsl.saturation, &mut r.hsl.luminance] {
                    for v in band.iter_mut() {
                        *v += sign * 0.05;
                    }
                }
                r.temperature_k = Some(r.temperature_k.or(r.as_shot_k).unwrap_or(5500.0) + sign * 25.0);
            });
            eprintln!("FITQUANT {name} {sign:+}: {:.6}", rep.err_before);
            worst = worst.max(rep.err_before);
        }
    }
    eprintln!("FITQUANT worst {worst:.6} (FIT_QUANT {FIT_QUANT})");
}

/// R13b's measurement: the COMMUTATION error at the recipes the fit ships.
/// For each finished recipe in the list (`AUTOSHADE_COMMUTE_LIST`, lines
/// `tag|raw|recipe.json`; the recipe's rasters resolve beside it), the
/// export of that recipe at the fit's working edge is read by the fit's own
/// steps (`fit_target`, the analysis pair, the evidence model) against the
/// fit's preview of the same recipe on its plate. The two describe one
/// recipe, so every bit of J* between them is the gap between previewing at
/// the analysis size and rendering, then resampling.
#[test]
#[ignore = "measurement: prints the finished recipes' commutation error; run with --ignored"]
fn the_preview_of_a_finished_recipe_against_its_resampled_export() {
    let Some(list) = crate::config::live_env_os("AUTOSHADE_COMMUTE_LIST") else { return };
    let text = std::fs::read_to_string(std::path::PathBuf::from(list)).expect("the list");
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut parts = line.splitn(3, '|');
        let (tag, raw, recipe_path) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
        let (raw, recipe_path) = (std::path::Path::new(raw), std::path::Path::new(recipe_path));
        let mut finished: EditRecipe =
            serde_json::from_str(&std::fs::read_to_string(recipe_path).expect("recipe")).expect("recipe json");
        let dir = recipe_path.parent().expect("recipe dir");
        for mask in &mut finished.masks {
            for path in mask.bitmap_paths_mut() {
                let local = dir.join(path.as_str());
                if local.is_file() {
                    *path = local.to_string_lossy().into_owned();
                }
            }
        }
        let (mut src, mut base) = crate::pipeline::fit_source(raw).expect("the fit source develops");
        let export = render::render_to_image(raw, &finished, None, Some(crate::pipeline::FIT_SOURCE_EDGE)).expect("export");
        let target = crate::pipeline::fit_target(&mut src, Some(raw), &export, &mut base).expect("frame").image;
        let (s_img, t_img) = analysis_pair(&src, &target);
        let (sp, tp) = (pixels_of(&s_img), pixels_of(&t_img));
        let evidence = evidence_model_paired(&sp, &tp, presence_of(&t_img).as_deref(), s_img.width(), s_img.height());
        let t = look::look_terms(&pixels_of(&render::develop_preview(&s_img, &finished)), &tp, &evidence);
        eprintln!(
            "COMMUTE {tag}: J* {:.6} (E_paired {:.5}, E_dist {:.5}, p {:.3}) FIT_QUANT {FIT_QUANT}",
            t.total(), t.paired, t.dist, t.share
        );
    }
}

/// The joint objective's reading of finished GLOBAL recipes on one pair
/// (RULING-1180-joint-r1, the pricing of a curve by its free knots). For each
/// line `tag|raw|target|recipe.json` of `AUTOSHADE_JOINT_LIST`, the recipe's
/// global controls (masks and the colour field stripped) are rendered on the
/// fit's plate and read by J* against the fit target as the solve reads them;
/// dof and F = J* + FIT_QUANT · dof follow. Two finished recipes of one pair
/// read side by side say whether the one with the lower J* also has the
/// lower F — whether the search or the price chose between them. With
/// `AUTOSHADE_JOINT_KEEP_LOCAL` set the recipe is read whole (masks and the
/// colour field kept, rasters resolved beside the recipe): the finished
/// frame on the one ruler.
#[test]
#[ignore = "measurement: prints J*, dof and F of finished global recipes; run with --ignored"]
fn the_joint_objective_of_finished_global_recipes() {
    let Some(list) = crate::config::live_env_os("AUTOSHADE_JOINT_LIST") else { return };
    let text = std::fs::read_to_string(std::path::PathBuf::from(list)).expect("the list");
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut parts = line.splitn(4, '|');
        let (tag, raw, target_path, recipe_path) =
            (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
        let raw = std::path::Path::new(raw);
        let mut global: EditRecipe =
            serde_json::from_str(&std::fs::read_to_string(recipe_path).expect("recipe")).expect("recipe json");
        let keep_local = crate::config::live_env_os("AUTOSHADE_JOINT_KEEP_LOCAL").is_some();
        if keep_local {
            let dir = std::path::Path::new(recipe_path).parent().expect("recipe dir");
            for mask in &mut global.masks {
                for path in mask.bitmap_paths_mut() {
                    let local = dir.join(path.as_str());
                    if local.is_file() {
                        *path = local.to_string_lossy().into_owned();
                    }
                }
            }
        } else {
            global.masks.clear();
            global.colour_field = None;
        }
        let (mut src, mut base) = crate::pipeline::fit_source(raw).expect("the fit source develops");
        let target_img = image::open(target_path).expect("the target");
        let target = crate::pipeline::fit_target(&mut src, Some(raw), &target_img, &mut base).expect("frame").image;
        let (s_img, t_img) = analysis_pair(&src, &target);
        let (sp, tp) = (pixels_of(&s_img), pixels_of(&t_img));
        let evidence = evidence_model_paired(&sp, &tp, presence_of(&t_img).as_deref(), s_img.width(), s_img.height());
        let law = render::WbLaw::of(&base);
        let dof = super::joint::dof_of(&global, &base, law);
        let bp = pixels_of(&render::develop_preview(&s_img, &base));
        let t = look::look_terms(&pixels_of(&render::develop_preview(&s_img, &global)), &tp, &evidence);
        let base_t = look::look_terms(&bp, &tp, &evidence);
        eprintln!(
            "JOINT {tag}{}: J* {:.6} dof {dof} F {:.6} (E_paired {:.5}, E_dist {:.5}, p {:.3}; base J* {:.6}) FIT_QUANT {FIT_QUANT}",
            if keep_local { " (whole)" } else { " (global)" },
            t.total(), t.total() + FIT_QUANT * dof as f32, t.paired, t.dist, t.share, base_t.total()
        );
        // The same render on the plain rulers: CIE76 per pixel, uniform or
        // presence-weighted, as rendered or pooled at the sigma 2 px the
        // objective read until 2026-11-07, and by fifths.
        let (w, h) = (s_img.width() as usize, s_img.height() as usize);
        let rp = pixels_of(&render::develop_preview(&s_img, &global));
        // The joint descent's G on this recipe, read on both models the solve
        // can carry (the structural one in Full mode, the structure-blind one
        // in Atmosphere mode) and, as `Joint::refusal` reads it since
        // 2026-11-09, each class between the recipe WITHOUT it and the
        // recipe's render under the one validity law (`fit::validity`,
        // `joint::without_class`).
        let frame = |r: &EditRecipe| pixels_of(&render::develop_preview(&s_img, r));
        let fpx = frame(&global);
        let withouts: Vec<(super::joint::Refusal, EditRecipe)> =
            super::joint::CLASSES.iter().map(|&c| (c, super::joint::without_class(&base, &global, c))).collect();
        let without_px: Vec<Option<Vec<[f32; 3]>>> =
            withouts.iter().map(|(_, w)| (*w != global).then(|| frame(w))).collect();
        for (model, name) in [(&evidence, "structural"), (&evidence.structure_blind(&tp), "blind")] {
            use super::validity::Cells;
            let cells = Cells::new(&bp, &tp, model);
            let lines: Vec<String> = withouts
                .iter()
                .zip(&without_px)
                .map(|((class, _), before)| match before {
                    Some(before) => cells.describe(class.class(), &cells.judge(class.class(), before, &fpx)),
                    None => format!("{class:?}: unmoved"),
                })
                .collect();
            eprintln!("GATE {tag} ({name}, {} cells): {}", cells.count(), lines.join(" | "));
        }
        if let Some(dump) = crate::config::live_env_os("AUTOSHADE_VALIDITY_DUMP") {
            dump_validity_probe(&std::path::PathBuf::from(dump), tag, &s_img, &base, &global, &tp, &evidence);
        }
        let de_of = |a: &[[f32; 3]], b: &[[f32; 3]]| -> Vec<f32> {
            a.iter()
                .zip(b)
                .map(|(x, y)| {
                    let (x, y) = (super::pixels::cie_lab(x), super::pixels::cie_lab(y));
                    ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
                })
                .collect()
        };
        let de = de_of(&rp, &tp);
        let pooled = de_of(
            &super::structure::low_pass_pixels(&rp, w, h, 2.0),
            &super::structure::low_pass_pixels(&tp, w, h, 2.0),
        );
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
        let wmean = |v: &[f32]| {
            let (mut s, mut m) = (0.0f64, 0.0f64);
            for (i, d) in v.iter().enumerate() {
                let wi = evidence.source_presence_weights.get(i).copied().unwrap_or(0.0).max(0.0) as f64;
                s += wi * *d as f64;
                m += wi;
            }
            (s / m.max(1e-9)) as f32
        };
        let fifths: Vec<String> = (0..5)
            .map(|k| format!("{:.2}", mean(&de[(h * k / 5) * w..(h * (k + 1) / 5) * w])))
            .collect();
        eprintln!(
            "DE76 {tag}{}: uniform {:.3} weighted {:.3} pooled-uniform {:.3} pooled-weighted {:.3} fifths {} (plate {w}x{h})",
            if keep_local { " (whole)" } else { " (global)" },
            mean(&de), wmean(&de), mean(&pooled), wmean(&pooled), fifths.join("/")
        );
    }
}
