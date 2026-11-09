//! The colour-grade stage: the brightness-dependent part of the colour change
//! the white balance leaves, solved onto the shadow / midtone / highlight
//! wheels.

use super::*;
use super::evidence::{
    EVIDENCE_MIN_SHARE, WB_CHROMA_FLOOR, wb_axes_by_region, wb_common, wb_common_shared, weighted_median,
};

/// Hue step of the wheel search, in degrees. The engine's wheel tints toward
/// the fully saturated hue at the pixel's own luma, so two degrees moves a
/// grey's tint by well under one 8-bit code at any Saturation the fit ships.
const GRADE_HUE_STEP: usize = 2;

/// Below this channel spread a pixel has no hue to sort it by and testifies
/// in the neutral group of [`solve_wheel`]'s hue split — the renderer's own
/// chroma gate, the one the cast-curve band statistics use.
const GRADE_NEUTRAL_CHROMA: f32 = 0.06;

/// The part of a set of `(red, blue)` log-chromaticity demands every group
/// asks alike, read on the white balance's two axes (Kelvin `red − blue`,
/// tint their mean) by [`wb_common`] and returned as `(red, blue)`.
fn common_part(groups: &[(f32, f32, f32)]) -> [f32; 2] {
    let kelvin: Vec<(f32, f32)> = groups.iter().map(|&(u, v, m)| (u - v, m)).collect();
    let tint: Vec<(f32, f32)> = groups.iter().map(|&(u, v, m)| (0.5 * (u + v), m)).collect();
    let (k, t) = (wb_common(&kelvin), wb_common(&tint));
    [t + 0.5 * k, t - 0.5 * k]
}

