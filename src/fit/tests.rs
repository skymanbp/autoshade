
/// L06-1/2: a zero-variance pair (lens-cap frame against itself) must
/// refuse to fit — the CDF inverse would produce a constant tone map and
/// err==0 would accept it silently.
#[test]
fn a_degenerate_pair_refuses_to_fit_and_says_why() {
    use image::{DynamicImage, RgbImage};
    let black = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0])));
    let report = fit_recipe(&black, &black);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_DEGENERATE),
        "the refusal is disclosed through the rationale channel: {:?}",
        report.recipe.rationale
    );
    assert!(report.recipe.tone_curve.is_empty(), "no constant tone map is produced");
    assert_eq!(report.recipe.exposure_ev, 0.0, "the recipe stays neutral");
}

use super::*;

#[test]
fn fit_budget_scales_monotonically_with_strength() {
    let low = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.0));
    let mid = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.65));
    let high = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0));
    assert!(low.ev < mid.ev && mid.ev < high.ev);
    assert!(low.sat < mid.sat && mid.sat < high.sat);
    assert!(low.wb_gain.0 > mid.wb_gain.0 && mid.wb_gain.0 > high.wb_gain.0);
    assert!(low.wb_gain.1 < mid.wb_gain.1 && mid.wb_gain.1 < high.wb_gain.1);
    assert!(low.cast_ratio < mid.cast_ratio && mid.cast_ratio < high.cast_ratio);
    assert!(low.zone_gain.0 > mid.zone_gain.0 && mid.zone_gain.0 > high.zone_gain.0);
    assert!(low.zone_gain.1 < mid.zone_gain.1 && mid.zone_gain.1 < high.zone_gain.1);
    // R34 §D4. The field's gain bound is FLAT up to the default — the
    // field does not ship there, so a dial that moved it below the default
    // would be a number with no behaviour behind it — and opens above it.
    assert_eq!(low.field_gain, mid.field_gain, "flat where the field never ships");
    assert!(mid.field_gain < high.field_gain, "and monotone where it does");
    // The two calibration points the reference pair set: 0.85 must reach
    // the measured sky demand (R +0.465) and 1.0 must clear 0.75.
    let at85 = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.85));
    assert!(at85.field_gain >= 0.50, "0.85 reaches the reference sky: {}", at85.field_gain);
    assert!(high.field_gain >= 0.75, "1.0 reaches the full ladder: {}", high.field_gain);
    assert_eq!(low.vetoes, VetoPolicy::Withhold);
    assert_eq!(high.vetoes, VetoPolicy::Disclose);
}

#[test]
fn fit_budget_default_is_byte_identical_to_pre_f1() {
    let b = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.65));
    assert_eq!(b, FitBudget {
        ev: 1.0,
        sat: 30.0,
        wb_gain: (0.80, 1.25),
        wb_ratio: 1.40,
        wb_rotation_share: ROT_SHARE,
        cast_ratio: CAST_ACCEPT_RATIO,
        slope: (0.5, 1.5),
        confidence_cap: 0.50,
        hsl_band: HSL_BAND_LIMIT_DEFAULT,
        // R33 §F added this axis; its DEFAULT point is the pair of
        // constants the zone shrink used to carry alone, so the shipped
        // default recipe is unchanged and the dial now reaches the zone.
        zone_gain: (
            crate::fit_zoned::ZONE_ATMOS_GAIN_MIN,
            crate::fit_zoned::ZONE_ATMOS_GAIN_MAX,
        ),
        // R34 §D4 added this axis; its DEFAULT point is the analyzer's own
        // `BOUNDS_HIGH` gain, so a default-strength recipe — which never
        // ships a field at all — is byte-identical.
        field_gain: crate::fit_field::default_gain_bound(),
        vetoes: VetoPolicy::Withhold,
    });
    assert_eq!(60.0 * b.sat / ATMOSPHERE_SAT_LIMIT, 60.0);
    assert_eq!(b.cast_ratio, CAST_ACCEPT_RATIO);
    assert_eq!(b.wb_rotation_share, ROT_SHARE);
    assert_eq!(RESIDUAL_SLOPE_CAP * b.slope.1 / ATMOSPHERE_CURVE_SLOPE_MAX, RESIDUAL_SLOPE_CAP);

    // The legacy provider wrapper and the F1 options path must serialize
    // the same ordinary Full-mode solve at the pinned default. This is a
    // small in-repo surrogate for the external calibration/live corpus.
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(32, 32, |x, y| {
        let v = 24 + ((x + y) % 180) as u8;
        image::Rgb([v, v, v])
    }));
    let target = DynamicImage::ImageRgb8(RgbImage::from_fn(32, 32, |x, y| {
        let v = 34 + ((x + y) % 180) as u8;
        image::Rgb([v, v, v])
    }));
    let legacy = fit_recipe_from_promoted_with_disclosure(
        &src,
        &target,
        &EditRecipe::default(),
        false,
        false,
        None,
    );
    let f1 = fit_recipe_from_with(
        &src,
        &target,
        &EditRecipe::default(),
        FitOptions { strength: crate::recipe::GradeStrength::new(0.65), provider: None },
    );
    assert_eq!(
        serde_json::to_value(&legacy.recipe).unwrap(),
        serde_json::to_value(&f1.recipe).unwrap(),
        "the default options path changed an ordinary Full recipe"
    );
}

/// The frame under a different illuminant: same pixels, same layout, one
/// per-channel scale. Nothing about the scene changed except its light.
fn relit(source: &DynamicImage, gains: [f32; 3]) -> DynamicImage {
    use image::GenericImageView;
    let (w, h) = (source.width(), source.height());
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let p = source.get_pixel(x, y).0;
        image::Rgb(std::array::from_fn(|c| {
            (p[c] as f32 * gains[c]).round().clamp(0.0, 255.0) as u8
        }))
    }))
}

/// R33 §E, the positive half: a FULL-mode pair whose light changed colour
/// now gets a white balance.
///
/// Until R33 this assertion was impossible to write: `temperature_k` was
/// assigned in exactly one function, on the branch taken when the pair's
/// structure was judged unrecoverable, so the commonest pair there is —
/// the same frame under different light — had no white-balance stage at
/// all and had to express the whole cast through saturation, the per-band
/// mixer and three channel curves. The gates then (correctly) refused most
/// of that, and the fit under-reached on colour by construction.
#[test]
fn a_full_mode_pair_whose_light_changed_colour_gets_a_white_balance() {
    let source = synth();
    // Gentle on purpose: the estimator works in LINEAR light, where an
    // sRGB gain pair of 1.06 / 0.95 is a gain ratio of 1.27 — inside the
    // shipped default's 1.40 white-balance budget, so this pair tests the
    // stage and the admission rather than the budget.
    let target = relit(&source, [1.06, 1.0, 0.95]);
    let report = fit_recipe(&source, &target);
    assert_eq!(report.mode, FitMode::Full, "premise: the structure is untouched");
    assert_eq!(report.pairing, PairingScale::Pixel, "premise: the texture survived");
    let k = report.recipe.temperature_k.expect(
        "a Full-mode pair under warmer light must be able to say so through white balance",
    );
    assert!(k > 5500.0, "…and the warmer light reads warmer than as-shot: {k}");
    // ADMITTED BY THE CELLS, and the report says which verdict it got.
    assert!(
        report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_WB_CELLS_VOUCHED),
        "the admission is disclosed: {}",
        report.recipe.rationale
    );
    assert!(
        !report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_WB_CELLS_REFUSED),
        "…and only one of the two verdicts is ever emitted"
    );
}

/// …and the negative half, which is the anti-laundering rule: the stage
/// solves from the population, but what SHIPS is decided by the target's
/// own cells. Feed it a pair whose light did not change and the estimator
/// has nothing to find; feed it one where the demand moves the frame the
/// wrong way and the cells return it to as-shot.
#[test]
fn a_full_mode_white_balance_ships_only_where_the_target_cells_vouch_it() {
    // No cast at all: nothing to solve, nothing to disclose.
    let source = synth();
    let report = fit_recipe(&source, &source);
    assert_eq!(report.recipe.temperature_k, None, "an identity pair invents no illuminant");
    assert!(
        !report.notes.iter().any(|n| {
            n.key == crate::rationale::keys::FIT_NOTE_WB_CELLS_VOUCHED
                || n.key == crate::rationale::keys::FIT_NOTE_WB_CELLS_REFUSED
        }),
        "…and says nothing about a stage that never moved: {}",
        report.recipe.rationale
    );
    // The instrument itself, on the render the stage would have shipped:
    // a demand pulled the WRONG way is refused over the whole frame.
    let (s_img, t_img) = analysis_pair(&source, &relit(&source, [1.16, 1.0, 0.84]));
    let (sp, tp) = (pixels_of(&s_img), pixels_of(&t_img));
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let cells = crate::fit_cells::PairedCells::build(&tp, s_img.width(), s_img.height(), &evidence)
        .expect("a populated pair builds cells");
    let cool = EditRecipe { temperature_k: Some(3200.0), ..Default::default() };
    let backwards = pixels_of(&render::develop_preview(&s_img, &cool));
    assert!(
        !cells.vouch(&sp, &backwards, None).vouched(),
        "cooling a frame whose target got warmer must never be vouched"
    );
}

/// 2026-10-21. A white balance is judged along the axis it can move.
///
/// The stage's cell verdict used to be put to the FULL direction — luminance
/// and colour together — while a white balance moves chromaticity only. On a
/// target that is both warmer and darker the direction to each cell's target
/// mean is mostly "darker", so a correct warming read as mis-aligned wherever
/// the tone stage had left a luminance gap, and the cast went back to
/// as-shot (the lantern street: 0.866 converged, 0.065 diverged, 0.502
/// aligned). The zone colour probe has read chromaticity since 2026-10-19;
/// this is the same measurement at frame scope.
///
/// MUTATION THIS KILLS: put the white-balance vouch back on `MoveAxis::Full`.
#[test]
fn a_white_balance_is_vouched_along_chromaticity_not_the_full_direction() {
    let source = synth();
    // Warmer AND darker: the same gentle cast as the admission test, under a
    // 0.8 sRGB scale, so every cell's target is first of all dimmer.
    let target = relit(&source, [1.06 * 0.8, 0.8, 0.95 * 0.8]);
    // The instrument, on the move the stage makes from the UNTONED frame —
    // the gap the global tone stage leaves when it refuses, as it did on
    // the lantern street: a pure warming toward a dimmer target.
    let (s_img, t_img) = analysis_pair(&source, &target);
    let (sp, tp) = (pixels_of(&s_img), pixels_of(&t_img));
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let cells = crate::fit_cells::PairedCells::build(&tp, s_img.width(), s_img.height(), &evidence)
        .expect("a populated pair builds cells");
    let warm = EditRecipe { temperature_k: Some(6400.0), ..Default::default() };
    let warmed = pixels_of(&render::develop_preview(&s_img, &warm));
    let full = cells.vouch(&sp, &warmed, None);
    let colour = cells.vouch_along(crate::fit_cells::MoveAxis::Colour, &sp, &warmed, None);
    eprintln!("full {full:?} colour {colour:?}");
    // Convergence and divergence are axis-free (a warming alone does not
    // bring a dimmer cell CLOSER); only the direction half differs, and it
    // differs by the whole verdict: 0.25 aligned on the full direction,
    // 0.96 along chromaticity, on this pair.
    assert!(
        full.aligned < 0.70 && colour.aligned >= 0.70,
        "the chromaticity reading must see the warming the full direction hides: {colour:?} vs {full:?}"
    );
    // …and the stage ships it.
    let report = fit_recipe(&source, &target);
    assert_eq!(report.mode, FitMode::Full, "premise: the structure is untouched");
    let k = report.recipe.temperature_k.expect("the warmer, darker target still gets its white balance");
    assert!(k > 5500.0, "…and it reads warmer than as-shot: {k}");
    assert!(
        report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_WB_CELLS_VOUCHED),
        "the admission is disclosed: {}",
        report.recipe.rationale
    );
}

/// R36 for the white balance (2026-10-21). A cast whose SIZE the cells
/// refuse while agreeing on its DIRECTION ships at the largest share they
/// vouch, instead of going back to as-shot whole.
///
/// The fixture asks every cell for the same warm move at two sizes: the
/// bright half of the ramp is relit by a strong warm cast, the dark half by
/// a gentle one. The population solves ONE white balance between the two,
/// which pushes the gentle half PAST its own target — diverged over the
/// line, aligned near one — exactly the seaside street's shape
/// (0.847 / 0.117 / 0.954). The share search then finds the size the gentle
/// half still converges under.
///
/// 2026-10-22: the dark range is no longer left idle. With the demand read
/// from the toned render and opposition by sign alone, an idle quarter read
/// −0.032 on the Kelvin axis after the tone sliders (0.102 of the mass,
/// over `WB_OPPOSING_SHARE`), the Kelvin demand went to zero and only a
/// tint −4.5 was vouched (0.026 / 0.000 / 0.349, refused on DIRECTION).
/// Under `WB_OPPOSING_RATIO` that range is a sliver (0.09 of the rest's
/// +0.35) and the idle fixture's Kelvin axis reads +0.349, opposing 0.000
/// — but that is a red/blue ratio of e^0.349 = 1.42 (free fit 9150 K /
/// tint −0.3), over the default budget's 1.40, and the stage leaves no
/// white-balance sentence at all. So the dark quarter still gets the gentle
/// warm cast this comment always described, and the bright cast is a step
/// milder.
///
/// MUTATION THIS KILLS: delete the share search (the cast goes back to
/// as-shot and the refusal note rides).
#[test]
fn a_white_balance_refused_on_size_ships_at_the_share_the_cells_vouch() {
    use image::GenericImageView;
    let source = synth();
    let (w, h) = (source.width(), source.height());
    let target = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let p = source.get_pixel(x, y).0;
        let luma = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0;
        // Most of the frame asks for a warm cast the default strength's
        // budget admits (a stronger one stays as-shot without a verdict at
        // all) and the darkest quarter for a gentle one: the population
        // solves one cast between the two, the full move pushes the dark
        // cells' means past their own targets (a refusal on SIZE: diverged
        // over 0.10 while the cells keep the direction over 0.70 — measured
        // 2026-10-22: 0.636 / 0.266 / 0.851), and a share of it small enough
        // to leave those dark means inside the blind-move line is what ships
        // (0.266 of the move, 6200 K / tint −3.9: 0.721 / 0.000 / 0.870).
        let gains = if luma > 0.25 { [1.085, 1.0, 0.915] } else { [1.015, 1.0, 0.985] };
        image::Rgb(std::array::from_fn(|c| {
            (p[c] as f32 * gains[c]).round().clamp(0.0, 255.0) as u8
        }))
    }));
    let report = fit_recipe(&source, &target);
    assert_eq!(report.mode, FitMode::Full, "premise: the structure is untouched");
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_WB_CELLS_VOUCHED_AT_SHARE)
        .unwrap_or_else(|| {
            panic!("the size refusal must ship a share, and say so: {}", report.recipe.rationale)
        });
    let share = note
        .args
        .iter()
        .find(|(k, _)| *k == "share")
        .and_then(|(_, v)| v.parse::<f32>().ok())
        .expect("the sentence prints the share");
    assert!(share > 0.0 && share < 1.0, "a share of the move, not the move: {share}");
    let full_diverged = note
        .args
        .iter()
        .find(|(k, _)| *k == "full_diverged")
        .and_then(|(_, v)| v.parse::<f32>().ok())
        .expect("the sentence carries the full move's refusal");
    assert!(full_diverged > 0.10, "premise: the full move was refused on size: {note:?}");
    let k = report.recipe.temperature_k.expect("the share ships a white balance");
    assert!(k > 5500.0, "…warmer than as-shot: {k}");
    assert!(
        !report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_WB_CELLS_REFUSED),
        "one outcome, one sentence: {}",
        report.recipe.rationale
    );
}

/// A NEUTRAL source under coloured light: the classic one-sided band, and
/// the case R33 §E exists for.
///
/// A neutral develop has no chroma at all, so `evidence_hue_band` puts no
/// source mass in any band; the target's warm rendition puts mass in Red
/// and Orange. The frozen evidence therefore calls those bands one-sided —
/// UNMEASURABLE, never "already equal" — and the mixer left them neutral
/// forever, because a band one-sided on the TARGET side has no source
/// member that could ever vouch it.
///
/// What changes it is not a loosened gate: the white-balance stage
/// corrects the light first, and the band is then two-sided ON THE RENDER
/// THIS STAGE SOLVES FROM. The admission still has to be earned — the
/// target's own cells must say the earlier stages moved those pixels
/// toward it — and the report says which bands were admitted that way.
#[test]
fn a_band_that_became_two_sided_once_the_light_was_corrected_is_admitted_by_the_cells() {
    let (w, h) = (192u32, 128u32);
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let l = 0.12 + 0.74 * (x as f32 / (w - 1) as f32) + 0.06 * (y as f32 / (h - 1) as f32);
        let v = (l.clamp(0.0, 1.0) * 255.0).round() as u8;
        image::Rgb([v, v, v])
    }));
    let target = relit(&source, [1.06, 1.0, 0.95]);
    let report = fit_recipe(&source, &target);
    assert_eq!(report.mode, FitMode::Full, "premise: nothing about the scene moved");
    let vouched = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_HSL_BANDS_VOUCHED)
        .unwrap_or_else(|| panic!("no band was admitted by the cells: {}", report.recipe.rationale));
    let bands = vouched
        .args
        .iter()
        .find(|(key, _)| *key == "bands")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    assert!(!bands.is_empty(), "the note names the bands it admitted");
    // …and the same bands are no longer listed as refused for want of
    // two-sided evidence: one band, one verdict.
    let refused = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_HSL_BANDS)
        .and_then(|n| n.args.iter().find(|(key, _)| *key == "refused"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    for band in bands.split(", ") {
        assert!(
            !refused.contains(band),
            "{band} is both admitted and refused: refused=[{refused}] vouched=[{bands}]"
        );
    }
}

/// R33 §D. A pair whose TEXTURE was re-synthesised: the same slow
/// left-to-right base, a known affine tone map, and a per-pixel hash of
/// its own on each side. The layout corresponds; the pixels do not.
///
/// The paired robust estimator does not notice. It reports 54 map points
/// and rejects 0.1% of them, because every pair it forms IS internally
/// consistent — it is measuring the regression of one noise draw on
/// another, and errors-in-variables shrinks toward unity the slope
/// it recovers. Measured on this fixture it under-reads the map contrast
/// by 10%, while the population-quantile arm sitting next to it in the
/// same function recovers it to 0.3%: the two marginals carry the map
/// exactly, the pairing carries nothing.
///
/// So the estimator follows the PAIRING SCALE, not the robust fit's own
/// opinion of itself — that is all [`paired_correspondence`] does. The
/// Cell branch this test drives is not reachable through the shipped
/// solve (D_fine on this very fixture is 0.406, which is Atmosphere, and
/// a Full solve's fine reading cannot abstain — see the function's doc);
/// it is pinned because the coupling must survive whoever moves the mode
/// line, and because the numbers below are the evidence that it should.
#[test]
fn a_pair_whose_texture_was_resynthesised_is_read_by_population_not_by_pairing() {
    let (w, h) = (192u32, 128u32);
    // One hash, two seeds: same distribution, independent draws.
    let hash = |i: u32, seed: u32| {
        let mut v = i.wrapping_mul(747796405).wrapping_add(seed.wrapping_mul(2891336453));
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 10_000) as f32 / 10_000.0 - 0.5
    };
    let base = |x: u32| 0.20 + 0.55 * (x as f32 / (w - 1) as f32);
    let map = |v: f32| (0.5 + 1.35 * (v - 0.5)).clamp(0.0, 1.0);
    let amp = 0.40f32;
    // The target is the map applied to a RE-SYNTHESISED source, not the
    // map applied to the source: a repaint invents its own texture and
    // then wears the look, which is why the two marginals still agree.
    let build = |seed: u32, mapped: bool| {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let v = (base(x) + amp * hash(y * w + x, seed)).clamp(0.0, 1.0);
            let v = if mapped { map(v) } else { v };
            let c = (v * 255.0).round() as u8;
            image::Rgb([c, c, c])
        }))
    };
    let (s_img, t_img) = (build(1, false), build(9_999, true));
    let (sp, tp) = (pixels_of(&s_img), pixels_of(&t_img));
    let evidence = evidence_model_for(&sp, &tp, w, h);
    let robust = paired_robust_tone(
        &sp,
        &tp,
        &|i: usize| evidence.source_weights[i].min(evidence.target_weights[i]),
        true,
    );
    let r = robust.as_ref().expect("premise: the robust fit answers on this pair");
    assert!(
        r.points.len() >= 6 && r.rejected_share <= 0.5,
        "premise: the robust fit's OWN diagnostics are clean here — {} points, {:.3} rejected — so nothing but the pairing scale can refuse it",
        r.points.len(),
        r.rejected_share
    );

    let (s_cdf, t_cdf) = tone_cdf_pair_weighted(&sp, &tp, &evidence);
    let span = |m: &dyn Fn(f32) -> f32| m(0.65) - m(0.35);
    let truth = span(&map);
    let marginal = span(&|x: f32| {
        quantile(&t_cdf, cdf_at(&s_cdf, x).clamp(P_CLIP, 1.0 - P_CLIP))
    });
    let paired = span(&|x: f32| sample_tone_points(&r.points, x));
    assert!(
        (marginal - truth).abs() <= 0.05 * truth,
        "the population arm must recover the map's contrast within 5%: truth {truth:.4}, marginal {marginal:.4}"
    );
    assert!(
        paired <= 0.95 * truth,
        "premise: the paired arm under-reads it by more than 5%: truth {truth:.4}, paired {paired:.4}"
    );

    // …and the scale, not the diagnostics, is what routes between them.
    assert_eq!(
        paired_correspondence(robust.as_ref(), PairingScale::Pixel).len(),
        r.points.len(),
        "at pixel scale the paired arm is unchanged"
    );
    assert!(
        paired_correspondence(robust.as_ref(), PairingScale::Cell).is_empty(),
        "at cell scale the tone stage falls to its population arm"
    );
}

#[test]
fn wb_default_strength_is_byte_identical_to_head() {
    let source = hazy_canyon_source();
    let target = vivid_warm_target();
    let report = fit_recipe_from_with(
        &source,
        &target,
        &EditRecipe::default(),
        FitOptions { strength: crate::recipe::GradeStrength::new(0.65), provider: None },
    );
    assert_eq!(report.recipe.temperature_k, None);
    assert_eq!(report.recipe.tint, 0.0);
    assert_eq!(report.recipe.exposure_ev, -0.28);
    assert_eq!(report.recipe.saturation, 0.0);
    assert_eq!(
        report.recipe.tone_curve,
        vec![
            crate::recipe::CurvePoint { input: 0, output: 0 },
            crate::recipe::CurvePoint { input: 65, output: 61 },
            crate::recipe::CurvePoint { input: 131, output: 118 },
            crate::recipe::CurvePoint { input: 179, output: 190 },
            crate::recipe::CurvePoint { input: 255, output: 255 },
        ]
    );
    assert_eq!(report.recipe.confidence, 0.25);

    let Some(dir) = calibration_corpus() else { return };
    let source = image::open(dir.join("neutral.jpg")).expect("calibration source");
    let target = image::open(dir.join("target.jpg")).expect("calibration target");
    let report = fit_recipe_from_with(
        &source,
        &target,
        &EditRecipe::default(),
        FitOptions { strength: crate::recipe::GradeStrength::new(0.65), provider: None },
    );
    // RE-PINNED by step 9, user ruling 1 (2026-08-31). The calibration
    // pair no longer PERSISTS a white balance at the shipped default, and
    // that is the budget doing its job rather than a capability lost.
    // Measured first-party on this corpus: the per-pixel estimator asks
    // K 16050 / tint +34.3, gains [1.263, 0.931, 0.764], gain RATIO
    // 1.6534, where the marginal-median estimator asked K 7100 / +22.3 at
    // ratio 1.2314. `ATMOSPHERE_WB_GAIN_RATIO` allows 1.40 at
    // `GradeStrength::DEFAULT`, so the demand is refused and the recipe
    // keeps the as-shot white balance.
    //
    // The refusal is a function of STRENGTH, not a ceiling. `wb_ratio` is
    // `between(1.20, ATMOSPHERE_WB_GAIN_RATIO, 3.0)` interpolated on the
    // strength axis, so at strength 1.0 the budget is 3.0 and the very
    // same pair persists the very same move — which the second arm below
    // measures rather than asserting. Wanting a larger white-balance
    // shift is exactly what the F1 freedom axis already ships.
    //
    // RE-PINNED 2026-10-23 for 2d0e750: the demand is now the consensus of
    // the per-range chromaticity medians, inverted in log-gain space against
    // the engine's own `[gr(K), 1 - 0.2*tint/100, gb(K)]`, where the old
    // read booked part of every warm move as green. On this pair it asks
    // K 12700 / tint +72.7, gains [1.234, 0.855, 0.788], ratio 1.566 (was
    // 16050 / +34.3 at ratio 1.653): still past 1.40 at the default, still
    // inside 3.0 at full strength. No cast curves ship on this Atmosphere
    // pair, so 8b4f4ec's hand-off is not involved.
    assert_eq!(report.recipe.temperature_k, None);
    assert_eq!(report.recipe.tint, 0.0);
    // The EXPOSURE solve stays on the marginal weighted-luma median over
    // the shared-content population, and that is now a measured decision
    // rather than a postponement: the paired per-pixel form was
    // implemented and measured on 2026-09-02, moved neither this pair nor
    // `p37`, and stopped `flat_sky_to_cloud_deck` fitting at all. The
    // reasoning is at the solve; this assertion is what would catch an
    // accidental side effect, and it reads -1.00 on both statistics.
    assert_eq!(report.recipe.exposure_ev, -1.0);
    let full = fit_recipe_from_with(
        &source,
        &target,
        &EditRecipe::default(),
        FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None },
    );
    assert_eq!(
        full.recipe.temperature_k,
        Some(12700.0),
        "the same demand is INSIDE the budget once the strength axis widens it"
    );
    assert!((full.recipe.tint - 72.7).abs() <= 0.05, "tint {}", full.recipe.tint);
}

/// Step 9 / acceptance A1: a pair in which NO pixel changed its
/// chromaticity may not persist a colour cast.
///
/// The ground truth is structural and readable in the fixture's own
/// source rather than asserted here: `flat_sky_to_cloud_deck`'s land
/// branch never consults `clouds`, so the lower half is byte-identical
/// between the two builds, and its sky keeps the same chromaticity vector
/// `[l*0.83, l*0.92, l]` with only the luminance `l` redrawn. There is no
/// cast to find. Three INDEPENDENT per-channel medians nonetheless read
/// K 4400 / tint +55.2 on it - 27x this tolerance - because the source's
/// smooth sky and the target's cloud deck put the three marginals'
/// halfway points in different sub-populations, so their ratio is no
/// pixel's colour. The fixture is 384x256 == `ANALYZE_EDGE`, so
/// `analysis_pair` does not resample it.
///
/// Supervisor mutations M-1-A (the three marginal medians restored) and
/// M-1-E (the geometric-mean normalisation deleted) both go red here.
///
/// Since 2026-10-21 a demand that is the anchor itself is NO demand: the
/// stage persists nothing — not a 50 K-rounded copy of the as-shot Kelvin
/// — and says nothing. So "the anchor stands" reads as `None`, and a tint
/// of exactly 0.
#[test]
fn a_pair_that_changed_no_pixel_chromaticity_persists_no_cast() {
    let (src, tgt) = flat_sky_to_cloud_deck();
    let report = fit_recipe(&src, &tgt);
    assert_eq!(
        report.mode,
        FitMode::Atmosphere,
        "premise: the cloud deck is content-divergent, so RC1 is what runs"
    );
    assert_eq!(
        report.recipe.temperature_k, None,
        "no chromaticity moved, so no white balance may persist: {:?} K, tint {}",
        report.recipe.temperature_k, report.recipe.tint
    );
    assert_eq!(
        report.recipe.tint, 0.0,
        "no chromaticity moved, so no tint may be invented: {}",
        report.recipe.tint
    );
}

/// Step 9 / acceptance A2: a readable correspondence field chooses the
/// POPULATION and the PAIRING, and the estimator may not take one without
/// the other.
///
/// The target here is the same target, RECOMPOSED - rolled down by 64 of
/// its 256 rows, 25% of the frame. Content preserved, moved in frame,
/// which is squarely inside Atmosphere's remit and is exactly what
/// same-index pairing cannot survive: with the population restricted to
/// the shared content but the pairing left on the raw index, the
/// estimator reads a large invented cast off a pair whose true `gr/gb` is
/// 1.000000. 64 of 256 rows is 12 of the sidecar's 48 grid cells exactly,
/// so a field that knows the roll is a two-line variant of
/// `identity_test_field`.
///
/// It asserts on the value the SOLVE returns rather than on the recipe,
/// and that is not a convenience: both wrong answers demand a gain ratio
/// above `ATMOSPHERE_WB_GAIN_RATIO`, so all three arms persist `None` and
/// a black-box assertion could not see the defect at all.
///
/// Supervisor mutation M-1-B (read `tp` instead of the remapped array
/// when the field is readable) goes red here and only here.
#[test]
fn a_readable_field_chooses_the_pairing_and_not_only_the_population() {
    let (src, tgt) = flat_sky_to_cloud_deck();
    let rolled = {
        let rgb = tgt.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            *rgb.get_pixel(x, (y + h - 64) % h)
        }))
    };
    let (s_img, t_img) = analysis_pair(&src, &rolled);
    let (w, h) = (s_img.width(), s_img.height());
    let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
    let tp = pixels_of(&t_img);
    let evidence = evidence_model_for(&sp, &tp, w, h).structure_blind(&tp);
    let g = crate::correspond::GRID;
    let cells = g * g;
    let field = crate::correspond::CorrespondenceField {
        map_y: (0..cells).map(|c| (((c / g) + 12) % g) as f32).collect(),
        ..identity_field()
    };
    let pc = correspondence_for_pair(&field, &tp, (w, h), (w, h));
    let shared = shared_content_population(&evidence, &pc)
        .expect("both sides carry evidence mass");
    assert!(
        shared.readable(),
        "premise: the field must be readable, retention {:.3}/{:.3}",
        shared.source_retained,
        shared.target_retained
    );
    let (pair_tp, pair_w) =
        atmosphere_wb_pairing(&tp, &evidence, Some(&pc), Some(&shared));
    // Under the per-range consensus (2026-10-21) a broken pairing no longer
    // shows as a wrong Kelvin — its ranges OPPOSE one another and the stage
    // ships nothing — so the premise is read on the per-range axes the
    // estimator itself consensuses: opposition at or past the veto line, or
    // a demand the true pair (gr/gb exactly 1) cannot have.
    let unpaired = wb_axes_by_range(&sp, &tp, &pair_w);
    let (unpaired_k, unpaired_opposing) = wb_consensus(&unpaired.kelvin);
    assert!(
        unpaired_opposing >= WB_OPPOSING_SHARE || unpaired_k.abs() > 0.10,
        "premise: same-index pairing against a moved target IS broken: \
         kelvin axis {unpaired_k:.3}, opposing {unpaired_opposing:.3}, ranges {:?}",
        unpaired.kelvin
    );
    let paired = wb_axes_by_range(&sp, pair_tp, &pair_w);
    let (paired_k, paired_opposing) = wb_consensus(&paired.kelvin);
    assert!(
        paired_opposing < WB_OPPOSING_SHARE && paired_k.abs() <= 0.02,
        "the field that chose the population must also choose the pairing: \
         kelvin axis {paired_k:.3}, opposing {paired_opposing:.3}, ranges {:?}",
        paired.kelvin
    );
    let (_, _, wanted) =
        atmosphere_wb_from_populations(&sp, pair_tp, &pair_w, 5500.0);
    assert!(
        (wanted[0] / wanted[2] - 1.0).abs() <= 0.02,
        "…and the solved gains say the same: {wanted:?}"
    );
}

/// A range opposes the white-balance consensus only when it asks the other
/// way by at least `WB_OPPOSING_RATIO` of the median: a tenth of the mass
/// asking −0.03 against +0.37 (0.08 of it) is an idle range and the demand
/// ships; the same tenth asking −0.20 (0.54 of it) is a second light and
/// the axis has no demand.
///
/// MUTATION THIS KILLS: WB_OPPOSING_RATIO = 0.0 (sign alone)
#[test]
fn a_sliver_the_other_way_does_not_veto_the_white_balance() {
    let (idle, idle_opposing) = wb_consensus(&[(0.37, 0.90), (-0.03, 0.10)]);
    assert!(
        idle == 0.37 && idle_opposing == 0.0,
        "a range asking 0.08 of the consensus the other way is idle: {idle:.3}, opposing {idle_opposing:.3}"
    );
    let (vetoed, vetoed_opposing) = wb_consensus(&[(0.37, 0.90), (-0.20, 0.10)]);
    assert!(
        vetoed == 0.0 && (vetoed_opposing - 0.10).abs() < 1e-6,
        "a tenth asking 0.54 of the consensus the other way vetoes: {vetoed:.3}, opposing {vetoed_opposing:.3}"
    );
}

#[test]
fn global_cast_is_measured_when_every_band_is_one_sided_and_consistent() {
    let source = vec![[0.08, 0.16, 0.82]; 256];
    let target = vec![[0.82, 0.34, 0.08]; 256];
    let evidence = evidence_model_for(&source, &target, 16, 16);
    let cast = evidence.global_cast.expect("coherent one-sided bands are a global cast");
    assert!(cast.rotation_deg.abs() > 20.0);
    assert!(cast.chroma_ratio > 0.5);
}

#[test]
fn opposed_band_rotation_is_still_withheld() {
    let mut source = Vec::new();
    let mut target = Vec::new();
    for i in 0..256 {
        let p = if i % 2 == 0 { [0.08, 0.16, 0.82] } else { [0.82, 0.16, 0.08] };
        let q = if i % 2 == 0 { [0.82, 0.34, 0.08] } else { [0.08, 0.34, 0.82] };
        source.push(p);
        target.push(q);
    }
    assert!(evidence_model_for(&source, &target, 16, 16).global_cast.is_none());
}

#[test]
fn high_strength_discloses_instead_of_withholding() {
    let px = vec![[0.4, 0.4, 0.4]; 64];
    let mut evidence = evidence_model(&px, &px);
    evidence.identifiability = 0.9;
    let budget = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0));
    let report = compose_report(
        EditRecipe::default(),
        Measured {
            err_before: 0.2,
            err_after: 0.1,
            joint_after: None,
            after_px: &px,
            tp: &px,
            same_frame: true,
            mode: FitMode::Atmosphere,
            divergence: Some(Divergence::matched()),
            divergence_coarse: Some(Divergence::matched()),
            pairing: PairingScale::Pixel,
            evidence: &evidence,
            structural_evidence: Some(&evidence),
            defer_disclosure: false,
        },
        SolveFacts {
            budget: Some(budget),
            strength: Some(1.0),
            veto_luma: Some("luma bins 06-08".into()),
            veto_hue: None,
            wb_clamped: None,
            wb_search_bound: None,
            wb_rotation_coverage: None,
            wb_rotation_disclosure: None,
            wb_cells: None,
            wb_foreign_hue_withheld: false,
            wb_rotation_withheld: false,
            sat_pegged: None,
            cast: CastOutcome::default(),
            cast_admitted_by_strength: None,
            cast_admitted: None,
            cast_projected: None,
            evidence_refused: true,
            sat_fitted: None,
            regressed: None,
            tone_harm: None,
            curve_harm: None,
            detail: (0.0, 0.0),
            detail_withheld: false,
            robust: None,
            paired: false,
            vouched_bands: None,
            cast_cells: None,
            hsl: HslStageFacts::default(),
            atmosphere_reference: AtmosphereReference::WholeFrame,
        },
    );
    assert!(report.recipe.confidence <= 0.35);
    assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_VETO_DISCLOSED));
}

#[test]
fn frame_regression_law_holds_at_strength_one() {
    let (src, tgt) = structural_permutation_pair();
    let report = fit_recipe_from_with(
        &src,
        &tgt,
        &EditRecipe::default(),
        FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None },
    );
    assert!(report.err_after <= report.err_before + 1e-4);
}

#[test]
fn cast_ratio_is_pinned_to_head_at_default() {
    assert_eq!(
        FitBudget::for_strength(crate::recipe::GradeStrength::default()).cast_ratio,
        CAST_ACCEPT_RATIO
    );
}

#[test]
fn wb_rotation_budget_opens_linearly_with_strength() {
    let at_zero = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.0));
    let at_default = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.65));
    let at_mid = FitBudget::for_strength(crate::recipe::GradeStrength::new(0.85));
    let at_full = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0));
    assert_eq!(at_zero.wb_rotation_share, ROT_SHARE);
    assert_eq!(at_default.wb_rotation_share, ROT_SHARE);
    assert!((at_mid.wb_rotation_share - 0.5928571).abs() <= 1e-3);
    assert_eq!(at_full.wb_rotation_share, 1.0);
}

#[test]
fn synthetic_full_region_wb_rotation_exceeds_seventy_percent_budget() {
    let cur = vec![[0.10f32, 0.25, 0.82]; 1000];
    let with = vec![[0.82f32, 0.32, 0.10]; 1000];
    let evidence = evidence_model(&cur, &cur);
    let rotated_share = rehued_share_weighted(&cur, &with, &evidence);
    assert!(rotated_share > 0.99, "synthetic chromatic region is fully re-hued");
    assert!(rotated_share > FitBudget::for_strength(crate::recipe::GradeStrength::new(0.70)).wb_rotation_share);
}

/// The joint arm of the terminal do-no-harm check was documented as a live
/// veto from the day it was written (899798e) and never wired: both
/// terminal sites computed `joint_base` / `joint_after` and then handed
/// `terminal_harm` two `None`s, so `harm.joint` was false on every
/// photograph and `FIT_NOTE_JOINT_REGRESSED` was unreachable. The two
/// readings are what the sites must pass.
#[test]
fn the_joint_veto_is_wired_at_both_terminal_sites() {
    // The BODY, not the file: this test's own literals would count.
    let src = crate::fit::source_text();
    let src = crate::source_before_tests(&src);
    assert_eq!(
        src.matches("terminal_harm(err_before, err_after, joint_base, joint_after)").count(),
        2,
        "both terminal sites hand the joint readings to terminal_harm"
    );
    assert!(
        !src.contains("terminal_harm(err_before, err_after, None, None)"),
        "a terminal site still discards its joint readings"
    );
}

/// A flat side has no gradient to correlate at any offset, so the
/// correlation term contributes nothing and D is the band-energy ratio
/// alone: a checkerboard that became flat (or a flat patch that grew a
/// checkerboard) reads divergent, a flat patch that stayed flat reads
/// matched, and two textured sides correlate. Pinned because the 2026-09-24
/// audit's abstention here was tried and withdrawn (see the function).
#[test]
fn a_flat_side_reads_through_the_energy_term_alone() {
    let (w, h) = (40u32, 30u32);
    let n = (w * h) as usize;
    let flat = vec![[0.5f32; 3]; n];
    let textured: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            let v = if ((i % w as usize) / 4 + (i / w as usize) / 4).is_multiple_of(2) { 0.2 } else { 0.8 };
            [v; 3]
        })
        .collect();
    let weights = vec![1.0f32; n];
    for (a, b) in [(&flat, &textured), (&textured, &flat)] {
        let read = structure_divergence(a, b, w, h, &weights).expect("one flat side still reads");
        assert_eq!(read.correlation, 1.0, "the correlation term is switched off: {read:?}");
        assert!(read.d >= DIVERGENCE_ZONE && (read.d - read.energy_error).abs() < 1e-5, "{read:?}");
    }
    let same = structure_divergence(&flat, &flat, w, h, &weights).expect("two flat sides read");
    assert_eq!(same.d, 0.0, "a uniform patch that stayed uniform: {same:?}");
    let read = structure_divergence(&textured, &textured, w, h, &weights)
        .expect("two textured sides correlate");
    assert!(read.correlation > 0.99, "{read:?}");
}

/// The detail stage's own escape from the scalar do-no-harm check applies
/// only to a recipe whose non-detail controls stand at the base; the HSL
/// wheel is one of them and was not read.
#[test]
fn an_hsl_move_is_not_a_detail_only_companion() {
    let base = EditRecipe::default();
    let mut detail_only = base.clone();
    detail_only.clarity = 12.0;
    assert!(only_detail_and_quantized_companions(&detail_only, &base));
    let mut with_hsl = detail_only.clone();
    with_hsl.hsl.saturation[0] = 20.0;
    assert!(!with_hsl.hsl.is_neutral(), "premise: the wheel moved");
    assert!(!only_detail_and_quantized_companions(&with_hsl, &base));
}

/// The rescore's evidence model is measured on the base the caller names:
/// the calibration base on a photograph, which is what the CLI and the
/// desktop app now pass. Named twice, it reproduces the solve's model bit
/// for bit; named as the default it does not — so the parameter is read.
#[test]
fn a_rescore_measures_against_the_base_the_caller_names() {
    let Some(root) = calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    let calibrated = EditRecipe { exposure_ev: 0.6, contrast: 15.0, ..EditRecipe::default() };
    let solved = fit_recipe_from_with(&source, &target, &calibrated, FitOptions::default());
    let same_base = rescore_report(
        &source,
        &target,
        &solved.recipe,
        &calibrated,
        solved.err_before,
        &solved.notes,
    );
    assert_evidence_models_bit_equal(&same_base.evidence, &solved.evidence);
    let other_base = rescore_report(
        &source,
        &target,
        &solved.recipe,
        &EditRecipe::default(),
        solved.err_before,
        &solved.notes,
    );
    assert!(
        other_base.evidence.source_pixels != solved.evidence.source_pixels,
        "a different base develops different evidence pixels"
    );
}

#[test]
fn rescoring_round_trips_fractional_strength_and_budget() {
    let prior = vec![crate::rationale::Note::new(
        crate::rationale::keys::FIT_NOTE_STRENGTH,
        vec![("pct", "64".into()), ("s", "0.6440".into())],
    )];
    let strength = carried_strength_from_notes(&prior);
    assert_eq!(strength.get(), 0.644);
    assert_eq!(FitBudget::for_strength(strength), FitBudget::for_strength(crate::recipe::GradeStrength::new(0.644)));
    let src = hazy_canyon_source();
    let tgt = vivid_warm_target();
    let rescored =
        rescore_report(&src, &tgt, &EditRecipe::default(), &EditRecipe::default(), 0.2, &prior);
    let carried_s = rescored
        .notes
        .iter()
        .find(|note| note.key == crate::rationale::keys::FIT_NOTE_STRENGTH)
        .and_then(|note| note.args.iter().find(|(key, _)| *key == "s"))
        .map(|(_, value)| value.as_str());
    assert_eq!(carried_s, Some("0.6440"));
}

/// F1 review: a rescoring after the deep step must RE-DERIVE the
/// high-strength veto disclosure and keep its cap. Before this pin the
/// rescored report dropped `veto_luma`/`veto_hue` (and, in Full mode, the
/// budget itself), so the same unsupported movement came back uncapped.
#[test]
fn rescoring_re_derives_the_high_strength_veto_disclosure_and_its_cap() {
    let src = hazy_canyon_source();
    let tgt = vivid_warm_target();
    let full = FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None };
    let solved = fit_recipe_from_with(&src, &tgt, &EditRecipe::default(), full);
    let disclosed = |r: &FitReport| {
        r.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_VETO_DISCLOSED)
    };
    assert!(disclosed(&solved), "fixture must disclose unsupported movement at strength 1.0");
    let rescored = rescore_report(
        &src,
        &tgt,
        &solved.recipe,
        &EditRecipe::default(),
        solved.err_before,
        &solved.notes,
    );
    assert!(disclosed(&rescored), "the rescoring must re-derive the disclosure for the same recipe");
    let cap = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0)).confidence_cap;
    assert!(
        rescored.recipe.confidence <= cap + 1e-6,
        "rescored confidence {} above the strength cap {cap}",
        rescored.recipe.confidence
    );
    // Default control: no strength note carried → withhold policy → no
    // disclosure, exactly the pre-F1 rescoring.
    let shipped = fit_recipe_from_with(&src, &tgt, &EditRecipe::default(), FitOptions::default());
    let rescored_default =
        rescore_report(
            &src,
            &tgt,
            &shipped.recipe,
            &EditRecipe::default(),
            shipped.err_before,
            &shipped.notes,
        );
    assert!(!disclosed(&rescored_default), "the shipped default must not gain a disclosure");
}

#[test]
fn cast_admission_disclosure_tracks_strength_budget() {
    assert!(cast_admitted_by_strength(
        2.4,
        FitBudget::for_strength(crate::recipe::GradeStrength::new(0.85)).cast_ratio,
        0.85,
    ));
    assert!(!cast_admitted_by_strength(
        2.4,
        FitBudget::for_strength(crate::recipe::GradeStrength::new(0.65)).cast_ratio,
        0.65,
    ));
    let mut rationale = String::new();
    let mut notes = Vec::new();
    crate::rationale::push_note(
        &mut rationale,
        &mut notes,
        crate::rationale::Note::new(
            crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_BY_STRENGTH,
            vec![("ratio", "2.400".into()), ("budget", "2.593".into())],
        ),
    );
    assert!(rationale.contains("measured ratio 2.400"));
}

#[test]
fn early_exit_atmosphere_report_keeps_the_strength_cap() {
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(32, 32, |x, y| {
        let v = 32 + ((x + y) % 160) as u8;
        image::Rgb([v, v, v])
    }));
    let report = fit_recipe_from_promoted_with_disclosure_opts(
        &src,
        &src,
        &EditRecipe::default(),
        true,
        false,
        FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None },
    );
    assert!(report.recipe.confidence <= 0.35);
    assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_STRENGTH));
}

#[test]
fn confidence_tracks_measured_error_under_the_strength_cap() {
    let src = hazy_canyon_source();
    let tgt = vivid_warm_target();
    for s in [0.65, 0.85, 1.0] {
        let report = fit_recipe_from_with(
            &src,
            &tgt,
            &EditRecipe::default(),
            FitOptions { strength: crate::recipe::GradeStrength::new(s), provider: None },
        );
        let cap = FitBudget::for_strength(crate::recipe::GradeStrength::new(s)).confidence_cap;
        assert!(report.recipe.confidence <= cap + 1e-6, "s={s} confidence {} cap {cap}", report.recipe.confidence);
        let ladder = confidence_from_look_err(report.err_after);
        if ladder < cap - 1e-5 {
            assert!(report.recipe.confidence <= ladder + 1e-5, "s={s} confidence {} ladder {} cap {} err {}", report.recipe.confidence, ladder, cap, report.err_after);
        }
    }
}
use image::RgbImage;

/// The knot-support gate itself, pinned at the unit level: a knot with
/// no measured testimony must contribute NOTHING to the solve, however
/// loud the estimated map's extrapolation is there. Supervisor mutation
/// MC (drop `support` from the weight composition) goes red here. (At
/// the stage-wiring level the same mutation is absorbed by the spline
/// model-selection — defense in depth, not dead code: the marginal path
/// scores on knots alone and has only this gate.)
#[test]
fn unsupported_knots_cannot_pull_the_solve() {
    let map = |x: f32| if x <= 0.66 { x } else { (x * 1.4).min(1.0) };
    let gated: [f32; 8] = [1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0];
    let (ev_g, s_g) = fit_tone_sliders_supported(&map, &gated, &[]);
    assert!(
        ev_g.abs() < 0.06 && s_g.iter().all(|v| v.abs() < 0.08),
        "identity-on-supported must solve near-neutral: ev={ev_g} s={s_g:?}"
    );
    let (ev_a, s_a) = fit_tone_sliders_supported(&map, &[1.0; 8], &[]);
    assert!(
        ev_a.abs() >= 0.06 || s_a.iter().any(|v| v.abs() >= 0.08),
        "premise: without the gate the phantom demand must visibly drag              the solve (ev={ev_a} s={s_a:?}) — if this stops holding, the              gate has nothing to guard and both asserts need re-deriving"
    );
}

/// The zone refinement's reason to exist, at the unit level. The map is the
/// engine's own response to a Blacks −15 move (p10 pulled 0.020 under the
/// knot solve's rendering), seen by a population living in 0.04–0.30 only
/// and FLAT outside it — the quantile transport clamps at `P_CLIP`, so the
/// x = 0 knot reads a lifted extrapolation, as the neon street's building
/// did. The knot solve answers with Blacks UP; the refinement on the
/// engine's rendered response must land the population's p10 on the map and
/// never score worse than where it started. Mutation:
/// `refine_tone_sliders_on` returning its input goes red on the p10 assert.
#[test]
fn the_spline_refinement_follows_a_bend_the_knots_cannot_see() {
    let render_at = |ev: f32, s: [f32; 5], x: f32| {
        crate::render::sample_tone_model(&crate::render::tone_model_knots(ev, s), x)
    };
    let map = |x: f32| render_at(0.0, [0.0, 0.0, 0.0, 0.0, -0.15], x.clamp(0.04, 0.30));
    const N: usize = 24;
    let score_set: Vec<(f32, f32, f32)> = (0..N)
        .map(|q| {
            let x = 0.04 + 0.26 * (q as f32 + 0.5) / N as f32;
            (x, map(x), 1.0)
        })
        .collect();
    // The population's support: the knots 0, 0.10 and 0.25.
    let support: [f32; 8] = [1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let (ev0, s0) = fit_tone_sliders_supported(&map, &support, &score_set);
    let (ev1, s1) = refine_tone_sliders_on(&score_set, ev0, s0);
    let score = |ev: f32, s: [f32; 5]| {
        let sse: f32 =
            score_set.iter().map(|&(x, y, m)| m * (render_at(ev, s, x) - y).powi(2)).sum();
        8.0 * sse / N as f32 + s.iter().map(|v| TONE_PRIOR as f32 * v * v).sum::<f32>()
    };
    let p10 = 0.04 + 0.26 * 0.10;
    let miss0 = (render_at(ev0, s0, p10) - map(p10)).abs();
    let miss1 = (render_at(ev1, s1, p10) - map(p10)).abs();
    // Measured: the knot solve misses by 0.0204 (Blacks +6), the refinement
    // by 0.0008 (Blacks −11).
    assert!(miss0 > 0.015, "premise: the knot solve alone misses the bend at p10 ({miss0})");
    assert!(miss1 < 0.006, "the refinement lands p10 on the map ({miss1})");
    assert!(score(ev1, s1) <= score(ev0, s0) + 1e-7, "never worse than the start");
}

/// The robust estimator's reason to exist, pinned at the unit level: a
/// 30% invented sub-population in the target must lose weight BY THE
/// ESTIMATOR'S OWN MECHANISM and leave the map on the clean population's
/// truth. Supervisor mutation MA (Tukey weights forced to 1 = plain
/// least squares) drags the contaminated bins' means and this goes red.
#[test]
fn robust_regression_downweights_invented_target_content() {
    let (w, h) = (128usize, 96usize);
    let mut sp = Vec::with_capacity(w * h);
    let mut tp = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let l = x as f32 / (w - 1) as f32;
            sp.push([l, l, l]);
            let mapped = (l * 1.3).min(1.0);
            if x < w * 3 / 10 && y < h / 2 {
                // invented content: a flat bright warm patch nothing in
                // the source explains (15% of the frame, dark-source bins)
                tp.push([0.85, 0.75, 0.55]);
            } else {
                tp.push([mapped, mapped, mapped]);
            }
        }
    }
    let fit = paired_robust_tone(&sp, &tp, &|_| 1.0, true)
        .expect("an aligned synthetic pair must be estimable");
    assert!(
        fit.rejected_share > 0.10,
        "the invented patch must be down-weighted, not averaged in              (rejected {:.3})",
        fit.rejected_share
    );
    let mid = sample_tone_points(&fit.points, 0.5);
    assert!(
        (mid - 0.65).abs() < 0.03,
        "the map must stay on the clean population's truth at x=0.5:              got {mid:.3}, truth 0.650"
    );
    // The invented patch sits over dark source columns — the disclosure
    // ranges must name at least one of the ranges it poisoned.
    assert!(
        !fit.rejected_ranges.is_empty(),
        "rejection must localise itself for the disclosure"
    );
}

/// The pipeline half of the same contract: a fit over a partially
/// invented target must DISCLOSE what it rejected. Supervisor mutation MB
/// (delete the FIT_NOTE_ROBUST_REJECTED push in compose_report) goes red
/// here while the estimator itself still works.
#[test]
fn robust_rejection_reaches_the_disclosure() {
    let src = synth();
    let mut truth = EditRecipe { exposure_ev: 0.4, contrast: 12.0, ..Default::default() };
    truth.clamp();
    let rendered = render::develop_preview(&src, &truth);
    let mut tgt = rendered.to_rgb8();
    let (w, h) = (tgt.width(), tgt.height());
    // Scattered 8x8 LUMA-PRESERVING recolour blocks (~12% of the frame):
    // the luma structure is untouched, so neither the global divergence
    // statistic nor the 3x3 spatial-evidence screen reacts — the
    // PER-PIXEL robust verdict (chromatic residual + hue incoherence) is
    // the only thing standing between the invented hues and the colour
    // stages. Exactly the estimator's niche.
    for by in 0..h / 8 {
        for bx in 0..w / 8 {
            if (bx * 3 + by * 5) % 8 == 0 {
                for dy in 0..8 {
                    for dx in 0..8 {
                        let p = *tgt.get_pixel(bx * 8 + dx, by * 8 + dy);
                        let y = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
                        let warm = [
                            (y * 1.45).min(255.0) as u8,
                            (y * 0.95) as u8,
                            (y * 0.35) as u8,
                        ];
                        tgt.put_pixel(bx * 8 + dx, by * 8 + dy, image::Rgb(warm));
                    }
                }
            }
        }
    }
    let tgt = image::DynamicImage::ImageRgb8(tgt);
    let report = fit_recipe(&src, &tgt);
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_ROBUST_REJECTED)
        .expect("a partially invented target must carry the rejection note");
    let pct: f32 = note
        .args
        .iter()
        .find(|(k, _)| *k == "pct")
        .map(|(_, v)| v.parse().unwrap())
        .expect("the note must carry the rejected percentage");
    assert!(pct >= 5.0, "a 15% invented region must reject visibly, got {pct}%");
}

/// The user's own Lightroom develop as ground truth (p36: pure-global
/// tier — Exposure +0.50, Contrast +14, Highlights -44, Shadows +40,
/// Whites -18, Sat -18/Vib +24, custom curve; the export is pixel-aligned
/// with the neutral render by construction). LR's and this engine's
/// parameter spaces differ, so the pin is the directly comparable core:
/// the paired path must engage, exposure must land near the LR anchor,
/// and the residual/confidence must hold the measured line. Supervisor
/// mutation MC (knot support forced to all-ones) resurrects the phantom
/// -knot degeneracy (ev pegged at +3) and this goes red.
#[test]
fn p36_fixture_recovers_the_lightroom_exposure_anchor() {
    let Some(root) = calibration_corpus() else { return };
    // The source is the camera's EMBEDDED PREVIEW — the very base the
    // CLI `match` solves against for a RAW (main.rs's calibration-stamp
    // note) — so the LR anchor means the same thing here as it does on
    // the command line.
    let (n, t) = (root.join("p36-preview.jpg"), root.join("p36-target.jpg"));
    if !n.is_file() || !t.is_file() {
        crate::test_skipped("p36 fixture test", "pair not in the corpus");
        return;
    }
    let src = image::open(n).unwrap();
    let tgt = image::open(t).unwrap();
    let report = fit_recipe(&src, &tgt);
    eprintln!(
        "P36_FIXTURE ev={} c={} h={} s={} sat={} err={:.4}->{:.4} conf={:.3}",
        report.recipe.exposure_ev,
        report.recipe.contrast,
        report.recipe.highlights,
        report.recipe.shadows,
        report.recipe.saturation,
        report.err_before,
        report.err_after,
        report.recipe.confidence
    );
    assert!(
        report.notes.iter().any(|note| {
            note.key == crate::rationale::keys::FIT_SUMMARY_WITH_CURVE_PAIRED
                || note.key == crate::rationale::keys::FIT_SUMMARY_NO_CURVE_PAIRED
        }),
        "the aligned export must take the paired path: {}",
        report.recipe.rationale
    );
    // LR's Exposure2012 and this engine's exposure_ev are different
    // parameter spaces (different base curves, different shoulder), so
    // the anchor is directional and bounded rather than numeric: the
    // LR +0.50 brightening must come back as a moderate positive ev
    // (measured 0.75 with the residual curve carrying the shape), and
    // NEVER as the phantom-knot degeneracy this test exists to catch
    // (support mutation MC pegs ev at +3.0 and dies here).
    assert!(
        report.recipe.exposure_ev > 0.20 && report.recipe.exposure_ev < 1.20,
        "exposure must land as a moderate positive move, got {}",
        report.recipe.exposure_ev
    );
    assert!(report.err_after < 0.035, "residual {:.4}", report.err_after);
    assert!(
        report.recipe.confidence >= 0.55,
        "confidence {:.3}",
        report.recipe.confidence
    );
}

#[test]
fn p36_full_rescore_round_trip_keeps_structural_evidence_absent() {
    let Some(root) = calibration_corpus() else { return };
    let (source_path, target_path) =
        (root.join("p36-preview.jpg"), root.join("p36-target.jpg"));
    if !source_path.is_file() || !target_path.is_file() {
        crate::test_skipped("p36 rescore test", "pair not in the corpus");
        return;
    }
    let source = image::open(source_path).expect("p36 preview");
    let target = image::open(target_path).expect("p36 target");
    let solved = fit_recipe(&source, &target);
    assert_eq!(solved.mode, FitMode::Full);
    assert!(solved.structural_evidence.is_none());
    let rescored = rescore_report(
        &source,
        &target,
        &solved.recipe,
        &EditRecipe::default(),
        solved.err_before,
        &solved.notes,
    );
    assert_eq!(rescored.mode, FitMode::Full);
    assert!(rescored.structural_evidence.is_none());
    assert_eq!(
        serde_json::to_vec(&rescored.recipe).unwrap(),
        serde_json::to_vec(&solved.recipe).unwrap(),
        "Full-mode rescore must reproduce the solved recipe byte for byte"
    );
}

/// M-F1: `fit_tone_sliders` degraded to return neutral (or any solver
/// regression that stops beating the ground truth under the engine's own
/// penalised objective) — on a look the weighted model can represent
/// exactly, the solve must score at least as well as the generating
/// parameters themselves.
///
/// Recorded honestly: the fit-side knot WEIGHTING itself has no
/// test-observable effect — an unweighted inner solve is a worse proposer
/// in the saturated regime, but the outer acceptance loop re-scores every
/// candidate by real rendering and masks it (verified by running the full
/// suite under that mutant). The weights stay in the solve for model
/// consistency — three sites, one definition — not because a test pins
/// them.
#[test]
fn the_fit_models_the_same_weighted_engine_it_renders_against() {
    let ev = 1.5f32;
    let truth = [-0.6f32, 0.0, 0.35, 0.0, 0.0]; // contrast −60, shadows +35
    let weights = render::tone_knot_weights(ev);
    let tone = |x: f32| -> f32 {
        let i = render::TONE_KNOTS_X
            .iter()
            .position(|&k| (k - x).abs() < 1e-6)
            .expect("fit samples the tone map at the knots only");
        let b = render::tone_slider_basis(x);
        render::tone_exposure_curve(x, ev)
            + weights[i] * (0..5).map(|k| b[k] * truth[k]).sum::<f32>()
    };
    let (got_ev, got) = fit_tone_sliders(&tone);
    // The saturated regime is deliberately non-identifiable (several
    // (ev, sliders) pairs render the same 8 knots, and the ridge prior
    // picks the smallest sliders), so the property is NOT parameter
    // recovery. It is: under the engine's OWN penalised objective — knot
    // error through the weighted model, plus the magnitude prior — the
    // solution must be at least as good as the ground truth itself. The
    // pristine solve minimises exactly this, so it passes structurally;
    // a solve that dropped the weights optimises a different forward
    // model and lands on parameters this objective scores worse.
    let true_score = |cand_ev: f32, s: &[f32; 5]| -> f64 {
        let w = render::tone_knot_weights(cand_ev);
        let sse: f64 = render::TONE_KNOTS_X
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let b = render::tone_slider_basis(x);
                let rendered = render::tone_exposure_curve(x, cand_ev)
                    + w[i] * (0..5).map(|k| b[k] * s[k]).sum::<f32>();
                let e = (rendered - tone(x)) as f64;
                e * e
            })
            .sum();
        sse + s.iter().map(|&v| TONE_PRIOR * v as f64 * v as f64).sum::<f64>()
    };
    let (got_score, truth_score) = (true_score(got_ev, &got), true_score(ev, &truth));
    assert!(
        got_score <= truth_score + 1e-6,
        "the fit landed on (ev {got_ev}, {got:?}) scoring {got_score:.6} — WORSE under \
             the engine's own model than the ground truth's {truth_score:.6}: the solve is \
             not modelling the engine it renders against"
    );
}

/// Synthetic frame with real tonal + chromatic coverage: a neutral luma ramp
/// plus orange / blue / green ramps (192×128 — analysis-sized already).
fn synth() -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let l = x as f32 / (w - 1) as f32;
            let p = match y * 4 / h {
                0 => [l, l, l],
                1 => [l, l * 0.6, l * 0.2],
                2 => [l * 0.2, l * 0.7, l],
                _ => [l * 0.3, l, l * 0.4],
            };
            img.put_pixel(x, y, image::Rgb(p.map(|c| (c * 255.0).round() as u8)));
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// Same landscape footprint, but the target grows a high-frequency cloud
/// deck over the source's smooth sky. The lower half is kept identical so
/// the fixture exercises structural evidence rather than a wholesale
/// unrelated-frame rejection.
fn flat_sky_to_cloud_deck() -> (DynamicImage, DynamicImage) {
    let (w, h) = (384u32, 256u32);
    let build = |clouds: bool| {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let xf = x as f32 / (w - 1) as f32;
            let yf = y as f32 / (h - 1) as f32;
            let l = if y < h / 2 {
                if clouds {
                    let broad = (xf * 8.0 + yf * 5.0).sin();
                    let billow = (xf * 31.0 - yf * 17.0).sin();
                    let knots = if ((x / 18) + (y / 12)) % 2 == 0 { -0.16 } else { 0.16 };
                    (0.55 + 0.28 * broad + 0.18 * billow + knots).clamp(0.04, 0.98)
                } else {
                    0.62 + 0.05 * xf - 0.03 * yf
                }
            } else {
                let ridge = if yf > 0.68 + 0.10 * (xf * 9.0).sin() { 0.22 } else { 0.42 };
                (ridge + 0.18 * xf + 0.03 * (xf * 43.0).sin()).clamp(0.02, 0.90)
            };
            let p = if y < h / 2 {
                [l * 0.83, l * 0.92, l]
            } else {
                [l, l * 0.78, l * 0.55]
            };
            image::Rgb(p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
        }))
    };
    (build(false), build(true))
}

/// A pure structural permutation: both frames carry exactly the same
/// pixel population, while the sky's spatial arrangement is scrambled.
/// Atmosphere mode therefore has a reachable neutral look target even
/// though structural correlation is intentionally broken.
fn structural_permutation_pair() -> (DynamicImage, DynamicImage) {
    let (w, h) = (384u32, 256u32);
    let source = RgbImage::from_fn(w, h, |x, y| {
        let xf = x as f32 / (w - 1) as f32;
        let yf = y as f32 / (h - 1) as f32;
        let l = if y < h / 2 {
            (0.55 + 0.22 * (xf * 19.0 + yf * 7.0).sin()
                + 0.10 * (xf * 53.0 - yf * 11.0).sin())
                .clamp(0.05, 0.95)
        } else {
            (0.25 + 0.45 * xf + 0.08 * (xf * 29.0).sin()).clamp(0.03, 0.90)
        };
        image::Rgb(if y < h / 2 {
            [l * 0.82, l * 0.92, l]
        } else {
            [l, l * 0.78, l * 0.55]
        }
        .map(|v| (v * 255.0).round() as u8))
    });
    let mut target = source.clone();
    let sky_n = (w * h / 2) as usize;
    for i in 0..sky_n {
        let from = (i * 193) % sky_n;
        let (x, y) = ((i as u32) % w, (i as u32) / w);
        let (sx, sy) = ((from as u32) % w, (from as u32) / w);
        target.put_pixel(x, y, *source.get_pixel(sx, sy));
    }
    (DynamicImage::ImageRgb8(source), DynamicImage::ImageRgb8(target))
}

#[test]
fn content_divergence_fires_on_flat_sky_to_cloud_deck() {
    let (src, tgt) = flat_sky_to_cloud_deck();
    let synthetic = structure_divergence_for(&src, &tgt, &EditRecipe::default(), None);
    eprintln!("STRUCTURE_CALIBRATION synthetic={synthetic:?}");
    assert!(
        synthetic.expect("a 384x256 analysis raster always resolves").d
            >= DIVERGENCE_GLOBAL,
        "a generated cloud deck must cross the global threshold: {synthetic:?}"
    );

    // The calibration corpus is intentionally optional for portable
    // CI (see `calibration_dir`); where present it pins the measured
    // number rather than merely the side of the threshold.
    let Some(root) = calibration_corpus() else { return };
    if root.join("neutral.jpg").exists() {
        let source = image::open(root.join("neutral.jpg")).unwrap();
        let target = image::open(root.join("target.jpg")).unwrap();
        let measured = structure_divergence_for(
            &source,
            &target,
            &EditRecipe::default(),
            None,
        );
        eprintln!("STRUCTURE_CALIBRATION real={measured:?}");
        assert!(
            (measured.expect("resolvable").d - 0.491).abs() <= 0.05,
            "generated-cloud calibration drifted: {measured:?}"
        );
    }
}

#[test]
fn content_divergence_is_calibrated_on_every_shipped_showcase_asset() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
    // The two shipped reverse-fit panels — the viaduct as composed for
    // v1.2.2 (`01de443`) and the Cornwall panel as RE-composed for v1.2.3
    // (`f3885b2`) — each read at the top-row offsets the panel geometry
    // fixes: the neutral conversion at left, the generated target in the
    // middle. Both generators were asked
    // for a grade and not a rebuild, so both sit UNDER the threshold and
    // the full solve ran on each; the fired arm of the same statistic is
    // pinned on a synthetic pair by
    // `content_divergence_fires_on_flat_sky_to_cloud_deck`. The pins are
    // the values measured when the panels were composed; a panel swapped
    // for another generation moves them, which is the point. Re-measured
    // on the shipped v1.2.3 bytes (2026-09-02): viaduct 0.17987, Cornwall
    // 0.13568 — the Cornwall re-composition did not move its reading off
    // the pin it had, which is why the pin below is the same number.
    for (file, want) in [
        ("showcase-viaduct-reverse-fit.jpg", 0.180f32),
        ("showcase-cornwall-reverse-fit.jpg", 0.136f32),
    ] {
        let panel = image::open(root.join(file)).unwrap();
        let source = panel.crop_imm(0, 136, 532, 356);
        let target = panel.crop_imm(535, 136, 530, 356);
        let measured =
            structure_divergence_for(&source, &target, &EditRecipe::default(), None);
        eprintln!("STRUCTURE_CALIBRATION {file} {measured:?}");
        assert!(
            measured.is_some_and(|r| r.d < DIVERGENCE_GLOBAL
                && (r.d - want).abs() <= 0.05),
            "{file} calibration drifted: {measured:?}, expected {want:.3}"
        );
    }
}

/// The coarse reading is a SECOND SCALE, not a more forgiving one.
///
/// It was added to answer "is the scene still in the same place?" apart
/// from "are the same pixels still there?", on the hypothesis that a
/// generative repaint keeps the first and destroys the second. The table
/// below is what the instrument actually reads, and it refuses the
/// hypothesis: on the pairs whose layout is REPLACED the coarse number is
/// far above the global threshold, and on the two shipped panels — whose
/// layout is untouched — the two readings sit within 0.03 of each other.
/// There is no side of any threshold the coarse reading puts a
/// same-layout repaint on that the fine reading does not, which is why the
/// mode line still reads the fine number and no evidence gate was moved
/// onto this one (see [`COARSE_SIGMA_DIVISOR`] for the real pair's own
/// four scopes).
#[test]
fn the_coarse_reading_is_a_second_scale_not_a_more_forgiving_one() {
    let d = EditRecipe::default();
    let measure = |src: &DynamicImage, tgt: &DynamicImage| {
        let pair = divergence_pair_for(src, tgt, &d);
        (
            pair.fine.expect("a 384x256 raster always resolves").d,
            pair.coarse.expect("the low-pass keeps the geometry").d,
        )
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
    let panel_pair = |file: &str| {
        let panel = image::open(root.join(file)).unwrap();
        (panel.crop_imm(0, 136, 532, 356), panel.crop_imm(535, 136, 530, 356))
    };
    let (viaduct_src, viaduct_tgt) = panel_pair("showcase-viaduct-reverse-fit.jpg");
    let (cornwall_src, cornwall_tgt) = panel_pair("showcase-cornwall-reverse-fit.jpg");
    let (cloud_src, cloud_tgt) = flat_sky_to_cloud_deck();
    let (perm_src, perm_tgt) = structural_permutation_pair();
    let identity = synth();
    for (name, (fine, coarse), want_fine, want_coarse) in [
        ("viaduct", measure(&viaduct_src, &viaduct_tgt), 0.180f32, 0.187f32),
        ("cornwall", measure(&cornwall_src, &cornwall_tgt), 0.136, 0.110),
        ("cloud-deck", measure(&cloud_src, &cloud_tgt), 1.642, 0.805),
        ("permutation", measure(&perm_src, &perm_tgt), 1.970, 1.053),
        ("identity", measure(&identity, &identity), 0.0, 0.0),
    ] {
        eprintln!("TWO_SCALE {name} fine={fine:.4} coarse={coarse:.4}");
        assert!(
            (fine - want_fine).abs() <= 0.05,
            "{name} fine drifted: {fine:.4}, expected {want_fine:.3}"
        );
        assert!(
            (coarse - want_coarse).abs() <= 0.05,
            "{name} coarse drifted: {coarse:.4}, expected {want_coarse:.3}"
        );
    }
    // The property the mode line depends on, stated as an assertion and not
    // only as a number: a same-layout panel is Full at BOTH scales, and a
    // replaced-layout pair is divergent at both. Neither scale rescues or
    // convicts a pair the other does not.
    for (name, src, tgt) in [
        ("viaduct", &viaduct_src, &viaduct_tgt),
        ("cornwall", &cornwall_src, &cornwall_tgt),
    ] {
        let (fine, coarse) = measure(src, tgt);
        assert!(fine < DIVERGENCE_GLOBAL && coarse < DIVERGENCE_GLOBAL, "{name}: {fine} {coarse}");
    }
    for (name, src, tgt) in [
        ("cloud-deck", &cloud_src, &cloud_tgt),
        ("permutation", &perm_src, &perm_tgt),
    ] {
        let (fine, coarse) = measure(src, tgt);
        assert!(fine >= DIVERGENCE_GLOBAL && coarse >= DIVERGENCE_GLOBAL, "{name}: {fine} {coarse}");
    }
    // …and the pairing scale the readings choose is the FINE one's verdict.
    assert_eq!(divergence_pair_for(&viaduct_src, &viaduct_tgt, &d).scale(), PairingScale::Pixel);
    assert_eq!(divergence_pair_for(&cloud_src, &cloud_tgt, &d).scale(), PairingScale::Cell);
}

#[test]
fn evidence_gating_does_not_regress_any_shipped_showcase_pair() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
    let mut pairs = Vec::new();
    for (name, file) in [
        ("viaduct", "showcase-viaduct-reverse-fit.jpg"),
        ("cornwall", "showcase-cornwall-reverse-fit.jpg"),
    ] {
        let panel = image::open(root.join(file)).unwrap();
        pairs.push((
            name.to_string(),
            panel.crop_imm(0, 136, 532, 356),
            panel.crop_imm(535, 136, 530, 356),
        ));
    }
    for (name, source, target) in pairs {
        let report = fit_recipe(&source, &target);
        eprintln!(
            "SHOWCASE pair={name} mode={:?} err={:.6}->{:.6} confidence={:.3} ev={:.2} c={:.1} h={:.1} s={:.1} w={:.1} b={:.1} sat={:.1}",
            report.mode,
            report.err_before,
            report.err_after,
            report.recipe.confidence,
            report.recipe.exposure_ev,
            report.recipe.contrast,
            report.recipe.highlights,
            report.recipe.shadows,
            report.recipe.whites,
            report.recipe.blacks,
            report.recipe.saturation,
        );
        assert!(
            report.err_after <= report.err_before + 1e-6,
            "{name} regressed: {:.6} -> {:.6}",
            report.err_before,
            report.err_after
        );
    }
}

#[test]
fn same_content_evidence_diagnosis_reports_cornwall_support_and_terminal_readings() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
    let triptych = image::open(root.join("showcase-cornwall-reverse-fit.jpg")).unwrap();
    let source = triptych.crop_imm(0, 136, 532, 356);
    let target = triptych.crop_imm(535, 136, 530, 356);
    // 532x356 against 530x356 thumbnails to 384x257 against 384x258: the
    // evidence must be built in the one geometry the fit itself uses.
    let (s_img, t_img) = analysis_pair(&source, &target);
    let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
    let tp = pixels_of(&t_img);
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let report = fit_recipe(&source, &target);
    let supported = evidence.spatial_supported.iter().filter(|&&v| v).count();
    let luma_weight: f32 = evidence.luma.iter().map(|r| r.weight).sum();
    let joint_base = crate::fit_zoned::joint_reading_with_evidence(
        &sp,
        &tp,
        &evidence.source_weights,
        &evidence.target_weights,
    );
    let after = pixels_of(&render::develop_preview(&s_img, &report.recipe));
    let joint_after = crate::fit_zoned::joint_reading_with_evidence(
        &after,
        &tp,
        &evidence.source_weights,
        &evidence.target_weights,
    );
    eprintln!(
        "SAME_CONTENT_DIAG cornwall d={:.4} supported={}/{} ident={:.4} luma_weight={:.4} look={:.6}->{:.6} joint={joint_base:?}->{joint_after:?} recipe={:?}",
        report.divergence.expect("resolvable").d,
        supported,
        evidence.spatial_supported.len(),
        evidence.identifiability,
        luma_weight,
        report.err_before,
        report.err_after,
        report.recipe,
    );
    let scale = s_img.width().max(s_img.height()).div_ceil(192).max(1);
    let sw = s_img.width().div_ceil(scale);
    let sh = s_img.height().div_ceil(scale);
    let mut ss = Vec::new();
    let mut tt = Vec::new();
    for y in (0..s_img.height()).step_by(scale as usize) {
        for x in (0..s_img.width()).step_by(scale as usize) {
            let i = (y * s_img.width() + x) as usize;
            ss.push(sp[i]);
            tt.push(tp[i]);
        }
    }
    let mut cells = Vec::new();
    for row in 0..3u32 {
        for col in 0..3u32 {
            let mut mask = vec![0.0f32; ss.len()];
            for y in 0..sh {
                for x in 0..sw {
                    if x * 3 / sw == col && y * 3 / sh == row {
                        mask[(y * sw + x) as usize] = 1.0;
                    }
                }
            }
            cells.push(structure_divergence(&ss, &tt, sw, sh, &mask).map(|r| r.d));
        }
    }
    eprintln!("SAME_CONTENT_DIAG cells={cells:?}");
    assert!(report.divergence.expect("resolvable").d < DIVERGENCE_GLOBAL);
    assert!(supported > evidence.spatial_supported.len() / 2);
    assert!(luma_weight > 0.5);
    // The paired robust estimator fits this real pair — re-measured
    // 2026-09-02 on the SHIPPED v1.2.3 panel: look 0.05888 -> 0.03061,
    // joint 0.17777 -> 0.04699, with the panel's cast projected at
    // t = 0.515 — so pin the fit, not a reset. The viaduct panel is the
    // wrong subject
    // here — its top row is the pair whose full solve needed zones and
    // tiles, and on the panel thumbnails the global-only path ends in a
    // do-no-harm reset (0.0354 -> 0.0354), which is exactly what this test
    // must NOT be satisfied by.
    assert!(
        report.err_after < report.err_before,
        "the paired path must actually fit this pair: {:.4} -> {:.4}",
        report.err_before,
        report.err_after
    );
    assert!(report.err_after < 0.045, "look residual {:.4}", report.err_after);
    assert!(
        !report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_NOTE_REGRESSED),
        "no do-no-harm terminal reset on a fittable pair: {}",
        report.recipe.rationale
    );
    let joint = joint_after.expect("the joint family had an opinion before the fit");
    assert!(joint.weighted < 0.06, "joint after {:.4}", joint.weighted);
}

/// Step-7b conservation law, half one: an IDENTITY field (every cell maps
/// to itself at full confidence) projects to the original target array
/// byte-for-byte — a field that says "nothing moved, everything
/// corresponds" must change nothing. Supervisor mutation M-7b-A (the
/// within-cell offset dropped from the remap) goes red here: without it
/// every pixel of a cell reads the cell's one corner sample.
#[test]
fn the_field_remap_is_identity_under_an_identity_field() {
    let (w, h) = (96u32, 64u32);
    let tp: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let v = (i.wrapping_mul(2_654_435_761) >> 8) as u8 as f32 / 255.0;
            [v, v, v]
        })
        .collect();
    let c = identity_field();
    let pc = correspondence_for_pair(&c, &tp, (w, h), (w, h));
    assert_eq!(pc.tp, tp, "identity field, identical dims: the remap must be a no-op");
    assert!(pc.conf.iter().all(|&x| x == 1.0));
    assert!((pc.coverage - 1.0).abs() < 1e-6 && (pc.median - 1.0).abs() < 1e-6);
}

/// A full-confidence identity field over the correspondence grid —
/// the shared conservation fixture (`correspond::identity_test_field`).
fn identity_field() -> crate::correspond::CorrespondenceField {
    crate::correspond::identity_test_field()
}

/// Step-7b, the mechanism itself: content SHIFTED between the renditions
/// breaks same-index pairing (the estimator sees a random association and
/// refuses), and the field's remap repairs it — the paired robust fit
/// recovers the true tone map through the shift. Supervisor mutation
/// M-7b-C (confidence/remap not reaching the pairs) goes red here.
#[test]
fn a_confident_shift_field_recovers_the_pairs_the_shift_broke() {
    let g = crate::correspond::GRID;
    let (w, h) = (192u32, 192u32); // 4 px per grid cell
    let shift_cells = 6usize; // content moves right by 24 px
    let col_luma = |x: u32| -> f32 {
        0.08 + 0.84 * ((x.wrapping_mul(2_654_435_761) >> 8) as u8 as f32 / 255.0)
    };
    let tone = |s: f32| 0.15 + 0.6 * s;
    let sp: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let v = col_luma(i % w);
            [v, v, v]
        })
        .collect();
    // The target: the SAME columns, tone-mapped, moved right by the shift
    // (content at source x sits at target x + 24).
    let tp: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let x = i % w;
            let v = tone(col_luma((x + w - 24) % w));
            [v, v, v]
        })
        .collect();
    // Same-index pairing is a random association: the estimator must not
    // manufacture a map out of it (either refuses or lands far off).
    let broken = paired_robust_tone(&sp, &tp, &|_| 1.0, true);
    let map_err = |r: &PairedRobustTone| {
        (1..=9)
            .map(|k| {
                let x = k as f32 / 10.0;
                (sample_tone_points(&r.points, x) - tone(x)).abs()
            })
            .fold(0.0f32, f32::max)
    };
    if let Some(r) = broken.as_ref() {
        assert!(
            map_err(r) > 0.05 || r.rejected_share > 0.5,
            "premise: a 24-px shift must actually break same-index pairing"
        );
    }
    // The field that KNOWS the shift: cell cx corresponds to target cell
    // cx + 6. Confidence 1 — the sidecar's smoothness term would grant a
    // rigid translation exactly this.
    let cells = g * g;
    let field = crate::correspond::CorrespondenceField {
        map_x: (0..cells).map(|c| (((c % g) + shift_cells) % g) as f32).collect(),
        ..identity_field()
    };
    let pc = correspondence_for_pair(&field, &tp, (w, h), (w, h));
    let repaired = paired_robust_tone(&sp, &pc.tp, &|i| pc.conf[i], true)
        .expect("the remapped pairing must carry a fittable map");
    assert!(
        map_err(&repaired) < 0.03,
        "the remap must recover the true tone map through the shift: err {:.4}",
        map_err(&repaired)
    );
}

/// Step-7b gate: the provider is consulted EXACTLY on content-divergent
/// pairs — never on a Full-mode pair (a paid-in-seconds sidecar run per
/// ordinary fit would be a regression), exactly once on a divergent one,
/// and a failing provider degrades with the reason in the rationale while
/// the atmosphere recipe stands. Supervisor mutation M-7b-B (the gate
/// dropped, provider consulted unconditionally) goes red on the first
/// assertion.
#[test]
fn the_provider_is_consulted_only_on_a_content_divergent_pair() {
    use std::cell::Cell;
    let calls = Cell::new(0u32);
    let failing = |_: &DynamicImage,
                   _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        calls.set(calls.get() + 1);
        Err(anyhow::anyhow!("no GPU on this machine"))
    };
    // Full-mode pair (a frame against itself): the gate must not consult.
    let (src, _) = structural_permutation_pair();
    let full = fit_recipe_with(&src, &src, FitOptions { strength: crate::recipe::GradeStrength::default(), provider: Some(&failing) });
    assert_eq!(calls.get(), 0, "a Full-mode fit must never pay for a correspondence run");
    assert_eq!(full.mode, FitMode::Full);
    // Divergent pair: exactly one consultation; the failure is disclosed
    // with its reason and the atmosphere fit stands unchanged.
    let (src, tgt) = structural_permutation_pair();
    let plain = fit_recipe(&src, &tgt);
    let report = fit_recipe_with(&src, &tgt, FitOptions { strength: crate::recipe::GradeStrength::default(), provider: Some(&failing) });
    assert_eq!(calls.get(), 1, "one divergent fit, one consultation");
    assert_eq!(report.mode, FitMode::Atmosphere);
    assert!(
        report.recipe.rationale.contains("no GPU on this machine"),
        "the failure reason must reach the rationale: {}",
        report.recipe.rationale
    );
    assert_eq!(
        (report.recipe.exposure_ev, report.recipe.tint, report.recipe.saturation),
        (plain.recipe.exposure_ev, plain.recipe.tint, plain.recipe.saturation),
        "a failing provider must leave the fit exactly as it was"
    );
}

/// R30 batch 1 (R2-lite): every Atmosphere report states which
/// POPULATION its white balance and exposure were read over — a
/// whole-frame per-channel median on both sides, i.e. a distribution
/// pairing whose premise is exactly what Atmosphere mode denies. Zero
/// behaviour change, so the assertion is on the note, and the dials are
/// pinned against the same fit before the disclosure existed.
#[test]
fn an_atmosphere_report_states_the_population_its_white_balance_came_from() {
    let (src, tgt) = structural_permutation_pair();
    let report = fit_recipe(&src, &tgt);
    assert_eq!(report.mode, FitMode::Atmosphere);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION),
        "the reference population must be disclosed: {}",
        report.recipe.rationale
    );
    // A Full-mode report must NOT carry it — the sentence is a claim
    // about the Atmosphere solve, and Full solves on paired evidence.
    let full = fit_recipe(&src, &src);
    assert_eq!(full.mode, FitMode::Full);
    assert!(
        !full
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION),
        "a Full report must not claim an Atmosphere population"
    );
}

/// R30 batch 1 (R2-lite): with no correspondence field the unpaired share
/// of that population is UNKNOWN, and an absent number must read as
/// unknown rather than as zero. With a field it is stated, with its
/// threshold and its grid resolution.
#[test]
fn the_unpaired_share_reads_as_unmeasured_when_there_is_no_field() {
    let (src, tgt) = structural_permutation_pair();
    let bare = fit_recipe(&src, &tgt);
    let has = |r: &FitReport, k: &str| r.notes.iter().any(|n| n.key == k);
    assert!(
        has(&bare, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNMEASURED),
        "no provider: the share is unmeasured, not zero: {}",
        bare.recipe.rationale
    );
    assert!(!has(&bare, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNPAIRED));
    // A failing provider is the same epistemic position, and its own
    // reason still rides the step-7b sentence.
    let failing = |_: &DynamicImage,
                   _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Err(anyhow::anyhow!("no GPU on this machine"))
    };
    let failed = fit_recipe_with(
        &src,
        &tgt,
        FitOptions {
            strength: crate::recipe::GradeStrength::default(),
            provider: Some(&failing),
        },
    );
    assert!(has(&failed, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNMEASURED));
    assert!(has(&failed, crate::rationale::keys::FIT_CORRESPONDENCE_UNAVAILABLE));
    // A measured field replaces "unmeasured" with the number.
    let ok = |_: &DynamicImage,
              _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(identity_field())
    };
    let measured = fit_recipe_with(
        &src,
        &tgt,
        FitOptions {
            strength: crate::recipe::GradeStrength::default(),
            provider: Some(&ok),
        },
    );
    // R30 R2: an identity field answers for every target cell, so the
    // restriction it authorises is the empty one — but the SHARE is now
    // an exclusion, and the sentence that reports it changed with it.
    assert!(has(&measured, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_EXCLUDED));
    assert!(!has(&measured, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNPAIRED));
    assert!(!has(&measured, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNMEASURED));
    // …and it must be a DIALLESS change: same recipe as the bare fit.
    assert_eq!(
        (
            measured.recipe.exposure_ev,
            measured.recipe.temperature_k,
            measured.recipe.tint,
            measured.recipe.saturation,
        ),
        (
            bare.recipe.exposure_ev,
            bare.recipe.temperature_k,
            bare.recipe.tint,
            bare.recipe.saturation,
        ),
        "R2-lite is disclosure only"
    );
}

/// R30 R2 fixture: RC-A's shape, reduced to something a unit test can
/// assert on, and deliberately sharper than the calibration pair.
///
/// Rows `[0, INVENTED_ROWS)` are INVENTED — the target replaced them with
/// content of its own texture — and are strictly BRIGHTER than the rest in
/// every channel, so they span percentiles 40-100 and therefore own every
/// whole-frame per-channel median by construction. Rows
/// `[INVENTED_ROWS, h)` CORRESPOND, and carry the one thing the fit is
/// supposed to find: a warm cast of `(1.060, 1.000, 0.955)` that sits
/// inside the Atmosphere white-balance budget.
///
/// The invented block is also 1.25x brighter than its own source, so a
/// whole-frame exposure reads +0.69 EV where the truth is 0.00 — the
/// fixture separates BOTH of the two controls this batch moves.
const INVENTED_ROWS: u32 = 58; // 29 of the sidecar's 48 cell rows
fn invented_half_pair() -> (DynamicImage, DynamicImage) {
    let (w, h) = (96u32, 96u32);
    let tex = |x: u32, y: u32, salt: u32| -> f32 {
        let v = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503) ^ salt.wrapping_mul(97))
            >> 9;
        (v & 0xff) as f32 / 255.0
    };
    let mut s = image::RgbImage::new(w, h);
    let mut t = image::RgbImage::new(w, h);
    let put = |img: &mut image::RgbImage, x: u32, y: u32, p: [f32; 3]| {
        img.put_pixel(
            x,
            y,
            image::Rgb([
                (p[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (p[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (p[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            ]),
        )
    };
    for y in 0..h {
        for x in 0..w {
            let n = tex(x, y, 0);
            if y < INVENTED_ROWS {
                let m = tex(x, y, 7);
                put(&mut s, x, y, [0.40 + 0.22 * n, 0.44 + 0.22 * n, 0.50 + 0.22 * n]);
                put(&mut t, x, y, [0.49 + 0.27 * m, 0.55 + 0.27 * m, 0.64 + 0.27 * m]);
            } else {
                let sp = [0.12 + 0.16 * n, 0.11 + 0.16 * n, 0.10 + 0.16 * n];
                put(&mut s, x, y, sp);
                put(&mut t, x, y, [sp[0] * 1.060, sp[1] * 1.000, sp[2] * 0.955]);
            }
        }
    }
    (DynamicImage::ImageRgb8(s), DynamicImage::ImageRgb8(t))
}

/// The white balance the corresponding region of [`invented_half_pair`]
/// actually carries, read as the ratio of that region's LINEAR per-channel
/// MEANS — a statistic with no bimodality to trip over, so it is the
/// truth the restricted solve is measured against rather than another
/// median. Measured: 1.2181.
const CORRESPONDING_TRUE_WB: f32 = 1.2181;
/// …and its true exposure: the corresponding region's target is its source
/// times a cast whose luma is 1.00, so a correct solve reads 0 EV.
const CORRESPONDING_TRUE_EV: f32 = 0.0;

/// Atmosphere mode on demand, through the route the zoned pass itself
/// uses (`divergent_zone_promotes`). These tests are about what the
/// Atmosphere solve READS, not about where a synthetic texture happens to
/// land relative to `DIVERGENCE_GLOBAL`; promoting explicitly keeps the
/// two questions apart, and keeps a fixture tweak from silently moving a
/// test onto the Full path where the restriction does not exist.
fn atmosphere_fit(
    src: &DynamicImage,
    tgt: &DynamicImage,
    provider: Option<CorrespondenceProvider<'_>>,
) -> FitReport {
    fit_recipe_from_promoted_with_disclosure_opts(
        src,
        tgt,
        &EditRecipe::default(),
        true,
        false,
        FitOptions { strength: crate::recipe::GradeStrength::default(), provider },
    )
}

/// A field confident only where [`invented_half_pair`] corresponds.
fn corresponding_field() -> crate::correspond::CorrespondenceField {
    let g = crate::correspond::GRID;
    let split = (INVENTED_ROWS as usize * g) / 96;
    crate::correspond::CorrespondenceField {
        confidence: (0..g * g).map(|c| if c / g >= split { 1.0 } else { 0.0 }).collect(),
        ..identity_field()
    }
}

/// The white-balance ratio a recipe's persisted dials actually apply —
/// `gr/gb`, the one number "warm or cold" means.
fn wb_ratio(r: &EditRecipe) -> f32 {
    let g = render::wb_gains(
        r.as_shot_k.unwrap_or(5500.0),
        r.temperature_k.unwrap_or(5500.0),
        r.tint,
    );
    g[0] / g[2]
}

/// R30 R2, the directional law: when a correspondence field says a slab of
/// the target has no counterpart in the source, the Atmosphere global
/// white balance and exposure must be solved from the part that DOES —
/// not from a whole-frame median that the invented slab helped define.
///
/// Both dials are asserted, in the direction the fixture makes true by
/// construction: the invented half is bluer and brighter than the half
/// that corresponds, so dropping it must move the white balance WARMER
/// and the exposure DOWN. Supervisor mutations M-R2-A (restriction
/// removed), M-R2-C (the cut kept everything) and M-R2-F (`target_answered`
/// always 1) all go red on the first assertion.
#[test]
fn the_atmosphere_solve_drops_target_content_no_source_answers_for() {
    let (src, tgt) = invented_half_pair();
    let field = |_: &DynamicImage,
                 _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(corresponding_field())
    };
    let whole = atmosphere_fit(&src, &tgt, None);
    let paired = atmosphere_fit(&src, &tgt, Some(&field));
    assert_eq!(whole.mode, FitMode::Atmosphere, "premise: both arms are the Atmosphere solve");
    assert_eq!(paired.mode, FitMode::Atmosphere, "the field never moves mode selection");
    match paired.atmosphere_reference {
        AtmosphereReference::SharedContent { .. } => {}
        other => panic!("the restriction must be in force, got {other:?}"),
    }
    // The whole-frame solve is defined by the invented block and misses the
    // cast the corresponding region carries (under the per-range consensus
    // of 2026-10-21 the invented half's cool against the real half's warm is
    // two lights, and the whole-frame solve ships nothing — the ratio of the
    // three marginal medians it replaced landed cool of neutral); the
    // restricted one recovers it.
    assert!(
        (wb_ratio(&whole.recipe) - CORRESPONDING_TRUE_WB).abs() >= 0.06,
        "premise: the invented block owns the whole-frame solve and hides the cast: {:.4}",
        wb_ratio(&whole.recipe)
    );
    assert!(
        (wb_ratio(&paired.recipe) - CORRESPONDING_TRUE_WB).abs() < 0.06,
        "the restricted solve must recover the corresponding region's white balance \
             {CORRESPONDING_TRUE_WB:.4}: whole {:.4}, paired {:.4}",
        wb_ratio(&whole.recipe),
        wb_ratio(&paired.recipe)
    );
    assert!(
        whole.recipe.exposure_ev > 0.5,
        "premise: the invented block is brighter and owns the whole-frame exposure: {}",
        whole.recipe.exposure_ev
    );
    assert!(
        (paired.recipe.exposure_ev - CORRESPONDING_TRUE_EV).abs() < 0.20,
        "the restricted solve must recover the corresponding region's exposure \
             {CORRESPONDING_TRUE_EV:.2}: whole {}, paired {}",
        whole.recipe.exposure_ev,
        paired.recipe.exposure_ev
    );
}

/// R30 R2: the restriction is applied to BOTH marginals, and this is the
/// test that says why it has to be. `median(target)/median(source)` is a
/// ratio of two populations; moving one of them onto the shared content
/// while the other stays on the whole frame does not repair the
/// mismatched pairing, it exchanges it for a louder one. On this fixture
/// the truth is `gr/gb` 1.2181 at 0.00 EV, and a one-sided cut answers:
///
/// | reference population | `gr/gb` | EV     |
/// |----------------------|---------|--------|
/// | whole frame          | 0.911   | +0.694 |
/// | TARGET side only     | 1.945   | −2.867 |
/// | SOURCE side only     | 0.512   | +3.593 |
/// | both (shipped)       | 1.216   | +0.032 |
///
/// Supervisor mutations M-R2-I (source restriction dropped) and M-R2-J
/// (target restriction dropped) each go red on the exposure bound, which
/// no one-sided cut can satisfy — both overshoot the ±1 EV budget and peg.
#[test]
fn the_reference_restriction_moves_both_marginals_together() {
    let (src, tgt) = invented_half_pair();
    let field = |_: &DynamicImage,
                 _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(corresponding_field())
    };
    let paired = atmosphere_fit(&src, &tgt, Some(&field));
    // A one-sided cut cannot land here: each of them overshoots the EV
    // budget in opposite directions and pegs at ±1.00.
    assert!(
        paired.recipe.exposure_ev.abs() < 0.20,
        "a two-sided restriction reads the corresponding region's own exposure: {}",
        paired.recipe.exposure_ev
    );
    assert!(
        (wb_ratio(&paired.recipe) - CORRESPONDING_TRUE_WB).abs() < 0.06,
        "…and its own white balance: {:.4}",
        wb_ratio(&paired.recipe)
    );
    // The disclosure states both retained shares, because both were cut.
    let note = paired
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED)
        .expect("the shared-content sentence is present");
    assert!(note.args.iter().any(|(k, _)| *k == "src"));
    assert!(note.args.iter().any(|(k, _)| *k == "tgt"));
}

/// R30 R2, the conservation law: a pair with NO usable field is
/// byte-for-byte the fit it always was — the restriction is reachable only
/// through a field, and the unrestricted path reads the very slices it
/// always read. Both silences are covered: no provider at all, and a
/// provider that failed.
///
/// What this test does NOT pin, deliberately: that the no-field path reads
/// the EVIDENCE's own weights. Both arms here are no-field arms, so a
/// defect in that path moves them together and stays invisible. That half
/// is pinned by `a_field_that_drops_nothing_moves_nothing` (an identity
/// field takes the restricted path and must land on the same dials) and by
/// the pre-existing `wb_default_strength_is_byte_identical_to_head`.
/// Measured, not assumed: a mutation replacing the no-field reference with
/// a flat population goes red in four tests including those two.
#[test]
fn no_correspondence_field_leaves_the_atmosphere_dials_untouched() {
    let (src, tgt) = invented_half_pair();
    let bare = atmosphere_fit(&src, &tgt, None);
    let failing = |_: &DynamicImage,
                   _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Err(anyhow::anyhow!("no GPU on this machine"))
    };
    let failed = atmosphere_fit(&src, &tgt, Some(&failing));
    let r = &failed.recipe;
    assert_eq!(
        (r.exposure_ev, r.temperature_k, r.tint, r.saturation, r.contrast),
        (
            bare.recipe.exposure_ev,
            bare.recipe.temperature_k,
            bare.recipe.tint,
            bare.recipe.saturation,
            bare.recipe.contrast
        ),
        "a failed provider must change no dial against the no-provider fit"
    );
    assert_eq!(r.tone_curve, bare.recipe.tone_curve, "nor the tone curve");
    // Both silences reach the same verdict, and neither is the restricted
    // one: an absent field and a broken one are the same epistemic state.
    assert_eq!(bare.atmosphere_reference, AtmosphereReference::WholeFrame);
    assert_eq!(failed.atmosphere_reference, AtmosphereReference::WholeFrame);
}

/// R30 R2, the other conservation law: a field that answers for EVERY
/// target cell authorises the empty restriction, and the empty restriction
/// must move nothing. This is the law that keeps the mechanism honest —
/// a restriction that changed dials when it dropped nothing would be
/// changing the estimator, not its population.
#[test]
fn a_field_that_drops_nothing_moves_nothing() {
    let (src, tgt) = invented_half_pair();
    let ok = |_: &DynamicImage,
              _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(identity_field())
    };
    let bare = atmosphere_fit(&src, &tgt, None);
    let full = atmosphere_fit(&src, &tgt, Some(&ok));
    match full.atmosphere_reference {
        AtmosphereReference::SharedContent { source, target } => assert!(
            (source - 1.0).abs() < 1e-6 && (target - 1.0).abs() < 1e-6,
            "an identity field retains both sides whole: {source} / {target}"
        ),
        other => panic!("expected the restriction in force, got {other:?}"),
    }
    assert_eq!(
        (
            full.recipe.exposure_ev,
            full.recipe.temperature_k,
            full.recipe.tint,
            full.recipe.saturation
        ),
        (
            bare.recipe.exposure_ev,
            bare.recipe.temperature_k,
            bare.recipe.tint,
            bare.recipe.saturation
        ),
        "a restriction that drops nothing must move nothing"
    );
    assert_eq!(full.recipe.tone_curve, bare.recipe.tone_curve);
}

/// R30 R2: a paired target too thin to be READ as a population does not
/// get read. The whole-frame medians stand, the whole-frame sentence
/// stands with them, and a second sentence says why it had to — the
/// alternative (solving a global control on a corner of the frame and
/// calling it global) is the failure this batch exists to stop, in a
/// different costume. Supervisor mutation M-R2-B (the retention floor
/// removed) goes red here.
#[test]
fn a_thin_paired_target_keeps_the_whole_frame_reading_and_says_so() {
    let g = crate::correspond::GRID;
    // Confident only on the last four rows of cells: ~8% of the target is
    // answered, far under the retention floor.
    let sliver = |_: &DynamicImage,
                  _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(crate::correspond::CorrespondenceField {
            confidence: (0..g * g)
                .map(|c| if c / g >= g - 4 { 1.0 } else { 0.0 })
                .collect(),
            ..identity_field()
        })
    };
    let (src, tgt) = invented_half_pair();
    let bare = atmosphere_fit(&src, &tgt, None);
    let thin = atmosphere_fit(&src, &tgt, Some(&sliver));
    match thin.atmosphere_reference {
        AtmosphereReference::Thin { source, target } => assert!(
            source < SHARED_POPULATION_MIN_RETENTION
                || target < SHARED_POPULATION_MIN_RETENTION,
            "the fixture must put a side under the floor: {source} / {target}"
        ),
        other => panic!("expected a thin verdict, got {other:?}"),
    }
    let has = |r: &FitReport, k: &str| r.notes.iter().any(|n| n.key == k);
    assert!(has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION));
    assert!(has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_THIN));
    assert!(!has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED));
    assert!(has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNPAIRED));
    assert!(!has(&thin, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_EXCLUDED));
    assert_eq!(
        (thin.recipe.exposure_ev, thin.recipe.temperature_k, thin.recipe.tint),
        (bare.recipe.exposure_ev, bare.recipe.temperature_k, bare.recipe.tint),
        "a refused restriction must leave the solve exactly as it was"
    );
}

/// R30 R2, added at adjudication: the retention floor is TWO-SIDED, and
/// the target's mask is the ANSWERED bitmap rather than the source's
/// confidence. The batch's own fixtures keep the two sides' retention
/// equal, so neither rule was being measured.
///
/// A field can be fully confident and still answer for almost none of the
/// target: every source cell landing in one corner is what "the target is
/// mostly generated" looks like at the limit — the island pair's 24% and
/// `p37`'s 93% pushed all the way. There the source keeps ALL of its
/// evidence while the target keeps a sliver, and the two rules diverge:
///
///   * a floor that accepted EITHER side would read the two medians over
///     a whole source and a corner of a target — a louder version of the
///     mismatched pairing this batch exists to repair, not a repair;
///   * a target mask taken from `conf` would call the target fully
///     retained, because every SOURCE cell is confident, and restrict on
///     a population it never measured.
///
/// Adjudicator mutations ADJ-1 (`readable()` on `||`) and ADJ-2 (the
/// target side masked by `conf`) both go red here; both were green
/// against the eight tests the batch shipped with.
#[test]
fn a_confident_field_answering_only_a_corner_is_thin_on_the_target_side() {
    let g = crate::correspond::GRID;
    // Every cell confident, every cell landing in the same 4x4 corner:
    // 16 of the grid's cells are answered for, and the rest of the
    // target is content no source cell speaks for.
    let corner = |_: &DynamicImage,
                  _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(crate::correspond::CorrespondenceField {
            confidence: vec![1.0; g * g],
            map_x: (0..g * g).map(|c| (c % 4) as f32).collect(),
            map_y: (0..g * g).map(|c| ((c / g) % 4) as f32).collect(),
            ..identity_field()
        })
    };
    let (src, tgt) = invented_half_pair();
    let bare = atmosphere_fit(&src, &tgt, None);
    let skewed = atmosphere_fit(&src, &tgt, Some(&corner));
    match skewed.atmosphere_reference {
        AtmosphereReference::Thin { source, target } => {
            // The premise: this fixture SEPARATES the two sides. Without
            // that separation the test would pass against both mutants
            // for the same reason the batch's fixtures did.
            assert!(
                source >= SHARED_POPULATION_MIN_RETENTION,
                "premise: the source side must stay fat, got {source}"
            );
            assert!(
                target < SHARED_POPULATION_MIN_RETENTION,
                "premise: the target side must fall under the floor, got {target}"
            );
        }
        other => panic!(
            "a fat source and a cornered target must refuse, got {other:?}"
        ),
    }
    let has = |r: &FitReport, k: &str| r.notes.iter().any(|n| n.key == k);
    assert!(has(&skewed, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_THIN));
    assert!(!has(&skewed, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED));
    assert_eq!(
        (skewed.recipe.exposure_ev, skewed.recipe.temperature_k, skewed.recipe.tint),
        (bare.recipe.exposure_ev, bare.recipe.temperature_k, bare.recipe.tint),
        "a refused restriction must leave the solve exactly as it was"
    );
}

/// R30 R2: the rationale says which of the three things happened, and
/// never two of them. The whole-frame sentence keeps its exact old
/// meaning, so it must be ABSENT the moment the medians came from
/// somewhere else — and R2-lite's "defined those two controls all the
/// same" must be absent with it, because it is then false. Supervisor
/// mutations M-R2-D (the shared sentence pushed unconditionally) and
/// M-R2-E (the old unpaired key kept while restricted) go red here.
#[test]
fn the_reference_disclosure_names_exactly_one_population() {
    let (src, tgt) = invented_half_pair();
    let field = |_: &DynamicImage,
                 _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(corresponding_field())
    };
    let paired = atmosphere_fit(&src, &tgt, Some(&field));
    let has = |r: &FitReport, k: &str| r.notes.iter().any(|n| n.key == k);
    assert!(has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED));
    assert!(
        !has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION),
        "the whole-frame claim must not survive a restricted solve: {}",
        paired.recipe.rationale
    );
    assert!(!has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_THIN));
    assert!(has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_EXCLUDED));
    assert!(!has(&paired, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_UNPAIRED));
    // The share it prints is the share it dropped, to the printed
    // precision — the disclosure and the population are one fact.
    let retained = match paired.atmosphere_reference {
        AtmosphereReference::SharedContent { target, .. } => target,
        other => panic!("expected the restriction in force, got {other:?}"),
    };
    let printed = paired
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED)
        .and_then(|n| n.args.iter().find(|(k, _)| *k == "tgt").map(|(_, v)| v.clone()))
        .expect("the shared sentence carries its retained share");
    assert_eq!(printed, format!("{:.0}", retained * 100.0));
    // A whole-frame report is the mirror image, with no leakage either way.
    let whole = atmosphere_fit(&src, &tgt, None);
    assert!(has(&whole, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_POPULATION));
    assert!(!has(&whole, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_SHARED));
    assert!(!has(&whole, crate::rationale::keys::FIT_ATMOSPHERE_REFERENCE_THIN));
}

/// R30 R2: the retained share the sentence prints is read off the SAME
/// mask the solve drops, weighed on the EVIDENCE MASS the solve actually
/// carries — not off a pixel count, and not off `target_unpaired`'s
/// grid-resolution twin. One derivation, two consumers.
///
/// The evidence model here is built from a real pair and then given
/// deliberately NON-UNIFORM weights, because a derived model cannot
/// separate the two readings: `evidence_range` sets a range's weight to
/// its own `two_sided_share`, so the per-pixel weight is
/// `weight / target_evidence_share` ~ 1 wherever a range is two-sided and
/// the mass share equals the pixel share by construction. The first
/// version of this test used a derived model, the two shares coincided
/// (0.494 against 0.500), and supervisor mutation M-R2-H — the share
/// counted on pixels — survived it GREEN. With the weights below the two
/// readings are 0.90 and 0.50, and M-R2-H goes red.
#[test]
fn the_retained_share_is_the_evidence_mass_the_mask_keeps() {
    let (w, h) = (96u32, 64u32);
    let n = (w * h) as usize;
    let sp: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            let v = 0.1 + 0.8 * ((i % 251) as f32 / 251.0);
            [v, v, v]
        })
        .collect();
    let tp: Vec<[f32; 3]> = sp.iter().map(|p| [p[0] * 0.9, p[1] * 0.9, p[2] * 0.9]).collect();
    let mut evidence = evidence_model_for(&sp, &tp, w, h);
    // Ten times the weight on the bottom half of the frame, on both
    // sides. The field below keeps exactly that half.
    let heavy = |i: usize| if i / w as usize >= (h / 2) as usize { 1.0 } else { 0.1 };
    for (i, weight) in evidence.source_weights.iter_mut().enumerate() {
        *weight = heavy(i);
    }
    for (i, weight) in evidence.target_weights.iter_mut().enumerate() {
        *weight = heavy(i);
    }
    let g = crate::correspond::GRID;
    let field = crate::correspond::CorrespondenceField {
        confidence: (0..g * g).map(|c| if c / g >= g / 2 { 1.0 } else { 0.0 }).collect(),
        ..identity_field()
    };
    let pc = correspondence_for_pair(&field, &tp, (w, h), (w, h));
    let pop = shared_content_population(&evidence, &pc).expect("a populated pair restricts");
    // The premise the test rests on: on this model the mass share and the
    // pixel share are DIFFERENT numbers, so an implementation counting the
    // wrong one cannot pass by coincidence.
    let kept_pixels = pc.target_answered[..pop.target.len()]
        .iter()
        .filter(|a| **a > 0.0)
        .count() as f32
        / pop.target.len() as f32;
    assert!(
        (pop.target_retained - kept_pixels).abs() > 0.15,
        "premise: mass share {} must be far from the pixel share {kept_pixels}",
        pop.target_retained
    );
    for (kept_v, all_v, retained, label) in [
        (&pop.source, &evidence.source_weights, pop.source_retained, "source"),
        (&pop.target, &evidence.target_weights, pop.target_retained, "target"),
    ] {
        let kept: f64 = kept_v.iter().map(|w| w.max(0.0) as f64).sum();
        let all: f64 = all_v[..kept_v.len()].iter().map(|w| w.max(0.0) as f64).sum();
        assert!(all > 0.0, "premise: the fixture carries {label} evidence");
        assert!(
            (retained as f64 - kept / all).abs() < 1e-5,
            "the {label} retained share must be the kept evidence mass: {retained} vs {}",
            kept / all
        );
        // …and every kept weight is the evidence's own, never a rescaled one.
        for i in 0..kept_v.len() {
            let w = all_v[i];
            assert!(
                kept_v[i] == w || kept_v[i] == 0.0,
                "a kept {label} weight must be the evidence's own: {} vs {w}",
                kept_v[i]
            );
        }
    }
}

/// R30 R2: the restriction moves the reference population and NOTHING
/// else the Atmosphere contract promises — the confidence cap, the
/// structure-blind ruler and the absence of channel curves all survive it.
#[test]
fn the_atmosphere_contract_survives_the_reference_restriction() {
    let (src, tgt) = invented_half_pair();
    let field = |_: &DynamicImage,
                 _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(corresponding_field())
    };
    let paired = atmosphere_fit(&src, &tgt, Some(&field));
    assert!(
        paired.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP,
        "the atmosphere cap is not negotiable: {}",
        paired.recipe.confidence
    );
    assert!(
        paired.recipe.red_curve.is_empty()
            && paired.recipe.green_curve.is_empty()
            && paired.recipe.blue_curve.is_empty(),
        "Atmosphere mode never emits channel curves"
    );
    assert!(
        paired.structural_evidence.is_some(),
        "the structural model still travels beside the blind ruler"
    );
    let bare = atmosphere_fit(&src, &tgt, None);
    assert_eq!(
        paired.evidence.spatial_weights, bare.evidence.spatial_weights,
        "the structure-blind ruler is untouched by the restriction"
    );
}

/// R30 batch 1 (R2-lite): the unpaired share is a TARGET-side reading,
/// not the source-side `coverage` under another name. An identity field
/// answers for every target cell, so the share is zero; a field where
/// only the top half of the source is confident (and maps into the top
/// half of the target) leaves the bottom half of the target unanswered.
#[test]
fn the_unpaired_share_is_read_from_the_targets_side() {
    let g = crate::correspond::GRID;
    let (w, h) = (96u32, 64u32);
    let tp: Vec<[f32; 3]> = vec![[0.5; 3]; (w * h) as usize];
    let identity = correspondence_for_pair(&identity_field(), &tp, (w, h), (w, h));
    assert!(
        identity.target_unpaired.abs() < 1e-6,
        "an identity field answers for every target cell: {}",
        identity.target_unpaired
    );
    assert_eq!(identity.grid, (g, g));
    // Confidence only in the top half. Coverage (source side) and the
    // unpaired share (target side) must BOTH read a half — the same
    // number here, by construction, but from opposite sides.
    let half = crate::correspond::CorrespondenceField {
        confidence: (0..g * g).map(|c| if c / g < g / 2 { 1.0 } else { 0.0 }).collect(),
        ..identity_field()
    };
    let pc = correspondence_for_pair(&half, &tp, (w, h), (w, h));
    assert!((pc.coverage - 0.5).abs() < 1e-6, "coverage {}", pc.coverage);
    assert!((pc.target_unpaired - 0.5).abs() < 1e-6, "unpaired {}", pc.target_unpaired);
    // And now the case that separates them: EVERY source cell is
    // confident, but they all map onto the top half of the target. The
    // source-side coverage says 100%; the target-side share says half the
    // target had no partner — which is the reading R2-lite exists to
    // publish, and the one `coverage` cannot give.
    let piled = crate::correspond::CorrespondenceField {
        map_y: (0..g * g).map(|c| ((c / g) / 2) as f32).collect(),
        ..identity_field()
    };
    let pc = correspondence_for_pair(&piled, &tp, (w, h), (w, h));
    assert!((pc.coverage - 1.0).abs() < 1e-6, "coverage {}", pc.coverage);
    assert!(
        (pc.target_unpaired - 0.5).abs() < 1e-6,
        "a fully confident field can still leave half the target unpaired: {}",
        pc.target_unpaired
    );
}

/// Step-7b conservation law, half two: a field that answers "everything
/// corresponds in place" leaves the divergent fit's DIALS untouched (the
/// disclosure note is the only difference), and mode selection never
/// reads the field at all.
#[test]
fn an_identity_field_discloses_and_changes_no_dial() {
    let ok = |_: &DynamicImage,
              _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(identity_field())
    };
    let (src, tgt) = structural_permutation_pair();
    let plain = fit_recipe(&src, &tgt);
    let report = fit_recipe_with(&src, &tgt, FitOptions { strength: crate::recipe::GradeStrength::default(), provider: Some(&ok) });
    assert_eq!(report.mode, FitMode::Atmosphere, "mode selection never reads the field");
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_CORRESPONDENCE),
        "the measured field must be disclosed: {}",
        report.recipe.rationale
    );
    assert_eq!(
        (report.recipe.exposure_ev, report.recipe.tint, report.recipe.saturation),
        (plain.recipe.exposure_ev, plain.recipe.tint, plain.recipe.saturation),
        "an identity field must change no dial"
    );
    assert!(report.correspondence.is_some(), "the zoned passes read it from the report");
}

#[test]
fn content_divergent_calibration_keeps_an_atmosphere_recipe() {
    let Some(root) = calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    let report = fit_recipe(&source, &target);
    eprintln!(
        "CONTENT_DIVERGENT_DIAG mode={:?} d={:.4} look={:.6}->{:.6} confidence={:.3} recipe ev={:.2} temp={:?} tint={:.1} curve={} sat={:.1} detail={:.1}/{:.1}",
        report.mode,
        report.divergence.map_or(f32::NAN, |r| r.d),
        report.err_before,
        report.err_after,
        report.recipe.confidence,
        report.recipe.exposure_ev,
        report.recipe.temperature_k,
        report.recipe.tint,
        report.recipe.tone_curve.len(),
        report.recipe.saturation,
        report.recipe.clarity,
        report.recipe.texture,
    );
    assert_eq!(report.mode, FitMode::Atmosphere);
    assert!(
        report.recipe.exposure_ev.abs() > 0.001
            || report.recipe.temperature_k.is_some()
            || report.recipe.tint.abs() > 0.001
            || !report.recipe.tone_curve.is_empty()
            || report.recipe.saturation.abs() > 0.001
            || report.recipe.clarity.abs() > 0.001
            || report.recipe.texture.abs() > 0.001,
        "a content-divergent pair must return a non-empty Atmosphere recipe: {}",
        report.recipe.rationale,
    );
    assert!(report.recipe.red_curve.is_empty());
    assert!(report.recipe.green_curve.is_empty());
    assert!(report.recipe.blue_curve.is_empty());
    assert!(report.recipe.exposure_ev <= -0.5);
}

#[test]
fn calibration_atmosphere_report_uses_one_population_ruler() {
    let Some(root) = calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    let report = fit_recipe(&source, &target);
    assert_eq!(report.mode, FitMode::Atmosphere);
    assert!(report.structural_evidence.is_some());
    assert!((-1.0..=-0.5).contains(&report.recipe.exposure_ev));
    assert_eq!(report.recipe.saturation, 0.0);
    assert_eq!((report.recipe.clarity, report.recipe.texture), (0.0, 0.0));
    assert!(!report.recipe.tone_curve.is_empty());
    assert!(report.err_after < report.err_before);
    assert!(report.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP);
    let note = report
        .notes
        .iter()
        .find(|note| {
            note.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_POPULATION_EVIDENCE
        })
        .expect("Atmosphere reports disclose the structural ranges excluded from their ruler");
    let ranges = note
        .args
        .iter()
        .find(|(key, _)| *key == "luma_ranges")
        .map(|(_, value)| value.as_str())
        .expect("population-evidence note carries luma_ranges");
    let names_an_interior_range = ranges.split("luma[").skip(1).any(|part| {
        let Some((bounds, _)) = part.split_once(']') else { return false };
        let Some((lo, hi)) = bounds.split_once('-') else { return false };
        let (Ok(lo), Ok(hi)) = (lo.parse::<f32>(), hi.parse::<f32>()) else {
            return false;
        };
        lo >= 0.29 && hi <= 0.82
    });
    assert!(
        names_an_interior_range,
        "disclosure did not name a structural range in [0.29, 0.82]: {ranges}"
    );
}

#[test]
fn calibration_atmosphere_rescore_reproduces_report_ruler() {
    let Some(root) = calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    let solved = fit_recipe(&source, &target);
    let rescored = rescore_report(
        &source,
        &target,
        &solved.recipe,
        &EditRecipe::default(),
        solved.err_before,
        &solved.notes,
    );
    assert_eq!(rescored.mode, FitMode::Atmosphere);
    assert!(rescored.structural_evidence.is_some());
    assert_eq!(rescored.err_before.to_bits(), solved.err_before.to_bits());
    assert_eq!(rescored.err_after.to_bits(), solved.err_after.to_bits());
    assert_eq!(
        rescored.recipe.confidence.to_bits(),
        solved.recipe.confidence.to_bits(),
        "solve and rescore must derive confidence from the same blind ruler"
    );
    assert_evidence_models_bit_equal(&rescored.evidence, &solved.evidence);
}

#[test]
fn atmosphere_global_obeys_ev_wb_saturation_and_curve_budgets() {
    let (src, tgt) = structural_permutation_pair();
    let report = fit_recipe(&src, &tgt);
    assert_eq!(report.mode, FitMode::Atmosphere, "premise: {:?}", report.divergence);
    let r = &report.recipe;
    assert!(r.exposure_ev.abs() <= ATMOSPHERE_EV_LIMIT);
    assert!(r.saturation.abs() <= ATMOSPHERE_SAT_LIMIT);
    if let Some(k) = r.temperature_k {
        let gains = render::wb_gains(r.as_shot_k.unwrap_or(5500.0), k, r.tint);
        let lo = gains.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = gains.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(gains.iter().all(|g| {
            (ATMOSPHERE_WB_GAIN_MIN..=ATMOSPHERE_WB_GAIN_MAX).contains(g)
        }));
        assert!(hi / lo <= ATMOSPHERE_WB_GAIN_RATIO + 1e-5);
    }
    assert_eq!(r.tone_curve.len(), 5, "Atmosphere tone must stay a robust five-point map");
    for pair in r.tone_curve.windows(2) {
        let slope = (pair[1].output as f32 - pair[0].output as f32)
            / (pair[1].input as f32 - pair[0].input as f32);
        assert!(
            (ATMOSPHERE_CURVE_SLOPE_MIN - 1e-6..=ATMOSPHERE_CURVE_SLOPE_MAX + 1e-6)
                .contains(&slope),
            "atmosphere slope {slope} escaped its budget: {:?}",
            r.tone_curve
        );
    }
}

#[test]
fn atmosphere_global_never_emits_rgb_curves_and_caps_confidence() {
    let (src, tgt) = structural_permutation_pair();
    let report = fit_recipe(&src, &tgt);
    assert_eq!(report.mode, FitMode::Atmosphere);
    assert!(report.recipe.red_curve.is_empty());
    assert!(report.recipe.green_curve.is_empty());
    assert!(report.recipe.blue_curve.is_empty());
    assert!(report.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP);
}

#[test]
fn a_divergent_sky_promotes_the_global_fit_when_it_covers_35_percent() {
    let source = synth();
    let full = fit_recipe_from_promoted(&source, &source, &EditRecipe::default(), false);
    assert_eq!(full.mode, FitMode::Full, "premise: matched content uses Full mode");
    let promoted = fit_recipe_from_promoted(&source, &source, &EditRecipe::default(), true);
    assert_eq!(promoted.mode, FitMode::Atmosphere);
    assert!(
        promoted.divergence.expect("resolvable").d < DIVERGENCE_GLOBAL,
        "the zone-share branch, not global D, must be load-bearing"
    );
    assert_eq!(DIVERGENT_COVER_PROMOTES, 0.35);
}

#[test]
fn residual_curve_cannot_exceed_two_to_one_slope() {
    let cliff = vec![
        CurvePoint { input: 0, output: 0 },
        CurvePoint { input: 64, output: 20 },
        CurvePoint { input: 128, output: 40 },
        CurvePoint { input: 149, output: 98 },
        CurvePoint { input: 170, output: 193 },
        CurvePoint { input: 255, output: 255 },
    ];
    let projected = project_curve_slopes(&cliff, 0.0, RESIDUAL_SLOPE_CAP);
    assert_eq!(projected.first(), cliff.first());
    assert_eq!(projected.last(), cliff.last());
    for pair in projected.windows(2) {
        assert!(pair[1].output >= pair[0].output, "projection lost monotonicity");
        let slope = (pair[1].output as f32 - pair[0].output as f32)
            / (pair[1].input as f32 - pair[0].input as f32);
        assert!(slope <= RESIDUAL_SLOPE_CAP + 1e-6, "slope {slope}: {projected:?}");
    }
    let already_safe = vec![
        CurvePoint { input: 0, output: 0 },
        CurvePoint { input: 64, output: 48 },
        CurvePoint { input: 128, output: 128 },
        CurvePoint { input: 192, output: 208 },
        CurvePoint { input: 255, output: 255 },
    ];
    assert_eq!(
        project_curve_slopes(&already_safe, 0.0, RESIDUAL_SLOPE_CAP),
        already_safe,
        "an in-budget showcase-like curve must remain byte-identical"
    );
}

#[test]
fn atmosphere_saturation_cap_is_load_bearing() {
    // A structurally divergent pair whose target ALSO demands far more
    // chroma than the +/-30 budget allows. Atmosphere reads that demand on
    // population evidence, so the fitted demand must land ON the budget;
    // without the clamp the chase would run past it.
    let (src, tgt) = structural_permutation_pair();
    let boosted = render::develop_preview(
        &tgt,
        &EditRecipe { saturation: 90.0, ..Default::default() },
    );
    let report = fit_recipe(&src, &boosted);
    assert_eq!(report.mode, FitMode::Atmosphere, "premise: {:?}", report.divergence);
    assert_eq!(ATMOSPHERE_SAT_LIMIT, 30.0, "the calibrated atmosphere saturation budget");
    assert_eq!(
        report.recipe.saturation, ATMOSPHERE_SAT_LIMIT,
        "population evidence may reach, but never exceed, the Atmosphere budget"
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_SAT_PEGGED),
        "hitting the cap has to be disclosed: {}",
        report.recipe.rationale
    );
    assert!(report.notes.iter().any(|n| {
        n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_POPULATION_EVIDENCE
    }));
}

#[test]
fn residual_tone_curve_projects_a_cliff_through_the_real_producer() {
    // The PRODUCER, not the projector helper: `residual_curve_cannot_
    // exceed_two_to_one_slope` passes the cap into `project_curve_slopes`
    // itself, so it stays green when the constant is raised or when the
    // producer stops calling it. This drives the 4:1 upper ramp that the
    // generated-cloud fit drew and demands the shipped points obey 2:1.
    let recipe = EditRecipe::default();
    let cliff = |x: f32| {
        if x < 0.62 { x * 0.45 } else { (0.279 + (x - 0.62) * 4.0).min(1.0) }
    };
    let pts = residual_tone_curve(&recipe, &cliff);
    assert!(!pts.is_empty(), "premise: the sliders alone cannot express this map");
    assert_eq!(RESIDUAL_SLOPE_CAP, 2.0, "the calibrated residual-curve slope cap");
    for pair in pts.windows(2) {
        let slope = (pair[1].output as f32 - pair[0].output as f32)
            / (pair[1].input as f32 - pair[0].input as f32).max(1.0);
        assert!(
            slope <= 2.0 + 1e-6,
            "a shipped residual segment kept slope {slope}: {pts:?}"
        );
    }
}

#[test]
fn ordinary_same_content_roundtrip_remains_in_full_fit_mode() {
    let source = synth();
    let target = render::develop_preview(
        &source,
        &EditRecipe {
            exposure_ev: 0.35,
            contrast: 18.0,
            highlights: -25.0,
            whites: 12.0,
            saturation: 15.0,
            ..Default::default()
        },
    );
    let report = fit_recipe(&source, &target);
    assert_eq!(
        report.mode,
        FitMode::Full,
        "same-content engine roundtrip diverged: {:?}",
        report.divergence
    );
}

#[test]
fn atmosphere_rationale_names_unrecoverable_structure_and_discloses_d() {
    let (src, tgt) = structural_permutation_pair();
    let report = fit_recipe(&src, &tgt);
    let summary = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_SUMMARY_ATMOSPHERE)
        .expect("Atmosphere summary note");
    let disclosed = summary.args.iter().find(|(key, _)| *key == "d").unwrap().1.clone();
    assert_eq!(
        disclosed,
        format!("{:.3}", report.divergence.expect("resolvable").d)
    );
    assert!(report.recipe.rationale.contains("structure cannot be reconstructed"));
    assert!(report.recipe.rationale.contains(&format!("D={disclosed}")));
    assert!(report
        .notes
        .iter()
        .any(|n| n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_CONFIDENCE));
}

#[test]
fn identity_fit_is_near_neutral() {
    let img = synth();
    let rep = fit_recipe(&img, &img);
    let r = &rep.recipe;
    // The terminal do-no-harm reset would satisfy EVERY assertion below
    // by wiping the recipe to default, so a broken solve (exposure +3,
    // contrast +90) would look identical to a correct one. Demand that
    // the safety net did NOT fire: on an identity pair the honest solve
    // is already near-neutral, so there is nothing for it to catch.
    assert!(
        !rep.recipe.rationale.contains("do-no-harm terminal case"),
        "the identity solve must stand on its own, not on the reset: {}",
        rep.recipe.rationale
    );
    assert!(r.exposure_ev.abs() < 0.06, "exposure {}", r.exposure_ev);
    for (name, v) in [
        ("contrast", r.contrast),
        ("highlights", r.highlights),
        ("shadows", r.shadows),
        ("whites", r.whites),
        ("blacks", r.blacks),
        ("saturation", r.saturation),
    ] {
        assert!(v.abs() < 6.0, "{name} should stay near 0, got {v}");
    }
    assert!(rep.err_after < 0.02, "identity residual {}", rep.err_after);
}

#[test]
fn roundtrip_recovers_tone_and_saturation() {
    // Render a KNOWN recipe through the real engine, then fit it back.
    let src = synth();
    let mut truth = EditRecipe {
        exposure_ev: 0.35,
        contrast: 18.0,
        highlights: -25.0,
        whites: 12.0,
        saturation: 15.0,
        ..Default::default()
    };
    truth.clamp();
    let target = render::develop_preview(&src, &truth);
    let rep = fit_recipe(&src, &target);
    let r = &rep.recipe;
    // The luma CDF of the target IS the engine's own tone map of the source,
    // so the solve must land close (exposure/slider trade-offs allowed).
    assert!((r.exposure_ev - 0.35).abs() < 0.20, "exposure {}", r.exposure_ev);
    assert!(r.contrast > 3.0 && r.contrast < 45.0, "contrast {}", r.contrast);
    assert!(r.highlights < -8.0 && r.highlights > -50.0, "highlights {}", r.highlights);
    assert!(r.saturation > 5.0 && r.saturation < 30.0, "saturation {}", r.saturation);
    // And the fitted recipe must actually reproduce the look through the engine.
    assert!(
        rep.err_after < (rep.err_before * 0.5).max(0.012),
        "residual {} vs before {}",
        rep.err_after,
        rep.err_before
    );
}

#[test]
fn hazy_to_clean_fit_stays_sane() {
    // Regression for the 2026-07-07 real-photo failure: fitting a
    // low-contrast, low-chroma, blue-cast base toward a clean punchy
    // target produced mutually-cancelling pegged tone sliders
    // (Exposure +1.5 / Contrast −97 / Shadows −100), pegged per-band hue
    // rotations (+45) and a purple sky — while the old metric reported
    // "improved". The prior, the stage order and the correspondence gate
    // must keep every fitted control in its sane regime.
    let clean = synth();
    let mut haze = EditRecipe {
        exposure_ev: -0.3,
        contrast: -45.0,
        blacks: 40.0,
        saturation: -40.0,
        // a shadow-weighted blue cast at realistic haze strength (the
        // midpoint pin keeps it out of the highlights, like real haze)
        blue_curve: vec![
            CurvePoint { input: 0, output: 25 },
            CurvePoint { input: 128, output: 132 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    haze.clamp();
    let base = render::develop_preview(&clean, &haze);
    let rep = fit_recipe(&base, &clean);
    let r = &rep.recipe;
    assert!(
        r.contrast > -20.0 && r.contrast.abs() < 90.0,
        "degenerate contrast {}",
        r.contrast
    );
    assert!(
        r.shadows.abs() < 90.0 && r.whites.abs() < 90.0 && r.blacks.abs() < 90.0,
        "pegged tone sliders: sh {} wh {} bl {}",
        r.shadows,
        r.whites,
        r.blacks
    );
    assert!(r.exposure_ev.abs() <= 1.0, "runaway exposure {}", r.exposure_ev);
    // NOTE deliberately no "slider not pegged" assertion for hue: the
    // correspondence gate already rejects mismatched populations, and a
    // genuine in-gate rotation larger than the engine's ±13.5° range
    // legitimately clamps. What must hold is the RESULT (below): no band
    // of the fitted render lands tens of degrees off the target.
    assert!(
        rep.err_after < rep.err_before,
        "fit made the look worse: {} -> {}",
        rep.err_before,
        rep.err_after
    );
    assert_eq!(rep.mode, FitMode::Full);
    assert!(
        r.exposure_ev.abs()
            + r.contrast.abs()
            + r.shadows.abs()
            + r.blacks.abs()
            + r.saturation.abs()
            > 1.0,
        "same-content haze removal was incorrectly returned as a neutral recipe: {:?}",
        r
    );
    // The decisive invariant: render the fitted recipe and check every
    // populated band's centroid hue against the target — the purple-sky
    // failure class means some band lands tens of degrees off.
    let fitted = pixels_of(&render::develop_preview(&base, &rep.recipe));
    let (fb, ftot) = band_stats(&fitted);
    let (tb, ttot) = band_stats(&pixels_of(&clean));
    let mut worst = 0.0f64;
    for i in 0..8 {
        let (x, y) = (&fb[i], &tb[i]);
        if x.w / ftot < 0.015 || y.w / ttot < 0.015 {
            continue;
        }
        let mut d = y.sin.atan2(y.cos).to_degrees() - x.sin.atan2(x.cos).to_degrees();
        while d > 180.0 {
            d -= 360.0;
        }
        while d < -180.0 {
            d += 360.0;
        }
        worst = worst.max(d.abs());
    }
    assert!(worst < 15.0, "a band's hue is still {worst:.1}° off after the fit");
}

/// 192×128 canyon: 15.6% neutral ramp (tone evidence — without it the
/// pale sky is the only `is_neutralish` population and the tone solve
/// degenerates), 68.8% warm-rock ramp, 15.6% pale-blue sky. `warm` = the
/// region-graded target: rocks red-lifted (`l^0.7`), ramp + sky IDENTICAL
/// to the source — the grade a global cast cannot express without
/// collateral damage.
fn canyon(warm: bool) -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let l = 0.15 + 0.80 * x as f32 / (w - 1) as f32;
            let p = if y < 16 {
                [l, l, l] // neutral ramp
            } else if y < 112 {
                // Rocks: the warm grade is a pure RED OFFSET — a hue move
                // toward orange that symmetric chroma expansion (the
                // saturation stage) cannot express, so the residual lands
                // squarely on the red channel curve, as in the real photo.
                let r = if warm { (0.85 * l + 0.18).min(1.0) } else { 0.85 * l };
                [r, 0.52 * l, 0.30 * l]
            } else {
                [0.64, 0.68, 0.73] // pale blue sky, hue ≈ 213°, chroma 0.09
            };
            img.put_pixel(
                x,
                y,
                image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
            );
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// [`canyon`]'s warm rock regrade with every rock pixel rescaled to its
/// source luma (`luma601` of the sRGB values; the rescale only ever darkens,
/// so nothing clips), so the paired tone arm has no brightness change to
/// read and the channel curves carry the whole regrade (2026-10-22). At the
/// warm pair's own +0.18 the rebuilt cast re-hues nothing (share 0.000);
/// +0.45 is the smallest offset swept (0.18 / 0.24 / 0.30 / 0.36 / 0.45)
/// whose curves violet the sky.
fn canyon_warm_luma_kept(offset: f32) -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let l = 0.15 + 0.80 * x as f32 / (w - 1) as f32;
            let p = if y < 16 {
                [l, l, l]
            } else if y < 112 {
                let source = [0.85 * l, 0.52 * l, 0.30 * l];
                let warm = [(0.85 * l + offset).min(1.0), 0.52 * l, 0.30 * l];
                let k = luma601(&source) / luma601(&warm);
                warm.map(|c| c * k)
            } else {
                [0.64, 0.68, 0.73]
            };
            img.put_pixel(
                x,
                y,
                image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
            );
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// R17: the P20 × reimagine murk, distilled — the target re-hues a
/// luma-CONCENTRATED bright band out of the source's neutral class. The
/// share ratio stays under 1.75× (the old gate passed and the murky fit
/// shipped), but the leavers bend the source evidence CDF far past the
/// contamination ceiling; the solve must fall back to full-pixel CDFs.
#[test]
fn a_rehued_bright_grey_band_falls_back_to_full_cdfs() {
    let n = 64 * 64;
    let mut sp: Vec<[f32; 3]> = Vec::with_capacity(n);
    let mut tp: Vec<[f32; 3]> = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / n as f32;
        if t < 0.4 {
            let l = 0.30 + 0.5 * t; // mid grey, neutral on BOTH sides
            sp.push([l, l, l]);
            tp.push([l, l, l]);
        } else if t < 0.6 {
            let l = 0.75 + 0.5 * (t - 0.4); // bright grey → re-hued vivid blue
            sp.push([l, l, l]);
            tp.push([0.3 * l, 0.5 * l, l]);
        } else {
            let l = 0.2 + 0.5 * t; // chromatic on both sides
            sp.push([l, 0.6 * l, 0.3 * l]);
            tp.push([l, 0.6 * l, 0.3 * l]);
        }
    }
    // Premise: this is the case the SHARE gate cannot see (0.6 vs 0.4 =
    // 1.5×, under 1.75×) — only the contamination measure convicts it.
    let s_share = sp.iter().filter(|p| is_neutralish(p)).count() as f32 / n as f32;
    let t_share = tp.iter().filter(|p| is_neutralish(p)).count() as f32 / n as f32;
    assert!(
        s_share.max(t_share) <= 1.75 * s_share.min(t_share),
        "premise broken: the share gate would already catch this ({s_share} vs {t_share})"
    );
    let c = neutral_gate_misprediction(&sp, &tp);
    assert!(
        c > 2.0 * NEUTRAL_MISPREDICTION_MAX,
        "the concentrated band must contaminate with margin: {c}"
    );
    let (s_cdf, t_cdf) = tone_cdf_pair(&sp, &tp);
    assert_eq!(s_cdf, luma_cdf(&sp), "source side must fall back to the full CDF");
    assert_eq!(t_cdf, luma_cdf(&tp), "target side must fall back to the full CDF");
}

/// R17 counterpart #1: benign one-sided inflation keeps the gate. The
/// source's extra neutrals (a uniform desaturation — the haze-pair
/// geometry) span the same luma ramp as the shared class, so the
/// evidence CDF barely moves even though a sixth of the frame is
/// neutral on the source side only.
#[test]
fn a_uniformly_inflated_neutral_class_keeps_the_gate() {
    let n = 64 * 64;
    let mut sp: Vec<[f32; 3]> = Vec::with_capacity(n);
    let mut tp: Vec<[f32; 3]> = Vec::with_capacity(n);
    for i in 0..n {
        let l = 0.2 + 0.6 * (i as f32 / n as f32);
        match i % 6 {
            0 => {
                sp.push([l, l, l]); // neutral in the source only…
                tp.push([l, 0.8 * l, 0.6 * l]); // …chromatic in the target
            }
            1..=3 => {
                sp.push([l, l, l]); // the shared class
                tp.push([l, l, l]);
            }
            _ => {
                sp.push([l, 0.6 * l, 0.3 * l]); // chromatic on both sides
                tp.push([l, 0.6 * l, 0.3 * l]);
            }
        }
    }
    let c = neutral_gate_misprediction(&sp, &tp);
    assert!(
        c < 0.5 * NEUTRAL_MISPREDICTION_MAX,
        "uniform inflation must stay clear of the ceiling: {c}"
    );
    let (s_cdf, _) = tone_cdf_pair(&sp, &tp);
    assert_ne!(s_cdf, luma_cdf(&sp), "the benign pair must stay neutral-gated");
}

/// R17 anchor on the LIVE haze pair: its neutral identification is
/// genuinely broken — the haze recipe's blue cast tints the clean
/// frame's dark greys OUT of the source-side class while the global
/// desaturation pulls colours IN, and the gated evidence map misses the
/// shared class by 0.13 mean luma (measured; worst at the dark ranks).
/// The misprediction gate must fall back to full-pixel CDFs — and the
/// fit, now solving on honest evidence, must land far below its
/// starting error (0.0892 -> 0.0229 measured; the GATED solve under the
/// R17 dense residual knots collapses to a do-no-harm reset, because
/// faithful sampling faithfully implements a broken map).
#[test]
fn the_haze_pairs_broken_identification_falls_back_and_still_fits() {
    let clean = synth();
    let mut haze = EditRecipe {
        exposure_ev: -0.3,
        contrast: -45.0,
        blacks: 40.0,
        saturation: -40.0,
        blue_curve: vec![
            CurvePoint { input: 0, output: 25 },
            CurvePoint { input: 128, output: 132 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    haze.clamp();
    let base = render::develop_preview(&clean, &haze);
    let pair = analysis_pair(&base, &clean);
    let (sp, tp) = (pixels_of(&pair.0), pixels_of(&pair.1));
    let m = neutral_gate_misprediction(&sp, &tp);
    assert!(
        m > NEUTRAL_MISPREDICTION_MAX,
        "premise broken: the haze pair's neutral evidence reads clean ({m:.4})"
    );
    let rep = fit_recipe(&base, &clean);
    // Measured 0.0892 -> 0.0229 (0.26×); 0.35× keeps real margin without
    // letting the win quietly rot.
    assert!(
        rep.err_after < 0.35 * rep.err_before,
        "the fallback solve must still close most of the gap ({:.4} -> {:.4})",
        rep.err_before,
        rep.err_after
    );
}

/// R17 counterpart #2: matched populations read ZERO contamination. The
/// canyon pair's neutral members (ramp + pale sky) are IDENTICAL on
/// both sides, and the returned CDFs stay neutral-gated (≠ the
/// full-pixel CDFs, which include the rocks).
#[test]
fn matched_neutral_members_keep_the_gate() {
    let pair = analysis_pair(&canyon(false), &canyon(true));
    let (sp, tp) = (pixels_of(&pair.0), pixels_of(&pair.1));
    let c = neutral_gate_misprediction(&sp, &tp);
    assert!(
        c < 0.5 * NEUTRAL_MISPREDICTION_MAX,
        "premise broken: the canyon pair's neutral members diverged ({c})"
    );
    let (s_cdf, _) = tone_cdf_pair(&sp, &tp);
    assert_ne!(s_cdf, luma_cdf(&sp), "the canyon pair must stay neutral-gated");
}

/// R17: residual-curve knots follow the LUT's OUTPUT spacing. On a steep
/// camera base the old fixed-x placement left a 38-u8 input gap right
/// across the band holding a real frame's tonal mass, and the curve's
/// piecewise-linear rendering chorded ~10/255 below the promised map
/// inside it. The base curve here is the P20 camera calibration
/// verbatim.
#[test]
fn residual_knots_stay_dense_in_the_curve_input_space() {
    let recipe = EditRecipe {
        base_curve: vec![
            [0.0, 0.0],
            [0.22091886, 0.25904202],
            [0.23851417, 0.29325512],
            [0.25317693, 0.32551318],
            [0.28152493, 0.38514173],
            [0.34115347, 0.49266863],
            [0.39687195, 0.6060606],
            [0.42033234, 0.6539589],
            [0.4848485, 0.74486804],
            [0.51808405, 0.77614856],
            [0.5474096, 0.8005865],
            [0.60117304, 0.8445748],
            [1.0, 1.0],
        ],
        ..EditRecipe::default()
    };
    let curve = residual_tone_curve(&recipe, &|x: f32| (0.9 * x + 0.02).clamp(0.0, 1.0));
    assert!(curve.len() >= 8, "a nontrivial map earns a dense curve: {} pts", curve.len());
    for w in curve.windows(2) {
        let gap = w[1].input as i32 - w[0].input as i32;
        assert!(
            gap <= 32,
            "knot gap {gap} u8 between inputs {} and {} — interpolation sag territory",
            w[0].input,
            w[1].input
        );
    }
}

/// The veto's discriminator is pinned on a real reconstruction and a
/// synthetic non-no-op cast. The haze pair's accepted correction rotates
/// pixels only INTO
/// the target's own hue families — measured foreign-share delta ≈ 0.000)
/// The real canyon reconstruction is upstream-refused and therefore
/// creates 0.000000 foreign share; the synthetic canyon cast below
/// paints the foreign population and must trigger the veto.
///
/// The end-to-end verdicts live in `hazy_to_clean_fit_stays_sane` and
/// hues ≥ 45° from everything the target contains). The end-to-end
/// `warm_rock_cast_must_not_violet_the_pale_sky`.
#[test]
fn foreign_hue_veto_measures_real_canyon_reconstruction() {
    // Canyon: rebuild stage 4's exact inputs (fit minus its cast curves →
    // `cur`; curves re-derived and rendered → `with`).
    let cur2: Vec<[f32; 3]> = (0..4096)
        .map(|i| if i % 2 == 0 { [0.65, 0.20, 0.12] } else { [0.72, 0.34, 0.10] })
        .collect();
    let tp2 = cur2.clone();
    let mut with2 = cur2.clone();
    for p in with2.iter_mut().take(512) {
        *p = [0.10, 0.22, 0.75];
    }
    let cf = foreign_hue_bins(&tp2).expect("target has chromatic mass");
    let created = foreign_share(&with2, &cf) - foreign_share(&cur2, &cf);
    assert!(
        cast_paints_foreign_hues(&cur2, &with2, &tp2),
        "veto must fire on a cast that creates a foreign blue population ({created:.4})"
    );
    assert!(created > 2.0 * VETO_CREATED_SHARE, "margin eroded: created {created:.4}");
    let evidence = evidence_model(&cur2, &tp2);
    let supported = evidence.source_weights.iter().take(512).filter(|&&w| w > 0.0).count();
    let supported_total = evidence.source_weights.iter().filter(|&&w| w > 0.0).count();
    assert!(
        cast_paints_foreign_hues_weighted(&cur2, &with2, &tp2, &evidence),
        "the production evidence-weighted veto must see the supported foreign population ({supported}/512 supported, {supported_total} total)"
    );

    // The real canyon reconstruction is upstream-refused: its rendered
    // candidate is unchanged and creates zero foreign share. This is a
    // diagnostic, not the veto acceptance itself.
    let src = canyon(false);
    let tgt = canyon(true);
    let (s2, t2) = analysis_pair(&src, &tgt);
    let tp2 = pixels_of(&t2);
    let mut pre = fit_recipe(&src, &tgt).recipe;
    pre.red_curve.clear();
    pre.green_curve.clear();
    pre.blue_curve.clear();
    let cur_real = pixels_of(&render::develop_preview(&s2, &pre));
    let mut with_real = pre.clone();
    with_real.red_curve = residual_channel_curve(&cur_real, &tp2, 0);
    with_real.green_curve = residual_channel_curve(&cur_real, &tp2, 1);
    with_real.blue_curve = residual_channel_curve(&cur_real, &tp2, 2);
    let with_real = pixels_of(&render::develop_preview(&s2, &with_real));
    let cf_real = foreign_hue_bins(&tp2).expect("canyon target has chromatic mass");
    let created_real = foreign_share(&with_real, &cf_real) - foreign_share(&cur_real, &cf_real);
    eprintln!("CANYON_VETO_REAL created={created_real:.6} with_eq_cur={}", with_real == cur_real);
    assert!(created_real <= VETO_CREATED_SHARE);
    assert!(!cast_paints_foreign_hues_weighted(
        &cur_real,
        &with_real,
        &tp2,
        &evidence_model(&cur_real, &tp2),
    ));

    // The inverse case is equally important: a foreign population created
    // only inside a structurally replaced cell is not evidence about a
    // global cast and must not veto a correction supported elsewhere.
    let (w, h) = (64usize, 64usize);
    let mut cur3 = Vec::with_capacity(w * h);
    let mut tgt3 = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let l = 0.30 + 0.35 * x as f32 / (w - 1) as f32;
            cur3.push([l, 0.55 * l, 0.25 * l]);
            let tl = if y < 8 {
                if (x / 2 + y / 2) % 2 == 0 { 0.28 } else { 0.72 }
            } else {
                l
            };
            tgt3.push([tl, 0.55 * tl, 0.25 * tl]);
        }
    }
    let mut with3 = cur3.clone();
    for p in with3.iter_mut().take(w * 8) {
        *p = [0.10, 0.22, 0.75];
    }
    assert!(cast_paints_foreign_hues(&cur3, &with3, &tgt3));
    let evidence3 = evidence_model(&cur3, &tgt3);
    assert!(
        !cast_paints_foreign_hues_weighted(&cur3, &with3, &tgt3, &evidence3),
        "unsupported invented pixels must not withhold the cast stage"
    );

    // Haze: under the marginal estimator this cast was refused — from
    // unpaired statistics, moving a source-only band is indistinguishable
    // from content mismatch. The PAIRED robust estimator changes the
    // epistemics: every blue-cast pixel has its own paired target, the
    // movement is hue-coherent with the global edit, and each moved pixel
    // is individually vouched — so the cast that empties the cast-invented
    // Red/Blue bands ships, WITH the vouched-passage disclosure beside
    // the withheld note (E-15: the veto that held for unvouched pixels
    // and the passage that was earned must both be readable). The real
    // canyon reconstruction above stays refused: its vanished population
    // is content, incoherent, unvouched.
    let clean = synth();
    let mut haze = EditRecipe {
        exposure_ev: -0.3,
        contrast: -45.0,
        blacks: 40.0,
        saturation: -40.0,
        blue_curve: vec![
            CurvePoint { input: 0, output: 25 },
            CurvePoint { input: 128, output: 132 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    haze.clamp();
    let base = render::develop_preview(&clean, &haze);
    let report = fit_recipe(&base, &clean);
    let rec = &report.recipe;
    assert!(
        !rec.blue_curve.is_empty(),
        "the vouched paired cast must un-cast the haze: {}",
        rec.rationale
    );
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.key == crate::rationale::keys::FIT_NOTE_VOUCHED_CONVERGENCE),
        "vouched passage through the one-sided bands must be disclosed: {}",
        rec.rationale
    );
    assert!(
        !report
            .notes
            .iter()
            .any(|note| note.key == crate::rationale::keys::FIT_NOTE_REHUE_BLOCKED),
        "a vouched coherent un-cast is not a re-hue refusal: {}",
        rec.rationale
    );
}

#[test]
fn warm_rock_cast_must_not_violet_the_pale_sky() {
    // Regression for the 2026-07-09 real-machine canyon failure: the target
    // warms the frame-dominant rocks and keeps the small pale sky blue. The
    // channel-CDF cast stage answers the rocks' demand with a global red
    // lift whose 5-knot interpolation drags the sky's red up too → violet
    // sky. The aggregate acceptance gate passed it because the rotated sky
    // is CROSS-BAND invisible to the hue term (mass lands in Purple/Magenta
    // — empty in the target — and drains out of Blue: the two-sided band
    // gate skips both). The pixel-aligned hue-damage veto must reject the
    // curves; saturation alone (hue-preserving) then matches the chroma.
    let src = canyon(false);
    let tgt = canyon(true);
    let rep = fit_recipe(&src, &tgt);
    // Render the fitted recipe and audit the sky region (rows y ≥ 108).
    let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
    let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
        for x in 0..192 {
            let p = out.get_pixel(x, y);
            let (r, g, b) =
                (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
            if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                continue; // desaturated sky pixels carry no hue verdict
            }
            let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
            sin += hue.sin();
            cos += hue.cos();
            n += 1.0;
        }
    }
    assert!(n > 0.0, "no chromatic sky pixels to audit — the fixture or a stage broke");
    {
        let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
        let d = (mean - 213.0 + 540.0).rem_euclid(360.0) - 180.0;
        assert!(
            d.abs() < 30.0,
            "sky hue drifted to {mean:.0}° (Δ{d:.0}°) — a violet/purple cast leaked through"
        );
    }
    // And rejecting the cast must not have made the overall fit worse.
    assert!(
        rep.err_after <= rep.err_before + 0.01,
        "fit made the look worse: {} -> {}",
        rep.err_before,
        rep.err_after
    );
}

/// Same geometry as [`canyon`], but the target regrades the WHOLE scene
/// warm: rocks red-lifted AND the pale-blue sky replaced by a pale gold
/// one ([0.92, 0.78, 0.58]: hue ≈ 35°, chroma ≈ 0.34, luma ≈ 0.80 vs the
/// source sky's 0.67 — brighter, like the real golden-hour target). The
/// destination hue is TARGET-NATIVE, so the foreign-hue veto stays silent
/// — this models the 2026-07-09 real-machine failure #2 (reimagine-5):
/// the hazy pale sky was rotated ~170° into the target's own orange.
fn canyon_gold_target() -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let l = 0.15 + 0.80 * x as f32 / (w - 1) as f32;
            let p = if y < 16 {
                [l, l, l]
            } else if y < 112 {
                [(0.85 * l + 0.18).min(1.0), 0.52 * l, 0.30 * l]
            } else {
                // PALE gold sky: bright golden-hour skies keep a HIGH blue
                // channel (b ≈ 0.6) — the demanded blue curve is a gentle
                // top-end dip (like the real pair's 255→188), not a global
                // crush that would wake the aggregate gate on rock damage.
                [0.92, 0.78, 0.58]
            };
            img.put_pixel(
                x,
                y,
                image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
            );
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// POLICY regression for real-machine failure #2 (2026-07-09, P21 ×
/// reimagine-5): when the target's statistics demand rotating a large
/// coherent chromatic region into a hue the target DOES populate (blue
/// hazy sky → vivid target-native orange, ~170°), both existing gates
/// pass — the foreign-hue veto by design (fit.rs "Known non-goal"), the
/// aggregate ratio because the frame-dominant demand is genuine. From
/// non-pixel-aligned statistics such a rotation is INDISTINGUISHABLE from
/// content mismatch, so the policy is to refuse it: hue-preserving stages
/// (tone + saturation) may chase the look, the cast curves may not
/// re-hue a region. Deliberate cost: a true whole-scene regrade (sky
/// genuinely gone gold) is not chased either — that expressiveness
/// belongs to the zoned fit, not to global curves.
#[test]
fn cast_must_not_rotate_the_sky_into_a_target_native_hue() {
    let src = canyon(false);
    let tgt = canyon_gold_target();
    let rep = fit_recipe(&src, &tgt);
    let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
    let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
        for x in 0..192 {
            let p = out.get_pixel(x, y);
            let (r, g, b) =
                (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
            if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                continue;
            }
            let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
            sin += hue.sin();
            cos += hue.cos();
            n += 1.0;
        }
    }
    assert!(n > 0.0, "no chromatic sky pixels to audit — the fixture or a stage broke");
    {
        let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
        let d = (mean - 213.0 + 540.0).rem_euclid(360.0) - 180.0;
        assert!(
            d.abs() < 30.0,
            "sky hue rotated to {mean:.0}° (Δ{d:.0}°) — a target-native re-hue leaked through"
        );
    }
    assert!(
        rep.recipe.red_curve.is_empty()
            && rep.recipe.green_curve.is_empty()
            && rep.recipe.blue_curve.is_empty(),
        "the whole-scene regrade's cast curves must be withheld"
    );
    assert!(
        rep.err_after <= rep.err_before + 0.01,
        "fit made the look worse: {} -> {}",
        rep.err_before,
        rep.err_after
    );
}

/// The REAL-pair geometry (2026-07-09 #2, P21 × reimagine-5), where
/// the rotation gate is the UNIQUE rejector — measured on this fixture:
/// stage-4 ratio 0.450 (aggregate gate PASSES: crushing blue genuinely
/// fixes the channel means frame-wide), foreign-hue veto false (the
/// destination orange is target-native), rotation gate true (the hazy
/// pale-blue sky re-hues ~170°). Unwiring the rotation gate from
/// `fit_cast_stage` flips this test (curves accepted → orange sky),
/// which the synthetic `canyon` pairs cannot detect: there the re-hue
/// also damages the aggregate, so the ratio gate rejects redundantly.
fn hazy_canyon_source() -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let l = 0.25 + 0.60 * x as f32 / (w - 1) as f32;
            let p = if y < 16 {
                [l, l, l]
            } else if y < 112 {
                // hazy desaturated rocks: warm but muted
                [0.95 * l + 0.03, 0.88 * l + 0.03, 0.80 * l + 0.04]
            } else {
                [0.60, 0.63, 0.67] // hazy pale-blue sky, hue ≈ 214°
            };
            img.put_pixel(
                x,
                y,
                image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
            );
        }
    }
    DynamicImage::ImageRgb8(img)
}

fn vivid_warm_target() -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let l = 0.25 + 0.60 * x as f32 / (w - 1) as f32;
            let p = if y < 16 {
                [l, l, l]
            } else if y < 112 {
                [(1.05 * l + 0.15).min(1.0), 0.55 * l, 0.30 * l] // vivid warm rocks
            } else {
                [0.92, 0.72, 0.48] // vivid gold sky
            };
            img.put_pixel(
                x,
                y,
                image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
            );
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// The Cornwall shape, distilled to the one property the canyon family
/// cannot express: a large SINGLE-HUED sky whose LUMINANCE ramps from
/// zenith to horizon, over warm ground the target lifts further. The
/// canyon skies are one flat colour, so their curves cannot sort them —
/// which is exactly why the pixel-aligned gates were enough there and
/// were not enough on a photograph.
///
/// Measured on the real pair before the fixture was drawn (2026-09-01):
/// the Cornwall sky's hue holds within 1.6° across luminance octiles in
/// the source, the target and the no-cast fit, and the admitted curves
/// fan it to 33.1° in the delivered render — 226.8° in the dark half,
/// 193.8° in the bright clouds.
///
/// 2026-10-22: the target sky moved from [0.66, 0.80] to [0.62, 0.78]. Under
/// the ungated paired tone arm the old pair's weighted cast fanned only
/// +12.1° end to end and was ADMITTED, so nothing here projected; at
/// [0.62, 0.78] the candidate reads 36.3°, the stage convicts at 20.7° and
/// projects (t = 0.429, +5.7°) at both strength stops.
fn coast(warm: bool) -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let p = if y < 64 {
                // Sky: hue ≈ 214° at every level, luminance 0.22 → 0.90.
                let level = 0.22 + 0.68 * y as f32 / 63.0;
                if warm {
                    [0.62 * level, 0.78 * level, 1.00 * level]
                } else {
                    [0.74 * level, 0.85 * level, 1.00 * level]
                }
            } else {
                // Ground: a warm ramp the target red-lifts, exactly the
                // demand the channel-CDF answers with a global cast.
                let level = 0.15 + 0.70 * x as f32 / (w - 1) as f32;
                if warm {
                    [(0.85 * level + 0.12).min(1.0), 0.52 * level, 0.30 * level]
                } else {
                    [0.85 * level, 0.52 * level, 0.30 * level]
                }
            };
            img.put_pixel(
                x,
                y,
                image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
            );
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// Widest circular gap between the mean hues of a band's luma octiles —
/// the coherence a viewer reads as "the sky is one colour", measured on
/// a DELIVERED render rather than on the census, so the end-to-end claim
/// is about the picture and not about the gate's own arithmetic.
fn hue_spread_across_luma(img: &DynamicImage, rows: std::ops::Range<u32>) -> f64 {
    const OCTILES: usize = 8;
    let rgb = img.to_rgb8();
    let mut acc = [(0.0f64, 0.0f64, 0.0f64); OCTILES];
    for y in rows {
        for x in 0..rgb.width() {
            let p = rgb.get_pixel(x, y);
            let (r, g, b) =
                (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
            if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                continue; // no hue verdict from a desaturated pixel
            }
            let octile =
                ((((r + g + b) / 3.0) * OCTILES as f32) as usize).min(OCTILES - 1);
            let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
            acc[octile].0 += hue.sin();
            acc[octile].1 += hue.cos();
            acc[octile].2 += 1.0;
        }
    }
    let total: f64 = acc.iter().map(|a| a.2).sum();
    assert!(total > 0.0, "the audited band must contain chromatic pixels");
    let means: Vec<f64> = acc
        .iter()
        .filter(|a| a.2 >= total * 0.05)
        .map(|a| a.0.atan2(a.1).to_degrees().rem_euclid(360.0))
        .collect();
    let mut worst = 0.0f64;
    for (i, a) in means.iter().enumerate() {
        for b in &means[i + 1..] {
            worst = worst.max(((b - a + 540.0).rem_euclid(360.0) - 180.0).abs());
        }
    }
    worst
}

/// Rebuild a candidate of the shape stage 4 judges: the fitted recipe
/// minus its channel curves (`cur`) and the curves re-derived on that
/// state (`with`). Shared by the foreign-hue and rotation pin tests so
/// they read one census.
///
/// NOT the stage's own candidate, and the difference is the estimator:
/// this derives the curves with [`residual_channel_curve`], the solve
/// with [`residual_channel_curve_weighted`] over the evidence ×
/// robust-pairing weights. On a synthetic fixture whose pairing is exact
/// the weights are flat and the two agree, which is why the pin tests
/// below may use it; on a real photograph they do not. Measured on the
/// `p40` calibration pair (2026-09-21): this rebuild reads 13.5° of added
/// hue fan where the gate read 11.0°, and 15.4° against 11.9° after the
/// develop window moved onto the DefaultCrop rectangle. A test whose
/// subject is the GATE'S verdict therefore reads the gate's own published
/// number — see `the_fan_gate_costs_a_real_two_temperature_pair_nothing`.
struct CastCandidate {
    /// The stage's input render: the fitted recipe minus its curves.
    cur: Vec<[f32; 3]>,
    /// The same render with the curves re-derived on `cur`.
    with_px: Vec<[f32; 3]>,
    /// The target's analysis pixels, in the source's geometry.
    tp: Vec<[f32; 3]>,
    /// The candidate recipe, so a test can assert a cast was demanded.
    with: EditRecipe,
}

fn cast_stage_candidate(src: &DynamicImage, tgt: &DynamicImage) -> CastCandidate {
    cast_stage_candidate_from(src, tgt, &EditRecipe::default())
}

fn cast_stage_candidate_from(
    src: &DynamicImage,
    tgt: &DynamicImage,
    base: &EditRecipe,
) -> CastCandidate {
    let (s, t) = analysis_pair(src, tgt);
    let tp = pixels_of(&t);
    let mut pre = fit_recipe_from(src, tgt, base).recipe;
    pre.red_curve = Vec::new();
    pre.green_curve = Vec::new();
    pre.blue_curve = Vec::new();
    let cur = pixels_of(&render::develop_preview(&s, &pre));
    let mut with = pre.clone();
    with.red_curve = residual_channel_curve(&cur, &tp, 0);
    with.green_curve = residual_channel_curve(&cur, &tp, 1);
    with.blue_curve = residual_channel_curve(&cur, &tp, 2);
    let with_px = pixels_of(&render::develop_preview(&s, &with));
    CastCandidate { cur, with_px, tp, with }
}

/// The deliberate cost the fan gate names, drawn so it can be MEASURED:
/// a target that genuinely lights one region at two colour temperatures.
///
/// Same geometry as [`coast`], ground untouched, but the sky's hue is a
/// function of its own brightness — the dark end cooled and the bright
/// end warmed by 25° each, so the target itself carries a 50° hue fan
/// across luminance. The three channel curves can express exactly that
/// (it is a per-level, per-channel move), which is why the fit reaches
/// for it; and every milder version of those curves reproduces less of
/// it, so the projection has nothing to trade. This is the pair where
/// the rescue must give up and the refusal stand.
fn two_temperature_coast() -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let p = if y < 64 {
                let level = 0.22 + 0.68 * y as f32 / 63.0;
                // Green rides the level: 0.958 at the dark end (hue
                // ≈ 190°) to 0.742 at the bright end (≈ 240°), so the
                // target's own sky carries a 50° fan across luminance.
                let green = 0.9584 - 0.2167 * (y as f32 / 63.0);
                [0.74 * level, green * level, 1.00 * level]
            } else {
                let level = 0.15 + 0.70 * x as f32 / (w - 1) as f32;
                [0.85 * level, 0.52 * level, 0.30 * level]
            };
            img.put_pixel(
                x,
                y,
                image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
            );
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// The projection's readings off a finished report, by note key.
fn projection_arg(report: &FitReport, key: &str, arg: &str) -> Option<f32> {
    report
        .notes
        .iter()
        .find(|n| n.key == key)
        .and_then(|n| n.args.iter().find(|(k, _)| *k == arg))
        .and_then(|(_, v)| v.parse::<f32>().ok())
}

/// v1.2.3 — the fourth gate, and the failure class that shipped in
/// v1.2.2 on the Cornwall showcase pair. Three independent monotone
/// channel maps sort a single-hued region into a hue FAN by luminance;
/// no pixel travels 75°, every destination is target-native, the
/// region's mean hue barely moves (218.3° → 217.6° on the real pair) —
/// so all three earlier gates read clean and the tint shipped.
///
/// CAST-2 (user ruling 2026-09-01): the gate still convicts, but the
/// stage no longer throws the cast away. It shrinks the three curves
/// along the projection path until the fan clears `FAN_PROJECT_DEG` and
/// ships THAT. So this test's subject is the conviction and the rescue
/// together: the fitted curves must never reach the frame, and what does
/// reach it must read inside the target.
#[test]
fn cast_curves_that_fan_a_coherent_sky_are_shrunk_not_shipped() {
    let src = coast(false);
    let tgt = coast(true);
    let c = cast_stage_candidate(&src, &tgt);
    let (cur, with_px, tp) = (c.cur, c.with_px, c.tp);
    assert!(!c.with.red_curve.is_empty(), "premise broken: no cast demanded");
    let evidence = evidence_model(&cur, &tp);

    // PREMISES — the three pre-v1.2.3 gates are silent here, which is
    // the whole reason this gate exists. If a fixture drift wakes one of
    // them the assertions below stop testing the fan gate, so they fail
    // with their numbers rather than passing for the wrong reason.
    let rehued = rehued_share_weighted(&cur, &with_px, &evidence);
    assert!(
        rehued < 0.1 * ROT_SHARE,
        "premise broken: the rotation budget sees this ({rehued:.4})"
    );
    assert!(
        !cast_paints_foreign_hues_weighted(&cur, &with_px, &tp, &evidence),
        "premise broken: the foreign-hue veto fires — the destinations should be target-native"
    );

    // THE reading, and its margin over the threshold.
    let (share, fan, delivered_by_census) = hue_fan_weighted(&cur, &with_px, &evidence)
        .expect("the sky is a region-sized hue class");
    assert!(share > 0.5, "premise broken: the sky class is not region-sized ({share:.3})");
    // 2026-10-22: 2.0×, was 2.5×. The re-drawn coast (see [`coast`]) reads
    // 36.3° here, the old one 32.2° under the ungated tone arm; none of the
    // twelve variants swept that still project end to end cleared 37.5°.
    assert!(fan > 2.0 * FAN_DEG, "margin eroded: fan {fan:.1}° against {FAN_DEG}°");

    // END TO END: the FITTED curves never ship — what ships is the
    // projection, and the note says so with the numbers behind it.
    let rep = fit_recipe(&src, &tgt);
    assert!(
        rep.recipe.red_curve != c.with.red_curve
            || rep.recipe.green_curve != c.with.green_curve
            || rep.recipe.blue_curve != c.with.blue_curve,
        "the fanning curves must not ship as fitted: {}",
        rep.recipe.rationale
    );
    assert!(
        !rep.recipe.red_curve.is_empty(),
        "…and the projection must have found a milder cast to ship: {}",
        rep.recipe.rationale
    );
    let arg = |key: &str, name: &str| {
        projection_arg(&rep, key, name)
            .unwrap_or_else(|| panic!("the note carries {name}: {}", rep.recipe.rationale))
    };
    use crate::rationale::keys;
    assert!(arg(keys::FIT_NOTE_CAST_PROJECTED, "fan_before") >= FAN_DEG,
        "the disclosed fan is the one that convicted");
    assert!(arg(keys::FIT_NOTE_CAST_PROJECTED, "share") > 0.5,
        "the disclosed share is the class that fanned");
    assert_eq!(arg(keys::FIT_NOTE_CAST_PROJECTED, "limit"), FAN_DEG,
        "the note names the limit it measured against");
    let t = arg(keys::FIT_NOTE_CAST_PROJECTED, "t");
    assert!((0.0..1.0).contains(&t), "the shipped cast is milder than the fitted one (t {t})");
    assert!(
        arg(keys::FIT_NOTE_CAST_PROJECTED_FAN, "fan_after") <= FAN_PROJECT_DEG,
        "the projection must reach its own target, not just the refusal line"
    );
    // …and it discloses at least what an ADMISSION does: the re-hued
    // share on its own head note, and the admission's own foreign-hue
    // clause beside it (measured or not-measurable, never a fabricated
    // 0.000). These are invented curves, so the two pixel-aligned
    // readings matter here more than they do on a measured cast.
    assert!(
        arg(keys::FIT_NOTE_CAST_PROJECTED, "rehued") < ROT_SHARE,
        "the projected cast's re-hued share is disclosed, and it passed"
    );
    assert!(
        rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED_FOREIGN
            || n.key == keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA),
        "the projection must carry the foreign-hue clause too: {}",
        rep.recipe.rationale
    );
    // One sentence per outcome: a projected cast is not also an admitted
    // one. (The exclusivity itself is pinned in
    // `a_projected_cast_is_never_also_disclosed_as_an_admitted_one`.)
    assert!(
        !rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED),
        "a projected cast must not also claim admission: {}",
        rep.recipe.rationale
    );

    // …and the DELIVERED sky: the picture claim, not the census's own
    // arithmetic. TWO bars, and the first is the one this test carried
    // BEFORE the projection existed — `FAN_DEG`, the refusal line: a
    // rescued cast has to leave the delivered frame no worse than the
    // widest fan the gate would have let through unprojected. It is
    // deliberately NOT widened to the projection's own target plus one
    // class width (7.5 + 15 = 22.5°, which is 50% looser than FAN_DEG
    // itself); the fixture does not need the room. Measured 2026-09-02 on
    // this tree: 14.58° delivered, 0.42° of margin under the bar, against
    // the 42.4° the fitted curves would have delivered; 5.98° on the
    // re-drawn coast (2026-10-22).
    let delivered = render::develop_preview(&src, &rep.recipe);
    let spread = hue_spread_across_luma(&delivered, 0..64);
    assert!(
        spread < FAN_DEG as f64,
        "the delivered sky fanned {spread:.1}° across luminance, against the \
             {FAN_DEG}° refusal line this bar is (measured 5.98° on 2026-10-22)"
    );
    assert!(
        spread < 0.5 * delivered_by_census as f64,
        "the projection must be most of the way back from the fitted fan \
             ({spread:.1}° delivered against the fitted census's {delivered_by_census:.1}°)"
    );
    assert!(
        rep.err_after <= rep.err_before + 0.01,
        "fit made the look worse: {} -> {}",
        rep.err_before,
        rep.err_after
    );
}

/// The other half of the ruling: when the projection cannot pay, the
/// refusal stands — and now says the rescue was tried.
///
/// [`two_temperature_coast`] is the case the fan gate deliberately
/// refuses and the release notes call unmeasured: a target whose sky
/// really is two colour temperatures. Every point on the projection path
/// reproduces a proportional share of that fan, so there is no `t` that
/// both clears `FAN_PROJECT_DEG` and buys more than the fit's own
/// quantisation — and the stage keeps its hands off the frame instead of
/// shipping a cast that is neither the target's look nor honest about it.
#[test]
fn a_projection_that_cannot_clear_the_target_leaves_the_refusal_standing() {
    let src = coast(false);
    let tgt = two_temperature_coast();
    let c = cast_stage_candidate(&src, &tgt);
    let evidence = evidence_model(&c.cur, &c.tp);
    assert!(!c.with.red_curve.is_empty(), "premise broken: no cast demanded");
    let (_, fan, _) = hue_fan_weighted(&c.cur, &c.with_px, &evidence)
        .expect("premise broken: the two-temperature sky is not a region-sized class");
    assert!(fan >= FAN_DEG, "premise broken: the fitted cast does not fan ({fan:.1}°)");

    let rep = fit_recipe(&src, &tgt);
    assert!(
        rep.recipe.red_curve.is_empty()
            && rep.recipe.green_curve.is_empty()
            && rep.recipe.blue_curve.is_empty(),
        "the cast must be withheld when the projection cannot clear: {}",
        rep.recipe.rationale
    );
    use crate::rationale::keys;
    assert!(
        rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_HUE_FANNED),
        "the refusal must still disclose: {}",
        rep.recipe.rationale
    );
    assert!(
        !rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_PROJECTED),
        "a refused cast must not also claim a projection: {}",
        rep.recipe.rationale
    );
    // The refusal sentence now owes the reader the fact that the cheaper
    // answer was tried; a refusal that does not say so invites the ask.
    let rendered = crate::rationale::render_one(
        rep.notes.iter().find(|n| n.key == keys::FIT_NOTE_CAST_HUE_FANNED).unwrap(),
    );
    assert!(
        rendered.contains("Shrinking them"),
        "the refusal must say the projection was tried: {rendered}"
    );
}

/// The search bisects on `t` for the LARGEST value that clears, which is
/// only the right thing to look for if the fan GROWS with `t`. Measured
/// rather than assumed: eleven points on the path, on the fixture whose
/// fan is the reason the gate exists.
#[test]
fn the_projected_fan_grows_with_t() {
    let src = coast(false);
    let tgt = coast(true);
    let (s_img, _) = analysis_pair(&src, &tgt);
    let c = cast_stage_candidate(&src, &tgt);
    let evidence = evidence_model(&c.cur, &c.tp);
    let fitted = [
        c.with.red_curve.as_slice(),
        c.with.green_curve.as_slice(),
        c.with.blue_curve.as_slice(),
    ];
    let mut base = c.with.clone();
    let mut seen: Vec<(f32, f32)> = Vec::new();
    for step in 0..=10 {
        let t = step as f32 / 10.0;
        let [red, green, blue] = projected_cast_curves(fitted, t);
        base.red_curve = red;
        base.green_curve = green;
        base.blue_curve = blue;
        let px = pixels_of(&render::develop_preview(&s_img, &base));
        let fan = hue_fan_weighted(&c.cur, &px, &evidence)
            .map(|(_, added, _)| added)
            .unwrap_or(0.0);
        seen.push((t, fan));
    }
    for pair in seen.windows(2) {
        let ((t0, f0), (t1, f1)) = (pair[0], pair[1]);
        assert!(
            f1 >= f0 - 0.5,
            "the fan must not shrink as the cast is restored: \
                 t {t0:.1} → {f0:.1}°, t {t1:.1} → {f1:.1}° (whole ladder {seen:?})"
        );
    }
    let (_, first) = seen[0];
    let (_, last) = seen[seen.len() - 1];
    assert!(
        last > first + 2.0 * FAN_DEG,
        "premise broken: the path does not span the fan ({first:.1}° → {last:.1}°)"
    );
}

/// The bottom half of the path carries NO chromatic difference: at
/// `t ≤ 0.5` the three channels hold one and the same curve, and at
/// `t = 0` that curve is the identity — which is why the search always
/// has a well-defined floor and why a projection can never be worse than
/// the refusal it replaces.
#[test]
fn the_bottom_of_the_projection_path_is_one_curve_then_none() {
    let red = vec![
        CurvePoint { input: 0, output: 23 },
        CurvePoint { input: 128, output: 115 },
        CurvePoint { input: 255, output: 179 },
    ];
    let green = vec![
        CurvePoint { input: 0, output: 56 },
        CurvePoint { input: 128, output: 105 },
        CurvePoint { input: 255, output: 189 },
    ];
    let blue = vec![
        CurvePoint { input: 0, output: 50 },
        CurvePoint { input: 128, output: 107 },
        CurvePoint { input: 255, output: 209 },
    ];
    let fitted = [red.as_slice(), green.as_slice(), blue.as_slice()];

    let [r1, g1, b1] = projected_cast_curves(fitted, 1.0);
    assert_eq!((r1, g1, b1), (red.clone(), green.clone(), blue.clone()),
        "t = 1 must reproduce the fitted curves byte for byte");

    let [r0, g0, b0] = projected_cast_curves(fitted, 0.0);
    for c in [&r0, &g0, &b0] {
        assert!(c.iter().all(|p| p.input == p.output), "t = 0 must be the identity: {c:?}");
    }
    assert!(cast_curves_are_identity(&[r0, g0, b0]));

    // …and everywhere in the bottom half the three curves are EQUAL, so
    // no chromatic difference between the channels survives at all.
    for step in 0..=5 {
        let t = step as f32 / 10.0;
        let [r, g, b] = projected_cast_curves(fitted, t);
        assert_eq!(r, g, "t = {t}: red and green must be one shared curve");
        assert_eq!(g, b, "t = {t}: green and blue must be one shared curve");
    }
    // A channel the fit left EMPTY comes back EMPTY wherever the
    // projection leaves it on the identity — `t = 1` above all, where the
    // whole promise is to reproduce the fitted curves byte for byte.
    // Without this the projection handed an unfitted channel back as five
    // dead knots, and `cast_curves_are_identity` could not catch it
    // because it only bails when ALL THREE channels are dead.
    let empty: Vec<CurvePoint> = Vec::new();
    let mixed = [red.as_slice(), empty.as_slice(), blue.as_slice()];
    let [mr, mg, mb] = projected_cast_curves(mixed, 1.0);
    assert_eq!(
        (mr, mg, mb),
        (red.clone(), empty.clone(), blue.clone()),
        "t = 1 must reproduce an unfitted channel as unfitted, not as an identity curve"
    );
    // …and a channel the projection MOVES off the identity is emitted, so
    // emptying is a statement about the curve and not about the channel.
    let moved = projected_cast_curves(mixed, 0.75);
    assert!(
        !moved[1].is_empty(),
        "the middle channel is off the identity at t = 0.75 and must ship: {:?}",
        moved[1]
    );
    assert!(!cast_curves_are_identity(&moved));

    // At the midpoint that shared curve is the per-knot MEAN of the three
    // — the "common shape" the projection is named for.
    let [mid, _, _] = projected_cast_curves(fitted, 0.5);
    assert_eq!(
        mid,
        vec![
            CurvePoint { input: 0, output: 43 },
            CurvePoint { input: 128, output: 109 },
            CurvePoint { input: 255, output: 192 },
        ],
        "t = 0.5 must be the per-knot mean of the three fitted outputs"
    );
}

/// A rescore re-renders and re-scores; it does not re-run the gates, so
/// every gate fact has to ride its own note back. The projection's do
/// too — otherwise a rescored recipe would keep the shrunk curves and
/// lose the only sentence that says why they are shrunk.
#[test]
fn a_rescored_projection_carries_its_own_readings_back() {
    use crate::rationale::keys;
    let src = coast(false);
    let tgt = coast(true);
    let solved = fit_recipe(&src, &tgt);
    let head = |r: &FitReport, arg: &str| projection_arg(r, keys::FIT_NOTE_CAST_PROJECTED, arg);
    assert!(
        head(&solved, "t").is_some(),
        "premise broken: this pair no longer projects: {}",
        solved.recipe.rationale
    );
    let rescored = rescore_report(
        &src,
        &tgt,
        &solved.recipe,
        &EditRecipe::default(),
        solved.err_before,
        &solved.notes,
    );
    for arg in ["share", "fan_before", "t", "ratio", "bound", "rehued"] {
        assert_eq!(
            head(&rescored, arg),
            head(&solved, arg),
            "the rescore must carry {arg} back, not re-invent or drop it"
        );
    }
    assert_eq!(
        projection_arg(&rescored, keys::FIT_NOTE_CAST_PROJECTED_FAN, "fan_after"),
        projection_arg(&solved, keys::FIT_NOTE_CAST_PROJECTED_FAN, "fan_after"),
        "…and the fan clause with it"
    );
    assert_eq!(
        projection_arg(&rescored, keys::FIT_NOTE_CAST_ADMITTED_FOREIGN, "foreign"),
        projection_arg(&solved, keys::FIT_NOTE_CAST_ADMITTED_FOREIGN, "foreign"),
        "…and the foreign clause the projection borrows from the admission"
    );
    // …and it does not ALSO claim admission on the way back.
    assert!(
        !rescored.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED),
        "a rescored projection must not become an admission: {}",
        rescored.recipe.rationale
    );
}

/// A rescue has to be worth having. The four gates decide whether a cast
/// the fit MEASURED may ship; whether a milder one the fit INVENTED is
/// worth shipping is the projection's own question, and the answer is
/// the fit's own quantisation budget: below [`FIT_QUANT`] of absolute
/// look error a difference is not a difference (the terminal do-no-harm
/// check says so with the same constant), and the stage's standing
/// doctrine is that marginal gain does not earn regional risk.
///
/// Driven through a synthetic gate so the bar is exercised alone, with
/// every other reading held clean.
#[test]
fn a_projection_worth_less_than_the_fits_own_quantisation_is_not_shipped() {
    let img = synth();
    let red = vec![
        CurvePoint { input: 0, output: 23 },
        CurvePoint { input: 255, output: 179 },
    ];
    let green = vec![
        CurvePoint { input: 0, output: 56 },
        CurvePoint { input: 255, output: 189 },
    ];
    let blue = vec![
        CurvePoint { input: 0, output: 50 },
        CurvePoint { input: 255, output: 209 },
    ];
    let fitted = [red.as_slice(), green.as_slice(), blue.as_slice()];
    let clean_gate = |ratio: f32| {
        move |_: &[[f32; 3]]| CastOutcome {
            readings: Some(CastReadings {
                ratio,
                bound: CAST_ACCEPT_RATIO,
                foreign: Some(0.0),
                rehued: 0.0,
                fan: Some(0.0),
            }),
            ..CastOutcome::default()
        }
    };
    // err_without = 0.05. A ratio of 0.99 buys 0.0005 of look error —
    // under FIT_QUANT (0.0018), so nothing ships…
    let mut recipe = EditRecipe::default();
    assert!(
        search_cast_projection(&img, &mut recipe, fitted, 0.05, clean_gate(0.99)).is_none(),
        "a rescue worth 0.0005 of look error must not ship"
    );
    // …and 0.90 buys 0.005, which does.
    let mut recipe = EditRecipe::default();
    let won = search_cast_projection(&img, &mut recipe, fitted, 0.05, clean_gate(0.90));
    assert!(won.is_some(), "a rescue worth 0.005 of look error must ship");
    assert!(
        !recipe.red_curve.is_empty(),
        "…and the recipe must come back carrying it"
    );
}

/// The 4a' and 4b loops call `fit_cast_stage` repeatedly, so the rescue
/// has to be deterministic AND idempotent under re-fits or the loops
/// could not converge. Both halves, on the fixture that projects.
#[test]
fn the_projection_is_deterministic_and_idempotent() {
    let once = fit_recipe(&coast(false), &coast(true)).recipe;
    let twice = fit_recipe(&coast(false), &coast(true)).recipe;
    assert_eq!(once.red_curve, twice.red_curve, "the same pair must fit the same cast");
    assert_eq!(once.green_curve, twice.green_curve);
    assert_eq!(once.blue_curve, twice.blue_curve);
    assert_eq!(once.rationale, twice.rationale, "…and disclose it the same way");

    // Idempotence: a curve set already ON the path is its own `t = 1`.
    let c = cast_stage_candidate(&coast(false), &coast(true));
    let fitted = [
        c.with.red_curve.as_slice(),
        c.with.green_curve.as_slice(),
        c.with.blue_curve.as_slice(),
    ];
    let projected = projected_cast_curves(fitted, 0.37);
    let again = projected_cast_curves(
        [&projected[0], &projected[1], &projected[2]],
        1.0,
    );
    assert_eq!(again, projected, "projecting a projected cast at t = 1 must change nothing");
}

/// One sentence per outcome. A projected cast writes the projection's
/// notes and NOT the admission's, and the exclusivity is structural (the
/// two `SolveFacts` fields), not a convention at the two push sites.
#[test]
fn a_projected_cast_is_never_also_disclosed_as_an_admitted_one() {
    use crate::rationale::keys;
    for (name, src, tgt) in [
        ("projected", coast(false), coast(true)),
        ("refused", coast(false), two_temperature_coast()),
        ("admitted", haze_pair().0, haze_pair().1),
    ] {
        let rep = fit_recipe(&src, &tgt);
        let has = |key: &str| rep.notes.iter().any(|n| n.key == key);
        assert!(
            !(has(keys::FIT_NOTE_CAST_PROJECTED) && has(keys::FIT_NOTE_CAST_ADMITTED)),
            "{name}: one cast, two accounts of itself: {}",
            rep.recipe.rationale
        );
        assert!(
            !(has(keys::FIT_NOTE_CAST_PROJECTED) && has(keys::FIT_NOTE_CAST_HUE_FANNED)),
            "{name}: projected and refused at once: {}",
            rep.recipe.rationale
        );
        // …and the projection's two notes travel together.
        assert_eq!(
            has(keys::FIT_NOTE_CAST_PROJECTED),
            has(keys::FIT_NOTE_CAST_PROJECTED_FAN)
                || has(keys::FIT_NOTE_CAST_PROJECTED_FAN_NA),
            "{name}: the projection's fan clause went missing: {}",
            rep.recipe.rationale
        );
        // One head note per OUTCOME, whichever head it is — the strength
        // budget's admission sentence counts. It cannot fire on a
        // projected cast at the shipped calibration (the gain bar forces
        // ratio < 1, this needs ratio > CAST_ACCEPT_RATIO), so this pins
        // the guard rather than a behaviour anyone can reach today.
        assert!(
            !(has(keys::FIT_NOTE_CAST_PROJECTED)
                && has(keys::FIT_NOTE_CAST_ADMITTED_BY_STRENGTH)),
            "{name}: a projected cast also claimed admission by strength: {}",
            rep.recipe.rationale
        );
        // The foreign-hue clause is SHARED between the two heads — one
        // sentence, one translation — so on a projected pair it must be
        // PRESENT while the admission HEAD stays absent. A shared clause
        // is not a shared verdict.
        if has(keys::FIT_NOTE_CAST_PROJECTED) {
            assert!(
                has(keys::FIT_NOTE_CAST_ADMITTED_FOREIGN)
                    || has(keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA),
                "{name}: the projection dropped the foreign-hue clause: {}",
                rep.recipe.rationale
            );
            assert!(
                !has(keys::FIT_NOTE_CAST_ADMITTED),
                "{name}: …and the admission HEAD must still be absent: {}",
                rep.recipe.rationale
            );
        }
    }
}

/// The projection does not step around the strength budget: the milder
/// candidate is judged against the bound the path was GIVEN, exactly as
/// the fitted cast would have been, and the note quotes that bound.
#[test]
fn a_projected_cast_is_judged_by_the_strength_budgets_bound() {
    use crate::rationale::keys;
    // End to end at the shipped default: the bound in the note is the
    // budget's, not the `CAST_ACCEPT_RATIO` anchor by coincidence.
    let rep = fit_recipe(&coast(false), &coast(true));
    let bound = projection_arg(&rep, keys::FIT_NOTE_CAST_PROJECTED, "bound")
        .unwrap_or_else(|| panic!("premise broken: no projection here: {}", rep.recipe.rationale));
    assert_eq!(
        bound,
        FitBudget::for_strength(crate::recipe::GradeStrength::default()).cast_ratio,
        "the projection must quote the bound the strength budget set"
    );

    // …and at strength 1.0, END TO END, the same pair quotes the WIDENED
    // bound. That is the discriminating arm: the two stops differ ONLY in
    // the budget, so the assertion cannot pass by coincidence the way a
    // single-stop reading against `CAST_ACCEPT_RATIO` can (at the default
    // the two numbers are both 2.0). The budget threads from the panel
    // dial through `full_cast_accept_ratio` into the gate that judges each
    // projected candidate. Measured 2026-09-02: this pair projects at both
    // stops — t = 0.659 at the default, t = 0.399 at 1.0.
    let widened = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0)).cast_ratio;
    assert_ne!(widened, bound, "premise broken: the two strength stops share a bound");
    let full = fit_recipe_from_with(
        &coast(false),
        &coast(true),
        &EditRecipe::default(),
        FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None },
    );
    assert_eq!(
        projection_arg(&full, keys::FIT_NOTE_CAST_PROJECTED, "bound"),
        Some(widened),
        "the projection at full strength must quote the widened bound: {}",
        full.recipe.rationale
    );
}

/// v1.2.3 fix-up — the PRECEDENCE, as the design states it: the fan gate
/// must be the ONLY gate that convicted for a cast to be rescued.
///
/// The arm used to fire on `!rehue_blocked && hue_fanned.is_some()`, which
/// also let a pair the RATIO gate had convicted be rescued — and
/// `FIT_NOTE_CAST_PROJECTED` names the fan and only the fan, so that pair
/// would have shipped a sentence omitting one of the two verdicts its
/// curves had to survive. Driven through synthetic outcomes so each
/// combination is exercised exactly once, without needing a fixture that
/// happens to trip two gates at the same time.
#[test]
fn a_cast_the_ratio_gate_convicts_is_not_rescued_by_the_projection() {
    use crate::rationale::keys;
    let readings = CastReadings {
        ratio: 3.4,
        bound: CAST_ACCEPT_RATIO,
        foreign: Some(0.0),
        rehued: 0.0,
        fan: Some(38.0),
    };
    let fan_only = CastOutcome {
        hue_fanned: Some((0.917, 38.0)),
        readings: Some(readings),
        ..CastOutcome::default()
    };
    assert_eq!(
        fan_only.earns_projection(),
        Some((0.917, 38.0)),
        "a fan-ONLY conviction is the case the projection exists for"
    );

    // Ratio AND fan: no projection, and the refusal keeps the note it
    // already had — the fan note, which wins a double rejection because
    // it is the more specific statement.
    let both = CastOutcome { ratio_rejected: true, ..fan_only };
    assert_eq!(
        both.earns_projection(),
        None,
        "a cast the ratio gate convicted must not be rescued"
    );
    assert!(both.refused(), "…it stays refused");
    assert_eq!(
        both.note().map(|n| n.key),
        Some(keys::FIT_NOTE_CAST_HUE_FANNED),
        "…with the note it already had"
    );

    // A pixel-aligned veto: no projection either, and that is the whole
    // of the viaduct pair's byte-identity.
    let vetoed = CastOutcome { rehue_blocked: true, ..fan_only };
    assert_eq!(vetoed.earns_projection(), None);
    assert_eq!(vetoed.note().map(|n| n.key), Some(keys::FIT_NOTE_REHUE_BLOCKED));
    let all_three = CastOutcome { rehue_blocked: true, ratio_rejected: true, ..fan_only };
    assert_eq!(all_three.earns_projection(), None);

    // …and a RATIO-only conviction was never rescuable: there is no fan
    // reading to hand the projection.
    let ratio_only = CastOutcome {
        ratio_rejected: true,
        readings: Some(readings),
        ..CastOutcome::default()
    };
    assert_eq!(ratio_only.earns_projection(), None);
    assert_eq!(ratio_only.note().map(|n| n.key), Some(keys::FIT_NOTE_CAST_REJECTED));
}

/// v1.2.3 fix-up — the search must not bisect PAST the band its own gain
/// bar opens.
///
/// A bisection finds the largest member of a DOWNWARD-CLOSED set. The
/// admissibility half is one (the fan grows with `t`, every gate clears
/// toward the identity); the gain half is the opposite — it falls to zero
/// as `t → 0`, because at `t = 0` there are no curves. Testing both inside
/// the loop makes the clearing set an interval `[a, b]` with `a > 0`, and
/// a probe that fails on the GAIN moves the search DOWN, away from the
/// band, to `None`. The refusal is conservative, but the sentence it then
/// wrote — "no milder version both cleared the limit and left the frame
/// closer to the target" — was a claim about the whole path made by a
/// search that never looked at it; the sentence now says what the search
/// did (nothing cleared, or the best-paying clearing point did not pay).
///
/// The property survives the v1.2.4 sweep unchanged, and it is the reason
/// the sweep could be added at all: the bisection still sees only
/// admissibility, so the gain bar is still applied after the search
/// rather than inside it — now to the maximum the sweep found rather than
/// to the frontier.
///
/// The synthetic path here is exactly that shape: admissible up to
/// `t = 0.4`, worth more than [`FIT_QUANT`] only above `t = 0.36`. The two
/// probes the ruling names are asserted as premises — `t = 0.5` fails on
/// the fan, `t = 0.25` fails on the gain — so the fixture cannot drift
/// into testing something else.
#[test]
fn the_search_does_not_bisect_past_a_band_the_gain_bar_opens() {
    let err_without = 0.05f32;
    // ratio = 1 − 0.1·t, so gain = err_without·0.1·t = 0.005·t, crossing
    // FIT_QUANT (0.0018) at t = 0.36; the fan is over the target above
    // t = 0.4. Clearing band: (0.36, 0.4].
    let judge = |t: f32| CastOutcome {
        readings: Some(CastReadings {
            ratio: 1.0 - 0.1 * t,
            bound: CAST_ACCEPT_RATIO,
            foreign: Some(0.0),
            rehued: 0.0,
            fan: Some(if t <= 0.4 { 0.0 } else { 4.0 * FAN_PROJECT_DEG }),
        }),
        ..CastOutcome::default()
    };
    let gain = |t: f32| err_without * (1.0 - judge(t).readings.unwrap().ratio);
    assert!(
        judge(0.5).readings.unwrap().fan.unwrap() > FAN_PROJECT_DEG,
        "premise: t = 0.5 must fail on the FAN"
    );
    assert!(gain(0.5) > FIT_QUANT, "premise: …and not on the gain");
    assert!(
        judge(0.25).readings.unwrap().fan.unwrap() <= FAN_PROJECT_DEG,
        "premise: t = 0.25 must be admissible"
    );
    assert!(gain(0.25) <= FIT_QUANT, "premise: …and fail on the GAIN");

    let (t, out) = search_cast_projection_t(err_without, judge)
        .expect("the band inside (0.25, 0.5) must be found");
    assert!(
        (0.36..=0.4).contains(&t),
        "the search must land in the clearing band (0.36, 0.4], not at {t}"
    );
    assert!(gain(t) > FIT_QUANT, "…and the winner must actually pay ({})", gain(t));
    assert_eq!(
        out.readings.unwrap().fan,
        Some(0.0),
        "…and be the admissible outcome, not the last probe"
    );
}

/// v1.2.4 — the search takes the BEST-PAYING admissible shrink, not the
/// strongest one.
///
/// v1.2.3 judged the gain at exactly one point, the admissible frontier,
/// and said so: a shrink that pays only at a milder `t` was not found, and
/// the two-family HSL pair was refused although every `t ≤ 0.25` on its
/// path was admissible and paid 0.0019–0.0033. The path here has that
/// shape written down — admissible up to `t = 0.4`, gain peaking at
/// `t = 0.2` and falling to a tenth of [`FIT_QUANT`] at the frontier — so
/// a frontier-only search must refuse it and a sweep must find the peak.
/// Both premises are asserted before the search runs, so the fixture
/// cannot drift into testing something else.
#[test]
fn the_search_takes_the_best_paying_admissible_shrink() {
    let err_without = 0.05f32;
    // gain(t) = 0.0033 − 0.06·(t − 0.2)², so the peak is 0.0033 at
    // t = 0.2 and the frontier at t = 0.4 pays 0.0009 — half the bar.
    let gain = |t: f32| 0.0033 - 0.06 * (t - 0.2) * (t - 0.2);
    let judge = |t: f32| CastOutcome {
        readings: Some(CastReadings {
            ratio: 1.0 - gain(t) / err_without,
            bound: CAST_ACCEPT_RATIO,
            foreign: Some(0.0),
            rehued: 0.0,
            fan: Some(if t <= 0.4 { 0.0 } else { 4.0 * FAN_PROJECT_DEG }),
        }),
        ..CastOutcome::default()
    };
    assert!(
        judge(0.4).readings.unwrap().fan.unwrap() <= FAN_PROJECT_DEG
            && judge(0.45).readings.unwrap().fan.unwrap() > FAN_PROJECT_DEG,
        "premise: the admissible frontier is at t = 0.4"
    );
    assert!(gain(0.4) < FIT_QUANT, "premise: the FRONTIER does not pay ({})", gain(0.4));
    assert!(gain(0.2) > FIT_QUANT, "premise: the interior peak does ({})", gain(0.2));

    let (t, out) = search_cast_projection_t(err_without, judge)
        .expect("an admissible paying shrink exists at t = 0.2 and must be found");
    assert!(
        (t - 0.2).abs() <= 0.01,
        "the search must land on the gain PEAK, not the frontier: {t}"
    );
    assert!(
        gain(t) > gain(0.4),
        "…so it must pay more than the frontier does ({} vs {})",
        gain(t),
        gain(0.4)
    );
    assert_eq!(
        out.readings.unwrap().fan,
        Some(0.0),
        "…and carry the winning probe's own readings"
    );
}

/// v1.2.3 fix-up — the haze regression is NEVER projected, asserted in the
/// tree rather than by a probe that does not ship.
///
/// The claim the release notes make about this pair is byte-identity: its
/// recipe and rationale with the rescue live are what they were without
/// it. What makes that true is not a comparison, it is the rescue arm's
/// GUARD — `CastOutcome::earns_projection` returns `None` here because the
/// fan gate never convicts this cast (7.8° against a 15° line), so every
/// `fit_cast_stage` call on this pair runs the same code with
/// `rescue = true` as with `rescue = false`. That guard is what this test
/// measures, because it is the thing that can break; a literal
/// rescue-on/rescue-off comparison is not expressible from here without a
/// test seam in the solve, and a seam would be the more fragile pin.
#[test]
fn the_haze_correction_is_never_projected() {
    use crate::rationale::keys;
    let (base, clean) = haze_pair();
    let c = cast_stage_candidate(&base, &clean);
    let evidence = evidence_model(&c.cur, &c.tp);
    let out =
        cast_gate_outcome_with_ratio(
            &c.cur, &c.with_px, &c.tp, &evidence, None, None, CAST_ACCEPT_RATIO,
        );
    let fan = out
        .readings
        .expect("a judged cast carries readings")
        .fan
        .expect("this pair's census has a fan to report");
    assert!(
        fan < FAN_DEG,
        "premise broken: the haze correction now trips the fan gate ({fan:.1}°)"
    );
    assert_eq!(out.hue_fanned, None, "…so the gate does not convict it");
    assert_eq!(
        out.earns_projection(),
        None,
        "…and the rescue arm's guard is false, whatever the other gates say"
    );

    // End to end: the admission, and nothing about a projection.
    let rep = fit_recipe(&base, &clean);
    assert!(
        !rep.recipe.blue_curve.is_empty(),
        "premise broken: the haze un-cast is no longer shipped: {}",
        rep.recipe.rationale
    );
    assert!(
        rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED),
        "the haze cast is ADMITTED, on its own merits: {}",
        rep.recipe.rationale
    );
    assert!(
        !rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_PROJECTED),
        "…and never projected: {}",
        rep.recipe.rationale
    );
    assert!(
        !rep.recipe.rationale.contains("shrunk toward the shape"),
        "…not even in words: {}",
        rep.recipe.rationale
    );
}

/// The fan gate's OTHER half: it must not touch a cast that is doing its
/// job. The haze regression's un-cast is the pair whose curves the fit
/// has always shipped, and it opens 7.8° — 1.9× under the threshold.
/// Measured beside it, the pairs the other gates already refuse:
/// canyon-gold 5.2°, hazy→vivid 2.7°.
///
/// CANYON-WARM MOVED (2026-09-02, CAST-2), and the move is asserted below
/// rather than quietly dropped. On the fan-gate-only build its whole
/// recipe was RESET to the calibration base by the terminal do-no-harm
/// check — err 0.0387 → 0.0387, confidence on the 0.25 floor — so the
/// candidate this test rebuilds was the cast on a BARE base and read
/// 7.5°. With the projection the fit lands instead (0.0387 → 0.0339,
/// confidence 0.406), the candidate is rebuilt on a real recipe, and it
/// reads 17.2°: convicted by the gate, then projected to +7°. Since the
/// tone sliders' own do-no-harm (2026-10-21) the landing is 0.0349 at
/// confidence 0.389: the regression asks contrast +3.1 and nothing else,
/// which leaves the tonal reading worse (0.0399 → 0.0430), so the stage
/// ships nothing, the cast is solved from the untoned frame, and the
/// candidate reads 17.5°.
///
/// 2026-10-22, the ungated paired tone arm: canyon-warm's regression reads
/// the red offset's luma lift and asks contrast −21.6 / exposure +0.25 /
/// blacks +14, and the candidate rebuilt on that recipe fans 12.1° — under
/// the line, so the fan gate leaves it alone and the pixel-aligned re-hue
/// gate withholds the curves (0.0387 → 0.0365 at confidence 0.356, no
/// projection). A luma-kept regrade ([`canyon_warm_luma_kept`]) does not
/// bring the projection back: the tone stage then ships nothing, and at
/// offsets 0.18 / 0.24 / 0.30 / 0.36 / 0.45 the candidate fans 8.2 / 10.0 /
/// 12.1 / 12.9 / 14.2° and the re-hue gate withholds every time. So the
/// pair stays and is pinned as withheld. Canyon-gold's candidate on its new
/// recipe reads 10.1° (was 5.2°), so its ceiling is 0.75 of the line.
#[test]
fn the_fan_gate_leaves_a_legitimate_cast_alone() {
    let clean = synth();
    let mut haze = EditRecipe {
        exposure_ev: -0.3,
        contrast: -45.0,
        blacks: 40.0,
        saturation: -40.0,
        blue_curve: vec![
            CurvePoint { input: 0, output: 25 },
            CurvePoint { input: 128, output: 132 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    haze.clamp();
    let hazed = render::develop_preview(&clean, &haze);
    let report = fit_recipe(&hazed, &clean);
    assert!(
        !report.recipe.blue_curve.is_empty(),
        "premise broken: the haze un-cast is no longer admitted: {}",
        report.recipe.rationale
    );
    for (name, src, tgt, ceiling) in [
        ("haze", hazed.clone(), clean.clone(), 0.6),
        ("canyon_gold", canyon(false), canyon_gold_target(), 0.75),
        ("hazy_vivid", hazy_canyon_source(), vivid_warm_target(), 0.3),
    ] {
        let c = cast_stage_candidate(&src, &tgt);
        let fan = hue_fan_weighted(&c.cur, &c.with_px, &evidence_model(&c.cur, &c.tp))
            .map(|(_, added, _)| added)
            .unwrap_or(0.0);
        assert!(
            fan < ceiling * FAN_DEG,
            "margin eroded: {name} fans {fan:.1}° against the {FAN_DEG}° threshold"
        );
    }
    // Canyon-warm, pinned where it now sits — since 2026-10-22 as a pair the
    // fan gate does NOT convict and the re-hue gate withholds, so a future
    // change that sends it back to a bare-base reset (or lets its curves
    // through) fails here with its number.
    let warm = fit_recipe(&canyon(false), &canyon(true));
    assert!(
        warm.err_after < warm.err_before,
        "canyon-warm must land rather than reset: {} -> {}",
        warm.err_before,
        warm.err_after
    );
    // …in NUMBERS. The ruling that accepted this fixture's move named the
    // landing it accepted, so the landing is what is asserted, with the
    // tolerance stated: measured 2026-09-02 on this tree, err_after
    // 0.0339 and reported confidence 0.4061; re-measured 2026-10-21 with
    // the tone sliders withheld on their own tonal reading, 0.0349 and
    // 0.3889; 2026-10-22 under the ungated tone arm, 0.0365 and 0.3564.
    assert!(
        (warm.err_after - 0.0365).abs() < 0.001,
        "canyon-warm's landing moved off the measured 0.0365: {} -> {}",
        warm.err_before,
        warm.err_after
    );
    assert!(
        (warm.recipe.confidence - 0.356).abs() < 0.01,
        "…and its reported confidence off the measured 0.356: {}",
        warm.recipe.confidence
    );
    let c = cast_stage_candidate(&canyon(false), &canyon(true));
    let fan = hue_fan_weighted(&c.cur, &c.with_px, &evidence_model(&c.cur, &c.tp))
        .map(|(_, added, _)| added)
        .unwrap_or(0.0);
    assert!(fan < FAN_DEG, "canyon-warm's candidate is left alone by the fan gate ({fan:.1}°)");
    assert!(
        (fan - 12.1).abs() < 0.2,
        "canyon-warm's candidate reads {fan:.1}°, not the 12.1° measured on 2026-10-22"
    );
    use crate::rationale::keys;
    assert!(
        warm.recipe.red_curve.is_empty()
            && warm.recipe.green_curve.is_empty()
            && warm.recipe.blue_curve.is_empty()
            && warm.notes.iter().any(|n| n.key == keys::FIT_NOTE_REHUE_BLOCKED),
        "…its curves are withheld by the re-hue gate: {}",
        warm.recipe.rationale
    );
    assert!(
        !warm.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_PROJECTED
            || n.key == keys::FIT_NOTE_CAST_HUE_FANNED),
        "…and the fan gate neither projected nor refused them: {}",
        warm.recipe.rationale
    );
}

/// v1.2.3 fix-up — the gate's stated WORST CASE, asserted instead of
/// promised. The gate judges the spread the curves ADD and subtracts the
/// spread the class arrived with; because a class is a bin of the BEFORE
/// hue, that baseline is bounded by one class width — and one class
/// width IS `FAN_DEG` (360° / FAN_HUE_CLASSES). So an ADMITTED cast can
/// leave up to 2 × FAN_DEG of ABSOLUTE in-class spread in the delivered
/// frame, which is the tolerance the rustdoc, ARCHITECTURE.md and the
/// release notes all now state in words. The haze pair is the one whose
/// cast the fit admits, so it is where the words get checked.
#[test]
fn an_admitted_cast_delivers_at_most_two_class_widths_of_hue_fan() {
    let clean = synth();
    let mut haze = EditRecipe {
        exposure_ev: -0.3,
        contrast: -45.0,
        blacks: 40.0,
        saturation: -40.0,
        blue_curve: vec![
            CurvePoint { input: 0, output: 25 },
            CurvePoint { input: 128, output: 132 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    haze.clamp();
    let hazed = render::develop_preview(&clean, &haze);
    assert!(
        !fit_recipe(&hazed, &clean).recipe.blue_curve.is_empty(),
        "premise broken: this pair's cast is no longer admitted"
    );
    let candidate = cast_stage_candidate(&hazed, &clean);
    let (_, added, delivered) = hue_fan_weighted(
        &candidate.cur,
        &candidate.with_px,
        &evidence_model(&candidate.cur, &candidate.tp),
    )
    .expect("premise broken: the admitted haze pair has no region-sized hue class");
    assert!(
        delivered < 2.0 * FAN_DEG,
        "an ADMITTED cast delivered {delivered:.1}° of ABSOLUTE in-class hue spread, \
             past the 2 × {FAN_DEG}° worst case the docs claim (added {added:.1}°)"
    );
    // …and the two halves of that arithmetic, so a failure says which
    // one broke: the admission bound, and the class-width bound on the
    // baseline the gate subtracts.
    assert!(added < FAN_DEG, "premise broken: the haze cast would be refused ({added:.1}°)");
    assert_eq!(360.0 / FAN_HUE_CLASSES as f32, FAN_DEG, "one hue class is one FAN_DEG wide");
    assert!(
        delivered - added < 360.0 / FAN_HUE_CLASSES as f32,
        "the baseline the gate subtracts ({:.1}°) is not bounded by one class width",
        delivered - added
    );
}

/// v1.2.3 — the stage's ADMISSION was silent. Every way of producing
/// nothing disclosed (R23-6 A-2 closed the last one), and the strength
/// budget disclosed when IT bought a marginal cast, but the commonest
/// outcome of the whole stage — the curves shipped on their own merits —
/// reached the user as an unexplained presence. The note carries the
/// four gates' own readings so the admission can be checked, not just
/// believed.
#[test]
fn an_admitted_cast_discloses_the_readings_that_let_it_through() {
    let clean = synth();
    let mut haze = EditRecipe {
        exposure_ev: -0.3,
        contrast: -45.0,
        blacks: 40.0,
        saturation: -40.0,
        blue_curve: vec![
            CurvePoint { input: 0, output: 25 },
            CurvePoint { input: 128, output: 132 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    haze.clamp();
    let report = fit_recipe(&render::develop_preview(&clean, &haze), &clean);
    assert!(
        !report.recipe.blue_curve.is_empty(),
        "premise broken: this pair's cast is no longer admitted: {}",
        report.recipe.rationale
    );
    let note = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED)
        .unwrap_or_else(|| {
            panic!("an admitted cast must say so: {}", report.recipe.rationale)
        });
    let arg = |name: &str| {
        note.args
            .iter()
            .find(|(k, _)| *k == name)
            .and_then(|(_, v)| v.parse::<f32>().ok())
            .unwrap_or_else(|| panic!("the note carries {name}: {:?}", note.args))
    };
    // The two readings whose gates are one-sided thresholds ARE on the
    // passing side of their own gate, and the claim is only made for
    // them: the rehued share and the fan reject above their constant,
    // full stop.
    assert!(arg("rehued") < ROT_SHARE, "the disclosed rehued share passed");
    let fan_arg = |name: &str| {
        let n = report
            .notes
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FAN)
            .unwrap_or_else(|| panic!("an admitted cast reports its fan: {}", report.recipe.rationale));
        n.args
            .iter()
            .find(|(k, _)| *k == name)
            .and_then(|(_, v)| v.parse::<f32>().ok())
            .unwrap_or_else(|| panic!("the fan note carries {name}: {:?}", n.args))
    };
    assert!(fan_arg("fan") < FAN_DEG, "the disclosed fan passed");
    assert_eq!(fan_arg("limit"), FAN_DEG, "the fan note names the limit it measured against");
    let foreign_arg = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FOREIGN)
        .and_then(|n| n.args.iter().find(|(k, _)| *k == "foreign"))
        .and_then(|(_, v)| v.parse::<f32>().ok())
        .expect("this pair's target carries hue evidence, so the share is measurable");
    assert!(foreign_arg < VETO_CREATED_SHARE, "the disclosed foreign share passed");

    // The RATIO is NOT claimed to have passed a threshold, because it
    // has none: the ratio arm rejects only when the evidence is also
    // unidentifiable, so an admitted ratio may exceed its bound. What
    // the note must get right is WHICH bound the path used — and that is
    // `budget.cast_ratio`, which the strength budget moves. Asserting
    // against the CAST_ACCEPT_RATIO constant would have passed for a
    // note quoting a bound the fit never applied.
    assert_eq!(
        arg("bound"),
        FitBudget::for_strength(crate::recipe::GradeStrength::default()).cast_ratio,
        "the default-strength solve discloses the default-strength bound"
    );
    // …and it is whatever bound the gate was HANDED, which at any
    // strength but the default is a different number from the anchor.
    // The end-to-end arm cannot make this point on its own: at the
    // default strength `budget.cast_ratio` IS `CAST_ACCEPT_RATIO`, so a
    // note hard-coding the constant would agree with it. (Re-fitting the
    // same pair at max strength does not help either — the budget also
    // moves the solve, and this pair's max-strength cast is refused by
    // the fan gate at 19°.) So the threading is asserted where it lives.
    let widened = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0)).cast_ratio;
    assert_ne!(
        widened, CAST_ACCEPT_RATIO,
        "premise broken: max strength no longer widens the cast bound"
    );
    let candidate = cast_stage_candidate(&render::develop_preview(&clean, &haze), &clean);
    let handed = cast_gate_outcome_with_ratio(
        &candidate.cur,
        &candidate.with_px,
        &candidate.tp,
        &evidence_model(&candidate.cur, &candidate.tp),
        None,
        None,
        widened,
    )
    .readings
    .expect("the gate always keeps its readings")
    .bound;
    assert_eq!(
        handed, widened,
        "the gate must record the bound it was GIVEN, not the default anchor"
    );

    // And a REFUSED stage says the opposite thing, never both.
    let refused = fit_recipe(&coast(false), &coast(true));
    assert!(
        !refused
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED),
        "a withheld cast must not also claim admission: {}",
        refused.recipe.rationale
    );
}

/// v1.2.3 fix-up — an abstaining gate is not a gate that measured zero.
/// Two of the four readings can decline to answer (`foreign` when the
/// target carries no hue evidence at all, `fan` when no hue class is
/// region-sized across two luma slices), and the admission note used to
/// print `0.000` for both — a measurement never taken, disclosed as if
/// it had been, in the very note that exists so the admission can be
/// checked.
#[test]
fn an_unmeasured_cast_reading_says_so_instead_of_printing_a_zero() {
    // 1) The PLUMBING: a target with no chromatic mass at all leaves the
    //    foreign-hue census with nothing to be foreign to, and the fan
    //    census with no region-sized class. Both must arrive as None.
    let grey = DynamicImage::ImageRgb8(RgbImage::from_fn(48, 48, |x, y| {
        let v = (24 + ((x * 3 + y * 2) % 200)) as u8;
        image::Rgb([v, v, v])
    }));
    let (s, t) = analysis_pair(&grey, &grey);
    let tp = pixels_of(&t);
    let cur = pixels_of(&render::develop_preview(&s, &EditRecipe::default()));
    let evidence = evidence_model(&cur, &tp);
    let readings = cast_gate_outcome_with_ratio(
        &cur,
        &cur,
        &tp,
        &evidence,
        None,
        None,
        CAST_ACCEPT_RATIO,
    )
    .readings
    .expect("the gate always keeps its readings");
    assert_eq!(readings.foreign, None, "a colourless target has no foreign-hue share");
    assert_eq!(readings.fan, None, "a colourless target has no region-sized hue class");

    // 2) The DISCLOSURE: an abstention becomes a sentence, and that
    //    sentence carries no number at all — the failure mode was a
    //    digit, so the assertion is about digits.
    let abstained = cast_admission_notes(CastReadings {
        ratio: 0.5,
        bound: CAST_ACCEPT_RATIO,
        foreign: None,
        rehued: 0.0,
        fan: None,
    });
    let keys_of: Vec<&str> = abstained.iter().map(|n| n.key).collect();
    assert_eq!(
        keys_of,
        vec![
            crate::rationale::keys::FIT_NOTE_CAST_ADMITTED,
            crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA,
            crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FAN_NA,
        ]
    );
    for note in &abstained[1..] {
        assert!(note.args.is_empty(), "a not-measurable clause carries no reading");
        let text = crate::rationale::render_one(note);
        assert!(
            !text.chars().any(|c| c.is_ascii_digit()),
            "an unmeasured reading must not print a number: {text}"
        );
        assert!(
            text.contains("not measurable"),
            "an unmeasured reading must SAY it was not measured: {text}"
        );
    }

    // 3) …and a MEASURED fan is signed, because the curves can narrow a
    //    class as easily as widen one, and "opened a −3 degree hue fan"
    //    reported a narrowing as an opening.
    let narrowed = cast_admission_notes(CastReadings {
        ratio: 0.5,
        bound: CAST_ACCEPT_RATIO,
        foreign: Some(0.0),
        rehued: 0.0,
        fan: Some(-3.2),
    });
    let fan_note = narrowed
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FAN)
        .expect("a measured fan writes the measured clause");
    assert_eq!(
        fan_note.args.iter().find(|(k, _)| *k == "fan").map(|(_, v)| v.as_str()),
        Some("-3.2"),
        "a narrowing reads as a signed change, not as an opened fan"
    );
    assert!(crate::rationale::render_one(fan_note).contains("narrowed"));
    let widened = cast_admission_notes(CastReadings { fan: Some(8.4), ..CastReadings::default() });
    assert_eq!(
        widened
            .iter()
            .find(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_ADMITTED_FAN)
            .and_then(|n| n.args.iter().find(|(k, _)| *k == "fan"))
            .map(|(_, v)| v.as_str()),
        // ONE decimal: the clause prints against a limit, and `{:+.0}`
        // rounded the admitted haze pair's 14.6 up to a "+15" that reads
        // as a violation of the 15 beside it.
        Some("+8.4")
    );
}

fn free_atmosphere_wb_for_pair(
    src: &DynamicImage,
    target: &DynamicImage,
) -> (f32, f32, f32) {
    let (s_img, t_img) = analysis_pair(src, target);
    let base = EditRecipe::default();
    let sp = pixels_of(&render::develop_preview(&s_img, &base));
    let tp = pixels_of(&t_img);
    let structural = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let evidence = structural.structure_blind(&tp);
    // Rule 09: the suite holds NO private copy of the solve. Everything
    // above is the shipped preamble (`analysis_pair` -> `develop_preview`
    // -> `evidence_model_for` -> `structure_blind`); the estimator itself
    // is the shipped one, called with `provider: None` — which is what
    // all three callers of this helper pass. The copy that used to live
    // here had drifted twice: it rounded the tint BEFORE the 401-step
    // search instead of after, and it read `evidence.source_weights` /
    // `evidence.target_weights` where production had moved to the
    // shared-content reference, so since R30 R2 it had been testing a
    // solve production no longer performed.
    let anchor = base.as_shot_k.unwrap_or(5500.0);
    let (pair_tp, pair_w) = atmosphere_wb_pairing(&tp, &evidence, None, None);
    let (wb_k, wb_tint, _) =
        atmosphere_wb_from_populations(&sp, pair_tp, &pair_w, anchor);
    (anchor, wb_k, wb_tint)
}

fn mean_hue_in_rows(img: &DynamicImage, rows: std::ops::Range<u32>) -> f64 {
    let rgb = img.to_rgb8();
    let (mut sin, mut cos, mut count) = (0.0f64, 0.0f64, 0.0f64);
    for y in rows {
        for x in 0..rgb.width() {
            let p = rgb.get_pixel(x, y);
            let (r, g, b) =
                (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
            if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                continue;
            }
            let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
            sin += hue.sin();
            cos += hue.cos();
            count += 1.0;
        }
    }
    assert!(count > 0.0, "the audited band must contain chromatic pixels");
    sin.atan2(cos).to_degrees().rem_euclid(360.0)
}

fn hue_distance(a: f64, b: f64) -> f64 {
    (a - b + 540.0).rem_euclid(360.0) - 180.0
}

#[test]
fn wb_clamp_stays_on_the_manifold_and_never_invents_tint() {
    let src = hazy_canyon_source();
    let target = vivid_warm_target();
    let (anchor, free_k, free_tint) = free_atmosphere_wb_for_pair(&src, &target);
    let strength = crate::recipe::GradeStrength::new(0.85);
    let budget = FitBudget::for_strength(strength);
    let (clamped_k, clamped_tint, clamped, before, after, lambda) =
        budgeted_wb(anchor, free_k, free_tint, budget);
    if clamped {
        assert!(before > budget.wb_ratio && after <= budget.wb_ratio);
    } else {
        assert_eq!(lambda, 1.0);
        assert!(wb_gains_fit_budget(render::wb_gains(anchor, free_k, free_tint), budget));
    }
    assert_eq!(clamped_tint, round1(free_tint * lambda));
    assert!(clamped_tint.abs() <= free_tint.abs());
    let expected_k = ((anchor.ln() + (free_k.ln() - anchor.ln()) * lambda).exp()
        / 50.0)
        .round()
        * 50.0;
    assert_eq!(clamped_k, expected_k);
    assert!(wb_gains_fit_budget(
        render::wb_gains(anchor, clamped_k, clamped_tint),
        budget
    ));

    let report = fit_recipe_from_with(
        &src,
        &target,
        &EditRecipe::default(),
        FitOptions { strength, provider: None },
    );
    assert!(report.recipe.tint.abs() <= free_tint.abs());
    assert!(report.recipe.temperature_k.is_some());
    assert_eq!(report.recipe.temperature_k, Some(clamped_k));
    assert_eq!(report.recipe.tint, clamped_tint);
}

#[test]
fn wb_lambda_shrinks_at_high_strength() {
    let (width, height) = (192u32, 128u32);
    let pair_with_blue_band = |band: [f32; 3]| {
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(
            width,
            height,
            |x, y| {
                let level = 0.22 + 0.60 * x as f32 / (width - 1) as f32;
                let p = if y >= 96 { band } else { [level, level, level] };
                image::Rgb(p.map(|value| (value * 255.0).round() as u8))
            },
        ));
        let target = DynamicImage::ImageRgb8(RgbImage::from_fn(
            width,
            height,
            |x, y| {
                let level = 0.22 + 0.60 * x as f32 / (width - 1) as f32;
                let p = if y >= 96 {
                    band
                } else {
                    [(1.15 * level + 0.10).min(1.0), 0.60 * level, 0.28 * level]
                };
                image::Rgb(p.map(|value| (value * 255.0).round() as u8))
            },
        ));
        (source, target)
    };

    let (probe_source, probe_target) = pair_with_blue_band([0.62, 0.65, 0.65]);
    let (anchor, probe_k, probe_tint) =
        free_atmosphere_wb_for_pair(&probe_source, &probe_target);
    let (probe_chosen_k, probe_chosen_tint, _, _, _, _) = budgeted_wb(
        anchor,
        probe_k,
        probe_tint,
        FitBudget::for_strength(crate::recipe::GradeStrength::new(0.85)),
    );
    let gains = render::wb_gains(anchor, probe_chosen_k, probe_chosen_tint);
    let mut warm_bins = Vec::new();
    for x in 0..width {
        let level = 0.22 + 0.60 * x as f32 / (width - 1) as f32;
        let warm = [(1.15 * level + 0.10).min(1.0), 0.60 * level, 0.28 * level];
        let (hue, _, _) = render::rgb_to_hsl(warm[0], warm[1], warm[2]);
        let bin = ((hue * 24.0) as usize).min(23);
        if !warm_bins.contains(&bin) {
            warm_bins.push(bin);
        }
    }
    let mut blue_band = None;
    'red: for red in 40..=70 {
        for green in 40..=70 {
            for blue in 40..=70 {
                let pixel = [red as f32 / 100.0, green as f32 / 100.0, blue as f32 / 100.0];
                let chroma = pixel.iter().copied().fold(0.0f32, f32::max)
                    - pixel.iter().copied().fold(f32::INFINITY, f32::min);
                if chroma < VETO_SUPPORT_CHROMA {
                    continue;
                }
                let (before_hue, _, _) =
                    render::rgb_to_hsl(pixel[0], pixel[1], pixel[2]);
                let before_degrees = before_hue as f64 * 360.0;
                if !(170.0..=250.0).contains(&before_degrees) {
                    continue;
                }
                let moved = [
                    render::linear_to_srgb(render::srgb_to_linear(pixel[0]) * gains[0]),
                    render::linear_to_srgb(render::srgb_to_linear(pixel[1]) * gains[1]),
                    render::linear_to_srgb(render::srgb_to_linear(pixel[2]) * gains[2]),
                ];
                let moved_chroma = moved.iter().copied().fold(0.0f32, f32::max)
                    - moved.iter().copied().fold(f32::INFINITY, f32::min);
                let (after_hue, _, _) = render::rgb_to_hsl(moved[0], moved[1], moved[2]);
                let after_degrees = after_hue as f64 * 360.0;
                let before_bin = ((before_hue * 24.0) as usize).min(23);
                let after_bin = ((after_hue * 24.0) as usize).min(23);
                let bin_distance = |a: usize, b: usize| {
                    let forward = (a as isize - b as isize).rem_euclid(24) as usize;
                    forward.min(24 - forward)
                };
                let after_is_foreign = bin_distance(after_bin, before_bin) > VETO_FAR_BINS
                    && warm_bins
                        .iter()
                        .all(|&warm_bin| bin_distance(after_bin, warm_bin) > VETO_FAR_BINS);
                if moved_chroma >= 0.06
                    && hue_distance(before_degrees, after_degrees).abs() >= 50.0
                    && after_is_foreign
                {
                    blue_band = Some(pixel);
                    break 'red;
                }
            }
        }
    }
    let blue_band = blue_band.expect("a retained blue band exposes the warm WB damage");
    let (source, target) = pair_with_blue_band(blue_band);
    let (anchor, free_k, free_tint) = free_atmosphere_wb_for_pair(&source, &target);
    let (chosen_k, chosen_tint, _, _, _, _) = budgeted_wb(
        anchor,
        free_k,
        free_tint,
        FitBudget::for_strength(crate::recipe::GradeStrength::new(0.85)),
    );
    let before = render::develop_preview(&source, &EditRecipe::default());
    let demand = EditRecipe {
        temperature_k: Some(chosen_k),
        tint: chosen_tint,
        ..EditRecipe::default()
    };
    let after = render::develop_preview(&source, &demand);
    let before_hue = mean_hue_in_rows(&before, 96..128);
    let after_hue = mean_hue_in_rows(&after, 96..128);
    let foreign = foreign_hue_bins(&pixels_of(&target)).expect("target hue census");
    let foreign_before = foreign_share(&pixels_of(&before), &foreign);
    let foreign_after = foreign_share(&pixels_of(&after), &foreign);
    assert!(
        hue_distance(before_hue, after_hue).abs() >= 45.0,
        "fixture premise: fitted {free_k:.0}/{free_tint:+.1} -> {chosen_k:.0}/{chosen_tint:+.1} rotated the retained blue band only {before_hue:.1} -> {after_hue:.1}"
    );
    assert!(cast_paints_foreign_hues(
        &pixels_of(&before),
        &pixels_of(&after),
        &pixels_of(&target)
    ), "foreign-hue premise failed: fitted {free_k:.0}/{free_tint:+.1} -> {chosen_k:.0}/{chosen_tint:+.1}; band {before_hue:.1} -> {after_hue:.1}, target warm {}, shares {foreign_before:.3} -> {foreign_after:.3}",
        mean_hue_in_rows(&target, 0..96));

    let report = fit_recipe_from_promoted_with_disclosure_opts(
        &source,
        &target,
        &EditRecipe::default(),
        true,
        false,
        FitOptions { strength: crate::recipe::GradeStrength::new(0.85), provider: None },
    );
    assert_eq!(report.mode, FitMode::Atmosphere);
    assert!(report.recipe.temperature_k.is_some());
    assert!(report.notes.iter().any(|note| {
        note.key == crate::rationale::keys::FIT_NOTE_WB_CLAMPED
    }));
    let default_report = fit_recipe_from_promoted(&source, &target, &EditRecipe::default(), true);
    assert_eq!(default_report.recipe.temperature_k, None);
    assert_eq!(default_report.recipe.tint, 0.0);
    assert!(!default_report.notes.iter().any(|note| {
        note.key == crate::rationale::keys::FIT_NOTE_WB_WITHHELD_FOREIGN_HUE
    }));
}

#[test]
fn wb_is_withheld_when_every_lambda_paints_a_foreign_hue() {
    let before = vec![[0.08f32, 0.16, 0.82]; 1000];
    let after = vec![[0.92f32, 0.18, 0.58]; 1000];
    let target = vec![[0.82f32, 0.48, 0.16]; 1000];
    assert!(wb_moves_pixels_into_foreign_hues(&before, &after, &target));
    let mut rationale = String::new();
    let mut notes = Vec::new();
    crate::rationale::push_note(
        &mut rationale,
        &mut notes,
        crate::rationale::Note::plain(crate::rationale::keys::FIT_NOTE_WB_WITHHELD_FOREIGN_HUE),
    );
    assert_eq!(notes[0].key, crate::rationale::keys::FIT_NOTE_WB_WITHHELD_FOREIGN_HUE);
    assert!(rationale.contains("White balance withheld"));
}

#[test]
fn rotation_gate_is_the_unique_rejector_on_the_real_pair_geometry() {
    let src = hazy_canyon_source();
    let tgt = vivid_warm_target();
    // Pin the gate DECISIONS at stage 4 so this test keeps meaning "only
    // the rotation gate stands here" — if a fixture drift makes the ratio
    // gate reject too, the premise asserts below fail with numbers.
    let (s2, t2) = analysis_pair(&src, &tgt);
    let tp2 = pixels_of(&t2);
    let rep = fit_recipe(&src, &tgt);
    let mut pre = rep.recipe.clone();
    pre.red_curve = Vec::new();
    pre.green_curve = Vec::new();
    pre.blue_curve = Vec::new();
    let cur = pixels_of(&render::develop_preview(&s2, &pre));
    let mut with = pre.clone();
    with.red_curve = residual_channel_curve(&cur, &tp2, 0);
    with.green_curve = residual_channel_curve(&cur, &tp2, 1);
    with.blue_curve = residual_channel_curve(&cur, &tp2, 2);
    assert!(!with.blue_curve.is_empty(), "premise broken: no blue crush demanded");
    let with_px = pixels_of(&render::develop_preview(&s2, &with));
    let ratio = look_err(&with_px, &tp2) / look_err(&cur, &tp2);
    assert!(
        ratio < CAST_ACCEPT_RATIO,
        "premise broken: the aggregate gate rejects too (ratio {ratio:.3}) — the \
             rotation gate is no longer uniquely load-bearing on this fixture"
    );
    assert!(
        !cast_paints_foreign_hues(&cur, &with_px, &tp2),
        "premise broken: the foreign-hue veto fires — destination should be target-native"
    );
    assert!(
        cast_rotates_a_region(&cur, &with_px),
        "the rotation gate must fire on the real-pair geometry"
    );
    // End-to-end: the fit must have withheld the curves (rotation gate is
    // the only rejector, per the premises above) and kept the sky blue.
    assert!(
        rep.recipe.red_curve.is_empty()
            && rep.recipe.green_curve.is_empty()
            && rep.recipe.blue_curve.is_empty(),
        "cast curves must be withheld on the real-pair geometry"
    );
    let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
    let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
        for x in 0..192 {
            let p = out.get_pixel(x, y);
            let (r, g, b) =
                (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
            if r.max(g).max(b) - r.min(g).min(b) < 0.03 {
                continue;
            }
            let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
            sin += hue.sin();
            cos += hue.cos();
            n += 1.0;
        }
    }
    assert!(n > 0.0, "no chromatic sky pixels to audit");
    let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
    let d = (mean - 214.0 + 540.0).rem_euclid(360.0) - 180.0;
    assert!(
        d.abs() < 30.0,
        "sky hue rotated to {mean:.0}° (Δ{d:.0}°) — the rotation gate is unwired"
    );
}

/// The rotation budget's discriminator, pinned on the three live pairs
/// (calibration-probe numbers, 2026-07-09): the golden-sky regrade
/// rotates 12.5% of the frame ~170° (must fire — both earlier gates are
/// blind to a target-native destination), the violet cast 12.5% at 112°
/// (must fire), the haze correction ≈0.01% past 60° and ~0 past 75°
/// (must NOT fire). End-to-end verdicts live in
/// `cast_must_not_rotate_the_sky_into_a_target_native_hue` /
/// `warm_rock_cast_must_not_violet_the_pale_sky`; this pins the primitive
/// so a threshold tweak that flips one side fails HERE with numbers.
///
/// THE THIRD FIXTURE CAST-2 MOVED (2026-09-02), recorded here rather than
/// inherited from that batch's other two. The stage-4 reconstruction below
/// now neutralises the MIXER before re-deriving the curves
/// (`pre.hsl = Hsl::default()`); delete that one line and the violet leg
/// goes red with "share 0.0000".
///
/// What moved is not the gate — it is where canyon-warm's protection comes
/// from. On the shipped pipeline that pair's mixer now attaches FIRST
/// ([Orange sat +18 lum +18, Blue sat -18 lum -2.6]), and against THAT state
/// the re-derived cast curves rotate nothing: measured 0.0000 of the frame
/// with the mixer in place, 0.1250 with it neutralised. So the fixture's
/// cast is no longer stopped by the rotation veto; it is stopped by the
/// composition of the mixer and the hue-fan projection, which convicts the
/// curves at 16.9° on the pipeline's weighted census (17.2° on this test's
/// unweighted stage-4 reconstruction — two populations, both stated) and
/// ships a shrunk cast at t = 0.653 reading +7.2°.
///
/// The protection itself is intact and is measured where it belongs — on
/// the DELIVERED frame, by `warm_rock_cast_must_not_violet_the_pale_sky`,
/// whose sky now reads 216.9° against that test's ±30° guard around 213°
/// (213.9° before CAST-2, a 3.0° move inside a 30° band).
///
/// Neutralising the mixer is also the right reading of what ROT_DEG and
/// ROT_SHARE are calibrated ON: the PAIR, not the point the do-no-harm
/// loops happened to land on. And it is a no-op on the pre-CAST-2 tree,
/// where both canyon recipes were already the bare calibration base.
#[test]
fn rotation_gate_separates_regrade_from_haze() {
    // Reconstruct stage-4's exact inputs for each pair, like the veto pin
    // test. Also reports whether the re-derived curves are non-empty, so
    // each leg can assert its premise (an empty-curve pair would make the
    // share trivially 0 and the leg vacuous).
    let stage4 = |src: &DynamicImage, tgt: &DynamicImage| {
        let (s2, t2) = analysis_pair(src, tgt);
        let tp2 = pixels_of(&t2);
        let mut pre = fit_recipe(src, tgt).recipe;
        pre.red_curve = Vec::new();
        pre.green_curve = Vec::new();
        pre.blue_curve = Vec::new();
        // …and the MIXER, so what this calibration reads is a function of
        // the PAIR and not of where the do-no-harm loops happened to
        // land. It is the state the 4a' loop's own neutral probe judges.
        // Before v1.2.3's projection both canyon pairs' whole recipes
        // were reset to the base, so this was already the state; with the
        // violet pair now landing, leaving its mixer in place hides the
        // rotation the fitted mixer has already made (measured: 0.1250 of
        // the frame re-hued without it, 0.0000 with).
        pre.hsl = crate::recipe::Hsl::default();
        let cur = pixels_of(&render::develop_preview(&s2, &pre));
        let mut with = pre.clone();
        with.red_curve = residual_channel_curve(&cur, &tp2, 0);
        with.green_curve = residual_channel_curve(&cur, &tp2, 1);
        with.blue_curve = residual_channel_curve(&cur, &tp2, 2);
        let nonempty = !(with.red_curve.is_empty()
            && with.green_curve.is_empty()
            && with.blue_curve.is_empty());
        let with_px = pixels_of(&render::develop_preview(&s2, &with));
        (cur, with_px, nonempty)
    };
    // Golden-sky regrade: destination hue is target-native, so neither
    // earlier hue veto sees it.
    let (c1, w1, ne1) = stage4(&canyon(false), &canyon_gold_target());
    assert!(ne1, "premise broken: the golden pair no longer provokes cast curves");
    let s1 = rehued_share(&c1, &w1);
    assert!(
        cast_rotates_a_region(&c1, &w1),
        "rotation gate must fire on the golden-sky regrade (share {s1:.4})"
    );
    assert!(s1 > 2.0 * ROT_SHARE, "margin eroded: golden share {s1:.4}");
    // Violet canyon: also caught here (112° ≥ 75°) — an independent net
    // under the foreign-hue veto.
    // 2026-10-22: the luma-kept regrade ([`canyon_warm_luma_kept`]). On
    // `canyon(true)` the ungated tone arm now fits contrast −21.6 / exposure
    // +0.25 / blacks +14 to the red offset's luma lift, and the cast rebuilt
    // on that recipe re-hues nothing (share 0.000).
    let (c2, w2, ne2) = stage4(&canyon(false), &canyon_warm_luma_kept(0.45));
    assert!(ne2, "premise broken: the violet pair no longer provokes cast curves");
    let s2 = rehued_share(&c2, &w2);
    assert!(
        cast_rotates_a_region(&c2, &w2),
        "rotation gate must fire on the violet cast (share {s2:.4})"
    );
    assert!(s2 > 2.0 * ROT_SHARE, "margin eroded: violet share {s2:.4}");
    let clean = synth();
    let mut haze = EditRecipe {
        exposure_ev: -0.3,
        contrast: -45.0,
        blacks: 40.0,
        saturation: -40.0,
        blue_curve: vec![
            CurvePoint { input: 0, output: 25 },
            CurvePoint { input: 128, output: 132 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    haze.clamp();
    let base = render::develop_preview(&clean, &haze);
    let (c3, w3, ne3) = stage4(&base, &clean);
    assert!(ne3, "premise broken: the haze pair no longer provokes cast curves");
    let s3 = rehued_share(&c3, &w3);
    assert!(
        !cast_rotates_a_region(&c3, &w3),
        "rotation gate must NOT fire on the haze correction (share {s3:.4})"
    );
    // 0.1× also pins ROT_DEG from BELOW: at 45° the haze pair's share is
    // 0.0134 (0.27× ROT_SHARE) and would fail here; at 75° it measures
    // ≈ 0.0001. 2026-10-22: 0.2×, was 0.1× — under the ungated tone arm the
    // haze candidate is rebuilt on a toned recipe and reads 0.0052 at 75°,
    // 0.0052 at 60° and 0.0056 at 45° (measured with `ROT_DEG` mutated), so
    // NO bar here separates 45° from 75° any more: the pin from below is
    // lost on this fixture (the gold and violet legs read 0.125 at all three).
    assert!(s3 < 0.2 * ROT_SHARE, "margin eroded: haze share {s3:.4} (measured 0.0052)");
}

/// R18: the pass-through exemption's exact borders, patrolled (the R17
/// disclosure left the band unmonitored). A rotation invisible on both
/// ends (cc < 0.05 ∧ wc < 0.09) is exempt; crossing EITHER visibility
/// floor puts it straight back in the census. How much the band forgives
/// on real frames is measured in [`ROT_VISIBLE_AFTER`]'s doc — it is not
/// a rounding error, and the borders below are what keeps it bounded.
#[test]
fn the_pass_through_exemption_borders_are_patrolled() {
    let share = |c: [f32; 3], w: [f32; 3]| rehued_share(&vec![c; 100], &vec![w; 100]);
    // Inside the blind band: a faint blue (cc 0.045) flipped ~174° to a
    // faint warm (wc 0.085) — pass-through, exempt.
    assert_eq!(share([0.655, 0.68, 0.70], [0.735, 0.68, 0.65]), 0.0);
    // After side crosses 0.09: the same faint blue painted a VISIBLE
    // warm — back in the census.
    assert!(share([0.655, 0.68, 0.70], [0.745, 0.68, 0.65]) > 0.99);
    // Before side crosses 0.05: a visible tint re-hued — counted even
    // though the destination stays faint.
    assert!(share([0.645, 0.68, 0.70], [0.70, 0.68, 0.65]) > 0.99);
}

#[test]
fn rotation_census_sees_a_barely_tinted_before_side() {
    // H17: a faint blue (chroma 0.035 — UNDER the visible-tint gate,
    // above the measurable-hue floor) painted strong gold is a ~180°
    // re-hue; the old both-sides-visible census skipped it entirely,
    // reopening the golden-sky class one threshold to the left.
    let cur = vec![[0.665f32, 0.68, 0.70]; 1000];
    let with = vec![[0.92f32, 0.78, 0.58]; 1000];
    assert!(rehued_share(&cur, &with) > 0.99, "the whole faint-blue field re-hued");
    assert!(cast_rotates_a_region(&cur, &with));
    // A truly NEUTRAL before side has no hue to rotate — colourising
    // neutrals is what a corrective cast legitimately does.
    let neutral = vec![[0.68f32, 0.68, 0.68]; 1000];
    assert_eq!(rehued_share(&neutral, &with), 0.0);
}

/// The haze pair reused by several calibration tests below.
fn haze_pair() -> (DynamicImage, DynamicImage) {
    let clean = synth();
    let mut haze = EditRecipe {
        exposure_ev: -0.3,
        contrast: -45.0,
        blacks: 40.0,
        saturation: -40.0,
        blue_curve: vec![
            CurvePoint { input: 0, output: 25 },
            CurvePoint { input: 128, output: 132 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    haze.clamp();
    let base = render::develop_preview(&clean, &haze);
    (base, clean)
}

/// THE CALIBRATION RECORD for the joint value-range family (R23-6, retuned
/// on real pairs in R24 batch 2).
///
/// This test records the fixture measurements and asserts the policy
/// boundary: a refusal emits the refusal note, and a genuine miss emits
/// the miss note. The measurements are diagnostics, not a widened bound.
/// The table below remains executable evidence for fixture drift,
/// assertions — a fixture drift that moves the numbers must fail loudly
/// rather than quietly invalidate the constants.
///
/// Measured on the FIXTURES (weighted reading, base → finished fit). ALL
/// SIX ROWS RE-MEASURED 2026-09-02 on this tree: the previous table
/// predated the hue-fan gate and its projection and every row of it had
/// drifted, some far enough to contradict the assertions below.
///   identity                        0.0000 → 0.0000   (pure quantisation)
///   roundtrip (known recipe)        0.0590 → 0.0022
///   haze → clean (evidence-limited) 0.1937 → 0.0426
///   canyon warm (violet class)      0.1796 → 0.0437   LANDS since v1.2.3
///   canyon gold (rotation class)    0.2768 → 0.2768   refused, unchanged
///   hazy canyon → vivid warm        0.4913 → 0.4554
///
/// Look error and reported confidence beside them, same run (the two
/// synthetic-recipe rows are not re-measured here — they carry no FAR
/// classification and nothing below reads them):
///   haze → clean       0.0547 → 0.0133   confidence 0.680
///   canyon warm        0.0387 → 0.0349   confidence 0.389 (0.0339 / 0.406
///                                        until the tone sliders' own
///                                        do-no-harm, 2026-10-21)
///   canyon gold        0.0547 → 0.0547   confidence 0.250 (whole recipe
///                                        reset by the terminal check)
///   hazy → vivid warm  0.1286 → 0.0964   confidence 0.250
///
/// Measured on SIX REAL (RAW, finished JPEG) pairs off the user's library,
/// 2026-08-17, EXIF-timestamp-confirmed same frame, through `autoshade
/// match` (finished reading, `look_err` before → after, reported
/// confidence). These are what retired the "provisional" label:
///   A1 astro composite     0.141   0.156 → 0.061   the nonsense pair
///   A4 low-sat neutral     0.054   0.091 → 0.063
///   A5 portrait, cropped   0.035   0.070 → 0.029   not-same-frame warned
///   A2 vivid warm          0.030   0.028 → 0.010
///   A6 portrait            0.024   0.124 → 0.028
///   A3 monochrome          0.019   0.024 → 0.012
///
/// Three facts the policy and its diagnostics record:
///   * SEPARATION — the fits that REACH their target land at 0.0000-0.0437
///     (four fixtures) and 0.054 at worst across the five honest real
///     pairs, while a target no global model can reach stays high:
///     0.4554 for the synthetic repaint (the real-pair geometry of
///     2026-07-09 #2), 0.2768 for canyon gold and 0.141 for the real
///     astro composite. That is the gap [`fit_zoned::JOINT_FAR_ERR`] =
///     0.10 sits in — a factor of ten now, not the factor of two the old
///     table showed. The second reading still earns its place, but on the
///     REAL pair rather than on this fixture: the astro composite's
///     `look_err` reads 0.061, i.e. 0.63 confidence, over a render whose
///     Milky Way is gone, whereas the synthetic repaint's 0.0964 now
///     lands on the 0.25 confidence floor, so on that fixture the two
///     readings agree.
///   * THE REFUSAL CLASS is a separate claim, and since v1.2.3 it has ONE
///     member: canyon gold, 0.2768 → 0.2768, i.e. 2.8× JOINT_FAR_ERR —
///     the solver withheld one-sided movement and the terminal do-no-harm
///     check reset the whole recipe (look error 0.0547 → 0.0547). Its
///     refusal note must be emitted and the miss note must not be. Canyon
///     warm LEFT this band in v1.2.3 (CAST-2): its fan-convicted cast is
///     projected instead of thrown away, the reset no longer fires, and
///     it lands at 0.0437 — asserted below where it now is rather than
///     dropped from the fixture set. The old record's "canyon warm 0.061
///     and canyon gold 0.093 … 0.093 is the tightest constraint, 8% of
///     headroom" was stale on both counts: neither number survives
///     re-measurement, and 0.093 sits BELOW JOINT_FAR_ERR, so a refusal
///     reading it would have emitted no refusal note at all.
///   * MONOTONICITY — every pair improves or holds, and on this tree
///     without exception: the identity pair's old +0.0009 of rounding now
///     reads 0.0000 → 0.0000. That is what
///     [`fit_zoned::JOINT_DRIFT_TOL`] = 0.05 has its headroom over, and
///     all six real pairs improve by far more than 0.05
///     (`pipeline::tests::r16_composed_fit_on_a_real_pair`).
#[test]
fn joint_family_is_calibrated_on_the_fixture_set() {
    let edge = ANALYZE_EDGE;
    let read = |src: &DynamicImage, tgt: &DynamicImage| -> (f32, f32, f32) {
        let s2 = src.thumbnail(edge, edge);
        let tp2 = pixels_of(&tgt.thumbnail(edge, edge));
        let rep = fit_recipe(src, tgt);
        let base_px = pixels_of(&render::develop_preview(&s2, &EditRecipe::default()));
        let fit_px = pixels_of(&render::develop_preview(&s2, &rep.recipe));
        let evidence = evidence_model(&base_px, &tp2);
        let b = crate::fit_zoned::joint_reading_with_evidence(
            &base_px,
            &tp2,
            &evidence.source_weights,
            &evidence.target_weights,
        )
        .expect("base reading");
        let a = crate::fit_zoned::joint_reading_with_evidence(
            &fit_px,
            &tp2,
            &evidence.source_weights,
            &evidence.target_weights,
        )
        .expect("fit reading");
        (b.weighted, a.weighted, rep.err_after)
    };
    // The two bands are asserted SEPARATELY (R24 batch 2). Haze sits in
    // the REACHED band: the paired robust estimator un-casts it and its
    // joint reading lands at 0.0426 (2026-09-02), well under
    // JOINT_FAR_ERR, with the recipe fitting 0.0547 -> 0.0133 and never
    // reset to neutral — a disclosed partial fit rather than a no-op.
    let mut reached_max = 0.0f32;
    let mut refusal_max = 0.0f32;
    // Canyon GOLD is the refusal band's remaining member; canyon warm
    // left it in v1.2.3 and is asserted where it went, below.
    {
        let (name, src, tgt) = ("canyon gold", canyon(false), canyon_gold_target());
        let (before, after, _) = read(&src, &tgt);
        let report = fit_recipe(&src, &tgt);
        assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_REFUSED), "{name}: refusal FAR note missing: {}", report.recipe.rationale);
        assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_MISS), "{name}: refusal emitted miss FAR note: {}", report.recipe.rationale);
        assert!(
            after <= before + crate::fit_zoned::JOINT_DRIFT_TOL,
            "{name}: the fit must not push the joint reading past the drift \
                 tolerance ({before:.4} -> {after:.4})"
        );
        refusal_max = refusal_max.max(after);
    }
    {
        // Canyon warm LEFT the refusal band in v1.2.3 (CAST-2). Its cast
        // was convicted by the hue-fan gate and then projected, and the
        // projected cast was enough for the terminal do-no-harm check to
        // stop resetting the whole recipe: measured 0.0387 -> 0.0339 at
        // confidence 0.406, where the fan-gate-only build reset to the
        // base and reported 0.0387 -> 0.0387 at the 0.25 floor. Since
        // 2026-10-22 the ungated tone arm's sliders (contrast −21.6,
        // exposure +0.25, blacks +14) are what land it, and its curves are
        // withheld by the re-hue gate (see
        // `the_fan_gate_leaves_a_legitimate_cast_alone`). So it is
        // asserted where it now is — no FAR classification of either
        // kind — rather than dropped from the fixture set.
        let (before, after, _) = read(&canyon(false), &canyon(true));
        let report = fit_recipe(&canyon(false), &canyon(true));
        assert!(report.err_after < report.err_before, "canyon warm no longer resets: {} -> {}", report.err_before, report.err_after);
        // …and it lands WHERE it was measured to land, not merely
        // somewhere better (2026-09-02, this tree: 0.0387 -> 0.0339 at
        // confidence 0.4061, joint reading 0.1796 -> 0.0437 (0.0590 since
        // 2026-10-22); 2026-10-21,
        // the tone sliders' own do-no-harm withholding the contrast +3.1
        // the regression asks for: 0.0387 -> 0.0349 at 0.3889; 2026-10-22,
        // the ungated tone arm: 0.0387 -> 0.0365 at 0.3564).
        assert!(
            (report.err_after - 0.0365).abs() < 0.001
                && (report.recipe.confidence - 0.356).abs() < 0.01,
            "canyon warm's landing moved off the measured 0.0365 / 0.356: {} -> {} at {}",
            report.err_before, report.err_after, report.recipe.confidence
        );
        assert!(
            (after - 0.0590).abs() < 0.005,
            "…and its joint reading off the measured 0.0590: {before:.4} -> {after:.4}"
        );
        assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_REFUSED), "canyon warm is no longer a refusal: {}", report.recipe.rationale);
        assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_MISS), "nor a miss: {}", report.recipe.rationale);
        assert!(
            after <= before + crate::fit_zoned::JOINT_DRIFT_TOL,
            "canyon warm: {before:.4} -> {after:.4}"
        );
        reached_max = reached_max.max(after);
    }
    {
        let (before, after, _) = read(&synth(), &synth());
        assert!(
            after <= before + crate::fit_zoned::JOINT_DRIFT_TOL,
            "identity: {before:.4} -> {after:.4}"
        );
        reached_max = reached_max.max(after);
    }
    {
        // The paired robust estimator moved this pair OUT of the refusal
        // bucket (vouched convergence un-casts the haze; look error
        // 0.0547 -> 0.0133, joint reading 0.1937 -> 0.0426, re-measured
        // 2026-09-02): no FAR classification of either kind rides, and
        // the joint reading lands with the reached fixtures.
        let (base, clean) = haze_pair();
        let (before, after, _) = read(&base, &clean);
        let report = fit_recipe(&base, &clean);
        assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_REFUSED), "the vouched haze fit is no longer a refusal: {}", report.recipe.rationale);
        assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_MISS), "nor a miss: {}", report.recipe.rationale);
        assert!(
            after <= before + crate::fit_zoned::JOINT_DRIFT_TOL,
            "evidence-limited haze: {before:.4} -> {after:.4}"
        );
        reached_max = reached_max.max(after);
    }
    {
        let src = synth();
        let mut truth = EditRecipe {
            exposure_ev: 0.35,
            contrast: 18.0,
            highlights: -25.0,
            whites: 12.0,
            saturation: 15.0,
            ..Default::default()
        };
        truth.clamp();
        let tgt = render::develop_preview(&src, &truth);
        let (before, after, _) = read(&src, &tgt);
        assert!(after < before * 0.5, "roundtrip: {before:.4} -> {after:.4}");
        reached_max = reached_max.max(after);
    }
    // Keep the observed values in the transcript for calibration review;
    // the policy assertion is the typed-note split above, not a ceiling
    // derived from this observed maximum.
    let (_, _, look) = read(&hazy_canyon_source(), &vivid_warm_target());
    eprintln!("JOINT_REFUSAL_MEASURED worst={refusal_max:.4}; reached={reached_max:.4}");
    // THE REAL-PAIR ANCHORS, recorded as literals because the photographs
    // cannot ship (RAW + finished JPEG off the user's library, measured
    // through `autoshade match` on 2026-08-17 — the table in this test's
    // doc). These are retained as real-pair diagnostics, not as a way to
    // widen or backfill the policy line.
    // from above, and without them the constant could drift back to any
    // value the fixtures tolerate. They are what makes this test fail on
    // the pre-R24 ladder.
    // Both sides are constants, so they are checked at COMPILE time: a
    // ladder that stops bracketing the real pairs must not wait for
    // someone to run the suite.
    const REAL_NONSENSE_WEIGHTED: f32 = 0.141; // A1, the astro composite
    const REAL_HONEST_WORST: f32 = 0.054; // A4, worst of the five honest
    const _: () = assert!(
        crate::fit_zoned::JOINT_FAR_ERR <= REAL_NONSENSE_WEIGHTED,
        "the real pair the user called nonsense reads 0.141 and must raise \
             the joint warning — that is why the line moved off 0.25"
    );
    const _: () = assert!(
        REAL_HONEST_WORST < crate::fit_zoned::JOINT_FAR_ERR,
        "…and the worst HONEST real pair reads 0.054 and must not"
    );
    // The other end of the same anchor: on that pair the ladder must
    // reach its floor, not merely warn. It reported 0.578 before this
    // retune — a warning printed beside "we are 58% sure" is the
    // incoherence the tie between the two constants exists to prevent.
    assert_eq!(
        clamp_confidence(
            1.0 - REAL_NONSENSE_WEIGHTED * crate::fit_zoned::JOINT_CONFIDENCE_SLOPE
        ),
        CONFIDENCE_FLOOR,
        "the nonsense pair must bottom the confidence out, not shade it"
    );
    // The evidence-weighted scalar now sees spatial damage too. Keep a
    // broad numerical bound so a unit/normalisation regression is caught;
    // this fixture no longer needs to fool the scalar to calibrate the
    // independent joint family.
    // Root-cause fix keeps same-content ranges available even when one
    // local cell is structurally noisy. The new measured calibration
    // value is 0.1234; retain a narrow regression bound above it.
    assert!(
        look < 0.125,
        "the evidence-weighted scalar changed scale unexpectedly ({look:.4})"
    );
}

/// The joint family may REPORT the worst bucket but must never gate on
/// it — measured here, because the temptation is obvious and the data
/// says the opposite. Both measured casts improve the worst bucket, and
/// the cast that should be refused improves it by even more, so a drift
/// gate cannot use that bucket to distinguish the two decisions.
#[test]
fn the_worst_bucket_cannot_gate_a_stage() {
    let edge = ANALYZE_EDGE;
    let cast_pair = |src: &DynamicImage, tgt: &DynamicImage| -> (f32, f32) {
        let s2 = src.thumbnail(edge, edge);
        let tp2 = pixels_of(&tgt.thumbnail(edge, edge));
        let rep = fit_recipe(src, tgt);
        let mut pre = rep.recipe.clone();
        pre.red_curve = Vec::new();
        pre.green_curve = Vec::new();
        pre.blue_curve = Vec::new();
        let cur = pixels_of(&render::develop_preview(&s2, &pre));
        let mut with = pre.clone();
        with.red_curve = residual_channel_curve(&cur, &tp2, 0);
        with.green_curve = residual_channel_curve(&cur, &tp2, 1);
        with.blue_curve = residual_channel_curve(&cur, &tp2, 2);
        let with_px = pixels_of(&render::develop_preview(&s2, &with));
        (
            crate::fit_zoned::joint_reading(&cur, &tp2).expect("without").worst,
            crate::fit_zoned::joint_reading(&with_px, &tp2).expect("with").worst,
        )
    };
    let (base, clean) = haze_pair();
    let (haze_without, haze_with) = cast_pair(&base, &clean);
    assert!(
        haze_with < haze_without,
        "the measured haze cast improves the worst bucket ({haze_without:.4} -> {haze_with:.4})"
    );
    let (gold_without, gold_with) = cast_pair(&canyon(false), &canyon_gold_target());
    assert!(
        gold_with < gold_without,
        "the measured wrecking cast also improves the worst bucket ({gold_without:.4} -> {gold_with:.4})"
    );
    assert!(
        haze_with - haze_without > gold_with - gold_without,
        "both casts improve the bucket, and the wrecking cast improves it more; a drift gate cannot distinguish them: haze {haze_without:.4} -> {haze_with:.4}; wrecking {gold_without:.4} -> {gold_with:.4}"
    );
}

/// The confidence family is ONE calibration: the FAR warning fires
/// exactly where the slope has already bottomed the number out. Two
/// literals could drift apart silently; this is why they are named.
#[test]
fn the_confidence_family_is_one_calibration() {
    let bottom = (1.0 - CONFIDENCE_FLOOR) / CONFIDENCE_SLOPE;
    assert!(
        (bottom - FIT_FAR_ERR).abs() <= 0.01,
        "the FAR line ({FIT_FAR_ERR}) must be the residual at which the \
             slope reaches the floor ({bottom})"
    );
    assert_eq!(confidence_from_look_err(bottom + 0.001), CONFIDENCE_FLOOR);
    assert_eq!(confidence_from_look_err(0.0), CONFIDENCE_CEIL);
    // The joint ladder is its own calibration with the same shape.
    let jb = (1.0 - CONFIDENCE_FLOOR) / crate::fit_zoned::JOINT_CONFIDENCE_SLOPE;
    assert!(
        (jb - crate::fit_zoned::JOINT_FAR_ERR).abs() <= 0.01,
        "the joint FAR line must be the weighted reading at which its own \
             slope reaches the floor ({jb})"
    );
}

/// R23-6 A-2: the colour stage's SILENT arm. `ratio_fail` empties all
/// three channel curves and used to push no note, while the hue gates
/// beside it did disclose — so the commonest way for the colour stage to
/// produce nothing was also the only one the user could not read about.
///
/// The decision is pinned as a PURE function because no fixture in this
/// repo reaches the ratio arm AT ALL — an end-to-end test would pass on
/// some other gate's note and prove nothing about the arm it is named
/// for. The old count here ("of the six fixture pairs, 13 stage runs
/// accept, 8 are hue-only rejections and 5 are both") was written when
/// the stage had three gates; it has four since v1.2.3, and the count is
/// re-measured on this tree by instrumenting the gate block and running
/// the whole library battery in its own pass (2026-09-02, with the shared
/// five-pair calibration corpus present): 545 attributable stage runs —
///
/// | outcome                              | runs |
/// |--------------------------------------|------|
/// | admitted, curves shipped             |  111 |
/// | admitted by projection               |   29 |
/// | refused, rotation budget alone       |  265 |
/// | refused, hue fan alone               |   67 |
/// | refused, rotation budget AND hue fan |   44 |
/// | no curves fitted (nothing to judge)  |   29 |
/// | refused by the aggregate ratio       |    0 |
///
/// Zero, not "always with a hue gate": on every pair in this repo one of
/// the three DESTINATION gates convicts first, or the curves pay. So the
/// arm's note is verified here, where the decision is, and the census
/// above is the reason that is the right place for it rather than a
/// smaller ambition.
#[test]
fn a_silently_rejected_colour_stage_now_says_so() {
    use crate::rationale::keys;
    // The arm this test exists for: no hue damage, the aggregate simply
    // did not earn the risk.
    let key = |outcome: CastOutcome| outcome.note().map(|note| note.key);
    assert_eq!(
        key(CastOutcome { ratio_rejected: true, ..CastOutcome::default() }),
        Some(keys::FIT_NOTE_CAST_REJECTED)
    );
    // The hue note wins a double rejection — more specific, and the
    // thing worth saying.
    assert_eq!(
        key(CastOutcome {
            rehue_blocked: true,
            ratio_rejected: true,
            ..CastOutcome::default()
        }),
        Some(keys::FIT_NOTE_REHUE_BLOCKED)
    );
    assert_eq!(
        key(CastOutcome { rehue_blocked: true, ..CastOutcome::default() }),
        Some(keys::FIT_NOTE_REHUE_BLOCKED)
    );
    // v1.2.3, the same precedence question one gate down: the fan
    // refusal is more specific than "did not buy enough" and LESS
    // specific than the pixel-aligned verdict. The second assert is what
    // keeps every recipe the pixel gates already govern byte-identical.
    assert_eq!(
        key(CastOutcome {
            hue_fanned: Some((0.9, 38.0)),
            ratio_rejected: true,
            ..CastOutcome::default()
        }),
        Some(keys::FIT_NOTE_CAST_HUE_FANNED)
    );
    assert_eq!(
        key(CastOutcome {
            hue_fanned: Some((0.9, 38.0)),
            rehue_blocked: true,
            ..CastOutcome::default()
        }),
        Some(keys::FIT_NOTE_REHUE_BLOCKED)
    );
    // …and it carries its readings, because a refusal the user cannot
    // check is not a disclosure.
    let fanned = CastOutcome { hue_fanned: Some((0.917, 37.6)), ..CastOutcome::default() }
        .note()
        .expect("the fan refusal writes a note");
    assert_eq!(
        fanned.args,
        vec![
            ("share", "0.917".to_string()),
            // ONE decimal, for the same reason the admitted clause has
            // one: at `{:.0}` a convicting 15.4 rendered as "15 degrees
            // apart (limit 15)".
            ("fan", "37.6".to_string()),
            ("limit", format!("{FAN_DEG:.0}")),
        ]
    );
    // An ACCEPTED stage says nothing HERE — its own note is pushed from
    // the admission readings, not from this method.
    assert_eq!(key(CastOutcome::default()), None);

    // …and the end-to-end property that follows from it: whenever the
    // colour stage ships nothing, SOMETHING explains it.
    for (name, src, tgt) in [
        ("canyon warm", canyon(false), canyon(true)),
        ("canyon gold", canyon(false), canyon_gold_target()),
        ("hazy canyon", hazy_canyon_source(), vivid_warm_target()),
    ] {
        let rep = fit_recipe(&src, &tgt);
        let empty = rep.recipe.red_curve.is_empty()
            && rep.recipe.green_curve.is_empty()
            && rep.recipe.blue_curve.is_empty();
        if !empty {
            continue;
        }
        assert!(
            rep.notes.iter().any(|n| {
                n.key == keys::FIT_NOTE_CAST_REJECTED
                    || n.key == keys::FIT_NOTE_REHUE_BLOCKED
                    || n.key == keys::FIT_NOTE_REGRESSED
                    || n.key == keys::FIT_SUMMARY_ATMOSPHERE
            }),
            "{name}: an empty colour stage must disclose WHY: {}",
            rep.recipe.rationale
        );
    }
}

/// R23-6 A-5: the disclosure names controls for THIS pair, not the
/// blanket sentence every fit carries. The canyon-gold pair's residual
/// is a per-band hue move the solver has no dial for.
#[test]
fn the_unsolvable_controls_are_named_for_this_pair() {
    let rep = fit_recipe(&canyon(false), &canyon_gold_target());
    let note = rep
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED)
        .unwrap_or_else(|| panic!("no specific disclosure: {}", rep.recipe.rationale));
    let controls = &note.args.iter().find(|(k, _)| *k == "controls").expect("arg").1;
    assert!(controls.contains("hsl"), "expected the colour mixer named, got {controls}");
    // And it must NOT fire on a pair the solver actually reproduces —
    // a disclosure that always fires is the blanket sentence again.
    let src = synth();
    let truth = EditRecipe {
        exposure_ev: 0.35,
        contrast: 18.0,
        highlights: -25.0,
        whites: 12.0,
        saturation: 15.0,
        ..Default::default()
    };
    let good = fit_recipe(&src, &render::develop_preview(&src, &truth));
    assert!(
        !good
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED),
        "a fit that lands must not claim a missing control: {}",
        good.recipe.rationale
    );
}

/// R23-6 B-7: any file may now be the reverse-fit target, so a
/// reference that is not this frame must be WARNED about — and not
/// refused: the user chose the file.
#[test]
fn a_differently_shaped_reference_is_warned_about_not_refused() {
    let src = synth(); // 192x128, aspect 1.5
    // A genuinely different shape — `thumbnail` PRESERVES aspect, so it
    // cannot build this case; cropping can.
    let tall = synth().crop_imm(0, 0, 96, 128); // aspect 0.75
    assert!(!same_frame_plausible(&src, &tall));
    assert!(same_frame_plausible(&src, &synth()));
    // A resize of the SAME frame must not trip it: aspect survives
    // `thumbnail`, and its integer rounding is what the tolerance is for.
    assert!(same_frame_plausible(&src, &synth().thumbnail(97, 97)));
    let rep = fit_recipe(&src, &tall);
    assert!(
        rep.notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_NOTE_NOT_SAME_FRAME),
        "the doubt must be disclosed: {}",
        rep.recipe.rationale
    );
    // Refused would be wrong: a recipe still comes back.
    assert!(rep.err_after.is_finite());
    // …and the DISCLOSURE has to reach the number too (R24 batch 2). The
    // real cropped pair of 2026-08-17 printed "treat the result as
    // unreliable" directly beneath a confidence of 0.83, because both
    // readings are taken over populations the crop already made
    // incomparable and neither can see that. The sentence and the number
    // are one statement or the user believes the number.
    assert!(
        rep.recipe.confidence <= NOT_SAME_FRAME_CONFIDENCE_CAP,
        "a warned pair must not claim more than the cap, got {}",
        rep.recipe.confidence
    );
    // A CAP, not a verdict: the crop-only measurement behind it says the
    // residual could be framing, not that the fit is broken, so the floor
    // stays available to the two ladders that DO measure something.
    // The crop-only warning now reaches the calibrated floor (0.250)
    // because the shared evidence cap sees the incomparable populations;
    // it remains a warning with a recipe, not a refusal.
    assert!(rep.recipe.confidence >= CONFIDENCE_FLOOR);
    // And it must not touch a pair whose frames agree — the cap is keyed
    // to the warning, so a silent pair keeps whatever it earned.
    let same = fit_recipe(&src, &synth());
    assert!(
        same.recipe.confidence > NOT_SAME_FRAME_CONFIDENCE_CAP,
        "an unwarned pair must be free of the cap, got {}",
        same.recipe.confidence
    );
}

/// R23 review MED-3: an ADJUSTED recipe gets an adjusted REPORT.
///
/// The deep reverse-fit moves a solved recipe's saturation and used to hand
/// the solve's own notes to the result. This pins the replacement contract
/// at the only place that can enforce it — outcome notes re-derived from the
/// adjusted recipe's own render, solve notes carried, and the two
/// do-no-harm sentences dropped rather than repeated about a recipe they no
/// longer describe. No network: `rescore_report` is deterministic.
#[test]
fn an_adjusted_recipe_gets_re_derived_notes_not_the_solves() {
    use crate::rationale::{keys, Note};
    let (src, tgt) = (hazy_canyon_source(), vivid_warm_target());
    let solved = fit_recipe(&src, &tgt);

    // The PRIOR the deep path would hand over. Built explicitly rather than
    // taken from `solved`, so the assertions below hold whatever this
    // fixture's solve happens to produce this release — the contract is
    // about which KEYS survive an adjustment, not about one pair's numbers.
    let prior = vec![
        Note::plain(keys::FIT_NOTE_SAT_PEGGED),
        Note::plain(keys::FIT_NOTE_CAST_REJECTED),
        Note::plain(keys::FIT_NOTE_REGRESSED),
        Note::plain(keys::FIT_NOTE_JOINT_REGRESSED),
        Note::new(
            keys::FIT_NOTE_SAT_REDUCED,
            vec![("sat_fitted", "+52".into()), ("sat_now", "+26".into())],
        ),
    ];
    let mut moved = solved.recipe.clone();
    // The deep path's own fixed step (advisor::judge::FIT_ACTION_SAT_STEP,
    // not re-exported); the size is immaterial here, only that it moves.
    moved.saturation += 10.0;
    moved.clamp();
    let rep =
        rescore_report(&src, &tgt, &moved, &EditRecipe::default(), solved.err_before, &prior);
    let has = |k: &str| rep.notes.iter().any(|n| n.key == k);

    // (1) The terminal-reset verdict must NOT survive. This is the arm with
    // teeth: the GUI raises 「THE REVERSE-FIT WAS DISCARDED … reset to
    // neutral」 off this key, so carrying it told the user nothing had been
    // applied while the adjusted recipe was being persisted.
    assert!(!has(keys::FIT_NOTE_REGRESSED), "the solve's terminal reset was carried over");
    assert!(!has(keys::FIT_NOTE_JOINT_REGRESSED), "…and so was its joint arm");
    // (2) Nor the do-no-harm pull-back, whose quoted pair the move breaks.
    assert!(
        !has(keys::FIT_NOTE_SAT_REDUCED),
        "a saturation the recipe no longer has was reported: {}",
        rep.recipe.rationale
    );
    assert!(
        !rep.recipe.rationale.contains("+26"),
        "the stale saturation value leaked into the rationale: {}",
        rep.recipe.rationale
    );
    // (3) The two SOLVE facts do survive — the adjustment cannot falsify
    // "the chroma chase hit the cap" or "which gate refused the curves".
    assert!(has(keys::FIT_NOTE_SAT_PEGGED), "a solve fact was dropped");
    assert!(has(keys::FIT_NOTE_CAST_REJECTED), "a solve fact was dropped");
    // (4) The outcome notes are re-DERIVED, not absent: the summary quotes
    // this recipe's own residual, and the report is self-consistent.
    assert!(
        has(keys::FIT_SUMMARY_WITH_CURVE)
            || has(keys::FIT_SUMMARY_NO_CURVE)
            || has(keys::FIT_SUMMARY_ATMOSPHERE),
        "no summary was derived"
    );
    assert_eq!(rep.err_before, solved.err_before, "err_before is the caller's, unchanged");
    assert!(rep.recipe.rationale.contains(&format!("{:.3}", rep.err_after)));
    assert_eq!(rep.recipe.saturation, moved.saturation, "the adjusted recipe rides through");
    // …and the joint family's own accounting still holds: a reading or the
    // fail-open disclosure, never both and never neither.
    assert_ne!(
        has(keys::FIT_NOTE_JOINT),
        has(keys::FIT_NOTE_JOINT_NONE),
        "the joint reading and its fail-open note must be exclusive"
    );
}

/// The joint family's ADDITIONAL terminal veto (R23-6 C, role 3). No
/// fixture in this repo reaches it — by design, since it has 56× headroom
/// over the largest non-improvement measured — so the decision itself is
/// pinned here, both arms and the fail-open direction.
#[test]
fn the_terminal_check_reads_both_metrics_and_fails_open() {
    use crate::fit_zoned::{JointReading, JOINT_DRIFT_TOL};
    let j = |w: f32| {
        Some(JointReading { worst: w, worst_label: "shadows/colour", weighted: w, buckets: 6 })
    };
    // A clean fit: neither arm objects.
    assert_eq!(
        terminal_harm(0.09, 0.03, j(0.18), j(0.04)),
        TerminalHarm { scalar: false, joint: false }
    );
    // The scalar arm alone (R16's rule, untouched).
    assert!(terminal_harm(0.010, 0.030, j(0.10), j(0.02)).scalar);
    // The JOINT arm alone — the case that motivated it: the frame-global
    // number improves while the value ranges are driven apart.
    let joint_only = terminal_harm(0.09, 0.03, j(0.10), j(0.10 + JOINT_DRIFT_TOL + 0.01));
    assert!(joint_only.joint && !joint_only.scalar);
    assert!(joint_only.any(), "the two arms are OR-ed, not AND-ed");
    // …and it is BOUNDED: drift inside the tolerance is not harm.
    assert!(!terminal_harm(0.09, 0.03, j(0.10), j(0.10 + JOINT_DRIFT_TOL - 0.01)).joint);
    // FAIL-OPEN in both directions: no reading ⇒ no verdict from this
    // arm, never a silent pass dressed as approval.
    assert!(!terminal_harm(0.09, 0.03, None, j(0.9)).joint);
    assert!(!terminal_harm(0.09, 0.03, j(0.0), None).joint);
    // …but the scalar arm still stands on its own when it does.
    assert!(terminal_harm(0.01, 0.9, None, None).any());
    // It can only REJECT: a joint reading that improves cannot rescue a
    // recipe the scalar convicts.
    assert!(terminal_harm(0.010, 0.030, j(0.90), j(0.001)).any());
}

/// The joint reading may only LOWER the reported confidence, and the
/// case it exists for is the one the scalar over-reports.
#[test]
fn the_joint_reading_can_only_lower_confidence() {
    let (unreachable_source, unreachable_target) = structural_permutation_pair();
    let rep = fit_recipe(&unreachable_source, &unreachable_target);
    let scalar_alone = confidence_from_look_err(rep.err_after);
    assert!(
        rep.recipe.confidence < scalar_alone - 0.1,
        "the joint and evidence caps may only lower the scalar claim \
              ({scalar_alone:.2} vs reported {:.2})",
        rep.recipe.confidence
    );
    assert!(rep.recipe.confidence >= CONFIDENCE_FLOOR);
    // …and on a fit that genuinely lands, it must not invent doubt.
    //
    // The canonical same-content example is the haze fixture. Its
    // evidence-limited reading is recorded above as 0.1937 -> 0.1532;
    // this assertion must therefore stay a real fit check, not an identity
    // shortcut.
    let (src, target) = haze_pair();
    let good = fit_recipe(&src, &target);
    // The canonical haze solve now lands at the calibrated floor (0.250)
    // because its shared evidence is deliberately partial. Its measured
    // look and joint readings still improve, so the fit is not a refusal.
    assert!(
        good.recipe.confidence >= CONFIDENCE_FLOOR,
        "a landed fit must not fall below the confidence floor, got {}",
        good.recipe.confidence
    );
    assert!(
        good.err_after < good.err_before
            && good.recipe.rationale.contains("Residual look error"),
        "the landed haze fit must disclose and improve its measured solve"
    );
}

#[test]
fn quantile_and_cdf_are_inverse_on_a_ramp() {
    let px: Vec<[f32; 3]> = (0..4096)
        .map(|i| {
            let v = i as f32 / 4095.0;
            [v, v, v]
        })
        .collect();
    let cdf = luma_cdf(&px);
    for &x in &[0.1f32, 0.25, 0.5, 0.75, 0.9] {
        let p = cdf_at(&cdf, x);
        let back = quantile(&cdf, p);
        assert!((back - x).abs() < 0.01, "x={x} → p={p} → {back}");
    }
}

#[test]
fn evidence_gate_withholds_one_sided_value_ranges() {
    let source: Vec<[f32; 3]> = (0..4096)
        .map(|i| if i % 2 == 0 { [0.10, 0.25, 0.70] } else { [0.18, 0.35, 0.82] })
        .collect();
    let target: Vec<[f32; 3]> = (0..4096)
        .map(|i| if i % 2 == 0 { [0.70, 0.25, 0.10] } else { [0.82, 0.35, 0.18] })
        .collect();
    let e = evidence_model(&source, &target);
    let blue = e
        .hue
        .iter()
        .find(|r| r.source_share > 0.4 && !r.target_populated)
        .expect("the blue source band is one-sided");
    assert_eq!(blue.weight, 0.0);
    assert!(blue.source_populated && !blue.target_populated);
}

/// Each source bin owns its share of one cumulative target-rank quota.
/// A target member is consumed whole at a boundary, so rounding can drift
/// by at most one member over the population; it must not re-charge that
/// boundary overshoot to every later bin and starve the tail.
#[test]
fn rank_pairing_uses_a_cumulative_target_quota() {
    let source = (0..EVIDENCE_LUMA_BINS)
        .flat_map(|bin| {
            let value = (bin as f32 + 0.5) / EVIDENCE_LUMA_BINS as f32;
            [[value; 3]; 3]
        })
        .collect::<Vec<_>>();
    let target = (0..source.len())
        .map(|i| {
            let value = (i as f32 + 0.5) / source.len() as f32;
            [value; 3]
        })
        .collect::<Vec<_>>();
    let source_zone = vec![1.0; source.len()];
    let mut target_zone = vec![0.0; target.len()];
    target_zone[..25].fill(1.0);
    target_zone[25] = 0.5;
    let support_weights = vec![1.0; source.len()];
    let support_divergence = vec![0.0; source.len()];
    let ranges = aggregate_ranges(
        &source,
        &target,
        &source_zone,
        &target_zone,
        SupportField {
            spatial_weights: &support_weights,
            spatial_divergence: &support_divergence,
            globally_same_content: true,
        },
    );

    assert!(
        ranges
            .luma
            .iter()
            .all(|range| range.target_populated && range.target_share > 0.0),
        "every source bin must receive target rank mass: {:?}",
        ranges.luma
    );
}

fn assert_evidence_models_bit_equal(actual: &EvidenceModel, expected: &EvidenceModel) {
    let bits = |values: &[f32]| values.iter().map(|value| value.to_bits()).collect::<Vec<_>>();
    assert_eq!(actual.source_pixels, expected.source_pixels);
    assert_eq!(bits(&actual.source_membership), bits(&expected.source_membership));
    assert_eq!((actual.width, actual.height), (expected.width, expected.height));
    assert_eq!(actual.spatial_supported, expected.spatial_supported);
    assert_eq!(bits(&actual.source_weights), bits(&expected.source_weights));
    assert_eq!(bits(&actual.target_weights), bits(&expected.target_weights));
    assert_eq!(bits(&actual.source_hue_weights), bits(&expected.source_hue_weights));
    assert_eq!(bits(&actual.target_hue_weights), bits(&expected.target_hue_weights));
    assert_eq!(actual.luma, expected.luma);
    assert_eq!(actual.hue, expected.hue);
    assert_eq!(actual.identifiability.to_bits(), expected.identifiability.to_bits());
    assert_eq!(bits(&actual.spatial_weights), bits(&expected.spatial_weights));
    assert_eq!(bits(&actual.spatial_divergence), bits(&expected.spatial_divergence));
    assert_eq!(actual.globally_same_content, expected.globally_same_content);
    assert_eq!(actual.population.to_bits(), expected.population.to_bits());
}

#[test]
fn structure_blind_reaggregates_structural_withholding_but_keeps_population_vetoes() {
    let mut source = Vec::new();
    source.extend(std::iter::repeat_n([0.32; 3], 100));
    source.extend(std::iter::repeat_n([0.50; 3], 100));
    source.extend(std::iter::repeat_n([0.05, 0.10, 0.80], 100));
    source.extend(std::iter::repeat_n([0.68, 0.35, 0.12], 100));
    let mut target = source.clone();
    let blue_luma = luma601(&source[200]);
    target[200..300].fill([blue_luma; 3]);

    let n = source.len();
    let ones = vec![1.0; n];
    let structural_bin = evidence_luma_bin(0.32);
    let mut ingredients = evidence_model_for(&source, &target, 20, 20);
    ingredients.spatial_weights.fill(1.0);
    ingredients.spatial_divergence.fill(0.0);
    ingredients.spatial_supported.fill(true);
    ingredients.globally_same_content = false;
    for (i, pixel) in source.iter().enumerate() {
        if evidence_luma_bin(luma601(pixel)) == structural_bin {
            ingredients.spatial_weights[i] = 0.0;
            ingredients.spatial_divergence[i] = 2.0;
            ingredients.spatial_supported[i] = false;
        }
    }
    let structural = ingredients.scoped(&target, &ones, &ones);
    let withheld = &structural.luma[structural_bin];
    assert!(withheld.source_populated && withheld.target_populated);
    assert_eq!(withheld.weight, 0.0, "premise: this range is withheld only for structure");

    let blind = structural.structure_blind(&target);
    let restored = &blind.luma[structural_bin];
    assert!(restored.two_sided_share > 0.0);
    assert_eq!(restored.weight.to_bits(), restored.two_sided_share.to_bits());

    let blue = blind
        .hue
        .iter()
        .find(|range| range.label == "Blue")
        .expect("Blue evidence band");
    assert!(blue.source_populated && !blue.target_populated);
    assert_eq!(blue.weight, 0.0, "a one-sided population fact must still veto");
    let empty = blind
        .luma
        .iter()
        .find(|range| !range.source_populated && !range.target_populated)
        .expect("fixture leaves an unpopulated luma range");
    assert_eq!(empty.weight, 0.0, "an unpopulated range must remain excluded");
    assert_eq!(blind.population.to_bits(), structural.population.to_bits());
    assert_eq!(
        blind.source_membership.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        structural.source_membership.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert!(blind.spatial_supported.iter().all(|supported| *supported));

    let identical = evidence_model_for(&source, &source, 20, 20);
    assert!(identical.globally_same_content, "premise: identical frames are structurally supported");
    let expected = identical.scoped(&source, &ones, &ones);
    assert_evidence_models_bit_equal(&identical.structure_blind(&source), &expected);
}

/// The range label names the bins that were FOLDED. `last` clamps to the
/// model's top bin inside the function, so a caller's over-long request
/// used to be labelled with a bin the verdict never read.
///
/// MUTATION: print the caller's `last` instead of the clamped one in
/// `luma_evidence_for_bins` and the first assertion fails.
#[test]
fn a_luma_range_label_names_the_bins_actually_folded() {
    let px = vec![[0.4, 0.4, 0.4]; 64];
    let evidence = evidence_model_for(&px, &px, 8, 8);
    let top = EVIDENCE_LUMA_BINS - 1;
    let clipped = luma_evidence_for_bins(&evidence, 0, 99);
    assert_eq!(clipped.label, format!("luma bins 00-{top:02}"));
    let exact = luma_evidence_for_bins(&evidence, 0, top);
    assert_eq!(clipped.label, exact.label, "the same bins fold under the same name");
    assert_eq!(clipped.source_share.to_bits(), exact.source_share.to_bits());
    // An in-range request is labelled as asked.
    assert_eq!(luma_evidence_for_bins(&evidence, 3, 5).label, "luma bins 03-05");
}

/// The Atmosphere luma veto returns the WB to the base, so the clamp fact
/// the stage produced must not outlive it: the report would otherwise say
/// the white balance was "reduced from X to Y" while the recipe carries
/// the base's. Reachable at strengths between the shipped default and the
/// 0.85 disclosure threshold, where the stage clamps and the veto still
/// withholds.
///
/// MUTATION: drop `facts.clamped = None` from `withhold_atmosphere_tone`
/// and the clamp assertion fails.
#[test]
fn the_atmosphere_luma_veto_withdraws_the_wb_clamp_fact_with_the_wb() {
    let base = EditRecipe { tint: -3.0, ..Default::default() };
    let mut recipe = EditRecipe {
        exposure_ev: -0.5,
        temperature_k: Some(7000.0),
        tint: 12.0,
        tone_curve: vec![
            CurvePoint { input: 0, output: 0 },
            CurvePoint { input: 128, output: 150 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    let mut facts = WbFacts {
        clamped: Some((1.4, 1.2, 0.1, 0.9)),
        search_bound: Some(40000.0),
        rotation_coverage: 0.9,
        rotation_disclosure: None,
        foreign_hue_withheld: false,
        rotation_withheld: false,
    };
    withhold_atmosphere_tone(&mut recipe, &base, &mut facts);
    assert_eq!(recipe.temperature_k, base.temperature_k);
    assert_eq!(recipe.tint, base.tint);
    assert_eq!(recipe.exposure_ev, base.exposure_ev);
    assert_eq!(recipe.tone_curve, base.tone_curve);
    assert_eq!(facts.clamped, None, "a clamp describes a white balance the recipe no longer has");
    assert_eq!(facts.search_bound, Some(40000.0), "the search's own fact stands");
    assert_eq!(facts.rotation_coverage, 0.9);
}

#[test]
fn differently_sized_evidence_uses_one_aligned_prefix_domain() {
    let aligned = (0..8)
        .map(|i| {
            let value = 0.1 + i as f32 * 0.1;
            [value, value * 0.9, value * 0.8]
        })
        .collect::<Vec<_>>();
    let extra = [[0.98, 0.02, 0.02]; 4];
    for (source, target) in [
        ([aligned.as_slice(), &extra].concat(), aligned.clone()),
        (aligned.clone(), [aligned.as_slice(), &extra].concat()),
    ] {
        let evidence = evidence_model_for(&source, &target, 4, 2);
        let n = source.len().min(target.len());
        assert_eq!(evidence.population, n as f32);
        assert!(evidence.luma.iter().chain(&evidence.hue).all(|range| {
            range.source_share <= 1.0 && range.target_share <= 1.0
        }));
        let ones = vec![1.0; source.len().max(target.len())];
        let scoped = evidence.scoped(&target, &ones, &ones);
        assert_evidence_models_bit_equal(&scoped, &evidence);
    }
}

#[test]
fn frame_evidence_shares_match_an_analytic_four_block_golden() {
    let counts = [(1usize, 10usize), (5, 20), (9, 30), (13, 40)];
    let source = counts
        .iter()
        .flat_map(|&(bin, count)| {
            let value = (bin as f32 + 0.5) / EVIDENCE_LUMA_BINS as f32;
            std::iter::repeat_n([value; 3], count)
        })
        .collect::<Vec<_>>();
    let target = [
        std::iter::repeat_n([0.15; 3], 25).collect::<Vec<_>>(),
        std::iter::repeat_n([0.35; 3], 25).collect::<Vec<_>>(),
        std::iter::repeat_n([0.65; 3], 25).collect::<Vec<_>>(),
        std::iter::repeat_n([0.85; 3], 25).collect::<Vec<_>>(),
    ]
    .concat();
    let evidence = evidence_model_for(&source, &target, 10, 10);

    for &(bin, count) in &counts {
        let expected = count as f32 / 100.0;
        assert_eq!(evidence.luma[bin].source_share.to_bits(), expected.to_bits());
        assert_eq!(evidence.luma[bin].target_share.to_bits(), expected.to_bits());
    }
    let occupied = counts.iter().map(|&(bin, _)| bin).collect::<Vec<_>>();
    for (bin, range) in evidence.luma.iter().enumerate() {
        if !occupied.contains(&bin) {
            assert_eq!(range.source_share, 0.0);
            assert_eq!(range.target_share, 0.0);
        }
    }
}

#[test]
fn blind_move_veto_counts_soft_membership_mass() {
    let source = vec![[0.4; 3]; 1_400];
    let target = source.clone();
    let frame = evidence_model(&source, &target);
    let mut zone = vec![1.0; source.len()];
    zone[1_000..].fill(0.05);
    let mut evidence = frame.scoped(&target, &zone, &zone);
    let bin = evidence_luma_bin(0.4);
    evidence.luma[bin].weight = 0.0;
    let expected_population = zone.iter().sum::<f32>();
    assert_eq!(evidence.population.to_bits(), expected_population.to_bits());
    assert!((evidence.population - 1_020.0).abs() < 0.01);

    let mut feather_move = source.clone();
    feather_move[1_000..1_060].fill([0.5; 3]);
    assert!(!moves_unsupported_range(&source, &feather_move, &evidence));
    assert!(!moves_unsupported_luma_range(&source, &feather_move, &evidence));

    let mut interior_move = source.clone();
    interior_move[..60].fill([0.5; 3]);
    assert!(moves_unsupported_range(&source, &interior_move, &evidence));
    assert!(moves_unsupported_luma_range(&source, &interior_move, &evidence));
}

/// A deterministic texture with structure at scales that survive the
/// analysis thumbnail (periods of ~180-380 px, plus a little hash noise),
/// spread over many luma bins. `family` picks the wave orientation and
/// periods, so two families share a histogram but no structure.
fn textured(width: u32, height: u32, family: u32) -> DynamicImage {
    let (px, py, pd) = if family == 0 { (37.0, 29.0, 61.0) } else { (23.0, 41.0, 53.0) };
    DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
        let (fx, fy) = if family == 0 { (x as f32, y as f32) } else { (y as f32, x as f32) };
        let wave = 70.0 * (fx / px).sin() * (fy / py).cos() + 40.0 * ((fx + 2.0 * fy) / pd).sin();
        let h = x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ family.wrapping_mul(0x9E37_79B9);
        let noise = ((h >> 8) & 0x1f) as f32 - 16.0;
        let v = (128.0 + wave + noise).clamp(0.0, 255.0) as u8;
        image::Rgb([v, v.saturating_add(20), v.saturating_sub(20)])
    }))
}

/// The bug this pins: a 1600x1067 source thumbnails to 384x256 and a
/// 1600x1069 target to 384x257, and every evidence statistic (and the
/// frame-wide divergence, which returns `matched` on unequal lengths)
/// pairs pixel i with pixel i. One row decided whether the gate existed.
#[test]
fn analysis_pair_puts_a_one_row_taller_target_in_the_source_geometry() {
    let src = textured(1600, 1067, 0);
    let tgt = textured(1600, 1069, 0);
    assert_eq!(
        (tgt.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE).height(), src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE).height()),
        (257, 256),
        "premise: independent thumbnails disagree by one row"
    );
    let (s, t) = analysis_pair(&src, &tgt);
    assert_eq!((s.width(), s.height()), (384, 256));
    assert_eq!((t.width(), t.height()), (384, 256));
    // The target goes through the source's operator (box filter, rows
    // forced), not a different resampling kernel: a Lanczos3 arm here
    // changed a same-scene fit's residual by 1.8x on its own.
    assert_eq!(t.as_bytes(), tgt.thumbnail_exact(384, 256).as_bytes());
    assert_ne!(
        t.as_bytes(),
        tgt.resize_exact(384, 256, image::imageops::FilterType::Lanczos3).as_bytes(),
        "premise: the two operators differ on this texture"
    );
    // An equal-shape pair is byte-for-byte the two thumbnails it always was.
    let same = textured(1600, 1067, 1);
    let (s2, t2) = analysis_pair(&src, &same);
    assert_eq!(s2.as_bytes(), src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE).as_bytes());
    assert_eq!(t2.as_bytes(), same.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE).as_bytes());
}

/// End to end: a target from the other texture family shares the
/// source's histogram but none of its structure, so the frame-wide
/// same-content verdict must be false and populated luma ranges must be
/// withheld -- and one extra target row must not change that verdict
/// from the equal-height pair's.
#[test]
fn one_extra_target_row_does_not_disable_the_structural_gate() {
    let src = textured(1600, 1067, 0);
    let rotated = textured(1600, 1067, 1);
    let taller = textured(1600, 1069, 1);
    let verdicts = |evidence: &EvidenceModel| {
        evidence
            .luma
            .iter()
            .map(|r| (r.source_populated, r.target_populated, r.weight > 0.0))
            .collect::<Vec<_>>()
    };
    let equal = fit_recipe(&src, &rotated);
    let uneven = fit_recipe(&src, &taller);
    let equal_structural = equal.structural_evidence.as_ref().unwrap_or(&equal.evidence);
    let uneven_structural = uneven.structural_evidence.as_ref().unwrap_or(&uneven.evidence);
    assert!(!equal_structural.globally_same_content, "premise: the other family is divergent");
    assert!(
        equal_structural.luma.iter().any(|r| r.source_populated && r.weight <= 0.0),
        "premise: the divergent pair withholds at least one populated range"
    );
    assert!(
        !uneven_structural.globally_same_content,
        "one extra target row must not turn the same-content verdict on"
    );
    assert_eq!(
        verdicts(uneven_structural),
        verdicts(equal_structural),
        "the range verdicts must not depend on a row of rounding"
    );
    assert_eq!(
        uneven_structural.source_pixels.len(),
        equal_structural.source_pixels.len(),
        "both pairs are judged in the source's analysis geometry"
    );
}

#[test]
fn evidence_weighted_objective_sees_spatial_permutation() {
    let mut source = Vec::with_capacity(4096);
    for y in 0..64 {
        for x in 0..64 {
            // Both halves carry the same 50/50 value population, but one
            // is a fine checker and the other a broad stripe. Swapping
            // them preserves every marginal statistic exactly while
            // moving genuinely different structure between cells.
            let high = if y < 32 {
                (x + y) % 2 == 0
            } else {
                x < 32
            };
            let v = if high { 0.78 } else { 0.22 };
            source.push([v, v, v]);
        }
    }
    let evidence = evidence_model(&source, &source);
    let mut shuffled = source.clone();
    shuffled.rotate_left(2048);
    let identity = look_err_with_evidence(&source, &source, &evidence);
    let permuted = look_err_with_evidence(&shuffled, &source, &evidence);
    assert!(permuted > identity + 0.001, "spatially wrong render must score worse: {identity} -> {permuted}");
}


// Mutation guard: replace BOTH `evidence.source_weights` and
// `evidence.target_weights` in `look_err_with_evidence` with uniform
// `vec![1.0f32; n]` values. The calibrated weighted score below must then
// differ and this named test turns RED.
#[test]
fn evidence_weighted_objective_changes_the_fitted_population_score() {
    let source: Vec<[f32; 3]> = (0..4096)
        .map(|i| {
            let x = i as f32 / 4095.0;
            [0.08 + 0.84 * x, 0.10 + 0.78 * x, 0.12 + 0.70 * x]
        })
        .collect();
    let target: Vec<[f32; 3]> = source
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i < 1024 {
                [p[0] * 0.55, p[1] * 0.55, p[2] * 0.55]
            } else {
                [p[0] * 1.18, p[1] * 1.18, p[2] * 1.18]
            }
        })
        .collect();
    let evidence = evidence_model(&source, &target);
    let weighted = look_err_with_evidence(&source, &target, &evidence);
    let uniform = {
        let mut unweighted = evidence.clone();
        unweighted.source_weights.fill(1.0);
        unweighted.target_weights.fill(1.0);
        look_err_with_evidence(&source, &target, &unweighted)
    };
    eprintln!("EVIDENCE_OBJECTIVE_CALIBRATION weighted={weighted:.6} uniform={uniform:.6}");
    assert!((weighted - uniform).abs() > 0.01,
        "the objective must use the evidence population, not a uniform replacement");
    assert!((weighted - 0.0591).abs() < 0.005,
        "the evidence-weighted objective calibration drifted: {weighted:.6}");
}

/// The POSITIVE twin of `cast_gate_withholds_motion_where_hue_evidence_is_zero`
/// (R33 §E), at the same seam and on the same verdict.
///
/// The negative fixture blocks through the OTHER door into
/// `source_hue_is_withheld`: its moved pixels are structurally
/// unsupported, so no voucher can ever reach them and the refusal is
/// unconditional. That left the arm's whole vouching half pinned only
/// end-to-end, through a full haze solve, where a change of heart in
/// any of six stages would move the same assertion.
///
/// Here the band is one-sided and the pixels ARE supported: a cyan strip
/// laid over the grey field at the SAME luma (so the rank-equalised
/// structure reading sees one scene, and the 3x3 spatial evidence
/// supports all 4096 pixels), whose paired target is the clean grey
/// underneath. Aqua therefore carries 0.156 of the source and none of
/// the target — UNMEASURABLE, and a cast that de-casts the strip moves
/// 0.156 of the population through it.
///
/// With no voucher that is a blind move and the curves are refused. With
/// one, every moved pixel is individually vouched (robust weight, and
/// its own paired target is where it went), the refusal lifts, and the
/// band is NAMED as carried rather than silently reclassified: two
/// different outcomes must not read the same.
#[test]
fn a_vouched_pixel_carries_the_cast_through_a_one_sided_band_and_is_named() {
    let (w, h) = (64usize, 64usize);
    let hash = |i: usize| {
        let mut v = (i as u32).wrapping_mul(747796405).wrapping_add(2891336453);
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 1000) as f32 / 1000.0 - 0.5
    };
    let mut cur = Vec::with_capacity(w * h);
    let mut target = Vec::with_capacity(w * h);
    let mut with_cast = Vec::with_capacity(w * h);
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        let t = 0.10 * hash(i) + 0.14 * x as f32 / (w - 1) as f32;
        let grey = 0.34 + t;
        let clean = [grey, grey + 0.02, grey + 0.01];
        if y < 10 {
            // Luma-matched to `clean` to within 0.0005, so the strip is a
            // pure COLOUR difference and the structure reading, which is
            // rank-equalised luma, sees the same scene on both sides.
            let cyan = [grey - 0.22, grey + 0.10, grey + 0.18];
            cur.push(cyan);
            target.push(clean);
            with_cast.push(std::array::from_fn(|c| cyan[c] + 0.7 * (clean[c] - cyan[c])));
        } else {
            cur.push(clean);
            target.push(clean);
            with_cast.push(clean);
        }
    }
    let evidence = evidence_model(&cur, &target);
    assert!(
        evidence.spatial_supported.iter().all(|&s| s),
        "premise: a luma-matched colour strip leaves every pixel structurally supported"
    );
    let aqua = &evidence.hue[4];
    assert!(
        aqua.source_populated && !aqua.target_populated && aqua.weight <= 0.0,
        "premise: the band is ONE-SIDED, not sparse and not two-sided: {aqua:?}"
    );
    assert!(
        !cast_rotates_a_region(&cur, &with_cast),
        "premise: the legacy rotation veto must not be what decides this"
    );

    let strict = cast_gate_outcome(&cur, &with_cast, &target, &evidence, None, None);
    assert!(
        strict.rehue_blocked,
        "with no paired verdict a move through an unmeasurable band is blind: {strict:?}"
    );
    assert_eq!(
        vouched_hue_band_names(&cur, &with_cast, &evidence, None, None),
        None,
        "…and nothing was carried, so nothing is named as carried"
    );

    let weights = vec![1.0f32; w * h];
    let vouch = Some((weights.as_slice(), target.as_slice(), crate::fit_cells::MoveAxis::Full));
    let vouched = cast_gate_outcome(&cur, &with_cast, &target, &evidence, vouch, None);
    assert!(
        !vouched.rehue_blocked,
        "every moved pixel converged on its OWN paired target: {vouched:?}"
    );
    assert_eq!(
        vouched_hue_band_names(&cur, &with_cast, &evidence, vouch, None).as_deref(),
        Some("Aqua"),
        "the band the voucher carried movement through must be disclosed by name"
    );
    // The two gates the strength budget owns are untouched by any of
    // this: the voucher lifts ONE arm, and the readings both calls take
    // are the same readings.
    assert_eq!(strict.readings, vouched.readings);
    assert_eq!(strict.ratio_rejected, vouched.ratio_rejected);
}

/// R34 §D3, and the positive twin of
/// `cast_gate_withholds_motion_where_hue_evidence_is_zero`: the REGION's
/// own cells carry a cast through a one-sided band that no per-pixel
/// voucher could.
///
/// The per-pixel arm is not merely weak here, it is ABSENT: `hue_vouch` is
/// `robust_tone.filter(|_| paired)`, and `paired` is false at cell scale,
/// so a solve that reached this state has no paired map to vouch with —
/// which is exactly the state a repainted region puts the solve in. What
/// remains is the target's own verdict on the render: the strip's cell
/// means moved toward, and in the direction of, the target's.
#[test]
fn the_regions_cells_carry_the_cast_through_a_one_sided_band_and_are_named_apart() {
    let (w, h) = (64usize, 64usize);
    let hash = |i: usize, seed: u32| {
        let mut v = (i as u32)
            .wrapping_mul(747796405)
            .wrapping_add(seed.wrapping_mul(2891336453));
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 1000) as f32 / 1000.0 - 0.5
    };
    let mut cur = Vec::with_capacity(w * h);
    let mut target = Vec::with_capacity(w * h);
    let mut with_cast = Vec::with_capacity(w * h);
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        let base = 0.14 * x as f32 / (w - 1) as f32;
        // Independent texture draws on the two sides: pixel i on one side
        // is not pixel i on the other, which is what a repaint does.
        let here = 0.34 + base + 0.10 * hash(i, 1);
        let there = 0.34 + base + 0.10 * hash(i, 9_999);
        let clean = [there, there + 0.02, there + 0.01];
        if y < 10 {
            let cyan = [here - 0.22, here + 0.10, here + 0.18];
            cur.push(cyan);
            target.push(clean);
            with_cast.push(std::array::from_fn(|c| cyan[c] + 0.7 * (clean[c] - cyan[c])));
        } else {
            cur.push([here, here + 0.02, here + 0.01]);
            target.push(clean);
            with_cast.push([here, here + 0.02, here + 0.01]);
        }
    }
    let evidence = evidence_model(&cur, &target);
    let aqua = &evidence.hue[4];
    assert!(
        aqua.source_populated && !aqua.target_populated && aqua.weight <= 0.0,
        "premise: the band is ONE-SIDED, not sparse and not two-sided: {aqua:?}"
    );

    let strict = cast_gate_outcome(&cur, &with_cast, &target, &evidence, None, None);
    assert!(
        strict.rehue_blocked,
        "with no verdict at either scale the move is blind: {strict:?}"
    );

    let cells = crate::fit_cells::PairedCells::build(&target, w as u32, h as u32, &evidence)
        .expect("a populated pair builds cells");
    let verdicts = cells.verdicts(&cur, &with_cast, None);
    let arm = Some((&cells, verdicts.as_slice()));
    let carried = cast_gate_outcome(&cur, &with_cast, &target, &evidence, None, arm);
    assert!(
        !carried.rehue_blocked,
        "the region's cells moved toward their own targets: {carried:?}"
    );
    assert_eq!(
        cell_vouched_hue_band_names(&cur, &with_cast, &evidence, None, arm).as_deref(),
        Some("Aqua"),
        "the band the REGION carried movement through is disclosed by name"
    );
    assert_eq!(
        vouched_hue_band_names(&cur, &with_cast, &evidence, None, arm),
        None,
        "…and it is NOT claimed as individually vouched pixels: two outcomes, two lists"
    );
    // The two gates the strength budget owns are untouched: the cell arm
    // lifts ONE arm, and both calls take the same readings.
    assert_eq!(strict.readings, carried.readings);
    assert_eq!(strict.ratio_rejected, carried.ratio_rejected);
}

/// R34 §D3's OTHER half, and the rule the first implementation got
/// wrong: the cell arm answers for pixels the paired arm cannot be ASKED
/// about, not for pixels it did not vouch.
///
/// Same fixture as above with ONE change — both sides draw the same
/// texture, so every pixel HAS a counterpart. The band is still one-sided,
/// the cells still converge, and the veto still stands, because a pixel
/// that survived and moved away from its own counterpart is evidence
/// against the move rather than a request for a second opinion. Guarding
/// on `!converges` instead rotated the canyon-gold sky 158° into the
/// target's native gold with the cells reading 0.921 / 0.079 / 0.981.
#[test]
fn the_cell_arm_is_refused_where_the_pixels_did_survive() {
    let (w, h) = (64usize, 64usize);
    let hash = |i: usize| {
        let mut v = (i as u32).wrapping_mul(747796405).wrapping_add(2891336453);
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 1000) as f32 / 1000.0 - 0.5
    };
    let mut cur = Vec::with_capacity(w * h);
    let mut target = Vec::with_capacity(w * h);
    let mut with_cast = Vec::with_capacity(w * h);
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        let here = 0.34 + 0.14 * x as f32 / (w - 1) as f32 + 0.10 * hash(i);
        let clean = [here, here + 0.02, here + 0.01];
        if y < 10 {
            let cyan = [here - 0.22, here + 0.10, here + 0.18];
            cur.push(cyan);
            target.push(clean);
            with_cast.push(std::array::from_fn(|c| cyan[c] + 0.7 * (clean[c] - cyan[c])));
        } else {
            cur.push(clean);
            target.push(clean);
            with_cast.push(clean);
        }
    }
    let evidence = evidence_model(&cur, &target);
    let aqua = &evidence.hue[4];
    assert!(
        aqua.source_populated && !aqua.target_populated && aqua.weight <= 0.0,
        "premise: the band is still ONE-SIDED: {aqua:?}"
    );
    let survived = evidence
        .spatial_weights
        .iter()
        .filter(|w| **w >= 1.0 - DIVERGENCE_GLOBAL)
        .count();
    assert_eq!(
        survived,
        evidence.spatial_weights.len(),
        "premise: with one texture draw every pixel's own structure survived"
    );

    let cells = crate::fit_cells::PairedCells::build(&target, w as u32, h as u32, &evidence)
        .expect("a populated pair builds cells");
    let verdicts = cells.verdicts(&cur, &with_cast, None);
    let strip = cells.cell_of(5 * w + 5).expect("the strip sits in a cell");
    assert_eq!(
        verdicts.get(strip).copied().flatten(),
        Some(true),
        "premise: the strip's own cell DID move toward its target — the arm is \
             refused on the pixels' standing, not because the cells disagreed"
    );
    let arm = Some((&cells, verdicts.as_slice()));

    let carried = cast_gate_outcome(&cur, &with_cast, &target, &evidence, None, arm);
    assert!(
        carried.rehue_blocked,
        "a surviving region may not overrule its own pixels: {carried:?}"
    );
    assert_eq!(
        cell_vouched_hue_band_names(&cur, &with_cast, &evidence, None, arm),
        None,
        "…and nothing is claimed to have been carried by cells"
    );
}

#[test]
fn cast_gate_withholds_motion_where_hue_evidence_is_zero() {
    let mut cur = Vec::with_capacity(4096);
    let mut target = Vec::with_capacity(4096);
    let mut with_cast = Vec::with_capacity(4096);
    for y in 0..64 {
        for x in 0..64 {
            let d = 0.04 * x as f32 / 63.0;
            let blue = [0.18 + d, 0.34 + d, 0.72 + d];
            cur.push(blue);
            if y < 22 {
                target.push(if (x + y) % 2 == 0 {
                    [0.88, 0.12, 0.08]
                } else {
                    [0.08, 0.82, 0.16]
                });
                with_cast.push([0.30 + d, 0.48 + d, 0.90 + d]);
            } else {
                target.push(blue);
                with_cast.push(blue);
            }
        }
    }
    let evidence = evidence_model(&cur, &target);
    assert!(
        evidence
            .spatial_supported
            .iter()
            .take(22 * 64)
            .all(|&supported| !supported),
        "the invented top region must have no structurally supported pixels"
    );
    assert!(
        evidence
            .spatial_supported
            .iter()
            .skip(22 * 64)
            .any(|&supported| supported),
        "the unchanged lower region must retain spatial evidence"
    );
    assert!(
        !cast_rotates_a_region(&cur, &with_cast),
        "fixture must isolate unsupported-range motion from the legacy rotation veto"
    );
    let outcome = cast_gate_outcome(&cur, &with_cast, &target, &evidence, None, None);
    assert!(
        outcome.rehue_blocked,
        "a global cast moved a zero-evidence hue range without triggering legacy vetoes: {outcome:?}"
    );
}

#[test]
fn tone_stage_keeps_withheld_luma_range_at_identity() {
    let (w, h) = (192u32, 128u32);
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = if y < 43 {
            0.72 + 0.20 * x as f32 / (w - 1) as f32
        } else {
            0.12 + 0.42 * x as f32 / (w - 1) as f32
        };
        image::Rgb([(v * 255.0).round() as u8; 3])
    }));
    let target = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = if y < 43 {
            if (x / 3 + y / 3) % 2 == 0 { 0.70 } else { 0.96 }
        } else {
            (0.12 + 0.42 * x as f32 / (w - 1) as f32) * 1.12 + 0.025
        };
        image::Rgb([(v.clamp(0.0, 1.0) * 255.0).round() as u8; 3])
    }));
    let report = fit_recipe(&source, &target);
    assert_eq!(report.mode, FitMode::Atmosphere, "premise: the checker sky is divergent");
    let structural = report
        .structural_evidence
        .as_ref()
        .expect("Atmosphere retains the structural ruler for residual and detail work");
    let fitted = render::develop_preview(&source, &report.recipe).to_rgb8();
    let mut no_detail_recipe = report.recipe.clone();
    no_detail_recipe.clarity = 0.0;
    no_detail_recipe.texture = 0.0;
    let without_detail = render::develop_preview(&source, &no_detail_recipe).to_rgb8();
    let original = source.to_rgb8();
    let mut delta = 0.0f32;
    let mut tone_delta = 0.0f32;
    let mut withheld = 0usize;
    for y in 0..43 {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if !source_luma_is_withheld(i, structural) {
                continue;
            }
            delta += (fitted.get_pixel(x, y)[0] as f32
                - original.get_pixel(x, y)[0] as f32)
                .abs()
                / 255.0;
            tone_delta += (without_detail.get_pixel(x, y)[0] as f32
                - original.get_pixel(x, y)[0] as f32)
                .abs()
                / 255.0;
            withheld += 1;
        }
    }
    assert!(withheld > 100, "fixture must contain a withheld high-luma population");
    delta /= withheld as f32;
    tone_delta /= withheld as f32;
    assert!(
        tone_delta > 0.05,
        "the structure-blind Atmosphere tone ruler did not move the population ({delta:.4}, tone {tone_delta:.4}): ev={:.2} c={:.1} h={:.1} s={:.1} w={:.1} b={:.1} curve={:?}; {}",
        report.recipe.exposure_ev,
        report.recipe.contrast,
        report.recipe.highlights,
        report.recipe.shadows,
        report.recipe.whites,
        report.recipe.blacks,
        report.recipe.tone_curve,
        report.recipe.rationale,
    );
    assert!(report.evidence.globally_same_content);
    assert!(report.evidence.spatial_supported.iter().all(|&supported| supported));
    let note = report
        .notes
        .iter()
        .find(|n| {
            n.key == crate::rationale::keys::FIT_NOTE_ATMOSPHERE_POPULATION_EVIDENCE
        })
        .expect("the population ruler must be disclosed");
    let named = &note
        .args
        .iter()
        .find(|(k, _)| *k == "luma_ranges")
        .expect("luma_ranges arg")
        .1;
    let (structural_luma, _) = withheld_range_names(structural);
    assert!(!structural_luma.is_empty());
    assert_eq!(named, &structural_luma, "the note names the structural ranges excluded from Atmosphere");
}

#[test]
fn confidence_identifiability_separates_low_evidence_render() {
    let px: Vec<[f32; 3]> = (0..4096)
        .map(|i| {
            let v = 0.1 + 0.8 * (i % 64) as f32 / 63.0;
            [v, v, v]
        })
        .collect();
    let mut high = evidence_model(&px, &px);
    high.identifiability = 1.0;
    let mut low = high.clone();
    low.identifiability = 0.05;
    let report = |evidence: &EvidenceModel| {
        compose_report(
            EditRecipe::default(),
            Measured {
                err_before: 0.02,
                err_after: 0.01,
                joint_after: None,
                after_px: &px,
                tp: &px,
                same_frame: true,
                mode: FitMode::Full,
                divergence: Some(Divergence::matched()),
                divergence_coarse: Some(Divergence::matched()),
                pairing: PairingScale::Pixel,
                evidence,
                structural_evidence: None,
                defer_disclosure: false,
            },
            SolveFacts {
                budget: None,
                strength: None,
                veto_luma: None,
                veto_hue: None,
                wb_clamped: None,
                wb_search_bound: None,
                wb_rotation_coverage: None,
                wb_rotation_disclosure: None,
                wb_cells: None,
                wb_foreign_hue_withheld: false,
                wb_rotation_withheld: false,
                sat_pegged: None,
                cast: CastOutcome::default(),
                cast_admitted_by_strength: None,
                cast_admitted: None,
                cast_projected: None,
                evidence_refused: false,
                sat_fitted: None,
                regressed: None,
                tone_harm: None,
                curve_harm: None,
                detail: (0.0, 0.0),
                detail_withheld: false,
                robust: None,
                paired: false,
                vouched_bands: None,
                cast_cells: None,
                hsl: HslStageFacts::default(),
                atmosphere_reference: AtmosphereReference::WholeFrame,
            },
        )
    };
    let high_report = report(&high);
    let low_report = report(&low);
    assert!(
        high_report.recipe.confidence - low_report.recipe.confidence > 0.5,
        "the production confidence path must expose identifiability: {} vs {}",
        high_report.recipe.confidence,
        low_report.recipe.confidence
    );
}

#[test]
fn detail_fit_writes_texture_only_from_two_sided_evidence() {
    let (w, h) = (192u32, 128u32);
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let xf = x as f32 / (w - 1) as f32;
        let ripple = 0.025 * (x as f32 * 0.43).sin()
            + 0.015 * ((x + y) as f32 * 1.17).sin();
        let v = (0.18 + 0.62 * xf + ripple).clamp(0.03, 0.95);
        image::Rgb([(v * 255.0).round() as u8; 3])
    }));
    let truth = EditRecipe { texture: 100.0, ..Default::default() };
    let target = render::develop_preview(&source, &truth);
    let source_px = pixels_of(&source);
    let target_px = pixels_of(&target);
    let detail_evidence = evidence_model(&source_px, &target_px);
    let report = fit_recipe(&source, &target);
    eprintln!(
        "DETAIL_FIT clarity={:.0} texture={:.0} ident={:.3} budget=+/-{:.0}",
        report.recipe.clarity,
        report.recipe.texture,
        detail_evidence.identifiability,
        DETAIL_CONTROL_LIMIT,
    );
    assert!(
        report.recipe.texture > 0.0 || report.recipe.clarity > 0.0,
        "the reverse fit did not write either rendered detail control in {:?} mode at evidence {:.3}: {}",
        report.mode,
        detail_evidence.identifiability,
        report.recipe.rationale,
    );
    assert!(report.recipe.texture.abs() <= DETAIL_CONTROL_LIMIT);
    assert!(report.recipe.clarity.abs() <= DETAIL_CONTROL_LIMIT);
    assert!(
        report.err_after <= report.err_before + FIT_QUANT,
        "the detail-specific terminal exception exceeded its bounded look budget"
    );
    assert!(
        detail_residual(&pixels_of(&render::develop_preview(&source, &report.recipe)), &target_px, &detail_evidence)
            < detail_residual(&source_px, &target_px, &detail_evidence),
        "the emitted detail controls did not improve the supported frequency reading"
    );
    assert!(report
        .notes
        .iter()
        .any(|n| n.key == crate::rationale::keys::FIT_NOTE_DETAIL));
}

#[test]
fn ground_truth_parameter_recovery_uses_the_evidence_population() {
    let Some(root) = calibration_corpus() else { return };
    let raw = root.join("source.arw");
    let source = if raw.exists() {
        render::render_to_image(&raw, &EditRecipe::default(), None, Some(ANALYZE_EDGE))
            .expect("develop calibration source.arw")
    } else {
        image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg")
    };
    let source = source.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
    let base = if raw.exists() {
        crate::pipeline::calibration_recipe(crate::pipeline::fit_calibration(&raw))
    } else {
        EditRecipe::default()
    };
    let mut truth = base.clone();
    truth.exposure_ev = 0.45;
    truth.contrast = 18.0;
    truth.highlights = -30.0;
    truth.shadows = 12.0;
    truth.whites = 8.0;
    truth.blacks = -6.0;
    truth.saturation = 14.0;
    let target = render::develop_preview(&source, &truth);
    let report = fit_recipe_from(&source, &target, &base);
    eprintln!(
        "GROUND_TRUTH truth ev={:.2} c={:.1} h={:.1} s={:.1} w={:.1} b={:.1} sat={:.1}; recovered ev={:.2} c={:.1} h={:.1} s={:.1} w={:.1} b={:.1} sat={:.1}; confidence={:.3} err={:.3}->{:.3}",
        truth.exposure_ev, truth.contrast, truth.highlights, truth.shadows, truth.whites, truth.blacks, truth.saturation,
        report.recipe.exposure_ev, report.recipe.contrast, report.recipe.highlights, report.recipe.shadows,
        report.recipe.whites, report.recipe.blacks, report.recipe.saturation, report.recipe.confidence,
        report.err_before, report.err_after,
    );
    eprintln!("GROUND_TRUTH rationale={}", report.recipe.rationale);
    let baseline_four_error = (1.65f32 - 0.45).abs()
        + (-32.2f32 - 18.0).abs()
        + (-15.6f32 - 12.0).abs()
        + (30.1f32 - -6.0).abs();
    let recovered_four_error = (report.recipe.exposure_ev - truth.exposure_ev).abs()
        + (report.recipe.contrast - truth.contrast).abs()
        + (report.recipe.shadows - truth.shadows).abs()
        + (report.recipe.blacks - truth.blacks).abs();
    assert!(
        recovered_four_error < 0.5 * baseline_four_error,
        "parameter recovery, not residual, is the gate: {recovered_four_error:.1} vs baseline {baseline_four_error:.1}"
    );
    assert!((report.recipe.exposure_ev - truth.exposure_ev).abs() < 0.35);
    assert!(report.recipe.confidence < 0.927, "a poorly identified inverse must not retain baseline confidence");
}

#[test]
fn invented_sky_gradient_energy_is_not_amplified() {
    let Some(root) = calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    let mask = image::open(root.join("sky-mask.png"))
        .expect("calibration sky-mask.png")
        .to_luma8();
    let report = fit_recipe(&source, &target);
    let fitted = render::develop_preview(&source, &report.recipe);
    let gradient = |image: &DynamicImage| {
        let rgb = image.to_rgb8();
        let resized = image::imageops::resize(
            &mask,
            rgb.width(),
            rgb.height(),
            image::imageops::FilterType::Triangle,
        );
        let mut sum = 0.0f32;
        let mut total = 0.0f32;
        for y in 1..rgb.height().saturating_sub(1) {
            for x in 1..rgb.width().saturating_sub(1) {
                let weight = resized.get_pixel(x, y)[0] as f32 / 255.0;
                if weight <= 0.0 {
                    continue;
                }
                let lum = |x, y| {
                    let p = rgb.get_pixel(x, y);
                    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
                        / 255.0
                };
                let dx = 0.5 * (lum(x + 1, y) - lum(x - 1, y));
                let dy = 0.5 * (lum(x, y + 1) - lum(x, y - 1));
                sum += (dx * dx + dy * dy).sqrt() * weight;
                total += weight;
            }
        }
        sum / total.max(1e-6)
    };
    let ratio = gradient(&fitted) / gradient(&source).max(1e-6);
    eprintln!(
        "SKY_GRADIENT ratio={ratio:.4}; ev={:.2} contrast={:.1} highlights={:.1} shadows={:.1} whites={:.1} blacks={:.1} sat={:.1} clarity={:.1} texture={:.1} confidence={:.3}",
        report.recipe.exposure_ev,
        report.recipe.contrast,
        report.recipe.highlights,
        report.recipe.shadows,
        report.recipe.whites,
        report.recipe.blacks,
        report.recipe.saturation,
        report.recipe.clarity,
        report.recipe.texture,
        report.recipe.confidence,
    );
    assert!(ratio <= 1.0, "invented sky was sharpened: {ratio:.4}x neutral");
    assert_eq!(report.mode, FitMode::Atmosphere);
    assert!(
        report.recipe.exposure_ev.abs()
            + report.recipe.contrast.abs()
            + report.recipe.tone_curve.len() as f32
            + report.recipe.saturation.abs()
            > 1.0,
        "content-divergent calibration pair was incorrectly returned as an empty recipe"
    );
    assert_eq!(report.recipe.saturation, 0.0);
    assert_eq!(report.recipe.clarity, 0.0);
    assert_eq!(report.recipe.texture, 0.0);
}

#[test]
fn confidence_separates_calibration_recipes_by_evidence_spend() {
    let Some(root) = calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    let conservative = fit_recipe(&source, &target);
    let preferred = calibration_recipe(&root);
    let rescored = rescore_report(
        &source,
        &target,
        &preferred,
        &EditRecipe::default(),
        conservative.err_before,
        &[],
    );
    assert_eq!((conservative.mode, rescored.mode), (FitMode::Atmosphere, FitMode::Atmosphere));
    assert_evidence_models_bit_equal(&rescored.evidence, &conservative.evidence);
    let (thumb, _) = analysis_pair(&source, &target);
    let evidence = conservative
        .structural_evidence
        .as_ref()
        .expect("Atmosphere preserves structural diagnostics");
    let conservative_px = pixels_of(&render::develop_preview(&thumb, &conservative.recipe));
    let preferred_px = pixels_of(&render::develop_preview(&thumb, &preferred));
    let motion = |after: &[[f32; 3]]| {
        let mut supported = 0.0;
        let mut unsupported = 0.0;
        for (i, (before, after)) in evidence.source_pixels.iter().zip(after).enumerate() {
            let delta = (0..3).map(|ch| (after[ch] - before[ch]).abs()).sum::<f32>() / 3.0;
            if evidence.source_weights[i] > 0.0 { supported += delta } else { unsupported += delta }
        }
        (supported / after.len() as f32, unsupported / after.len() as f32)
    };
    let (cs, cu) = motion(&conservative_px);
    let (ps, pu) = motion(&preferred_px);
    eprintln!(
        "CONFIDENCE_SEPARATION conservative={:.3} preferred={:.3} margin={:.3} movement={:.3}/{:.3} motion_su={cs:.4}/{cu:.4} vs {ps:.4}/{pu:.4} pair_ident={:.3}",
        conservative.recipe.confidence,
        rescored.recipe.confidence,
        rescored.recipe.confidence - conservative.recipe.confidence,
        movement_identifiability(&conservative_px, evidence),
        movement_identifiability(&preferred_px, evidence),
        evidence.identifiability,
    );
    assert!(
        (rescored.recipe.confidence - conservative.recipe.confidence).abs() < 0.01,
        "confidence must use the same population ruler, independent of structural spend"
    );
    assert!(
        movement_identifiability(&preferred_px, evidence)
            > movement_identifiability(&conservative_px, evidence) + 0.1,
        "premise: the two recipes remain structurally distinguishable"
    );
    assert!(conservative.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP);
    assert!(rescored.recipe.confidence <= ATMOSPHERE_CONFIDENCE_CAP);
}

// ---------------------------------------------------------------------
// stage 4a — the per-band colour mixer
// ---------------------------------------------------------------------

/// RGB directions whose hue lands on an ACR band centre (red 0°,
/// green 120°, blue 240°, yellow 60°, magenta 300°), each ORTHOGONAL to
/// Rec.601 luma and normalised to unit chroma. Scaling a family's chroma
/// therefore leaves its luma distribution untouched, so a fixture built
/// from them isolates the colour question from the tone one.
const BAND_DIRECTIONS: [[f32; 3]; 5] = [
    [0.70100, -0.29900, -0.29900],
    [-0.58700, 0.41300, -0.58700],
    [-0.11400, -0.11400, 0.88600],
    [0.11400, 0.11400, -0.88600],
    [0.58700, -0.41300, 0.58700],
];

/// One colour family per horizontal quarter, all four riding the same
/// luminance ramp, each family's chroma scaled independently.
fn band_frame(families: [usize; 4], chroma: [f32; 4]) -> DynamicImage {
    let (w, h) = (384u32, 256u32);
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let xf = x as f32 / (w - 1) as f32;
        let quarter = (y * 4 / h) as usize;
        let l = 0.26 + 0.26 * xf + 0.04 * (xf * 41.0).sin();
        let u = BAND_DIRECTIONS[families[quarter]];
        let p = [0usize, 1, 2].map(|c| (l * (1.0 + chroma[quarter] * u[c])).clamp(0.0, 1.0));
        image::Rgb(p.map(|v| (v * 255.0).round() as u8))
    }))
}

/// `source` developed through the REAL engine with a mixer-only recipe.
fn developed(source: &DynamicImage, edit: impl FnOnce(&mut crate::recipe::Hsl)) -> DynamicImage {
    let mut recipe = EditRecipe::default();
    edit(&mut recipe.hsl);
    render::develop_preview(source, &recipe)
}

/// The canonical inverse problem for this stage: the target IS the source
/// carrying a KNOWN per-band edit, -`edit` Green / +`edit` Blue saturation.
/// Frame-mean chroma barely moves, so the ONE global saturation number has
/// nothing useful to say — only a per-band control can express this.
fn engine_hsl_pair(edit: f32) -> (DynamicImage, DynamicImage) {
    let source = band_frame([0, 1, 2, 4], [0.55; 4]);
    let target = developed(&source, |hsl| {
        hsl.saturation[3] = -edit;
        hsl.saturation[5] = edit;
    });
    (source, target)
}

/// The same edit over TWO half-frame families, which gives each band a
/// population big enough to move the joint chromatic buckets the
/// unrepresented-controls disclosure reads.
fn two_family_hsl_pair(edit: f32) -> (DynamicImage, DynamicImage) {
    let source = band_frame([2, 2, 1, 1], [0.55; 4]);
    let target = developed(&source, |hsl| {
        hsl.saturation[3] = -edit;
        hsl.saturation[5] = edit;
    });
    (source, target)
}

/// A demand far past the per-band ceiling: the blue quarter is 1.64x more
/// chromatic than the source's, which +18 can only partly close.
fn over_cap_band_pair() -> (DynamicImage, DynamicImage) {
    (
        band_frame([0, 1, 2, 4], [0.55; 4]),
        band_frame([0, 1, 2, 4], [0.55, 0.55, 0.90, 0.55]),
    )
}

/// The target repaints the green quarter yellow: Green exists only in the
/// source, Yellow only in the target. Nothing else moves.
fn one_sided_band_pair() -> (DynamicImage, DynamicImage) {
    (
        band_frame([0, 1, 2, 4], [0.55; 4]),
        band_frame([0, 3, 2, 4], [0.55; 4]),
    )
}

fn hsl_note<'a>(report: &'a FitReport, key: &str) -> Option<&'a crate::rationale::Note> {
    report.notes.iter().find(|n| n.key == key)
}

fn note_arg(report: &FitReport, key: &str, arg: &str) -> String {
    hsl_note(report, key)
        .and_then(|n| n.args.iter().find(|(k, _)| *k == arg))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

fn names_hsl(report: &FitReport) -> bool {
    report
        .notes
        .iter()
        .any(|n| n.args.iter().any(|(k, v)| *k == "controls" && v.contains("hsl")))
}

/// (a) A band both frames can speak for is SOLVED, and the one global
/// saturation number demonstrably could not have carried it.
#[test]
fn per_band_colour_is_solved_from_two_sided_population_evidence() {
    let (src, tgt) = engine_hsl_pair(15.0);
    let report = fit_recipe(&src, &tgt);
    let budget = FitBudget::for_strength(crate::recipe::GradeStrength::default()).hsl_band;
    assert_eq!(report.mode, FitMode::Full, "premise: the fixture is a same-content pair");
    let hsl = &report.recipe.hsl;
    assert!(
        hsl.saturation[5] >= 10.0 && hsl.saturation[5] <= budget,
        "Blue must recover most of its +15: {:?}",
        hsl.saturation
    );
    assert!(
        hsl.saturation[3] <= -10.0 && hsl.saturation[3] >= -budget,
        "…and Green most of its -15: {:?}",
        hsl.saturation
    );
    // THE point of the stage: the target's frame-mean chroma is almost
    // unchanged, so the single global dial has nothing to say and the
    // recovery cannot be credited to it.
    assert!(
        report.recipe.saturation.abs() <= 5.0,
        "one global saturation cannot express opposed bands: {}",
        report.recipe.saturation
    );
    assert!(
        report.err_after < report.err_before,
        "the composed fit must end closer: {} -> {}",
        report.err_before,
        report.err_after
    );
    let moved = note_arg(&report, crate::rationale::keys::FIT_NOTE_HSL_BANDS, "moved");
    assert!(
        moved.contains("Green sat -") && moved.contains("Blue sat +"),
        "the disclosure names what moved and by how much: {moved:?}"
    );
}

/// (b) A band only ONE frame can speak for is refused BY NAME. One-sided
/// is unmeasurable, never silently read as "these already match".
#[test]
fn a_one_sided_band_is_refused_by_name_never_read_as_equal() {
    let (src, tgt) = one_sided_band_pair();
    let report = fit_recipe(&src, &tgt);
    // Premise, straight off the shared evidence model: Green is in the
    // source alone and Yellow in the target alone.
    assert!(
        report.evidence.hue[3].source_populated && !report.evidence.hue[3].target_populated,
        "premise: Green is source-only"
    );
    assert!(
        !report.evidence.hue[2].source_populated && report.evidence.hue[2].target_populated,
        "premise: Yellow is target-only"
    );
    for band in [2usize, 3] {
        assert_eq!(report.recipe.hsl.saturation[band], 0.0, "band {band} must not move");
        assert_eq!(report.recipe.hsl.luminance[band], 0.0, "band {band} must not move");
    }
    let refused = note_arg(&report, crate::rationale::keys::FIT_NOTE_HSL_BANDS, "refused");
    assert!(
        refused.contains("Green (one-sided)") && refused.contains("Yellow (one-sided)"),
        "the refusal is typed and named, not silence: {refused:?}"
    );
}

/// (c) The hue axis is never written, on any pair, at any strength —
/// including a target whose only edit IS a band rotation, where the
/// temptation to rotate back is greatest.
#[test]
fn the_per_band_mixer_never_rotates_a_hue_band() {
    let rotated_source = band_frame([0, 1, 2, 4], [0.55; 4]);
    let rotated = developed(&rotated_source, |hsl| {
        hsl.hue[3] = -60.0;
        hsl.hue[5] = 60.0;
    });
    let mut pairs: Vec<(&str, DynamicImage, DynamicImage)> = vec![
        ("hue-rotated", rotated_source.clone(), rotated),
        ("solved", engine_hsl_pair(15.0).0, engine_hsl_pair(15.0).1),
        ("over-cap", over_cap_band_pair().0, over_cap_band_pair().1),
        ("one-sided", one_sided_band_pair().0, one_sided_band_pair().1),
    ];
    let (cloud_src, cloud_tgt) = flat_sky_to_cloud_deck();
    pairs.push(("cloud-deck", cloud_src, cloud_tgt));
    let (perm_src, perm_tgt) = structural_permutation_pair();
    pairs.push(("permutation", perm_src, perm_tgt));
    for (name, src, tgt) in &pairs {
        for strength in [0.0f32, 0.65, 1.0] {
            let report = fit_recipe_from_with(
                src,
                tgt,
                &EditRecipe::default(),
                FitOptions {
                    strength: crate::recipe::GradeStrength::new(strength),
                    provider: None,
                },
            );
            assert_eq!(
                report.recipe.hsl.hue,
                [0.0f32; 8],
                "{name} at strength {strength} rotated a band: {:?}",
                report.recipe.hsl.hue
            );
        }
    }
}

/// (d) A move the frame ruler will not pay for is given back to zero and
/// DISCLOSED.
///
/// The cloud-deck pair is the honest fixture for this claim, and the
/// synthetic over-cap pair is not: on that one the whole fit terminally
/// resets, so a neutral mixer proves nothing about the mixer (measured —
/// disabling BOTH of 4a's do-no-harm arms left that test green). Here the
/// fit SUCCEEDS (the frame ends closer, no terminal reset) and the gate
/// ADMITS bands, so zero can only be 4a giving its own move back.
#[test]
fn a_per_band_move_the_frame_ruler_refuses_shrinks_to_zero_and_says_so() {
    let (src, tgt) = flat_sky_to_cloud_deck();
    let report = fit_recipe(&src, &tgt);
    assert!(
        report.err_after < report.err_before,
        "premise: the fit itself succeeds here ({:.4} -> {:.4})",
        report.err_before,
        report.err_after
    );
    assert!(
        !report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_REGRESSED),
        "premise: no terminal do-no-harm reset stands behind the neutral mixer"
    );
    let refused = note_arg(&report, crate::rationale::keys::FIT_NOTE_HSL_BANDS, "refused");
    let admitted = (0..EVIDENCE_HUE_BANDS)
        .filter(|&band| {
            let range = &report.evidence.hue[band];
            (range.source_populated || range.target_populated)
                && !refused.contains(range.label.as_str())
        })
        .count();
    assert!(admitted > 0, "premise: the population gate admitted a band: refused={refused:?}");
    assert!(
        report.recipe.hsl.is_neutral(),
        "the refused move must return to neutral: {:?}",
        report.recipe.hsl
    );
    assert!(
        hsl_note(&report, crate::rationale::keys::FIT_NOTE_HSL_WITHDRAWN_ERROR).is_some(),
        "…and say so: {}",
        report.recipe.rationale
    );
}

/// (d') The 4a' arbitration, pinned on a SYNTHETIC pair. Stage 4a judges
/// its mixer on a render the cast stage has not touched; the arbitration
/// after it takes that verdict again on the FINISHED frame, against the
/// same recipe with the mixer given back and the cast refitted on THAT
/// state. Until this test the only frame that reached it was the viaduct
/// crop out of the calibration corpus, so on a bare checkout the
/// arbitration was live and unwatched — deleting the loop left the whole
/// battery green.
///
/// `two_family_hsl_pair(50.0)` is the pin because the SAME frame gives
/// both answers, one strength apart. Measured 2026-09-02 by counting the
/// loop's iterations: at the lowest strength stage 4a hands the
/// arbitration a non-neutral mixer, the finished frame says it costs
/// something, and three halvings later it is gone — while the fit still
/// IMPROVES (0.0328 -> 0.0205), which is what distinguishes an
/// arbitrated withdrawal from the terminal do-no-harm reset. At full
/// strength the same frame keeps its mixer: the arbitration is reading
/// the state, not disliking the fixture.
#[test]
fn the_finished_frame_arbitration_withdraws_a_synthetic_mixer_that_costs_it() {
    let (src, tgt) = two_family_hsl_pair(50.0);
    let solve = |s: f32| {
        fit_recipe_from_with(
            &src,
            &tgt,
            &EditRecipe::default(),
            FitOptions { strength: crate::recipe::GradeStrength::new(s), provider: None },
        )
    };
    let low = solve(0.0);
    assert_eq!(low.mode, FitMode::Full, "premise: this pair is fitted as one frame");
    assert!(
        !low.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_REGRESSED),
        "premise: no terminal reset stands behind the neutral mixer"
    );
    assert!(
        low.err_after < low.err_before - 0.01,
        "premise: the fit itself succeeds here ({:.4} -> {:.4})",
        low.err_before,
        low.err_after
    );
    assert!(
        low.recipe.hsl.is_neutral(),
        "the finished frame says the mixer costs it, so it must go: {:?}",
        low.recipe.hsl
    );
    assert!(
        hsl_note(&low, crate::rationale::keys::FIT_NOTE_HSL_WITHDRAWN_ERROR).is_some(),
        "…and say so: {}",
        low.recipe.rationale
    );
    // The other answer, same frame: at full strength the mixer earns its
    // place on the finished frame and is delivered.
    let high = solve(1.0);
    assert_eq!(high.mode, FitMode::Full);
    assert!(
        !high.recipe.hsl.is_neutral(),
        "the arbitration reads the state, not the fixture: {:?}",
        high.recipe.hsl
    );
}

/// (e) The strength dial really turns this stage: the ceiling is monotone
/// across the three stops AND it binds at each of them.
#[test]
fn hsl_band_budget_is_monotone_across_the_three_strength_stops() {
    let cap = |s: f32| FitBudget::for_strength(crate::recipe::GradeStrength::new(s)).hsl_band;
    assert_eq!(cap(0.0), HSL_BAND_LIMIT_MIN);
    assert_eq!(cap(0.65), HSL_BAND_LIMIT_DEFAULT);
    assert_eq!(cap(1.0), HSL_BAND_LIMIT_MAX);
    assert!(cap(0.0) < cap(0.65) && cap(0.65) < cap(1.0));
    // …and it is a real constraint, not a number nothing reads: one pair
    // whose demand (+/-25) sits between the default ceiling and the full
    // one, solved at all three stops.
    let (src, tgt) = engine_hsl_pair(25.0);
    let solved = |s: f32| {
        fit_recipe_from_with(
            &src,
            &tgt,
            &EditRecipe::default(),
            FitOptions { strength: crate::recipe::GradeStrength::new(s), provider: None },
        )
        .recipe
        .hsl
        .saturation[5]
    };
    let (low, mid, high) = (solved(0.0), solved(0.65), solved(1.0));
    assert!(low <= cap(0.0) + 1e-3, "strength 0 must not exceed its ceiling: {low}");
    assert!(
        mid > cap(0.0) && mid <= cap(0.65) + 1e-3,
        "the default stop spends past the tight ceiling and stops at its own: {mid}"
    );
    assert!(high > cap(0.65), "strength 1 spends past the default ceiling: {high}");
}

/// v1.2.4 B1 — what the fan gate costs on a REAL two-temperature scene,
/// measured instead of assumed.
///
/// v1.2.3 shipped the gate with its cost measured only on synthetic
/// two-temperature frames (`two_temperature_coast`), and said so: on a
/// real photograph lit at two colour temperatures at once the gate could
/// in principle refuse a cast the photograph genuinely needs. Two such
/// photographs were then found in the user's own library by searching
/// 169 per-exemplar descriptions for mixed lighting and looking at the
/// finished renders: `p40`, a night street under neon and shop signs
/// beneath a fiery sunset sky, and `p41`, a building at twilight with a
/// warm horizon band, a deep blue sky and a warm lamp.
///
/// The measured answer (2026-09-02, both pairs at the shipped default)
/// is that the gate costs these pairs NOTHING, and the margin is the
/// interesting part: `p40`'s fitted cast reads 13.5° of added fan against
/// the 15° line — 1.5° of headroom, the closest any real pair in the
/// corpus comes — and ships ADMITTED as fitted, 0.0788 → 0.0335 at
/// confidence 0.612. `p41`'s reads 9.0°, and its cast is refused by a
/// different gate entirely (the pixel-aligned re-hue veto), so the fan
/// gate is not what withheld it there either.
///
/// WHICH fan this test reads, since 2026-09-21. Until then it read a
/// candidate it rebuilt itself (`cast_stage_candidate_from`), whose
/// channel curves come from the UNWEIGHTED [`residual_channel_curve`]
/// while the solve derives its own over the evidence × robust-pairing
/// weights. The two have never been one candidate on a real pair — the
/// rebuild read 13.5° where the gate itself read 11.0° — and a 2–3° proxy
/// gap is not a rounding error on a 15° line. The subject here is the
/// GATE'S verdict, so the pin is the gate's own reading, the one it
/// publishes in [`keys::FIT_NOTE_CAST_ADMITTED_FAN`] and the photographer
/// reads in the rationale.
///
/// Re-measured 2026-09-21, twice, because two fixes moved the frame this
/// solve reads: the calibration's base look began to be estimated against
/// the picture the camera drew (`render::camera_base_look` — the lens
/// profile's corner lift no longer enters the CDF match on a body whose
/// preview does not show it), and then the develop window moved onto the
/// DefaultCrop rectangle for a RAW that declares no active area
/// (`decode::aligned_demosaic_roi`), which puts every render of this body
/// on Lightroom's own frame instead of 32 px right and 20 px down of it.
///
/// The question this test asks keeps its answer, and on a shared frame it
/// is answered for one more pair than before. `p40` is unmoved: 11.0° of
/// added fan before the origin fix and 11.9° after, against the 15° line,
/// admitted both times, 0.0742 → 0.0338 at confidence 0.601. `p41` is
/// what moved: its cast used to be withheld — by neither the fan gate nor
/// the projection, both measured absent, and the 2026-09-02 record above
/// names the pixel-aligned re-hue veto — and on the shared frame it is
/// ADMITTED at 9.0°, its fit going 0.0745 → 0.0188 at confidence 0.498
/// where the same pair gave 0.0714 → 0.0581 at 0.370 one frame earlier.
/// The six corpus pairs' residuals moved both ways under the two fixes;
/// the ledger's Part 13 carries the table. The figures of 2026-09-02 stay
/// as the record of what the gate was shipped on.
///
/// Re-measured 2026-09-23 for the third calibration era: the base look
/// is now matched on 64-column block means of the two pictures
/// (`render::camera_base_knots`), not on their pixel distributions, so
/// the neutral frame this solve starts from carries a different curve on
/// both pairs. The question keeps its answer on both. `p40` is admitted
/// at 12.1° (11.9° before), its fit 0.0676 → 0.0319 (0.0742 → 0.0338)
/// at confidence 0.605 (0.601); `p41` is admitted at 10.3° (9.0°), its
/// fit 0.0802 → 0.0253 (0.0745 → 0.0188) at 0.496 (0.498). Both fans
/// stay under the 15° line by the margin the mutation below measures.
///
/// Re-measured 2026-10-23 for 2d0e750 (the cast hand-off of 8b4f4ec never
/// enters here: both as-fitted fans are under the limit). The question
/// keeps its answer on both: admitted, no refusal, no projection. `p40`
/// is admitted at 11.4° (11.8° at 2d0e750's parent); its tone is now the
/// paired robust arm, shipped ungated, where the parent fell to the luma
/// CDF (contrast +3.1 -> -21), which is that commit's disclosed cost on
/// the night street: fit 0.0676 -> 0.0410 (0.0319), confidence 0.556
/// (0.604). `p41` now ships a white balance, 3800 K / tint -100, where the
/// parent shipped none — the consensus read with `WB_OPPOSING_RATIO` 0.20
/// no longer lets a sliver of opposing range veto it — so the curves are
/// left less to carry: fan 4.0° (10.2°), fit 0.0800 -> 0.0282 (0.0800 ->
/// 0.0251), confidence 0.489 (0.496).
///
/// Both pairs are OPTIONAL, like every other corpus pair: absent, the
/// test says so and passes, and the synthetic two-temperature fixture
/// carries the refusal side of the same question on its own.
///
/// Mutation: halve [`FAN_DEG`] and `p40` goes from admitted to convicted,
/// red on the fan-margin assertion.
#[test]
fn the_fan_gate_costs_a_real_two_temperature_pair_nothing() {
    use crate::rationale::keys;
    let Some(root) = calibration_corpus() else { return };
    for (code, want_fan, want_before, want_after, want_conf) in [
        ("p40", 11.4f32, 0.0676f32, 0.0410f32, 0.556f32),
        ("p41", 4.0, 0.0800, 0.0282, 0.489),
    ] {
        let raw = root.join(format!("{code}.arw"));
        let target_path = root.join(format!("{code}-target.jpg"));
        if !raw.is_file() || !target_path.is_file() {
            crate::test_skipped(
                "the_fan_gate_costs_a_real_two_temperature_pair_nothing",
                &format!("the two-temperature pair {code} is not in this corpus"),
            );
            continue;
        }
        // Loaded exactly as CLI `match` loads a RAW: the frame developed
        // at the default recipe on a 2048 edge, against the calibration
        // recipe as the base.
        let src = render::render_to_image(&raw, &EditRecipe::default(), None, Some(2048))
            .expect("develop the two-temperature RAW");
        let tgt = image::open(target_path).expect("the finished rendition");
        let base = crate::pipeline::calibration_recipe(crate::pipeline::fit_calibration(&raw));
        let report = fit_recipe_from(&src, &tgt, &base);
        // The cast SHIPS — which is the whole claim, and what makes the
        // fan reading below exist: the gate publishes what it passed on
        // only when it admits.
        assert!(
            report.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED),
            "{code}: this pair's cast is the thing the gate is supposed to cost nothing, and it \
                 did not ship: {}",
            report.recipe.rationale
        );
        let fan = projection_arg(&report, keys::FIT_NOTE_CAST_ADMITTED_FAN, "fan")
            .expect("an admitted cast on a two-temperature frame publishes the fan it passed on");
        eprintln!(
            "TWO_TEMPERATURE {code} gate_fan={fan:.1} err={:.6}->{:.6} conf={:.6}",
            report.err_before, report.err_after, report.recipe.confidence
        );
        assert!(
            (fan - want_fan).abs() <= 0.6,
            "{code}: the cast's published fan moved off the measured {want_fan}°: {fan:.1}°"
        );
        assert!(
            fan < FAN_DEG,
            "{code}: the fan gate now convicts a real two-temperature pair ({fan:.1}° \
                 against {FAN_DEG}°) — that is the cost this test exists to measure"
        );
        assert!(
            !report.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_HUE_FANNED
                || n.key == keys::FIT_NOTE_CAST_PROJECTED),
            "{code}: …so neither the refusal nor the projection may appear: {}",
            report.recipe.rationale
        );
        assert!(
            (report.err_before - want_before).abs() <= 0.002
                && (report.err_after - want_after).abs() <= 0.002,
            "{code}: residual moved off the measured {want_before} -> {want_after}: {:.6} -> {:.6}",
            report.err_before,
            report.err_after
        );
        assert!(
            (report.recipe.confidence - want_conf).abs() <= 0.02,
            "{code}: confidence moved off the measured {want_conf}: {}",
            report.recipe.confidence
        );
    }
}

/// A frame whose only colour evidence is a per-band HUE gap: one field
/// rotated, its chroma and luminance untouched.
///
/// The rest of the frame is a neutral luminance ramp, so the joint
/// LUMINANCE x CHROMA buckets are identical between the two builds and the
/// only thing that moved is where the coloured field sits on the hue
/// circle.
fn band_rotation_frame(hue: f32) -> DynamicImage {
    let (w, h) = (192u32, 128u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let level = 0.20 + 0.60 * y as f32 / (h - 1) as f32;
            let p = if x < w - w / 8 {
                [level, level, level]
            } else {
                // HSL -> RGB, written out because the engine's own
                // converter is private to `render` and this fixture wants
                // one hue at a fixed chroma and luminance.
                let s = 0.22f32;
                let c = (1.0 - (2.0 * level - 1.0).abs()) * s;
                let hp = hue.rem_euclid(360.0) / 60.0;
                let xx = c * (1.0 - (hp % 2.0 - 1.0).abs());
                let (r, g, b) = match hp as u32 {
                    0 => (c, xx, 0.0),
                    1 => (xx, c, 0.0),
                    2 => (0.0, c, xx),
                    3 => (0.0, xx, c),
                    4 => (xx, 0.0, c),
                    _ => (c, 0.0, xx),
                };
                let m = level - c / 2.0;
                [r + m, g + m, b + m]
            };
            img.put_pixel(
                x,
                y,
                image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)),
            );
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// v1.2.4 A20 — the BAND-CENTROID arm of `residual_is_colour_shaped`,
/// tested on its own.
///
/// The function has two arms and only one of them had a fixture. The
/// chromatic-bucket arm answers "a value range of this frame's colour is
/// far off and the neutral ranges at the same brightness are not"; the
/// band-centroid arm answers "a populated hue band's centroid is more
/// than [`UNREPRESENTED_HUE_DEG`] away from the target's", which is the
/// one axis the per-band mixer is forbidden to solve. A residual made
/// ONLY of a hue rotation leaves the first arm silent — rotating a hue
/// changes neither the luminance nor the chroma distribution the joint
/// buckets are built from — so this pair separates them: 30 degrees of
/// Blue-band rotation, every chromatic bucket well under
/// [`UNREPRESENTED_CHROMATIC_ERR`], and the disclosure still names `hsl`.
///
/// Mutation: raise [`UNREPRESENTED_HUE_DEG`] to 200 (past anything a hue
/// circle can produce) and this goes red on the colour-shape assertion,
/// with the bucket-arm premise still passing — which is what proves the
/// centroid arm is what was speaking.
#[test]
fn a_band_centroid_gap_alone_makes_the_residual_colour_shaped() {
    let after = band_rotation_frame(215.0);
    let target = band_rotation_frame(255.0);
    let (a_img, t_img) = analysis_pair(&after, &target);
    let after_px = pixels_of(&a_img);
    let tp = pixels_of(&t_img);
    let evidence = evidence_model_for(&after_px, &tp, a_img.width(), a_img.height());

    // PREMISE — the chromatic-BUCKET arm is silent: only the hue moved,
    // so the luminance x chroma distributions are the same on both sides.
    let buckets = crate::fit_zoned::joint_buckets_with_evidence(
        &after_px,
        &tp,
        Some(&evidence.source_weights),
        Some(&evidence.target_weights),
    );
    let worst_of = |chromatic: bool| {
        buckets
            .iter()
            .filter(|b| b.chromatic == chromatic)
            .map(|b| b.err)
            .fold(0.0f32, f32::max)
    };
    let (chromatic_worst, neutral_worst) = (worst_of(true), worst_of(false));
    eprintln!(
        "BAND_CENTROID chromatic_worst={chromatic_worst:.4} neutral_worst={neutral_worst:.4}"
    );
    assert!(
        chromatic_worst < UNREPRESENTED_CHROMATIC_ERR
            || chromatic_worst < neutral_worst + UNREPRESENTED_CHROMATIC_LEAD,
        "premise: the bucket arm must be silent here ({chromatic_worst:.4} vs \
             {neutral_worst:.4})"
    );

    // …and the CENTROID arm is what speaks.
    assert!(
        residual_is_colour_shaped(&after_px, &tp, &evidence),
        "a populated band's centroid {UNREPRESENTED_HUE_DEG}° off is a per-band colour job"
    );
    let note = unrepresented_note(
        &EditRecipe::default(),
        &after_px,
        &tp,
        0.10,
        FitMode::Full,
        &evidence,
    )
    .expect("a colour-shaped residual above the floor is disclosed");
    let controls = note
        .args
        .iter()
        .find(|(k, _)| *k == "controls")
        .map(|(_, v)| v.as_str())
        .unwrap_or_default();
    assert!(
        controls.contains("hsl"),
        "…and the disclosure names the axis the mixer cannot reach: {controls:?}"
    );
}

/// v1.2.4 A7 — the PROJECTION's two abstaining clauses, rendered and
/// asserted rather than merely reachable in principle.
///
/// A projected cast discloses at least what an admitted one does, and two
/// of those readings can decline to answer: `foreign` when the target
/// carries no hue evidence to be foreign to, `fan` when no hue class is
/// region-sized across two luma slices. Both abstentions had keys and
/// translations and neither had ever been produced by a fixture, so
/// nothing in the tree said what they print — and the failure mode they
/// exist to prevent is a digit: `0.000` for a measurement never taken.
///
/// The abstention itself is read off a FIXTURE rather than hand-written:
/// the same colourless frame the admission's abstention test uses, put
/// through the two censuses the projection reads, so "no region-sized
/// class / no foreign class" is a measurement here and not an assumption.
/// The clauses are then built from those two `None`s.
///
/// Mutation: make `cast_projection_notes` fall back to
/// `Some(0.0)` for either reading and the digit assertions go red.
#[test]
fn an_unmeasured_projected_reading_says_so_instead_of_printing_a_zero() {
    use crate::rationale::keys;
    // 1) The PLUMBING, measured on a frame with no chromatic mass at all.
    let grey = DynamicImage::ImageRgb8(RgbImage::from_fn(48, 48, |x, y| {
        let v = (24 + ((x * 3 + y * 2) % 200)) as u8;
        image::Rgb([v, v, v])
    }));
    let (s, t) = analysis_pair(&grey, &grey);
    let tp = pixels_of(&t);
    let cur = pixels_of(&render::develop_preview(&s, &EditRecipe::default()));
    let evidence = evidence_model(&cur, &tp);
    assert_eq!(
        foreign_hue_bins_weighted(&tp, &evidence.target_hue_weights),
        None,
        "a colourless target gives the foreign-hue census nothing to be foreign to"
    );
    assert_eq!(
        hue_fan_weighted(&cur, &cur, &evidence),
        None,
        "…and no hue class is region-sized across two luma slices"
    );

    // 2) The DISCLOSURE: a projection carrying both abstentions writes
    //    the not-measurable clauses, in the order the admission's are in,
    //    and neither prints a number.
    let abstained = cast_projection_notes(CastProjection {
        share: 0.917,
        fan_before: 37.6,
        t: 0.363,
        fan_after: None,
        ratio: 0.525,
        bound: CAST_ACCEPT_RATIO,
        rehued: 0.0,
        foreign: None,
    });
    let keys_of: Vec<&str> = abstained.iter().map(|n| n.key).collect();
    assert_eq!(
        keys_of,
        vec![
            keys::FIT_NOTE_CAST_PROJECTED,
            keys::FIT_NOTE_CAST_ADMITTED_FOREIGN_NA,
            keys::FIT_NOTE_CAST_PROJECTED_FAN_NA,
        ],
        "a projection that could not measure either reading still says three things"
    );
    for note in &abstained[1..] {
        assert!(note.args.is_empty(), "a not-measurable clause carries no reading");
        let text = crate::rationale::render_one(note);
        assert!(
            !text.chars().any(|c| c.is_ascii_digit()),
            "an unmeasured reading must not print a number: {text}"
        );
        assert!(
            text.contains("not measurable"),
            "an unmeasured reading must SAY it was not measured: {text}"
        );
    }

    // 3) …and the same abstention on the SEARCH side, where it has to
    //    mean the opposite of a refusal: a census with no opinion cannot
    //    put a candidate over the projection target, so the shrink is
    //    admissible and the pair is rescued rather than refused for want
    //    of a reading.
    let judged = search_cast_projection_t(0.05, |t| CastOutcome {
        readings: Some(CastReadings {
            ratio: 1.0 - 0.1 * t,
            bound: CAST_ACCEPT_RATIO,
            foreign: None,
            rehued: 0.0,
            fan: None,
        }),
        ..CastOutcome::default()
    });
    assert!(
        judged.is_some(),
        "an abstaining fan census must CLEAR the projection target, not fail it"
    );
}

/// v1.2.4 A39 — the TERMINAL delivered-fan check, withdrawing arm.
///
/// The fan gate is a calibration applied to one stage's candidate, and the
/// 4b do-no-harm loop re-fits that stage after every saturation step; the
/// FAN_DEG = 20 experiment showed the loop walking to a 19° cast that
/// shipped and left 20.6° in the delivered sky. This is the structural
/// re-read that closes that: the same census, on the finished render
/// against the untouched base.
///
/// The subject is the coast pair's cast AS FITTED — the curves the gate
/// convicts (36.3° as a candidate since 2026-10-22) and that therefore
/// never ship — put into the recipe by hand, which is exactly the state a loop that walked around the gate
/// would hand over.
///
/// Mutation: widen `delivered_fan_conviction`'s test to `fan > 4.0 *
/// FAN_DEG` and this goes red on its first assertion.
#[test]
fn the_terminal_check_takes_the_curves_out_of_a_fanning_render() {
    let (src, tgt) = (coast(false), coast(true));
    let (s_img, t_img) = analysis_pair(&src, &tgt);
    let base = EditRecipe::default();
    let sp = pixels_of(&render::develop_preview(&s_img, &base));
    let tp = pixels_of(&t_img);
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let mut recipe = cast_stage_candidate(&src, &tgt).with;
    assert!(!recipe.red_curve.is_empty(), "premise: the fixture demanded a cast");
    let mut end_px = pixels_of(&render::develop_preview(&s_img, &recipe));
    let (share, fan) = delivered_fan_conviction(&sp, &end_px, &evidence)
        .expect("premise: the as-fitted cast fans the delivered render past the limit");
    eprintln!("TERMINAL_FAN as-fitted share={share:.3} fan={fan:.1}");
    assert!(fan > FAN_DEG, "premise: {fan:.1}° must be over the {FAN_DEG}° line");

    let acted = withdraw_curves_for_delivered_fan(
        &s_img,
        &sp,
        &evidence,
        &mut recipe,
        &mut end_px,
    )
    .expect("a convicted delivered render must be acted on");
    assert_eq!(acted.2, None, "withdrawing the curves must clear the reading");
    assert!(
        recipe.red_curve.is_empty()
            && recipe.green_curve.is_empty()
            && recipe.blue_curve.is_empty(),
        "…by taking the three channel curves out of the recipe"
    );
    assert_eq!(
        delivered_fan_conviction(&sp, &end_px, &evidence),
        None,
        "…and the render the caller keeps must be the one that clears"
    );
}

/// …and the other arm: a delivered fan the curves did not cause is
/// DISCLOSED, not paid for.
///
/// Withdrawing a control that is not the cause would cost the user look
/// error for nothing, so when the reading survives the withdrawal the
/// curves go back exactly as they were and the numbers are published
/// instead. The case is not hypothetical: `p36` delivers 12.9° of added
/// fan carrying no cast curves at all. Here it is driven at full size by
/// a colour-grade split — shadows and highlights sent to opposite hues,
/// which is a per-luminance hue move by construction — and the recipe
/// DOES carry channel curves, deliberately: one shape shared by all three
/// channels, the achromatic half of the projection path, which moves
/// contrast and cannot sort a hue class by luminance. Without them the
/// arm would be vacuous — withdrawing nothing from a recipe that has
/// nothing changes nothing, and the mutation below could not fail.
///
/// Mutation: move `*recipe = without; *end_px = px;` out of the
/// `still.is_none()` guard so the withdrawal is unconditional, and this
/// goes red on the "unchanged" assertion.
#[test]
fn a_delivered_fan_the_curves_did_not_cause_is_disclosed_not_withdrawn() {
    let (src, tgt) = (coast(false), coast(true));
    let (s_img, t_img) = analysis_pair(&src, &tgt);
    let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
    let tp = pixels_of(&t_img);
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let mut recipe = EditRecipe::default();
    recipe.color_grade.shadow_hue = 30.0;
    recipe.color_grade.shadow_sat = 100.0;
    recipe.color_grade.highlight_hue = 210.0;
    recipe.color_grade.highlight_sat = 100.0;
    recipe.color_grade.blending = 0.0;
    let shared = vec![
        crate::recipe::CurvePoint { input: 0, output: 0 },
        crate::recipe::CurvePoint { input: 128, output: 134 },
        crate::recipe::CurvePoint { input: 255, output: 255 },
    ];
    recipe.red_curve.clone_from(&shared);
    recipe.green_curve.clone_from(&shared);
    recipe.blue_curve = shared;
    let before = recipe.clone();
    let mut end_px = pixels_of(&render::develop_preview(&s_img, &recipe));
    let (share, fan) = delivered_fan_conviction(&sp, &end_px, &evidence)
        .expect("premise: the colour-grade split fans the render past the limit");
    eprintln!("TERMINAL_FAN uncaused share={share:.3} fan={fan:.1}");

    let (_, reported, still) = withdraw_curves_for_delivered_fan(
        &s_img,
        &sp,
        &evidence,
        &mut recipe,
        &mut end_px,
    )
    .expect("a convicted delivered render must be acted on");
    assert_eq!(reported, fan, "the disclosure reports the reading it convicted on");
    let still = still.expect("no curve was the cause, so the reading must survive");
    assert!(
        still > FAN_DEG,
        "…and it must still be over the line to be reported ({still:.1})"
    );
    assert!(
        !recipe.red_curve.is_empty(),
        "premise: there were curves to withdraw, so keeping them means something"
    );
    assert_eq!(recipe, before, "the recipe must come back UNCHANGED");
    assert_eq!(
        delivered_fan_conviction(&sp, &end_px, &evidence).map(|(_, f)| f),
        Some(fan),
        "…and so must the render the caller keeps"
    );
}

/// The standing margin the check leaves, asserted so a change that eats it
/// says so here rather than by silently withdrawing a fit's cast curves.
///
/// Measured 2026-09-02 across the whole library battery: 108 finished
/// Full-mode renders, the widest delivered fan among them the coast
/// fixture's 14.2° against the 15° line. This walks the fixtures that
/// carry a real cast and pins both halves — every one clears, and the
/// worst is close enough to the line to be worth reading.
///
/// 2026-10-22: the worst is now the cornwall panel's 12.9°; the coast pair
/// read 12.4° under the ungated tone arm and 7.6° once re-drawn (see
/// [`coast`]).
///
/// Mutation: set `FAN_PROJECT_DEG` to `FAN_DEG` (let the projection keep
/// twice the fan it is allowed) and the coast pair's delivered reading
/// goes over the line, red here. (2026-10-22, run: the re-drawn coast stays
/// at 7.6° and the two-temperature coast climbs 0.3° → 13.9°, off the pin.)
#[test]
fn no_shipped_fit_delivers_a_hue_fan_past_the_limit() {
    let panel_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
    let panel = image::open(panel_root.join("showcase-cornwall-reverse-fit.jpg")).unwrap();
    let (hz_src, hz_tgt) = haze_pair();
    let (tf_src, tf_tgt) = two_family_hsl_pair(50.0);
    let pairs: Vec<(&str, DynamicImage, DynamicImage)> = vec![
        ("coast", coast(false), coast(true)),
        ("two-temperature coast", coast(false), two_temperature_coast()),
        ("haze", hz_src, hz_tgt),
        ("two-family hsl", tf_src, tf_tgt),
        (
            "cornwall panel",
            panel.crop_imm(0, 136, 532, 356),
            panel.crop_imm(535, 136, 530, 356),
        ),
    ];
    let mut worst = 0.0f32;
    for (name, src, tgt) in &pairs {
        let report = fit_recipe(src, tgt);
        let (s_img, t_img) = analysis_pair(src, tgt);
        let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
        let tp = pixels_of(&t_img);
        let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
        let px = pixels_of(&render::develop_preview(&s_img, &report.recipe));
        let reading = hue_fan_weighted(&sp, &px, &evidence);
        eprintln!("DELIVERED_FAN {name} {reading:?}");
        assert_eq!(
            delivered_fan_conviction(&sp, &px, &evidence),
            None,
            "{name} delivered a hue fan past {FAN_DEG}°: {reading:?}"
        );
        if let Some((_, fan, _)) = reading {
            worst = worst.max(fan);
        }
    }
    assert!(
        (worst - 12.9).abs() < 0.5,
        "the worst delivered fan moved off the measured 12.9°: {worst:.1}"
    );
}

/// (f) Once the mixer has closed a band gap, the residual the
/// unrepresented-controls disclosure reads no longer has the shape of a
/// per-band colour job — and while the gap is only PARTLY closed it still
/// does, and the disclosure still names `hsl`. Both halves on one pair,
/// separated only by the budget.
#[test]
fn solving_the_bands_takes_the_colour_shape_out_of_the_residual() {
    let (src, tgt) = two_family_hsl_pair(50.0);
    let (s_img, t_img) = analysis_pair(&src, &tgt);
    let base = EditRecipe::default();
    let sp = pixels_of(&render::develop_preview(&s_img, &base));
    let tp = pixels_of(&t_img);
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let fit = |s: f32| {
        fit_recipe_from_with(
            &src,
            &tgt,
            &base,
            FitOptions { strength: crate::recipe::GradeStrength::new(s), provider: None },
        )
    };

    // Default budget: +/-18 against a +/-50 demand. The residual is still
    // a per-band colour job, and the disclosure says so.
    let partial = fit(0.65);
    let partial_px = pixels_of(&render::develop_preview(&s_img, &partial.recipe));
    assert!(!partial.recipe.hsl.is_neutral(), "premise: the mixer did attach here");
    assert!(
        residual_is_colour_shaped(&partial_px, &tp, &evidence),
        "a HALF-closed band gap is still a per-band colour residual"
    );
    // …and whether the disclosure NAMES it is a second question, decided
    // by the residual's SIZE rather than its shape, and on this pair the
    // v1.2.4 projection sweep moved it. Measured 2026-09-02 on this tree:
    // the cast the fan gate convicts at 15.5° is now shrunk to t = 0.318,
    // whose look-error ratio is 0.885 — a gain of 0.0033, nearly twice
    // `FIT_QUANT` — which takes the finished residual 0.032837 → 0.022693,
    // under the `FIT_QUANT_CLEAN` = 0.025 floor at which
    // `unrepresented_note` returns early because there is nothing left to
    // explain. So this pair ships a better fit and a shorter rationale,
    // and both halves are asserted here with their numbers so neither can
    // move in silence. The `hsl` contract itself is not weakened — it is
    // asserted below on a WIDER gap, where the same half-closed shape
    // survives at a residual the floor does not silence.
    //
    // 2026-10-22: the floor is now reached by the TONE sliders, not by the
    // projection. The engine's saturation mixer darkens every pixel of this
    // target (mean luma change −0.0235, equal to the mean absolute change),
    // which the ungated paired tone arm reads and answers (exposure −0.25,
    // contrast +3.1, shadows +3.6); the cast solved after it fans 17.8° and
    // no point of the projection path pays, so it is refused, and the
    // finished residual is 0.0231. A luma-kept target removes the darkening
    // and with it the whole fit: tone withheld, every saturation level
    // rendered farther than the source, neutral recipe (0.0115 → 0.0115),
    // mixer given back — the premise above fails — so the engine-developed
    // pair stays.
    assert!(
        (partial.err_after - 0.0231).abs() < 0.001,
        "the partial fit's finished residual moved off the measured 0.0231: {}",
        partial.err_after
    );
    assert!(
        partial.err_after < FIT_QUANT_CLEAN,
        "…which is what puts it UNDER the disclosure floor {FIT_QUANT_CLEAN}: {}",
        partial.err_after
    );
    assert!(
        partial.recipe.exposure_ev < 0.0
            && partial.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_CAST_HUE_FANNED)
            && partial.recipe.red_curve.is_empty(),
        "…and the tone sliders, not a cast the fan gate refused, are what bought that: {}",
        partial.recipe.rationale
    );
    assert!(
        !names_hsl(&partial),
        "…so the residual is below the size at which the disclosure speaks: {}",
        partial.recipe.rationale
    );

    // The `hsl` half of the contract, on a gap +/-18 cannot come close to
    // closing: +/-80 demanded. The mixer still attaches, the residual is
    // still a per-band colour job, and at 0.0601 it is well clear of the
    // floor — so the sentence that CAST-2's fix-up put back is still
    // asserted in the tree, on the pair that can carry it.
    let (wide_src, wide_tgt) = two_family_hsl_pair(80.0);
    let wide = fit_recipe_from_with(
        &wide_src,
        &wide_tgt,
        &base,
        FitOptions { strength: crate::recipe::GradeStrength::new(0.65), provider: None },
    );
    let (wide_s_img, wide_t_img) = analysis_pair(&wide_src, &wide_tgt);
    let wide_px = pixels_of(&render::develop_preview(&wide_s_img, &wide.recipe));
    let wide_tp = pixels_of(&wide_t_img);
    let wide_evidence = evidence_model_for(
        &pixels_of(&render::develop_preview(&wide_s_img, &base)),
        &wide_tp,
        wide_s_img.width(),
        wide_s_img.height(),
    );
    assert!(!wide.recipe.hsl.is_neutral(), "premise: the mixer attaches on the wide gap too");
    assert!(
        (wide.err_after - 0.0601).abs() < 0.002,
        "the wide pair's residual moved off the measured 0.0601: {}",
        wide.err_after
    );
    assert!(
        wide.err_after > FIT_QUANT_CLEAN,
        "…and it must stay ABOVE the disclosure floor {FIT_QUANT_CLEAN}: {}",
        wide.err_after
    );
    assert!(
        residual_is_colour_shaped(&wide_px, &wide_tp, &wide_evidence),
        "a gap this far past the ceiling is still a per-band colour residual"
    );
    assert!(
        names_hsl(&wide),
        "…so the disclosure must still name it: {}",
        wide.recipe.rationale
    );

    // Strength 1: the ceiling now covers the demand. The counterfactual is
    // the SAME finished recipe with the mixer zeroed, so the flip is
    // attributable to this stage and to nothing else in the pipeline.
    let full = fit(1.0);
    let full_px = pixels_of(&render::develop_preview(&s_img, &full.recipe));
    let mut without = full.recipe.clone();
    without.hsl = crate::recipe::Hsl::default();
    let without_px = pixels_of(&render::develop_preview(&s_img, &without));
    assert!(
        residual_is_colour_shaped(&without_px, &tp, &evidence),
        "premise: without the mixer this fit still leaves a per-band colour residual"
    );
    assert!(
        !residual_is_colour_shaped(&full_px, &tp, &evidence),
        "…and the mixer is what takes that shape out of it"
    );
    assert!(!names_hsl(&full), "…so `hsl` is no longer named: {}", full.recipe.rationale);
}

/// …and the other half of the same contract, on a pair whose colour
/// residual is a per-band HUE rotation: that axis is never solved, so the
/// disclosure must go on naming `hsl`. Closing the saturation gap must
/// never be allowed to launder a rotation into silence.
#[test]
fn a_residual_the_mixer_cannot_reach_is_still_named() {
    let (src, tgt) = flat_sky_to_cloud_deck();
    let report = fit_recipe(&src, &tgt);
    assert_eq!(report.mode, FitMode::Atmosphere, "premise: the cloud deck is content-divergent");
    assert!(names_hsl(&report), "the unreachable residual stays disclosed: {}", report.recipe.rationale);
}

// --- cast→WB hand-off (2026-10-23) -----------------------------------------

/// Five hue bands — grey, orange, blue, green, magenta — each a luminance
/// ramp along x (0.08 → 0.92), the cast→WB hand-off's source.
fn hue_ramps() -> DynamicImage {
    let (w, h) = (192u32, 160u32);
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let l = 0.08 + 0.84 * x as f32 / (w - 1) as f32;
            let p = match y * 5 / h {
                0 => [l * 0.9, l * 0.9, l * 0.9],
                1 => [l, l * 0.6, l * 0.25],
                2 => [l * 0.3, l * 0.6, l],
                3 => [l * 0.35, l * 0.8, l * 0.4],
                _ => [l, l * 0.25, l * 0.45],
            };
            img.put_pixel(x, y, image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)));
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// The hand-off's target: [`hue_ramps`] under a 3500 K white balance (the
/// anchor is 5500 K), with its top `warm_rows` rows — part of the grey band —
/// under 25000 K instead. A cool frame whose grey strip went warm.
fn cooled_ramps(warm_rows: u32) -> DynamicImage {
    let src = hue_ramps();
    let cool = render::develop_preview(
        &src,
        &EditRecipe { temperature_k: Some(3500.0), ..Default::default() },
    )
    .to_rgb8();
    let warm = render::develop_preview(
        &src,
        &EditRecipe { temperature_k: Some(25000.0), ..Default::default() },
    )
    .to_rgb8();
    DynamicImage::ImageRgb8(RgbImage::from_fn(192, 160, |x, y| {
        if y < warm_rows { *warm.get_pixel(x, y) } else { *cool.get_pixel(x, y) }
    }))
}

/// FNV-1a 64 of a recipe's JSON — a by-value pin of the whole recipe,
/// rationale included, in sixteen hex digits.
fn recipe_fnv(recipe: &EditRecipe) -> u64 {
    let json = serde_json::to_string(recipe).expect("a recipe serialises");
    json.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// The hand-off ships. [`cooled_ramps`]`(0)` asks for a 3500 K white
/// balance against the 5500 K anchor, out of the default budget, so the
/// white-balance stage leaves as-shot and the three channel curves are asked
/// for the whole cast — and fan a hue class past the limit, so before the
/// hand-off the projection shrank them. The hand-off walks the white balance
/// toward the cast inside the budget and ships the curves re-fitted on it at
/// full strength. Measured 2026-10-23: 5500 K / 0 → 3900 K / +40.6, curves
/// re-fitted reading +3.8°, cells 1.000 converged / 0.000 diverged.
#[test]
fn a_fan_the_white_balance_can_take_back_is_handed_to_it() {
    use crate::rationale::keys;
    let rep = fit_recipe(&hue_ramps(), &cooled_ramps(0));
    let note = rep
        .notes
        .iter()
        .find(|n| n.key == keys::FIT_NOTE_CAST_HANDED_TO_WB)
        .unwrap_or_else(|| panic!("the cast must be handed to the white balance: {}", rep.recipe.rationale));
    let arg = |name: &str| -> f32 {
        note.args
            .iter()
            .find(|(k, _)| *k == name)
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or_else(|| panic!("the note carries {name}"))
    };
    // The white balance moved, toward the cast, and the note says from where
    // to where — the shipped Kelvin, not a probe's.
    let k = rep.recipe.temperature_k.expect("the hand-off ships a white balance");
    assert_eq!(arg("k_before"), 5500.0, "it starts from the as-shot anchor");
    assert_eq!(arg("k_after"), k, "the note names the Kelvin that shipped");
    assert_eq!(arg("tint_after"), rep.recipe.tint, "…and the tint");
    assert!(k < 5500.0, "a 3500 K target asks for a cooler white balance ({k} K)");
    // The conviction it answered, and the reading that let it ship.
    assert!(arg("fan_before") >= FAN_DEG, "the as-fitted fan convicted ({})", arg("fan_before"));
    assert!(arg("fan_after").abs() < FAN_DEG, "the re-fitted curves clear the limit ({})", arg("fan_after"));
    assert_eq!(arg("limit"), FAN_DEG);
    // `HandOffFacts.share` is the CONVICTED class's share of the census:
    // region-sized by the fan gate's own floor, and a share.
    let share = arg("share");
    assert!((FAN_SHARE..=1.0).contains(&share), "the class share is a fan-gate class ({share})");
    // The cell verdict it was admitted on is a vouch.
    assert!(arg("converged") >= 0.70 && arg("diverged") <= 0.10 && arg("aligned") >= 0.70);
    // Curves at t = 1: they ship as an ADMITTED cast, never a projected one.
    assert!(
        !rep.recipe.red_curve.is_empty() || !rep.recipe.blue_curve.is_empty(),
        "the re-fitted curves ship: {}",
        rep.recipe.rationale
    );
    assert!(
        !rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_PROJECTED),
        "a handed cast is not also projected: {}",
        rep.recipe.rationale
    );
    assert!(
        rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_ADMITTED),
        "the re-fitted curves go through the ordinary admission: {}",
        rep.recipe.rationale
    );
    // 3900 K sits inside the domain, so no bound sentence.
    assert!(!rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_WB_SEARCH_BOUND));
    assert!(rep.err_after < rep.err_before, "{} -> {}", rep.err_before, rep.err_after);
}

/// The same frame with eleven rows of its grey band warmed instead: every
/// white balance the hand-off can reach inside the budget clears all four
/// cast gates, and the target's own cells refuse each of them (measured
/// 2026-10-23: 0.111 and 0.122 of the frame moved away, the line is 0.10),
/// so the stage projects exactly as it did before the hand-off existed.
/// Pinned BY VALUE: the recipe's JSON hash, read at 9631905 before any of
/// the hand-off was written.
#[test]
fn a_hand_off_the_cells_refuse_leaves_the_projection_as_it_was() {
    use crate::rationale::keys;
    let rep = fit_recipe(&hue_ramps(), &cooled_ramps(11));
    assert!(
        !rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_HANDED_TO_WB),
        "the cells refused it: {}",
        rep.recipe.rationale
    );
    assert!(
        rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_PROJECTED),
        "premise: this pair is on the projection path: {}",
        rep.recipe.rationale
    );
    assert_eq!(rep.recipe.temperature_k, None, "the white balance stays as-shot");
    assert_eq!(
        recipe_fnv(&rep.recipe),
        0x1fb7_ceb6_9005_bd5d,
        "the projection path's recipe moved: {}",
        rep.recipe.rationale
    );
}

/// A cast the fan gate never convicts never reaches the hand-off: the haze
/// correction, the fan gate's own legitimate case, pinned by value at
/// 9631905.
#[test]
fn a_cast_with_no_fan_never_enters_the_hand_off() {
    use crate::rationale::keys;
    let clean = synth();
    let mut haze = EditRecipe {
        exposure_ev: -0.3,
        contrast: -45.0,
        blacks: 40.0,
        saturation: -40.0,
        blue_curve: vec![
            CurvePoint { input: 0, output: 25 },
            CurvePoint { input: 128, output: 132 },
            CurvePoint { input: 255, output: 255 },
        ],
        ..Default::default()
    };
    haze.clamp();
    let rep = fit_recipe(&render::develop_preview(&clean, &haze), &clean);
    assert!(!rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_HANDED_TO_WB));
    assert_eq!(
        recipe_fnv(&rep.recipe),
        0x732f_f6a1_d6b6_ea19,
        "a recipe the hand-off never touched moved: {}",
        rep.recipe.rationale
    );
}

/// `pull`, as the hand-off measures it: the white-balance estimator read on
/// the curves' own move. A move that changes no pixel's colour reads no
/// direction at all; a pure per-channel gain reads that gain's chromaticity.
#[test]
fn the_hand_offs_pull_reads_a_white_balance_move_and_nothing_else() {
    let cur: Vec<[f32; 3]> = (0..400)
        .map(|i| {
            let l = 0.1 + 0.8 * (i % 20) as f32 / 19.0;
            match i / 100 {
                0 => [l, l, l],
                1 => [l, 0.6 * l, 0.3 * l],
                2 => [0.3 * l, 0.6 * l, l],
                _ => [0.4 * l, l, 0.5 * l],
            }
        })
        .collect();
    let weights = vec![1.0f32; cur.len()];
    assert_eq!(cast_wb_direction(&cur, &cur, &weights, 5500.0), None, "no move, no pull");
    // Linear gains R 1.2, B 0.8: a warm white balance.
    let gain = [1.2f32, 1.0, 0.8];
    let warmed: Vec<[f32; 3]> = cur
        .iter()
        .map(|p| {
            std::array::from_fn(|c| {
                render::linear_to_srgb((render::srgb_to_linear(p[c]) * gain[c]).min(1.0))
            })
        })
        .collect();
    let d = cast_wb_direction(&cur, &warmed, &weights, 5500.0).expect("a gain is a direction");
    let red_blue = d[0] - d[2];
    assert!(
        (red_blue - (1.2f32 / 0.8).ln()).abs() < 0.05,
        "the red/blue part is the gain's own ({red_blue:.3} against {:.3})",
        (1.2f32 / 0.8).ln()
    );
    assert!(d.iter().sum::<f32>().abs() < 1e-3, "chromaticity only: unit geometric mean ({d:?})");
}

/// The selection rule: the look error decides, `pull` only breaks ties
/// inside FIT_QUANT of the best, and the winner must still beat the frame
/// without curves by more than FIT_QUANT.
#[test]
fn the_hand_off_breaks_look_error_ties_by_pull_and_keeps_the_gain_bar() {
    let vouch = crate::fit_cells::CellVouch { converged: 1.0, diverged: 0.0, aligned: 1.0, read: 1 };
    let candidate = |k: f32, err: f32, pull: f32| CastHandOff {
        wb: (k, 0.0),
        saturation: 0.0,
        curves: [Vec::new(), Vec::new(), Vec::new()],
        outcome: CastOutcome::default(),
        vouch,
        err,
        pull,
    };
    let walk = || {
        vec![
            candidate(6000.0, 0.0100, 0.50),
            candidate(7000.0, 0.0100 + 0.9 * FIT_QUANT, 0.20),
            candidate(8000.0, 0.0100 + 1.1 * FIT_QUANT, 0.05),
        ]
    };
    // 8000 K pulls least but sits outside the band; 7000 K is the tie that
    // pulls least.
    let pick = pick_cast_hand_off(walk(), 0.0200).expect("the band's least pull ships");
    assert_eq!(pick.wb.0, 7000.0);
    // The gain bar is on the PICK, not on the best error.
    assert!(pick_cast_hand_off(walk(), 0.0100 + 1.5 * FIT_QUANT).is_none());
    assert!(pick_cast_hand_off(Vec::new(), 1.0).is_none(), "nothing admitted, nothing handed");
}

// --- 4b termination and the hand-off's white-balance gates (2026-10-26) ---

/// The 4b do-no-harm loop on a re-fit that does to saturation what the
/// cast→WB hand-off does, admitted every round, against an end state that
/// never stops objecting. Before 2026-10-26 the 4b re-fit's hand-off
/// re-chased saturation from stage 3's start, so `recipe.saturation` came
/// back at that value each round and the loop never descended. The 4b
/// re-fit keeps the caller's saturation, so the loop walks 40 → 20 → 10 →
/// 5 → 2.5 → 0 and stops for want of dial.
#[test]
fn the_do_no_harm_loop_descends_when_the_hand_off_would_re_raise_saturation() {
    use super::solve::{DO_NO_HARM_RESCUE, step_down_until_harmless};
    let stage_start = 40.0;
    let mut recipe = EditRecipe { saturation: stage_start, ..Default::default() };
    let mut seen = Vec::new();
    let out = step_down_until_harmless(&mut recipe, true, |r| {
        if let Some(start) = DO_NO_HARM_RESCUE.hand_off_chase_from(stage_start) {
            r.saturation = start;
        }
        seen.push(r.saturation);
        true
    });
    assert_eq!(out, Ok(5), "the loop must descend to zero, not run on: {seen:?}");
    assert_eq!(seen, vec![20.0, 10.0, 5.0, 2.5, 0.0]);
    assert_eq!(recipe.saturation, 0.0);
}

/// …and the bound, on stubs that cannot descend: the pre-fix hand-off
/// (chase from stage 3's start every round), and a mixer carrying a hue
/// rotation `halved_hsl` never halves. Both stop at
/// `DO_NO_HARM_MAX_ROUNDS` and say so through `Err`, which the solve turns
/// into its warning line; a loop whose end state stops objecting stops at
/// once.
#[test]
fn the_do_no_harm_loop_stops_at_its_bound_when_nothing_descends() {
    use super::solve::{CastRescue, DO_NO_HARM_MAX_ROUNDS, step_down_until_harmless};
    let mut chased = EditRecipe { saturation: 40.0, ..Default::default() };
    let out = step_down_until_harmless(&mut chased, true, |r| {
        if let Some(start) = CastRescue::ChaseSaturation.hand_off_chase_from(40.0) {
            r.saturation = start;
        }
        true
    });
    assert_eq!(out, Err(DO_NO_HARM_MAX_ROUNDS));
    let mut rotated = EditRecipe::default();
    rotated.hsl.hue[0] = 10.0;
    assert_eq!(step_down_until_harmless(&mut rotated, true, |_| true), Err(DO_NO_HARM_MAX_ROUNDS));
    assert_eq!(rotated.hsl.hue[0], 10.0, "the halving never touches hue");
    let mut quiet = EditRecipe { saturation: 40.0, ..Default::default() };
    assert_eq!(step_down_until_harmless(&mut quiet, true, |_| false), Ok(1));
    assert_eq!(quiet.saturation, 20.0);
    assert_eq!(step_down_until_harmless(&mut quiet, false, |_| true), Ok(0));
}

/// A neutral-and-blue frame whose target is the same frame under a cool
/// white balance: a warm white balance turns the neutrals a hue the target
/// holds nowhere. Above the default strength the white-balance stage does
/// not ship that white balance whole, and a hand-off candidate asking for
/// it is refused on the same reading; at the default neither reads the
/// gates. The white balance the stage DOES ship clears the hand-off's gates,
/// because both read one step with one reader (`read_wb_step`).
#[test]
fn a_hand_off_white_balance_the_stage_would_shrink_is_refused_above_default() {
    use super::solve::hand_off_wb_step_refused;
    use crate::recipe::GradeStrength;
    let (w, h) = (96u32, 64u32);
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let l = 0.10 + 0.80 * x as f32 / (w - 1) as f32;
        let p = if y < h / 2 { [l, l, l] } else { [l * 0.3, l * 0.55, l] };
        image::Rgb(p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
    }));
    let target = render::develop_preview(
        &src,
        &EditRecipe { temperature_k: Some(4200.0), ..Default::default() },
    );
    let wb_stage = EditRecipe::default();
    let before_wb = pixels_of(&render::develop_preview(&src, &wb_stage));
    let tp = pixels_of(&target);
    let evidence = evidence_model_for(&before_wb, &tp, w, h);
    let strong = GradeStrength::new(1.0);
    let budget = FitBudget::for_strength(strong);
    let warm = (16000.0f32, 0.0f32);
    let mut warmed = wb_stage.clone();
    warmed.temperature_k = Some(warm.0);
    let reading = super::atmosphere::read_wb_step(
        &before_wb,
        &pixels_of(&render::develop_preview(&src, &warmed)),
        &tp,
        &evidence,
    );
    assert!(!reading.admits(budget), "premise: the warm step trips a stage gate (foreign {}, rotated {})", reading.foreign, reading.rotated);
    assert!(
        hand_off_wb_step_refused(&src, &wb_stage, warm, (&before_wb, &tp, &evidence), budget, strong),
        "the hand-off must refuse a white balance the stage's gates refuse"
    );
    let default = GradeStrength::default();
    assert!(
        !hand_off_wb_step_refused(
            &src,
            &wb_stage,
            warm,
            (&before_wb, &tp, &evidence),
            FitBudget::for_strength(default),
            default,
        ),
        "at the default strength the stage reads neither gate, and neither does the hand-off"
    );
    // The stage, asked for the same white balance at the same strength.
    let mut staged = wb_stage.clone();
    let facts = super::atmosphere::solve_white_balance(
        &src,
        &tp,
        &mut staged,
        &wb_stage,
        &evidence,
        warm,
        5500.0,
        budget,
        strong,
    );
    let shipped = (staged.temperature_k.unwrap_or(5500.0), staged.tint);
    assert_ne!(shipped, warm, "the stage does not ship it whole either");
    assert!(facts.clamped.is_some() || facts.foreign_hue_withheld, "…and says why");
    assert!(
        !hand_off_wb_step_refused(&src, &wb_stage, shipped, (&before_wb, &tp, &evidence), budget, strong),
        "what the stage ships, the hand-off's gates admit: {shipped:?}"
    );
}

// --- the mixer's two do-no-harm loops share 4b's bound -------------------

/// A call counter that fails the test instead of hanging it: past this many
/// rounds a loop is spinning, whatever its bound is supposed to be.
const SPIN_GUARD: u32 = 1000;

/// A mixer rotating Red by `hue` and saturating Yellow by `sat`.
fn mixer(hue: f32, sat: f32) -> crate::recipe::Hsl {
    let mut hsl = crate::recipe::Hsl::default();
    (hsl.hue[0], hsl.saturation[2]) = (hue, sat);
    hsl
}

/// The stage's own do-no-harm (4a), on a mixer whose halving cannot reach
/// neutral — a hue rotation from the base recipe, which `halved_hsl` keeps by
/// design — against a frame that objects to every candidate. The loop stops
/// at `DO_NO_HARM_MAX_ROUNDS` and withdraws with the last objection, exactly
/// as it does when the halving reaches neutral.
#[test]
fn the_mixer_stage_do_no_harm_stops_at_its_bound_when_nothing_descends() {
    use super::hsl::halve_mixer_until_harmless;
    use super::solve::DO_NO_HARM_MAX_ROUNDS;
    let rotated = mixer(10.0, 40.0);
    let mut calls = 0u32;
    let out = halve_mixer_until_harmless(rotated, |_| {
        calls += 1;
        assert!(calls <= SPIN_GUARD, "the stage's do-no-harm spun past {SPIN_GUARD} rounds");
        Some(HslWithdrawal::Blind)
    });
    assert_eq!(out, Err(HslWithdrawal::Blind));
    assert_eq!(calls, DO_NO_HARM_MAX_ROUNDS);
    // The bound never binds where halving does reach neutral: a band at the
    // budget ceiling is withdrawn after eight judged candidates.
    let pegged = crate::recipe::Hsl { hue: [0.0; 8], saturation: [100.0; 8], luminance: [-100.0; 8] };
    let mut judged = 0u32;
    let out = halve_mixer_until_harmless(pegged, |_| {
        judged += 1;
        Some(HslWithdrawal::Error)
    });
    assert_eq!(out, Err(HslWithdrawal::Error));
    assert!(judged < DO_NO_HARM_MAX_ROUNDS, "{judged} rounds to neutral");
    let kept = mixer(0.0, 20.0);
    assert_eq!(halve_mixer_until_harmless(kept.clone(), |_| None), Ok(kept));
}

/// The mixer against its own absence (4a'), on the same non-descending
/// mixer: the loop stops at `DO_NO_HARM_MAX_ROUNDS` and withdraws the mixer
/// to the neutral state its comparison was measured against.
#[test]
fn the_mixer_against_its_absence_stops_at_its_bound_when_nothing_descends() {
    use super::solve::{DO_NO_HARM_MAX_ROUNDS, halve_mixer_until_it_earns};
    let mut recipe = EditRecipe { hsl: mixer(10.0, 40.0), ..Default::default() };
    let mut refits = 0u32;
    let out = halve_mixer_until_it_earns(
        &mut recipe,
        |_| true,
        |_| {
            refits += 1;
            assert!(refits <= SPIN_GUARD, "the 4a' loop spun past {SPIN_GUARD} rounds");
        },
    );
    assert_eq!(out, Err(DO_NO_HARM_MAX_ROUNDS));
    assert!(recipe.hsl.is_neutral(), "withdrawn to neutral: {:?}", recipe.hsl);
    let mut pegged = EditRecipe {
        hsl: crate::recipe::Hsl { hue: [0.0; 8], saturation: [100.0; 8], luminance: [0.0; 8] },
        ..Default::default()
    };
    let out = halve_mixer_until_it_earns(&mut pegged, |_| true, |_| {});
    assert!(matches!(out, Err(n) if n < DO_NO_HARM_MAX_ROUNDS), "{out:?}");
    let mut quiet = EditRecipe { hsl: mixer(0.0, 40.0), ..Default::default() };
    assert_eq!(halve_mixer_until_it_earns(&mut quiet, |r| r.hsl.saturation[2] > 20.0, |_| {}), Ok(1));
    assert_eq!(quiet.hsl.saturation[2], 20.0);
}


// --- the hand-off's white-balance gates, reached from the public entry ---

/// Two bands, grey and orange, each a luminance ramp along x.
fn grey_and_orange_ramps() -> DynamicImage {
    let (w, h) = (192u32, 160u32);
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let l = 0.08 + 0.84 * x as f32 / (w - 1) as f32;
        let c = if y < h / 2 { [0.9, 0.9, 0.9] } else { [1.0, 0.6, 0.25] };
        image::Rgb(c.map(|c: f32| ((l * c).clamp(0.0, 1.0) * 255.0).round() as u8))
    }))
}

/// [`grey_and_orange_ramps`] against itself under a +40 tint, above the
/// default strength: the white-balance stage leaves the light as shot, the
/// channel curves are asked for the whole cast and fan, and the cast→WB
/// hand-off walks white balances the strength budget admits — and the
/// white-balance stage's own two gates, read on each candidate's step by
/// `hand_off_wb_step_refused`, refuse some of them. Nothing ships through
/// the hand-off. Measured 2026-10-26: 6 candidates refused at 0.9.
#[test]
fn a_hand_off_candidate_the_stage_gates_refuse_is_refused_from_the_public_entry() {
    use super::solve::route;
    use crate::recipe::GradeStrength;
    use crate::rationale::keys;
    let src = grey_and_orange_ramps();
    let target = render::develop_preview(
        &src,
        &EditRecipe { temperature_k: Some(5500.0), tint: 40.0, ..Default::default() },
    );
    route::take();
    let rep = fit_recipe_with(
        &src,
        &target,
        FitOptions { strength: GradeStrength::new(0.9), provider: None },
    );
    let seen = route::take();
    assert!(seen.hand_off_entered >= 1, "premise: the hand-off is entered: {seen:?}");
    assert!(seen.wb_step_refusals >= 1, "the stage's gates refuse a candidate: {seen:?}");
    assert!(
        !rep.notes.iter().any(|n| n.key == keys::FIT_NOTE_CAST_HANDED_TO_WB),
        "nothing ships through the hand-off: {}",
        rep.recipe.rationale
    );
}
