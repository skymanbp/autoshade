//! F6's invariants. The solver is checked ALGEBRAICALLY rather than by eye: a
//! chart is built by applying a known keystone to a rectilinear scene, and the
//! solver's answer composed with that keystone must leave no perspective term.
//! That is the whole claim — "the verticals are parallel again" — stated as
//! arithmetic instead of as a picture.

use super::*;
use crate::recipe::{Crop, EditRecipe};

/// One of the 13 matrices this operator's library actually selected, VERBATIM:
/// the `crs:UprightTransform_1` of a photo whose `crs:PerspectiveUpright` is 1.
/// A 0.9786° turn with a keystone, and the scale Adobe folded in to cover the
/// frame.
// The digits ARE the datum, because the point of this constant is that it is
// what Lightroom wrote — and `f32` rounding is exactly what the reader does to
// the same text at run time (`crs_f32`), so truncating the literal would make
// the test agree with a number no sidecar contains.
#[allow(clippy::excessive_precision)]
const ADOBE_MATRIX: [f32; 9] = [
    1.016961743,
    0.007720874,
    -0.012341308,
    -0.017371966,
    1.016961743,
    0.000205112,
    0.0,
    0.0,
    1.0,
];

fn with_matrices(mode: f32, mats: Vec<[f32; 9]>) -> EditRecipe {
    EditRecipe { perspective_upright: mode, upright_transform: mats, ..Default::default() }
}

/// 3:2 — the shape every case in the Lightroom kit was exported at, so a
/// coefficient measured there and a coefficient asserted here mean the same
/// thing. Two of the seven sliders read the frame's shape (see [`manual`]).
const KIT: f32 = 1.5;

/// What [`transform`] computes for the square frames these probes build, so a
/// test that calls [`manual`] directly and one that goes through the stage are
/// asking for the same map.
const SQUARE: f32 = 1.0;

/// A rectilinear scene of bars, seen through `truth`.
///
/// Built by inverse-mapping each destination pixel — the same way [`apply`]
/// works — so the chart IS what the engine would have produced, and a sinusoid
/// rather than hard bars so every pixel carries a gradient and the solver has
/// the samples it asks for.
fn chart(w: usize, h: usize, truth: Homography, vertical_bars: bool) -> Vec<f32> {
    let inv = truth.inverse().expect("the probe keystone is invertible");
    let (dw, dh) = (w as f32 - 1.0, h as f32 - 1.0);
    const PERIOD: f32 = 0.0625; // 16 px on a 256 px frame
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| match inv.map(x as f32 / dw, y as f32 / dh) {
            Some((u, v)) => {
                let t = if vertical_bars { u } else { v };
                0.5 + 0.4 * (t / PERIOD * std::f32::consts::TAU).sin()
            }
            None => 0.5,
        })
        .collect()
}

/// A keystone about the frame centre: the divisor varies along y.
fn keystone(kv: f32) -> Homography {
    Homography::about_centre([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, kv, 1.0])
}

/// How much PERSPECTIVE a map has left, scale-free: the last row's length over
/// its own homogeneous scale.
fn perspective_strength(h: Homography) -> f32 {
    let m = h.0;
    let s = if m[8].abs() > 1e-9 { m[8].abs() } else { 1.0 };
    (m[6] * m[6] + m[7] * m[7]).sqrt() / s
}

/// A flat 16-bit frame, for the paths that need an image rather than a plane.
fn flat(side: u32, v: u16) -> image::DynamicImage {
    image::DynamicImage::ImageRgb16(image::ImageBuffer::from_pixel(side, side, image::Rgb([v, v, v])))
}

#[test]
fn adobes_own_matrix_is_read_verbatim_and_pivots_on_the_frame_centre() {
    let r = with_matrices(1.0, vec![Homography::IDENTITY.0, ADOBE_MATRIX]);
    let h = upright_from_sidecar(&r).expect("mode 1 selects index 1");
    assert_eq!(h.0, ADOBE_MATRIX, "Adobe's numbers are rendered, not re-derived");
    // The measurement the whole coordinate system rests on: the frame centre in
    // [0,1] coordinates is a fixed point.
    let (u, v) = h.map(0.5, 0.5).expect("the centre is not on the horizon");
    assert!(
        (u - 0.5).abs() < 1e-6 && (v - 0.5).abs() < 1e-6,
        "the pivot is the frame centre in [0,1] coordinates: ({u}, {v})"
    );
    // And the other measurement: Adobe pre-scaled it to cover the frame, so
    // this engine adds no fill of its own.
    assert!(h.covers(), "Adobe's matrix already covers the frame");
}

