//! The statement's numeric core against `src/fixtures/stmt-golden.json`, the
//! numbers the numpy prototype computes for a closed-form 96x64 frame.

use super::*;

const W: usize = 96;
const H: usize = 64;

fn golden() -> serde_json::Value {
    serde_json::from_str(include_str!("../fixtures/stmt-golden.json")).expect("the golden parses")
}

/// The golden's frame, from its own integer formulas: a sky that brightens to
/// the right over a darker ground, and a soft sky mask (255 above row 30, 0
/// below row 40, an integer ramp between).
fn frame() -> (Vec<[u8; 3]>, Vec<u8>) {
    let mut rgb = Vec::with_capacity(W * H);
    let mut sky = Vec::with_capacity(W * H);
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let px = if y < 36 {
                [60 + x + (y * 2) % 23, 70 + (x * 5) / 4 + y % 17, 120 + x / 2 + (y * 3) % 29]
            } else {
                [8 + (x * 3 + y * 5) % 61, 10 + (x * 2 + y * 7) % 53, 6 + (x * 5 + y * 3) % 47]
            };
            rgb.push(px.map(|c| c as u8));
            sky.push(((40 - y) * 255 / 10).clamp(0, 255) as u8);
        }
    }
    (rgb, sky)
}

/// One row of sky: 1.6 % of the frame.
fn thin_sky() -> Vec<u8> {
    (0..W * H).map(|i| if i < W { 255 } else { 0 }).collect()
}

fn weights(mask: &[u8]) -> Vec<f32> {
    mask.iter().map(|&m| f32::from(m) / 255.0).collect()
}

fn nums(v: &serde_json::Value) -> Vec<f64> {
    v.as_array().expect("an array").iter().map(|x| x.as_f64().expect("a number")).collect()
}

fn name_of(s: &str) -> AreaName {
    match s {
        "sky" => AreaName::Sky,
        "ground" => AreaName::Ground,
        "frame" => AreaName::Frame,
        other => panic!("unknown area {other}"),
    }
}

fn statement_of(v: &serde_json::Value) -> Vec<(AreaName, AreaStatement)> {
    v["areas"]
        .as_object()
        .expect("areas")
        .iter()
        .map(|(name, a)| {
            let l = nums(&a["L"]);
            let band = |k: &str| {
                let v = nums(&a[k]);
                [v[0] as f32, v[1] as f32]
            };
            (
                name_of(name),
                AreaStatement {
                    l: std::array::from_fn(|k| l[k] as f32),
                    ab: [band("ab_shadows"), band("ab_mids"), band("ab_highlights")],
                },
            )
        })
        .collect()
}

fn close(what: &str, got: f64, want: f64, tol: f64) {
    assert!((got - want).abs() <= tol, "{what}: got {got:.4}, the golden says {want:.4} (tolerance {tol})");
}

/// Every number of `measured` (one area per golden key) within `tol_l` for L*
/// and the centres, `tol_ab` for (a*, b*).
fn assert_measured(case: &str, areas: &[Area], lab: &[[f32; 3]], golden: &serde_json::Value, tol_l: f64, tol_ab: f64) {
    let want = golden.as_object().expect("measured");
    assert_eq!(areas.len(), want.len(), "{case}: the areas");
    for area in areas {
        let g = &want[area.name.as_str()];
        let got = measure(lab, &area.weight);
        let tag = |k: &str| format!("{case} {} {k}", area.name.as_str());
        close(&tag("share"), f64::from(got.share), g["share"].as_f64().unwrap(), 1e-3);
        for (k, w) in nums(&g["L"]).iter().enumerate() {
            close(&tag(&format!("L[{k}]")), f64::from(got.l[k]), *w, tol_l);
        }
        for (k, w) in nums(&g["centres"]).iter().enumerate() {
            close(&tag(&format!("centre[{k}]")), f64::from(got.centres[k]), *w, tol_l);
        }
        for (k, band) in ["ab_shadows", "ab_mids", "ab_highlights"].iter().enumerate() {
            for (c, w) in nums(&g[*band]).iter().enumerate() {
                close(&tag(&format!("{band}[{c}]")), f64::from(got.ab[k][c]), *w, tol_ab);
            }
        }
    }
}

/// The frame moved to the case's statement, then: its mean 8-bit sRGB within
/// 0.6 of a code, each probe within 2 codes, the re-measured anchors within
/// 0.3 L* and the bands within 0.5 a*, b*.
fn assert_case(key: &str, mask: &[u8]) {
    let g = golden();
    let case = &g[key];
    let (rgb, _) = frame();
    let lab = lab_of_rgb8(&rgb);
    let areas = areas_of(&weights(mask));
    assert_measured(key, &areas, &lab, &case["measured"], 0.05, 0.05);
    let out: Vec<[u8; 3]> =
        moved(&lab, &areas, &statement_of(&case["statement"])).expect("every area stated").into_iter().map(rgb8_of).collect();
    let mean = nums(&case["moved"]["mean_rgb8"]);
    for c in 0..3 {
        let got = out.iter().map(|p| f64::from(p[c])).sum::<f64>() / out.len() as f64;
        close(&format!("{key} moved mean channel {c}"), got, mean[c], 0.6);
    }
    for probe in case["moved"]["probes"].as_array().unwrap() {
        let (x, y) = (probe["x"].as_u64().unwrap() as usize, probe["y"].as_u64().unwrap() as usize);
        let want = nums(&probe["rgb8"]);
        for c in 0..3 {
            close(&format!("{key} probe ({x}, {y}) channel {c}"), f64::from(out[y * W + x][c]), want[c], 2.0);
        }
    }
    assert_measured(&format!("{key} moved"), &areas, &lab_of_rgb8(&out), &case["moved"]["measured"], 0.3, 0.5);
}

