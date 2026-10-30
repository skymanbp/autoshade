//! Camera Raw's Exposure (v1.17.0), measured: one transfer at the TOTAL
//! exposure, read from Camera Raw's own renders.
//!
//! # What was measured
//!
//! A synthetic DNG (camera = linear ProPhoto, "Adobe Standard" with an
//! identity curve and no tables, so the output IS the transfer) of 291 grey
//! patches, rendered by Camera Raw at 54 total exposures from −5 to +6 EV
//! (`ramp_table.rs`):
//!
//! * **The transfer is pointwise.** The same on any background, with bright
//!   content or none, mosaic or linear DNG, process version 11 or 15.4.
//! * **The total is what counts.** BaselineExposure +1 at slider 0 renders
//!   exactly as slider +1, so a photograph's baseline and its slider are one
//!   transfer at their sum.
//! * **Its shape moves with the total.** White stays white from −1 to +6; a
//!   negative exposure lifts the top back towards white by up to a stop, and
//!   from −3 down the transfer is all but the plain gain; the mid-highs kink
//!   at every even exposure from 0 up. Between two measured rows the transfer
//!   is their blend in exposure: leaving any row out, its neighbours predict
//!   it to 0.66 L* at worst.
//! * **The channel rule.** The brightest and darkest channels go through the
//!   transfer and the middle one keeps its LINEAR place between them, in
//!   ProPhoto linear: ΔE median 0.10–0.14 on 288 colour patches at every
//!   exposure measured, against 38–96 at worst for per-channel, by the
//!   brightest channel or by luminance.
//! * **The over-range headroom.** Under an As Shot white balance whose camera
//!   neutral is not grey, the transfer sits further up: the greys render as
//!   the transfer at `E + h` on the input over `2^h`, and `h` depends on the
//!   neutral only through its largest channel over its smallest
//!   ([`HEADROOM_RATIO`] → [`HEADROOM_SHIFT`], read between the measured
//!   points). Measured from Camera Raw renders, 12 points, 2026-10-29: a
//!   synthetic camera whose matrix puts each neutral exactly at the profile's
//!   white (no white shift, so no colour cast), greys at 0 EV with an
//!   identity curve; that form explains every neutral's greys to rms
//!   0.11–0.13 L* (worst 0.27), the same as the grey neutral's own, where
//!   the transfer at `E + h` on the unscaled input misses by up to 10 L*.
//!   (1, 1, 0.4), (0.4, 1, 0.4), (0.399, 1, 0.593), (0.5, 1, 1) and (1, 0.5, 1)
//!   all read 0.995–1.000: the shift does not depend on which channel.
//!
//! Past the measured range the transfer is held: below −5 EV a plain gain on
//! the −5 row, above +6 the +6 row (a total past +6 needs a slider near its
//! +5 end on a bright body, and is not measured).

use super::ramp_table::{RAMP_E, RAMP_LOG2X, RAMP_RATIO};
use crate::recipe::CameraStart;

/// The columns of [`RAMP_RATIO`].
const RAMP_N: usize = RAMP_LOG2X.len();

/// The camera neutral's largest channel over its smallest at the measured
/// points (module head: 12 Camera Raw renders, 2026-10-29; the two renders
/// at 2.0 and the two near 2.5 are averaged into one point each).
const HEADROOM_RATIO: [f32; 10] = [1.0, 1.15, 1.30, 1.45, 1.80, 2.0, 2.3, 2.5, 3.0, 4.0];

/// The shift each ratio rendered at, in EV (fitted per render to 0.005 EV).
const HEADROOM_SHIFT: [f32; 10] = [0.0, 0.185, 0.360, 0.525, 0.835, 0.9975, 0.995, 0.995, 0.995, 0.995];

/// No measured shift reaches a stop; a stamped headroom past it is corrupt.
const HEADROOM_MAX: f32 = 1.0;

