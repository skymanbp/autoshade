//! Lightroom's lens-defect corrections that act on PIXELS rather than on the
//! frame's geometry (v1.5.0): de-fringing.
//!
//! The geometric half of the Lens panel — profile distortion, the profile and
//! manual CA scales, the manual vignette — has been in `render.rs` for many
//! versions as resampling and radial gain. What lived nowhere until now is the
//! half that looks at COLOUR: the six de-fringe controls, which R25 carried to
//! the sidecar and drew no pixel from, on the ground that Adobe's 0..100 hue
//! scale has no published mapping to an actual hue angle.
//!
//! The user revoked that policy on 2026-09-17 ("slight deviation from
//! Lightroom is allowed, compatibility is the aim"), so the mapping is stated
//! here as OURS and named for the kit ladder that will measure it: the purple
//! scale spans the violet-to-magenta arc a fast lens actually fringes with,
//! and the green scale the yellow-green-to-cyan one, which puts Adobe's own
//! defaults (30/70 purple, 40/60 green) over exactly those two bands.

use rayon::prelude::*;

use super::{chroma, luma601, rgb_to_hsl, smoothstep, CHROMA_GATE};
use crate::recipe::EditRecipe;

/// One degree of hue, as a fraction of the circle. The two scales below are
/// stated in DEGREES and converted here, because degrees are the units the
/// windows were chosen in and `360.0 / 360.0` is an expression clippy refuses
/// to read as "the whole turn" (`eq_op`).
const DEG: f32 = 1.0 / 360.0;

/// Adobe's purple hue slider, 0..100, in hue TURNS. 30/70 — its own defaults —
/// land on 262° and 318°, violet through magenta (`DF-P*`).
const PURPLE_SCALE: (f32, f32) = (220.0 * DEG, 360.0 * DEG);

/// Adobe's green hue slider, 0..100, in hue turns. 40/60 — its own defaults —
/// land on 108° and 132°, the narrow green a lens fringes with (`DF-G*`).
const GREEN_SCALE: (f32, f32) = (60.0 * DEG, 180.0 * DEG);

/// How soft the hue window's own edges are, as a fraction of its width: a hard
/// window would cut a gradient of fringe in half and leave a visible step.
const WINDOW_SOFT: f32 = 0.25;

/// The local contrast (3×3 luma max − min, gamma-encoded) over which the
/// correction opens. Fringing is a HIGH-CONTRAST edge artefact; a flat field
/// of the same hue is a photograph of something purple (`DF-EDGE`).
const EDGE_GATE: (f32, f32) = (0.06, 0.25);

/// Adobe's Amount band. At 20 a fringe pixel is taken all the way to its own
/// luminance; below that the correction is proportional.
const DEFRINGE_MAX: f32 = 20.0;

/// One hue window, resolved from its two sliders.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Window {
    lo: f32,
    hi: f32,
    soft: f32,
    strength: f32,
}

impl Window {
    /// `None` when the amount is zero or the two sliders leave no window.
    fn of(amount: f32, lo: f32, hi: f32, scale: (f32, f32)) -> Option<Window> {
        if amount <= 0.0 {
            return None;
        }
        let at = |v: f32| scale.0 + (scale.1 - scale.0) * (v.clamp(0.0, 100.0) / 100.0);
        let (lo, hi) = (at(lo), at(hi));
        if hi - lo < 1e-4 {
            return None;
        }
        Some(Window {
            lo,
            hi,
            soft: ((hi - lo) * WINDOW_SOFT).max(1e-3),
            strength: (amount / DEFRINGE_MAX).clamp(0.0, 1.0),
        })
    }

    /// How far inside this window a hue sits, 0..1 — on the hue CIRCLE, so a
    /// window whose top end reaches past magenta still measures the reds that
    /// sit on the other side of 0.
    fn membership(&self, hue: f32) -> f32 {
        let h = if hue < self.lo - self.soft { hue + 1.0 } else { hue };
        smoothstep(self.lo - self.soft, self.lo + self.soft, h)
            * (1.0 - smoothstep(self.hi - self.soft, self.hi + self.soft, h))
    }
}

