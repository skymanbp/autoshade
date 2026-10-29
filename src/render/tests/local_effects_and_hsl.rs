// One part of the engine's tests (src/render/tests.rs includes it): mask dehaze, the engine's active counts, local point curves, hue and sharpness, HSL, colour grading and RGB curves.

#[test]
fn mask_dehaze_renders_only_inside_the_mask() {
    // A left-half Linear mask (weight 1 at nx=0, ramping to 0 at nx=0.5 and
    // clamped past it) with Dehaze +100: every column left of the midpoint
    // must move, every column at or right of it must be BYTE-identical —
    // the local dehaze may not leak the frame-wide airlight inversion
    // outside its coverage. Measured: columns 0..=31 changed, 32..=63 not.
    let (w, h) = (64usize, 16usize);
    let base: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            // A hazy, low-contrast, slightly blue ramp — the case dehaze is for.
            let u = (i % w) as f32 / (w - 1) as f32;
            [0.30 + 0.5 * u, 0.34 + 0.45 * u, 0.42 + 0.40 * u]
        })
        .collect();
    let mut out = base.clone();
    apply_develop_anon(
        &mut out,
        w,
        h,
        &EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear {
                    zero_x: 0.5,
                    zero_y: 0.0,
                    full_x: 0.0,
                    full_y: 0.0,
                },
                amount: 1.0,
                dehaze: 100.0,
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let changed = (0..3).any(|c| out[i][c].to_bits() != base[i][c].to_bits());
            // The eased handle is below the existing 0.001 work floor at
            // the first pixel next to the zero edge (t = 1/64,
            // smoothstep(t) < 0.001), so column 31 is intentionally part
            // of the unchanged plateau under the shipped profile.
            if x >= w / 2 - 1 {
                assert!(
                    !changed,
                    "uncovered column {x} moved: {:?} → {:?}",
                    base[i], out[i]
                );
            } else {
                assert!(changed, "covered column {x} did not move: {:?}", base[i]);
            }
        }
    }
    // Positive dehaze deepens tone: the fully-covered edge must darken.
    assert!(out[0][0] < base[0][0] - 0.05, "dehazed pixel must darken: {:?}", out[0]);
}

#[test]
fn engine_active_counts_local_clarity_dehaze_texture() {
    // The activity rule feeds the GUI's ● marker AND the mask-raster budget
    // loader. Before R22 a clarity-only mask read "parked" and its bitmap
    // was never loaded — so even after the engine learned to render it, the
    // raster would have been missing. All three must count.
    assert!(!engine_active(&LocalAdjustment::default()), "a bare mask is inert");
    for (name, m) in [
        ("clarity", LocalAdjustment { clarity: 50.0, ..Default::default() }),
        ("dehaze", LocalAdjustment { dehaze: -30.0, ..Default::default() }),
        ("texture", LocalAdjustment { texture: 15.0, ..Default::default() }),
        // R23-1b: the same rule for the two new local controls — a
        // hue-only or sharpness-only bitmap mask must load its raster and
        // read as active, or it renders nothing for the same reason.
        ("hue", LocalAdjustment { hue: 40.0, ..Default::default() }),
        ("sharpness", LocalAdjustment { sharpness: -60.0, ..Default::default() }),
    ] {
        assert!(engine_active(&m), "local {name} alone must count as active");
    }
}

/// The render's local-NR pass and [`engine_active`] read ONE gate,
/// [`LOCAL_NR_GATE`]: a mask whose only move is a Noise value at or under
/// it is inert (the pass allocates nothing there), one just over it is
/// active. `!= 0.0` once made a 0.05 mask read as active — a ● the user
/// sees for a render that does nothing.
///
/// MUTATION: `>=` or `!= 0.0` back in `engine_active`'s NR term.
#[test]
fn engine_active_reads_the_renders_own_noise_gate() {
    let nr = |v: f32| LocalAdjustment { noise_reduction: v, ..Default::default() };
    assert!(!engine_active(&nr(LOCAL_NR_GATE)), "at the gate the pass allocates nothing");
    assert!(!engine_active(&nr(LOCAL_NR_GATE * 0.5)), "…and below it too");
    assert!(engine_active(&nr(LOCAL_NR_GATE + 0.05)), "just over it the pass runs");
}

