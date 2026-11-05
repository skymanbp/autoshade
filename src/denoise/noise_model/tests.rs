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

const W: usize = 2048;
const H: usize = 1536;
const WB: [f64; 3] = [2.5, 1.0, 1.6];

/// The truth the mosaic is drawn from: photon 2e-4 and read 4e-6 (sd 0.002) of
/// full scale, a radial gain of 1.88 at the corner (the night pair's, msg-e3
/// round 3), a neutral scene whose level ramps across the frame (so level and
/// radius are not the same variable), and a textured block whose two greens
/// and colour differ site by site.
fn truth() -> NoiseModel {
    NoiseModel { a: 2e-4, b: 4e-6, c2: 0.7, c4: 0.17, band: 0.0, width: W, height: H }
}

fn textured(x: usize, y: usize) -> bool {
    (1300..1700).contains(&x) && (200..600).contains(&y)
}

fn mosaic() -> Vec<f32> {
    let t = truth();
    let sites = [(0usize, 0usize), (0, 1), (1, 0), (1, 1)]; // RGGB
    let mut out = vec![0.0f32; W * H];
    out.par_chunks_mut(W).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let colour = sites.iter().position(|s| *s == (y % 2, x % 2)).unwrap();
            let s_grey = 0.002 + 0.12 * (x as f64 / W as f64).powi(2) + 0.01 * (y as f64 / H as f64);
            let mut s = match colour {
                0 => s_grey / WB[0],
                3 => s_grey / WB[2],
                _ => s_grey,
            };
            if textured(x, y) {
                s *= 1.0 + 0.6 * normal(7, (y * W + x) as u64).clamp(-1.5, 1.5);
            }
            let g = t.gain(t.radius(x as f64 + 0.5, y as f64 + 0.5));
            let noisy = s + (t.a * s + t.b).sqrt() * normal(1, (y * W + x) as u64);
            *v = (g * noisy) as f32;
        }
    });
    out
}

fn fitted() -> (NoiseModel, Vec<Patch>) {
    let rggb = rawler::cfa::CFA::new("RGGB");
    let sites = bayer_sites(&rggb).expect("RGGB is a 2x2 Bayer");
    let p = patches(&mosaic(), W, H, sites, WB);
    (NoiseModel::fit(&p, W, H).expect("the synthetic mosaic fits"), p)
}

/// The noise model is measured, not assumed: drawn from photon + read noise
/// under a radial gain, the fit on the mosaic's own patches gives back the
/// variance surface it was drawn from (level × radius), and it does so on the
/// noise-only patches — the textured block is outside the band. The tolerance
/// is ten times the fit's own sampling error: 3072 patches of 255 degrees of
/// freedom estimate a log-variance to 0.089 each, four parameters from them to
/// about 0.089 / sqrt(3072 / 4) = 0.3 %.
///
/// MUTATION: fix the gain at 1 (`c2 = c4 = 0` in `refined`) and the surface
/// misses the corners by the gain (1.88): this fails.
#[test]
fn the_noise_model_is_read_off_the_mosaic_with_the_cameras_gain() {
    let (m, p) = fitted();
    let t = truth();
    for level in [0.001, 0.01, 0.05, 0.1] {
        for r in [0.0, 0.3, 0.6, 0.9] {
            let (got, want) = (m.var(level, r), t.var(level, r));
            assert!((got / want - 1.0).abs() < 0.03, "level {level} r {r}: {got:.3e} vs {want:.3e} ({m:?})");
        }
    }
    let tex: Vec<&Patch> = p
        .iter()
        .enumerate()
        .filter(|(k, _)| {
            let (cx, cy) = ((k % (W / 2 / PATCH)) * PATCH * 2, (k / (W / 2 / PATCH)) * PATCH * 2);
            textured(cx, cy) && textured(cx + 2 * PATCH - 1, cy + 2 * PATCH - 1)
        })
        .map(|(_, p)| p)
        .collect();
    assert!(tex.len() >= 100, "the fixture holds a textured block: {}", tex.len());
    assert!(tex.iter().all(|q| m.residual(q) > m.band), "texture only adds variance: every textured patch is above the band");
}

