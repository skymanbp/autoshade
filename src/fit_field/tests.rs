//! Falsifiers for the local-field analyzer.  The synthetic pairs are built with a
//! deterministic LCG (never `rand`) so every figure below is reproducible; the
//! corpus tests skip cleanly when `AUTOSHADE_FIT_CALIBRATION_DIR` is unset, exactly
//! like `fit::calibration_corpus`.

use std::path::{Path, PathBuf};

use super::*;

/// R34 §D4. The analysis instrument's solve options ARE the defaults, so every
/// pre-R34 caller is unchanged byte for byte and the ceiling the sequencer
/// stops on is the number it has always been. Only the SHIPPED field
/// (`fit_zoned::field::attach_colour_field`) ever passes anything else.
#[test]
fn the_analysis_solve_options_are_the_analyzers_own_constants() {
    let opts = FieldSolveOpts::default();
    assert_eq!(opts.gain, BOUNDS_HIGH[1], "the gain bound is the analyzer's own");
    assert_eq!(opts.gain, default_gain_bound(), "…named once, for the budget's default point");
    assert!(opts.local_support, "…and the structural support term is ON");
    for p in 0..PARAMS {
        assert_eq!(
            opts.bounds(p),
            (BOUNDS_LOW[p], BOUNDS_HIGH[p]),
            "parameter {p} keeps its constants under the default options"
        );
    }
    // Opening the GAIN bound moves the three gain axes and nothing else: EV
    // and slope keep their constants, because the demand R34 measured is
    // chromatic and a bound widened for no reason stops meaning anything.
    let wide = FieldSolveOpts { gain: 0.80, local_support: false };
    assert_eq!(wide.bounds(0), (BOUNDS_LOW[0], BOUNDS_HIGH[0]), "EV is untouched");
    assert_eq!(wide.bounds(4), (BOUNDS_LOW[4], BOUNDS_HIGH[4]), "slope is untouched");
    for p in 1..=3 {
        assert_eq!(wide.bounds(p), (-0.80, 0.80), "gain axis {p} follows the budget");
    }
}

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
            let d = delta(c, guide[i], &[ev(i, guide[i]) as f64, 0.0, 0.0, 0.0, 0.0]);
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
        zones: None,
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

