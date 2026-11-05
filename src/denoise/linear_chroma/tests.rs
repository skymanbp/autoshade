use super::*;

/// A deterministic standard normal for site `i` of stream `seed`.
fn normal(seed: u64, i: u64) -> f64 {
    let h = |x: u64| {
        let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let u1 = ((h(seed.wrapping_mul(1_000_003) ^ (2 * i)) >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
    let u2 = ((h(seed.wrapping_mul(1_000_033) ^ (2 * i + 1)) >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

const W: usize = 1024;
const H: usize = 768;
const WB: [f64; 3] = [2.5, 1.0, 1.6];
/// Photon and read noise of full scale (read sd 0.002), no gain.
const A: f64 = 2e-4;
const B: f64 = 4e-6;

/// An RGGB mosaic of a grey scene at `level` (green), its red-minus-green
/// colour difference stepped by `step` (white-balanced units) right of
/// `x = W/2`, with photon and read noise.
fn mosaic(level: f64, step: f64) -> Vec<f32> {
    let mut out = vec![0.0f32; W * H];
    out.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let s = match (y % 2, x % 2) {
                (0, 0) => (level + if x >= W / 2 { step } else { 0.0 }) / WB[0],
                (1, 1) => level / WB[2],
                _ => level,
            };
            *v = (s + (A * s + B).sqrt() * normal(3, (y * W + x) as u64)) as f32;
        }
    });
    out
}

/// Per column of cells, the mean of `wR·R − G` over all rows.
fn column_dr(m: &[f32]) -> Vec<f64> {
    (0..W / 2)
        .map(|cx| {
            (0..H / 2)
                .map(|cy| {
                    let r = f64::from(m[2 * cy * W + 2 * cx]);
                    let g = (f64::from(m[2 * cy * W + 2 * cx + 1]) + f64::from(m[(2 * cy + 1) * W + 2 * cx])) / 2.0;
                    WB[0] * r - g
                })
                .sum::<f64>()
                / (H / 2) as f64
        })
        .collect()
}

fn var_dr(m: &[f32], cols: std::ops::Range<usize>) -> f64 {
    let v: Vec<f64> = (0..H / 2)
        .flat_map(|cy| cols.clone().map(move |cx| (cx, cy)))
        .map(|(cx, cy)| {
            let r = f64::from(m[2 * cy * W + 2 * cx]);
            let g = (f64::from(m[2 * cy * W + 2 * cx + 1]) + f64::from(m[(2 * cy + 1) * W + 2 * cx])) / 2.0;
            WB[0] * r - g
        })
        .collect();
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (v.len() - 1) as f64
}

fn rggb() -> rawler::cfa::CFA {
    rawler::cfa::CFA::new("RGGB")
}

/// The fair test over a window of half side `rho`: [`fair_k`] of its cells.
fn fair(rho: usize) -> (f64, usize) {
    (fair_k(((2 * rho + 1) * (2 * rho + 1)) as f64), rho)
}

/// The constant's reading of cell `i` (the skipped mean over the cell's
/// window at its own noise, [`skipped_mean`]): what the cell keeps where its
/// plane is refuted.
fn constant_reading(cells: &Cells, i: usize, k: f64, rho: usize) -> [f32; 2] {
    let (cw, ch) = (cells.cw, cells.ch);
    let (cx, cy) = (i % cw, i / cw);
    let (sd, _) = cells.own_noise(i);
    let (y0, y1, x0, x1) = (cy.saturating_sub(rho), (cy + rho + 1).min(ch), cx.saturating_sub(rho), (cx + rho + 1).min(cw));
    [0, 1].map(|j| {
        let w: Vec<f32> = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (y, x))).map(|(y, x)| cells.d[y * cw + x][j]).collect();
        skipped_mean(&w, cells.d[i][j], sd[j], k, &mut Vec::new()).0
    })
}

/// The plane's share of a cell's own noise at its fixed point over a full
/// window of half side `rho`, `k` in the cell's true sd (the module):
/// `1/Σᵢ (Z(tᵢ) − 2tᵢ·φ(tᵢ))`, `tᵢ = k·√(1 + hᵢ)`,
/// `hᵢ = (1/N + uᵢ²/Σu² + vᵢ²/Σv²)/Z`, `Z` the accepted share (read with the
/// `tᵢ` in turn).
fn plane_share(k: f64, rho: usize) -> f64 {
    let r = rho as i64;
    let side = (2 * rho + 1) as f64;
    let n = side * side;
    let su2: f64 = side * (-r..=r).map(|u| (u * u) as f64).sum::<f64>();
    let mut z = normal_mass(k * (1.0 + 1.0 / n).sqrt()).0;
    let mut share = 0.0;
    for _ in 0..6 {
        let (mut total, mut zsum) = (0.0f64, 0.0f64);
        for u in -r..=r {
            for v in -r..=r {
                let h = (1.0 / n + (u * u) as f64 / su2 + (v * v) as f64 / su2) / z;
                let t = k * (1.0 + h).sqrt();
                let (zt, phi) = normal_mass(t);
                total += zt - 2.0 * t * phi;
                zsum += zt;
            }
        }
        z = zsum / n;
        share = 1.0 / total;
    }
    share
}

#[test]
fn the_stage_takes_the_colour_noise_and_leaves_the_greens_and_the_mean() {
    let before = mosaic(0.01, 0.0);
    let mut after = before.clone();
    let (SourceReading { model }, _) = filter_mosaic(&mut after, W, H, &rggb(), WB, fair(3), None, 0, true).expect("a Bayer mosaic with noise fits a model");
    // The greens are never written.
    for y in 0..H {
        for x in 0..W {
            if (y + x) % 2 == 1 {
                assert_eq!(before[y * W + x].to_bits(), after[y * W + x].to_bits(), "green site ({x}, {y}) moved");
            }
        }
    }
    // The colour noise falls by more than half…
    let (v0, v1) = (var_dr(&before, 0..W / 2), var_dr(&after, 0..W / 2));
    assert!(v1 < 0.5 * v0, "colour-difference variance {v0:.3e} -> {v1:.3e}");
    // …and the frame's mean colour stays where it was (within a tenth of
    // one cell's noise sd over the whole frame).
    let (m0, m1) = (column_dr(&before), column_dr(&after));
    let (s0, s1) = (m0.iter().sum::<f64>() / m0.len() as f64, m1.iter().sum::<f64>() / m1.len() as f64);
    assert!((s1 - s0).abs() < 0.1 * v0.sqrt(), "mean colour difference {s0:.3e} -> {s1:.3e} (cell sd {:.3e})", v0.sqrt());
    assert!(model.a > 0.0 && model.b > 0.0, "{model:?}");
}

#[test]
fn a_colour_edge_the_noise_cannot_make_is_kept() {
    // A step of 12 cell noise sds in R − G: far outside the threshold, so the
    // window never mixes the two sides and the step comes out whole.
    let level = 0.01;
    let cell_sd = (WB[0] * WB[0] * (A * level / WB[0] + B) + (A * level + B) / 2.0).sqrt();
    let step = 12.0 * cell_sd;
    let before = mosaic(level, step);
    let mut after = before.clone();
    filter_mosaic(&mut after, W, H, &rggb(), WB, fair(3), None, 0, true).expect("model");
    let (m0, m1) = (column_dr(&before), column_dr(&after));
    let side = |m: &[f64], cols: std::ops::Range<usize>| cols.clone().map(|c| m[c]).sum::<f64>() / cols.len() as f64;
    let e = W / 4; // the edge, in cells
    for c in [e - 2, e - 1, e, e + 1] {
        // the columns at the edge keep their own side's colour
        let want = if c < e { side(&m0, 0..e - 8) } else { side(&m0, e + 8..W / 2) };
        assert!((m1[c] - want).abs() < 0.15 * step, "column {c}: {:.3e} vs its side {want:.3e} (step {step:.3e})", m1[c]);
    }
    let kept = side(&m1, e..e + 4) - side(&m1, e - 4..e);
    assert!((kept / step - 1.0).abs() < 0.1, "the step {step:.3e} came out {kept:.3e}");
}

/// The test's noise is the hypothesis's: the anchor's own level. A dark
/// region (green 0.01) beside a bright one (green 0.5) of one hue
/// (`wR·R = hue·G`), the edge in the middle of a patch, under the module's
/// photon and read noise: the patch straddling the edge has its mean level,
/// and so its sd, between the two sides'. The test's `k` is the law's (the
/// fair test of half side 3); the fixture's step is sized to the two sds it
/// tells apart — `k·√(σ_own·σ_patch)` in the sd of a difference of two cells
/// (`σ_d`, the first pass's width), so the step lies between `k` dark cells'
/// own `σ_d` and `k` of the straddling patch's (about 5 and 2 of them: the
/// hue solved for it from the noise law): a dark cell tested at the patch's
/// sd lets the bright side vote; tested at its own it does not. The dark
/// columns of that patch keep their own colour difference: the mean over the
/// rows within 0.15 of the step of the noiseless scene's (the colour edge
/// test's limit), the column next to the edge excluded on neither side (a
/// red site's four greens are all dark there). (Before the law, this test
/// read `k` as the geometric mean of the two sds' steps, 21.2, the free
/// number of the law it replaced: rel1180 msg-e3 round 17.)
///
/// MUTATION: the test's sd read from the cell's patch (the form before
/// rel1180 round 14).
#[test]
fn a_dark_cells_test_is_at_its_own_noise() {
    let (dark, bright) = (0.01, 0.5);
    let (k, rho) = fair(3);
    // One cell's R − G sd at a level and hue: the site's own and its four greens.
    let cell_sd = |g: f64, hue: f64| (WB[0] * WB[0] * (A * hue * g / WB[0] + B) + (A * g + B) / 4.0).sqrt();
    let mean_level = (dark + bright) / 2.0;
    let sd_d = |g: f64, hue: f64| 2f64.sqrt() * cell_sd(g, hue);
    let mut hue = 1.0;
    for _ in 0..20 {
        hue = 1.0 + k * (sd_d(dark, hue) * sd_d(mean_level, hue)).sqrt() / (bright - dark);
    }
    let step = (hue - 1.0) * (bright - dark);
    println!(
        "k {k:.3} over half side {rho}: hue {hue:.4}, step {step:.4e} = {:.2} own sd_d, {:.2} patch sd_d",
        step / sd_d(dark, hue),
        step / sd_d(mean_level, hue)
    );
    assert!(step / sd_d(mean_level, hue) < k && k < step / sd_d(dark, hue));
    let e = W / 4 + PATCH / 2; // the edge, in cells: mid-patch
    let scene = |x: usize| if x / 2 < e { dark } else { bright };
    let site = |x: usize, y: usize, g: f64| match (y % 2, x % 2) {
        (0, 0) => hue * g / WB[0],
        (1, 1) => g / WB[2],
        _ => g,
    };
    let make = |noise: bool| -> Vec<f32> {
        let mut out = vec![0.0f32; W * H];
        out.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let s = site(x, y, scene(x));
                *v = (s + if noise { (A * s + B).sqrt() * normal(41, (y * W + x) as u64) } else { 0.0 }) as f32;
            }
        });
        out
    };
    let (m, clean) = (make(true), make(false));
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let pts = patches(&m, W, H, sites, WB);
    let model = NoiseModel::fit(&pts, W, H).expect("a Bayer mosaic with noise fits a model");
    let cells = Cells::of(&m, W, H, sites, WB, &model, &pts).expect("cells");
    let truth = Cells::of(&clean, W, H, sites, WB, &model, &patches(&clean, W, H, sites, WB)).expect("cells");
    let col = |c: &Cells, cx: usize, f: &(dyn Fn(usize) -> f64 + Sync)| (0..c.ch).into_par_iter().map(|cy| f(cy * c.cw + cx)).sum::<f64>() / c.ch as f64;
    for cx in e - PATCH / 2..e {
        let got = col(&cells, cx, &|i| f64::from(cells.filtered(i, k, rho)[0]));
        let want = col(&truth, cx, &|i| f64::from(truth.d[i][0]));
        println!("column {cx} (edge at {e}): R - G {got:.4e} against the scene's {want:.4e}: {:+.3} of the step", (got - want) / step);
        assert!(
            (got - want).abs() < 0.15 * step,
            "column {cx} (edge at {e}, k {k:.2}): R - G {got:.4e} against the scene's {want:.4e} (step {step:.4e}; own sd_d {:.4e}, the patch's mean-level sd_d {:.4e})",
            sd_d(dark, hue),
            sd_d(mean_level, hue)
        );
    }
}