/// The white balance's over-range headroom, in stops, for a camera neutral:
/// the measured shift at its max/min ratio, linear between the measured
/// points and held past the last one (module head).
pub(crate) fn headroom_of(neutral: [f32; 3]) -> f32 {
    let (lo, hi) = neutral.iter().fold((f32::INFINITY, 0.0f32), |(a, b), &v| (a.min(v), b.max(v)));
    if !(lo > 0.0 && hi.is_finite()) {
        return 0.0;
    }
    let r = hi / lo;
    let last = HEADROOM_RATIO.len() - 1;
    if r >= HEADROOM_RATIO[last] {
        return HEADROOM_SHIFT[last];
    }
    let i = HEADROOM_RATIO.partition_point(|&v| v <= r).clamp(1, last) - 1;
    let f = ((r - HEADROOM_RATIO[i]) / (HEADROOM_RATIO[i + 1] - HEADROOM_RATIO[i])).clamp(0.0, 1.0);
    HEADROOM_SHIFT[i] + (HEADROOM_SHIFT[i + 1] - HEADROOM_SHIFT[i]) * f
}

/// Camera Raw's exposure transfer at one camera start: the measured rows that
/// bracket the total plus the headroom, blended in exposure, read on the
/// input over `2^headroom`. `x`, the white-balanced linear input relative to
/// white, goes to `F(x)`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Ramp {
    start: CameraStart,
    /// Below the lowest row: the plain gain left over.
    under: f32,
    /// The input's scale before the table: `2^-headroom`.
    pre: f32,
    /// `F` at the columns, `x = 2^RAMP_LOG2X[i]`.
    y: [f32; RAMP_N],
}

impl Ramp {
    /// The transfer a photograph's camera start ran at.
    pub(crate) fn of(start: CameraStart) -> Self {
        let ev = if start.ev.is_finite() { start.ev } else { 0.0 };
        let h = if start.headroom.is_finite() { start.headroom.clamp(0.0, HEADROOM_MAX) } else { 0.0 };
        let total = ev + h;
        let last = RAMP_E.len() - 1;
        let e = total.clamp(RAMP_E[0], RAMP_E[last]);
        let j = RAMP_E.partition_point(|&v| v <= e).clamp(1, last) - 1;
        let f = ((e - RAMP_E[j]) / (RAMP_E[j + 1] - RAMP_E[j])).clamp(0.0, 1.0);
        let mut y: [f32; RAMP_N] = std::array::from_fn(|i| {
            let x = RAMP_LOG2X[i].exp2();
            let a = RAMP_RATIO[j][i] * x * RAMP_E[j].exp2();
            let b = RAMP_RATIO[j + 1][i] * x * RAMP_E[j + 1].exp2();
            a + (b - a) * f
        });
        // The table states each ratio to five places, so deep in the toe two
        // neighbouring columns can come out a rounding step apart the wrong
        // way (3.3964e-6 then 3.3963e-6 at +1.35 EV); the transfer is
        // monotone, and its inverse searches it as such.
        for i in 1..RAMP_N {
            y[i] = y[i].max(y[i - 1]);
        }
        let under = if total < RAMP_E[0] { (total - RAMP_E[0]).exp2() } else { 1.0 };
        Ramp { start: CameraStart { ev, headroom: h, tables: start.tables }, under, pre: (-h).exp2(), y }
    }

    /// The camera start this transfer is.
    pub(crate) fn start(&self) -> CameraStart {
        self.start
    }

    /// `F(x)` for an input `x` relative to white. Past white (over the
    /// headroom) the input is held at white's output.
    pub(crate) fn transfer(&self, x: f32) -> f32 {
        let x = x * self.pre;
        if x <= 0.0 || !x.is_finite() {
            return 0.0;
        }
        let c = x.log2();
        let y = if c >= 0.0 {
            self.y[RAMP_N - 1]
        } else if c <= RAMP_LOG2X[0] {
            self.y[0] * x / RAMP_LOG2X[0].exp2()
        } else {
            let i = RAMP_LOG2X.partition_point(|&v| v <= c).clamp(1, RAMP_N - 1) - 1;
            let f = (c - RAMP_LOG2X[i]) / (RAMP_LOG2X[i + 1] - RAMP_LOG2X[i]);
            self.y[i] + (self.y[i + 1] - self.y[i]) * f
        };
        y * self.under
    }

