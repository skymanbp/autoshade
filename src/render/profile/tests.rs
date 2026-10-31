use super::*;
use crate::dcp::{Profile, Table};

/// A profile carrying exactly what a test asks for and nothing else.
fn bare() -> Profile {
    Profile {
        name: "Test".into(),
        unique_model: "TEST-1".into(),
        illuminant: [Some(17), Some(21)],
        color_matrix: [None, None],
        forward_matrix: [None, None],
        hue_sat_map: [None, None],
        look_table: None,
        tone_curve: vec![[0.0, 0.0], [1.0, 1.0]],
        baseline_exposure_offset: 0.0,
        has_third_illuminant: false,
    }
}

/// A flat table that states one correction everywhere.
fn flat(delta: [f32; 3]) -> Table {
    Table { hue: 2, sat: 2, val: 1, srgb_value: false, data: vec![delta; 4] }
}

const SPACE: crate::render::ExportColorSpace = crate::render::ExportColorSpace::Srgb;

/// A profile with no tables and no matrices is not a stage at all, whatever
/// tone it states: the stage is colour data only (v1.17.0), so a profile's
/// tone curve and baseline exposure offset never make one.
///
/// MUTATION: build a stage for a curve or an offset.
#[test]
fn a_profile_with_no_tables_is_not_a_stage_whatever_tone_it_states() {
    assert!(Stage::build(&bare(), SPACE, Some(5500.0)).is_none(), "nothing to do");
    let toned = Profile {
        tone_curve: vec![[0.0, 0.0], [0.25, 0.4], [1.0, 1.0]],
        baseline_exposure_offset: -0.35,
        ..bare()
    };
    assert!(Stage::build(&toned, SPACE, Some(5500.0)).is_none(), "tone is not the stage's");
}

/// The reference round trip is EXACT when no table acts, so the two matrices
/// really are inverses and the stage cannot tint a picture by existing: what
/// comes back is the colour it was given.
///
/// What this can and cannot see, stated because the F7 mutation sweep asked:
/// it sees an ASYMMETRY between the two matrices and nothing else. Building
/// `from_ref` from the primaries again rather than inverting `to_ref` is
/// `inv(A⁻¹B) = B⁻¹A`, the same matrix, so that mutation cannot be driven at
/// all; and changing the reference WHITE on both sides at once leaves an exact
/// round trip through a wrong space. The white point belongs to
/// [`a_saturation_scale_of_zero_renders_the_frame_grey`], which is where a
/// neutral stops coming back neutral.
///
/// MUTATION: build `from_ref` at a different white than `to_ref`.
#[test]
fn the_reference_round_trip_returns_the_colour_it_was_given() {
    let mut p = bare();
    // An identity table: a stage that does nothing but convert to the
    // reference space and back.
    p.hue_sat_map[0] = Some(flat([0.0, 1.0, 1.0]));
    let s = Stage::build(&p, SPACE, Some(5500.0)).expect("a table makes a stage");
    for px in [[0.2f32, 0.5, 0.8], [1.0, 1.0, 1.0], [0.04, 0.04, 0.04], [0.9, 0.1, 0.3]] {
        let mut got = px;
        s.apply(&mut got);
        let want = px;
        for ch in 0..3 {
            assert!(
                (got[ch] - want[ch]).abs() < 2e-4,
                "an identity table changed {px:?} into {got:?} (channel {ch})"
            );
        }
    }
}

/// A saturation scale from the table really desaturates the PICTURE — the
/// claim the "Camera BW" profiles rest on, where the mean scale is 0.0039.
///
/// MUTATION: apply the table's factors to the working-space RGB directly
/// instead of through HSV.
#[test]
fn a_saturation_scale_of_zero_renders_the_frame_grey() {
    let mut p = bare();
    p.look_table = Some(flat([0.0, 0.0, 1.0]));
    let s = Stage::build(&p, SPACE, None).expect("a look table makes a stage");
    let mut px = [0.8f32, 0.2, 0.1];
    s.apply(&mut px);
    let (hi, lo) = (px[0].max(px[1]).max(px[2]), px[0].min(px[1]).min(px[2]));
    assert!(hi - lo < 5e-3, "saturation 0 must leave a grey: {px:?}");
    // …and it is a GREY, not black: desaturating in HSV keeps the VALUE, which
    // is the reference-space maximum, so the answer sits between the input's
    // darkest and brightest channels rather than collapsing. Measured 0.4916
    // for this pixel; the band is wide because the number belongs to the
    // primaries, and the claim is that no luminance was invented or lost.
    assert!((0.2..=0.8).contains(&hi), "a grey between the input's own ends: {px:?}");
}