/// Every vertex belongs to exactly one evidence cell, through the cell's own
/// column and the `FIELD_Y_REFINE` vertex rows that refine its row — the
/// ownership the cell admission copies verdicts along. A span that skipped a
/// bin, a row that two cells claimed, or a vertex no cell owned would each
/// break the count.
#[test]
fn cell_vertex_spans_partition_the_grid_by_cell() {
    let mut owner = vec![None::<usize>; VERTICES];
    for cell in 0..FIELD_X * CELL_ROWS {
        for span in cell_vertex_spans(cell) {
            assert_eq!(span.len(), FIELD_B, "one span per vertex row, all luma bins");
            for vertex in span {
                let (ix, iy) = ((vertex / FIELD_B) % FIELD_X, vertex / FIELD_B / FIELD_X);
                assert_eq!(ix, cell % FIELD_X, "a cell owns only its own column");
                assert_eq!(iy / FIELD_Y_REFINE, cell / FIELD_X, "…and only the rows that refine its row");
                assert!(owner[vertex].replace(cell).is_none(), "vertex {vertex} claimed twice");
            }
        }
    }
    assert!(owner.iter().all(Option::is_some), "every vertex has a cell");
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

/// v1.17.1. A channel at the clip in BOTH frames is a measurement: the pixel
/// testifies, and the field keeps it where the target keeps it. Left half:
/// pixels scattered, one in four a warm one the target takes 10 % less red
/// (0.80 → 0.72), the rest a bright one whose red sits at the clip in both
/// frames (luma 0.64 and 0.71, so they share the field's brightness-bin
/// vertices and no field can serve both exactly).
/// Right half: the bright pixel clipped in the SOURCE only (the target's red
/// 0.90) — its true source value is unknown, and it still does not testify.
/// Until v1.17.1 the clipped-in-both pixels were left out of the solve and
/// took their vertices' red cut anyway, as p36's clouds did (0.999 → 0.972
/// against a target at 0.998). Measured 2026-11-02: the clipped-in-both red
/// renders at 0.988 under the rule (the warm pixels' at 0.765), at 0.935
/// with those pixels left out (0.750).
///
/// MUTATION THIS KILLS: weigh only pixels inside the clip in both frames, or
/// let a one-sided clip testify.
#[test]
fn a_channel_clipped_in_both_frames_testifies_and_keeps_its_colour() {
    let (w, h) = FIXTURE;
    let mut lcg = Lcg(0x5eed_0c11);
    let (warm, bright) = ([0.80f32, 0.60, 0.45], [1.0f32, 0.62, 0.45]);
    let (mut current, mut target, mut kind) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..w * h {
        let left = i % w < w / 2;
        if lcg.unit() < 0.25 {
            current.push(warm.map(byte_round));
            target.push([0.72f32, 0.60, 0.45].map(byte_round));
            kind.push(0u8);
        } else {
            current.push(bright.map(byte_round));
            target.push(if left { bright } else { [0.90f32, 0.62, 0.45] }.map(byte_round));
            kind.push(if left { 1 } else { 2 });
        }
    }
    let evidence = fit::evidence_model_for(&current, &target, w as u32, h as u32);
    let field = LocalField::solve(&current, &target, w as u32, h as u32, &evidence).expect("solve");
    let kind = &kind;
    let of = |k: u8| (0..w * h).filter(move |&i| kind[i] == k);
    let render = field.render(&current);
    let red = |k: u8| of(k).map(|i| render[i][0] as f64).sum::<f64>() / of(k).count() as f64;
    let (kept, cut) = (red(1), red(0));
    assert!(of(1).all(|i| testifies(&current[i], &target[i])), "clipped in both frames testifies");
    assert!(of(2).all(|i| !testifies(&current[i], &target[i]) && field.weight[i] == 0.0), "a one-sided clip does not");
    assert!(of(1).any(|i| field.weight[i] > 0.0), "premise: the clipped-in-both pixels carry evidence weight");
    assert!(cut < 0.78, "premise: the warm pixels' red was cut toward 0.72: {cut:.4}");
    assert!(kept >= 0.98, "the clipped-in-both red is kept where the target keeps it: {kept:.4}");
}

/// v1.18.0. A channel at the BLACK clip is a dark pixel, not a missing
/// measurement ([`testifies`]). A night sky at the analysis raster's size:
/// on its top 38 rows (of 256) the frame being moved carries red at 11 codes,
/// green 20, blue 40, and its target is darker there with its red crushed to
/// 0 (green ×0.7, blue ×0.8 — the shape a dark target sky's top rows were
/// read with: red 0 against the render's 11-13 codes, green and blue
/// unclipped); below them a ramp to a bright ground the target leaves as it
/// is. Every top-row pixel's luma is inside its clip codes on both sides, so
/// under the rule they testify and the field darkens those rows toward the
/// target. The bar is one 8-bit code of mean luma: the quantum, the least
/// move an 8-bit render can show.
///
/// MUTATION THIS KILLS: drop a pixel with any channel at or under 1/255 (the
/// rule before v1.18.0) — the top vertex rows lose every witness (occupancy 0
/// printed) and ship the smoother's continuation of the unchanged rows below:
/// the rows do not darken.
#[test]
fn a_channel_at_the_black_clip_testifies_and_the_dark_rows_darken() {
    let (w, h) = (384usize, 256usize);
    let top = 38usize;
    let mut lcg = Lcg(0x5eed_b1ac);
    let (mut current, mut target) = (Vec::with_capacity(w * h), Vec::with_capacity(w * h));
    for i in 0..w * h {
        let y = i / w;
        let t = 1.0 + 0.2 * (lcg.unit() - 0.5);
        if y < top {
            let c = [11.0 / 255.0 * t, 20.0 / 255.0 * t, 40.0 / 255.0 * t].map(byte_round);
            current.push(c);
            target.push([0.0, byte_round(c[1] * 0.7), byte_round(c[2] * 0.8)]);
        } else {
            let v = (0.08 + 0.85 * (y - top) as f32 / (h - top - 1) as f32) * t;
            let c = [byte_round(v * 0.9), byte_round(v), byte_round(v * 1.1)];
            current.push(c);
            target.push(c);
        }
    }
    let evidence = fit::evidence_model_for(&current, &target, w as u32, h as u32);
    let field = LocalField::solve(&current, &target, w as u32, h as u32, &evidence).expect("solve");
    // Per vertex row: the occupancy over the row, and the EV of the two dark
    // bins (the bins the top rows' guide luma reads) unweighted over x.
    let rows_of_top = (top * FIELD_Y).div_ceil(h) + 1;
    for j in 0..rows_of_top.min(FIELD_Y) {
        let occupancy: f32 = (0..FIELD_X * FIELD_B).map(|k| field.occupancy[j * FIELD_X * FIELD_B + k]).sum();
        let mut ev = 0.0f32;
        for x in 0..FIELD_X {
            for b in 0..2 { ev += field.grid[(j * FIELD_X + x) * FIELD_B + b][0]; }
        }
        let ev = ev / (2 * FIELD_X) as f32;
        println!("vertex row {j:2}: occupancy {occupancy:9.3}  EV bins 0-1 {ev:+.5}");
    }
    let render = field.render(&current);
    let mean_luma = |px: &[[f32; 3]]| {
        px[..top * w].iter().map(|p| fit::luma601(p) as f64).sum::<f64>() / (top * w) as f64
    };
    let (before, after, asked) = (mean_luma(&current), mean_luma(&render), mean_luma(&target));
    println!("top rows mean luma, codes: current {:.3}  field {:.3}  target {:.3}", before * 255.0, after * 255.0, asked * 255.0);
    assert!(
        (before - after) * 255.0 > 1.0,
        "the dark top rows darken toward their target by more than one code: {:.3} -> {:.3} (target {:.3})",
        before * 255.0, after * 255.0, asked * 255.0,
    );
    assert!(
        (0..top * w).all(|i| target[i][0] <= 1.0 / 255.0 && testifies(&current[i], &target[i])),
        "premise: every top-row pixel has its target red at the black clip and testifies",
    );
}

/// The property the analyzer's ceiling and the user's export rest on: the
/// field the solver measured its ceiling on (`apply_flat` over its own splat
/// table) and the field the engine renders from the recipe are the same
/// bytes, for a FULL solved field. Both read the eight corners with the same
/// weights, accumulate in f64 in the same order and apply the delta in f64;
/// until v1.17.1 `slice_flat` rounded the parameters to f32 in between and
/// the two renders of one grid differed by 1 ulp (pixel 42 of a two-class
/// frame, 1052788456 against 1052788457).
#[test]
fn the_engine_renders_the_analyzers_field_bit_for_bit() {
    for (current, target, w, h) in [two_band_pair(), {
        let (w, h) = FIXTURE;
        let current = ramp(w, h, 0.05, 0.9, 0x5eed_00e1);
        let target = current.iter().enumerate()
            .map(|(i, c)| if i % w < w / 2 { [byte_round(c[0] * 1.08), c[1], byte_round(c[2] * 0.9)] } else { *c })
            .collect();
        (current, target, w as u32, h as u32)
    }] {
        let evidence = fit::evidence_model_for(&current, &target, w, h);
        let field = LocalField::solve(&current, &target, w, h, &evidence).expect("solve");
        let (w, h) = (w as usize, h as usize);
        let guide = smooth_3tap_luma(&current, w, h);
        let splat = splat_table(&guide, w, h);
        let flat: Vec<f32> = field.grid.iter().flatten().copied().collect();
        assert!(flat.iter().any(|&v| v != 0.0), "premise: the solved field is not zero");
        let analyzer = apply_flat(&current, &guide, &splat, &flat);
        let engine = field.render(&current);
        let moved = analyzer.iter().zip(&current).filter(|(a, c)| a != c).count();
        assert!(moved > w * h / 4, "premise: the field moves the frame ({moved} pixels)");
        for (i, (a, b)) in analyzer.iter().zip(&engine).enumerate() {
            for channel in 0..3 {
                assert_eq!(a[channel].to_bits(), b[channel].to_bits(), "pixel {i} channel {channel}");
            }
        }
    }
}

/// The recon's proven property: pull the solution hard enough toward zero and the
/// field's render IS the global render.  A `matvec` that ignores the Tikhonov term
/// solves the free problem instead and moves the render by whole levels.
#[test]
fn field_infinite_tikhonov_reproduces_the_global_render() {
    let (current, target, w, h) = two_band_pair();
    let evidence = fit::evidence_model_for(&current, &target, w, h);
    let pinned = LocalField::solve_with(
        &current, &target, w, h, &evidence, 1e6, SMOOTH, ITERATIONS, FieldSolveOpts::default(),
    )
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

/// A dark, thinly lit sky over a bright, well-lit ground, each asking for its
/// own colour change: the data term's curvature is ∝ occupancy·c², so the
/// sky's red gain carries a diagonal two orders under the ground's EV — the
/// spread that made the seaside street's three solves stop on the 90-step
/// budget, not on the tolerance. Jacobi-preconditioned by the system's own
/// diagonal the solve reaches the tolerance inside the budget.
///
/// MUTATION: make `System::preconditioner` return all ones (plain CG) — the
/// solve runs the whole budget and stops short of the tolerance.
#[test]
fn a_dark_sky_over_a_bright_ground_solves_to_its_tolerance_inside_the_budget() {
    // The analysis raster's own size: the data curvature grows with a vertex's
    // pixel count, and the spread it makes is what the budget ran out on.
    let (w, h) = (384usize, 256usize);
    let mut lcg = Lcg(0x0d4c_5c1e);
    let current: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let y = i / w;
            let t = 0.03 * (lcg.unit() - 0.5);
            if y < h * 3 / 5 {
                let v = 0.03 + 0.10 * y as f32 / (h * 3 / 5) as f32 + 0.3 * t;
                [byte_round(v * 0.45), byte_round(v * 0.8), byte_round(v * 1.25)]
            } else {
                let v = 0.60 + 0.38 * (y - h * 3 / 5) as f32 / (h * 2 / 5) as f32 + t;
                [byte_round(v * 1.05), byte_round(v * 0.95), byte_round(v * 0.8)]
            }
        })
        .collect();
    let guide = smooth_3tap_luma(&current, w, h);
    let target: Vec<[f32; 3]> = current
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let p = if i / w < h * 3 / 5 { [0.0, 0.30, 0.05, -0.05, 0.0] } else { [-0.30, 0.0, 0.0, 0.0, 0.1] };
            let d = delta(c, guide[i], &p);
            [byte_round(c[0] + d[0]), byte_round(c[1] + d[1]), byte_round(c[2] + d[2])]
        })
        .collect();
    let evidence = fit::evidence_model_for(&current, &target, w as u32, h as u32);
    let field = LocalField::solve(&current, &target, w as u32, h as u32, &evidence).expect("solve");
    assert!(
        field.solve.iterations < ITERATIONS && field.solve.relative_residual <= 1e-5,
        "the solve stops on its tolerance, not on its budget: {} steps, relative residual {:.2e}",
        field.solve.iterations,
        field.solve.relative_residual,
    );
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
        "identifiability <= 1e-5 must refuse: the field solve is an estimation (DEF-4)",
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
/// of the part of the objective a colour field moves. DEF-5 (presence is
/// membership): every pixel is in the structural reading's core, and the planted
/// +/-0.5 EV step at the guide's 0.5 line reorders the luminance ranks there, so
/// `S` reads its cap 1.0 on the source AND on the field's render — structure no
/// tone field moves. J* = local + 0.10 S then reads 0.1895 -> 0.1207 (ratio 0.64)
/// while the local part the field answers for reads 0.0895 -> 0.0207 (0.23);
/// before DEF-5 the core held only the well-supported pixels and S stayed under
/// the cap. The structural term left the objective on 2026-11-07; the test asserts the local part closed by half.  Also the occupancy floor's falsifier, rewritten in
/// v1.18.0 to the law that replaced the policy zero: an unmeasured vertex
/// ships the solve's own value (the smoother's continuation of its measured
/// neighbours), because a zero written there is rendered as a measured "no
/// change" and draws a plateau edge on the cell boundary.
#[test]
fn field_recovers_a_planted_two_band_exposure() {
    let (current, target, w, h) = two_band_pair();
    let evidence = fit::evidence_model_for(&current, &target, w, h);
    let field = LocalField::solve(&current, &target, w, h, &evidence).expect("solve");
    let (with, without) = (
        fit::look_terms(&field.render(&current), &target, &evidence),
        fit::look_terms(&current, &target, &evidence),
    );
    assert!(
        (with.total() - field.ceiling).abs() < 1e-6 && (without.total() - field.global).abs() < 1e-6,
        "the terms read are the solve's own ceiling and global ({with:?} / {without:?})"
    );
    assert!(
        with.total() < 0.5 * without.total(),
        "the field must close most of a representable edit: local {} vs global {} (J* {} vs {})",
        with.total(),
        without.total(),
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
    let measured = supported(&field.occupancy);
    assert!(measured.iter().any(|m| !m), "premise: this pair leaves cells below the occupancy floor");
    // An unmeasured vertex beside a measured one (along x, y or luma) carries
    // the solve's continuation of that neighbour's change, never the policy
    // zero (MUTATION: write 0.0 into unmeasured vertices again).
    let mut continued = 0usize;
    for (vertex, cell) in field.grid.iter().enumerate() {
        if measured[vertex] {
            continue;
        }
        let (spatial, bin) = (vertex / FIELD_B, vertex % FIELD_B);
        let (iy, ix) = (spatial / FIELD_X, spatial % FIELD_X);
        let neighbours = [
            (ix > 0).then(|| vertex - FIELD_B),
            (ix + 1 < FIELD_X).then(|| vertex + FIELD_B),
            (iy > 0).then(|| vertex - FIELD_X * FIELD_B),
            (iy + 1 < FIELD_Y).then(|| vertex + FIELD_X * FIELD_B),
            (bin > 0).then(|| vertex - 1),
            (bin + 1 < FIELD_B).then(|| vertex + 1),
        ];
        let Some(next) = neighbours.into_iter().flatten().find(|n| measured[*n] && field.grid[*n] != [0.0; PARAMS])
        else {
            continue;
        };
        continued += 1;
        assert!(
            *cell != [0.0; PARAMS],
            "unmeasured vertex {vertex} beside measured vertex {next} ({:?}) must carry the solve's value, not zero",
            field.grid[next],
        );
    }
    assert!(continued > 0, "premise: an unmeasured vertex borders a measured one");
}

/// v1.17.0: a demand the gain bound cannot meet keeps its HUE and gives up its
/// size. The target moves every pixel's hue and not its luma — the per-channel
/// multiplier change (+0.80, −0.466, +0.30), luma601-neutral, whose spread
/// (half 0.63) is nearly twice the analysis bound's 0.35. The same system is
/// solved twice: with the bound, and with the gains unbounded. Limited along
/// its direction ([`limit_colour_change`]), every data-carrying vertex's
/// change points where the unbounded solve's points (cos ≥ 0.999) and every
/// pixel moves along the line from where it is toward its target. Clamped
/// channel by channel — the box the solve used to hold it to — green and red
/// stop at their bounds while blue keeps its own, and the change turns.
///
/// The reference is the unbounded solve, not the demand itself, because the
/// regulariser alone turns a weakly measured vertex toward grey (its Tikhonov
/// term prices a common change through EV below a chromatic one through the
/// gains — measured here cos 0.985 to the demand at a vertex carrying 8.4 of
/// mass, bound or no bound); that is not the limit's doing.
///
/// MUTATION: clamp each gain to its bound on its own in
/// `limit_colour_change`, or re-solve the colour of the free vertices around
/// the limited ones.
#[test]
fn a_pure_hue_demand_keeps_its_hue_under_the_gain_limit() {
    let (w, h) = FIXTURE;
    let current = ramp(w, h, 0.25, 0.45, 0x5eed_0007);
    let want = [0.80f32, -(0.299 * 0.80 + 0.114 * 0.30) / 0.587, 0.30];
    assert!((0.299 * want[0] + 0.587 * want[1] + 0.114 * want[2]).abs() < 1e-6, "premise: a pure hue change");
    let target: Vec<[f32; 3]> =
        current.iter().map(|c| std::array::from_fn(|ch| byte_round(c[ch] * (1.0 + want[ch])))).collect();
    let evidence = fit::evidence_model_for(&current, &target, w as u32, h as u32);
    let solve = |gain: f32| {
        LocalField::solve_with(
            &current, &target, w as u32, h as u32, &evidence, TIKHONOV, SMOOTH, ITERATIONS,
            FieldSolveOpts { gain, ..FieldSolveOpts::default() },
        )
        .expect("solve")
    };
    let (field, free) = (solve(default_gain_bound()), solve(10.0));
    assert!(field.saturated > 0, "premise: the demand is past the bound and the limit binds");
    assert_eq!(free.saturated, 0, "premise: unbounded, nothing is limited");
    let cos = |a: [f32; 3], b: [f32; 3]| {
        let dot: f32 = (0..3).map(|c| a[c] * b[c]).sum();
        let norm = |v: [f32; 3]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm(a) * norm(b)).max(1e-12)
    };
    let measured = supported(&field.occupancy);
    let mut worst = 1.0f32;
    for (vertex, (cell, open)) in field.grid.iter().zip(&free.grid).enumerate() {
        // A vertex carries the demand where the pixels are: one with less than
        // a cell's floor of mass of its own is the smoother's.
        if !measured[vertex] || field.occupancy[vertex] < OCCUPANCY_MIN {
            continue;
        }
        let (m, unbounded) = (multiplier(cell), multiplier(open));
        worst = worst.min(cos(m, unbounded));
        assert!(
            cos(m, unbounded) >= 0.999,
            "vertex {vertex}'s change {m:?} turned from the unbounded solve's {unbounded:?} (cos {:.4})",
            cos(m, unbounded)
        );
        assert!(cell[1..4].iter().all(|g| g.abs() <= default_gain_bound() + 1e-6), "inside the bound");
    }
    assert!(worst < 1.0, "premise: data-carrying vertices were read");
    let render = field.render(&current);
    let along: f32 = render
        .iter()
        .zip(&current)
        .zip(&target)
        .map(|((r, c), t)| cos(std::array::from_fn(|ch| r[ch] - c[ch]), std::array::from_fn(|ch| t[ch] - c[ch])))
        .sum::<f32>()
        / current.len() as f32;
    assert!(along >= 0.99, "pixels moved off the line toward their targets: mean cos {along:.4}");
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

/// R33 §G REPLACED the rule this pin used to state.
///
/// It read `the_local_field_never_reaches_the_engine_or_the_recipe_schema`,
/// and it was right for as long as the field was an instrument: a grep over
/// render.rs / recipe.rs / xmp.rs for the analyzer's own module name, catching
/// persistence or engine wiring before a schema snapshot could. The field now
/// SHIPS, as `EditRecipe::colour_field`, so that sentence is no longer the
/// claim anyone wants to make.
///
/// What survives is the half that was never about shipping: the engine renders
/// a `ColourField` off the RECIPE, and must not reach into the analyzer to do
/// it. One direction of dependency — `fit_field` calls `render`, never the
/// reverse — is what keeps the solver out of the render path and lets the
/// render be a pure function of the recipe in front of it. `freemask` is still
/// fully analysis-side and keeps its original, stronger claim.
#[test]
fn the_engine_renders_the_field_from_the_recipe_and_never_calls_the_analyzer() {
    let engine = crate::render::engine_source();
    let sidecar = crate::xmp::source_text();
    for source in [
        engine.as_str(),
        crate::render::TESTS_SOURCE,
        include_str!("../recipe.rs"),
        sidecar.as_str(),
        crate::xmp::TESTS_SOURCE,
    ] {
        // CODE only. The engine now DOCUMENTS the analyzer it shares its guide
        // and its render with, and an intra-doc link is the opposite of a
        // hidden dependency — it is the reader being told where the other
        // half lives. What must not appear is a path expression.
        let code: String = source
            .lines()
            .map(str::trim_start)
            .filter(|line| !line.starts_with("//"))
            .collect::<Vec<_>>()
            .join("
");
        assert!(!code.contains("fit_field"), "the engine must not depend on the solver");
        assert!(!code.contains("freemask"));
    }
    // …and the shipping half of the same fact, stated positively: the field is
    // a recipe control with a registry row, an export-loss tier and no `crs:`
    // key of its own.
    let row = crate::advisor::catalogue::global_control("colour_field")
        .expect("the field is a registry control");
    assert!(row.engine_only, "no response schema can state ninety-six vertices");
    assert_eq!(row.tier, Some(crate::advisor::catalogue::Tier::RenderedNotExported));
    assert!(matches!(row.crs, crate::advisor::catalogue::CrsKey::None));
}

/// A pair whose left third is REPLACED by a different texture and whose right
/// two thirds keep their structure under a tone move. `width` and `height` set
/// the cell geometry `local_support` will cut, which is the variable the tests
/// below are actually about.
fn structural_pair(width: u32, height: u32) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let source = (0..width * height)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let v = (40 + ((x * 37 + y * 53) % 180)) as f32 / 255.0;
            [v, v, v]
        })
        .collect::<Vec<_>>();
    let target = source
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (x, y) = (i as u32 % width, i as u32 / width);
            if x < width / 3 {
                let v = (30 + ((x * 91 + y * 17) % 200)) as f32 / 255.0;
                [v, v, v]
            } else {
                [p[0] * 0.8, p[1] * 0.8, p[2] * 0.8]
            }
        })
        .collect::<Vec<_>>();
    (source, target)
}

