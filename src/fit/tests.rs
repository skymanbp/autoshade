
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
        slope: (0.5, 1.5),
        confidence_cap: 0.50,
        hsl_band: HSL_BAND_LIMIT_DEFAULT,
        // v1.17.1 added this axis: the band ceiling's share of its ±100
        // reach on the wheel's 0..100. The default recipe's grade is held
        // at 18 where the saturation chase's 60 held it before.
        grade_wheel: HSL_BAND_LIMIT_DEFAULT,
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
}

/// …and the negative half. Feed the solve a pair whose light did not change
/// and it invents no illuminant; and the cells' instrument (the zone
/// probes' voucher), asked about a move the wrong way, refuses it.
#[test]
fn a_full_mode_identity_pair_invents_no_illuminant_and_the_cells_refuse_a_backwards_move() {
    // No cast at all: nothing to solve.
    let source = synth();
    let report = fit_recipe(&source, &source);
    assert_eq!(report.recipe.temperature_k, None, "an identity pair invents no illuminant");
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
/// this is the same measurement at frame scope. v1.18.0: the white balance
/// ships on the joint objective, with no cell verdict of its own; the
/// instrument stays the zone probes' (`one_zone`), read here at frame scope.
///
/// MUTATION THIS KILLS: read the chromaticity vouch on `MoveAxis::Full`.
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
    // …and the joint solve ships it.
    let report = fit_recipe(&source, &target);
    assert_eq!(report.mode, FitMode::Full, "premise: the structure is untouched");
    let k = report.recipe.temperature_k.expect("the warmer, darker target still gets its white balance");
    assert!(k > 5500.0, "…and it reads warmer than as-shot: {k}");
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

/// A reading for the record: the report's recipe as JSON beside its two
/// errors and confidence (`RECIPE <tag> …`), printed under `--nocapture`.
fn dump_recipe(tag: &str, report: &FitReport) {
    eprintln!(
        "RECIPE {tag} err {:.6} -> {:.6} conf {:.3} {}",
        report.err_before,
        report.err_after,
        report.recipe.confidence,
        serde_json::to_string(&report.recipe).unwrap_or_default()
    );
}

/// The synthetic cases of the validity prototype (design-control-validity-1180.md
/// §4–5), written under `AUTOSHADE_VALIDITY_DUMP` by `dump_validity_probe`: each
/// case's pair, evidence and the frames of the recipe the law is to judge. A
/// case's recipe is read from `AUTOSHADE_VALIDITY_RECIPES/<case>.json` where
/// that file exists (the warm canyon's shipped recipe of 2026-11-07 is on file),
/// else fitted here (the stage's curve candidate for the coast, the fanning
/// coast and the golden sky; the restricted Atmosphere solve for the invented
/// half, beside the corresponding region's true cast as a white balance).
#[test]
#[ignore = "measurement: exports the fixture pairs for the NumPy validity prototype; run with --ignored"]
fn export_validity_fixtures() {
    let Some(dir) = crate::config::live_env_os("AUTOSHADE_VALIDITY_DUMP") else { return };
    let dir = std::path::PathBuf::from(dir);
    let recipes = crate::config::live_env_os("AUTOSHADE_VALIDITY_RECIPES").map(std::path::PathBuf::from);
    let recipe_of = |name: &str| -> Option<EditRecipe> {
        let text = std::fs::read_to_string(recipes.as_ref()?.join(format!("{name}.json"))).ok()?;
        serde_json::from_str(&text).ok()
    };
    let dump = |tag: &str, src: &DynamicImage, tgt: &DynamicImage, recipe: &EditRecipe| {
        let (s, t) = analysis_pair(src, tgt);
        let (sp, tp) = (pixels_of(&s), pixels_of(&t));
        let evidence = evidence_model_paired(&sp, &tp, None, s.width(), s.height());
        super::self_consistency::dump_validity_probe(&dir, tag, &s, &EditRecipe::default(), recipe, &tp, &evidence);
    };
    {
        let (src, tgt) = (canyon(false), canyon(true));
        let r = recipe_of("warm_rock").unwrap_or_else(|| fit_recipe(&src, &tgt).recipe);
        dump("warm_rock", &src, &tgt, &r);
    }
    for (tag, src, tgt) in [
        ("coast_candidate", coast(false), coast(true)),
        ("fan_coast_candidate", coast(false), two_temperature_coast()),
        ("golden_candidate", canyon(false), canyon_gold_target()),
    ] {
        let r = recipe_of(tag).unwrap_or_else(|| cast_stage_candidate(&src, &tgt).with);
        dump(tag, &src, &tgt, &r);
    }
    {
        let (src, tgt) = invented_half_pair();
        let field = |_: &DynamicImage,
                     _: &DynamicImage|
         -> anyhow::Result<crate::correspond::CorrespondenceField> {
            Ok(corresponding_field())
        };
        let paired = recipe_of("invented_paired").unwrap_or_else(|| atmosphere_fit(&src, &tgt, Some(&field)).recipe);
        dump("invented_paired", &src, &tgt, &paired);
        let law = render::WbLaw::of(&EditRecipe::default());
        let (k, tint) = law.invert([1.060f32.ln(), 0.0, 0.955f32.ln()], WB_SEARCH_K);
        let mut true_wb = paired.clone();
        (true_wb.temperature_k, true_wb.tint) = (Some(k), tint);
        dump("invented_true_wb", &src, &tgt, &true_wb);
    }
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
    // Re-pinned for v1.18.0 (R8 / R12): this pair solves in Atmosphere mode
    // (D 0.458), and the Atmosphere path's elimination (exposure and the
    // five-point curve, one joint solve) sets back each control that earns
    // less than FIT_QUANT of J* against its own absence: the exposure -0.28
    // and the curve (0,0) (65,61) (131,118) (179,190) (255,255) both did, so
    // neither ships. The saturation demand is past the Atmosphere cap and
    // ships at the cap, +30 (0 before v1.18.0). Measured 2026-11-04: J*
    // 0.4368 -> 0.4267.
    // RE-PINNED 2026-11-09 (the control-validity law; gates/j111): the
    // saturation at the cap moves the pale-blue sky's cell AGAINST its ask —
    // the target's sky is gold, the ask points from blue toward gold, a
    // global saturation pushes the cell further blue (cos −1) — so the law
    // refuses the chroma dials as the stage proposed them; re-estimated from
    // the base under the law, and under F, no chroma dial earns its quantum
    // against its absence (J* 0.3312 with them, 0.3325 without, one quantum
    // 0.0023 the dof price of seven), and the base ships: 0.3325 -> 0.3325.
    dump_recipe("wb_default hazy 0.65", &report);
    assert_eq!(report.recipe.temperature_k, None);
    assert_eq!(report.recipe.tint, 0.0);
    assert_eq!(report.recipe.exposure_ev, 0.0);
    assert_eq!(report.recipe.saturation, 0.0);
    assert!(report.recipe.tone_curve.is_empty(), "{:?}", report.recipe.tone_curve);
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
    // RE-PINNED 2026-11-09 (the control-validity law + the joint solver;
    // gates/j118-toys): the stage's demand (K 12700 / tint +72.7, ratio
    // 1.566 past the default budget's 1.40) is REFUSED as proposed and the
    // white balance is RE-ESTIMATED from the cells' own asks under the law
    // and inside the gain budget (`Joint::ask_fit`, the budget read on every
    // trial), so a white balance within the budget ships: K 8000, tint 0 —
    // the 1.17.1 rule "a demand over the budget ships nothing" was the
    // stage's, not the law's. The exposure lands at -0.87 beside it (J*
    // 0.161 -> 0.090), where the Atmosphere stage's marginal median read
    // -1.00 alone.
    dump_recipe("wb_default cal 0.65", &report);
    let k = report.recipe.temperature_k.expect("a white balance inside the budget ships");
    assert!((k - 8000.0).abs() <= 100.0, "K {k}");
    assert_eq!(report.recipe.tint, 0.0);
    // The EXPOSURE solve stays on the marginal weighted-luma median over
    // the shared-content population, and that is now a measured decision
    // rather than a postponement: the paired per-pixel form was
    // implemented and measured on 2026-09-02, moved neither this pair nor
    // `p37`, and stopped `flat_sky_to_cloud_deck` fitting at all. The
    // reasoning is at the solve; this assertion is what would catch an
    // accidental side effect, and it reads -1.00 on both statistics.
    // Under the joint solve (2026-11-09) the exposure is a coordinate of F
    // beside the white balance: -0.87 (gates/j118-toys).
    // -0.83 since the Atmosphere curve carries the withheld ranges' identity
    // (J121/J122, gates/j122-tests; -0.87 on the free knots, J118).
    assert!((report.recipe.exposure_ev - -0.83).abs() <= 0.011, "exposure {}", report.recipe.exposure_ev);
    let full = fit_recipe_from_with(
        &source,
        &target,
        &EditRecipe::default(),
        FitOptions { strength: crate::recipe::GradeStrength::new(1.0), provider: None },
    );
    dump_recipe("wb_default cal 1.0", &full);
    // RE-PINNED 2026-11-09 (gates/j118-wb): at full strength the budget no
    // longer binds and the white balance is the law's own re-estimate from
    // the cells' asks — K 12400, tint 0 (the stage's demand, K 12700 / tint
    // +72.7, moved the neutral cells' tint axis against their ask and was
    // refused as proposed; the Kelvin the cells ask lands within 300 K of
    // it). Exposure -0.93, J* 0.161 -> 0.088.
    // RE-PINNED 2026-11-09 (gates/j121-tests; the bounded Atmosphere curve,
    // J119): the white balance's leave-one-out frame carries the tone, and
    // with the stage's shape at one strength the stage's own demand — K 12700,
    // tint +74.5 — is admitted on every hue-band cell (no WhiteBalance
    // refusal in the trace); with the free knots it moved the neutral cells'
    // tint axis against their ask and was re-estimated to K 12400 / tint 0.
    // With the Atmosphere curve carrying the withheld ranges (J121/J122,
    // gates/j122b-two): K 12350 / tint +75.0, exposure -0.71 (was -0.93),
    // J* 0.161 -> 0.085 (was 0.088).
    let k = full.recipe.temperature_k.expect("the white balance ships at full strength");
    assert!((k - 12350.0).abs() <= 100.0, "the stage's Kelvin, admitted at full strength: {k}");
    assert!((full.recipe.tint - 75.0).abs() <= 1.0, "tint {}", full.recipe.tint);
    assert!((full.recipe.exposure_ev - -0.71).abs() <= 0.011, "exposure {}", full.recipe.exposure_ev);
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
    // The pairing the solve itself reads (`shared_content_pairing`): the
    // field's remap, and the evidence model built on that pair.
    let structural = evidence_model_for(&sp, &tp, w, h);
    let (pair_tp, judged) = shared_content_pairing(&sp, &tp, &structural, Some(&pc), Some(&shared));
    let pair_w = atmosphere_wb_pairing(&judged.expect("a readable field is judged on the shared content"));
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
        atmosphere_wb_from_populations(&sp, pair_tp, &pair_w, render::WbLaw::of(&EditRecipe::default()));
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

/// 2026-11-01. The Full solve's white balance is the BRIGHTNESS-INVARIANT
/// component of the paired colour change: per tonal region of the colour
/// grade, the part every region asks (`wb_common`). Same sign everywhere:
/// the smallest. Any two regions disagreeing, or none testifying: nothing.
/// The seaside street's regions read +0.93 / +0.72 / −0.14 on the Kelvin
/// axis and its white balance, which used to take the mass-weighted median
/// of the luma ranges and land on the 40000 K clamp, takes none of it.
///
/// MUTATION THIS KILLS: return the mass-weighted median (or the largest)
/// instead of the smallest agreeing value.
#[test]
fn the_white_balance_takes_only_the_colour_change_every_tonal_region_shares() {
    assert_eq!(wb_common(&[(0.44, 3.0), (0.30, 2.0), (0.17, 3.0)]), 0.17);
    assert_eq!(wb_common(&[(-0.40, 1.0), (-2.6, 9.0)]), -0.40);
    assert_eq!(wb_common(&[(0.93, 5.0), (0.72, 0.5), (-0.14, 1.3)]), 0.0);
    assert_eq!(wb_common(&[(0.20, 1.0), (0.0, 1.0)]), 0.0);
    assert_eq!(wb_common(&[(-2.58, 9.0)]), -2.58);
    assert_eq!(wb_common(&[]), 0.0);
}

/// rel1180 RULING-1180-sc-r2: the white balance takes only what every band
/// shares (`wb_common_shared`). A region whose interval `ask ± se` contains
/// zero cannot propose a move but BOUNDS it: the common ask is clipped into
/// that interval. Its error carries one output code's quantisation at its
/// level (`region_se`, `code_step_variance`), so a quantised 0.0 bounds
/// nothing finer than a code.
///
/// * The gate's 4400 K readings (Kelvin): shadows 0.0 (median error
///   0.00016, display level 0.185), midtones −0.0317, highlights −0.0109 — the
///   shadows' interval is wider than a code and the −0.0109 passes whole.
/// * A real pair's 8250 K readings: shadows +0.954, midtones +1.032,
///   highlights −0.00086 with a median error of 0.00088 at display level 0.936 — a
///   measured "stay": +0.954 is clipped into the highlights' interval.
///
/// MUTATIONS THIS KILLS: bounding dropped (a zero-containing region
/// abstains) -> the 8250 K case passes +0.954 whole; the quantisation floor
/// dropped -> 0.0 ± 0.00016 clips −0.0109 to −0.00016; the control-unit
/// mapping dropped -> the 4350 K case clips −0.0148 to −0.00295 (the chase
/// stalled there); a resolving sign split no longer vetoes.
#[test]
fn a_tonal_region_reading_zero_within_its_error_bounds_the_white_balance() {
    use super::evidence::{WbAxes, median_se, region_se, wb_common_shared, wb_move_shared};
    const SHADOW: f32 = 0.185;
    const HIGH: f32 = 0.936;
    let axes = |kelvin: &[(f32, f32)]| WbAxes {
        kelvin: kelvin.iter().map(|&(k, _)| (k, 1.0)).collect(),
        tint: kelvin.iter().map(|_| (0.0, 1.0)).collect(),
        se: kelvin.iter().map(|&(_, se)| [se, 0.0]).collect(),
        ..Default::default()
    };
    let mid = region_se(0.00014, 0.4);
    let shadows = region_se(0.00016, SHADOW);
    assert!(shadows > 0.0109, "the quantised 0.0 is wider than a code: {shadows}");
    assert_eq!(wb_common_shared(&axes(&[(0.0, shadows), (-0.0317, mid), (-0.0109, region_se(0.00001, 0.8))]), 0), -0.0109);
    let high = region_se(0.00088, HIGH);
    let common = wb_common_shared(&axes(&[(0.954, region_se(0.0025, 0.2)), (1.032, mid), (-0.00086, high)]), 0);
    assert!((common - (-0.00086 + high)).abs() < 1e-6 && common < 0.01, "bounded to {common}");
    // Read where the control acts (RULING-1180-sc-r3): the gate's 4350 K
    // readings, Kelvin, each region with its own response over the step
    // 4550 -> 4350 K and its one-code log step. Shadows 0.0 (se 0.011316,
    // code 0.03920, response 1.4856), midtones -0.014677 (se 0.005247,
    // response 0.99305), highlights 0.0 (se 0.002952, code 0.010226,
    // response 0.49628): neither zero can see a move of 0.0148 in gain
    // (0.0264 / 0.0206 per code), so the midtones' ask passes whole in the
    // control's unit; read in the ask's unit the highlights clip it to
    // -0.00295 and the chase stalled at 4350 K.
    let at_4350 = WbAxes {
        kelvin: vec![(0.0, 58690.4), (-0.014677, 13902.1), (0.0, 16738.5)],
        tint: vec![(0.0, 1.0); 3],
        se: vec![[0.011316, 1.0], [0.005247, 1.0], [0.002952, 1.0]],
        region: vec![0, 1, 2],
        code: vec![0.03920, 0.018166, 0.010226],
    };
    let mapped = wb_move_shared(&at_4350, 0, &[Some(1.4856), Some(0.99305), Some(0.49628)]);
    assert!((mapped - (-0.014677 / 0.99305)).abs() < 1e-6, "in the control's unit: {mapped}");
    assert!((wb_common_shared(&at_4350, 0) - (-0.002952)).abs() < 1e-6, "in the ask's unit");
    // A resolving sign split still vetoes; nothing resolving asks nothing.
    assert_eq!(wb_common_shared(&axes(&[(0.5, 0.01), (-0.3, 0.01)]), 0), 0.0);
    assert_eq!(wb_common_shared(&axes(&[(0.0, 0.01), (0.001, 0.01)]), 0), 0.0);
    // The median's error: √(π/2) · 1.4826 · MAD / √n_eff.
    let sample: Vec<(f32, f32)> = [-1.0f32, -0.5, 0.0, 0.5, 1.0].iter().map(|&v| (v, 1.0)).collect();
    let se = median_se(&sample, 0.0);
    assert!((se - (std::f32::consts::PI / 2.0).sqrt() * 1.482_602_2 * 0.5 / 5.0f32.sqrt()).abs() < 1e-5, "{se}");
}

/// A split-toned pair: the target warms the shadows, leaves the midtones
/// and cools the highlights of a neutral ramp. No illuminant does that, so
/// the white balance takes nothing (as shot), and the colour-grade wheels,
/// each solved from its own region's paired pixels, carry the split: a
/// warm shadow wheel, a cool highlight wheel, a midtone wheel that stays
/// far weaker than either.
///
/// This is also the line between a split tone and the FAN the cast curves'
/// gate refuses (`FAN_DEG`): the gate refuses hue sorted by luminance that
/// a per-channel curve invents and no region asked for; a wheel is solved
/// from the median change of its OWN region's paired pixels, so it moves a
/// region only the way that region testifies. The ramp's regions testify
/// warm, nothing and cool, and the wheels say exactly that.
///
/// MUTATION THIS KILLS: solve every wheel on the whole frame (no region
/// weight) — all three take one hue; or return the mass-weighted median
/// from `paired_wb_from_populations` — the white balance moves.
#[test]
fn a_split_tone_goes_to_the_grade_wheels_and_not_to_the_white_balance() {
    let n = 4096;
    let lin = render::srgb_to_linear;
    let enc = render::linear_to_srgb;
    let source: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            let v = 0.06 + 0.9 * i as f32 / (n - 1) as f32;
            [v, v, v]
        })
        .collect();
    let target: Vec<[f32; 3]> = source
        .iter()
        .map(|p| {
            let l = p[0];
            // Warm below a quarter, cool above three quarters, untouched
            // between: red against blue by ±0.30 in log linear light.
            let k: f32 = if l < 0.25 { 0.15 } else if l > 0.75 { -0.15 } else { 0.0 };
            [enc(lin(l) * k.exp()), l, enc(lin(l) * (-k).exp())]
        })
        .collect();
    let weights = vec![1.0f32; n];
    let axes = wb_axes_by_region(&source, &target, &weights);
    assert_eq!(axes.kelvin.len(), 3, "premise: all three regions testify");
    assert!(
        axes.kelvin[0].0 > 0.2 && axes.kelvin[2].0 < -0.2,
        "premise: the shadows ask warm and the highlights cool: {:?}",
        axes.kelvin
    );
    let law = render::WbLaw::of(&EditRecipe::default());
    let (k, tint, _) = paired_wb_from_populations(&source, &target, &weights, law);
    let as_shot = law.as_shot();
    assert_eq!(
        (k, tint),
        ((as_shot.0 / 50.0).round() * 50.0, round1(as_shot.1)),
        "no illuminant moves shadows and highlights opposite ways"
    );
    let grade = crate::recipe::ColorGrade::default();
    let warm = |h: f32| !(60.0..=330.0).contains(&h);
    let cool = |h: f32| (170.0..=270.0).contains(&h);
    let (sh_hue, sh_sat) = grade::solve_wheel(&source, &target, &weights, &grade, 0, [0.0, 0.0]);
    let (mid_hue, mid_sat) = grade::solve_wheel(&source, &target, &weights, &grade, 1, [0.0, 0.0]);
    let (hi_hue, hi_sat) = grade::solve_wheel(&source, &target, &weights, &grade, 2, [0.0, 0.0]);
    assert!(warm(sh_hue) && sh_sat > 5.0, "the shadow wheel warms: hue {sh_hue} sat {sh_sat}");
    assert!(cool(hi_hue) && hi_sat > 5.0, "the highlight wheel cools: hue {hi_hue} sat {hi_sat}");
    assert!(
        mid_sat < 0.5 * sh_sat.min(hi_sat),
        "the midtones asked nothing: hue {mid_hue} sat {mid_sat} against {sh_sat} / {hi_sat}"
    );
}