/// A hue shift turns the colour by the degrees the table states.
///
/// MUTATION: read the shift as a fraction of the circle rather than as degrees.
#[test]
fn a_hue_shift_turns_the_colour_by_that_many_degrees() {
    let mut p = bare();
    p.look_table = Some(flat([120.0, 1.0, 1.0]));
    let s = Stage::build(&p, SPACE, None).expect("stage");
    // Pure red in the reference space, turned by 120°, has to come back green:
    // whatever the working space, the GREEN channel must end up the largest.
    let mut px = [0.6f32, 0.05, 0.05];
    s.apply(&mut px);
    assert!(px[1] > px[0] && px[1] > px[2], "120° from red is green, got {px:?}");
}

/// The two calibrations blend in RECIPROCAL temperature, and the ends are the
/// ends.
///
/// MUTATION: blend linearly in kelvin, or ignore the white balance entirely.
#[test]
fn the_calibrations_blend_by_the_frames_own_white_balance() {
    let mut p = bare();
    p.hue_sat_map = [Some(flat([0.0, 1.0, 1.0])), Some(flat([60.0, 1.0, 1.0]))];
    // Standard A (2856 K) and D65 (6504 K) are the two ends Sony's profiles use.
    let at = |k: f32| {
        let t = blend_calibrations(&p, Some(k)).expect("a blend");
        t.data[0][0]
    };
    assert!((at(2856.0) - 0.0).abs() < 1e-3, "at the first illuminant: {}", at(2856.0));
    assert!((at(6504.0) - 60.0).abs() < 1e-3, "at the second: {}", at(6504.0));
    // The midpoint in RECIPROCAL temperature is 1/((1/2856 + 1/6504)/2) ≈ 3969 K,
    // where the blend is half. A linear-in-kelvin blend would put the half-way
    // point at 4680 K instead, so this number is the whole test.
    let mid = 1.0 / ((1.0 / 2856.0 + 1.0 / 6504.0) / 2.0);
    assert!((at(mid) - 30.0).abs() < 0.5, "reciprocal midpoint {mid:.0} K gave {}", at(mid));
    assert!(at(4680.0) > 30.0, "a linear blend would read 30 here, not {}", at(4680.0));
    // No white balance at all takes the FIRST calibration rather than guessing.
    assert!((blend_calibrations(&p, None).expect("first").data[0][0]).abs() < 1e-6);
    // …and so does a frame whose two calibrations are the same illuminant.
    p.illuminant = [Some(21), Some(21)];
    assert!((blend_calibrations(&p, Some(6000.0)).expect("first").data[0][0]).abs() < 1e-6);
}