/// A33 (v1.2.4). The structural reading ABSTAINS below its resolvable core,
/// and an abstention is not a claim that the structure survived.
///
/// [`fit::structure_divergence`] used to manufacture a `matched()` verdict —
/// D = 0, correlation 1, energy error 0 — for any mask whose three-pixel
/// erosion left fewer than [`fit::STRUCTURE_MIN_CORE_PX`] pixels. Every
/// consumer reads D = 0 as "this region is structurally identical", so a
/// region too small to measure was granted the strongest possible verdict in
/// its own favour, for free.
///
/// The floor is exact and this fixture sits on it deliberately: a 16-px cell
/// erodes to a 10 x 10 core, which is 100 and resolves, and a 15-px one to
/// 9 x 10 = 90, which does not.
///
/// MUTATION (2026-09-02): restore the old answer —
/// `return Some(Divergence::matched());` under the core floor in
/// `structure_divergence` — and the None assertion fails, along with the two
/// abstaining columns below.
#[test]
fn the_structural_reading_abstains_below_its_resolvable_core() {
    let (width, height) = (64u32, 64u32);
    let (source, target) = structural_pair(width, height);
    let square = |side: u32| {
        (0..width * height)
            .map(|i| {
                let (x, y) = (i % width, i / width);
                f32::from(x < side && y < side)
            })
            .collect::<Vec<f32>>()
    };
    // 12 x 12 erodes to 6 x 6 = 36 core pixels; 20 x 20 to 14 x 14 = 196.
    assert!(
        fit::structure_divergence(&source, &target, width, height, &square(12)).is_none(),
        "a mask under the core floor must abstain, not report a match",
    );
    let resolved = fit::structure_divergence(&source, &target, width, height, &square(20))
        .expect("a mask over the core floor resolves");
    assert!(
        resolved.d > 0.0,
        "the replaced corner is not a match: {resolved:?}",
    );
    // Geometry disagreement is the other abstention, and it is the one that
    // used to hand a caller a matched verdict for a mask of the wrong size.
    assert!(
        fit::structure_divergence(&source, &target, width, height, &square(20)[..10]).is_none(),
        "a weights vector of the wrong length must abstain",
    );
}