/// v1.17.1. The colour-grade wheels obey the strength budget as every stage
/// does: each wheel's Saturation is held under `FitBudget::grade_wheel`, the
/// mixer's band ceiling read on the wheel's axis (6 at Strength 0, 45 at
/// Strength 1). Strength is a restraint budget, not an amount: at 0 the
/// stage still ships what the tightest budget admits. A split-toned pair
/// (a structured neutral ramp; the target warms the darker half and cools
/// the brighter half by ±0.4 in log red/blue) whose regions ask far more
/// than the floor, through the stage itself with the pair's own cells:
/// at the Strength 0 ceiling no wheel exceeds 6 and the stage says it held
/// the grade; at the Strength 1 ceiling none exceeds 45 and the wheels move
/// further than at 0 (measured 2026-11-02: shadows / midtones / highlights
/// 6.0 / 0.9 / 6.0 at Strength 0, the paired pixels asking 88; 30.7 / 0.9 /
/// 45.0 at Strength 1, every cell converged). (The whole solve does not reach the stage on such a
/// pair: from a neutral source every target hue band is one-sided, and the
/// paired regression down-weights the toned regions.)
///
/// MUTATION THIS KILLS: clamp the wheels to the saturation chase's ceiling
/// (30 at Strength 0) instead of `grade_wheel`, or drop the clamp.
#[test]
fn the_grade_wheels_obey_the_strength_budget() {
    let (w, h) = (192u32, 128u32);
    let lin = render::srgb_to_linear;
    let enc = render::linear_to_srgb;
    let mut src = RgbImage::new(w, h);
    let mut tgt = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let block = if (x / 8 + y / 8) % 2 == 0 { 0.04 } else { -0.04 };
            let l = (0.08 + 0.84 * x as f32 / (w - 1) as f32 + block).clamp(0.02, 0.98);
            let k: f32 = if l < 0.5 { 0.20 } else { -0.20 };
            let t = [enc(lin(l) * k.exp()), l, enc(lin(l) * (-k).exp())];
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            src.put_pixel(x, y, image::Rgb([byte(l); 3]));
            tgt.put_pixel(x, y, image::Rgb(t.map(byte)));
        }
    }
    let (s_img, t_img) = analysis_pair(&DynamicImage::ImageRgb8(src), &DynamicImage::ImageRgb8(tgt));
    let tp = pixels_of(&t_img);
    let weights = vec![1.0f32; tp.len()];
    let ceiling = |s: f32| FitBudget::for_strength(crate::recipe::GradeStrength::new(s)).grade_wheel;
    assert!((ceiling(0.0) - HSL_BAND_LIMIT_MIN).abs() < 1e-4 && (ceiling(1.0) - HSL_BAND_LIMIT_MAX).abs() < 1e-4);
    let stage = |s: f32| {
        let mut recipe = EditRecipe::default();
        let held = grade::fit_grade_stage(&s_img, &tp, &weights, ceiling(s), &mut recipe);
        let cg = &recipe.color_grade;
        ([cg.shadow_sat, cg.midtone_sat, cg.highlight_sat], held)
    };
    let (low, low_held) = stage(0.0);
    let (high, _) = stage(1.0);
    assert!(low.iter().all(|&sat| sat <= ceiling(0.0) + 0.05), "Strength 0 wheels {low:?} over {}", ceiling(0.0));
    assert!(high.iter().all(|&sat| sat <= ceiling(1.0) + 0.05), "Strength 1 wheels {high:?} over {}", ceiling(1.0));
    assert!(
        high.iter().copied().fold(0.0, f32::max) > ceiling(0.0) + 1.0,
        "premise: the split asks more than the floor ceiling — Strength 1 wheels {high:?}"
    );
    let (held_at, asked) = low_held.expect("the dial held the grade at Strength 0 and the stage must say so");
    assert!((held_at - ceiling(0.0)).abs() < 1e-4 && asked > held_at, "held at {held_at}, asked {asked}");
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
            veto_cells: Some("luma[0.35-0.41]/neutral".into()),
            wb_clamped: None,
            wb_search_bound: None,
            scaled: Vec::new(),
            sat_pegged: None,
            evidence_refused: true,
            regressed: None,
            unearned: None,
            detail: (0.0, 0.0),
            detail_withheld: false,
            robust: None,
            paired: false,
            hsl: HslStageFacts::default(),
            grade_held: None,
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

/// A reading has a floor at its own resolution: a band the ranks cannot
/// resolve reads as empty, not as zero. A flat frame against the same flat
/// frame with ONE pixel a single rank bin brighter: on a million-pixel
/// core that pixel leaves every band below its quantisation floor
/// (`band_energy_floors`), so both sides are the same empty bands and D
/// reads 0 — where the old 1e-6 offset read the one pixel as structure
/// grown from nothing (band 0 alone log2(2e-4/1e-6) ≈ 7.7). The same frame
/// with a 10x10 block one bin brighter is content the ranks resolve, and
/// the floor does not hide it.
#[test]
fn a_band_the_ranks_cannot_resolve_reads_as_empty_not_as_zero() {
    let (w, h) = (1000u32, 1000u32);
    let n = (w * h) as usize;
    let level = 0.5f32;
    let bin = 1.0 / (budget::HIST_BINS - 1) as f32;
    let flat = vec![[level; 3]; n];
    let weights = vec![1.0f32; n];
    let mut one_pixel = flat.clone();
    one_pixel[(h as usize / 2) * w as usize + w as usize / 2] = [level + bin; 3];
    let floors = structure::band_energy_floors(((w - 6) * (h - 6)) as usize);
    let read = structure_divergence(&flat, &one_pixel, w, h, &weights).expect("a million-pixel core reads");
    assert!(read.d.abs() <= 1e-6, "one pixel one bin off is below every band's floor {floors:?}: {read:?}");
    let mut block = flat.clone();
    for y in 495..505 {
        for x in 495..505 {
            block[y * w as usize + x] = [level + bin; 3];
        }
    }
    let resolved = structure_divergence(&flat, &block, w, h, &weights).expect("a million-pixel core reads");
    assert!(resolved.energy_error > 0.1, "a resolved block is not floored away: {resolved:?}");
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

/// The same escape reads the grade wheels and vibrance (review A P11): a
/// recipe that moved a wheel, or vibrance, is not detail-only.
#[test]
fn a_grade_wheel_or_vibrance_move_is_not_a_detail_only_companion() {
    let base = EditRecipe::default();
    let mut detail_only = base.clone();
    detail_only.texture = 8.0;
    assert!(only_detail_and_quantized_companions(&detail_only, &base));
    let mut with_wheel = detail_only.clone();
    with_wheel.color_grade.shadow_sat = 15.0;
    assert!(!only_detail_and_quantized_companions(&with_wheel, &base));
    let mut with_vibrance = detail_only.clone();
    with_vibrance.vibrance = 10.0;
    assert!(!only_detail_and_quantized_companions(&with_vibrance, &base));
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

/// F1 review, re-read on the law (v1.18.0): a rescoring after the deep step
/// must RE-DERIVE the high-strength disclosure of the cells no evidence
/// speaks for ([`Cells::blind_cell_names`] between the base's render and the
/// recipe's) and keep its cap. Before this pin the rescored report dropped
/// the disclosure (and, in Full mode, the budget itself), so the same
/// movement came back uncapped. The pair: a grey ramp both sides share under
/// a bright blue strip the target repaints DARK — textured in the source,
/// flat in the target, so the strip's pixels do not pair and no target pixel
/// stands in its brightness range. A tint moves that strip with no ask to
/// measure against: blind, disclosed from strength 0.85 and withheld below.
#[test]
fn rescoring_re_derives_the_high_strength_veto_disclosure_and_its_cap() {
    let (w, h) = (128u32, 128u32);
    let px = |c: [f32; 3]| image::Rgb(c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
    let ramp = |x: u32| 0.25 + 0.40 * x as f32 / (w - 1) as f32;
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        if y < h / 2 {
            if (x / 4 + y / 4) % 2 == 0 { px([0.80, 0.86, 0.96]) } else { px([0.64, 0.70, 0.90]) }
        } else {
            px([ramp(x); 3])
        }
    }));
    let tgt = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        if y < h / 2 { px([0.06, 0.06, 0.08]) } else { px([ramp(x); 3]) }
    }));
    let recipe = EditRecipe { tint: 80.0, ..EditRecipe::default() };
    let disclosed = |r: &FitReport| {
        r.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_VETO_DISCLOSED)
    };
    let full = vec![crate::rationale::Note::new(
        crate::rationale::keys::FIT_NOTE_STRENGTH,
        vec![("pct", "100".into()), ("s", "1.0000".into())],
    )];
    let rescored = rescore_report(&src, &tgt, &recipe, &EditRecipe::default(), 0.3, &full);
    assert!(
        disclosed(&rescored),
        "the rescoring derives the strip's blind cell from the law: {}",
        rescored.recipe.rationale
    );
    let cap = FitBudget::for_strength(crate::recipe::GradeStrength::new(1.0)).confidence_cap;
    assert!(
        rescored.recipe.confidence <= cap + 1e-6,
        "rescored confidence {} above the strength cap {cap}",
        rescored.recipe.confidence
    );
    // Default control: no strength note carried → withhold policy → no
    // disclosure, exactly the pre-F1 rescoring.
    let rescored_default = rescore_report(&src, &tgt, &recipe, &EditRecipe::default(), 0.3, &[]);
    assert!(!disclosed(&rescored_default), "the shipped default must not gain a disclosure");
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
    // (measured 0.75 with the residual curve carrying the shape; 0.65 under
    // v1.18.0's J*), and
    // NEVER as the phantom-knot degeneracy this test exists to catch
    // (support mutation MC pegs ev at +3.0 and dies here).
    assert!(
        report.recipe.exposure_ev > 0.20 && report.recipe.exposure_ev < 1.20,
        "exposure must land as a moderate positive move, got {}",
        report.recipe.exposure_ev
    );
    // v1.18.0 (iii): J* reads in perceptual units (1 dE76 of local-mean
    // offset is 0.0075), so the residual is no longer comparable with the old
    // 0.035 line drawn in display-sRGB units: measured 0.1548 -> 0.0682 on
    // J* (0.44 of the base). The pin is now the fit's own gain: the residual
    // a fraction of the untouched base. J33 (per-pixel dE76, 2026-11-07)
    // shrank the BASE's reading 0.1548 → 0.1149 while the fit's residual held
    // (0.0682 → 0.0641), so the ratio reads 0.558; the line is 0.65 (the
    // coast's re-read of the same day, J76, moved 0.5× → 0.7× for the same
    // reason). Until 2026-11-08 the line was 0.5×.
    assert!(
        report.err_after < 0.65 * report.err_before,
        "residual {:.4} vs before {:.4}",
        report.err_after,
        report.err_before
    );
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
        // RE-PINNED for v1.18.0 DEF-3 (a reading is made of its mask's own
        // pixels at every scale): 0.491 -> 0.892 (correlation 0.791, energy
        // error 0.867). This reading's mask is the whole frame, so the one
        // edge DEF-3 renormalises is the frame border (with every weight 1
        // the two blurs differ only where a tap crosses it, up to 48 px in at
        // sigma 16), so the whole move is the border's. Still past
        // DIVERGENCE_GLOBAL (0.35), the side this test is named for.
        assert!(
            (measured.expect("resolvable").d - 0.892).abs() <= 0.05,
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
        // RE-PINNED for v1.18.0 DEF-3 (normalised band blurs, the frame
        // border one edge of the mask): cloud-deck 1.642 / 0.805 -> 1.979 /
        // 1.268, permutation 1.970 / 1.053 -> 2.077 / 1.221; the two shipped
        // panels stay on their pins (viaduct 0.189 / 0.204, Cornwall 0.147 /
        // 0.125, within 0.03 of each other as the doc above says).
        ("cloud-deck", measure(&cloud_src, &cloud_tgt), 1.979, 1.268),
        ("permutation", measure(&perm_src, &perm_tgt), 2.077, 1.221),
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
    // v1.18.0 (iii): J* reads in perceptual units, so the old 0.045 line
    // (display sRGB, 0.0589 -> 0.0306) no longer applies: measured on J*
    // 0.1539 -> 0.0566 (0.37 of the base). Pinned as the fit's gain.
    // RE-PINNED 2026-11-08 (J74/J76): on the per-pixel dE76 ruler (J33) the
    // base reads 0.0909 and the fit 0.0582 — 0.64 of it; the fit's reach is
    // where it was, the base's reading shrank with the ruler. Under the law
    // the tone the stages proposed enters at 0.11 of its estimate and the fit
    // ships exposure +0.09, saturation +54, texture +20 and the mixer's band
    // populations (with the detail controls folded into the tone frame, J74's
    // defect, it shipped nothing: 0.0909 -> 0.0879). Pinned at 0.7.
    assert!(
        report.err_after < 0.7 * report.err_before,
        "look residual {:.4} vs before {:.4}",
        report.err_after,
        report.err_before
    );
    assert!(
        !report
            .notes
            .iter()
            .any(|n| n.key == crate::rationale::keys::FIT_NOTE_REGRESSED),
        "no do-no-harm terminal reset on a fittable pair: {}",
        report.recipe.rationale
    );
    let joint = joint_after.expect("the joint family had an opinion before the fit");
    // RE-PINNED (J76): the joint value-range reading is an instrument, not
    // the ruler (J33); the fit lowers it (measured 0.1779 -> 0.0908; the
    // pre-J* recipe's grade wheels read 0.0470, and the per-pixel ruler
    // prefers this one). Pinned as an improvement, not at the old 0.06.
    let joint_before = joint_base.expect("the joint family had an opinion before the fit").weighted;
    assert!(joint.weighted < joint_before, "joint after {:.4} vs before {:.4}", joint.weighted, joint_before);
}

