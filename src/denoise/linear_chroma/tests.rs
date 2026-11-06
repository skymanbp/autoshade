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
    let (SourceReading { model, .. }, _) = filter_mosaic(&mut after, W, H, &rggb(), WB, at25(), lv25(), None, 0, true).expect("a Bayer mosaic with noise fits a model");
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

/// A question is fair to its population: of `n` true answers, fewer than one
/// is expected to be refused. On three populations (49, 289 and 529 questions)
/// the law's `k` puts the expected false refusals, `2·N·Φ(−k) = N·(1 − Z(k))`,
/// at one: the closed form `Φ⁻¹(1 − 1/(2N))`, read here by bisection on the
/// module's own [`normal_mass`].
///
/// MUTATION: the expected refusals set to N/2 (a `k` of 0.67): this test and
/// the flat frame's false-read count fail.
#[test]
fn a_question_is_fair_to_its_population() {
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

#[test]
fn a_pattern_that_is_not_a_two_by_two_bayer_is_left_alone() {
    let before = mosaic(0.01, 0.0);
    let mut after = before.clone();
    let xtrans = rawler::cfa::CFA::new("GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG");
    assert!(filter_mosaic(&mut after, W, H, &xtrans, WB, at25(), lv25(), None, 0, true).is_none());
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
    let (_, c0) = filter_mosaic(&mut kept, W, H, &rggb(), WB, at25(), lv25(), None, 0, false).expect("a model");
    let (_, c1) = filter_mosaic(&mut written, W, H, &rggb(), WB, at25(), lv25(), None, 0, true).expect("a model");
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
    let ones = || vec![1.0f32; cw * ch];
    let chroma = CellChroma { cw, ch, wb: [wr, wb], origin: [(0, 0), (1, 1)], planes, measured: [ones(), ones()], green: flat(), slopes: [flat(), flat()], site_var: Vec::new(), k: 0.0 };
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
    let ones = || vec![1.0f32; cw * ch];
    let origin = [(0, 0), (1, 1)];
    let chroma = CellChroma { cw, ch, wb: [WB[0] as f32, WB[2] as f32], origin, planes, measured: [ones(), ones()], green: flat(), slopes: [flat(), flat()], site_var: Vec::new(), k: 0.0 };
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
    let mut cells = Cells::of(&m, W, H, sites, WB, &model, &pts).expect("cells");
    // The scene's chroma is given at every cell: the measurement (which sites
    // are measured, rel1180 RULING r30) is the other tests'.
    cells.meas.iter_mut().for_each(|q| *q = [true; 2]);
    let scene: Vec<[f32; 2]> = cells.d.iter().map(|d| RATIO.map(|r| ((r - 1.0) * f64::from(d[2])) as f32)).collect();
    let chroma = cells.chroma(&scene, WB, 3);
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
    let chroma = cells.chroma(&measured, WB, 3);
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
    let (ran, _) = filter_mosaic(&mut denoised, W, H, &rggb(), WB, at25(), lv25(), Some(&reading), 0, true).expect("the stage runs");
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

/// The window the stage runs at the default strength.
fn at25() -> usize {
    window(DEFAULT_AMOUNT, RHO_G_MAX).expect("on")
}

/// The light's levels the stage reads at the default strength.
fn lv25() -> u32 {
    levels(DEFAULT_AMOUNT).expect("on")
}

/// An RGGB mosaic whose cell column `cx` has the light `g(cx)` (green) and
/// the colour difference `d(cx)` (R − G, white-balanced), the blue grey, with
/// photon noise `a` and the module's read noise (`noise` false: none).
fn scene(g: impl Fn(usize) -> f64 + Sync, d: impl Fn(usize) -> f64 + Sync, a: f64, seed: u64, noise: bool) -> Vec<f32> {
    let mut m = vec![0.0f32; W * H];
    m.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let gl = g(x / 2);
            let sv = match (y % 2, x % 2) {
                (0, 0) => (gl + d(x / 2)) / WB[0],
                (1, 1) => gl / WB[2],
                _ => gl,
            };
            *v = (sv + if noise { (a * sv + B).sqrt() * normal(seed, (y * W + x) as u64) } else { 0.0 }) as f32;
        }
    });
    m
}

/// The cells of mosaic `m`, at the model fitted on it (or at `model`).
fn cells_of(m: &[f32], model: Option<NoiseModel>) -> Cells {
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let pts = patches(m, W, H, sites, WB);
    let model = model.unwrap_or_else(|| NoiseModel::fit(&pts, W, H).expect("model"));
    Cells::of(m, W, H, sites, WB, &model, &pts).expect("cells")
}

/// One cell's R − G variance at green `g` (photon `a`): the site's own and its four greens.
fn cell_var(g: f64, a: f64) -> f64 {
    WB[0] * WB[0] * (a * g / WB[0] + B) + (a * g + B) / 4.0
}

/// The mean over all rows of column `cx` of plane `j` of `f`.
fn colmean(f: &[[f32; 2]], cx: usize, j: usize) -> f64 {
    (0..H / 2).map(|cy| f64::from(f[cy * (W / 2) + cx][j])).sum::<f64>() / (H / 2) as f64
}

/// The slider reads the window the display pass leaves: at each strength the
/// smallest half side (at least one cell) at which a flat frame keeps no more
/// than the display pass's share of a cell's white noise, `4 / display_samples`
/// — the plane read's [`plane_read`] share with the shared green at its bound,
/// `share·(1 + 4·ρ_G,max·τ)` ([`RHO_G_MAX`], rel1180 RULING r26 Q26-1 / Q26-3),
/// the weights checked here by brute force (each window's least-squares
/// plane read at the cell, averaged over the windows holding it). Asserted at
/// each strength: the half side is the least, and with the false reads' share
/// at the module's frame ([`derived_flat`]) it stays under the display share.
/// Printed per strength with the box share (the read before the plane), the
/// share with the module's own shared green, `ν`, `μ` ([`window_dof`]), the
/// reach ([`reach`]), the step-read level and the widths ([`ladder`]); the
/// bound of `ρ_G` and where it is attained, and `ρ_G` off a neutral; 0 is off,
/// above 100 as at 100.
#[test]
fn the_slider_reads_the_window_the_display_pass_leaves() {
    for rho in [1usize, 3, 6] {
        let n = 2 * rho + 1;
        let big_n = (n * n) as f64;
        let s = (rho * (rho + 1) * (2 * rho + 1)) as f64 / 3.0;
        // Brute force: cell (0, 0) read by every window holding it, each window's
        // plane hat `1/N + u_i·u_j/(n·S) + v_i·v_j/(n·S)`.
        let r = rho as isize;
        let mut w = std::collections::HashMap::new();
        for cy in -r..=r {
            for cx in -r..=r {
                for jy in cy - r..=cy + r {
                    for jx in cx - r..=cx + r {
                        let h = 1.0 / big_n + ((-cx) * (jx - cx)) as f64 / (n as f64 * s) + ((-cy) * (jy - cy)) as f64 / (n as f64 * s);
                        *w.entry((jx, jy)).or_insert(0.0) += h / big_n;
                    }
                }
            }
        }
        let sq: f64 = w.values().map(|x| x * x).sum();
        let lag: f64 = w.iter().map(|(&(x, y), v)| v * (w.get(&(x + 1, y)).unwrap_or(&0.0) + w.get(&(x, y + 1)).unwrap_or(&0.0))).sum();
        let p = plane_read(rho);
        println!("half side {rho}: plane share {:.9} (brute force {sq:.9}), tau {:.6} (brute force {:.6}), centre {:.6} (brute force {:.6}); box share {:.9}", p.share, p.tau, lag / 2.0 / sq, p.centre, w[&(0, 0)], box_share(rho));
        assert!((p.share - sq).abs() < 1e-12 && (p.tau - lag / 2.0 / sq).abs() < 1e-9 && (p.centre - w[&(0, 0)]).abs() < 1e-12 && (p.side - w[&(1, 0)]).abs() < 1e-12);
    }
    assert_eq!(window(0.0, RHO_G_MAX), None, "0 is off");
    let level = 0.01;
    let site = A * level + B;
    let var = WB[0] * WB[0] * (A * level / WB[0] + B) + site / 4.0;
    let neutral = |w: f64, g: f64| {
        let sg = A * g + B;
        sg / 16.0 / (w * w * (A * g / w + B) + sg / 4.0)
    };
    println!(
        "rho_G bound {RHO_G_MAX} (a neutral, colour gains >= 1): at gain 1 {:.6} (green 0.01) / {:.6} (green 0.5); gain 1.6 {:.6}, 2.5 {:.6}; off a neutral, the colour site black at green 0.01 / 0.5: {:.4} / {:.4}; the module's frame {:.6}",
        neutral(1.0, 0.01),
        neutral(1.0, 0.5),
        neutral(1.6, 0.01),
        neutral(2.5, 0.01),
        (A * 0.01 + B) / 16.0 / (WB[0] * WB[0] * B + (A * 0.01 + B) / 4.0),
        (A * 0.5 + B) / 16.0 / (WB[0] * WB[0] * B + (A * 0.5 + B) / 4.0),
        site / 16.0 / var
    );
    let (k, _) = step_read_level(1, fair_k((W / 2 * H / 2) as f64));
    for nr in [1.0f32, 10.0, 20.0, 25.0, 50.0, 75.0, 100.0] {
        let (rho, lv) = (window(nr, RHO_G_MAX).expect("on"), levels(nr).expect("on"));
        let display = 4.0 / display_samples(nr);
        let p = plane_read(rho);
        let bound = p.share * (1.0 + 4.0 * RHO_G_MAX * p.tau);
        let (boxed, false_reads) = derived_flat(rho, lv, (W / 2, H / 2), var, site / 16.0);
        let (nu, mu) = window_dof(rho, lv);
        println!(
            "colour NR {nr}: display share of a cell's noise {display:.6}; half side {rho}, plane share {:.6} (at the bound {bound:.6}; with the module's shared green {boxed:.6}, + false reads <= {false_reads:.2e}; the box share {:.6}); levels {lv}, nu {nu:.4}, mu {mu:.4}, reach {}, step read above {:.3} light sds, widths {:?}",
            p.share,
            box_share(rho),
            reach(rho, lv),
            step_read_level(lv, fair_k((W / 2 * H / 2) as f64)).0.max(k),
            ladder(rho, lv)
        );
        assert!(bound <= display || rho == 1, "colour NR {nr}: half side {rho} leaves {bound} > {display}");
        // The false reads' share: the window's own null (`p`, a frame's `C·p`
        // over `N·C`) is the frame's size free; the light's law (about one false
        // read a level a frame, lifting the windows its kernel touches) is a
        // frame's, over its `N·C` — at the module's 0.2 MP frame printed above,
        // on a 24 MP frame (6e6 cells) asserted.
        let (window_null, ..) = false_window(rho, lv, 6.0e6);
        let lifted: f64 = (1..=lv).map(|l| ((2 * rho + 1 + composed_kernel(l).len() - 1) as f64).powi(2)).sum::<f64>() / (((2 * rho + 1) * (2 * rho + 1)) as f64 * 6.0e6);
        println!("colour NR {nr}: false reads' share, the window's null {:.2e}, the light's law's on a 24 MP frame {lifted:.2e}", window_null / ((2 * rho + 1) * (2 * rho + 1)) as f64);
        assert!(bound + window_null / ((2 * rho + 1) * (2 * rho + 1)) as f64 + lifted <= display || rho == 1, "colour NR {nr}: with the false reads {} > {display}", bound + lifted);
        assert!(rho == 1 || { let q = plane_read(rho - 1); q.share * (1.0 + 4.0 * RHO_G_MAX * q.tau) > display }, "colour NR {nr}: half side {rho} is not the least");
    }
    assert_eq!(window(250.0, RHO_G_MAX), window(100.0, RHO_G_MAX));
}

/// The flat frame's reading: the module's grey at 0.01, read at the default
/// window, with the interior cells (every window holding them whole), the
/// cells' true difference variances and the shared green's covariance of two
/// axis neighbours' differences.
struct Flat {
    cells: Cells,
    out: Vec<[f32; 2]>,
    interior: Vec<usize>,
    var: [f64; 2],
    shared: f64,
}

fn flat() -> Flat {
    let level = 0.01;
    let cells = cells_of(&mosaic(level, 0.0), None);
    let rho = at25();
    let out = cells.guided(rho, lv25());
    let r = 2 * rho;
    let interior = (r..cells.ch - r).flat_map(|cy| (r..cells.cw - r).map(move |cx| cy * W / 2 + cx)).collect();
    let site = A * level + B;
    let var = [WB[0] * WB[0] * (A * level / WB[0] + B) + site / 4.0, WB[2] * WB[2] * (A * level / WB[2] + B) + site / 4.0];
    Flat { cells, out, interior, var, shared: site / 16.0 }
}

/// The share of a flat frame's noise the read keeps (see the module): the
/// plane read's with the shared green, `share·(1 + 4·ρ_G·τ)` ([`plane_read`]); and the bound of
/// the false reads' share. A slope is read on a flat frame only where a window
/// reads beyond its null at `k_F` or where the light read a coefficient on
/// its noise; each read window adds at most `σ²_d/N` of slope energy over its
/// cells (`a = cov/(var + r̄)`, `E[cov²] = σ²_d·var/N`, so `E[a²·var] ≤ σ²_d/N`,
/// and a cell's slope term is the mean over at most `N` windows), so the share
/// is at most the expected read windows over `N·C`: `C·p` of the window's null
/// (`p` its Chernoff bound, [`false_window`]) and, per light level, the law's
/// one false read lifting the windows its detail kernel touches.
fn derived_flat(rho: usize, levels: u32, cells: (usize, usize), var: f64, shared: f64) -> (f64, f64) {
    let PlaneRead { share: s, tau, .. } = plane_read(rho);
    let n = 2 * rho + 1;
    let c = (cells.0 * cells.1) as f64;
    let lifted: f64 = (1..=levels).map(|l| ((n + composed_kernel(l).len() - 1) as f64).powi(2)).sum();
    (s * (1.0 + 4.0 * shared / var * tau), (c * false_window(rho, levels, c).0 + lifted) / ((n * n) as f64 * c))
}

/// The window's null on a flat frame, from the taps ([`window_dof`]): its
/// light variance is `(μ·r̄/ν)·χ²_ν` (Satterthwaite), and its slope is read
/// past `r̄·(1 + k_F·√(2/ν))` — `x = (1 + k_F·√(2/ν))/μ` times the null's mean —
/// so a window is falsely read with probability at most the Chernoff bound
/// `(x·e^(1 − x))^(ν/2)`. Returns `(p, ν, μ, x)`.
fn false_window(rho: usize, levels: u32, cells: f64) -> (f64, f64, f64, f64) {
    let (nu, mu) = window_dof(rho, levels);
    let x = (1.0 + fair_k(cells) * (2.0 / nu).sqrt()) / mu;
    ((x * (1.0 - x).exp()).powf(nu / 2.0), nu, mu, x)
}