#[test]
fn the_mode_is_an_index_and_zero_means_off() {
    let six = vec![
        Homography::IDENTITY.0,
        ADOBE_MATRIX,
        ADOBE_MATRIX,
        [1.002647, 0.001180, -0.001913, -0.002654, 1.002647, 0.000003, 0.0, 0.0, 1.0],
        ADOBE_MATRIX,
        Homography::IDENTITY.0,
    ];
    assert_eq!(upright_from_sidecar(&with_matrices(0.0, six.clone())), None, "mode 0 is off");
    assert_eq!(
        upright_from_sidecar(&with_matrices(3.0, six.clone())).map(|h| h.0),
        Some(six[3]),
        "mode 3 selects index 3, not the first non-identity one"
    );
    // `_5` is the identity Lightroom writes for a Guided correction with no
    // guides drawn, and an identity is not an answer — the caller must fall
    // through to the solver.
    assert_eq!(
        upright_from_sidecar(&with_matrices(5.0, six.clone())),
        None,
        "identity is not an answer"
    );
    // A mode the document has no matrix for falls through the same way.
    assert_eq!(upright_from_sidecar(&with_matrices(4.0, vec![six[0], six[1]])), None);
}

#[test]
fn every_manual_slider_moves_the_frame_and_neutral_moves_nothing() {
    assert_eq!(manual(&EditRecipe::default(), KIT), None, "at rest the stage costs no resample");
    // Named setters rather than seven struct literals: what is under test is
    // that EACH of the seven reaches the map, one field at a time.
    type Set = fn(&mut EditRecipe);
    let probes: [(&str, Set); 7] = [
        ("vertical", |r| r.perspective_vertical = 40.0),
        ("horizontal", |r| r.perspective_horizontal = -40.0),
        ("rotate", |r| r.perspective_rotate = 3.0),
        ("scale", |r| r.perspective_scale = 120.0),
        ("aspect", |r| r.perspective_aspect = 50.0),
        ("x", |r| r.perspective_x = 30.0),
        ("y", |r| r.perspective_y = -30.0),
    ];
    for (name, set) in probes {
        let mut r = EditRecipe::default();
        set(&mut r);
        let h = manual(&r, KIT).unwrap_or_else(|| panic!("{name} moved and the stage said nothing"));
        assert_ne!(h, Homography::IDENTITY, "{name} produced the identity");
        let (u, v) =
            h.map(0.5, 0.5).unwrap_or_else(|| panic!("{name} put the centre on its horizon"));
        // Five of the seven pivot on the frame centre, which is what makes them
        // compose with Adobe's centre-pivoted matrix by plain multiplication.
        // The two OFFSETS are the exception BY DEFINITION — moving the frame is
        // their whole job — so they are checked against the distance they are
        // supposed to move it instead of against standing still.
        // The two offsets also differ in HANDEDNESS, which is measured: the same
        // slider value moves x positive and y negative (see `OFFSET`), so the
        // probe's `y = −30` slides the frame the way `x = +30` does.
        match name {
            "x" => assert!(
                (u - (0.5 + 0.3 * OFFSET)).abs() < 1e-5,
                "x slid to {u}, not {}",
                0.5 + 0.3 * OFFSET
            ),
            "y" => assert!(
                (v - (0.5 + 0.3 * OFFSET)).abs() < 1e-5,
                "y −30 must slide POSITIVE: {v}, not {}",
                0.5 + 0.3 * OFFSET
            ),
            _ => assert!(
                (u - 0.5).abs() < 1e-5 && (v - 0.5).abs() < 1e-5,
                "{name} must pivot on the frame centre: ({u}, {v})"
            ),
        }
        assert!(h.inverse().is_some(), "{name} must be invertible at its working range");
    }
    // ASPECT IS NOT SCALE, and the assertions above cannot tell them apart: a
    // plain gain on both axes is not the identity, fixes the centre and is
    // invertible, so it satisfies every one of them. What separates the two
    // sliders is AREA — Aspect trades one axis against the other (`diag(1/g, g)`,
    // determinant 1) while Scale changes both (`diag(s, s)`). Measured on the
    // frame the map produces rather than on the matrix, so the claim is about
    // the picture.
    let area = |r: &EditRecipe| {
        let h = manual(r, KIT).expect("moves");
        let (x0, y0) = h.map(0.0, 0.0).expect("corner");
        let (x1, y1) = h.map(1.0, 1.0).expect("corner");
        ((x1 - x0) * (y1 - y0)).abs()
    };
    let rest = area(&EditRecipe { perspective_scale: 100.000_01, ..Default::default() });
    for aspect in [50.0f32, -50.0, 90.0] {
        let got = area(&EditRecipe { perspective_aspect: aspect, ..Default::default() });
        assert!(
            (got / rest - 1.0).abs() < 1e-3,
            "Aspect {aspect} changed the frame's AREA by {:.4}× — that is Scale's job, not \
             Aspect's",
            got / rest
        );
    }
    let zoomed = area(&EditRecipe { perspective_scale: 150.0, ..Default::default() });
    assert!(
        (zoomed / rest - 2.25).abs() < 1e-2,
        "…while Scale 150 really is 1.5² of the area: {:.4}×",
        zoomed / rest
    );
}

