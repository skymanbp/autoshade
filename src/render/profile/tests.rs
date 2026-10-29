use super::*;
use crate::dcp::{Profile, Table};

/// A profile carrying exactly what a test asks for and nothing else.
///
/// Its tone curve is a stated IDENTITY rather than absent: a profile with no
/// curve renders through Adobe's default one, and these tests isolate the
/// tables, the gain and the reference space from it. The tests about the
/// default curve take the curve away explicitly.
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

/// What the stage's exposure transfer alone does to `px` (v1.17.0): Camera
/// Raw's transfer at the stage's camera start, by its channel rule in the
/// reference space. Every stage carries it, so a test about a table, a
/// matrix or a curve states its expectation through it.
fn ramped(s: &Stage, px: [f32; 3]) -> [f32; 3] {
    let r = s.ramp.transfer_rgb(super::super::mat_vec3(&s.to_ref, &px).map(|c| c.max(0.0)));
    super::super::mat_vec3(&s.from_ref, &r)
}

/// The grey the stage's transfer takes to `y`.
fn unramp(s: &Stage, y: f32) -> f32 {
    s.ramp.input_of(y)
}

/// A profile with nothing to say — no table, a STATED straight curve, a total
/// exposure of 0 EV — still renders Camera Raw's exposure transfer, which is
/// not the identity even at 0 EV (its toe: measured on exactly such a
/// profile, `tone_cr`), and nothing else. A body baseline that the profile's
/// offset cancels starts at 0 EV.
///
/// MUTATION: skip the stage when no table or curve acts (the transfer would
/// go missing), or ignore the offset in the start.
#[test]
fn a_profile_with_nothing_to_say_renders_the_transfer_alone() {
    let s = Stage::build(&bare(), SPACE, Some(5500.0), 0.0).expect("the transfer is always there");
    let (lo, mut px) = ([0.002f32; 3], [0.002f32; 3]);
    s.apply(&mut px);
    assert!(px[0] < lo[0], "the toe at 0 EV: {px:?}");
    let want = ramped(&s, lo);
    assert!((px[0] - want[0]).abs() < 1e-6, "{px:?} vs {want:?}");
    let mut cancelled = bare();
    cancelled.baseline_exposure_offset = -0.35;
    let s = Stage::build(&cancelled, SPACE, Some(5500.0), 0.35).expect("a stage");
    assert_eq!(s.start(), crate::recipe::CameraStart { ev: 0.0, headroom: 0.0 }, "0 EV in total");
}

