//! Camera Raw's near-white colour lift (v1.17.0, **measured**): before the
//! camera matrix, in white-balanced camera RGB, Camera Raw lifts a colour's
//! lower channels toward its higher ones as their sensor values near the
//! white level, so a bright saturated colour starts to pale on the way to
//! white. Nothing in the DNG SDK does this; it is read from Camera Raw renders
//! alone.
//!
//! The law, measured from Camera Raw renders, 11 renders, 2026-10-29 —
//! synthetic linear DNGs of 1728-patch colour lattices (12 levels a channel)
//! and slope sweeps, with an identity tone curve and no tables, inverted
//! through each render's own grey transfer:
//!
//! * The channel whose camera neutral is LARGEST (the smallest white-balance
//!   multiplier — green on every Bayer body) is the *anchor*; the other two
//!   are a *pair*. Each member of the pair rises toward the other:
//!   `a' = a + k(raw_a) · max(b − a, 0)`. The anchor then rises toward the
//!   pair's mean: `s' = s + k(raw_s) · max((a' + b') / 2 − s, 0)`.
//! * Differences are in white-balanced units; the slope `k` is read at the
//!   channel's own SENSOR value relative to the white level, before the white
//!   balance and before any exposure (the same lift in camera units at total
//!   exposures −2, 0, +0.35 through `BaselineExposure`, and +2).
//! * It acts in camera space: a camera with sRGB primaries gives the same lift
//!   in its camera units as one with ProPhoto primaries.
//! * Fit over the seven lattices, each neutral with one largest entry or a tie
//!   (grey; (0.5, 1, 0.7); (0.399, 1, 0.593); (0.75, 1, 0.35); (1, 0.6, 0.8);
//!   (0.7, 0.6, 1); (1, 0.5, 1)): worst channel error 0.0035 of white, rms
//!   0.0006–0.0015, where leaving the lift out misses by up to 0.107. Ties
//!   resolve to green, then red (the grey neutral anchors on green, (1, 0.5, 1)
//!   on red).
//!
//! `k` is [`LIFT_K`], one value per 0.01 of sensor value from 0.20 to 0.99,
//! read where each channel of the pair was driven by a channel at the white
//! level (the two readings agree to 0.0034); 0 below, 1 at the white level.

/// The sensor value of [`LIFT_K`]'s first entry, and its step.
const LIFT_V0: f32 = 0.20;
const LIFT_STEP: f32 = 0.01;

/// The lift's slope by sensor value, 0.20 … 0.99 in steps of 0.01, then 1 at
/// the white level (module head).
static LIFT_K: [f32; 81] = [
    0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0006, 0.0018,
    0.0034, 0.0053, 0.0075, 0.0102, 0.0131, 0.0165, 0.0202, 0.0242, 0.0286, 0.0334,
    0.0385, 0.0440, 0.0499, 0.0560, 0.0626, 0.0695, 0.0767, 0.0843, 0.0923, 0.1007,
    0.1094, 0.1184, 0.1278, 0.1375, 0.1476, 0.1581, 0.1689, 0.1801, 0.1916, 0.2035,
    0.2157, 0.2283, 0.2412, 0.2545, 0.2681, 0.2821, 0.2966, 0.3113, 0.3264, 0.3417,
    0.3576, 0.3737, 0.3901, 0.4069, 0.4240, 0.4416, 0.4596, 0.4777, 0.4965, 0.5154,
    0.5347, 0.5545, 0.5744, 0.5948, 0.6155, 0.6367, 0.6579, 0.6796, 0.7019, 0.7243,
    0.7474, 0.7704, 0.7941, 0.8181, 0.8420, 0.8667, 0.8916, 0.9167, 0.9437, 0.9670,
    1.0000,
];

/// The slope at a sensor value relative to the white level.
fn slope(raw: f32) -> f32 {
    if raw.is_nan() || raw <= LIFT_V0 {
        return 0.0;
    }
    let t = (raw - LIFT_V0) / LIFT_STEP;
    let i = t.floor() as usize;
    if i + 1 >= LIFT_K.len() {
        return 1.0;
    }
    let f = t - i as f32;
    LIFT_K[i] + (LIFT_K[i + 1] - LIFT_K[i]) * f
}