/// The flat rule: a patch is flat when its colour-difference variances are the
/// ones the noise model predicts, within the fit's own band — so the
/// noise-only ground is flat (all but its sampling tail) and the textured
/// block is not, by the same band the fit drew from the green residuals. The
/// band itself is the MAP edge (`f(u) < 2·f(−u)`): on this fixture it sits
/// outside the noise-only residuals' sampling spread (0.089), not inside it.
///
/// MUTATION: compare the colour variances against the green model alone
/// (drop the `wR²·var(m_R)` term of `chroma_var`) and no patch is flat.
#[test]
fn a_patch_is_flat_when_its_colour_noise_is_the_models() {
    let (m, p) = fitted();
    assert!(m.band > 0.089 && m.band < 1.0, "band {}", m.band);
    let pw = W / 2 / PATCH;
    let (mut ground, mut ground_flat, mut tex, mut tex_flat) = (0, 0, 0, 0);
    for (k, q) in p.iter().enumerate() {
        let (cx, cy) = ((k % pw) * PATCH * 2, (k / pw) * PATCH * 2);
        let inside = textured(cx, cy) && textured(cx + 2 * PATCH - 1, cy + 2 * PATCH - 1);
        let outside = !textured(cx, cy) && !textured(cx + 2 * PATCH - 1, cy + 2 * PATCH - 1)
            && !textured(cx + 2 * PATCH - 1, cy) && !textured(cx, cy + 2 * PATCH - 1);
        if inside {
            tex += 1;
            tex_flat += usize::from(m.is_flat(q, WB));
        } else if outside {
            ground += 1;
            ground_flat += usize::from(m.is_flat(q, WB));
        }
    }
    assert_eq!(tex_flat, 0, "no textured patch is flat ({tex} of them)");
    assert!(ground_flat as f64 >= 0.95 * ground as f64, "noise-only ground is flat: {ground_flat} of {ground}");
}

/// The fit reads a uniform stride of a large frame's patches, the stride that
/// keeps at least [`FIT_PATCHES`] of them, and its variance surface stays
/// within the error the patch size leaves room for (0.0434 in `ln` variance)
/// of the fit on every patch. Patches drawn from the model with the 8-cell
/// estimate's own spread (0.2523 in `ln`), 4.2 times [`FIT_PATCHES`] (so the
/// stride is 4); a small frame is read whole.
///
/// MUTATION: read every patch (stride 1) and the first assertion fails.
#[test]
fn the_fit_reads_a_stride_that_keeps_its_surface_inside_the_patch_rule() {
    let t = truth();
    let n = FIT_PATCHES * 42 / 10;
    let drawn: Vec<Patch> = (0..n as u64)
        .map(|i| {
            let m = 0.002 + 0.12 * (0.5 + 0.5 * normal(11, i).tanh());
            let r = 0.5 + 0.5 * normal(13, i).tanh();
            Patch { m, m_r: m, m_b: m, v_g: t.var(m, r) * (0.2523 * normal(17, i)).exp(), v_dr: 0.0, v_db: 0.0, r, usable: true }
        })
        .collect();
    let stride = n / FIT_PATCHES;
    assert_eq!(stride, 4);
    let sampled: Vec<Patch> = drawn.iter().step_by(stride).copied().collect();
    let fit = NoiseModel::fit(&drawn, W, H).expect("a model");
    assert_eq!(Some(fit), NoiseModel::fit_all(&sampled, W, H), "the fit is the fit on the uniform stride");
    let all = NoiseModel::fit_all(&drawn, W, H).expect("a model");
    for level in [0.002, 0.01, 0.05, 0.1] {
        for r in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let e = (fit.var(level, r) / all.var(level, r)).ln().abs();
            assert!(e < 0.0434, "level {level} r {r}: |ln var| {e:.4} against the all-patch fit");
        }
    }
    let small: Vec<Patch> = drawn[..FIT_PATCHES - 1].to_vec();
    assert_eq!(NoiseModel::fit(&small, W, H), NoiseModel::fit_all(&small, W, H), "a small frame is read whole");
}

