// One part of the engine's tests (src/render/tests.rs includes it): Temperature and Tint on Camera Raw's law (v1.17.0) — a recipe that names a camera profile white-balances as Camera Raw does, and its eyedropper inverts that.

/// A recipe on Camera Raw's law at its own as-shot white.
fn camera_raw_wb_recipe(kelvin: f32, tint: f32) -> EditRecipe {
    EditRecipe {
        camera_profile: "Adobe Standard".into(),
        temperature_k: Some(kelvin),
        tint,
        as_shot_k: Some(5400.0),
        as_shot_tint: Some(14.0),
        // The measured frame's own start: its body's +0.35 EV baseline
        // ("Adobe Standard" offsets nothing) and its As Shot neutral's full
        // stop of headroom.
        camera_start: Some(crate::recipe::CameraStart { ev: 0.35, headroom: 1.0 }),
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

/// The reverse fit's white-balance law reads As Shot where the render puts
/// it: on Camera Raw's law the stamped as-shot tint, on the engine's 0 — and
/// a recipe with no Temperature and no Tint IS As Shot on both.
///
/// MUTATION: return `(anchor, 0)` as As Shot on Camera Raw's law (the fit then
/// writes a green shift of the as-shot tint into every white balance it
/// moves), or read a bare recipe as `(anchor, 0)`.
#[test]
fn the_fits_white_balance_law_reads_as_shot_where_the_render_does() {
    let open = EditRecipe { temperature_k: None, tint: 0.0, ..camera_raw_wb_recipe(5400.0, 14.0) };
    let law = WbLaw::of(&open);
    assert_eq!(law.as_shot(), (5400.0, 14.0));
    assert_eq!(law.point_of(&open), (5400.0, 14.0));
    assert_eq!(law.gains(law.point_of(&open)), [1.0; 3]);
    assert_eq!(law.point(Some(6000.0), 0.0), (6000.0, 0.0), "a Temperature with Tint 0 is absolute 0");
    let engine = EditRecipe { camera_profile: String::new(), ..open };
    assert_eq!(WbLaw::of(&engine).as_shot(), (5400.0, 0.0));
}

/// The gains the fit reasons with are the render's own: on both laws,
/// [`WbLaw::gains`] is what [`apply_recipe_wb`] does to a mid grey, read as
/// chromaticity (green 1).
///
/// MUTATION: compute the fit's gains with `wb_gains` on Camera Raw's law (the
/// v1.16.0 arithmetic), or read them at another level than the render's grey.
#[test]
fn the_fits_white_balance_gains_are_what_the_render_does_to_grey() {
    for profile in ["Adobe Standard", ""] {
        for (k, t) in [(3200.0f32, 14.0f32), (8800.0, 16.6), (15000.0, -20.0), (5400.0, 60.0), (23500.0, 75.8)] {
            let r = EditRecipe { camera_profile: profile.into(), ..camera_raw_wb_recipe(k, t) };
            let mut px = vec![[linear_to_srgb(0.18); 3]];
            apply_recipe_wb(&mut px, &r);
            let lin = px[0].map(srgb_to_linear);
            let rendered = lin.map(|v| v / lin[1]);
            let law = WbLaw::of(&r);
            // The engine law's gains carry its tint on green; compare chromaticity.
            let fit = law.gains(law.point_of(&r)).map(|v| v / law.gains(law.point_of(&r))[1]);
            for c in 0..3 {
                assert!(
                    (fit[c] - rendered[c]).abs() < 4e-3,
                    "{profile:?} {k} K / {t:+}: the fit reads {fit:?}, the render does {rendered:?}"
                );
            }
        }
    }
}

/// Camera Raw's law inverts its own moves: a demand the law itself produced
/// at (Temperature, Tint) comes back as that pair, to the solve's 50 K /
/// 0.1 quantisation (within one mired and half a tint unit).
///
/// MUTATION: invert with the engine law's separable two-step on Camera Raw's
/// law (its Temperature carries green, so the Kelvin then comes back wrong),
/// or search the tint from 0 rather than from the as-shot tint.
#[test]
fn camera_raws_law_inverts_its_own_white_balance_moves() {
    let law = WbLaw::of(&camera_raw_wb_recipe(5400.0, 14.0));
    for (k, t) in [(4000.0f32, 14.0f32), (8800.0, 16.6), (3200.0, 50.0), (15000.0, -20.0), (6500.0, -60.0), (25000.0, 40.0)] {
        let (k2, t2) = law.invert(law.log_gains((k, t)), (2000.0, 40000.0));
        assert!((1e6 / k2 - 1e6 / k).abs() <= 1.0, "{k} K / {t:+}: inverted {k2} K / {t2:+}");
        assert!((t2 - t).abs() <= 0.5, "{k} K / {t:+}: inverted {k2} K / {t2:+}");
    }
}

/// CIE Lab (D65) of a linear sRGB triple. The matrix is IEC 61966-2-1's
/// published sRGB → XYZ, digits as published.
#[allow(clippy::excessive_precision)]
fn lab_of_linear(c: [f32; 3]) -> [f32; 3] {
    let m = [[0.4124564f32, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];
    let w = [0.95047f32, 1.0, 1.08883];
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let xyz: [f32; 3] = std::array::from_fn(|i| f((m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2]) / w[i]));
    [116.0 * xyz[1] - 16.0, 500.0 * (xyz[0] - xyz[1]), 200.0 * (xyz[1] - xyz[2])]
}

/// Camera Raw's white balance at the fits' extremes (v1.17.0, measured): p39
/// from its as-shot 5400 K / +14, the flat packet of the user's export
/// rendered by Camera Raw at each (Temperature, Tint), and the mean Lab
/// change of the frame's neutral pixels (chroma under 6) at L 40–60 — 24 720
/// pixels — against the change the law makes to a mid grey. The engine's
/// value is pinned within 0.75 of the miss it had when this was measured, so
/// the law can get better here and cannot quietly get worse.
///
/// What it says about the law: from 3000 K to 40000 K and Tint ±100 it
/// holds to 2.6 a* and 5.6 b* on a grey, but not evenly — on the warm side it
/// moves b* 1.14–1.19 times as far as Camera Raw and brightens 1.0–1.7 L
/// less, its green tint moves a* 1.19–1.26 times as far, and with warm and
/// magenta together Camera Raw brightens 8.4–9.1 L where the law brightens
/// 3.5. Below 3000 K it falls away: at 2500 K Camera Raw is 6.1 b* bluer, at
/// 2000 K 13.3. Camera Raw takes its white balance in the camera's own space,
/// before the profile's hue/saturation tables (the DNG pipeline the profile
/// stage already builds at the as-shot white); one Bradford adaptation of
/// the rendered frame, fitted at 4000–10000 K, is a stand-in for it, and
/// these misses are the stand-in's shape away from where it was fitted. The
/// camera matrices re-solved at the new white WITHOUT the tables move only
/// 0.6–0.75 of Camera Raw's chroma (measured offline on the same renders),
/// so the tables are part of the law and are not modelled. The reverse fit
/// reads the law only through [`WbLaw`], so it inherits exactly these misses.
///
/// MUTATION: change any of the four `WB_*` constants, the clamp, or the
/// as-shot white the move starts from.
#[test]
fn camera_raws_white_balance_at_the_fits_extremes_is_pinned_to_its_measured_miss() {
    // (Temperature, Tint, Camera Raw's ΔL Δa Δb, the law's miss when measured: engine − Camera Raw)
    // v1.17.0: the two warmest rows re-read once the white balance acted
    // under Camera Raw's measured exposure transfer at the frame's own start
    // (`tone_cr::Ramp`) rather than the earlier fitted ramp: their miss
    // shrank by 0.6–0.8 (2000 K was −3.97 −10.64 +13.26, 2500 K −1.69 −3.88
    // +6.13); every other row moved by 0.46 or less (3000 K's b*).
    #[allow(clippy::type_complexity)]
    let measured: [(f32, f32, [f32; 3], [f32; 3]); 16] = [
        (2000.0, 14.0, [-0.05, 24.41, -72.27], [-3.87, -9.95, 12.46]),
        (2500.0, 14.0, [-1.92, 10.88, -58.28], [-1.61, -3.31, 5.43]),
        (3000.0, 14.0, [-1.45, -0.05, -40.82], [-1.62, 0.35, -0.61]),
        (4000.0, 14.0, [-1.12, -1.48, -17.69], [-0.18, -0.26, -2.33]),
        (7500.0, 14.0, [2.13, 3.80, 12.91], [-0.98, -0.85, 2.44]),
        (10000.0, 14.0, [3.31, 6.45, 20.40], [-1.35, -1.33, 3.93]),
        (15000.0, 14.0, [4.28, 9.03, 27.29], [-1.49, -1.85, 4.97]),
        (25000.0, 14.0, [4.84, 10.89, 32.01], [-1.41, -2.19, 5.60]),
        (40000.0, 14.0, [5.17, 11.31, 34.25], [-1.54, -2.15, 4.87]),
        (5400.0, -100.0, [-0.08, -24.04, 19.79], [0.25, -6.30, 0.79]),
        (5400.0, -60.0, [-0.11, -16.15, 13.23], [0.19, -3.00, 0.80]),
        (5400.0, 60.0, [0.73, 11.26, -9.00], [-0.70, 0.39, -1.73]),
        (5400.0, 100.0, [1.37, 23.36, -18.07], [-1.33, -1.84, -3.37]),
        (25000.0, 76.0, [8.44, 27.16, 34.30], [-4.88, -3.32, 2.68]),
        (40000.0, -26.0, [4.93, 0.53, 35.02], [-1.58, -2.06, 4.37]),
        (15000.0, 100.0, [9.08, 31.70, 29.08], [-6.17, -4.64, 0.80]),
    ];
    let grey = [linear_to_srgb(0.18); 3];
    let before = lab_of_linear([0.18; 3]);
    for (k, t, camera_raw, miss) in measured {
        let mut px = vec![grey];
        apply_recipe_wb(&mut px, &camera_raw_wb_recipe(k, t));
        let after = lab_of_linear(px[0].map(srgb_to_linear));
        for c in 0..3 {
            let now = after[c] - before[c] - camera_raw[c];
            assert!(
                (now - miss[c]).abs() <= 0.75,
                "{k} K / {t:+}: channel {c} misses Camera Raw by {now:+.2} (measured {:+.2})",
                miss[c]
            );
        }
    }
}