    /// [`Ramp::transfer`]'s inverse: the input that renders `y`. An output
    /// at or under the transfer's floor answers 0; one at or past white's
    /// output answers white (over the headroom).
    pub(crate) fn input_of(&self, y: f32) -> f32 {
        let y = y / self.under;
        if y <= 0.0 || !y.is_finite() {
            return 0.0;
        }
        let x = if y >= self.y[RAMP_N - 1] {
            1.0
        } else if y <= self.y[0] {
            if self.y[0] > 0.0 { RAMP_LOG2X[0].exp2() * y / self.y[0] } else { 0.0 }
        } else {
            let i = self.y.partition_point(|&v| v <= y).clamp(1, RAMP_N - 1) - 1;
            let span = self.y[i + 1] - self.y[i];
            let f = if span > 0.0 { (y - self.y[i]) / span } else { 0.0 };
            (RAMP_LOG2X[i] + (RAMP_LOG2X[i + 1] - RAMP_LOG2X[i]) * f).exp2()
        };
        x / self.pre
    }

    /// [`Ramp::transfer`] on a linear triple by the channel rule (module
    /// head): the brightest and darkest channels through the transfer, the
    /// middle one at its linear place between them.
    pub(crate) fn transfer_rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        by_rule(rgb, |v| self.transfer(v))
    }

    /// [`Ramp::transfer_rgb`]'s inverse below white: the brightest and
    /// darkest channels back through the transfer, the middle one at the
    /// place the rule kept.
    pub(crate) fn input_rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        by_rule(rgb, |v| self.input_of(v))
    }
}

/// `f` on the brightest and darkest channels of `rgb`, the middle one at its
/// linear place between them.
fn by_rule(rgb: [f32; 3], f: impl Fn(f32) -> f32) -> [f32; 3] {
    let (mut hi, mut mid, mut lo) = (0usize, 1usize, 2usize);
    if rgb[hi] < rgb[mid] {
        std::mem::swap(&mut hi, &mut mid);
    }
    if rgb[mid] < rgb[lo] {
        std::mem::swap(&mut mid, &mut lo);
    }
    if rgb[hi] < rgb[mid] {
        std::mem::swap(&mut hi, &mut mid);
    }
    let (a, b) = (f(rgb[hi]), f(rgb[lo]));
    let span = rgb[hi] - rgb[lo];
    let mut out = [0.0f32; 3];
    out[hi] = a;
    out[mid] = if span > 1e-12 { b + (a - b) * (rgb[mid] - rgb[lo]) / span } else { b };
    out[lo] = b;
    out
}

/// Camera Raw's Exposure slider on a frame the profile stage rendered at a
/// camera start: the light taken back under the start's transfer and put
/// through the transfer at the start plus `ev` — one transfer at the total
/// (module head). The measured transfer carries its own gain (`F(x; E)` is
/// `x · 2^E` times the measured ratio), so the light is NOT multiplied as
/// well: that would apply the slider twice.
#[derive(Clone, Debug)]
pub(crate) struct Exposure {
    from: Ramp,
    to: Ramp,
}

impl Exposure {
    /// `ev` on a frame started at `start`; `None` at 0 EV.
    pub(crate) fn new(start: CameraStart, ev: f32) -> Option<Self> {
        (ev != 0.0 && ev.is_finite()).then(|| Exposure {
            from: Ramp::of(start),
            to: Ramp::of(CameraStart { ev: start.ev + ev, ..start }),
        })
    }