/// A33's consumer half. `local_support` modulates the field solve cell by
/// cell, and an unread cell must contribute NO claim (0.0) rather than the
/// full support of a measured match (1.0) — except where NO cell in the frame
/// resolves, which is a frame the instrument cannot read at all and where
/// withholding every cell would starve a solve that used to run.
///
/// The 190 x 128 frame cuts ten 16-px columns and two 15-px ones, so the same
/// frame carries both answers: cells 5 (x 80..94) and 11 (x 175..189) abstain
/// while their neighbours resolve. The 96 x 64 frame cuts 8 x 8 cells, whose
/// 2 x 2 core is far under the floor, so nothing in it resolves.
///
/// MUTATION (2026-09-02): `readings[cell].unwrap_or(1.0)` — the abstaining
/// columns read as fully supported and the first assertion fails.
#[test]
fn an_unreadable_cell_makes_no_support_claim_but_an_unreadable_frame_keeps_one() {
    let (width, height) = (190u32, 128u32);
    let (source, target) = structural_pair(width, height);
    let support = local_support(&source, &target, width, height);
    let at = |x: u32| support[(60 * width + x) as usize];
    assert_eq!(
        (at(85), at(180)),
        (0.0, 0.0),
        "a cell whose core is 90 px makes no claim either way",
    );
    assert!(
        at(70) > 0.5,
        "the kept two thirds resolve and read as supported: {}",
        at(70),
    );
    assert!(
        at(20) < at(70),
        "the replaced third reads lower than the kept one: {} against {}",
        at(20),
        at(70),
    );

    // The wholesale exception, and the reason it exists: every cell of this
    // frame is unreadable, so the modulation is dropped rather than applied
    // as a frame-wide zero.
    let (width, height) = (96u32, 64u32);
    let (source, target) = structural_pair(width, height);
    let support = local_support(&source, &target, width, height);
    assert!(
        support.iter().all(|v| *v == 1.0),
        "a frame nothing can read must keep the pre-v1.2.4 constant",
    );
}