/// F6, 2026-09-19: each slider carries the coefficient the Lightroom kit
/// MEASURED, and the expectations below are Lightroom's own fitted numbers
/// rather than this module's constants rearranged.
///
/// That distinction is the whole value of the test. Asserting
/// `manual` against `KEYSTONE` would pass for any value of `KEYSTONE`, which is
/// how five of these seven sliders shipped wrong: every existing test checked
/// direction, pivot, invertibility and area, and all of them passed while the
/// keystone pointed the wrong way and Aspect was 4.25× too strong. So the
/// literals here are the fits — 15 `PERSP-*` cases, residual under 0.05 px on
/// 85–96 of 96 blocks — and the tolerances are the spread those fits showed.
///
/// MUTATION: any of the five calibrated constants moved by more than its stated
/// tolerance; the keystone's sign; the Y offset's sign; dropping `aspect` from
/// the rotation or from the keystone.
#[test]
fn the_manual_sliders_carry_the_coefficients_the_kit_measured() {
    /// `manual`'s map, back in the centred box the kit reported its fits in.
    fn centred(r: &EditRecipe, aspect: f32) -> [f32; 9] {
        // `manual` returns `about_centre(M)` = B·M·T, with T translating by −½
        // and B by +½, so M = T·X·B — the conjugation the OTHER way round.
        // Getting it backwards conjugates by ±1 instead and quietly rescales the
        // answer: a keystone of −0.325 reads −0.245, because h22 comes back 1.325
        // and the renormalisation divides it out.
        const HALF: f32 = 0.5;
        let t = Homography([1.0, 0.0, -HALF, 0.0, 1.0, -HALF, 0.0, 0.0, 1.0]);
        let b = Homography([1.0, 0.0, HALF, 0.0, 1.0, HALF, 0.0, 0.0, 1.0]);
        let m = b.then(manual(r, aspect).expect("moves")).then(t);
        // Renormalised on h22, which is the convention the fits are quoted in.
        m.0.map(|v| v / m.0[8])
    }
    let v = |s: f32| EditRecipe { perspective_vertical: s, ..Default::default() };
    let h = |s: f32| EditRecipe { perspective_horizontal: s, ..Default::default() };

    // --- keystone: coefficient and sign -------------------------------------
    // `V+50` fitted −0.32521, `V+100` −0.65050 (exactly twice it), `H+50`
    // −0.48814 — 1.5009× the vertical coefficient, which is this frame's aspect.
    // `PERSP14-V+50`, the same slider on a different photograph and a different
    // lens, fitted −0.33231, so the coefficient is a property of the slider.
    for (name, r, k, ki) in [
        ("V+50", v(50.0), -0.32521, 7),
        ("V-50", v(-50.0), 0.325_37, 7),
        ("V+100", v(100.0), -0.650_50, 7),
        ("H+50", h(50.0), -0.488_14, 6),
        ("H-50", h(-50.0), 0.488_29, 6),
    ] {
        let m = centred(&r, KIT);
        assert!(
            (m[ki] - k).abs() < 0.006,
            "{name}: keystone {} against Lightroom's {k}",
            m[ki]
        );
        // NO same-axis stretch, which is a decision and not an oversight — see
        // `KEYSTONE`. Lightroom applies 1.27788 here on a 51 mm frame and 0.9487
        // on a 15.5 mm one at the same slider, so it follows the lens and six
        // readings over two photographs do not identify it. Asserted, because a
        // stretch quietly reappearing is exactly the regression this deviation
        // invites, and because the quadratic that was nearly shipped lived here.
        for gi in [0usize, 4] {
            assert!(
                (m[gi] - 1.0).abs() < 1e-5,
                "{name}: the keystone must not scale either axis: {}",
                m[gi]
            );
        }
    }
    // The cross term stays out: Lightroom's vertical keystone reads +0.0006 on
    // the horizontal slot and its horizontal reads +0.0002 on the vertical, so
    // neither slider leaks into the other's axis.
    assert!(centred(&v(100.0), KIT)[6].abs() < 1e-6, "a vertical keystone is vertical only");
    assert!(centred(&h(100.0), KIT)[7].abs() < 1e-6, "…and a horizontal one horizontal only");

    // The short-edge unit, which is the one thing the kit could NOT settle and
    // therefore the one thing worth pinning deliberately: turn the frame
    // portrait and the two coefficients trade places, because the short edge
    // trades places with the long one. Every kit frame is 3:2, so this asserts
    // the CHOICE documented on `KEYSTONE` rather than a measurement.
    let portrait = 1.0 / KIT;
    assert!(
        (centred(&v(50.0), portrait)[7] - centred(&h(50.0), KIT)[6]).abs() < 1e-5,
        "portrait's vertical keystone must equal landscape's horizontal one"
    );

    // --- rotate: a RIGID rotation, in pixels --------------------------------
    // `ROT+5` fitted +4.9884° of true rotation and `ROT-10` −10.0143°, so the
    // slider is degrees. Asserted in PIXEL space, because that is where the
    // claim lives: a per-axis box turns a rotation into a shear, and the frame's
    // own shape must not change what "5 degrees" means.
    for deg in [5.0f32, -10.0, 0.25] {
        let m = centred(&EditRecipe { perspective_rotate: deg, ..Default::default() }, KIT);
        let (sin, cos) = deg.to_radians().sin_cos();
        // pixel-space linear part = [[m00, m01·aspect], [m10/aspect, m11]]
        let px = [m[0], m[1] * KIT, m[3] / KIT, m[4]];
        for (got, want) in px.iter().zip([cos, -sin, sin, cos]) {
            assert!(
                (got - want).abs() < 1e-5,
                "Rotate {deg}° must be rigid in pixels: {px:?} against \
                 {:?}",
                [cos, -sin, sin, cos]
            );
        }
    }

    // --- aspect: reciprocal, area-preserving, ln(1.1) per full slider -------
    // `ASP+50` fitted 0.95346 × 1.04881 and `ASP-50` its mirror, area 1.0004.
    for (s, sx, sy) in [(50.0f32, 0.953_46, 1.048_81), (-50.0, 1.048_81, 0.953_46)] {
        let m = centred(&EditRecipe { perspective_aspect: s, ..Default::default() }, KIT);
        assert!(
            (m[0] - sx).abs() < 0.0015 && (m[4] - sy).abs() < 0.0015,
            "Aspect {s}: {} × {} against Lightroom's {sx} × {sy}",
            m[0],
            m[4]
        );
    }

    // --- offsets: 0.8121 of the frame per full slider, and opposite signs ---
    // `X+20` fitted +0.1621 of the width, `Y+20` −0.1625 of the height.
    let mx = centred(&EditRecipe { perspective_x: 20.0, ..Default::default() }, KIT);
    let my = centred(&EditRecipe { perspective_y: 20.0, ..Default::default() }, KIT);
    assert!((mx[2] - 0.1621).abs() < 0.0006, "X+20 slid {} of the width", mx[2]);
    assert!((my[5] + 0.1625).abs() < 0.0006, "Y+20 slid {} of the height", my[5]);
    assert!(
        mx[2] * my[5] < 0.0,
        "the two offsets differ in handedness: {} and {}",
        mx[2],
        my[5]
    );

    // --- scale: the one slider that was already right ----------------------
    // `SCALE80` fitted 0.79995 × 0.80007 and `SCALE120` 1.19989 × 1.20011.
    for (s, want) in [(80.0f32, 0.8), (120.0, 1.2)] {
        let m = centred(&EditRecipe { perspective_scale: s, ..Default::default() }, KIT);
        assert!(
            (m[0] - want).abs() < 0.0004 && (m[4] - want).abs() < 0.0004,
            "Scale {s}: {} × {} against Lightroom's {want}",
            m[0],
            m[4]
        );
    }
}