/// R25 P6. The registry's one-hot/zeroing probe
/// (`catalogue::local_tiers_agree_with_the_engines_own_activity_gate`)
/// cannot reach a `Shape::Curve` row — neither `1.0` nor `0.0` is a curve
/// — so it skips all four and their `Rendered` claim would be a free
/// declaration. This is the compensating test that probe's doc names, and
/// it probes BOTH directions the same way the scalar arm does.
#[test]
fn engine_active_counts_local_point_curves() {
    use crate::recipe::CurvePoint;
    let lift = || vec![CurvePoint { input: 64, output: 96 }];
    assert!(!engine_active(&LocalAdjustment::default()), "premise: a bare mask is inert");
    for (name, m) in [
        ("main_curve", LocalAdjustment { main_curve: lift(), ..Default::default() }),
        ("red_curve", LocalAdjustment { red_curve: lift(), ..Default::default() }),
        ("green_curve", LocalAdjustment { green_curve: lift(), ..Default::default() }),
        ("blue_curve", LocalAdjustment { blue_curve: lift(), ..Default::default() }),
    ] {
        // ONE-HOT: neutral everywhere but this curve ⇒ the mask wakes up.
        assert!(engine_active(&m), "local {name} alone must count as active");
        // ZEROING: an already-active mask with this curve emptied still
        // renders (the other term holds it up) — so the term is additive,
        // not a gate that swallows the rest.
        let with_slider = LocalAdjustment { exposure_ev: 1.0, ..m.clone() };
        assert!(engine_active(&with_slider), "premise: the two-term mask is active");
        let mut without = with_slider;
        without.main_curve.clear();
        without.red_curve.clear();
        without.green_curve.clear();
        without.blue_curve.clear();
        assert!(engine_active(&without), "clearing {name} must not mute the exposure move");
    }
}

