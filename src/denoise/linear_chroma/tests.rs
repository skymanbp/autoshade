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

#[test]
fn the_stage_takes_the_colour_noise_and_leaves_the_greens_and_the_mean() {
    let before = mosaic(0.01, 0.0);
    let mut after = before.clone();
    let (model, _, _) = filter_mosaic(&mut after, W, H, &rggb(), WB, 1.0, |_| 3).expect("a Bayer mosaic with noise fits a model");
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
    filter_mosaic(&mut after, W, H, &rggb(), WB, 1.0, |_| 3).expect("model");
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

#[test]
fn the_slider_scales_k_and_zero_is_off() {
    assert_eq!(scale_of(0.0), None);
    assert_eq!(scale_of(DEFAULT_AMOUNT), Some(1.0));
    assert_eq!(scale_of(50.0), Some(2.0));
    assert_eq!(scale_of(100.0), Some(4.0));
    assert_eq!(scale_of(250.0), Some(4.0));
}

/// Where the photo has nothing to protect `k` is unbounded and the filter is
/// the plain mean of its window; the slider still weakens it, through the
/// window: at 10 it is 1 cell, at 25 it is 7 (as many samples as the display
/// pass averages there), and the residual colour noise at 10 is the larger.
///
/// MUTATION: the window fixed at the default strength's (the slider is then
/// dead below 25 on such a photo).
#[test]
fn where_k_is_unbounded_the_slider_weakens_the_stage_through_its_window() {
    let before = mosaic(0.01, 0.0);
    let at = |nr: f32| {
        let mut m = before.clone();
        let (_, k, _) = filter_mosaic(&mut m, W, H, &rggb(), WB, scale_of(nr).unwrap(), |k| window(nr, k)).expect("a model");
        (k, var_dr(&m, 0..W / 2))
    };
    assert_eq!((window(10.0, f64::INFINITY), window(25.0, f64::INFINITY), window(100.0, f64::INFINITY)), (1, 7, 23), "the display pass's sample count in cells");
    let ((k10, v10), (k25, v25)) = (at(10.0), at(25.0));
    assert!(k10.is_infinite() && k25.is_infinite(), "a noise-only frame has no texture to protect: k {k10} / {k25}");
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
/// `t = k·√(1 + 1/(N·Z))` read with it to its fixed point. On daylight's `k`
/// (wide `t`) it is the plain mean's window; at the night sky's `k` (1.43,
/// rel1180 msg-e3 round 9) it is wider, 11 cells at 25; unbounded `k` is the
/// plain mean's. Printed per strength at the three photos' `k`.
///
/// MUTATION: the window from the plain mean's share (the `Z − 2t·φ(t)`
/// factor dropped): the night sky's window is then 7 cells and keeps 2.3
/// times the display pass's share.
#[test]
fn the_window_leaves_the_display_share_at_the_width_the_stage_runs_at() {
    for k in [1.432, 6.210, 13.614, f64::INFINITY] {
        for nr in [10.0f32, 20.0, 25.0, 50.0, 75.0, 100.0] {
            let ks = k * scale_of(nr).unwrap();
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
            let off = |r: usize| ((2 * r + 1) as f64 - n.sqrt()).abs();
            assert!(
                (rho == 1 || off(rho) <= off(rho - 1)) && off(rho) <= off(rho + 1),
                "k {k}, colour NR {nr}: the skipped mean at t = {t:.3} leaves the display share over {n:.1} cells, half side {rho} is not the nearest"
            );
        }
    }
    assert_eq!(window(DEFAULT_AMOUNT, 13.614), plain_window(DEFAULT_AMOUNT), "daylight's t is wide: the plain mean's window");
    assert_eq!(window(DEFAULT_AMOUNT, 1.432), 11, "the night sky at 25");
}

/// On a flat frame the filter keeps of a cell's own noise what the skipped
/// mean at its test width keeps, and no more: the slope of the filtered
/// colour difference on the cell's own sample is `1 / (N·(Z − 2t·φ(t)))`,
/// `t = k·√(1 + 1/n)` in the cell's true sd (the model's sd over the true
/// one carried in, `n = N·Z`), within three standard errors of the slope
/// (its residual sd over the sample sd, over the square root of the cells
/// that are independent: one per window). At `k` = 1.6 and 2.5, the night
/// sky's order of `k` and a daylight one.
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
    let rho = 3;
    let n_win = ((2 * rho + 1) * (2 * rho + 1)) as f64;
    // The true sd of each colour difference of one cell (the mosaic's own
    // noise: the site's, the two greens averaged).
    let g_var = (A * level + B) / 2.0;
    let true_sd = [
        (WB[0] * WB[0] * (A * level / WB[0] + B) + g_var).sqrt(),
        (WB[2] * WB[2] * (A * level / WB[2] + B) + g_var).sqrt(),
    ];
    let interior: Vec<usize> = (rho..cells.ch - rho).flat_map(|cy| (rho..cells.cw - rho).map(move |cx| cy * W / 2 + cx)).collect();
    for k in [1.6, 2.5] {
        let f: Vec<[f32; 2]> = interior.par_iter().map(|&i| cells.filtered(i, k, rho)).collect();
        for j in 0..2 {
            let (mut sdd, mut sdf) = (0.0f64, 0.0f64);
            for (n, &i) in interior.iter().enumerate() {
                let d = f64::from(cells.d[i][j]);
                sdd += d * d;
                sdf += d * f64::from(f[n][j]);
            }
            let slope = sdf / sdd;
            let resid: f64 = interior
                .iter()
                .enumerate()
                .map(|(n, &i)| (f64::from(f[n][j]) - slope * f64::from(cells.d[i][j])).powi(2))
                .sum::<f64>()
                / interior.len() as f64;
            let se = resid.sqrt() / (sdd / interior.len() as f64).sqrt() / (interior.len() as f64 / n_win).sqrt();
            let r = f64::from(cells.sd[0][j]) / 2f64.sqrt() / true_sd[j];
            let mut t = k * r * 2f64.sqrt();
            // t and n = N·Z depend on each other; the iteration settles within a few steps.
            for _ in 0..8 {
                t = k * r * (1.0 + 1.0 / (n_win * normal_mass(t).0)).sqrt();
            }
            let (z, phi) = normal_mass(t);
            let want = 1.0 / (n_win * (z - 2.0 * t * phi));
            assert!(
                (slope - want).abs() <= 3.0 * se,
                "k {k}, colour difference {j}: the filter keeps {slope:.4} of a cell's own noise; the skipped mean at t = {t:.3} keeps {want:.4} (3 se {:.4}; the window mean 1/N = {:.4})",
                3.0 * se,
                1.0 / n_win
            );
        }
    }
}

#[test]
fn a_pattern_that_is_not_a_two_by_two_bayer_is_left_alone() {
    let before = mosaic(0.01, 0.0);
    let mut after = before.clone();
    let xtrans = rawler::cfa::CFA::new("GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG");
    assert!(filter_mosaic(&mut after, W, H, &xtrans, WB, 1.0, |_| 3).is_none());
    assert!(before.iter().zip(&after).all(|(a, b)| a.to_bits() == b.to_bits()));
}

/// The sensor measures colour once per cell; pixel-scale chroma is the
/// demosaic's, so the stage's cell chroma is the pixel's: after the rebuild
/// every pixel's green is its own (bit for bit) and its chroma `wR·R − G`,
/// `wB·B − G` is the cell chroma at the pixel's centre. A field linear in the
/// cell grid is read exactly by the bilinear tap, so a ramp plane must come
/// back as the ramp at the pixel's position in cells, `(x + ox − 0.5)/2` (a
/// cell's two pixels sit a quarter cell either side of its centre), whatever
/// noise the demosaic left in the pixel's own red and blue.
///
/// MUTATION: the pixel takes its own cell's chroma (nearest cell, no
/// bilinear read): the chroma is then a step at every cell edge.
#[test]
fn the_pixels_chroma_is_the_cells_chroma_read_at_the_pixel() {
    let (cw, ch, ox, oy, w, h) = (40usize, 30usize, 6usize, 4usize, 64usize, 44usize);
    let ramp = |j: usize, u: f64, v: f64| if j == 0 { 0.01 * u - 0.003 * v } else { -0.002 * u + 0.008 * v + 0.05 };
    let planes =
        [0, 1].map(|j| (0..cw * ch).map(|i| ramp(j, (i % cw) as f64, (i / cw) as f64) as f32).collect::<Vec<f32>>());
    let chroma = CellChroma { cw, ch, wb: [WB[0] as f32, WB[2] as f32], planes };
    let before: Vec<[f32; 3]> =
        (0..w * h).map(|i| [0, 1, 2].map(|c| (0.2 + 0.05 * normal(11 + c as u64, i as u64)) as f32)).collect();
    let mut after = before.clone();
    chroma.rebuild(&mut after, w, ox, oy);
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        let (x, y) = (i % w, i / w);
        assert_eq!(b[1].to_bits(), a[1].to_bits(), "the green of pixel ({x}, {y}) moved");
        let (u, v) = (((x + ox) as f64 - 0.5) / 2.0, ((y + oy) as f64 - 0.5) / 2.0);
        for (j, (wc, c)) in [(WB[0], a[0]), (WB[2], a[2])].into_iter().enumerate() {
            let got = wc * f64::from(c) - f64::from(a[1]);
            let want = ramp(j, u, v);
            assert!(
                (got - want).abs() < 2e-5,
                "pixel ({x}, {y}), colour difference {j}: chroma {got:.6}, the cell chroma at ({u}, {v}) cells is {want:.6}"
            );
        }
    }
}

/// The reading behind `photo_k` on a real RAW (`E3_PROBE=<raw path>`): the
/// model, the textured share and, per rung, the removed edge energy over the
/// noise's share. Run by hand: `cargo test --release --lib photo_k_reading -- --ignored --nocapture`.
#[test]
#[ignore]
fn photo_k_reading() {
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
    let (k, read) = photo_k(&cells, &model, &pts, wb, rho);
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
    println!("  -> photo k {k:.3}; model fit {:.2} s, k reading {:.2} s", t1.as_secs_f64(), (t2 - t1).as_secs_f64());
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