/// The test is fair to its window: of N true neighbours, fewer than one is
/// expected to be refused. On three windows (half sides 3, 8 and 11: 49, 289
/// and 529 cells) the law's `k` puts the expected false refusals,
/// `2·N·Φ(−k) = N·(1 − Z(k))`, at one: the closed form `Φ⁻¹(1 − 1/(2N))`,
/// read here by bisection on the module's own [`normal_mass`]. Each strength
/// of the slider is a fixed point of the window and the law (printed) — or,
/// where the whole half side has none, the larger window of the two it
/// alternates between — the same above 100 as at 100; 0 is off.
///
/// MUTATION: the expected refusals set to N/2 (a `k` of 0.67): this test, the
/// 12-sd colour edge test and the own-noise test fail.
#[test]
fn the_test_is_fair_to_its_window() {
    for rho in [3usize, 8, 11] {
        let n = ((2 * rho + 1) * (2 * rho + 1)) as f64;
        let (mut lo, mut hi) = (0.0f64, 10.0f64);
        while hi - lo > 1e-12 {
            let mid = (lo + hi) / 2.0;
            if n * (1.0 - normal_mass(mid).0) > 1.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let want = (lo + hi) / 2.0;
        let k = fair_k(n);
        println!("half side {rho}, {n} cells: k {k:.6} (closed form {want:.6}), expected false refusals {:.6}", n * (1.0 - normal_mass(k).0));
        assert!((k - want).abs() < 1e-9, "half side {rho}: the law's k {k} against the closed form {want}");
    }
    assert_eq!(fair_test(0.0), None, "0 is off");
    for nr in [1.0f32, 10.0, 20.0, 25.0, 50.0, 75.0, 100.0] {
        let (k, rho) = fair_test(nr).expect("on");
        let n = ((2 * rho + 1) * (2 * rho + 1)) as f64;
        println!("colour NR {nr}: half side {rho} ({n} cells), k {k:.4}");
        let (w, cycle) = (window(nr, k), rho > 1 && window(nr, fair(rho - 1).0) == rho);
        println!("colour NR {nr}: the window at the test's k {w}{}", if cycle && w == rho - 1 { " (no whole fixed point: the larger of the two)" } else { "" });
        assert!(w == rho || w == rho - 1 && cycle, "colour NR {nr}: the window at the test's k is {w}, not {rho}");
        assert_eq!(k.to_bits(), fair_k(n).to_bits(), "colour NR {nr}: the test's k for its window");
    }
    assert_eq!(fair_test(250.0), fair_test(100.0));
}

/// The slider weakens the stage through its window: at 10 it is 1 cell, at
/// 25 it is 7 at an unbounded `k` (as many samples as the display pass
/// averages there), and on a noise-only frame the residual colour noise at
/// 10 is the larger.
///
/// MUTATION: the window fixed at the default strength's (the slider is then
/// dead below 25).
#[test]
fn the_slider_weakens_the_stage_through_its_window() {
    let before = mosaic(0.01, 0.0);
    let at = |nr: f32| {
        let mut m = before.clone();
        filter_mosaic(&mut m, W, H, &rggb(), WB, fair_test(nr).expect("on"), None, 0, true).expect("a model");
        var_dr(&m, 0..W / 2)
    };
    assert_eq!((window(10.0, f64::INFINITY), window(25.0, f64::INFINITY), window(100.0, f64::INFINITY)), (1, 7, 23), "the display pass's sample count in cells");
    let (v10, v25) = (at(10.0), at(25.0));
    assert!(v10 > v25, "residual colour variance at 10 ({v10:.3e}) must exceed that at 25 ({v25:.3e})");
}

/// The window holds as many samples as the display pass averages: at each
/// strength the odd square of cells nearest (in half side) to
/// `display_samples / 4` cells, the half side `(√N − 1)/2` rounded and at
/// least one; printed per strength with the display pass's noise share.
///
/// MUTATION: the window from the pixel count instead of the cell count (the
/// stage then averages four times the samples the display pass does).
#[test]
fn the_window_averages_as_many_samples_as_the_display_pass_it_replaces() {
    for nr in [10.0f32, 20.0, 25.0, 50.0, 75.0, 100.0] {
        let n_px = display_samples(nr);
        let rho = plain_window(nr);
        let cells = n_px / 4.0;
        println!("colour NR {nr}: display share {:.6}, N {n_px:.1} px = {cells:.1} cells -> half side {rho} cells", 1.0 / n_px);
        let want = (((cells.sqrt() - 1.0) / 2.0).round().max(1.0)) as usize;
        assert_eq!(rho, want, "at {nr}: {cells:.1} cells want a half side of {want}");
        // The nearest odd square in half side: no other half side is nearer.
        let off = |r: usize| ((2 * r + 1) as f64 - cells.sqrt()).abs();
        assert!(rho == 1 || off(rho) <= off(rho - 1) && off(rho) <= off(rho + 1), "at {nr}: half side {rho}");
    }
    assert_eq!(plain_window(DEFAULT_AMOUNT), 7, "at 25 the display pass averages 1005 pixels, 251 cells");
}

/// The stage's window leaves the display pass's share at the width it runs
/// at: the skipped mean over `N` cells at `t` keeps `1/(N·(Z − 2t·φ(t)))` of
/// a sample's noise, so the window is the odd square nearest (in half side)
/// to the `N` at which that is the display share `4 / display_samples`,
/// `t = k·√(1 + 1/(N·Z))` read with it to its fixed point — at each
/// strength's fair test ([`fair_test`]) and at an unbounded `k` (the plain
/// mean's window). Printed per strength.
///
/// MUTATION: the window from the plain mean's share (the `Z − 2t·φ(t)`
/// factor dropped): the window is then the plain mean's at every `k`.
#[test]
fn the_window_leaves_the_display_share_at_the_width_the_stage_runs_at() {
    for fair_at in [true, false] {
        for nr in [10.0f32, 20.0, 25.0, 50.0, 75.0, 100.0] {
            let ks = if fair_at { fair_test(nr).expect("on").0 } else { f64::INFINITY };
            let k = ks;
            let plain = display_samples(nr) / 4.0;
            let (mut t, mut n) = (ks, plain);
            for _ in 0..50 {
                if !t.is_finite() {
                    break;
                }
                let (z, phi) = normal_mass(t);
                n = plain / (z - 2.0 * t * phi);
                t = ks * (1.0 + 1.0 / (n * z)).sqrt();
            }
            let rho = window(nr, ks);
            println!("k {k:.3} colour NR {nr}: k_s {ks:.3}, t {t:.3}, N {n:.1} cells (plain {plain:.1}) -> half side {rho} cells");
            let off = |n: f64, r: usize| ((2 * r + 1) as f64 - n.sqrt()).abs();
            assert!(
                (rho == 1 || off(n, rho) <= off(n, rho - 1)) && off(n, rho) <= off(n, rho + 1),
                "k {k}, colour NR {nr}: the skipped mean at t = {t:.3} leaves the display share over {n:.1} cells, half side {rho} is not the nearest"
            );
            if fair_at {
                // The plane's share over the same window (the module): the N at
                // which it is the display share, in the ratio of the two shares
                // there, holds the same half side.
                let cells = ((2 * rho + 1) * (2 * rho + 1)) as f64;
                let tc = ks * (1.0 + 1.0 / (cells * normal_mass(t).0)).sqrt();
                let (z, phi) = normal_mass(tc);
                let (constant, plane) = (1.0 / (cells * (z - 2.0 * tc * phi)), plane_share(ks, rho));
                let n_plane = n * plane / constant;
                println!(
                    "  colour NR {nr}: half side {rho} ({cells} cells), k {ks:.4}: share of a cell's noise, constant {constant:.6}, plane {plane:.6} -> N {n_plane:.1}"
                );
                assert!(
                    (rho == 1 || off(n_plane, rho) <= off(n_plane, rho - 1)) && off(n_plane, rho) <= off(n_plane, rho + 1),
                    "colour NR {nr}: the plane's share asks for another half side than {rho} ({n_plane:.1} cells)"
                );
            }
        }
    }
    assert_eq!(window(DEFAULT_AMOUNT, f64::INFINITY), plain_window(DEFAULT_AMOUNT), "an unbounded k: the plain mean's window");
}

/// On a flat frame the filter keeps of a cell's own noise what its estimate
/// at its test width keeps, and no more: the plane's share at its fixed point,
/// `1/Σᵢ (Z(tᵢ) − 2tᵢ·φ(tᵢ))` with `tᵢ = k·√(1 + hᵢ)` at each place of the
/// window ([`plane_share`], derived in the module to first order: the
/// skipped mean's `1/(N·(Z − 2t·φ(t)))` with `t` read at each place), on the
/// cells read on their plane, and the skipped mean's on the cells whose
/// plane the whiteness read refuted (counted here) — `k` in the cell's true
/// sd (the model's sd over the true one carried in) — plus what its four
/// axis neighbours carry of it through the green they share with it — each
/// sample enters the estimate with an influence whose mean slope is 1
/// (Stein's lemma on jointly normal samples), so each neighbour adds
/// `cov/(N·v)` (`cov = σ²_G/16`, `v` one cell's variance) — within three
/// standard errors of the slope (its residual sd over the sample sd, over
/// the square root of the cells that are independent: one per window). At
/// the fair test of two windows (half sides 3 and 7: `k` 2.32 and 2.84).
///
/// MUTATION: every pass anchored on the cell's own sample (the form before
/// rel1180 round 8): the test then keeps the neighbours on the sample's side
/// and the slope is several times the bound.
#[test]
fn the_filter_keeps_of_a_cells_own_noise_only_what_the_skipped_mean_keeps() {
    let level = 0.01;
    let m = mosaic(level, 0.0);
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let pts = patches(&m, W, H, sites, WB);
    let model = NoiseModel::fit(&pts, W, H).expect("model");
    let cells = Cells::of(&m, W, H, sites, WB, &model, &pts).expect("cells");
    // The true sd of each colour difference of one cell (the mosaic's own
    // noise: the site's, its four green neighbours averaged), and the
    // covariance of two axis-neighbouring cells' differences (one green shared).
    let g_var = (A * level + B) / 4.0;
    let shared = (A * level + B) / 16.0;
    let true_sd = [
        (WB[0] * WB[0] * (A * level / WB[0] + B) + g_var).sqrt(),
        (WB[2] * WB[2] * (A * level / WB[2] + B) + g_var).sqrt(),
    ];
    for (k, rho) in [fair(3), fair(7)] {
        let n_win = ((2 * rho + 1) * (2 * rho + 1)) as f64;
        let interior: Vec<usize> = (rho..cells.ch - rho).flat_map(|cy| (rho..cells.cw - rho).map(move |cx| cy * W / 2 + cx)).collect();
        let f: Vec<([f32; 2], [u32; 2], [Read; 2])> = interior.par_iter().map(|&i| cells.filtered_counted(i, k, rho)).collect();
        for (j, &true_sd) in true_sd.iter().enumerate() {
            let (mut sdd, mut sdf) = (0.0f64, 0.0f64);
            for (n, &i) in interior.iter().enumerate() {
                let d = f64::from(cells.d[i][j]);
                sdd += d * d;
                sdf += d * f64::from(f[n].0[j]);
            }
            let slope = sdf / sdd;
            let resid: f64 = interior
                .iter()
                .enumerate()
                .map(|(n, &i)| (f64::from(f[n].0[j]) - slope * f64::from(cells.d[i][j])).powi(2))
                .sum::<f64>()
                / interior.len() as f64;
            let se = resid.sqrt() / (sdd / interior.len() as f64).sqrt() / (interior.len() as f64 / n_win).sqrt();
            let r = f64::from(cells.sd[0][j]) / 2f64.sqrt() / true_sd;
            let mut t = k * r * 2f64.sqrt();
            // t and n = N·Z depend on each other; the iteration settles within a few steps.
            for _ in 0..8 {
                t = k * r * (1.0 + 1.0 / (n_win * normal_mass(t).0)).sqrt();
            }
            let (z, phi) = normal_mass(t);
            let constant = 1.0 / (n_win * (z - 2.0 * t * phi));
            let plane = plane_share(k * r, rho);
            let refuted = f.iter().filter(|q| q.2[j] != Read::Plane).count() as f64 / f.len() as f64;
            let want = (1.0 - refuted) * plane + refuted * constant + 4.0 * shared / (n_win * true_sd * true_sd);
            println!(
                "k {k:.3} over half side {rho}, colour difference {j}: slope {slope:.5} (se {se:.5}); the plane {plane:.5}, the constant {constant:.5}, refuted {refuted:.5} of the cells -> {want:.5}"
            );
            assert!(
                (slope - want).abs() <= 3.0 * se,
                "k {k:.3} over half side {rho}, colour difference {j}: the filter keeps {slope:.4} of a cell's own noise; the plane at k = {:.3} keeps {plane:.4}, the constant {constant:.4}, {refuted:.4} of the cells refuted -> {want:.4} (3 se {:.4}; the window mean 1/N = {:.4})",
                k * r,
                3.0 * se,
                1.0 / n_win
            );
        }
    }
}

/// The ramp's cells ([`ramp_mosaic`]): R − G rises from column `RAMP.0` to
/// `RAMP.1`.
const RAMP: (usize, usize) = (160, 352);

/// An RGGB mosaic of a grey at 0.01 whose R − G rises by `s` cell sds a cell
/// over [`RAMP`] (read-noise dominated, so a cell's sd is nearly the same
/// along it), and that sd.
fn ramp_mosaic(s: f64) -> (Vec<f32>, f64) {
    const A2: f64 = 2e-6;
    let level = 0.01;
    let cell_sd = (WB[0] * WB[0] * (A2 * level / WB[0] + B) + (A2 * level + B) / 4.0).sqrt();
    let (x0, x1) = RAMP;
    let ramp = |cx: usize| s * cell_sd * (cx.clamp(x0, x1) - x0) as f64;
    let mut m = vec![0.0f32; W * H];
    m.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let sv = match (y % 2, x % 2) {
                (0, 0) => (level + ramp(x / 2)) / WB[0],
                (1, 1) => level / WB[2],
                _ => level,
            };
            *v = (sv + (A2 * sv + B).sqrt() * normal(57, (y * W + x) as u64)) as f32;
        }
    });
    (m, cell_sd)
}

