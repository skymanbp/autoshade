//! Falsifiers for the local-field analyzer.  The synthetic pairs are built with a
//! deterministic LCG (never `rand`) so every figure below is reproducible; the
//! corpus tests skip cleanly when `AUTOSHOP_FIT_CALIBRATION_DIR` is unset, exactly
//! like `fit::calibration_corpus`.

use std::path::{Path, PathBuf};

use super::*;

// --------------------------------------------------------------------------
// synthetic fixtures
// --------------------------------------------------------------------------

/// A 64-bit LCG (Knuth's multiplier) — deterministic across machines and runs.
pub(crate) struct Lcg(pub(crate) u64);

impl Lcg {
    pub(crate) fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) & 0xff_ffff) as f32 / 16_777_216.0
    }
}

/// Analysis rasters come from 8-bit images; the fixtures live on the same grid.
fn byte_round(v: f32) -> f32 {
    (v.clamp(0.0, 1.0) * 255.0).round() / 255.0
}

fn byte_of(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A VERTICAL luma ramp with deterministic texture and a mild chroma tilt.  The
/// ramp runs down the y axis so a left/right split (test 8) is orthogonal to luma;
/// `low`/`high` decide which luma bins the pair populates.
pub(crate) fn ramp(
    width: usize, height: usize, low: f32, high: f32, seed: u64,
) -> Vec<[f32; 3]> {
    let mut lcg = Lcg(seed);
    let mut px = Vec::with_capacity(width * height);
    for y in 0..height {
        let base = low + (high - low) * y as f32 / (height - 1) as f32;
        for _ in 0..width {
            let v = (base + 0.03 * (lcg.unit() - 0.5)).clamp(0.02, 0.96);
            px.push([byte_round(v * 1.02), byte_round(v), byte_round(v * 0.97)]);
        }
    }
    px
}

/// A target built with the field's OWN delta model, so the planted edit is exactly
/// representable and the test measures the solver, not the model.
pub(crate) fn plant(
    current: &[[f32; 3]], width: usize, height: usize, ev: impl Fn(usize, f32) -> f32,
) -> Vec<[f32; 3]> {
    let guide = smooth_3tap_luma(current, width, height);
    current
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let d = delta(c, guide[i], &[ev(i, guide[i]), 0.0, 0.0, 0.0, 0.0]);
            [byte_round(c[0] + d[0]), byte_round(c[1] + d[1]), byte_round(c[2] + d[2])]
        })
        .collect()
}

/// 144x96 keeps every luma bin's vertices above the occupancy floor at analysis-like
/// densities without making `local_support`'s 96 divergence readings expensive.
const FIXTURE: (usize, usize) = (144, 96);

/// The two-band pair used by tests 3, 4, 5, 6 and 7: +0.5 EV under the guide-luma
/// midpoint, -0.5 EV above it, over a ramp that populates all eight luma bins.
pub(crate) fn two_band_pair() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, u32, u32) {
    let (w, h) = FIXTURE;
    let current = ramp(w, h, 0.03, 0.95, 0x5eed_0001);
    let target = plant(&current, w, h, |_, guide| if guide < 0.5 { 0.5 } else { -0.5 });
    (current, target, w as u32, h as u32)
}

fn zero_field(width: u32, height: u32) -> LocalField {
    LocalField {
        grid: vec![[0.0; PARAMS]; VERTICES],
        occupancy: vec![0.0; VERTICES],
        ceiling: 0.0,
        global: 0.0,
        band_marginal: [[0.0; PARAMS]; FIELD_B],
        band_dispersion: [0.0; FIELD_B],
        remainder: Vec::new(),
        weight: Vec::new(),
        saturated: 0,
        solve: SolveInfo { iterations: 0, relative_residual: 0.0 },
        width,
        height,
    }
}

fn bin_mass(occupancy: &[f32], bin: usize) -> f32 {
    (0..FIELD_X * FIELD_Y).map(|spatial| occupancy[spatial * FIELD_B + bin]).sum()
}