/// The window's effective samples from the taps against a direct
/// evaluation: the variance about the window's plane of white noise smoothed
/// by the composed kernel over a whole window, `Q = gᵀMg/N` with `M` the
/// projector off the plane built cell by cell, its mean `r·tr(CM)/N` and
/// variance `2r²·tr((CM)²)/N²` summed over the window's cells' correlations;
/// printed with the false read's bound per strength.
#[test]
fn a_windows_light_variance_has_the_derived_degrees_of_freedom() {
    for (rho, levels) in [(1usize, 1u32), (3, 2), (6, 3), (9, 3)] {
        let (nu, mu) = window_dof(rho, levels);
        let k = composed_kernel(levels);
        let auto = |d: usize| -> f64 { k.iter().zip(k.get(d..).unwrap_or(&[])).map(|(a, b)| a * b).sum() };
        let n = 2 * rho + 1;
        let cells: Vec<(f64, f64)> = (0..n * n).map(|j| ((j % n) as f64 - rho as f64, (j / n) as f64 - rho as f64)).collect();
        let big_n = n * n;
        let su: f64 = cells.iter().map(|c| c.0 * c.0).sum();
        let corr = |a: usize, b: usize| auto((a % n).abs_diff(b % n)) * auto((a / n).abs_diff(b / n)) / auto(0).powi(2);
        let m = |a: usize, b: usize| f64::from(u8::from(a == b)) - 1.0 / big_n as f64 - cells[a].0 * cells[b].0 / su - cells[a].1 * cells[b].1 / su;
        let cm: Vec<Vec<f64>> = (0..big_n).map(|a| (0..big_n).map(|b| (0..big_n).map(|q| corr(a, q) * m(q, b)).sum()).collect()).collect();
        let tr: f64 = (0..big_n).map(|a| cm[a][a]).sum();
        let tr2: f64 = (0..big_n).map(|a| (0..big_n).map(|b| cm[a][b] * cm[b][a]).sum::<f64>()).sum();
        let (direct, mean) = (tr * tr / tr2, tr / big_n as f64);
        println!("half side {rho}, levels {levels}: nu {nu:.6} (direct {direct:.6}), mu {mu:.6} (direct {mean:.6})");
        assert!((nu - direct).abs() < 1e-9 * nu && (mu - mean).abs() < 1e-12, "half side {rho}, levels {levels}");
    }
    for nr in [1.0f32, 10.0, 20.0, 25.0, 50.0, 75.0, 100.0] {
        let (rho, lv) = (window(nr, RHO_G_MAX).expect("on"), levels(nr).expect("on"));
        let c = (W / 2 * H / 2) as f64;
        let (p, nu, mu, x) = false_window(rho, lv, c);
        println!("colour NR {nr}: half side {rho}, levels {lv}: nu {nu:.4}, mu {mu:.4}; read past {x:.3} x the null's mean; false window read <= {p:.3e} (C p {:.3e} at C {c})", c * p);
    }
}

/// A flat frame keeps the derived share: the noise left in the read plane,
/// over the cell's true variance, is the box term with the shared green and
/// at most the false reads' share ([`derived_flat`]), within three standard
/// errors (blocks of the read's support, `2n` cells a side). The windows whose
/// slope is read printed.
///
/// The same against the display share directly (the stage keeps what the
/// slider says, rel1180 RULING r25 Q25-1).
///
/// MUTATION: the slope read everywhere (the rule dropped): the line of a
/// flat frame's noise adds its slope term (printed; the ramp test is its
/// witness).
#[test]
fn a_flat_frame_keeps_the_derived_share() {
    let Flat { cells, out, interior: _, var, shared } = flat();
    let rho = at25();
    let b = 2 * (2 * rho + 1);
    let r = 2 * rho;
    let light = cells.read_light(lv25(), fair_k((cells.cw * cells.ch) as f64));
    let read = slopes_read(&cells, &light, rho, lv25());
    println!("flat frame: windows whose slope is read {} of {}; light read per level {:?}", read.iter().filter(|&&x| x).count(), read.len(), read_counts(&light, lv25()));
    for (j, &cv) in var.iter().enumerate() {
        let (boxed, false_reads) = derived_flat(rho, lv25(), (cells.cw, cells.ch), cv, shared);
        let want = boxed;
        let mean = out.iter().map(|q| f64::from(q[j])).sum::<f64>() / out.len() as f64;
        let mut blocks = Vec::new();
        for by in (r..cells.ch - r - b).step_by(b) {
            for bx in (r..cells.cw - r - b).step_by(b) {
                let s: f64 = (by..by + b).flat_map(|y| (bx..bx + b).map(move |x| (x, y))).map(|(x, y)| (f64::from(out[y * cells.cw + x][j]) - mean).powi(2)).sum();
                blocks.push(s / (b * b) as f64 / cv);
            }
        }
        let nb = blocks.len() as f64;
        let got = blocks.iter().sum::<f64>() / nb;
        let se = (blocks.iter().map(|x| (x - got).powi(2)).sum::<f64>() / (nb - 1.0) / nb).sqrt();
        println!("flat frame, half side {rho}, colour difference {j}: noise left {got:.6} (se {se:.6}) of a cell's variance; derived plane {boxed:.6} + false reads <= {false_reads:.3e} (white plane {:.6})", plane_read(rho).share);
        assert!(got - want <= 3.0 * se + false_reads && want - got <= 3.0 * se, "colour difference {j}: the flat frame keeps {got:.6} of a cell's noise, derived {want:.6} + {false_reads:.3e} (3 se {:.6})", 3.0 * se);
        let display = 4.0 / display_samples(DEFAULT_AMOUNT);
        println!("flat frame, colour difference {j}: against the display share {display:.6}");
        assert!(got <= display + 3.0 * se, "colour difference {j}: the flat frame keeps {got:.6}, the display share {display:.6} (3 se {:.6})", 3.0 * se);
    }
}

/// Per level of `levels`, the light's coefficients read (the cells whose bit
/// `l` is set).
fn read_counts(light: &Light, levels: u32) -> Vec<usize> {
    (0..levels).map(|l| light.bits.iter().filter(|&&b| b & (1 << l) != 0).count()).collect()
}

/// Per window of half side `rho`, is its slope read under `light`
/// ([`slope_read`], the stage's own rule and quantities)?
fn slopes_read(cells: &Cells, light: &Light, rho: usize, levels: u32) -> Vec<bool> {
    let (k, nu) = (fair_k((cells.cw * cells.ch) as f64), window_dof(rho, levels).0);
    box_means((cells.cw, cells.ch), rho, |i| {
        let g = f64::from(light.g[i]);
        let (x, y) = cells.place(i);
        [g, g * g, f64::from(light.r[i]), x, y, x * x, y * y, x * y, x * g, y * g]
    }, |_, [g, gg, r, x, y, xx, yy, xy, xg, yg], _| {
        let var = about_plane(gg - g * g, xg - x * g, yg - y * g, xx - x * x, yy - y * y, xy - x * y);
        var + r > 0.0 && slope_read(var, r, k, nu)
    })
}

/// A window's variance of a quantity about its own plane in position: its
/// variance `v` less what its covariances with `x`, `y` (`cx`, `cy`) explain,
/// the window's own `x`, `y` moments `sxx`, `syy`, `sxy`.
fn about_plane(v: f64, cx: f64, cy: f64, sxx: f64, syy: f64, sxy: f64) -> f64 {
    let det = sxx * syy - sxy * sxy;
    if det <= 0.0 {
        return 0.0;
    }
    (v - (syy * cx * cx - 2.0 * sxy * cx * cy + sxx * cy * cy) / det).max(0.0)
}

/// A cell's own noise kept is the derived share: on a flat frame the read's
/// regression on the cells' own samples is the plane read's centre weight and
/// its four axis neighbours' through the shared green, `w(0) + 4·ρ_G·w(1)`
/// ([`plane_read`]; no light slope is read but at the false reads), within
/// three standard errors.
#[test]
fn a_cells_own_noise_kept_is_the_derived_share() {
    let Flat { cells, out, interior, var, shared } = flat();
    let rho = at25();
    let n = (2 * rho + 1) as f64;
    let support = (2.0 * n - 1.0).powi(2);
    let p = plane_read(rho);
    for (j, &cv) in var.iter().enumerate() {
        let want = p.centre + 4.0 * shared / cv * p.side;
        let mean_d = interior.iter().map(|&i| f64::from(cells.d[i][j])).sum::<f64>() / interior.len() as f64;
        let (mut sdd, mut sdf) = (0.0f64, 0.0f64);
        for &i in &interior {
            let d = f64::from(cells.d[i][j]) - mean_d;
            sdd += d * d;
            sdf += d * f64::from(out[i][j]);
        }
        let slope = sdf / sdd;
        let mean_f = interior.iter().map(|&i| f64::from(out[i][j])).sum::<f64>() / interior.len() as f64;
        let resid = interior.iter().map(|&i| (f64::from(out[i][j]) - mean_f - slope * (f64::from(cells.d[i][j]) - mean_d)).powi(2)).sum::<f64>() / interior.len() as f64;
        let se = resid.max(0.0).sqrt() / (sdd / interior.len() as f64).sqrt() / (interior.len() as f64 / support).sqrt();
        println!("own noise, half side {rho}, colour difference {j}: slope {slope:.6} (se {se:.6}); derived {want:.6} (the plane read's centre and neighbours)");
        assert!((slope - want).abs() <= 3.0 * se, "colour difference {j}: the read keeps {slope:.6} of a cell's own noise, derived {want:.6} (3 se {:.6})", 3.0 * se);
    }
}

/// The read's reach: a cell's read depends on the cells within `2ρ` (its
/// windows' lines, read over the windows holding it) and each of those on the
/// light within the composed kernel's half width at `levels` (the light's
/// read, its residual and its read levels): `2ρ + (len(K_L) − 1)/2`.
fn reach(rho: usize, levels: u32) -> usize {
    2 * rho + (composed_kernel(levels).len() - 1) / 2
}

/// A ramp under a flat light: on the R − G ramp of [`ramp_mosaic`] (0.5, 1
/// and 1.5 cell sds a cell, the round-16 probe's slopes, and none) the
/// window's plane fits the ramp exactly, so every window votes and the ramp is
/// read on its line (rel1180 RULING r26 Q26-1): noiseless, exact within f32
/// rounding on the columns the read's whole reach lies on the ramp
/// ([`reach`]); under the noise (the light's and the colour's), against the
/// ramp itself, the read is unbiased (its mean error within three standard
/// errors) and keeps the plane read's share of each cell's own noise with
/// the shared green ([`derived_flat`], within three standard errors and the
/// false reads' share, blocks of rows of twice the reach), under the
/// display share; every cell read at the slider's width (printed with the
/// windows refused).
///
/// MUTATION: the plane's position terms dropped (the constant hypothesis):
/// the windows refuse the ramp and the cells fall to small widths (red).
#[test]
fn a_ramp_under_a_flat_light_is_read_on_its_line() {
    const A2: f64 = 2e-6;
    let (x0, x1) = RAMP;
    let rho = at25();
    let r = reach(rho, lv25());
    let level = 0.01;
    let display = 4.0 / display_samples(DEFAULT_AMOUNT);
    let mut failed = Vec::new();
    for s in [0.0f64, 0.5, 1.0, 1.5] {
        let (m, cell_sd) = ramp_mosaic(s);
        let ramp = move |cx: usize| s * cell_sd * (cx.clamp(x0, x1) - x0) as f64;
        let cells = cells_of(&m, None);
        let cw = cells.cw;
        let (out, used) = cells.read_widths(rho, lv25(), &cells.read_light(lv25(), fair_k((cw * cells.ch) as f64)));
        let clean = cells_of(&scene(|_| level, ramp, A2, 0, false), Some(cells.model)).guided(rho, lv25());
        let scale = (0..cells.d.len()).fold(0.0f32, |m, i| m.max(cells.d[i][0].abs()).max(ramp(i % cw).abs() as f32).max(cells.d[i][2].abs()));
        let tol = 64.0 * f32::EPSILON * scale;
        let (mut worst0, mut errs, mut sq, mut widths) = (0.0f32, Vec::new(), Vec::new(), vec![0usize; ladder(rho, lv25()).len() + 1]);
        let rows: Vec<usize> = (r..cells.ch - r).collect();
        for chunk in rows.chunks(2 * r).filter(|c| c.len() == 2 * r) {
            let (mut e1, mut e2, mut nn) = (0.0f64, 0.0f64, 0.0f64);
            for &cy in chunk {
                for cx in x0 + r..=x1 - r {
                    let i = cy * cw + cx;
                    worst0 = worst0.max((clean[i][0] - ramp(cx) as f32).abs());
                    let v = WB[0] * WB[0] * (A2 * (level + ramp(cx)) / WB[0] + B) + (A2 * level + B) / 4.0;
                    let e = f64::from(out[i][0]) - ramp(cx);
                    e1 += e / v.sqrt();
                    e2 += e * e / v;
                    nn += 1.0;
                    widths[used[i][0] as usize] += 1;
                }
            }
            errs.push(e1 / nn);
            sq.push(e2 / nn);
        }
        let ((bias, bse), (kept, kse)) = (mean_se(&errs), mean_se(&sq));
        let (want, false_reads) = derived_flat(rho, lv25(), (cw, cells.ch), cell_var(level, A2), (A2 * level + B) / 16.0);
        let total = widths.iter().sum::<usize>() as f64;
        let light = cells.read_light(lv25(), fair_k((cw * cells.ch) as f64));
        let noise = cells.noise();
        let lines = cells.lines(rho, lv25(), &light, &noise);
        let refused = (rows[0]..=*rows.last().expect("rows")).flat_map(|cy| (x0 + rho..=x1 - rho).map(move |cx| cy * cw + cx)).filter(|&i| lines[i][4] == 0.0).count();
        println!(
            "ramp {s} sd/cell, half side {rho}, reach {r}: noiseless worst {worst0:.3e} (f32 rounding {tol:.3e}); under the noise bias {bias:+.4} (se {bse:.4}) cell sds, noise kept {kept:.6} (se {kse:.6}) of a cell's variance, derived {want:.6} + false reads <= {false_reads:.2e}, display {display:.6}; widths then kept {:?}; windows on the ramp refused {refused}",
            widths.iter().map(|w| format!("{:.4}", *w as f64 / total)).collect::<Vec<_>>()
        );
        if worst0 > tol {
            failed.push(format!("{s} sd/cell noiseless: {worst0:e} against {tol:e}"));
        }
        if bias.abs() > 3.0 * bse || kept - want > 3.0 * kse + false_reads || want - kept > 3.0 * kse || kept > display + 3.0 * kse {
            failed.push(format!("{s} sd/cell: bias {bias:+.4} (se {bse:.4}), kept {kept:.6} (se {kse:.6}) against {want:.6}, display {display:.6}"));
        }
    }
    assert!(failed.is_empty(), "the ramp is not read on its line: {failed:?}");
}