/// A ramp is read on its plane. A noisy R − G ramp of `s` cell sds per cell
/// over 192 cells (read-noise dominated, so a cell's sd is the same along
/// it), no edge: the filtered field's step energy along the ramp — the mean
/// of its squared forward differences less the ramp's own step² — is within
/// the measurement's error of zero, at every slope the round-16 probe read
/// (0.5, 1 and 1.5 sd per cell) over the fair test of half sides 3 and 8.
/// The measurement's error: the standard error of the same statistic on the
/// unfiltered differences (its noise, `2v` per difference), from blocks of
/// `2ρ + 1` rows (a window's rows are not independent). A plateau reads as
/// a drop and a step, more step energy than the ramp has; a test against
/// one constant holds a strip of the ramp and walks along it (rel1180 msg-e3
/// round 16: 3.67 of a cell's variance at 1 sd per cell, half side 8, more
/// than the noisy input's own 2.17). A flat ramp (slope 0) printed beside;
/// the share of cells whose plane the whiteness read refuted printed.
///
/// MUTATION: the plane's slopes forced to zero (the constant form): the ramp
/// then holds the strip mean's step energy (rel1180 msg-e3 round 19: 0.94 of
/// a cell's variance at 1 sd per cell, half side 3; 3.67 at half side 8).
#[test]
fn a_ramp_is_read_on_its_plane() {
    let (x0, x1) = RAMP;
    for s in [0.0f64, 0.5, 1.0, 1.5] {
        let (m, cell_sd) = ramp_mosaic(s);
        let step = s * cell_sd;
        let sites = bayer_sites(&rggb()).expect("Bayer");
        let pts = patches(&m, W, H, sites, WB);
        let model = NoiseModel::fit(&pts, W, H).expect("model");
        let cells = Cells::of(&m, W, H, sites, WB, &model, &pts).expect("cells");
        for (k, rho) in [fair(3), fair(8)] {
            let (lo, hi) = (x0 + rho + 1, x1 - rho - 1);
            let rows: Vec<usize> = (rho..cells.ch - rho).collect();
            let per_row: Vec<(f64, f64, f64)> = rows
                .par_iter()
                .map(|&cy| {
                    let q: Vec<([f32; 2], [u32; 2], [Read; 2])> = (lo..=hi).map(|cx| cells.filtered_counted(cy * cells.cw + cx, k, rho)).collect();
                    let f: Vec<f32> = q.iter().map(|q| q.0[0]).collect();
                    let d: Vec<f32> = (lo..=hi).map(|cx| cells.d[cy * cells.cw + cx][0]).collect();
                    let e = |v: &[f32]| v.windows(2).map(|w| f64::from(w[1] - w[0]).powi(2) - step * step).sum::<f64>() / (v.len() - 1) as f64;
                    (e(&f), e(&d), q.iter().filter(|q| q.2[0] != Read::Plane).count() as f64 / q.len() as f64)
                })
                .collect();
            let g = 2 * rho + 1;
            let blocks: Vec<(f64, f64)> = per_row
                .chunks(g)
                .filter(|c| c.len() == g)
                .map(|c| (c.iter().map(|p| p.0).sum::<f64>() / g as f64, c.iter().map(|p| p.1).sum::<f64>() / g as f64))
                .collect();
            let refuted = per_row.iter().map(|p| p.2).sum::<f64>() / per_row.len() as f64;
            let nb = blocks.len() as f64;
            let v = cell_sd * cell_sd;
            let mean = |f: &dyn Fn(&(f64, f64)) -> f64| blocks.iter().map(f).sum::<f64>() / nb;
            let (ef, ed) = (mean(&|b| b.0) / v, mean(&|b| b.1) / v);
            let se = |f: &dyn Fn(&(f64, f64)) -> f64, at: f64| (blocks.iter().map(|b| (f(b) / v - at).powi(2)).sum::<f64>() / (nb - 1.0) / nb).sqrt();
            let (se_f, se_d) = (se(&|b| b.0, ef), se(&|b| b.1, ed));
            println!(
                "slope {s} sd/cell, half side {rho} (k {k:.3}): excess step energy filtered {ef:.4} (se {se_f:.4}), measured {ed:.4} (se {se_d:.4}), in one cell's variance; refuted {refuted:.4} of the cells"
            );
            if s > 0.0 {
                assert!(ef.abs() <= se_d, "slope {s} sd/cell, half side {rho}: the filtered ramp holds {ef:.4} of a cell's variance more step energy than the ramp, the measurement's error {se_d:.4}");
            }
        }
    }
}