/// The reference round trip is EXACT when no table acts, so the two matrices
/// really are inverses and the stage cannot tint a picture by existing: what
/// comes back is the colour it was given, through the exposure ramp alone.
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
    let s = Stage::build(&p, SPACE, Some(5500.0), 0.0).expect("a table makes a stage");
    for px in [[0.2f32, 0.5, 0.8], [1.0, 1.0, 1.0], [0.04, 0.04, 0.04], [0.9, 0.1, 0.3]] {
        let mut got = px;
        s.apply(&mut got);
        let want = ramped(&s, px);
        for ch in 0..3 {
            assert!(
                (got[ch] - want[ch]).abs() < 2e-4,
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
    let s = Stage::build(&p, SPACE, None, 0.0).expect("an offset alone makes a stage");
    assert_eq!(s.start().ev, -1.0, "−1 stop");
    let mut px = [0.4f32, 0.4, 0.4];
    s.apply(&mut px);
    let want = s.ramp.transfer(0.4);
    for c in px {
        assert!((c - want).abs() < 1e-3, "the transfer at −1 EV: {px:?} vs {want}");
    }
    // −1 EV darkens a midtone close to the plain halving (the measured row).
    assert!((want / 0.2 - 1.0).abs() < 0.15, "{want}");
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
    let s = Stage::build(&p, SPACE, None, 0.0).expect("a look table makes a stage");
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
    let s = Stage::build(&p, SPACE, None, 0.0).expect("stage");
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
    // …and that refusal leaves such a profile's stage with no curve: what it
    // renders is Camera Raw's exposure transfer alone.
    let mut p = bare();
    p.baseline_exposure_offset = 0.0;
    p.tone_curve = vec![[0.0, 0.0], [1.0, 1.0]];
    let s = Stage::build(&p, SPACE, Some(5500.0), 0.0).expect("the transfer");
    assert!(s.tone.is_none(), "an identity curve is no curve to apply");
}

/// The tone lookup's two ends: above 1 the curve is continued along the chord
/// through the origin, `lut[n−1]·x` — the identity for a curve ending at
/// (1, 1), so a highlight outside the working gamut passes through, which is
/// what the wide-gamut path carries — and at or below 0 it answers the
/// curve's own start.
///
/// Below 0 the shipped `lut[0]·max(x, 0)` could only ever be ±0 — a step at
/// exactly 0 for any curve that does not start there. (Whether the top
/// should instead continue at the shoulder's tangent, which compresses every
/// such highlight, is a decision for the profile fixtures; this test pins
/// what ships.)
///
/// MUTATION: `lut[0] * x.max(0.0)` back in the bottom arm (the lifted curve
/// reads 0 at 0), or the shoulder's tangent in the top arm (1.5 reads 1.25).
#[test]
fn the_tone_lookup_passes_white_through_and_starts_where_the_curve_does() {
    let n = TONE_LUT;
    let sqrt: Vec<f32> = (0..n).map(|i| (i as f32 / (n - 1) as f32).sqrt()).collect();
    // Inside the range it is the curve.
    assert!((sample(&sqrt, 0.25) - 0.5).abs() < 2e-3, "√0.25: {}", sample(&sqrt, 0.25));
    // Above 1: the chord through the origin — the identity for a curve that
    // ends at (1, 1) — so a highlight past white is carried, not clipped and
    // not compressed by the shoulder's slope.
    let got = sample(&sqrt, 1.5);
    assert!((got - 1.5).abs() < 2e-3, "1.5 passes through on a curve ending at 1: {got}");
    let half: Vec<f32> = sqrt.iter().map(|v| v * 0.5).collect();
    assert!((sample(&half, 2.0) - 1.0).abs() < 2e-3, "the chord's slope is the curve's end value");
    // A curve that starts above zero answers its start at 0 and below it.
    let lifted: Vec<f32> = (0..n).map(|i| 0.1 + 0.9 * i as f32 / (n - 1) as f32).collect();
    assert_eq!(sample(&lifted, 0.0), 0.1, "the curve at 0 is its first sample");
    assert_eq!(sample(&lifted, -0.5), 0.1, "…and below 0 the same, not a sign times a clamp");
}

/// A profile that states no tone curve renders through Adobe's default one —
/// the DNG SDK's `dng_tone_curve_acr3_default`, which `dng_render` starts from
/// and replaces only with a profile's own valid curve.
///
/// The two numbers are the SDK table's own: kTable at 0.5 is 0.80486, and at
/// 0.18 (between samples 184 and 185) it is 0.3881.
///
/// MUTATION: render a curve-less profile through the identity, or resample the
/// table onto a different grid without interpolating.
#[test]
fn a_profile_without_a_curve_renders_adobes_default_curve() {
    assert_eq!(acr3_default::ACR3_DEFAULT.len(), 1025, "the SDK's 1025 samples");
    assert_eq!(acr3_default::ACR3_DEFAULT[0], 0.0);
    assert_eq!(acr3_default::ACR3_DEFAULT[1024], 1.0);
    assert!(acr3_default::ACR3_DEFAULT.windows(2).all(|w| w[1] >= w[0]), "monotone");
    let mut p = bare();
    p.tone_curve = Vec::new();
    let s = Stage::build(&p, SPACE, None, 0.0).expect("the default curve is something to apply");
    for (x, want) in [(0.5f32, 0.80486f32), (0.18, 0.3881)] {
        assert!((default_curve(x) - want).abs() < 1e-3, "the table at {x}: {}", default_curve(x));
        // The stage meets the curve after the exposure transfer.
        let mut grey = [unramp(&s, x); 3];
        s.apply(&mut grey);
        for c in grey {
            assert!((c - want).abs() < 1e-3, "grey {x} through the default curve: {grey:?}");
        }
    }
    // A curve the profile states but this reader cannot use is, to the SDK, not
    // a valid curve either — the default renders.
    p.tone_curve = vec![[0.5, 0.7]];
    let s = Stage::build(&p, SPACE, None, 0.0).expect("an unusable curve is no curve");
    let mut grey = [unramp(&s, 0.5); 3];
    s.apply(&mut grey);
    assert!((grey[0] - 0.80486).abs() < 1e-3, "{grey:?}");
}

/// A profile that states its OWN curve renders through that curve, not the
/// default one, exactly as before.
///
/// MUTATION: compose the default curve under the profile's own.
#[test]
fn a_profile_with_its_own_curve_keeps_it() {
    let mut p = bare();
    p.tone_curve = vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]];
    let s = Stage::build(&p, SPACE, None, 0.0).expect("a real curve");
    let mut grey = [unramp(&s, 0.5); 3];
    s.apply(&mut grey);
    assert!(grey.iter().all(|c| (c - 0.62).abs() < 2e-3), "its own curve at its knot: {grey:?}");
}

