//! The camera profile stage (v1.5.0, F7): Adobe's `.dcp` tables, rendered.
//!
//! # Where this sits
//!
//! Between the camera→working-space matrix and the working transfer encode, in
//! LINEAR light — the only place the profile's tables mean what they were
//! authored to mean. Everything downstream (exposure, tone, the mixer, the
//! masks) sees a frame that already carries the profile, which is the order
//! Lightroom's own panel implies: the profile establishes the rendering and the
//! sliders adjust from there.
//!
//! The order WITHIN the stage is Adobe's, taken from RawTherapee's long-lived
//! implementation of the same format (`rtengine/dcp.cc`, `DCPProfile::apply`
//! and `step2ApplyTile`): reference-space matrix → HSV → HueSatMap → RGB →
//! baseline-exposure scale → HSV → LookTable → RGB → ProfileToneCurve.
//!
//! # What is rendered, and the one thing that is not
//!
//! The tables and the tone curve are rendered. The profile's own COLOUR MATRIX
//! is deliberately not adopted: the engine's camera→XYZ transform stays the one
//! it has always used, the one its calibration lane and its base-curve
//! estimator were measured against. The tables are what distinguishes one
//! profile from another — "Adobe Standard" and "Camera Vivid" for the same body
//! carry the same colorimetry and different tables — so rendering them is what
//! makes the name mean something, while swapping the matrix underneath would
//! move every pixel of every photograph for a difference the profile name is
//! not about. Stated as a deviation rather than hidden: if the Lightroom kit
//! shows the matrix half matters, [`Stage::build`] is where it changes.
//!
//! # The reference space
//!
//! Adobe authors these tables in ProPhoto RGB's primaries, so the stage
//! converts into them and back. The round trip is exact in the absence of a
//! table (the two matrices are inverses), which is what lets a profile with no
//! tables cost nothing but arithmetic.

use crate::dcp::{Profile, Table};

/// ProPhoto RGB (ROMM) primaries — the DNG specification's reference space, and
/// the space Adobe's tables are authored in.
///
/// Built against the WORKING space's white rather than ProPhoto's own D50.
/// That is a chromatic adaptation, written as one argument instead of a
/// Bradford matrix, and it is not optional: these tables are indexed by HSV, so
/// a SATURATION of zero has to mean the same colour on both sides of the
/// conversion. Pairing ProPhoto's D50 with the working space's D65 and calling
/// them one XYZ turns a neutral into a tinted triple on the way in — measured:
/// a grey through a table whose saturation scale is 0 came back
/// [0.560, 0.464, 0.343] instead of grey. Mapping white to white is what a
/// white-balanced frame means, and it is what makes the achromatic axis exact.
const PROPHOTO_PRIM: [[f32; 2]; 3] = [[0.7347, 0.2653], [0.1596, 0.8404], [0.0366, 0.0001]];

/// How many samples the tone curve is resolved to. The profiles that carry one
/// state 128 knots over [0, 1]; a 1024-entry table is eight samples per knot,
/// which is past the point where the interpolation shows.
const TONE_LUT: usize = 1024;

/// A camera profile, prepared for one photograph.
///
/// Built once per render and then asked per pixel, so everything that depends
/// only on the profile and the white balance — the illuminant blend, the tone
/// lookup, the two matrices — is resolved here rather than in the loop.
pub(crate) struct Stage {
    /// Working space → ProPhoto linear, and back.
    to_ref: [[f32; 3]; 3],
    from_ref: [[f32; 3]; 3],
    hue_sat: Option<Table>,
    look: Option<Table>,
    tone: Option<Vec<f32>>,
    /// `2^BaselineExposureOffset`, as a linear gain.
    gain: f32,
}

impl Stage {
    /// Prepare `profile` for a frame in `space`, or `None` when the profile
    /// carries nothing this stage can act on.
    ///
    /// `kelvin` is the frame's own white balance, which selects between the
    /// profile's two calibrations. Absent — a file with no usable white balance
    /// — takes the FIRST hue/sat map, because a profile that ships only one
    /// ships it as Data1 and guessing a temperature to blend with would be
    /// inventing a measurement.
    pub(crate) fn build(
        profile: &Profile,
        space: super::ExportColorSpace,
        kelvin: Option<f32>,
    ) -> Option<Self> {
        let hue_sat = blend_calibrations(profile, kelvin);
        let look = profile.look_table.clone();
        let tone = tone_lut(&profile.tone_curve);
        let gain = if profile.baseline_exposure_offset == 0.0 {
            1.0
        } else {
            profile.baseline_exposure_offset.exp2()
        };
        if hue_sat.is_none() && look.is_none() && tone.is_none() && gain == 1.0 {
            return None;
        }
        // Working RGB → XYZ (D65-adapted, as the export spaces are) → ProPhoto.
        let space_to_xyz = super::rgb_to_xyz(super::space_primaries(space), super::D65_XY);
        let ref_to_xyz = super::rgb_to_xyz(PROPHOTO_PRIM, super::D65_XY);
        let to_ref = super::mat_mul3(&super::inv3(&ref_to_xyz), &space_to_xyz);
        Some(Stage { from_ref: super::inv3(&to_ref), to_ref, hue_sat, look, tone, gain })
    }