// --------------------------------------------------------------------------
// 1-8: unit falsifiers, no corpus
// --------------------------------------------------------------------------

/// The splat must be a partition of unity with every id inside the grid — a dropped
/// `min(low + 1, limit - 1)` runs the last column/row/bin off the end — and an
/// all-zero grid must leave the render untouched to the bit.
#[test]
fn field_splat_is_a_partition_of_unity() {
    let (w, h) = FIXTURE;
    let current = ramp(w, h, 0.03, 0.95, 0x51ed_2701);
    let guide = smooth_3tap_luma(&current, w, h);
    let splat = splat_table(&guide, w, h);
    assert_eq!(splat.ids.len(), w * h, "one splat row per pixel");
    for (ids, tri) in splat.ids.iter().zip(&splat.tri) {
        let sum: f32 = tri.iter().sum();
        assert!((sum - 1.0).abs() <= 1e-5, "splat weights must sum to 1, got {sum}");
        for &id in ids {
            assert!((id as usize) < VERTICES, "vertex id {id} outside the {VERTICES}-vertex grid");
        }
    }
    let rendered = zero_field(w as u32, h as u32).render(&current);
    for (i, (a, b)) in rendered.iter().zip(&current).enumerate() {
        for channel in 0..3 {
            assert_eq!(
                a[channel].to_bits(),
                b[channel].to_bits(),
                "a zero grid must reproduce the current render at pixel {i}",
            );
        }
    }
}

/// `<F x, y> == <x, F^T y>`: the only thing that keeps the CG solving the system it
/// thinks it is solving.  A transposed channel index in `adjoint` breaks it.
#[test]
fn field_adjoint_matches_forward() {
    let (w, h) = (48usize, 32usize);
    let current = ramp(w, h, 0.05, 0.90, 0x9e37_79b9);
    let target = current.clone();
    let guide = smooth_3tap_luma(&current, w, h);
    let weights = vec![1.0f32; w * h];
    let system = System::new(&current, &target, &guide, &weights, w, h);
    let mut lcg = Lcg(0x1234_5678_9abc_def0);
    let x: Vec<f32> = (0..VERTICES * PARAMS).map(|_| lcg.unit() - 0.5).collect();
    let y: Vec<[f32; 3]> = (0..w * h)
        .map(|_| [lcg.unit() - 0.5, lcg.unit() - 0.5, lcg.unit() - 0.5])
        .collect();
    let left: f64 = system
        .forward(&x)
        .iter()
        .zip(&y)
        .map(|(a, b)| (0..3).map(|c| a[c] as f64 * b[c] as f64).sum::<f64>())
        .sum();
    let right = dot(&x, &system.adjoint(&y));
    let scale = left.abs().max(right.abs()).max(1e-12);
    assert!(
        (left - right).abs() / scale <= 1e-4,
        "adjoint is not the transpose of forward: <Fx,y>={left} vs <x,F'y>={right}",
    );
}

/// The recon's proven property: pull the solution hard enough toward zero and the
/// field's render IS the global render.  A `matvec` that ignores the Tikhonov term
/// solves the free problem instead and moves the render by whole levels.
#[test]
fn field_infinite_tikhonov_reproduces_the_global_render() {
    let (current, target, w, h) = two_band_pair();
    let evidence = fit::evidence_model_for(&current, &target, w, h);
    let pinned = LocalField::solve_with(&current, &target, w, h, &evidence, 1e6, SMOOTH, ITERATIONS)
        .expect("a measurable pair must solve");
    let rendered = pinned.render(&current);
    let mut worst = 0.0f32;
    for (a, b) in rendered.iter().zip(&current) {
        for channel in 0..3 {
            worst = worst.max((a[channel] - b[channel]).abs());
            assert_eq!(
                byte_of(a[channel]),
                byte_of(b[channel]),
                "lambda=1e6 must reproduce the global render byte for byte",
            );
        }
    }
    assert!(worst < 0.5 / 255.0, "and it must do so with room to spare, not by rounding: {worst}");
    let free = LocalField::solve(&current, &target, w, h, &evidence).expect("premise solve");
    let moved = free
        .render(&current)
        .iter()
        .zip(&current)
        .flat_map(|(a, b)| (0..3).map(|c| (a[c] - b[c]).abs()).collect::<Vec<_>>())
        .fold(0.0f32, f32::max);
    assert!(
        moved > 8.0 / 255.0,
        "premise: at the production lambda the same pair must move the render far \
         more than the pinned one ({moved}) — otherwise this test proves nothing",
    );
}