/// One wheel solved on the paired pixels of region `region` of the grade
/// split: the `(hue, saturation)` whose move on the region's own pixels
/// best delivers the region's median log-chromaticity change.
///
/// The demand is the white balance's own statistic, read per region: each
/// paired pixel's log change, red and blue against green
/// ([`wb_axes_by_region`]'s axes, on the frame as the stage finds it),
/// weighted by its trust and by the region weight the grade gives it, and
/// the weighted MEDIAN of each — read twice over before it is a wheel's:
///
/// * less `common`, the part every TONAL REGION asks alike (see
///   [`fit_grade_stage`]): that part is an illuminant's, the white
///   balance's, whether the white-balance stage shipped it or gave it back;
/// * and then only the part every HUE GROUP of the region asks alike — the
///   eight mixer bands and the neutrals, each holding at least
///   [`EVIDENCE_MIN_SHARE`] of the region's testifying mass, read by
///   [`wb_common`] exactly as the white balance reads its regions. A wheel
///   moves every hue of its region one way; where two hue families at one
///   brightness move opposite ways, that difference is per-hue and the
///   mixer's. Measured 2026-11-01 before this rule: on the two-family band
///   pair (two families at one brightness, one desaturated and one
///   saturated) the midtone wheel took the region median, the cells
///   admitted it, and the finished frame regressed until the whole fit was
///   reset (0.0328 -> 0.0328, where it had reached 0.0231).
///
/// A least-squares fit on the pixels
/// themselves was tried first and is not what a wheel answers: a wheel
/// pulls each pixel toward its hue, so a pixel's move depends on the
/// pixel's own colour, and the fit was carried by the saturated pixels the
/// hue moves furthest — on the canyon pair (2026-11-01) the highlights,
/// whose median asked to cool (red against blue −0.04), took hue 20 at
/// Saturation 63 from the blue sky inside them. The median is the region's
/// testimony.
///
/// The wheel's move is the engine's model ([`render::wheel_tint`],
/// [`render::WHEEL_SAT_REACH`]) on the region's OWN pixels, each weighted
/// by its trust and region weight, linearised in the amount and averaged
/// on the demand's two axes. Read on a single grey instead, a highlight
/// wheel looked weak (a grey near white barely tints) and was solved to the
/// clamp, which then dragged every coloured highlight toward its hue — the
/// village pair's highlights took hue 30 at Saturation 100 against a
/// remainder of almost nothing. For each hue the amount is the
/// least-squares one, clamped to the control's range, and the hue kept is
/// the one that leaves the least of the demand. Pixels too dark to carry a
/// chromaticity ([`WB_CHROMA_FLOOR`]) do not testify.
pub(super) fn solve_wheel(
    cur: &[[f32; 3]],
    tp: &[[f32; 3]],
    weights: &[f32],
    cg: &crate::recipe::ColorGrade,
    region: usize,
    common: [f32; 2],
) -> (f32, f32) {
    let n = cur.len().min(tp.len()).min(weights.len());
    let lit = |p: &[f32; 3]| p[0].max(p[1]).max(p[2]) >= WB_CHROMA_FLOOR;
    // Per hue group: the eight mixer bands, then the neutrals.
    let mut red: [Vec<(f32, f32)>; 9] = Default::default();
    let mut blue: [Vec<(f32, f32)>; 9] = Default::default();
    let mut mass = [0.0f64; 9];
    let mut members: Vec<(usize, f32, f32)> = Vec::new();
    for i in 0..n {
        let e = weights[i].max(0.0);
        let l = luma601(&cur[i]);
        let w = render::color_grade_regions(cg, l)[region];
        if e <= 0.0 || w <= 1e-3 || !lit(&cur[i]) || !lit(&tp[i]) {
            continue;
        }
        let d: [f32; 3] = std::array::from_fn(|c| {
            (render::srgb_to_linear(tp[i][c]).max(1e-5) / render::srgb_to_linear(cur[i][c]).max(1e-5)).ln()
        });
        let p = cur[i];
        let group = if p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]) < GRADE_NEUTRAL_CHROMA {
            8
        } else {
            let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
            let (b0, b1, w1) = render::bracket_bands(h * 360.0, &render::HSL_CENTERS);
            if w1 < 0.5 { b0 } else { b1 }
        };
        red[group].push((d[0] - d[1], e * w));
        blue[group].push((d[2] - d[1], e * w));
        mass[group] += (e * w) as f64;
        members.push((i, w, l));
    }
    if members.is_empty() {
        return (0.0, 0.0);
    }
    let total: f64 = mass.iter().sum();
    let mut groups: Vec<(f32, f32, f32)> = Vec::new();
    for g in 0..9 {
        if total <= 0.0 || mass[g] / total < EVIDENCE_MIN_SHARE as f64 {
            continue;
        }
        groups.push((
            weighted_median(&mut red[g]) - common[0],
            weighted_median(&mut blue[g]) - common[1],
            mass[g] as f32,
        ));
    }
    let want = common_part(&groups);
    let reach = render::WHEEL_SAT_REACH;
    let mut best = (0.0f32, 0.0f32, want[0] * want[0] + want[1] * want[1]);
    for hue in (0..360).step_by(GRADE_HUE_STEP) {
        let h = hue as f32;
        // The wheel's move per unit amount on the region's own pixels, on the
        // demand's two axes: each pixel's log linear change, linearised at
        // amount 0, trust- and region-weighted, then averaged.
        let (mut dir, mut mass) = ([0.0f64; 2], 0.0f64);
        for &(i, w, l) in &members {
            let m = (weights[i].max(0.0) * w) as f64;
            let tint = render::wheel_tint(h, l);
            let slope: [f32; 3] = std::array::from_fn(|c| {
                let base = render::srgb_to_linear(cur[i][c]).max(1e-5);
                let moved = render::srgb_to_linear((cur[i][c] + 1e-3 * w * (tint[c] - cur[i][c])).clamp(0.0, 1.0));
                (moved.max(1e-5) / base).ln() / 1e-3
            });
            dir[0] += m * (slope[0] - slope[1]) as f64;
            dir[1] += m * (slope[2] - slope[1]) as f64;
            mass += m;
        }
        let dir = [(dir[0] / mass) as f32, (dir[1] / mass) as f32];
        let den = dir[0] * dir[0] + dir[1] * dir[1];
        let num = dir[0] * want[0] + dir[1] * want[1];
        if num <= 0.0 || den <= 0.0 {
            continue;
        }
        let amount = (num / den).min(reach);
        let left = (want[0] - amount * dir[0]).powi(2) + (want[1] - amount * dir[1]).powi(2);
        if left < best.2 {
            best = (h, amount / reach * 100.0, left);
        }
    }
    (best.0, best.1)
}