#[test]
fn a_vertical_keystone_spreads_one_edge_and_gathers_the_other() {
    let up = EditRecipe { perspective_vertical: 100.0, ..Default::default() };
    let h = manual(&up, KIT).expect("a full-throw keystone");
    // The width of the frame's top edge against its bottom edge, after the map.
    let width_at = |y: f32| {
        let (l, _) = h.map(0.0, y).expect("the edge is not on the horizon");
        let (r, _) = h.map(1.0, y).expect("the edge is not on the horizon");
        r - l
    };
    let (top, bottom) = (width_at(0.0), width_at(1.0));
    assert!(
        bottom > top * 1.5,
        "a full keystone must really open the bottom: {bottom} vs {top}"
    );
    // Sign, stated, and MEASURED (2026-09-19): POSITIVE gathers the TOP and
    // opens the bottom. That is the opposite of what this engine shipped, and
    // the kit is unambiguous about it — Lightroom's `PERSP-V+50` fits a keystone
    // of −0.325, and independently fills the TOP corners of that export with its
    // void, which is what gathering the top leaves behind.
    //
    // Flipping the slider MIRRORS the frame top for bottom, so the negative
    // throw's top is the positive throw's bottom EXACTLY — which is the claim,
    // not merely "bigger" (an inequality here reads as true of an asymmetric map
    // too, and `1/(1+½k)` against `1/(1−½k)` is symmetric). The stretch reads
    // `|slider|`, so it is the same on both throws and cannot break the mirror.
    let down = EditRecipe { perspective_vertical: -100.0, ..Default::default() };
    let d = manual(&down, KIT).unwrap();
    let dl = d.map(0.0, 0.0).unwrap().0;
    let dr = d.map(1.0, 0.0).unwrap().0;
    assert!((dr - dl - bottom).abs() < 1e-6, "negative must OPEN the top: {}", dr - dl);
    assert!(dr - dl > 1.0, "…to more than the frame's own width: {}", dr - dl);
}