/// The fit is not low by the log of its variance estimates: a patch's green
/// variance is a sample variance of `PATCH² − 1` degrees of freedom, whose log
/// sits below the truth's by [`log_bias`] in the mean (−0.016 at 63), and a
/// least-squares fit of the logs would carry it whole (rel1180 RULING r26
/// Q26-5: the two-level fixture's model −1.3 / −2.1 % at its two greens).
/// Drawn: patches over level and radius whose `v_g` is the truth's variance
/// times `χ²_k/k` (the sum of `k` squared normals). The fitted surface's mean
/// log error over the drawn patches is within three standard errors of 0 —
/// `√ψ′(k/2)/√n`, `ψ′(x) ≈ 1/x + 1/(2x²) + 1/(6x³)` the log's own spread.
///
/// MUTATION: the correction dropped (`residual` without `log_bias`): the mean
/// reads −0.016, about eleven standard errors (red).
#[test]
fn the_fit_is_not_low_by_the_log_of_its_variance_estimates() {
    let t = truth();
    let n = FIT_PATCHES - 1;
    let k = PATCH * PATCH - 1;
    let drawn: Vec<Patch> = (0..n as u64)
        .map(|i| {
            let m = 0.002 + 0.12 * (0.5 + 0.5 * normal(21, i).tanh());
            let r = 0.5 + 0.5 * normal(23, i).tanh();
            let chi: f64 = (0..k as u64).map(|j| normal(29, i * k as u64 + j).powi(2)).sum();
            Patch { m, m_r: m, m_b: m, v_g: t.var(m, r) * chi / k as f64, v_dr: 0.0, v_db: 0.0, r, usable: true }
        })
        .collect();
    let fit = NoiseModel::fit(&drawn, W, H).expect("a model");
    let mean = drawn.iter().map(|p| (fit.var(p.m, p.r) / t.var(p.m, p.r)).ln()).sum::<f64>() / n as f64;
    let x = k as f64 / 2.0;
    let se = (1.0 / x + 1.0 / (2.0 * x * x) + 1.0 / (6.0 * x * x * x)).sqrt() / (n as f64).sqrt();
    println!("fit over {n} drawn patches of {k} degrees: mean ln(fit / truth) {mean:+.5} (se {se:.5}); log bias {:+.6}; {fit:?}", log_bias(k as f64));
    for level in [0.01, 0.05, 0.1] {
        println!("  level {level}: ln(fit / truth) at r 0.25 / 0.5 / 0.75: {}", [0.25, 0.5, 0.75].map(|r| format!("{:+.4}", (fit.var(level, r) / t.var(level, r)).ln())).join(" / "));
    }
    assert!(mean.abs() <= 3.0 * se, "the fitted surface is {mean:+.5} off the truth in the log's mean (3 se {:.5})", 3.0 * se);
}

/// A pattern that is not a 2×2 RGB Bayer has no two greens in a cell to read
/// the noise from: the estimator says so instead of guessing.
#[test]
fn only_a_two_by_two_bayer_cell_carries_the_estimator() {
    assert_eq!(bayer_sites(&rawler::cfa::CFA::new("RGGB")), Some([(0, 0), (0, 1), (1, 0), (1, 1)]));
    assert_eq!(bayer_sites(&rawler::cfa::CFA::new("GBRG")), Some([(1, 0), (0, 0), (1, 1), (0, 1)]));
    assert!(bayer_sites(&rawler::cfa::CFA::new("RGBE")).is_none());
}
