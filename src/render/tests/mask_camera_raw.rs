// One part of the engine's tests (src/render/tests.rs includes it): a mask's local controls on Camera Raw's law (v1.17.0, `render::mask_cr`), and the white balance a camera-profile start opens at.

/// A camera-profile recipe at its own as-shot white, with `masks`.
fn camera_raw_masked(exposure_ev: f32, masks: Vec<LocalAdjustment>) -> EditRecipe {
    EditRecipe {
        camera_profile: "Adobe Standard".into(),
        exposure_ev,
        as_shot_k: Some(5400.0),
        as_shot_tint: Some(14.0),
        masks,
        ..Default::default()
    }
}

/// A grey ramp and two colours, one row each, `w` wide.
fn camera_raw_frame(w: usize) -> Vec<[f32; 3]> {
    let mut px: Vec<[f32; 3]> = (0..w).map(|i| [0.02 + 0.9 * i as f32 / (w - 1) as f32; 3]).collect();
    px.extend((0..w).map(|_| [0.55f32, 0.35, 0.20]));
    px.extend((0..w).map(|_| [0.20f32, 0.35, 0.60]));
    px
}

fn developed(r: &EditRecipe, w: usize) -> Vec<[f32; 3]> {
    let mut px = camera_raw_frame(w);
    apply_develop_anon(&mut px, w, 3, r);
    px
}

/// Camera Raw's local Exposure is its global Exposure, and the two add in
/// EV: a +1 EV frame under a −0.25 EV mask renders as a +0.75 EV frame.
///
/// MUTATION: give the mask its own ramp (or none — the old law's plain
/// gain), or run it before the global Exposure's ramp.
#[test]
fn camera_raw_local_exposure_adds_to_the_global_in_ev() {
    let w = 24;
    let full = LocalAdjustment { mask: whole_frame(), amount: 1.0, exposure_ev: -0.25, ..Default::default() };
    let masked = developed(&camera_raw_masked(1.0, vec![full]), w);
    let plain = developed(&camera_raw_masked(0.75, Vec::new()), w);
    for (i, (a, b)) in masked.iter().zip(&plain).enumerate() {
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() < 4e-3, "pixel {i}: {a:?} vs {b:?}");
        }
    }
}

/// On this law a drawn mask that moves nothing leaves the develop exactly
/// as it was, and a moving one moves only what it covers.
///
/// MUTATION: let the local slider pass run for a still mask, or blend a
/// mask's move where its weight is 0.
#[test]
fn camera_raw_local_controls_move_only_what_the_mask_covers() {
    let w = 16;
    let left_half = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 };
    let none = developed(&camera_raw_masked(0.0, Vec::new()), w);
    let still = developed(&camera_raw_masked(0.0, vec![LocalAdjustment { mask: whole_frame(), amount: 1.0, ..Default::default() }]), w);
    assert_eq!(still, none, "a still mask is no move");
    for m in [
        LocalAdjustment { shadows: 60.0, ..Default::default() },
        LocalAdjustment { contrast: 50.0, ..Default::default() },
        LocalAdjustment { dehaze: 40.0, ..Default::default() },
        LocalAdjustment { temperature: -50.0, ..Default::default() },
        LocalAdjustment { hue: 50.0, ..Default::default() },
    ] {
        let out = developed(&camera_raw_masked(0.0, vec![LocalAdjustment { mask: left_half.clone(), amount: 1.0, ..m.clone() }]), w);
        for row in 0..3 {
            let right = row * w + w - 1;
            assert_eq!(out[right], none[right], "{m:?}: the uncovered edge stays");
        }
        let moved = (0..3 * w).filter(|&i| i % w < 3).any(|i| (0..3).any(|c| (out[i][c] - none[i][c]).abs() > 2e-3));
        assert!(moved, "{m:?}: the covered edge moves");
    }
    // Temp −50 cools what it covers: the warm patch loses red to blue.
    let cool = developed(
        &camera_raw_masked(0.0, vec![LocalAdjustment { mask: whole_frame(), amount: 1.0, temperature: -50.0, ..Default::default() }]),
        w,
    );
    let (a, b) = (cool[w], none[w]);
    assert!(a[2] - a[0] > b[2] - b[0] + 0.02, "cooler: {b:?} -> {a:?}");
}

/// A camera-profile recipe opens its white balance AT As Shot — the values
/// Lightroom shows on open — renders exactly what the unstamped recipe
/// rendered, and writes Lightroom's `WhiteBalance="As Shot"`; a recipe that
/// names no profile is left as it was, byte for byte.
///
/// MUTATION: stamp a recipe with no profile, stamp over a moved Tint, drop
/// the white balance stage's skip at As Shot, or write the stamped pair as
/// a Custom white balance.
#[test]
fn a_camera_profile_start_opens_its_white_balance_at_as_shot() {
    let fresh = camera_raw_masked(0.0, Vec::new());
    let mut opened = fresh.clone();
    opened.open_white_balance_at_as_shot();
    assert_eq!((opened.temperature_k, opened.tint), (Some(5400.0), 14.0), "Temp and Tint read As Shot");
    assert!(opened.white_balance_is_as_shot() && !fresh.white_balance_is_as_shot());
    assert_eq!(developed(&opened, 16), developed(&fresh, 16), "the stamp renders the same");
    let xmp = crate::xmp::recipe_to_xmp(&opened);
    assert!(xmp.contains(r#"crs:WhiteBalance="As Shot""#), "{xmp}");
    assert!(xmp.contains(r#"crs:Temperature="5400""#) && xmp.contains(r#"crs:Tint="+14""#), "{xmp}");
    // A moved Tint is a white balance of the user's: not stamped over.
    let mut moved = EditRecipe { tint: -20.0, ..fresh.clone() };
    moved.open_white_balance_at_as_shot();
    assert_eq!((moved.temperature_k, moved.tint), (None, -20.0));
    // …and once Temperature leaves As Shot, the writer's white balance is Custom.
    let custom = EditRecipe { temperature_k: Some(6000.0), ..opened.clone() };
    assert!(crate::xmp::recipe_to_xmp(&custom).contains(r#"crs:WhiteBalance="Custom""#));
    // A recipe with no camera profile: nothing changes, to the byte.
    let old = EditRecipe { as_shot_k: Some(5400.0), as_shot_tint: Some(14.0), ..Default::default() };
    let mut kept = old.clone();
    kept.open_white_balance_at_as_shot();
    assert_eq!(serde_json::to_string(&kept).unwrap(), serde_json::to_string(&old).unwrap());
    assert_eq!(crate::xmp::recipe_to_xmp(&kept), crate::xmp::recipe_to_xmp(&old));
    assert!(!kept.white_balance_is_as_shot());
}