    /// The camera start of the exposed frame: the start plus the slider.
    pub(crate) fn exposed(&self) -> CameraStart {
        self.to.start()
    }

    /// On one grey value after the start's transfer (linear, relative to
    /// white).
    pub(crate) fn grey(&self, y: f32) -> f32 {
        self.to.transfer(self.from.input_of(y))
    }

    /// On a linear triple after the start's transfer, in the stage's
    /// reference primaries, by the channel rule.
    pub(crate) fn rgb(&self, y: [f32; 3]) -> [f32; 3] {
        self.to.transfer_rgb(self.from.input_rgb(y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The transfer reads the measured rows exactly at a row's exposure, and
    /// the headroom is the row `h` stops up on the input over `2^h`.
    ///
    /// MUTATION: read the row at `E` alone under a headroom, or blend the
    /// rows in linear gain instead of exposure.
    #[test]
    fn the_transfer_is_the_measured_row() {
        let j = RAMP_E.iter().position(|&e| e == 1.0).expect("a +1 row");
        let r = Ramp::of(CameraStart { ev: 1.0, headroom: 0.0, tables: 0 });
        for i in (10..RAMP_N).step_by(7) {
            let x = RAMP_LOG2X[i].exp2();
            let want = RAMP_RATIO[j][i] * x * 2.0;
            assert!((r.transfer(x) - want).abs() <= 1e-5 * want.max(1e-3), "x {x}: {} vs {want}", r.transfer(x));
        }
        let up = Ramp::of(CameraStart { ev: 0.5, headroom: 0.5, tables: 0 });
        for &x in &[0.01f32, 0.1, 0.5, 1.5] {
            assert!((up.transfer(x) - r.transfer(x * 0.5f32.sqrt())).abs() < 1e-6, "headroom at {x}");
        }
        // The measured points themselves, and a ratio past the last one.
        assert!((headroom_of([1.0, 1.0, 1.0 / 1.15]) - 0.185).abs() < 1e-4);
        assert!((headroom_of([0.4, 1.0, 0.4]) - 0.995).abs() < 1e-4, "{}", headroom_of([0.4, 1.0, 0.4]));
        assert_eq!(headroom_of([1.0, 1.0, 0.1]), 0.995);
        assert_eq!(headroom_of([1.0; 3]), 0.0);
    }

    /// The triple keeps its middle ratio, the inverse undoes the transfer
    /// below white, and an exposure is one transfer at the total: +0.5 then
    /// +0.5 is +1, and +1 doubles a dark grey (the toe's ratio barely moves
    /// there), not quadruples it.
    ///
    /// MUTATION: multiply the light by `2^ev` before the exposed transfer.
    #[test]
    fn the_triple_inverts_and_exposures_add() {
        let start = CameraStart { ev: 0.35, headroom: 1.0, tables: 0 };
        let r = Ramp::of(start);
        let px = [0.30f32, 0.12, 0.05];
        let out = r.transfer_rgb(px);
        let ratio = |p: [f32; 3]| (p[1] - p[2]) / (p[0] - p[2]);
        assert!((ratio(out) - ratio(px)).abs() < 1e-5);
        let back = r.input_rgb(out);
        for k in 0..3 {
            assert!((back[k] - px[k]).abs() < 2e-3 * px[k], "channel {k}: {back:?}");
        }
        let half = Exposure::new(start, 0.5).expect("an exposure");
        let again = Exposure::new(half.exposed(), 0.5).expect("an exposure");
        let one = Exposure::new(start, 1.0).expect("an exposure");
        for &y in &[0.02f32, 0.18, 0.6] {
            assert!((again.grey(half.grey(y)) - one.grey(y)).abs() < 2e-3 * one.grey(y), "at {y}");
        }
        let up = one.grey(0.05) / 0.05;
        assert!((1.8..2.2).contains(&up), "+1 EV on a dark grey: x{up}");
        assert!(Exposure::new(start, 0.0).is_none());
    }
}