/// Target == current: there is nothing to win, so the ceiling must equal the global
/// figure to the bit and no vertex may carry a value.
#[test]
fn field_identity_pair_is_all_zero() {
    let (w, h) = FIXTURE;
    let current = ramp(w, h, 0.03, 0.95, 0x0bad_c0de);
    let evidence = fit::evidence_model_for(&current, &current, w as u32, h as u32);
    let field = LocalField::solve(&current, &current, w as u32, h as u32, &evidence)
        .expect("an identity pair is still measurable");
    assert_eq!(
        field.ceiling.to_bits(),
        field.global.to_bits(),
        "an identity pair's ceiling IS the global figure: {} vs {}",
        field.ceiling,
        field.global,
    );
    for (vertex, cell) in field.grid.iter().enumerate() {
        assert_eq!(*cell, [0.0; PARAMS], "vertex {vertex} moved on an identity pair");
    }
    assert_eq!(field.saturated, 0);
    assert_eq!(field.band_dispersion, [0.0; FIELD_B]);
}

/// Two solves of one input must be bit-identical: no HashMap iteration, no threads,
/// no fast-math.
#[test]
fn field_solve_is_deterministic() {
    let (current, target, w, h) = two_band_pair();
    let evidence = fit::evidence_model_for(&current, &target, w, h);
    let first = LocalField::solve(&current, &target, w, h, &evidence).expect("first solve");
    let second = LocalField::solve(&current, &target, w, h, &evidence).expect("second solve");
    for (vertex, (a, b)) in first.grid.iter().zip(&second.grid).enumerate() {
        for p in 0..PARAMS {
            assert_eq!(a[p].to_bits(), b[p].to_bits(), "vertex {vertex} parameter {p} drifted");
        }
    }
    assert_eq!(first.ceiling.to_bits(), second.ceiling.to_bits());
    assert_eq!(first.solve.iterations, second.solve.iterations);
}

/// Two refusals, both silent failures if they go missing: a pair the objective
/// cannot measure, and a pair with no fit-weight mass left after the evidence,
/// support and clipping factors.
#[test]
fn field_refuses_without_evidence() {
    let (current, target, w, h) = two_band_pair();
    let evidence = fit::evidence_model_for(&current, &target, w, h);
    assert!(
        LocalField::solve(&current, &target, w, h, &evidence).is_some(),
        "premise: with real evidence this pair solves, so the refusals below are the \
         evidence's doing and not the pair's",
    );
    let mut unmeasurable = evidence.clone();
    unmeasurable.identifiability = 1e-6;
    assert!(
        LocalField::solve(&current, &target, w, h, &unmeasurable).is_none(),
        "identifiability <= 1e-5 must refuse, exactly as look_err_with_evidence does",
    );
    let mut massless = evidence.clone();
    massless.source_weights = vec![0.0; (w * h) as usize];
    assert!(
        LocalField::solve(&current, &target, w, h, &massless).is_none(),
        "zero fit-weight mass must refuse rather than solve an empty system",
    );
}