#[test]
fn the_void_is_written_where_the_map_pulls_from_outside_the_frame() {
    // The picture is a flat mid-tone, so all three of "the void", "the
    // picture" and "a smeared edge sample" are distinguishable VALUES rather
    // than merely non-zero: the void is full scale, the picture is 40 000, and
    // a clamping sampler would answer the picture's own value out there.
    let img = flat(64, 40_000);
    // A big offset slides the frame, so one side must have nothing to read. 30
    // and not the slider's end, because `OFFSET` was measured at 0.8121 on
    // 2026-09-19: a full throw now slides the frame four fifths of its width and
    // leaves too little picture for the third assertion to mean anything.
    let slide = EditRecipe { perspective_x: 30.0, ..Default::default() };
    let out = apply(&img, manual(&slide, SQUARE).unwrap()).to_rgb16();
    // WHITE, measured off Lightroom's own exports — see `VOID`. Asserting the
    // colour and not merely "not the picture" is the point: black was the
    // documented guess this replaces, and a test that only counted a void
    // would have passed either answer.
    let void = out.pixels().filter(|p| p.0 == [u16::MAX, u16::MAX, u16::MAX]).count();
    assert!(void > 64 * 8, "a quarter-frame slide must leave a void: {void} px");
    assert_eq!(
        out.pixels().filter(|p| p.0 == [0, 0, 0]).count(),
        0,
        "the void is white, not the black this engine used to write"
    );
    // …and NOT by smearing the edge, which is what a clamping sampler would do.
    let lit = out.pixels().filter(|p| p.0[0] == 40_000).count();
    assert!(lit > 64 * 32, "most of the frame is still the picture: {lit} px");
    // The identity costs nothing and changes nothing.
    assert_eq!(
        apply(&img, Homography::IDENTITY).to_rgb16().into_raw(),
        img.to_rgb16().into_raw(),
        "the identity is byte-identical"
    );
}