/// Apply the three solved wheels to `px` exactly as the engine's grade
/// would at this point of the pipeline, region by region in its order.
fn modelled(px: &[[f32; 3]], cg: &crate::recipe::ColorGrade) -> Vec<[f32; 3]> {
    let wheels = [
        (cg.shadow_hue, cg.shadow_sat),
        (cg.midtone_hue, cg.midtone_sat),
        (cg.highlight_hue, cg.highlight_sat),
    ];
    px.iter()
        .map(|p| {
            let mut q = *p;
            let w = render::color_grade_regions(cg, luma601(p));
            for (r, (h, s)) in wheels.iter().enumerate() {
                let amt = s / 100.0 * w[r] * render::WHEEL_SAT_REACH;
                if amt <= 0.0 {
                    continue;
                }
                let tint = render::wheel_tint(*h, luma601(&q));
                for c in 0..3 {
                    q[c] = (q[c] + (tint[c] - q[c]) * amt).clamp(0.0, 1.0);
                }
            }
            q
        })
        .collect()
}

/// The colour-grade stage. It runs right after the white-balance stage and
/// the saturation chase, and before the per-band mixer and the cast curves,
/// which close their loop on what it leaves.
///
/// WHY IT EXISTS (2026-11-01). A white balance is an illuminant: it moves
/// every brightness of the frame by the same channel ratios, so the
/// white-balance stage takes only the part of the paired colour change the
/// brightness ranges have IN COMMON ([`paired_wb_from_populations`]).
/// A target graded warm in the shadows and cool in the highlights asks for
/// more than that, and the rest is still the target's: on the seaside street
/// the paired ranges asked log(B/R) −1.01 in the deepest shadows, −0.89 in
/// the mids, −0.27 in the brightest land and +0.16 in the sky, and the one
/// white balance that took the mass-weighted middle of that landed on the
/// 40000 K clamp and turned the sky yellow. That remainder is what the
/// colour-grade wheels are for: one hue and one strength per tonal region.
///
/// A SPLIT TONE IS NOT A FAN. The one validity law (`fit::validity`)
/// refuses a cast that sorts one hue class into a spread of hues by
/// luminance — each brightness range of that class is a cell left farther
/// from its own ask — because a per-channel curve does that as an artefact
/// of its own shape and no single control can take it back. A grade looks
/// alike in kind — hue by luminance — and is admitted on different evidence:
/// each wheel is solved from the paired pixels of ITS tonal region only, so
/// it moves a region the way that region's own pixels testify, and it is
/// one develop control per region a user can read and undo. The law
/// still judges the cast curves after this stage, on the frame this stage
/// leaves.
///
/// ONLY THE REMAINDER, AND ONLY THE SHARED PART OF IT. Before any wheel is
/// solved, the part of the change every tonal region asks alike is read
/// again on the frame the white balance left ([`wb_axes_by_region`],
/// [`wb_common`]) and taken off every region's demand: it is the white
/// balance's whether that stage shipped it (it then reads about zero) or
/// gave it back over budget or on the cells' word. Without this the wheels
/// picked up a cast the white balance withheld — on the uniform 3500 K
/// hand-off fixture three wheels of one hue (210 / 204 / 208, measured
/// 2026-11-01), a white balance by another name that never reached the
/// hand-off. Inside a region each wheel then takes only what every hue
/// group shares ([`solve_wheel`]); the per-hue part is the mixer's.
///
/// THE STRENGTH DIAL REACHES IT. Each wheel's Saturation is held under the
/// strength budget's wheel ceiling (`wheel_ceiling`, [`FitBudget::grade_wheel`]:
/// 6 at Strength 0, 18 at the default, 45 at 1), the mixer's own band
/// ceiling read on the wheel's axis. Strength is a restraint budget, not an
/// amount: at 0 the stage still ships what the tightest budget admits. Until
/// v1.17.1 the wheels obeyed only the saturation chase's ceiling (30 / 60),
/// above what the seaside street's wheels asked at any strength, so the
/// grade moved about as far at Strength 0 as at 1 (shadows 30.0 / 29.1,
/// midtones 21.6 / 16.5 at 0 / 1; 2026-11-02).
///
/// What ships of it is the joint objective's question (RULING-1180-joint-r1):
/// the validity law reads the chroma dials on the cells they move, and each
/// wheel is judged against its absence on F. The
/// disclosure reads the shipped wheels off the recipe (`compose_report`); this returns
/// `Some((ceiling, asked))` when a wheel that shipped was held at the
/// strength ceiling, `asked` being the largest Saturation the paired pixels
/// asked for, so the report can say the dial restrained the stage.
pub(super) fn fit_grade_stage(
    s_img: &DynamicImage,
    tp: &[[f32; 3]],
    weights: &[f32],
    wheel_ceiling: f32,
    recipe: &mut EditRecipe,
) -> Option<(f32, f32)> {
    let restore = recipe.color_grade.clone();
    let before = pixels_of(&render::develop_preview(s_img, recipe));
    let mut cg = restore.clone();
    // Each wheel on the frame as the wheels before it in the engine's order
    // leave it, by the engine's own model.
    let mut modelled_px = before.clone();
    // The part every tonal region asks alike, read on the frame the white
    // balance left: about zero where it shipped, the withheld part where it
    // gave it back. Never a wheel's.
    // Read exactly as the white balance reads it (`wb_common_shared`).
    let shared = wb_axes_by_region(&before, tp, weights);
    let common = {
        let (k, t) = (wb_common_shared(&shared, 0), wb_common_shared(&shared, 1));
        [t + 0.5 * k, t - 0.5 * k]
    };
    let mut solved = [(0.0f32, 0.0f32); 3];
    for (region, slot) in solved.iter_mut().enumerate() {
        *slot = solve_wheel(&modelled_px, tp, weights, &cg, region, common);
        match region {
            0 => (cg.shadow_hue, cg.shadow_sat) = *slot,
            1 => (cg.midtone_hue, cg.midtone_sat) = *slot,
            _ => (cg.highlight_hue, cg.highlight_sat) = *slot,
        }
        modelled_px = modelled(&before, &cg);
    }
    if solved.iter().all(|&(_, sat)| sat < 0.5) {
        return None;
    }
    // The stages after the grade (clarity, texture, saturation, the masks)
    // carry its tint on; one secant step on the realised move puts the
    // strength back on the modelled one, per region.
    recipe.color_grade = cg.clone();
    let after = pixels_of(&render::develop_preview(s_img, recipe));
    let mut scaled = solved;
    let ceiling = wheel_ceiling.clamp(0.0, 100.0);
    let mut asked = 0.0f32;
    for (region, slot) in scaled.iter_mut().enumerate() {
        let (mut along, mut realised) = (0.0f64, 0.0f64);
        for i in 0..before.len().min(after.len()).min(weights.len()) {
            let w = render::color_grade_regions(&cg, luma601(&before[i]))[region] * weights[i].max(0.0);
            if w <= 0.0 {
                continue;
            }
            for c in 0..3 {
                let want = (modelled_px[i][c] - before[i][c]) as f64;
                let got = (after[i][c] - before[i][c]) as f64;
                along += w as f64 * want * got;
                realised += w as f64 * got * got;
            }
        }
        if realised > 0.0 && along > 0.0 {
            slot.1 = (slot.1 * (along / realised) as f32).clamp(0.0, 100.0);
        }
        // The strength budget's wheel ceiling.
        if slot.1 > ceiling {
            asked = asked.max(slot.1);
        }
        slot.1 = slot.1.min(ceiling);
    }
    let held = (asked > 0.0).then_some((ceiling, asked));
    let cg = &mut recipe.color_grade;
    (cg.shadow_hue, cg.shadow_sat) = (scaled[0].0, round1(scaled[0].1));
    (cg.midtone_hue, cg.midtone_sat) = (scaled[1].0, round1(scaled[1].1));
    (cg.highlight_hue, cg.highlight_sat) = (scaled[2].0, round1(scaled[2].1));
    held
}