/// R25 P6, the render half: a mask whose ONLY move is a point curve must
/// move the pixels it covers and leave every other pixel BIT-IDENTICAL.
///
/// Both halves matter. Before this batch a curve-only mask fell through
/// `tone_identity` exactly as a clarity-only mask fell through every gate
/// before R22 — it rendered nothing while the sidecar carried it — and the
/// bit-identical half is what proves the local curve is weighted by the
/// mask instead of being applied to the frame.
#[test]
fn a_local_point_curve_darkens_only_inside_the_mask() {
    use crate::recipe::CurvePoint;
    let (w, h) = (16usize, 4usize);
    let base: Vec<[f32; 3]> = (0..w * h).map(|_| [0.60, 0.50, 0.40]).collect();
    // Full effect at x=0, zero from x=w/2 on (the ramp's own convention).
    let left_half = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 };
    let run = |m: LocalAdjustment| -> Vec<[f32; 3]> {
        let mut out = base.clone();
        apply_masks(
            &mut out,
            w,
            h,
            &EditRecipe { masks: vec![m], ..Default::default() },
            &MaskRasterSnapshot::default(),
            MaskFrame::AsRendered,
            FilmScale::NATIVE,
            ExportColorSpace::Srgb,
        );
        out
    };
    // A pull-down master curve: midtones map lower, ends pinned.
    let darken = vec![
        CurvePoint { input: 0, output: 0 },
        CurvePoint { input: 128, output: 64 },
        CurvePoint { input: 255, output: 255 },
    ];
    let out = run(LocalAdjustment {
        mask: left_half.clone(),
        amount: 1.0,
        main_curve: darken,
        ..Default::default()
    });
    assert!(
        out[0][0] < base[0][0] - 0.05,
        "the fully covered pixel must darken: {:?} → {:?}",
        base[0],
        out[0]
    );
    for x in w / 2..w {
        assert_eq!(out[x], base[x], "uncovered column {x} moved on a curve-only mask");
    }

    // The per-channel arm: a RED lift touches red and nothing else, so the
    // three channel curves are wired to three different fields (a copied
    // index would show up here as green or blue moving too).
    let out = run(LocalAdjustment {
        mask: left_half,
        amount: 1.0,
        red_curve: vec![
            CurvePoint { input: 0, output: 0 },
            CurvePoint { input: 128, output: 192 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    });
    assert!(out[0][0] > base[0][0] + 0.05, "red must lift: {:?} → {:?}", base[0], out[0]);
    // Green and blue keep their value — ProPhoto's green and blue, the
    // channels a curve acts on since v1.17.0 (`tone::apply_rgb_curves`,
    // measured: Camera Raw's mask curves too) — to within the transfer
    // LUTs' round trip (the working sRGB green and blue move with a
    // ProPhoto red, as they do in Camera Raw). The claim being made is
    // "the red curve touched one channel", and 2e-3 is 25× below the 0.05
    // swing above.
    let (pb, po) = (curve_channels(&base[0]), curve_channels(&out[0]));
    for (ch, name) in [(1usize, "green"), (2, "blue")] {
        assert!(
            (po[ch] - pb[ch]).abs() < 2e-3,
            "a red curve must leave {name} untouched: {:?} → {:?}",
            base[0],
            out[0]
        );
    }
    for x in w / 2..w {
        assert_eq!(out[x], base[x], "uncovered column {x} moved on a red-curve-only mask");
    }
}

/// R23-1b: the two controls the XMP writer emitted as a literal `"0"` from
/// the first sidecar on. Both must actually MOVE PIXELS, inside the mask
/// only — a slider that exports but does not render is the #15a/#10B defect
/// R22 fixed for clarity/dehaze/texture, reintroduced.
#[test]
fn local_hue_rotates_and_local_sharpness_signs_both_ways_inside_the_mask() {
    // A saturated red left half / right half, so a hue rotation is
    // measurable as a channel swing and the mask edge is unambiguous.
    let (w, h) = (16usize, 4usize);
    let base: Vec<[f32; 3]> = (0..w * h).map(|_| [0.80, 0.25, 0.20]).collect();
    // Covers the LEFT half only (the linear ramp reaches full at x=0).
    let left_half = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 };
    let run = |m: LocalAdjustment| -> Vec<[f32; 3]> {
        let mut out = base.clone();
        apply_masks(
            &mut out,
            w,
            h,
            &EditRecipe { masks: vec![m], ..Default::default() },
            &MaskRasterSnapshot::default(),
            MaskFrame::AsRendered,
            FilmScale::NATIVE,
            ExportColorSpace::Srgb,
        );
        out
    };

    // HUE: +100 is +30°, so red → orange (green rises, blue barely moves).
    let hue = run(LocalAdjustment {
        mask: left_half.clone(),
        amount: 1.0,
        hue: 100.0,
        ..Default::default()
    });
    assert!(
        hue[0][1] > base[0][1] + 0.05,
        "a +30° rotation must swing red toward orange: {:?} → {:?}",
        base[0],
        hue[0]
    );
    let right = w - 1;
    assert_eq!(hue[right], base[right], "the uncovered half must not rotate");
    // …and the rotation is a rotation: -100 goes the other way (toward
    // magenta — blue rises), not "less of the same".
    let back = run(LocalAdjustment {
        mask: left_half.clone(),
        amount: 1.0,
        hue: -100.0,
        ..Default::default()
    });
    assert!(back[0][2] > base[0][2] + 0.02, "-30° must swing the other way: {:?}", back[0]);

    // SHARPNESS: signed. A flat patch has no detail to sharpen, so measure
    // on an edge — the frame's own left column against its neighbour after
    // a step is introduced.
    let (mut edged, ew, eh) = detail_frame();
    let flat = edged.clone();
    apply_masks(
        &mut edged,
        ew,
        eh,
        &EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 },
                amount: 1.0,
                sharpness: 100.0,
                ..Default::default()
            }],
            ..Default::default()
        },
        &MaskRasterSnapshot::default(),
        MaskFrame::AsRendered,
        FilmScale::NATIVE,
        ExportColorSpace::Srgb,
    );
    let energy = |d: &[[f32; 3]], lo: usize, hi: usize| -> f32 {
        let mut e = 0.0;
        for y in 0..eh {
            for x in lo..hi.saturating_sub(1) {
                e += (luma601(&d[y * ew + x + 1]) - luma601(&d[y * ew + x])).abs();
            }
        }
        e
    };
    assert!(
        energy(&edged, 0, ew / 4) > energy(&flat, 0, ew / 4) * 1.01,
        "positive local sharpness must RAISE edge energy inside the mask"
    );
    assert!(
        (energy(&edged, 3 * ew / 4, ew) - energy(&flat, 3 * ew / 4, ew)).abs() < 1e-4,
        "…and leave the uncovered side alone"
    );
    // The negative half is the point of the signed band: it SOFTENS.
    let mut softened = flat.clone();
    apply_masks(
        &mut softened,
        ew,
        eh,
        &EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 },
                amount: 1.0,
                sharpness: -100.0,
                ..Default::default()
            }],
            ..Default::default()
        },
        &MaskRasterSnapshot::default(),
        MaskFrame::AsRendered,
        FilmScale::NATIVE,
        ExportColorSpace::Srgb,
    );
    assert!(
        energy(&softened, 0, ew / 4) < energy(&flat, 0, ew / 4) * 0.99,
        "negative local sharpness must LOWER edge energy (this is the blur half)"
    );
}

/// Deterministic mid-tone frame with fine and coarse structure — enough
/// detail for an unsharp mask to bite, no values near 0 or 1 where the
/// midtone weight or the clamps would mask a real difference.
fn detail_frame() -> (Vec<[f32; 3]>, usize, usize) {
    let (w, h) = (64usize, 48usize);
    let mut data = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let coarse = if (x / 8 + y / 8) % 2 == 0 { 0.08 } else { -0.08 };
            let fine = if (x + y) % 2 == 0 { 0.03 } else { -0.03 };
            let base = 0.45 + coarse + fine;
            data.push([base, base * 0.95 + 0.02, base * 0.9 + 0.04]);
        }
    }
    (data, w, h)
}

