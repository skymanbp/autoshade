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
        tone_curve: Vec::new(),
        baseline_exposure_offset: 0.0,
        has_third_illuminant: false,
    }
}

/// A flat table that states one correction everywhere.
fn flat(delta: [f32; 3]) -> Table {
    Table { hue: 2, sat: 2, val: 1, srgb_value: false, data: vec![delta; 4] }
}

const SPACE: crate::render::ExportColorSpace = crate::render::ExportColorSpace::Srgb;

/// A profile with nothing to say builds NOTHING, so a photograph developed
/// through a matrix-only profile costs no arithmetic and moves no pixel.
///
/// This is the invariant the whole stage rests on: the F7 batch must not change
/// a single existing render, and it cannot if the stage refuses to exist
/// wherever there is no table, no curve and no exposure offset.
///
/// MUTATION: return `Some` unconditionally from `Stage::build`.
#[test]
fn a_profile_with_no_tables_and_no_curve_is_not_a_stage() {
    assert!(Stage::build(&bare(), SPACE, Some(5500.0)).is_none(), "nothing to apply");
    // An identity tone curve is nothing to apply either — a profile stating
    // `0,0 … 1,1` is stating that it does not touch the tone.
    let mut p = bare();
    p.tone_curve = vec![[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]];
    assert!(Stage::build(&p, SPACE, Some(5500.0)).is_none(), "an identity curve is not a curve");
    // …but a real one is.
    p.tone_curve = vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]];
    assert!(Stage::build(&p, SPACE, Some(5500.0)).is_some(), "a real curve builds a stage");
}

/// The reference round trip is EXACT when no table acts, so the two matrices
/// really are inverses and the stage cannot tint a picture by existing.
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
    // A baseline exposure offset of zero in stops: a stage that does nothing
    // but convert to the reference space and back.
    p.baseline_exposure_offset = 0.0;
    p.hue_sat_map[0] = Some(flat([0.0, 1.0, 1.0]));
    let s = Stage::build(&p, SPACE, Some(5500.0)).expect("a table makes a stage");
    for px in [[0.2f32, 0.5, 0.8], [1.0, 1.0, 1.0], [0.04, 0.04, 0.04], [0.9, 0.1, 0.3]] {
        let mut got = px;
        s.apply(&mut got);
        for ch in 0..3 {
            assert!(
                (got[ch] - px[ch]).abs() < 2e-4,
                "an identity table changed {px:?} into {got:?} (channel {ch})"
            );
        }
    }
}

