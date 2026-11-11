//! The self-consistency gate's zoned control (v1.18.0): the mask Color tint,
//! set on the calibration RAW's sky zone, rendered by the app itself and
//! reverse-fitted from the straight conversion with the same sky raster
//! handed to the segmenter. See `fit::self_consistency` for the gate's three
//! assertions; the zone's tint is the control set, everything else the fit
//! carries — global controls, other masks, the colour field — is a control
//! not set.

use super::*;

/// The gate's pair: the calibration RAW's straight conversion, the app's own
/// render of it with a Color tint of 30 at hue 200 on the corpus develop's
/// sky zone, the base the fit starts from, and the corpus's sky raster at the
/// frame's size saved under a scratch path named `name`.
fn mask_tint_case(name: &str) -> Option<(crate::render::Plate, DynamicImage, crate::recipe::EditRecipe, GrayImage, crate::store::OwnedRaster)> {
    let root = fit::calibration_corpus()?;
    let raw = root.join("source.arw");
    if !raw.is_file() {
        crate::test_skipped("self-consistency gate", "the corpus carries no source.arw");
        return None;
    }
    let saved = fit::calibration_recipe(&root);
    let sky_geometry = saved
        .masks
        .iter()
        .find(|m| m.role == crate::recipe::MaskRole::ZoneSky)
        .expect("the corpus develop carries its sky zone")
        .mask
        .clone();
    let (neutral, target, base) = fit::self_consistency::straight_pair(&raw, |made| {
        made.masks = vec![crate::recipe::LocalAdjustment {
            mask: sky_geometry.clone(),
            role: crate::recipe::MaskRole::ZoneSky,
            toning_hue: 200.0,
            toning_amount: 30.0,
            ..Default::default()
        }];
    });
    let sky = image::imageops::resize(
        &image::open(root.join("sky-mask.png")).expect("sky raster").to_luma8(),
        neutral.width(),
        neutral.height(),
        image::imageops::FilterType::Triangle,
    );
    let path = tests::fixture_mask_path(name);
    sky.save(path.path()).unwrap();
    Some((neutral, target, base, sky, path))
}

/// The zone layer of the gate below by itself (the alone check's probe, no
/// global solve, well under a minute): the two semantic zones attached on
/// the base carry the tint that was set — its hue, its amount, nothing on
/// the zone's tone or gains — through the boundary gate, and the frame lands
/// within the fit's own rounding. Two defects held it at 11 of 30 and a frame
/// of 0.0155 (2026-11-10): the tint read after the zone's tone
/// (`one_zone::attach_one_zone_as`) and the boundary ruler's multiplier
/// transport (`boundary::Owed`).
#[test]
fn the_zones_alone_on_the_base_carry_a_mask_colour_tint_whole() {
    let Some((neutral, target, base, sky, path)) = mask_tint_case("self-consistency-mask-tint-zones") else { return };
    let rep = super::entry::probe_on_base_for_tests(&neutral, &target, &path, &base, (sky.clone(), sky));
    for m in &rep.recipe.masks {
        eprintln!(
            "PROBE mask {:?} amount {}: ev {} con {} hi {} sh {} wh {} bl {} gains {:?} sat {} tint {} at {}",
            m.role, m.amount, m.exposure_ev, m.contrast, m.highlights, m.shadows, m.whites, m.blacks, m.color_gains, m.saturation, m.toning_amount, m.toning_hue
        );
    }
    eprintln!("PROBE frame: {:.5} -> {:.5}\n{}", rep.err_before, rep.err_after, rep.recipe.rationale);
    path.remove();
    let zone = rep.recipe.masks.iter().find(|m| m.role == crate::recipe::MaskRole::ZoneSky).expect("the sky zone ships");
    assert!(
        (zone.toning_amount - 30.0).abs() <= 2.0 && (zone.toning_hue - 200.0).abs() <= 5.0,
        "the sky zone carries a tint of {} at {}, of 30 at 200 set",
        zone.toning_amount,
        zone.toning_hue
    );
    let tone = [zone.exposure_ev, zone.contrast, zone.highlights, zone.shadows, zone.whites, zone.blacks];
    assert!(
        tone.iter().all(|v| *v == 0.0) && zone.color_gains.is_none_or(|g| g == [1.0; 3]) && zone.saturation == 0.0,
        "a tint and nothing else was set: tone {tone:?} gains {:?} saturation {}",
        zone.color_gains,
        zone.saturation
    );
    assert!(rep.err_after <= fit::FIT_QUANT, "the frame lands at {:.5} > FIT_QUANT {}", rep.err_after, fit::FIT_QUANT);
}

#[test]
fn the_zoned_fit_recovers_a_mask_colour_tint_it_rendered() {
    let Some((neutral, target, base, sky, path)) = mask_tint_case("self-consistency-mask-tint") else { return };
    SEGMENT_BOTH_OVERRIDE.with(|value| *value.borrow_mut() = Some((sky.clone(), sky.clone())));
    let seg = SegmentOpts {
        python_bin: "unused-by-segmentation-test-override".into(),
        script: "unused-by-segmentation-test-override".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let rep = fit_recipe_zoned_from(&neutral, &target, &seg, &path, &base);
    let zone = rep.recipe.masks.iter().find(|m| m.role == crate::recipe::MaskRole::ZoneSky);
    let (hue, amount) = zone.map_or((0.0, 0.0), |m| (m.toning_hue, m.toning_amount));
    let mut failures = Vec::new();
    if rep.err_after > fit::FIT_QUANT {
        failures.push(format!("the finished frame lands at {:.5} > FIT_QUANT {}", rep.err_after, fit::FIT_QUANT));
    }
    if amount < 15.0 || (hue - 200.0).abs() > 30.0 {
        failures.push(format!("the sky zone carries a tint of {amount:.1} at {hue:.1} deg, of 30 at 200 set"));
    }
    let others: Vec<String> = rep
        .recipe
        .masks
        .iter()
        .filter(|m| m.role != crate::recipe::MaskRole::ZoneSky)
        .map(|m| format!("{:?} {}", m.role, m.name))
        .chain(rep.recipe.colour_field.as_ref().map(|_| "colour field".to_string()))
        .chain(
            [
                ("exposure", rep.recipe.exposure_ev - base.exposure_ev),
                ("contrast", rep.recipe.contrast - base.contrast),
                ("saturation", rep.recipe.saturation - base.saturation),
                ("tint", rep.recipe.tint - base.tint),
            ]
            .into_iter()
            .filter(|(_, d)| *d != 0.0)
            .map(|(k, d)| format!("{k} {d:+.2}")),
        )
        .chain(rep.recipe.temperature_k.map(|k| format!("temperature {k}")))
        .chain((!rep.recipe.hsl.is_neutral()).then(|| "mixer".to_string()))
        .chain((!rep.recipe.color_grade.is_neutral()).then(|| "grade".to_string()))
        .chain((!rep.recipe.red_curve.is_empty()).then(|| "cast curves".to_string()))
        .collect();
    if !others.is_empty() {
        failures.push(format!("controls not set moved: {others:?}"));
    }
    eprintln!("GATE mask tint: err {:.5} -> {:.5}; zone tint {amount:.1} at {hue:.1}; others {others:?}", rep.err_before, rep.err_after);
    path.remove();
    assert!(failures.is_empty(), "mask tint: {}\n{}", failures.join("; "), rep.recipe.rationale);
}
