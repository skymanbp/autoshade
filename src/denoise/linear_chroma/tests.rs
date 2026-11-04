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
    let (model, _) = filter_mosaic(&mut after, W, H, &rggb(), WB, 1.0, 3).expect("a Bayer mosaic with noise fits a model");
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
    filter_mosaic(&mut after, W, H, &rggb(), WB, 1.0, 3).expect("model");
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
/// reach rule: at 10 the window is 2 cells, at 25 it is 3, and the residual
/// colour noise at 10 is the larger.
///
/// MUTATION: the window fixed at the default strength's (the slider is then
/// dead below 25 on such a photo).
#[test]
fn where_k_is_unbounded_the_slider_weakens_the_stage_through_its_window() {
    let before = mosaic(0.01, 0.0);
    let at = |nr: f32| {
        let mut m = before.clone();
        let (_, k) = filter_mosaic(&mut m, W, H, &rggb(), WB, scale_of(nr).unwrap(), window(nr)).expect("a model");
        (k, var_dr(&m, 0..W / 2))
    };
    assert_eq!((window(10.0), window(25.0), window(100.0)), (2, 3, 8), "the reach rule in cells");
    let ((k10, v10), (k25, v25)) = (at(10.0), at(25.0));
    assert!(k10.is_infinite() && k25.is_infinite(), "a noise-only frame has no texture to protect: k {k10} / {k25}");
    assert!(v10 > v25, "residual colour variance at 10 ({v10:.3e}) must exceed that at 25 ({v25:.3e})");
}

#[test]
fn a_pattern_that_is_not_a_two_by_two_bayer_is_left_alone() {
    let before = mosaic(0.01, 0.0);
    let mut after = before.clone();
    let xtrans = rawler::cfa::CFA::new("GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG");
    assert!(filter_mosaic(&mut after, W, H, &xtrans, WB, 1.0, 3).is_none());
    assert!(before.iter().zip(&after).all(|(a, b)| a.to_bits() == b.to_bits()));
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
    let rho = window(DEFAULT_AMOUNT);
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
}