/// A ramp of light and colour together (the light rising by `s` of its own
/// sds a cell and R − G by `s` of its own over [`RAMP`], read-noise
/// dominated), on the columns the read's whole reach lies on the ramp
/// ([`reach`]): noiseless, the read is the ramp exactly (f32 rounding); under
/// the noise the read light is a plane in position and its own noise, so no
/// light slope is read (the slope rule reads the light about its own plane)
/// and the light's noise is not copied into the colour at all: the read keeps
/// the plane read's share of each cell's own noise with the shared green
/// ([`derived_flat`]), asserted within three standard errors and the false
/// reads' share, and unbiased (blocks of rows of twice the reach). The windows
/// whose light slope is read printed; a false light read's speck
/// `k_F·σ_{w_0}·α` printed in cell sds (it lifts a slope only where it puts
/// content beyond the plane).
///
/// MUTATION: the slope rule read on the light's variance about its mean (not
/// about its plane): the light's ramp reads as content and a slope fitted to
/// what the plane leaves, its noise, is copied (red).
#[test]
fn a_ramp_of_light_and_colour_together_is_read_exactly() {
    const A2: f64 = 2e-6;
    let (x0, x1) = RAMP;
    let rho = at25();
    let r = reach(rho, lv25());
    let level = 0.01;
    let (sd, sd_g) = (cell_var(level, A2).sqrt(), ((A2 * level + B) / 2.0).sqrt());
    let k1 = composed_kernel(1);
    let s_w0 = (1.0 - 2.0 * k1[2] * k1[2] + k1.iter().map(|w| w * w).sum::<f64>().powi(2)).sqrt();
    let mut failed = Vec::new();
    for s in [0.5f64, 1.0, 1.5] {
        let t = move |cx: usize| (cx.clamp(x0, x1) - x0) as f64;
        let g = move |cx: usize| level + s * sd_g * t(cx);
        let d = move |cx: usize| s * sd * t(cx);
        let noisy = cells_of(&scene(g, d, A2, 61, true), None);
        let clean = cells_of(&scene(g, d, A2, 61, false), Some(noisy.model));
        let cw = noisy.cw;
        let (a, c) = (noisy.guided(rho, lv25()), clean.guided(rho, lv25()));
        let scale = (0..cw * noisy.ch).fold(0.0f32, |m, i| m.max(noisy.d[i][0].abs()).max(noisy.d[i][2].abs()));
        let tol = 64.0 * f32::EPSILON * scale;
        let mut worst0 = 0.0f32;
        let (mut errs, mut sq) = (Vec::new(), Vec::new());
        let rows: Vec<usize> = (r..noisy.ch - r).collect();
        for chunk in rows.chunks(2 * r).filter(|c| c.len() == 2 * r) {
            let (mut e1, mut e2, mut nn) = (0.0f64, 0.0f64, 0.0f64);
            for &cy in chunk {
                for cx in x0 + r..=x1 - r {
                    let i = cy * cw + cx;
                    worst0 = worst0.max((c[i][0] - clean.d[i][0]).abs());
                    let v = WB[0] * WB[0] * (A2 * (g(cx) + d(cx)) / WB[0] + B) + (A2 * g(cx) + B) / 4.0;
                    let e = f64::from(a[i][0]) - f64::from(clean.d[i][0]);
                    e1 += e / v.sqrt();
                    e2 += e * e / v;
                    nn += 1.0;
                }
            }
            errs.push(e1 / nn);
            sq.push(e2 / nn);
        }
        let ((bias, bse), (kept, kse)) = (mean_se(&errs), mean_se(&sq));
        let (want, false_reads) = derived_flat(rho, lv25(), (cw, noisy.ch), cell_var(level, A2), (A2 * level + B) / 16.0);
        let light = noisy.read_light(lv25(), fair_k((cw * noisy.ch) as f64));
        let read = slopes_read(&noisy, &light, rho, lv25());
        let alpha = sd / sd_g;
        println!(
            "light and colour ramp {s} sd/cell, half side {rho}, reach {r}: noiseless worst {worst0:.3e} (f32 rounding {tol:.3e}); under the noise bias {bias:+.4} (se {bse:.4}) cell sds, noise kept {kept:.6} (se {kse:.6}), derived {want:.6} + false reads <= {false_reads:.2e}; windows whose light slope is read {} of {}; a false light read's speck k_F sigma_w0 alpha = {:.3} cell sds",
            read.iter().filter(|&&x| x).count(),
            read.len(),
            fair_k((cw * noisy.ch) as f64) * s_w0 * sd_g * alpha / sd
        );
        assert!(worst0 <= tol, "ramp {s}: the noiseless read moves the ramp by {worst0:e}, f32 rounding {tol:e}");
        if bias.abs() > 3.0 * bse || kept - want > 3.0 * kse + false_reads || want - kept > 3.0 * kse {
            failed.push(format!("{s} sd/cell: bias {bias:+.4} (se {bse:.4}), kept {kept:.6} (se {kse:.6}) against {want:.6}"));
        }
    }
    assert!(failed.is_empty(), "the joint ramp's read is not the derived share: {failed:?}");
}

/// A curved band carried by the light: the round-21 fixture (R − G rising by
/// 16 cell sds through a logistic 16 cells wide, read-noise dominated) with
/// the light following it (16 of its own sds through the same logistic, as a
/// cloud's luma follows its colour): at the two knees the read's mean over the
/// rows less the band's — the cells' own noiseless colour difference, which
/// on a light gradient carries the colour site's green offset, `(g(cx) −
/// g(cx − 1))/4` (its four green neighbours, one from the cell beside it;
/// 0.052 cell sds at the knees) — is within the measurement's error (the standard error
/// of the same column mean on the unfiltered differences, blocks of `2n`
/// rows). The same band under a flat light is printed beside: the box mean's
/// bias, the regime's.
#[test]
fn a_curved_band_carried_by_the_light_is_followed_at_its_knees() {
    const A2: f64 = 2e-6;
    let level = 0.01;
    let (sd, sd_g) = (cell_var(level, A2).sqrt(), ((A2 * level + B) / 2.0).sqrt());
    let (mid, width) = ((W / 4) as f64, 16.0 / (2.0 * 9f64.ln()));
    let sig = move |cx: usize| 1.0 / (1.0 + (-(cx as f64 - mid) / width).exp());
    let band = move |cx: usize| 16.0 * sd * sig(cx);
    let knee = [(mid - width * (2.0 + 3f64.sqrt()).ln()).round() as usize, (mid + width * (2.0 + 3f64.sqrt()).ln()).round() as usize];
    let rho = at25();
    let g = 2 * (2 * rho + 1);
    let r = 2 * rho;
    let mut failed = Vec::new();
    for carried in [true, false] {
        let light = move |cx: usize| level + if carried { 16.0 * sd_g * sig(cx) } else { 0.0 };
        let cells = cells_of(&scene(light, band, A2, 83, true), None);
        let clean = cells_of(&scene(light, band, A2, 83, false), Some(cells.model));
        let out = cells.guided(rho, lv25());
        let rows: Vec<usize> = (r..cells.ch - r).collect();
        let mut line = format!("curved band, light {}: column: read - band (cell sds) | se:", if carried { "carried" } else { "flat" });
        let mut at_knees = Vec::new();
        for cx in W / 4 - 24..=W / 4 + 24 {
            let col = |f: &dyn Fn(usize) -> f64| -> Vec<f64> { rows.chunks(g).filter(|c| c.len() == g).map(|c| c.iter().map(|&cy| f(cy * cells.cw + cx)).sum::<f64>() / g as f64).collect() };
            let (fb, db) = (col(&|i| f64::from(out[i][0])), col(&|i| f64::from(cells.d[i][0])));
            let nb = fb.len() as f64;
            let (fm, dm) = (fb.iter().sum::<f64>() / nb, db.iter().sum::<f64>() / nb);
            let se = (db.iter().map(|x| (x - dm).powi(2)).sum::<f64>() / (nb - 1.0) / nb).sqrt();
            let bias = fm - colmean(&clean.d.iter().map(|q| [q[0], q[1]]).collect::<Vec<_>>(), cx, 0);
            if cx % 4 == 0 {
                line += &format!("  {cx}: {:+.3} | {:.3}", bias / sd, se / sd);
            }
            if knee.contains(&cx) {
                at_knees.push((cx, bias, se));
            }
        }
        println!("{line}");
        for (cx, bias, se) in at_knees {
            println!("  knee {cx}: {:+.4} cell sds (se {:.4})", bias / sd, se / sd);
            if carried && bias.abs() > se {
                failed.push(format!("knee {cx}: {:+.4} cell sds, se {:.4}", bias / sd, se / sd));
            }
        }
    }
    assert!(failed.is_empty(), "the read band is off the band beyond the measurement's error: {failed:?}");
}

/// The bleed of a one-sided R − G edge (`s` cell sds at green `level`, at
/// column W/4, the module's noise) under the light `g`, read at the default
/// window: per column beside the edge, the read's mean over the rows less its
/// side's level, over the step; and the edge window's light variance over its
/// ridge.
fn step_bleed(s: f64, level: f64, g: impl Fn(usize) -> f64 + Sync, seed: u64) -> (Vec<(usize, f64)>, f64, Vec<usize>) {
    let step = s * cell_var(level, A).sqrt();
    let e = W / 4;
    let cells = cells_of(&scene(&g, |cx| if cx >= e { step } else { 0.0 }, A, seed, true), None);
    let rho = at25();
    let light = cells.read_light(lv25(), fair_k((cells.cw * cells.ch) as f64));
    let out = cells.guided_on(rho, lv25(), &light);
    let side = |cols: std::ops::Range<usize>| cols.clone().map(|cx| (0..cells.ch).map(|cy| f64::from(cells.d[cy * cells.cw + cx][0])).sum::<f64>() / cells.ch as f64).sum::<f64>() / cols.len() as f64;
    let (lo, hi) = (side(0..e - 40), side(e + 40..W / 2));
    // The read where the stage reads: a site not measured keeps the develop's
    // own colour, which the rebuild takes, not the read (rel1180 RULING r30).
    let measured_mean = |cx: usize| {
        let ids: Vec<usize> = (0..cells.ch).map(|cy| cy * cells.cw + cx).filter(|&i| cells.meas[i][0]).collect();
        ids.iter().map(|&i| f64::from(out[i][0])).sum::<f64>() / ids.len() as f64
    };
    let bleed = (e - 6..e + 6)
        .filter(|&cx| (0..cells.ch).any(|cy| cells.meas[cy * cells.cw + cx][0]))
        .map(|cx| (cx, (measured_mean(cx) - if cx < e { lo } else { hi }) / step))
        .collect();
    let cy = cells.ch / 2;
    let win: Vec<usize> = (cy - rho..=cy + rho).flat_map(|y| (e - 1 - rho..=e - 1 + rho).map(move |x| y * W / 2 + x)).collect();
    let n = win.len() as f64;
    let mg = win.iter().map(|&i| f64::from(light.g[i])).sum::<f64>() / n;
    let vg = win.iter().map(|&i| (f64::from(light.g[i]) - mg).powi(2)).sum::<f64>() / n;
    let eps = win.iter().map(|&i| f64::from(light.r[i])).sum::<f64>() / n;
    (bleed, vg / eps, read_counts(&light, lv25()))
}

/// The light's step, in its own sds a cell, above which the light's read
/// keeps a step at the two cells beside it: at level `l` the detail
/// coefficient there of a unit step is `w_l = m_l − m_{l+1}` (`m_l` the mass
/// of the composed kernel `K_l` on the step's side), its noise
/// `σ_{w_l}² = v·((ΣK_l²)² − 2·(ΣK_l·K_{l+1})² + (ΣK_{l+1}²)²)`; the step is
/// read at every level there once `Δ·|w_l| > k·σ_{w_l}` at each, so above
/// `k / min_l |w_l|/σ_{w_l}` (in `√v`). Returns it with the per-level
/// coefficients over their noise (the smaller of the two cells).
fn step_read_level(levels: u32, k: f64) -> (f64, Vec<f64>) {
    let ks: Vec<Vec<f64>> = (0..=levels).map(composed_kernel).collect();
    let half = |l: usize| (ks[l].len() / 2) as isize;
    let at = |l: usize, d: isize| if d.abs() <= half(l) { ks[l][(d + half(l)) as usize] } else { 0.0 };
    // The mass of `K_l` on offsets `≥ from` (the step's side seen from a cell).
    let mass = |l: usize, from: isize| (from..=half(l)).map(|d| at(l, d)).sum::<f64>();
    let z: Vec<f64> = (0..levels as usize)
        .map(|l| {
            let span = half(l + 1);
            let dot = |a: usize, b: usize| (-span..=span).map(|d| at(a, d) * at(b, d)).sum::<f64>();
            let sw = (dot(l, l).powi(2) - 2.0 * dot(l, l + 1).powi(2) + dot(l + 1, l + 1).powi(2)).sqrt();
            [1isize, 0].iter().map(|&from| (mass(l, from) - mass(l + 1, from)).abs() / sw).fold(f64::INFINITY, f64::min)
        })
        .collect();
    (k / z.iter().copied().fold(f64::INFINITY, f64::min), z)
}

/// A step the light carries is kept where the light's step is read: R − G
/// edges of 3, 4, 5, 6, 8 and 12 cell sds with the light stepping one stop at
/// the same place, in two arms — the round-23 fixture (green 0.01, the
/// record's) and green where one stop is 20 of the light's own sds a cell.
/// The light's step is read above [`step_read_level`] (at the frame's `k_F`):
/// above it the bleed beside the edge is asserted within the colour edge
/// test's limit (0.15) at every step; below it, printed as the known limit
/// with the arm's light sds ("where the light's own edge is under its noise
/// the colour is averaged across it, as it was without the guide"). The read
/// guide's window variance at the edge over its ridge and the light's
/// coefficients read per level printed.
///
/// MUTATION: the guide made constant: the read is the box mean, and the read
/// arm bleeds as the iso-luminant step does (red).
#[test]
fn a_step_the_light_carries_is_kept() {
    let e = W / 4;
    let (read_at, z) = step_read_level(lv25(), fair_k((W / 2 * H / 2) as f64));
    println!("a one-sided light step is read at the cells beside it above {read_at:.3} of the light's own sds a cell (coefficient over its noise per level {z:?})");
    let twenty = (200.0 * A + ((200.0 * A).powi(2) + 800.0 * B).sqrt()) / 2.0;
    let mut failed = Vec::new();
    for level in [0.01, twenty] {
        let sd_g = ((A * level + B) / 2.0).sqrt();
        let stop = level / sd_g;
        let read = stop >= read_at;
        println!("arm: one stop at green {level:.6} is {stop:.2} of the light's own sds a cell: {}", if read { "read, asserted" } else { "under the read, the known limit (printed)" });
        for s in [3.0f64, 4.0, 5.0, 6.0, 8.0, 12.0] {
            let g = move |cx: usize| if cx < e { level } else { 2.0 * level };
            let (bleed, ratio, counts) = step_bleed(s, level, g, 5);
            let mut line = format!("step {s} sd, light one stop at {level:.4} ({stop:.2} light sds): var_W(Ĝ)/ε at the edge {ratio:.2}; light read per level {counts:?}; err/step:");
            for (cx, b) in &bleed {
                line += &format!("  {cx}: {b:+.3}");
                if read && b.abs() > 0.15 {
                    failed.push(format!("green {level:.4}, {s} sd, column {cx}: {b:+.3}"));
                }
            }
            println!("{line}");
        }
    }
    assert!(failed.is_empty(), "where the light's step is read the colour bleeds beyond 0.15 of the step: {failed:?}");
}

/// The known limits (see the module), printed: a weak luma step — the light
/// stepping by one of its own sds a cell, under `k_F·σ` at every level — with
/// the same colour steps, and the iso-luminant step (the light flat).
#[test]
fn a_weak_luma_step_and_an_iso_luminant_step_are_the_known_limits() {
    let level = 0.01;
    let e = W / 4;
    let sd_g = ((A * level + B) / 2.0).sqrt();
    for s in [3.0f64, 4.0, 5.0, 6.0, 8.0, 12.0] {
        let (bleed, ratio, counts) = step_bleed(s, level, move |cx: usize| if cx < e { level } else { level + sd_g }, 7);
        println!("weak luma step (1 light sd), colour {s} sd: var_W(Ĝ)/ε {ratio:.2}; light read per level {counts:?}; err/step:{}", bleed.iter().map(|(cx, b)| format!("  {cx}: {b:+.3}")).collect::<String>());
    }
    for s in [3.0f64, 4.0, 5.0, 6.0, 8.0, 12.0] {
        let (bleed, ratio, counts) = step_bleed(s, level, |_| level, 3);
        println!("iso-luminant step {s} sd: var_W(Ĝ)/ε {ratio:.2}; light read per level {counts:?}; err/step:{}", bleed.iter().map(|(cx, b)| format!("  {cx}: {b:+.3}")).collect::<String>());
    }
}

/// The seeds of the flat frames the light's false reads and the shared noise
/// are read over.
const SEEDS: u64 = 20;