#[test]
fn constrain_crop_shrinks_until_it_fits_and_is_silent_when_it_already_does() {
    // 18, for the slide it produces rather than for the number: `OFFSET` was
    // measured at 0.8121 on 2026-09-19, so this is a 0.146-frame slide — the size
    // this probe was built around. At the 60 it used to say, the slide is 0.487
    // and the `wide` crop below has its own CENTRE inside the void, which
    // `constrain_crop` rightly refuses outright: it shrinks about that centre,
    // and shrinking about a point that is itself empty never reaches the picture.
    // The refusal is the correct answer to a crop like that, and a test of the
    // shrink path must not hand it one.
    let slide = EditRecipe { perspective_x: 18.0, ..Default::default() };
    let slid = manual(&slide, SQUARE).unwrap();
    // The whole frame cannot fit inside a slid one, so the constraint must act.
    const DIMS: (u32, u32) = (96, 96);
    let c = constrain_crop(slid, None, DIMS).expect("a slid frame constrains the whole-frame crop");
    assert!(c.right - c.left < 1.0 && c.bottom - c.top < 1.0, "it has to shrink: {c:?}");
    // It shrinks about the crop's own centre and keeps its aspect, because a
    // photographer who set a ratio did not ask for that ratio minus a corner.
    //
    // The crop has to REACH the void, or there is nothing to constrain and the
    // two assertions below never run. They used to sit under an `if let Some`
    // on a 0.2–0.8 crop that a 0.15-frame slide leaves entirely inside, so the
    // aspect was never once checked — an assertion that can be skipped is not
    // an assertion. This one runs into the slid edge and `expect`s an answer.
    let wide = Crop { left: 0.0, top: 0.4, right: 0.9, bottom: 0.6 };
    let k = constrain_crop(slid, Some(&wide), DIMS).expect("a crop into the void must constrain");
    let before = (wide.right - wide.left) / (wide.bottom - wide.top);
    let after = (k.right - k.left) / (k.bottom - k.top);
    assert!((before - after).abs() < 1e-3, "the aspect must survive: {before} vs {after}");
    assert!(
        ((k.left + k.right) * 0.5 - (wide.left + wide.right) * 0.5).abs() < 1e-3,
        "and so must the centre"
    );
    // A crop already inside the warped frame is left alone — `None`, not a
    // rewrite of the same numbers.
    let small = Crop { left: 0.45, top: 0.45, right: 0.55, bottom: 0.55 };
    assert_eq!(constrain_crop(slid, Some(&small), DIMS), None, "nothing to constrain");
    assert_eq!(
        constrain_crop(Homography::IDENTITY, None, DIMS),
        None,
        "the identity constrains nothing"
    );
}

#[test]
fn the_solver_undoes_a_keystone_it_did_not_put_there() {
    const KV: f32 = 0.45;
    let truth = keystone(KV);
    let data = chart(256, 256, truth, true);
    let solved =
        solve_upright(&data, 256, 256, 3).expect("a chart of converging bars states a point");
    // THE CLAIM, as arithmetic: the solver's answer composed with the keystone
    // that was applied leaves no perspective term. "The verticals are parallel
    // again", without looking at a picture.
    let before = perspective_strength(truth);
    let after = perspective_strength(truth.then(solved));
    assert!(before > 0.1, "premise: the probe really is a keystone ({before})");
    assert!(
        after < before * 0.1,
        "the correction must remove the perspective it was shown: {before} -> {after}"
    );
    // And the answer covers the frame, the way Adobe's own matrices do.
    assert!(solved.covers(), "a solved map must not deliver empty corners");
}

#[test]
fn full_and_auto_differ_by_the_damping_and_guided_is_refused() {
    let truth = keystone(0.45);
    let data = chart(256, 256, truth, true);
    let full = solve_upright(&data, 256, 256, 4).expect("Full solves");
    let auto = solve_upright(&data, 256, 256, 1).expect("Auto solves");
    let (sf, sa) = (perspective_strength(full), perspective_strength(auto));
    assert!(sa < sf, "Auto is a damped Full: {sa} against {sf}");
    assert!(sa > sf * 0.2, "…damped, not switched off: {sa} against {sf}");
    // Guided needs the guides a photographer draws. No sidecar in this library
    // carries them in a form this engine models, so the mode is refused rather
    // than answered with a guess.
    assert_eq!(solve_upright(&data, 256, 256, 5), None, "Guided is a named refusal");
    assert_eq!(solve_upright(&data, 256, 256, 0), None, "mode 0 is not the solver's business");
}