/// The de-fringe pass: where a high-contrast edge carries one of the two
/// fringe hues, pull that pixel toward its own luminance.
///
/// Lightroom's own operator is unpublished. This is the honest reading of what
/// it must do — a fringe is chroma the lens put where the picture has only a
/// luminance step, so the correction removes chroma and nothing else: no pixel
/// changes brightness, and a saturated subject that is not on an edge is never
/// touched.
pub(crate) fn defringe(data: &mut [[f32; 3]], w: usize, h: usize, r: &EditRecipe) {
    let purple =
        Window::of(r.defringe_purple, r.defringe_purple_lo, r.defringe_purple_hi, PURPLE_SCALE);
    let green = Window::of(r.defringe_green, r.defringe_green_lo, r.defringe_green_hi, GREEN_SCALE);
    if (purple.is_none() && green.is_none()) || w < 3 || h < 3 {
        return;
    }
    // The luma plane first: the edge test reads NEIGHBOURS, so it cannot read
    // a buffer this pass is already rewriting.
    let luma: Vec<f32> = data.par_iter().map(luma601).collect();
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for l in (y.saturating_sub(1)..(y + 2).min(h))
                .flat_map(|dy| (x.saturating_sub(1)..(x + 2).min(w)).map(move |dx| dy * w + dx))
                .map(|i| luma[i])
            {
                lo = lo.min(l);
                hi = hi.max(l);
            }
            let edge = smoothstep(EDGE_GATE.0, EDGE_GATE.1, hi - lo);
            let gate = smoothstep(CHROMA_GATE.0, CHROMA_GATE.1, chroma(px));
            if edge <= 0.0 || gate <= 0.0 {
                continue;
            }
            let hue = rgb_to_hsl(px[0], px[1], px[2]).0;
            let hit = |win: Option<Window>| win.map_or(0.0, |v| v.membership(hue) * v.strength);
            let amount = (hit(purple) + hit(green)).min(1.0) * edge * gate;
            if amount <= 0.0 {
                continue;
            }
            let grey = luma601(px);
            for c in px.iter_mut() {
                *c += (grey - *c) * amount;
            }
        }
    });
}

// ── the automatic lateral-CA solver ──────────────────────────────────────────

/// The smallest radial lever (`r · ∂G/∂r`, in gamma units × pixels) a sample
/// must carry to join the fit. Below it the pixel has no edge to be displaced,
/// and a least-squares slope through featureless pixels is a slope through
/// noise.
///
/// This is the ONLY admission rule. An inner-radius floor — "the centre of the
/// frame says nothing about a radial error" — was written here first and then
/// MEASURED away: least squares already weights a sample by its lever
/// SQUARED, so on a 129 px probe the disc inside `rn < 0.25` holds 9.7 % of the
/// pixels but ⟨lever²⟩ ≈ 1.40 against ≈ 24.8 outside it, i.e. 0.6 % of the
/// regression's weight. Moving the answer by one slider unit away from a true
/// 12 would need the centre to disagree by ≈ 176 units — outside the ±100 band
/// the control can even hold. A floor that cannot change the answer is not a
/// guard, so it is gone rather than sitting here untested.
const CA_MIN_LEVER: f32 = 0.5;

/// Every nth pixel in each direction. The estimate is a single number over the
/// whole frame, so a quarter of a 61 MP frame is already 3.8 million samples.
const CA_STRIDE: usize = 2;