/// Per window of half side `rho` on `cells` under `light`: the window's
/// measured covariance of the read light with R − G about the window's plane
/// in position, and the shared noise the stage derives for it
/// ([`shared_noise`], with the plane's terms); their means over the whole
/// windows.
fn shared_read(cells: &Cells, light: &Light, rho: usize, levels: u32) -> (f64, f64) {
    let (mass, mass_u) = (window_mass(rho, levels), window_mass_u(rho, levels));
    let per: Vec<[f64; 2]> = box_means((cells.cw, cells.ch), rho, |i| {
        let g = f64::from(light.g[i]);
        let d = f64::from(cells.d[i][0]);
        let (x, y) = cells.place(i);
        let [t1, held, held_u] = cells.shared_terms(light, i, levels, &mass, &mass_u);
        [g, d, g * d, t1, held, held_u, x, y, x * x, y * y, x * y, x * g, y * g, x * d, y * d]
    }, |_, [g, d, gd, t1, held, held_u, x, y, xx, yy, xy, xg, yg, xd, yd], n| {
        // The covariance about the window's plane in position (the one the
        // light's slope is read on).
        let (sxx, syy, sxy) = (xx - x * x, yy - y * y, xy - x * y);
        let det = sxx * syy - sxy * sxy;
        let (sxg, syg, sxd, syd) = (xg - x * g, yg - y * g, xd - x * d, yd - y * d);
        let cov = gd - g * d - (syy * sxg * sxd - sxy * (sxg * syd + syg * sxd) + sxx * syg * syd) / det;
        [cov, shared_noise(t1, held, held_u, n)]
    });
    let whole: Vec<usize> = (rho..cells.ch - rho).flat_map(|y| (rho..cells.cw - rho).map(move |x| y * cells.cw + x)).collect();
    let m = whole.len() as f64;
    (whole.iter().map(|&i| per[i][0]).sum::<f64>() / m, whole.iter().map(|&i| per[i][1]).sum::<f64>() / m)
}

/// The mean and standard error of `xs`.
fn mean_se(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let m = xs.iter().sum::<f64>() / n;
    (m, (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0) / n).sqrt())
}

/// The shared noise is witnessed on its own measurement: over [`SEEDS`] flat
/// frames at the default strength, the windows' measured covariance of the
/// read light with R − G against the shared noise the stage derives for them
/// (the cell's own share less the window's, [`Cells::shared_terms`]), within
/// three standard errors over the seeds; its distance from 0 in standard
/// errors printed. At the smallest levels in the slider's table (the guide
/// keeping most of its noise) the correction printed with what it moves a
/// read slope by.
///
/// MUTATION: the correction dropped (`shared_noise` 0): the derived side is
/// 0 against the measured (red); its plane terms dropped (`held_u` 0): the
/// derived side is off the measured (printed).
#[test]
fn the_shared_noise_is_the_measured_covariance() {
    let (rho, levels) = (at25(), lv25());
    let (mut got, mut want) = (Vec::new(), Vec::new());
    for seed in 0..SEEDS {
        let cells = cells_of(&scene(|_| 0.01, |_| 0.0, A, 100 + seed, true), None);
        let light = cells.read_light(levels, fair_k((cells.cw * cells.ch) as f64));
        let (m, d) = shared_read(&cells, &light, rho, levels);
        got.push(m);
        want.push(d);
    }
    let ((g, se), (d, dse)) = (mean_se(&got), mean_se(&want));
    println!("shared noise, half side {rho}, levels {levels}, {SEEDS} flat frames: measured {g:.4e} (se {se:.2e}), derived {d:.4e} (se {dse:.2e}); derived is {:.2} se from 0", d.abs() / se);
    let (rho1, l1) = (window(1.0, RHO_G_MAX).expect("on"), super::levels(1.0).expect("on"));
    let cells = cells_of(&scene(|_| 0.01, |_| 0.0, A, 100, true), None);
    let light = cells.read_light(l1, fair_k((cells.cw * cells.ch) as f64));
    let (m1, d1) = shared_read(&cells, &light, rho1, l1);
    let read = slopes_read(&cells, &light, rho1, l1);
    let r_mean = light.r.iter().map(|&x| f64::from(x)).sum::<f64>() / light.r.len() as f64;
    let sd_d = cell_var(0.01, A).sqrt();
    println!(
        "at colour NR 1 (half side {rho1}, levels {l1}): shared noise measured {m1:.4e}, derived {d1:.4e}; mean residual {r_mean:.4e}; windows read {} of {}; a read slope moves by kappa/(var + r) <= {:.4e} (in cell sds of R - G per light sd {:.4e})",
        read.iter().filter(|&&x| x).count(),
        read.len(),
        d1.abs() / r_mean,
        d1.abs() / r_mean * r_mean.sqrt() / sd_d
    );
    assert!((g - d).abs() <= 3.0 * se, "the shared noise measured {g:.4e} against the derived {d:.4e} (3 se {:.2e})", 3.0 * se);
}

/// The light's false reads are the law's: over [`SEEDS`] flat frames, the
/// light's detail coefficients read per level against the law's expectation
/// `C·(1 − Z(k_F)) = 1` (two tails, `C` the frame's cells, [`fair_k`]): the
/// mean over the seeds within three standard errors of at most 1.
#[test]
fn the_lights_false_reads_are_the_laws() {
    let levels = lv25();
    let mut per: Vec<Vec<f64>> = vec![Vec::new(); levels as usize];
    let mut c = 0.0;
    for seed in 0..SEEDS {
        let cells = cells_of(&scene(|_| 0.01, |_| 0.0, A, 100 + seed, true), None);
        c = (cells.cw * cells.ch) as f64;
        let light = cells.read_light(levels, fair_k(c));
        for (l, &n) in read_counts(&light, lv25()).iter().enumerate() {
            per[l].push(n as f64);
        }
    }
    let expected = c * (1.0 - normal_mass(fair_k(c)).0);
    let mut over = Vec::new();
    for (l, xs) in per.iter().enumerate() {
        let (m, se) = mean_se(xs);
        println!("light level {l}: false reads per frame over {SEEDS} flat frames {m:.3} (se {se:.3}); the law {expected:.3} (k_F {:.4}, C {c}); per seed {xs:?}", fair_k(c));
        if m - 3.0 * se > expected {
            over.push(format!("level {l}: {m:.3} (se {se:.3})"));
        }
    }
    assert!(over.is_empty(), "the light's false reads exceed the law's {expected:.3}: {over:?}");
}

/// A dark cell is read at its own light (S1's fixture): the left half of the
/// frame dark (green 0.01), the right bright (0.5), the scene's hue on R —
/// the colour difference `(hue − 1)·G`, a line through the light — sized as
/// S1 sized it (a step between the window's fair `k` of the dark cells' own
/// `σ_d` and of the frame's mean level's): the dark columns beside the edge
/// keep their own colour within 0.15 of the step.
///
/// MUTATION: `ε` at the frame's mean level instead of each window's own.
#[test]
fn a_dark_cell_is_read_at_its_own_light() {
    let (dark, bright) = (0.01, 0.5);
    let k = fair_k(49.0);
    let cell_sd = |g: f64, hue: f64| (WB[0] * WB[0] * (A * hue * g / WB[0] + B) + (A * g + B) / 4.0).sqrt();
    let mean_level = (dark + bright) / 2.0;
    let sd_d = |g: f64, hue: f64| 2f64.sqrt() * cell_sd(g, hue);
    let mut hue = 1.0;
    for _ in 0..20 {
        hue = 1.0 + k * (sd_d(dark, hue) * sd_d(mean_level, hue)).sqrt() / (bright - dark);
    }
    let step = (hue - 1.0) * (bright - dark);
    let e = W / 4 + PATCH / 2;
    let g = move |cx: usize| if cx < e { dark } else { bright };
    let d = move |cx: usize| (hue - 1.0) * g(cx);
    let cells = cells_of(&scene(g, d, A, 41, true), None);
    let out = cells.guided(at25(), lv25());
    let mut line = format!("dark cell: hue {hue:.4}, step {step:.4e}; err/step:");
    for cx in e - PATCH / 2..e {
        let got = colmean(&out, cx, 0);
        let want = (0..H / 2).map(|cy| f64::from(cells.d[cy * W / 2 + cx][0])).sum::<f64>() / (H / 2) as f64;
        line += &format!("  {cx}: {:+.4}", (got - want) / step);
        assert!((got - want).abs() < 0.15 * step, "column {cx} (edge at {e}): R - G {got:.4e} against its own {want:.4e} (step {step:.4e})");
    }
    println!("{line}");
}

/// An RGGB mosaic as [`scene`], with photon `a` and read noise `b` of its own.
fn scene_ab(g: impl Fn(usize) -> f64 + Sync, d: impl Fn(usize) -> f64 + Sync, a: f64, b: f64, seed: u64) -> Vec<f32> {
    let mut m = vec![0.0f32; W * H];
    m.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let gl = g(x / 2);
            let sv = match (y % 2, x % 2) {
                (0, 0) => (gl + d(x / 2)) / WB[0],
                (1, 1) => gl / WB[2],
                _ => gl,
            };
            *v = (sv + (a * sv + b).max(0.0).sqrt() * normal(seed, (y * W + x) as u64)) as f32;
        }
    });
    m
}

/// One cell's R − G sd at green `g` and colour difference `d` (photon `a`,
/// read `b`): the red site's own noise at its level and its four greens'.
fn cell_sd_at(g: f64, d: f64, a: f64, b: f64) -> f64 {
    (WB[0] * WB[0] * (a * (g + d) / WB[0] + b) + (a * g + b) / 4.0).sqrt()
}

/// The photon and read terms at which the module's mosaic has p38's traced
/// noise on its wall (rel1180 msg-e3 R25-6: green 0.0066, R − G +0.0025, the
/// light's sd 0.00033 a cell, the colour's 0.00094): one cell's green
/// variance `(a·G + b)/2` and its R − G variance [`cell_sd_at`], two linear
/// equations in `a`, `b`.
fn trace_noise() -> (f64, f64) {
    let (g, d, sl, sc) = (0.0066, 0.0025, 0.00033f64, 0.00094f64);
    // a·g + b = 2·sl², and wR·a·(g + d) + wR²·b + (a·g + b)/4 = sc².
    let p = 2.0 * sl * sl;
    let q = sc * sc - p / 4.0;
    let a = (q - WB[0] * WB[0] * p) / (WB[0] * (g + d) - WB[0] * WB[0] * g);
    (a, p - a * g)
}

/// Per cell and colour difference, the share of the cells read at each width
/// of [`ladder`] and kept as sampled (the last), over `cells`.
fn width_shares(used: &[[u8; 2]], cells: impl Iterator<Item = usize> + Clone, widths: usize, j: usize) -> Vec<f64> {
    let n = cells.clone().count() as f64;
    (0..=widths).map(|w| cells.clone().filter(|&i| used[i][j] as usize == w).count() as f64 / n).collect()
}

/// A window's residual and its null on a flat frame: over the whole windows
/// of a grid that do not overlap (so independent), the residual about each
/// window's plane less its null mean over its null sd ([`vote_null`], three
/// degrees taken) has
/// mean 0 and variance 1 within three standard errors — the measured
/// `var(R)` against the derived, the check the ruling names. Printed: the
/// derived and measured degrees of freedom `ν_d = 2·E[R]²/var(R)`, the null's
/// pieces (the shared green's correlation `c/s`), and the windows refused per
/// colour difference against the one-tailed law's expectation `C·Φ(−k_F)`.
#[test]
fn a_windows_residual_has_the_derived_null() {
    // Two flat levels a stop apart, so the fitted model carries the photon
    // term (a one-level frame fits a = 0 and states the red site's variance
    // at the green's level); the windows away from the step. Read at the
    // fixture's own model (the null's derivation alone) and at the model
    // fitted on the frame (with the fit's own error, traced).
    let e = W / 4;
    let m = scene(move |cx| if cx < e { 0.01 } else { 0.02 }, |_| 0.0, A, 211, true);
    let known = cells_of(&m, Some(NoiseModel::known(A, B, W, H)));
    let (mut failed, z0, _) = null_check(&known, e, "the fixture's model");
    // At the fitted model the null is the model's: off by the model's own
    // error at the cells' levels, `δ` (the fit's sampling error, Q26-5), so its
    // z mean moves by `−δ·E[R]/sd(R)` — asserted within three standard errors.
    let fitted = cells_of(&m, None);
    let (_, z1, se1) = null_check(&fitted, e, "the fitted model");
    let delta = |j: usize| {
        let (mut got, mut want) = (0.0, 0.0);
        for (i, q) in fitted.d.iter().enumerate() {
            let level = if i % fitted.cw < e { 0.01 } else { 0.02 };
            let w = [WB[0], WB[2]][j];
            got += fitted.own_noise(i).0[j].powi(2);
            want += w * w * (A * (level + f64::from(q[j])) / w + B) + (A * level + B) / 4.0;
        }
        got / want - 1.0
    };
    let rho = at25();
    let big_n = ((2 * rho + 1) * (2 * rho + 1)) as f64;
    let scale = (big_n - 3.0) / (2.0 * (big_n - 3.0)).sqrt();
    for j in 0..2 {
        let predicted = -delta(j) * scale;
        println!("the vote's null at the fitted model, colour difference {j}: the model's own error {:+.4} % of the cells' variance moves z by {predicted:+.4}; measured {:+.4} (the fixture's model {:+.4}, se {:.4})", 100.0 * delta(j), z1[j] - z0[j], z0[j], se1[j]);
        if (z1[j] - z0[j] - predicted).abs() > 3.0 * se1[j] {
            failed.push(format!("the fitted model, colour difference {j}: z moves {:+.4} against the model's error's {predicted:+.4}", z1[j] - z0[j]));
        }
    }
    assert!(failed.is_empty(), "the residual's null is not the derived one: {failed:?}");
}