/// A step the plane cannot tell from a ramp one neighbour at a time is
/// kept to the colour edge test's limit wherever the constant keeps it: a
/// one-sided R − G edge of `s` cell sds (the module's noise, the edge at
/// column W/4) at every step the round-17 sweep read (3, 4, 5, 6, 8 and 12
/// sds) over the fair test of the windows the stage runs first (half sides
/// 1, 8 and 12: colour NR 1–20, 25 and 50, [`fair_test`]; half side 3, which
/// no strength runs, is the read's known limit, see the module, and stays in
/// [`a_ramp_is_read_on_its_plane`]). At each of the six columns
/// either side of the edge where the constant's reading on the same windows
/// ([`constant_reading`]) is within 0.15 of the step of its own side's level
/// (the limit of [`a_colour_edge_the_noise_cannot_make_is_kept`]), the
/// filtered mean over the rows is within it too. Where the constant itself is
/// over it (a step the noise could have made, no neighbour refusable: the
/// test's own regime) the column is printed as the regime's limit, not
/// asserted. Each column's error over the step is printed for both, with the
/// share of its cells whose plane the whiteness read refuted and the
/// standard error of the unfiltered column mean (blocks of `2ρ + 1` rows).
///
/// MUTATION: the whiteness read dropped (the plane everywhere): the column
/// beside a 5-sd edge reads 0.47 of the step at half side 8 (rel1180 msg-e3
/// round 18).
#[test]
fn a_step_is_read_as_the_constant_reads_it() {
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let level = 0.01;
    let sd = (WB[0] * WB[0] * (A * level / WB[0] + B) + (A * level + B) / 4.0).sqrt();
    let e = W / 4;
    for s in [3.0f64, 4.0, 5.0, 6.0, 8.0, 12.0] {
        let m = mosaic(level, s * sd);
        let p = patches(&m, W, H, sites, WB);
        let model = NoiseModel::fit(&p, W, H).expect("model");
        let c = Cells::of(&m, W, H, sites, WB, &model, &p).expect("cells");
        let side = |cols: std::ops::Range<usize>| cols.clone().map(|cx| (0..c.ch).map(|cy| f64::from(c.d[cy * c.cw + cx][0])).sum::<f64>() / c.ch as f64).sum::<f64>() / cols.len() as f64;
        let (lo, hi) = (side(0..e - 8), side(e + 8..W / 2));
        let step = s * sd;
        for (k, rho) in [fair(1), fair(8), fair(12)] {
            let g = 2 * rho + 1;
            let mut line = format!("step {s} sd, half side {rho} (k {k:.3}), err/step plane | constant (share read as the constant, se):");
            for cx in e - 3..e + 3 {
                let rows: Vec<(f64, f64, f64, f64)> = (0..c.ch)
                    .into_par_iter()
                    .map(|cy| {
                        let i = cy * c.cw + cx;
                        let q = c.filtered_counted(i, k, rho);
                        (f64::from(q.0[0]), f64::from(constant_reading(&c, i, k, rho)[0]), f64::from(c.d[i][0]), f64::from(u8::from(q.2[0] != Read::Plane)))
                    })
                    .collect();
                let n = rows.len() as f64;
                let (plane, constant) = (rows.iter().map(|r| r.0).sum::<f64>() / n, rows.iter().map(|r| r.1).sum::<f64>() / n);
                let refuted = rows.iter().map(|r| r.3).sum::<f64>() / n;
                let blocks: Vec<f64> = rows.chunks(g).filter(|b| b.len() == g).map(|b| b.iter().map(|r| r.2).sum::<f64>() / g as f64).collect();
                let nb = blocks.len() as f64;
                let bm = blocks.iter().sum::<f64>() / nb;
                let se = (blocks.iter().map(|b| (b - bm).powi(2)).sum::<f64>() / (nb - 1.0) / nb).sqrt();
                let want = if cx < e { lo } else { hi };
                let (got, base) = ((plane - want) / step, (constant - want) / step);
                let regime = base.abs() >= 0.15;
                line += &format!("  {cx}: {got:+.3} | {base:+.3} ({refuted:.3}, {:.3}){}", se / step, if regime { " <regime>" } else { "" });
                assert!(
                    regime || got.abs() < 0.15,
                    "step {s} sd, half side {rho}, column {cx} (edge {e}): the filter reads {got:+.4} of the step off its side, the constant {base:+.4} (the limit 0.15)"
                );
            }
            println!("{line}");
        }
    }
}

/// A flat window reads the constant's value: a plane contains the constant.
/// On a window of one value at the fair test, and on a window of noise at an
/// unbounded `k` (every sample accepted: the plane's value at the cell of a
/// full symmetric window is its mean, the centred places orthogonal to the
/// intercept), the plane's reading (its whiteness read at the fixture
/// frame's `k`) is the skipped mean's within f32 rounding, at half sides 3
/// and 8.
#[test]
fn a_flat_window_reads_the_constants_value() {
    for rho in [3usize, 8] {
        let side = 2 * rho + 1;
        let mut w = Window { centre: rho * side + rho, cols: side, ..Window::default() };
        for y in 0..side {
            for x in 0..side {
                w.place.push([x as f32 - rho as f32, y as f32 - rho as f32]);
                w.count.push(1.0);
            }
        }
        for (k, noisy) in [(fair(rho).0, false), (f64::INFINITY, true)] {
            for j in 0..2 {
                w.value[j] = (0..side * side).map(|i| (0.0123 + if noisy { 0.004 * normal(71 + j as u64, i as u64) } else { 0.0 }) as f32).collect();
            }
            let sigma = [0.004f64; 2];
            let (got, _, kept) = local_plane(&w, sigma, [0.0; 2], [k, fair_k((W / 2 * (H / 2)) as f64)], &mut Vec::new(), &mut Vec::new());
            for j in 0..2 {
                let (want, _) = skipped_mean(&w.value[j], w.value[j][w.centre], sigma[j], k, &mut Vec::new());
                let scale = w.value[j].iter().fold(0.0f32, |a, q| a.max(q.abs()));
                println!("half side {rho}, k {k:.3}, noisy {noisy}, difference {j}: plane {:.9e} (read {:?}), constant {want:.9e}", got[j], kept[j]);
                assert!((got[j] - want).abs() <= f32::EPSILON * scale, "half side {rho}, k {k}, noisy {noisy}: the plane reads {:e}, the constant {want:e}", got[j]);
            }
        }
    }
}

/// A model is read only where its own check can refuse it. A window of half
/// side 1 (colour NR 1–20) has at most 12 axis pairs a direction, under the
/// fixture frame's `k_F²` (20.8): the whiteness read has no reach, and every
/// cell reads the constant's value ([`constant_reading`]) bit for bit, on
/// each fixture the step sweep and the ramp test use: a flat grey, a ramp of
/// 1 sd per cell and a step of 5 sds ([`a_step_is_read_as_the_constant_reads_it`]).
///
/// MUTATION: the reach rule dropped: the plane is read at half side 1, and
/// on the flat fixture alone 144 361 of the 196 608 cells differ from the
/// constant's reading (rel1180 msg-e3 round 20; the accepted set of three by
/// three is lopsided wherever a neighbour is refused).
#[test]
fn a_window_out_of_the_reads_reach_reads_the_constant() {
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let level = 0.01;
    let sd = (WB[0] * WB[0] * (A * level / WB[0] + B) + (A * level + B) / 4.0).sqrt();
    let (k, rho) = fair(1);
    for (name, m) in [("flat", mosaic(level, 0.0)), ("ramp 1 sd/cell", ramp_mosaic(1.0).0), ("step 5 sd", mosaic(level, 5.0 * sd))] {
        let p = patches(&m, W, H, sites, WB);
        let model = NoiseModel::fit(&p, W, H).expect("model");
        let c = Cells::of(&m, W, H, sites, WB, &model, &p).expect("cells");
        let (differ, other): (usize, usize) = (0..c.cw * c.ch)
            .into_par_iter()
            .map(|i| {
                let (got, _, read) = c.filtered_counted(i, k, rho);
                let want = constant_reading(&c, i, k, rho);
                (usize::from(got.iter().zip(&want).any(|(a, b)| a.to_bits() != b.to_bits())), usize::from(read.iter().any(|r| *r != Read::NoReach)))
            })
            .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        println!("{name}: half side {rho} (k {k:.4}, k_F {:.4}): {differ} of {} cells differ from the constant's reading; {other} read in reach", c.frame_k, c.cw * c.ch);
        assert_eq!(differ, 0, "{name}: {differ} cells at half side {rho} are not the constant's reading");
    }
}

#[test]
fn a_pattern_that_is_not_a_two_by_two_bayer_is_left_alone() {
    let before = mosaic(0.01, 0.0);
    let mut after = before.clone();
    let xtrans = rawler::cfa::CFA::new("GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG");
    assert!(filter_mosaic(&mut after, W, H, &xtrans, WB, fair(3), None, 0, true).is_none());
    assert!(before.iter().zip(&after).all(|(a, b)| a.to_bits() == b.to_bits()));
}

/// The demosaic reads the sensor's own mosaic: a develop that demosaics asks
/// the stage not to write (`write` false), so its mosaic comes back bit for
/// bit as it went in — the demosaic's green at a red or blue site is built
/// from that site's own sample, and a filtered sample written back would move
/// the pixel's luma — while the cell chroma the rebuild writes the colour from
/// is the one a writing stage returns (the write is an output, not an input
/// of the pass).
///
/// MUTATION: the stage writes whatever `write` says: the mosaic then carries
/// the filtered samples.
#[test]
fn a_demosaicing_develop_reads_the_sensors_own_mosaic() {
    let before = mosaic(0.01, 0.0);
    let (mut kept, mut written) = (before.clone(), before.clone());
    let (_, c0) = filter_mosaic(&mut kept, W, H, &rggb(), WB, fair(3), None, 0, false).expect("a model");
    let (_, c1) = filter_mosaic(&mut written, W, H, &rggb(), WB, fair(3), None, 0, true).expect("a model");
    assert!(kept.iter().zip(&before).all(|(a, b)| a.to_bits() == b.to_bits()), "the mosaic a demosaic reads was written");
    assert!(written.iter().zip(&before).any(|(a, b)| a != b), "the writing stage left its mosaic as it was");
    for j in 0..2 {
        assert!(c0.planes[j].iter().zip(&c1.planes[j]).all(|(a, b)| a.to_bits() == b.to_bits()), "colour difference {j}'s cell chroma");
        assert!(c0.slopes[j].iter().zip(&c1.slopes[j]).all(|(a, b)| a.to_bits() == b.to_bits()), "colour difference {j}'s slopes");
    }
}

/// The luma that is the green: the rebuild then keeps each pixel's green bit
/// for bit, and the chroma read, the slope and the star tests read the
/// colour differences on the demosaic's own greens (the luma kept is the next
/// test's).
const GREEN_LUMA: [f32; 3] = [0.0, 1.0, 0.0];