/// A planted two-band exposure the field's own model can express: the solver must
/// recover its sign per luma bin, leave each bin spatially uniform, and close most
/// of the objective.  Also the occupancy floor's falsifier — unmeasured vertices
/// must be zero, not whatever the Laplacian diffused into them.
#[test]
fn field_recovers_a_planted_two_band_exposure() {
    let (current, target, w, h) = two_band_pair();
    let evidence = fit::evidence_model_for(&current, &target, w, h);
    let field = LocalField::solve(&current, &target, w, h, &evidence).expect("solve");
    assert!(
        field.ceiling < 0.5 * field.global,
        "the field must close most of a representable edit: ceiling {} vs global {}",
        field.ceiling,
        field.global,
    );
    for bin in [0usize, 1, 2] {
        assert!(
            field.band_marginal[bin][0] > 0.0,
            "bin {bin} carries the planted +0.5 EV, got {:?}",
            field.band_marginal[bin],
        );
    }
    for bin in [5usize, 6, 7] {
        assert!(
            field.band_marginal[bin][0] < 0.0,
            "bin {bin} carries the planted -0.5 EV, got {:?}",
            field.band_marginal[bin],
        );
    }
    // MEASURED on this fixture (per 255): bins 0-2 read 0.00 / 2.02 / 1.21 and bins
    // 5-7 read 5.43 / 7.35 / 0.38.  The upper bins sit higher because the -EV band
    // thins the target's population there and leaves their vertices barely over the
    // occupancy floor — a property of the metric that survived every size (96x64 ..
    // 192x128) and amplitude (0.25 .. 0.5) swept, so the assertion is set above it
    // rather than tuned to hide it (see the report's finding on BAND_DISPERSION_MAX).
    // The falsifier is the ORDER OF MAGNITUDE against the spatially structured
    // fixture, which reads 20-52 per 255 on the same metric.
    for bin in [0usize, 1, 2, 5, 6, 7] {
        assert!(
            field.band_dispersion[bin] < 10.0 / 255.0,
            "value-based edit read as spatially structured: bin {bin} = {} per 255",
            field.band_dispersion[bin] * 255.0,
        );
    }
    let unmeasured = field.occupancy.iter().filter(|&&o| o < OCCUPANCY_MIN).count();
    assert!(unmeasured > 0, "premise: this pair leaves vertices below the occupancy floor");
    for (vertex, (cell, &occupancy)) in field.grid.iter().zip(&field.occupancy).enumerate() {
        if occupancy < OCCUPANCY_MIN {
            assert_eq!(
                *cell, [0.0; PARAMS],
                "vertex {vertex} carries {occupancy} of mass and must be zeroed",
            );
        }
    }
}

/// The same amplitude arranged spatially instead of by value: every populated bin
/// must read as spatially structured and its band marginal must be near zero — the
/// verdict that stops the band producer from rendering something the frame law would
/// refuse.  NOTE bin 0 is excluded by construction, not by convenience: the luma
/// effect of EV and the gains is identically 0 at `c_b = 0`, so the metric is blind
/// there; the test pins that as a premise instead of hiding it.
#[test]
fn field_band_dispersion_flags_spatially_structured_bins() {
    let (w, h) = FIXTURE;
    // A mid-luma ramp: +/-0.5 EV on either half stays inside the display range at
    // every luma, so no pixel is dropped as clipped and every bin keeps both halves.
    let current = ramp(w, h, 0.30, 0.62, 0x5eed_0002);
    let target = plant(&current, w, h, |i, _| if i % w < w / 2 { 0.5 } else { -0.5 });
    let evidence = fit::evidence_model_for(&current, &target, w as u32, h as u32);
    let field = LocalField::solve(&current, &target, w as u32, h as u32, &evidence).expect("solve");
    assert_eq!(
        field.band_dispersion[0], 0.0,
        "premise: at c_b = 0 the dispersion metric has nothing to measure",
    );
    let total: f32 = (0..FIELD_B).map(|bin| bin_mass(&field.occupancy, bin)).sum();
    let mut tested = 0usize;
    for bin in 1..FIELD_B {
        if bin_mass(&field.occupancy, bin) < 0.015 * total {
            continue;
        }
        tested += 1;
        // The memo's line is 5/255; the spatially uniform fixture's worst bin already
        // reads 7.35/255, so 15/255 is the line that actually separates the two cases.
        // MEASURED here: 21.9 / 35.9 / 51.8 per 255.
        assert!(
            field.band_dispersion[bin] > 15.0 / 255.0,
            "bin {bin} is spatially structured and must read so, got {} per 255",
            field.band_dispersion[bin] * 255.0,
        );
        assert!(
            field.band_marginal[bin][0].abs() < 0.15,
            "bin {bin}'s spatial halves must cancel in the marginal, got {}",
            field.band_marginal[bin][0],
        );
    }
    assert!(tested >= 3, "premise: at least three populated bins carry the split, got {tested}");
}

