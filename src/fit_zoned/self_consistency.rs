//! The self-consistency gate's zoned control (v1.18.0): the mask Color tint,
//! set on the calibration RAW's sky zone, rendered by the app itself and
//! reverse-fitted from the straight conversion with the same sky raster
//! handed to the segmenter. See `fit::self_consistency` for the gate's three
//! assertions; the zone's tint is the control set, everything else the fit
//! carries — global controls, other masks, the colour field — is a control
//! not set.

use super::*;

#[test]
#[ignore = "self-consistency gate: fails at v1.18.0 start, a defect this version fixes (rel1180 msg2); run with --ignored"]
fn the_zoned_fit_recovers_a_mask_colour_tint_it_rendered() {
    let Some(root) = fit::calibration_corpus() else { return };
    let raw = root.join("source.arw");
    if !raw.is_file() {
        crate::test_skipped("self-consistency gate", "the corpus carries no source.arw");
        return;
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
    let path = tests::fixture_mask_path("self-consistency-mask-tint");
    sky.save(path.path()).unwrap();
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
