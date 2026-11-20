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
///
/// The answer is on the scale `r` states its masks' Temp and Tint on
/// (`EditRecipe::mask_wb_era`), because the develop it searches with reads
/// them on that scale (`mask_wb_gains`, 2026-11-20). One recipe reaches here
/// before its numbers were carried over: the payload of a sidecar v1.18.3 or
/// an earlier build wrote, projected again by `xmp::payload::restore` to
/// learn what that build's document said. Its mask's own Temp and Tint and
/// every candidate are then that build's numbers under that build's law, so
/// the search visits the points that build's search visited and ends where it
/// ended — the three numbers in the sidecar, to the bit.
///
/// To the bit is what it takes. The search stops once a step moves less than
/// half its probe (`H`), short of the writer's printed precision, so its
/// answer belongs to its path, and searching on today's scale to restate the
/// answer afterwards does not arrive: a few ten-thousandths to a third of a
/// slider unit apart where the gains fit that build's range, and whole slider
/// units where they reach the end of it — that build's Temp stopped at its
/// ±100, today's runs on. Either way the sidecar reader would take the
/// difference for an edit made in Lightroom
/// (`an_older_builds_projection_is_found_again_on_its_own_scale` and the
/// measurement beside it have the sizes).
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
        apply_develop_with_rasters(&mut data, w, h, &rr, &rasters, MaskFrame::AsRendered, FilmScale::NATIVE, false, false);
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

/// v1.18.3's mask white balance and its sidecar projection, kept as that
/// build had them, so a test can state what that build rendered and wrote
/// without asking today's law or today's search.
#[cfg(test)]
pub(crate) mod v1_18_3 {
    use super::*;
    use crate::recipe::LocalAdjustment;

    const GREYS: usize = 16;
    const BLOCK: usize = 16;
    const LM_ITERATIONS: usize = 20;
    const H: f32 = 0.005;

    /// The gains a mask's Temp and Tint rendered: ∓80 mired at full scale
    /// around 5500 K, the Tint into the shared model as it stood.
    pub(crate) fn mask_wb_gains(temperature: f32, tint: f32) -> [f32; 3] {
        let mired = 1e6 / 5500.0 - (temperature.clamp(-100.0, 100.0) / 100.0) * 80.0;
        wb_gains(5500.0, 1e6 / mired, tint)
    }

    /// `m` as that build rendered it, for today's develop to draw: its Temp
    /// and Tint folded into its colour gains by the law above — the product
    /// that build's mask stage compiled into one table — so today's mask law
    /// has no part in the picture. Only the guard on a pixel blown in every
    /// channel can tell the two apart (gains leave it white, a Temp colours
    /// it), so the frames compared here stay under the clip.
    pub(crate) fn as_rendered(m: &LocalAdjustment) -> LocalAdjustment {
        let own = (m.temperature != 0.0 || m.tint != 0.0).then(|| mask_wb_gains(m.temperature, m.tint));
        let folded = match (own, m.color_gains) {
            (None, gains) => gains,
            (Some(g), gains) => {
                let c = gains.unwrap_or([1.0; 3]);
                Some([g[0] * c[0], g[1] * c[1], g[2] * c[2]])
            }
        };
        LocalAdjustment { temperature: 0.0, tint: 0.0, color_gains: folded, ..m.clone() }
    }