/// The detail controls sit in BOTH frames of every class
/// (`joint::without_class`, 2026-11-09; until then they opened the chain as
/// the tone's before-frame, J74): a candidate whose tone is the base's is not
/// read as a tone move whatever its texture does to the cells' brightness
/// medians, and a tone on a texture is read from the textured frame. The
/// mechanism, measured on the coherent coast's own pair: a
/// texture of +20 alone moves the darkest neutral cell by −0.086 ln luma
/// against an ask of +0.187 (a cell chosen by source brightness holds the
/// troughs of a texture, and a texture gain deepens them), a move the tone's
/// coordinates cannot scale away. Until 2026-11-08 it was read as the tone's:
/// the repair bisected to λ = 0 four rounds running, fell through to the
/// base, and the pair shipped a neutral recipe (0.0909 → 0.0879).
#[test]
fn the_detail_controls_sit_in_both_frames_of_every_class() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
    let triptych = image::open(root.join("showcase-cornwall-reverse-fit.jpg")).unwrap();
    let (s_img, t_img) = analysis_pair(&triptych.crop_imm(0, 136, 532, 356), &triptych.crop_imm(535, 136, 530, 356));
    let base = EditRecipe::default();
    let textured = EditRecipe { texture: 20.0, ..base.clone() };
    // Only the detail controls moved: without any class the candidate is
    // itself, so no class is judged and the texture is judged by none.
    for class in super::joint::CLASSES {
        assert_eq!(super::joint::without_class(&base, &textured, class), textured, "{class:?}: no class moved");
    }
    // A tone on top of it is read from the textured frame, not from the base.
    let toned = EditRecipe { exposure_ev: 0.3, ..textured.clone() };
    assert_eq!(
        super::joint::without_class(&base, &toned, super::joint::Refusal::Tone),
        textured,
        "the tone's before-frame is the candidate with the base's tone: the texture stays in it"
    );
    // The mechanism: the texture alone moves a cell's brightness median
    // beyond what its evidence asks, so folded into the tone frame it would
    // refuse a tone no coordinate of the tone could scale away.
    let sp = pixels_of(&render::develop_preview(&s_img, &base));
    let tp = pixels_of(&t_img);
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let cells = super::validity::Cells::new(&sp, &tp, &evidence);
    let dp = pixels_of(&render::develop_preview(&s_img, &textured));
    let verdict = cells.judge(super::validity::Class::Tone, &sp, &dp);
    eprintln!("DETAIL_FRAME {}", cells.describe(super::validity::Class::Tone, &verdict));
    assert!(
        !verdict.refused.is_empty(),
        "premise: texture +20 alone moves a cell's brightness median beyond its ask on this pair"
    );
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

/// [`invented_half_pair`] with the CORRESPONDING part of the target dimmed to
/// 0.75 of its display value: the shared content now asks for a real exposure
/// cut, while the invented slab (target rows 0-57, no source counterpart, 60 %
/// of the frame) stays as bright as before.
fn dimmed_shared_half_pair() -> (DynamicImage, DynamicImage) {
    let (src, tgt) = invented_half_pair();
    let mut t = tgt.to_rgb8();
    for (_, y, p) in t.enumerate_pixels_mut() {
        if y >= INVENTED_ROWS {
            for c in p.0.iter_mut() {
                *c = (*c as f32 * 0.75).round() as u8;
            }
        }
    }
    (src, DynamicImage::ImageRgb8(t))
}