#[test]
fn hsl_adjusts_only_the_targeted_colour_band() {
    use crate::recipe::Hsl;
    // Red-band saturation -100 desaturates a red pixel toward grey but leaves
    // a blue pixel (a different band) untouched.
    let mut hsl = Hsl::default();
    hsl.saturation[0] = -100.0; // red band
    let mut data = vec![[0.8_f32, 0.1, 0.1], [0.1, 0.1, 0.8]];
    apply_hsl(&mut data, &hsl);
    let red = data[0];
    assert!(
        (red[0] - red[1]).abs() < 0.05 && (red[1] - red[2]).abs() < 0.05,
        "red pixel desaturated toward grey: {red:?}"
    );
    let blue = data[1];
    // ALL three channels — hsl_to_rgb derives them from three different
    // hue offsets, so a green-only defect is a real failure class, and
    // green is exactly the channel a blue→cyan cast moves first (U14).
    for (c, want) in [0.1f32, 0.1, 0.8].iter().enumerate() {
        assert!(
            (blue[c] - want).abs() < 0.02,
            "blue pixel untouched on every channel: {blue:?}"
        );
    }
    // EVERY band that is not red or one of its feathered neighbours
    // (orange, magenta) is a control: the single blue probe above let a
    // routing leak into yellow/green/aqua/purple pass (R12). Probe
    // pixels are generated by the engine's own hue converter at each
    // band's centre hue, saturated and mid-bright.
    for (name, hue) in
        [("yellow", 60.0f32), ("green", 120.0), ("aqua", 180.0), ("purple", 280.0)]
    {
        let (r0, g0, b0) = hsl_to_rgb(hue / 360.0, 0.7, 0.45);
        let mut probe = vec![[r0, g0, b0]];
        apply_hsl(&mut probe, &hsl);
        for c in 0..3 {
            assert!(
                (probe[0][c] - [r0, g0, b0][c]).abs() < 0.02,
                "red-band sat must not reach the {name} band: {:?} vs {:?}",
                probe[0],
                (r0, g0, b0)
            );
        }
    }
}

#[test]
fn hsl_neutral_is_identity_and_grey_is_untouched() {
    use crate::recipe::Hsl;
    // A neutral HSL is an exact no-op.
    let mut data = vec![[0.6_f32, 0.2, 0.2], [0.5, 0.5, 0.5]];
    let orig = data.clone();
    apply_hsl(&mut data, &Hsl::default());
    assert_eq!(data, orig);
    // A grey pixel has no hue, so even a strong all-band push leaves it alone.
    let hsl = Hsl { saturation: [100.0; 8], ..Hsl::default() };
    let mut grey = vec![[0.5_f32, 0.5, 0.5]];
    apply_hsl(&mut grey, &hsl);
    assert!(
        (grey[0][0] - 0.5).abs() < 1e-4
            && (grey[0][1] - 0.5).abs() < 1e-4
            && (grey[0][2] - 0.5).abs() < 1e-4,
        "grey untouched: {:?}",
        grey[0]
    );
    // A LUMINANCE push probes the chroma gate itself: the saturation
    // case is nearly vacuous for grey (s = 0 short-circuits both hue
    // converters), while a grey that slipped the gate would be SCALED
    // by the luminance term on all three channels (U14).
    let hsl_lum = Hsl { luminance: [100.0; 8], ..Hsl::default() };
    let mut grey2 = vec![[0.5_f32, 0.5, 0.5]];
    apply_hsl(&mut grey2, &hsl_lum);
    assert_eq!(grey2[0], [0.5, 0.5, 0.5], "grey must not respond to band luminance");
}

#[test]
fn hsl_does_not_blotch_a_near_grey_sky() {
    use crate::recipe::Hsl;
    // A near-grey overcast "sky": alternating pixels lean faintly blue vs
    // faintly aqua (s ≈ 3%), the way real demosaiced sky noise does. With
    // OPPOSITE luminance on the blue and aqua bands, the un-weighted code
    // would slam adjacent pixels to wildly different luma (a checkerboard
    // blotch). The saturation fade must keep the patch smooth.
    let mut data: Vec<[f32; 3]> = (0..64)
        .map(|i| if i % 2 == 0 { [0.71, 0.715, 0.726] } else { [0.71, 0.726, 0.722] })
        .collect();
    let hsl = Hsl { luminance: [0.0, 0.0, 0.0, 0.0, 60.0, -80.0, 0.0, 0.0], ..Hsl::default() };
    apply_hsl(&mut data, &hsl);
    let lumas: Vec<f32> = data.iter().map(luma601).collect();
    let spread = lumas.iter().cloned().fold(f32::MIN, f32::max)
        - lumas.iter().cloned().fold(f32::MAX, f32::min);
    assert!(spread < 0.04, "near-grey sky must not blotch — luma spread {spread}");
}