/// The 96-cell structural support on a REAL pair, where the cells carry a
/// photograph's own structure rather than a planted texture and the reading
/// has to separate a replaced sky from kept land. The synthetic frame in
/// `an_unreadable_cell_makes_no_support_claim_but_an_unreadable_frame_keeps_one`
/// proves the abstention rule; this proves the reading is worth having.
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
/// PROVENANCE — RE-MEASURED 2026-11-04 for v1.18.0's objective (J*), by the
/// three steps below on the R15 tree: NumPy ceiling 0.087511 against the
/// Rust port's 0.087511, `max |grid_rust - grid_numpy| = 1.0e-5`, both
/// solvers 33 CG iterations, on a global figure of 0.138096. The two
/// solvers agree as before; what moved is the ruler the ceiling is read on
/// (`look_err_with_evidence` is J* now, per-pixel CIE76 on local means where
/// pixels pair) and the inputs it is read from (the global render, solved on
/// J*, and the cells' structural support under the tie rule, 780f90d).
/// 0.073202 -> 0.087511.
///
/// RE-MEASURED 2026-11-03 for v1.18.0's Jacobi-preconditioned
/// conjugate gradients (`System::preconditioner`; `scripts/field_check.py`
/// ported with it), by the three steps below: NumPy ceiling 0.073203 against
/// the Rust port's 0.073203, `max |grid_rust - grid_numpy| = 1.5e-6`, both
/// solvers 34 CG iterations where the unpreconditioned solve took 52 (the
/// same tolerance reached sooner: the diagonal scales away the spread
/// between well- and thinly-evidenced vertices). The constant stands.
///
/// RE-MEASURED 2026-11-03 for v1.17.1's f64 parameter read
/// (`slice_flat` and `delta` keep the per-pixel parameters in the f64 they
/// are accumulated in, as the engine does), by the three steps below: NumPy
/// ceiling 0.073202, unchanged, against the Rust port's 0.073203 (52 CG
/// iterations where it took 51: the solve's forward operator lost one
/// rounding per pixel), `max |grid_rust - grid_numpy| = 6.6e-6` against
/// 1.0e-5 before. The constant stands. Below, the earlier provenance as it
/// stood.
///
/// RE-MEASURED 2026-10-29 for v1.17.0's direction-preserving
/// limit (`System::solve`: each vertex's colour change scaled along its own
/// direction and held, the slopes' active set alone; `scripts/field_check.py`
/// ported with it), by the three steps below: NumPy ceiling 0.073202 against
/// the Rust port's 0.073202, on the same global 0.099869, `max |grid_rust -
/// grid_numpy| = 1.0e-5`, 51 CG iterations, 748 supported vertices. The box
/// solve read 0.072526: holding each channel's own best value up to its bound
/// fits this pair 0.0007 closer, by turning the hue of the change where the
/// bound binds. Below, the earlier provenance as it stood.
///
/// RE-MEASURED 2026-10-21 on the refined grid (12×32×8, 3072
/// vertices; `FIELD_Y_REFINE = 4`, `TIKHONOV = 1/4`, `SMOOTH = (1/4, 1, 1/4)`,
/// the occupancy floor read per cell) on NumPy 2.3.5 / Python 3.13.3, by the
/// three documented steps below with `scripts/field_check.py` now carrying
/// production's bound-constrained active-set loop and per-cell floor: NumPy
/// ceiling 0.072526 against the Rust port's 0.07252583 on a global figure of
/// 0.099869, `max |grid_rust - grid_numpy| = 6.0e-7`, Rust 50 CG iterations,
/// relative residual 8.7e-6, 748 supported vertices, 13 saturated. Two
/// things moved the figure from 0.070085: the 2026-10-19 bound-constrained
/// solve (at the 8-row grid it reads 0.069767, and against the clip-after
/// NumPy solver of that day the grids disagreed by 0.138 — the cross-check
/// had gone stale with it, which is why the loop is ported), and the
/// refinement itself, which on THIS pair fits 0.0024 worse at any smoother
/// weight (0.07124 twofold, 0.07253 fourfold) while fitting the night pair
/// better (ceiling 0.0577 → 0.0553) and removing the band the refinement
/// exists for. Below, the earlier provenance as it stood.
///
/// RE-MEASURED 2026-11-06 (v1.18.0, the joint objective) on NumPy 2.3.5 /
/// Python 3.13.3 (Windows 11), by the three documented steps below, after
/// `scripts/field_check.py` was ported to b7630a5 (an unmeasured vertex
/// ships the solve's own value — the script had still zeroed it, and the two
/// grids disagreed by 0.547 at 2320 vertices) and 1320ee9 (the luma gate at
/// the black end): `max |grid_rust - grid_numpy| = 4.5e-7`,
/// `weight_max_abs_delta = 0.0`, NumPy ceiling 0.127520 against the Rust
/// port's 0.127521 on a global figure of 0.245557, 42 iterations. What moved
/// from 0.087511 is the INPUT: the global solve is the joint objective
/// (RULING-1180-joint-r1/r2) and ships a different recipe on this pair, and
/// the field's ceiling is a function of that render. Below, the earlier
/// provenance as it stood.
///
/// RE-MEASURED 2026-08-31 (step 9) on NumPy 2.3.5 / Python 3.13.3
/// (Windows 11), by the same three documented steps:
///   `cargo test --release -- --ignored export_calibration_field_inputs_for_numpy`
///   `python scripts/field_check.py`   (tikhonov=1.0, smooth=(1,1,1), iterations=90)
///   `cargo test --release -- --ignored compare_calibration_field_with_numpy`
/// reported `numpy ceiling (production objective) = 0.070085` against the Rust
/// port's 0.070085, on a global figure of 0.099869, with
/// `max |grid_rust - grid_numpy| = 8.6e-6` and `weight_max_abs_delta = 0.0`.
/// The two solvers still agree to four more digits than the bound asks; what
/// MOVED is the INPUT — step 9's Atmosphere white balance is a per-pixel
/// log-ratio median instead of three marginal medians, this pair's demand
/// (gain ratio 1.6534) is outside the default atmosphere WB budget, so the
/// global render no longer carries a persisted 7100 K / +22.3, and the local
/// field's ceiling is a function of that render. The value before step 9 was
/// 0.067_702_0 (2026-08-30, after step 4a's per-band colour mixer), and
/// 0.070_022_5 before that (2026-08-27, before the mixer). The recon's
/// published grid figures are NOT reusable here: they were measured on the
/// RAW-path renders, a different baseline.
///
/// RE-PINNED 2026-11-09 (gates/j124-fieldcmp, the joint solve under the one
/// law, J91-J122): the same run reported `numpy ceiling (production
/// objective) = 0.041176` against the Rust port's 0.041176, on a global
/// figure of 0.086328, with `max |grid_rust - grid_numpy| = 8.9e-7` and
/// `weight_max_abs_delta = 0.0`. The solvers agree as before; what moved is
/// again the INPUT — the global recipe the joint solve lands on is a
/// different render, and the field's ceiling is a function of that render.
/// The value before the joint solve was 0.127_520_6 (2026-11-04).
const NUMPY_FIELD_CEILING_CALIBRATION: f32 = 0.041_175_7;
/// The largest per-parameter disagreement between the two solvers over all
/// 3072 vertices: measured 1.0e-5 in the 2026-11-04 J* run, 1.5e-6 in the 2026-11-03 preconditioned run, 6.6e-6 in the 2026-11-03 run before it, 1.0e-5 in the 2026-10-29 run, 6.0e-7 in the 2026-10-21 run (1.50e-5 at the 8-row
/// grid in 2026-08), bounded here at a legible 1e-4.
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
            if testifies(&pair.current[i], &pair.target[i]) {
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
        "supported_vertices": supported(&field.occupancy).iter().filter(|&&s| s).count(),
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
        supported(&field.occupancy).iter().filter(|&&s| s).count(),
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

/// v1.18.0, "a zone's correction is read on its own pixels". A two-zone toy at
/// analysis size: a sky over a land of the SAME luma (so the luma bins cannot
/// separate them, as p42's dark horizon sky and dark ground do not), the sky
/// asking +0.5 EV and the land asking nothing, the horizon placed 1.7 rows
/// below vertex row 19 so that row's population is mostly sky while the land
/// rows under it read it. Solved for the zone-owned read, the land pixels next
/// to the horizon read only land-owned vertices — the field moves them by
/// less than the display can show (their 8-bit codes are their input's) — and
/// the sky foot keeps its lift. Read plain (every corner kept, the mutation),
/// the land foot carries the sky's lift.
#[test]
fn a_zone_owned_field_keeps_the_skys_lift_off_the_land_under_the_horizon() {
    let (w, h) = (384usize, 256usize);
    let horizon = 158usize;
    let spacing = (h - 1) as f32 / (FIELD_Y - 1) as f32;
    assert!(
        (horizon as f32 - 19.0 * spacing) > 0.5 && (horizon as f32 - 19.0 * spacing) < 0.5 * spacing,
        "premise: vertex row 19 sits above the horizon, inside its lower half-spacing"
    );
    let mut lcg = Lcg(0x5eed_00b8);
    let current: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let v = 0.30 + 0.02 * (lcg.unit() - 0.5);
            if i / w < horizon { [byte_round(v * 0.9), byte_round(v), byte_round(v * 1.2)] }
            else { [byte_round(v * 1.1), byte_round(v), byte_round(v * 0.85)] }
        })
        .collect();
    let target = plant(&current, w, h, |i, _| if i / w < horizon { 0.5 } else { 0.0 });
    let sky: Vec<f32> = (0..w * h).map(|i| if i / w < horizon { 1.0 } else { 0.0 }).collect();
    let evidence = fit::evidence_model_for(&current, &target, w as u32, h as u32);
    let solve = |sky: Option<&[f32]>| {
        LocalField::solve_zoned(
            &current, &target, w as u32, h as u32, &evidence, TIKHONOV, SMOOTH, ITERATIONS,
            FieldSolveOpts::default(), sky,
        )
        .expect("the toy solves")
    };
    let owned = solve(Some(&sky));
    let zones = owned.zones.as_ref().expect("a zoned solve carries its zones");
    let row_owner = |vy: usize| zones.owners[(vy * FIELD_X + FIELD_X / 2) * FIELD_B + 2];
    assert_eq!(row_owner(19), crate::recipe::ColourField::OWNER_SKY, "premise: the straddling row is the sky's");
    assert_eq!(row_owner(20), crate::recipe::ColourField::OWNER_LAND, "premise: the row under it is the land's");
    let plain = solve(None);
    assert!(plain.zones.is_none() && plain.as_recipe_field().owners.is_empty(), "no partition, no owners");
    let foot = |field: &LocalField, rows: std::ops::Range<usize>| {
        let out = field.render(&current);
        let (mut worst, mut lift, mut n) = (0u8, 0.0f64, 0usize);
        for y in rows {
            for x in 0..w {
                let i = y * w + x;
                for c in 0..3 {
                    worst = worst.max(byte_of(out[i][c]).abs_diff(byte_of(current[i][c])));
                    lift += (out[i][c] - current[i][c]) as f64;
                    n += 1;
                }
            }
        }
        (worst, lift / n as f64 * 255.0)
    };
    let (land_owned, land_owned_mean) = foot(&owned, horizon..horizon + 4);
    let (land_plain, land_plain_mean) = foot(&plain, horizon..horizon + 4);
    let (_, sky_owned) = foot(&owned, horizon - 4..horizon);
    let (_, sky_plain) = foot(&plain, horizon - 4..horizon);
    let (_, sky_asked) = {
        let mut lift = 0.0f64;
        for i in (horizon - 4) * w..horizon * w {
            for c in 0..3 { lift += (target[i][c] - current[i][c]) as f64; }
        }
        (0u8, lift / (4 * w * 3) as f64 * 255.0)
    };
    eprintln!(
        "land foot: owned worst {land_owned} codes mean {land_owned_mean:+.3} | plain worst {land_plain} mean {land_plain_mean:+.3}; \
         sky foot lift: asked {sky_asked:+.3} owned {sky_owned:+.3} plain {sky_plain:+.3}"
    );
    assert_eq!(land_owned, 0, "the land foot reads no sky lift: its codes are its input's");
    assert!(land_plain > 0, "premise of the mutation: read plain, the land foot lifts");
    assert!(sky_owned >= sky_plain, "the sky foot keeps its lift: {sky_owned:+.3} against the plain read's {sky_plain:+.3}");
}

