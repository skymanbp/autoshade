use super::*;

/// A frame with mass in every luminance band and both chroma classes —
/// the joint family's own fixture, so its unit tests do not depend on
/// `fit`'s.
fn joint_fixture(warm_shift: f32) -> Vec<[f32; 3]> {
    let mut px = Vec::with_capacity(4096);
    for i in 0..4096 {
        let l = 0.05 + 0.9 * (i % 64) as f32 / 63.0;
        if (i / 64) % 2 == 0 {
            px.push([l, l, l]); // neutral
        } else {
            px.push([(l + warm_shift).clamp(0.0, 1.0), l * 0.7, l * 0.35]); // chromatic
        }
    }
    px
}

/// The eight bucket weights must partition unity — every pixel counted
/// exactly once across the family, so the shares read as frame fractions
/// and the area weighting means what it says.
#[test]
fn the_joint_buckets_partition_unity() {
    let px = joint_fixture(0.0);
    let mut total = vec![0.0f32; px.len()];
    let mut w = Vec::new();
    for b in 0..JOINT_BUCKETS {
        joint_weights(&px, b, &mut w);
        assert_eq!(w.len(), px.len());
        for (t, x) in total.iter_mut().zip(&w) {
            assert!((0.0..=1.0).contains(x), "weight out of range: {x}");
            *t += x;
        }
    }
    for (i, t) in total.iter().enumerate() {
        assert!((t - 1.0).abs() < 1e-5, "pixel {i} got total weight {t}");
    }
    // …and the shares of the qualifying buckets cannot exceed the frame.
    let sum: f32 = joint_buckets(&px, &px).iter().map(|b| b.share).sum();
    assert!(sum <= 1.0 + 1e-5, "shares sum to {sum}");
}

/// Buckets correspond by VALUE, not by position: two frames of totally
/// different SIZE and layout that hold the same value populations must
/// read as matched. This is the property that makes the reading immune
/// to the composition differences frame-global matching dies on.
#[test]
fn the_joint_family_matches_by_value_not_by_position() {
    let a = joint_fixture(0.0);
    // Same populations, HALF the pixels, reversed order.
    let mut b: Vec<[f32; 3]> = a.iter().step_by(2).copied().collect();
    b.reverse();
    assert_ne!(a.len(), b.len());
    let r = joint_reading(&a, &b).expect("both sides carry the same values");
    assert!(r.weighted < 0.01, "same values must read matched: {r:?}");
    assert!(r.buckets >= 4, "the fixture must exercise most of the family: {r:?}");
    // A real difference must show up, and in a CHROMATIC bucket — the
    // shift only touches coloured pixels.
    let warm = joint_fixture(0.25);
    let r2 = joint_reading(&a, &warm).expect("reading");
    assert!(r2.weighted > 10.0 * r.weighted, "the warm shift must register: {r2:?}");
    assert!(
        r2.worst_label.ends_with("colour"),
        "a chroma-only difference must land in a colour bucket: {r2:?}"
    );
}

/// FAIL-OPEN: no evidence ⇒ no opinion, never "no problem".
#[test]
fn the_joint_family_abstains_without_evidence() {
    // A frame with two pixels cannot clear the share floor on 8 buckets.
    let tiny = vec![[0.5f32, 0.5, 0.5]; 2];
    let other = vec![[0.9f32, 0.1, 0.1]; 2];
    // Every bucket but one is empty on at least one side.
    let r = joint_reading(&tiny, &other);
    assert!(r.is_none() || r.unwrap().buckets < JOINT_BUCKETS);
    // An EMPTY side has no opinion at all.
    assert_eq!(joint_reading(&[], &[]), None);
    assert_eq!(joint_buckets(&[], &tiny).len(), 0);
}

#[test]
fn joint_far_classification_keeps_refusals_out_of_the_miss_note() {
    assert_eq!(
        classify_joint_far(JOINT_FAR_ERR + 0.01, true),
        Some(JointFarCause::Refused)
    );
    assert_eq!(
        JointFarCause::Refused.note_key(),
        crate::rationale::keys::FIT_NOTE_JOINT_REFUSED
    );
    assert_eq!(
        classify_joint_far(JOINT_FAR_ERR + 0.01, false),
        Some(JointFarCause::Miss)
    );
    assert_eq!(
        JointFarCause::Miss.note_key(),
        crate::rationale::keys::FIT_NOTE_JOINT_MISS
    );
    assert_eq!(classify_joint_far(JOINT_FAR_ERR - 0.001, true), None);
}

/// The analysis grid IS the render's grid. [`mask_weights`] and
/// `render::apply_masks`' own `weight_at` must ask about the same points,
/// or every zone is SOLVED on a population half a pixel away from the one
/// the mask will actually reach (R29 C2 — `render::MASK_SAMPLE_CENTRE`).
///
/// The fixture is the discriminating one: a 2-wide raster [0, 255] read by
/// a 4-wide frame. At pixel centres, `sx = nx·2 − 0.5` over
/// nx = 0.125/0.375/0.625/0.875 gives −0.25, 0.25, 0.75, 1.25, which clamp
/// to 0, ¼, ¾, 1 — symmetric about the frame centre and reaching BOTH
/// ends. At the refuted `x/w` the same raster reads 0, ½, 1, 1.
#[test]
fn zone_moments_sample_on_the_renders_own_grid() {
    let mut m = GrayImage::new(2, 1);
    m.put_pixel(0, 0, image::Luma([0]));
    m.put_pixel(1, 0, image::Luma([255]));
    assert_eq!(mask_weights(&m, 4, 1), vec![0.0, 0.25, 0.75, 1.0]);
}

#[test]
fn zone_moments_use_only_the_weighted_pixels() {
    // Two distinct populations; a binary mask must reproduce the selected
    // population's stats exactly, and `share` must count the weights.
    let px = [
        [0.8f32, 0.2, 0.2], // red-ish (masked out)
        [0.8, 0.2, 0.2],
        [0.2, 0.2, 0.8], // blue-ish (selected)
        [0.2, 0.2, 0.8],
    ];
    let m = zone_moments(&px, &[0.0, 0.0, 1.0, 1.0]);
    assert!((m.share - 0.5).abs() < 1e-6, "share {}", m.share);
    let b_lin = render::srgb_to_linear(0.8);
    let d_lin = render::srgb_to_linear(0.2);
    assert!((m.mean_lin[2] - b_lin).abs() < 1e-6, "blue mean {}", m.mean_lin[2]);
    assert!((m.mean_lin[0] - d_lin).abs() < 1e-6, "red mean {}", m.mean_lin[0]);
    assert!((m.chroma - 0.6).abs() < 1e-6, "chroma {}", m.chroma);
    // Soft weights: half-weight pixels still average to the same MEANS
    // (weights normalise out) but halve the share.
    let soft = zone_moments(&px, &[0.0, 0.0, 0.5, 0.5]);
    assert!((soft.mean_lin[2] - b_lin).abs() < 1e-6);
    assert!((soft.share - 0.25).abs() < 1e-6, "soft share {}", soft.share);
    // Degenerate mask: share 0, no NaNs.
    let dead = zone_moments(&px, &[0.0; 4]);
    assert_eq!(dead.share, 0.0);
    assert!(dead.luma_lin == 0.0 && dead.chroma == 0.0);
}

#[test]
fn zone_dials_recover_a_known_channel_transform() {
    // Forward-transform a zone with known per-channel linear gains, then
    // ask the fit to recover them from moments alone. The TOTAL demand
    // (gains × 2^EV) must reproduce the true gains exactly — the EV/gain
    // SPLIT is a rendering choice, the product is the identified move.
    let g_true = [1.9f32, 1.1, 0.45];
    let src: Vec<[f32; 3]> = vec![
        [0.30, 0.35, 0.45],
        [0.40, 0.42, 0.50],
        [0.25, 0.30, 0.38],
        [0.35, 0.38, 0.46],
    ];
    let lin = |c: f32| render::srgb_to_linear(c);
    let srgb = |c: f32| {
        if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    };
    let tgt: Vec<[f32; 3]> = src
        .iter()
        .map(|p| {
            let mut q = [0.0f32; 3];
            for c in 0..3 {
                q[c] = srgb((lin(p[c]) * g_true[c]).clamp(0.0, 1.0));
            }
            q
        })
        .collect();
    let w = vec![1.0f32; src.len()];
    let ms = zone_moments(&src, &w);
    let mt = zone_moments(&tgt, &w);
    let d = fit_zone_dials(&ms, &mt);
    let bright = 2.0f32.powf(d.exposure_ev);
    for (c, &truth) in g_true.iter().enumerate() {
        let total = d.color_gains[c] * bright;
        assert!(
            (total - truth).abs() < 5e-3,
            "channel {c}: recovered {total} vs true {truth}"
        );
    }
    // The brightness split itself must be sane (the transform brightens
    // red, dims blue — net luma slightly up).
    assert!(d.exposure_ev.abs() < 1.0, "ev {}", d.exposure_ev);
}

#[test]
fn zone_dials_are_neutral_for_matching_zones() {
    let px: Vec<[f32; 3]> = vec![[0.6, 0.63, 0.67], [0.55, 0.58, 0.62]];
    let w = vec![1.0f32; px.len()];
    let m1 = zone_moments(&px, &w);
    let m2 = zone_moments(&px, &w);
    let d = fit_zone_dials(&m1, &m2);
    assert!(d.exposure_ev.abs() < 0.01, "ev {}", d.exposure_ev);
    for c in 0..3 {
        assert!((d.color_gains[c] - 1.0).abs() < 0.01, "gain {c}: {}", d.color_gains[c]);
    }
    assert!(zone_sat_step(m1.chroma, m2.chroma).is_none(), "sat must converge");
}

#[test]
fn zone_dials_turn_a_pale_sky_golden_through_the_engine() {
    // The acceptance geometry of the real failure (P21 ×
    // reimagine-5, batch #2): hazy pale-BLUE sky, vivid GOLD target sky
    // (the fixtures fit.rs's rotation-gate tests pin). The zoned dials,
    // applied through the engine's bitmap-mask recolour stage, must land
    // the sky in the target's warm family — exactly the regrade the
    // global fit refuses by design — and leave the rocks equal to the
    // control render. (A Temp/Tint-only variant of this test was tried
    // first and could NOT pass: WB gains cap at r/b ≈ 1.9× where this
    // repaint demands ≈ 5.3× — that measurement is why color_gains
    // exists.)
    use crate::recipe::{EditRecipe, LocalAdjustment, MaskGeometry};
    use image::{DynamicImage, GrayImage, RgbImage};

    let (w, h) = (16u32, 16u32);
    let sky_src = [0.60f32, 0.63, 0.67]; // hazy pale blue (hue ≈ 214°)
    let sky_tgt = [0.92f32, 0.72, 0.48]; // vivid gold
    let rock = [0.55f32, 0.45, 0.35];
    let build = |sky: [f32; 3]| -> DynamicImage {
        let img = RgbImage::from_fn(w, h, |_, y| {
            let p = if y >= 12 { sky } else { rock };
            image::Rgb(p.map(|c| (c * 255.0).round() as u8))
        });
        DynamicImage::ImageRgb8(img)
    };
    let src = build(sky_src);
    let tgt = build(sky_tgt);
    // Binary sky mask on disk — the production carrier (Bitmap geometry).
    let mask_path = fixture_mask_path("zoned-dials-mask");
    GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 12 { 255u8 } else { 0 }]))
        .save(mask_path.path())
        .unwrap();

    let px_of = |img: &DynamicImage| -> Vec<[f32; 3]> {
        img.to_rgb8()
            .pixels()
            .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
            .collect()
    };
    let weights: Vec<f32> = (0..w * h).map(|i| if i / w >= 12 { 1.0 } else { 0.0 }).collect();
    let ms = zone_moments(&px_of(&src), &weights);
    let mt = zone_moments(&px_of(&tgt), &weights);
    assert!(ms.share >= MIN_ZONE_SHARE && mt.share >= MIN_ZONE_SHARE);
    let d = fit_zone_dials(&ms, &mt);
    assert!(
        d.color_gains[0] > 1.2 && d.color_gains[2] < 0.6,
        "blue→gold demands strong warm gains: {:?}",
        d.color_gains
    );

    let recipe = EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: mask_path.path().to_string_lossy().into_owned() },
            role: MaskRole::ZoneSky,
            amount: 1.0,
            exposure_ev: d.exposure_ev,
            color_gains: Some(d.color_gains),
            ..Default::default()
        }],
        ..Default::default()
    };
    let out = px_of(&render::develop_preview(&src, &recipe));
    let control = px_of(&render::develop_preview(&src, &EditRecipe::default()));
    let sky_i = (14 * w + 8) as usize;
    let rock_i = (4 * w + 8) as usize;
    // Sky: source has b > r (blue); the zoned render must land it in the
    // target's warm family (r > g > b) with a clear warm margin, near
    // the target colour (the EV rides the tone LUT's shoulder, so exact
    // equality is not expected — family + proximity is the contract).
    let sky = out[sky_i];
    assert!(sky[0] > sky[2] + 0.10, "sky must turn warm (r >> b): {sky:?}");
    assert!(sky[0] > sky[1] && sky[1] > sky[2], "gold orders r > g > b: {sky:?}");
    for c in 0..3 {
        assert!(
            (sky[c] - sky_tgt[c]).abs() < 0.25,
            "sky channel {c} far from the target: {sky:?} vs {sky_tgt:?}"
        );
    }
    // Rocks: outside the mask — must match the control render.
    for c in 0..3 {
        assert!(
            (out[rock_i][c] - control[rock_i][c]).abs() < 1e-4,
            "rocks must be untouched: {:?} vs {:?}",
            out[rock_i],
            control[rock_i]
        );
    }
    mask_path.remove();
}

/// A blue sky over warm rocks, and the same frame with the sky repainted
/// at THE SAME LUMINANCE — Rec.601 0.5180 for every sky colour here, ripple
/// slope included — so the pair is a PURE hue change and the only band the
/// frozen per-pixel evidence can call zero-evidence is the hue one. (A
/// repaint that also lifts luma takes the luma evidence one-sided with it,
/// the sky's `source_weights` fall to zero, and the region ABSTAINS before
/// the rule under test is ever reached — which is what
/// `a_region_of_only_unsupported_pixels_abstains_and_the_refusal_stands`
/// pins on purpose.)
///
/// The sky's TEXTURE is re-synthesised as well: an independent per-pixel
/// draw on each side of the pair, so the region's own structural reading
/// lands past `fit::DIVERGENCE_GLOBAL` and its pixels stop being each
/// other's counterparts. That is not decoration — it is the ONLY state in
/// which the cells are asked at all, because a region whose pixels pair
/// may not overrule them. The land half is byte-identical on both sides.
///
/// `two_ways` repaints the right half the OTHER way, cool instead of warm.
/// The zone's MEAN still asks for a warm gain — and the pixel-scale robust
/// pairing still throws one of the two halves out as outliers, which is
/// exactly the blindness R34 answers — while the region's cells, which read
/// all of it, are asked for two opposite moves by one gain.
fn zero_evidence_hue_pair(two_ways: bool) -> (DynamicImage, DynamicImage, GrayImage) {
    zero_evidence_hue_pair_with(two_ways, false)
}

/// `pale_sixth`: the frame is 96 wide (eight-pixel cells, so the sixth is
/// exactly two of the twelve cell columns) and the SOURCE's rightmost
/// sixth of the sky already sits nine tenths of the way to the warm colour
/// — the same luminance, the same direction, a small move left to make;
/// the zone gain is bounded by the strength window, so a move that small
/// is less than half of what the bounded gain applies. The
/// whole target sky is re-synthesised, so no sky pixel pairs and every
/// hue band the move touches is zero-evidence; the zone mean then solves
/// one gain for the blue five sixths, and that gain pushes the pale sixth
/// past its own targets: every cell aligned, a sixth of the mass diverged.
/// That is the reference sky's shape (0.865 / 0.135 / 1.000) and the case
/// the vouched-share search exists for.
fn zero_evidence_hue_pair_with(
    two_ways: bool,
    pale_sixth: bool,
) -> (DynamicImage, DynamicImage, GrayImage) {
    let (w, h) = if pale_sixth { (96u32, 64u32) } else { (64u32, 64u32) };
    let hash = |i: u32, seed: u32| {
        let mut v = i.wrapping_mul(747796405).wrapping_add(seed.wrapping_mul(2891336453));
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 10_000) as f32 / 10_000.0 - 0.5
    };
    let build = |repaint: bool, two_ways: bool| {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let ripple = (x as f32 / (w - 1) as f32) * 0.08;
            let blue = [0.34 + 0.3 * ripple, 0.55 + 0.5 * ripple, 0.82 + ripple];
            let p = if y < h / 2 {
                let grain = 0.22 * hash(y * w + x, if repaint { 9_999 } else { 1 });
                let warm = [0.60 + 0.8 * ripple, 0.495 + 0.4 * ripple, 0.42 + 0.2 * ripple];
                let sky = if !repaint {
                    if pale_sixth && x >= w * 5 / 6 {
                        std::array::from_fn(|c| blue[c] + 0.9 * (warm[c] - blue[c]))
                    } else {
                        blue
                    }
                } else if two_ways && x >= w / 2 {
                    [0.16 + 0.2 * ripple, 0.6165 + 0.667 * ripple, 0.95 + 0.4 * ripple]
                } else {
                    warm
                };
                sky.map(|channel| channel + grain)
            } else {
                [0.42 + ripple, 0.34 + 0.5 * ripple, 0.25 + 0.3 * ripple]
            };
            image::Rgb(p.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8))
        }))
    };
    let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
    (build(false, two_ways), build(true, two_ways), mask)
}

/// R34 §D2, and the RE-PIN of `zoned_color_gains_cannot_move_a_zero_evidence_hue_band`.
///
/// OLD RULE: a zone's colour controls are withheld whenever the probe moves
/// a hue band the frozen per-pixel evidence calls zero-evidence — always,
/// by name, whatever the target says about the result.
/// NEW RULE: they are withheld unless the REGION's own 12x8 cell means
/// vouch the rendered move — converged, not diverged, and pointing in the
/// direction each cell's own target asks for.
/// WHY: a repaint that leaves the layout alone and changes the colour has
/// no two-sided hue evidence BY CONSTRUCTION (the source sky holds no
/// Orange at all), so the old rule refused every same-layout recolour on
/// the ground that it was a recolour. The cells can tell that case from a
/// region whose layout moved, and this fixture is both halves of it.
#[test]
fn zoned_color_gains_move_a_zero_evidence_hue_band_only_where_the_regions_cells_vouch_it() {
    let (src, tgt, mask) = zero_evidence_hue_pair(false);
    let path = fixture_mask_path("zoned-evidence-hue");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);

    attach_zones(&src, &tgt, &mut report, &mask, &mask, &path);

    let sky = report
        .recipe
        .masks
        .iter()
        .find(|mask| mask.role == MaskRole::ZoneSky)
        .expect("the sky zone must attach");
    let gains = sky.color_gains.expect("a zone always carries its gains");
    // What is pinned here is the ADMISSION, not the size of the move: the
    // gain is still bounded by `zone_gain` at this Strength and still
    // shrunk by the boundary gate afterwards (measured 1.039 on this
    // fixture), so the rule is that a warm gain SHIPS where R33 flattened
    // it to unity — not that it arrives whole. `gains_withheld` is the
    // module's own definition of flattened-to-unity, so the two cannot
    // drift apart.
    assert!(
        !gains_withheld(Some(gains)) && gains[0] > gains[2],
        "the warm recolour the region's cells vouched must SHIP: {gains:?}"
    );
    let note = report
        .notes
        .iter()
        .find(|note| note.key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_CELLS)
        .unwrap_or_else(|| panic!("the admission was silent: {}", report.recipe.rationale));
    assert!(
        note.args.iter().any(|(key, value)| *key == "hue_bands" && value != "none"),
        "the band the pixel-scale reading withholds is still named: {note:?}"
    );
    let share = |name: &str| {
        note.args
            .iter()
            .find(|(key, _)| *key == name)
            .and_then(|(_, v)| v.parse::<f32>().ok())
            .unwrap_or_else(|| panic!("{name} missing from {note:?}"))
    };
    assert!(share("converged") >= 0.70, "the note prints what admitted it: {note:?}");
    assert!(share("aligned") >= 0.70, "…including the direction share: {note:?}");
    path.remove();
}

/// …and the other half of the same rule: a repaint that took half the
/// region one way and half the other is still refused, because the single
/// gain the zone's mean asks for drags one of those halves away from its
/// own cell targets. The refusal is now a MEASUREMENT rather than a bare
/// "zero evidence".
#[test]
fn a_region_whose_cells_ask_for_opposite_moves_keeps_its_colour_refusal() {
    let (src, tgt, mask) = zero_evidence_hue_pair(true);
    let path = fixture_mask_path("zoned-evidence-hue-two-ways");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);

    attach_zones(&src, &tgt, &mut report, &mask, &mask, &path);

    if let Some(sky) = report.recipe.masks.iter().find(|m| m.role == MaskRole::ZoneSky) {
        assert_eq!(
            sky.color_gains,
            Some([1.0; 3]),
            "half the region asks for the opposite move, so none of the gain ships"
        );
        assert_eq!(sky.saturation, 0.0);
    }
    let note = report
        .notes
        .iter()
        .find(|note| note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR_CELLS)
        .unwrap_or_else(|| panic!("the zoned refusal was silent: {}", report.recipe.rationale));
    assert!(
        note.args.iter().any(|(key, value)| *key == "hue_bands" && value != "none"),
        "the refused hue band must be named: {note:?}"
    );
    let cells = note
        .args
        .iter()
        .find(|(key, _)| *key == "cells")
        .map(|(_, v)| v.as_str())
        .unwrap_or_else(|| panic!("the refusal must print what it measured: {note:?}"));
    assert!(
        cells.contains("converged") || cells.contains("abstained"),
        "a refusal is a measurement now: {cells}"
    );
    path.remove();
}

/// R36. Between the two halves above: every cell asks for the same warm
/// push, a sixth of the mass has most of that push already made, and the
/// single gain the zone's mean solves pushes that sixth past its own targets.
/// R34 refused the whole move on the diverged share alone (the reference
/// sky's 0.865 / 0.135 / 1.000). The move the cells DO vouch is a share
/// of it, and that share ships — warm, non-unity, with the sentence that
/// prints both verdicts. The opposite-moves fixture above stays refused:
/// its `aligned` share fails, and a smaller move is not what it asks for.
#[test]
fn a_move_the_cells_refuse_only_for_its_size_ships_at_the_share_they_vouch() {
    let (src, tgt, mask) = zero_evidence_hue_pair_with(false, true);
    let path = fixture_mask_path("zoned-evidence-hue-pale-sixth");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);

    attach_zones(&src, &tgt, &mut report, &mask, &mask, &path);

    eprintln!("pale-sixth fixture: {}", report.recipe.rationale);
    let sky = report
        .recipe
        .masks
        .iter()
        .find(|m| m.role == MaskRole::ZoneSky)
        .expect("the sky zone must attach");
    let gains = sky.color_gains.expect("a zone always carries its gains");
    assert!(
        !gains_withheld(Some(gains)) && gains[0] > gains[2],
        "the vouched share of the warm move must SHIP: {gains:?}"
    );
    let note = report
        .notes
        .iter()
        .find(|note| note.key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_AT_SHARE)
        .unwrap_or_else(|| panic!("the shared admission was silent: {}", report.recipe.rationale));
    let arg = |name: &str| {
        note.args
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("{name} missing from {note:?}"))
    };
    let share: f32 = arg("share").parse().unwrap();
    assert!(share > 0.0 && share < 1.0, "a share of the move, not the move and not nothing: {note:?}");
    assert!(arg("full").contains("diverged"), "the refused full move is printed: {note:?}");
    assert!(arg("converged").parse::<f32>().unwrap() >= 0.70, "{note:?}");
    assert!(arg("aligned").parse::<f32>().unwrap() >= 0.70, "{note:?}");
    assert!(arg("diverged").parse::<f32>().unwrap() <= 0.10, "the shipped share is the one the cells vouch: {note:?}");
    assert!(
        !report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR_CELLS),
        "a shipped share is not also a refusal"
    );
    path.remove();
}

/// The abstention is load-bearing and unchanged: a region made only of
/// pixels no evidence weighted has nothing to say, so every strict
/// refusal stands and the note says WHY it could not be lifted.
/// The PAIRED twin of `zero_evidence_hue_pair`: the same geometry and the
/// same pure hue change (source sky blue, target sky warm at the same luma,
/// land byte-identical), but the sky's grain is ONE draw shared by both
/// sides, so every sky pixel IS its own counterpart and the region reads
/// under `DIVERGENCE_GLOBAL` — `PairingScale::Pixel`, where the cells are
/// never asked. The source holds no Orange, so the band the move lands in is
/// one-sided by construction; what differs from the cell fixture is only
/// WHO can be asked about it.
///
/// `two_ways`: the right half of the target sky goes cool instead. The zone
/// mean still solves one warm gain; the left half's pixels land closer to
/// their own targets and the right half's land farther, so half the moved
/// mass is unvouched — far over the share line — and the veto must stand.
fn paired_hue_pair(two_ways: bool) -> (DynamicImage, DynamicImage, GrayImage) {
    let (w, h) = (64u32, 64u32);
    let hash = |i: u32| {
        let mut v = i.wrapping_mul(747796405).wrapping_add(2891336453);
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 10_000) as f32 / 10_000.0 - 0.5
    };
    let build = |repaint: bool| {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let ripple = (x as f32 / (w - 1) as f32) * 0.08;
            let p = if y < h / 2 {
                let grain = 0.22 * hash(y * w + x);
                let sky = if !repaint {
                    [0.34 + 0.3 * ripple, 0.55 + 0.5 * ripple, 0.82 + ripple]
                } else if two_ways && x >= w / 2 {
                    [0.16 + 0.2 * ripple, 0.6165 + 0.667 * ripple, 0.95 + 0.4 * ripple]
                } else {
                    [0.60 + 0.8 * ripple, 0.495 + 0.4 * ripple, 0.42 + 0.2 * ripple]
                };
                sky.map(|channel| channel + grain)
            } else {
                [0.42 + ripple, 0.34 + 0.5 * ripple, 0.25 + 0.3 * ripple]
            };
            image::Rgb(p.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8))
        }))
    };
    let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
    (build(false), build(true), mask)
}

/// 2026-10-21 (user ruling: the zone probes get the per-pixel paired vouch).
///
/// OLD RULE: a region that pairs at pixel scale had its colour withheld
/// whenever the probe moved a one-sided hue band — the strict doctrine, with
/// no voucher of any kind, on the ground that a region may not overrule its
/// own pixels. NEW RULE: its own pixels ARE asked, one by one — the zone's
/// robust pairing weights and each pixel's paired target, exactly the
/// voucher the global end-state guard consults — and the band's veto is
/// lifted for the pixels that converge toward their counterparts. The
/// pixels that do not keep the veto, and the mirror half of this test is
/// the case where enough of them do that the refusal stands unchanged.
#[test]
fn zoned_color_gains_move_a_one_sided_hue_band_where_the_regions_own_pixels_vouch_it() {
    let (src, tgt, mask) = paired_hue_pair(false);
    let path = fixture_mask_path("zoned-paired-hue");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);

    attach_zones(&src, &tgt, &mut report, &mask, &mask, &path);

    let sky = report
        .recipe
        .masks
        .iter()
        .find(|mask| mask.role == MaskRole::ZoneSky)
        .expect("the sky zone must attach");
    let gains = sky.color_gains.expect("a zone always carries its gains");
    assert!(
        !gains_withheld(Some(gains)) && gains[0] > gains[2],
        "the warm recolour the region's own pixels vouched must SHIP: {gains:?}"
    );
    let note = report
        .notes
        .iter()
        .find(|note| note.key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_PIXELS)
        .unwrap_or_else(|| panic!("the paired admission was silent: {}", report.recipe.rationale));
    assert!(
        note.args.iter().any(|(key, value)| *key == "hue_bands" && !value.is_empty()),
        "the sentence must name the one-sided bands it carried movement through: {note:?}"
    );
    assert!(
        !report.notes.iter().any(|note| is_colour_refusal(note.key)),
        "one outcome, one sentence: {}",
        report.recipe.rationale
    );
    path.remove();

    // The mirror: half the paired pixels move AWAY from their own targets,
    // so half the moved mass is unvouched and the strict refusal — R33's
    // own sentence, the pixel-paired carrier — rides exactly as before.
    let (src, tgt, mask) = paired_hue_pair(true);
    let path = fixture_mask_path("zoned-paired-hue-two-ways");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);

    attach_zones(&src, &tgt, &mut report, &mask, &mask, &path);

    if let Some(sky) = report.recipe.masks.iter().find(|m| m.role == MaskRole::ZoneSky) {
        assert_gains_withheld(sky.color_gains);
    }
    assert!(
        report.notes.iter().any(|note| note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR),
        "a diverging half keeps the veto, and says so: {}",
        report.recipe.rationale
    );
    assert!(
        !report.notes.iter().any(|note| note.key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_PIXELS),
        "nothing shipped, so nothing may claim to have: {}",
        report.recipe.rationale
    );
    path.remove();
}

#[test]
fn a_region_of_only_unsupported_pixels_abstains_and_the_refusal_stands() {
    let (src, tgt, mask) = zero_evidence_hue_pair(false);
    let path = fixture_mask_path("zoned-evidence-hue-abstain");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    // Zero the SUPPORT everywhere the MASK reaches on the analysis raster
    // — its own soft edge included. Zeroing by analysis ROW instead misses
    // the cell row the feathered boundary straddles, and one read cell is
    // enough to make the region speak rather than abstain. DEF-5: support
    // is the spatial support the cells' trust is read from; the estimation
    // weights only drive and no longer reach a cell's vote, so zeroing them
    // (what this test did before) left every cell asked at its presence.
    let (s_img, _) = fit::analysis_pair(&src, &tgt);
    let alpha = mask_weights(&mask, s_img.width(), s_img.height());
    for (weight, member) in report.evidence.spatial_weights.iter_mut().zip(&alpha) {
        if *member > 0.0 {
            *weight = 0.0;
        }
    }

    attach_zones(&src, &tgt, &mut report, &mask, &mask, &path);

    if let Some(sky) = report.recipe.masks.iter().find(|m| m.role == MaskRole::ZoneSky) {
        assert_eq!(sky.color_gains, Some([1.0; 3]), "an abstention is never a vouch");
    }
    let note = report
        .notes
        .iter()
        .find(|note| note.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR_CELLS)
        .unwrap_or_else(|| panic!("the zoned refusal was silent: {}", report.recipe.rationale));
    assert!(
        note.args.iter().any(|(key, value)| *key == "cells" && value.contains("abstained")),
        "the note must say the cells abstained rather than refused: {note:?}"
    );
    path.remove();
}

// ---- orchestration ----------------------------------------------------

use image::{DynamicImage, GrayImage, RgbImage};

/// The golden-pair toy geometry shared by the orchestration tests: warm
/// rocks (top 12 rows) that the target renders a touch warmer, and a sky
/// that is a neutral grey in the source and a saturated gold in the target.
///
/// The source sky was grey-BLUE and the rocks identical until 2026-10-21,
/// when the zone probes got the per-pixel paired vouch; the fixture had only
/// ever worked because that gold was refused. Admitted, the k=1 solution
/// matches the sky to 0.008, but the 16-px frame's 24-px analysis ramp reads
/// a 0.061 luma rim to the boundary ruler (an alpha-blended gold against a
/// linear-mix allowance), the shared shrink goes to k≈0.1 — and on the path
/// from BLUE to gold the zone's residual is not monotone: `zone_err`'s chroma
/// term passes through grey, so a tenth of the move read worse than none, the
/// zone was refused, and the raster the refusal deletes left the next run on
/// the same path with nothing to attach. From a neutral sky the same path is
/// monotone and the sky ships at its shrunk share with its colour. The rocks
/// moved because with the gold refused the GLOBAL solve used to carry part of
/// the sky's demand and the land zone attached to undo it on the inverted
/// raster; with the gold shipped by the zone the global solve is neutral
/// (terminal do-no-harm) and identical rocks left the land zone nothing to
/// do. Measured 2026-10-21 over four variants: identical rocks attach the
/// sky alone; darker rocks make the global solve carry the land (saturation
/// +15, a curve) and the land zone attaches twice as range bands; these
/// slightly warmer rocks keep the global solve neutral and attach the sky
/// (warm gains) and the inverted land zone (warm gains) side by side, which
/// is the geometry the orchestration pins describe. The two forced-Atmosphere
/// tests keep the old pair ([`zoned_pair_gold`]): their subject is a sky
/// whose colour reaches its CELLS, and a grey sky has no hue band to move.
///
/// 2026-10-22: the land's left four columns are one neutral grey, identical
/// on both sides. With the global paired tone arm ungated and the white
/// balance read by luma-bin consensus, a frame of only warm rock and
/// grey-to-gold sky let the GLOBAL stages absorb the gold (ev +0.3, contrast
/// 24, 8300 K, saturation 40, cast curves), the sky zone shipped cool gains
/// [1.01, 0.99, 1.07] and the land attached twice, non-inverted. Pixels
/// that match on both sides carry the global tone and veto the warm
/// demand: measured, the global solve is neutral again and the sky ships
/// warm [1.07, 0.98, 0.91] beside the inverted land zone.
pub(super) fn zoned_pair() -> (DynamicImage, DynamicImage, GrayImage) {
    let (w, h) = (16u32, 16u32);
    let build = |sky: [f32; 3], rock: [f32; 3]| -> DynamicImage {
        let img = RgbImage::from_fn(w, h, |x, y| {
            let p = if y >= 12 {
                sky
            } else if x < 4 {
                [0.63, 0.63, 0.63]
            } else {
                rock
            };
            image::Rgb(p.map(|c| (c * 255.0).round() as u8))
        });
        DynamicImage::ImageRgb8(img)
    };
    let sky_mask =
        GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 12 { 255u8 } else { 0 }]));
    (
        build([0.63, 0.63, 0.63], [0.55, 0.45, 0.35]),
        build([0.92, 0.72, 0.48], [0.60, 0.46, 0.33]),
        sky_mask,
    )
}

/// The pre-2026-10-21 fixture: identical rocks, a grey-BLUE source sky under
/// the same gold. Kept for the forced-Atmosphere pins, whose sky is probed at
/// Cell scale (no per-pixel vouch) and whose premise needs a source hue band
/// for the colour move to be measured against the cells at all.
pub(super) fn zoned_pair_gold() -> (DynamicImage, DynamicImage, GrayImage) {
    let (w, h) = (16u32, 16u32);
    let build = |sky: [f32; 3]| -> DynamicImage {
        let img = RgbImage::from_fn(w, h, |_, y| {
            let p = if y >= 12 { sky } else { [0.55f32, 0.45, 0.35] };
            image::Rgb(p.map(|c| (c * 255.0).round() as u8))
        });
        DynamicImage::ImageRgb8(img)
    };
    let sky_mask =
        GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 12 { 255u8 } else { 0 }]));
    (build([0.60, 0.63, 0.67]), build([0.92, 0.72, 0.48]), sky_mask)
}

/// A mask fixture no OTHER process can pull out from under this one.
/// These tests write a mask, render through it, then delete it — at a
/// FIXED relative path under ./out that every concurrent `cargo test` on
/// the same checkout shared, so one run's cleanup made another run's mask
/// inert (the zone then "failed to attach" nowhere near its own code).
/// Process-unique, in the temp dir, and no ./out litter left behind.
///
/// Process-unique in its DIRECTORY, not merely in its file name. Scoping
/// only the name left the mask's PARENT as the shared system temp root,
/// and [`crate::store::OwnedRaster::claim_sibling`] writes BESIDE the
/// mask: every run added one more `mask-zone-tile-N.png` to that root.
/// Past 999 the claim could no longer succeed, so every spatial tile
/// abstained with `raster-claim` and tests nowhere near that code began
/// failing -- on a machine whose only distinguishing property was that the
/// suite had been run often enough. A per-process directory makes the
/// siblings as scoped as the mask.
pub(super) fn fixture_mask_path(name: &str) -> crate::store::OwnedRaster {
    let dir = std::env::temp_dir().join(format!("autoshade-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("fixture mask directory");
    crate::store::OwnedRaster::scratch(dir.join(format!("{name}.png")))
}

fn semantic_attachment(
    sw: Vec<f32>,
    tw: Vec<f32>,
    path: &crate::store::OwnedRaster,
) -> ZoneAttachment {
    ZoneAttachment {
        source_weights: sw,
        target_weights: tw,
        coverage: None,
        mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
        components: Vec::new(),
        range: None,
        name: String::new(),
        role: MaskRole::ZoneSky,
        inverted: false,
        label: MaskRole::ZoneSky.tag().to_string(),
        min_share: MIN_ZONE_SHARE,
    }
}

fn divergence(d: f32) -> Option<fit::Divergence> {
    Some(fit::Divergence { correlation: 1.0 - d, energy_error: 0.0, d })
}

fn pretend_full_support(evidence: &mut fit::EvidenceModel) {
    evidence.spatial_weights.fill(1.0);
    evidence.spatial_divergence.fill(0.0);
    evidence.spatial_supported.fill(true);
    evidence.globally_same_content = true;
    evidence.source_weights.fill(1.0);
    evidence.target_weights.fill(1.0);
    // v1.18.0 (R11, R2): the objective JUDGES at the presence weights
    // (membership x spatial support) and pairs where the model pairs
    // (`paired` = `spatial_supported` on an aligned model), both derived from
    // the spatial support above when the model is built. A frame pretended
    // fully supported is fully present and fully paired, or the judge keeps
    // the support this helper overrode.
    evidence.source_presence_weights.fill(1.0);
    evidence.target_presence_weights.fill(1.0);
    evidence.paired.fill(true);
}

pub(super) fn neutral_report(src: &DynamicImage, tgt: &DynamicImage) -> fit::FitReport {
    let (s, t) = fit::analysis_pair(src, tgt);
    // The entry residual is read on the report's OWN evidence model, at the
    // analysis geometry. J* pools E_paired at sigma 2 px in the frame's
    // layout, so `fit::look_err`, which infers a near-square layout from the
    // pixel count, reads a non-square fixture on a different ruler (the
    // 384x12 range fixture: 0.0021 there, 0.0732 here) and every stage gate
    // that starts from `err_after` compared two rulers.
    let evidence = fit::evidence_model_for(
        &fit::pixels_of(&s),
        &fit::pixels_of(&t),
        s.width(),
        s.height(),
    );
    let err = fit::look_err_with_evidence(&fit::pixels_of(&s), &fit::pixels_of(&t), &evidence);
    fit::FitReport {
        correspondence: None,
        recipe: crate::recipe::EditRecipe::default(),
        err_before: err,
        err_after: err,
        notes: Vec::new(),
        mode: fit::FitMode::Full,
        divergence: divergence(0.0),
        divergence_coarse: divergence(0.0),
        pairing: fit::PairingScale::Pixel,
        evidence,
        structural_evidence: None,
        atmosphere_reference: fit::AtmosphereReference::WholeFrame,
    }
}

#[test]
fn full_zone_in_atmosphere_frame_reads_structural_evidence() {
    let edge = 64u32;
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(edge, edge, |x, y| {
        let value = 0.30 + 0.02 * ((x + y) % 2) as f32;
        image::Rgb([(value * 255.0).round() as u8; 3])
    }));
    let divergent = DynamicImage::ImageRgb8(RgbImage::from_fn(edge, edge, |x, y| {
        let value = if ((x / 3) + (y / 5)) % 2 == 0 { 0.12f32 } else { 0.88 };
        image::Rgb([(value * 255.0).round() as u8; 3])
    }));
    let carried = fit::fit_recipe(&source, &divergent);
    assert_eq!(carried.mode, fit::FitMode::Atmosphere, "premise: divergent frame");
    assert!(
        carried.structural_evidence.is_some(),
        "an Atmosphere frame must carry structural evidence for its Full zones"
    );
    let target = DynamicImage::ImageRgb8(RgbImage::from_fn(edge, edge, |x, y| {
        let value = 0.50 + 0.02 * ((x + y) % 2) as f32;
        image::Rgb([(value * 255.0).round() as u8; 3])
    }));
    let (s_img, t_img) = fit::analysis_pair(&source, &target);
    let (sp, tp) = (fit::pixels_of(&s_img), fit::pixels_of(&t_img));
    let mut ingredients = fit::evidence_model_for(&sp, &tp, edge, edge);
    pretend_full_support(&mut ingredients);
    ingredients.spatial_weights.fill(0.0);
    ingredients.spatial_divergence.fill(1.0);
    ingredients.spatial_supported.fill(false);
    ingredients.globally_same_content = false;
    let ones = vec![1.0; sp.len()];
    let structural = ingredients.scoped(&tp, &ones, &ones);
    assert!(
        structural
            .luma
            .iter()
            .any(|range| range.source_populated && range.target_populated && range.weight == 0.0),
        "premise: the synthetic structural model withholds a populated tone range"
    );
    let blind = structural.structure_blind(&tp);
    let frame_err = fit::look_err_with_evidence(&sp, &tp, &blind);
    let build_report = || {
        let mut report = neutral_report(&source, &target);
        report.mode = fit::FitMode::Atmosphere;
        report.divergence = divergence(0.8);
        report.err_before = frame_err;
        report.err_after = frame_err;
        report.evidence = blind.clone();
        report.structural_evidence = Some(structural.clone());
        report
    };
    let mask = GrayImage::from_pixel(edge, edge, image::Luma([255u8]));
    let path = fixture_mask_path("atmosphere-frame-full-zone-structural");
    mask.save(path.path()).unwrap();
    let attachment = semantic_attachment(ones.clone(), ones, &path);

    let mut full_report = build_report();
    let mut full_frame_err = frame_err;
    let _ = attach_one_zone(
        &s_img,
        &tp,
        &mut full_report,
        &mut full_frame_err,
        &attachment,
        divergence(0.0),
        None,
    );
    assert!(
        full_report.notes.iter().any(|note| {
            is_tone_refusal(note.key)
                && note.args.iter().any(|(key, value)| {
                    *key == "label" && value == MaskRole::ZoneSky.tag()
                })
        }),
        "a Full zone must retain structural withholding: {}",
        full_report.recipe.rationale
    );

    let mut atmosphere_report = build_report();
    let mut atmosphere_frame_err = frame_err;
    let _ = attach_one_zone(
        &s_img,
        &tp,
        &mut atmosphere_report,
        &mut atmosphere_frame_err,
        &attachment,
        divergence(0.8),
        None,
    );
    assert!(
        !atmosphere_report.notes.iter().any(|note| {
            is_tone_refusal(note.key)
        }),
        "an Atmosphere zone must read the blind report ruler: {}",
        atmosphere_report.recipe.rationale
    );
    path.remove();
}

fn legacy_zoned_fit(
    src: &DynamicImage,
    target: &DynamicImage,
    seg: &SegmentOpts,
    mask_path: &crate::store::OwnedRaster,
    base: &crate::recipe::EditRecipe,
) -> FitReport {
    match segment_both(src, target, seg, mask_path) {
        Ok((src_mask, tgt_mask)) => {
            let zone_divergence = measure_zone_divergence(src, target, base, &src_mask);
            let divergent_cover = [zone_divergence.sky, zone_divergence.land]
                .into_iter()
                .filter(|zone| zone.divergence.is_some_and(|d| d.d >= fit::DIVERGENCE_ZONE))
                .map(|zone| zone.share)
                .sum::<f32>();
            let mut report = fit::fit_recipe_from_promoted_with_disclosure(
                src,
                target,
                base,
                divergent_cover >= fit::DIVERGENT_COVER_PROMOTES,
                true,
                None,
            );
            // v1.18.0 (R2, S-mode): the route re-pairs both rulers by the
            // finest mode read and sets the local flag before any zone.
            super::entry::local_verdicts_sky_land(src, target, base, &mut report, &zone_divergence, &src_mask);
            attach_zones_with_divergence(
                src,
                target,
                &mut report,
                &src_mask,
                &tgt_mask,
                mask_path,
                zone_divergence,
            );
            report
        }
        Err(e) => {
            let mut report = fit::fit_recipe_from_promoted_with_disclosure(
                src,
                target,
                base,
                false,
                true,
                None,
            );
            super::entry::local_verdicts_unzoned(src, target, base, &mut report);
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONED_UNAVAILABLE,
                    vec![("e", crate::rationale::error_line(&e))],
                ),
            );
            range::attach_ranges(src, target, &mut report, &[]);
            report
        }
    }
}

#[test]
fn layered_disabled_is_byte_identical_to_current_zoned_fit() {
    let (source, target, sky) = zoned_pair();
    let seg = SegmentOpts {
        python_bin: "autoshade-test-no-such-python".into(),
        script: "Cargo.toml".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let layers = ZonedLayerOpts {
        field: false, spatial: false, free_masks: false, refine_masks: false, detail: false,
    };

    let semantic_path = fixture_mask_path("layered-disabled-semantic");
    sky.save(semantic_path.path()).unwrap();
    SEGMENT_BOTH_OVERRIDE.with(|value| *value.borrow_mut() = Some((sky.clone(), sky.clone())));
    let disabled_semantic = fit_recipe_zoned_inner(
        &source,
        &target,
        &seg,
        &semantic_path,
        &crate::recipe::EditRecipe::default(),
        None,
        layers,
    );
    SEGMENT_BOTH_OVERRIDE.with(|value| *value.borrow_mut() = Some((sky.clone(), sky)));
    let legacy_semantic = legacy_zoned_fit(
        &source,
        &target,
        &seg,
        &semantic_path,
        &crate::recipe::EditRecipe::default(),
    );
    assert_eq!(
        serde_json::to_vec(&disabled_semantic.recipe).unwrap(),
        serde_json::to_vec(&legacy_semantic.recipe).unwrap(),
        "disabled semantic layers changed the pre-layer recipe bytes",
    );
    assert_eq!(disabled_semantic.err_after.to_bits(), legacy_semantic.err_after.to_bits());
    semantic_path.remove();

    let range_path = fixture_mask_path("layered-disabled-range");
    let disabled_range = fit_recipe_zoned_inner(
        &source,
        &target,
        &seg,
        &range_path,
        &crate::recipe::EditRecipe::default(),
        None,
        layers,
    );
    let legacy_range = legacy_zoned_fit(
        &source,
        &target,
        &seg,
        &range_path,
        &crate::recipe::EditRecipe::default(),
    );
    assert_eq!(
        serde_json::to_vec(&disabled_range.recipe).unwrap(),
        serde_json::to_vec(&legacy_range.recipe).unwrap(),
        "disabled range layers changed the pre-layer recipe bytes",
    );
    assert_eq!(disabled_range.err_after.to_bits(), legacy_range.err_after.to_bits());
    range_path.remove();

    let (Some(head_semantic), Some(head_range)) = (
        crate::config::live_env("AUTOSHADE_LAYERED_HEAD_SEMANTIC"),
        crate::config::live_env("AUTOSHADE_LAYERED_HEAD_RANGE"),
    ) else {
        return;
    };
    let root = fit::calibration_corpus().expect("HEAD equivalence needs calibration corpus");
    let source = image::open(root.join("neutral.jpg")).unwrap();
    let target = image::open(root.join("target.jpg")).unwrap();
    let cfg = crate::config::Config::load();
    let corr = crate::correspond::fit_provider(
        crate::correspond::CorrespondOpts::from_config(&cfg),
    );
    let semantic_path = fixture_mask_path("layered-head-semantic");
    let mut semantic = fit_recipe_zoned_inner(
        &source,
        &target,
        &SegmentOpts::from_config(&cfg, "sky"),
        &semantic_path,
        &crate::recipe::EditRecipe::default(),
        Some(&corr),
        layers,
    );
    crate::pipeline::stamp_fit_calibration(
        &mut semantic.recipe,
        crate::pipeline::fit_calibration(&root.join("neutral.jpg")),
    );
    let head_semantic: crate::recipe::EditRecipe =
        serde_json::from_slice(&std::fs::read(head_semantic).unwrap()).unwrap();
    assert_head_equivalent(
        &semantic.recipe,
        &head_semantic,
        &root.join("neutral.jpg"),
        "disabled semantic layers",
    );
    semantic_path.remove();

    let range_path = fixture_mask_path("layered-head-range");
    let range_seg = SegmentOpts {
        python_bin: cfg.python_bin.clone(),
        script: "D:/no-such-dir/none.py".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let mut range = fit_recipe_zoned_inner(
        &source,
        &target,
        &range_seg,
        &range_path,
        &crate::recipe::EditRecipe::default(),
        Some(&corr),
        layers,
    );
    crate::pipeline::stamp_fit_calibration(
        &mut range.recipe,
        crate::pipeline::fit_calibration(&root.join("neutral.jpg")),
    );
    let head_range: crate::recipe::EditRecipe =
        serde_json::from_slice(&std::fs::read(head_range).unwrap()).unwrap();
    assert_head_equivalent(
        &range.recipe,
        &head_range,
        &root.join("neutral.jpg"),
        "disabled range layers",
    );
    range_path.remove();
}

/// Write the two artefacts the HEAD-equivalence arm above reads.
///
/// That arm compares the layered fit, with every layer disabled, against a
/// recipe some earlier executable produced on the calibration pair. Until
/// v1.2.4 nothing in the tree could produce one, so the baseline was a
/// file with no procedure behind it and the arm was unrunnable for anyone
/// who did not already have it. The procedure is now exactly this:
///
/// ```text
/// git switch --detach <the commit you want as the baseline>
/// AUTOSHADE_LAYERED_HEAD_SEMANTIC=<dir>/semantic.json \
/// AUTOSHADE_LAYERED_HEAD_RANGE=<dir>/range.json \
/// AUTOSHADE_FIT_CALIBRATION_DIR=<corpus> \
///   cargo test --release -- --ignored export_layered_head_equivalence_artifacts
/// git switch -                       # then run the arm on your branch
/// ```
///
/// It writes to the SAME paths the arm reads, which is the point: the
/// baseline is whatever executable last ran this, and the commit that
/// produced it is a `git switch` away rather than a memory. Ignored by
/// default because it overwrites that baseline, and because it needs the
/// calibration corpus and the correspondence sidecar.
///
/// v1.2.4's own rebaseline is recorded in the batch report: the artefacts
/// carried before it came from the pre-layer executable, and this release
/// deliberately changes what the disabled-layer path produces on a real
/// pair (the structural reading abstains where it used to claim a match,
/// and the attachment gate reads one share ruler), so those files describe
/// a fit the tree no longer performs. The two synthetic arms above, which
/// compare against `legacy_zoned_fit` in the same process, are the
/// equivalence check that does not age.
#[test]
#[ignore]
fn export_layered_head_equivalence_artifacts() {
    let layers = ZonedLayerOpts {
        field: false, spatial: false, free_masks: false, refine_masks: false, detail: false,
    };
    let (Some(head_semantic), Some(head_range)) = (
        crate::config::live_env("AUTOSHADE_LAYERED_HEAD_SEMANTIC"),
        crate::config::live_env("AUTOSHADE_LAYERED_HEAD_RANGE"),
    ) else {
        panic!("set AUTOSHADE_LAYERED_HEAD_SEMANTIC and AUTOSHADE_LAYERED_HEAD_RANGE");
    };
    let root = fit::calibration_corpus().expect("the artefacts are cut on the corpus pair");
    let source = image::open(root.join("neutral.jpg")).unwrap();
    let target = image::open(root.join("target.jpg")).unwrap();
    let cfg = crate::config::Config::load();
    let corr = crate::correspond::fit_provider(
        crate::correspond::CorrespondOpts::from_config(&cfg),
    );
    let write = |path: &str, report: &FitReport| {
        std::fs::write(path, serde_json::to_vec_pretty(&report.recipe).unwrap()).unwrap();
        eprintln!("wrote {path}");
    };

    let semantic_path = fixture_mask_path("layered-head-export-semantic");
    let mut semantic = fit_recipe_zoned_inner(
        &source,
        &target,
        &SegmentOpts::from_config(&cfg, "sky"),
        &semantic_path,
        &crate::recipe::EditRecipe::default(),
        Some(&corr),
        layers,
    );
    crate::pipeline::stamp_fit_calibration(
        &mut semantic.recipe,
        crate::pipeline::fit_calibration(&root.join("neutral.jpg")),
    );
    write(&head_semantic, &semantic);
    semantic_path.remove();

    // The same fit with segmentation pointed at nothing, which is how the
    // arm reaches the luminance-range path instead of the semantic one.
    let range_path = fixture_mask_path("layered-head-export-range");
    let range_seg = SegmentOpts {
        python_bin: cfg.python_bin.clone(),
        script: "D:/no-such-dir/none.py".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let mut range = fit_recipe_zoned_inner(
        &source,
        &target,
        &range_seg,
        &range_path,
        &crate::recipe::EditRecipe::default(),
        Some(&corr),
        layers,
    );
    crate::pipeline::stamp_fit_calibration(
        &mut range.recipe,
        crate::pipeline::fit_calibration(&root.join("neutral.jpg")),
    );
    write(&head_range, &range);
    range_path.remove();
}

/// R33 §G, its gate moved by R41. The colour field SHIPS at the shipped
/// default Strength and above, and below the default nowhere.
///
/// Both halves are the point. The dial means "how far past Lightroom may
/// this fit go", and this is the first control that leaves Lightroom
/// entirely — classic XMP has no coordinate system for a smooth local
/// field — so below the default (the calibration point, one click back)
/// the recipe must be BYTE-IDENTICAL to the build before R33 §G, which is
/// what the first assertions say. From the default up the field attaches,
/// names itself, and is kept only if the frame it renders is measurably
/// closer to the target than the frame without it.
#[test]
fn the_colour_field_ships_from_the_default_strength_and_not_below() {
    let (source, target, sky) = zoned_pair();
    let seg = SegmentOpts {
        python_bin: "unused-colour-field".into(),
        script: "unused-colour-field".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let solve = |strength: f32, tag: &str| {
        let path = fixture_mask_path(tag);
        sky.save(path.path()).unwrap();
        SEGMENT_BOTH_OVERRIDE
            .with(|value| *value.borrow_mut() = Some((sky.clone(), sky.clone())));
        fit_recipe_zoned_inner_with_options(
            &source,
            &target,
            &seg,
            &path,
            &crate::recipe::EditRecipe::default(),
            fit::FitOptions {
                strength: crate::recipe::GradeStrength::new(strength),
                provider: None,
            },
            SHIPPED_LAYERS,
        )
    };
    // Every way the stage can end: attached, withheld, given back for the
    // frame, or given back for one zone it would have paid for out of another.
    let verdict_keys = [
        crate::rationale::keys::FIELD_ATTACHED,
        crate::rationale::keys::FIELD_WITHHELD,
        crate::rationale::keys::FIELD_REGRESSED,
        crate::rationale::keys::FIELD_ZONE_REGRESSED,
    ];

    let below = solve(crate::recipe::GradeStrength::CALIBRATED, "colour-field-below");
    assert!(
        below.recipe.colour_field.is_none(),
        "below the default the field stays an instrument: {}",
        below.recipe.rationale
    );
    assert!(
        !serde_json::to_string(&below.recipe).unwrap().contains("colour_field"),
        "…and writes no key, so an archived recipe's fingerprint is unchanged"
    );
    for key in verdict_keys {
        assert!(
            !below.notes.iter().any(|n| n.key == key),
            "a stage that cannot run must not narrate itself either"
        );
    }

    // At the default and above the stage RUNS, and says which way it went.
    // Which of the three it says depends on the fixture's own headroom, and
    // that is the honest shape of this assertion: the pin is that the stage
    // is reached and accounts for itself, never that this fixture must have
    // something left over.
    for (strength, tag) in [
        (crate::recipe::GradeStrength::DEFAULT, "colour-field-default"),
        (1.0, "colour-field-full"),
    ] {
        let ran = solve(strength, tag);
        let verdict = ran.notes.iter().find(|n| verdict_keys.contains(&n.key));
        let Some(verdict) = verdict else {
            panic!(
                "at strength {strength} the field stage must reach a verdict and disclose it: {}",
                ran.recipe.rationale
            );
        };
        if verdict.key == crate::rationale::keys::FIELD_ATTACHED {
            let field = ran.recipe.colour_field.as_ref().expect("attached means carried");
            assert!(field.renderable(), "an attached field must be one the engine can render");
            assert_eq!(field.amount, 1.0, "it attaches at full amount; the user dials it down");
            assert_eq!(
                field.grid.len(),
                field.x * field.y * field.b,
                "the grid holds exactly the vertices its shape declares"
            );
            let arg = |name: &str| {
                verdict
                    .args
                    .iter()
                    .find(|(key, _)| *key == name)
                    .and_then(|(_, value)| value.parse::<f32>().ok())
                    .unwrap_or_else(|| panic!("the attach note carries `{name}`"))
            };
            assert!(
                arg("after") < arg("before"),
                "a kept field is a field that moved the frame toward the target: {}",
                ran.recipe.rationale
            );
        } else {
            assert!(
                ran.recipe.colour_field.is_none(),
                "a withheld or regressed field is not carried"
            );
        }
    }
}

#[test]
fn field_disabled_layer_is_byte_identical() {
    let (source, target, sky) = zoned_pair();
    let seg = SegmentOpts {
        python_bin: "unused-field-none".into(),
        script: "unused-field-none".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let path = fixture_mask_path("field-disabled-byte-identity");
    sky.save(path.path()).unwrap();
    SEGMENT_BOTH_OVERRIDE.with(|value|
        *value.borrow_mut() = Some((sky.clone(), sky.clone())));
    let disabled = fit_recipe_zoned_inner(
        &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
        ZonedLayerOpts {
            field: false, spatial: false, free_masks: false, refine_masks: false, detail: false,
        },
    );
    SEGMENT_BOTH_OVERRIDE.with(|value|
        *value.borrow_mut() = Some((sky.clone(), sky)));
    field::FIELD_FORCE_NONE.with(|value| value.set(true));
    let refused = fit_recipe_zoned_inner(
        &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
        ZonedLayerOpts {
            field: true, spatial: false, free_masks: false, refine_masks: false, detail: false,
        },
    );
    assert_eq!(serde_json::to_vec(&disabled.recipe).unwrap(),
        serde_json::to_vec(&refused.recipe).unwrap());
    assert_eq!(disabled.recipe.rationale, refused.recipe.rationale);
    assert_eq!(disabled.err_after.to_bits(), refused.err_after.to_bits());
    path.remove();
}

/// REWRITTEN (v1.18.0, R3): the stop rule reads one ruler on one state, and a
/// stale ceiling stops nothing. Until R3 the field's ceiling, solved BEFORE
/// the zones, was compared with the frame after them and could skip the
/// tiles and the free masks together ("tiles, free masks"); this test forced
/// that ceiling 1e-4 under the producer-free frame to make it fire. R3 says
/// the ceiling bounds the field and the producers carved from its remainder
/// (the free masks) and nothing about the tiles: no stop runs before the
/// tiles, and the free masks' stop reads the field re-solved on the render
/// they would start from (`field::reread_field`), through `fit::ships`. So
/// the same forced first-solve ceiling must skip no tile, and any stop that
/// does fire names the free masks alone and carries two readings of that
/// re-read field under FIT_QUANT apart.
#[test]
fn a_stale_field_ceiling_stops_no_tile_and_only_a_reread_one_stops_the_free_masks() {
    let (current, target, width, height) = crate::fit_field::tests::two_band_pair();
    let image = |pixels: &[[f32; 3]]| DynamicImage::ImageRgb8(RgbImage::from_fn(
        width, height, |x, y| image::Rgb(pixels[(y * width + x) as usize]
            .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)),
    ));
    let (source, target) = (image(&current), image(&target));
    let seg = SegmentOpts {
        python_bin: "autoshade-test-no-such-python".into(),
        script: "target/b2-no-segment.py".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let path = fixture_mask_path("field-stop");
    // The FIRST solve's ceiling forced 1e-4 under the producer-free frame:
    // under the pre-R3 rule this stopped every producer; it now reaches
    // no stop at all (the override does not touch the re-read field).
    field::FIELD_CEILING_OVERRIDE.with(|value| value.set(Some(1e-4)));
    let report = fit_recipe_zoned_inner(
        &source, &target, &seg, &path, &crate::recipe::EditRecipe::default(), None,
        ZonedLayerOpts {
            field: true, spatial: true, free_masks: true, refine_masks: false, detail: false,
        },
    );
    for stop in report.notes.iter().filter(|note| note.key == crate::rationale::keys::LOCAL_STOP) {
        let arg = |key: &str| stop.args.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(arg("skipped"), "free masks", "a stop skips the free masks only: {stop:?}");
        let (global, ceiling): (f32, f32) = (arg("global").parse().unwrap(), arg("ceiling").parse().unwrap());
        assert!(
            ceiling < global && !fit::ships(ceiling, global),
            "a stop reads a re-read ceiling under FIT_QUANT from its own frame: {stop:?}"
        );
    }
    assert!(
        !report.notes.iter().any(|note| note.key == crate::rationale::keys::LOCAL_STOP
            && note.args.iter().any(|(key, value)| *key == "skipped" && value.contains("tiles"))),
        "the stale first-solve ceiling skipped the tiles: {}",
        report.recipe.rationale
    );
    let finished = report.notes.iter().filter(|note| {
        note.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED
            || note.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
    }).count();
    // Never twice. It used to be exactly once, read after a stop that ended
    // the producers; with no stop the free masks and the colour field run,
    // and the field takes this frame 0.0307 -> 0.0027 (measured), under the
    // residual that discloses an unrepresented look, so it reads 0 here.
    assert!(finished <= 1, "finished disclosure count {finished}: {}", report.recipe.rationale);
    path.remove();
}

#[test]
fn calibration_local_field_discloses_ceiling_and_realized_share() {
    let Some(root) = fit::calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).unwrap();
    let target = image::open(root.join("target.jpg")).unwrap();
    let seg = SegmentOpts {
        python_bin: "autoshade-test-no-such-python".into(),
        script: "target/b2-no-segment.py".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let head_path = fixture_mask_path("field-calibration-head");
    let head = fit_recipe_zoned_inner(
        &source, &target, &seg, &head_path, &crate::recipe::EditRecipe::default(), None,
        ZonedLayerOpts {
            field: false, spatial: true, free_masks: false, refine_masks: true, detail: false,
        },
    );
    let path = fixture_mask_path("field-calibration");
    let report = fit_recipe_zoned(&source, &target, &seg, &path);
    let ceiling = report.notes.iter()
        .find(|note| note.key == crate::rationale::keys::LOCAL_CEILING)
        .expect("calibration field ceiling disclosure");
    let number = |name: &str| ceiling.args.iter()
        .find_map(|(key, value)| (*key == name).then(|| value.parse::<f32>().unwrap()))
        .unwrap();
    assert!(number("ceiling") <= number("global"));
    // The producer-free share is measured, not written: `field.global` and
    // the report's own `err_after` are the same objective on the same
    // pixels, so the disclosure must read exactly 0.000 before any producer.
    assert_eq!(number("realized"), 0.0, "{}", report.recipe.rationale);
    // The quadtree must run under the cap the analyzer disclosed, end to end.
    let arg = |key: &str, name: &str| report.notes.iter().find(|note| note.key == key)
        .and_then(|note| note.args.iter().find_map(|(k, v)| (*k == name).then(|| v.clone())));
    assert_eq!(arg(crate::rationale::keys::LOCAL_SHAPE, "cap"),
        arg(crate::rationale::keys::TILE_DEPTH_CAP, "cap"),
        "{}", report.recipe.rationale);
    assert!(arg(crate::rationale::keys::LOCAL_SHAPE, "cap").is_some());
    assert!(report.notes.iter().any(|note| note.key == crate::rationale::keys::LOCAL_REALIZED));
    // 2026-08-30, tile-boundary root fix. This was an exact-or-better
    // claim; it is now a bounded one, and the bound is measured, not
    // guessed. The analyzer's OWN cap is tighter than the default the
    // head arm runs under on this pair -- `free_form` verdict, effective
    // tile cap 2 against 4 -- so the head arm attaches one extra tile
    // (d2r3c2, d2r3c1, then d2r2c0) that the field path never reaches.
    // That third tile is not a seam: its cross-boundary step is 0.0031
    // and it is kept whole at k=1. Until the seam budget could actually
    // bind, the free-mask stage happened to cover the gap; with the
    // budget binding, both of this pair's free-mask proposals are
    // refused by the zone estimator instead, so the cap's own cost is
    // now visible in the arithmetic: 9.6e-5, 0.13% of the reading. What
    // is being paid for is the analyzer's stopping rule, not a
    // regression in the fit, so the guard keeps its direction under a
    // stated ceiling rather than a claim it can no longer make.
    const FIELD_CAP_COST: f32 = 2e-4;
    assert!(report.err_after <= head.err_after + FIELD_CAP_COST,
        "field path {} regressed HEAD semantics {} by more than the              analyzer's disclosed tile-cap cost", report.err_after, head.err_after);
    // RE-PINNED (v1.18.0): 16 -> 24 KiB, measured 20,177 bytes on J*, where
    // R3 lets the tiles run before any stop; the tile and range verdicts are
    // the bulk of the text (by sentence
    // family: zoned tile 1.3 KB, colour ranges 1.2 KB, range boundary gate
    // 0.8 KB). The persisted cap is `EditRecipe::MAX_RATIONALE` (512 KiB);
    // this bound only catches a runaway disclosure.
    assert!(report.recipe.rationale.len() < 24 * 1024,
        "rationale is {} bytes: {}", report.recipe.rationale.len(), report.recipe.rationale);
    eprintln!("calibration HEAD={} field={} rationale={}",
        head.err_after, report.err_after, report.recipe.rationale.len());
    head_path.remove();
    path.remove();
}

/// The pre-batch executable clamped its persisted rationale at 4096
/// bytes (this batch raised that bound after the clamp ate the tile
/// attachment disclosure): its text must be a clamped prefix of ours,
/// never a different story, and every other field must survive the same
/// store normalization byte for byte.
fn assert_head_equivalent(
    current: &crate::recipe::EditRecipe,
    head: &crate::recipe::EditRecipe,
    raw: &std::path::Path,
    what: &str,
) {
    let mut current = current.clone();
    let mut head = head.clone();
    let current_rationale = std::mem::take(&mut current.rationale);
    let head_rationale = std::mem::take(&mut head.rationale);
    assert!(
        current_rationale.starts_with(&head_rationale)
            && (current_rationale.len() == head_rationale.len()
                || head_rationale.len() >= 4096 - 4),
        "{what}: the pre-batch rationale is not a clamped prefix \
             (head {} bytes, current {} bytes)",
        head_rationale.len(),
        current_rationale.len(),
    );
    assert_eq!(
        normalized_persisted_recipe(&current, raw),
        normalized_persisted_recipe(&head, raw),
        "{what}: disabled layers differ from the pre-batch executable",
    );
}

fn normalized_persisted_recipe(
    recipe: &crate::recipe::EditRecipe,
    raw: &std::path::Path,
) -> Vec<u8> {
    let bytes = crate::pipeline::recipe_store_bytes(raw, recipe, crate::diag::stderr()).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    if let Some(masks) = value.get_mut("masks").and_then(serde_json::Value::as_array_mut) {
        for (index, adjustment) in masks.iter_mut().enumerate() {
            if let Some(mask) = adjustment.get_mut("mask")
                && mask.get("kind").and_then(serde_json::Value::as_str) == Some("bitmap")
                && let Some(path) = mask.get_mut("path")
            {
                *path = serde_json::Value::String(format!("<bitmap-{index}>"));
            }
        }
    }
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn zone_divergence_uses_the_same_source_mask_on_both_sides() {
    // The gold pair: its sky population reads D well clear of the flat
    // land's 0, which is what the discrimination check below needs (the
    // grey-sky pair's sky reads 0.047, inside the 0.05 line).
    let (src, tgt, source_mask) = zoned_pair_gold();
    let measured = measure_zone_divergence(
        &src,
        &tgt,
        &crate::recipe::EditRecipe::default(),
        &source_mask,
    );
    let fit::DivergenceRaster { source: sp, target: tp, width: w, height: h, .. } =
        fit::divergence_raster(&src, &tgt, &crate::recipe::EditRecipe::default());
    let source_weights = mask_weights(&source_mask, w, h);
    let direct = fit::structure_divergence(&sp, &tp, w, h, &source_weights);
    assert_eq!(measured.sky.divergence, direct);
    assert!(direct.is_some(), "the source population resolves (the equality above is not vacuous)");

    // A deliberately wrong target-derived population produces a different
    // reading. The production helper cannot receive this mask: D owns the
    // source correspondence while moment matching remains target-masked.
    // (Over this fixture that population is the flat land alone: no
    // gradient on either side, so its D is the energy term's 0 — a uniform
    // patch that stayed uniform — while the sky population's blend rows
    // carry gradient on both sides and correlate.)
    let target_mask = GrayImage::from_fn(16, 16, |_, y| {
        image::Luma([if y < 6 { 255u8 } else { 0 }])
    });
    let wrong_weights = mask_weights(&target_mask, w, h);
    let wrong = fit::structure_divergence(&sp, &tp, w, h, &wrong_weights);
    assert!(
        (wrong.expect("resolvable").d - direct.expect("resolvable").d).abs() > 0.05,
        "the two mask populations must be discriminating: source={direct:?}, target={wrong:?}"
    );

    // Optional measured calibration; the corpus is located by an
    // environment variable (`fit::calibration_dir`), never by a path
    // literal in the source.
    let Some(fixture) = fit::calibration_corpus() else { return };
    let saved = fixture.join("sky-mask.png");
    if fixture.join("neutral.jpg").exists() && saved.exists() {
        let source = image::open(fixture.join("neutral.jpg")).unwrap();
        let target = image::open(fixture.join("target.jpg")).unwrap();
        let mask = image::open(&saved).unwrap().to_luma8();
        let actual = measure_zone_divergence(
            &source,
            &target,
            &crate::recipe::EditRecipe::default(),
            &mask,
        );
        eprintln!(
            "CALIBRATION_DIVERGENCE sky={:?} land={:?}",
            actual.sky.divergence.map(|r| r.d),
            actual.land.divergence.map(|r| r.d)
        );
        // RE-PINNED for v1.18.0 DEF-3: sky 1.186 -> 1.875, land 0.436 ->
        // 0.510. The sky core's sigma 8 / 16 bands no longer read the land
        // below the mask (nor the land's the sky above it); both stay on
        // their side of DIVERGENCE_ZONE (0.65), so the zone verdicts hold.
        assert!(
            actual.sky.divergence.is_some_and(|r| (r.d - 1.875).abs() <= 0.05),
            "sky calibration drifted: {:?}",
            actual.sky.divergence
        );
        assert!(
            actual.land.divergence.is_some_and(|r| (r.d - 0.510).abs() <= 0.05),
            "land calibration drifted: {:?}",
            actual.land.divergence
        );
    }
}

#[test]
fn atmosphere_zone_shrinks_gains_toward_unity_but_keeps_their_direction() {
    let original = [1.49f32, 0.83, 0.69];
    let default_window =
        fit::FitBudget::for_strength(crate::recipe::GradeStrength::default()).zone_gain;
    assert_eq!(
        default_window,
        (ZONE_ATMOS_GAIN_MIN, ZONE_ATMOS_GAIN_MAX),
        "the shipped default window is the budget's DEFAULT point, unchanged"
    );
    let shrunk = shrink_atmosphere_gains_in(original, default_window);
    assert!(shrunk
        .iter()
        .all(|g| (ZONE_ATMOS_GAIN_MIN..=ZONE_ATMOS_GAIN_MAX).contains(g)));
    // …and the dial now reaches it: at Strength 1.0 the same fitted gains
    // keep more of their demand, at Strength 0 less, and the direction is
    // the same scalar shrink in every case.
    let wide = shrink_atmosphere_gains_in(
        original,
        fit::FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0)).zone_gain,
    );
    let narrow = shrink_atmosphere_gains_in(
        original,
        fit::FitBudget::for_strength(crate::recipe::GradeStrength::new(0.0)).zone_gain,
    );
    for channel in 0..3 {
        let demand = |g: f32| (g - 1.0).abs();
        assert!(
            demand(wide[channel]) >= demand(shrunk[channel]) - 1e-6
                && demand(shrunk[channel]) >= demand(narrow[channel]) - 1e-6,
            "channel {channel} is not monotone in strength: {narrow:?} {shrunk:?} {wide:?}"
        );
    }
    let mut common_k: Option<f32> = None;
    for channel in 0..3 {
        assert_eq!(
            (original[channel] - 1.0).signum(),
            (shrunk[channel] - 1.0).signum(),
            "channel {channel} reversed its hue direction"
        );
        assert!((shrunk[channel] - 1.0).abs() <= (original[channel] - 1.0).abs());
        let k = (shrunk[channel] - 1.0) / (original[channel] - 1.0);
        if let Some(first) = common_k {
            assert!((k - first).abs() <= 1e-6, "channels did not share one shrink scalar");
        } else {
            common_k = Some(k);
        }
    }
    assert!(
        shrunk.iter().any(|g| {
            (*g - ZONE_ATMOS_GAIN_MIN).abs() <= 1e-6
                || (*g - ZONE_ATMOS_GAIN_MAX).abs() <= 1e-6
        }),
        "the largest legal k must reach a budget boundary: {shrunk:?}"
    );
}

/// RE-PIN (R34 §D2), REWRITTEN for v1.18.0 (R1).
///
/// R34 §D2: an Atmosphere zone is attached -- its divergence does not veto it
/// -- and its colour follows its own cells: withheld unless the target's own
/// 12x8 cell means over that zone vouch the rendered move, with exactly one
/// colour verdict disclosed for it.
/// R1 (v1.18.0) held the shrunk set to a frame gain of FIT_QUANT and refused
/// this sky on 0.11175 -> 0.11064 (a gain of 0.00111) although its own zone
/// improved (0.653 -> 0.649). R19 (v1.18.0): a shrunk LOCAL correction is
/// held to do-no-harm on its own zone (no worse than the shipped set without
/// it) with the frame a guard of FIT_QUANT (`AcceptedZone::still_accepted`),
/// its gain having been decided at k = 1 on its own population. The sky
/// shrunk to k 0.184 leaves its zone better and the frame inside the guard,
/// so it SHIPS. What the route must do: attach the zone in Atmosphere mode,
/// disclose one colour verdict for it, keep the shrunk sky and refuse
/// nothing. That the colour the cells decide is the colour the solve renders
/// is pinned at the solve, in `atmosphere_zone_skips_the_within_zone_cdf_solve`.
#[test]
fn a_divergent_zone_attaches_in_atmosphere_mode_its_colour_follows_its_cells_and_its_shrink_ships_on_its_own_zone() {
    let (src, tgt, sky_mask) = zoned_pair_gold();
    let mask_path = fixture_mask_path("zoned-atmos-attached");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    attach_zones_with_divergence(
        &src,
        &tgt,
        &mut report,
        &sky_mask,
        &sky_mask,
        &mask_path,
        ZoneDivergences {
            sky: ZoneDivergence { divergence: divergence(0.80), share: 0.25 },
            land: ZoneDivergence { divergence: divergence(0.0), share: 0.75 },
        },
    );
    mask_path.remove();
    let about_sky = |note: &crate::rationale::Note| {
        note.args.iter().any(|(name, value)| *name == "label" && value == "sky")
    };
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::ZONE_MODE_ATMOSPHERE && about_sky(n)),
        "the divergent sky must be solved in Atmosphere mode: {}",
        report.recipe.rationale
    );
    let vouched = report.notes.iter().any(|n| {
        (n.key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_CELLS
            || n.key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_PIXELS)
            && about_sky(n)
    });
    let refused = report.notes.iter().any(|n| is_colour_refusal(n.key) && about_sky(n));
    assert!(
        vouched != refused,
        "exactly one colour verdict must be disclosed for sky: {}",
        report.recipe.rationale
    );
    assert!(
        !report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_SHRUNK_REFUSED && about_sky(n)),
        "R19: a shrunk sky no worse on its own zone, the frame inside its guard, is not refused: {}",
        report.recipe.rationale
    );
    assert!(
        report.recipe.masks.iter().any(|m| m.role == MaskRole::ZoneSky),
        "the shrunk sky ships: {:?}",
        report.recipe.masks
    );
}

/// The Atmosphere solve is bounded: exposure within ZONE_ATMOS_EV_LIMIT,
/// saturation within ZONE_ATMOS_SAT_LIMIT, and none of the within-zone CDF
/// tone dials (contrast, highlights, shadows, whites, blacks). Read at the
/// SOLVE (`attach_one_zone`), where those controls are set. v1.18.0 (R1): on
/// this fixture the whole sky/land route no longer ships the sky -- its
/// boundary shrink leaves a frame gain under FIT_QUANT (see the test above)
/// -- so the route's recipe can no longer show what the solve chose; the
/// fixture, the divergence and the running frame are the route's own.
/// R34 §D2: the colour verdict the cells give and the gains the solve
/// renders agree.
#[test]
fn atmosphere_zone_skips_the_within_zone_cdf_solve() {
    let (src, tgt, sky_mask) = zoned_pair_gold();
    let mask_path = fixture_mask_path("zoned-atmos-no-cdf");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let t_px = fit::pixels_of(&t_img);
    let sw = mask_weights(&sky_mask, s_img.width(), s_img.height());
    let tw = mask_weights(&sky_mask, t_img.width(), t_img.height());
    let attachment = semantic_attachment(sw, tw, &mask_path);
    // The route's running frame starts at the report's own entry residual.
    let mut frame_err = report.err_after;
    let accepted = attach_one_zone(
        &s_img,
        &t_px,
        &mut report,
        &mut frame_err,
        &attachment,
        divergence(0.80),
        None,
    );
    mask_path.remove();
    let zone = accepted.unwrap_or_else(|| {
        panic!("the Atmosphere sky must attach at its solve: {}", report.recipe.rationale)
    });
    assert!(report
        .notes
        .iter()
        .any(|n| n.key == crate::rationale::keys::ZONE_MODE_ATMOSPHERE));
    let sky = &report.recipe.masks[zone.mask_index];
    assert!(sky.exposure_ev.abs() <= ZONE_ATMOS_EV_LIMIT, "{sky:?}");
    assert_eq!(
        (sky.contrast, sky.highlights, sky.shadows, sky.whites, sky.blacks),
        (0.0, 0.0, 0.0, 0.0, 0.0),
        "an Atmosphere zone solves no within-zone CDF tone"
    );
    assert!(sky.saturation.abs() <= ZONE_ATMOS_SAT_LIMIT, "{sky:?}");
    assert_colour_verdict_matches_recipe(&report, "sky", sky.color_gains);
}

#[test]
fn a_matching_zone_next_to_a_divergent_one_keeps_full_mode() {
    let (src, tgt, sky_mask) = zoned_pair();
    let mask_path = fixture_mask_path("zoned-independent-modes");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    attach_zones_with_divergence(
        &src,
        &tgt,
        &mut report,
        &sky_mask,
        &sky_mask,
        &mask_path,
        ZoneDivergences {
            sky: ZoneDivergence { divergence: divergence(0.80), share: 0.25 },
            land: ZoneDivergence { divergence: divergence(0.20), share: 0.75 },
        },
    );
    assert!(report
        .notes
        .iter()
        .any(|n| n.key == crate::rationale::keys::ZONE_MODE_ATMOSPHERE));
    assert!(report
        .notes
        .iter()
        .any(|n| n.key == crate::rationale::keys::ZONE_MODE_FULL));
    mask_path.remove();
}

#[test]
fn semantic_regions_select_independent_modes_and_worst_confidence() {
    let (w, h) = (12u32, 4u32);
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
        image::Rgb([80 + (x * 3) as u8, 100, 120])
    }));
    let target = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
        image::Rgb([if x < 4 { 180 } else if x < 8 { 60 } else { 130 }, 100, 120])
    }));
    let dir = std::env::temp_dir().join(format!("autoshade-semantic-product-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut regions = Vec::new();
    let mut rasters = Vec::new();
    for (index, class_id) in [10u16, 20, 30].into_iter().enumerate() {
        let mask = GrayImage::from_fn(w, h, |x, _| {
            image::Luma([if x / 4 == index as u32 { 255 } else { 0 }])
        });
        let path = crate::store::OwnedRaster::scratch(dir.join(format!("region-{class_id}.png")));
        mask.save(path.path()).unwrap();
        regions.push(semantic::SemanticRegion {
            class_id,
            label: format!("class-{class_id}"),
            mean_confidence: 0.9,
            source: mask.clone(),
            target: mask,
            source_share: 1.0 / 3.0,
            target_share: 1.0 / 3.0,
        });
        rasters.push(path);
    }
    let mut report = neutral_report(&source, &target);
    let divergences = [0.1f32, 0.8, 0.2]
        .into_iter()
        .map(|d| ZoneDivergence { divergence: divergence(d), share: 1.0 / 3.0 })
        .collect::<Vec<_>>();
    let global_confidence = report.recipe.confidence;
    attach_semantic_regions(&source, &target, &mut report, &regions, &rasters, &divergences);
    assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_MODE_FULL));
    assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_MODE_ATMOSPHERE));
    let residuals = report.notes.iter().filter(|n| n.key == crate::rationale::keys::ZONE_ATTACHED)
        .filter_map(|n| n.args.iter().find(|(name, _)| *name == "after").and_then(|(_, value)| value.parse::<f32>().ok()))
        .collect::<Vec<_>>();
    if !residuals.is_empty() {
        let worst = semantic::worst_region_residual(&residuals);
        let expected = global_confidence.min(fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE));
        assert!((report.recipe.confidence - expected).abs() <= 1e-6,
            "confidence did not use the worst accepted region: got {}, expected {}",
            report.recipe.confidence, expected);
        let disclosed = report.notes.iter()
            .find(|n| n.key == crate::rationale::keys::ZONE_CONFIDENCE)
            .and_then(|n| n.args.iter().find(|(name, _)| *name == "worst"))
            .and_then(|(_, value)| value.parse::<f32>().ok())
            .expect("semantic confidence must disclose the worst accepted region");
        assert!((disclosed - worst).abs() <= 0.001, "confidence disclosure used {disclosed}, expected worst {worst}");
    }
    for raster in rasters { raster.remove(); }
    let _ = std::fs::remove_dir_all(dir);
}

fn boundary_fixture_pixels(rim_each_side: f32) -> (Vec<[f32; 3]>, Vec<f32>, u32, u32) {
    let (w, h) = (12u32, 4u32);
    let line_weights = [1.0, 1.0, 1.0, 1.0, 0.8, 0.6, 0.4, 0.2, 0.0, 0.0, 0.0, 0.0];
    let mut pixels = Vec::with_capacity((w * h) as usize);
    let mut weights = Vec::with_capacity((w * h) as usize);
    for _ in 0..h {
        for (x, weight) in line_weights.iter().copied().enumerate() {
            let value = match x {
                0..=3 => 0.20,
                4..=5 => 0.20 + rim_each_side,
                6..=7 => 0.40 - rim_each_side,
                _ => 0.40,
            };
            pixels.push([value; 3]);
            weights.push(weight);
        }
    }
    (pixels, weights, w, h)
}

/// The reference every `boundary_fixture_pixels` reading is taken
/// against: the SAME shape with no rim at all. Its settled sky is the
/// fixture own 0.20 on every row and its band-sky cells carry that same
/// 0.20, so `M == 1.0` exactly and `transported` is the identity - which
/// is WHY the differential ruler reproduces every pre-step-9 absolute
/// reading on this fixture, rather than merely happening not to move it.
fn boundary_fixture_reference() -> Vec<[f32; 3]> {
    boundary_fixture_pixels(0.0).0
}

/// `(reference, rendered, weights, width, height)` — the two buffers a
/// differential reading needs, plus the geometry they share.
type BoundaryArm = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<f32>, u32, u32);

/// One row of the 12-wide boundary shape: four settled-sky columns, four
/// transition columns (weights .8/.6/.4/.2, so only the first two reach
/// [`ZONE_BOUNDARY_MID`] and are sampled), four settled-land columns.
fn boundary_row(settled_sky: f32, band_sky: f32, band_land: f32, land: f32) -> [f32; 12] {
    [
        settled_sky, settled_sky, settled_sky, settled_sky,
        band_sky, band_sky, band_land, band_land,
        land, land, land, land,
    ]
}

/// A step-9 boundary arm: one (reference, rendered) pair over the shared
/// 12x4 feather. Both buffers are given explicitly, because the whole
/// point of the ruler is that it reads the DIFFERENCE between them.
fn boundary_arm(reference_row: [f32; 12], rendered_row: [f32; 12]) -> BoundaryArm {
    let (w, h) = (12u32, 4u32);
    let line_weights = [1.0, 1.0, 1.0, 1.0, 0.8, 0.6, 0.4, 0.2, 0.0, 0.0, 0.0, 0.0];
    let n = (w * h) as usize;
    let (mut reference, mut rendered, mut weights) =
        (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..h {
        for x in 0..12usize {
            reference.push([reference_row[x]; 3]);
            rendered.push([rendered_row[x]; 3]);
            weights.push(line_weights[x]);
        }
    }
    (reference, rendered, weights, w, h)
}

/// What the pre-step-9 ruler read, expressed WITHOUT a second copy of the
/// old code: a reference that is flat at `settled` has an identity
/// multiplier and a band reading of zero, so the differential collapses
/// to `brightest sky-half - settled sky`, which is exactly the absolute
/// statistic this batch replaces.
fn absolute_rim(
    settled: f32,
    rendered: &[[f32; 3]],
    weights: &[f32],
    w: u32,
    h: u32,
) -> BoundaryReading {
    let flat = vec![[settled; 3]; rendered.len()];
    boundary_rim(&flat, rendered, rendered, weights, w, h)
}

fn image_of(px: &[[f32; 3]], w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let p = px[(y * w + x) as usize];
        image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
    }))
}

/// Two zone masks over `weights`, so a boundary-gate call has something
/// real to shrink. Returns the owned raster so the caller can remove it.
fn boundary_zone_masks(
    name: &str,
    weights: &[f32],
    w: u32,
    h: u32,
    sky_ev: f32,
    land_ev: f32,
) -> (crate::store::OwnedRaster, Vec<LocalAdjustment>) {
    let mask = GrayImage::from_fn(w, h, |x, y| {
        image::Luma([(weights[(y * w + x) as usize] * 255.0).round() as u8])
    });
    let path = fixture_mask_path(name);
    mask.save(path.path()).unwrap();
    let geometry =
        MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() };
    let masks = vec![
        LocalAdjustment {
            mask: geometry.clone(),
            role: MaskRole::ZoneSky,
            amount: 1.0,
            exposure_ev: sky_ev,
            ..Default::default()
        },
        LocalAdjustment {
            mask: geometry,
            role: MaskRole::ZoneLand,
            amount: 1.0,
            inverted: true,
            exposure_ev: land_ev,
            ..Default::default()
        },
    ];
    (path, masks)
}

fn soft_zone_pair(
    name: &str,
    sky_ev: f32,
    land_ev: f32,
) -> (DynamicImage, GrayImage, crate::store::OwnedRaster, fit::FitReport) {
    let (w, h) = (32u32, 12u32);
    let source = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([115; 3])));
    let mask = GrayImage::from_fn(w, h, |x, _| {
        let value = if x < 8 {
            255
        } else if x >= 24 {
            0
        } else {
            (((23 - x) as f32 / 15.0) * 255.0).round() as u8
        };
        image::Luma([value])
    });
    let path = fixture_mask_path(name);
    mask.save(path.path()).unwrap();
    let geometry = MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() };
    let mut report = neutral_report(&source, &source);
    report.recipe.masks = vec![
        LocalAdjustment {
            mask: geometry.clone(),
            role: MaskRole::ZoneSky,
            amount: 1.0,
            exposure_ev: sky_ev,
            color_gains: Some([0.94, 0.98, 1.02]),
            ..Default::default()
        },
        LocalAdjustment {
            mask: geometry,
            role: MaskRole::ZoneLand,
            amount: 1.0,
            inverted: true,
            exposure_ev: land_ev,
            color_gains: Some([1.03, 1.01, 0.98]),
            ..Default::default()
        },
    ];
    (source, mask, path, report)
}

/// Colour withheld means UNITY gains. Since step 9 the boundary gate no
/// longer waves through a correction whose rim the scene was hiding, so
/// more zones reach `shrink_zone_corrections` - and that function writes
/// each gain through its deliberately explicit common/differential
/// decomposition. Until 2026-09-24 that read `(1.0 + k*c) + k*(v - c)`,
/// which for a withheld channel cancelled to unity only up to float
/// rounding (0.99999994 measured); the differential is now summed before
/// the unity offset and cancels exactly (pinned at the function). The
/// tolerance below stays because the property these tests assert is "no
/// colour move", not "these bits".
fn gains_withheld(gains: Option<[f32; 3]>) -> bool {
    gains.is_some_and(|g| g.iter().all(|v| (v - 1.0).abs() <= 1e-6))
}

fn assert_gains_withheld(gains: Option<[f32; 3]>) {
    assert!(gains_withheld(gains), "colour was not withheld: {gains:?}");
}

/// The shrink's explicit common/differential decomposition must not
/// manufacture a colour cast for a zone whose colour was withheld: with a
/// second zone carrying a real cast (so the common term is not zero), the
/// withheld zone's gains are EXACTLY unity at every `k` and every share
/// split, its additive dials stay at zero, and the cast zone keeps its
/// direction. CI on the 2026-09-24 close-out read 0.99999994 here while
/// the unity offset was added first.
#[test]
fn a_withheld_channel_is_exactly_unity_under_every_shrink() {
    let originals = vec![
        LocalAdjustment {
            role: MaskRole::ZoneSky,
            amount: 1.0,
            exposure_ev: 0.8,
            color_gains: Some([1.0; 3]),
            ..Default::default()
        },
        LocalAdjustment {
            role: MaskRole::ZoneLand,
            amount: 1.0,
            inverted: true,
            exposure_ev: -0.3,
            saturation: 12.0,
            color_gains: Some([1.42, 0.97, 0.71]),
            ..Default::default()
        },
    ];
    for share_sky in [0.25f32, 0.5, 0.75] {
        let shares = [share_sky, 1.0 - share_sky];
        for k in [0.05f32, 0.3, 0.5, 0.7, 0.999, 1.0] {
            let mut masks = originals.clone();
            shrink_zone_corrections(&mut masks, &originals, &shares, k);
            assert_eq!(
                masks[0].color_gains,
                Some([1.0; 3]),
                "the withheld sky at k={k}, sky share {share_sky}"
            );
            assert_eq!(masks[0].saturation, 0.0, "k={k}, sky share {share_sky}");
            assert!(masks[0].exposure_ev > 0.0, "the sky keeps its direction at k={k}");
            let land = masks[1].color_gains.expect("the cast zone keeps its gains");
            assert!(
                land[0] > 1.0 && land[1] < 1.0 && land[2] < 1.0,
                "the cast keeps its direction at k={k}: {land:?}"
            );
        }
        let mut masks = originals.clone();
        shrink_zone_corrections(&mut masks, &originals, &shares, 0.0);
        assert!(
            masks.iter().all(|m| m.color_gains.is_none() && m.exposure_ev == 0.0),
            "k=0 is no local correction: {masks:?}"
        );
    }
}

/// R34 §D2. The RECIPE and the SENTENCE must say the same thing about one
/// zone's colour: exactly one verdict is disclosed for it, unity gains go
/// with the refusal and a moved gain goes with the vouch. R33's pins
/// asserted only the gains, which could not tell "refused" from "nobody
/// looked" — and could not survive a design where both outcomes exist.
fn assert_colour_verdict_matches_recipe(
    report: &FitReport,
    label: &str,
    gains: Option<[f32; 3]>,
) {
    let says = |key_matches: &dyn Fn(&str) -> bool| {
        report.notes.iter().any(|note| {
            key_matches(note.key)
                && note.args.iter().any(|(name, value)| *name == "label" && value == label)
        })
    };
    let vouched = says(&|key| {
        key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_CELLS
            || key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_PIXELS
    });
    let refused = says(&|key| is_colour_refusal(key));
    assert!(
        vouched != refused,
        "exactly one colour verdict must be disclosed for {label}: {}",
        report.recipe.rationale
    );
    if refused {
        assert_gains_withheld(gains);
    } else {
        assert!(
            !gains_withheld(gains),
            "a colour the cells vouched must actually ship: {gains:?}"
        );
    }
}

fn note_number(note: &crate::rationale::Note, name: &str) -> f32 {
    note.args
        .iter()
        .find(|(key, _)| *key == name)
        .unwrap_or_else(|| panic!("missing {name} in {:?}", note.args))
        .1
        .parse()
        .unwrap()
}

#[test]
fn boundary_rim_is_measured_across_the_mask_transition_band() {
    let (pixels, weights, w, h) = boundary_fixture_pixels(0.12);
    let reference = boundary_fixture_reference();
    let reading = boundary_rim(&reference, &pixels, &pixels, &weights, w, h);
    assert_eq!(reading.transitions, h as usize, "one feather crossing per row");
    assert!(
        (reading.rim - 0.12).abs() <= 1e-6,
        "the brightest introduced sky-half deviation must be measured against the \
             uncorrected render: {reading:?}"
    );
}

#[test]
fn opposite_sign_zone_pair_exceeds_the_rim_budget_before_shrinking() {
    assert_eq!(ZONE_BOUNDARY_RIM_MAX, 0.012, "the measured calibration is pinned");
    let (pixels, weights, w, h) = boundary_fixture_pixels(0.013);
    let reading = boundary_rim(&boundary_fixture_reference(), &pixels, &pixels, &weights, w, h);
    assert!(
        reading.rim > ZONE_BOUNDARY_RIM_MAX,
        "the just-over-budget opposite-sign shape must exercise the gate: {reading:?}"
    );
}

#[test]
fn rim_shrink_keeps_each_zones_direction_and_lands_inside_the_budget() {
    let (source, mask, path, mut report) = soft_zone_pair("zoned-rim-shrink", -0.65, 0.20);
    let weights = mask_weights(&mask, source.width(), source.height());
    // The uncorrected render: a literally uniform grey, so its band reads
    // exactly its own settled sky and the differential reduces to the
    // pre-step-9 absolute reading. This fixture therefore keeps its old
    // verdict by arithmetic, not by luck.
    let reference = fit::pixels_of(&render::develop_preview(
        &source,
        &crate::recipe::EditRecipe::default(),
    ));
    let initial = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[0.5, 0.5],
        0,
        &reference,
        initial,
    );
    let BoundaryGateResult::Kept { k, before, after, .. } = verdict else {
        panic!("a shrinkable pair was dropped: {}", report.recipe.rationale);
    };
    assert!(before.rim > ZONE_BOUNDARY_RIM_MAX, "premise: {before:?}");
    assert!((0.0..1.0).contains(&k), "the largest passing k must really shrink: {k}");
    assert!(after.rim <= ZONE_BOUNDARY_RIM_MAX, "shrunk rim: {after:?}");
    let sky = &report.recipe.masks[0];
    let land = &report.recipe.masks[1];
    assert!(sky.exposure_ev < 0.0, "a darkening sky reversed: {}", sky.exposure_ev);
    assert!(land.exposure_ev > 0.0, "a brightening land reversed: {}", land.exposure_ev);
    assert!((sky.exposure_ev / -0.65 - k).abs() < 1e-5);
    assert!((land.exposure_ev / 0.20 - k).abs() < 1e-5);
    for (gain, original) in sky.color_gains.unwrap().into_iter().zip([0.94, 0.98, 1.02]) {
        assert_eq!((gain - 1.0).signum(), (original - 1.0f32).signum());
        assert!(((gain - 1.0) / (original - 1.0) - k).abs() < 1e-5);
    }
    path.remove();
}

/// RE-PINNED by step 9, user ruling 4 (2026-08-31), and the reason it
/// moved matters more than the number. The p90 rank is now a MAGNITUDE,
/// matching the doctrine [`boundary_step`] already states: a correction
/// that darkens its side of a border is as visible a seam as one that
/// brightens it, and a signed percentile lets a dark edge hide behind a
/// bright one. So this fixture reads +0.0200 where it used to read
/// -0.020. The SHAPE did not move - `|-0.020| == 0.0200` - the RANK did,
/// and this is not a regression.
///
/// What the test pins today is therefore stronger than what it pinned
/// before: the same-sign bow is SCENE CONTENT, present in the
/// uncorrected render too, so the differential ruler charges nothing for
/// it and the gate keeps the candidate at exactly k=1 - even though the
/// magnitude of that bow is nearly twice the budget.
#[test]
fn same_sign_zone_pair_needs_no_shrink() {
    let (pixels, weights, w, h) = boundary_fixture_pixels(-0.02);
    let against_flat = boundary_rim(&boundary_fixture_reference(), &pixels, &pixels, &weights, w, h);
    assert!(
        (against_flat.rim - 0.0200).abs() <= 1e-6,
        "a dark bow is now ranked by magnitude: {against_flat:?}"
    );
    assert!(
        against_flat.rim > ZONE_BOUNDARY_RIM_MAX,
        "and it no longer hides under a signed percentile: {against_flat:?}"
    );

    let source = image_of(&pixels, w, h);
    let reference = fit::pixels_of(&render::develop_preview(
        &source,
        &crate::recipe::EditRecipe::default(),
    ));
    let mut report = neutral_report(&source, &source);
    report.recipe.masks = vec![
        LocalAdjustment { role: MaskRole::ZoneSky, exposure_ev: -0.35, ..Default::default() },
        LocalAdjustment { role: MaskRole::ZoneLand, exposure_ev: -0.90, ..Default::default() },
    ];
    let expected = boundary_rim(&reference, &pixels, &pixels, &weights, w, h);
    assert!(
        expected.rim <= ZONE_BOUNDARY_RIM_MAX,
        "the scene bow is in the reference too, so nothing is charged: {expected:?}"
    );
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[0.5, 0.5],
        0,
        &reference,
        pixels,
    );
    let BoundaryGateResult::Kept { k, after, .. } = verdict else {
        panic!("same-sign pair was dropped");
    };
    assert_eq!(k, 1.0);
    assert_eq!(after.rim, expected.rim);
    assert_eq!(report.recipe.masks[0].exposure_ev, -0.35);
    assert_eq!(report.recipe.masks[1].exposure_ev, -0.90);
}

#[test]
fn every_accepted_fixture_zone_still_passes_the_boundary_gate() {
    let (src, tgt, sky_mask) = zoned_pair();
    let path = fixture_mask_path("zoned-boundary-calibration-sky");
    sky_mask.save(path.path()).unwrap();
    let mut sky_report = fit::fit_recipe(&src, &tgt);
    attach_zones(&src, &tgt, &mut sky_report, &sky_mask, &sky_mask, &path);

    let (w, h) = (16u32, 16u32);
    let build = |sky: [f32; 3], rock: [f32; 3]| -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let p = if y >= 12 {
                sky
            } else if x < 6 {
                [0.63, 0.63, 0.63]
            } else {
                rock
            };
            image::Rgb(p.map(|c| (c * 255.0).round() as u8))
        }))
    };
    let land_src = build([0.60, 0.63, 0.67], [0.45, 0.42, 0.40]);
    // The land pair of `zoned_orchestration_corrects_the_land_through_the_inverted_raster`,
    // grey patch included (2026-10-22).
    let land_tgt = build([0.92, 0.72, 0.48], [0.58, 0.46, 0.37]);
    let land_path = fixture_mask_path("zoned-boundary-calibration-land");
    sky_mask.save(land_path.path()).unwrap();
    let mut land_report = fit::fit_recipe(&land_src, &land_tgt);
    attach_zones(
        &land_src,
        &land_tgt,
        &mut land_report,
        &sky_mask,
        &sky_mask,
        &land_path,
    );

    let mut measured = Vec::new();
    let mut candidates = Vec::new();
    let mut charges = Vec::new();
    for (fixture, report) in [("sky", &sky_report), ("sky+land", &land_report)] {
        let note = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::ZONE_BOUNDARY_PASSED)
            .unwrap_or_else(|| panic!("{fixture} lacked a boundary verdict: {}", report.recipe.rationale));
        let rim = note_number(note, "after");
        eprintln!(
            "BOUNDARY_CALIBRATION {fixture}: before={:.4} after={:.4} charged={:.4} \
                 colour={:.4}/{:.4} k={:.3} n={}",
            note_number(note, "before"),
            rim,
            note_number(note, "charged"),
            note_number(note, "colour"),
            note_number(note, "colour_charged"),
            note_number(note, "k"),
            note_number(note, "transitions"),
        );
        charges.push((
            fixture,
            rim,
            note_number(note, "charged"),
            note_number(note, "colour"),
            note_number(note, "colour_charged"),
        ));
        assert!(rim <= ZONE_BOUNDARY_RIM_MAX, "{fixture} rim {rim:.3}");
        candidates.push((fixture, note_number(note, "before"), note_number(note, "k")));
        for mask in &report.recipe.masks {
            measured.push((fixture, mask.role, rim));
        }
    }
    // RE-PINNED 2026-11-03 (v1.17.1): 4 -> 2. The finished-frame curves now
    // correct the land the inverted zone used to correct; on the `sky+land`
    // toy the zones that remain are refused after this gate by the frame's
    // structural term, which on this 16-pixel toy swings 0.140 -> 0.354 (the
    // swing documented below), so that fixture ships none. Its boundary
    // verdict is still read above; the `sky` fixture still reaches the gate
    // with both zone classes.
    // RE-PINNED (v1.18.0, J*): 2 -> 3. The swing that refused the `sky+land`
    // zones was the display objective's S reading three flat blocks a few
    // codes apart as whole rank steps; J*'s S charges a tie within one output
    // code by the fraction of a code that separates it (780f90d), and the
    // zones' do-no-harm reads the per-pixel Lab terms, so that fixture's sky
    // zone reaches the gate and ships again (measured: `sky` ZoneSky and
    // ZoneLand at rim 0.006, `sky+land` ZoneSky at 0.007).
    // RE-PINNED for v1.18.0 step 1: 3 -> 4. The `sky+land` fixture's land
    // zone now reaches the gate beside its sky: `sky` ZoneSky and ZoneLand
    // at rim 0.004, `sky+land` ZoneSky and ZoneLand at 0.007. (Reaching the
    // gate is not shipping: with DEF-3 reverted this entry is here too while
    // the orchestration toy ships its sky alone.) No single step-1 fix
    // reverted alone (DEF-1, DEF-3, DEF-4, the judge's presence, the cell
    // votes) brings the count back to 3; the combination was not separated.
    assert_eq!(measured.len(), 4, "every accepted zone must reach the boundary gate: {measured:?}");
    assert!(
        measured.iter().any(|(_, role, _)| *role == MaskRole::ZoneSky)
            && measured.iter().any(|(_, role, _)| *role == MaskRole::ZoneLand),
        "both supported zone classes may reach the boundary gate: {measured:?}"
    );
    // RE-DERIVED by step 9 against the differential ruler. Before it,
    // all four entries read -0.004 and passed at k=1 without a shrink;
    // the doc block above `ZONE_BOUNDARY_RIM_MAX` claimed -0.007 for the
    // same four, and measurement says the TEST was the accurate one
    // (-0.0044) and the doc block stale. Both are now superseded.
    //
    // The whole move is the REFERENCE, not the magnitude rank: measured
    // first-party on this fixture, the pre-step-9 statistic's signed 90th
    // percentile is -0.0044 and its MAGNITUDE 90th percentile is 0.0044,
    // while the introduced-rim reading is 0.0461. The sky zone was
    // shipping a 0.046 luma rim it had itself introduced, invisible to
    // the absolute ruler because the scene's own bow under the feather
    // cancelled it — ARM B's false negative, on a repository fixture.
    // Both fixtures now shrink onto the ceiling instead of passing free.
    //
    // RE-DERIVED AGAIN (2026-09-10) for the unified per-crossing budget
    // and the colour coordinate that landed with it. Measured, and the
    // two halves of that change had very different effects here:
    //
    //   * the per-crossing CHARGE moved nothing. Both fixtures put a
    //     real scene step under the contour — the photograph its own
    //     texture, the synthetic pair its rock/sky edge at y=12 — so on
    //     both the budget saturates and `charged` equals the raw rim.
    //     `sky` is therefore pinned at exactly the numbers the scalar
    //     rule produced: 0.0460 before, 0.012 after, k=0.244.
    //   * the COLOUR coordinate moved `sky+land`, and it is the halo the
    //     coordinator's brief describes, on a repository fixture. That
    //     pair asks for wide per-channel gains ([0.60, 0.63, 0.67] ->
    //     [0.92, 0.72, 0.48]), and a luma-only ruler read the rim those
    //     gains paint as 0.019/0.012 while the channels were moving far
    //     more. Ranking `max(charged luma, charged colour)` the shrink
    //     now runs until the COLOUR p90 reaches the ceiling: k falls
    //     0.852 -> 0.088 and the kept luma rim 0.012 -> 0.002, with the
    //     colour rim sitting on 0.012.
    //
    // RE-DERIVED (2026-10-21) for the redrawn fixtures: `sky` is now the
    // grey-sky pair whose gold the sky zone's own pixels vouch (rocks a
    // touch warmer in the target), `sky+land` the half-vivid land pair.
    // Measured first-party: `sky` 0.0480 before, 0.0060 kept, k 0.123;
    // `sky+land` 0.0140 before, 0.0050 kept, k 0.248 — and on BOTH the
    // colour rank sits on the 0.012 ceiling while the kept luma rim is
    // under it, so both shrinks now stop where the colour coordinate says.
    //
    // RE-DERIVED (2026-10-22) for the grey land patches both pairs carry
    // (see `zoned_pair`): without them the global stage absorbed the sky
    // gold and each pair attached three masks. Measured: `sky` 0.031
    // before, 0.005 kept, k 0.137; `sky+land` 0.053 before, 0.004 kept,
    // k 0.103; colour on the 0.012 ceiling on both, as before.
    //
    // RE-DERIVED (2026-10-22, later) for the boundary ruler's colour
    // one-code dead band (R1) and unfooted bands leaving the soft ruler (R2).
    // `sky` now shrinks less: 0.031 before, 0.006 kept, k 0.137 -> 0.194.
    // `sky+land` went k 0.103 -> 0.165 on its old land target, and at that
    // strength the shrunk-zone do-no-harm refused BOTH zones on the frame,
    // 0.05845 -> 0.12834 (it read 0.03925 at k 0.103). That jump is this
    // toy's, not the rule's: the frame's structural term
    // (`structure_divergence`, weight 0.10) reads three flat blocks whose
    // rendered lumas sit within a few codes of each other (sky 0.710 /
    // grey 0.701 at k 0.165), and it swings with single-code changes —
    // D 0.093 at k 0.103, 0.910 at 0.130, 1.506 at 0.165, 0.102 at 0.200 —
    // while the tonal term moves smoothly (0.0357 -> 0.0342) and both zone
    // residuals improve (sky 0.235 -> 0.202, land 0.312 -> 0.256). The land
    // target is milder now ([0.62, 0.46, 0.34] -> [0.58, 0.46, 0.37], the
    // pair the orchestration test pins): measured, 0.054 before, 0.008
    // kept, k 0.165, and the frame there reads 0.04672 -> 0.05909, inside
    // the 0.02 drift. The structural term still swings on this toy (the
    // same render reads 0.06889 at k 0.103), so these pins hold the gate's
    // own k, not a smooth frame curve. Colour on the 0.012 ceiling on both.
    // RE-PINNED 2026-11-03 (v1.17.1): `sky` 0.006 -> 0.003 — the sky zone
    // has less to paint once the curves carry the shared colour.
    // RE-PINNED (v1.18.0, J*): the global recipe under these zones and the
    // zones' own verdicts are read on J*, so the candidates the gate shrinks
    // moved: kept `sky` 0.003 -> 0.006 on both of its zones, and the third
    // entry, `sky+land`'s sky zone, 0.007.
    // RE-PINNED for v1.18.0 step 1: `sky` 0.006 -> 0.004 on both zones,
    // `sky+land` 0.007 on both (its land zone the fourth entry).
    let expected = [0.004f32, 0.004, 0.007, 0.007];
    for ((fixture, role, rim), expected) in measured.iter().zip(expected) {
        assert!(
            (*rim - expected).abs() <= 0.002,
            "{fixture}/{role:?} boundary calibration drifted: {rim:.3} vs {expected:.3}"
        );
    }
    // RE-DERIVED (2026-10-19) for the exposure-invariant texture gate. The
    // `sky` zone's fitted tone used to fail the absolute-gradient ratio at
    // full strength, so the tone ladder backed it off before this gate saw
    // it; per unit of brightness the full tone passes, the candidate carries
    // the whole fitted move (rim 0.0460 -> 0.0660) and the shrink lands it on
    // the SAME kept rim, 0.012, at k 0.244 -> 0.183. `sky+land` is unmoved.
    // 2026-11-02: the white balance takes only the brightness-common colour
    // change and the colour-grade stage carries the per-range, per-hue
    // remainder, so the land pair's global recipe under its zones differs and
    // its candidate reads 0.0570 at k 0.164.
    // 2026-11-03 (v1.17.1): `sky` 0.031 -> 0.003 at k 0.212 — the sky zone
    // has less to paint once the curves carry the shared colour; `sky+land`
    // 0.057 -> 0.016 at k 0.316 — the finished-frame curves now correct the
    // land the inverted zone used to correct, so the candidate pair moves less.
    // v1.18.0 (J*, same mechanism as above): `sky` 0.003 -> 0.039 at k
    // 0.212 -> 0.165; `sky+land` 0.016 -> 0.047 at k 0.316 -> 0.166.
    // v1.18.0 step 1: `sky` 0.039 -> 0.030 at k 0.165 -> 0.152; `sky+land`
    // 0.047 -> 0.061 at k 0.166 -> 0.153 (its candidate now carries the land
    // zone too). `sky` reads 0.030 / k 0.152 with each step-1 fix reverted
    // alone; `sky+land` reads 0.025 / k 0.268 with DEF-3 reverted and 0.055 /
    // k 0.149 with the judge's presence reverted.
    let expected_candidates = [("sky", 0.0300f32, 0.152f32), ("sky+land", 0.0610, 0.153)];
    for ((fixture, before, k), (want_fixture, want_before, want_k)) in
        candidates.iter().zip(expected_candidates)
    {
        assert_eq!(*fixture, want_fixture);
        assert!(
            (*before - want_before).abs() <= 0.002,
            "{fixture} candidate rim drifted: {before:.4} vs {want_before:.4}"
        );
        assert!(
            (*k - want_k).abs() <= 0.02,
            "{fixture} shrink drifted: k={k:.3} vs {want_k:.3}"
        );
    }
    assert!(
        measured.iter().all(|(_, _, rim)| *rim <= ZONE_BOUNDARY_RIM_MAX),
        "accepted fixture calibration: {measured:?}"
    );
    // The attribution itself, asserted rather than left to the numbers
    // above, because a re-pin that cannot say WHICH ruler moved is not a
    // re-pin. Both fixtures saturate the luma budget, so the charge is
    // inert on both...
    // 2026-11-03 (v1.17.1): read on `sky+land` only. On `sky` the kept rim
    // fell to 0.003 under a charge of 0.008 — the sky zone has less to paint
    // once the curves carry the shared colour, so its own rim no longer
    // reaches the scene step the charge reads.
    for (fixture, rim, charged, _, _) in charges.iter().filter(|c| c.0 == "sky+land") {
        assert!(
            (charged - rim).abs() <= 0.001,
            "{fixture}: a scene step under the contour saturates the luma budget, so the \
                 per-crossing charge may not move this verdict: {charges:?}"
        );
    }
    // ...and on both the colour coordinate is what stopped the shrink,
    // exactly on the ceiling, with the luma rim still under it.
    for (fixture, rim, _, _, colour) in &charges {
        assert!(
            colour > rim,
            "{fixture}: the channels moved more than luma did: {charges:?}"
        );
        assert!(
            (colour - ZONE_BOUNDARY_RIM_MAX).abs() <= 0.001,
            "{fixture}: the shrink stopped where the COLOUR rank reached the ceiling: {charges:?}"
        );
    }
    path.remove();
    land_path.remove();
}

/// Step 9: the `Dropped` verdict is now an INVARIANT rather than a
/// policy branch, and this test is what keeps it from being dead code.
///
/// The old body asked a +0.03 scene rim already present in the source to
/// be dropped, which is precisely the false positive this batch removes:
/// under the differential ruler that rim is in the reference too, costs
/// nothing, and the pair is kept. What CAN still reach the branch is an
/// engine bug - a zero-dialled attached mask that is not a render no-op -
/// because `shrink_zone_corrections` at k=0 zeroes every additive dial
/// and drops every gain, so the k=0 render is REQUIRED to reproduce the
/// reference. Handing the gate a reference the k=0 render cannot
/// reproduce is how that bug is simulated here.
#[test]
fn a_zero_dialled_mask_that_is_not_a_render_no_op_is_dropped_with_its_own_note() {
    let (pixels, line_weights, w, h) = boundary_fixture_pixels(0.0);
    let source = image_of(&pixels, w, h);
    let (path, masks) =
        boundary_zone_masks("zoned-rim-unshrinkable", &line_weights, w, h, -0.2, 0.1);
    let mut report = neutral_report(&source, &source);
    report.recipe.masks = masks;
    // A reference whose band sits 0.05 luma BELOW what any k reproduces.
    // Its settled sky is untouched, so the transport multiplier is
    // exactly 1.0 and the gap cannot be explained away as treatment.
    let mut reference = fit::pixels_of(&render::develop_preview(
        &source,
        &crate::recipe::EditRecipe::default(),
    ));
    for (i, weight) in line_weights.iter().enumerate() {
        if (ZONE_BOUNDARY_LOW..ZONE_BOUNDARY_HIGH).contains(weight) {
            reference[i] = reference[i].map(|c| c - 0.05);
        }
    }
    let initial = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
    let premise = boundary_rim(&reference, &initial, &initial, &line_weights, w, h);
    assert!(premise.rim > ZONE_BOUNDARY_RIM_MAX, "premise: {premise:?}");
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &line_weights,
        &[0.5, 0.5],
        0,
        &reference,
        initial,
    );
    assert!(matches!(verdict, BoundaryGateResult::Dropped));
    assert!(report.recipe.masks.is_empty(), "the failed pair must be removed");
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_BOUNDARY_DROPPED)
        .expect("the boundary failure needs its own typed note");
    assert_eq!(note_number(note, "k"), 0.0);
    assert!(note_number(note, "after") > ZONE_BOUNDARY_RIM_MAX);
    assert!(report.recipe.rationale.contains("even shared shrink k=0"));
    path.remove();
}

#[test]
fn boundary_gate_discloses_the_applied_shrink_and_measured_rim() {
    let (source, mask, path, mut report) = soft_zone_pair("zoned-rim-disclosure", -0.65, 0.20);
    let weights = mask_weights(&mask, source.width(), source.height());
    let reference = fit::pixels_of(&render::develop_preview(
        &source,
        &crate::recipe::EditRecipe::default(),
    ));
    let initial = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[0.5, 0.5],
        0,
        &reference,
        initial,
    );
    let BoundaryGateResult::Kept { k, before, after, .. } = verdict else {
        panic!("disclosure fixture dropped");
    };
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_BOUNDARY_PASSED)
        .expect("typed boundary pass note");
    assert!((note_number(note, "k") - k).abs() <= 0.0005);
    assert!((note_number(note, "before") - before.rim).abs() <= 0.0005);
    assert!((note_number(note, "after") - after.rim).abs() <= 0.0005);
    assert!(report.recipe.rationale.contains("introduced transition rim"));
    assert!(report.recipe.rationale.contains("shared differential shrink k="));
    path.remove();
}

/// REWRITTEN (v1.18.0, R1): one law for every gate family. Until v1.18.0 a
/// semantic region was admitted under the zones' 0.02 frame drift and the
/// shared bitmap gate re-checked the composed frame under the tolerance its
/// CALLER passed (`frame_tol`), so a region that regressed the frame by 0.01
/// was kept. R1 removed `frame_tol` with every per-family allowance: the
/// composed frame ships only on a J* gain of FIT_QUANT over the frame without
/// the region (`fit::ships`), and a loss of any size refuses. The same region
/// is run twice through `attach_semantic_regions`: once from the honest
/// region-free frame, where it passes, to read the frame it lands on (F);
/// then with the running frame set to F - 0.01, i.e. the region now
/// regresses the frame by 0.01. It must be refused, and say why.
///
/// MUTATION THIS KILLS: a frame allowance on the semantic-region caller in
/// `regions.rs` (the second run then keeps the region).
#[test]
fn a_region_that_costs_the_frame_is_refused_by_the_boundary_gate() {
    let (src, tgt, mask) = zoned_pair();
    let run = |frame_before: Option<f32>, name: &str| {
        let dir = std::env::temp_dir().join(format!("autoshade-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raster = crate::store::OwnedRaster::scratch(dir.join("region-10.png"));
        mask.save(raster.path()).unwrap();
        let region = semantic::SemanticRegion {
            class_id: 10,
            label: "sky".to_string(),
            mean_confidence: 0.9,
            source: mask.clone(),
            target: mask.clone(),
            source_share: 0.25,
            target_share: 0.25,
        };
        let mut report = neutral_report(&src, &tgt);
        let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
        report.err_after = frame_before.unwrap_or_else(|| {
            fit::look_err_with_evidence(
                &fit::pixels_of(&s_img),
                &fit::pixels_of(&t_img),
                &report.evidence,
            )
        });
        let divergences = [ZoneDivergence { divergence: divergence(0.05), share: 0.25 }];
        attach_semantic_regions(&src, &tgt, &mut report, &[region], &[raster], &divergences);
        let _ = std::fs::remove_dir_all(&dir);
        report
    };
    let first = run(None, "region-frame-tol-a");
    let has = |report: &fit::FitReport, key: &str| report.notes.iter().any(|n| n.key == key);
    assert!(
        has(&first, crate::rationale::keys::ZONE_BOUNDARY_PASSED),
        "premise: the region passes the gate: {}",
        first.recipe.rationale
    );
    let landed = first.err_after;
    let second = run(Some(landed - 0.01), "region-frame-tol-b");
    assert!(
        has(&second, crate::rationale::keys::REGION_BOUNDARY_REFUSED),
        "a region that regresses the frame by 0.01 was kept (landed {landed:.5}): {}",
        second.recipe.rationale
    );
    assert!(
        second.recipe.rationale.contains("the composed frame regressed"),
        "the refusal names the frame: {}",
        second.recipe.rationale
    );
    assert!(second.recipe.masks.len() < first.recipe.masks.len(), "the refused region leaves no mask");
}

/// Step 9 / R1 (acceptance A4, ARM A): the gate charges the CORRECTION,
/// not the scene. A +0.060 bow the scene already has under the feather
/// used to read +0.064 absolute; even the k=0 render kept +0.060 of it,
/// so the whole zone pair was dropped for a rim no zone dial made. That
/// is the island failure verbatim: "candidate rim 0.060 luma, and even
/// shared shrink k=0 left 0.058 (budget 0.012, 817 measured
/// transitions)". Supervisor mutation M-2-A (the absolute reading
/// restored) goes red on the second assertion.
#[test]
fn the_boundary_gate_charges_the_correction_and_not_the_scene() {
    let (reference, rendered, weights, w, h) = boundary_arm(
        boundary_row(0.20, 0.26, 0.40, 0.40),
        boundary_row(0.20, 0.264, 0.40, 0.40),
    );
    let absolute = absolute_rim(0.20, &rendered, &weights, w, h);
    assert!(
        (absolute.rim - 0.064).abs() <= 1e-6 && absolute.rim > ZONE_BOUNDARY_RIM_MAX,
        "premise: the scene's own bow alone busts the budget: {absolute:?}"
    );
    let introduced = boundary_rim(&reference, &rendered, &rendered, &weights, w, h);
    assert_eq!(introduced.transitions, h as usize, "one crossing per row");
    assert!(
        (introduced.rim - 0.004).abs() <= 1e-6,
        "only what the correction added may be charged: {introduced:?}"
    );

    let source = image_of(&reference, w, h);
    let (path, masks) = boundary_zone_masks("s9-arm-a", &weights, w, h, -0.2, 0.1);
    let mut report = neutral_report(&source, &source);
    report.recipe.masks = masks;
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[0.5, 0.5],
        0,
        &reference,
        rendered,
    );
    let BoundaryGateResult::Kept { k, after, .. } = verdict else {
        panic!("a correction that introduced 0.004 was dropped: {}", report.recipe.rationale);
    };
    assert_eq!(k, 1.0, "nothing to shrink");
    assert!((after.rim - 0.004).abs() <= 1e-6, "{after:?}");
    path.remove();
}

/// Step 9 / R2 (ARM B): the mirror image - a rim the correction really
/// introduced is charged even when the scene HIDES it. A -0.040 scene
/// notch plus a +0.030 introduced rim reads -0.010 absolute, inside the
/// budget, so the old fast path kept it at k=1 and shipped a seam 2.5x
/// the visibility line. Without this arm the whole change would be
/// indistinguishable from raising the budget.
#[test]
fn a_rim_the_correction_introduced_is_charged_even_where_the_scene_hides_it() {
    let (reference, rendered, weights, w, h) = boundary_arm(
        boundary_row(0.20, 0.16, 0.40, 0.40),
        boundary_row(0.20, 0.19, 0.40, 0.40),
    );
    let absolute = absolute_rim(0.20, &rendered, &weights, w, h);
    assert!(
        (absolute.rim - 0.010).abs() <= 1e-6 && absolute.rim <= ZONE_BOUNDARY_RIM_MAX,
        "premise: the absolute reading passes this seam: {absolute:?}"
    );
    let introduced = boundary_rim(&reference, &rendered, &rendered, &weights, w, h);
    assert!(
        (introduced.rim - 0.030).abs() <= 1e-6 && introduced.rim > ZONE_BOUNDARY_RIM_MAX,
        "the introduced rim must be charged: {introduced:?}"
    );

    let source = image_of(&reference, w, h);
    let (path, masks) = boundary_zone_masks("s9-arm-b", &weights, w, h, 0.30, -0.10);
    let mut report = neutral_report(&source, &source);
    report.recipe.masks = masks;
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[0.5, 0.5],
        0,
        &reference,
        rendered,
    );
    let BoundaryGateResult::Kept { k, after, .. } = verdict else {
        panic!("the k=0 render is a no-op here, so the gate may not drop: {}", report.recipe.rationale);
    };
    assert_ne!(k, 1.0, "the fast path shipped an introduced rim of 0.030");
    assert!(after.rim <= ZONE_BOUNDARY_RIM_MAX, "and the shrink must land it: {after:?}");
    path.remove();
}

/// Step 9 / R3 (ARM C): ONE multiply applied identically to the WHOLE
/// frame introduces no seam, so the reading must be zero. It is the arm
/// that separates the shipped ruler from a plain difference of
/// differences, and the reason the difference is not plain: every zone
/// dial is multiplicative in LINEAR light, so a band pixel carrying a
/// content rim above the settled sky moves MORE in absolute luma under
/// the same multiplier. Transporting the reference through the settled
/// sky's own multiplier cancels that identically. Supervisor mutation
/// M-2-B (the additive form) goes red here and nowhere else.
#[test]
fn one_multiply_over_the_whole_frame_introduces_no_rim() {
    let gain = 2.0f32;
    let lift = |v: f32| render::linear_to_srgb(gain * render::srgb_to_linear(v));
    let reference_row = boundary_row(0.180, 0.240, 0.240, 0.400);
    let mut rendered_row = reference_row;
    for value in &mut rendered_row {
        *value = lift(*value);
    }
    let (reference, rendered, weights, w, h) = boundary_arm(reference_row, rendered_row);

    let absolute = absolute_rim(lift(0.180), &rendered, &weights, w, h);
    assert!(
        absolute.rim > 6.0 * ZONE_BOUNDARY_RIM_MAX,
        "premise: the absolute ruler reads this seamless frame as a large rim: {absolute:?}"
    );
    let additive =
        (rendered_row[4] - rendered_row[0]) - (reference_row[4] - reference_row[0]);
    assert!(
        additive > ZONE_BOUNDARY_RIM_MAX,
        "premise: even a plain difference of differences busts the budget: {additive}"
    );
    let introduced = boundary_rim(&reference, &rendered, &rendered, &weights, w, h);
    assert!(
        introduced.rim <= 1e-4,
        "a uniform multiply is not a seam: {introduced:?} (additive would read {additive})"
    );

    let source = image_of(&reference, w, h);
    let (path, masks) = boundary_zone_masks("s9-arm-c", &weights, w, h, -0.2, 0.1);
    let mut report = neutral_report(&source, &source);
    report.recipe.masks = masks;
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[0.5, 0.5],
        0,
        &reference,
        rendered,
    );
    let BoundaryGateResult::Kept { k, .. } = verdict else {
        panic!("a seamless frame was dropped: {}", report.recipe.rationale);
    };
    assert_eq!(k, 1.0);
    path.remove();
}

/// Step 9 / R4 (ARM D): `rendered == reference` reads EXACTLY 0.0. Not a
/// tolerance - exact equality is what the `M == 1.0` short circuit in
/// `boundary_line_rims` buys, and it is what makes the retained `Dropped`
/// branch an invariant instead of a float coin flip: without it the sRGB
/// round trip leaves up to 8.9e-08. Supervisor mutation M-2-D (the short
/// circuit removed) goes red here and a tolerance-based assertion would
/// not catch it.
#[test]
fn an_unchanged_render_introduces_exactly_zero_rim() {
    let row = boundary_row(0.20, 0.26, 0.40, 0.40);
    let (reference, rendered, weights, w, h) = boundary_arm(row, row);
    let reading = boundary_rim(&reference, &rendered, &rendered, &weights, w, h);
    assert_eq!(reading.transitions, h as usize, "and it is measured, not skipped");
    assert_eq!(reading.rim, 0.0, "an unchanged render has no introduced rim: {reading:?}");
}

/// Step 9 / R5 (ARM E): ONE argmax on the DIFFERENCE, never two
/// independent maxima. The band carries two humps; the correction leaves
/// the taller one alone and lifts the shorter one by +0.050. Two
/// independent maxima would compare the tallest rendered pixel with the
/// tallest reference pixel - the same pixel, unchanged - and report 0.000
/// while a 0.050 seam ships. Supervisor mutation M-2-C goes red here;
/// this arm exists BECAUSE the specification predicted M-2-C would
/// survive on a single-hump fixture.
#[test]
fn the_rim_is_one_argmax_on_the_difference_not_two_independent_maxima() {
    let (reference, rendered, weights, w, h) = boundary_arm(
        [0.20, 0.20, 0.20, 0.20, 0.28, 0.22, 0.40, 0.40, 0.40, 0.40, 0.40, 0.40],
        [0.20, 0.20, 0.20, 0.20, 0.28, 0.27, 0.40, 0.40, 0.40, 0.40, 0.40, 0.40],
    );
    let luma = |p: &[f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let peak = |px: &[[f32; 3]]| {
        (0..12usize)
            .filter(|x| (0.5..0.95).contains(&weights[*x]))
            .map(|x| luma(&px[x]))
            .fold(f32::NEG_INFINITY, f32::max)
    };
    assert!(
        (peak(&rendered) - peak(&reference)).abs() <= 1e-6,
        "premise: the two buffers share their maximum, so two maxima cancel"
    );
    let reading = boundary_rim(&reference, &rendered, &rendered, &weights, w, h);
    assert!(
        (reading.rim - 0.050).abs() <= 1e-6,
        "the pixel that actually moved must be the one reported: {reading:?}"
    );
}

/// Step 9 / R6 (ARM F, user ruling 4): the p90 rank is a MAGNITUDE. One
/// scan line crosses the feather TWICE - a +0.030 bright bow and a -0.050
/// dark one - and a signed percentile would report the bright one and let
/// the larger dark seam hide behind it. Supervisor mutation M-2-E (the
/// rank reverted to signed) goes red here; this arm exists BECAUSE the
/// specification predicted M-2-E would survive on a single-run fixture.
///
/// RE-SHAPED 2026-10-22 (R2): the second crossing was an alpha bump
/// (0.8/0.6) with settled LAND on both sides — no feet, and the soft ruler
/// now skips such a band whole (4 transitions read, not 8). It is now a real
/// second transition, land back up to a settled sky (0.6/0.8, then 1.0), so
/// the line still crosses the feather twice; the readings are unchanged
/// (8 transitions, rim 0.050).
#[test]
fn the_rim_percentile_ranks_by_magnitude_so_a_dark_seam_cannot_hide() {
    let (w, h) = (16u32, 4u32);
    let line_weights = [
        1.0, 1.0, 1.0, 1.0, 0.8, 0.6, 0.0, 0.0, 0.0, 0.0, 0.6, 0.8, 1.0, 1.0, 1.0, 1.0f32,
    ];
    let n = (w * h) as usize;
    let (mut reference, mut rendered, mut weights) =
        (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..h {
        for (x, weight) in line_weights.iter().copied().enumerate() {
            let base = if weight == 0.0 { 0.40 } else { 0.20 };
            let moved = match x {
                4 | 5 => base + 0.030,
                10 | 11 => base - 0.050,
                _ => base,
            };
            reference.push([base; 3]);
            rendered.push([moved; 3]);
            weights.push(weight);
        }
    }
    let reading = boundary_rim(&reference, &rendered, &rendered, &weights, w, h);
    assert_eq!(reading.transitions, 2 * h as usize, "two crossings per row");
    assert!(
        (reading.rim - 0.050).abs() <= 1e-6,
        "the darker seam is the larger one and must be the one reported: {reading:?}"
    );
}

/// Step 9, scope addition: an ATTACHED-BUT-INERT zone correction is a
/// REFUSAL, on the zone gate.
///
/// RC2 caused this. With the transported differential a `k=0` render
/// reads exactly 0.0 by construction, so the `Dropped` branch that used
/// to catch a correction no shrink could rescue is structurally
/// unreachable, and the bisection hands back the largest passing `k`
/// instead — which for a correction whose every visible strength
/// introduces a seam is a `k` that renders to nothing. Attaching that is
/// strictly worse than refusing it: it keeps a raster on disk and
/// discloses a before/after pair it did not produce.
///
/// The criterion is byte identity of the render, not a threshold on `k`;
/// the fixture below therefore drives it by rendering to the reference
/// exactly, which is the general case rather than the `k == 0.0` corner.
/// Supervisor mutation M-4-A (the check deleted) goes red here.
#[test]
fn an_inert_zoned_correction_is_refused_rather_than_attached() {
    let (pixels, weights, w, h) = boundary_fixture_pixels(0.0);
    let source = image_of(&pixels, w, h);
    let (path, masks) = boundary_zone_masks("s9-inert-zone", &weights, w, h, -0.2, 0.1);
    let mut report = neutral_report(&source, &source);
    report.recipe.masks = masks;
    let reference = fit::pixels_of(&render::develop_preview(
        &source,
        &crate::recipe::EditRecipe::default(),
    ));
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[0.5, 0.5],
        0,
        &reference,
        reference.clone(),
    );
    assert!(
        matches!(verdict, BoundaryGateResult::Dropped),
        "a correction that moves no pixel may not attach"
    );
    assert!(report.recipe.masks.is_empty(), "and its masks must not survive");
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_BOUNDARY_INERT)
        .expect("an inert refusal needs its own typed note, not the PASSED one");
    assert!(
        report.recipe.rationale.contains("byte-identical to the frame without it"),
        "the disclosure must say what happened: {}",
        report.recipe.rationale
    );
    assert_eq!(note_number(note, "n"), 2.0);
    assert!(
        !report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::ZONE_BOUNDARY_PASSED),
        "a refusal may not also announce a pass"
    );
    path.remove();
}

/// `(source, mask, weights, reference, candidate, raster, report)` — one
/// soft-feather arm, rendered at k=1, with everything a boundary reading
/// or a gate call needs.
type HazyFeather = (
    DynamicImage,
    GrayImage,
    Vec<f32>,
    Vec<[f32; 3]>,
    Vec<[f32; 3]>,
    crate::store::OwnedRaster,
    fit::FitReport,
);

const HAZY: (u32, u32) = (64, 256);

fn hazy_arm(
    name: &str,
    source: DynamicImage,
    mask: GrayImage,
    exposure_ev: f32,
    gains: Option<[f32; 3]>,
) -> HazyFeather {
    let path = fixture_mask_path(name);
    mask.save(path.path()).unwrap();
    let weights = mask_weights(&mask, HAZY.0, HAZY.1);
    let mut report = neutral_report(&source, &source);
    report.recipe.masks = vec![LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
        role: MaskRole::ZoneSky,
        amount: 1.0,
        exposure_ev,
        color_gains: gains,
        ..Default::default()
    }];
    let reference = fit::pixels_of(&render::develop_preview(
        &source,
        &crate::recipe::EditRecipe::default(),
    ));
    let candidate = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
    (source, mask, weights, reference, candidate, path, report)
}

/// A hazy frame carrying a NARROW soft mask edge — the shape the
/// desert-dusk defect was measured on, at fixture scale.
///
/// The field rises 8 code values over 256 rows (0.031 code/px), so the
/// scene's own change across any baseline either ruler reads is a small
/// fraction of one code: featureless by exactly the rule
/// [`BOUNDARY_STEP_FLOOR`] states, and nothing here can mask anything.
/// `scene_edge` plants a hard 30-code luma step under the contour
/// instead — the control arm, where the neighbourhood can mask the whole
/// ceiling and the charge must vanish bit for bit.
///
/// 256 ROWS on purpose. Both halves of this batch are stated in ANALYSIS
/// pixels — the budget's 3-px baseline and the widener's share-of-height
/// cap — so only a frame at the analysis grid's own scale exercises
/// either rule. `ramp` is the alpha transition's width; three is what a
/// segmentation model emits in haze, and the whole height of the
/// correction is then delivered across those three pixels.
fn hazy_feather(
    name: &str,
    ramp: f32,
    exposure_ev: f32,
    gains: Option<[f32; 3]>,
    scene_edge: bool,
) -> HazyFeather {
    let (w, h) = HAZY;
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let base = 100.0 + y as f32 * 8.0 / (h - 1) as f32
            + if scene_edge && x >= 32 { 30.0 } else { 0.0 };
        let v = base.round().clamp(0.0, 255.0) as u8;
        image::Rgb([v, v, v])
    }));
    let mask = GrayImage::from_fn(w, h, |x, _| {
        let t = ((x as f32 - (32.0 - ramp * 0.5)) / ramp).clamp(0.0, 1.0);
        image::Luma([(t * 255.0).round() as u8])
    });
    hazy_arm(name, source, mask, exposure_ev, gains)
}

fn hazy_rim(
    reference: &[[f32; 3]],
    rendered: &[[f32; 3]],
    weights: &[f32],
) -> BoundaryReading {
    boundary_rim(reference, rendered, rendered, weights, HAZY.0, HAZY.1)
}

/// The same raster at a different dose, so a test can ask what the gate
/// WOULD have kept under a rule it no longer applies.
fn dose(
    source: &DynamicImage,
    path: &crate::store::OwnedRaster,
    exposure_ev: f32,
) -> Vec<[f32; 3]> {
    let mut recipe = crate::recipe::EditRecipe::default();
    recipe.masks.push(LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
        role: MaskRole::ZoneSky,
        amount: 1.0,
        exposure_ev,
        ..Default::default()
    });
    fit::pixels_of(&render::develop_preview(source, &recipe))
}

fn zone_share(weights: &[f32]) -> f32 {
    weights.iter().sum::<f32>() / weights.len().max(1) as f32
}

/// R37. A step the paired TARGET itself carries across the feather is
/// not a seam. The reference pair's sky zone was shrunk to k=0.105 for
/// reproducing the target's own horizon (+8.4 codes at the median against
/// the source's +3.0), because the ceiling was absolute. Four targets over
/// the narrow hazy feather at one dose: the candidate itself (every
/// introduced step is asked, charged exactly 0.0, kept whole), the
/// untouched frame (nothing asked: the context rule bit for bit), the
/// OPPOSITE dose (asked the other way: the context rule bit for bit) and
/// half the dose (half asked: a larger k than none, smaller than whole).
#[test]
fn a_step_the_target_itself_carries_is_not_charged_as_a_seam() {
    let (source, _, weights, reference, candidate, path, _) =
        hazy_feather("r37-soft-asked", 3.0, 0.30, None, false);
    let against = |target: Option<&[[f32; 3]]>| {
        boundary_rim_toward(target, &reference, &candidate, &candidate, &weights, HAZY.0, HAZY.1, None)
    };
    let none = against(None);
    assert!(
        none.transitions > 0 && none.charged > ZONE_BOUNDARY_RIM_MAX,
        "premise: on context alone this dose is over the ceiling: {none:?}"
    );
    assert_eq!(none.asked, 0.0, "nothing is asked without a target");
    let whole = against(Some(&candidate));
    assert_eq!(
        (whole.charged, whole.colour_charged),
        (0.0, 0.0),
        "every introduced step is one the target carries: {whole:?}"
    );
    assert_eq!(whole.rim.to_bits(), none.rim.to_bits(), "the raw rim is a reading, not a charge");
    assert!((whole.asked - whole.rim).abs() <= 1e-3, "and the disclosure says how much was asked: {whole:?}");
    let untouched = against(Some(&reference));
    assert_eq!(
        (untouched.charged.to_bits(), untouched.colour_charged.to_bits(), untouched.asked),
        (none.charged.to_bits(), none.colour_charged.to_bits(), 0.0),
        "a target with no step there asks nothing: {untouched:?}"
    );
    let opposite = dose(&source, &path, -0.30);
    let other_way = against(Some(&opposite));
    assert_eq!(
        other_way.charged.to_bits(),
        none.charged.to_bits(),
        "a step the other way is not asked: {other_way:?}"
    );
    let half = dose(&source, &path, 0.15);
    let half_asked = against(Some(&half));
    assert!(
        half_asked.charged > 0.0 && half_asked.charged < none.charged,
        "half asked, half charged: {half_asked:?} against {none:?}"
    );
    assert!(half_asked.asked > 0.0 && half_asked.asked < whole.asked, "{half_asked:?}");

    // The gate, on fresh copies of the same arm: the same source, mask and
    // dose render the same candidate, so the readings above are the
    // gate's own.
    let gate = |name: &str, target: Option<&[[f32; 3]]>| -> (f32, f32, f32) {
        let (source, _, weights, reference, candidate, path, mut report) =
            hazy_feather(name, 3.0, 0.30, None, false);
        let verdict = enforce_boundary_gate_toward(
            target,
            &source,
            &mut report,
            &weights,
            &[zone_share(&weights)],
            0,
            &reference,
            candidate,
        );
        path.remove();
        let BoundaryGateResult::Kept { k, after, .. } = verdict else {
            panic!("{name} was dropped: {}", report.recipe.rationale);
        };
        let note = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::ZONE_BOUNDARY_PASSED)
            .expect("typed boundary pass note");
        assert!((note_number(note, "asked") - after.asked).abs() <= 0.0005, "{note:?}");
        // The pass sentence itself names the asked part.
        assert!(
            report.recipe.rationale.contains("of which the target's own boundary asks"),
            "{}",
            report.recipe.rationale
        );
        (k, after.charged, after.asked)
    };
    let (k_none, _, asked_none) = gate("r37-soft-gate-none", None);
    let (k_whole, charged_whole, asked_whole) = gate("r37-soft-gate-whole", Some(&candidate));
    let (k_half, _, _) = gate("r37-soft-gate-half", Some(&half));
    let (k_opposite, _, _) = gate("r37-soft-gate-opposite", Some(&opposite));
    assert_eq!((k_whole, charged_whole), (1.0, 0.0), "the target's own horizon is kept whole");
    assert!(asked_whole > ZONE_BOUNDARY_RIM_MAX && asked_none == 0.0, "{asked_whole} {asked_none}");
    assert_eq!(k_opposite.to_bits(), k_none.to_bits(), "a horizon the other way buys nothing");
    assert!(k_none < k_half && k_half < 1.0, "half asked, half free: none {k_none} half {k_half}");
    path.remove();
}

/// R37. The unasked part of an introduced step, by cases.
#[test]
fn the_unasked_part_of_a_step_is_its_excess_over_what_the_target_carries() {
    assert_eq!(unasked(0.02, 0.03), 0.0, "inside what is asked");
    assert!((unasked(0.03, 0.02) - 0.01).abs() <= 1e-7, "the overshoot");
    assert_eq!(unasked(0.02, -0.03), 0.02, "the other way: all of it");
    assert_eq!(unasked(-0.02, -0.03), 0.0, "sign-symmetric");
    assert_eq!(unasked(0.02, 0.0), 0.02, "no target: the rule as it stood");
    assert_eq!(unasked(0.0, 0.03), 0.0, "nothing introduced reads exactly 0.0");
}

/// R37. A cell's share is a mean the cell can vouch for: eight crossings
/// at least; both means a code or more; the target moving the
/// correction's way; and the target's mean two standard errors from
/// zero — else nothing.
#[test]
fn a_cell_share_needs_a_quorum_a_mean_its_noise_cannot_hide_and_a_code() {
    let cell = |asked: &[f32], made: f32| {
        let mut cell = CellSums::default();
        for v in asked {
            cell.add([0.0; 4], [made; 4], Some([*v; 4]), [0.0; 2]);
            cell.crossings += 1;
        }
        cell
    };
    let eight = |v: f32| vec![v; TARGET_STEP_MIN_CROSSINGS];
    // 0.02 on average, ±0.05 pixel to pixel: a step buried in noise
    // until enough pixels are averaged.
    let noisy = |n: usize| -> Vec<f32> {
        (0..n).map(|i| 0.02 + if i % 2 == 0 { 0.05 } else { -0.05 }).collect()
    };
    let cells = vec![
        cell(&eight(0.02), 0.03),
        cell(&[0.02; TARGET_STEP_MIN_CROSSINGS - 1], 0.03),
        cell(&[0.02, -0.02, 0.02, -0.02, 0.02, -0.02, 0.02, -0.02, 0.02, -0.02], 0.03),
        cell(&eight(0.5 * BOUNDARY_STEP_FLOOR), 0.03),
        cell(&eight(0.02), 0.5 * BOUNDARY_STEP_FLOOR),
        cell(&eight(0.06), 0.03),
        cell(&eight(-0.02), 0.03),
        cell(&noisy(10), 0.03),
        cell(&noisy(40), 0.03),
        CellSums::default(),
    ];
    let share = cell_shares(&cells);
    assert!(
        (share[0][0] - 2.0 / 3.0).abs() <= 1e-6,
        "quorum, exact, five codes asked of a seven-code step: two thirds"
    );
    assert_eq!(share[0][1], 0.0, "one short of the quorum");
    assert_eq!(share[0][2], 0.0, "a mean of nothing");
    assert_eq!(share[0][3], 0.0, "half a code asked is the ruler's rounding");
    assert_eq!(share[0][4], 0.0, "a candidate step under a code has no share");
    assert!((share[0][5] - 2.0).abs() <= 1e-6, "more asked than made: the whole step, and then some");
    assert_eq!(share[0][6], 0.0, "the other way");
    assert_eq!(share[0][7], 0.0, "ten pixels: a five-code mean 1.3 standard errors out is noise");
    assert!((share[0][8] - 2.0 / 3.0).abs() <= 1e-6, "forty pixels: 2.5 standard errors out, honoured");
    assert_eq!(share[0][9], 0.0, "an empty cell reads nothing");
    assert!(share[1..].iter().all(|channel| channel == &share[0]), "each channel is read the same way");
}

/// R2 (2026-10-22). An alpha DIP inside the zone is not a boundary: a band
/// whose two settled ends both lie inside the mask has no feet, the soft
/// ruler cannot read its context, and it is skipped whole. A uniform frame,
/// a hard zone edge (no transition band there at all) and a 0.6-alpha strip
/// four columns wide well inside the zone, under a colour push: before the
/// rule the strip's 40% shortfall was read at the one-code floor budget and
/// the gate shrank the push; now nothing is measured and it is kept whole.
/// The footed transition is still charged — the guard is
/// `a_narrow_feather_over_haze_is_charged_against_its_own_flat_context`.
#[test]
fn an_alpha_dip_inside_the_zone_is_not_read_as_a_boundary() {
    let (w, h) = HAZY;
    let source = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([115; 3])));
    let mask = GrayImage::from_fn(w, h, |x, _| {
        image::Luma([if x < 32 { 0 } else if (40..44).contains(&x) { 153 } else { 255 }])
    });
    let (source, _, weights, reference, candidate, path, mut report) =
        hazy_arm("r2-interior-dip", source, mask, 0.0, Some([1.20, 1.0, 0.85]));
    let reading = hazy_rim(&reference, &candidate, &weights);
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[zone_share(&weights)],
        0,
        &reference,
        candidate,
    );
    path.remove();
    let BoundaryGateResult::Kept { k, .. } = verdict else {
        panic!("the push was dropped: {}", report.recipe.rationale);
    };
    assert_eq!(k, 1.0, "an interior alpha dip does not shrink the zone: {}", report.recipe.rationale);
    assert_eq!(reading.transitions, 0, "the dip has no feet, so no crossing is read: {reading:?}");
}

/// R1 (2026-10-22). The COLOUR charge reads its step through a one-code
/// dead band: a single channel's step is a difference of two 8-bit renders,
/// and at the floor budget's x3 rate one code of rounding was the whole
/// ceiling. One code introduced charges nothing; two charge exactly the
/// ceiling (one code past the dead band, times ceiling / floor).
#[test]
fn a_one_code_colour_step_at_the_floor_budget_is_rounding_and_charges_nothing() {
    let floor = BOUNDARY_STEP_FLOOR;
    assert_eq!(colour_charge_of_one_crossing(floor, floor), 0.0, "one code is rounding");
    assert_eq!(colour_charge_of_one_crossing(-floor, floor), 0.0, "either sign");
    let two = colour_charge_of_one_crossing(2.0 * floor, floor);
    assert!((two - ZONE_BOUNDARY_RIM_MAX).abs() <= 1e-6, "two codes charge the ceiling: {two}");
    let saturated = colour_charge_of_one_crossing(2.0 * floor, ZONE_BOUNDARY_RIM_MAX);
    assert!((saturated - floor).abs() <= 1e-7, "at a saturated budget the charge is the step past the dead band: {saturated}");
}

/// R2/R3 (2026-10-22). A cell's mean is read from FOOTED transitions only.
/// One evidence cell holds, on alternate rows, a footed band (land, then
/// alpha 0.6/0.8, then sky) and an in-in dip at the same columns (sky on
/// both sides). The target carries the candidate's own step on the footed
/// rows and none in the dips (it gives the dip the full treatment). Mixed,
/// the cell's mean target step is 0.75 codes — under the one-code floor, so
/// it asked nothing; footed only, 1.5 codes, the candidate's own: share 1.
#[test]
fn a_cell_share_is_read_from_footed_transitions_only() {
    let (w, h) = (96usize, 256usize);
    let footed = |y: usize| y.is_multiple_of(2);
    let alpha = |x: usize, y: usize| -> f32 {
        match x {
            34 => 0.6,
            35 => 0.8,
            x if x >= 36 => 1.0,
            x if !footed(y) && x >= 32 => 1.0,
            _ => 0.0,
        }
    };
    let weights: Vec<f32> = (0..w * h).map(|i| alpha(i % w, i / w)).collect();
    let d = 5.0 / 255.0;
    let reference = vec![[0.4f32; 3]; w * h];
    let candidate: Vec<[f32; 3]> = weights.iter().map(|a| [0.4 + a * d; 3]).collect();
    let target: Vec<[f32; 3]> = (0..w * h)
        .map(|i| if footed(i / w) { candidate[i] } else if weights[i] > 0.0 { [0.4 + d; 3] } else { [0.4; 3] })
        .collect();
    let reading =
        boundary_rim_toward(Some(&target), &reference, &candidate, &candidate, &weights, w as u32, h as u32, None);
    assert!(
        (reading.asked - 0.4 * d).abs() <= 1e-4,
        "the target's own step is asked once the dips stop diluting the cell: {reading:?}"
    );
    assert_eq!(reading.charged, 0.0, "and nothing the target carries is charged: {reading:?}");
    assert_eq!(reading.transitions, h / 2, "one footed crossing per footed row: {reading:?}");
}

/// R37. The sub-zone trial rulers read a target-referenced gap as a MEAN
/// per evidence cell: a re-synthesised texture — the reference pair's
/// sky at pixel scale — is zero-mean noise of several codes at every
/// crossing, and a per-crossing rank of it refused, on 2026-09-11, a
/// band set that improved the sky's deltaE 30.4 -> 25.4 and every worst
/// seam and target step, for "regressing" one cell by that noise.
#[test]
fn a_target_referenced_cell_gap_reads_the_band_shape_not_the_texture() {
    let (w, h) = (192usize, 96usize);
    let sky = |i: usize| i / w < h / 2;
    let flat: Vec<[f32; 3]> = (0..w * h).map(|i| if sky(i) { [0.5; 3] } else { [0.2; 3] }).collect();
    // An eight-row feather across the horizon, 1 above it and 0 below,
    // whose 50% contour falls between rows h/2-1 and h/2.
    let ramp: Vec<f32> =
        (0..w * h).map(|i| (((h / 2 + 4) as f32 - (i / w) as f32 - 0.5) / 8.0).clamp(0.0, 1.0)).collect();
    // The target's re-synthesised twin: three codes up and down, pixel
    // by pixel, on both sides of the horizon.
    let noisy: Vec<[f32; 3]> = flat
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let sign = if (i % w + i / w) % 2 == 0 { 1.0 } else { -1.0 };
            p.map(|c| c + sign * 3.0 / 255.0)
        })
        .collect();
    let (wu, hu) = (w as u32, h as u32);
    let worst = |gaps: &[[f32; 3]]| gaps.iter().fold(0.0f32, |m, g| m.max(g[0]).max(g[1]));
    let clean = (rim_cell_gaps(&flat, &flat, &ramp, wu, hu, None), step_cell_gaps(&flat, &flat, &ramp, wu, hu, None));
    assert_eq!(clean.0.len(), crate::fit_cells::CELLS_X * crate::fit_cells::CELLS_Y);
    assert_eq!((worst(&clean.0), worst(&clean.1)), (0.0, 0.0), "the render IS the target");
    let twin = (rim_cell_gaps(&noisy, &flat, &ramp, wu, hu, None), step_cell_gaps(&noisy, &flat, &ramp, wu, hu, None));
    assert!(
        worst(&twin.0) < BOUNDARY_STEP_FLOOR && worst(&twin.1) < BOUNDARY_STEP_FLOOR,
        "against its re-synthesised twin the gap is the noise's mean, under a code: {:?} {:?}",
        worst(&twin.0),
        worst(&twin.1)
    );
    // And a four-code shape the render has and the target does not is
    // read through that same noise: a lift of the band alone for the
    // soft ruler, of the whole sky side for the hard one.
    let lifted = |rows: std::ops::Range<usize>| -> Vec<[f32; 3]> {
        flat.iter()
            .enumerate()
            .map(|(i, p)| if rows.contains(&(i / w)) { p.map(|c| c + 4.0 / 255.0) } else { *p })
            .collect()
    };
    let band = rim_cell_gaps(&noisy, &lifted(h / 2 - 4..h / 2), &ramp, wu, hu, None);
    let side = step_cell_gaps(&noisy, &lifted(0..h / 2), &ramp, wu, hu, None);
    for (name, gaps) in [("band", &band), ("side", &side)] {
        let read: Vec<f32> = gaps.iter().map(|g| g[0]).filter(|v| *v > 0.0).collect();
        assert_eq!(read.len(), crate::fit_cells::CELLS_X, "{name}: one horizon cell per column");
        assert!(read.iter().all(|v| *v >= 3.0 / 255.0), "{name}: four codes read as at least three: {read:?}");
    }
}

/// A1 (2026-09-10). The soft family charges its own transitions now, and
/// this fixture is the shape that forced it: a two-to-three pixel
/// segmentation feather over featureless haze, where the scalar ceiling
/// let the shrink park a three-code seam and call it inside budget.
#[test]
fn a_narrow_feather_over_haze_is_charged_against_its_own_flat_context() {
    let (source, _, weights, reference, candidate, path, mut report) =
        hazy_feather("soft-charge-haze", 3.0, 0.30, None, false);
    let measured = hazy_rim(&reference, &candidate, &weights);
    assert!(measured.transitions > 0, "premise: the feather must be read: {measured:?}");
    assert!(
        measured.charged >= measured.rim,
        "the charge may never weaken a reading: {measured:?}"
    );
    let rate = ZONE_BOUNDARY_RIM_MAX / BOUNDARY_STEP_FLOOR;
    assert!(
        (measured.charged / measured.rim - rate).abs() <= 0.01,
        "a featureless neighbourhood earns the floor and nothing more, so every transition \
             pays the whole ceiling/floor exchange of {rate:.2}x: {measured:?}"
    );
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[zone_share(&weights)],
        0,
        &reference,
        candidate,
    );
    let BoundaryGateResult::Kept { k, after, .. } = verdict else {
        panic!("a shrinkable feather was dropped: {}", report.recipe.rationale);
    };
    assert!((0.0..1.0).contains(&k), "the dose must really shrink: k={k}");
    assert!(after.gated() <= ZONE_BOUNDARY_RIM_MAX, "the kept reading: {after:?}");
    assert!(
        after.rim <= BOUNDARY_STEP_FLOOR + 1e-6,
        "on a flat neighbourhood every budget is the floor, so the kept seam is ONE code \
             instead of the three the scalar ceiling allowed: {after:?}"
    );
    // The scalar rule, reproduced first-party rather than kept as a second
    // copy of the old code: at twice the kept dose the RAW rim is still
    // inside the old budget — that is the shrink the gate used to hand
    // back — while the charged reading is over the ceiling.
    let doubled = dose(&source, &path, 0.30 * (2.0 * k).min(1.0));
    let old_rule = hazy_rim(&reference, &doubled, &weights);
    assert!(
        old_rule.rim <= ZONE_BOUNDARY_RIM_MAX,
        "premise: the raw-rim rule would have kept twice this dose: {old_rule:?}"
    );
    assert!(
        old_rule.charged > ZONE_BOUNDARY_RIM_MAX,
        "and the charged rule refuses it: {old_rule:?}"
    );
    path.remove();
}

/// A2: the control arm. The charge is contextual, not a tightening — put
/// a real luma step under the same feather and the reading is the raw one
/// BIT FOR BIT, which is the scalar rule's own arithmetic.
///
/// RE-PINNED 2026-10-22 (R1): the colour charge reads its step through the
/// one-code dead band, so at a saturated budget it is the raw colour LESS
/// one code (measured 0.02745 raw, 0.02353 charged), not the raw bits; luma
/// is still the raw reading bit for bit.
#[test]
fn a_hard_scene_edge_under_the_feather_saturates_the_budget_and_charges_nothing() {
    let (_, _, weights, reference, candidate, path, _) =
        hazy_feather("soft-charge-scene-edge", 3.0, 0.30, None, true);
    let measured = hazy_rim(&reference, &candidate, &weights);
    assert!(measured.transitions > 0, "premise: the feather must be read: {measured:?}");
    assert_eq!(
        measured.charged.to_bits(),
        measured.rim.to_bits(),
        "a 30-code scene step across the band saturates the ceiling, \
             so the luma reading is the raw one: {measured:?}"
    );
    assert!(
        (measured.colour_charged - (measured.colour - BOUNDARY_STEP_FLOOR)).abs() <= 1e-6,
        "and the colour charge is the raw step past its one-code dead band: {measured:?}"
    );
    path.remove();
}

/// B then A, the order the producer runs them in. Widening the feather
/// first gives the ramp back its slope credit, so the SAME dose is charged
/// almost nothing and the shared shrink lands where the raw rim alone
/// asked instead of a third of the way there.
///
/// What this arm can and cannot claim is worth stating, because the
/// instrument decides it: the transition-band ruler reads the correction's
/// SHORTFALL at the 50% contour, which a wider ramp does not reduce — it
/// is the ramp's height, not its steepness. What widening buys is the
/// slope CREDIT, and only inside the window the budget's own constants
/// draw (see [`crate::mask_refine`]'s cap): wide enough to persist past
/// two 3-px baselines, narrow enough that `BOUNDARY_STEP_SHAPE` times its
/// slope still covers the shortfall. Inside that window the charge nearly
/// vanishes, which is what this test pins.
#[test]
fn widening_the_feather_first_buys_back_the_charge_the_budget_takes() {
    let (source, mask, weights, reference, candidate, narrow_path, mut narrow) =
        hazy_feather("soft-charge-narrow", 3.0, 0.30, None, false);
    let narrow_reading = hazy_rim(&reference, &candidate, &weights);
    let BoundaryGateResult::Kept { k: narrow_k, .. } = enforce_boundary_gate(
        &source,
        &mut narrow,
        &weights,
        &[zone_share(&weights)],
        0,
        &reference,
        candidate,
    ) else {
        panic!("the narrow arm was dropped: {}", narrow.recipe.rationale);
    };
    narrow_path.remove();

    let crate::mask_refine::WidenOutcome::Widened { mask: widened, reading } =
        crate::mask_refine::widen_smooth_feather(&source, &mask)
    else {
        panic!("featureless haze must be widened, not abstained on");
    };
    assert!(
        reading.widened_share >= 0.99,
        "the whole contour of this fixture is haze: {reading:?}"
    );
    let (_, _, wide_weights, wide_reference, wide_candidate, wide_path, mut wide) =
        hazy_arm("soft-charge-widened", source.clone(), widened, 0.30, None);
    let measured = hazy_rim(&wide_reference, &wide_candidate, &wide_weights);
    assert!(
        measured.charged <= 1.25 * measured.rim,
        "a ramp that persists past two baselines earns nearly its whole budget back, \
             against the narrow arm's {:.2}x: {measured:?}",
        narrow_reading.charged / narrow_reading.rim
    );
    let verdict = enforce_boundary_gate(
        &source,
        &mut wide,
        &wide_weights,
        &[zone_share(&wide_weights)],
        0,
        &wide_reference,
        wide_candidate,
    );
    let BoundaryGateResult::Kept { k: wide_k, after, .. } = verdict else {
        panic!("the widened arm was dropped: {}", wide.recipe.rationale);
    };
    // Printed, like BOUNDARY_CALIBRATION, so the numbers this argument
    // rests on are readable from a test run instead of a changelog.
    eprintln!(
        "FEATHER_WIDENING narrow: rim={:.4} charged={:.4} k={:.3} | \
             widened: rim={:.4} charged={:.4} k={:.3} kept={:.4}",
        narrow_reading.rim,
        narrow_reading.charged,
        narrow_k,
        measured.rim,
        measured.charged,
        wide_k,
        after.gated(),
    );
    assert!(after.gated() <= ZONE_BOUNDARY_RIM_MAX, "the kept reading: {after:?}");
    assert!(
        wide_k > 2.0 * narrow_k,
        "widening must hand back most of the ceiling/floor exchange the charge took: \
             k={wide_k:.4} against the narrow arm's {narrow_k:.4}"
    );
    wide_path.remove();
}

/// The colour ruler, on the arrangement a luma-only one cannot see: gains
/// normalised to leave luma601 where it was. Without the per-channel
/// difference in differences this zone reads as no seam at all.
#[test]
fn a_luma_preserving_colour_zone_is_a_seam_the_luma_ruler_cannot_see() {
    let (source, _, weights, reference, candidate, path, mut report) = hazy_feather(
        "soft-colour-seam",
        3.0,
        0.0,
        Some([1.205, 0.964, 0.723]),
        false,
    );
    let measured = hazy_rim(&reference, &candidate, &weights);
    assert!(measured.transitions > 0, "premise: the feather must be read: {measured:?}");
    assert!(
        measured.rim < BOUNDARY_STEP_FLOOR,
        "premise: these gains move luma601 by less than one code: {measured:?}"
    );
    assert!(
        measured.charged <= ZONE_BOUNDARY_RIM_MAX,
        "premise: the luma gate alone would keep this zone whole: {measured:?}"
    );
    assert!(
        measured.colour > 4.0 * measured.rim,
        "the channels moved where luma did not: {measured:?}"
    );
    assert!(
        measured.colour_charged > ZONE_BOUNDARY_RIM_MAX,
        "and the colour gate must refuse the halo: {measured:?}"
    );
    let verdict = enforce_boundary_gate(
        &source,
        &mut report,
        &weights,
        &[zone_share(&weights)],
        0,
        &reference,
        candidate,
    );
    let BoundaryGateResult::Kept { k, after, .. } = verdict else {
        panic!("a shrinkable colour seam was dropped: {}", report.recipe.rationale);
    };
    assert!(k < 1.0, "the gate must shrink on the colour reading alone: k={k}");
    assert!(after.gated() <= ZONE_BOUNDARY_RIM_MAX, "{after:?}");
    path.remove();
}

/// The regression guard the colour ruler needs: on a NEUTRAL frame under
/// a pure exposure dial the per-channel reading is the luma reading, so
/// the second coordinate cannot have moved any verdict that was decided
/// on a grey fixture.
///
/// RE-PINNED 2026-10-22 (R1): the raw readings are still one reading, but
/// the colour CHARGE now reads its step through the one-code dead band and
/// luma's does not, so on grey the colour charge is the luma charge's rate
/// applied to one code less (measured 0.0720 luma, 0.0600 colour) — never
/// above it, so `gated()` on a grey fixture is still the luma charge.
#[test]
fn a_pure_exposure_zone_reads_the_same_in_colour_as_in_luma() {
    let (_, _, weights, reference, candidate, path, _) =
        hazy_feather("soft-colour-neutral", 3.0, 0.30, None, false);
    let measured = hazy_rim(&reference, &candidate, &weights);
    let rate = measured.charged / measured.rim;
    assert!(
        (measured.colour - measured.rim).abs() <= 1e-6
            && (measured.colour_charged - rate * (measured.colour - BOUNDARY_STEP_FLOOR)).abs() <= 1e-5,
        "R = G = B makes the two coordinates one reading, less the colour dead band: {measured:?}"
    );
    assert_eq!(measured.gated().to_bits(), measured.charged.to_bits(), "{measured:?}");
    path.remove();
}

#[test]
fn local_quality_gate_rejects_texture_amplification_and_crushing() {
    let (w, h) = (16u32, 16u32);
    let before: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let ripple = if (i + i / w) % 2 == 0 { -0.025 } else { 0.025 };
            [0.45 + ripple; 3]
        })
        .collect();
    let amplified: Vec<[f32; 3]> = before
        .iter()
        .map(|p| [0.45 + 3.0 * (p[0] - 0.45); 3])
        .collect();
    let crushed = vec![[0.45; 3]; before.len()];
    let clean: Vec<[f32; 3]> = before.iter().map(|p| [p[0] + 0.02; 3]).collect();
    let clipped: Vec<[f32; 3]> = (0..before.len())
        .map(|i| if i % 2 == 0 { [0.0; 3] } else { [1.0; 3] })
        .collect();
    let weights = vec![1.0; before.len()];
    let high = local_quality(&before, &amplified, &weights, w, h);
    let low = local_quality(&before, &crushed, &weights, w, h);
    let good = local_quality(&before, &clean, &weights, w, h);
    let clip = local_quality(&before, &clipped, &weights, w, h);
    assert!(!high.texture_passes() && !high.passes(), "amplification passed: {high:?}");
    assert!(!low.texture_passes() && !low.passes(), "crushing passed: {low:?}");
    assert!(good.passes(), "clean correction failed: {good:?}");
    assert!(!clip.clipping_passes() && !clip.passes(), "clipping half was bypassed: {clip:?}");
}

/// The gate guards contrast, not exposure: a dark zone lifted 3x (≈1.6 EV,
/// inside `ZONE_EV_LIMIT`) carries its gradients up with it and must read 1,
/// while the same lift WITH its ripple tripled about the lifted mean is still
/// amplification. Before 2026-10-19 the first read 3.0 and was dropped, which
/// left the colour field to chase the lift unguarded.
#[test]
fn local_quality_gate_reads_a_pure_lift_as_unchanged_texture() {
    let (w, h) = (16u32, 16u32);
    let before: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let ripple = if (i + i / w) % 2 == 0 { -0.006 } else { 0.006 };
            [0.05 + ripple; 3]
        })
        .collect();
    let lifted: Vec<[f32; 3]> = before.iter().map(|p| p.map(|v| 3.0 * v)).collect();
    let amplified: Vec<[f32; 3]> =
        before.iter().map(|p| p.map(|v| 0.15 + 9.0 * (v - 0.05))).collect();
    let weights = vec![1.0; before.len()];
    let lift = local_quality(&before, &lifted, &weights, w, h);
    let amp = local_quality(&before, &amplified, &weights, w, h);
    assert!((lift.texture_ratio - 1.0).abs() <= 1e-4 && lift.passes(), "pure lift: {lift:?}");
    assert!((amp.texture_ratio - 3.0).abs() <= 1e-3 && !amp.passes(), "amplified lift: {amp:?}");
}

#[test]
fn local_quality_gate_passes_every_accepted_fixture_zone() {
    let (src, tgt, sky_mask) = zoned_pair();
    let mask_path = fixture_mask_path("zoned-quality-calibration");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&src, &tgt);
    attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
    assert!(
        !report.recipe.masks.is_empty(),
        "the accepted zoned fixture was lost: {}",
        report.recipe.rationale
    );
    let passed = report
        .notes
        .iter()
        .filter(|n| n.key == crate::rationale::keys::ZONE_QUALITY_PASSED)
        .count();
    assert_eq!(passed, report.recipe.masks.len(), "every accepted zone needs a pass verdict");
    assert!(
        !report.notes.iter().any(|n| {
            n.key == crate::rationale::keys::ZONE_QUALITY_TEXTURE_FAILED
                || n.key == crate::rationale::keys::ZONE_QUALITY_CLIPPING_FAILED
        }),
        "an accepted calibration zone failed local quality: {}",
        report.recipe.rationale
    );
    mask_path.remove();

    // Calibrate the rejecting side on the saved generated-cloud correction
    // when the supervisor material is present. Rendering the same global
    // recipe with and without its two bitmap zones isolates local quality.
    let Some(material) = fit::calibration_corpus() else { return };
    let saved_mask = material.join("sky-mask.png");
    let raw = material.join("source.arw");
    if material.join("fitted.recipe.json").exists() && saved_mask.exists() && raw.exists() {
        let with_zones = fit::calibration_recipe(&material);
        let mut without_zones = with_zones.clone();
        without_zones.masks.clear();
        let before_image =
            render::render_to_image(&raw, &without_zones, None, Some(384)).unwrap();
        let after_image =
            render::render_to_image(&raw, &with_zones, None, Some(384)).unwrap();
        let before = fit::pixels_of(&before_image);
        let after = fit::pixels_of(&after_image);
        let weights = mask_weights(
            &image::open(&saved_mask).unwrap().to_luma8(),
            before_image.width(),
            before_image.height(),
        );
        let saved = local_quality(&
            before,
            &after,
            &weights,
            before_image.width(),
            before_image.height(),
        );
        // 1.024 per unit of brightness (2026-10-19); the absolute-gradient
        // reading this replaced measured 0.961 on the same pair.
        assert!(
            (saved.texture_ratio - 1.024).abs() <= 0.02,
            "saved generated-cloud quality calibration drifted: {saved:?}"
        );
        assert!(
            saved.passes(),
            "the specified mean-gradient/clipping statistic cannot honestly reject the saved correction; this measured contradiction is reported: {saved:?}"
        );
    }
}

/// A synthetic zone whose target is BOTH much brighter and much more
/// saturated than its source. `sat` multiplies the zone's chroma about
/// its own mean and `ev` scales it; the upper half of the frame is
/// byte-identical on both sides so only the lower half is under test.
fn strictly_better_fixture(sat: f32, ev: f32, warm: f32, target: bool) -> DynamicImage {
    let (w, h) = (96u32, 96u32);
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let p: [f32; 3] = if y < h / 2 {
            let v = 0.30 + 0.30 * x as f32 / (w - 1) as f32;
            [v, v, v]
        } else {
            let v = 0.16 + 0.10 * (((x / 8) + (y / 8)) % 2) as f32;
            let base = [v * 1.10, v * 0.95, v * 0.80];
            if target {
                let mid = (base[0] + base[1] + base[2]) / 3.0;
                [
                    (mid + (base[0] - mid) * sat) * ev * warm,
                    (mid + (base[1] - mid) * sat) * ev,
                    (mid + (base[2] - mid) * sat) * ev / warm,
                ]
            } else {
                base
            }
        };
        image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
    }))
}

/// Drive the lower half of one of those pairs through `attach_one_zone`
/// and hand back the outcome plus the report it wrote.
fn run_strictly_better_zone(
    name: &str,
    sat: f32,
    ev: f32,
    warm: f32,
    atmosphere: bool,
) -> (Option<AcceptedZone>, fit::FitReport, f32, f32) {
    let (w, h) = (96u32, 96u32);
    let src = strictly_better_fixture(sat, ev, warm, false);
    let tgt = strictly_better_fixture(sat, ev, warm, true);
    let mask =
        GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= h / 2 { 255u8 } else { 0 }]));
    let path = fixture_mask_path(name);
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let t_px = fit::pixels_of(&t_img);
    let sw = mask_weights(&mask, s_img.width(), s_img.height());
    let tw = mask_weights(&mask, t_img.width(), t_img.height());
    let attachment = semantic_attachment(sw, tw, &path);
    // The running frame on the gate's own ruler, as `dilution_run` reads it:
    // `neutral_report` carries the plain look distance, and since v1.18.0 (R1)
    // the gate compares the two readings with no allowance to absorb a
    // difference of rulers.
    let mut frame_err = fit::look_err_with_evidence(&fit::pixels_of(&s_img), &t_px, &report.evidence);
    let frame_before = frame_err;
    let accepted = attach_one_zone(
        &s_img,
        &t_px,
        &mut report,
        &mut frame_err,
        &attachment,
        if atmosphere {
            // Forced past DIVERGENCE_ZONE so the SAME pair is judged by the
            // Atmosphere match arm — the mode split itself is under test.
            Some(fit::Divergence { correlation: 0.0, energy_error: 1.0, d: 1.0 })
        } else {
            measure_zone_divergence(&src, &tgt, &crate::recipe::EditRecipe::default(), &mask)
                .sky
                .divergence
        },
        None,
    );
    path.remove();
    (accepted, report, frame_before, frame_err)
}

/// RE-READ (RULING-1180-int-r5 (b), one law at the zone's acceptance). This
/// was the end-to-end pin of the strictly-better arm (R30 batch 1): a
/// correction the two ratio arms refused and the absolute arm bought had to
/// attach and say so. The arms are retired; what it witnesses under J* is
/// the one law's own verdict, END TO END: a zone that earns FIT_QUANT on its
/// own pixels with the frame inside its guard ATTACHES and its note carries
/// the readings it was decided on (the free-mask lesson: a disclosure that
/// never reaches the note table was not made). Read on the (sat 4, ev 3.5,
/// warm 1.4) pair (DEF-3: the (4, 8, 1) pair's correction flattens the
/// checker the target keeps, the structural term 0.088 -> 0.698, and the
/// R19 guard refuses it, rightly).
#[test]
fn a_zone_that_earns_on_its_own_pixels_attaches_and_names_the_readings_it_shipped_on() {
    let (accepted, report, frame_before, frame_after) =
        run_strictly_better_zone("strictly-better-attach", 4.0, 3.5, 1.4, false);
    accepted.unwrap_or_else(|| {
        panic!("a zone that earns on its own pixels attaches: {}", report.recipe.rationale)
    });
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_SHIPPED_ON_OWN_PIXELS)
        .unwrap_or_else(|| panic!("the zone attached in silence: {}", report.recipe.rationale));
    let (own_before, own_after) = (note_number(note, "own_before"), note_number(note, "own_after"));
    assert!(
        fit::ships(own_after, own_before),
        "the note's own readings are a FIT_QUANT gain: {own_before} -> {own_after}"
    );
    assert!(
        (note_number(note, "frame_before") - frame_before).abs() <= 1e-5
            && (note_number(note, "frame_after") - frame_after).abs() <= 1e-5,
        "the note's frame pair is the guard's: {note:?} vs {frame_before} -> {frame_after}"
    );
    assert!(frame_after - frame_before <= fit::FIT_QUANT, "premise: inside the frame guard");
    assert!(!report.recipe.masks.is_empty(), "an accepted zone leaves its mask attached");
}

/// REWRITTEN (v1.18.0, the structural term's tie rule, 780f90d). Until
/// v1.18.0 this fixture reached the local-quality gate with its colour class
/// vouched by the zone's pixels, and pinned that the gate withheld the colour
/// alone and let the tone ship. Under the tie rule the clipped-bright target
/// zone reads as the flat block it is (D 0.418, was under the 0.35 pairing
/// line), so the zone is Cell-scale: its pixels are not each other's
/// counterparts and the colour move is put to the target's 12x8 cell means
/// instead, which do not vouch it (1.000 converged, 0.483 aligned over 12
/// cells). The outcome the old name stated still holds, by the evidence gate
/// that now fires first: the colour class is withheld whole, said once, and
/// the tone ships alone through the absolute arm (0.942 -> 0.515). The
/// quality gate's colour-only refusal no longer has a fixture that reaches
/// it (lane c notes, rel1180).
#[test]
fn a_colour_move_the_cells_do_not_vouch_leaves_the_tone_to_ship_alone() {
    let (accepted, report, _, _) =
        run_strictly_better_zone("quality-withholds-colour", 2.0, 8.0, 1.0, false);
    let zone = accepted
        .unwrap_or_else(|| panic!("the tone move must still attach: {}", report.recipe.rationale));
    let mask = &report.recipe.masks[zone.mask_index];
    assert_gains_withheld(mask.color_gains);
    assert_eq!(mask.saturation, 0.0, "the gate withholds the whole colour class");
    assert!(mask.exposure_ev > 0.0, "the tone move ships: {mask:?}");
    let withheld = report
        .notes
        .iter()
        .filter(|n| n.key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR_CELLS)
        .count();
    assert_eq!(withheld, 1, "the cells' refusal of the colour class is said once: {}", report.recipe.rationale);
    assert!(
        !report.notes.iter().any(|n| {
            n.key == crate::rationale::keys::ZONE_COLOUR_WITHHELD_BY_QUALITY
                || n.key == crate::rationale::keys::ZONE_QUALITY_TEXTURE_FAILED
                || n.key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_PIXELS
        }),
        "one outcome, one sentence: {}",
        report.recipe.rationale
    );
    assert!(
        report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_SHIPPED_ON_OWN_PIXELS),
        "the tone ships on its own pixels: {}",
        report.recipe.rationale
    );
}

/// RE-READ (RULING-1180-int-r5 (b)). This pinned that an ATMOSPHERE zone
/// was judged by its own do-no-harm arm and never by the Full moments
/// predicate (no strictly-better note). Under one law both modes ship on
/// their own pixels; what it witnesses now is that an improving atmosphere
/// zone attaches on that law — its own pixels gain FIT_QUANT and so do no
/// harm — says so with the one note, and names the Atmosphere mode.
/// RE-READ (RULING-1180-int-r6 (3)): the Atmosphere do-no-harm is no longer
/// a condition of its own; it is `ships_local`'s gain on the same own
/// pixels. The shipped zone's note is that gain, and on the (1, 1.25, 0.7)
/// pair the same route REFUSES the Atmosphere zone on `ships_local` alone
/// (its own pixels gain FIT_QUANT, the frame rising +0.00271 past
/// the FIT_QUANT guard), under the Atmosphere drop note.
#[test]
fn an_improving_atmosphere_zone_ships_on_its_own_pixels_and_names_its_mode() {
    let (accepted, report, _frame_before, _frame_after) =
        run_strictly_better_zone("atmos-do-no-harm", 4.0, 8.0, 1.0, true);
    assert!(
        accepted.is_some(),
        "an improving atmosphere zone attaches: {}",
        report.recipe.rationale
    );
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_SHIPPED_ON_OWN_PIXELS)
        .unwrap_or_else(|| panic!("the one law names its readings: {}", report.recipe.rationale));
    assert!(
        fit::ships(note_number(note, "own_after"), note_number(note, "own_before")),
        "its own pixels gained FIT_QUANT, which is its do-no-harm: {note:?}"
    );
    assert!(
        report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_MODE_ATMOSPHERE),
        "the zone must say it was fitted as atmosphere: {}",
        report.recipe.rationale
    );
    let (refused, report, frame_before, frame_after) =
        run_strictly_better_zone("atmos-refused", 1.0, 1.25, 0.7, true);
    assert!(refused.is_none(), "ships_local refuses this atmosphere zone: {}", report.recipe.rationale);
    let drop = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_ATMOSPHERE_DROPPED)
        .unwrap_or_else(|| panic!("the refusal is the Atmosphere drop: {}", report.recipe.rationale));
    let (own_before, own_after, drift) =
        (note_number(drop, "own_before"), note_number(drop, "own_after"), note_number(drop, "drift"));
    assert!(
        !fit::ships_local(own_after, own_before, frame_before + drift, frame_before),
        "the drop is ships_local's verdict on its own readings: {drop:?}"
    );
    assert_eq!(frame_after, frame_before, "a refused zone leaves the running frame where it was");
}

/// R39. A SHRUNK correction is held to do-no-harm against the render
/// without it, not to the k=1 law. Pinned on the two verdicts that
/// separate the predicates: a hairline move toward the target is KEPT, and
/// the same-sized move away is REFUSED. The frame condition is R19's guard
/// (a rise of at most FIT_QUANT), read at its edge. (RULING-1180-int-r5
/// (b): the premise that every k=1 moments arm refused the hairline gain
/// went with those arms.) RE-READ (RULING-1180-int-r6 (2)): the zone half
/// reads the one objective on the zone's own pixels (`own_reading`, the J*
/// `ships_local` read at k = 1), not the display moments; the two verdicts
/// it separates are the same, read in J*.
#[test]
fn a_shrunk_correction_is_held_to_do_no_harm_not_to_the_k1_law() {
    let flat = |v: u8| DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([v, v, v])));
    let (src, tgt) = (flat(102), flat(128));
    let report = neutral_report(&src, &tgt);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let n = (s_img.width() * s_img.height()) as usize;
    let grey = |v: f32| vec![[v, v, v]; n];
    let zone = AcceptedZone {
        label: "sky".to_string(),
        range: None,
        mask_index: 0,
        source_weights: vec![1.0; n],
        target_weights: vec![1.0; n],
        mask_weights: vec![1.0; n],
        before: 0.0,
        after: 0.0,
        rendered: Vec::new(),
        judged_before: 0.0,
        own_weights: (vec![1.0; n], vec![1.0; n]),
    };
    let target = fit::pixels_of(&t_img);
    let without = zone.judged(&report, &grey(0.40), &target);
    let (toward, away) = (grey(0.403), grey(0.397));
    let judged_toward = zone.judged(&report, &toward, &target);
    let judged_away = zone.judged(&report, &away, &target);
    assert!(
        judged_toward < without && judged_away > without,
        "premise: 0.403 sits nearer the 0.50 target than 0.40 and 0.397 farther, in own-pixel J*: \
         {judged_toward} / {without} / {judged_away}"
    );
    // RE-PINNED (v1.18.0, R19): the frame half of the predicate is a GUARD
    // for a local correction — the shrunk set may not raise the frame by more
    // than FIT_QUANT — and the zone half (no worse than the set without it)
    // is the do-no-harm; the gain was decided at k = 1 on the zone's own
    // population. (Under R1 the frame half was the earning line, a gain of
    // FIT_QUANT, and this test handed it a set that earned 2·FIT_QUANT.) The
    // zone half is what these two verdicts separate, so the frame is handed
    // a set inside the guard.
    let fb = 0.100f32;
    let earned = fb - 2.0 * fit::FIT_QUANT;
    let r = &report;
    assert!(zone.still_accepted(r, &toward, &target, without, fb, earned), "a hairline gain does no harm");
    assert!(!zone.still_accepted(r, &away, &target, without, fb, earned), "a move away from the target is harm");
    // The frame side at its edge: a gain, a tie and a rise up to FIT_QUANT
    // are inside the guard; a rise of FIT_QUANT and a hair more is not.
    assert!(zone.still_accepted(r, &toward, &target, without, fb, fb - fit::FIT_QUANT - 1e-4), "a frame gain is inside the guard");
    assert!(zone.still_accepted(r, &toward, &target, without, fb, fb), "a tie is inside the guard");
    assert!(zone.still_accepted(r, &toward, &target, without, fb, fb + fit::FIT_QUANT - 1e-4), "a rise under FIT_QUANT is inside the guard");
    assert!(
        !zone.still_accepted(r, &toward, &target, without, fb, fb + fit::FIT_QUANT + 1e-4),
        "a rise past FIT_QUANT breaks the guard"
    );
}

/// R39. On the shared routes each zone's baseline is the shipped set
/// WITHOUT that zone, rendered — not the residual its acceptance read
/// with the earlier zones at full strength. Two hard halves of a flat
/// frame: the target lifts the left half only; the shrunk set lifts the
/// left (helps) and darkens the right (harms: the target never asked).
/// The right correction must go, the left must stay, `originals` must
/// follow `accepted` in step and the refusal must be a typed note.
#[test]
fn refuse_shrunk_zones_judges_each_zone_against_the_shipped_set_without_it() {
    let edge = 64u32;
    let source = DynamicImage::ImageRgb8(image::RgbImage::from_fn(edge, edge, |_, _| {
        image::Rgb([120, 110, 100])
    }));
    let paths = [fixture_mask_path("shrunk-left"), fixture_mask_path("shrunk-right")];
    let masks = [0u32, 32u32]
        .iter()
        .zip(&paths)
        .map(|(&x0, path)| {
            let mask = GrayImage::from_fn(edge, edge, |x, _| {
                image::Luma([if x >= x0 && x < x0 + 32 { 255 } else { 0 }])
            });
            mask.save(path.path()).unwrap();
            mask
        })
        .collect::<Vec<_>>();
    let adjustment = |path: &crate::store::OwnedRaster, ev: f32| LocalAdjustment {
        mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
        name: "half".to_string(),
        role: MaskRole::Custom,
        amount: 1.0,
        exposure_ev: ev,
        ..Default::default()
    };
    let mut wanted = crate::recipe::EditRecipe::default();
    wanted.masks.push(adjustment(&paths[0], 0.30));
    let target_img = render::develop_preview(&source, &wanted);
    let tgt_px = fit::pixels_of(&target_img);
    let mut report = neutral_report(&source, &target_img);
    // The shipped (shrunk) set, as the boundary gate leaves it on the recipe.
    report.recipe.masks.push(adjustment(&paths[0], 0.10));
    report.recipe.masks.push(adjustment(&paths[1], -0.10));
    let mut originals = vec![adjustment(&paths[0], 0.40), adjustment(&paths[1], -0.40)];
    let pixels = fit::pixels_of(&render::develop_preview(&source, &report.recipe));
    let zone = |i: usize, label: &str| AcceptedZone {
        label: label.to_string(),
        range: None,
        mask_index: i,
        source_weights: mask_weights(&masks[i], edge, edge),
        target_weights: mask_weights(&masks[i], edge, edge),
        mask_weights: mask_weights(&masks[i], edge, edge),
        before: 0.0,
        after: 0.0,
        rendered: Vec::new(),
        judged_before: 0.0,
        own_weights: (mask_weights(&masks[i], edge, edge), mask_weights(&masks[i], edge, edge)),
    };
    let mut accepted = vec![zone(0, "left"), zone(1, "right")];
    let frame = fit::look_err_with_evidence(&pixels, &tgt_px, &report.evidence);
    // RE-PINNED (v1.18.0, R1): the frame half of `still_accepted` is the
    // stage-vs-absence law (`fit::ships`), one verdict for the whole shrunk
    // set, and a tie goes to absence — the `frame, frame` this test passed
    // refused every zone. The frame half is not under test here (the
    // leave-one-out baselines are), so it is handed a set that earned
    // 2·FIT_QUANT over its absence.
    let refused = refuse_shrunk_zones(
        &source, &mut report, &mut accepted, &mut originals, 0, &pixels, &tgt_px, 0.25,
        frame + 2.0 * fit::FIT_QUANT, frame,
    );
    for path in &paths {
        path.remove();
    }
    assert!(refused, "the darkened right half must be refused");
    assert_eq!(
        accepted.iter().map(|z| z.label.as_str()).collect::<Vec<_>>(),
        vec!["left"],
        "the lifted left half still helps its zone and stays"
    );
    assert_eq!(originals.len(), 1, "the k=1 controls follow the survivors in step");
    assert!((originals[0].exposure_ev - 0.40).abs() < 1e-6, "and they are the LEFT zone's");
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_SHRUNK_REFUSED)
        .expect("the refusal is a typed note");
    assert!(note.args.iter().any(|(k, v)| *k == "label" && v == "right"), "{note:?}");
}

/// R19: the SKIP/floor split itself, pinned — setting the skip back to
/// the acceptance floor would silently re-decline the (0.012, 0.02]
/// band untried (the regression this split exists to prevent).
#[test]
fn the_skip_line_sits_below_the_acceptance_floor() {
    assert!(zone_skips(0.009, 0.1), "the observed matched domain skips");
    assert!(zone_skips(0.012, 0.1), "the domain ceiling itself skips");
    assert!(!zone_skips(0.0121, 0.1), "just above the ceiling is attempted");
    assert!(!zone_skips(0.016, 0.1), "the between-yardsticks band is attempted");
    assert!(!zone_skips(0.009, 0.5), "a matched score a stop apart is attempted");
}

/// R18: a zone that already matches the target is LEFT ALONE with an
/// honest note — not "corrected", not reported as a dropped
/// improvement (the murk-era pair's sky read 0.012, got dialled, and
/// the "dropped: needs ≤ 50%" outcome line was mistaken for a
/// discarded win three rounds running). Identical frames: both zones
/// match, nothing attaches, the raster is reclaimed.
#[test]
fn an_already_matched_zone_is_left_alone_and_says_so() {
    let (src, _tgt, sky_mask) = zoned_pair();
    let mask_path = fixture_mask_path("zoned-matched-mask");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&src, &src);
    attach_zones(&src, &src, &mut report, &sky_mask, &sky_mask, &mask_path);
    assert!(
        report.recipe.masks.is_empty(),
        "nothing to correct on an identical pair: {}",
        report.recipe.rationale
    );
    assert!(
        report.recipe.rationale.contains("already matches the target"),
        "the honest note must replace the misleading drop line: {}",
        report.recipe.rationale
    );
    assert!(
        !report.recipe.rationale.contains("correction dropped"),
        "no drop line on a matched zone: {}",
        report.recipe.rationale
    );
    assert!(!mask_path.path().exists(), "no zone kept the raster — it must be reclaimed");
}

/// R33 §H. The deep arm adjusts one global dial and re-derives the
/// report through `fit::rescore_report`, which rebuilds the GLOBAL solve's
/// account field by field off `SolveFacts`. Every note this module writes
/// — the zone verdicts, the evidence withholdings, the boundary gate, the
/// quality gate, the attachment line, the XMP loss — had no field to ride
/// on and was dropped on the floor, so a `--zoned` fit that went through
/// `--deep` reached the user with its masks still rendering and its whole
/// local half missing from the rationale.
///
/// The carrying rule is a denylist (`rationale::GLOBAL_SOLVE_KEYS`), so
/// this pin does not name the producer keys it expects: it asserts that
/// EVERY note the zoned pass added to the global report is still there
/// afterwards, in the same order, whatever those notes turn out to be.
#[test]
fn a_rescored_zoned_report_still_carries_every_note_its_producers_wrote() {
    let (src, tgt, sky_mask) = zoned_pair();
    let mask_path = fixture_mask_path("rescore-carry-mask");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&src, &tgt);
    let global_only: Vec<&'static str> = report.notes.iter().map(|n| n.key).collect();
    attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
    let produced: Vec<&'static str> = report
        .notes
        .iter()
        .map(|n| n.key)
        .filter(|k| !global_only.contains(k))
        .collect();
    assert!(
        produced.len() >= 4,
        "premise: the zoned pass wrote several notes of its own, got {}",
        produced.len()
    );

    // The deep arm's own move: one global dial, nothing local touched.
    let mut adjusted = report.recipe.clone();
    adjusted.saturation += 4.0;
    adjusted.clamp();
    let rescored = fit::rescore_report(
        &src,
        &tgt,
        &adjusted,
        &crate::recipe::EditRecipe::default(),
        report.err_before,
        &report.notes,
    );

    let after: Vec<&'static str> = rescored.notes.iter().map(|n| n.key).collect();
    let surviving: Vec<&'static str> =
        after.iter().copied().filter(|k| produced.contains(k)).collect();
    assert_eq!(
        surviving, produced,
        "a rescored zoned report must carry every producer note, in order: {}",
        rescored.recipe.rationale
    );
    // …and each one reaches the persisted rationale too, not just the vec.
    for note in rescored.notes.iter().filter(|n| produced.contains(&n.key)) {
        let rendered = crate::rationale::render_one(note);
        assert!(
            rescored.recipe.rationale.contains(rendered.trim()),
            "carried note missing from the rationale string: {rendered}"
        );
    }
    // The three the rescore DROPS on purpose stay dropped: carrying is
    // not a licence for a stale global claim to ride back in.
    for dropped in [
        crate::rationale::keys::FIT_NOTE_REGRESSED,
        crate::rationale::keys::FIT_NOTE_JOINT_REGRESSED,
        crate::rationale::keys::FIT_NOTE_SAT_REDUCED,
    ] {
        assert!(
            !after.contains(&dropped) || report.notes.iter().all(|n| n.key != dropped),
            "a deliberately dropped global note came back through the carry"
        );
    }
}

#[test]
fn zoned_orchestration_attaches_the_sky_mask_and_improves_the_zone() {
    let (src, tgt, sky_mask) = zoned_pair();
    let mask_path = fixture_mask_path("zoned-orch-mask");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&src, &tgt);
    let err_global = report.err_after;
    attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
    // Until 2026-10-21 this pinned the sky's colour WITHHELD (one-sided hue
    // on a region whose pixels pair); the paired vouch now ships the gold.
    assert!(
        report.recipe.masks.iter().any(|m| {
            m.role == MaskRole::ZoneSky
                && m.color_gains.is_some_and(|g| !gains_withheld(Some(g)) && g[0] > g[2])
        }),
        "the sky zone must attach with its warm recolour: {}",
        report.recipe.rationale
    );
    assert!(
        report.recipe.masks.iter().any(|m| m.role == MaskRole::ZoneLand && m.inverted),
        "the independently supported land correction must still attach: {}",
        report.recipe.rationale
    );
    // THE CARRIER. Both zones ride Lightroom's own Select Sky, over ONE
    // shared claimed PNG, prompted at that alpha's own centre of mass —
    // which is what lets the sidecar carry corrections classic XMP had to
    // skip as raster masks (`xmp::masks_xml`, and
    // `a_reverse_fit_zone_rides_out_as_lightrooms_own_select_sky`).
    let zone_geoms = report
        .recipe
        .masks
        .iter()
        .filter(|m| m.role.is_zone())
        .map(|m| (m.role, m.inverted, m.mask.clone()))
        .collect::<Vec<_>>();
    assert!(!zone_geoms.is_empty(), "premise: a zone attached");
    let want = raster_centroid(&sky_mask);
    for (role, adj_inverted, g) in &zone_geoms {
        let crate::recipe::MaskGeometry::AiMask {
            subtype, ref_x, ref_y, inverted, blend_mode, value, mask_version, raster,
            provenance, gesture, ..
        } = g
        else {
            panic!("{role:?} must ride a Select Sky component, not {g:?}");
        };
        assert_eq!(
            (*subtype, *blend_mode, *value, *mask_version),
            (2, 0, 1.0, 1),
            "{role:?}: the wire format Lightroom reads"
        );
        assert_eq!((*ref_x, *ref_y), want, "{role:?}: prompted at the alpha's centre of mass");
        assert!(
            !*inverted,
            "{role:?}: the inversion has ONE home and it is the adjustment's flag"
        );
        assert_eq!(
            *adj_inverted,
            *role == MaskRole::ZoneLand,
            "{role:?}: …which the land zone sets and the sky zone does not"
        );
        assert!(
            provenance.is_empty() && gesture.is_empty(),
            "{role:?}: the fit mints neither Adobe's digests nor a refinement stroke"
        );
        assert_eq!(
            raster.as_deref(),
            Some(mask_path.path().to_string_lossy().as_ref()),
            "{role:?}: ONE claimed raster, the same file both zones always shared"
        );
    }
    // The zoned gate judges each ZONE; frame-global error is only bounded
    // (the insurance tolerance, once per attached zone), never required
    // to improve.
    let bound = err_global
        + fit::FIT_QUANT * report.recipe.masks.len() as f32;
    assert!(
        report.err_after <= bound,
        "zoned err {} exceeded the insurance bound {bound}",
        report.err_after
    );
    // Until 2026-10-21 this pinned the partial sky hue REFUSAL of a grey-blue
    // sky's recolour. The fixture's sky is neutral grey now — no source hue
    // band, so the hue guard has nothing to withhold and nothing to vouch —
    // and the warm recolour asserted above ships under no refusal sentence.
    assert!(
        !report.notes.iter().any(|note| {
            is_colour_refusal(note.key)
                && note.args.iter().any(|(name, value)| *name == "label" && value == "sky")
        }),
        "the shipped sky recolour may not also be called refused: {}",
        report.recipe.rationale
    );
    // The XMP honesty note. It used to read 「the Lightroom sidecar carries
    // the global fit only (classic XMP cannot hold raster masks)」, which
    // was true while the zones were `MaskGeometry::Bitmap`. They ride out
    // as Lightroom's own Select Sky now, so the honest sentence is the
    // OTHER half of the same fact: the intent reaches the sidecar and the
    // alpha rendered here is ours, not Adobe's.
    assert!(
        report.recipe.rationale.contains("rides out as Lightroom's own Select Sky mask")
            && report.recipe.rationale.contains("not Adobe's"),
        "rationale must carry the XMP honesty note: {}",
        report.recipe.rationale
    );
    assert!(
        !report.recipe.rationale.contains("global fit only"),
        "…and must no longer claim the sidecar carries the global fit ALONE: {}",
        report.recipe.rationale
    );
    // R23-6 A-4: confidence must NOT be the frame-global look error's
    // verdict any more. The old line was
    // `confidence = (1 - zoned_err * 6).clamp(0.25, 0.95)`, which on this
    // fixture reports a number derived from a metric the module's own
    // attach_one_zone doc proves cannot see the zone. It now comes from
    // the accepted zones, and says so.
    assert!(
        report.recipe.rationale.contains("Confidence for this fit comes from"),
        "the zoned fit must say where its confidence came from: {}",
        report.recipe.rationale
    );
    let frame_verdict = fit::clamp_confidence(1.0 - report.err_after * 6.0);
    assert!(
        report.recipe.confidence != frame_verdict
            || (report.err_after - 0.0).abs() < 1e-6,
        "confidence still reads as the frame-global formula ({} vs {frame_verdict})",
        report.recipe.confidence
    );
    mask_path.remove();
}

/// THE ASSERTION THAT WAS MISSING: a zoned fit's corrections survive the
/// sidecar and come back rendering THE SAME PIXELS, byte for byte.
///
/// The inversion has two homes across that border and exactly one net.
/// Leaving here it is `LocalAdjustment::inverted` — the land zone's, set
/// by `land_attachment`, which documents the choice. Coming back it is the
/// `Mask/Image` component's own bit, because that is where Lightroom's own
/// files put it and `parse_one_correction` deliberately does not spell it
/// twice. `LocalAdjustment::net_inverted` is the same fact either way. If
/// either side ever applied BOTH bits, or neither, the land zone would
/// come back covering the sky it excludes — and this comparison says so in
/// the only currency that matters, the developed frame.
///
/// TWO THINGS ARE NORMALISED, both named rather than assumed:
///
///  * THE ALPHA. A `Mask/Image` carries the INTENT and Lightroom rebuilds
///    its own sky from it, so `raster` is `None` on import BY DESIGN
///    (`MaskLossReason::AiMaskRecomputed` is that disclosure). Re-pointing
///    it at the same claimed PNG substitutes the alpha this engine would
///    recompute, which is what makes the two renders comparable at all.
///    The assertion just above the re-pointing pins the other half: until
///    it resolves, the mask applies NOTHING — not, in the inverted case, a
///    whole-frame edit.
///
///  * THE ÷100 DIALS. The writer emits `v / 100.0` and the reader returns
///    `(v * 100.0 * 10_000).round() / 10_000` (`xmp.rs`'s `scaled`), so a
///    fitted saturation of 11.73456 comes back 11.7346 — a real 5e-5 step,
///    small enough to move a byte for reasons that have nothing to do with
///    masks. `crs:LocalExposure2012` is `v / 4.0` out and `× 4.0` back with
///    no rounding, and both are exact in binary, so the comparison recipe
///    keeps the FIT's geometry, role and inversion and carries one
///    exposure. The writer's numeric fidelity has its own tests in `xmp`;
///    this one is about the mask.
///
/// MUTATION: make `ai_mask_xml` write the geometry's raw bit instead of
/// the net, or drop `own_inverted` from the AI arm of `mask_weight`, and
/// the land zone's two renders stop matching.
#[test]
fn a_zone_survives_the_sidecar_round_trip_byte_for_byte() {
    let (src, tgt, sky_mask) = zoned_pair();
    let mask_path = fixture_mask_path("zoned-roundtrip-mask");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&src, &tgt);
    attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
    // The fit's own masks, carrying one exactly-representable dial — see
    // the ÷100 note above.
    let zones: Vec<crate::recipe::LocalAdjustment> = report
        .recipe
        .masks
        .iter()
        .filter(|m| m.role.is_zone())
        .map(|m| crate::recipe::LocalAdjustment {
            mask: m.mask.clone(),
            role: m.role,
            inverted: m.inverted,
            name: m.name.clone(),
            exposure_ev: -0.75,
            ..Default::default()
        })
        .collect();
    assert_eq!(zones.len(), 2, "premise: the fit attached both zones");
    assert!(
        zones.iter().any(|m| m.role == MaskRole::ZoneLand && m.inverted),
        "premise: one of them is the INVERTED land zone"
    );
    // Every render below is of a recipe whose ONLY content is the mask, so
    // nothing a global control does on the round trip can be mistaken for
    // something the mask did.
    let render = |m: &crate::recipe::LocalAdjustment| {
        crate::render::develop_preview(
            &src,
            &crate::recipe::EditRecipe { masks: vec![m.clone()], ..Default::default() },
        )
        .to_rgb8()
        .into_raw()
    };
    let plain = crate::render::develop_preview(&src, &crate::recipe::EditRecipe::default())
        .to_rgb8()
        .into_raw();

    for m in &zones {
        let was = render(m);
        assert_ne!(plain, was, "{:?}: premise — the zone changes the render", m.role);
        let doc = crate::xmp::recipe_to_xmp(&crate::recipe::EditRecipe {
            masks: vec![m.clone()],
            ..Default::default()
        });
        let mut back = crate::xmp::xmp_to_recipe(&doc);
        assert_eq!(
            back.masks.len(),
            1,
            "{:?}: the sidecar must keep the correction: {:?}",
            m.role,
            back.masks
        );
        assert_eq!(
            render(&back.masks[0]),
            plain,
            "{:?}: an AI mask whose alpha has not resolved must apply NOTHING",
            m.role
        );
        // The SLOT, not `geometry_raster_path_mut`: that helper repoints a
        // raster a mask already HAS, and an imported AI mask has none —
        // this is standing in for the segmenter that would fill it.
        let MaskGeometry::AiMask { raster, .. } = &mut back.masks[0].mask else {
            panic!("{:?}: expected the Select Sky component back", m.role);
        };
        *raster = Some(mask_path.path().to_string_lossy().into_owned());
        assert_eq!(
            render(&back.masks[0]),
            was,
            "{:?}: the round trip must not move a pixel — one net, however many homes \
                 it has had",
            m.role
        );
    }
    mask_path.remove();
}

/// R34 §D5. A ZONE whose texture was re-synthesised while its layout held:
/// the same slow base ramp, a known affine contrast map, and an
/// independent per-pixel hash on each side of the zone. The bottom half is
/// byte-identical on both sides, so only the zone is under test.
fn resynthesised_zone(target: bool) -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let hash = |i: u32, seed: u32| {
        let mut v = i.wrapping_mul(747796405).wrapping_add(seed.wrapping_mul(2891336453));
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 10_000) as f32 / 10_000.0 - 0.5
    };
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = if y < h / 2 {
            let seed = if target { 9_999 } else { 1 };
            let base = 0.20 + 0.55 * (x as f32 / (w - 1) as f32)
                + 0.60 * hash(y * w + x, seed);
            let base = base.clamp(0.0, 1.0);
            if target { (0.5 + 1.35 * (base - 0.5)).clamp(0.0, 1.0) } else { base }
        } else {
            0.45 + 0.10 * (x as f32 / (w - 1) as f32)
        };
        image::Rgb([(v * 255.0).round() as u8; 3])
    }))
}

/// R34 §D5: MODE governs the control set, SCALE governs the estimator.
///
/// The zone above is a Full zone at either divergence — the mode line is
/// `DIVERGENCE_ZONE` (0.65) and both readings here are under it — so the
/// ONLY thing that changes between the two runs is which estimator solved
/// its tone. At Pixel scale the paired regression measures the regression
/// of one noise draw on another and errors-in-variables shrinks the slope
/// it recovers; at Cell scale the zone's own luma distribution carries the
/// map exactly. This is R33 §D's frame-level measurement, at zone level.
///
/// 2026-10-22, the claim EXTENDED: the dilution guard
/// (`ZONE_PAIRED_SPREAD_FLOOR`) now catches that shrunken slope at Pixel
/// scale too and hands the zone to the population estimator, so a zone
/// whose pixels do not pair solves from its own population whether the D
/// line or the guard sent it there. The hash is 0.60 (was 0.40) so the
/// guard fires well under its floor: the paired map reproduces 0.58 of the
/// target's p10–p90 (0.430 vs 0.739), and the runs land at 0.2649 (Pixel)
/// and 0.2552 (Cell) against a target spread of 0.2659, −0.4% and −4.0%,
/// where the map the guard discarded would have missed by more than 40%.
/// The spread bound is 6%: the bound sits 1 percentage point above the
/// measurement. Measured Cell −4.0% before the distribution branch's
/// spline refinement (`fit::refine_tone_sliders_on`), −5.04% after it
/// (0.2525 vs 0.2659).
/// Measured on this fixture: hash 0.40 reads ratio 0.77, 0.50 reads 0.66,
/// 0.70 reads 0.50; at 0.35 and below the guard stays quiet but the
/// Pixel-scale zone is dropped outright and renders the untouched source,
/// so the comparison would no longer see a paired solve.
#[test]
fn a_full_zone_whose_pixels_do_not_pair_solves_its_tone_from_its_own_population() {
    let (src, tgt) = (resynthesised_zone(false), resynthesised_zone(true));
    let mask = GrayImage::from_fn(192, 128, |_, y| {
        image::Luma([if y < 64 { 255u8 } else { 0 }])
    });
    let path = fixture_mask_path("zone-pairing-scale");
    mask.save(path.path()).unwrap();
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let t_px = fit::pixels_of(&t_img);
    let sw = mask_weights(&mask, s_img.width(), s_img.height());
    let tw = mask_weights(&mask, t_img.width(), t_img.height());
    let spread = |px: &[[f32; 3]], weights: &[f32]| -> f32 {
        let mass = weights.iter().map(|w| *w as f64).sum::<f64>().max(1e-9);
        let mean = px
            .iter()
            .zip(weights)
            .map(|(p, w)| fit::luma601(p) as f64 * *w as f64)
            .sum::<f64>()
            / mass;
        (px.iter()
            .zip(weights)
            .map(|(p, w)| *w as f64 * (fit::luma601(p) as f64 - mean).powi(2))
            .sum::<f64>()
            / mass)
            .sqrt() as f32
    };
    let wanted = spread(&t_px, &tw);
    // The rendered spread, and the dilution guard's squeeze ratio if it fired.
    let solve = |d: f32| -> (f32, Option<f32>) {
        let mut report = neutral_report(&src, &tgt);
        let attachment = semantic_attachment(sw.clone(), tw.clone(), &path);
        let mut frame_err = report.err_after;
        attach_one_zone(
            &s_img,
            &t_px,
            &mut report,
            &mut frame_err,
            &attachment,
            divergence(d),
            None,
        );
        let rendered = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        let diluted = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::ZONE_TONE_PAIRING_DILUTED)
            .map(|n| {
                n.args
                    .iter()
                    .find(|(k, _)| *k == "ratio")
                    .map(|(_, v)| v.parse::<f32>().expect("a numeric ratio"))
                    .expect("the guard discloses its ratio")
            });
        (spread(&rendered, &sw), diluted)
    };
    let ((paired, ratio), (population, cell_diluted)) = (solve(0.20), solve(0.50));
    let ratio = ratio.unwrap_or_else(|| {
        panic!(
            "the Pixel-scale run's paired map must be caught by the dilution guard: \
                 target {wanted:.4}, pixel scale {paired:.4}"
        )
    });
    assert!(ratio < super::limits::ZONE_PAIRED_SPREAD_FLOOR, "the guard fired under its floor: {ratio}");
    assert!(cell_diluted.is_none(), "a cell-scale zone has no paired map to discard");
    for (scale, landed) in [("pixel", paired), ("cell", population)] {
        assert!(
            (landed / wanted - 1.0).abs() < 1.0 - ratio,
            "the population arm must land closer to the target's own spread than the \
                 discarded paired map would have: target {wanted:.4}, {scale} scale \
                 {landed:.4}, paired ratio {ratio}"
        );
        assert!(
            (landed / wanted - 1.0).abs() <= 0.06,
            "…within 6% of it: target {wanted:.4}, {scale} scale {landed:.4}"
        );
    }
    // …and the scale is DISCLOSED where it changed the estimator, and only
    // there: a Pixel-scale zone is what every zone did before R34.
    let scale_note = |d: f32| {
        let mut report = neutral_report(&src, &tgt);
        let attachment = semantic_attachment(sw.clone(), tw.clone(), &path);
        let mut frame_err = report.err_after;
        attach_one_zone(
            &s_img, &t_px, &mut report, &mut frame_err, &attachment, divergence(d), None,
        );
        report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::ZONE_PAIRING_SCALE)
    };
    assert!(scale_note(0.50), "a cell-scale zone says which estimator solved it");
    assert!(!scale_note(0.20), "a pixel-scale zone says nothing new");
    path.remove();
}

#[test]
fn synthetic_zone_survives_luminance_with_one_sided_hue_refusal() {
    let (w, h) = (64u32, 64u32);
    let build = |target: bool| DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = 0.30 + 0.30 * x as f32 / (w - 1) as f32;
        let p = if y < h / 2 {
            if target { [v * 1.5, v * 1.5, v * 1.5] } else { [v * 0.72, v * 0.86, v] }
        } else { [0.45, 0.38, 0.28] };
        image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
    }));
    let src = build(false);
    let tgt = build(true);
    let sp = fit::pixels_of(&src);
    let tp = fit::pixels_of(&tgt);
    let evidence = fit::evidence_model_for(&sp, &tp, w, h);
    let blue = evidence.hue.iter().find(|r| r.label == "Blue").expect("blue evidence range");
    assert!(blue.source_populated && !blue.target_populated, "synthetic fixture must have one-sided hue evidence: {blue:?}");
    assert!(evidence.luma.iter().any(|r| r.weight > 0.0), "synthetic fixture must retain two-sided luma evidence");
    let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
    let path = fixture_mask_path("synthetic-zone-survival");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let t_px = fit::pixels_of(&t_img);
    let sw = mask_weights(&mask, s_img.width(), s_img.height());
    let tw = mask_weights(&mask, t_img.width(), t_img.height());
    let attachment = semantic_attachment(sw, tw, &path);
    let mut frame_err = report.err_after;
    let accepted = attach_one_zone(
        &s_img,
        &t_px,
        &mut report,
        &mut frame_err,
        &attachment,
        measure_zone_divergence(&src, &tgt, &crate::recipe::EditRecipe::default(), &mask)
            .sky
            .divergence,
        None,
    );
    assert!(accepted.is_some(), "supported luminance must keep the synthetic sky zone: {}", report.recipe.rationale);
    // The hue BAND evidence is one-sided (the neutral target has no blue
    // population), but the region's cells are not: the move the band could
    // not verify takes a blue sky to the target's neutral, and read along
    // chromaticity (`fit_cells::MoveAxis::Colour`) every cell says so. This
    // test used to pin `color_gains == [1, 1, 1]` with a refusal sentence;
    // that refusal came from the full-RGB cosine, where the target's x1.5
    // brightening outweighed the colour move it was asked about.
    let sky = report.recipe.masks.last().expect("accepted synthetic sky mask");
    let gains = sky.color_gains.expect("the verified neutralising move ships");
    assert!(
        gains[0] > 1.0 && gains[2] < 1.0,
        "a blue sky taken toward a neutral target warms: {gains:?}"
    );
    assert!(
        report.notes.iter().any(|n| matches!(
            n.key,
            crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_CELLS
                | crate::rationale::keys::ZONE_COLOUR_VOUCHED_AT_SHARE
        )),
        "the band's refusal overturned by the cells must be disclosed: {}",
        report.recipe.rationale
    );
    path.remove();
}

/// The mirror of the test above, and the branch nothing pinned: a
/// structurally unsupported region's TONE verdict must be named by luma
/// evidence, and the recipe must say the same thing as the sentence. A
/// hand mutation that deleted the tone-zeroing left the whole library
/// green, because every existing guard measured the COLOUR half of the
/// same split.
///
/// Since R36 the verdict has two arms, and this pins both on one
/// achromatic builder: a ONE-WAY target (every sky cell asks to be
/// brighter) is refused in full on the diverged share but agrees on
/// direction, so the share the cells vouch ships and the sentence still
/// names the luma ranges the pixel reading withheld; a TWO-WAY target
/// (the right half asks to be darker) fails `aligned`, is never asked for
/// less of one move, keeps the refusal sentence and leaves every tone
/// dial at zero.
#[test]
fn synthetic_zone_tone_verdict_is_named_by_luma_evidence_and_matches_the_recipe() {
    let (w, h) = (96u32, 96u32);
    // ACHROMATIC everywhere: `fit::evidence_hue_band` returns None below
    // chroma 0.06, so the colour branch cannot fire and the notes under
    // test are the only ones that can appear.
    //
    // The sky half is STRUCTURALLY divergent — a smooth ramp against
    // hard stripes — because that is the only mechanism that can withhold
    // a luma range: bins are rank-paired (`fit::evidence_model_for`), so
    // a source bin is never target-empty at equal pixel counts, and the
    // withholding clause that remains is `!spatial_supported`. The ground
    // half is byte-identical on both sides, so its cells stay supported.
    let build = |target: bool, two_ways: bool| {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v: f32 = if y < h / 2 {
                if target {
                    let bright = (y / 2) % 2 == 0;
                    if two_ways && x >= w / 2 {
                        if bright { 0.15 } else { 0.05 }
                    } else if bright {
                        0.85
                    } else {
                        0.55
                    }
                } else {
                    0.35 + 0.20 * x as f32 / (w - 1) as f32
                }
            } else {
                0.18 + 0.12 * (((x / 8) + (y / 8)) % 2) as f32
            };
            image::Rgb([(v.clamp(0.0, 1.0) * 255.0).round() as u8; 3])
        }))
    };
    let tone_dials = |m: &LocalAdjustment| {
        [m.exposure_ev, m.contrast, m.highlights, m.shadows, m.whites, m.blacks]
    };
    let named = |note: &crate::rationale::Note| {
        note.args.iter().any(|(k, v)| *k == "luma_ranges" && !v.is_empty() && v != "none")
    };
    let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
    for two_ways in [false, true] {
        let src = build(false, two_ways);
        let tgt = build(true, two_ways);
        let sp = fit::pixels_of(&src);
        let tp = fit::pixels_of(&tgt);
        let evidence = fit::evidence_model_for(&sp, &tp, w, h);
        let unsupported = evidence.spatial_supported.iter().filter(|&&s| !s).count();
        assert!(
            sp.iter().chain(tp.iter()).all(|p| fit::evidence_hue_band(p).is_none()),
            "synthetic fixture must be achromatic so the colour branch cannot fire"
        );
        assert!(unsupported > 0, "synthetic fixture must contain structurally unsupported pixels");
        let path = fixture_mask_path(if two_ways {
            "synthetic-zone-tone-two-ways"
        } else {
            "synthetic-zone-tone-one-way"
        });
        mask.save(path.path()).unwrap();
        let mut report = neutral_report(&src, &tgt);
        let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
        let t_px = fit::pixels_of(&t_img);
        let sw = mask_weights(&mask, s_img.width(), s_img.height());
        let tw = mask_weights(&mask, t_img.width(), t_img.height());
        let attachment = semantic_attachment(sw, tw, &path);
        let mut frame_err = report.err_after;
        attach_one_zone(
            &s_img,
            &t_px,
            &mut report,
            &mut frame_err,
            &attachment,
            measure_zone_divergence(&src, &tgt, &crate::recipe::EditRecipe::default(), &mask)
                .sky
                .divergence,
            None,
        );
        eprintln!(
            "TONE_FIXTURE two_ways={two_ways} unsupported={unsupported}: {}",
            report.recipe.rationale
        );
        // A refused tone move with nothing else to ship leaves NO zone
        // mask at all ("every control ... solved to neutral"), which is the
        // strongest form of "every dial at zero".
        let sky = report.recipe.masks.last();
        if two_ways {
            let note = report
                .notes
                .iter()
                .find(|n| is_tone_refusal(n.key))
                .unwrap_or_else(|| panic!("the zoned tone refusal was silent: {}", report.recipe.rationale));
            assert!(named(note), "the refused luma range must be named: {note:?}");
            if let Some(sky) = sky {
                assert_eq!(tone_dials(sky), [0.0; 6], "a refused tone move leaves every dial at zero");
            }
            assert!(
                !report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_TONE_VOUCHED_AT_SHARE),
                "cells asking for opposite moves are not asking for a smaller one: {}",
                report.recipe.rationale
            );
        } else {
            // DEF-5: the cells vote at PRESENCE. Read at the estimation
            // weights (which withheld every sky luma range but a few) part
            // of the region was asked by a handful of pixels and the full
            // move was refused for its size, shipping a share; read at
            // presence the whole brightening is vouched (0.992 closer,
            // 0.008 away, 0.992 aligned), so the WHOLE move ships on region
            // evidence, still naming the luma ranges the pixel reading
            // withholds.
            let sky = sky.expect("the synthetic sky zone was pushed");
            let note = report
                .notes
                .iter()
                .find(|n| n.key == crate::rationale::keys::ZONE_TONE_VOUCHED_BY_CELLS)
                .unwrap_or_else(|| panic!("the region voucher was silent: {}", report.recipe.rationale));
            assert!(named(note), "the withheld luma range must still be named: {note:?}");
            assert!(
                !report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_TONE_VOUCHED_AT_SHARE),
                "a move vouched whole is not also shipped at a share: {}",
                report.recipe.rationale
            );
            assert!(tone_dials(sky).iter().any(|d| *d != 0.0), "a vouched move must move a dial: {sky:?}");
            assert!(!report.notes.iter().any(|n| is_tone_refusal(n.key)), "a shipped move is not also a refusal");
        }
        assert!(
            !report.notes.iter().any(|n| is_colour_refusal(n.key)),
            "an achromatic pair must not claim a refused hue band: {}",
            report.recipe.rationale
        );
        path.remove();
    }
}

/// Step-7b conservation, zoned: an IDENTITY field (everything
/// corresponds in place at full confidence) leaves every zone verdict
/// byte-identical, and a ZERO-confidence field abstains wholesale — the
/// field may refuse to help, never starve or drop a zone. The latter
/// also pins the share GATE never reading the confidence (supervisor
/// mutation M-7b-D: compose the gate with confidence and the zero-field
/// run drops the zones this asserts equal).
#[test]
fn an_identity_or_abstaining_field_leaves_the_zone_verdicts_unchanged() {
    // The synthetic zoned fixture, not the calibration corpus: its two
    // renditions share one geometry by construction, which is what makes
    // the identity law EXACT (the corpus target is two rows taller than
    // its source, and on mismatched geometry an identity field genuinely
    // row-aligns the pairing — a real, wanted change documented on
    // `correspondence_for_pair`, not the law under test here).
    let (source, target, sky_mask) = zoned_pair();
    let fingerprint = |masks: &[crate::recipe::LocalAdjustment]| -> String {
        masks
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.mask = crate::recipe::MaskGeometry::Bitmap { path: String::new() };
                serde_json::to_string(&m).unwrap()
            })
            .collect::<Vec<_>>()
            .join("|")
    };
    let run = |field: Option<crate::correspond::CorrespondenceField>| -> String {
        let mask_path = fixture_mask_path("corr-conserve");
        sky_mask.save(mask_path.path()).unwrap();
        let mut report = fit::fit_recipe(&source, &target);
        if let Some(f) = field {
            let (s_img, t_img) = fit::analysis_pair(&source, &target);
            report.correspondence = Some(fit::correspondence_for_pair(
                &f,
                &fit::pixels_of(&t_img),
                (s_img.width(), s_img.height()),
                (t_img.width(), t_img.height()),
            ));
        }
        attach_zones(&source, &target, &mut report, &sky_mask, &sky_mask, &mask_path);
        mask_path.remove();
        assert!(
            !report.recipe.masks.is_empty(),
            "premise: zones attach on the calibration pair: {}",
            report.recipe.rationale
        );
        fingerprint(&report.recipe.masks)
    };
    let plain = run(None);
    assert_eq!(
        plain,
        run(Some(crate::correspond::identity_test_field())),
        "an identity field must change no zone verdict"
    );
    let mut zero = crate::correspond::identity_test_field();
    zero.confidence = vec![0.0; zero.confidence.len()];
    assert_eq!(
        plain,
        run(Some(zero)),
        "a zero-confidence field must abstain wholesale, never starve a zone"
    );
}

/// One luma bin, two populations, built at the analysis size so no
/// thumbnail resampling blends its edges. The top two thirds are REPLACED
/// content whose source ramp lives entirely in luma bin 6 (0.353-0.412);
/// the ground is a near-flat 0.34-0.40 ramp -- identical on both sides,
/// then +0.08 brighter on the target -- whose upper part shares that bin.
/// Frame-wide the bin keeps well under 35% structural survival and is
/// withheld; the ground alone keeps all of it.
/// A zone whose own population lives BETWEEN the engine's tone knots. Its
/// luma spans 0.298-0.447, which is evidence bins 5 to 7, while the eight
/// knots (`render::TONE_KNOTS_X`) fall in bins 0, 1, 4, 8, 11, 13, 15 and
/// 16 — and the robust map's own span, widened by the 1/32 the knot reader
/// allows either side, still reaches neither 0.25 below nor 0.50 above.
/// The interquartile spread measures 0.059 against the identifiability
/// floor of 0.05, so the tone solve RUNS and then finds nothing to stand
/// on, which is the case this fixture exists to reach.
/// `shifted` warms the zone without leaving its band, so a colour move
/// attaches and the tone move is the only thing left unanswered.
fn unsupported_knot_fixture(shifted: bool) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(96, 96, |x, y| {
        // The WHOLE frame stays inside the band, the zone included. A
        // brighter surround would put its own values into the zone through
        // the analysis resample of the mask edge, and a few hundred blended
        // pixels are all it takes to clear `fit::SUPPORT_MIN_PIXELS` at a
        // knot the zone itself never reaches.
        let base = (76 + ((x * 5 + y * 3) % 39)) as f32;
        if shifted && y >= 48 {
            image::Rgb([(base * 1.18).min(255.0) as u8, base as u8, (base * 0.86) as u8])
        } else {
            image::Rgb([base as u8; 3])
        }
    }))
}

/// v1.2.4: a zone that cannot support a tone solve SAYS SO.
///
/// `fit::fit_tone_sliders_supported` answers an under-determined knot
/// system with a neutral solve, and that is the right answer: one
/// supported knot cannot separate exposure from contrast, and a guess
/// would put a dial on the recipe that no pixel of the zone asked for.
/// What it did not do was say anything. The zone attached with its colour
/// move, its tone residual stayed on the frame, and nothing in the report
/// distinguished "this zone wanted no tone move" from "this zone could not
/// be asked" — the same silence the free-mask refusals were built to end.
///
/// MUTATION (2026-09-02): delete the `if let Some(knots)` push that
/// follows the solve — the note disappears and this test fails.
#[test]
fn a_zone_that_cannot_support_a_tone_solve_says_so() {
    let (w, h) = (96u32, 96u32);
    let src = unsupported_knot_fixture(false);
    let tgt = unsupported_knot_fixture(true);
    let mask =
        GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= h / 2 { 255u8 } else { 0 }]));
    let path = fixture_mask_path("zone-unsupported-knots");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    pretend_full_support(&mut report.evidence);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let t_px = fit::pixels_of(&t_img);
    let sw = mask_weights(&mask, s_img.width(), s_img.height());
    let tw = mask_weights(&mask, t_img.width(), t_img.height());
    let attachment = semantic_attachment(sw, tw, &path);
    let mut frame_err = report.err_after;
    let accepted = attach_one_zone(
        &s_img,
        &t_px,
        &mut report,
        &mut frame_err,
        &attachment,
        divergence(0.0),
        None,
    );
    path.remove();
    assert!(
        accepted.is_some(),
        "premise: the colour move must attach: {}",
        report.recipe.rationale,
    );
    let note = report
        .notes
        .iter()
        .find(|note| note.key == crate::rationale::keys::ZONE_TONE_UNSUPPORTED)
        .unwrap_or_else(|| panic!("no disclosure: {}", report.recipe.rationale));
    assert!(
        note.args.iter().any(|(key, value)| *key == "knots" && value == "0"),
        "the disclosure carries the count it measured: {:?}",
        note.args,
    );
}

/// A 16x16 grey pair whose bottom half is the zone: the source a luma ramp
/// 0.35–0.65, the target the same tones spread 1.6x and lifted to 0.58, plus a
/// ±`noise` checkerboard. The checkerboard puts both signs into every
/// source level, so the paired conditional median stays on the 1.6x line
/// while the target's own spread widens — the loosely paired zone of
/// [`ZONE_PAIRED_SPREAD_FLOOR`] in miniature. The top half is identical on
/// both sides.
fn dilution_pair(noise: f32) -> (DynamicImage, DynamicImage, GrayImage) {
    let (w, h) = (16u32, 16u32);
    let source_luma = |x: u32, y: u32| 0.35 + 0.30 * ((y - 8) * w + x) as f32 / 127.0;
    let build = |target: bool| {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v = if y < 8 {
                0.5
            } else if target {
                let sign = if (x / 4 + y) % 2 == 0 { 1.0 } else { -1.0 };
                (0.58 + 1.6 * (source_luma(x, y) - 0.5) + sign * noise).clamp(0.02, 0.98)
            } else {
                source_luma(x, y)
            };
            image::Rgb([(v * 255.0).round() as u8; 3])
        }))
    };
    let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 8 { 255u8 } else { 0 }]));
    (build(false), build(true), mask)
}

/// Attach the dilution pair's zone and read back (note, shipped spread,
/// target spread, the zone's tone controls, shipped and target p10).
fn dilution_run(
    noise: f32,
    name: &str,
) -> (Option<crate::rationale::Note>, f32, f32, String, (f32, f32)) {
    let (src, tgt, mask) = dilution_pair(noise);
    let path = fixture_mask_path(name);
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    pretend_full_support(&mut report.evidence);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let t_px = fit::pixels_of(&t_img);
    let sw = mask_weights(&mask, s_img.width(), s_img.height());
    let tw = mask_weights(&mask, t_img.width(), t_img.height());
    let attachment = semantic_attachment(sw.clone(), tw.clone(), &path);
    // The running frame on the gate's own ruler (`neutral_report` reads the
    // plain look distance, which is not what `attach_one_zone` compares).
    let mut frame_err =
        fit::look_err_with_evidence(&fit::pixels_of(&s_img), &t_px, &report.evidence);
    let accepted = attach_one_zone(
        &s_img,
        &t_px,
        &mut report,
        &mut frame_err,
        &attachment,
        divergence(0.05),
        None,
    );
    let shipped = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    path.remove();
    assert!(accepted.is_some(), "premise: the zone attaches: {}", report.recipe.rationale);
    let spread = |cdf: &[f32]| fit::quantile(cdf, 0.9) - fit::quantile(cdf, 0.1);
    let m = report.recipe.masks.last().expect("the zone's mask");
    let controls = format!(
        "ev {} contrast {} highlights {} shadows {} whites {} blacks {}",
        m.exposure_ev, m.contrast, m.highlights, m.shadows, m.whites, m.blacks
    );
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_TONE_PAIRING_DILUTED)
        .cloned();
    (
        note,
        spread(&zone_luma_cdf(&shipped, &sw)),
        spread(&zone_luma_cdf(&t_px, &tw)),
        controls,
        (
            fit::quantile(&zone_luma_cdf(&shipped, &sw), 0.1),
            fit::quantile(&zone_luma_cdf(&t_px, &tw), 0.1),
        ),
    )
}

/// 2026-10-22: a Pixel-scale zone whose paired tone map SQUEEZES its spread
/// under [`ZONE_PAIRED_SPREAD_FLOOR`] is solved by distribution matching —
/// the neon street's building, mapped through the conditional median, read
/// 0.70 of the target's p10–p90 under the mask alphas and shipped a frame
/// regression. The noisy pair fires the guard and ships within 15 % of the
/// target's spread; the clean pair pairs exactly and keeps the paired
/// branch, silently.
///
/// MUTATION THIS KILLS: `ZONE_PAIRED_SPREAD_FLOOR = 0.0` (the guard never
/// fires; the noisy pair carries no note and ships the squeezed map), and
/// `fit::refine_tone_sliders_on` returning its input (the noisy pair's p10
/// ships 0.039 above the target's).
#[test]
fn a_paired_tone_map_that_squeezes_the_zone_is_replaced_by_the_distribution_match() {
    let (note, shipped, target, controls, (p10, target_p10)) =
        dilution_run(0.20, "zone-dilution-noisy");
    let note = note.unwrap_or_else(|| {
        panic!("the guard must fire: shipped {shipped:.3} vs target {target:.3} ({controls})")
    });
    let ratio = note_number(&note, "ratio");
    assert!(
        ratio < super::limits::ZONE_PAIRED_SPREAD_FLOOR,
        "the note reads under its own floor: {:?}",
        note.args
    );
    assert!(
        (shipped / target - 1.0).abs() <= 0.15,
        "the distribution match ships the target's spread: shipped {shipped:.3} vs target \
         {target:.3}, paired ratio {ratio} ({controls})"
    );
    // The population score set and the spline refinement put the zone's p10
    // on the target's (measured 0.3720 vs 0.3722; the knot solve alone
    // shipped 0.4107).
    assert!(
        (p10 - target_p10).abs() <= 0.01,
        "the distribution match ships the target's p10: shipped {p10:.4} vs target \r
         {target_p10:.4} ({controls})"
    );

    let (note, shipped, target, controls, _) = dilution_run(0.0, "zone-dilution-clean");
    assert!(
        note.is_none(),
        "an exactly paired zone keeps the paired branch: {note:?} (shipped {shipped:.3} vs \
         target {target:.3}, {controls})"
    );
}

fn poisoned_bin_fixture() -> (DynamicImage, DynamicImage, GrayImage) {
    let (w, h) = (fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
    let sky_rows = h * 2 / 3;
    let build = |target: bool| {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v: f32 = if y < sky_rows {
                if target {
                    if (y / 8) % 2 == 0 { 0.05 } else { 0.15 }
                } else {
                    0.36 + 0.04 * x as f32 / (w - 1) as f32
                }
            } else {
                let ground = 0.34 + 0.06 * x as f32 / (w - 1) as f32;
                if target { ground + 0.08 } else { ground }
            };
            image::Rgb([(v.clamp(0.0, 1.0) * 255.0).round() as u8; 3])
        }))
    };
    let sky_mask = GrayImage::from_fn(w, h, |_, y| {
        image::Luma([if y < sky_rows { 255u8 } else { 0 }])
    });
    (build(false), build(true), sky_mask)
}

#[test]
fn a_zone_is_judged_by_its_own_members_not_the_frames_bins() {
    let (src, tgt, sky_mask) = poisoned_bin_fixture();
    let sp = fit::pixels_of(&src);
    let tp = fit::pixels_of(&tgt);
    let evidence = fit::evidence_model(&sp, &tp);
    let sky = mask_weights(&sky_mask, src.width(), src.height());
    let ground: Vec<f32> = sky.iter().map(|w| 1.0 - w).collect();
    let unsupported = evidence.spatial_supported.iter().filter(|&&s| !s).count();
    assert_eq!(
        unsupported,
        (src.width() * src.height() * 2 / 3) as usize,
        "premise: exactly the replaced sky is structurally unsupported"
    );
    let frame_bin = &evidence.luma[6];
    assert!(
        frame_bin.source_populated && frame_bin.weight <= 0.0,
        "premise: frame-wide bin 6 is populated yet withheld: {frame_bin:?}"
    );
    let ground_view = evidence.scoped(&tp, &ground, &ground);
    assert!(
        ground_view.luma[6].weight > 0.0,
        "the ground's own bin 6 must carry evidence: {:?}",
        ground_view.luma[6]
    );
    assert!(ground_view.luma[5].weight > 0.0, "{:?}", ground_view.luma[5]);
    let sky_view = evidence.scoped(&tp, &sky, &sky);
    assert!(
        sky_view.luma[6].source_populated && sky_view.luma[6].weight <= 0.0,
        "the sky's own bin 6 stays withheld: {:?}",
        sky_view.luma[6]
    );
    // Over the whole frame the scoped view IS the frame model, bit for bit.
    let ones = vec![1.0f32; sp.len()];
    let frame_view = evidence.scoped(&tp, &ones, &ones);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(frame_view.luma, evidence.luma);
    assert_eq!(frame_view.hue, evidence.hue);
    assert_eq!(bits(&frame_view.source_weights), bits(&evidence.source_weights));
    assert_eq!(bits(&frame_view.target_weights), bits(&evidence.target_weights));
    assert_eq!(bits(&frame_view.source_hue_weights), bits(&evidence.source_hue_weights));
    assert_eq!(bits(&frame_view.target_hue_weights), bits(&evidence.target_hue_weights));
    assert_eq!(frame_view.identifiability.to_bits(), evidence.identifiability.to_bits());
    assert_eq!(frame_view.population.to_bits(), evidence.population.to_bits());
    assert_eq!(evidence.population, evidence.source_pixels.len() as f32);
}

/// The ground zone's tone move touches only ground pixels, all of them
/// structurally supported; the frame-wide verdict would still have vetoed
/// it through the sky-poisoned bin. Judged by its own members it attaches
/// with a real exposure move and no tone refusal.
#[test]
fn a_ground_zone_is_not_vetoed_by_the_sky_it_does_not_touch() {
    let (src, tgt, sky_mask) = poisoned_bin_fixture();
    let path = fixture_mask_path("poisoned-bin-land");
    sky_mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let t_px = fit::pixels_of(&t_img);
    let sw = mask_weights(&sky_mask, s_img.width(), s_img.height());
    let tw = mask_weights(&sky_mask, t_img.width(), t_img.height());
    let land = ZoneAttachment {
        source_weights: sw.iter().map(|w| 1.0 - w).collect(),
        target_weights: tw.iter().map(|w| 1.0 - w).collect(),
        coverage: None,
        mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
        components: Vec::new(),
        range: None,
        name: String::new(),
        role: MaskRole::ZoneLand,
        inverted: true,
        label: MaskRole::ZoneLand.tag().to_string(),
        min_share: MIN_ZONE_SHARE,
    };
    let before = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    let mut frame_err = report.err_after;
    let accepted = attach_one_zone(
        &s_img,
        &t_px,
        &mut report,
        &mut frame_err,
        &land,
        measure_zone_divergence(&src, &tgt, &crate::recipe::EditRecipe::default(), &sky_mask)
            .land
            .divergence,
        None,
    );
    assert!(accepted.is_some(), "the ground zone must attach: {}", report.recipe.rationale);
    let zone = report.recipe.masks.last().expect("attached land mask");
    assert!(zone.exposure_ev > 0.0, "a real tone move must survive: {zone:?}");
    assert!(
        !report
            .notes
            .iter()
            .any(|n| is_tone_refusal(n.key)),
        "the ground zone must not be vetoed through the sky's bins: {}",
        report.recipe.rationale
    );
    // Premise, stated by the frame-wide model itself: this very move would
    // have been vetoed through the replaced sky's identical luma bins.
    let after = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    assert!(
        fit::moved_unsupported_luma_range_names(&before, &after, &report.evidence).is_some(),
        "premise: the frame-wide verdict names the poisoned bin for this move"
    );
    path.remove();
}

/// The calibration land is a Full zone inside an Atmosphere frame. Its own
/// rerendered mid-tones keep too little structural survival to testify, so
/// the Full zone reads the carried structural model and withholds those
/// ranges; the frame's blind ruler would allow them.
///
/// RE-READ for v1.18.0 (DEF-3). With each range's bands read inside its own
/// pixels, the land's luma[0.29-0.35] / [0.35-0.41] now read divergence
/// 0.396 / 0.365 and carry structural weight 0.033 / 0.037 (they read 0
/// before); luma[0.41-0.47] / [0.47-0.53] / [0.53-0.59] read 0.501 / 0.552 /
/// 0.508 and are still withheld (structural weight 0, blind 0.027 / 0.032 /
/// 0.017). The land's tone move (+0.04 EV) passes the zone's own gate on that
/// model, so no tone note is written: the test's earlier first half ("a
/// withheld note") was a reading of the move, not of the law, and is not
/// asserted. The law — the zone withholds what its own structural model
/// does not keep, where the blind ruler would allow it — is asserted on the
/// ranges that still carry it.
#[test]
fn calibration_land_zone_withholds_the_mid_tones_its_own_structure_does_not_keep() {
    let Some(root) = fit::calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    let sky_mask = image::open(root.join("sky-mask.png"))
        .expect("calibration sky-mask.png")
        .to_luma8();
    let mask_path = fixture_mask_path("calibration-land-scratch");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&source, &target);
    assert_eq!(report.mode, fit::FitMode::Atmosphere);
    assert!(report.structural_evidence.is_some());
    attach_zones(&source, &target, &mut report, &sky_mask, &sky_mask, &mask_path);
    let land_tag = MaskRole::ZoneLand.tag();
    // Any land tone note there is names none of the sky's bright bins.
    for note in report.notes.iter().filter(|note| {
        is_tone_refusal(note.key) && note.args.iter().any(|(key, value)| *key == "label" && value == land_tag)
    }) {
        let ranges = note.args.iter().find(|(key, _)| *key == "luma_ranges").map_or("", |(_, value)| value.as_str());
        assert!(
            !["luma[0.59-0.65]", "luma[0.65-0.71]", "luma[0.71-0.76]", "luma[0.76-0.82]"]
                .iter()
                .any(|range| ranges.contains(range)),
            "the land note inherited the sky's bright bins: {ranges}"
        );
    }

    let (s_img, t_img) = fit::analysis_pair(&source, &target);
    let tgt_px = fit::pixels_of(&t_img);
    let land_source = mask_weights(&sky_mask, s_img.width(), s_img.height())
        .iter()
        .map(|weight| 1.0 - weight)
        .collect::<Vec<_>>();
    let land_target = mask_weights(&sky_mask, t_img.width(), t_img.height())
        .iter()
        .map(|weight| 1.0 - weight)
        .collect::<Vec<_>>();
    let blind_land = report.evidence.scoped(&tgt_px, &land_source, &land_target);
    let structural_land = report
        .structural_evidence
        .as_ref()
        .unwrap()
        .scoped(&tgt_px, &land_source, &land_target);
    for label in [
        "luma[0.41-0.47]",
        "luma[0.47-0.53]",
        "luma[0.53-0.59]",
    ] {
        let blind = blind_land
            .luma
            .iter()
            .find(|range| range.label == label)
            .unwrap_or_else(|| panic!("unknown land range {label}"));
        let structural = structural_land
            .luma
            .iter()
            .find(|range| range.label == label)
            .unwrap();
        assert!(blind.weight > 0.0, "blind scope would also withhold {label}");
        assert_eq!(structural.weight, 0.0, "structural scope did not withhold {label}");
    }
    mask_path.remove();
}


/// The hue twin of [`poisoned_bin_fixture`]. The top two thirds are
/// REPLACED content that occupies the same warm hue band as the ground, so
/// frame-wide that band keeps too little structural survival to testify;
/// the ground alone keeps all of it, and it is the ground that carries the
/// hue move the target asks for.
fn poisoned_hue_fixture() -> (DynamicImage, DynamicImage, GrayImage) {
    let (w, h) = (fit::ANALYZE_EDGE, fit::ANALYZE_EDGE);
    let sky_rows = h * 2 / 3;
    let warm = |v: f32, muted: bool| {
        // The SAME hue angle on both sides (28.3 deg against 29.5 deg, one
        // evidence band), a different chroma: the ground's move must stay
        // inside the band whose testimony this test is about.
        let (r, g, b) = if muted { (1.00, 0.72, 0.45) } else { (1.00, 0.62, 0.28) };
        image::Rgb([
            ((v * r).clamp(0.0, 1.0) * 255.0).round() as u8,
            ((v * g).clamp(0.0, 1.0) * 255.0).round() as u8,
            ((v * b).clamp(0.0, 1.0) * 255.0).round() as u8,
        ])
    };
    let build = |target: bool| {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            if y < sky_rows {
                if target {
                    // Replaced: a stripe deck in the same hue family, so it
                    // lands in the same evidence band and poisons it.
                    warm(if (y / 8) % 2 == 0 { 0.35 } else { 0.75 }, false)
                } else {
                    warm(0.50 + 0.20 * x as f32 / (w - 1) as f32, false)
                }
            } else {
                // The ground: identical structure on both sides, rotated
                // toward yellow on the target.
                warm(0.55 + 0.15 * x as f32 / (w - 1) as f32, target)
            }
        }))
    };
    let sky_mask = GrayImage::from_fn(w, h, |_, y| {
        image::Luma([if y < sky_rows { 255u8 } else { 0 }])
    });
    (build(false), build(true), sky_mask)
}

/// A21. The scoped HUE verdict is the zone's own, asserted without going
/// through the frame-wide identity.
///
/// `a_zone_is_judged_by_its_own_members_not_the_frames_bins` pins the LUMA
/// half of `EvidenceModel::scoped` on its own members and the hue half only
/// through the whole-frame identity `scoped(ones, ones) == frame`, which a
/// zone-path defect cannot move. This is the missing half: a hue band the
/// FRAME withholds, the GROUND keeps and the SKY does not.
///
/// MUTATION: `EvidenceModel::scoped` returning `hue: self.hue.clone()`
/// instead of `ranges.hue`. The whole-frame identity stays green because
/// over the whole frame the two are the same object; the band search below
/// finds nothing and this test fails.
#[test]
fn a_zones_scoped_hue_verdict_is_its_own_not_the_frames() {
    let (src, tgt, sky_mask) = poisoned_hue_fixture();
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let sp = fit::pixels_of(&s_img);
    let tp = fit::pixels_of(&t_img);
    let frame = fit::evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let sky = mask_weights(&sky_mask, s_img.width(), s_img.height());
    let ground: Vec<f32> = sky.iter().map(|w| 1.0 - w).collect();
    let ground_view = frame.scoped(&tp, &ground, &ground);
    let sky_view = frame.scoped(&tp, &sky, &sky);
    let band = (0..frame.hue.len())
        .find(|&i| {
            frame.hue[i].source_populated
                && frame.hue[i].weight <= 0.0
                && ground_view.hue[i].weight > 0.0
        })
        .unwrap_or_else(|| {
            panic!(
                "premise: a hue band the frame withholds and the ground keeps\n\
                     frame  {:?}\nground {:?}",
                frame.hue, ground_view.hue,
            )
        });
    assert!(
        sky_view.hue[band].weight <= 0.0,
        "the replaced sky must keep withholding its own band: {:?}",
        sky_view.hue[band],
    );
    assert!(
        ground_view.hue[band].two_sided_share > 0.0,
        "the ground's own band must be two-sided: {:?}",
        ground_view.hue[band],
    );
}

/// A23. The neutral-solution exit, REACHED.
///
/// Its own doc used to say it was not reachable from a synthetic pair and
/// that only the six-arm live runs evidenced it. It is reachable, and the
/// construction is the one the exit describes: a zone whose residual is
/// large while every class the estimator could move is withheld. The
/// replaced sky of [`poisoned_hue_fixture`] is exactly that: it is
/// structurally unsupported on both axes, so the tone probe names its luma
/// ranges and the chroma probe names its hue band, both withholding arms
/// fire, and every dial the estimator produced is zeroed. What is left is a
/// solution that came out neutral beside a residual that did not.
///
/// MUTATION: give the exit `ZONE_ALREADY_MATCHED` instead of
/// `ZONE_NO_MOVEMENT_SURVIVED` and the key assertion below fails.
#[test]
fn the_neutral_solution_exit_is_reached_when_every_class_is_withheld() {
    let (src, tgt, sky_mask) = poisoned_hue_fixture();
    let path = fixture_mask_path("poisoned-bin-sky-neutral");
    sky_mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let t_px = fit::pixels_of(&t_img);
    let sky = ZoneAttachment {
        source_weights: mask_weights(&sky_mask, s_img.width(), s_img.height()),
        target_weights: mask_weights(&sky_mask, t_img.width(), t_img.height()),
        coverage: None,
        mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
        components: Vec::new(),
        range: None,
        name: String::new(),
        role: MaskRole::ZoneSky,
        inverted: false,
        label: MaskRole::ZoneSky.tag().to_string(),
        min_share: MIN_ZONE_SHARE,
    };
    let masks_before = report.recipe.masks.len();
    let mut frame_err = report.err_after;
    let accepted = attach_one_zone(
        &s_img,
        &t_px,
        &mut report,
        &mut frame_err,
        &sky,
        measure_zone_divergence(&src, &tgt, &crate::recipe::EditRecipe::default(), &sky_mask)
            .sky
            .divergence,
        None,
    );
    assert!(
        accepted.is_none(),
        "a neutral solution must not attach: {}",
        report.recipe.rationale
    );
    assert_eq!(
        report.recipe.masks.len(),
        masks_before,
        "the exit must pop its own mask: {}",
        report.recipe.rationale,
    );
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_NO_MOVEMENT_SURVIVED)
        .unwrap_or_else(|| {
            panic!("the neutral-solution exit was not reached: {}", report.recipe.rationale)
        });
    // It is the NEUTRAL-SOLUTION exit and not the already-matched one: the
    // residual it discloses is far above the skip line it does not claim.
    let before = note
        .args
        .iter()
        .find(|(key, _)| *key == "before")
        .map(|(_, value)| value.parse::<f32>().expect("a numeric residual"))
        .expect("the exit discloses the residual it leaves behind");
    assert!(
        before > ZONE_SKIP_ERR,
        "the exit must fire on a zone that is NOT matched, read {before}",
    );
    assert!(
        !report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::ZONE_ALREADY_MATCHED),
        "no arm may claim a match here: {}",
        report.recipe.rationale,
    );
    path.remove();
}

/// A24. The arbitration's REJECT branch, driven deterministically.
///
/// `loopback_multi_run_arbitrates_against_the_seeded_two` asserts the
/// reject branch only `if` the run happened to refuse, so nothing pinned it.
/// This fixture makes the tie certain: the loopback bridge hands the
/// multi-class call the same sky plane the single-class call returns, and
/// [`zoned_pair`]'s land is byte-identical on both sides, so the two-region
/// reference and the multi-region candidate solve the same sky and nothing
/// else. `multi_error >= two_error` then holds by equality and the
/// candidate is dropped, so the branch runs on every execution of this
/// test rather than on the ones that happen to refuse.
///
/// WHAT THIS PINS, and what it does not, both measured on 2026-09-02.
/// Deleting the `chosen.notes.push(refusal)` in the reject branch fails
/// here: the branch must publish the comparison it lost, and the loser's
/// `mask-region-*` must not reach the kept recipe. Making
/// `release_unselected_rasters` a no-op does NOT fail here, and the
/// measurement says why: on this fixture the candidate's one region is
/// `ZONE_DROPPED`, so it claims no raster and there is no orphan to leave
/// behind. The release itself is pinned next door by
/// `release_unselected_rasters_keeps_exactly_what_the_kept_recipe_references`,
/// which does fail under that same no-op. The on-disk sweep below stays as
/// a guard for the day a refused candidate does claim one.
///
/// 2026-10-22: on [`zoned_pair`]'s gold sky the region's boundary gate now
/// re-checks the frame under the 0.02 drift the region was admitted on, not
/// 0.0, so the region (Atmosphere, D = 0.809) is kept and the four-region
/// candidate WINS — the reject branch was never reached. The same layout
/// with a darker, bluer target sky (0.30, 0.40, 0.62) brings the tie back:
/// measured, both candidates finish at 0.026381 under the reference's
/// ruler, so `multi >= two` again holds by equality and the branch runs.
#[test]
fn the_multi_region_reject_branch_discloses_the_comparison_it_lost() {
    let (src, tgt) = {
        let (w, h) = (16u32, 16u32);
        let build = |sky: [f32; 3], rock: [f32; 3]| {
            DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
                let p = if y >= 12 {
                    sky
                } else if x < 4 {
                    [0.63, 0.63, 0.63]
                } else {
                    rock
                };
                image::Rgb(p.map(|c| (c * 255.0).round() as u8))
            }))
        };
        (
            build([0.63, 0.63, 0.63], [0.55, 0.45, 0.35]),
            build([0.30, 0.40, 0.62], [0.60, 0.46, 0.33]),
        )
    };
    let base = crate::recipe::EditRecipe::default();
    let dir = crate::test_dir("seg-arb-reject");
    let seg = loopback_segment_opts(&dir, PlaneSource::Input, src.width(), src.height());
    let anchor = crate::store::OwnedRaster::scratch(dir.join("anchor.png"));
    let report = fit_recipe_zoned_with_regions(
        &src,
        &tgt,
        &seg,
        &anchor,
        &base,
        fit::FitOptions::default(),
        4,
    );
    let refusal = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::REGION_FRAME_REFUSED)
        .unwrap_or_else(|| {
            panic!(
                "the reject branch must be reached on this fixture: {}",
                report.recipe.rationale
            )
        });
    let arg = |key: &str| {
        refusal
            .args
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.parse::<f32>().expect("a numeric reading"))
            .unwrap_or_else(|| panic!("the refusal discloses {key}"))
    };
    assert!(
        arg("multi") >= arg("two"),
        "the refusal must disclose the comparison it lost: {refusal:?}",
    );
    // By FILE, not by variant: the sky/land zones are Select Sky
    // components and their alpha is still a claim this hygiene rule owns.
    let referenced = report
        .recipe
        .masks
        .iter()
        .filter_map(|m| {
            std::path::Path::new(render::geometry_raster_path(&m.mask)?)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .collect::<std::collections::HashSet<_>>();
    assert!(
        referenced.iter().all(|name| !name.starts_with("mask-region-")),
        "a refused candidate must ship no region mask: {referenced:?}",
    );
    let on_disk = std::fs::read_dir(&dir)
        .expect("the run's own directory")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    for name in &on_disk {
        if name.starts_with("mask-") || name == "anchor.png" {
            assert!(
                referenced.contains(name),
                "the reject branch left an orphan claim {name}; kept {referenced:?}",
            );
        }
    }
    for name in &referenced {
        assert!(on_disk.contains(name), "referenced raster {name} is missing");
    }
}

/// The neutral-solution exit does not borrow the "already matches" claim.
///
/// `neutral_zone` (below `local_quality`) fires when every dial the
/// estimator produced came back within 1e-4 of neutral -- a fact about the
/// SOLUTION, typically because the evidence gates withheld every class. It
/// is independent of how far the zone is from its target, and it used to
/// borrow `ZONE_ALREADY_MATCHED`, whose sentence asserts the opposite and
/// prints the contradicting residual inside its own claim. Same rule and
/// same fix as `ZONE_SHARE_NO_CORRECTION` (rationale.rs:354-360).
///
/// This test pins the two things that are decidable from the note alone:
/// the sentences are different, and the new one makes no claim about
/// matching. The exit ITSELF is reached and asserted by
/// `the_neutral_solution_exit_is_reached_when_every_class_is_withheld`,
/// which builds the construction the exit describes — a large residual
/// beside a solution that came out neutral because every movable class was
/// withheld. v1.2.3's note here said no synthetic pair could reach it; that
/// was true only of the 16x16 block fixtures it had tried.
///
/// MUTATION: give `ZONE_NO_MOVEMENT_SURVIVED` the text of
/// `ZONE_ALREADY_MATCHED` and both assertions below fail.
#[test]
fn the_neutral_solution_note_makes_no_claim_about_matching() {
    let rendered = crate::rationale::render_one(&crate::rationale::Note::new(
        crate::rationale::keys::ZONE_NO_MOVEMENT_SURVIVED,
        vec![("label", "sky".to_string()), ("before", "0.143".to_string())],
    ));
    assert!(
        !rendered.contains("already matches"),
        "the neutral-solution exit must not claim a match: {rendered}"
    );
    assert!(
        rendered.contains("0.143") && rendered.contains("uncorrected"),
        "it must still disclose the residual it is leaving behind: {rendered}"
    );
    let matched = crate::rationale::render_one(&crate::rationale::Note::new(
        crate::rationale::keys::ZONE_ALREADY_MATCHED,
        vec![("label", "sky".to_string()), ("before", "0.143".to_string())],
    ));
    assert_ne!(rendered, matched, "a terminal exit must not borrow another's sentence");

    // And the wiring, since no synthetic fixture can reach that exit (see
    // the doc above). A SOURCE-level pin, stated as one: it reads the
    // `neutral_zone` block and checks which key it emits. Precedent is
    // `pipeline.rs`'s own source-counting test. It cannot tell whether the
    // block is reachable -- only that, when reached, it says this.
    // The luma-only tone ladder probes the fitted tone before backing off,
    // and a zone's before / after readings share one population.
    // Read from the BODY — the whole file holds these literals right
    // here, and a pin that reads itself cannot fail.
    let src = crate::fit_zoned::source_text();
    let src = crate::source_before_tests(&src);
    assert!(src.contains("for factor in [1.0f32, 0.75, 0.5, 0.25, 0.0] {"));
    assert!(src.contains("let m_after = zone_moments(&zoned_px, &zw_source);"));
    // …and the boundary ruler keeps reading the mask's own raster.
    assert!(src.contains("weights: &zone.mask_weights,"));
    let at = src.find("    if neutral_zone {").expect("the neutral-solution exit moved");
    let block = &src[at..at + 900];
    assert!(
        block.contains("ZONE_NO_MOVEMENT_SURVIVED"),
        "the neutral-solution exit no longer emits its own key"
    );
    assert!(
        !block.contains("keys::ZONE_ALREADY_MATCHED"),
        "the neutral-solution exit went back to borrowing the match claim"
    );
}

/// With its colour class withheld a zone is judged on tone alone -- so the
/// skip line is asked of tone alone too. A zone whose luma already matches
/// is left alone with the honest note instead of being dialled for a
/// hairline tone gain against a chroma gap it may not touch.
///
/// The colour is withheld here by a region at CELL scale whose cells carry
/// no evidence (an abstention is never an admission). Until 2026-10-21 the
/// one-sided band alone withheld it on a region whose pixels pair; the
/// paired vouch now ships that recolour, which is the other test's claim.
#[test]
fn a_zone_whose_movable_class_already_matches_is_left_alone() {
    let (w, h) = (16u32, 16u32);
    let build = |sky: [f32; 3]| -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |_, y| {
            let p = if y >= 12 { sky } else { [0.55f32, 0.45, 0.35] };
            image::Rgb(p.map(|c| (c * 255.0).round() as u8))
        }))
    };
    // Same luma, opposite hue: the blue sky's band exists only on the
    // source side and the warm target's only on the target side.
    let src = build([0.60, 0.63, 0.67]);
    let tgt = build([0.67, 0.62, 0.59]);
    let sky_mask =
        GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 12 { 255u8 } else { 0 }]));
    let path = fixture_mask_path("movable-class-matched");
    sky_mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);
    let (s_img, t_img) = fit::analysis_pair(&src, &tgt);
    let t_px = fit::pixels_of(&t_img);
    let sw = mask_weights(&sky_mask, s_img.width(), s_img.height());
    let tw = mask_weights(&sky_mask, t_img.width(), t_img.height());
    // No support under the mask (DEF-5: the cells' trust is the spatial
    // support; the estimation weights no longer reach a cell's vote).
    for (weight, member) in report.evidence.spatial_weights.iter_mut().zip(&sw) {
        if *member > 0.0 {
            *weight = 0.0;
        }
    }
    let attachment = semantic_attachment(sw, tw, &path);
    let mut frame_err = report.err_after;
    let accepted = attach_one_zone(
        &s_img,
        &t_px,
        &mut report,
        &mut frame_err,
        &attachment,
        // Past the pairing line and short of the mode line: the cells are
        // asked, and with no evidence under the mask they abstain.
        Some(fit::Divergence { correlation: 0.5, energy_error: 0.0, d: 0.5 }),
        None,
    );
    assert!(
        report.notes.iter().any(|n| is_colour_refusal(n.key)),
        "premise: the abstaining cells withhold colour: {}",
        report.recipe.rationale
    );
    assert!(
        accepted.is_none() && report.recipe.masks.is_empty(),
        "a tone-matched zone must not be dialled: {}",
        report.recipe.rationale
    );
    assert!(
        report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_ALREADY_MATCHED),
        "the movable class already matches and must say so: {}",
        report.recipe.rationale
    );
    path.remove();
}

/// The zone stage's confidence is read on the render that ships, so the
/// confidence verdict has to come after the band trials and read each
/// accepted region on the finished frame. RE-PINNED (v1.18.0, R1): on the
/// calibration pair the bands no longer replace the single sky zone (a band
/// set must gain FIT_QUANT on the frame over it, and here it does not:
/// "frame do-no-harm, 7 trials"), so the trial that decides what ships is
/// the one that KEPT the zone; the verdict must still come after it. Until v1.17.1 it was taken before
/// `replace_zone`, from the single sky zone's own residual, and the shipped
/// confidence described a correction the recipe no longer carried.
#[test]
fn a_zoned_fits_confidence_is_read_on_the_render_that_ships() {
    let Some(root) = fit::calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    let sky_mask = image::open(root.join("sky-mask.png")).expect("calibration sky-mask.png").to_luma8();
    let mask_path = fixture_mask_path("calibration-confidence-scratch");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&source, &target);
    let global_confidence = report.recipe.confidence;
    attach_zones(&source, &target, &mut report, &sky_mask, &sky_mask, &mask_path);
    let position = |key: &str| report.notes.iter().position(|n| n.key == key);
    let bands = position(crate::rationale::keys::ZONE_SUBZONES_ATTACHED)
        .or_else(|| position(crate::rationale::keys::ZONE_SUBZONES_REGRESSED))
        .unwrap_or_else(|| panic!("premise: the band trials ran on the calibration pair: {}", report.recipe.rationale));
    let verdict = position(crate::rationale::keys::ZONE_CONFIDENCE).expect("the zone stage states its confidence");
    assert!(verdict > bands, "the confidence was read before the band trials decided what ships");
    let note = &report.notes[verdict];
    let (worst, frame) = (note_number(note, "worst"), note_number(note, "frame"));
    // It is read on the frame that ships: the finished frame's own error
    // (`err_after`, set by the band trial that won) ...
    let (s_img, t_img) = fit::analysis_pair(&source, &target);
    let shipped = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
    let shipped_err = fit::look_err_with_evidence(&shipped, &fit::pixels_of(&t_img), &report.evidence);
    assert!((frame - shipped_err).abs() <= 0.0005, "the verdict read frame {frame}, the shipped recipe renders {shipped_err}");
    // ... and the shipped confidence is the verdict it states, through the
    // slope, floored by the global stage's claim (the note's three decimals
    // bound the comparison at half a unit of the third decimal times the slope).
    let claim = global_confidence.min(fit::clamp_confidence(1.0 - worst * ZONE_CONFIDENCE_SLOPE));
    assert!(
        (report.recipe.confidence - claim).abs() <= 0.0005 * ZONE_CONFIDENCE_SLOPE,
        "confidence {} is not the stated verdict's {claim} (worst {worst})",
        report.recipe.confidence
    );
    mask_path.remove();
}

#[test]
fn calibration_sky_zone_survives_luminance_with_partial_chroma_refusal() {
    let Some(root) = fit::calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    // READ the corpus, never OWN it: `attach_zones` deletes the raster
    // it is handed when no zone survives, and this line used to hand it
    // the user's irreplaceable calibration mask. The scratch copy is the
    // convention every other test here already follows.
    let sky_mask = image::open(root.join("sky-mask.png"))
        .expect("calibration sky-mask.png")
        .to_luma8();
    let mask_path = fixture_mask_path("calibration-sky-scratch");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&source, &target);
    attach_zones(&source, &target, &mut report, &sky_mask, &sky_mask, &mask_path);
    // RE-PINNED for v1.3.0's fit (R36 residual banding, R37's target-referenced
    // seam ruler), measured with the corpus in reach for the first time since
    // v1.2.6. Before step 9 the absolute rim ruler charged the scene's own bow
    // to this correction and shrank it to -0.12..-0.15 EV; step 9's
    // differential ruler let the single sky zone keep -0.186 EV. Since v1.3.0
    // the sky survives as TWO bands — the residual earned a partition ("Zoned
    // sky accepted 2 bands after 15 trials") — and the seam ruler reads the
    // target's own boundary (it asks 0.217 here; the introduced rim 0.037 ->
    // 0.007 after shared differential shrink k=0.154). The band that carries
    // the luminance move keeps -0.174 EV with its colour withheld: the
    // target's 12x8 cell means did not vouch the move (0.628 converged, 0.287
    // diverged over 60 cells). RE-PINNED 2026-09-24 for the tone ladder that
    // probes the fitted step first: the band passes the local quality gate
    // at its full step, where it used to ship three quarters of it
    // (-0.152 EV, the ladder's old first rung; the seam ruler then asked
    // 0.220 and shrank by k=0.193). The partition re-arbitrated around that
    // step: the second band's own move went from -0.062 EV to -0.007 EV at
    // saturation +2.2 (was +2.4) and the land bands' saturation from +2.4 to
    // +1.6; the sky bands' deltaE after the fit 21.41 -> 21.24, the land
    // bands' 5.91 -> 6.08, the frame-wide residual 0.095 either way.
    // Measured by reverting each of the two zoned-fit corrections alone in
    // a copy of the tree: the ladder moves every number above, the
    // after-reading population moves one land gain by one ulp. The second
    // band still ships the small colour move its own cells did vouch, so
    // the "partial" refusal of this test's name is literally partial: one
    // band refuses colour, the other does not. RE-PINNED 2026-10-21 for the
    // refined colour field (12×32×8, `fit_field::FIELD_Y_REFINE`): the
    // head-of-sequencer field's band marginal proposes different luminance
    // ranges, so the sky partition re-arbitrated (breaks 0.276, overlap
    // 0.543, 7 trials, R² 0.914; the sky bands' deltaE 21.02 -> 20.25 against
    // 21.41 -> 21.24 before), and the band that carries the luminance move
    // (-0.211 EV) now ships its colour at 0.672 of the solved move on the
    // R36 share ladder: the FULL move is still refused by the cells (0.559
    // converged, 0.338 diverged over 60 cells — the refusal this test is
    // named for), and the share the cells did vouch (0.783 closer, 0.080
    // away, 0.916 aligned) carries gains [1.037 0.985 0.977] at saturation
    // +4.2. The second band: -0.047 EV, saturation +5.3. On the 11-pair final
    // check the calibration pair's own deltaE against its target went
    // 5.57 -> 4.41. "Partial" now reads: the full move refused, a share
    // shipped — on both bands.
    // RE-PINNED 2026-10-23 for 2d0e750's boundary soft ruler, which reads a
    // colour step through a one-code dead band: the global fit is unchanged
    // (0.189 -> 0.100), but the whole-sky shrink now stops at k=0.362
    // (0.235) and the banded one at k=0.200 (0.088), so more of the solved
    // move ships. The partition re-arbitrated (breaks 0.395, overlap 0.130,
    // 7 trials, R² 0.812); the sky bands' deltaE 19.75 -> 18.36 (21.02 ->
    // 20.25), the whole sky's zone residual 0.164 -> 0.122 (0.162 -> 0.138),
    // frame 0.087 (0.092). The luminance band keeps -0.371 EV (-0.211) and
    // ships its colour at 0.688 of the solved move, the full move still
    // refused by the cells (0.601 converged, 0.254 diverged over 45 cells);
    // the second band -0.067 EV (-0.047), saturation +8.3 (+5.3). The kept
    // rim now reads 0.015 raw, over the 0.012 ceiling, because the target's
    // own boundary asks 0.227 of it: the gate compares the charged ranks
    // (`BoundaryReading::gated`, 0.011 luma / 0.012 colour), so that is what
    // is asserted below, as `every_accepted_fixture_zone_...` reads it.
    // RE-PINNED (v1.18.0, R1): the sky survives as ONE zone again. The
    // bands still lower the sky's deltaE (19.74 -> 18.37) but a band set now
    // replaces the single zone only on a J* gain of FIT_QUANT over it
    // (`subzones::replace_zone`, `fit::ships`), and it does not earn one:
    // "kept the current recipe ... (frame do-no-harm, 7 trials)", read the
    // same with S in the ruler and in S-mode (lane c probe). The single
    // zone carries the luminance move and ships its colour at a share of the
    // solved move, the full move refused by its cells: the partial chroma
    // refusal this test is named for.
    // RE-PINNED (v1.18.0, R19: a local correction ships on its own
    // population, the frame a guard): the sky survives as TWO bands again.
    // The band set earns on the sky's own pixels (deltaE 20.10 -> 18.46,
    // "Zoned sky accepted 2 bands after 7 trials", breaks 0.395, overlap
    // 0.130, R² 0.807), where R1's frame reading tied it. The band that
    // carries the luminance move (-0.345 EV, gains [1.057 0.975 0.975], sat
    // +5.0) ships its colour at 0.734 of the solved move, the full move
    // refused by its cells (0.601 converged, 0.254 diverged over 45 cells):
    // the partial chroma refusal this test is named for, now on that band.
    // The second band: -0.030 EV, sat +7.9. Every boundary verdict stays
    // inside the ceiling on the charged ranks (sky: rim 0.167 -> 0.039 raw
    // after k = 0.219, charged 0.012 / colour 0.011; land: charged 0.012).
    let sky_zones: Vec<_> =
        report.recipe.masks.iter().filter(|mask| mask.role == MaskRole::ZoneSky).collect();
    assert!(!sky_zones.is_empty(), "the calibration sky survives: {}", report.recipe.rationale);
    let sky = *sky_zones
        .iter()
        .min_by(|a, b| a.exposure_ev.total_cmp(&b.exposure_ev))
        .expect("non-empty");
    eprintln!(
        "CALIBRATION_SKY bands={} ev={:.3} gains={:?} sat={:.1} rationale={}",
        sky_zones.len(), sky.exposure_ev, sky.color_gains, sky.saturation, report.recipe.rationale
    );
    assert!(sky.exposure_ev < 0.0, "the sky zone carries a darkening luminance move: ev {}", sky.exposure_ev);
    assert!(
        !gains_withheld(sky.color_gains),
        "the sky ships the share of its colour the cells vouched: {:?} sat {}",
        sky.color_gains,
        sky.saturation
    );
    let verdicts: Vec<_> =
        report.notes.iter().filter(|note| note.key == crate::rationale::keys::ZONE_BOUNDARY_PASSED).collect();
    assert!(!verdicts.is_empty(), "calibration sky must reach the boundary gate");
    for note in verdicts {
        let gated = note_number(note, "charged").max(note_number(note, "colour_charged"));
        assert!(gated <= ZONE_BOUNDARY_RIM_MAX, "charged {gated} (raw rim {})", note_number(note, "after"));
    }
    let label = if sky.name.is_empty() { MaskRole::ZoneSky.tag().to_string() } else { sky.name.clone() };
    let colour_note = report
        .notes
        .iter()
        .find(|note| {
            note.key == crate::rationale::keys::ZONE_COLOUR_VOUCHED_AT_SHARE
                && note.args.iter().any(|(key, value)| *key == "label" && *value == label)
        })
        .expect("the one-sided calibration sky must refuse its full colour move and ship a share");
    assert!(
        colour_note
            .args
            .iter()
            .any(|(key, value)| *key == "hue_bands" && value.contains("Aqua"))
    );
    assert!(!report.notes.iter().any(|note| {
        is_tone_refusal(note.key)
            && note.args.iter().any(|(key, value)| {
                *key == "label" && value == MaskRole::ZoneSky.tag()
            })
    }));
    mask_path.remove();
}

/// The zone stage's verdict is bounded by BOTH stages: it may not raise
/// the global fit's own claim, and the global fit may not keep a claim
/// the zones contradict.
#[test]
fn a_zoned_fits_confidence_comes_from_the_zones_it_accepted() {
    let (src, tgt, sky_mask) = zoned_pair();
    let mask_path = fixture_mask_path("zoned-confidence-mask");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&src, &tgt);
    let global_conf = report.recipe.confidence;
    attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
    assert!(
        !report.recipe.masks.is_empty(),
        "premise: a zone attaches on this fixture: {}",
        report.recipe.rationale
    );
    assert!(
        report.recipe.confidence <= global_conf + 1e-6,
        "the zone stage must not raise the global claim ({} > {global_conf})",
        report.recipe.confidence
    );
    assert!(
        report.recipe.confidence >= 0.25,
        "…nor sink below the family floor: {}",
        report.recipe.confidence
    );
    mask_path.remove();
}

#[test]
fn zoned_orchestration_corrects_the_land_beside_the_shrunk_sky() {
    // The first real-pair render's lesson: repainting ONLY the sky leaves
    // everything outside the mask with the global look — on the real
    // pair a blue haze band clashed against the new gold sky. The land
    // zone reuses the SAME raster inverted; when the target's land
    // differs too (muted vs vivid warm), the land zone must attach beside
    // the sky's own correction. (Until 2026-10-21 the sky's one-sided hue
    // was withheld and this pinned unity gains; its pixels pair, so the
    // paired vouch now ships the warm recolour.)
    let (w, h) = (16u32, 16u32);
    // Muted hazy land → warmer, brighter land (the real pair's demand, at
    // half the reach it had until 2026-10-21: with the tone stage answering
    // for itself the global solve no longer carries part of a fully vivid
    // [0.80, 0.50, 0.28] land, the land zone then drifts the frame past its
    // tolerance and the sky zone is refused at its boundary shrink. Measured
    // over the variants on that date: this target keeps the global solve
    // neutral and attaches the warm sky beside the warm inverted land).
    //
    // 2026-10-22: the land's left six columns are one neutral grey on both
    // sides. The ungated global tone arm and its cast curves now carry the
    // land's warmth (saturation 60, curves) and overshoot the sky, which
    // then shipped cool [0.99, 1.00, 1.21] beside two non-inverted land
    // zones. Measured over patch widths 0/2/4/5/6/7/8/10 of grey 0.63 and
    // greys 0.50/0.75 at widths 4/6: only this one keeps the sky at pixel
    // scale (D 0.250) with its gold vouched by its own pixels, warm
    // [0.996, 1.01, 0.958], beside the inverted land zone; the land's
    // warmth arrives through the global solve, so the render check below
    // is what pins it.
    //
    // Later on 2026-10-22 the boundary ruler's colour dead band (R1) moved
    // this pair's shared shrink from k 0.103 to 0.165, and at that strength
    // the shrunk-zone do-no-harm refused both zones: the frame read 0.05845
    // -> 0.12834. That is this 16-px toy's structural term (three flat
    // blocks, the grey patch and the sky rendered within a few codes of
    // each other) swinging D 0.093 -> 1.506 between k 0.103 and 0.165 while
    // both zones' own residuals improved; see
    // `every_accepted_fixture_zone_still_passes_the_boundary_gate`. The
    // land target is milder, [0.62, 0.46, 0.34] -> [0.58, 0.46, 0.37]
    // (still warmer and brighter than the source rock): measured, k 0.165,
    // the frame 0.04672 -> 0.05909 (inside the 0.02 drift), the sky warm
    // [0.995, 1.013, 0.936] beside the inverted land zone, and the land
    // pixel renders (164, 118, 87). Of 32 variants measured (grey widths
    // 4–8, greys 0.50–0.72, three land targets), three keep the warm sky
    // beside an inverted land zone, and on this one the frame strays least
    // from its entry over k = 0, 0.05 … 1 (at most +0.034, at k 0.80).
    let build = |sky: [f32; 3], rock: [f32; 3]| -> DynamicImage {
        let img = RgbImage::from_fn(w, h, |x, y| {
            let p = if y >= 12 {
                sky
            } else if x < 6 {
                [0.63, 0.63, 0.63]
            } else {
                rock
            };
            image::Rgb(p.map(|c| (c * 255.0).round() as u8))
        });
        DynamicImage::ImageRgb8(img)
    };
    let src = build([0.60, 0.63, 0.67], [0.45, 0.42, 0.40]);
    let tgt = build([0.92, 0.72, 0.48], [0.58, 0.46, 0.37]);
    let sky_mask =
        GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 12 { 255u8 } else { 0 }]));
    let mask_path = fixture_mask_path("zoned-orch-land-mask");
    sky_mask.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&src, &tgt);
    let entry = report.err_after;
    attach_zones(&src, &tgt, &mut report, &sky_mask, &sky_mask, &mask_path);
    // REWRITTEN (v1.18.0, J*). v1.17.1 pinned that the finished-frame curves
    // correct the land the inverted zone used to correct and that both shrunk
    // zones were then refused on the frame's structural swing (end error
    // 0.04087). On J* the global solve lands the land (its zone reads 0.006,
    // "already matches", so no inverted land zone forms), and the sky zone,
    // shrunk to k 0.166 by the boundary gate, earns its keep on the frame and
    // ships (the swing that refused it was the display objective's S reading
    // flat blocks a few codes apart as whole rank steps; see
    // `every_accepted_fixture_zone_still_passes_the_boundary_gate`).
    let zones: Vec<_> = report.recipe.masks.iter().filter(|m| m.role.is_zone()).map(|m| m.role).collect();
    // RE-READ for v1.18.0 step 1 (it was
    // `zoned_orchestration_leaves_the_matched_land_and_ships_the_shrunk_sky`):
    // under DEF-3 the global solve no longer lands the land (residual 0.189
    // -> 0.156, saturation pulled back from +60 to 0, D 0.290 at pixel
    // scale), so the land zone has something to correct and ships on its own
    // pixels (+0.02 EV, gains [0.98 1.01 1.01], zone residual 0.055 ->
    // 0.043, 5 FIT_QUANT) beside the sky, both kept by the boundary gate at
    // k 0.153; the frame 0.15638 -> 0.14769. That is the law the first
    // paragraph above states — the land zone attaches beside the sky's own
    // correction — and the render check below still reads a warm land pixel
    // (147, 108, 100). With DEF-3 alone reverted the land is landed by the
    // global solve and only the sky ships, as R1 pinned.
    assert_eq!(
        zones,
        vec![MaskRole::ZoneSky, MaskRole::ZoneLand],
        "the land zone ships beside the sky on this toy: {}",
        report.recipe.rationale
    );
    assert!(
        !report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_SHRUNK_REFUSED),
        "the shipped sky is not also refused: {}",
        report.recipe.rationale
    );
    assert!(
        report.err_after < entry,
        "the zone stage leaves the frame better than it found it: {entry} -> {}",
        report.err_after
    );
    // Render check: a land pixel must move toward the vivid warm target.
    let out = render::develop_preview(&src, &report.recipe).to_rgb8();
    let p = out.get_pixel(8, 4);
    let (r, b) = (p[0] as f32 / 255.0, p[2] as f32 / 255.0);
    assert!(r > b + 0.10, "land must turn warm (r >> b): {p:?}");
    mask_path.remove();
}

#[test]
fn zoned_fit_survives_a_composition_share_mismatch() {
    // The real-pair failure geometry (2026-07-09): the generative target
    // holds ~3× more sky than the source, so the FRAME-global look_err
    // barely moves (or drifts up) when the zone is repainted correctly —
    // the first gate (frame-global improvement) dropped a correction
    // whose zone moments landed almost exactly on the target's (measured
    // zone residual 0.507 → 0.015, global drift +0.0024). The zone-local
    // gate must attach it; the rationale must surface the composition
    // difference honestly.
    let (w, h) = (16u32, 16u32);
    let build = |sky: [f32; 3], sky_rows: u32| -> DynamicImage {
        let img = RgbImage::from_fn(w, h, |_, y| {
            let p = if y >= h - sky_rows { sky } else { [0.55f32, 0.45, 0.35] };
            image::Rgb(p.map(|c| (c * 255.0).round() as u8))
        });
        DynamicImage::ImageRgb8(img)
    };
    let mask_of = |sky_rows: u32| {
        GrayImage::from_fn(w, h, |_, y| {
            image::Luma([if y >= h - sky_rows { 255u8 } else { 0 }])
        })
    };
    // Source: 2 sky rows (12.5%). Target: 6 gold rows (37.5%) — 3× more.
    let src = build([0.60, 0.63, 0.67], 2);
    let tgt = build([0.92, 0.72, 0.48], 6);
    let (sm, tm) = (mask_of(2), mask_of(6));
    let mask_path = fixture_mask_path("zoned-orch-share-mask");
    sm.save(mask_path.path()).unwrap();
    let mut report = fit::fit_recipe(&src, &tgt);
    attach_zones(&src, &tgt, &mut report, &sm, &tm, &mask_path);
    assert!(
        report.recipe.rationale.contains("compositions differ")
            && report
                .notes
                .iter()
                .any(|note| note.key == crate::rationale::keys::ZONE_SHARE_NO_CORRECTION),
        "the structurally changed populations must disclose the bounded refusal: {}",
        report.recipe.rationale
    );
    assert!(
        report.recipe.masks.is_empty(),
        "neither composition-mismatched population carries two-sided evidence: {}",
        report.recipe.rationale
    );
    assert!(
        report.recipe.rationale.contains("compositions differ")
            || report.recipe.rationale.contains("share of the two frames differs"),
        "rationale must surface the share mismatch: {}",
        report.recipe.rationale
    );
    mask_path.remove();
}

#[test]
fn zoned_orchestration_skips_a_degenerate_sky() {
    // An empty sky mask must skip BOTH zones: without a valid sky
    // partition, "land" would mean "everything" — a weaker-gated re-run
    // of the global fit, not a semantic zone.
    let (src, tgt, _) = zoned_pair();
    let empty = GrayImage::from_pixel(16, 16, image::Luma([0u8]));
    let mask_path = fixture_mask_path("zoned-orch-empty-mask");
    let mut report = fit::fit_recipe(&src, &tgt);
    attach_zones(&src, &tgt, &mut report, &empty, &empty, &mask_path);
    assert!(report.recipe.masks.is_empty(), "no mask on a degenerate partition");
    assert!(
        report.recipe.rationale.contains("no usable sky partition"),
        "rationale must say why: {}",
        report.recipe.rationale
    );
}

#[test]
fn zoned_fit_degrades_gracefully_without_python() {
    // A missing/broken python must yield the plain global fit plus an
    // honest note — never an error (the graceful-fallback contract).
    let (src, tgt, _) = zoned_pair();
    let seg = SegmentOpts {
        python_bin: "autoshade-test-no-such-python".into(),
        // Must EXIST so the failure exercised is the launch, not the
        // script check.
        script: "Cargo.toml".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let mask_path = fixture_mask_path("zoned-orch-nopython-mask");
    let report = fit_recipe_zoned(&src, &tgt, &seg, &mask_path);
    // The FALLBACK attaches no zone: the sky/land pair needs the segmenter
    // that failed to launch. The later stages run on the global fit exactly
    // as they would without a zone step, under their own gates, and the
    // free-mask stage keeps a field mask on this fixture (frame 0.0315 ->
    // 0.0300): that stage's verdict, not the fallback's.
    assert!(
        report.recipe.masks.iter().all(|m| m.role == crate::recipe::MaskRole::Custom),
        "fallback must not attach a sky or land zone: {}",
        report.recipe.rationale
    );
    assert!(
        report.recipe.rationale.contains("automatic luminance-range fallback"),
        "rationale must explain the fallback: {}",
        report.recipe.rationale
    );
    assert!(
        report.notes.iter().any(|note| note.key == crate::rationale::keys::ZONED_UNAVAILABLE),
        "typed fallback verdict must name segmentation unavailability",
    );
    // The temporary segmentation inputs must not survive the fallback.
    for suffix in [".src-in.png", ".tgt-in.png", ".tgt-mask.png"] {
        let mut p = mask_path.path().as_os_str().to_owned();
        p.push(suffix);
        assert!(
            !std::path::Path::new(&p).exists(),
            "temp file {suffix} leaked past the fallback"
        );
    }
}

enum PlaneSource {
    Input,
    Black,
}

/// A loopback stand-in for BOTH segmentation bridges. The single-class
/// call gets its input copied back as the mask (argv 4 → 6); the
/// `--multi` call (argv 9) gets one "sky" plane beside the manifest —
/// the input again, or an all-black fixture — and a manifest that names
/// it. `width`/`height` are what the test EXPECTS the bridge to have
/// handed over: the bridge's own dimension check turns a wrong sizing
/// into a refusal instead of a silently different plane.
fn loopback_segment_opts(
    dir: &std::path::Path,
    plane: PlaneSource,
    width: u32,
    height: u32,
) -> SegmentOpts {
    let black = dir.join("black.png");
    GrayImage::from_pixel(width, height, image::Luma([0u8])).save(&black).unwrap();
    let manifest = |name: &str| {
        format!(
            "{{\"version\":1,\"width\":{width},\"height\":{height},\"planes\":[{{\"class_id\":2,\
                 \"label\":\"sky\",\"mean_confidence\":0.5,\"share\":0.5,\"path\":\"{name}\"}}]}}"
        )
    };
    // sh reads the plane name from the manifest path at run time via a
    // template; batch expands `%~n6` inline.
    std::fs::write(dir.join("manifest.tmpl"), manifest("PLANE")).unwrap();
    let (bat_plane, sh_plane) = match plane {
        PlaneSource::Input => ("%4".to_string(), "$4".to_string()),
        PlaneSource::Black => ("%~dp0black.png".to_string(), black.display().to_string()),
    };
    let bat = format!(
        "@echo off\r\nif \"%9\"==\"--multi\" (\r\n  copy /y \"{bat_plane}\" \"%~dpn6.class-2.png\" >nul\r\n  \
             echo {}>\"%6\"\r\n) else (\r\n  copy /y \"%4\" \"%6\" >nul\r\n)\r\nexit /b 0\r\n",
        manifest("%~n6.class-2.png")
    );
    let sh = format!(
        "if [ \"$9\" = \"--multi\" ]; then\n  stem=$(basename \"$6\" .json)\n  \
             cp \"{sh_plane}\" \"$(dirname \"$6\")/$stem.class-2.png\"\n  \
             sed \"s/PLANE/$stem.class-2.png/\" \"{}\" > \"$6\"\nelse\n  cp \"$4\" \"$6\"\nfi\nexit 0\n",
        dir.join("manifest.tmpl").display()
    );
    let python_bin = crate::write_stand_in(dir, "segment-stub", &bat, &sh);
    // The script must merely exist — the bridge refuses a missing one.
    let script = dir.join("segment.py");
    std::fs::write(&script, "# stand-in\n").unwrap();
    SegmentOpts {
        python_bin,
        script,
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    }
}

/// `segmentation_input` is the one sizing rule: borrow through 2048 px on
/// the long edge (2048 itself included), thumbnail above it.
#[test]
fn segmentation_input_downscales_only_above_the_edge() {
    let frame = |w, h| DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([9, 9, 9])));
    assert!(matches!(segmentation_input(&frame(1600, 1067)), std::borrow::Cow::Borrowed(_)));
    assert!(matches!(segmentation_input(&frame(2048, 1365)), std::borrow::Cow::Borrowed(_)));
    let big = frame(2400, 1600);
    let large = segmentation_input(&big);
    assert!(matches!(large, std::borrow::Cow::Owned(_)));
    assert_eq!(large.dimensions(), (2048, 1365));
}

/// The two bridges hand the sidecar identical inputs — the Rust-layer
/// falsifier behind the seeded legacy run's byte identity. The
/// multi-class bridge used to thumbnail unconditionally, and
/// `image::thumbnail` UPSCALES a smaller frame, so on every ≤2048 frame
/// (the calibration corpus is 1600 px) its sky plane differed from the
/// single-class mask while the Python-layer identity test, which fed both
/// modes the same file, stayed green. The stand-ins copy their input back
/// as mask/plane, so the bytes each bridge returns ARE what it sent.
#[test]
fn multi_and_single_class_inputs_are_prepared_identically() {
    for (tag, w, h, ew, eh) in [
        ("seg-input-native", 300u32, 200u32, 300u32, 200u32),
        ("seg-input-large", 2400, 1600, 2048, 1365),
    ] {
        let dir = crate::test_dir(tag);
        let seg = loopback_segment_opts(&dir, PlaneSource::Input, ew, eh);
        let frame = |seed: u8| {
            DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
                image::Rgb([
                    seed.wrapping_add((x % 7) as u8 * 9),
                    60u8.wrapping_add((y % 11) as u8 * 13),
                    128,
                ])
            }))
        };
        let (src, tgt) = (frame(40), frame(90));
        let legacy_path = crate::store::OwnedRaster::scratch(dir.join("legacy.png"));
        let multi_path = crate::store::OwnedRaster::scratch(dir.join("multi.png"));
        let (sm, tm) = segment_both(&src, &tgt, &seg, &legacy_path)
            .unwrap_or_else(|e| panic!("{tag}: single-class bridge: {e:#}"));
        let (regions, rasters, (ms, mt), _refinements, _widenings, _mattes) =
            segment_multiclass_both(&src, &tgt, &seg, &multi_path, 4)
                .unwrap_or_else(|e| panic!("{tag}: multi-class bridge: {e:#}"));
        assert_eq!(sm.dimensions(), (ew, eh), "{tag}: single-class input sizing");
        assert_eq!(ms.dimensions(), (ew, eh), "{tag}: multi-class input sizing");
        assert!(
            sm.as_raw() == ms.as_raw() && tm.as_raw() == mt.as_raw(),
            "{tag}: the two bridges handed the sidecar different bytes"
        );
        assert_eq!(regions.len(), 1, "{tag}: the one plane pairs into one region");
        for raster in rasters {
            raster.remove();
        }
        legacy_path.remove();
        multi_path.remove();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The multi-class layer failing is not the sky fit failing. With a broken
/// interpreter the multi bridge fails, the historical route (stubbed masks)
/// succeeds, and the report is that route's report plus ONE typed
/// `SEMANTIC_REGIONS_UNAVAILABLE` note — never `ZONED_UNAVAILABLE`, whose
/// text promises a luminance-range fallback that did not run.
///
/// R35 re-pinned this to the OVERFLOW of the 64-note cap, which its
/// carrier and band notes made this fixture trip; R36 raised the cap to
/// what a zoned fit actually carries and the original pin is back.
#[test]
fn multi_segmentation_failure_keeps_the_legacy_zones_with_its_own_note() {
    let (src, tgt, sky) = zoned_pair();
    // An ABSOLUTE interpreter path, because that is the real shape: the
    // bundled helper resolves one, and `AUTOSHADE_PYTHON` is one. A bare
    // name produces a sidecar error with no path in it, and a disclosure
    // test written against that fixture cannot fail — which is how the
    // first version of the guard below passed against its own mutation.
    // Derived from `temp_dir`, never written as a literal.
    let missing = std::env::temp_dir()
        .join("autoshade-e-leak-probe")
        .join("no-such-python");
    let seg = SegmentOpts {
        python_bin: missing.to_string_lossy().into_owned(),
        script: "Cargo.toml".into(),
        target: "sky".into(),
        reference_point: None,
        prompt_points: None,
    };
    let path = fixture_mask_path("multi-unavailable");
    sky.save(path.path()).unwrap();
    let base = crate::recipe::EditRecipe::default();
    SEGMENT_BOTH_OVERRIDE.with(|v| *v.borrow_mut() = Some((sky.clone(), sky.clone())));
    let multi = fit_recipe_zoned_with_regions(&src, &tgt, &seg, &path, &base, fit::FitOptions::default(), 4);
    SEGMENT_BOTH_OVERRIDE.with(|v| *v.borrow_mut() = Some((sky.clone(), sky.clone())));
    let legacy = fit_recipe_zoned_with_regions(&src, &tgt, &seg, &path, &base, fit::FitOptions::default(), 2);
    let own = multi
        .notes
        .iter()
        .filter(|n| n.key == crate::rationale::keys::SEMANTIC_REGIONS_UNAVAILABLE)
        .collect::<Vec<_>>();
    assert_eq!(own.len(), 1, "exactly one typed hand-off: {}", multi.recipe.rationale);
    assert!(
        !multi.notes.iter().any(|n| n.key == crate::rationale::TRUNCATED_SENTINEL),
        "a zoned fit must fit under the typed-note cap: {} notes",
        multi.notes.len()
    );
    // …and the hand-off's reason went through the disclosure door. The
    // sidecar's own error names paths; a rationale is user-visible and is
    // pasted into bug reports, so neither an absolute path nor an unbounded
    // traceback may reach it.
    let reason = own[0].args.iter().find(|(k, _)| *k == "e").map(|(_, v)| v.as_str());
    let reason = reason.expect("the hand-off note carries its reason");
    assert!(
        !reason.contains("autoshade-e-leak-probe") && !reason.contains('\n'),
        "the hand-off reason leaked this machine's layout or a multi-line trace: {reason}"
    );
    assert!(
        reason.contains("no-such-python"),
        "…while still SAYING which program could not be launched: {reason}"
    );
    assert!(
        reason.chars().count() <= 160,
        "the hand-off reason is unbounded ({} chars): {reason}",
        reason.chars().count()
    );
    assert!(
        !multi.notes.iter().any(|n| n.key == crate::rationale::keys::ZONED_UNAVAILABLE)
            && !multi.recipe.rationale.contains("luminance-range fallback"),
        "the sky/land route ran; no range fallback may be narrated: {}",
        multi.recipe.rationale
    );
    // Everything but that one appended sentence IS the historical route.
    let appended = crate::rationale::render_one(own[0]);
    assert_eq!(
        multi.recipe.rationale.strip_suffix(appended.as_str()),
        Some(legacy.recipe.rationale.as_str()),
        "the hand-off note is appended to the historical rationale, nothing else changes"
    );
    // Two runs of a raster-allocating pipeline can never claim the same
    // sibling FILENAMES: `OwnedRaster::claim_sibling` takes the first free
    // name beside the mask, and the first run's rasters are still on disk
    // when the second one runs. Before step 9 the comparison happened to
    // work because neither run KEPT a sibling raster; the differential rim
    // leaves the zone corrections weaker in this fixture, the spatial
    // sweep now finds residual worth two tiles and a free mask, and both
    // runs keep three. Compare with each distinct bitmap path replaced by
    // its order of first appearance - which is the property this test is
    // about (same masks, same sharing, same dials) and not an assertion
    // about which filename the filesystem handed out.
    let canonical = |recipe: &crate::recipe::EditRecipe| {
        let mut out = recipe.clone();
        out.rationale.clear();
        let mut seen: Vec<String> = Vec::new();
        for mask in &mut out.masks {
            // Every raster carrier, not just `Bitmap`: a zone's alpha rides
            // inside its Select Sky component and its claimed filename is
            // exactly the thing this canonicalisation exists to erase.
            if let Some(path) = render::geometry_raster_path_mut(&mut mask.mask) {
                let index = seen.iter().position(|p| p == path).unwrap_or_else(|| {
                    seen.push(path.clone());
                    seen.len() - 1
                });
                *path = format!("raster#{index}");
            }
        }
        out
    };
    let (a, b) = (canonical(&multi.recipe), canonical(&legacy.recipe));
    assert_eq!(serde_json::to_vec(&a).unwrap(), serde_json::to_vec(&b).unwrap());
    assert_eq!(
        multi.recipe.masks.len(),
        legacy.recipe.masks.len(),
        "and both routes must allocate the same number of masks"
    );
    assert_eq!(multi.err_after.to_bits(), legacy.err_after.to_bits());
    path.remove();
}

/// A manifest whose every plane misses the support floor resolves to NO
/// region. That is a hand-off to the SEEDED historical route — which
/// judges the sky partition on its own numbers, drops its anchor and runs
/// the sequencer — plus one typed `SEMANTIC_REGIONS_NONE` note; not a
/// fourth exit that rendered `{s}` placeholders and stranded the anchor.
#[test]
fn empty_semantic_region_set_hands_off_to_the_seeded_legacy_route() {
    let dir = crate::test_dir("seg-no-region");
    let (src, tgt, _) = zoned_pair();
    let seg = loopback_segment_opts(&dir, PlaneSource::Black, src.width(), src.height());
    let path = crate::store::OwnedRaster::scratch(dir.join("anchor.png"));
    let report = fit_recipe_zoned_with_regions(
        &src, &tgt, &seg, &path, &crate::recipe::EditRecipe::default(), fit::FitOptions::default(), 4,
    );
    assert!(
        report.notes.iter().any(|n| n.key == crate::rationale::keys::SEMANTIC_REGIONS_NONE
            && n.args.iter().any(|(k, v)| *k == "n" && v == "4")),
        "typed hand-off naming the requested count: {}",
        report.recipe.rationale
    );
    let no_partition = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONED_NO_PARTITION)
        .unwrap_or_else(|| panic!("the historical route judges the partition: {}", report.recipe.rationale));
    assert!(no_partition.args.iter().any(|(k, _)| *k == "s"));
    assert!(
        report.recipe.rationale.contains("sky covers 0% of the source frame")
            && !report.recipe.rationale.contains("{s}"),
        "numbers, not placeholders: {}",
        report.recipe.rationale
    );
    assert!(!path.path().exists(), "the anchor raster must not outlive a failed partition");
    assert!(report.recipe.masks.iter().all(|m| render::geometry_raster_path(&m.mask)
        .is_none_or(|path| !path.contains("mask-region-"))));
    let leftovers = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("multi") || n.contains("mask-region"))
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "sidecar inputs/planes leaked: {leftovers:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The release rule behind arbitration, on synthetic reports: the loser's
/// own rasters go, a raster both reports share stays, the anchor goes
/// only when nothing kept references it, and a raster outside the store
/// is never touched. The loopback arbitration test below exercises the
/// call sites; this pins the rule itself on every branch.
#[test]
fn release_unselected_rasters_keeps_exactly_what_the_kept_recipe_references() {
    let dir = crate::test_dir("release-unselected");
    let outside = crate::test_dir("release-unselected-outside");
    let file = |d: &std::path::Path, n: &str| {
        let p = d.join(n);
        std::fs::write(&p, b"x").unwrap();
        p.to_string_lossy().into_owned()
    };
    let report_with = |paths: &[&str]| {
        let mut report = neutral_report(
            &DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, image::Rgb([90, 90, 90]))),
            &DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, image::Rgb([110, 110, 110]))),
        );
        for p in paths {
            report.recipe.masks.push(crate::recipe::LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: (*p).to_string() },
                role: MaskRole::Custom,
                ..Default::default()
            });
        }
        report
    };
    // The same report shape, but referencing its rasters the way the REAL
    // sky/land route does now — through a Select Sky component.
    let zoned_report_with = |paths: &[&str]| {
        let mut report = neutral_report(
            &DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, image::Rgb([90, 90, 90]))),
            &DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, image::Rgb([110, 110, 110]))),
        );
        for p in paths {
            report.recipe.masks.push(crate::recipe::LocalAdjustment {
                mask: crate::recipe::MaskGeometry::select_sky(0.5, 0.3, false, (*p).to_string()),
                role: MaskRole::ZoneSky,
                ..Default::default()
            });
        }
        report
    };
    let anchor = crate::store::OwnedRaster::scratch(dir.join("anchor.png"));
    std::fs::write(anchor.path(), b"x").unwrap();
    let (loser_only, shared, foreign) = (
        file(&dir, "mask-region-2-sky.png"),
        file(&dir, "mask-zone-tile.png"),
        file(&outside, "mask-region-9-far.png"),
    );
    // Branch 1: the candidate loses, the kept report references the
    // shared raster and the anchor.
    let candidate = report_with(&[&loser_only, &shared, &foreign]);
    let kept = report_with(&[&shared, &anchor.path().to_string_lossy()]);
    release_unselected_rasters(&candidate, &kept, &anchor);
    assert!(!std::path::Path::new(&loser_only).exists(), "the loser's own raster is released");
    assert!(std::path::Path::new(&shared).exists(), "a raster the kept recipe references stays");
    assert!(anchor.path().exists(), "the anchor stays while the kept recipe references it");
    assert!(std::path::Path::new(&foreign).exists(), "a raster outside the store is never touched");
    // Branch 2: nothing kept references the anchor — it goes too.
    let kept = report_with(&[&shared]);
    release_unselected_rasters(&candidate, &kept, &anchor);
    assert!(!anchor.path().exists(), "an unreferenced anchor is released");
    assert!(std::path::Path::new(&shared).exists());
    // Branch 3: the kept report references the anchor through a SELECT SKY
    // component, which is what the sky/land route produces. Reading the
    // geometry's variant instead of its raster path here would have
    // deleted the alpha the winning report renders both zones from — the
    // photographer's zoned fit landing on a missing file.
    std::fs::write(anchor.path(), b"x").unwrap();
    let loser = file(&dir, "mask-region-3-sky.png");
    let candidate = report_with(&[&loser]);
    let kept = zoned_report_with(&[&anchor.path().to_string_lossy()]);
    release_unselected_rasters(&candidate, &kept, &anchor);
    assert!(
        anchor.path().exists(),
        "a zone's own alpha is a reference like any other — the anchor must survive"
    );
    assert!(!std::path::Path::new(&loser).exists(), "…and the loser's raster still goes");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&outside);
}

/// The multi arm's arbitration and claim hygiene, deterministically. With
/// the loopback sidecar both routes see the SAME sky plane (the frame's
/// luma), so the seeded two-region run inside the multi arm is exactly an
/// independent unseeded one. A refusal must hand that report back byte
/// for byte plus ONE note — never a transplant — with every region
/// raster gone; a win must beat it on the shared ruler. Either way no
/// claim outlives the recipe that references it and no sidecar file
/// survives the run.
#[test]
fn loopback_multi_run_arbitrates_against_the_seeded_two() {
    let (src, tgt, _) = zoned_pair();
    let base = crate::recipe::EditRecipe::default();
    let run = |tag: &str, regions: usize| {
        let dir = crate::test_dir(tag);
        let seg = loopback_segment_opts(&dir, PlaneSource::Input, src.width(), src.height());
        let anchor = crate::store::OwnedRaster::scratch(dir.join("anchor.png"));
        let report = fit_recipe_zoned_with_regions(&src, &tgt, &seg, &anchor, &base, fit::FitOptions::default(), regions);
        (dir, report)
    };
    let (four_dir, four) = run("seg-arb-four", 4);
    let (two_dir, two) = run("seg-arb-two", 2);
    let files = |dir: &std::path::Path| {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    // By FILE, not by variant — see the sibling helper above.
    let referenced = |report: &FitReport| {
        report.recipe.masks.iter().filter_map(|m| {
            std::path::Path::new(render::geometry_raster_path(&m.mask)?)
                .file_name().map(|n| n.to_string_lossy().into_owned())
        }).collect::<std::collections::HashSet<_>>()
    };
    // Claim hygiene holds on both branches: every mask raster on disk is
    // referenced by the recipe, every referenced raster exists, and no
    // sidecar input/manifest/plane survived the run.
    let on_disk = files(&four_dir);
    let refs = referenced(&four);
    for name in &on_disk {
        assert!(!name.contains(".multi"), "sidecar file leaked: {name}");
        if name.starts_with("mask-") || name == "anchor.png" {
            assert!(refs.contains(name), "orphan claim {name}; recipe references {refs:?}");
        }
    }
    for name in &refs {
        assert!(on_disk.contains(name), "referenced raster {name} missing from {on_disk:?}");
    }
    let refusals = four.notes.iter()
        .filter(|n| n.key == crate::rationale::keys::REGION_FRAME_REFUSED)
        .collect::<Vec<_>>();
    if let [refusal] = refusals[..] {
        // The reference report, byte for byte, plus exactly the one note.
        assert_eq!(
            four.recipe.rationale,
            format!("{}{}", two.recipe.rationale, crate::rationale::render_one(refusal)),
            "a refusal appends one note to the reference rationale and transplants nothing"
        );
        // A raster's claimed NAME is not content: both candidates claim in
        // the one develop store, the refused one first, so a raster the kept
        // run also claims is numbered past the refused one's (`-2.png`, see
        // `OwnedRaster::claim_sibling`) — on the night pair (2026-10-20) the
        // multi run's free mask did exactly that. The claim suffix is folded
        // out; everything the raster carries is still compared byte for byte.
        let unclaimed = |json: String| -> String {
            let mut out = String::with_capacity(json.len());
            let mut rest = json.as_str();
            while let Some(at) = rest.find(".png") {
                let head = &rest[..at];
                let stem = head.trim_end_matches(|c: char| c.is_ascii_digit());
                out.push_str(if stem.len() < head.len() && stem.ends_with('-') { &stem[..stem.len() - 1] } else { head });
                out.push_str(".png");
                rest = &rest[at + 4..];
            }
            out.push_str(rest);
            out
        };
        let normalise = |r: &FitReport, dir: &std::path::Path| {
            let mut recipe = r.recipe.clone();
            recipe.rationale.clear();
            unclaimed(serde_json::to_string(&recipe).unwrap().replace(&dir.to_string_lossy().replace('\\', "\\\\"), "<dir>")
                .replace(&dir.to_string_lossy().into_owned(), "<dir>"))
        };
        assert_eq!(normalise(&four, &four_dir), normalise(&two, &two_dir));
        assert_eq!(four.err_after.to_bits(), two.err_after.to_bits());
        assert!(refs.iter().all(|n| !n.starts_with("mask-region-")), "refused regions must not ship: {refs:?}");
    } else {
        assert!(refusals.is_empty(), "at most one arbitration note: {}", four.recipe.rationale);
        let ruler = &two.evidence;
        assert!(
            frame_err_under(&src, &tgt, &four, ruler) < frame_err_under(&src, &tgt, &two, ruler),
            "a kept multi result beats the reference on the reference's own ruler"
        );
    }
    let _ = std::fs::remove_dir_all(&four_dir);
    let _ = std::fs::remove_dir_all(&two_dir);
}

// Mutation guard: make `fit::append_finished_disclosure` an unconditional
// early return. The required finished disclosure below then panics, so
// deleting the disclosure cannot satisfy this test.
#[test]
fn unrepresented_note_is_derived_from_the_finished_zoned_render() {
    let (w, h) = (64u32, 64u32);
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
        let texture = (((x * 17) % 23) as f32 / 22.0 - 0.5) * 0.08;
        let v = (0.36 + 0.36 * x as f32 / (w - 1) as f32 + texture).clamp(0.08, 0.92);
        let p = [0.70 * v, 0.82 * v, v];
        image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
    }));
    let sky_mask = GrayImage::from_fn(w, h, |_, y| {
        image::Luma([if y >= 40 { 255u8 } else { 0 }])
    });
    let path = fixture_mask_path("zoned-finished-disclosure");
    sky_mask.save(path.path()).unwrap();
    let truth = crate::recipe::EditRecipe {
        masks: vec![LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
            role: MaskRole::ZoneSky,
            amount: 1.0,
            color_gains: Some([0.20, 0.65, 1.50]),
            ..Default::default()
        }],
        ..Default::default()
    };
    let tgt = render::develop_preview(&src, &truth);
    let eager = fit::fit_recipe(&src, &tgt);
    let eager_note = eager
        .notes
        .iter()
        .find(|n| {
            n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED
                || n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
        })
        .unwrap_or_else(|| {
            panic!(
                "premise: the pre-zone render leaves a measurable unrepresented colour residual: {:?} {:.3}->{:.3} {}",
                eager.mode,
                eager.err_before,
                eager.err_after,
                eager.recipe.rationale,
            )
        });
    let eager_controls = eager_note
        .args
        .iter()
        .find(|(key, _)| *key == "controls")
        .expect("controls arg")
        .1
        .clone();

    let mut report = fit::fit_recipe_from_promoted_with_disclosure(
        &src,
        &tgt,
        &crate::recipe::EditRecipe::default(),
        false,
        true,
        None,
    );
    assert!(!report
        .notes
        .iter()
        .any(|n| {
            n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED
                || n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
        }));
    attach_zones_with_divergence(
        &src,
        &tgt,
        &mut report,
        &sky_mask,
        &sky_mask,
        &path,
        ZoneDivergences {
            sky: ZoneDivergence { divergence: divergence(0.80), share: 0.375 },
            land: ZoneDivergence { divergence: divergence(0.0), share: 0.625 },
        },
    );
    assert!(
        !report.recipe.masks.is_empty(),
        "fixture must deliver a zoned render: {}",
        report.recipe.rationale,
    );
    let finished_controls = report
        .notes
        .iter()
        .find(|n| {
            n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED
                || n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_UNREPRESENTED
        })
        .and_then(|n| n.args.iter().find(|(key, _)| *key == "controls"))
        .map(|(_, value)| value.as_str());
    let finished_controls = finished_controls.unwrap_or_else(|| {
        panic!("the finished zoned render must carry its disclosure: {}", report.recipe.rationale)
    });
    assert_ne!(
        finished_controls,
        eager_controls.as_str(),
        "the disclosure was copied from the pre-zone render"
    );
    path.remove();
}

/// A sky of two populations, blue sky and neutral cloud, over a brown land.
/// `target_of` makes the target from each source pixel (display-encoded in,
/// display-encoded out); the grain is one draw shared by both sides, so every
/// pixel is its own counterpart and the zones pair at pixel scale.
fn two_population_sky_pair(
    target_of: &dyn Fn([f32; 3], bool) -> [f32; 3],
) -> (DynamicImage, DynamicImage, GrayImage, Vec<bool>) {
    let (w, h) = (64u32, 64u32);
    let hash = |i: u32| {
        let mut v = i.wrapping_mul(747796405).wrapping_add(2891336453);
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 10_000) as f32 / 10_000.0 - 0.5
    };
    // Clouds are whole 8x8 blocks, so they survive the analysis downscale.
    let cloud = |x: u32, y: u32| y < h / 2 && hash(1_000_003 + (y / 8) * 8 + x / 8) > 0.15;
    let source_px = |x: u32, y: u32| -> [f32; 3] {
        let grain = 0.06 * hash(y * w + x);
        if y >= h / 2 {
            [0.42 + grain, 0.34 + grain, 0.25 + grain]
        } else if cloud(x, y) {
            [0.86 + grain, 0.86 + grain, 0.86 + grain]
        } else {
            [0.47 + grain, 0.63 + grain, 0.86 + grain]
        }
    };
    let quantise = |p: [f32; 3]| image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| quantise(source_px(x, y))));
    let tgt = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let p = src.as_rgb8().expect("rgb8").get_pixel(x, y).0.map(|c| c as f32 / 255.0);
        quantise(target_of(p, y < h / 2))
    }));
    let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y < h / 2 { 255 } else { 0 }]));
    let clouds = (0..w * h).map(|i| cloud(i % w, i / w)).collect();
    (src, tgt, mask, clouds)
}

/// The angle between two Oklab directions, in degrees.
fn oklab_degrees_apart(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[1].atan2(a[0]) - b[1].atan2(b[0])).to_degrees().rem_euclid(360.0).min(
        (b[1].atan2(b[0]) - a[1].atan2(a[0])).to_degrees().rem_euclid(360.0),
    )
}

/// The estimator alone (`tint::read_zone_tint`), on pairs whose target IS the
/// candidate under one mask Color tint (hue 231, amount 0.5, the render's own
/// `render::tint_px`), with a quarter of the zone at half coverage: the read
/// returns that direction and that amount, because the solve is the model's
/// own least squares. The hue comes back on Lightroom's wheel to the tenth
/// of a degree the recipe stores.
#[test]
fn the_tint_read_returns_the_tint_a_target_was_made_with() {
    let dir = crate::render::tint_push(231.0);
    let k = 0.5;
    let candidate: Vec<[f32; 3]> = (0..4000)
        .map(|i| {
            let u = (i % 97) as f32 / 96.0;
            match i % 3 {
                0 => [0.2 + 0.7 * u; 3],
                1 => [0.30 + 0.25 * u, 0.42 + 0.3 * u, 0.62 + 0.3 * u],
                _ => [0.5 + 0.3 * u, 0.4 + 0.25 * u, 0.3 + 0.2 * u],
            }
        })
        .collect();
    let cover = |i: usize| if i.is_multiple_of(4) { 0.5 } else { 1.0 };
    let target: Vec<[f32; 3]> =
        candidate.iter().enumerate().map(|(i, p)| crate::render::tint_px(*p, dir, k * cover(i))).collect();
    let read = super::tint::read_zone_tint(&candidate, &target, &|_| 1.0, &cover).expect("a tint to read");
    assert!((read.hue - 231.0).abs() <= 0.5, "hue {}", read.hue);
    let wheel = crate::render::tint_push(read.hue);
    assert!(oklab_degrees_apart(wheel, dir) <= 0.5, "hue {} pushes {wheel:?}, not {dir:?}", read.hue);
    assert!((read.amount - 0.5).abs() <= 0.005, "amount {}", read.amount);
    assert_eq!(read.pairs, 4000);
    // Nothing to read where the pairs agree.
    assert!(super::tint::read_zone_tint(&candidate, &candidate, &|_| 1.0, &cover).is_none());
}

/// The zone tint trial, end to end. A target that IS the source under one
/// mask Color tint on the sky (hue 231, amount 0.5; the land untouched) is
/// the move per-channel gains cannot make: solved from the zone's mean they
/// push the blue sky and the neutral clouds two different ways. The trial
/// reads one tint from every paired pixel, both populations alike, and
/// replaces the plain solve because it lands the zone closer pixel by
/// pixel: its read (the `ZONE_TINTED` sentence) is within 3° of the
/// target's push and within 0.03 of its amount, the colour voucher lets it
/// through, the sky ships that hue, and BOTH populations move toward their
/// targets. The amount SHIPPED is not pinned: the boundary-continuity gate
/// that runs after every zone may shrink the whole correction at the
/// horizon (it does here, by its own sentence), and that verdict is its own.
#[test]
fn a_zone_whose_target_is_one_tint_away_ships_that_tint_and_both_populations_converge() {
    let (dir, k) = (crate::render::tint_push(231.0), 0.5);
    let (src, tgt, mask, clouds) =
        two_population_sky_pair(&|p, sky| if sky { crate::render::tint_px(p, dir, k) } else { p });
    let path = fixture_mask_path("zoned-tinted-sky");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);

    attach_zones(&src, &tgt, &mut report, &mask, &mask, &path);

    let sky = report
        .recipe
        .masks
        .iter()
        .find(|m| m.role == MaskRole::ZoneSky)
        .unwrap_or_else(|| panic!("the sky zone must attach: {}", report.recipe.rationale));
    let read = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::ZONE_TINTED)
        .unwrap_or_else(|| panic!("the tint says so: {}", report.recipe.rationale));
    let arg = |k: &str| read.args.iter().find(|(key, _)| *key == k).map(|(_, v)| v.parse::<f32>().unwrap()).unwrap();
    assert!((arg("amount") - 0.5).abs() <= 0.03, "the read amount {} is the target's 0.5", arg("amount"));
    let read_push = crate::render::tint_push(arg("hue"));
    assert!(oklab_degrees_apart(read_push, dir) <= 3.0, "the read hue {} pushes the target's way", arg("hue"));
    assert!(
        sky.toning_amount > 0.0,
        "the sky must ship the tint (gains {:?} sat {}): {}",
        sky.color_gains,
        sky.saturation,
        report.recipe.rationale
    );
    let pushed = crate::render::tint_push(sky.toning_hue);
    assert!(
        oklab_degrees_apart(pushed, dir) <= 3.0,
        "the tint's hue {} must push the target's way ({:.1}° apart)",
        sky.toning_hue,
        oklab_degrees_apart(pushed, dir)
    );
    for refused in [
        crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR,
        crate::rationale::keys::ZONE_COLOUR_WITHHELD_BY_QUALITY,
    ] {
        assert!(
            !report.notes.iter().any(|n| n.key == refused && n.args.iter().any(|(k, v)| *k == "label" && v == MaskRole::ZoneSky.tag())),
            "the voucher must pass the tinted sky: {}",
            report.recipe.rationale
        );
    }
    let (s, t) = (fit::pixels_of(&src), fit::pixels_of(&tgt));
    let rendered = fit::pixels_of(&render::develop_preview(&src, &report.recipe));
    let gap = |px: &[[f32; 3]], want_cloud: bool| {
        let (mut sum, mut n) = (0.0f32, 0usize);
        for i in 0..px.len() / 2 {
            if clouds[i] == want_cloud {
                sum += (0..3).map(|c| (px[i][c] - t[i][c]).abs()).sum::<f32>();
                n += 1;
            }
        }
        sum / n.max(1) as f32
    };
    for (name, is_cloud) in [("clouds", true), ("sky", false)] {
        let (before, after) = (gap(&s, is_cloud), gap(&rendered, is_cloud));
        assert!(after < 0.75 * before, "{name} must converge: {before:.4} -> {after:.4}: {}", report.recipe.rationale);
    }
    path.remove();
}

/// The other side of the trial: a sky whose target is the source under
/// per-channel GAINS (a warm recolour, the move the plain solve is built
/// for) must not come back as a tint. The tinted candidate's own gains can
/// at best land where the plain solve does, so the trial's margin keeps the
/// plain one and the sky's tint amount stays ≤ 0.02 — the tint does not
/// absorb what gains explain.
#[test]
fn a_zone_whose_target_is_a_gains_recolour_does_not_read_as_a_tint() {
    let gains = [1.12, 1.0, 0.86];
    let (src, tgt, mask, _) = two_population_sky_pair(&|p, sky| {
        if sky {
            let lin = p.map(render::srgb_to_linear);
            std::array::from_fn(|c| render::linear_to_srgb(lin[c] * gains[c]))
        } else {
            p
        }
    });
    let path = fixture_mask_path("zoned-gains-sky");
    mask.save(path.path()).unwrap();
    let mut report = neutral_report(&src, &tgt);

    attach_zones(&src, &tgt, &mut report, &mask, &mask, &path);

    let sky = report
        .recipe
        .masks
        .iter()
        .find(|m| m.role == MaskRole::ZoneSky)
        .unwrap_or_else(|| panic!("the sky zone must attach: {}", report.recipe.rationale));
    assert!(
        sky.toning_amount <= 2.0,
        "a gains recolour read as a tint of {} toward {}: {}",
        sky.toning_amount,
        sky.toning_hue,
        report.recipe.rationale
    );
    assert!(
        !report.notes.iter().any(|n| n.key == crate::rationale::keys::ZONE_TINTED),
        "no tint sentence for a gains recolour: {}",
        report.recipe.rationale
    );
    path.remove();
}

/// The colour field's PINNED path, REWRITTEN for v1.18.0 (R1). A settled
/// zone — its render already the target's — woven through the frame in 8-px
/// stripes, finer than the field's 12-column grid, and 0.16 darker than its
/// neighbours, close enough that the two share the field's luma vertices.
/// Every other pixel asks for +0.10 of light. A field solved for the frame
/// cannot give the stripes a vertex of their own, so it lifts them with their
/// neighbours; the zone is pinned (its own render as its target at full
/// weight) and the field solved again. Until v1.18.0 that re-solve had to
/// hold the zone within the 0.02 a zone could cost, and it did, so the field
/// shipped. R1 made the per-zone half of the field's do-no-harm "a loss of
/// any size refuses" (`field::zone_regressed`): the re-solve still lifts the
/// stripes (the zone reads 0.0061 -> 0.0120 on it), so the field is given
/// back, says which zone it would have cost, and ships nothing.
#[test]
fn a_zone_the_pinned_field_still_regresses_takes_the_field_back() {
    pinned_zone_fixture(None, "field-pin");
}

/// The same zone carrying little frame evidence (weight 0.1, where the night
/// pair's settled tile sat in content the evidence down-weights). The pin's
/// full weight must reach the support-free pass B that most cells take, not
/// pass A alone: the re-solve then hears the zone as it hears the
/// full-evidence one, and the zone's cost on it reads alike (measured 0.00634
/// against 0.00592 at full evidence) — and is given back by the same law.
#[test]
fn a_pinned_zone_is_heard_at_full_weight_by_both_field_solves() {
    let (low_before, low_after) = pinned_zone_fixture(Some(0.1), "field-pin-low");
    let (full_before, full_after) = pinned_zone_fixture(None, "field-pin-full");
    let (low, full) = (low_after - low_before, full_after - full_before);
    assert!(
        (low / full - 1.0).abs() <= 0.15,
        "the pinned re-solve hears the low-evidence zone as it hears the full-evidence one: \
         cost {low:.6} against {full:.6}"
    );
}

/// Runs the settled-zone fixture and returns the regressed zone's reading
/// (before, after) from the give-back sentence.
fn pinned_zone_fixture(zone_evidence: Option<f32>, name: &str) -> (f32, f32) {
    let (w, h) = (96u32, 64u32);
    let in_zone = |x: u32| (32..64).contains(&x) && (x / 8).is_multiple_of(2);
    let grey = |x: u32, y: u32| {
        0.40 + if in_zone(x) { -0.16 } else { 0.0 } + 0.03 * ((x * 7 + y * 13) % 5) as f32 / 4.0
    };
    let to8 = |p: [f32; 3]| image::Rgb(p.map(|c| (c * 255.0).round() as u8));
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| to8([grey(x, y); 3])));
    let tgt = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = grey(x, y);
        to8(if in_zone(x) { [v; 3] } else { [v + 0.10; 3] })
    }));
    // One raster per run: the runs go in parallel, and each removes its own.
    let path = fixture_mask_path(name);
    GrayImage::from_fn(w, h, |x, _| image::Luma([if in_zone(x) { 255u8 } else { 0 }]))
        .save(path.path())
        .unwrap();
    let mut report = neutral_report(&src, &tgt);
    if let Some(weight) = zone_evidence {
        // The analysis frame is this fixture scaled; map each of its columns
        // back to the fixture's own.
        let stride = report.evidence.width as usize;
        for (i, w8) in report.evidence.source_weights.iter_mut().enumerate() {
            if in_zone(((i % stride) * w as usize / stride) as u32) {
                *w8 = weight;
            }
        }
    }
    report.recipe.masks.push(crate::recipe::LocalAdjustment {
        mask: crate::recipe::MaskGeometry::Bitmap { path: path.path().to_string_lossy().into_owned() },
        name: "settled".into(),
        amount: 1.0,
        ..Default::default()
    });
    let (s, t) = fit::analysis_pair(&src, &tgt);
    report.err_after = fit::look_err_with_evidence(
        &fit::pixels_of(&render::develop_preview(&s, &report.recipe)),
        &fit::pixels_of(&t),
        &report.evidence,
    );
    super::field::attach_colour_field(
        &src,
        &tgt,
        &mut report,
        crate::recipe::GradeStrength::new(0.85),
        &[],
    );
    let read = |note: &crate::rationale::Note, key: &str| -> f32 {
        note.args.iter().find(|(k, _)| *k == key).map(|(_, v)| v.parse().unwrap()).unwrap()
    };
    let regressed = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIELD_ZONE_REGRESSED)
        .unwrap_or_else(|| panic!("the pinned re-solve still costs the settled zone: {}", report.recipe.rationale));
    assert!(
        regressed.args.iter().any(|(k, v)| *k == "label" && v == "settled"),
        "the give-back names the zone: {regressed:?}"
    );
    let (before, after) = (read(regressed, "before"), read(regressed, "after"));
    assert!(after > before, "premise: the re-solved field costs the zone ({before} -> {after})");
    assert!(
        !report.notes.iter().any(|n| n.key == crate::rationale::keys::FIELD_ZONE_PINNED),
        "a pin that did not hold is not reported as shipped: {}",
        report.recipe.rationale
    );
    assert!(report.recipe.colour_field.is_none(), "the field is given back: {}", report.recipe.rationale);
    std::fs::remove_file(path.path()).ok();
    (before, after)
}

/// R21 (2): the owed treatment is each content's LIGHT through its own
/// multiplier. At the measured night-horizon pixel (reference red 39,
/// settled sky 47, settled land 11 codes, area share a = 0.64, the land
/// zone's red gain x14.5, the sky's x1) the light form owes ~53 codes;
/// the area share applied to the whole light owed 97, which no pixel of
/// the frame asked for (the rendered pixel read 47). MUTATION: make
/// `light_share` return the area share — the light form then reads 97.
#[test]
fn the_owed_treatment_carries_each_contents_light_through_its_own_multiplier() {
    let lin = |code: f32| render::srgb_to_linear(code / 255.0);
    let (v, sky, land, a, m_sky, m_land) = (39.0, 47.0, 11.0, 0.64f32, 1.0f32, 14.5f32);
    let owe = |share: f32| render::linear_to_srgb((share * m_sky + (1.0 - share) * m_land) * lin(v)) * 255.0;
    let light = owe(super::boundary::light_share(a, lin(sky), lin(land), 1.0));
    let area = owe(a);
    assert!((light - 53.0).abs() <= 1.5, "the light form owes ~53 codes, read {light:.1}");
    assert!((area - 97.0).abs() <= 1.0, "the area form it replaces owed 97, read {area:.1}");
    assert_eq!(super::boundary::light_share(1.0, lin(sky), lin(land), 1.0), 1.0, "an all-sky pixel is owed the sky's treatment");
    assert_eq!(super::boundary::light_share(a, lin(sky), lin(land), 0.0), 1.0, "an untrusted mixture is read as sky, as before");
}