#[test]
fn color_grade_tints_the_targeted_tonal_region() {
    use crate::recipe::ColorGrade;
    // A blue shadow wheel pushes a DARK pixel toward blue; neutral is a no-op.
    let cg = ColorGrade { shadow_hue: 240.0, shadow_sat: 100.0, blending: 100.0, ..Default::default() };
    let mut data = vec![[0.15_f32, 0.15, 0.15]]; // dark grey
    apply_color_grade(&mut data, &cg);
    let p = data[0];
    assert!(p[2] > p[0] && p[2] > p[1], "shadow tinted blue: {p:?}");

    let mut d2 = vec![[0.4_f32, 0.3, 0.2]];
    let orig = d2.clone();
    apply_color_grade(&mut d2, &ColorGrade::default()); // neutral
    assert_eq!(d2, orig);
}

#[test]
fn rgb_curves_shape_each_channel_independently() {
    use crate::recipe::CurvePoint;
    // Each per-channel curve lifts ITS channel only, via the full
    // pipeline. Only the red curve used to be exercised — an engine that
    // ignored green_curve/blue_curve entirely stayed green (R12).
    let lift = || vec![
        CurvePoint { input: 0, output: 60 },
        CurvePoint { input: 255, output: 255 },
    ];
    let cases: [(&str, EditRecipe, usize); 3] = [
        ("red", EditRecipe { red_curve: lift(), ..Default::default() }, 0),
        ("green", EditRecipe { green_curve: lift(), ..Default::default() }, 1),
        ("blue", EditRecipe { blue_curve: lift(), ..Default::default() }, 2),
    ];
    for (name, r, ch) in cases {
        let mut data = vec![[0.0_f32, 0.0, 0.0]];
        apply_develop_anon(&mut data, 1, 1, &r);
        let p = data[0];
        assert!(p[ch] > 0.15, "{name} channel lifted: {p:?}");
        for c in (0..3).filter(|c| *c != ch) {
            assert!(p[c] < 0.02, "{name} curve must not move channel {c}: {p:?}");
        }
    }
}

#[test]
fn curve_lut_pins_missing_endpoints_but_keeps_explicit_ones() {
    use crate::recipe::CurvePoint;
    // One mid-curve click must NOT flatten the image to a constant: the
    // missing (0,0)/(1,1) endpoints are pinned, so the LUT stays a real
    // ramp through the clicked point.
    let one = curve_lut(&[CurvePoint { input: 128, output: 128 }]);
    assert!(one[0].abs() < 1e-6 && (one[255] - 1.0).abs() < 1e-6, "ends pinned");
    assert!((one[128] - 128.0 / 255.0).abs() < 1e-2, "clicked point honoured");
    assert!(one[64] > 0.1 && one[64] < 0.4, "shadows still a ramp, not clamped flat");
    assert!(one[192] > 0.6 && one[192] < 0.9, "highlights still a ramp");

    // Two inner points: everything outside them ramps to the pins instead
    // of freezing into crushed/blown bands.
    let two = curve_lut(&[
        CurvePoint { input: 64, output: 64 },
        CurvePoint { input: 192, output: 192 },
    ]);
    assert!(two[32] > 0.05 && two[32] < 0.2, "below-first ramps from (0,0)");
    assert!(two[224] > 0.8 && two[224] < 0.95, "above-last ramps to (1,1)");

    // An explicit endpoint stays authoritative — lifted blacks survive.
    let lifted = curve_lut(&[
        CurvePoint { input: 0, output: 40 },
        CurvePoint { input: 255, output: 255 },
    ]);
    assert!((lifted[0] - 40.0 / 255.0).abs() < 1e-3, "explicit (0,40) wins over the pin");
}

/// Runs `apply_masks` on `base` with one mask and nothing else in the recipe.
fn tint_masked(base: &[[f32; 3]], w: usize, h: usize, m: LocalAdjustment) -> Vec<[f32; 3]> {
    let mut out = base.to_vec();
    apply_masks(
        &mut out,
        w,
        h,
        &EditRecipe { masks: vec![m], ..Default::default() },
        &MaskRasterSnapshot::default(),
        MaskFrame::AsRendered,
        FilmScale::NATIVE,
        ExportColorSpace::Srgb,
    );
    out
}