/// The stage's start is the body's baseline exposure PLUS the profile's
/// offset, in stops — Adobe's `TotalBaselineExposure` — and its transfer is
/// the measured one at that total.
///
/// MUTATION: ignore the baseline argument, or subtract it.
#[test]
fn the_start_composes_the_bodys_baseline_with_the_profiles_offset() {
    let mut p = bare();
    p.baseline_exposure_offset = -1.0;
    let s = Stage::build(&p, SPACE, None, 0.5).expect("−0.5 EV in total");
    assert_eq!(s.start().ev, -0.5, "−1 + 0.5");
    let mut px = [0.4f32; 3];
    s.apply(&mut px);
    let want = super::super::tone_cr::Ramp::of(crate::recipe::CameraStart { ev: -0.5, headroom: 0.0 }).transfer(0.4);
    assert!(px.iter().all(|c| (c - want).abs() < 1e-3), "−0.5 EV: {px:?} vs {want}");
    p.baseline_exposure_offset = 0.0;
    let s = Stage::build(&p, SPACE, None, 1.0).expect("+1 EV");
    assert_eq!(s.start().ev, 1.0);
}

/// Light the exposure lifts towards white is rolled off, not clipped, before
/// the look table and the curve — Camera Raw's transfer (v1.17.0, measured;
/// the DNG SDK's `dng_function_exposure_ramp` clips at 1 instead); nothing
/// past 1 reaches the curve.
///
/// MUTATION: clip at white again (0.8 at +1 EV would land on 1).
#[test]
fn light_lifted_towards_white_rolls_off_under_camera_raws_transfer() {
    let mut p = bare();
    p.baseline_exposure_offset = 0.0;
    let s = Stage::build(&p, SPACE, None, 1.0).expect("+1 EV");
    let mut px = [0.8f32; 3];
    s.apply(&mut px);
    let open = s.ramp.transfer(0.4);
    assert!(px.iter().all(|&c| c > open && c < 0.999), "0.8 at +1 EV rolls off below white: {px:?}");
    let mut past = [1.5f32; 3];
    s.apply(&mut past);
    assert!(past.iter().all(|&c| c <= 1.0 + 1e-6), "nothing past white: {past:?}");
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
    let s = Stage::build_for(&p, SPACE, frame, 0.0).expect("matrices make a stage");
    assert!(s.owns_camera());
    for level in [0.02f32, 0.18, 0.6] {
        let out = s.develop_camera([level; 3]);
        let want = s.ramp.transfer(level);
        for c in out {
            assert!((c - want).abs() < 2e-4, "grey {level} came out {out:?}, not {want}");
        }
    }
    // Without a neutral the white cannot be solved, and the stage leaves the
    // matrix to the engine's own path — it does not invent a white.
    let s = Stage::build_for(&p, SPACE, Frame::default(), 0.0);
    assert!(s.is_none_or(|s| !s.owns_camera()));
    // …and a profile with ColorMatrix but no ForwardMatrix keeps that path too.
    let no_fm = Profile { forward_matrix: [None, None], ..with_matrices() };
    let frame = Frame { neutral: Some(neutral_at(&no_fm, 1, D65_XY64)), ..Frame::default() };
    assert!(Stage::build_for(&no_fm, SPACE, frame, 0.0).is_none_or(|s| !s.owns_camera()));
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
    let s = Stage::build_for(&p, SPACE, Frame { neutral: Some(n), ..Frame::default() }, 0.0)
        .expect("stage");
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
    let only = Stage::build_for(&p, SPACE, Frame::default(), 0.0).expect("stage");
    let identity = Frame { creative: Some(flat([0.0, 1.0, 1.0])), ..Frame::default() };
    let s = Stage::build_for(&p, SPACE, identity, 0.0).expect("stage");
    let (mut px, mut want) = ([0.4f32, 0.2, 0.1], [0.4f32, 0.2, 0.1]);
    s.apply(&mut px);
    only.apply(&mut want);
    for c in 0..3 {
        assert!((px[c] - want[c]).abs() < 1e-4, "the profile's own table must stay under an identity creative one: {px:?} vs {want:?}");
    }
    let shifted = Frame { creative: Some(flat([20.0, 1.0, 1.0])), ..Frame::default() };
    let t = Stage::build_for(&p, SPACE, shifted, 0.0).expect("stage");
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

/// The white shift between one white and itself is the identity, and between
/// two whites it keeps the working white's luminance.
///
/// MUTATION: drop the luminance scale, or conjugate the adaptation with the
/// working primaries instead of the reference ones.
#[test]
fn the_white_shift_keeps_white_luminance_and_is_identity_on_one_white() {
    let w = temp_tint_to_xy(5400.0, 14.0);
    let id = white_shift(w, w);
    for (i, row) in id.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            assert!((v - if i == j { 1.0 } else { 0.0 }).abs() < 1e-5, "{id:?}");
        }
    }
    let m = white_shift(temp_tint_to_xy(4000.0, 14.0), w);
    let xyz = rgb_to_xyz64(PROPHOTO_PRIM, D65_XY64);
    let white = vec64(&xyz, vec64(&mat3_f64(&m), [1.0; 3]));
    assert!((white[1] - 1.0).abs() < 1e-5, "luminance {}", white[1]);
    // White balanced for a warmer light than the as-shot one: the frame cools.
    let o = super::super::mat_vec3(&m, &[0.5; 3]);
    assert!(o[2] > o[0] + 0.02, "{o:?}");
}