    /// Apply the profile to one LINEAR working-space pixel, in place.
    pub(crate) fn apply(&self, px: &mut [f32; 3]) {
        let mut r = super::mat_vec3(&self.to_ref, px);
        // HSV is undefined for a negative component, and ProPhoto's gamut is
        // wide enough that a real photograph almost never has one here. The
        // clamp is a guard, not a gamut decision: it keeps a stray negative out
        // of the hue arithmetic instead of producing a colour from a sign.
        for c in &mut r {
            *c = c.max(0.0);
        }
        if let Some(t) = &self.hue_sat {
            let mut hsv = to_hsv(r);
            t.apply(&mut hsv);
            r = from_hsv(hsv);
        }
        if self.gain != 1.0 {
            for c in &mut r {
                *c *= self.gain;
            }
        }
        if let Some(t) = &self.look {
            let mut hsv = to_hsv(r);
            t.apply(&mut hsv);
            r = from_hsv(hsv);
        }
        if let Some(lut) = &self.tone {
            r = adobe_tone(r, lut);
        }
        *px = super::mat_vec3(&self.from_ref, &r);
    }
}

/// The hue/sat map for this frame's white balance.
///
/// A profile with two calibrations states them at two illuminants — Standard A
/// (2856 K) and D65 (6504 K) on every Sony profile measured — and Adobe blends
/// between them in RECIPROCAL temperature, which is the axis the two ends are
/// evenly spaced on. One calibration is used as itself.
fn blend_calibrations(profile: &Profile, kelvin: Option<f32>) -> Option<Table> {
    let (a, b) = (profile.hue_sat_map[0].as_ref(), profile.hue_sat_map[1].as_ref());
    match (a, b) {
        (Some(a), Some(b)) if a.hue == b.hue && a.sat == b.sat && a.val == b.val => {
            let w = illuminant_weight(profile, kelvin);
            if w <= 0.0 {
                return Some(a.clone());
            }
            if w >= 1.0 {
                return Some(b.clone());
            }
            let data = a
                .data
                .iter()
                .zip(&b.data)
                .map(|(x, y)| {
                    [
                        x[0] + (y[0] - x[0]) * w,
                        x[1] + (y[1] - x[1]) * w,
                        x[2] + (y[2] - x[2]) * w,
                    ]
                })
                .collect();
            Some(Table { data, ..a.clone() })
        }
        // Two tables of different shapes cannot be blended entry by entry, and
        // interpolating one onto the other's grid would be inventing samples.
        // Adobe ships none like that; if one appears, the first calibration is
        // used whole rather than a fabricated mixture.
        (Some(a), _) => Some(a.clone()),
        (None, b) => b.cloned(),
    }
}

/// Where this frame's white balance sits between the profile's two
/// calibrations: 0 = the first, 1 = the second.
fn illuminant_weight(profile: &Profile, kelvin: Option<f32>) -> f32 {
    let (Some(k), Some(t1), Some(t2)) = (
        kelvin.filter(|k| k.is_finite() && *k > 1000.0),
        profile.illuminant[0].and_then(illuminant_kelvin),
        profile.illuminant[1].and_then(illuminant_kelvin),
    ) else {
        return 0.0;
    };
    if (t1 - t2).abs() < 1.0 {
        return 0.0;
    }
    let (i, i1, i2) = (1.0 / k, 1.0 / t1, 1.0 / t2);
    ((i - i1) / (i2 - i1)).clamp(0.0, 1.0)
}