/// A Linear mask at full weight over the whole frame: the full side runs
/// past the right edge, so every pixel centre sits on it.
fn whole_frame() -> MaskGeometry {
    MaskGeometry::Linear { zero_x: 2.0, zero_y: 0.0, full_x: 1.5, full_y: 0.0 }
}

/// The mask Color tint (`crs:LocalToningHue` / `LocalToningSaturation`), the
/// activity half: at amount 0 it is a no-op to the bit and the activity rule
/// calls the mask inert whatever its hue; at amount > 0 it moves only what
/// the mask covers, and a grey takes the swatch's colour.
#[test]
fn a_mask_tint_is_inert_at_amount_zero_and_moves_only_what_the_mask_covers() {
    let (w, h) = (16usize, 3usize);
    let rows: [[f32; 3]; 3] = [[0.60, 0.60, 0.60], [0.55, 0.70, 0.85], [0.10, 0.10, 0.12]];
    let base: Vec<[f32; 3]> = (0..w * h).map(|i| rows[i / w]).collect();
    let left_half = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 };
    let parked = LocalAdjustment { mask: left_half.clone(), toning_hue: 231.0, ..Default::default() };
    assert!(!engine_active(&parked), "a hue with no amount is inert");
    let out = tint_masked(&base, w, h, parked);
    assert!(
        out.iter().zip(&base).all(|(a, b)| (0..3).all(|c| a[c].to_bits() == b[c].to_bits())),
        "amount 0 must not move a single bit"
    );
    let tinted = LocalAdjustment {
        mask: left_half,
        toning_hue: 231.0,
        toning_amount: 50.0,
        ..Default::default()
    };
    assert!(engine_active(&tinted), "a tint with an amount is active");
    let out = tint_masked(&base, w, h, tinted);
    for row in 0..h {
        let edge = row * w + w - 1;
        assert_eq!(out[edge], base[edge], "row {row}: the uncovered half must not move");
    }
    // 231° is a blue: the grey turns blue (b over r).
    assert!(out[0][2] > out[0][0] + 0.03, "the grey must take the tint: {:?}", out[0]);
}