/// Under the stage's tone the undo and the redo are inverses for a colour the
/// stage could have produced, so a shift that does nothing changes nothing.
///
/// MUTATION: undo the curve per channel (it does not invert `adobe_tone` on
/// a colour), or leave the transfer out of one side.
#[test]
fn under_the_stage_tone_undo_and_redo_are_inverses() {
    let under = UnderTone::new(super::super::ExportColorSpace::Srgb, crate::recipe::CameraStart { ev: 0.35, headroom: 1.0 });
    for px in [[0.05f32, 0.05, 0.05], [0.4, 0.2, 0.1], [0.1, 0.3, 0.6], [0.8, 0.75, 0.7], [0.02, 0.01, 0.03]] {
        let back = under.redo(under.undo(px));
        for c in 0..3 {
            assert!((back[c] - px[c]).abs() < 2e-3, "{px:?} -> {back:?}");
        }
    }
}

/// A chosen white goes to a camera neutral the way the DNG SDK's `SetWhiteXY`
/// does — the calibration diagonal in front of the `ColorMatrix` at the
/// white's own temperature — and that neutral reads back as the same white
/// through the same calibrated matrices (v1.17.0, the Custom white balance
/// Camera Raw puts into the camera stage).
///
/// MUTATION: interpolate the matrix at D50 instead of the white's own
/// temperature; leave the calibration out of either direction; normalise
/// the neutral by green instead of its largest component.
#[test]
fn a_chosen_white_and_its_camera_neutral_read_each_other_back() {
    let p = with_matrices();
    let cc = Some([1.0031f32, 1.0, 0.9653]);
    for (k, tint) in [(2500.0, 0.0), (3200.0, 20.0), (5400.0, 14.0), (9000.0, -30.0), (30000.0, 0.0)] {
        let xy = temp_tint_to_xy(k, tint);
        let n = camera_neutral_of(&p, xy, cc).expect("a neutral");
        assert!((n.iter().cloned().fold(0.0f32, f32::max) - 1.0).abs() < 1e-6, "{n:?}");
        let back = white_temperature_with(&p, n, cc).expect("a white");
        assert!((1e6 / f64::from(back) - 1e6 / k).abs() < 0.5, "{k} K read back as {back} K");
        let plain = camera_neutral_of(&p, xy, None).expect("a neutral");
        let ratio = [0, 1, 2].map(|c| n[c] / plain[c]);
        assert!((ratio[0] / ratio[1] - 1.0031).abs() < 1e-4 && (ratio[2] / ratio[1] - 0.9653).abs() < 1e-4, "{ratio:?}");
    }
    assert_eq!(camera_neutral_of(&bare(), D65_XY64, None), None, "no ColorMatrix, no neutral");
}