/// Colour noise has no content at a pixel's scale; the pixel's light has —
/// the stage decides the colour, the demosaic's luma stays. A demosaic's
/// camera-native frame (signed dark noise, a lit sky, and a row of pixels at
/// the white level), a cell chroma that moves every pixel's red and blue, and
/// the luma of a working space (`0.25·qR + 0.70·qG + 0.05·qB` on the
/// white-balanced channels `q`): after the rebuild every pixel's luma is the
/// demosaic's own, to the rounding of its three channels to `f32` (the luma is
/// a sum of three products; it cannot be bit-identical through a change of
/// its terms, only to their rounding), its colour differences `wR·R − G`,
/// `wB·B − G` the cell chroma, except on the pixels the rebuild reports
/// brought into range — and those are exactly the pixels whose rebuilt
/// colour would have put a channel over the white level: at most 1 with their
/// luma kept where a grey of that luma fits, the demosaic's own pixel where
/// none does (the white row: its luma is past any grey's).
///
/// MUTATION: the move along the grey dropped (the pixel keeps its green and
/// takes `R = (G + c)/wR`, round 11's form): the luma moves by the red's and
/// the blue's share of the change.
#[test]
fn the_rebuild_keeps_the_demosaics_luma() {
    let (cw, ch, w, h) = (34usize, 26usize, 64usize, 48usize);
    let (wr, wb) = (WB[0] as f32, WB[2] as f32);
    let luma = [0.25 * wr, 0.70, 0.05 * wb];
    let l = luma.map(f64::from);
    let planes = [
        (0..cw * ch).map(|i| (0.02 + 0.01 * normal(31, i as u64)) as f32).collect::<Vec<f32>>(),
        (0..cw * ch).map(|i| (-0.015 + 0.01 * normal(37, i as u64)) as f32).collect::<Vec<f32>>(),
    ];
    let flat = || vec![0.0f32; cw * ch];
    let chroma = CellChroma { cw, ch, wb: [wr, wb], origin: [(0, 0), (1, 1)], planes, green: flat(), slopes: [flat(), flat()] };
    // Rows 0..16 the night's signed noise about 0, rows 16..47 a lit sky,
    // row 47 at the white level (the green a hair under it).
    let before: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let y = i / w;
            let level = if y < 16 { 0.0 } else if y < h - 1 { 0.3 } else { 0.999 };
            let sd = if y < 16 { 0.004 } else if y < h - 1 { 0.02 } else { 0.0 };
            [0, 1, 2].map(|c| {
                let base = if y == h - 1 { [1.0 / f64::from(wr) + 0.55, level, 1.0 / f64::from(wb) + 0.35][c] } else { level / [WB[0], 1.0, WB[2]][c] };
                (base + sd * normal(41 + c as u64, i as u64)) as f32
            })
        })
        .collect();
    let mut after = before.clone();
    let pulled = chroma.rebuild(&mut after, w, 0, 0, luma);
    let y_of = |p: &[f32; 3]| (0..3).map(|c| l[c] * f64::from(p[c])).sum::<f64>();
    let rounding = |p: &[f32; 3]| (0..3).map(|c| (l[c] * f64::from(p[c])).abs()).sum::<f64>() * f64::from(f32::EPSILON);
    let mut over = 0;
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        let (x, y) = (i % w, i / w);
        let c = [0usize, 1].map(|j| {
            let (oy, ox) = [(0usize, 0usize), (1, 1)][j];
            crate::render::bilinear_plane(&chroma.planes[j], cw, ch, (x as f32 - ox as f32) / 2.0, (y as f32 - oy as f32) / 2.0)
        });
        let decided = [f64::from((b[1] + c[0]) / wr), f64::from(b[1]), f64::from((b[1] + c[1]) / wb)];
        let lift = (y_of(b) - (0..3).map(|k| l[k] * decided[k]).sum::<f64>()) / (l[0] / f64::from(wr) + l[1] + l[2] / f64::from(wb));
        let goes_over = [lift / f64::from(wr), lift, lift / f64::from(wb)].iter().zip(&decided).any(|(g, d)| d + g > 1.0);
        let grey_fits = [1.0 / f64::from(wr), 1.0, 1.0 / f64::from(wb)]
            .iter()
            .all(|g| g * y_of(b) / (l[0] / f64::from(wr) + l[1] + l[2] / f64::from(wb)) <= 1.0);
        if goes_over {
            over += 1;
            if grey_fits {
                assert!(a.iter().all(|&v| v <= 1.0), "pixel ({x}, {y}) brought into range has a channel over the white level: {a:?}");
            } else {
                assert_eq!(a, b, "pixel ({x}, {y}): no grey of its luma fits, so it stays the demosaic's");
            }
        } else {
            let got = [f64::from(wr) * f64::from(a[0]) - f64::from(a[1]), f64::from(wb) * f64::from(a[2]) - f64::from(a[1])];
            for j in 0..2 {
                assert!((got[j] - f64::from(c[j])).abs() < 1e-5, "pixel ({x}, {y}): colour difference {j} {:.6}, the cell chroma {:.6}", got[j], c[j]);
            }
        }
        let dy = (y_of(a) - y_of(b)).abs();
        assert!(dy <= 2.0 * rounding(a), "pixel ({x}, {y}): luma {:.9} after, the demosaic's {:.9} (rounding {:.2e})", y_of(a), y_of(b), rounding(a));
    }
    assert_eq!(pulled, over, "the pixels reported brought into range are the ones whose colour went over the white level");
    assert!(over > 0 && over <= w, "the white row is the one that goes over: {over}");
}

/// The sensor measures colour once per cell; pixel-scale chroma is the
/// demosaic's, so the stage's cell chroma is the pixel's: after the rebuild
/// every pixel's green is its own (bit for bit) and its chroma `wR·R − G`,
/// `wB·B − G` is the cell chroma read at the pixel in its own site grid. A
/// field linear in that grid is read exactly by the bilinear tap, so a ramp
/// plane must come back as the ramp at the pixel's position in the grid,
/// `(x + ox − dx)/2` (`dx` the colour site's column in its cell: 0 for an
/// RGGB red, 1 for its blue; the same down the rows), whatever
/// noise the demosaic left in the pixel's own red and blue — on cells whose
/// slope against green is 0 (a flat cell: the slope is the next test's).
///
/// MUTATION: the pixel takes its own cell's chroma (nearest cell, no
/// bilinear read): the chroma is then a step at every cell edge.
#[test]
fn the_pixels_chroma_is_the_cells_chroma_read_at_the_pixel() {
    let (cw, ch, ox, oy, w, h) = (40usize, 30usize, 6usize, 4usize, 64usize, 44usize);
    let ramp = |j: usize, u: f64, v: f64| if j == 0 { 0.01 * u - 0.003 * v } else { -0.002 * u + 0.008 * v + 0.05 };
    let planes =
        [0, 1].map(|j| (0..cw * ch).map(|i| ramp(j, (i % cw) as f64, (i / cw) as f64) as f32).collect::<Vec<f32>>());
    let flat = || vec![0.0f32; cw * ch];
    let origin = [(0, 0), (1, 1)];
    let chroma = CellChroma { cw, ch, wb: [WB[0] as f32, WB[2] as f32], origin, planes, green: flat(), slopes: [flat(), flat()] };
    let before: Vec<[f32; 3]> =
        (0..w * h).map(|i| [0, 1, 2].map(|c| (0.2 + 0.05 * normal(11 + c as u64, i as u64)) as f32)).collect();
    let mut after = before.clone();
    chroma.rebuild(&mut after, w, ox, oy, GREEN_LUMA);
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        let (x, y) = (i % w, i / w);
        assert_eq!(b[1].to_bits(), a[1].to_bits(), "the green of pixel ({x}, {y}) moved");
        for (j, (wc, c)) in [(WB[0], a[0]), (WB[2], a[2])].into_iter().enumerate() {
            let (dy, dx) = origin[j];
            let (u, v) = (((x + ox) as f64 - dx as f64) / 2.0, ((y + oy) as f64 - dy as f64) / 2.0);
            let got = wc * f64::from(c) - f64::from(a[1]);
            let want = ramp(j, u, v);
            assert!(
                (got - want).abs() < 2e-5,
                "pixel ({x}, {y}), colour difference {j}: chroma {got:.6}, the cell chroma at ({u}, {v}) cells is {want:.6}"
            );
        }
    }
}