/// v1.18.0, "the field is read on the partition its owners were counted on"
/// and "the field's read is the owner count transposed". A soft zone edge:
/// the sky's coverage falls from 1 to 0 over 24 pixel rows that all lie
/// between a sky-owned vertex row (lifted +0.5 EV) and a land-owned one (no
/// move), so every pixel on the ramp has corners of both zones. With `a` the
/// sky-owned row's trilinear weight, the count transposed gives that row
/// `a·c` and the land-owned row `(1 − a)·(1 − c)`, renormalised once, so every
/// row reads the full lift × `a·c / (a·c + (1 − a)·(1 − c))` — continuous in
/// both `c` and `a`, and monotone along the ramp because both fall; read on a
/// hard cut at one half (the mutation) the whole lift jumps between two rows
/// — the iso-line drawn into the picture.
/// The solve reads the same partition: on a soft-edged two-zone toy the
/// analyzer's forward map over the zone-owned splat and the engine's render
/// of the solved field are the same bytes.
#[test]
fn a_soft_zone_edge_reads_the_field_at_its_coverage_without_a_step() {
    use crate::recipe::ColourField as F;
    let (w, h) = (8usize, 64usize);
    let (fx, fy, fb) = (2usize, 3usize, 2usize);
    let lifted = |v: usize| v / fb / fx < 2;
    let field = F {
        x: fx, y: fy, b: fb,
        grid: (0..fx * fy * fb).map(|v| if lifted(v) { [0.5, 0.0, 0.0, 0.0, 0.0] } else { [0.0; 5] }).collect(),
        owners: (0..fx * fy * fb).map(|v| if lifted(v) { F::OWNER_SKY } else { F::OWNER_LAND }).collect(),
        amount: 1.0, enabled: true,
    };
    // Vertex rows at pixel rows 0, 31.5 and 63: rows 32..=56 lie between the
    // sky-owned row 1 and the land-owned row 2.
    let (top, run) = (32usize, 24usize);
    let coverage = |y: usize| ((top + run) as f32 - y as f32).clamp(0.0, run as f32) / run as f32;
    let sky: Vec<f32> = (0..w * h).map(|i| coverage(i / w)).collect();
    assert!((0..h).filter(|&y| coverage(y) > 0.0 && coverage(y) < 1.0).count() == run - 1, "premise: a 24-row ramp");
    let input = vec![[0.35f32, 0.35, 0.35]; w * h];
    let mut read = input.clone();
    crate::render::apply_colour_field(&mut read, w, h, Some(&field), Some(&sky));
    let row = |y: usize| read[y * w + w / 2][1];
    let lift = row(top) - input[0][1];
    assert!(lift > 0.05, "premise: the sky reads its lift above the ramp: {lift}");
    assert_eq!(row(top + run), input[0][1], "premise: the land under the ramp reads no lift");
    let step = lift / run as f32;
    let mut widest = 0.0f32;
    for y in top..top + run {
        let fall = row(y) - row(y + 1);
        assert!(fall >= -1e-7, "the read along the ramp is monotone: row {y} {} -> {}", row(y), row(y + 1));
        widest = widest.max(fall);
    }
    // The count transposed, row by row: the sky-owned vertex row sits at
    // grid row 1, pixel row y at grid row y·(fy − 1)/(h − 1).
    let law = |y: usize| {
        let a = 2.0 - y as f64 * (fy - 1) as f64 / (h - 1) as f64;
        let c = coverage(y) as f64;
        lift as f64 * a * c / (a * c + (1.0 - a) * (1.0 - c))
    };
    let mut widest_law = 0.0f64;
    for y in top..=top + run {
        let read = (row(y) - input[0][1]) as f64;
        assert!(
            (read - law(y)).abs() <= 1e-3 * step as f64,
            "row {y} reads the count transposed: {read:.6} against {:.6}", law(y)
        );
        if y < top + run { widest_law = widest_law.max(law(y) - law(y + 1)); }
    }
    eprintln!(
        "ramp: full lift {lift:.5}, one coverage step x lift {step:.5}, widest neighbour step {widest:.5} (the law's {widest_law:.5})"
    );

    // The solve's forward map reads the same partition as the engine.
    let (w, h) = (192usize, 128usize);
    let (top, run) = (74usize, 24usize);
    let coverage = |y: usize| ((top + run) as f32 - y as f32).clamp(0.0, run as f32) / run as f32;
    let sky: Vec<f32> = (0..w * h).map(|i| coverage(i / w)).collect();
    let current = ramp(w, h, 0.25, 0.40, 0x5eed_0b1d);
    let target = plant(&current, w, h, |i, _| 0.5 * coverage(i / w));
    let evidence = fit::evidence_model_for(&current, &target, w as u32, h as u32);
    let field = LocalField::solve_zoned(
        &current, &target, w as u32, h as u32, &evidence, TIKHONOV, SMOOTH, ITERATIONS,
        FieldSolveOpts::default(), Some(&sky),
    )
    .expect("the soft-edged toy solves");
    let guide = smooth_3tap_luma(&current, w, h);
    let mut splat = splat_table(&guide, w, h);
    let zones = own_splat(&mut splat, &sky);
    assert_eq!(Some(&zones.owners), field.zones.as_ref().map(|z| &z.owners), "one count of the owners");
    let flat: Vec<f32> = field.grid.iter().flatten().copied().collect();
    let analyzer = apply_flat(&current, &guide, &splat, &flat);
    let engine = field.render(&current);
    let soft_moved = (0..w * h)
        .filter(|&i| sky[i] > 0.0 && sky[i] < 1.0 && engine[i] != current[i])
        .count();
    assert!(soft_moved > w * run / 2, "premise: the field moves the soft rows ({soft_moved} pixels)");
    for (i, (a, b)) in analyzer.iter().zip(&engine).enumerate() {
        for channel in 0..3 {
            assert_eq!(a[channel].to_bits(), b[channel].to_bits(), "pixel {i} channel {channel}");
        }
    }
}