/// Level (mode 2) is a ROTATION and nothing else — the mode whose whole promise
/// is "straighten this, do not reshape it".
///
/// Its own test because it is the one mode that never reaches the keystone
/// arm: the tests above drive modes 1, 3 and 4, and a Level that quietly fell
/// through to the warp would have passed every one of them. So would a Level
/// that answered on a frame stating no lines at all, which is what the sample
/// floor refuses.
///
/// MUTATION: let mode 2 fall through to the warp, or drop
/// `Tune::MIN_SAMPLES` from the levelling gate.
#[test]
fn level_turns_the_frame_and_leaves_its_shape_alone() {
    // A chart that is BOTH turned and keystoned. The keystone is the part
    // under test: Level must leave it alone, and a Level that fell through to
    // the keystone arm would remove it. A chart of PARALLEL bars cannot say
    // that — its vanishing point is at infinity, so the row a fall-through
    // computes is zero and the two behaviours are indistinguishable.
    let turn = Homography::about_centre([
        0.994_522, -0.104_528, 0.0, 0.104_528, 0.994_522, 0.0, 0.0, 0.0, 1.0,
    ]); // 6°
    let truth = keystone(0.45).then(turn);
    assert!(perspective_strength(truth) > 0.1, "premise: the chart really is keystoned");
    let data = chart(256, 256, truth, true);
    let solved = solve_upright(&data, 256, 256, 2).expect("a turned chart states its lines");
    assert!(
        perspective_strength(solved) < 1e-4,
        "Level must add NO perspective term: {}",
        perspective_strength(solved)
    );
    assert_ne!(solved, Homography::IDENTITY, "…but it must still turn the frame");
    // …and the picture's own keystone is still there afterwards, which is the
    // other half of "straighten, do not reshape".
    let left = perspective_strength(truth.then(solved));
    assert!(
        left > perspective_strength(truth) * 0.5,
        "Level removed the photograph's perspective, which is Vertical's job: {left}"
    );
    // …and it covers, like every solved map here.
    assert!(solved.covers(), "a levelled frame must not deliver empty corners");
    // A frame with nothing to level is refused rather than turned by noise:
    // the sample floor is the only thing standing between a blank frame and a
    // rotation fitted to rounding error.
    assert_eq!(solve_upright(&vec![0.5f32; 256 * 256], 256, 256, 2), None, "nothing to level");
}

#[test]
fn a_picture_that_states_no_lines_is_refused_rather_than_warped() {
    // Flat: no gradient anywhere, so no line and no point.
    let blank = vec![0.5f32; 256 * 256];
    assert_eq!(solve_upright(&blank, 256, 256, 3), None, "a flat frame says nothing");
    // A frame of PARALLEL bars has no finite vanishing point — the degenerate
    // case the eigenvalue separation exists to catch. Refusing is the right
    // answer: there is nothing to correct.
    let straight = chart(256, 256, Homography::IDENTITY, true);
    assert_eq!(solve_upright(&straight, 256, 256, 3), None, "parallel lines need no correction");
    // Too small to mean anything, and a short buffer, are refusals not panics.
    assert_eq!(solve_upright(&vec![0.5f32; 16 * 16], 16, 16, 3), None);
    assert_eq!(solve_upright(&blank[..10], 256, 256, 3), None);

    // A vanishing point INSIDE the frame is a different refusal, and one the
    // cases above never reach: they fail before a point is found at all, so
    // the floor in `null_row_through` was never the thing saying no. A very
    // strong keystone puts the convergence point within the picture, where a
    // "correction" would fold the frame through itself.
    //
    // Driven at the level the refusal lives at, because a synthetic chart
    // strong enough to do this is also a chart the sampler cannot render
    // faithfully — and the claim is about the guard, not about the chart.
    for (name, q) in [
        ("dead centre", [0.0f32, 0.0, 1.0]),
        ("just inside", [0.3f32, -0.2, 1.0]),
        ("on the edge of the disc", [0.48f32, 0.0, 1.0]),
    ] {
        assert_eq!(
            upright::null_row_through(q),
            None,
            "{name}: a point inside the frame is not a horizon"
        );
    }
    // …and one outside it IS answered, so the refusal above is the floor and
    // not a blanket no.
    let row = upright::null_row_through([2.0, 0.0, 1.0]).expect("a point well outside the frame");
    assert!((row[0] + 0.5).abs() < 1e-6 && row[1].abs() < 1e-6, "g = -q/|q|²: {row:?}");
}