/// A pixel's colour follows its light: the cell's colour, carried along the
/// cell's own slope against green, believed only where the green varies more
/// than its noise. A night sky of one colour (`wR·R = 1.4·G`, `wB·B = 0.7·G`
/// everywhere, sky and stars alike: only the light varies) with stars of one
/// cell's sd, under the model's noise; the light is the star's at each
/// cell's centre, the same on the cell's four sites. Over the scene's own
/// cell chroma (`(1.4 − 1)·G` and `(0.7 − 1)·G` on each cell's measured
/// green: the measurement of a colour difference, at its site against four
/// greens of which two lie in the neighbouring cells, is the star-centroid
/// test's, and on light held constant per cell those greens see another
/// cell's light), the rebuild of a demosaic whose greens are that light must
/// give each star's
/// core pixels (green at least 0.25, over ten times the sky) the scene's
/// colour ratio — the mean over the stars of the core's `wR·R / G` within 3
/// standard errors (its spread over the stars) of 1.4, and of `wB·B / G` of
/// 0.7. The bilinear read blends a bright cell with its darker neighbours, so
/// one cell chroma per pixel without the slope gives the core a ratio nearer
/// 1 than the star's.
///
/// MUTATION: the rebuild without the slope (`c` alone, round 10's additive
/// form): the core's ratio is then the blend's.
#[test]
fn a_pixels_colour_follows_its_light_where_the_green_varies_more_than_its_noise() {
    const RATIO: [f64; 2] = [1.4, 0.7];
    const SKY: f64 = 0.02;
    const PEAK: f64 = 0.6;
    const SD: f64 = 2.0;
    const STEP: usize = 64;
    // Star centres on a grid, each shifted inside its cell by its own offset.
    let stars: Vec<(f64, f64)> = (1..H / STEP - 1)
        .flat_map(|j| (1..W / STEP - 1).map(move |i| (i, j)))
        .map(|(i, j)| {
            let n = (j * W + i) as u64;
            ((i * STEP) as f64 + 2.0 * (normal(21, n).abs() % 1.0), (j * STEP) as f64 + 2.0 * (normal(22, n).abs() % 1.0))
        })
        .collect();
    // The scene's light at pixel (x, y): the sky and the nearest star, at
    // the centre of the pixel's cell.
    let light = |x: usize, y: usize| {
        let (x, y) = (x - x % 2, y - y % 2);
        let (i, j) = ((x + STEP / 2) / STEP, (y + STEP / 2) / STEP);
        let star = (i >= 1 && j >= 1 && i < W / STEP - 1 && j < H / STEP - 1).then(|| stars[(j - 1) * (W / STEP - 2) + i - 1]);
        SKY + star.map_or(0.0, |(sx, sy)| {
            let r2 = (x as f64 + 1.0 - sx).powi(2) + (y as f64 + 1.0 - sy).powi(2);
            PEAK * (-r2 / (2.0 * SD * SD)).exp()
        })
    };
    let mut m = vec![0.0f32; W * H];
    m.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let g = light(x, y);
            let s = match (y % 2, x % 2) {
                (0, 0) => RATIO[0] * g / WB[0],
                (1, 1) => RATIO[1] * g / WB[2],
                _ => g,
            };
            *v = (s + (A * s + B).sqrt() * normal(23, (y * W + x) as u64)) as f32;
        }
    });
    // The stage's own model and cells, the scene's chroma on the cells'
    // greens: the slope and the rebuild are under test here, the measurement,
    // the filter and its k the other tests'.
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let pts = patches(&m, W, H, sites, WB);
    let model = NoiseModel::fit(&pts, W, H).expect("a Bayer mosaic with noise fits a model");
    let cells = Cells::of(&m, W, H, sites, WB, &model, &pts).expect("cells");
    let scene: Vec<[f32; 2]> = cells.d.iter().map(|d| RATIO.map(|r| ((r - 1.0) * f64::from(d[2])) as f32)).collect();
    let chroma = cells.chroma(&scene, &model, WB, 3);
    let mut rgb: Vec<[f32; 3]> = (0..W * H).map(|i| [0.0, light(i % W, i / W) as f32, 0.0]).collect();
    chroma.rebuild(&mut rgb, W, 0, 0, GREEN_LUMA);
    for (c, wc) in [(0usize, WB[0]), (2, WB[2])] {
        let want = RATIO[c / 2];
        let per_star: Vec<f64> = stars
            .iter()
            .map(|&(sx, sy)| {
                let (x0, y0) = (sx as usize - 3, sy as usize - 3);
                let core: Vec<f64> = (y0..y0 + 7)
                    .flat_map(|y| (x0..x0 + 7).map(move |x| (x, y)))
                    .filter(|&(x, y)| light(x, y) >= 0.25)
                    .map(|(x, y)| wc * f64::from(rgb[y * W + x][c]) / f64::from(rgb[y * W + x][1]))
                    .collect();
                core.iter().sum::<f64>() / core.len() as f64
            })
            .collect();
        let n = per_star.len() as f64;
        let mean = per_star.iter().sum::<f64>() / n;
        let se = (per_star.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0) / n).sqrt();
        assert!(
            (mean - want).abs() <= 3.0 * se,
            "channel {c}: the stars' core pixels have the colour ratio {mean:.4}, the scene's is {want} (3 se {:.4}, {} stars)",
            3.0 * se,
            per_star.len()
        );
    }
}

/// A colour difference is measured at one place: the colour site, against
/// the green the demosaic reads there. A night sky of one colour
/// (`wR·R = 1.4·G`, `wB·B = 0.7·G` everywhere) with sharp stars (sd one
/// pixel), each site seeing the light at its own position, under the
/// model's noise; rebuilt over the cells' chroma as measured on a demosaic
/// whose greens are the light. A star's red and blue must sit where its
/// light sits: per star the centroid of `wR·R − sky` and of `wB·B − sky`
/// over a 9-pixel square, against the green's, and over the stars the mean
/// displacement on each axis within 3 standard errors (its spread over the
/// stars) of 0. Measured against the cell's mean green instead, the red's
/// content sits on its site, half a pixel up-left of the cell's centre where
/// the plane is read, so the red lands down-right and the blue up-left.
///
/// MUTATION: each colour difference measured against the cell's mean green
/// and read at the cell's centre (the form before rel1180 round 11).
#[test]
fn a_stars_colour_sits_where_its_light_sits() {
    const RATIO: [f64; 2] = [1.4, 0.7];
    const SKY: f64 = 0.02;
    const PEAK: f64 = 0.6;
    const SD: f64 = 1.0;
    const STEP: usize = 64;
    const HALF: usize = 4;
    let stars: Vec<(f64, f64)> = (1..H / STEP - 1)
        .flat_map(|j| (1..W / STEP - 1).map(move |i| (i, j)))
        .map(|(i, j)| {
            let n = (j * W + i) as u64;
            ((i * STEP) as f64 + 2.0 * (normal(31, n).abs() % 1.0), (j * STEP) as f64 + 2.0 * (normal(32, n).abs() % 1.0))
        })
        .collect();
    // The scene's light at pixel (x, y)'s centre: the sky and the nearest star.
    let light = |x: usize, y: usize| {
        let (i, j) = ((x + STEP / 2) / STEP, (y + STEP / 2) / STEP);
        let star = (i >= 1 && j >= 1 && i < W / STEP - 1 && j < H / STEP - 1).then(|| stars[(j - 1) * (W / STEP - 2) + i - 1]);
        SKY + star.map_or(0.0, |(sx, sy)| {
            let r2 = (x as f64 - sx).powi(2) + (y as f64 - sy).powi(2);
            PEAK * (-r2 / (2.0 * SD * SD)).exp()
        })
    };
    let mut m = vec![0.0f32; W * H];
    m.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let g = light(x, y);
            let s = match (y % 2, x % 2) {
                (0, 0) => RATIO[0] * g / WB[0],
                (1, 1) => RATIO[1] * g / WB[2],
                _ => g,
            };
            *v = (s + (A * s + B).sqrt() * normal(33, (y * W + x) as u64)) as f32;
        }
    });
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let pts = patches(&m, W, H, sites, WB);
    let model = NoiseModel::fit(&pts, W, H).expect("a Bayer mosaic with noise fits a model");
    let cells = Cells::of(&m, W, H, sites, WB, &model, &pts).expect("cells");
    let measured: Vec<[f32; 2]> = cells.d.iter().map(|d| [d[0], d[1]]).collect();
    let chroma = cells.chroma(&measured, &model, WB, 3);
    let mut rgb: Vec<[f32; 3]> = (0..W * H).map(|i| [0.0, light(i % W, i / W) as f32, 0.0]).collect();
    chroma.rebuild(&mut rgb, W, 0, 0, GREEN_LUMA);
    // Per star, the centroid of `value − sky` over the square about it.
    let centroid = |sx: f64, sy: f64, value: &dyn Fn(usize) -> f64, sky: f64| {
        let (x0, y0) = (sx.round() as usize - HALF, sy.round() as usize - HALF);
        let (mut mx, mut my, mut mass) = (0.0f64, 0.0f64, 0.0f64);
        for y in y0..=y0 + 2 * HALF {
            for x in x0..=x0 + 2 * HALF {
                let q = value(y * W + x) - sky;
                mx += q * x as f64;
                my += q * y as f64;
                mass += q;
            }
        }
        (mx / mass, my / mass)
    };
    for (c, wc) in [(0usize, WB[0]), (2, WB[2])] {
        let sky = RATIO[c / 2] * SKY;
        let shifts: Vec<(f64, f64)> = stars
            .iter()
            .map(|&(sx, sy)| {
                let (gx, gy) = centroid(sx, sy, &|i| f64::from(rgb[i][1]), SKY);
                let (cx, cy) = centroid(sx, sy, &|i| wc * f64::from(rgb[i][c]), sky);
                (cx - gx, cy - gy)
            })
            .collect();
        let n = shifts.len() as f64;
        for (axis, pick) in [("x", 0usize), ("y", 1)] {
            let v: Vec<f64> = shifts.iter().map(|s| if pick == 0 { s.0 } else { s.1 }).collect();
            let mean = v.iter().sum::<f64>() / n;
            let se = (v.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (n - 1.0) / n).sqrt();
            assert!(
                mean.abs() <= 3.0 * se,
                "channel {c}: the stars' colour sits {mean:+.4} px off their light along {axis} (3 se {:.4}, {} stars)",
                3.0 * se,
                shifts.len()
            );
        }
    }
}

/// The noise model is a property of the exposure, and a model fitted on a
/// denoised mosaic is not a measurement: handed the exposure's reading (read
/// once off the sensor's own mosaic, `read_source`), the stage runs at that
/// model whatever mosaic it is given — here the same frame with
/// each site averaged with its eight same-colour neighbours (a denoiser's
/// output: a ninth of the sensor's noise variance, not the sensor's law), on
/// which a fit of its own would read another model.
///
/// MUTATION: the stage fits its own model on the mosaic it is given.
#[test]
fn the_stage_runs_at_the_exposures_reading_not_at_a_fit_of_the_mosaic_it_is_given() {
    let sensor = mosaic(0.01, 0.0);
    let reading = read_source(&sensor, W, H, &rggb(), WB).expect("the sensor's mosaic has a model");
    let mut denoised: Vec<f32> = (0..W * H)
        .map(|i| {
            let (x, y) = ((i % W) as isize, (i / W) as isize);
            let mut s = 0.0f64;
            for (dy, dx) in (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (2 * dy, 2 * dx))) {
                let (xx, yy) = ((x + dx).clamp(x % 2, W as isize - 2 + x % 2), (y + dy).clamp(y % 2, H as isize - 2 + y % 2));
                s += f64::from(sensor[yy as usize * W + xx as usize]);
            }
            (s / 9.0) as f32
        })
        .collect();
    let refit = read_source(&denoised, W, H, &rggb(), WB).expect("a model");
    assert_ne!(refit.model, reading.model, "the denoised mosaic must read another model for this test to say anything");
    let (ran, _) = filter_mosaic(&mut denoised, W, H, &rggb(), WB, fair(3), Some(&reading), 0, true).expect("the stage runs");
    assert_eq!(ran, reading, "the stage ran at {ran:?}, the exposure's reading is {reading:?}");
}