/// "What the restriction excludes, no guard reads", for the exposure
/// (RULING-1180-int-r2): the Atmosphere exposure is SOLVED on the kept
/// population (the shared content), so it is earned and guarded there too.
/// On [`dimmed_shared_half_pair`] the shared content asks for an exposure cut
/// and the invented slab, which no source pixel answers for, is hurt by one;
/// the restricted solve's cut ships.
/// MUTATION: the exposure earned and guarded on the whole frame -> the slab
/// refuses the cut and the exposure goes back to 0.
#[test]
fn an_exposure_solved_on_the_shared_content_is_earned_there() {
    let (src, tgt) = dimmed_shared_half_pair();
    let field = |_: &DynamicImage,
                 _: &DynamicImage|
     -> anyhow::Result<crate::correspond::CorrespondenceField> {
        Ok(corresponding_field())
    };
    let paired = atmosphere_fit(&src, &tgt, Some(&field));
    assert_eq!(paired.mode, FitMode::Atmosphere, "premise: the Atmosphere solve");
    match paired.atmosphere_reference {
        AtmosphereReference::SharedContent { .. } => {}
        other => panic!("premise: the restriction is in force, got {other:?}"),
    }
    eprintln!(
        "SHARED_EXPOSURE {} (err {} -> {}): {}",
        paired.recipe.exposure_ev, paired.err_before, paired.err_after, paired.recipe.rationale
    );
    assert!(
        paired.recipe.exposure_ev <= -0.3,
        "the shared content's exposure cut ships: {}",
        paired.recipe.exposure_ev
    );
}

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
    dump_recipe("atmosphere_drops whole", &whole);
    dump_recipe("atmosphere_drops paired", &paired);
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
    // Under the law the whole-frame solve cannot ship the invented block's
    // +0.69 EV outright — the corresponding half's cells ask 0 EV and the
    // tone is scaled toward the base (+0.29, 2026-11-07) — but it still
    // sits above the truth the restricted solve lands on.
    // 2026-11-12: the brightening is read on the PICTURE, not on one dial.
    // Since the asks' curve (J142, 2026-11-11) the whole-frame solve carries
    // the invented block's brightness in the master curve with the exposure
    // at 0 (117 -> 145, 129 -> 161, 155 -> 193 of 255, the curve the identity
    // under 63: gates/j145), and a premise that read the exposure dial alone
    // called a brightened frame unmoved. The reading is what an exposure
    // does to a frame — the mean linear luma of the developed source against
    // as shot, in EV — whichever control carries it; the corresponding rows'
    // own reading is printed beside it.
    let lift_ev = |recipe: &EditRecipe, rows: std::ops::Range<u32>| -> f32 {
        let mean = |img: &DynamicImage| -> f64 {
            let rgb = img.to_rgb8();
            let (mut sum, mut n) = (0.0f64, 0.0f64);
            for (_, y, p) in rgb.enumerate_pixels() {
                if rows.contains(&y) {
                    let lin = p.0.map(|c| render::srgb_to_linear(c as f32 / 255.0));
                    sum += (0.299 * lin[0] + 0.587 * lin[1] + 0.114 * lin[2]) as f64;
                    n += 1.0;
                }
            }
            sum / n
        };
        (mean(&render::develop_preview(&src, recipe)) / mean(&render::develop_preview(&src, &EditRecipe::default()))).log2() as f32
    };
    let (whole_lift, paired_lift) = (lift_ev(&whole.recipe, 0..96), lift_ev(&paired.recipe, 0..96));
    eprintln!(
        "ATMOSPHERE_DROPS frame lift: whole {whole_lift:+.3} EV (dial {}), paired {paired_lift:+.3} EV (dial {}); corresponding rows: whole {:+.3}, paired {:+.3}",
        whole.recipe.exposure_ev,
        paired.recipe.exposure_ev,
        lift_ev(&whole.recipe, INVENTED_ROWS..96),
        lift_ev(&paired.recipe, INVENTED_ROWS..96)
    );
    assert!(
        whole_lift > CORRESPONDING_TRUE_EV + 0.20,
        "premise: the invented block is brighter and pulls the whole-frame solve's brightness up: {whole_lift:+.3} EV"
    );
    assert!(
        (paired_lift - CORRESPONDING_TRUE_EV).abs() < 0.20
            && (paired.recipe.exposure_ev - CORRESPONDING_TRUE_EV).abs() < 0.20,
        "the restricted solve must recover the corresponding region's exposure \
             {CORRESPONDING_TRUE_EV:.2}: whole {whole_lift:+.3} EV, paired {paired_lift:+.3} EV (dial {})",
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
    dump_recipe("reference_restriction paired", &paired);
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
    dump_recipe("calibration_atmosphere", &report);
    assert_eq!(report.mode, FitMode::Atmosphere);
    assert!(report.structural_evidence.is_some());
    assert!((-1.0..=-0.5).contains(&report.recipe.exposure_ev));
    assert_eq!(report.recipe.saturation, 0.0);
    assert_eq!((report.recipe.clarity, report.recipe.texture), (0.0, 0.0));
    // v1.18.0 R12: the tone curve solved on the exposure must earn FIT_QUANT
    // against its own absence (`eliminate_unearned`). On this pair it earns
    // 0.000835 of J* (0.137261 with it, 0.138096 without), so it is set back
    // and named; the exposure (0.209978 without it) ships alone.
    let unearned = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_UNEARNED)
        .and_then(|n| n.args.iter().find(|(k, _)| *k == "controls").map(|(_, v)| v.clone()))
        .unwrap_or_default();
    assert!(
        !report.recipe.tone_curve.is_empty() || unearned.contains("tone curve"),
        "the curve either ships or is named as unearned: {unearned}"
    );
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
    // v1.18.0 R12: the exposure and the curve solved on it are a joint solve,
    // and each must earn FIT_QUANT against its own absence
    // (`eliminate_unearned`). This pair's target is the source with its sky
    // pixels permuted, so the two populations are identical and the curve
    // earns exactly nothing on the Atmosphere ruler (0.100000 with it and
    // without it): it is set back and the note says so. A curve that ships
    // is still the robust five-point map inside the slope budget.
    let unearned = report
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_UNEARNED)
        .and_then(|n| n.args.iter().find(|(k, _)| *k == "controls").map(|(_, v)| v.clone()))
        .unwrap_or_default();
    assert!(
        r.tone_curve.is_empty() && unearned.contains("tone curve"),
        "a curve the identical populations cannot earn is set back and named: {:?} / {unearned}",
        r.tone_curve
    );
    assert!(r.tone_curve.is_empty() || r.tone_curve.len() == 5, "Atmosphere tone must stay a robust five-point map");
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
    // v1.18.0 R8: each slider of the joint tone solve must earn FIT_QUANT of
    // the tonal term against its own absence (`eliminate_unearned`). The
    // solve's contrast +7 and highlights -17.4 (with whites -1.6, blacks
    // -0.1) together earn 0.0016 of T_L (0.006796 solved, 0.008416 without
    // all four), under FIT_QUANT, so they are set back and named; the
    // exposure and shadows earn and ship. A slider that ships still lands in
    // the truth's direction.
    let unearned = rep
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_UNEARNED)
        .and_then(|n| n.args.iter().find(|(k, _)| *k == "controls").map(|(_, v)| v.clone()))
        .unwrap_or_default();
    assert!(
        (r.contrast > 3.0 && r.contrast < 45.0) || (r.contrast == 0.0 && unearned.contains("contrast")),
        "contrast {} ({unearned})",
        r.contrast
    );
    assert!(
        (r.highlights < -8.0 && r.highlights > -50.0)
            || (r.highlights == 0.0 && unearned.contains("highlights")),
        "highlights {} ({unearned})",
        r.highlights
    );
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
    // Measured 0.0892 -> 0.0229 (0.26×) on the pre-v1.18.0 objective.
    // Re-pinned for v1.18.0: J* reads the same pair 0.1518 -> 0.0618 (0.41×,
    // measured 2026-11-04) — a different ruler (per-pixel CIE76 on local
    // means where pixels pair, every present pixel at its presence weight),
    // and since R11 a different landing: the residual tone curve is withheld
    // on its tonal reading (a gain under FIT_QUANT) and the cast, which then
    // fans past the limit, ships projected (t = 0.783) rather than admitted.
    // 0.45× keeps the claim (most of the gap closed) with room for the
    // FIT_QUANT another stage may still move J* by.
    // RE-PINNED for v1.18.0 DEF-3 (normalised band blurs): 0.1514 -> 0.0729
    // (0.48×; 0.1509 -> 0.0645 with DEF-3 reverted). The colour terms close
    // as before (E_paired 0.180 -> 0.036, H_ab 0.085 -> 0.041; local()
    // 0.148 -> 0.033 on both builds); S is what moved: the fitted render
    // reads 0.396 against the target (0.312 reverted), the source 0.033
    // (0.028). The bar is the measured landing plus FIT_QUANT, as the joint
    // family's scalar is pinned: 0.0729 + 0.0023 = 0.0752 = 0.497× of the
    // starting error — still "most of the gap", the claim this pin carries.
    // RE-PINNED 2026-11-09 (the control-validity law, its leave-one-out
    // frames and per-class cells, the joint solver J103–J111; gates/j111):
    // J* reads the pair 0.1367 -> 0.0944 (0.69×). The law holds what the
    // pre-law solve shipped: the grade wheels tint the neutral cells, which
    // ask 0 ± one code; the curves' move in a cell shifts under every tone
    // step and the curves are refused at every level; the chroma dials at
    // the end earn less than their dof price against their absence. What
    // ships is the exposure, the saturation and the master curve (9 dof,
    // F 0.1115). The bar is the measured landing plus FIT_QUANT; the claim
    // is now "closes the gap the law admits", not "most of the gap".
    assert!(
        rep.err_after < 0.0944 + FIT_QUANT && rep.err_after < 0.75 * rep.err_before,
        "the fallback solve must still close the gap the law admits ({:.4} -> {:.4})",
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
            "sky hue drifted to {mean:.0}° (Δ{d:.0}°) — a violet/purple cast leaked through: {} RECIPE {}",
            rep.recipe.rationale,
            serde_json::to_string(&rep.recipe).unwrap_or_default()
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
///
/// Under the one validity law (v1.18.0) the same picture claim holds for
/// the law's reason: the sky's cells ask for gold, the curves that would
/// rotate them there move the rocks' cells against their asks and are
/// refused, and a re-hue of the sky alone is no global control's to make.
/// What the law DOES admit is a move every cell asks for: the rocks' cells
/// and the sky's both ask to warm (blue → gold is a warming), so the global
/// white balance warms, and on its way toward gold the pale blue sky
/// passes through grey — a loss of chroma, not a rotation (measured
/// 2026-11-07: Blue band saturation 0, saturation 0, the Orange band +15;
/// the sky's mean chroma 0.09 → 0.012). An achromatic sky is therefore read
/// as what it is rather than as a broken fixture; the audit prints the
/// reading.
#[test]
fn cast_must_not_rotate_the_sky_into_a_target_native_hue() {
    let src = canyon(false);
    let tgt = canyon_gold_target();
    let rep = fit_recipe(&src, &tgt);
    let out = render::develop_preview(&src, &rep.recipe).to_rgb8();
    let (mut sin, mut cos, mut n) = (0.0f64, 0.0f64, 0.0f64);
    let (mut chroma_sum, mut sky_n) = (0.0f64, 0.0f64);
    for y in 112..128 { // sky rows only — the fixtures paint rock below y=112
        for x in 0..192 {
            let p = out.get_pixel(x, y);
            let (r, g, b) =
                (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
            let chroma = r.max(g).max(b) - r.min(g).min(b);
            chroma_sum += chroma as f64;
            sky_n += 1.0;
            if chroma < 0.03 {
                continue;
            }
            let hue = render::rgb_to_hsl(r, g, b).0 as f64 * std::f64::consts::TAU;
            sin += hue.sin();
            cos += hue.cos();
            n += 1.0;
        }
    }
    let mean_chroma = chroma_sum / sky_n.max(1.0);
    eprintln!(
        "SKY_HUE_AUDIT chromatic {n} of {sky_n} sky pixels, mean chroma {mean_chroma:.3}; WB {:?} K tint {}; saturation {} band sat {:?} band hue {:?}",
        rep.recipe.temperature_k, rep.recipe.tint, rep.recipe.saturation, rep.recipe.hsl.saturation, rep.recipe.hsl.hue
    );
    if n > 0.0 {
        let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
        let d = (mean - 213.0 + 540.0).rem_euclid(360.0) - 180.0;
        assert!(
            d.abs() < 30.0,
            "sky hue rotated to {mean:.0}° (Δ{d:.0}°) — a target-native re-hue leaked through"
        );
    } else {
        assert!(
            mean_chroma < 0.03,
            "no chromatic sky pixel and yet the sky is not grey: mean chroma {mean_chroma:.3}"
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
/// number — see `the_fan_gate_now_costs_a_real_two_temperature_pair_its_cast`.
struct CastCandidate {
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
    CastCandidate { with }
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

/// v1.2.3 found the failure class on the Cornwall showcase pair: three
/// independent monotone channel maps sort a single-hued region into a hue
/// FAN by luminance. The coast fixture's sky moves by a per-channel gain
/// (source [0.74, 0.85, 1.00] x level, target [0.62, 0.78, 1.00] x level),
/// which the controls carry without sorting the class — the picture claim
/// this test always held, end to end: the delivered sky does not fan, and
/// the fit does not make the look worse. Under the one validity law
/// (`fit::validity`, 2026-11-07) a fan IS a refusal — each brightness slice
/// of the class is a cell moved against its own ask — so no gate reading is
/// pinned here; [`delivered_fan_of`] is a test instrument, not a gate.
/// Measured 2026-11-07 before the law: the frame shipped at 1.99°, the
/// delivered sky's octile spread 5.98° (2026-10-22).
#[test]
fn a_coherent_sky_cast_the_controls_carry_as_a_gain_ships_without_a_fan() {
    let src = coast(false);
    let tgt = coast(true);
    let rep = fit_recipe(&src, &tgt);
    let shipped = delivered_fan_of(&src, &tgt, &rep);
    let delivered = render::develop_preview(&src, &rep.recipe);
    let spread = hue_spread_across_luma(&delivered, 0..64);
    eprintln!(
        "COHERENT_SKY shipped fan {shipped:.2} sky spread {spread:.2} err {:.5} -> {:.5}",
        rep.err_before, rep.err_after
    );
    assert!(
        shipped < 0.7 * FAN_VISIBLE_DEG,
        "margin eroded: the shipped frame fans {shipped:.1}° against the {FAN_VISIBLE_DEG}° visibility edge"
    );
    assert!(
        spread < FAN_VISIBLE_DEG as f64,
        "the delivered sky fanned {spread:.1}° across luminance, against the {FAN_VISIBLE_DEG}° edge"
    );
    assert!(
        rep.err_after <= rep.err_before + 0.01,
        "fit made the look worse: {} -> {}",
        rep.err_before,
        rep.err_after
    );
}

/// [`two_temperature_coast`]: a target whose sky really is two colour
/// temperatures. Every size of the per-channel cast that would carry it
/// sorts the sky into a hue fan by brightness — each slice a cell moved
/// against its own ask — so under the one validity law no such cast is
/// admitted as proposed, the repair scales it to what the cells admit, and
/// what ships does not fan the sky or make the look worse. The retired fan
/// gate refused this pair outright and said so; the law reads the same
/// picture claim without a gate of its own.
#[test]
fn a_cast_that_fans_at_every_size_ships_no_fan() {
    let src = coast(false);
    let tgt = two_temperature_coast();
    let rep = fit_recipe(&src, &tgt);
    let shipped = delivered_fan_of(&src, &tgt, &rep);
    let delivered = render::develop_preview(&src, &rep.recipe);
    let spread = hue_spread_across_luma(&delivered, 0..64);
    let curves = !(rep.recipe.red_curve.is_empty() && rep.recipe.green_curve.is_empty() && rep.recipe.blue_curve.is_empty());
    eprintln!(
        "TWO_TEMPERATURE_COAST shipped fan {shipped:.2} sky spread {spread:.2} curves {curves} err {:.5} -> {:.5} :: {}",
        rep.err_before, rep.err_after, rep.recipe.rationale
    );
    assert!(shipped < FAN_VISIBLE_DEG, "the shipped frame fans {shipped:.1}° against the {FAN_VISIBLE_DEG}° edge");
    assert!(spread < FAN_VISIBLE_DEG as f64, "the delivered sky fanned {spread:.1}° across luminance");
    assert!(
        rep.err_after <= rep.err_before + FIT_QUANT,
        "the solve does no harm: {} -> {}",
        rep.err_before,
        rep.err_after
    );
}

/// The fan class on every fixture that ever provoked it (DEF-1,
/// RULING-1180-int-r4 Q-CW2; re-read under the one validity law
/// 2026-11-07): the haze pair, whose fitted candidate fanned 22.5° on
/// 2026-11-04; the hue-ramps pair under a 3500 K white balance, a
/// per-channel gain the curves carry legitimately; canyon-gold and
/// hazy→vivid, the pairs the retired rotation and foreign-hue gates
/// refused; canyon-warm, convicted at 30.6° under the retired fan gate. The
/// claim is the picture's: no shipped frame fans a hue class past the
/// visibility edge, and canyon-warm lands and delivers its colour rather
/// than resetting. The retired gate's candidate readings are not pinned —
/// the law refuses a fan as cells left worse than it found them. The
/// margins are the ones each pair always carried on its delivered frame.
#[test]
fn no_shipped_cast_fans_a_hue_class_and_the_colour_is_delivered() {
    let (hazed, clean) = haze_pair();
    for (name, src, tgt, ceiling) in [
        ("haze", hazed, clean, 1.0),
        ("hue_ramps", hue_ramps(), cooled_ramps(0), 0.6),
        ("canyon_gold", canyon(false), canyon_gold_target(), 0.75),
        ("hazy_vivid", hazy_canyon_source(), vivid_warm_target(), 0.3),
    ] {
        let rep = fit_recipe(&src, &tgt);
        let shipped = delivered_fan_of(&src, &tgt, &rep);
        eprintln!("FAN_ARM {name} shipped {shipped:.2} err {:.5} -> {:.5}", rep.err_before, rep.err_after);
        assert!(
            shipped < ceiling * FAN_VISIBLE_DEG,
            "margin eroded: {name}'s shipped frame fans {shipped:.1}° against the {FAN_VISIBLE_DEG}° edge"
        );
    }
    // Canyon-warm: lands rather than resets, fans nothing, and delivers the
    // colour (mean CIE76 over the analysis pair, 17.42 for the untouched
    // source).
    let (warm_src, warm_tgt) = (canyon(false), canyon(true));
    let warm = fit_recipe(&warm_src, &warm_tgt);
    assert!(
        warm.err_after < warm.err_before,
        "canyon-warm must land rather than reset: {} -> {}",
        warm.err_before,
        warm.err_after
    );
    let warm_fan = delivered_fan_of(&warm_src, &warm_tgt, &warm);
    assert!(warm_fan < FAN_VISIBLE_DEG, "canyon-warm's shipped frame fans {warm_fan:.2}° against {FAN_VISIBLE_DEG}°");
    let (s, t_img) = analysis_pair(&warm_src, &warm_tgt);
    let tp = pixels_of(&t_img);
    let mean_de = |recipe: &EditRecipe| {
        let px = pixels_of(&render::develop_preview(&s, recipe));
        px.iter().zip(&tp).map(|(a, b)| {
            let (a, b) = (cie_lab(a), cie_lab(b));
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        }).sum::<f32>() / px.len().max(1) as f32
    };
    let (identity_de, shipped_de) = (mean_de(&EditRecipe::default()), mean_de(&warm.recipe));
    eprintln!("CANYON_WARM shipped fan {warm_fan:.2} dE76 {identity_de:.2} -> {shipped_de:.2}");
    assert!(
        shipped_de < identity_de,
        "canyon-warm delivers colour: mean ΔE76 {identity_de:.2} -> {shipped_de:.2}"
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
    let (pair_tp, pair_w) = (tp.as_slice(), atmosphere_wb_pairing(&evidence));
    let (wb_k, wb_tint, _) =
        atmosphere_wb_from_populations(&sp, pair_tp, &pair_w, render::WbLaw::of(&base));
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
        budgeted_wb(render::WbLaw::of(&EditRecipe::default()), free_k, free_tint, budget);
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
    eprintln!("WB_CLAMP free {free_k:.0}/{free_tint:+.1} clamped {clamped_k:.0}/{clamped_tint:+.1} lambda {lambda:.4}");
    dump_recipe("wb_clamp 0.85", &report);
    // 2026-11-12 (the joint solve; user ruling 2026-11-06: the stages
    // propose, the joint objective decides). Until the joint solve what
    // shipped WAS the stage's clamped estimate, and this test pinned the two
    // equal, tint included. The solve now moves the white balance's two
    // coordinates on J* inside the law and the budget: on this pair the
    // estimate sits on the search's 40 000 K bound, unclamped (lambda 1),
    // with tint +14.1, and the solve keeps the Kelvin and settles the tint at
    // +16.4 (J* 0.3325 -> 0.2521, gates/j145). The clamp's manifold is
    // pinned above on `budgeted_wb`, the function that owns it; what the
    // SOLVE owes is a white balance that fits the strength budget and that
    // the law admits on every hue band's cell — a tint no cell asks for is a
    // refusal there.
    let shipped_k = report.recipe.temperature_k.expect("the white balance ships");
    assert!(
        wb_gains_fit_budget(render::wb_gains(anchor, shipped_k, report.recipe.tint), budget),
        "the shipped white balance fits the strength budget: {shipped_k} K, tint {}",
        report.recipe.tint
    );
    {
        use super::joint::{without_class, Refusal};
        use super::validity::{Cells, Class};
        let (s_img, t_img) = analysis_pair(&src, &target);
        let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
        let tp = pixels_of(&t_img);
        // The cells of the solve itself: the report carries the model it was
        // judged on (the structure-blind frame model in Atmosphere, the
        // paired one in Full), so no second model is built here.
        let cells = Cells::new(&sp, &tp, &report.evidence);
        let frame = without_class(&EditRecipe::default(), &report.recipe, Refusal::WhiteBalance);
        let before_px = pixels_of(&render::develop_preview(&s_img, &frame));
        let after_px = pixels_of(&render::develop_preview(&s_img, &report.recipe));
        let verdict = cells.judge(Class::WhiteBalance, &before_px, &after_px);
        eprintln!(
            "WB_CLAMP shipped {shipped_k:.0}/{:+.1}: {}",
            report.recipe.tint,
            cells.describe(Class::WhiteBalance, &verdict)
        );
        assert!(
            verdict.refused.is_empty(),
            "the white balance that ships moves no hue band's cell against its ask: {}",
            cells.describe(Class::WhiteBalance, &verdict)
        );
    }
}

/// A white balance the strength budget admits but the evidence does not: the
/// warm demand of a grey→warm ramp also rotates a RETAINED blue band (the
/// same in the target) by more than 45°. Under the one validity law the
/// band's cell asks nothing on the white balance's own axes and the warming
/// moves it, so the white balance is refused as proposed and the repair
/// scales it toward as-shot to what the band admits: what the solve ships
/// does not rotate the band the way the free demand would, at the shipped
/// default and above it. Until v1.18.0 the stage bisected its scalar against
/// a foreign-hue gate of its own — an instance of the law, retired with it.
#[test]
fn a_white_balance_that_would_rotate_a_retained_band_is_held_by_the_law() {
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

    // The band is searched for, not hand-picked: a pixel chromatic enough to
    // carry a hue (chroma ≥ 0.03), between 170° and 250°, that the probe
    // pair's budgeted warm gains move ≥ 50° into a hue more than three 15°
    // bins from every warm hue the target holds (fixture-search parameters,
    // not the law's).
    const BAND_CHROMA_MIN: f32 = 0.03;
    const FAR_BINS: usize = 3;
    let (probe_source, probe_target) = pair_with_blue_band([0.62, 0.65, 0.65]);
    let (anchor, probe_k, probe_tint) =
        free_atmosphere_wb_for_pair(&probe_source, &probe_target);
    let (probe_chosen_k, probe_chosen_tint, _, _, _, _) = budgeted_wb(
        render::WbLaw::of(&EditRecipe::default()),
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
                if chroma < BAND_CHROMA_MIN {
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
                let after_is_foreign = bin_distance(after_bin, before_bin) > FAR_BINS
                    && warm_bins
                        .iter()
                        .all(|&warm_bin| bin_distance(after_bin, warm_bin) > FAR_BINS);
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
    let (_, free_k, free_tint) = free_atmosphere_wb_for_pair(&source, &target);
    let (chosen_k, chosen_tint, _, _, _, _) = budgeted_wb(
        render::WbLaw::of(&EditRecipe::default()),
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
    let demanded_hue = mean_hue_in_rows(&after, 96..128);
    let demanded = hue_distance(before_hue, demanded_hue).abs();
    assert!(
        demanded >= 45.0,
        "fixture premise: fitted {free_k:.0}/{free_tint:+.1} -> {chosen_k:.0}/{chosen_tint:+.1} rotated the retained blue band only {before_hue:.1} -> {demanded_hue:.1}"
    );

    let band_rotation = |report: &FitReport| {
        let shipped = render::develop_preview(&source, &report.recipe);
        hue_distance(before_hue, mean_hue_in_rows(&shipped, 96..128)).abs()
    };
    let report = fit_recipe_from_promoted_with_disclosure_opts(
        &source,
        &target,
        &EditRecipe::default(),
        true,
        false,
        FitOptions { strength: crate::recipe::GradeStrength::new(0.85), provider: None },
    );
    dump_recipe("wb_band 0.85", &report);
    assert_eq!(report.mode, FitMode::Atmosphere);
    let rotated = band_rotation(&report);
    eprintln!(
        "WB_BAND free {free_k:.0}/{free_tint:+.1} budgeted {chosen_k:.0}/{chosen_tint:+.1} shipped {:?}/{:+.1}: band rotated {demanded:.1} demanded, {rotated:.1} shipped",
        report.recipe.temperature_k, report.recipe.tint
    );
    assert!(
        rotated < 45.0,
        "at 0.85 the shipped white balance rotated the retained band {rotated:.1}° (the free demand: {demanded:.1}°): {}",
        report.recipe.rationale
    );
    let default_report = fit_recipe_from_promoted(&source, &target, &EditRecipe::default(), true);
    dump_recipe("wb_band default", &default_report);
    let rotated = band_rotation(&default_report);
    assert!(
        rotated < 45.0,
        "at the default the shipped white balance rotated the retained band {rotated:.1}°: {}",
        default_report.recipe.rationale
    );
}

/// The hazy canyon under a vivid warm target: the retired rotation gate was
/// the one rejector of its cast curves (pinned 2026-07/09), and under the
/// one validity law the sky's cells ask no rotation, so any class that
/// rotated them would leave them worse than it found them. The picture claim
/// stays, end to end: the shipped sky's hue stays within 30° of the source's
/// blue (214°).
#[test]
fn the_hazy_canyons_sky_stays_blue_under_a_warm_target() {
    let src = hazy_canyon_source();
    let tgt = vivid_warm_target();
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
    assert!(n > 0.0, "no chromatic sky pixels to audit");
    let mean = sin.atan2(cos).to_degrees().rem_euclid(360.0);
    let d = (mean - 214.0 + 540.0).rem_euclid(360.0) - 180.0;
    assert!(
        d.abs() < 30.0,
        "sky hue rotated to {mean:.0}° (Δ{d:.0}°): {} RECIPE {}",
        rep.recipe.rationale,
        serde_json::to_string(&rep.recipe).unwrap_or_default()
    );
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
///                                   (landed 2026-11-08 to 2026-11-12:
///                                   0.2688 → 0.0493; refused again since
///                                   J145: 0.2688 → 0.1214)
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
///                                        reset by the terminal check;
///                                        2026-11-08: 0.1688 → 0.0704 at
///                                        0.630; since J145, 2026-11-12:
///                                        0.1685 → 0.0761 at 0.250)
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
///     member (none from 2026-11-08 to 2026-11-12, while channel curves
///     carried canyon gold's sky into the target's gold and stained its
///     grey ramp with it; one again since J145 heard the ramp — see its
///     block below. The policy — a one-sided reading is a refusal, not a
///     miss — is `JointFarCause`'s, not a fixture's): canyon gold,
///     0.2768 → 0.2768, i.e. 2.8× JOINT_FAR_ERR —
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
    // Canyon GOLD was the refusal band's last member; canyon warm left it
    // in v1.2.3 and is asserted where it went, below. RE-PINNED (v1.18.0,
    // J84, 2026-11-08): the mixer's Orange rotation carries the canyon-gold
    // cast (the mechanism that moved canyon warm on 2026-11-02) and the
    // terminal reset no longer stands behind a neutral recipe, so this pair
    // LANDS too: J* 0.1688 -> 0.0704 at confidence 0.630, joint reading
    // 0.2688 -> 0.0493, under JOINT_FAR_ERR — no FAR classification of
    // either kind (gates/j79). The band is FIT_QUANT wide, as canyon warm's.
    {
        let (name, src, tgt) = ("canyon gold", canyon(false), canyon_gold_target());
        let (before, after, _) = read(&src, &tgt);
        let report = fit_recipe(&src, &tgt);
        eprintln!(
            "JOINT_FAMILY {name}: joint {before:.4} -> {after:.4}, err {:.4} -> {:.4}, confidence {:.3}, joint notes {:?}",
            report.err_before,
            report.err_after,
            report.recipe.confidence,
            report.notes.iter().filter(|n| n.key.contains("joint")).map(|n| n.key).collect::<Vec<_>>()
        );
        assert!(report.err_after < report.err_before, "{name} no longer resets: {} -> {}", report.err_before, report.err_after);
        // RE-PINNED 2026-11-09 (the law's leave-one-out frames, per-class
        // cells and the joint solver J103–J111; gates/j111): 0.1685 -> 0.1200
        // at the 0.25 floor, joint reading 0.2688 -> 0.1977. The grey strip
        // (16 rows, grey on both sides) pins the white balance (its cell asks
        // 0.010 ± 0.008 of the Kelvin axis, the gradient wants 0.020 the other
        // way) and the neutral chroma (0 ± one code against the grade wheels'
        // tint); the pale sky's cell asks +0.064 ln of brightness and the
        // mixer's rotation toward gold darkens it by one code past se, so the
        // chroma dials are refused on their own axes; the curves are refused
        // on the orange cells at every level. What ships: exposure, highlights,
        // a red curve (3 dof). J84's 0.0704 / 0.0493 was read with the classes
        // in composition order on the fine grid, before the law's frames.
        // RE-PINNED 2026-11-09 (J121, gates/j121-tests): a class whose
        // re-estimate from the asks moved nothing is SCALED toward the base
        // instead of read as admitted at the base, so the chroma dials and
        // the curves refused on the orange cells keep the admitted fraction
        // of the stages' shape: 0.1685 -> 0.0694 at 0.345, joint reading
        // 0.2688 -> 0.0874 (under JOINT_FAR_ERR again — no FAR classification
        // of either kind, as J84 read it). J111's 0.1200 / 0.1977 was the
        // refit returning the base and the base passing as the repair.
        // RE-PINNED 2026-11-12 (J145, gates/j145): BACK IN THE REFUSAL BAND,
        // where it stood until 2026-11-08. The landings above were carried
        // by channel curves that rotated the pale sky into the target's gold
        // and stained the grey ramp with it (19.6 codes of 255 on 55b4737,
        // the ramp matching the target to begin with;
        // `cast_must_not_rotate_the_sky_into_a_target_native_hue` was red
        // while they shipped). With the ramp heard — its ranges pooled into
        // cells with standing — the curves are refused on it, the mixer's
        // rotation still carries the rocks, and the sky's gold, which no
        // global control delivers without the ramp, is withheld: 0.1685 ->
        // 0.0761 at the 0.25 floor, joint reading 0.2688 -> 0.1214, over
        // JOINT_FAR_ERR and classified a REFUSAL (the sky's hue band is
        // one-sided), never a miss.
        assert!(
            (report.err_after - 0.0761).abs() < FIT_QUANT
                && (report.recipe.confidence - 0.25).abs() < 0.02,
            "{name}'s landing moved off the measured 0.0761 / 0.250: {} -> {} at {}",
            report.err_before, report.err_after, report.recipe.confidence
        );
        assert!(
            (after - 0.1214).abs() < 0.01,
            "…and its joint reading off the measured 0.1214: {before:.4} -> {after:.4}"
        );
        assert!(report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_REFUSED), "{name}: refusal FAR note missing: {}", report.recipe.rationale);
        assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_MISS), "{name}: a refusal emitted the miss note: {}", report.recipe.rationale);
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
        // the ungated tone arm: 0.0387 -> 0.0365 at 0.3564; 2026-11-02, the
        // mixer carrying the rocks' hue change as a band rotation (Orange
        // -18): 0.0387 -> 0.0323 at 0.4299, joint reading 0.0590 -> 0.0467,
        // see `the_fan_gate_leaves_a_legitimate_cast_alone`).
        // v1.18.0 (iii): err_* are J* in perceptual units now, so the
        // landing is re-measured on it: 0.1545 -> 0.0692 at confidence 0.642,
        // joint reading 0.1858 -> 0.0371 (2026-11-04, rel1180). The band is
        // FIT_QUANT wide, the objective's own resolution. RE-PINNED 2026-11-06
        // (rel1180 RULING-1180-sc-r4 Q-SC-2b): the mixer's saturation and
        // luminance solved jointly, each band on its own population every
        // round, ship Blue saturation -18 beside the Orange rotation and the
        // projected red curve: 0.1605 -> 0.0657 at 0.653. RE-PINNED 2026-11-08
        // (J92, gates/j84: the validity law + the detail controls' own frame,
        // J74): 0.1306 -> 0.0444 at 0.616 — the base's reading moved with
        // J33's per-pixel ruler (0.1605 -> 0.1306), the landing with the
        // law's seeds. The joint reading is printed and pinned on the next
        // run (the 0.0371 line was never reached on this code).
        eprintln!(
            "JOINT_FAMILY canyon warm: joint {before:.4} -> {after:.4}, err {:.4} -> {:.4}, confidence {:.3}",
            report.err_before, report.err_after, report.recipe.confidence
        );
        // RE-PINNED 2026-11-09 (J121, gates/j122-tests): a class whose
        // re-estimate moved nothing is scaled, not reset, so the chroma dials
        // keep the admitted fraction of the stages' cast: 0.1306 -> 0.0347 at
        // 0.699 (0.0444 / 0.616 with the refit returning the base).
        assert!(
            (report.err_after - 0.0347).abs() < FIT_QUANT
                && (report.recipe.confidence - 0.699).abs() < 0.02,
            "canyon warm's landing moved off the measured 0.0347 / 0.699: {} -> {} at {}",
            report.err_before, report.err_after, report.recipe.confidence
        );
        assert!(after < crate::fit_zoned::JOINT_FAR_ERR, "…and its joint reading lands: {before:.4} -> {after:.4}");
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
        // RE-PINNED 2026-11-09 (gates/j122d-three; the law's leave-one-out
        // frames and the joint solver): the haze fit withholds the one-sided
        // hue bands Red and Blue and lands J* 0.137 -> 0.095 with the joint
        // reading 0.149 over JOINT_FAR_ERR — a REFUSAL (the evidence was
        // one-sided), never a miss. The pre-law fit vouched those bands and
        // read under the line.
        eprintln!(
            "JOINT_FAMILY haze: joint {before:.4} -> {after:.4}, err {:.4} -> {:.4}, notes {:?}",
            report.err_before,
            report.err_after,
            report.notes.iter().filter(|n| n.key.contains("joint")).map(|n| n.key).collect::<Vec<_>>()
        );
        assert!(!report.notes.iter().any(|n| n.key == crate::rationale::keys::FIT_NOTE_JOINT_MISS), "the haze fit is never a miss: {}", report.recipe.rationale);
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
    // v1.18.0 (iii): J* changed the scalar's units on purpose (display-sRGB
    // distribution terms -> CIE76 in Lab); re-measured 0.4267 on J*, the
    // bound kept FIT_QUANT above it.
    // RE-MEASURED for v1.18.0 DEF-3 (normalised band blurs): 0.4477 (+0.0210);
    // green under
    // the old bound with DEF-3 alone reverted, and 0.4477 with each other
    // step-1 fix reverted in turn. The bound stays FIT_QUANT above it.
    assert!(
        look < 0.4477 + FIT_QUANT,
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
    // RE-READ (v1.18.0, J79): the data this test recorded has moved — the
    // haze cast now improves the bucket by far more (0.4012 -> 0.1344) than
    // the wrecking cast does (0.3280 -> 0.3117), because the mixer's Orange
    // rotation already carries the canyon-gold cast and the residual curves
    // have little left to add — so a worst-bucket reading WOULD now tell the
    // two apart. The claim stands on structure, not on that coincidence: J*
    // is the one ruler (J33) and nothing gates on the bucket; the readings
    // are kept as readings.
    eprintln!("WORST_BUCKET haze {haze_without:.4} -> {haze_with:.4}; wrecking {gold_without:.4} -> {gold_with:.4}");
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

/// Whenever the colour stage's curves were demanded and do not reach the
/// finished recipe, SOMETHING explains it: the joint objective's elimination
/// names the controls that earned less than the quantum
/// (`FIT_NOTE_UNEARNED`), the repair names the class the law scaled
/// (`FIT_NOTE_SEED_SCALED`), the terminal base candidate says the solve
/// regressed (`FIT_NOTE_REGRESSED`), or the pair went to Atmosphere mode.
/// R23-6 A-2 pinned the retired cast gate's silent arm the same way (its
/// census of 545 stage runs found the aggregate-ratio arm never fired alone);
/// the stage judges nothing now, so the explanation is the joint solve's.
#[test]
fn an_empty_colour_stage_is_explained() {
    use crate::rationale::keys;
    for (name, src, tgt) in [
        ("canyon warm", canyon(false), canyon(true)),
        ("canyon gold", canyon(false), canyon_gold_target()),
        ("hazy canyon", hazy_canyon_source(), vivid_warm_target()),
    ] {
        let c = cast_stage_candidate(&src, &tgt);
        let demanded =
            !(c.with.red_curve.is_empty() && c.with.green_curve.is_empty() && c.with.blue_curve.is_empty());
        let rep = fit_recipe(&src, &tgt);
        let empty = rep.recipe.red_curve.is_empty()
            && rep.recipe.green_curve.is_empty()
            && rep.recipe.blue_curve.is_empty();
        eprintln!("EMPTY_STAGE {name}: demanded {demanded}, ships {}", !empty);
        if !demanded || !empty {
            continue;
        }
        assert!(
            rep.notes.iter().any(|n| {
                n.key == keys::FIT_NOTE_UNEARNED
                    || n.key == keys::FIT_NOTE_SEED_SCALED
                    || n.key == keys::FIT_NOTE_REGRESSED
                    || n.key == keys::FIT_SUMMARY_ATMOSPHERE
            }),
            "{name}: an empty colour stage must disclose WHY: {}",
            rep.recipe.rationale
        );
    }
}

/// R23-6 A-5: the disclosure names controls for THIS pair, not the
/// blanket sentence every fit carries. On the canyon-gold pair the mixer's
/// rotation carries the rocks (Red and Orange -18) and nothing global can
/// carry the pale sky into the target's gold: the channel curves that did
/// rotated it 171° and stained the grey ramp by 19.6 codes of 255 (55b4737;
/// `cast_must_not_rotate_the_sky_into_a_target_native_hue` was red while
/// they shipped), and with them this test read the pair as reproduced and
/// pinned the disclosure SILENT (J* 0.1688 -> 0.0704, gates/j78,
/// 2026-11-08). With the ramp heard (J145, 2026-11-12) the curves are
/// refused, the pair lands where the rocks take it (0.1685 -> 0.0761), the
/// sky's residual is colour-shaped, and the disclosure fires and names the
/// mixer — as it did until 2026-11-08 (gates/j145). It must NOT fire on a
/// pair the solver reproduces: a disclosure that always fires is the blanket
/// sentence again. A second positive case, on a finished zoned render, is
/// `fit_zoned::tests::unrepresented_note_is_derived_from_the_finished_zoned_render`.
#[test]
fn the_unsolvable_controls_are_named_for_this_pair() {
    let (src, tgt) = (canyon(false), canyon_gold_target());
    let rep = fit_recipe(&src, &tgt);
    {
        // The reading the note is built from, on the finished frame.
        let (thumb, t_img) = analysis_pair(&src, &tgt);
        let after = pixels_of(&render::develop_preview(&thumb, &rep.recipe));
        let tp = pixels_of(&t_img);
        let buckets = crate::fit_zoned::joint_buckets_with_evidence(
            &after,
            &tp,
            Some(&rep.evidence.source_weights),
            Some(&rep.evidence.target_weights),
        );
        let worst = |chromatic: bool| {
            buckets.iter().filter(|b| b.chromatic == chromatic).map(|b| b.err).fold(0.0f32, f32::max)
        };
        eprintln!(
            "CANYON_GOLD residual {:.4} -> {:.4}; colour-shaped {}; worst bucket chromatic {:.4} neutral {:.4}; hsl {:?} curves r{} g{} b{}",
            rep.err_before,
            rep.err_after,
            super::cast_outcome::residual_is_colour_shaped(&after, &tp, &rep.evidence),
            worst(true),
            worst(false),
            rep.recipe.hsl.hue,
            rep.recipe.red_curve.len(),
            rep.recipe.green_curve.len(),
            rep.recipe.blue_curve.len()
        );
    }
    assert!(rep.err_after < rep.err_before, "premise: canyon gold lands where the rocks take it: {} -> {}", rep.err_before, rep.err_after);
    let note = rep
        .notes
        .iter()
        .find(|n| n.key == crate::rationale::keys::FIT_NOTE_UNREPRESENTED)
        .unwrap_or_else(|| panic!("no specific disclosure: {}", rep.recipe.rationale));
    let controls = &note.args.iter().find(|(k, _)| *k == "controls").expect("arg").1;
    assert!(controls.contains("hsl"), "expected the colour mixer named, got {controls}");
    // And it must NOT fire on a synthetic pair the solver reproduces
    // exactly — a disclosure that always fires is the blanket sentence again.
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
    assert!(same_frame_plausible(&src, &crate::render::thumbnail(&synth(), 97, 97)));
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
        Note::new(keys::FIT_NOTE_UNEARNED, vec![("controls", "red curve".into())]),
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
    // "the chroma chase hit the cap" or "which controls the elimination set
    // back".
    assert!(has(keys::FIT_NOTE_SAT_PEGGED), "a solve fact was dropped");
    assert!(has(keys::FIT_NOTE_UNEARNED), "a solve fact was dropped");
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
    assert_eq!(actual.paired, expected.paired);
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
        (crate::render::thumbnail(&tgt, ANALYZE_EDGE, ANALYZE_EDGE).height(), crate::render::thumbnail(&src, ANALYZE_EDGE, ANALYZE_EDGE).height()),
        (257, 256),
        "premise: independent thumbnails disagree by one row"
    );
    let (s, t) = analysis_pair(&src, &tgt);
    assert_eq!((s.width(), s.height()), (384, 256));
    assert_eq!((t.width(), t.height()), (384, 256));
    // The target goes through the source's operator (box filter, rows
    // forced), not a different resampling kernel: a Lanczos3 arm here
    // changed a same-scene fit's residual by 1.8x on its own.
    assert_eq!(t.as_bytes(), crate::render::thumbnail_exact(&tgt, 384, 256).as_bytes());
    assert_ne!(
        t.as_bytes(),
        tgt.resize_exact(384, 256, image::imageops::FilterType::Lanczos3).as_bytes(),
        "premise: the two operators differ on this texture"
    );
    // An equal-shape pair is byte-for-byte the two thumbnails it always was.
    let same = textured(1600, 1067, 1);
    let (s2, t2) = analysis_pair(&src, &same);
    assert_eq!(s2.as_bytes(), crate::render::thumbnail(&src, ANALYZE_EDGE, ANALYZE_EDGE).as_bytes());
    assert_eq!(t2.as_bytes(), crate::render::thumbnail(&same, ANALYZE_EDGE, ANALYZE_EDGE).as_bytes());
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


/// v1.18.0 R11: the objective JUDGES every present pixel at its presence
/// weight, and the evidence model's estimation weights only DRIVE. This test
/// used to pin the opposite (J* read at `source_weights` / `target_weights`,
/// so replacing them with uniform weights had to move the score); R11 made
/// the estimation weights invisible to every term of J*, so the test now
/// pins both halves of the law that replaced it on the same fixture.
/// Mutation guards: reading J* at the estimation weights again turns the
/// first assertion red; ignoring the presence weights (uniform `w` in
/// `look_terms`) turns the second red.
#[test]
fn the_objective_judges_presence_weights_not_estimation_weights() {
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
    let judged = look_err_with_evidence(&source, &target, &evidence);
    let uniform_estimation = {
        let mut unweighted = evidence.clone();
        unweighted.source_weights.fill(1.0);
        unweighted.target_weights.fill(1.0);
        look_err_with_evidence(&source, &target, &unweighted)
    };
    let brightened_absent = {
        let mut absent = evidence.clone();
        absent.source_presence_weights[1024..].fill(0.0);
        absent.target_presence_weights[1024..].fill(0.0);
        look_err_with_evidence(&source, &target, &absent)
    };
    eprintln!(
        "PRESENCE_OBJECTIVE judged={judged:.6} uniform_estimation={uniform_estimation:.6} brightened_absent={brightened_absent:.6}"
    );
    assert_eq!(
        judged.to_bits(),
        uniform_estimation.to_bits(),
        "the estimation weights only drive: replacing them must not move J*"
    );
    // Measured on the per-pixel ruler (2026-11-07; DEF-5, presence is
    // membership): 0.076736 judged, 0.079941 with only the darkened quarter
    // present, a difference of 1.7 FIT_QUANT. Until 2026-11-07 the pooled
    // ruler with its hue and structural terms read 0.120126 and 0.077716
    // (E_paired 0.0980, H_ab 0.0137, S 0.4458).
    assert!(
        (judged - brightened_absent).abs() > FIT_QUANT,
        "J* judges at the presence weights: leaving only the darkened quarter present must move it \
         ({judged:.6} -> {brightened_absent:.6})"
    );
    assert!((judged - 0.0767).abs() < 0.005, "the presence-weighted objective calibration drifted: {judged:.6}");
}

/// The one validity law (`fit::validity`, 2026-11-07) on the cells of a
/// pair: a class leaves no cell WORSE on its own axes than it found it. A
/// cyan strip laid over a grey field at the SAME luma, whose paired target is
/// the clean grey underneath (the fixture the retired vouched-pixel arm
/// pinned): a cast that moves the strip 0.7 of the way to its target leaves
/// every cell nearer its ask and is admitted; the same cast run the other way
/// leaves the strip's cell farther than it found it and is refused, by name;
/// a tone that lifts the whole frame moves the grey cells against an ask of
/// nothing and is refused on them.
#[test]
fn the_law_admits_a_move_toward_a_cells_ask_and_refuses_one_against_it() {
    use super::validity::{Cells, Class};
    let (w, h) = (64usize, 64usize);
    let hash = |i: usize| {
        let mut v = (i as u32).wrapping_mul(747796405).wrapping_add(2891336453);
        v ^= v >> 16;
        v = v.wrapping_mul(2246822519);
        v ^= v >> 13;
        (v % 1000) as f32 / 1000.0 - 0.5
    };
    let (mut cur, mut target, mut toward, mut away, mut lifted) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        let t = 0.10 * hash(i) + 0.14 * x as f32 / (w - 1) as f32;
        let grey = 0.34 + t;
        let clean = [grey, grey + 0.02, grey + 0.01];
        if y < 10 {
            // Luma-matched to `clean` to within 0.0005: a pure COLOUR
            // difference, so the strip's cell asks on the chroma vector only.
            let cyan = [grey - 0.22, grey + 0.10, grey + 0.18];
            cur.push(cyan);
            target.push(clean);
            toward.push(std::array::from_fn(|c| cyan[c] + 0.7 * (clean[c] - cyan[c])));
            away.push(std::array::from_fn(|c| (cyan[c] + 0.4 * (cyan[c] - clean[c])).clamp(0.0, 1.0)));
            lifted.push(cyan.map(|v| (v + 0.08).min(1.0)));
        } else {
            cur.push(clean);
            target.push(clean);
            toward.push(clean);
            away.push(clean);
            lifted.push(clean.map(|v| (v + 0.08).min(1.0)));
        }
    }
    let evidence = evidence_model(&cur, &target);
    let cells = Cells::new(&cur, &target, &evidence);
    let admitted = cells.judge(Class::Curves, &cur, &toward);
    assert!(
        admitted.admits(true),
        "a move toward every cell's ask is admitted: {}",
        cells.describe(Class::Curves, &admitted)
    );
    let refused = cells.judge(Class::Curves, &cur, &away);
    let line = cells.describe(Class::Curves, &refused);
    assert!(!refused.refused.is_empty(), "a move against the strip's ask is refused: {line}");
    assert!(line.contains("/Aqua"), "…and the refusal names the strip's cell: {line}");
    let tone = cells.judge(Class::Tone, &cur, &lifted);
    let line = cells.describe(Class::Tone, &tone);
    assert!(!tone.refused.is_empty(), "a lift of cells that ask nothing is refused: {line}");
    // The tone's cells are brightness ranges (hues merged, 2026-11-09): the
    // greys fill them.
    assert!(line.contains("luma["), "…on the brightness ranges the greys fill: {line}");
}

/// A cell no evidence speaks for: standing, but no paired counterpart and no
/// target population in its brightness range on the class's own axes. The
/// law may not move it under `VetoPolicy::Withhold` and discloses the move
/// under `Disclose` — the retired unsupported-range vetoes' reading, now the
/// cells' ([`Cells::blind_cell_names`]). A bright, faintly blue strip whose
/// target rows are DARK (its brightness range holds no target pixels) and
/// whose pixels do not pair: a warming moves it with no ask to measure
/// against.
#[test]
fn a_cell_no_evidence_speaks_for_is_blind_withheld_or_disclosed() {
    use super::validity::{Cells, Class};
    let (w, h) = (64usize, 64usize);
    let (mut cur, mut target, mut warmed) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        let ramp = 0.25 + 0.40 * x as f32 / (w - 1) as f32;
        if y < 10 {
            cur.push([0.84, 0.88, 0.94]);
            target.push([0.08, 0.08, 0.09]);
            warmed.push([0.92, 0.88, 0.84]);
        } else {
            let grey = [ramp, ramp, ramp];
            cur.push(grey);
            target.push(grey);
            warmed.push(grey);
        }
    }
    let mut evidence = evidence_model(&cur, &target);
    for (i, paired) in evidence.paired.iter_mut().enumerate() {
        *paired = i / w >= 10;
    }
    let cells = Cells::new(&cur, &target, &evidence);
    let verdict = cells.judge(Class::WhiteBalance, &cur, &warmed);
    let line = cells.describe(Class::WhiteBalance, &verdict);
    assert!(verdict.refused.is_empty(), "no cell with an ask was moved: {line}");
    assert!(!verdict.blind.is_empty(), "the strip's cell is blind and was moved: {line}");
    assert!(!verdict.admits(true), "Withhold refuses a blind move: {line}");
    assert!(verdict.admits(false), "Disclose admits it: {line}");
    let named = cells.blind_cell_names(&cur, &warmed).expect("…and names the cell for the disclosure");
    // The white balance's cells are hue bands (brightness merged, 2026-11-09).
    assert!(named.contains("Blue"), "the bright blue strip's cell: {named}");
    assert_eq!(cells.blind_cell_names(&cur, &cur), None, "an unmoved frame has no blind cell to name");
}

/// An Atmosphere solve whose reference is a readable shared-content
/// population judges on that population ONLY (design §1 Silent): the pixels
/// outside it — an invented half asks for a scene, not an edit — have no
/// standing and no ask. The left half pairs (a grey ramp, the same on both
/// sides); the right half is a replaced scene (blue source, red target at
/// the same brightness). A move that re-hues the invented half is refused on
/// the whole frame's cells and silent on the restricted ones.
#[test]
fn an_invented_half_is_silent_on_the_restricted_cells() {
    use super::validity::{Cells, Class};
    let (w, h) = (64usize, 64usize);
    let (mut cur, mut target, mut moved, mut shared) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        let ramp = 0.20 + 0.50 * y as f32 / (h - 1) as f32;
        if x < 32 {
            let grey = [ramp, ramp, ramp];
            cur.push(grey);
            target.push(grey);
            moved.push(grey);
            shared.push(1.0f32);
        } else {
            cur.push([0.22, 0.35, 0.80]);
            target.push([0.70, 0.25, 0.20]);
            moved.push([0.20, 0.80, 0.35]);
            shared.push(0.0f32);
        }
    }
    let mut evidence = evidence_model(&cur, &target);
    for (i, paired) in evidence.paired.iter_mut().enumerate() {
        *paired = i % w < 32;
    }
    let whole = Cells::new(&cur, &target, &evidence);
    let verdict = whole.judge(Class::Curves, &cur, &moved);
    assert!(
        !verdict.admits(false),
        "on the whole frame the invented half's cells refuse the re-hue: {}",
        whole.describe(Class::Curves, &verdict)
    );
    // The shared-content model: the frame's, scoped to the membership
    // (`evidence::shared_content_pairing`).
    let restricted = Cells::new(&cur, &target, &evidence.scoped(&target, &shared, &shared));
    let verdict = restricted.judge(Class::Curves, &cur, &moved);
    assert!(
        verdict.admits(true),
        "on the shared population the invented half is silent: {}",
        restricted.describe(Class::Curves, &verdict)
    );
}

/// The repair's disclosure: a class the law refused as the stages proposed it
/// enters the joint solve scaled, and the report says which class and how
/// much (`FIT_NOTE_SEED_SCALED`, the class one of the four `values::CLASS_*`
/// so the GUI translates it).
#[test]
fn a_scaled_seed_is_disclosed_by_class_and_fraction() {
    use crate::rationale::{keys, values};
    let px = vec![[0.4, 0.4, 0.4]; 64];
    let evidence = evidence_model(&px, &px);
    let report = compose_report(
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
            evidence: &evidence,
            structural_evidence: None,
            defer_disclosure: false,
        },
        SolveFacts {
            budget: None,
            strength: None,
            veto_cells: None,
            wb_clamped: None,
            wb_search_bound: None,
            scaled: vec![(values::CLASS_WHITE_BALANCE, 0.38), (values::CLASS_CHANNEL_CURVES, 0.0)],
            sat_pegged: None,
            evidence_refused: false,
            regressed: None,
            unearned: None,
            detail: (0.0, 0.0),
            detail_withheld: false,
            robust: None,
            paired: false,
            hsl: HslStageFacts::default(),
            grade_held: None,
            atmosphere_reference: AtmosphereReference::WholeFrame,
        },
    );
    let notes: Vec<_> = report.notes.iter().filter(|n| n.key == keys::FIT_NOTE_SEED_SCALED).collect();
    assert_eq!(notes.len(), 2, "one note per scaled class: {}", report.recipe.rationale);
    assert_eq!(
        notes[0].args,
        vec![("class", values::CLASS_WHITE_BALANCE.to_string()), ("lambda", "0.38".to_string())]
    );
    assert!(
        report.recipe.rationale.contains("white balance the stages proposed")
            && report.recipe.rationale.contains("0.38 of that estimate"),
        "{}",
        report.recipe.rationale
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
    // RE-PINNED 2026-11-09 (the control-validity law, gates/j104–j111). Until
    // then this pinned the pre-law Atmosphere ruler: whole-frame medians
    // brightened the withheld top with the paired bottom (tone_delta > 0.05).
    // The law reads the opposite. The withheld top's population asks SPLIT —
    // the checker's quantile map sends the lower half to 0.70 (darker, −0.12
    // ln) and the upper half to 0.96 (brighter) — and no tone serves both, so
    // the tone must leave every withheld cell inside its asks (|move − ask| ≤
    // |ask| + se; a global brightening read move 0.39 against −0.12 and was
    // refused, gates/j104). What ships lifts the paired bottom through the
    // exposure and the master curve and holds the withheld top within one
    // code (tone_delta 0.0034): J* 0.0467 -> 0.0155 (gates/v6-withheld,
    // 2026-11-11). The claim is the law's verdict on the withheld cells, read
    // here as the solver reads it (the tone's leave-one-out frame against the
    // finished render).
    {
        use super::joint::{without_class, Refusal};
        use super::validity::{Cells, Class};
        let (s_img, t_img) = analysis_pair(&source, &target);
        let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
        let tp = pixels_of(&t_img);
        let evidence = evidence_model_paired(&sp, &tp, None, s_img.width(), s_img.height());
        let cells = Cells::new(&sp, &tp, &evidence);
        let frame = without_class(&EditRecipe::default(), &report.recipe, Refusal::Tone);
        let before_px = pixels_of(&render::develop_preview(&s_img, &frame));
        let after_px = pixels_of(&render::develop_preview(&s_img, &report.recipe));
        let verdict = cells.judge(Class::Tone, &before_px, &after_px);
        eprintln!("WITHHELD_TONE {} tone_delta {tone_delta:.4} delta {delta:.4}", cells.describe(Class::Tone, &verdict));
        assert!(
            verdict.admits(true),
            "the tone that ships is admitted on every cell, the withheld range's included: {}",
            cells.describe(Class::Tone, &verdict)
        );
    }
    assert!(
        tone_delta < 0.05,
        "a tone that brightened the withheld top with the paired bottom moved its lower half against its ask ({delta:.4}, tone {tone_delta:.4}): ev={:.2} c={:.1} h={:.1} s={:.1} w={:.1} b={:.1} curve={:?}; {}",
        report.recipe.exposure_ev,
        report.recipe.contrast,
        report.recipe.highlights,
        report.recipe.shadows,
        report.recipe.whites,
        report.recipe.blacks,
        report.recipe.tone_curve,
        report.recipe.rationale,
    );
    // RE-PINNED 2026-11-11 (gates/v6-withheld; the tone's other carrier and
    // the elimination's absence inside G, J139). The paired bottom is served
    // by a tone that holds the withheld top: under the bounded Atmosphere
    // curve (user ruling 2026-11-09) the shape is the stage's, and the stage
    // states one tone two ways. The exposure with a curve that undoes it over
    // the withheld ranges solves to nothing — the exposure brightens the top
    // against two cells' asks, and what the repair keeps of it earns less
    // than its price (0.0447 on five controls, eliminated to the base). The
    // curve alone — the withheld ranges pinned where they stand — is repaired
    // into an exposure beside the curve, solved the same way and ships:
    // 0.0467 -> 0.0155 on [exposure, tone curve]. Its exposure stays: without
    // it the law refuses the point, the absence is scaled into the law and
    // reads worse there (`Joint::absence`; the elimination took 18 renders).
    // From 2026-11-10 to J139 that absence was re-estimated from the cells'
    // asks into [shadows, tone curve] and taken at F 0.03033 against 0.03072
    // — four units of price for 0.0072 of the look (0.0467 -> 0.0227, 878
    // renders, gates/v2-withheld). Before that nothing shipped (0.0467 either
    // way, gates/j121-tests) and, once the curve was proposed, the
    // elimination took an absence the law refuses (gates/v1-withheld).
    // MUTATION (J139), as it read until 2026-11-12: repair the absence as a
    // start is repaired (`repair_with(.., REPAIR_REFITS)` in
    // `Joint::absence`) and the exposure is dropped.
    // RE-PINNED 2026-11-12 (gates/j145). The start that wins is no longer
    // the stage's. The asks' own curve (J142) — the withheld ranges where
    // they stand, the paired bottom on its cells' asks — enters as a start
    // of its own, needs no exposure beside it and reads 0.0467 -> 0.0094 on
    // [tone curve], F 0.02083 against 0.03072 for the stage's start. The
    // exposure is 0 because the recipe that ships never had one, and "the
    // exposure stays" pinned a start that lost. J139's mutation was run
    // again on this code: the stage's start ends on [shadows, tone curve] at
    // 0.0227 as before, the asks' curve still wins at 0.0094, and neither
    // this toy's shipped recipe nor any of the 120 quick fit tests tells the
    // two eliminations apart (gates/j145, mut-j139-*). What J139 changed
    // that can still be read is the elimination's cost — 18 renders against
    // 878 on this toy's stage start, p36 and p39 not ending a fit — and the
    // real pairs' rounds read it. The claim here is the landing: the tone
    // that holds the withheld top serves the paired bottom no worse than it
    // did on 2026-11-11.
    assert!(
        report.err_after <= 0.0155 + FIT_QUANT,
        "the tone that holds the withheld top serves the paired bottom no worse than the 0.0155 of 2026-11-11: {} -> {}",
        report.err_before,
        report.err_after
    );
    eprintln!("WITHHELD_TONE err {} -> {}", report.err_before, report.err_after);
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

/// The structural reading of two photographs, imposed on a synthetic pair's
/// model: every cell past the zone line, no pixel paired. (The instrument
/// reads two generated frames as more alike than two photographs ever are,
/// so a synthetic pair cannot earn the reading; the different-photo pair of
/// the calibration corpus does. `fit_zoned::field::tests` imposes it the
/// same way for J141.)
fn two_photographs_model(sp: &[[f32; 3]], tp: &[[f32; 3]], w: u32, h: u32) -> EvidenceModel {
    let mut ingredients = evidence_model_paired(sp, tp, None, w, h);
    ingredients.spatial_weights.fill(0.0);
    ingredients.spatial_divergence.fill(1.2);
    ingredients.spatial_supported.fill(false);
    ingredients.paired.fill(false);
    ingredients.globally_same_content = false;
    let ones = vec![1.0; sp.len()];
    ingredients.scoped(tp, &ones, &ones)
}

/// J142. A frame nothing pairs on has no withheld range: the identity pins
/// are for ranges withheld FROM a pairing. The withheld-top fixture pairs
/// (its ground is one scene) and its checker sky is pinned; the same frames
/// read as two photographs carry no weight on any range, and until J142
/// every populated range was pinned — the stage's tone was the identity.
/// MUTATION (J142-D): drop the `pairs_a_population` return in
/// `withheld_identity_pins` — the two-photographs model pins its ranges.
#[test]
fn a_frame_nothing_pairs_on_has_no_withheld_range() {
    let (w, h) = (192u32, 128u32);
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = if y < 43 { 0.72 + 0.20 * x as f32 / (w - 1) as f32 } else { 0.12 + 0.42 * x as f32 / (w - 1) as f32 };
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
    let (s_img, t_img) = analysis_pair(&source, &target);
    let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
    let tp = pixels_of(&t_img);
    let one_scene = evidence_model_paired(&sp, &tp, None, s_img.width(), s_img.height());
    assert!(one_scene.pairs_a_population(), "premise: the ground pairs");
    let pinned = super::atmosphere::withheld_identity_pins(&sp, &sp, &one_scene);
    assert!(!pinned.is_empty(), "premise: one scene with a withheld range pins it");
    let two = two_photographs_model(&sp, &tp, s_img.width(), s_img.height());
    assert_eq!(two.paired_share(), 0.0, "premise: no cell of the frame pairs");
    let unweighted = two.luma.iter().filter(|r| r.weight <= 0.0 && r.source_share >= EVIDENCE_MIN_SHARE).count();
    assert!(unweighted > pinned.len(), "premise: read as two photographs, more ranges carry no weight ({unweighted})");
    assert!(
        super::atmosphere::withheld_identity_pins(&sp, &sp, &two).is_empty(),
        "nothing is withheld from a pairing that does not exist"
    );
}

/// J142. Two photographs' tone is carried by the law's asks. The source is
/// a bright frame — a sky band over a mid-bright ground; the target another
/// photograph with the same share of the same sky, laid out elsewhere, over
/// a ground near black. The population asks are the ground far down and the
/// sky where it is, and one exposure with three quantile knots cannot say
/// that: the exposure dims the sky against its ask, the quantile curve's top
/// segment drags the sky's lower half down with the ground, and either shape
/// is scaled toward the base until the sky is admitted. The asks' curve
/// holds the sky by construction and takes the ground as far as the slope
/// budget lets a curve go. MUTATION (J142-C): do not offer the asks' start
/// (`alternatives` without "the asks carry the tone") — the ground stays
/// within a few codes of where it was.
#[test]
fn two_photographs_tone_is_carried_by_the_asks() {
    let (w, h) = (192u32, 128u32);
    let source = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = if y < 48 { 0.80 + 0.16 * x as f32 / (w - 1) as f32 } else { 0.30 + 0.35 * x as f32 / (w - 1) as f32 };
        image::Rgb([(v * 255.0).round() as u8; 3])
    }));
    let target = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = if x < 72 { 0.80 + 0.16 * y as f32 / (h - 1) as f32 } else { 0.03 + 0.07 * y as f32 / (h - 1) as f32 };
        image::Rgb([(v * 255.0).round() as u8; 3])
    }));
    let (s_img, t_img) = analysis_pair(&source, &target);
    let base = EditRecipe::default();
    let sp = pixels_of(&render::develop_preview(&s_img, &base));
    let tp = pixels_of(&t_img);
    let structural = two_photographs_model(&sp, &tp, s_img.width(), s_img.height());
    assert_eq!(structural.paired_share(), 0.0, "premise: no cell of the frame pairs");
    let report = fit_atmosphere_from_parts(
        &s_img,
        &sp,
        &tp,
        &base,
        false,
        DivergencePair { fine: None, coarse: None },
        &structural,
        false,
        crate::recipe::GradeStrength::new(0.85),
        None,
    );
    let after = pixels_of(&render::develop_preview(&s_img, &report.recipe));
    let mean_move = |pick: &dyn Fn(f32) -> bool, size: bool| -> f32 {
        let moves: Vec<f32> = sp
            .iter()
            .zip(&after)
            .filter(|(s, _)| pick(luma601(s)))
            .map(|(s, a)| if size { (luma601(a) - luma601(s)).abs() } else { luma601(a) - luma601(s) })
            .collect();
        moves.iter().sum::<f32>() / moves.len().max(1) as f32
    };
    let ground = mean_move(&|l| l < 0.70, false);
    let sky = mean_move(&|l| l > 0.78, true);
    eprintln!(
        "ASKS_TONE err {} -> {} ground {ground:+.4} sky {sky:.4} ev={:.2} curve={:?}; {}",
        report.err_before,
        report.err_after,
        report.recipe.exposure_ev,
        report.recipe.tone_curve.iter().map(|k| (k.input, k.output)).collect::<Vec<_>>(),
        report.recipe.rationale
    );
    {
        use super::joint::{without_class, Refusal};
        use super::validity::{Cells, Class};
        let blind = structural.structure_blind(&tp);
        let cells = Cells::new(&sp, &tp, &blind);
        let frame = without_class(&base, &report.recipe, Refusal::Tone);
        let before_px = pixels_of(&render::develop_preview(&s_img, &frame));
        let verdict = cells.judge(Class::Tone, &before_px, &after);
        assert!(
            verdict.admits(false),
            "the tone that ships is admitted on every range: {}",
            cells.describe(Class::Tone, &verdict)
        );
    }
    assert!(ground < -0.20, "the ground is taken down as its ranges ask ({ground:+.4}): {}", report.recipe.rationale);
    assert!(sky < 0.01, "the sky stays where its ranges ask it to ({sky:.4}): {}", report.recipe.rationale);
    assert!(
        report.err_after < 0.5 * report.err_before,
        "the pair closes more than half its distance: {} -> {}",
        report.err_before,
        report.err_after
    );
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
                veto_cells: None,
                wb_clamped: None,
                wb_search_bound: None,
                scaled: Vec::new(),
                sat_pegged: None,
                evidence_refused: false,
                regressed: None,
                unearned: None,
                detail: (0.0, 0.0),
                detail_withheld: false,
                robust: None,
                paired: false,
                hsl: HslStageFacts::default(),
                grade_held: None,
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
    let base = if raw.exists() {
        crate::pipeline::calibration_recipe(crate::pipeline::fit_calibration(&raw))
    } else {
        EditRecipe::default()
    };
    // v1.17.0: the source is developed under the base's camera profile — the
    // profile stage runs inside the RAW develop, so a base that names one
    // composes onto pixels that already carry it (`pipeline::fit_source`).
    let source = if raw.exists() {
        render::base_develop(&raw, &render::BasePart::of(&base), Some(ANALYZE_EDGE))
            .expect("develop calibration source.arw")
    } else {
        image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg")
    };
    let source = crate::render::thumbnail(&source, ANALYZE_EDGE, ANALYZE_EDGE);
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
    // v1.18.0 R12: the curve solved on the exposure earns 0.000835 of J*
    // here, under FIT_QUANT, and is set back (`eliminate_unearned`); the
    // recipe is the exposure alone at its -1.00 budget edge. The old sum
    // `|ev| + |contrast| + curve points + |sat| > 1.0` counted the curve's
    // five points and reads exactly 1.0 without them, so the non-empty check
    // is now the recipe's own: some control moved.
    assert!(
        report.recipe.exposure_ev.abs() > 0.001
            || report.recipe.contrast.abs() > 0.001
            || !report.recipe.tone_curve.is_empty()
            || report.recipe.saturation.abs() > 0.001,
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
        "CONFIDENCE_SEPARATION conservative={:.3} (J* {:.4}) preferred={:.3} (J* {:.4}) margin={:.3} movement={:.3}/{:.3} motion_su={cs:.4}/{cu:.4} vs {ps:.4}/{pu:.4} pair_ident={:.3}",
        conservative.recipe.confidence,
        conservative.err_after,
        rescored.recipe.confidence,
        rescored.err_after,
        rescored.recipe.confidence - conservative.recipe.confidence,
        movement_identifiability(&conservative_px, evidence),
        movement_identifiability(&preferred_px, evidence),
        evidence.identifiability,
    );
    dump_recipe("confidence conservative", &conservative);
    dump_recipe("confidence preferred", &rescored);
    // RE-READ (v1.18.0, J83): the two recipes are read by ONE ruler on ONE
    // evidence model (bit-equal, asserted above), and confidence is that
    // ruler's residual through `confidence_from_look_err` — so two recipes
    // with different residuals carry different confidences, in the
    // residual's order: measured 0.480 against 0.297 for the preferred
    // recipe's larger J*. Until 2026-11-08 the line was |Δ| < 0.01, a
    // coincidence of the pooled (σ 2 px) ruler the per-pixel reading replaced.
    assert!(
        (rescored.recipe.confidence - conservative.recipe.confidence)
            * (conservative.err_after - rescored.err_after)
            >= 0.0,
        "confidence follows the one ruler's residual: {:.3} at J* {:.4} vs {:.3} at J* {:.4}",
        rescored.recipe.confidence,
        rescored.err_after,
        conservative.recipe.confidence,
        conservative.err_after
    );
    // The former premise "the two recipes remain structurally
    // distinguishable (movement identifiability apart by 0.1)" served the
    // old claim and is none of the new one's; it read 0.579 vs 0.669 on the
    // law's fit (gates/j84, 2026-11-08). The readings stay in the print above.
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

/// (c) The hue axis rotates no band its paired pixels do not ask to rotate,
/// on any pair, at any strength: a saturation edit, an over-cap one, a
/// one-sided band, an unpaired cloud deck and a structural permutation all
/// keep every band's hue at 0.
///
/// Until 2026-11-02 this test read "the mixer never rotates a hue band" and
/// carried a band-rotated target (Green -60 / Blue +60) among its pairs. The
/// mixer now solves band hue from the paired pixels' own rotation
/// ([`a_band_rotated_a_few_degrees_is_solved_back_to_that_band_and_no_other`]),
/// so that pair is the inverse problem the stage answers, not a temptation it
/// refuses; it left this list for that test.
#[test]
fn the_per_band_mixer_rotates_no_band_its_paired_pixels_do_not_ask() {
    let mut pairs: Vec<(&str, DynamicImage, DynamicImage)> = vec![
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

/// The band-hue solve's inverse problem: the target is the source with ONE
/// band rotated a few degrees through the real engine (Blue +15, 4.5°), and
/// the fit rotates that band back and no other. The four families sit on
/// band centres (red, green, blue, magenta), so the rotated band's pixels
/// belong to it alone and the other bands' pixels ask nothing.
/// One control unit (0.3° of rotation): the target is 8-bit, so the rotated
/// family's hue is read through code-value rounding, and the solve rounds
/// its amount to 0.1. Measured 2026-11-02: Blue +15 solved back as +15.5,
/// look error 0.0072 -> 0.0025, with one equation per band; as +15.1, look
/// error 0.0018, with one per luma range (d) — each range's median against
/// that range's own reach, where the darker ranges' lower chroma fades the
/// rotation and a band-mean reach under a median demand overshot (+16.2).
const BAND_HUE_TOLERANCE: f32 = 1.0;
#[test]
fn a_band_rotated_a_few_degrees_is_solved_back_to_that_band_and_no_other() {
    let source = band_frame([0, 1, 2, 4], [0.55; 4]);
    let target = developed(&source, |hsl| hsl.hue[5] = 15.0);
    let report = fit_recipe(&source, &target);
    let hue = report.recipe.hsl.hue;
    eprintln!("BAND_HUE rotated {hue:?} err {:.4} -> {:.4}", report.err_before, report.err_after);
    assert!((hue[5] - 15.0).abs() <= BAND_HUE_TOLERANCE, "Blue solved back: {hue:?}");
    for band in (0..8).filter(|&b| b != 5) {
        assert_eq!(hue[band], 0.0, "band {band} rotated although its pixels ask nothing: {hue:?}");
    }
    let moved = note_arg(&report, crate::rationale::keys::FIT_NOTE_HSL_BANDS, "moved");
    assert!(moved.contains("Blue hue +"), "the report names the band that rotated: {moved:?}");
}

/// The band-hue rule's veto, the white balance's own ([`wb_consensus`]) on
/// the white balance's own unit, the band's luma ranges: the Blue family
/// rotated +4.5° on the darker half of its brightness ramp and -4.5° on the
/// brighter half — one band whose brightness ranges ask opposite rotations
/// at equal mass — ships no rotation on any band. A rotation that changes
/// sign with brightness is not a band's move.
///
/// Until 2026-11-02 (d) the fixture alternated the two rotations row by row
/// at the same brightness, because the demand was a consensus over pixels;
/// a range's median now stands for its pixels, so the opposition the rule
/// reads is between ranges, and the fixture puts it there.
#[test]
fn a_band_whose_ranges_ask_opposite_rotations_ships_nothing() {
    let source = band_frame([0, 1, 2, 4], [0.55; 4]);
    let up = developed(&source, |hsl| hsl.hue[5] = 15.0).to_rgb8();
    let down = developed(&source, |hsl| hsl.hue[5] = -15.0).to_rgb8();
    // band_frame's brightness ramps with x: the left half is the darker.
    let target = DynamicImage::ImageRgb8(RgbImage::from_fn(up.width(), up.height(), |x, y| {
        if x < up.width() / 2 { *up.get_pixel(x, y) } else { *down.get_pixel(x, y) }
    }));
    let report = fit_recipe(&source, &target);
    eprintln!("BAND_HUE opposed {:?}", report.recipe.hsl.hue);
    assert_eq!(report.recipe.hsl.hue, [0.0f32; 8], "opposite rotations ask nothing");
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
    // RE-READ on the one law (v1.18.0, J76): the only change on this pair is
    // the sky's content, which no control explains — the ground's cells ask
    // nothing (0.0000 ± 0.01) and the tone the stages proposed moved them by
    // 0.2, so it is scaled to the base, the chroma dials to 0.016 of their
    // estimate — and the frame is left where it was (0.0748 -> 0.0748), not
    // reset. The premise is that nothing terminal stands behind the neutral
    // mixer, which the REGRESSED check reads; until 2026-11-07 the frame also
    // ended a hair closer (0.0296 -> 0.0294 on the display ruler).
    eprintln!("PER_BAND_REFUSED {:.4} -> {:.4}: {}", report.err_before, report.err_after, report.recipe.rationale);
    assert!(
        report.err_after <= report.err_before,
        "premise: the fit leaves the frame no worse ({:.4} -> {:.4})",
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
    let unearned = note_arg(&report, crate::rationale::keys::FIT_NOTE_UNEARNED, "controls");
    assert!(
        crate::recipe::HSL_BANDS.iter().any(|band| unearned.contains(*band)),
        "…and say so (the unearned list names the band): {}",
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
///
/// RE-READ for v1.18.0 DEF-5 (presence is membership): it was
/// `the_finished_frame_arbitration_withdraws_a_synthetic_mixer_that_costs_it`.
/// At strength 0 the finished frame now PAYS for the mixer (Orange
/// saturation +6, Red/Orange luminance +6 / -4.5): J* 0.05835 with it,
/// 0.07288 with it given back (6.3 FIT_QUANT; 0.05632 / 0.07556 with DEF-3
/// also reverted), so the arbitration keeps it at both strengths, and what
/// this pair witnesses is the keeping side: the arbitration reads the state.
/// The withdrawing side has no fixture under DEF-5 — deleting the loop's
/// withdrawal leaves this test green; stated, not hidden.
#[test]
fn the_finished_frame_arbitration_keeps_a_synthetic_mixer_the_finished_frame_pays_for() {
    // RED on v1.17.0, a defect, not a stale image (measured): on this pair
    // the ±6 mixer no longer costs the finished frame at strength 0 (0.02444
    // against 0.02436, inside the 1e-4 margin), and where 4a' does find a
    // costing mixer — the two-family frame at edit 20 whose target also
    // carries a red curve lift +6 and a blue curve cut 4: 0.0360 against
    // 0.0144, at strength 0 and 1 alike — it compares against
    // a neutral alternative whose cast curves the terminal delivered-fan
    // check then withdraws (share 0.47, 19.4°), so the shipped recipe falls
    // back to the base by the terminal reset (0.0357 -> 0.0357). The 4a'
    // comparison must judge the alternative as it would ship; that is
    // `solve.rs`'s to fix, and this test stands as written.
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
        !low.recipe.hsl.is_neutral(),
        "the finished frame pays for the mixer, so it stays: {}",
        low.recipe.rationale
    );
    let mut neutral = low.recipe.clone();
    neutral.hsl = crate::recipe::Hsl::default();
    let given_back = rescore_report(&src, &tgt, &neutral, &EditRecipe::default(), low.err_before, &low.notes).err_after;
    let kept = rescore_report(&src, &tgt, &low.recipe, &EditRecipe::default(), low.err_before, &low.notes).err_after;
    assert!(
        ships(kept, given_back),
        "…by the arbitration's own reading: {given_back:.5} without it, {kept:.5} with it"
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

/// v1.2.4 B1 asked what the fan gate costs on a REAL two-temperature scene:
/// `p40` (a night street under neon beneath a fiery sky) and `p41` (a
/// building at twilight, a warm horizon, a deep blue sky, a warm lamp). The
/// history of that reading (13.5° → 11.0° → 12.1° → 11.4° → 6.3° → 14.6° →
/// 12.5°, then refused at 16.1° under J*; p41 admitted at 9.0° → 10.3° →
/// 4.0° → 13.5°) is in the v1.17.x ledgers. Under the one validity law the
/// question is the picture's: both pairs ship frames that do not fan a hue
/// class past the visibility edge, and land nearer the target. Both are
/// OPTIONAL, like every corpus pair; the lens-profile premise is v1.17.0's.
#[test]
fn the_two_temperature_pairs_ship_without_a_hue_fan() {
    let Some(root) = calibration_corpus() else { return };
    for code in ["p40", "p41"] {
        let raw = root.join(format!("{code}.arw"));
        let target_path = root.join(format!("{code}-target.jpg"));
        if !raw.is_file() || !target_path.is_file() {
            crate::test_skipped(
                "the_two_temperature_pairs_ship_without_a_hue_fan",
                &format!("the two-temperature pair {code} is not in this corpus"),
            );
            continue;
        }
        // Loaded exactly as CLI `match` loads a RAW: the frame developed at
        // the default recipe on a 2048 edge, against the calibration recipe
        // as the base.
        let src = render::render_to_image(&raw, &EditRecipe::default(), None, Some(2048))
            .expect("develop the two-temperature RAW");
        let tgt = image::open(target_path).expect("the finished rendition");
        let base = crate::pipeline::calibration_recipe(crate::pipeline::fit_calibration(&raw));
        assert!(
            base.lens_profile.adobe.is_some(),
            "{code}: the reading is measured with the lens's Adobe profile from this machine's pool, and the pool has none for it here"
        );
        let report = fit_recipe_from(&src, &tgt, &base);
        let fan = delivered_fan_of(&src, &tgt, &report);
        let curves = !(report.recipe.red_curve.is_empty()
            && report.recipe.green_curve.is_empty()
            && report.recipe.blue_curve.is_empty());
        eprintln!(
            "TWO_TEMPERATURE {code} temp={:?} shipped_fan={fan:.1} curves={curves} err={:.6}->{:.6} conf={:.6}",
            report.recipe.temperature_k, report.err_before, report.err_after, report.recipe.confidence
        );
        assert!(fan < FAN_VISIBLE_DEG, "{code}: the shipped frame fans {fan:.1}° against the {FAN_VISIBLE_DEG}° edge");
        assert!(
            report.err_after < report.err_before,
            "{code}: the fit lands nearer the target: {:.6} -> {:.6}",
            report.err_before,
            report.err_after
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

/// (f) The unrepresented-controls disclosure names `hsl` while the residual
/// still has the shape of a per-band colour job, and stops once the fit has
/// closed it: on a gap the default budget leaves open, and on a narrower one
/// the full-strength fit closes.
///
/// Since the attribution law at the mixer (rel1180 RULING-1180-sc-r4
/// Q-SC-2a) a move confined to two bands is theirs, and the default closes
/// the ±50 two-band job through the mixer alone: Green saturation -18,
/// Blue +18 / luminance -8, every tone slider and the saturation at zero,
/// J* 0.213 -> 0.0448, the residual no longer colour-shaped (the same recipe
/// with the mixer zeroed still is), so `hsl` is not named — the law's
/// reading, measured 2026-11-06. The naming is witnessed on the ±80 job,
/// which the default's per-band ceiling leaves open by the same law
/// (Green -18, Blue +18 / -7.1, J* 0.346 -> 0.266, colour-shaped).
///
/// Until v1.18.0 this test pinned the MIXER as what took the shape out: at
/// the default it attached and half-closed the ±50 gap, and at strength 1
/// the same recipe with the mixer zeroed was still colour-shaped. Under J*
/// and R8 the premise is gone on both counts, measured 2026-11-04. At the
/// default the finished-frame arbitration gives the mixer back (the cast
/// curves, admitted at +8.8 degrees, buy what it bought, and a mixer ships
/// only on a gain of FIT_QUANT over its absence), the residual stays
/// colour-shaped, and at J* 0.0495 it is above `FIT_QUANT_CLEAN`, so the
/// disclosure names `hsl`. At strength 1 the mixer attaches and the finished
/// residual is no longer colour-shaped — but neither is the same recipe with
/// the mixer zeroed, so the closing is not attributable to the mixer alone,
/// and that half is not asserted. The ±50 default half described in this
/// paragraph is the pre-law reading.
#[test]
fn a_per_band_residual_stays_named_until_the_full_strength_fit_closes_it() {
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

    // The default budget on the ±50 job: closed by the mixer (the law's
    // reading, printed; see the doc).
    let partial = fit(0.65);
    let partial_px = pixels_of(&render::develop_preview(&s_img, &partial.recipe));
    eprintln!(
        "PER_BAND default ±50: J* {:.4}, colour-shaped {}, names hsl {}",
        partial.err_after,
        residual_is_colour_shaped(&partial_px, &tp, &evidence),
        names_hsl(&partial)
    );

    // The wide gap: ±80 demanded. The mixer attaches, the residual is still a
    // per-band colour job well clear of the floor, and it is named.
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
    assert!(!wide.recipe.hsl.is_neutral(), "premise: the mixer attaches on the wide gap");
    assert!(
        wide.err_after > FIT_QUANT_CLEAN,
        "…and the residual stays ABOVE the disclosure floor {FIT_QUANT_CLEAN}: {}",
        wide.err_after
    );
    assert!(
        residual_is_colour_shaped(&wide_px, &wide_tp, &wide_evidence),
        "a gap this far past the ceiling is still a per-band colour residual"
    );
    assert!(names_hsl(&wide), "…so the disclosure must still name it: {}", wide.recipe.rationale);

    // Strength 1: the fit closes the shape and the disclosure goes quiet.
    let full = fit(1.0);
    let full_px = pixels_of(&render::develop_preview(&s_img, &full.recipe));
    assert!(!full.recipe.hsl.is_neutral(), "premise: the mixer attaches at full strength");
    assert!(
        !residual_is_colour_shaped(&full_px, &tp, &evidence),
        "the full-strength fit leaves no per-band colour residual"
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


/// The white-balance path walks from As Shot: the zero end of the path is the
/// as-shot Kelvin with no tint move (Tint is relative, v1.17.0, the Camera
/// Raw law family removed) and the far end the free fit, and every point
/// between renders between them.
///
/// MUTATION: start `wb_path_candidate` anywhere but the law's As Shot.
#[test]
fn the_white_balance_path_starts_at_as_shot() {
    let law = render::WbLaw::of(&EditRecipe { as_shot_k: Some(5950.0), as_shot_tint: Some(13.0), ..EditRecipe::default() });
    assert_eq!(wb_path_candidate(law, 8800.0, 16.6, 0.0), (5950.0, 0.0));
    assert_eq!(wb_path_candidate(law, 8800.0, 16.6, 1.0), (8800.0, 16.6));
    let (k, t) = wb_path_candidate(law, 8800.0, 16.6, 0.5);
    assert!(k > 5950.0 && k < 8800.0, "half way is between in Kelvin: {k}");
    assert!(t > 0.0 && t < 16.6, "and in tint: {t}");
}

/// The fan the user sees: the shipped recipe's render against the straight
/// conversion, at analysis size, over the shared evidence
/// ([`hue_fan_weighted`], a test instrument). 0 when no region-sized hue class
/// reads.
fn delivered_fan_of(src: &DynamicImage, tgt: &DynamicImage, report: &FitReport) -> f32 {
    let (s_img, t_img) = analysis_pair(src, tgt);
    let sp = pixels_of(&render::develop_preview(&s_img, &EditRecipe::default()));
    let tp = pixels_of(&t_img);
    let evidence = evidence_model_for(&sp, &tp, s_img.width(), s_img.height());
    let px = pixels_of(&render::develop_preview(&s_img, &report.recipe));
    hue_fan_weighted(&sp, &px, &evidence).map(|(_, fan, _)| fan).unwrap_or(0.0)
}

/// The hue fan's visibility edge, one class of the census below: the v1.2.3
/// fan gate was calibrated at it (2026-09-01: the Cornwall wreck 37.6°, the
/// synthetic Cornwall shape 44.6°, the admitted haze correction 7.8°,
/// canyon-warm 7.5°, canyon-gold 5.2°, hazy→vivid 2.7°; at 20° the Cornwall
/// sky still shipped a 20.6° fan, at 15° its spread across luminance octiles
/// was 1.6°, the target's own coherence). The gate is retired — under the one
/// validity law a fan is a refusal on its own reading, each brightness slice
/// of the class a cell moved against its ask — and this number is the bar
/// the tests read on the DELIVERED frame.
const FAN_VISIBLE_DEG: f32 = 15.0;
/// The census's hue classes (15° each) and the share a class, and a
/// brightness slice inside it, must hold to count as a region (0.05 = a
/// region, not speckle; calibrated by sweep with the edge above).
const FAN_HUE_CLASSES: usize = 24;
const FAN_SHARE: f32 = 0.05;

/// Test INSTRUMENT, not a gate: the hue fan an edit opens. Over the
/// evidence-weighted pixels chromatic enough to carry a hue on both sides
/// (chroma ≥ 0.03 before, ≥ 0.04 after — the retired gates' measurability
/// floors), the pixels are classed by their BEFORE hue into
/// [`FAN_HUE_CLASSES`] and sliced by brightness range; for the class the edit
/// spreads the most, returns `(share, added, delivered)` — its share of the
/// census population, the spread the edit ADDS between its slices' mean hues
/// (signed: negative narrows), and the absolute spread it carries after. The
/// v1.2.3 gate's own census, kept so the picture claims that gate guarded
/// (`a_coherent_sky_cast_the_controls_carry_as_a_gain_ships_without_a_fan`,
/// `no_shipped_cast_fans_a_hue_class_and_the_colour_is_delivered`) are still
/// measured on what ships.
fn hue_fan_weighted(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<(f32, f32, f32)> {
    let mut mass = [[0.0f32; EVIDENCE_LUMA_BINS]; FAN_HUE_CLASSES];
    let mut before = [[(0.0f32, 0.0f32); EVIDENCE_LUMA_BINS]; FAN_HUE_CLASSES];
    let mut after = [[(0.0f32, 0.0f32); EVIDENCE_LUMA_BINS]; FAN_HUE_CLASSES];
    let mut population = 0.0f32;
    for (i, (c, w)) in cur.iter().zip(with_px).enumerate() {
        let weight = evidence.source_hue_weights.get(i).copied().unwrap_or(0.0).max(0.0);
        if weight <= 0.0 { continue; }
        population += weight;
        let cc = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        let wc = w[0].max(w[1]).max(w[2]) - w[0].min(w[1]).min(w[2]);
        if cc < 0.03 || wc < 0.04 { continue; }
        let h0 = render::rgb_to_hsl(c[0], c[1], c[2]).0;
        let h1 = render::rgb_to_hsl(w[0], w[1], w[2]).0;
        let class = ((h0 * FAN_HUE_CLASSES as f32) as usize).min(FAN_HUE_CLASSES - 1);
        let bin = evidence_luma_bin(luma601(c));
        mass[class][bin] += weight;
        let (a0, a1) = (h0 * std::f32::consts::TAU, h1 * std::f32::consts::TAU);
        before[class][bin].0 += a0.sin() * weight;
        before[class][bin].1 += a0.cos() * weight;
        after[class][bin].0 += a1.sin() * weight;
        after[class][bin].1 += a1.cos() * weight;
    }
    if population <= 0.0 {
        return None;
    }
    // Widest circular gap between the slice means, pairwise.
    let spread = |values: &[f32]| -> f32 {
        let mut worst = 0.0f32;
        for (i, a) in values.iter().enumerate() {
            for b in &values[i + 1..] {
                let mut d = (b - a).abs() % 360.0;
                if d > 180.0 { d = 360.0 - d; }
                worst = worst.max(d);
            }
        }
        worst
    };
    let mean = |(sin, cos): (f32, f32)| sin.atan2(cos).to_degrees().rem_euclid(360.0);
    let mut worst: Option<(f32, f32, f32)> = None;
    for class in 0..FAN_HUE_CLASSES {
        let class_mass: f32 = mass[class].iter().sum();
        let share = class_mass / population;
        if share < FAN_SHARE { continue; }
        let (mut was, mut now) = (Vec::new(), Vec::new());
        for bin in 0..EVIDENCE_LUMA_BINS {
            if mass[class][bin] < class_mass * FAN_SHARE { continue; }
            was.push(mean(before[class][bin]));
            now.push(mean(after[class][bin]));
        }
        // One slice cannot fan.
        if was.len() < 2 { continue; }
        let delivered = spread(&now);
        let fan = delivered - spread(&was);
        if worst.is_none_or(|(_, seen, _)| fan > seen) {
            worst = Some((share, fan, delivered));
        }
    }
    worst
}

/// R22 (a): the band a tone slider earns on is the band its own response
/// weights most, fixed before any error is read. A 64x64 grey ramp (target
/// luma rising with the row) and a "whites" move that brightens only the six
/// brightest rows: the acting band is the brightest, and a darker-half error
/// that the move happens to leave alone does not move it there; the band
/// readings of the move land in the brightest bands and nowhere in the darker
/// half.
///
/// Both renders carry the same darker-row error against the target, so it is
/// no part of the move's response. The law's own mutation is at the solve:
/// the band read as the one whose reading the removal raises most (the
/// refused argmax over twenty) — the exposure-only self-consistency case
/// then ships unset tone sliders again.
#[test]
fn a_tone_sliders_band_is_the_band_its_own_response_weights_most() {
    let (w, h) = (64usize, 64usize);
    let target: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let v = 0.1 + 0.8 * (i / w) as f32 / (h - 1) as f32;
            [v, v, v]
        })
        .collect();
    let without: Vec<[f32; 3]> = target
        .iter()
        .enumerate()
        .map(|(i, p)| if i / w < 6 { [p[0] + 0.03, p[1], p[2]] } else { *p })
        .collect();
    let with: Vec<[f32; 3]> = without
        .iter()
        .enumerate()
        .map(|(i, p)| if i / w >= h - 6 { [(p[0] + 0.05).min(1.0), (p[1] + 0.05).min(1.0), (p[2] + 0.05).min(1.0)] } else { *p })
        .collect();
    let evidence = evidence_model(&without, &target);
    let bands = look::luma_bands(&target, &evidence);
    let lightness = |px: &[[f32; 3]]| px.iter().map(|p| cie_lab(p)[0]).collect::<Vec<_>>();
    let readings = bands.paired_readings(&with, &target, &evidence);
    assert_eq!(readings.len(), look::LUMA_BANDS, "every band reads: {readings:?}");
    let acting = bands.acting_band(&lightness(&with), &lightness(&without));
    assert_eq!(acting, Some(look::LUMA_BANDS - 1), "the move acts on the brightest band: {acting:?}");
    let move_only = bands.paired_readings(&with, &without, &evidence);
    assert!(
        move_only[..look::LUMA_BANDS / 2].iter().all(|&b| b < 1e-6),
        "the darker half does not read a move in the brightest rows: {move_only:?}"
    );
    assert_eq!(bands.acting_band(&lightness(&without), &lightness(&without)), None, "no move, no band");

    // THE VERDICT on readings that separate the two laws: the frame does not
    // earn (J* equal), the move's own band (the brightest) gains nothing, and
    // the darkest band — which the move does not touch — reads 3 FIT_QUANT
    // better with it. "The band a global control earns on is the band its own
    // curve acts on": the whites do not ship; the darkest band's gain belongs
    // to no tone slider that does not act there. With its own band earning,
    // the whites ship; the contrast (frame only) does not, even then.
    // MUTATION: the band read as the one the removal costs most (the refused
    // argmax over twenty) -> the first verdict ships.
    let mut without_bands = vec![0.01f32; look::LUMA_BANDS];
    without_bands[0] += 3.0 * FIT_QUANT;
    let solved_reading: BandReading = (0.05, vec![0.01; look::LUMA_BANDS], lightness(&with));
    let without_reading: BandReading = (0.05, without_bands.clone(), lightness(&without));
    assert!(
        !earns_on_its_band(Control::Whites, &bands, &solved_reading, &without_reading),
        "a gain on a band the whites do not act on is not the whites' gain"
    );
    without_bands[look::LUMA_BANDS - 1] += 2.0 * FIT_QUANT;
    let without_reading: BandReading = (0.05, without_bands, lightness(&without));
    assert!(
        earns_on_its_band(Control::Whites, &bands, &solved_reading, &without_reading),
        "the whites earn on the band they act on"
    );
    assert!(
        !earns_on_its_band(Control::Contrast, &bands, &solved_reading, &without_reading),
        "the contrast acts on the frame and reads the frame alone"
    );
}

/// R15: `E_dist` is in `E_paired`'s units. Two flat frames differ by one
/// colour, so every pixel's local-mean dE76 is the same d, and every
/// quantile of L*, a* and b* is offset by that channel's share of d: the
/// paired reading and the unpaired distribution reading of the same pair are
/// both d/100.
#[test]
fn the_unpaired_distribution_distance_reads_a_colour_offset_as_the_paired_term_does() {
    let n = 64 * 64;
    let (a, b) = (vec![[0.50f32, 0.40, 0.30]; n], vec![[0.40f32, 0.45, 0.35]; n]);
    let (x, y) = (cie_lab(&a[0]), cie_lab(&b[0]));
    let d = ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt() / 100.0;
    let model = evidence_model(&a, &b);
    let paired = look::look_terms(&a, &b, &model);
    // A model where nothing pairs: the pairing is set, not inferred from a
    // mode (a structure-blind model keeps its pairing since 2026-11-07).
    let mut nothing_pairs = model.clone();
    nothing_pairs.paired.fill(false);
    let unpaired = look::look_terms(&a, &b, &nothing_pairs);
    assert_eq!((paired.share, unpaired.share), (1.0, 0.0), "the first pairs, the second does not");
    assert!((paired.paired - d).abs() < 1e-4, "paired {} vs d {d}", paired.paired);
    assert!((unpaired.dist - d).abs() < 1e-3, "unpaired {} vs d {d}", unpaired.dist);
}