/// The vote's null on `cells` (the two-level frame, its step at column `e`),
/// read and printed under `label`: the failures (see
/// [`a_windows_residual_has_the_derived_null`]) and per colour difference
/// the z mean and its standard error.
fn null_check(cells: &Cells, e: usize, label: &str) -> (Vec<String>, [f64; 2], [f64; 2]) {
    let (rho, levels) = (at25(), lv25());
    let light = cells.read_light(levels, fair_k((cells.cw * cells.ch) as f64));
    let noise = cells.noise();
    let lines = cells.lines(rho, levels, &light, &noise);
    let k = fair_k((cells.cw * cells.ch) as f64);
    let n_w = 2 * rho + 1;
    let stats: Vec<[f64; 6]> = box_means((cells.cw, cells.ch), rho, |i| {
        let [d0, d1, _] = cells.d[i].map(f64::from);
        let [s0, s1, c, _] = noise[i].map(f64::from);
        let (x, y) = cells.place(i);
        [d0, d1, d0 * d0, d1 * d1, s0, s1, s0 * s0, s1 * s1, c, x, y, x * x, y * y, x * y, x * d0, y * d0, x * d1, y * d1]
    }, |i, [d0, d1, dd0, dd1, s0, s1, q0, q1, c, x, y, xx, yy, xy, xd0, yd0, xd1, yd1], n| {
        let a = [lines[i][0], lines[i][5]].map(f64::from);
        // On a flat frame the light's slope of a whole window is read only at
        // its false reads: those windows are left out of the null's check.
        let p = if a[0] != 0.0 || a[1] != 0.0 { 0.0 } else { 1.0 };
        let (sxx, syy, sxy) = (xx - x * x, yy - y * y, xy - x * y);
        let r0 = about_plane(dd0 - d0 * d0, xd0 - x * d0, yd0 - y * d0, sxx, syy, sxy);
        let r1 = about_plane(dd1 - d1 * d1, xd1 - x * d1, yd1 - y * d1, sxx, syy, sxy);
        let (m0, sd0) = vote_null(s0, q0, c, n, 3.0);
        let (m1, sd1) = vote_null(s1, q1, c, n, 3.0);
        [p, (r0 - m0) / sd0, (r1 - m1) / sd1, m0, sd0, c / s0]
    });
    let grid: Vec<usize> = (rho..cells.ch - rho).step_by(n_w).flat_map(|y| (rho..cells.cw - rho).step_by(n_w).map(move |x| y * cells.cw + x)).filter(|&i| stats[i][0] == 1.0 && (i % cells.cw).abs_diff(e) > 2 * rho + 1).collect();
    // The trace (rel1180 RULING r26 Q26-5): the model's variance at each
    // site's level against the fixture's truth (`A·s + B`, `s` the site's
    // stored level), and per side the measured variance of each colour
    // difference against the mean of the cells' own noise the null reads.
    println!("the null's trace ({label}): model {:?}", cells.model);
    for (side, level) in [(0usize, 0.01f64), (1, 0.02)] {
        let r = cells.model.radius(if side == 0 { W as f64 / 4.0 } else { 3.0 * W as f64 / 4.0 }, H as f64 / 2.0);
        let line = [("green", level), ("red", level / WB[0]), ("blue", level / WB[2])]
            .map(|(name, s)| format!("{name} at {s:.5}: model {:.4e} truth {:.4e} ({:+.3} %)", cells.model.var(s, r), A * s + B, 100.0 * (cells.model.var(s, r) / (A * s + B) - 1.0)))
            .join("; ");
        let cols = if side == 0 { 2 * rho + 2..e - 2 * rho - 2 } else { e + 2 * rho + 2..cells.cw - 2 * rho - 2 };
        let ids: Vec<usize> = (2 * rho + 2..cells.ch - 2 * rho - 2).flat_map(|cy| cols.clone().map(move |cx| cy * cells.cw + cx)).collect();
        let m = ids.len() as f64;
        let per = [0, 1].map(|j| {
            let mean = ids.iter().map(|&i| f64::from(cells.d[i][j])).sum::<f64>() / m;
            let var = ids.iter().map(|&i| (f64::from(cells.d[i][j]) - mean).powi(2)).sum::<f64>() / (m - 1.0);
            let own = ids.iter().map(|&i| f64::from(noise[i][j])).sum::<f64>() / m;
            let w = [WB[0], WB[2]][j];
            let truth = w * w * (A * level / w + B) + (A * level + B) / 4.0;
            format!("{}: measured {var:.4e} (se {:.1e}), own_noise {own:.4e} ({:+.3} %), truth {truth:.4e} ({:+.3} %)", ["R-G", "B-G"][j], var * (2.0 / (m - 1.0)).sqrt(), 100.0 * (own / var - 1.0), 100.0 * (truth / var - 1.0))
        });
        println!("the null's trace ({label}), green {level}: {line}");
        println!("the null's trace ({label}), green {level}: per-cell variance {} | {}", per[0], per[1]);
    }
    let mut failed = Vec::new();
    let (mut zs, mut ses) = ([0.0; 2], [0.0; 2]);
    for j in 0..2 {
        let z: Vec<f64> = grid.iter().map(|&i| stats[i][1 + j]).collect();
        let (m, se) = mean_se(&z);
        (zs[j], ses[j]) = (m, se);
        let v = z.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (z.len() - 1) as f64;
        let v_se = (z.iter().map(|x| ((x - m).powi(2) - v).powi(2)).sum::<f64>() / (z.len() - 1) as f64 / z.len() as f64).sqrt();
        let i0 = grid[grid.len() / 2];
        let (mean, sd) = (stats[i0][3], stats[i0][4]);
        let refused = lines.iter().filter(|l| l[5 * j + 4] == 0.0).count();
        println!(
            "the vote's null ({label}), colour difference {j}, half side {rho}, {} independent windows: z mean {m:+.4} (se {se:.4}), variance {v:.4} (se {v_se:.4}); derived nu_d {:.2} (white {}), measured {:.2}; shared green c/s {:.4}; windows refused {refused} of {} against the one-tailed law {:.3}",
            z.len(),
            2.0 * mean * mean / (sd * sd),
            n_w * n_w - 3,
            2.0 * mean * mean / (sd * sd * v),
            stats[i0][5],
            lines.len(),
            lines.len() as f64 * (1.0 - normal_mass(k).0) / 2.0
        );
        if m.abs() > 3.0 * se || (v - 1.0).abs() > 3.0 * v_se {
            failed.push(format!("{label}, colour difference {j}: z mean {m:+.4} (se {se:.4}), variance {v:.4} (se {v_se:.4})"));
        }
    }
    (failed, zs, ses)
}

/// Three flat materials across a straight edge at p38's traced numbers (the
/// wall green 0.0066, R − G +0.0025 | a frame strip three cells wide, green
/// 0.018, +0.023 | the dark gap three cells wide, green 0.004, +0.0175 | the
/// banner, green 0.0095, +0.072; rel1180 msg-e3 R25-6) under the trace's noise
/// ([`trace_noise`]): every wall cell, at every distance from the edge, reads
/// within `k_F` of its own colour noise of the wall's colour, and every banner
/// cell of the banner's. The per-column error profile (column means, in cell
/// sds) printed as the trace printed it (the halo was +1.4 / +4.3 / +15.6 cell
/// sds 6 / 4 / 1 cells from the strip), with the share of the cells read at
/// each width and the voting windows whose slope is read beside the edge (the
/// light's noise is copied only through a read slope, so where every voting
/// window's slope is 0 the copied share is 0, [`copied_share`] with the
/// cell's vote count).
///
/// MUTATION: the vote test dropped (every window votes): the halo returns (red).
#[test]
fn three_materials_across_an_edge_are_each_read_at_their_own_colour() {
    let (a, b) = trace_noise();
    let e = W / 4;
    let g = move |cx: usize| if cx < e { 0.0066 } else if cx < e + 3 { 0.018 } else if cx < e + 6 { 0.004 } else { 0.0095 };
    let d = move |cx: usize| if cx < e { 0.0025 } else if cx < e + 3 { 0.023 } else if cx < e + 6 { 0.0175 } else { 0.072 };
    let cells = cells_of(&scene_ab(g, d, a, b, 91), None);
    let (rho, levels) = (at25(), lv25());
    let k = fair_k((cells.cw * cells.ch) as f64);
    let light = cells.read_light(levels, k);
    let (out, used) = cells.read_widths(rho, levels, &light, );
    let noise = cells.noise();
    let lines = cells.lines(rho, levels, &light, &noise);
    let (sw, sb) = (cell_sd_at(0.0066, 0.0025, a, b), cell_sd_at(0.0095, 0.072, a, b));
    println!("three materials: noise a {a:.3e} b {b:.3e}: wall light sd {:.5}, colour sd {sw:.5}; banner colour sd {sb:.5}; wall -> strip {:.1} light sds", ((a * 0.0066 + b) / 2.0).sqrt(), 0.0114 / ((a * 0.0066 + b) / 2.0).sqrt());
    let mut line = String::from("three materials: column from the strip: read - its colour (cell sds), voting windows with a read slope:");
    let mut failed = Vec::new();
    for cx in (e - 14..e).chain(e + 6..e + 20) {
        let (want, sd) = if cx < e { (0.0025, sw) } else { (0.072, sb) };
        let col: Vec<usize> = (0..cells.ch).map(|cy| cy * cells.cw + cx).collect();
        let mean = col.iter().map(|&i| f64::from(out[i][0])).sum::<f64>() / col.len() as f64;
        let worst = col.iter().map(|&i| (f64::from(out[i][0]) - want).abs()).fold(0.0, f64::max);
        let sloped = (0..cells.ch).flat_map(|cy| (cx.saturating_sub(rho)..=cx + rho).map(move |x| cy * cells.cw + x)).filter(|&i| lines[i][4] == 1.0 && lines[i][0] != 0.0).count();
        line += &format!("  {}: {:+.2} ({sloped})", cx as isize - e as isize, (mean - want) / sd);
        if worst > k * sd {
            failed.push(format!("column {}: worst {:.2} cell sds", cx as isize - e as isize, worst / sd));
        }
    }
    println!("{line}");
    let widths = ladder(rho, levels);
    let near = (0..cells.ch).flat_map(|cy| (e - 14..e + 20).map(move |cx| cy * cells.cw + cx));
    println!("three materials: ladder {widths:?}; share of the profile's cells read at each width, then kept as sampled: {:?}", width_shares(&used, near, widths.len(), 0).iter().map(|x| format!("{x:.4}")).collect::<Vec<_>>());
    assert!(failed.is_empty(), "a material's cell reads beyond k_F of its colour noise: {failed:?}");
}

/// The photon and read terms at which the module's mosaic has p40's traced
/// noise at the foliage edge under the sky (rel1180 msg-e3 R38-3: the darkest
/// rim cells' colour variance 2.9e-6 at green 0.0101, the sky's 9.2e-5 at
/// 0.33), for the colour differences `d_dark`, `d_sky` there: two linear
/// equations in `a`, `b` ([`cell_sd_at`]).
fn rim_noise(d_dark: f64, d_sky: f64) -> (f64, f64) {
    let (gd, vd, gs, vs) = (0.0101, 2.9e-6, 0.33, 9.2e-5);
    // v = a·(wR·(g + d) + g/4) + b·(wR² + 1/4).
    let (pd, ps) = (WB[0] * (gd + d_dark) + gd / 4.0, WB[0] * (gs + d_sky) + gs / 4.0);
    let a = (vs - vd) / (ps - pd);
    (a, (vd - a * pd) / (WB[0] * WB[0] + 0.25))
}

/// An RGGB mosaic whose cell `(cx, cy)` has the light `g` (green) and the
/// colour difference `d` (R − G, white-balanced), the blue grey, with photon
/// `a` and read `b` noise.
fn scene_xy(g: impl Fn(usize, usize) -> f64 + Sync, d: impl Fn(usize, usize) -> f64 + Sync, a: f64, b: f64, seed: u64) -> Vec<f32> {
    let mut m = vec![0.0f32; W * H];
    m.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let gl = g(x / 2, y / 2);
            let sv = match (y % 2, x % 2) {
                (0, 0) => (gl + d(x / 2, y / 2)) / WB[0],
                (1, 1) => gl / WB[2],
                _ => gl,
            };
            *v = (sv + (a * sv + b).max(0.0).sqrt() * normal(seed, (y * W + x) as u64)) as f32;
        }
    });
    m
}

/// A dark row under a bright sky, the rim's own geometry (rel1180 msg-e3
/// R38-1, RULING r39) at p40's traced rim noise ([`rim_noise`]): the sky's
/// light carries a texture (0.32 / 0.34 a column apart) and its colour
/// follows it on a line (R − G +0.25 at 0.33, +0.03 at the foliage's 0.0101);
/// every 48 rows a
/// dark row two cells deep, its light ~30 times lower (0.0101 / 0.0140 a
/// column apart: the foliage's spread), its variance ~30 times lower, and the
/// darkest cells' colour 20 of their own sds under the sky's line (the
/// others on it) — a colour that is not linear in light, small in the sky's
/// units. A window of the slider's width over the row holds two rows of 23
/// dark cells among 529: judged at the window's mean variance, which the sky
/// sets, the dark cells' misfit hides and the window's line reads them toward
/// the sky's line (the purple rim); judged per sample, each residual over its
/// own variance at the level the stage read for it, the window refuses.
/// Asserted: no window over the row votes for R − G, and every measured cell
/// of the rows beside it reads within `k_F` of its own noise of its own colour
/// (its site difference's: the four greens' mean holds a neighbour's light).
/// Printed: the windows over the row and how many vote, the dark row's error
/// per light (cell sds, mean and worst), the share read at each width.
///
/// MUTATION: the vote's null pooled (8b608fc's form: the fit unweighted, the
/// window's mean variance): windows over the row vote and its cells read
/// toward the sky's line by many of their own sds (red).
#[test]
fn a_window_over_a_dark_row_is_judged_at_each_sides_own_noise() {
    // The sky's line (R - G +0.25 at 0.33, +0.03 at the foliage's 0.0101);
    // the darkest cells sit 20 of their OWN sds under it (round 38: the rim
    // cell 17-33 own sds under the line, its colour near 0), the noise
    // p40's traced variances at the colours the cells have.
    let (d_sky, d_line) = (0.25, 0.03);
    let slope = (d_sky - d_line) / (0.33 - 0.0101);
    let line = move |g: f64| d_sky + slope * (g - 0.33);
    let (off, (a, b)) = (0..30).fold((0.0, (0.0, 0.0)), |(off, _), _| {
        let (a, b) = rim_noise(line(0.0101) + off, d_sky);
        (-20.0 * cell_sd_at(0.0101, line(0.0101) + off, a, b), (a, b))
    });
    let dark = |cy: usize| cy % 48 == 24 || cy % 48 == 25;
    let g = move |cx: usize, cy: usize| match (dark(cy), cx.is_multiple_of(2)) {
        (true, true) => 0.0101,
        (true, false) => 0.0140,
        (false, true) => 0.32,
        (false, false) => 0.34,
    };
    let d = move |cx: usize, cy: usize| line(g(cx, cy)) + if dark(cy) && cx.is_multiple_of(2) { off } else { 0.0 };
    let cells = cells_of(&scene_xy(g, d, a, b, 99), Some(NoiseModel::known(a, b, W, H)));
    let (rho, levels) = (at25(), lv25());
    let k = fair_k((cells.cw * cells.ch) as f64);
    let light = cells.read_light(levels, k);
    let (out, used) = cells.read_widths(rho, levels, &light);
    let lines = cells.lines(rho, levels, &light, &cells.noise());
    let (cw, ch) = (cells.cw, cells.ch);
    // Each material's own colour as its sites measure it (the four greens'
    // mean holds a neighbour's light): the mean of its measured differences.
    let class = move |cx: usize, cy: usize| usize::from(dark(cy)) * 2 + cx % 2;
    let mut own = [(0.0f64, 0usize); 4];
    for i in (0..cw * ch).filter(|&i| cells.meas[i][0]) {
        let o = &mut own[class(i % cw, i / cw)];
        (o.0, o.1) = (o.0 + f64::from(cells.d[i][0]), o.1 + 1);
    }
    let want = |cx: usize, cy: usize| {
        let o = own[class(cx, cy)];
        o.0 / o.1.max(1) as f64
    };
    let sd = |cx: usize, cy: usize| cell_sd_at(g(cx, cy), d(cx, cy), a, b);
    let rows: Vec<usize> = (24..ch - 24).filter(|cy| cy % 48 == 24).collect();
    let over: Vec<usize> = rows.iter().flat_map(|&r| (r + 1 - rho..=r + rho).flat_map(move |cy| (rho..cw - rho).map(move |cx| cy * cw + cx))).collect();
    let voted = over.iter().filter(|&&i| lines[i][4] == 1.0).count();
    let widths = ladder(rho, levels);
    let per_width: Vec<String> = widths
        .iter()
        .map(|&w| {
            let l = cells.lines(w, levels, &light, &cells.noise());
            let o: Vec<usize> = rows.iter().flat_map(|&r| (r + 1 - w..=r + w).flat_map(move |cy| (w..cw - w).map(move |cx| cy * cw + cx))).collect();
            format!("{w}: {} of {}", o.iter().filter(|&&i| l[i][4] == 1.0).count(), o.len())
        })
        .collect();
    println!("dark row: noise a {a:.3e} b {b:.3e}; colour sd dark {:.5} / {:.5}, sky {:.5}; off {off:.4} ({:.1} sky sds); k_F {k:.3}, half side {rho}", sd(0, 24), sd(1, 24), sd(0, 0), off / sd(0, 0));
    println!("dark row: windows over a dark row voting for R - G, per half side: {}", per_width.join(", "));
    let mut failed = Vec::new();
    if voted > 0 {
        failed.push(format!("{voted} of {} windows over the dark row vote", over.len()));
    }
    let mut stats = [(0.0f64, 0.0f64, 0usize); 2];
    let mut checked = 0;
    for &r in &rows {
        for cy in r - 4..r + 6 {
            for cx in 1..cw - 1 {
                let i = cy * cw + cx;
                if !cells.meas[i][0] {
                    continue;
                }
                checked += 1;
                let z = (f64::from(out[i][0]) - want(cx, cy)) / sd(cx, cy);
                if dark(cy) {
                    let s = &mut stats[cx % 2];
                    (s.0, s.1, s.2) = (s.0 + z, s.1.max(z.abs()), s.2 + 1);
                }
                if z.abs() > k {
                    failed.push(format!("cell ({cx}, {cy}): {z:+.2} own sds"));
                }
            }
        }
    }
    for (p, name) in [(0usize, "0.0101 (20 own sds under the line)"), (1, "0.0140 (on it)")] {
        let (sum, worst, n) = stats[p];
        println!("dark row: measured cells at light {name}: {n}, read - own colour mean {:+.2} worst {worst:.2} own sds", sum / n.max(1) as f64);
    }
    let near = rows.iter().flat_map(|&r| (r..r + 2).flat_map(move |cy| (0..cw).map(move |cx| cy * cw + cx))).collect::<Vec<_>>();
    println!("dark row: {checked} measured cells checked beside the rows; ladder {widths:?}; the dark rows' share read at each width, then kept as sampled: {:?}", width_shares(&used, near.iter().copied(), widths.len(), 0).iter().map(|x| format!("{x:.4}")).collect::<Vec<_>>());
    failed.truncate(12);
    assert!(failed.is_empty(), "a window over the dark row reads it beyond k_F of its own noise: {failed:?}");
}