/// The EXIF light-source codes a camera profile actually uses, as correlated
/// colour temperatures.
///
/// Measured on the installed pool rather than transcribed whole: every Sony,
/// Canon, Apple and Leica profile read carries 17 (Standard A) and 21 (D65).
/// The daylight family is included because it costs nothing and a third-party
/// profile may state it; a code outside the list answers `None` and the frame
/// takes the first calibration rather than a guessed temperature.
fn illuminant_kelvin(code: u16) -> Option<f32> {
    Some(match code {
        1 | 4 | 9 => 5500.0,  // Daylight, Flash, Fine weather
        3 | 17 => 2856.0,     // Tungsten, Standard light A
        10 => 6000.0,         // Cloudy
        11 => 7500.0,         // Shade
        18 => 5500.0,         // Standard light B
        19 => 6500.0,         // Standard light C
        20 => 5503.0,         // D55
        21 => 6504.0,         // D65
        22 => 7504.0,         // D75
        23 => 5003.0,         // D50
        _ => return None,
    })
}

/// The profile's tone curve as a lookup over [0, 1], or `None` for a profile
/// that states none or states an identity.
fn tone_lut(knots: &[[f32; 2]]) -> Option<Vec<f32>> {
    if knots.len() < 2 {
        return None;
    }
    let mut pts: Vec<[f32; 2]> =
        knots.iter().filter(|k| (0.0..=1.0).contains(&k[0])).copied().collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    pts.dedup_by(|a, b| a[0] == b[0]);
    if pts.len() < 2 {
        return None;
    }
    let lut: Vec<f32> = (0..TONE_LUT)
        .map(|i| {
            let x = i as f32 / (TONE_LUT - 1) as f32;
            let j = pts.partition_point(|p| p[0] <= x);
            match (j.checked_sub(1).and_then(|k| pts.get(k)), pts.get(j)) {
                (Some(a), Some(b)) => a[1] + (b[1] - a[1]) * (x - a[0]) / (b[0] - a[0]),
                (Some(a), None) => a[1],
                (None, Some(b)) => b[1],
                (None, None) => x,
            }
        })
        .collect();
    // An identity curve is not worth a per-pixel pass.
    lut.iter()
        .enumerate()
        .any(|(i, v)| (v - i as f32 / (TONE_LUT - 1) as f32).abs() > 1e-4)
        .then_some(lut)
}

/// Adobe's own way of putting a tone curve on a colour: the curve acts on the
/// BRIGHTEST and DARKEST channels, and the middle one is placed back at the
/// same relative position between them.
///
/// Per-channel would be the obvious reading and it is the wrong one — it
/// desaturates as it brightens, because the three channels climb the curve's
/// shoulder at different rates. Taken from RawTherapee's `AdobeToneCurve`
/// (`rtengine/curves.h`), which has rendered this format for over a decade.
fn adobe_tone(rgb: [f32; 3], lut: &[f32]) -> [f32; 3] {
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
    let (a, b) = (sample(lut, rgb[hi]), sample(lut, rgb[lo]));
    let span = rgb[hi] - rgb[lo];
    let m = if span > 1e-9 { b + (a - b) * (rgb[mid] - rgb[lo]) / span } else { b };
    let mut out = [0.0f32; 3];
    out[hi] = a;
    out[mid] = m;
    out[lo] = b;
    out
}

/// The curve at `x`, linearly between samples. Values above 1 keep the curve's
/// slope at the top rather than clipping: a highlight outside the working gamut
/// is exactly what the wide-gamut path exists to carry.
fn sample(lut: &[f32], x: f32) -> f32 {
    let n = lut.len();
    if x <= 0.0 {
        return lut[0] * x.max(0.0);
    }
    if x >= 1.0 {
        return lut[n - 1] * x;
    }
    let s = x * (n - 1) as f32;
    let i = s.floor() as usize;
    let f = s - i as f32;
    lut[i] + (lut[(i + 1).min(n - 1)] - lut[i]) * f
}

/// RGB → (hue degrees, saturation, value), the plain HSV the DNG tables index.
fn to_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
    let v = r.max(g).max(b);
    let m = r.min(g).min(b);
    let c = v - m;
    if c <= 0.0 || v <= 0.0 {
        return [0.0, 0.0, v.max(0.0)];
    }
    let h = if v == r {
        ((g - b) / c).rem_euclid(6.0)
    } else if v == g {
        (b - r) / c + 2.0
    } else {
        (r - g) / c + 4.0
    };
    [(h * 60.0).rem_euclid(360.0), c / v, v]
}

fn from_hsv(hsv: [f32; 3]) -> [f32; 3] {
    let (h, s, v) = (hsv[0].rem_euclid(360.0) / 60.0, hsv[1].clamp(0.0, 1.0), hsv[2].max(0.0));
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

#[cfg(test)]
mod tests;