/// The baseline exposure offset is STOPS, so −1 halves the light.
///
/// MUTATION: treat the offset as a linear multiplier rather than as an
/// exponent — which would make Adobe's own −0.35 a 65 % darkening instead of a
/// 22 % one.
#[test]
fn the_baseline_exposure_offset_is_measured_in_stops() {
    let mut p = bare();
    p.baseline_exposure_offset = -1.0;
    let s = Stage::build(&p, SPACE, None).expect("an offset alone makes a stage");
    let mut px = [0.4f32, 0.4, 0.4];
    s.apply(&mut px);
    for c in px {
        assert!((c - 0.2).abs() < 1e-3, "−1 stop must halve: {px:?}");
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

/// Adobe's tone curve keeps the colour's shape: the curve acts on the brightest
/// and darkest channels and the middle one is put back where it was between
/// them.
///
/// Per-channel is the obvious implementation and the wrong one, so the test
/// states the difference as a number rather than as a shape: the middle channel
/// under per-channel application lands somewhere else entirely.
///
/// MUTATION: apply the lookup to each channel independently.
#[test]
fn the_tone_curve_is_applied_adobes_way_and_not_per_channel() {
    // A curve that lifts everything below the midpoint hard.
    let lut: Vec<f32> = (0..TONE_LUT)
        .map(|i| {
            let x = i as f32 / (TONE_LUT - 1) as f32;
            x.powf(0.5)
        })
        .collect();
    let rgb = [0.64f32, 0.25, 0.04];
    let got = adobe_tone(rgb, &lut);
    // hi 0.64 → 0.8, lo 0.04 → 0.2, and the middle keeps its RELATIVE place:
    // (0.25 − 0.04)/(0.64 − 0.04) = 0.35 → 0.2 + 0.35·(0.8 − 0.2) = 0.41.
    assert!((got[0] - 0.8).abs() < 2e-3, "brightest: {got:?}");
    assert!((got[2] - 0.2).abs() < 2e-3, "darkest: {got:?}");
    assert!((got[1] - 0.41).abs() < 3e-3, "middle, placed proportionally: {got:?}");
    // Per-channel would give sqrt(0.25) = 0.5, which is a visibly different
    // colour — the desaturation this scheme exists to avoid.
    assert!((got[1] - 0.5).abs() > 0.05, "this is the per-channel answer, not Adobe's");
    // A flat colour stays flat: no channel order to preserve, no division by a
    // zero span.
    let grey = adobe_tone([0.25, 0.25, 0.25], &lut);
    assert!(grey.iter().all(|c| (c - 0.5).abs() < 2e-3), "a grey stays grey: {grey:?}");
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

/// The tone lookup refuses what it cannot use instead of building a broken one
/// — and refuses an IDENTITY, which it can use perfectly well and should not.
///
/// The identity arm is not tidiness. `Stage::build` decides whether this
/// profile is a stage at all by asking whether the tone LUT is `Some`, so a
/// profile whose only content is a straight `0,0 → 1,1` curve would become a
/// per-pixel pass that converts to the reference space, samples a LUT and
/// converts back — arithmetic that cannot help and can only drift. Adobe ships
/// exactly such curves.
///
/// MUTATION: accept a single knot, keep duplicate x values (which divide by
/// a zero span in the interpolation), or return the identity LUT.
#[test]
fn the_tone_lookup_refuses_a_curve_it_cannot_read() {
    assert!(tone_lut(&[]).is_none(), "no knots");
    assert!(tone_lut(&[[0.0, 0.0]]).is_none(), "one knot is not a curve");
    // Two knots at the same x are one knot.
    assert!(tone_lut(&[[0.5, 0.1], [0.5, 0.9]]).is_none(), "a zero-width span");
    // Knots outside [0, 1] are dropped, and what is left has to still be a
    // curve — otherwise the lookup would extrapolate off a single point.
    assert!(tone_lut(&[[-1.0, 0.0], [2.0, 1.0], [0.5, 0.7]]).is_none(), "one usable knot");
    let lut = tone_lut(&[[0.0, 0.0], [0.25, 0.5], [1.0, 1.0]]).expect("a real curve");
    assert_eq!(lut.len(), TONE_LUT);
    assert!((lut[0]).abs() < 1e-6 && (lut[TONE_LUT - 1] - 1.0).abs() < 1e-6, "the ends are pinned");
    assert!(lut[TONE_LUT / 4] > 0.4, "the knot at 0.25 lifts to ~0.5: {}", lut[TONE_LUT / 4]);
    // An identity is readable and still refused, in both the two-knot spelling
    // and a longer one whose extra knots happen to sit on the diagonal.
    assert!(tone_lut(&[[0.0, 0.0], [1.0, 1.0]]).is_none(), "a straight line is not a curve");
    assert!(
        tone_lut(&[[0.0, 0.0], [0.25, 0.25], [0.5, 0.5], [1.0, 1.0]]).is_none(),
        "…nor is a straight line with more points on it"
    );
    // …and that refusal is what keeps such a profile from becoming a stage.
    let mut p = bare();
    p.baseline_exposure_offset = 0.0;
    p.tone_curve = vec![[0.0, 0.0], [1.0, 1.0]];
    assert!(
        Stage::build(&p, SPACE, Some(5500.0)).is_none(),
        "an identity curve is the whole profile here, so there is nothing to apply"
    );
}