/// The render model the Lightroom and Camera Raw renders were fitted with
/// (`render::tint_px`): an ADDITIVE shift of Oklab (a, b) by
/// `push(hue) · amount · window(luma)`, Oklab L untouched. On a grey ramp: L
/// holds to 1e-4 everywhere, the chroma shift is `|push|·amount·w` to 1e-4
/// at every step — all of it at the window's centre, and at the ends of the
/// ramp the window's own value there (0.28 of it at luma 0.02, 0.20 at 0.98).
/// The masked path renders the same pixels as the model at full weight. The
/// wheel lands every measured hue on its measured push exactly, turns
/// monotonically and closes on itself at 360, and `tint_from_push` gives
/// back the hue to the tenth of a degree the recipe stores and the amount.
#[test]
fn the_mask_tint_is_an_oklab_chroma_shift_under_a_lightness_window() {
    let (k, dir) = (0.5, tint_push(231.0));
    let reach = k * dir[0].hypot(dir[1]);
    let unit = dir.map(|c| c / dir[0].hypot(dir[1]));
    let ramp: Vec<f32> = (0..=48).map(|i| 0.02 + 0.96 * i as f32 / 48.0).collect();
    let ok = |p: [f32; 3]| linear_to_oklab(p.map(srgb_to_linear));
    for &v in ramp.iter().chain([TINT_WINDOW_CENTRE].iter()) {
        let (before, after) = (ok([v; 3]), ok(tint_px([v; 3], dir, k)));
        assert!((after[0] - before[0]).abs() <= 1e-4, "grey {v}: Oklab L moved {} -> {}", before[0], after[0]);
        let shift = (after[1] - before[1]).hypot(after[2] - before[2]);
        assert!(
            (shift - reach * tint_window(v)).abs() <= 1e-4,
            "grey {v}: chroma shift {shift:.5}, the model says {:.5}",
            reach * tint_window(v)
        );
        let along = (after[1] - before[1]) * unit[0] + (after[2] - before[2]) * unit[1];
        assert!((along - shift).abs() <= 1e-4, "grey {v}: the shift points along the swatch's hue");
    }
    assert!((tint_window(TINT_WINDOW_CENTRE) - 1.0).abs() < 1e-6, "the window is 1 at its centre");
    assert!((tint_window(0.02) - 0.2840).abs() < 1e-3 && (tint_window(0.98) - 0.1959).abs() < 1e-3);
    // Through the mask pass at full weight: the same pixels as the model.
    let base: Vec<[f32; 3]> = ramp.iter().map(|&v| [v; 3]).collect();
    let masked = tint_masked(
        &base,
        ramp.len(),
        1,
        LocalAdjustment { mask: whole_frame(), toning_hue: 231.0, toning_amount: 50.0, ..Default::default() },
    );
    for (i, &v) in ramp.iter().enumerate() {
        let model = tint_px([v; 3], dir, k);
        assert!((0..3).all(|c| (masked[i][c] - model[c]).abs() <= 1e-5), "grey {v}: {:?} vs {model:?}", masked[i]);
    }
    // The wheel: every measured hue on its measured push (angle, per-unit
    // shift), read back from the node table the fit wrote.
    let angle = |p: [f32; 2]| p[1].atan2(p[0]).to_degrees().rem_euclid(360.0);
    for (hue, deg, per_unit) in [
        (0.0f32, 12.0f32, 0.1276f32),
        (60.0, 100.0, 0.0926),
        (120.0, 155.3, 0.1576),
        (180.0, 190.5, 0.1422),
        (210.0, 217.7, 0.1004),
        (240.0, 276.6, 0.0861),
        (300.0, 336.1, 0.1531),
    ] {
        let p = tint_push(hue);
        assert!((angle(p) - deg).abs() < 1e-3, "hue {hue} pushes along {:.3}, measured {deg}", angle(p));
        assert!((p[0].hypot(p[1]) - per_unit).abs() < 1e-5, "hue {hue} pushes {:.5}, measured {per_unit}", p[0].hypot(p[1]));
    }
    // Monotone, closed at 360, and inverted to the tenth of a degree.
    let mut turned = 0.0f32;
    for tenth in 0..3600 {
        let (a, b) = (angle(tint_push(tenth as f32 / 10.0)), angle(tint_push((tenth + 1) as f32 / 10.0)));
        let step = (b - a).rem_euclid(360.0);
        assert!(step > 0.0 && step < 2.0, "the wheel turns forward at {}: {a} -> {b}", tenth as f32 / 10.0);
        turned += step;
    }
    assert!((turned - 360.0).abs() < 1e-2, "one turn of the wheel is one turn in Oklab ({turned})");
    for hue in [0.0f32, 43.0, 120.0, 210.0, 231.0, 300.0, 359.9] {
        let p = tint_push(hue).map(|c| c * 0.37);
        let (back, amount) = tint_from_push(p);
        let off = (back - hue + 180.0).rem_euclid(360.0) - 180.0;
        assert!(off.abs() < 0.051, "hue {hue} back as {back}");
        assert!((amount - 0.37).abs() < 2e-3, "hue {hue}: amount 0.37 back as {amount}");
    }
    // And the inverse matrices are the forward ones' inverse.
    let c = [0.31, 0.52, 0.07];
    let back = oklab_to_linear(linear_to_oklab(c));
    assert!((0..3).all(|i| (back[i] - c[i]).abs() < 1e-5), "{back:?}");
}