/// A stroke three cells wide of a third colour inside the banner (the
/// letters): the banner (green 0.0095, R − G +0.072, the trace's noise) with
/// a stroke at the banner's light 12 of its colour sds off it (the light does
/// not carry it, so no line separates them). No window at the slider's width
/// holds the stroke alone; the smallest width's windows centred on its middle
/// column do, and the stroke's cells are read there — within `k_F` of their
/// noise of the stroke's colour (asserted per cell), keeping the share of
/// their noise the middle column's three windows keep, each its plane at the
/// cell ([`read_noise`]; asserted within three standard errors). The
/// stroke at 3 and 6 colour sds printed beside.
///
/// MUTATIONS: the fall-through dropped, the sample kept where the slider's
/// width refuses: the stroke keeps its whole noise (red); a box fallback
/// instead: the stroke reads the banner's colour (red).
#[test]
fn a_stroke_narrower_than_the_window_is_read_at_a_smaller_width() {
    let (a, b) = trace_noise();
    let e = W / 4;
    let sb = cell_sd_at(0.0095, 0.072, a, b);
    let (rho, levels) = (at25(), lv25());
    let widths = ladder(rho, levels);
    let derived = read_noise(1, &[(0, -1), (0, 0), (0, 1)], (0, 0));
    let mut failed = Vec::new();
    for off in [12.0f64, 3.0, 6.0] {
        let ds = 0.072 - off * sb;
        let d = move |cx: usize| if (e..e + 3).contains(&cx) { ds } else { 0.072 };
        let cells = cells_of(&scene_ab(|_| 0.0095, d, a, b, 93), None);
        let light = cells.read_light(levels, fair_k((cells.cw * cells.ch) as f64));
        let (out, used) = cells.read_widths(rho, levels, &light);
        let k = fair_k((cells.cw * cells.ch) as f64);
        let ss = cell_sd_at(0.0095, ds, a, b);
        let rows = 4..cells.ch - 4;
        let stroke = rows.clone().flat_map(|cy| (e..e + 3).map(move |cx| cy * W / 2 + cx));
        let errs: Vec<f64> = stroke.clone().map(|i| (f64::from(out[i][0]) - ds) / ss).collect();
        let (bias, _) = mean_se(&errs);
        let worst = errs.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        // The middle column, every fifth row: reads that share no window.
        let kept: Vec<f64> = rows.clone().step_by(5).map(|cy| ((f64::from(out[cy * W / 2 + e + 1][0]) - ds) / ss - bias).powi(2)).collect();
        let (left, se) = mean_se(&kept);
        println!(
            "stroke {off} banner colour sds off the banner (stroke sd {ss:.5}): bias {bias:+.3} cell sds, worst {worst:.2}; noise left {left:.4} (se {se:.4}) of a cell's variance, derived at the smallest width {derived:.4}; widths {widths:?} shares then kept: {:?}",
            width_shares(&used, stroke, widths.len(), 0).iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>()
        );
        if off == 12.0 && (worst > k || (left - derived).abs() > 3.0 * se) {
            failed.push(format!("{off} sd: worst {worst:.2} cell sds, noise left {left:.4} (se {se:.4}) against {derived:.4}"));
        }
    }
    assert!(failed.is_empty(), "the stroke is not read at the smallest width that holds it: {failed:?}");
}

/// The share of white noise a cell at `cell` keeps when it is read as the mean
/// of the planes of the windows of half side `rho` centred at `centres`
/// (offsets on the grid), each window's least-squares plane read at the cell.
fn read_noise(rho: usize, centres: &[(isize, isize)], cell: (isize, isize)) -> f64 {
    let r = rho as isize;
    let n = (2 * rho + 1) as f64;
    let s = (rho * (rho + 1) * (2 * rho + 1)) as f64 / 3.0;
    let mut w = std::collections::HashMap::new();
    for &(cx, cy) in centres {
        for jy in cy - r..=cy + r {
            for jx in cx - r..=cx + r {
                let h = 1.0 / (n * n) + ((cell.0 - cx) * (jx - cx)) as f64 / (n * s) + ((cell.1 - cy) * (jy - cy)) as f64 / (n * s);
                *w.entry((jx, jy)).or_insert(0.0) += h / centres.len() as f64;
            }
        }
    }
    w.values().map(|x| x * x).sum()
}

/// The step `Δ` (in the colour sds of the side a window holds the most of) at
/// which a window of half side `rho` with ONE column of the other side at its
/// edge is refused reliably: its expected `z` at `k + 3·sd_z` (rel1180 RULING
/// r26 Q26-2). Its residual's excess over the null is `SS·Δ²/n` of a cell's
/// variance, `SS` the share of the column's indicator its fit leaves —
/// `1 − 1/n − ρ²/S` under the plane (`S = Σu²`), `1 − 1/n` under the constant —
/// against the null's sd `√(2(N − p))/N`, and the excess carries its own
/// spread through the noise, `sd_z² = 1 + 4·n·SS·Δ²/(N²·sd²)`. Solved by
/// bisection.
fn edge_bar(rho: usize, k: f64, plane: bool) -> f64 {
    let n = (2 * rho + 1) as f64;
    let big_n = n * n;
    let s = (rho * (rho + 1) * (2 * rho + 1)) as f64 / 3.0;
    let (ss, p) = if plane { (1.0 - 1.0 / n - (rho * rho) as f64 / s, 3.0) } else { (1.0 - 1.0 / n, 1.0) };
    let sd0 = (2.0 * (big_n - p)).sqrt() / big_n;
    let gap = |d: f64| ss * d * d / (n * sd0) - k - 3.0 * (1.0 + 4.0 * n * ss * d * d / (big_n * big_n * sd0 * sd0)).sqrt();
    let (mut lo, mut hi) = (0.0f64, 100.0f64);
    for _ in 0..80 {
        let mid = (lo + hi) / 2.0;
        if gap(mid) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// An iso-luminant step (the light flat at 0.01, the module's noise) with the
/// colour stepping by 3, 6 and 12 colour sds (the flat side's): the cell
/// beside the edge is painted by the window holding it as its ONE foreign
/// column, which votes until the step reaches [`edge_bar`] in the colour sds of
/// the side it holds — the other side, whose colour site is brighter and
/// noisier (both bars printed, the constant's and the plane's). From there up
/// the edge is kept: the bleed beside it at most the noise of a cell read by
/// its own side's windows alone (the edge cell's, [`read_noise`]: the one
/// column of window centres wholly on its side), over the step (asserted);
/// under it the step is partly averaged into its neighbour (printed, the
/// known limit). The fixture is the round-25 record's (3 / 6 / 12).
#[test]
fn an_iso_luminant_step_is_kept_where_it_rises_above_the_windows_noise() {
    let rho = at25();
    let r = rho as isize;
    let k = fair_k((W / 2 * H / 2) as f64);
    let (bar, bar0) = (edge_bar(rho, k, true), edge_bar(rho, k, false));
    let centres: Vec<(isize, isize)> = (-r..=r).map(|cy| (-r, cy)).collect();
    let one_sided = read_noise(rho, &centres, (0, 0)).sqrt();
    let sd_l = cell_var(0.01, A).sqrt();
    println!("iso-luminant step: half side {rho}: the edge is kept above {bar:.3} colour sds of the painting side (the constant's {bar0:.3}); a cell beside it read by its own side's windows keeps {one_sided:.4} of its sd");
    let mut failed = Vec::new();
    for s in [3.0f64, 6.0, 12.0] {
        let step = s * sd_l;
        // The stepped side's R − G sd: its red site at (0.01 + step)/wR.
        let sd_r = (WB[0] * WB[0] * (A * (0.01 + step) / WB[0] + B) + (A * 0.01 + B) / 4.0).sqrt();
        let delta = step / sd_r;
        let (bleed, ratio, counts) = step_bleed(s, 0.01, |_| 0.01, 3);
        let limit = one_sided / s;
        let kept = delta >= bar;
        println!(
            "iso-luminant step {s} sd ({delta:.3} sds of the stepped side; {}): var_W(Ĝ)/ε {ratio:.2}; light read per level {counts:?}; err/step:{}; bar {limit:.4}",
            if kept { "kept, asserted" } else { "under the bar, printed" },
            bleed.iter().map(|(cx, b)| format!("  {cx}: {b:+.4}")).collect::<String>()
        );
        if kept {
            for (cx, b) in &bleed {
                if b.abs() > limit {
                    failed.push(format!("{s} sd, column {cx}: {b:+.4} against {limit:.4}"));
                }
            }
        }
    }
    assert!(failed.is_empty(), "the iso-luminant step bleeds beyond a one-sided read's noise where it rises above the window's: {failed:?}");
}

/// The light's false reads are the law's where the photon term is real: two
/// flat patches a stop apart (green 0.01 left, 0.02 right, the module's
/// noise), so the fitted model carries the level's slope (printed); over
/// [`SEEDS`] frames, the coefficients read per level away from the step
/// (beyond the composed kernel's half width of it) against the law's
/// `C'·(1 − Z(k_F))` over those cells, the mean within three standard errors
/// (the larger of the measured and the binomial: a read on a smooth level
/// flags a cluster of cells, so the count is over-dispersed) of at most it.
///
/// MUTATION: the light's noise read at the cell's own sample: level 0 reads
/// above the law (red).
#[test]
fn the_lights_false_reads_are_the_laws_at_two_levels() {
    let levels = lv25();
    let e = W / 4;
    let half = (composed_kernel(levels).len() - 1) / 2 + 1;
    let mut per: Vec<Vec<f64>> = vec![Vec::new(); levels as usize];
    let (mut c, mut model) = (0.0, None);
    for seed in 0..SEEDS {
        let cells = cells_of(&scene(move |cx| if cx < e { 0.01 } else { 0.02 }, |_| 0.0, A, 200 + seed, true), None);
        model = Some(cells.model);
        let light = cells.read_light(levels, fair_k((cells.cw * cells.ch) as f64));
        let away: Vec<usize> = (0..cells.cw * cells.ch).filter(|&i| (i % cells.cw).abs_diff(e) > half).collect();
        c = away.len() as f64;
        for (l, xs) in per.iter_mut().enumerate() {
            xs.push(away.iter().filter(|&&i| light.bits[i] & (1 << l) != 0).count() as f64);
        }
    }
    let k = fair_k((W / 2 * H / 2) as f64);
    let law = c * (1.0 - normal_mass(k).0);
    println!("two levels a stop apart: fitted model {:?}; {c} cells away from the step; the law {law:.3} a level (k_F {k:.4})", model.expect("seeds"));
    let mut over = Vec::new();
    for (l, xs) in per.iter().enumerate() {
        let (m, se) = mean_se(xs);
        let err = se.max((law / SEEDS as f64).sqrt());
        println!("two levels, light level {l}: false reads per frame {m:.3} (se {se:.3}, binomial {:.3}); per seed {xs:?}", (law / SEEDS as f64).sqrt());
        if m - 3.0 * err > law {
            over.push(format!("level {l}: {m:.3} (se {err:.3})"));
        }
    }
    assert!(over.is_empty(), "the light's false reads exceed the law's {law:.3} where the photon term is real: {over:?}");
}

/// (b), rel1180 RULING r20: the rebuild's slope is read where it is resolved
/// at the frame's fairness and is nothing otherwise, so on a flat frame it
/// adds no grain: the rebuilt pixels' chroma (the stage's own read at 25 and
/// its slope) against the same rebuild with every slope zero, the energy the
/// slope adds, `mean((c_b − c_0)²)`, within 3 se of the zero-slope grain's
/// own measurement (its spread over 16 blocks of the frame). Printed: the
/// cells whose slope is read, against the law's expected `2·Φ(−k_F)·C`
/// windows (each lights the cells around it: adjacent windows share all but
/// a column).
///
/// MUTATION: the test dropped (every slope read) → the slope's own noise,
/// `σ_a²·var(G_px − G_cell)`, is added to every pixel and the grain rises.
#[test]
fn a_flat_cells_slope_is_nothing_and_adds_no_grain() {
    let m = mosaic(0.01, 0.0);
    let cells = cells_of(&m, None);
    let rho = at25();
    let out = cells.guided(rho, lv25());
    let chroma = cells.chroma(&out, WB, rho);
    let n = cells.cw * cells.ch;
    let mut zero = cells.chroma(&out, WB, rho);
    zero.slopes = [vec![0.0; n], vec![0.0; n]];
    // A demosaic's green: the site's own at a green site, its four greens' mean at a colour site.
    let g = |x: usize, y: usize| -> f32 {
        if (x + y) % 2 == 1 {
            return m[y * W + x];
        }
        let at = |x: isize, y: isize| m[(y.clamp(0, H as isize - 1) as usize) * W + x.clamp(0, W as isize - 1) as usize];
        let (x, y) = (x as isize, y as isize);
        (at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1)) / 4.0
    };
    let base: Vec<[f32; 3]> = (0..W * H).map(|i| [0.0, g(i % W, i / W), 0.0]).collect();
    let (mut rb, mut r0) = (base.clone(), base);
    chroma.rebuild(&mut rb, W, 0, 0, GREEN_LUMA);
    zero.rebuild(&mut r0, W, 0, 0, GREEN_LUMA);
    let read: [usize; 2] = [0, 1].map(|j| chroma.slopes[j].iter().filter(|a| **a != 0.0).count());
    let k = fair_k(n as f64);
    let law = (1.0 - normal_mass(k).0) * n as f64;
    let r = 4 * rho + 4;
    let (xs, ys) = (r..W - r, r..H - r);
    for (j, c, w) in [(0usize, 0usize, WB[0]), (1, 2, WB[2])] {
        let chroma_of = |p: &[f32; 3]| w * f64::from(p[c]) - f64::from(p[1]);
        let mut added = 0.0;
        let mut count = 0.0;
        let mut blocks = vec![(0.0f64, 0.0f64, 0.0f64); 16];
        for y in ys.clone() {
            for x in xs.clone() {
                let i = y * W + x;
                let (b, z) = (chroma_of(&rb[i]), chroma_of(&r0[i]));
                added += (b - z) * (b - z);
                count += 1.0;
                let bi = 4 * ((y - r) * 4 / (H - 2 * r)) + (x - r) * 4 / (W - 2 * r);
                blocks[bi].0 += z;
                blocks[bi].1 += z * z;
                blocks[bi].2 += 1.0;
            }
        }
        let added = added / count;
        let vars: Vec<f64> = blocks.iter().map(|(s, q, n)| q / n - (s / n) * (s / n)).collect();
        let mean = vars.iter().sum::<f64>() / 16.0;
        let se = (vars.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / 15.0 / 16.0).sqrt();
        println!(
            "colour difference {j}: the zero-slope grain {mean:.4e} (se {se:.2e}), the slope adds {added:.4e}; cells read {} of {n} (the law's windows {law:.2})",
            read[j]
        );
        assert!(added <= 3.0 * se, "colour difference {j}: the slope adds {added:.4e} of grain on a flat frame, over 3 se {:.4e}", 3.0 * se);
    }
}

/// The star row (rel1180 RULING r29 / r30 / r31): the trace's own profile
/// through a big star (its cells' green: a flank falling ×1.3, ×2.25, ×4.24
/// into a clipped core and out again; numbers only), neutral, on a frame of
/// 96 × 64 cells under a known model, the light per pixel log-linear between
/// the cells' green centres, every site the light over its gain clipped at
/// the white.
fn star_row(seed: u64, noise: bool) -> (Vec<f32>, usize, usize) {
    star_profile(seed, noise, 1.0)
}

/// The star row's light times `scale` ([`star_row`] at 1).
fn star_profile(seed: u64, noise: bool, scale: f64) -> (Vec<f32>, usize, usize) {
    const CLIP: f64 = 1.0687;
    let (cw, ch) = (96usize, 64usize);
    let (w, h) = (2 * cw, 2 * ch);
    let left = [0.0346, 0.0455, 0.1025, 0.4345];
    let right = [0.7363, 0.1845, 0.0738, 0.0368];
    let c0 = 40usize;
    let mut cell: Vec<(f64, f64)> = left.iter().enumerate().map(|(i, g)| ((c0 + i) as f64, (g / CLIP).ln())).collect();
    cell.extend(right.iter().enumerate().map(|(i, g)| ((c0 + 10 + i) as f64, (g / CLIP).ln())));
    let (xl, xr) = ((c0 + 3) as f64, (c0 + 10) as f64);
    let (rl, rr) = (left[3] / left[2], right[0] / right[1]);
    let light = |x: usize| {
        let c = (x as f64 - 0.5) / 2.0;
        if c > xl && c < xr {
            return ((left[3] / CLIP) * rl.powf(c - xl)).min((right[0] / CLIP) * rr.powf(xr - c));
        }
        if c < cell[0].0 || c > cell[cell.len() - 1].0 {
            return 0.0346 / CLIP;
        }
        let k = cell.windows(2).position(|p| c >= p[0].0 && c <= p[1].0).expect("between two cells");
        let (a, b) = (cell[k], cell[k + 1]);
        (a.1 + (b.1 - a.1) * (c - a.0) / (b.0 - a.0)).exp()
    };
    let gain = [STAR_WB[0], 1.0, STAR_WB[2]];
    let mut m = vec![0.0f32; w * h];
    m.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let s = scale * light(x) / gain[match (y % 2, x % 2) { (0, 0) => 0, (1, 1) => 2, _ => 1 }];
            let n = if noise { (STAR_A * s + STAR_B).sqrt() * normal(seed, (y * w + x) as u64) } else { 0.0 };
            *v = (s + n).min(1.0) as f32;
        }
    });
    (m, w, h)
}
const STAR_WB: [f64; 3] = [2.87109375, 1.0, 1.44921875];
const STAR_A: f64 = 7e-4;
const STAR_B: f64 = 6e-6;

