//! A mask's colour gains as the Temp, Tint and Exposure a sidecar can carry
//! (v1.17.0): Lightroom has no key for a mask's colour gains, so the writer
//! spells them as the nearest move of the mask's own white balance, found on
//! the develop's own output.

use super::*;

/// A mask's colour gains carried by its Temp and Tint alone: what a sidecar
/// writes for a mask whose gains Lightroom has no key for.
pub(crate) struct GainsAsWhite {
    /// Temp and Tint, each −1..1 (the sidecar's `crs:LocalTemperature` /
    /// `crs:LocalTint` scale).
    pub(crate) temp: f32,
    pub(crate) tint: f32,
    /// EV added to the mask's Exposure so the brightness the white move
    /// carries is taken back.
    pub(crate) exposure_ev: f32,
    /// Largest Oklab distance (×100) left on the greys: how near the
    /// projection is, which only the tests read (the writer has no use for a
    /// number it cannot act on; the clamp below is what it discloses).
    #[cfg_attr(not(test), expect(dead_code, reason = "the projection's measured error, read by its tests"))]
    pub(crate) residual: f32,
    /// The best Temp or Tint sits at the end of its range: the gains move
    /// more than the white balance can.
    pub(crate) clamped: bool,
}

/// Greys the projection compares on: `i / GREYS` for `i` in `1..GREYS`, each
/// a `BLOCK`-pixel square so a spatial stage reads a flat patch at its centre.
const GREYS: usize = 16;
const BLOCK: usize = 16;
/// Gauss-Newton steps at most; it stops sooner when a step moves nothing.
const LM_ITERATIONS: usize = 20;