/// Solve Lightroom's 「Remove Chromatic Aberration」, in the units of the
/// manual pair it fills.
///
/// The model is the first-order one the defect itself obeys: if the red
/// channel is magnified about the frame's centre by `1 + e` relative to green,
/// then at every pixel
///
/// ```text
///     R − G  ≈  (the subject's own red-minus-green)  +  e · r · ∂G/∂r
/// ```
///
/// so a least-squares line of `R − G` against the radial lever `r · ∂G/∂r`,
/// with an intercept that absorbs the subject's colour and any cast, has `e`
/// for its slope. The correction is the opposite sign, and it is returned in
/// SLIDER UNITS (`MANUAL_CA_PER_UNIT`, integral and clamped to Lightroom's
/// ±100 band) so that the answer is the same number on a preview raster and on
/// the full-resolution export — a ratio is scale-free, and rounding to the
/// control's own step collapses what measurement noise is left. One slider
/// unit is 0.07 px at the corner of a 61 MP frame.
pub(crate) fn solve_lateral_ca(data: &[[f32; 3]], w: usize, h: usize) -> (f32, f32) {
    if w < 8 || h < 8 {
        return (0.0, 0.0);
    }
    let (cx, cy) = ((w as f32 - 1.0) * 0.5, (h as f32 - 1.0) * 0.5);
    let rmax = (cx * cx + cy * cy).sqrt().max(1.0);
    // Sums for two regressions that share their x: n, Σx, Σx², Σy_r, Σx·y_r,
    // Σy_b, Σx·y_b.
    let zero = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let add = |a: (f64, f64, f64, f64, f64, f64, f64), b: (f64, f64, f64, f64, f64, f64, f64)| {
        (a.0 + b.0, a.1 + b.1, a.2 + b.2, a.3 + b.3, a.4 + b.4, a.5 + b.5, a.6 + b.6)
    };
    let sums = (1..h - 1)
        .into_par_iter()
        .step_by(CA_STRIDE)
        .map(|y| {
            let mut acc = zero;
            for x in (1..w - 1).step_by(CA_STRIDE) {
                let (ux, uy) = ((x as f32 - cx) / rmax, (y as f32 - cy) / rmax);
                let g = |i: usize| data[i][1];
                let i = y * w + x;
                let dx = (g(i + 1) - g(i - 1)) * 0.5;
                let dy = (g(i + w) - g(i - w)) * 0.5;
                // `r · ∂G/∂r`: the green gradient projected on the RADIAL
                // direction, times the radius. Written with the normalised
                // offsets because `(dx, dy) · (ux, uy) · rmax` IS
                // `dx·(x−cx) + dy·(y−cy)` — the unit direction's `1/r` and the
                // lever's `r` cancel, so neither appears.
                let lever = (dx * ux + dy * uy) * rmax;
                if lever.abs() < CA_MIN_LEVER {
                    continue;
                }
                let px = &data[i];
                acc = add(
                    acc,
                    (
                        1.0,
                        lever as f64,
                        (lever * lever) as f64,
                        (px[0] - px[1]) as f64,
                        (lever * (px[0] - px[1])) as f64,
                        (px[2] - px[1]) as f64,
                        (lever * (px[2] - px[1])) as f64,
                    ),
                );
            }
            acc
        })
        .reduce(|| zero, add);
    let (n, sx, sxx, syr, sxyr, syb, sxyb) = sums;
    let denom = n * sxx - sx * sx;
    if n < 64.0 || denom.abs() < 1e-9 {
        return (0.0, 0.0); // nothing with leverage: say nothing
    }
    let slope = |sy: f64, sxy: f64| ((n * sxy - sx * sy) / denom) as f32;
    // The CORRECTION is the opposite of the error, and `ca_r` is stated as
    // "positive magnifies the red channel" (`recipe::EditRecipe::ca_r`).
    let unit = |e: f32| (-e / crate::render::MANUAL_CA_PER_UNIT).round().clamp(-100.0, 100.0);
    (unit(slope(syr, sxyr)), unit(slope(syb, sxyb)))
}

/// The recipe the geometry stage should use: Lightroom's auto switch is an
/// INSTRUCTION, so rendering it means running [`solve_lateral_ca`] and handing
/// the answer to the same manual pair a photographer could have set by hand.
/// Borrowed — nothing copied — when the switch is off, which is every photo
/// that never asked.
///
/// It must run BEFORE `geometry_profile`, because that is what folds the pair
/// onto the camera's knots, and on the frame as the develop's first stages
/// left it, because that is the frame the geometry stage will resample.
pub(crate) fn with_auto_lateral_ca<'a>(
    r: &'a EditRecipe,
    data: &[[f32; 3]],
    w: usize,
    h: usize,
) -> std::borrow::Cow<'a, EditRecipe> {
    if !r.auto_lateral_ca {
        return std::borrow::Cow::Borrowed(r);
    }
    let (dr, db) = solve_lateral_ca(data, w, h);
    if dr == 0.0 && db == 0.0 {
        return std::borrow::Cow::Borrowed(r);
    }
    let mut solved = r.clone();
    // ADDED to the manual pair, not replacing it: the two controls are
    // separate in the sidecar, so a photographer who set both means both.
    solved.ca_r = (solved.ca_r + dr).clamp(-100.0, 100.0);
    solved.ca_b = (solved.ca_b + db).clamp(-100.0, 100.0);
    std::borrow::Cow::Owned(solved)
}

#[cfg(test)]
mod tests;