    /// A small frame of greys and colours, all under the clip, developed by `r`.
    pub(crate) fn developed(r: &EditRecipe) -> Vec<[f32; 3]> {
        let (w, h) = (24usize, 8usize);
        let mut px: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / (w - 1) as f32, (i / w) as f32 / (h - 1) as f32);
                [0.06 + 0.8 * x, 0.1 + 0.6 * (1.0 - x) * y + 0.15 * x, 0.12 + 0.7 * y]
            })
            .collect();
        super::super::develop::apply_develop_anon(&mut px, w, h, r);
        px
    }

    /// `r`, a recipe of that build, as that build developed it.
    pub(crate) fn developed_then(r: &EditRecipe) -> Vec<[f32; 3]> {
        developed(&EditRecipe { masks: r.masks.iter().map(as_rendered).collect(), ..r.clone() })
    }

    /// The largest channel difference between two developed frames.
    pub(crate) fn worst(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
        a.iter().zip(b).flat_map(|(x, y)| (0..3).map(move |c| (x[c] - y[c]).abs())).fold(0.0, f32::max)
    }

    /// The Temp, Tint (each −1..1) and Exposure offset that build wrote into
    /// a sidecar for `m`'s colour gains: `gains_as_white` as it stood, law and
    /// search, candidate by candidate.
    pub(crate) fn projection(r: &EditRecipe, m: &LocalAdjustment) -> Option<[f32; 3]> {
        m.color_gains.filter(|g| *g != [1.0; 3])?;
        let (w, h) = ((GREYS - 1) * BLOCK, BLOCK);
        let strip: Vec<[f32; 3]> =
            (0..w * h).map(|i| [((i % w) / BLOCK + 1) as f32 / GREYS as f32; 3]).collect();
        let whole = LocalAdjustment {
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
        let shown = |mask: LocalAdjustment| -> Vec<[f32; 3]> {
            let mut data = strip.clone();
            let rr = EditRecipe { masks: vec![as_rendered(&mask)], ..base.clone() };
            apply_develop_with_rasters(&mut data, w, h, &rr, &rasters, MaskFrame::AsRendered, FilmScale::NATIVE, false, false);
            (0..GREYS - 1)
                .map(|k| super::super::masks::linear_to_oklab(data[(h / 2) * w + k * BLOCK + BLOCK / 2].map(srgb_to_linear)))
                .collect()
        };
        let want = shown(whole.clone());
        let clamp = |c: [f32; 3]| [c[0].clamp(-1.0, 1.0), c[1].clamp(-1.0, 1.0), c[2]];
        let diff = |c: [f32; 3]| -> Vec<f32> {
            let got = shown(LocalAdjustment {
                color_gains: None,
                temperature: c[0] * 100.0,
                tint: c[1] * 100.0,
                exposure_ev: whole.exposure_ev + c[2],
                ..whole.clone()
            });
            got.iter().zip(&want).flat_map(|(a, b)| (0..3).map(move |i| a[i] - b[i])).collect()
        };
        let cost = |d: &[f32]| d.iter().map(|v| v * v).sum::<f32>();
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
        Some(best.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mask that covers the whole frame, with colour gains over a Temp, Tint
    /// and Exposure of its own.
    fn whole_mask(temperature: f32, tint: f32, exposure_ev: f32, gains: [f32; 3]) -> crate::recipe::LocalAdjustment {
        crate::recipe::LocalAdjustment {
            mask: crate::recipe::MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.0, full_x: 0.0, full_y: 0.0 },
            temperature,
            tint,
            exposure_ev,
            color_gains: Some(gains),
            ..Default::default()
        }
    }

    /// An older build's develop: it names no mask scale.
    fn older_develop(exposure_ev: f32, contrast: f32, as_shot: Option<(f32, f32)>) -> EditRecipe {
        EditRecipe {
            as_shot_k: as_shot.map(|a| a.0),
            as_shot_tint: as_shot.map(|a| a.1),
            exposure_ev,
            contrast,
            mask_wb_era: 0,
            ..EditRecipe::default()
        }
    }

    /// One mask of an older build's develop, measured: the three numbers that
    /// build wrote into a sidecar for the mask's gains (`v1_18_3::projection`,
    /// its law and its search kept beside today's), and how far from them two
    /// readings land. The distances are in the units of the recipe leaves the
    /// sidecar reader compares: slider units for Temp and Tint, EV for the
    /// Exposure offset.
    struct OlderProjection {
        /// What that build wrote: Temp and Tint (−1..1) and the EV offset.
        then: [f32; 3],
        /// This build's projection of the recipe as it stands — on the scale
        /// the recipe states — against `then`.
        on_its_scale: f32,
        /// Today's projection of the develop CARRIED OVER, restated on the
        /// old scale afterwards (× 150/80 and × 2, held to the range),
        /// against `then`.
        restated: f32,
    }

    fn older_projection(r: &EditRecipe, m: &crate::recipe::LocalAdjustment) -> OlderProjection {
        use crate::recipe::{LocalAdjustment, MASK_WB_ERA};
        assert_eq!(r.mask_wb_era, 0, "premise: an older build's develop");
        let miss = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).abs() * 100.0).max((a[1] - b[1]).abs() * 100.0).max((a[2] - b[2]).abs())
        };
        let then = v1_18_3::projection(r, m).expect("gains to carry");
        let found = gains_as_white(r, m).expect("gains to carry");
        assert_eq!(found.clamped, then[0].abs() >= 1.0 || then[1].abs() >= 1.0, "the end of that build's range");
        let (temperature, tint) = local_wb_from_legacy(m.temperature, m.tint);
        let today = gains_as_white(
            &EditRecipe { mask_wb_era: MASK_WB_ERA, ..r.clone() },
            &LocalAdjustment { temperature, tint, ..m.clone() },
        )
        .expect("gains to carry");
        let restated = [(today.temp * 150.0 / 80.0).clamp(-1.0, 1.0), (today.tint * 2.0).clamp(-1.0, 1.0), today.exposure_ev];
        OlderProjection {
            then,
            on_its_scale: miss([found.temp, found.tint, found.exposure_ev], then),
            restated: miss(restated, then),
        }
    }

    /// 2026-11-20: the three numbers an older build wrote into a sidecar for
    /// a mask's colour gains are found again, TO THE BIT, when the recipe
    /// states that build's scale — on plain and graded develops, with and
    /// without a Temp of the mask's own, and where the gains reach the end of
    /// that build's range. The sidecar reader allows a leaf Lightroom did not
    /// touch 2e-4 of a slider unit (`xmp::payload`); the bit is asked for
    /// here because the search does not converge to that: it stops within
    /// half its probe of the optimum, so anything that moves one candidate by
    /// one bit can send it somewhere else.
    ///
    /// That is why the develop reads an older recipe by that build's own law
    /// (`mask_wb_gains`) and not by anything gentler. The same gains searched
    /// on today's scale and restated on the old one do NOT land there (the
    /// premise at the end; on the five develops below 1.2e-4, 3.3e-4 and
    /// 5.1e-4 of a slider unit where the gains fit that build's range, 1.1
    /// and 10.8 slider units where they reach its end — that build's Temp
    /// stopped at its ±100 and its Tint and Exposure settled against it,
    /// today's Temp runs on).
    ///
    /// MUTATION THIS CATCHES: the mask stage no longer reading the recipe's
    /// `mask_wb_era` (the projection is today's, about half the size), or
    /// reading an older recipe through the carry-over and today's law (the
    /// third develop comes back 9e-6 of a slider unit off: one bit).
    #[test]
    fn an_older_builds_projection_is_found_again_on_its_own_scale() {
        let plain = older_develop(0.0, 0.0, None);
        let graded = older_develop(-0.3, 12.0, Some((5200.0, 4.0)));
        let cases = [
            (&plain, whole_mask(0.0, 0.0, 0.0, [0.93, 1.0, 1.10])),
            (&plain, whole_mask(0.0, 0.0, 0.0, [1.12, 0.99, 0.86])),
            (&graded, whole_mask(48.0, -20.0, -0.4, [0.8, 1.02, 1.3])),
            (&plain, whole_mask(-30.0, 12.0, 0.2, [1.25, 0.97, 0.7])),
            (&plain, whole_mask(0.0, 0.0, 0.0, [0.52, 1.09, 2.26])),
        ];
        let mut restated_worst = 0.0f32;
        for (r, m) in &cases {
            let p = older_projection(r, m);
            assert!(
                p.on_its_scale == 0.0,
                "that build wrote {:?}; read on its scale the projection is {:e} away",
                p.then,
                p.on_its_scale
            );
            eprintln!(
                "that build wrote {:?}; on its scale {:e} away; today's restated {:e} away",
                p.then, p.on_its_scale, p.restated
            );
            restated_worst = restated_worst.max(p.restated);
        }
        assert!(
            restated_worst > 2e-4,
            "premise: today's search restated is not that build's ({restated_worst:e}); if it ever is, the scale need not reach the search"
        );
    }

    /// A measurement (about a minute and a half, so not part of a normal
    /// run): the claim above over 240 pseudo-random masks — gains from 0.6 to
    /// 1.7 on red and blue, a Temp, Tint and Exposure of the mask's own on
    /// half of them, four base develops, none brighter than the frame it
    /// starts from (the kept copy of that build's law folds a mask's Temp
    /// into its gains, which is that build's picture only under the clip). It
    /// prints how many projections come back bit for bit and what restating
    /// today's projection would have missed by, apart for gains that fit that
    /// build's range and gains that reach its end, and it fails unless every
    /// one of the 240 comes back to the bit.
    ///
    /// What it measured on 2026-11-20, the three ways an older payload's
    /// projection could be made:
    /// - today's search, restated on the old scale afterwards: 48 of the 70
    ///   masks whose gains fit that build's range missed the reader's 2e-4
    ///   (the worst by 0.36 of a slider unit), and 169 of the 170 at the end
    ///   of it (the worst by 80 slider units);
    /// - that build's numbers carried over one candidate at a time and
    ///   rendered by today's law — the same law in exact arithmetic, a last
    ///   bit apart in a candidate's mired now and then: 205 came back to the
    ///   bit, 27 missed the 2e-4, the worst by 0.16 of a slider unit;
    /// - that build's own arithmetic in the mask stage (`mask_wb_gains`):
    ///   what the assertion below holds it to.
    #[test]
    #[ignore = "a measurement over 240 masks, about a minute: run it by name with --nocapture"]
    fn an_older_builds_projection_is_found_again_measured_over_many_masks() {
        let bases = [
            older_develop(0.0, 0.0, None),
            older_develop(-0.3, 12.0, Some((5200.0, 4.0))),
            older_develop(-0.8, -10.0, None),
            older_develop(-0.2, 25.0, Some((6500.0, -6.0))),
        ];
        let mut state = 0x2026_1120u32;
        let mut unit = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1u32 << 24) as f32
        };
        let (low, high) = (0.6f32.ln(), 1.7f32.ln());
        let total = 240usize;
        let (mut exact, mut beyond, mut worst) = (0usize, 0usize, 0.0f32);
        // Restating today's projection: [masks, misses, worst miss], for gains
        // inside that build's range and for gains at its end.
        let mut restated = [(0usize, 0usize, 0.0f32); 2];
        for i in 0..total {
            let gains = [(low + unit() * (high - low)).exp(), 0.95 + 0.1 * unit(), (low + unit() * (high - low)).exp()];
            let own = (i / bases.len()) % 2 == 1;
            let (temperature, tint, exposure_ev) =
                if own { (-60.0 + 120.0 * unit(), -40.0 + 80.0 * unit(), -0.5 + 0.8 * unit()) } else { (0.0, 0.0, 0.0) };
            let m = whole_mask(temperature, tint, exposure_ev, gains);
            let p = older_projection(&bases[i % bases.len()], &m);
            exact += usize::from(p.on_its_scale == 0.0);
            beyond += usize::from(p.on_its_scale > 2e-4);
            worst = worst.max(p.on_its_scale);
            if p.on_its_scale > 2e-4 {
                eprintln!("BEYOND: base {} mask ({temperature}, {tint}, {exposure_ev}, {gains:?}): {:e} from {:?}", i % bases.len(), p.on_its_scale, p.then);
            }
            let at_end = usize::from(p.then[0].abs() >= 1.0 || p.then[1].abs() >= 1.0);
            restated[at_end].0 += 1;
            restated[at_end].1 += usize::from(p.restated > 2e-4);
            restated[at_end].2 = restated[at_end].2.max(p.restated);
        }
        eprintln!("{total} masks: {exact} found bit for bit, the worst {worst:e} of a slider unit from what that build wrote, {beyond} beyond the reader's 2e-4");
        for (which, (masks, misses, worst)) in ["inside that build's range", "at the end of that build's range"].into_iter().zip(restated) {
            eprintln!("restating today's projection instead, gains {which}: {misses} of {masks} miss by more than 2e-4, the worst by {worst:e}");
        }
        assert_eq!((exact, beyond), (total, 0), "a projection on that build's scale is not what that build wrote");
    }

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
                FilmScale::NATIVE, false, false,
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

    /// A pixel at the white clip in every channel leaves the two ESTIMATED
    /// colour corrections — a mask's colour gains and the colour field —
    /// neutral, at the brightest value the stage gave it; a measured pixel —
    /// one step under the clip, or with a single channel at it — takes them
    /// as before. The photographer's own white balance and a mask's own Temp
    /// are instructions and colour a blown pixel like any other.
    ///
    /// MUTATION: drop `keep_blown_white` from either stage, read the clip on
    /// the brightest channel instead of the darkest, or guard a mask that
    /// carries a Temp and no gains.
    #[test]
    fn a_blown_pixel_takes_no_hue_from_an_estimated_colour_correction() {
        let develop = |r: &EditRecipe, px: [f32; 3]| {
            let mut data = vec![px; 16];
            apply_recipe_wb(&mut data, r);
            apply_develop_with_rasters(
                &mut data, 4, 4, r, &MaskRasterSnapshot::default(), MaskFrame::AsRendered,
                FilmScale::NATIVE, false, false,
            );
            data[5]
        };
        let neutral = |p: [f32; 3]| (p[0] - p[1]).abs() < 1e-4 && (p[1] - p[2]).abs() < 1e-4;
        let whole = crate::recipe::LocalAdjustment {
            mask: crate::recipe::MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.0, full_x: 0.0, full_y: 0.0 },
            ..Default::default()
        };
        // A mask's gains: the night pair's sky correction.
        let gains = crate::recipe::LocalAdjustment { color_gains: Some([0.52, 1.09, 2.26]), ..whole.clone() };
        let masked = EditRecipe { masks: vec![gains.clone()], ..EditRecipe::default() };
        let (blown, under, one) =
            (develop(&masked, [1.0; 3]), develop(&masked, [0.98; 3]), develop(&masked, [1.0, 0.6, 0.3]));
        assert!(neutral(blown) && blown[0] > 0.99, "a blown pixel under R x0.52 stays white: {blown:?}");
        assert!(under[0] < under[1] - 0.1, "a pixel under the clip takes the gains: {under:?}");
        assert!(one[0] < 0.8, "one channel at the clip beside two measurements is scaled: {one:?}");
        // A mask's own Temp is an instruction: it cools a blown pixel.
        let cooler = EditRecipe { masks: vec![crate::recipe::LocalAdjustment { temperature: -60.0, ..whole.clone() }], ..EditRecipe::default() };
        let blown = develop(&cooler, [1.0; 3]);
        assert!(blown[0] < blown[2] - 0.02, "a mask's Temp colours a blown pixel: {blown:?}");
        // With both, the gains read the pixel as the Temp left it: no longer
        // blown in every channel, so they scale it.
        let both = EditRecipe { masks: vec![crate::recipe::LocalAdjustment { temperature: -60.0, ..gains }], ..EditRecipe::default() };
        let (with_gains, temp_only) = (develop(&both, [1.0; 3]), blown);
        assert!(with_gains[0] < temp_only[0] - 0.05, "{with_gains:?} against {temp_only:?}");
        // The photographer's white balance, cooled from 6500 K to 3500 K.
        let cooled = EditRecipe { as_shot_k: Some(6500.0), temperature_k: Some(3500.0), ..EditRecipe::default() };
        let blown = develop(&cooled, [1.0; 3]);
        assert!(blown[0] < blown[2] - 0.02, "the white balance colours a blown pixel: {blown:?}");
        // The colour field: red down 30 %, blue up 30 %, everywhere.
        let field = crate::recipe::ColourField {
            x: 2,
            y: 2,
            b: 2,
            grid: vec![[0.0, -0.3, 0.0, 0.3, 0.0]; 8],
            owners: Vec::new(),
            amount: 1.0,
            enabled: true,
        };
        let fielded = |px: [f32; 3]| {
            let mut data = vec![px; 16];
            apply_colour_field(&mut data, 4, 4, Some(&field), &crate::render::FieldFrame::default(), None);
            data[5]
        };
        let (blown, under) = (fielded([1.0; 3]), fielded([0.9; 3]));
        assert!(neutral(blown) && blown[0] > 0.99, "a blown pixel takes no hue from the field: {blown:?}");
        assert!(under[0] < under[2] - 0.2, "a measured pixel takes the field: {under:?}");
    }
}