/// The two-area case: a soft sky over a ground, each measured, moved to its
/// stated numbers and blended by the soft mask.
///
/// MUTATION: use the unweighted quantile (every pixel weight 1 in
/// `weighted_quantiles`) — the sky's and the ground's anchors collapse onto the
/// frame's and `measured` goes red; drop the `(1 - total)` remainder or blend
/// the areas by a hard mask — the probes on the ramp rows go red.
#[test]
fn the_two_area_frame_is_measured_and_moved_as_the_golden_says() {
    let (_, sky) = frame();
    assert_case("two_areas", &sky);
}

/// The one-area case: one row of sky (1.6 % of the frame) is too small to
/// state, so the whole frame is ONE area named "frame".
///
/// MUTATION: compare the sky share with `SMALLEST_AREA` only on one side (or
/// drop the rule) — two areas come back and the case's area count goes red.
#[test]
fn a_sliver_of_sky_makes_the_whole_frame_one_area() {
    assert_case("one_area", &thin_sky());
    let areas = areas_of(&weights(&thin_sky()));
    assert_eq!(areas.len(), 1);
    assert_eq!(areas[0].name, AreaName::Frame);
    assert!(areas[0].weight.iter().all(|&w| w == 1.0));
}

fn assert_curve(key: &str) {
    let g = golden();
    let c = &g[key];
    let a = nums(&c["anchors"]);
    let s = nums(&c["stated"]);
    let curve = ToneCurve::through(&std::array::from_fn(|k| a[k] as f32), &std::array::from_fn(|k| s[k] as f32));
    for (at, want) in nums(&c["at"]).iter().zip(nums(&c["value"])) {
        close(&format!("{key} at L* {at}"), curve.at(*at), want, 0.01);
    }
}

/// The sky's tone curve sampled at L* 0, 5, …, 100: through the anchors, on to
/// black at black and white at white (the monotone cubic keeps the tails).
///
/// MUTATION: clamp the curve outside the anchors (hold every L* under the 2 %
/// anchor at its stated value, as the first prototype did) — the samples at
/// L* 0..15 and 80..100 go red; use a natural cubic spline instead of PCHIP
/// end slopes — the interior samples go red.
#[test]
fn the_tone_curve_matches_the_golden_through_its_tails() {
    assert_curve("curve");
}

/// Anchors that coincide (a ground black to its median) are one point at
/// their mean, and a stated lifted black stays lifted: the curve is 3.567 at
/// L* 0, not 0.
///
/// MUTATION: always prepend (0, 0) — the curve at L* 0 drops to 0; drop the
/// grouping (keep the three anchors at 0, 0, 0.1 as points) — the curve cannot
/// be built through repeated x and the low samples go red.
#[test]
fn coinciding_anchors_are_one_point_and_a_lifted_black_stays_lifted() {
    assert_curve("curve_black");
}

/// sRGB → Lab → sRGB returns every colour of an 8-bit lattice to within 0.001
/// (a quarter of a code): the inverse is the inverse of `fit::cie_lab`.
///
/// MUTATION: swap two rows of the inverse matrix, or use the forward
/// companding threshold (0.008856) on the f value in the inverse — the
/// round trip goes red.
#[test]
fn lab_to_srgb_inverts_the_objectives_conversion() {
    for r in (0..=255).step_by(17) {
        for g in (0..=255).step_by(17) {
            for b in (0..=255).step_by(17) {
                let rgb = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
                let lab = crate::fit::cie_lab(&rgb);
                let back = srgb_of_lab(lab.map(f64::from));
                for c in 0..3 {
                    assert!(
                        (back[c] - rgb[c]).abs() < 1e-3,
                        "{rgb:?} -> {lab:?} -> {back:?}"
                    );
                }
            }
        }
    }
    // Out of gamut is clipped, never NaN.
    let p = srgb_of_lab([50.0, 120.0, -120.0]);
    assert!(p.iter().all(|c| (0.0..=1.0).contains(c)));
}

/// A statement that does not name an area of the frame is refused, not
/// defaulted.
///
/// MUTATION: skip an unstated area instead of returning `UnstatedArea` — the
/// result is Ok and this goes red.
#[test]
fn moving_without_an_areas_numbers_is_refused() {
    let (rgb, sky) = frame();
    let lab = lab_of_rgb8(&rgb);
    let areas = areas_of(&weights(&sky));
    let only_sky = vec![(AreaName::Sky, AreaStatement { l: [10.0, 20.0, 30.0, 40.0, 50.0], ab: [[0.0; 2]; 3] })];
    assert_eq!(moved(&lab, &areas, &only_sky), Err(UnstatedArea(AreaName::Ground)));
}