/// The lift for one white balance: which channel anchors (module head).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ClipLift {
    anchor: usize,
    pair: [usize; 2],
}

impl ClipLift {
    /// For the white-balance multipliers `wb` (camera → white-balanced, the
    /// reciprocal of the camera neutral): the anchor is the channel with the
    /// smallest multiplier, ties to green, then red. `None` for an unusable
    /// white balance.
    pub(super) fn for_white(wb: [f32; 3]) -> Option<Self> {
        if wb.iter().any(|m| !(m.is_finite() && *m > 0.0)) {
            return None;
        }
        let anchor = [1usize, 0, 2].into_iter().fold(1, |best, j| if wb[j] < wb[best] { j } else { best });
        let pair = match anchor {
            0 => [1, 2],
            1 => [0, 2],
            _ => [0, 1],
        };
        Some(ClipLift { anchor, pair })
    }

    /// Lift the white-balanced camera triple `v` whose sensor values (before
    /// the white balance, relative to the white level) are `raw`.
    pub(super) fn apply(&self, raw: [f32; 3], v: [f32; 3]) -> [f32; 3] {
        let [a, b] = self.pair;
        let s = self.anchor;
        let mut out = v;
        out[a] = v[a] + slope(raw[a]) * (v[b] - v[a]).max(0.0);
        out[b] = v[b] + slope(raw[b]) * (v[a] - v[b]).max(0.0);
        out[s] = v[s] + slope(raw[s]) * ((out[a] + out[b]) * 0.5 - v[s]).max(0.0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured readings themselves: the slope at the sweep's points, a
    /// grey untouched, and a bright magenta paling toward white as measured
    /// (Camera Raw, grey neutral: (0.98, 0.686, 0.686) → green +0.066, blue
    /// +0.099; the lattice model gives +0.066 / +0.100).
    ///
    /// MUTATION: anchor on the largest multiplier, key the slope on the
    /// white-balanced value, or lift the anchor toward the pair's maximum.
    #[test]
    fn a_bright_colour_pales_toward_white_as_camera_raw_measured() {
        assert_eq!(slope(0.1), 0.0);
        assert!((slope(0.5) - 0.1094).abs() < 1e-4 && (slope(0.8) - 0.5347).abs() < 1e-4);
        assert_eq!(slope(1.0), 1.0);
        let grey = ClipLift::for_white([1.0; 3]).unwrap();
        assert_eq!(grey.anchor, 1);
        assert_eq!(grey.apply([0.9; 3], [0.9; 3]), [0.9; 3]);
        let px = [0.98, 0.686, 0.686];
        let out = grey.apply(px, px);
        assert!((out[2] - px[2] - 0.099).abs() < 0.003, "{out:?}");
        assert!((out[1] - px[1] - 0.066).abs() < 0.003, "{out:?}");
        assert_eq!(out[0], px[0], "the highest channel stays");
        // A daylight neutral (green largest): the slope reads the sensor
        // value, so blue at white-balanced 0.686 but sensor 0.48 barely moves.
        let day = ClipLift::for_white([2.0, 1.0, 1.0 / 0.7]).unwrap();
        let wb = [0.98, 0.686, 0.686];
        let raw = [0.49, 0.686, 0.48];
        let out = day.apply(raw, wb);
        assert!((out[2] / wb[2] - 1.042).abs() < 0.004, "{out:?}");
        assert!((out[1] / wb[1] - 1.079).abs() < 0.004, "{out:?}");
        // Ties: red anchors when red and blue share the largest neutral.
        assert_eq!(ClipLift::for_white([1.0, 2.0, 1.0]).unwrap().anchor, 0);
        assert_eq!(ClipLift::for_white([1.4, 1.6, 1.0]).unwrap().anchor, 2);
    }
}
