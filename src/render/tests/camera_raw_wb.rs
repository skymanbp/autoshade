// One part of the engine's tests (src/render/tests.rs includes it): Temperature and Tint on Camera Raw's law (v1.17.0) — a recipe that names a camera profile white-balances as Camera Raw does, and its eyedropper inverts that.

/// A recipe on Camera Raw's law at its own as-shot white.
fn camera_raw_wb_recipe(kelvin: f32, tint: f32) -> EditRecipe {
    EditRecipe {
        camera_profile: "Adobe Standard".into(),
        temperature_k: Some(kelvin),
        tint,
        as_shot_k: Some(5400.0),
        as_shot_tint: Some(14.0),
        ..Default::default()
    }
}

/// Temperature and Tint at the as-shot white leave a camera-profile frame as
/// it was: Camera Raw's tint is absolute, so the as-shot +14 is no move —
/// where the engine's own law reads the same +14 as a magenta shift.
///
/// MUTATION: shift the tint from 0 instead of from the as-shot tint, or let
/// the stage's undo and redo disagree.
#[test]
fn camera_raw_white_balance_at_the_as_shot_white_moves_nothing() {
    let mut px = vec![[0.42f32, 0.40, 0.37], [0.1, 0.2, 0.5], [0.8, 0.6, 0.3]];
    let before = px.clone();
    apply_recipe_wb(&mut px, &camera_raw_wb_recipe(5400.0, 14.0));
    for (a, b) in px.iter().zip(&before) {
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() < 3e-3, "{b:?} -> {a:?}");
        }
    }
    let mut engine = before.clone();
    apply_recipe_wb(&mut engine, &EditRecipe { camera_profile: String::new(), ..camera_raw_wb_recipe(5400.0, 14.0) });
    assert!((engine[0][1] - before[0][1]).abs() > 5e-3, "the engine law reads +14 as a move: {:?}", engine[0]);
}

/// Cooling the white makes a grey bluer and a little darker (Camera Raw's
/// measured exposure per mired), warming it the other way, and a green tint
/// raises green.
///
/// MUTATION: flip the adaptation's direction, or drop `WB_EV_PER_MIRED`.
#[test]
fn camera_raw_white_balance_cools_warms_and_tints_a_grey() {
    let grey = [0.45f32; 3];
    let run = |k: f32, t: f32| {
        let mut px = vec![grey];
        apply_recipe_wb(&mut px, &camera_raw_wb_recipe(k, t));
        px[0]
    };
    let cool = run(4000.0, 14.0);
    assert!(cool[2] > cool[0] + 0.03, "4000 K cools: {cool:?}");
    let luma = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    assert!(luma(cool) < luma(grey), "and darkens: {cool:?}");
    let warm = run(7500.0, 14.0);
    assert!(warm[0] > warm[2] + 0.03, "7500 K warms: {warm:?}");
    let green = run(5400.0, -30.0);
    assert!(green[1] > green[0].max(green[2]) + 0.01, "tint −30 is green: {green:?}");
}

/// The eyedropper on a camera-profile recipe solves the white Camera Raw's
/// law renders the clicked pixel neutral at, in Camera Raw's absolute tint.
///
/// MUTATION: solve with the engine law's model, or search tint from 0 only.
#[test]
fn the_eyedropper_on_camera_raws_law_neutralises_the_pixel_it_solves() {
    let base = camera_raw_wb_recipe(5400.0, 14.0);
    for px in [[0.55f32, 0.48, 0.38], [0.35, 0.40, 0.52], [0.50, 0.56, 0.47]] {
        let (k, tint) = solve_wb_for(px, &base);
        let mut out = vec![px];
        apply_recipe_wb(&mut out, &EditRecipe { temperature_k: Some(k), tint, ..base.clone() });
        let o = out[0];
        let spread = o[0].max(o[1]).max(o[2]) - o[0].min(o[1]).min(o[2]);
        assert!(spread < 0.01, "{px:?} solved {k} K / {tint:+.1} renders {o:?}");
    }
    // The engine law keeps its own solver.
    let engine = EditRecipe { camera_profile: String::new(), ..base };
    assert_eq!(solve_wb_for([0.55, 0.48, 0.38], &engine), solve_wb_from_neutral([0.55, 0.48, 0.38], 5400.0));
}