/// `m`'s colour gains, on top of its own Temp, Tint and Exposure, projected
/// onto Temp, Tint and Exposure alone; `None` when the gains are
/// neutral.
///
/// The develop renders a mask's Temp and Tint and its colour gains as two
/// different moves; a sidecar has keys only for the first. So the two are compared where the user sees them: on the develop's
/// own output of `r` for a strip of input greys under the mask at full
/// weight (the colour field, the spot removal and the other masks left out —
/// they are the same on both sides). The target is the mask as the app
/// renders it; a candidate drops the gains and sets Temp, Tint and the
/// Exposure offset. The search is the whole Temp/Tint range on a
/// coarse grid, then a compass search on all three down to the writer's
/// precision; its distance is Oklab over the greys' centres.
pub(crate) fn gains_as_white(r: &EditRecipe, m: &crate::recipe::LocalAdjustment) -> Option<GainsAsWhite> {
    m.color_gains.filter(|g| *g != [1.0; 3])?;
    let (w, h) = ((GREYS - 1) * BLOCK, BLOCK);
    let strip: Vec<[f32; 3]> =
        (0..w * h).map(|i| [((i % w) / BLOCK + 1) as f32 / GREYS as f32; 3]).collect();
    let whole = crate::recipe::LocalAdjustment {
        // A gradient with no length covers the whole frame (`mask_weight`).
        mask: crate::recipe::MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.0, full_x: 0.0, full_y: 0.0 },
        components: Vec::new(),
        range: Default::default(),
        inverted: false,
        enabled: true,
        amount: 1.0,
        ..m.clone()
    };
    let base = EditRecipe { masks: Vec::new(), colour_field: None, retouch: Vec::new(), ..r.clone() };
    let rasters = MaskRasterSnapshot::default();
    let shown = |mask: crate::recipe::LocalAdjustment| -> Vec<[f32; 3]> {
        let mut data = strip.clone();
        let rr = EditRecipe { masks: vec![mask], ..base.clone() };
        apply_develop_with_rasters(&mut data, w, h, &rr, &rasters, MaskFrame::AsRendered, FilmScale::NATIVE, false);
        (0..GREYS - 1)
            .map(|k| super::masks::linear_to_oklab(data[(h / 2) * w + k * BLOCK + BLOCK / 2].map(srgb_to_linear)))
            .collect()
    };
    let want = shown(whole.clone());
    // One candidate (Temp, Tint, EV offset): its Oklab distances, summed in
    // square and at most (×100).
    // One candidate (Temp, Tint, EV offset): its Oklab differences from the
    // target, grey by grey.
    let clamp = |c: [f32; 3]| [c[0].clamp(-1.0, 1.0), c[1].clamp(-1.0, 1.0), c[2]];
    let diff = |c: [f32; 3]| -> Vec<f32> {
        let got = shown(crate::recipe::LocalAdjustment {
            color_gains: None,
            temperature: c[0] * 100.0,
            tint: c[1] * 100.0,
            exposure_ev: whole.exposure_ev + c[2],
            ..whole.clone()
        });
        got.iter().zip(&want).flat_map(|(a, b)| (0..3).map(move |i| a[i] - b[i])).collect()
    };
    let cost = |d: &[f32]| d.iter().map(|v| v * v).sum::<f32>();
    // Seeded on a coarse grid over the whole Temp/Tint range (so the seed
    // picks the quadrant of a move that differs by sign) …
    let mut best = ([0.0f32; 3], f32::INFINITY, Vec::new());
    for i in 0..=4 {
        for j in 0..=4 {
            let c = [-1.0 + 0.5 * i as f32, -1.0 + 0.5 * j as f32, 0.0];
            let d = diff(c);
            let s = cost(&d);
            if s < best.1 {
                best = (c, s, d);
            }
        }
    }
    // … then damped Gauss-Newton on all three, the Jacobian by forward
    // differences of the writer's precision, until a step moves nothing the
    // writer would print.
    const H: f32 = 0.005;
    let mut damping = 1e-3f32;
    for _ in 0..LM_ITERATIONS {
        let jac: Vec<Vec<f32>> = (0..3)
            .map(|k| {
                let mut c = best.0;
                c[k] += if k < 2 && c[k] + H > 1.0 { -H } else { H };
                let step = c[k] - best.0[k];
                diff(c).iter().zip(&best.2).map(|(a, b)| (a - b) / step).collect()
            })
            .collect();
        let mut jtj = [[0.0f32; 3]; 3];
        let mut jtr = [0.0f32; 3];
        for a in 0..3 {
            jtr[a] = jac[a].iter().zip(&best.2).map(|(x, r)| x * r).sum();
            for b in 0..3 {
                jtj[a][b] = jac[a].iter().zip(&jac[b]).map(|(x, y)| x * y).sum();
            }
        }
        let mut accepted = false;
        while damping < 1e6 {
            let mut m = jtj;
            for (a, row) in m.iter_mut().enumerate() {
                row[a] += damping * jtj[a][a].max(1e-9);
            }
            let delta = mat_vec3(&inv3(&m), &jtr);
            let c = clamp([best.0[0] - delta[0], best.0[1] - delta[1], best.0[2] - delta[2]]);
            let d = diff(c);
            let s = cost(&d);
            if s < best.1 {
                let moved = (0..3).map(|k| (c[k] - best.0[k]).abs()).fold(0.0, f32::max);
                best = (c, s, d);
                damping = (damping * 0.1).max(1e-6);
                accepted = moved >= H * 0.5;
                break;
            }
            damping *= 10.0;
        }
        if !accepted {
            break;
        }
    }
    let [temp, tint, ev] = best.0;
    let residual = best.2.chunks(3).map(|d| d.iter().map(|v| v * v).sum::<f32>().sqrt() * 100.0).fold(0.0, f32::max);
    Some(GainsAsWhite { temp, tint, exposure_ev: ev, residual, clamped: temp.abs() >= 1.0 || tint.abs() >= 1.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gains a Temp and Tint render are projected back onto that Temp and
    /// Tint; gains past the law's reach are carried at the end of its range
    /// and say so; neutral gains are no projection.
    ///
    /// MUTATION: leave the mask's own Temp out of the target, drop the EV
    /// offset from the candidate, or stop the search at the coarse grid.
    #[test]
    fn colour_gains_are_carried_by_the_masks_temp_and_tint() {
        let r = EditRecipe { as_shot_k: Some(5200.0), as_shot_tint: Some(4.0), ..EditRecipe::default() };
        // The gains the develop shows on a middle grey for a mask at
        // Temp +30, Tint −20, luminance kept.
        let grey = |m: Option<crate::recipe::LocalAdjustment>| {
            let mut px = vec![[0.5f32; 3]; 16];
            let rr = EditRecipe { masks: m.into_iter().collect(), ..r.clone() };
            apply_develop_with_rasters(
                &mut px, 4, 4, &rr, &MaskRasterSnapshot::default(), MaskFrame::AsRendered,
                FilmScale::NATIVE, false,
            );
            px[5].map(srgb_to_linear)
        };
        let whole = |temperature: f32, tint: f32| crate::recipe::LocalAdjustment {
            mask: crate::recipe::MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.0, full_x: 0.0, full_y: 0.0 },
            temperature,
            tint,
            ..Default::default()
        };
        let (base, moved) = (grey(None), grey(Some(whole(30.0, -20.0))));
        let y = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        let k = y(base) / y(moved);
        let gains: [f32; 3] = std::array::from_fn(|c| moved[c] * k / base[c]);
        let mut m = crate::recipe::LocalAdjustment { color_gains: Some(gains), ..whole(0.0, 0.0) };
        let p = gains_as_white(&r, &m).expect("gains to carry");
        // Measured: (+0.403, −0.269), EV −0.159, 2.5 left. The gains are one
        // ratio on every grey and the law's move is that ratio only on the
        // middle one, so the nearest move over the strip is a little larger.
        assert!(!p.clamped && (p.temp - 0.3).abs() < 0.15 && (p.tint + 0.2).abs() < 0.1, "{} {}", p.temp, p.tint);
        assert!(p.exposure_ev < 0.0, "the warm move brightens and is taken back: {}", p.exposure_ev);
        // On top of the mask's own cooler Temp, the same gains ask less.
        m.temperature = -20.0;
        let q = gains_as_white(&r, &m).expect("gains to carry");
        assert!(!q.clamped && q.temp < p.temp - 0.1, "{} vs {}", q.temp, p.temp);
        // Far past the law: the end of Temp's range, named, and further off.
        m.temperature = 0.0;
        m.color_gains = Some([3.0, 1.0, 0.2]);
        let f = gains_as_white(&r, &m).expect("gains to carry");
        assert!(f.clamped && f.temp == 1.0 && f.residual > p.residual, "{} {}", f.temp, f.residual);
        m.color_gains = Some([1.0; 3]);
        assert!(gains_as_white(&r, &m).is_none());
        m.color_gains = None;
        assert!(gains_as_white(&r, &m).is_none());
    }
}