/// The analyzer is an in-process measuring instrument only.  This grep pin
/// catches either persistence or engine wiring before a schema snapshot can.
#[test]
fn the_local_field_never_reaches_the_engine_or_the_recipe_schema() {
    for source in [
        include_str!("../render.rs"),
        include_str!("../recipe.rs"),
        include_str!("../xmp.rs"),
    ] {
        assert!(!source.contains("fit_field"));
        assert!(!source.contains("freemask"));
    }
}

/// The 96-cell structural support must actually vary on a real pair: synthetic
/// fixtures are too small for `structure_divergence` (their cells fall under its
/// core-pixel floor and read as matched), so this is the one test where the
/// support term is live rather than a constant 1.0.
#[test]
fn calibration_local_support_is_not_constant() {
    let Some(root) = fit::calibration_corpus() else { return };
    let source = image::open(root.join("neutral.jpg")).unwrap();
    let target = image::open(root.join("target.jpg")).unwrap();
    let (s_img, t_img) = fit::analysis_pair(&source, &target);
    let support = local_support(
        &fit::pixels_of(&s_img), &fit::pixels_of(&t_img), s_img.width(), s_img.height(),
    );
    // The replaced sky reads D > 0.7 (support under 0.3); the kept land reads
    // D around 0.3 (support above 0.5). A constant would fail both halves.
    let lowest = support.iter().copied().reduce(f32::min).unwrap();
    let highest = support.iter().copied().reduce(f32::max).unwrap();
    assert!(lowest < 0.3 && highest > 0.5,
        "local support is inert on the calibration pair: {lowest}..{highest}");
}

// --------------------------------------------------------------------------
// 9: the calibration corpus against the NumPy solver
// --------------------------------------------------------------------------

/// The NumPy solver's ceiling on the calibration pair `neutral.jpg -> target.jpg`,
/// measured on the Rust module's OWN exported analysis pixels, guide and fit weight.
///
/// PROVENANCE — measured 2026-08-27 on NumPy 2.3.5 / Python 3.13.3 (Windows 11):
///   `cargo test --release -- --ignored export_calibration_field_inputs_for_numpy`
///   `python scripts/field_check.py`   (tikhonov=1.0, smooth=(1,1,1), iterations=90)
///   `cargo test --release -- --ignored compare_calibration_field_with_numpy`
/// reported `numpy ceiling (production objective) = 0.070022` against the Rust
/// port's 0.0700223, on a global figure of 0.0961453.  The recon's published grid
/// figures are NOT reusable here: they were measured on the RAW-path renders, a
/// different baseline.
const NUMPY_FIELD_CEILING_CALIBRATION: f32 = 0.070_022_5;
/// The largest per-parameter disagreement between the two solvers over all 768
/// vertices: measured 1.50e-5 in the same run, bounded here at a legible 1e-4.
const NUMPY_FIELD_GRID_MAX_DELTA: f32 = 1e-4;

fn probe_dir() -> PathBuf {
    PathBuf::from("target").join("field-probe")
}