/// A preview reads the stage's pass at its own scale: white noise averages
/// the same at any scale, so a pyramid level's cells are their blocks' means
/// and their sd is the model's `σ/2^level` — on a flat noisy frame the
/// level's colour differences spread as that sd says (the mean of their
/// squared deviation over the stated variance within 3 standard errors of
/// 1), at levels 1 and 2. The level a preview reads is the largest whose cell
/// is no larger than one of its pixels: at a long edge of 1280 a 9600-wide
/// sensor (7.5 sensor pixels a preview pixel) reads level 1 (cells of 4
/// pixels), a full-size develop level 0.
///
/// MUTATION: the level's sd left at the base's `σ` (not divided).
#[test]
fn a_preview_reads_the_pass_at_the_level_whose_cells_match_its_pixels() {
    assert_eq!(preview_level(9600, 6376, Some(1280)), 1);
    assert_eq!(preview_level(9600, 6376, None), 0);
    assert_eq!(preview_level(9600, 6376, Some(4800)), 0);
    let m = mosaic(0.02, 0.0);
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let pts = patches(&m, W, H, sites, WB);
    let model = NoiseModel::fit(&pts, W, H).expect("a model");
    let cells = Cells::of(&m, W, H, sites, WB, &model, &pts).expect("cells");
    // The cells' squared deviation over the variance their level states:
    // its mean and standard error, per colour difference.
    let spread = |l: &Cells, j: usize| {
        let n = (l.cw * l.ch) as f64;
        let mean = l.d.iter().map(|q| f64::from(q[j])).sum::<f64>() / n;
        let z: Vec<f64> = (0..l.cw * l.ch)
            .map(|i| {
                let (cx, cy) = (i % l.cw, i / l.cw);
                let sd = f64::from(l.sd[(cy / PATCH).min(l.ph - 1) * l.pw + (cx / PATCH).min(l.pw - 1)][j]) / 2f64.sqrt();
                (f64::from(l.d[i][j]) - mean).powi(2) / (sd * sd)
            })
            .collect();
        let zm = z.iter().sum::<f64>() / n;
        (zm, (z.iter().map(|v| (v - zm).powi(2)).sum::<f64>() / (n - 1.0) / n).sqrt())
    };
    for level in [1u32, 2] {
        let l = cells.level(level);
        for j in 0..2 {
            // Against the base's own reading of the same model, so the model's
            // own error on the colour differences cancels.
            let ((z0, s0), (z, s)) = (spread(&cells, j), spread(&l, j));
            let tol = 3.0 * ((s / z0).powi(2) + (z * s0 / (z0 * z0)).powi(2)).sqrt();
            assert!(
                (z / z0 - 1.0).abs() <= tol,
                "level {level}, colour difference {j}: the cells spread {:.4} times the variance the level states, relative to the base (3 se {tol:.4})",
                z / z0
            );
        }
    }
}

/// The rungs [`edge_energy_ladder`] reads the removed edge energy on: `2^(j/2)`,
/// `j = 0..LADDER` (1 to 64, half an octave apart — a resolution: the
/// crossing is interpolated between rungs).
const LADDER: usize = 13;

/// The textured patches [`edge_energy_ladder`] reads (the flat rule's complement): a
/// reading summed over the frame's patches, as the noise model's fit is, read
/// at the fit's own subsample rule (`noise_model::fit_stride`: the uniform
/// stride that keeps at least `FIT_PATCHES`, every one on a frame with
/// fewer). The reading is a reading, not the filter: it costs the filter's
/// window on the patches it reads, so it reads no more of them than the
/// model's own resolution needs.
fn reading_patches(model: &NoiseModel, pts: &[Patch], wb: [f64; 3]) -> Vec<usize> {
    let textured: Vec<usize> = (0..pts.len()).filter(|&i| pts[i].usable && !model.is_flat(&pts[i], wb)).collect();
    let stride = super::super::noise_model::fit_stride(textured.len());
    textured.into_iter().step_by(stride).collect()
}

/// The edge-energy ladder (the stage's `k` law before rel1180 round 14, kept
/// for the record as a probe): per rung, the edge energy the filter removes
/// from the photo's TEXTURED patches over the noise model's share of that
/// energy — a reading of the filter's sharpening and its removal together,
/// not of the noise removed — and the first rung's crossing of one, linear
/// between rungs (`f64::INFINITY` when there is none).
fn edge_energy_ladder(cells: &Cells, model: &NoiseModel, pts: &[Patch], wb: [f64; 3], rho: usize) -> (f64, Vec<(f64, f64)>) {
    let textured = reading_patches(model, pts, wb);
    if textured.is_empty() {
        return (f64::INFINITY, Vec::new());
    }
    let (cw, ch, pw) = (cells.cw, cells.ch, cells.pw);
    let origin = |p: usize| ((p % pw) * PATCH, (p / pw) * PATCH);
    // A patch's forward differences (to the right and below, inside the
    // frame): each pairs two neighbouring cells.
    let neighbours = move |cx: usize, cy: usize| {
        [(cx + 1 < cw).then(|| cy * cw + cx + 1), (cy + 1 < ch).then(|| (cy + 1) * cw + cx)].into_iter().flatten()
    };
    // Each cell's value is read once per patch (the patch and the row and
    // column its forward differences reach), not once per difference it is in.
    let energy = |value: &(dyn Fn(usize) -> [f32; 2] + Sync)| -> f64 {
        textured
            .par_iter()
            .map(|&p| {
                let (x0, y0) = origin(p);
                let (x1, y1) = ((x0 + PATCH + 1).min(cw), (y0 + PATCH + 1).min(ch));
                let bw = x1 - x0;
                let at: Vec<[f32; 2]> = (y0..y1).flat_map(|cy| (x0..x1).map(move |cx| cy * cw + cx)).map(value).collect();
                let mut e = 0.0f64;
                for cy in y0..y0 + PATCH {
                    for cx in x0..x0 + PATCH {
                        let c = at[(cy - y0) * bw + cx - x0];
                        for n in neighbours(cx, cy) {
                            let q = at[(n / cw - y0) * bw + n % cw - x0];
                            e += f64::from(q[0] - c[0]).powi(2) + f64::from(q[1] - c[1]).powi(2);
                        }
                    }
                }
                e
            })
            .sum()
    };
    // The noise's share: each difference carries both cells' variance less
    // the green they share, at the patch's level (`2·(v − c)` per difference
    // and colour difference, `2v = σ_d²`).
    let share: f64 = textured
        .iter()
        .map(|&p| {
            let [sr, sb] = cells.sd[p].map(f64::from);
            let c = f64::from(cells.cov[p]);
            let (x0, y0) = origin(p);
            let diffs: usize =
                (y0..y0 + PATCH).flat_map(|cy| (x0..x0 + PATCH).map(move |cx| neighbours(cx, cy).count())).sum();
            (sr * sr + sb * sb - 4.0 * c) * diffs as f64
        })
        .sum();
    let e_in = energy(&|i| [cells.d[i][0], cells.d[i][1]]);
    let mut read: Vec<(f64, f64)> = Vec::with_capacity(LADDER);
    for j in 0..LADDER {
        let k = 2f64.powf(j as f64 / 2.0);
        let removed = (e_in - energy(&|i| cells.filtered(i, k, rho))) / share;
        read.push((k, removed));
        if removed >= 1.0 {
            let k = match read.len() {
                1 => k,
                n => {
                    let ((k0, r0), (k1, r1)) = (read[n - 2], read[n - 1]);
                    k0 + (1.0 - r0) * (k1 - k0) / (r1 - r0)
                }
            };
            return (k, read);
        }
    }
    (f64::INFINITY, read)
}