/// HSV round-trips, including the cases that make a naive implementation
/// produce a hue out of nothing.
///
/// MUTATION: drop the achromatic guard in `to_hsv` (a grey then gets whatever
/// hue the division by a zero chroma produces).
#[test]
fn the_hsv_round_trip_survives_grey_black_and_the_hue_wrap() {
    for px in [
        [0.3f32, 0.3, 0.3],
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.51, 0.02, 0.77],
        [0.9, 0.9, 0.1],
    ] {
        let hsv = to_hsv(px);
        assert!(hsv[0].is_finite() && (0.0..360.0).contains(&hsv[0]), "{px:?} → {hsv:?}");
        assert!((0.0..=1.0).contains(&hsv[1]), "{px:?} → {hsv:?}");
        let back = from_hsv(hsv);
        for ch in 0..3 {
            assert!((back[ch] - px[ch]).abs() < 1e-5, "{px:?} → {hsv:?} → {back:?}");
        }
    }
    // A grey has no hue, and must not be given one.
    assert_eq!(to_hsv([0.42, 0.42, 0.42])[1], 0.0);
    assert_eq!(to_hsv([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
}


// --- the colour spec (v1.17.0): ForwardMatrix + the dual-illuminant weight ---

/// The DNG SDK's temperature (Robertson's method) puts the two calibration
/// illuminants where their EXIF codes say they are.
///
/// MUTATION: interpolate reciprocal temperature with `f` and `1 − f` swapped.
#[test]
fn robertsons_method_reads_illuminant_a_and_d65_at_their_temperatures() {
    let d65 = xy_to_kelvin([0.3127, 0.3290]);
    let a = xy_to_kelvin([0.44757, 0.40745]);
    assert!((d65 - 6504.0).abs() < 15.0, "D65 read as {d65:.1} K");
    assert!((a - 2856.0).abs() < 10.0, "A read as {a:.1} K");
    assert!(xy_to_kelvin([0.3457, 0.3585]) > a && xy_to_kelvin([0.3457, 0.3585]) < d65);
}

/// The SDK's linearised Bradford takes the D50 white to the D65 white.
///
/// MUTATION: build the adaptation as `Mb · A · Mb⁻¹`.
#[test]
fn the_bradford_adaptation_maps_white_to_white() {
    let m = map_white(D50_XY, D65_XY64);
    let w = vec64(&m, xy_to_xyz(D50_XY));
    let want = xy_to_xyz(D65_XY64);
    for i in 0..3 {
        assert!((w[i] - want[i]).abs() < 1e-9, "{w:?} vs {want:?}");
    }
}

/// A synthetic two-illuminant profile with ForwardMatrices: the stage owns
/// the camera transform, and a white-balanced grey (camera 1,1,1 at any
/// level) comes out as the SAME grey in the working space — the white
/// preservation the engine's own matrix path has always had.
fn with_matrices() -> Profile {
    Profile {
        color_matrix: [
            Some([[1.10, -0.35, -0.05], [-0.45, 1.30, 0.15], [-0.05, 0.25, 0.60]]),
            Some([[0.80, -0.25, -0.05], [-0.40, 1.25, 0.15], [-0.08, 0.20, 0.70]]),
        ],
        forward_matrix: [
            Some([[0.70, 0.15, 0.10], [0.25, 0.80, -0.05], [0.00, -0.10, 0.92]]),
            Some([[0.75, 0.10, 0.10], [0.30, 0.75, -0.05], [0.02, -0.12, 0.93]]),
        ],
        ..bare()
    }
}

fn neutral_at(p: &Profile, which: usize, xy: [f64; 2]) -> [f32; 3] {
    let cm = mat3_f64(&p.color_matrix[which].expect("matrix"));
    vec64(&cm, xy_to_xyz(xy)).map(|v| v as f32)
}

/// MUTATION: skip the ForwardMatrix normalisation; drop the Bradford step.
#[test]
fn a_profile_with_a_forward_matrix_owns_the_transform_and_keeps_grey_grey() {
    let p = with_matrices();
    let frame = Frame { neutral: Some(neutral_at(&p, 1, D65_XY64)), ..Frame::default() };
    let s = Stage::build_for(&p, SPACE, frame).expect("matrices make a stage");
    assert!(s.owns_camera());
    for level in [0.02f32, 0.18, 0.6] {
        let out = s.develop_camera([level; 3]);
        let want = level;
        for c in out {
            assert!((c - want).abs() < 2e-4, "grey {level} came out {out:?}, not {want}");
        }
    }
    // Without a neutral the white cannot be solved, and the stage leaves the
    // matrix to the engine's own path — it does not invent a white.
    let s = Stage::build_for(&p, SPACE, Frame::default());
    assert!(s.is_none_or(|s| !s.owns_camera()));
    // …and a profile with ColorMatrix but no ForwardMatrix keeps that path too.
    let no_fm = Profile { forward_matrix: [None, None], ..with_matrices() };
    let frame = Frame { neutral: Some(neutral_at(&no_fm, 1, D65_XY64)), ..Frame::default() };
    assert!(Stage::build_for(&no_fm, SPACE, frame).is_none_or(|s| !s.owns_camera()));
}

/// The white solved from a camera neutral (the SDK's `NeutralToXY`) lands on
/// the illuminant whose matrix produced it, and the calibration weight moves
/// monotonically with temperature between the two.
///
/// MUTATION: weight by temperature instead of reciprocal temperature; stop
/// the white iteration after one pass from D50.
#[test]
fn the_white_is_solved_from_the_neutral_and_the_weight_is_monotone_in_temperature() {
    let p = with_matrices();
    let t65 = white_temperature(&p, neutral_at(&p, 1, D65_XY64)).expect("D65 white");
    let ta = white_temperature(&p, neutral_at(&p, 0, [0.44757, 0.40745])).expect("A white");
    assert!((t65 - 6504.0).abs() < 20.0, "D65 neutral solved to {t65} K");
    assert!((ta - 2856.0).abs() < 20.0, "A neutral solved to {ta} K");
    assert!(illuminant_weight(&p, Some(t65)) > 0.99, "a D65 frame takes the D65 calibration");
    assert!(illuminant_weight(&p, Some(ta)) < 0.01, "an A frame takes the A calibration");
    let (na, nd) = (neutral_at(&p, 0, [0.44757, 0.40745]), neutral_at(&p, 1, D65_XY64));
    let mut last = (0.0f32, -1.0f32);
    for i in 0..=10 {
        let f = i as f32 / 10.0;
        let n = [0, 1, 2].map(|c| na[c] + (nd[c] - na[c]) * f);
        let t = white_temperature(&p, n).expect("white");
        let w = illuminant_weight(&p, Some(t));
        assert!(t > last.0 && w >= last.1, "step {i}: {t} K / weight {w} after {last:?}");
        last = (t, w);
    }
    assert_eq!(white_temperature(&p, [0.0, 1.0, 1.0]), None, "a zero neutral is unusable");
    assert_eq!(white_temperature(&bare(), nd), None, "no ColorMatrix, no white");
}

/// The body's own Adobe Standard profile, where Camera Raw is installed: a
/// D65 neutral solves to D65 and weighs (almost) nothing on illuminant A,
/// and its neutral maps to the working white through the whole stage.
#[test]
fn the_installed_profile_solves_a_d65_neutral_and_keeps_it_neutral() {
    let Ok((_, p)) = crate::dcp::find("Sony", "ILCE-7RM4A", "Adobe Standard") else {
        return; // no Camera Raw on this machine: the synthetic tests above carry the rule
    };
    assert!(p.forward_matrix.iter().all(Option::is_some), "the body's profile carries both");
    let n = neutral_at(&p, 1, D65_XY64);
    let t = white_temperature(&p, n).expect("white");
    assert!((t - 6504.0).abs() < 60.0, "D65 neutral solved to {t} K");
    assert!(1.0 - illuminant_weight(&p, Some(t)) < 0.02, "weight on A must be ≈ 0");
    let s = Stage::build_for(&p, SPACE, Frame { neutral: Some(n), ..Frame::default() }).expect("stage");
    assert!(s.owns_camera());
    for level in [0.05f32, 0.18, 0.5] {
        let out = s.develop_camera([level; 3]);
        let spread = out.iter().copied().fold(f32::MIN, f32::max) - out.iter().copied().fold(f32::MAX, f32::min);
        assert!(spread < 2e-3, "grey {level} came out {out:?}");
    }
}

/// A creative table goes on after the profile's own look table, which stays
/// (measured against Camera Raw: see `Stage::build_for`). An identity
/// creative table leaves the profile's table in force (a replacing creative
/// table would drop its halved saturation), and a creative hue shift still
/// shows on top. The ORDER of the two is Camera Raw's measured one and is not
/// pinned here: these flat tables commute.
///
/// MUTATION: let the creative table replace the profile's.
#[test]
fn a_creative_table_goes_on_after_the_profiles_look_table() {
    let p = Profile { look_table: Some(flat([0.0, 0.5, 1.0])), ..bare() };
    let only = Stage::build_for(&p, SPACE, Frame::default()).expect("stage");
    let identity = Frame { creative: Some(flat([0.0, 1.0, 1.0])), ..Frame::default() };
    let s = Stage::build_for(&p, SPACE, identity).expect("stage");
    let (mut px, mut want) = ([0.4f32, 0.2, 0.1], [0.4f32, 0.2, 0.1]);
    s.apply(&mut px);
    only.apply(&mut want);
    for c in 0..3 {
        assert!((px[c] - want[c]).abs() < 1e-4, "the profile's own table must stay under an identity creative one: {px:?} vs {want:?}");
    }
    let shifted = Frame { creative: Some(flat([20.0, 1.0, 1.0])), ..Frame::default() };
    let t = Stage::build_for(&p, SPACE, shifted).expect("stage");
    let mut moved = [0.4f32, 0.2, 0.1];
    t.apply(&mut moved);
    assert!((0..3).any(|c| (moved[c] - want[c]).abs() > 1e-3), "the creative hue shift must show on top: {moved:?}");
}

/// The DNG SDK's chromaticity of a temperature and tint (`Get_xy_coord`) is
/// the inverse of its temperature of a chromaticity (`Set_xy_coord`) along
/// the blackbody locus, and a tint moves the point ALONG the isotherm — the
/// temperature read back stays the one asked for, while the point moves.
///
/// MUTATION: interpolate the table rows the wrong way round, or move the tint
/// across the isotherm instead of along it.
#[test]
fn a_temperature_and_tint_name_a_white_on_its_isotherm() {
    for k in [2500.0, 3800.0, 5400.0, 6504.0, 9000.0, 20000.0] {
        let on = temp_tint_to_xy(k, 0.0);
        assert!((xy_to_kelvin(on) - k).abs() < k * 2e-4, "{k} K reads back {}", xy_to_kelvin(on));
        for t in [-30.0, 30.0] {
            let off = temp_tint_to_xy(k, t);
            assert!((xy_to_kelvin(off) - k).abs() < k * 2e-4, "{k} K tint {t} reads back {}", xy_to_kelvin(off));
            assert!((off[0] - on[0]).abs() + (off[1] - on[1]).abs() > 5e-3, "tint {t} moves the white");
        }
    }
    // A positive tint names a GREENER white (higher y at daylight) — so the
    // frame balanced for it renders magenta, Camera Raw's positive tint.
    assert!(temp_tint_to_xy(5400.0, 30.0)[1] > temp_tint_to_xy(5400.0, 0.0)[1]);
}

/// `Set_xy_coord` with its tint is `Get_xy_coord` the other way: a white
/// named by a temperature and a tint reads back as that temperature and tint
/// to within 0.03 mired and 0.002 tint units (the SDK's pair is not inverse
/// to the bit: one interpolates in reciprocal temperature, the other in the
/// isotherms' distances), so every point of Camera Raw's 50 K / whole-tint
/// grid reads back as itself after rounding.
///
/// MUTATION: interpolate the isotherm direction with the wrong weight, or
/// read the tint with the wrong sign or scale.
#[test]
fn a_white_named_by_temperature_and_tint_reads_back_as_them() {
    let (mut worst_mired, mut worst_tint) = (0.0f64, 0.0f64);
    for i in 0..=260 {
        let k = 2000.0 + 50.0 * f64::from(i);
        for t in -100..=100 {
            let t = f64::from(t);
            let (k2, t2) = xy_to_temp_tint(temp_tint_to_xy(k, t));
            worst_mired = worst_mired.max((1.0e6 / k2 - 1.0e6 / k).abs());
            worst_tint = worst_tint.max((t2 - t).abs());
            assert_eq!(((k2 / 50.0).round() * 50.0, t2.round()), (k, t), "{k} K / {t:+}");
        }
    }
    assert!(worst_mired < 0.03 && worst_tint < 0.002, "{worst_mired} mired, {worst_tint} tint");
    assert_eq!(xy_to_kelvin(temp_tint_to_xy(5400.0, 14.0)), xy_to_temp_tint(temp_tint_to_xy(5400.0, 14.0)).0);
}