fn star_cells(m: &[f32], w: usize, h: usize) -> Cells {
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let pts = patches(m, w, h, sites, STAR_WB);
    Cells::of(m, w, h, sites, STAR_WB, &NoiseModel::of_terms(STAR_A, STAR_B, w, h), &pts).expect("cells")
}

/// A colour site is measured only where its four greens lie on one plane; a
/// clipped sample is not a measurement; where a site is not measured its
/// colour stays the develop's own (rel1180 RULING r30 / r31), on the star row:
/// (iii) every site not measured keeps the demosaic's colour difference
/// exactly (form B, Q31-4); the sites whose four greens are off one plane by
/// far more than their noise are not measured; (i) the first measured cells
/// beside the ring read within `k_F` of the truth at their own per-sample sd;
/// (ii) pooled over the frames the read is its measurement within 3 se, and
/// the measurement's offset against the truth is its derivation, the
/// four-green mean's residual curvature `r/4` (`r` the planarity residual; at
/// planarity `z` it is `z·sd(r)/(4·sd(d))` of the site's sd, ≈ 0.38·z at a
/// blue site and 0.28·z at a red on a neutral), printed per cell; and the
/// noise model per sample: each flank cell's measured spread of `d` over its
/// modelled sd within its se (Q31-2).
#[test]
fn a_stars_ring_is_not_measured_and_keeps_the_develops_colour() {
    const FRAMES: u64 = 20;
    let (m0, w, h) = star_row(0, false);
    let clean = star_cells(&m0, w, h);
    let (cw, ch) = (clean.cw, clean.ch);
    let k = fair_k((cw * ch) as f64);
    let (rho, lv) = (at25(), lv25());
    let rows = 16..ch - 16;
    let zone = 36..58usize;
    // The noiseless planarity z per cell and difference (the row is constant along y).
    let z_of = |m: &[f32], cx: usize, cy: usize, j: usize| {
        let (dy, dx) = [(0usize, 0usize), (1, 1)][j];
        let (x, y) = (2 * cx + dx, 2 * cy + dy);
        let g = [(x, y - 1), (x, y + 1), (x + 1, y), (x - 1, y)].map(|(a, b)| f64::from(m[b * w + a]));
        let v: f64 = g.iter().map(|&s| STAR_A * s.max(0.0) + STAR_B).sum();
        let r = (g[0] + g[1]) - (g[2] + g[3]);
        (r, v.sqrt())
    };
    let clipped = |m: &[f32], cx: usize, cy: usize, j: usize| {
        let (dy, dx) = [(0usize, 0usize), (1, 1)][j];
        let (x, y) = (2 * cx + dx, 2 * cy + dy);
        [(x, y), (x, y - 1), (x, y + 1), (x + 1, y), (x - 1, y)].iter().any(|&(a, b)| m[b * w + a] >= 1.0)
    };
    let mut refused = vec![[0usize; 2]; cw];
    let mut first: Vec<Vec<Vec<f64>>> = vec![vec![Vec::new(); cw]; 2]; // (f - d0)/sd per cell
    let mut truth: Vec<Vec<Vec<f64>>> = vec![vec![Vec::new(); cw]; 2]; // f/sd per cell
    let mut pooled = [Vec::new(), Vec::new()];
    let mut spread: Vec<Vec<[Vec<f64>; 2]>> = vec![vec![[Vec::new(), Vec::new()]; cw]; 2];
    let mut worst = [0.0f64; 2];
    let mut site_err = 0.0f64;
    for seed in 1..=FRAMES {
        let (m, ..) = star_row(seed, true);
        let cells = star_cells(&m, w, h);
        let f = cells.guided(rho, lv);
        let chroma = cells.chroma(&f, STAR_WB, rho);
        // (iii): a stand-in demosaic (the noisy light per pixel at the scene's
        // own colour) — at a site not measured the rebuild keeps it.
        let mut rgb: Vec<[f32; 3]> = (0..w * h).map(|i| { let g = m[((i / w) * w + (i % w)) ^ 1].max(0.0); [g / STAR_WB[0] as f32, m[i], g / STAR_WB[2] as f32] }).collect();
        let before = rgb.clone();
        chroma.rebuild(&mut rgb, w, 0, 0, GREEN_LUMA);
        let mut means = [Vec::new(), Vec::new()];
        for cy in rows.clone() {
            for j in 0..2 {
                let (dy, dx) = [(0usize, 0usize), (1, 1)][j];
                let un: Vec<usize> = zone.clone().filter(|&cx| !cells.meas[cy * cw + cx][j]).collect();
                for &cx in &un {
                    refused[cx][j] += 1;
                    let p = (2 * cy + dy) * w + 2 * cx + dx;
                    let c = |q: [f32; 3]| f64::from(if j == 0 { STAR_WB[0] as f32 * q[0] } else { STAR_WB[2] as f32 * q[2] } - q[1]);
                    site_err = site_err.max((c(rgb[p]) - c(before[p])).abs());
                }
                for cx in zone.clone() {
                    let i = cy * cw + cx;
                    if cells.meas[i][j] {
                        spread[j][cx][0].push(f64::from(cells.d[i][j]));
                        spread[j][cx][1].push(cells.own_noise(i).0[j]);
                    }
                }
                let (Some(&lo), Some(&hi)) = (un.first(), un.last()) else { continue };
                let near = (lo.saturating_sub(2)..lo).rev().chain(hi + 1..hi + 3).filter(|&cx| cells.meas[cy * cw + cx][j]);
                for cx in near {
                    let i = cy * cw + cx;
                    let sd = cells.own_noise(i).0[j];
                    let e = f64::from(f[i][j]) / sd;
                    // (i) reads the read against the cell's own measurement:
                    // the measurement's offset from the truth is the derived
                    // residual, asserted pooled below (rel1180 RULING r31 Q31-1;
                    // a site whose noiseless planarity is past k_F is measured
                    // on the rows its noise brings under it, carrying its whole
                    // offset, -2.47 sd at the blue site of cell 50).
                    let o = (f64::from(f[i][j]) - f64::from(clean.d[i][j])) / sd;
                    worst[j] = worst[j].max(o.abs());
                    first[j][cx].push(o);
                    truth[j][cx].push(e);
                    means[j].push(o);
                }
            }
        }
        for j in 0..2 {
            pooled[j].push(means[j].iter().sum::<f64>() / means[j].len() as f64);
        }
    }
    let n_rows = (rows.len() as u64 * FRAMES) as f64;
    assert!(site_err <= 1e-5, "(iii): a site not measured is written {site_err:.2e} off the demosaic's colour");
    for j in 0..2 {
        let name = ["R-G", "B-G"][j];
        // The noiseless ring: z, the predicted offset r/(4 sd) and its neutral
        // approximation, the measured offset against the truth, the refused share.
        let line: Vec<String> = zone
            .clone()
            .map(|cx| {
                let i = 32 * cw + cx;
                let (r, sdr) = z_of(&m0, cx, 32, j);
                let sd = clean.own_noise(i).0[j];
                let t = &truth[j][cx];
                let mean = if t.is_empty() { f64::NAN } else { t.iter().sum::<f64>() / t.len() as f64 };
                format!("{cx}: z {:+.1} pred {:+.3} (~{:+.2}) meas {mean:+.3} refused {:.2}", r / sdr, r / 4.0 / sd, [0.28, 0.38][j] * r / sdr, refused[cx][j] as f64 / n_rows)
            })
            .collect();
        println!("star row {name}: {}", line.join(" | "));
        // The sites far off one plane (noiseless |z| > k_F + 3) and not clipped are not measured.
        for cx in zone.clone() {
            let (r, sdr) = z_of(&m0, cx, 32, j);
            if (r / sdr).abs() > k + 3.0 && !clipped(&m0, cx, 32, j) {
                assert!(refused[cx][j] as f64 >= 0.99 * n_rows, "{name} cell {cx}: planarity z {:+.1} is measured on {} of {} rows", r / sdr, n_rows as usize - refused[cx][j], n_rows);
            }
        }
        assert!(worst[j] <= k, "{name} (i): a first measured cell reads {:.3} of its sd off its measurement (k_F {k:.3})", worst[j]);
        let p = &pooled[j];
        let (mean, se) = mean_se(p);
        println!("star row {name}: (ii) the first measured cells against their measurement, pooled over {FRAMES} frames: {mean:+.4} sd (se {se:.4}); (i) worst {:.3} (k_F {k:.3})", worst[j]);
        assert!(mean.abs() <= 3.0 * se, "{name} (ii): the read is {mean:+.4} sd off its measurement (3 se {:.4})", 3.0 * se);
        // The model per sample: measured spread over modelled, at cells measured on 90 % of rows.
        for cx in zone.clone() {
            let [d, sd] = &spread[j][cx];
            if (d.len() as f64) < 0.9 * n_rows {
                continue;
            }
            let n = d.len() as f64;
            let mean_d = d.iter().sum::<f64>() / n;
            let var = d.iter().map(|x| (x - mean_d).powi(2)).sum::<f64>() / (n - 1.0);
            let model = (sd.iter().map(|s| s * s).sum::<f64>() / n).sqrt();
            let ratio = var.sqrt() / model;
            let se = (1.0 / (2.0 * (n - 1.0))).sqrt();
            assert!((ratio - 1.0).abs() <= 3.0 * se, "{name} cell {cx}: the measured spread of d is {ratio:.3} of the modelled per-sample sd (3 se {:.3})", 3.0 * se);
        }
    }
}

/// A pixel's own green enters its colour only where its departure from the
/// cell is evidence (rel1180 RULING r34), on the sign's case: a flat neutral
/// field at the sign field's green (0.0225) holding a bright block whose
/// colour difference rises with its green at the letters' slope (5.9, round
/// 34), 16 cells square, inside the windows' reach of the field around it;
/// the mosaic through PPG (the develop's demosaic) and the rebuild. On the
/// field pixels the window reaches from the block (and 6 pixels clear of it)
/// the colour differences' pixel-scale sd (each pixel less its 5 × 5 mean) is
/// at most the demosaic's own on the same pixels; the block's edge pixels
/// at green sites (the pixel each side of each straight edge) keep the slope
/// term: their departure is over `k_F·σ`. Printed: both sds, the slope the
/// field reads, the share of field pixels whose departure is taken, and the
/// edge's colour sites (PPG's green there leans on the site's own bright
/// colour sample and lands near the cell read).
#[test]
fn a_flat_fields_pixels_take_no_slope_beside_a_bright_block() {
    use rawler::imgop::sensor::bayer::{ppg::PPGDemosaic, Demosaic};
    const FIELD: f64 = 0.0225;
    const BLOCK: f64 = 0.3;
    const SLOPE: f64 = 5.9;
    const N: usize = 256;
    let (c0, c1) = (56usize, 72usize);
    let model = NoiseModel::of_terms(A, B, N, N);
    let sites = bayer_sites(&rggb()).expect("Bayer");
    let rho = at25();
    let inside = |x: usize, y: usize| (2 * c0..2 * c1).contains(&x) && (2 * c0..2 * c1).contains(&y);
    let (mut before, mut after) = ([Vec::new(), Vec::new()], [Vec::new(), Vec::new()]);
    let (mut taken, mut field_n, mut edge_open, mut edge_n) = (0usize, 0usize, 0usize, 0usize);
    let (mut colour_open, mut colour_n, mut colour_z) = (0usize, 0usize, Vec::new());
    let mut slopes = Vec::new();
    for seed in 0..4u64 {
        let mut m = vec![0.0f32; N * N];
        m.par_chunks_mut(N).enumerate().for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let g = if inside(x, y) { BLOCK } else { FIELD };
                let s = match (y % 2, x % 2) {
                    (0, 0) => (g + SLOPE * (g - FIELD)) / WB[0],
                    (1, 1) => g / WB[2],
                    _ => g,
                };
                *v = (s + (A * s + B).sqrt() * normal(900 + seed, (y * N + x) as u64)) as f32;
            }
        });
        let pts = patches(&m, N, N, sites, WB);
        let cells = Cells::of(&m, N, N, sites, WB, &model, &pts).expect("cells");
        let f = cells.guided(rho, lv25());
        let chroma = cells.chroma(&f, WB, rho);
        let pixels = rawler::pixarray::PixF32::new_with(m.clone(), N, N);
        let mut rgb = PPGDemosaic::new().demosaic(&pixels, &rggb(), &rawler::cfa::PlaneColor::default(), pixels.rect()).data;
        let demosaic = rgb.clone();
        chroma.rebuild(&mut rgb, N, 0, 0, GREEN_LUMA);
        let diff = |q: &[[f32; 3]], j: usize| -> Vec<f64> { q.iter().map(|p| f64::from(if j == 0 { WB[0] as f32 * p[0] } else { WB[2] as f32 * p[2] } - p[1])).collect() };
        let high = |c: &[f64], x: usize, y: usize| {
            let mut s = 0.0;
            for yy in y - 2..=y + 2 {
                for xx in x - 2..=x + 2 {
                    s += c[yy * N + xx];
                }
            }
            c[y * N + x] - s / 25.0
        };
        let (reach, clear) = (2 * rho, 6usize);
        for j in 0..2 {
            let (cb, ca) = (diff(&demosaic, j), diff(&rgb, j));
            for y in 2 * c0 - reach..2 * c1 + reach {
                for x in 2 * c0 - reach..2 * c1 + reach {
                    if (2 * c0 - clear..2 * c1 + clear).contains(&x) && (2 * c0 - clear..2 * c1 + clear).contains(&y) {
                        continue;
                    }
                    before[j].push(high(&cb, x, y));
                    after[j].push(high(&ca, x, y));
                }
            }
        }
        let gate = |x: usize, y: usize| {
            let dg = demosaic[y * N + x][1] - crate::render::upsample_at(&chroma.green, chroma.cw, chroma.ch, 2, x, y);
            f64::from(dg).powi(2) > chroma.k * chroma.k * chroma.departure_var(x, y)
        };
        for y in 2 * c0 - reach..2 * c1 + reach {
            for x in 2 * c0 - reach..2 * c1 + reach {
                if (2 * c0 - clear..2 * c1 + clear).contains(&x) && (2 * c0 - clear..2 * c1 + clear).contains(&y) {
                    continue;
                }
                field_n += 1;
                taken += usize::from(gate(x, y));
                slopes.push(f64::from(crate::render::upsample_at(&chroma.slopes[0], chroma.cw, chroma.ch, 2, x, y)));
            }
        }
        // The block's straight edges, 2 pixels clear of its corners: the pixel each side.
        for t in 2 * c0 + 2..2 * c1 - 2 {
            for (x, y) in [(2 * c0 - 1, t), (2 * c0, t), (2 * c1 - 1, t), (2 * c1, t), (t, 2 * c0 - 1), (t, 2 * c0), (t, 2 * c1 - 1), (t, 2 * c1)] {
                // A green site's departure is its own sample's; PPG's green at
                // a colour site leans on that site's bright colour sample
                // (`X − X₂`) and lands near the cell read: printed.
                if (x + y) % 2 == 1 {
                    edge_n += 1;
                    edge_open += usize::from(gate(x, y));
                } else {
                    colour_n += 1;
                    colour_open += usize::from(gate(x, y));
                    let dg = demosaic[y * N + x][1] - crate::render::upsample_at(&chroma.green, chroma.cw, chroma.ch, 2, x, y);
                    colour_z.push(f64::from(dg).abs() / chroma.departure_var(x, y).sqrt());
                }
            }
        }
    }
    slopes.sort_by(f64::total_cmp);
    colour_z.sort_by(f64::total_cmp);
    let sd = |v: &[f64]| (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt();
    println!(
        "block beside a flat field: the field's slope R-G median {:.3}; field pixels whose departure is taken {taken} of {field_n}; the block's edge pixels at green sites {edge_open} of {edge_n}, at colour sites {colour_open} of {colour_n} (|departure| / sd median {:.2})",
        slopes[slopes.len() / 2],
        colour_z[colour_z.len() / 2]
    );
    for j in 0..2 {
        println!("  {}: the field's pixel-scale sd {:.4e} (the demosaic's own {:.4e})", ["R-G", "B-G"][j], sd(&after[j]), sd(&before[j]));
        assert!(sd(&after[j]) <= sd(&before[j]), "{}: the field's pixel-scale sd {:.4e} exceeds the demosaic's own {:.4e}", ["R-G", "B-G"][j], sd(&after[j]), sd(&before[j]));
    }
    assert_eq!(edge_open, edge_n, "the block's edge pixels at green sites: the departure is taken on {edge_open} of {edge_n}");
}