/// The edge-energy ladder on a real RAW (`E3_PROBE=<raw path>`): the model,
/// the textured share and, per rung, the removed edge energy over the noise's
/// share (a lane probe, rel1180 msg-e3). Run by hand:
/// `cargo test --release --lib edge_energy_ladder_reading -- --ignored --nocapture`.
#[test]
#[ignore]
fn edge_energy_ladder_reading() {
    use rawler::rawimage::RawPhotometricInterpretation as Photo;
    let Ok(path) = std::env::var("E3_PROBE") else { return };
    let raw = rawler::decode_file(&path).expect("decode");
    let Photo::Cfa(config) = &raw.photometric else { panic!("not a CFA") };
    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();
    let width = raw.width;
    let mut v = raw.data.as_f32().into_owned();
    v.par_chunks_exact_mut(width * 2).for_each(|lines| {
        let (l0, l1) = lines.split_at_mut(width);
        l0.chunks_exact_mut(2).zip(l1.chunks_exact_mut(2)).for_each(|(a, b)| {
            a[0] = (a[0] - black[0]) / (white[0] - black[0]);
            a[1] = (a[1] - black[1]) / (white[1] - black[1]);
            b[0] = (b[0] - black[2]) / (white[2] - black[2]);
            b[1] = (b[1] - black[3]) / (white[3] - black[3]);
        });
    });
    let wb = [raw.wb_coeffs[0], raw.wb_coeffs[1], raw.wb_coeffs[2]].map(f64::from);
    let wb = [wb[0] / wb[1], 1.0, wb[2] / wb[1]];
    let sites = bayer_sites(&config.cfa).expect("Bayer");
    let t0 = std::time::Instant::now();
    let pts = patches(&v, width, raw.height, sites, wb);
    let model = NoiseModel::fit(&pts, width, raw.height).expect("model");
    let t1 = t0.elapsed();
    let cells = Cells::of(&v, width, raw.height, sites, wb, &model, &pts).expect("cells");
    let rho = plain_window(DEFAULT_AMOUNT);
    let (k, read) = edge_energy_ladder(&cells, &model, &pts, wb, rho);
    let t2 = t0.elapsed();
    let usable = pts.iter().filter(|p| p.usable).count();
    let textured = pts.iter().filter(|p| p.usable && !model.is_flat(p, wb)).count();
    println!(
        "{path}: {}x{}, model a {:.4e} b {:.4e} c2 {:+.4} c4 {:+.4} band {:.4}; {usable} usable patches of {}x{} cells, {textured} textured ({:.1} %); rho {rho} cells",
        width, raw.height, model.a, model.b, model.c2, model.c4, model.band, PATCH, PATCH, 100.0 * textured as f64 / usable as f64
    );
    for (k, r) in &read {
        println!("  k {k:7.3}: removed / noise share {r:.4}");
    }
    println!(
        "  -> ladder crossing {k:.3} over {} textured patches read; model fit {:.2} s, ladder {:.2} s",
        reading_patches(&model, &pts, wb).len(),
        t1.as_secs_f64(),
        (t2 - t1).as_secs_f64()
    );
    // The cost of the pass at the stage's test at the default strength, and
    // of the slopes the pixels are rebuilt on; the readings below at that test.
    let (k, rho) = fair_test(DEFAULT_AMOUNT).expect("on");
    let rho_k = rho;
    let t3 = std::time::Instant::now();
    let f: Vec<[f32; 2]> = (0..cells.cw * cells.ch).into_par_iter().map(|i| cells.filtered(i, k, rho_k)).collect();
    let t4 = t3.elapsed();
    let chroma = cells.chroma(&f, &model, wb, rho_k);
    let t5 = t3.elapsed();
    let rms = |j: usize| (chroma.slopes[j].iter().map(|a| f64::from(*a).powi(2)).sum::<f64>() / chroma.slopes[j].len() as f64).sqrt();
    println!(
        "  pass at k {k:.3} over half side {rho_k}: {:.2} s; slopes {:.2} s (rms slope R-G {:.4}, B-G {:.4})",
        t4.as_secs_f64(),
        (t5 - t4).as_secs_f64(),
        rms(0),
        rms(1)
    );
    drop((f, chroma));
    // Round 6: does the filter move the LINEAR mean of a flat patch's colour
    // differences? Per flat patch, the mean over its cells of (filtered - input),
    // in units of one cell's model sd; per level decile of the flat patches.
    for kk in [k, f64::INFINITY] {
        let f: Vec<[f32; 2]> = (0..cells.cw * cells.ch).into_par_iter().map(|i| cells.filtered(i, kk, rho)).collect();
        let mut rows: Vec<(f64, [f64; 2])> = Vec::new();
        for (pi, p) in pts.iter().enumerate() {
            if !(p.usable && model.is_flat(p, wb)) {
                continue;
            }
            let (px, py) = (pi % cells.pw, pi / cells.pw);
            let sd = cells.sd[pi];
            let mut acc = [0.0f64; 2];
            for cy in py * PATCH..(py + 1) * PATCH {
                for cx in px * PATCH..(px + 1) * PATCH {
                    let i = cy * cells.cw + cx;
                    for j in 0..2 {
                        acc[j] += f64::from(f[i][j] - cells.d[i][j]);
                    }
                }
            }
            let n = (PATCH * PATCH) as f64;
            rows.push((p.m, [acc[0] / n / (f64::from(sd[0]) / 2f64.sqrt()), acc[1] / n / (f64::from(sd[1]) / 2f64.sqrt())]));
        }
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        let per = rows.len() / 10;
        print!("  linear mean shift of flat patches, k {kk:.3} ({} patches), (filtered - input) / cell sd per level decile: R-G | B-G", rows.len());
        for q in 0..10 {
            let part = &rows[q * per..(q + 1) * per];
            let mean = |j: usize| part.iter().map(|r| r.1[j]).sum::<f64>() / part.len() as f64;
            println!();
            print!("    D{q} level {:.2e}: {:+.4} | {:+.4}", part[part.len() / 2].0, mean(0), mean(1));
        }
        println!();
    }
    // Round 8 (F1): how much of a cell's own noise the filter keeps. Per flat
    // patch and colour difference, about the patch's input mean: the residual
    // variance over the input variance, the slope of the filtered value on
    // the cell's own sample (the share of its own noise it keeps; the window
    // mean's is 1/N whatever the neighbours do) and the mean fraction of the
    // window that passes the test; per level decile, at the photo's k and
    // unbounded. 1/N for the full window is printed beside them.
    let n_win = ((2 * rho + 1) * (2 * rho + 1)) as f64;
    for kk in [k, f64::INFINITY] {
        let f: Vec<[f32; 2]> = (0..cells.cw * cells.ch).into_par_iter().map(|i| cells.filtered(i, kk, rho)).collect();
        let included = |i: usize| -> [f64; 2] {
            let (cw, ch) = (cells.cw, cells.ch);
            let (cx, cy) = (i % cw, i / cw);
            let all = ((cy + rho + 1).min(ch) - cy.saturating_sub(rho)) * ((cx + rho + 1).min(cw) - cx.saturating_sub(rho));
            cells.filtered_counted(i, kk, rho).1.map(|n| f64::from(n) / all as f64)
        };
        // (level, [var ratio, own slope, inclusion] x 2)
        let rows: Vec<(f64, [f64; 6])> = pts
            .par_iter()
            .enumerate()
            .filter(|(_, p)| p.usable && model.is_flat(p, wb))
            .map(|(pi, p)| {
                let (px, py) = (pi % cells.pw, pi / cells.pw);
                let idx: Vec<usize> = (py * PATCH..(py + 1) * PATCH)
                    .flat_map(|cy| (px * PATCH..(px + 1) * PATCH).map(move |cx| cy * cells.cw + cx))
                    .collect();
                let mut out = [0.0f64; 6];
                for j in 0..2 {
                    let mu = idx.iter().map(|&i| f64::from(cells.d[i][j])).sum::<f64>() / idx.len() as f64;
                    let (mut vin, mut vout, mut cov, mut inc) = (0.0, 0.0, 0.0, 0.0);
                    for &i in &idx {
                        let (d, g) = (f64::from(cells.d[i][j]) - mu, f64::from(f[i][j]) - mu);
                        vin += d * d;
                        vout += g * g;
                        cov += d * g;
                        inc += included(i)[j];
                    }
                    out[3 * j] = vout / vin;
                    out[3 * j + 1] = cov / vin;
                    out[3 * j + 2] = inc / idx.len() as f64;
                }
                (p.m, out)
            })
            .collect();
        let mut rows = rows;
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        let per = rows.len() / 10;
        print!(
            "  retention on flat patches, k {kk:.3}, window 1/N = {:.4}: per level decile, R-G var ratio / own slope / inclusion | B-G the same",
            1.0 / n_win
        );
        for q in 0..10 {
            let part = &rows[q * per..(q + 1) * per];
            let mean = |j: usize| part.iter().map(|r| r.1[j]).sum::<f64>() / part.len() as f64;
            println!();
            print!(
                "    D{q} level {:.2e}: {:.4} / {:.4} / {:.3} | {:.4} / {:.4} / {:.3}",
                part[part.len() / 2].0,
                mean(0),
                mean(1),
                mean(2),
                mean(3),
                mean(4),
                mean(5)
            );
        }
        println!();
    }
}

/// Q-e reading (rel1180 msg-e3 round 5): the noise model fitted on every
/// `s`-th patch (uniform stride, offsets 0..min(s, 4)) against the fit on all
/// of them — the worst |ln a|, |ln b|, the worst |ln var| over the frame's
/// (level decile, radius) grid and |band| — with the time of one fit.
/// `E3_PROBE=<raw>`; prints the crop and the dummy-decode stage rule too.
#[test]
#[ignore]
fn fit_sample_reading() {
    use rawler::rawimage::RawPhotometricInterpretation as Photo;
    let Ok(path) = std::env::var("E3_PROBE") else { return };
    let dummy = crate::decode::linear_colour_stage_reads(std::path::Path::new(&path)).expect("dummy decode");
    let raw = rawler::decode_file(&path).expect("decode");
    println!(
        "{path}: {}x{} crop {:?} active {:?}; dummy-decode reads {dummy}, full-decode reads {}",
        raw.width,
        raw.height,
        raw.crop_area,
        raw.active_area,
        reads(&raw)
    );
    let Photo::Cfa(config) = &raw.photometric else { panic!("not a CFA") };
    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();
    let width = raw.width;
    let mut v = raw.data.as_f32().into_owned();
    v.par_chunks_exact_mut(width * 2).for_each(|lines| {
        let (l0, l1) = lines.split_at_mut(width);
        l0.chunks_exact_mut(2).zip(l1.chunks_exact_mut(2)).for_each(|(a, b)| {
            a[0] = (a[0] - black[0]) / (white[0] - black[0]);
            a[1] = (a[1] - black[1]) / (white[1] - black[1]);
            b[0] = (b[0] - black[2]) / (white[2] - black[2]);
            b[1] = (b[1] - black[3]) / (white[3] - black[3]);
        });
    });
    let wb = [raw.wb_coeffs[0], raw.wb_coeffs[1], raw.wb_coeffs[2]].map(f64::from);
    let wb = [wb[0] / wb[1], 1.0, wb[2] / wb[1]];
    let sites = bayer_sites(&config.cfa).expect("Bayer");
    let t0 = std::time::Instant::now();
    let pts = patches(&v, width, raw.height, sites, wb);
    let t_patches = t0.elapsed().as_secs_f64();
    let t1 = std::time::Instant::now();
    let full = NoiseModel::fit_all(&pts, width, raw.height).expect("model");
    let t_fit = t1.elapsed().as_secs_f64();
    let usable = pts.iter().filter(|p| p.usable).count();
    let mut levels: Vec<f64> = pts.iter().filter(|p| p.usable).map(|p| p.m).collect();
    levels.sort_by(f64::total_cmp);
    let grid: Vec<(f64, f64)> = (0..10)
        .flat_map(|d| {
            let m = levels[(levels.len() - 1) * (2 * d + 1) / 20];
            (0..5).map(move |j| (m, f64::from(j) / 4.0))
        })
        .collect();
    println!(
        "  patches {t_patches:.2} s, full fit {t_fit:.2} s on {usable} usable of {} patches; a {:.4e} b {:.4e} c2 {:+.4} c4 {:+.4} band {:.4}",
        pts.len(),
        full.a,
        full.b,
        full.c2,
        full.c4,
        full.band
    );
    if let Ok(out) = std::env::var("E3_FLAT_OUT") {
        let (pw, ph) = ((width / 2) / PATCH, (raw.height / 2) / PATCH);
        assert_eq!(pw * ph, pts.len(), "patches are row-major pw x ph");
        let mut bytes = Vec::with_capacity(8 + pts.len());
        bytes.extend_from_slice(&(pw as u32).to_le_bytes());
        bytes.extend_from_slice(&(ph as u32).to_le_bytes());
        bytes.extend(pts.iter().map(|p| u8::from(full.is_flat(p, wb))));
        std::fs::write(&out, bytes).expect("flat mask");
        println!("  flat mask {pw}x{ph} patches -> {out}");
    }
    let mut s = 2usize;
    while pts.len() / s >= 16 {
        let mut worst = [0.0f64; 4];
        let mut t = 0.0f64;
        let offs = s.min(4);
        for off in 0..offs {
            let sub: Vec<Patch> = pts.iter().skip(off).step_by(s).copied().collect();
            let t2 = std::time::Instant::now();
            let Some(m) = NoiseModel::fit_all(&sub, width, raw.height) else {
                worst = [f64::INFINITY; 4];
                continue;
            };
            t += t2.elapsed().as_secs_f64();
            let e = [
                (m.a / full.a).ln().abs(),
                (m.b / full.b).ln().abs(),
                grid.iter().map(|&(l, r)| (m.var(l, r) / full.var(l, r)).ln().abs()).fold(0.0, f64::max),
                (m.band - full.band).abs(),
            ];
            for j in 0..4 {
                worst[j] = worst[j].max(e[j]);
            }
        }
        let n = pts.len().div_ceil(s);
        println!(
            "  stride {s:5}: {n:7} patches ({:9} cells): |ln a| {:.4} |ln b| {:.4} |ln var| {:.4} |band| {:.4}; fit {:.3} s",
            n * PATCH * PATCH,
            worst[0],
            worst[1],
            worst[2],
            worst[3],
            t / offs as f64
        );
        s *= 2;
    }
}