/// f32 little-endian, the layout `scripts/field_check.py` reads.
fn write_f32(path: &Path, values: &[f32]) {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("run scripts/field_check.py first: {} ({e})", path.display()));
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// The calibration pair reduced to exactly what the analyzer consumes: the global
/// fit's render, the target analysis raster and the report's own frozen evidence.
struct CalibrationPair {
    current: Vec<[f32; 3]>,
    target: Vec<[f32; 3]>,
    width: u32,
    height: u32,
    evidence: fit::EvidenceModel,
}

fn calibration_pair() -> Option<CalibrationPair> {
    let root = fit::calibration_corpus()?;
    let source = image::open(root.join("neutral.jpg")).expect("calibration neutral.jpg");
    let target = image::open(root.join("target.jpg")).expect("calibration target.jpg");
    let (s_img, t_img) = fit::analysis_pair(&source, &target);
    let report = fit::fit_recipe_from_promoted_with_disclosure(
        &source,
        &target,
        &crate::recipe::EditRecipe::default(),
        false,
        true,
        None,
    );
    // `render::` is reachable from a TEST only; the module body never touches it.
    let current = fit::pixels_of(&crate::render::develop_preview(&s_img, &report.recipe));
    Some(CalibrationPair {
        current,
        target: fit::pixels_of(&t_img),
        width: s_img.width(),
        height: s_img.height(),
        evidence: report.evidence,
    })
}

#[test]
fn calibration_field_ceiling_matches_the_numpy_solver() {
    let Some(pair) = calibration_pair() else { return };
    let field =
        LocalField::solve(&pair.current, &pair.target, pair.width, pair.height, &pair.evidence)
            .expect("the calibration pair is measurable and must produce a field");
    assert!(
        (field.ceiling - NUMPY_FIELD_CEILING_CALIBRATION).abs() <= 0.002,
        "the Rust port must agree with the NumPy solver: ceiling {} vs {}",
        field.ceiling,
        NUMPY_FIELD_CEILING_CALIBRATION,
    );
    assert!(
        field.ceiling <= field.global,
        "a ceiling above the global figure is not a ceiling: {} vs {}",
        field.ceiling,
        field.global,
    );
    assert!(field.solve.iterations <= 90, "the CG budget is 90 steps");
    assert_eq!(field.grid.len(), VERTICES);
    assert_eq!(field.remainder.len(), (pair.width * pair.height) as usize);
}

/// Probe 1 of 2: export exactly what `LocalField::solve` consumed so the NumPy
/// solver can be run on the same numbers.  Not part of the battery.
#[test]
#[ignore]
fn export_calibration_field_inputs_for_numpy() {
    let Some(pair) = calibration_pair() else { return };
    let out = probe_dir();
    std::fs::create_dir_all(&out).expect("create the probe directory");
    let support = local_support(&pair.current, &pair.target, pair.width, pair.height);
    let guide = smooth_3tap_luma(&pair.current, pair.width as usize, pair.height as usize);
    let weights: Vec<f32> = (0..pair.current.len())
        .map(|i| {
            if unclipped(&pair.current[i]) && unclipped(&pair.target[i]) {
                pair.evidence.source_weights[i].max(0.0) * support[i]
            } else {
                0.0
            }
        })
        .collect();
    let flatten = |px: &[[f32; 3]]| px.iter().flatten().copied().collect::<Vec<f32>>();
    write_f32(&out.join("current.bin"), &flatten(&pair.current));
    write_f32(&out.join("target.bin"), &flatten(&pair.target));
    write_f32(&out.join("guide.bin"), &guide);
    write_f32(&out.join("support.bin"), &support);
    write_f32(&out.join("evidence.bin"), &pair.evidence.source_weights);
    write_f32(&out.join("weights.bin"), &weights);
    let field =
        LocalField::solve(&pair.current, &pair.target, pair.width, pair.height, &pair.evidence)
            .expect("solve");
    let grid: Vec<f32> = field.grid.iter().flatten().copied().collect();
    write_f32(&out.join("rust-grid.bin"), &grid);
    let meta = serde_json::json!({
        "width": pair.width,
        "height": pair.height,
        "identifiability": pair.evidence.identifiability,
        "global": field.global,
        "ceiling": field.ceiling,
        "iterations": field.solve.iterations,
        "relative_residual": field.solve.relative_residual,
        "saturated": field.saturated,
        "supported_vertices": field.occupancy.iter().filter(|&&o| o >= OCCUPANCY_MIN).count(),
        "fit_weight_mass": weights.iter().map(|&w| w as f64).sum::<f64>(),
        "band_dispersion": field.band_dispersion,
        "band_marginal": field.band_marginal,
    });
    std::fs::write(out.join("meta.json"), serde_json::to_vec_pretty(&meta).expect("encode meta"))
        .expect("write meta.json");
    println!("exported {} pixels to {}", pair.current.len(), out.display());
}

