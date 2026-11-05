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
    let (SourceReading { model }, _) = filter_mosaic(&mut after, W, H, &rggb(), WB, at25(), lv25(), None, 0, true).expect("a Bayer mosaic with noise fits a model");
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
    window(DEFAULT_AMOUNT).expect("on")
}

/// The light's levels the stage reads at the default strength.
fn lv25() -> u32 {
    levels(DEFAULT_AMOUNT).expect("on")
}

/// `cells` with its colour differences and light replaced by `f(i, [d_R, d_B, G])`.
fn with_d(cells: &Cells, f: impl Fn(usize, [f32; 3]) -> [f32; 3]) -> Cells {
    Cells { d: cells.d.iter().enumerate().map(|(i, q)| f(i, *q)).collect(), sd: cells.sd.clone(), cov: cells.cov.clone(), ..*cells }
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

/// The triangle of window overlaps along one axis at half side `rho`: the
/// number of windows holding two cells `k` apart, `n − |k|`.
fn triangle(rho: usize) -> Vec<f64> {
    let n = 2 * rho + 1;
    (0..2 * n - 1).map(|k| (n - (k as isize - (n as isize - 1)).unsigned_abs()) as f64).collect()
}

/// The slider reads the window the display pass leaves: at each strength the
/// smallest half side (at least one cell) at which the read leaves on a flat
/// frame no more than the display pass's share of a cell's noise,
/// `4 / display_samples` — [`flat_share`], from [`box_share`]: `Σ T²/N⁴`
/// over the windows' overlap counts, checked here by brute force. Printed
/// per strength with its share; 0 is off, above 100 as at 100.
#[test]
fn the_slider_reads_the_window_the_display_pass_leaves() {
    for rho in [1usize, 3, 6, 9] {
        let n = (2 * rho + 1) as f64;
        let brute = triangle(rho).iter().map(|x| x * x).sum::<f64>().powi(2) / n.powi(8);
        println!("half side {rho}: box share {:.9} (brute force {brute:.9}), flat share {:.9}", box_share(rho), flat_share(rho));
        assert!((box_share(rho) - brute).abs() < 1e-15);
    }
    assert_eq!(window(0.0), None, "0 is off");
    for nr in [1.0f32, 10.0, 20.0, 25.0, 50.0, 75.0, 100.0] {
        let rho = window(nr).expect("on");
        let display = 4.0 / display_samples(nr);
        println!("colour NR {nr}: display share of a cell's noise {display:.6}; half side {rho}, share {:.6} (box {:.6} + slope {:.6})", flat_share(rho), box_share(rho), 0.25 * box_share(rho));
        assert!(flat_share(rho) <= display || rho == 1, "colour NR {nr}: half side {rho} leaves {} > {display}", flat_share(rho));
        assert!(rho == 1 || flat_share(rho - 1) > display, "colour NR {nr}: half side {rho} is not the least");
    }
    assert_eq!(window(250.0), window(100.0));
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
/// box term with the shared green, `box_share·(1 + 4·ρ_G·τ)`; and the bound of
/// the false reads' share. A slope is read on a flat frame only where a window
/// reads beyond its null at `k_F` or where the light read a coefficient on
/// its noise; each read window adds at most `σ²_d/N` of slope energy over its
/// cells (`a = cov/(var + r̄)`, `E[cov²] = σ²_d·var/N`, so `E[a²·var] ≤ σ²_d/N`,
/// and a cell's slope term is the mean over at most `N` windows), so the share
/// is at most the expected read windows over `N·C`: `C·p` of the window's null
/// (`p` its Chernoff bound, [`false_window`]) and, per light level, the law's
/// one false read lifting the windows its detail kernel touches.
fn derived_flat(rho: usize, levels: u32, cells: (usize, usize), var: f64, shared: f64) -> (f64, f64) {
    let t = triangle(rho);
    let tau = t.windows(2).map(|w| w[0] * w[1]).sum::<f64>() / t.iter().map(|x| x * x).sum::<f64>();
    let s = box_share(rho);
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
/// evaluation: the population variance of white noise smoothed by the
/// composed kernel over a whole window, its mean and variance summed over the
/// window's cells' correlations (`E[Q] = r·(1 − Σρ/N²)`, `var(Q) = 2r²·Σ(ρ −
/// ρ̄_j − ρ̄_m + ρ̄)²/N²`), printed with the false read's bound per strength.
#[test]
fn a_windows_light_variance_has_the_derived_degrees_of_freedom() {
    for (rho, levels) in [(1usize, 1u32), (3, 2), (6, 3), (9, 3)] {
        let (nu, mu) = window_dof(rho, levels);
        let k = composed_kernel(levels);
        let auto = |d: usize| -> f64 { k.iter().zip(k.get(d..).unwrap_or(&[])).map(|(a, b)| a * b).sum() };
        let n = 2 * rho + 1;
        let cells: Vec<(usize, usize)> = (0..n * n).map(|j| (j % n, j / n)).collect();
        let rho_of = |a: (usize, usize), b: (usize, usize)| auto(a.0.abs_diff(b.0)) * auto(a.1.abs_diff(b.1)) / auto(0).powi(2);
        let big_n = (n * n) as f64;
        let row: Vec<f64> = cells.iter().map(|&a| cells.iter().map(|&b| rho_of(a, b)).sum::<f64>() / big_n).collect();
        let all = row.iter().sum::<f64>() / big_n;
        let mean = 1.0 - all;
        let mut var = 0.0;
        for (j, &a) in cells.iter().enumerate() {
            for (m, &b) in cells.iter().enumerate() {
                var += (rho_of(a, b) - row[j] - row[m] + all).powi(2);
            }
        }
        let var = 2.0 * var / (big_n * big_n);
        let direct = 2.0 * mean * mean / var;
        println!("half side {rho}, levels {levels}: nu {nu:.6} (direct {direct:.6}), mu {mu:.6} (direct {mean:.6})");
        assert!((nu - direct).abs() < 1e-9 * nu && (mu - mean).abs() < 1e-12, "half side {rho}, levels {levels}");
    }
    for nr in [1.0f32, 10.0, 20.0, 25.0, 50.0, 75.0, 100.0] {
        let (rho, lv) = (window(nr).expect("on"), levels(nr).expect("on"));
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
        println!("flat frame, half side {rho}, colour difference {j}: noise left {got:.6} (se {se:.6}) of a cell's variance; derived box {boxed:.6} + false reads <= {false_reads:.3e} (white box {:.6})", box_share(rho));
        assert!(got - want <= 3.0 * se + false_reads && want - got <= 3.0 * se, "colour difference {j}: the flat frame keeps {got:.6} of a cell's noise, derived {want:.6} + {false_reads:.3e} (3 se {:.6})", 3.0 * se);
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
        [g, g * g, f64::from(light.r[i])]
    }, |_, [g, gg, r], _| {
        let var = (gg - g * g).max(0.0);
        var + r > 0.0 && slope_read(var, r, k, nu)
    })
}

/// A cell's own noise kept is the derived share: on a flat frame the read's
/// regression on the cells' own samples is the box term's `1/N + 4·ρ_G·(n −
/// 1)/(n·N)` (its centre weight and its neighbours' through the shared green;
/// no slope is read but at the false reads, see the module), within three
/// standard errors.
#[test]
fn a_cells_own_noise_kept_is_the_derived_share() {
    let Flat { cells, out, interior, var, shared } = flat();
    let rho = at25();
    let n = (2 * rho + 1) as f64;
    let big_n = n * n;
    let support = (2.0 * n - 1.0).powi(2);
    for (j, &cv) in var.iter().enumerate() {
        let want = 1.0 / big_n + 4.0 * shared / cv * (n - 1.0) / (n * big_n);
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
        println!("own noise, half side {rho}, colour difference {j}: slope {slope:.6} (se {se:.6}); derived {want:.6} (box)");
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
/// and 1.5 cell sds a cell, the round-16 probe's slopes, and none) the read
/// is affine in the differences at a given light, so the read of the cells
/// less the read of the same cells less the noiseless ramp is the read of the
/// ramp alone. Under the light's noise the read light is flat to its noise,
/// no window's slope is read but at its false reads, and the ramp is the box
/// mean's: exact within f32 rounding at every cell no read window reaches
/// (asserted), and the cells a read window reaches no more than the law's
/// false reads reach in a whole frame — one a light level, whose detail
/// kernel `K_l` lifts the windows it touches and each of those the cells
/// within `ρ` of them, `Σ_l (len K_l + 2n − 2)²` (asserted; their worst
/// printed, the noise left there); under a noiseless light exact everywhere,
/// on the columns the read's whole reach lies on the ramp ([`reach`]).
///
/// MUTATION: the slope read everywhere (the rule dropped): the light's
/// smoothed noise draws a line through the ramp (red).
#[test]
fn a_ramp_under_a_flat_light_is_read_on_its_line() {
    let (x0, x1) = RAMP;
    let rho = at25();
    let r = reach(rho, lv25());
    let n_w = 2 * rho + 1;
    let law: usize = (1..=lv25()).map(|l| (composed_kernel(l).len() + 2 * n_w - 2).pow(2)).sum();
    let mut failed = Vec::new();
    for s in [0.0f64, 0.5, 1.0, 1.5] {
        let (m, cell_sd) = ramp_mosaic(s);
        let ramp = |cx: usize| (s * cell_sd * (cx.clamp(x0, x1) - x0) as f64) as f32;
        let cells = cells_of(&m, None);
        let cw = cells.cw;
        let less = with_d(&cells, |i, q| [q[0] - ramp(i % cw), q[1], q[2]]);
        let (a, b) = (cells.guided(rho, lv25()), less.guided(rho, lv25()));
        let flat_light = |c: &Cells| with_d(c, |_, q| [q[0], q[1], 0.01]);
        let (a0, b0) = (flat_light(&cells).guided(rho, lv25()), flat_light(&less).guided(rho, lv25()));
        let light = cells.read_light(lv25(), fair_k((cells.cw * cells.ch) as f64));
        let read = slopes_read(&cells, &light, rho, lv25());
        let reached = box_means((cw, cells.ch), rho, |i| [f64::from(u8::from(read[i]))], |_, [m], _| m > 0.0);
        let scale = (0..cells.d.len()).fold(0.0f32, |m, i| m.max(cells.d[i][0].abs()).max(ramp(i % cw).abs()).max(cells.d[i][2].abs()));
        let tol = 64.0 * f32::EPSILON * scale;
        let (mut worst, mut worst_reached, mut worst0, mut n_reached, mut n) = (0.0f32, 0.0f32, 0.0f32, 0usize, 0usize);
        for cy in r..cells.ch - r {
            for cx in x0 + r..=x1 - r {
                let i = cy * cw + cx;
                let e = (a[i][0] - b[i][0] - ramp(cx)).abs();
                if reached[i] {
                    n_reached += 1;
                    worst_reached = worst_reached.max(e);
                } else {
                    worst = worst.max(e);
                }
                worst0 = worst0.max((a0[i][0] - b0[i][0] - ramp(cx)).abs());
                n += 1;
            }
        }
        println!(
            "ramp {s} sd/cell, half side {rho}, reach {r}: under the light's noise worst {worst:.3e} on the {} cells no read slope reaches; {n_reached} cells reached (the law's false reads reach at most {law} a frame), worst {:.4} cell sds; noiseless light worst {worst0:.3e} (f32 rounding {tol:.3e}); windows read {}",
            n - n_reached,
            f64::from(worst_reached) / cell_sd,
            read.iter().filter(|&&x| x).count()
        );
        assert!(worst0 <= tol, "ramp {s} sd/cell under a noiseless light: the read moves the ramp by {worst0:e}, f32 rounding {tol:e}");
        if worst > tol {
            failed.push(format!("{s} sd/cell: {:.4} cell sds", f64::from(worst) / cell_sd));
        }
        if n_reached > law {
            failed.push(format!("{s} sd/cell: {n_reached} cells reached by a read slope, the law's false reads at most {law}"));
        }
    }
    assert!(failed.is_empty(), "under the light's noise the read moves the ramp beyond f32 rounding, or reads slopes beyond the law's false reads: {failed:?}");
}

/// The noise a ramp of the light copies into the colour, over `α²·v`: the
/// read light's noise `K_L·g` less what the windows' lines take of it — the
/// mean over the windows holding a cell of each window's least-squares line
/// on the light, which on a ramp in `x` is the line in `x` along the rows,
/// `h_x(0, j) = Σ_{c} (1 + (0 − c)·(j − c)/S_xx)/n²` (`c` the window's centre,
/// `S_xx = (n² − 1)/12`), and the box mean down the columns, `h_y = t/n²` —
/// `Σ (K⊗K − h_x·K ⊗ h_y·K)²`, separable, from the taps. With it, the line
/// read's own share of the colour's white noise, `Σ h_x²·Σ h_y²`.
fn copied_share(rho: usize, levels: u32) -> (f64, f64) {
    let k = composed_kernel(levels);
    let half = (k.len() / 2) as isize;
    let n = 2 * rho + 1;
    let rr = half + 2 * rho as isize + 2;
    let at = |x: isize| -> f64 { if x.abs() <= half { k[(x + half) as usize] } else { 0.0 } };
    let sxx = ((n * n - 1) as f64) / 12.0;
    let (mut hx, mut hy) = (vec![0.0f64; (2 * rr + 1) as usize], vec![0.0f64; (2 * rr + 1) as usize]);
    for c in -(rho as isize)..=rho as isize {
        for j in c - rho as isize..=c + rho as isize {
            hx[(j + rr) as usize] += (1.0 + (-c * (j - c)) as f64 / sxx) / (n * n) as f64;
            hy[(j + rr) as usize] += 1.0 / (n * n) as f64;
        }
    }
    let through = |h: &[f64]| -> Vec<f64> { (-rr..=rr).map(|m| (-rr..=rr).map(|j| h[(j + rr) as usize] * at(j - m)).sum()).collect() };
    let (hxk, hyk) = (through(&hx), through(&hy));
    let kk: Vec<f64> = (-rr..=rr).map(at).collect();
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let copied = dot(&kk, &kk).powi(2) - 2.0 * dot(&kk, &hxk) * dot(&kk, &hyk) + dot(&hxk, &hxk) * dot(&hyk, &hyk);
    (copied, dot(&hx, &hx) * dot(&hy, &hy))
}

/// A ramp of light and colour together (the light rising by `s` of its own
/// sds a cell and R − G by `s` of its own over [`RAMP`], read-noise
/// dominated), on the columns the read's whole reach lies on the ramp
/// ([`reach`]): noiseless, the read is the ramp exactly (f32 rounding); under
/// the noise, the read less the read of the cells less the noiseless ramp is
/// the colour ramp read through the noisy light: the light's noise copied at
/// the line's slope, `α²·v·Σ(K − h·K)²` ([`copied_share`]) of a cell's
/// variance — asserted within three standard errors (blocks of rows of twice
/// the reach). The total noise kept (the read less the noiseless cells' read,
/// which carries the colour site's own green offset on a light ramp) printed
/// against the line read's share of
/// the colour's noise and the copied share; the false light read's speck,
/// `k_F·σ_{w_0}·α`, printed in cell sds.
///
/// MUTATION: the guide = the raw light: the light's noise is copied whole
/// (0.41–0.95 of a cell's variance, red).
#[test]
fn a_ramp_of_light_and_colour_together_is_read_exactly() {
    const A2: f64 = 2e-6;
    let (x0, x1) = RAMP;
    let rho = at25();
    let r = reach(rho, lv25());
    let level = 0.01;
    let (sd, sd_g) = (cell_var(level, A2).sqrt(), ((A2 * level + B) / 2.0).sqrt());
    let (copied, line) = copied_share(rho, lv25());
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
        let truth = |cx: usize| d(cx) as f32;
        let less = with_d(&noisy, |i, q| [q[0] - truth(i % cw), q[1], q[2]]);
        let (a, b, c) = (noisy.guided(rho, lv25()), less.guided(rho, lv25()), clean.guided(rho, lv25()));
        let scale = (0..cw * noisy.ch).fold(0.0f32, |m, i| m.max(noisy.d[i][0].abs()).max(noisy.d[i][2].abs()));
        let tol = 64.0 * f32::EPSILON * scale;
        let (mut worst, mut worst0) = (0.0f32, 0.0f32);
        let (mut blocks, mut kept) = (Vec::new(), (0.0f64, 0.0f64));
        let rows: Vec<usize> = (r..noisy.ch - r).collect();
        for chunk in rows.chunks(2 * r).filter(|c| c.len() == 2 * r) {
            let (mut ss, mut m) = (0.0f64, 0.0f64);
            for &cy in chunk {
                for cx in x0 + r..=x1 - r {
                    let i = cy * cw + cx;
                    let e = a[i][0] - b[i][0] - truth(cx);
                    worst = worst.max(e.abs());
                    worst0 = worst0.max((c[i][0] - clean.d[i][0]).abs());
                    ss += f64::from(e).powi(2);
                    kept.0 += f64::from(a[i][0] - c[i][0]).powi(2);
                    kept.1 += 1.0;
                    m += 1.0;
                }
            }
            blocks.push(ss / m / (sd * sd));
        }
        let nb = blocks.len() as f64;
        let got = blocks.iter().sum::<f64>() / nb;
        let se = (blocks.iter().map(|x| (x - got).powi(2)).sum::<f64>() / (nb - 1.0) / nb).sqrt();
        let alpha = sd / sd_g;
        let want = alpha * alpha * sd_g * sd_g * copied / (sd * sd);
        println!(
            "light and colour ramp {s} sd/cell, half side {rho}, reach {r}: noiseless worst {worst0:.3e} (f32 rounding {tol:.3e}); under the noise the light's noise copied {got:.6} (se {se:.6}) of a cell's variance, derived {want:.6}; worst {:.4} cell sds; noise kept {:.6} (line read {line:.6} + copied); a false light read's speck k_F sigma_w0 alpha = {:.3} cell sds",
            f64::from(worst) / sd,
            kept.0 / kept.1 / (sd * sd),
            fair_k((cw * noisy.ch) as f64) * s_w0 * sd_g * alpha / sd
        );
        assert!(worst0 <= tol, "ramp {s}: the noiseless read moves the ramp by {worst0:e}, f32 rounding {tol:e}");
        if (got - want).abs() > 3.0 * se {
            failed.push(format!("{s} sd/cell: copied {got:.6} (se {se:.6}), derived {want:.6}"));
        }
    }
    assert!(failed.is_empty(), "the light's noise copied is not the derived share: {failed:?}");
}

/// A curved band carried by the light: the round-21 fixture (R − G rising by
/// 16 cell sds through a logistic 16 cells wide, read-noise dominated) with
/// the light following it (16 of its own sds through the same logistic, as a
/// cloud's luma follows its colour): at the two knees the read's mean over the
/// rows less the band's is within the measurement's error (the standard error
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
            let bias = fm - band(cx);
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
    let bleed = (e - 6..e + 6).map(|cx| (cx, (colmean(&out, cx, 0) - if cx < e { lo } else { hi }) / step)).collect();
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
/// measured covariance of the read light with R − G, and the shared noise the
/// stage derives for it ([`shared_noise`]); their means over the whole
/// windows.
fn shared_read(cells: &Cells, light: &Light, rho: usize, levels: u32) -> (f64, f64) {
    let mass = window_mass(rho, levels);
    let per: Vec<[f64; 2]> = box_means((cells.cw, cells.ch), rho, |i| {
        let g = f64::from(light.g[i]);
        let d = f64::from(cells.d[i][0]);
        let [t1, held] = cells.shared_terms(light, i, levels, &mass);
        [g, d, g * d, t1, held]
    }, |_, [g, d, gd, t1, held], n| [gd - g * d, shared_noise(t1, held, n)]);
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
/// 0 against the measured (red).
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
    let (rho1, l1) = (window(1.0).expect("on"), super::levels(1.0).expect("on"));
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