/// The model against Lightroom's and Camera Raw's own pixels, gated on the
/// calibration corpus like every corpus test. Its `tint/` folder holds the seaside street
/// exported from Lightroom with nothing moved (`p39-flat.jpg`) and with ONE
/// whole-frame mask (one radial, `Flipped`, feather 0) carrying only the
/// Color swatch, `LocalToningHue` 210 at `LocalToningSaturation` 0.782609
/// and 0.391304
/// (`p39-tint-073.jpg`, `p39-tint-035.jpg` — the file names predate the
/// exact values, which are read from the XMP packet each export embeds).
/// The flat export is tinted as a baked image through the mask pass at those
/// exact settings and put to each tinted export, mean CIELAB ΔE76 over the
/// whole frame at 1200 px: no tint reads 10.45 and 5.38 there (floors 10 and
/// 5), the model ≤ 2.2 and ≤ 1.5. The joint fit's 1.70 is the MEAN of the
/// two exports; this model shape's own optimum, refitted on each export
/// alone, reads 2.07 and 1.28, so a 2.0 ceiling at the larger amount is not
/// one the model can meet and the ceiling sits just above its optimum.
///
/// The rest of the wheel is Camera Raw's renders of the 0.78 export's own
/// packet (`acr-*.jpg`, Camera Raw matching the Lightroom exports of that
/// packet to ΔE 0.32 / 0.33): the swatch at hues 0, 60, 120, 180, 240 and
/// 300 at amount 0.5 and at hue 210 at 0.25 and 1.0, each against the same
/// packet at amount 0 (`acr-none.jpg`), tinted through the mask pass and
/// held to the ceilings the joint fit's own residuals set (1.47–1.77 at the
/// six hues, 1.01 and 2.37 at 0.25 and 1.0, on its fitting sample).
#[test]
fn the_mask_tint_reproduces_lightroom_s_own_tint_exports() {
    let Some(root) = crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR") else {
        crate::test_skipped("mask tint calibration", "AUTOSHADE_FIT_CALIBRATION_DIR unset");
        return;
    };
    let dir = std::path::PathBuf::from(root).join("tint");
    let names = ["p39-flat.jpg", "p39-tint-073.jpg", "p39-tint-035.jpg"];
    if names.iter().any(|n| !dir.join(n).is_file()) {
        crate::test_skipped("mask tint calibration", &format!("no tint exports under {}", dir.display()));
        return;
    }
    let load = |name: &str| -> (Vec<[f32; 3]>, usize, usize) {
        let im = image::open(dir.join(name)).expect("tint export decodes");
        let h = (im.height() as f32 * 1200.0 / im.width() as f32).round() as u32;
        let im = im.resize_exact(1200, h, image::imageops::FilterType::Lanczos3).to_rgb8();
        (im.pixels().map(|p| p.0.map(|c| c as f32 / 255.0)).collect(), 1200, h as usize)
    };
    // Each side quantised to 8 bits first, as the exports are.
    let lab = |p: &[f32; 3]| -> [f32; 3] {
        let [r, g, b] = p.map(|v| srgb_to_linear((v * 255.0).round().clamp(0.0, 255.0) / 255.0));
        let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
        let x = f((0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047);
        let y = f(0.2126729 * r + 0.7151522 * g + 0.0721750 * b);
        let z = f((0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883);
        [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
    };
    let mean_de = |a: &[[f32; 3]], b: &[[f32; 3]]| -> f32 {
        let sum: f64 = a
            .iter()
            .zip(b)
            .map(|(p, q)| {
                let (x, y) = (lab(p), lab(q));
                ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt() as f64
            })
            .sum();
        (sum / a.len().max(1) as f64) as f32
    };
    let (flat, w, h) = load(names[0]);
    for (name, amount, untinted_floor, ceiling) in
        [(names[1], 78.2609, 10.0, 2.2), (names[2], 39.1304, 5.0, 1.5)]
    {
        let (tinted, tw, th) = load(name);
        assert_eq!((tw, th), (w, h), "{name}: the exports share one frame");
        let model = tint_masked(
            &flat,
            w,
            h,
            LocalAdjustment { mask: whole_frame(), toning_hue: 210.0, toning_amount: amount, ..Default::default() },
        );
        let (none, ours) = (mean_de(&flat, &tinted), mean_de(&model, &tinted));
        eprintln!("mask tint calibration, amount {:.2}: no tint ΔE {none:.2}, model ΔE {ours:.2}", amount / 100.0);
        assert!(none >= untinted_floor, "{name}: the export must differ from the flat one ({none:.2})");
        assert!(ours <= ceiling, "{name}: the model must land within ΔE {ceiling} ({ours:.2}, untinted {none:.2})");
    }
    let wheel = [
        ("acr-hue000.jpg", 0.0, 50.0, 2.0),
        ("acr-hue060.jpg", 60.0, 50.0, 2.0),
        ("acr-hue120.jpg", 120.0, 50.0, 2.0),
        ("acr-hue180.jpg", 180.0, 50.0, 2.0),
        ("acr-hue240.jpg", 240.0, 50.0, 2.0),
        ("acr-hue300.jpg", 300.0, 50.0, 2.0),
        ("acr-sat025.jpg", 210.0, 25.0, 1.2),
        ("acr-sat100.jpg", 210.0, 100.0, 2.6),
    ];
    if std::iter::once("acr-none.jpg").chain(wheel.iter().map(|c| c.0)).any(|n| !dir.join(n).is_file()) {
        crate::test_skipped("mask tint wheel calibration", &format!("no Camera Raw renders under {}", dir.display()));
        return;
    }
    let (none_px, w, h) = load("acr-none.jpg");
    for (name, hue, amount, ceiling) in wheel {
        let (tinted, tw, th) = load(name);
        assert_eq!((tw, th), (w, h), "{name}: the renders share one frame");
        let model = tint_masked(
            &none_px,
            w,
            h,
            LocalAdjustment { mask: whole_frame(), toning_hue: hue, toning_amount: amount, ..Default::default() },
        );
        let (none, ours) = (mean_de(&none_px, &tinted), mean_de(&model, &tinted));
        eprintln!("mask tint wheel, hue {hue} amount {:.2}: no tint ΔE {none:.2}, model ΔE {ours:.2}", amount / 100.0);
        assert!(none >= 3.0, "{name}: the render must differ from the untinted one ({none:.2})");
        assert!(ours <= ceiling, "{name}: the model must land within ΔE {ceiling} ({ours:.2}, untinted {none:.2})");
    }
}