/// Probe 2 of 2: read the NumPy solver's grid and render back, print both sides'
/// figures and the wall times.  Not part of the battery.
#[test]
#[ignore]
fn compare_calibration_field_with_numpy() {
    let Some(pair) = calibration_pair() else { return };
    let out = probe_dir();
    let support_start = std::time::Instant::now();
    let support = local_support(&pair.current, &pair.target, pair.width, pair.height);
    let support_seconds = support_start.elapsed().as_secs_f64();
    assert_eq!(support.len(), pair.current.len());
    let solve_start = std::time::Instant::now();
    let field =
        LocalField::solve(&pair.current, &pair.target, pair.width, pair.height, &pair.evidence)
            .expect("solve");
    let solve_seconds = solve_start.elapsed().as_secs_f64();

    let numpy_grid = read_f32(&out.join("numpy-grid.bin"));
    assert_eq!(numpy_grid.len(), VERTICES * PARAMS);
    let rust_grid: Vec<f32> = field.grid.iter().flatten().copied().collect();
    let max_delta = rust_grid
        .iter()
        .zip(&numpy_grid)
        .fold(0.0f32, |worst, (a, b)| worst.max((a - b).abs()));
    let numpy_render: Vec<[f32; 3]> = read_f32(&out.join("numpy-render.bin"))
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    assert_eq!(numpy_render.len(), pair.current.len());
    let numpy_ceiling = fit::look_err_with_evidence(&numpy_render, &pair.target, &pair.evidence);
    let numpy_meta = String::from_utf8(
        std::fs::read(out.join("numpy-solve.json")).expect("numpy-solve.json"),
    )
    .expect("numpy-solve.json is utf-8");

    println!("max |grid_rust - grid_numpy| = {max_delta:.6}");
    println!(
        "rust  iterations={} relative_residual={:.3e} ceiling={:.6} global={:.6}",
        field.solve.iterations, field.solve.relative_residual, field.ceiling, field.global,
    );
    println!("numpy solve = {numpy_meta}");
    println!("numpy ceiling (production objective) = {numpy_ceiling:.6}");
    println!(
        "saturated={} supported={}",
        field.saturated,
        field.occupancy.iter().filter(|&&o| o >= OCCUPANCY_MIN).count(),
    );
    println!("band_dispersion = {:?}", field.band_dispersion);
    println!("wall: LocalField::solve {solve_seconds:.3}s, local_support {support_seconds:.3}s");
    assert!(
        max_delta <= NUMPY_FIELD_GRID_MAX_DELTA,
        "the two solvers must agree vertex by vertex: {max_delta} > {NUMPY_FIELD_GRID_MAX_DELTA}",
    );
    assert!(
        (numpy_ceiling - NUMPY_FIELD_CEILING_CALIBRATION).abs() <= 0.002,
        "NUMPY_FIELD_CEILING_CALIBRATION is stale: measured {numpy_ceiling}",
    );
}