#[test]
fn the_stage_composes_upright_before_the_sliders_and_says_nothing_at_rest() {
    let img = flat(64, 30_000);
    assert_eq!(transform(&EditRecipe::default(), &img), None, "162 of 175 sidecars pay nothing");
    let up = with_matrices(1.0, vec![Homography::IDENTITY.0, ADOBE_MATRIX]);
    assert_eq!(
        transform(&up, &img).map(|h| h.0),
        Some(ADOBE_MATRIX),
        "Upright alone is Adobe's own"
    );
    let both = EditRecipe { perspective_scale: 150.0, ..up.clone() };
    let composed = transform(&both, &img).expect("both halves");
    let expect = Homography(ADOBE_MATRIX).then(manual(&both, SQUARE).unwrap());
    assert_eq!(composed.0, expect.0, "Upright runs first, the sliders adjust what it produced");
    // A mode with no matrix reaches the solver, which refuses a flat frame —
    // so the stage says nothing rather than inventing a warp.
    let unsolvable = EditRecipe { perspective_upright: 3.0, ..Default::default() };
    assert_eq!(transform(&unsolvable, &img), None, "a flat frame answers no dropdown");
}

#[test]
fn a_map_that_cannot_cover_the_frame_is_refused_rather_than_zoomed_forever() {
    // A map that has SLID the frame cannot be covered at all: the fill scales
    // about the CENTRE, and scaling about the centre never brings back what
    // left on one side. (A keystone always can be — zooming pulls the
    // destination corners toward the centre, which the map fixes — so the
    // refusal arm is for this shape, and the solver never produces it.)
    let slid = Homography([1.0, 0.0, 0.9, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    assert!(!slid.covers(), "premise: this one leaves a whole edge empty");
    assert_eq!(fill_the_frame(slid), None, "a slid frame is a refusal, not a crop of a crop");
    // A gentle one is scaled to exactly cover, and an already-covering map is
    // returned untouched rather than nudged.
    let filled = fill_the_frame(keystone(0.15)).expect("a gentle keystone can be covered");
    assert!(filled.covers(), "…and it does cover");
    assert_eq!(
        fill_the_frame(Homography(ADOBE_MATRIX)).map(|h| h.0),
        Some(ADOBE_MATRIX),
        "no nudge for Adobe's own"
    );
}

#[test]
fn the_inverse_is_an_inverse_and_a_degenerate_map_has_none() {
    let turned =
        EditRecipe { perspective_rotate: 7.0, perspective_aspect: -60.0, ..Default::default() };
    for h in [keystone(0.3), Homography(ADOBE_MATRIX), manual(&turned, KIT).unwrap()] {
        let inv = h.inverse().expect("invertible");
        for (x, y) in [(0.1, 0.2), (0.5, 0.5), (0.9, 0.75)] {
            let (u, v) = h.map(x, y).expect("in range");
            let (bx, by) = inv.map(u, v).expect("and back");
            assert!(
                (bx - x).abs() < 1e-4 && (by - y).abs() < 1e-4,
                "round trip: ({x}, {y}) -> ({u}, {v}) -> ({bx}, {by})"
            );
        }
    }
    // Scale zero collapses the frame to a point: no inverse, and the stage must
    // hand the picture back rather than divide by it.
    let zero = EditRecipe { perspective_scale: 0.0, ..Default::default() };
    let collapsed = manual(&zero, KIT).unwrap();
    assert_eq!(collapsed.inverse(), None, "a collapsed map has no inverse");
    let img = flat(8, 1);
    assert_eq!(
        apply(&img, collapsed).to_rgb16().into_raw(),
        img.to_rgb16().into_raw(),
        "a degenerate map returns the frame untouched"
    );
}