/// v1.18.0, "the field's read is the owner count transposed: each corner
/// carries the pixel's share in that corner's zone, renormalised once". A
/// pixel path at sky coverage one half runs along a row across the vertex
/// plane between two cells: on its left every cell has a sky-owned corner
/// column (lifted +0.5 EV) and a land-owned one (no move), on its right the
/// cell's corners are all land-owned (lifted +0.25 EV), so the sky owns none
/// of them. At `c = 1/2` the sky and land shares are equal, so on cells whose
/// corners are all owned the transposed read is the plain trilinear read, and
/// no two neighbouring pixels differ by more than one pixel's trilinear change
/// (the plain read's own widest neighbour step) — continuous across the plane.
/// A read renormalised per zone onto that zone's own corners (the mutation)
/// holds the sky-owned column's lift across the whole mixed cell and drops to
/// the land cell's plain read at the plane: a jump of half the lift's gap.
#[test]
fn a_half_covered_pixel_path_reads_the_field_continuously_across_a_vertex_plane() {
    use crate::recipe::ColourField as F;
    let (w, h) = (65usize, 4usize);
    let (fx, fy, fb) = (3usize, 2usize, 2usize);
    // Splat order: the luma bin fastest, then x, then y (`(y * fx + x) * fb + b`).
    let column = |v: usize| v / fb % fx;
    let field = F {
        x: fx, y: fy, b: fb,
        grid: (0..fx * fy * fb)
            .map(|v| match column(v) { 0 => [0.5, 0.0, 0.0, 0.0, 0.0], 1 => [0.0; 5], _ => [0.25, 0.0, 0.0, 0.0, 0.0] })
            .collect(),
        owners: (0..fx * fy * fb).map(|v| if column(v) == 0 { F::OWNER_SKY } else { F::OWNER_LAND }).collect(),
        amount: 1.0, enabled: true,
    };
    let sky = vec![0.5f32; w * h];
    let input = vec![[0.35f32, 0.35, 0.35]; w * h];
    let mut owned = input.clone();
    crate::render::apply_colour_field(&mut owned, w, h, Some(&field), Some(&sky));
    let mut plain = input.clone();
    crate::render::apply_colour_field(&mut plain, w, h, Some(&field), None);
    let y = h / 2;
    let px = |img: &[[f32; 3]], x: usize| img[y * w + x][1];
    let plane = (w - 1) / 2;
    assert!(px(&plain, 0) > px(&plain, plane) && px(&plain, w - 1) > px(&plain, plane), "premise: the plane's column reads no move");
    let trilinear_step = (0..w - 1).map(|x| (px(&plain, x + 1) - px(&plain, x)).abs()).fold(0.0f32, f32::max);
    assert!(trilinear_step > 0.0, "premise: the plain read moves along the row");
    let widest = (0..w - 1).map(|x| (px(&owned, x + 1) - px(&owned, x)).abs()).fold(0.0f32, f32::max);
    let across = (px(&owned, plane + 1) - px(&owned, plane - 1)).abs();
    eprintln!("cell plane at x {plane}: one pixel's trilinear change {trilinear_step:.5}, widest owned step {widest:.5}, across the plane {across:.5}");
    assert!(
        widest <= trilinear_step * (1.0 + 1e-3),
        "no step beyond one pixel's trilinear change: {widest:.5} > {trilinear_step:.5}"
    );
}