/// The planarity test's own witness (rel1180 RULING r33): the star row's
/// anisotropic profile scaled so its noiseless peak is half the white, so no
/// sample clips on any frame and every site the frame refuses is refused by
/// its planarity alone. The sites whose noiseless four greens are off one
/// plane by `|z| > k_F + 3` are not measured on 99 % of rows or more (the set
/// is not empty: under the clip it is the core's kink, the red site of cell
/// 47 at z +11.5; the exponential flank curves under `k_F + 3` at every
/// scale), and (iii) each keeps the demosaic's colour difference exactly. Printed: the colour difference the rebuild leaves at those sites
/// against the neutral truth, in their own sds and in 8-bit codes of the
/// white (the ring a read there would paint).
#[test]
fn a_bright_points_flank_under_the_clip_is_refused_by_its_planarity() {
    const FRAMES: u64 = 20;
    // The profile's unclipped peak, read at a scale far under the clip; the
    // row is then scaled to half the white.
    let probe = 1e-3;
    let (mp, w, h) = star_profile(0, false, probe);
    let top = mp.iter().map(|&v| f64::from(v)).fold(0.0, f64::max) / probe;
    let scale = 0.5 / top;
    let (m0, ..) = star_profile(0, false, scale);
    let clean = star_cells(&m0, w, h);
    let (cw, ch) = (clean.cw, clean.ch);
    let k = fair_k((cw * ch) as f64);
    let (rho, lv) = (at25(), lv25());
    let rows = 16..ch - 16;
    let z_of = |m: &[f32], cx: usize, cy: usize, j: usize| {
        let (dy, dx) = [(0usize, 0usize), (1, 1)][j];
        let (x, y) = (2 * cx + dx, 2 * cy + dy);
        let g = [(x, y - 1), (x, y + 1), (x + 1, y), (x - 1, y)].map(|(a, b)| f64::from(m[b * w + a]));
        let v: f64 = g.iter().map(|&s| STAR_A * s.max(0.0) + STAR_B).sum();
        ((g[0] + g[1]) - (g[2] + g[3])) / v.sqrt()
    };
    let set: [Vec<usize>; 2] = [0, 1].map(|j| (2..cw - 2).filter(|&cx| z_of(&m0, cx, 32, j).abs() > k + 3.0).collect());
    println!("under the clip: scale {scale:.6} (peak 0.5 of the white), k_F {k:.3}");
    for (j, name) in ["R-G", "B-G"].iter().enumerate() {
        println!("  {name}: sites of |z| > k_F + 3: {:?}", set[j].iter().map(|&cx| format!("{cx} z {:+.1}", z_of(&m0, cx, 32, j))).collect::<Vec<_>>());
    }
    assert!(!set[0].is_empty() || !set[1].is_empty(), "no site of the scaled row is off one plane by k_F + 3");
    let mut refused = vec![[0usize; 2]; cw];
    let (mut site_err, mut clipped) = (0.0f64, 0usize);
    let mut rim = [Vec::new(), Vec::new()];
    for seed in 1..=FRAMES {
        let (m, ..) = star_profile(seed, true, scale);
        clipped += m.iter().filter(|&&v| v >= 1.0).count();
        let cells = star_cells(&m, w, h);
        let f = cells.guided(rho, lv);
        let chroma = cells.chroma(&f, STAR_WB, rho);
        // (iii): the star row's stand-in demosaic (the noisy light per pixel
        // at the scene's own colour).
        let mut rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let g = m[((i / w) * w + (i % w)) ^ 1].max(0.0);
                [g / STAR_WB[0] as f32, m[i], g / STAR_WB[2] as f32]
            })
            .collect();
        let before = rgb.clone();
        chroma.rebuild(&mut rgb, w, 0, 0, GREEN_LUMA);
        for cy in rows.clone() {
            for j in 0..2 {
                let (dy, dx) = [(0usize, 0usize), (1, 1)][j];
                let c = |q: [f32; 3]| f64::from(if j == 0 { STAR_WB[0] as f32 * q[0] } else { STAR_WB[2] as f32 * q[2] } - q[1]);
                for &cx in &set[j] {
                    let i = cy * cw + cx;
                    let p = (2 * cy + dy) * w + 2 * cx + dx;
                    let sd = cells.own_noise(i).0[j];
                    rim[j].push((c(rgb[p]) / sd, c(rgb[p]) * 255.0));
                    if !cells.meas[i][j] {
                        refused[cx][j] += 1;
                        site_err = site_err.max((c(rgb[p]) - c(before[p])).abs());
                    }
                }
            }
        }
    }
    let n_rows = (rows.len() as u64 * FRAMES) as f64;
    for (j, name) in ["R-G", "B-G"].iter().enumerate() {
        let r = &rim[j];
        if r.is_empty() {
            continue;
        }
        let n = r.len() as f64;
        let mean = |f: fn(&(f64, f64)) -> f64| r.iter().map(f).sum::<f64>() / n;
        println!(
            "  {name}: the rebuilt colour difference at the set against the neutral truth: mean {:+.2} sd ({:+.1} codes), mean |.| {:.2} sd ({:.1} codes); refused shares {:?}",
            mean(|q| q.0),
            mean(|q| q.1),
            mean(|q| q.0.abs()),
            mean(|q| q.1.abs()),
            set[j].iter().map(|&cx| format!("{cx} {:.3}", refused[cx][j] as f64 / n_rows)).collect::<Vec<_>>()
        );
    }
    assert_eq!(clipped, 0, "a sample of the scaled row clips");
    assert!(site_err <= 1e-5, "(iii): a site not measured is written {site_err:.2e} off the demosaic's colour");
    for (j, name) in ["R-G", "B-G"].iter().enumerate() {
        for &cx in &set[j] {
            assert!(
                refused[cx][j] as f64 >= 0.99 * n_rows,
                "{name} cell {cx}: planarity z {:+.1} is measured on {} of {} rows",
                z_of(&m0, cx, 32, j),
                n_rows as usize - refused[cx][j],
                n_rows
            );
        }
    }
}

/// A clipped sample is not a measurement (rel1180 RULING r29 / r30 Q30-3 /
/// r31): on a flat frame with one red sample at the white in each of 35
/// clusters 64 cells apart (a window's reach is `2ρ + 1`), the windows holding
/// it fit and vote on their measured cells, so its eight neighbours keep the
/// voting share, `plane share·(1 + 4·ρ_G·τ)` (admitted, the windows holding
/// it are refused by their residual and the neighbours are read by the few
/// windows that do not hold it: a larger share).
#[test]
fn a_clipped_sample_is_not_measured_and_its_neighbours_keep_the_voting_share() {
    const LEVEL: f64 = 0.01;
    let (cw, ch) = (W / 2, H / 2);
    let rho = at25();
    let cents: Vec<(usize, usize)> = (48..ch - 47).step_by(64).flat_map(|y| (48..cw - 47).step_by(64).map(move |x| (x, y))).collect();
    let var = cell_var(LEVEL, A);
    let mut kept = Vec::new();
    for seed in 0..20u64 {
        let mut m = scene(|_| LEVEL, |_| 0.0, A, 700 + seed, true);
        for &(x, y) in &cents {
            m[2 * y * W + 2 * x] = 1.0;
        }
        let cells = cells_of(&m, Some(NoiseModel::of_terms(A, B, W, H)));
        let unmeasured = cents.iter().filter(|&&(x, y)| !cells.meas[y * cw + x][0]).count();
        if seed == 0 {
            println!("clipped cell: {unmeasured} of {} clipped sites not measured", cents.len());
        }
        let f = cells.guided(rho, lv25());
        for &(x, y) in &cents {
            let sq: f64 = (-1isize..=1).flat_map(|dy| (-1isize..=1).map(move |dx| (dx, dy))).filter(|&q| q != (0, 0)).map(|(dx, dy)| f64::from(f[(y as isize + dy) as usize * cw + (x as isize + dx) as usize][0]).powi(2)).sum();
            kept.push(sq / 8.0 / var);
        }
    }
    let (got, se) = mean_se(&kept);
    let p = plane_read(rho);
    let rho_g = (A * LEVEL + B) / 16.0 / var;
    let derived = p.share * (1.0 + 4.0 * rho_g * p.tau);
    println!("clipped cell: the neighbours keep {got:.5} (se {se:.5}) of a cell's variance against the derived voting share {derived:.6} (rho {rho}, rho_G {rho_g:.4})");
    assert!((got - derived).abs() <= 3.0 * se, "the clipped cell's neighbours keep {got:.5} against the voting share {derived:.6} (3 se {:.5})", 3.0 * se);
}

/// The window keeps the slider's promise on the frame it is applied to
/// (rel1180 RULING r27 Q27-4, r29 Q29-2): on a flat frame whose colour sites
/// sit dark under a bright green the shared-green share `ρ_G` exceeds the
/// neutral bound 1/20; the frame's own reading (the largest over its flat
/// patches) sets the window, at which the derived flat-frame share is at or
/// under the display pass's and the measured share is the derived.
#[test]
fn the_window_keeps_the_slider_on_a_frame_whose_shared_green_exceeds_the_bound() {
    const G: f64 = 0.5;
    const C: f64 = 0.002;
    let mut m = vec![0.0f32; W * H];
    m.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let s = match (y % 2, x % 2) { (0, 0) | (1, 1) => C, _ => G };
            *v = (s + (A * s + B).sqrt() * normal(901, (y * W + x) as u64)) as f32;
        }
    });
    // The fixture's own model (one green level: a fit could not separate its
    // terms); the rule under test is the window's, not the fit's. Its flat
    // patches are read in the fit's own band.
    let pts = patches(&m, W, H, bayer_sites(&rggb()).expect("Bayer"), WB);
    let mut model = NoiseModel::of_terms(A, B, W, H);
    model.band = NoiseModel::fit(&pts, W, H).expect("a fit").band;
    let reading = SourceReading::of(model, &pts, WB);
    let display = 4.0 / display_samples(DEFAULT_AMOUNT);
    let rho = window(DEFAULT_AMOUNT, reading.rho_g).expect("on");
    let p = plane_read(rho);
    // The fixture's own shared green per difference (its truth), the largest
    // the window's.
    let vg = A * G + B;
    let truth = [WB[0], WB[2]].map(|w| vg / 16.0 / (w * w * (A * C + B) + vg / 4.0));
    let derived = p.share * (1.0 + 4.0 * truth[0].max(truth[1]) * p.tau);
    let derived_r = p.share * (1.0 + 4.0 * truth[0] * p.tau);
    let cells = cells_of(&m, Some(reading.model));
    let f = cells.guided(rho, lv25());
    let (cw, ch) = (cells.cw, cells.ch);
    let m_ = 3 * rho;
    let ids: Vec<usize> = (m_..ch - m_).flat_map(|y| (m_..cw - m_).map(move |x| y * cw + x)).collect();
    let mean = ids.iter().map(|&i| f64::from(f[i][0])).sum::<f64>() / ids.len() as f64;
    let raw_mean = ids.iter().map(|&i| f64::from(cells.d[i][0])).sum::<f64>() / ids.len() as f64;
    let vd = ids.iter().map(|&i| (f64::from(cells.d[i][0]) - raw_mean).powi(2)).sum::<f64>() / ids.len() as f64;
    let share = ids.iter().map(|&i| (f64::from(f[i][0]) - mean).powi(2)).sum::<f64>() / ids.len() as f64 / vd;
    let at_bound = window(DEFAULT_AMOUNT, RHO_G_MAX).expect("on");
    let q = plane_read(at_bound);
    println!(
        "rho_G read off the frame {:.4} (median {:.4}; truth R {:.4} B {:.4}; bound {RHO_G_MAX}): window {rho}, derived share {derived:.6} (R {derived_r:.6}), measured R {share:.6}, display {display:.6}; at the bound's window {at_bound}: {:.6}",
        reading.rho_g,
        reading.rho_g_median,
        truth[0],
        truth[1],
        q.share * (1.0 + 4.0 * truth[0].max(truth[1]) * q.tau)
    );
    assert!(truth[0].max(truth[1]) > RHO_G_MAX, "the fixture's shared green exceeds the neutral bound");
    assert!(derived <= display, "the frame's window {rho} keeps {derived:.6} of a cell's noise, over the display pass's {display:.6}");
    // The measured share's error: a box of the read is correlated over its
    // reach; its se taken from the share over the frame's independent tiles.
    let tile = 4 * rho + 4;
    let tiles: Vec<f64> = (m_..ch - m_ - tile).step_by(tile).flat_map(|y| (m_..cw - m_ - tile).step_by(tile).map(move |x| (x, y))).map(|(x, y)| {
        let t: Vec<usize> = (y..y + tile).flat_map(|yy| (x..x + tile).map(move |xx| yy * cw + xx)).collect();
        t.iter().map(|&i| (f64::from(f[i][0]) - mean).powi(2)).sum::<f64>() / t.len() as f64 / vd
    }).collect();
    let (_, se) = mean_se(&tiles);
    assert!((share - derived_r).abs() <= 3.0 * se, "the measured share {share:.6} against the derived {derived_r:.6} (3 se {:.6})", 3.0 * se);
}
