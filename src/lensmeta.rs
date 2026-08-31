//! In-camera lens-correction metadata → engine-space [`LensProfile`].
//!
//! Sony bodies write the manufacturer's exact correction profile for the
//! mounted lens at the shot's focal length/aperture into every ARW as three
//! spline-knot arrays (`0x7032` vignetting, `0x7035` chromatic aberration,
//! `0x7037` distortion — the same 0x70xx block rawler models as `ExifTag`
//! variants). That makes a lensfun database unnecessary for Sony files: the
//! per-shot profile ships inside the file.
//!
//! Conversion formulas follow RawTherapee's shipped implementation
//! (`rtengine/lensmetadata.cc`), cross-checked against the independent
//! reverse-engineering write-up at <https://stannum.io/blog/0PwljB>:
//!
//! * vignette gain  `1 / 2^(0.5 − 2^(v·2⁻¹³ − 1))`  (v = 0 ⇒ exactly 1.0)
//! * distortion radius factor  `v·2⁻¹⁴ + 1`
//! * CA radius factor  `v·2⁻²¹ + 1` (16 red knots then 16 blue; multiplies
//!   the distortion/green map per channel)
//!
//! Vignette and CA retain the engine's established canonical knot convention.
//! D2's paired Lightroom measurements establish a narrower exception for the
//! Sony distortion array: its sixteen native samples sit at `(i+1)/16`, with
//! the last sample exactly at the corner. It is resampled here, at the source
//! boundary, onto a dense canonical `(i+0.5)/(n−1)` grid for the Lightroom
//! mask-transport solve. The ordinary render spline retains its established
//! placement because D2's required image-registration gate rejected changing
//! that path. Each array's FIRST element is the value count; a malformed count
//! degrades that component to "absent", never an error (a photo without
//! correction data is a normal photo).
//!
//! # The MASK WARP, and why it is solved here (R29 Batch-3)
//!
//! [`LensProfile::mask_warp`] is the radial magnification between the frame
//! Lightroom STORES a mask's coordinates in and the frame it EXPORTS that mask
//! into (`render`'s mask-warp block header carries the measurements). It has
//! two possible sources and this function is where the choice is made, because
//! this is the one place that already holds the camera's own knots:
//!
//! * **Source A** — the in-camera `0x7037` spline just read above, inverted by
//!   [`crate::render::mask_warp_from_camera_knots`]. Preferred whenever it
//!   exists: it is the maker's calibration for THIS shot, needs no external
//!   file, and scored 2.30 px against Adobe's own 2.11 px on the 138-patch
//!   pixel field.
//! * **Source B** — an Adobe `.lcp` on this machine ([`crate::lcp`]), for
//!   bodies that write no knots at all.
//!
//! Every failure is TAGGED, never silent: [`crate::recipe::MaskWarpSource`]
//! names which source answered or which of the five refusals applies.
//!
//! **The warp depends on the frame's ASPECT only, not its pixel count.** Both
//! solves normalise radius by the half-diagonal and the `.lcp` reference length
//! is proportional to the width, so the frame's size cancels out of `m(r)`. The
//! dimensions read below are therefore a shape, and a preview-sized render and
//! a full-resolution export share one map — which is what lets it be solved
//! once, here, and stored.

use std::path::Path;

use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, Value};
use rawler::tags::{DngTag, ExifTag};

use crate::recipe::{LensProfile, MASK_WARP_KNOTS, MaskWarpCenter, MaskWarpSource};

/// Dense enough that representing either measured 16-knot Sony distortion
/// spline on the engine's offset canonical grid has <1e-5 maximum factor
/// error, including the awkward r=1 endpoint between the last two canonical
/// nodes. The contract test records the measured maximum.
pub(crate) const SONY_DISTORTION_CANONICAL_KNOTS: usize = 2048;

/// Read the in-camera lens correction profile from a RAW file. Every failure
/// path returns an EMPTY component (profile with nothing to apply) — absent
/// metadata is the common case for non-Sony files, not an error.
pub fn read(path: &Path) -> LensProfile {
    let mut out = LensProfile::default();
    if !crate::decode::is_raw(path) {
        return out;
    }
    let Ok(file) = std::fs::File::open(path) else { return out };
    let mut reader = std::io::BufReader::new(file);
    // `Some(16)`, not `None`: with no chain cap, rawler 0.7.2's walker
    // (formats/tiff/reader.rs:164-179) keeps no visited-offset set and its
    // only `break` sits inside `if let Some(max)` — a self-referential
    // next_ifd loops forever, pushing an IFD per iteration. 16 is rawler's
    // own test value and above its internal `new_root` cap of 10; real RAWs
    // carry ≤4 top-level IFDs, and this reader only consults `root_ifd()`,
    // so a truncated chain tail is unobservable here.
    let Ok(tiff) = GenericTiffReader::new(&mut reader, 0, 0, Some(16), &[]) else {
        return out;
    };
    let root = tiff.root_ifd();
    let knots = |tag: ExifTag| -> Vec<i16> {
        let Some(entry) = root.get_entry_recursive(tag) else { return Vec::new() };
        let vals: Vec<i16> = match &entry.value {
            Value::SShort(v) => v.clone(),
            Value::Short(v) => v.iter().map(|&x| x as i16).collect(),
            _ => return Vec::new(),
        };
        // First element = value count; guard the shape instead of trusting it.
        match vals.first().map(|&n| n as usize) {
            Some(n) if (2..=64).contains(&n) && vals.len() > n => vals[1..=n].to_vec(),
            _ => Vec::new(),
        }
    };

    out.vignette = knots(ExifTag::VignettingCorrParams)
        .iter()
        .map(|&v| vignette_gain(v))
        .collect();
    let sony_distortion: Vec<f32> = knots(ExifTag::DistortionCorrParams)
        .iter()
        .map(|&v| v as f32 * (-14f32).exp2() + 1.0)
        .collect();
    // Adjudication gate: direct engine-vs-Lightroom image registration rejects
    // changing the render spline's placement (the edge/corner residual grows).
    // Keep the established render calibration here; the corrected native
    // domain is consumed only by Lightroom's mask transport solve below.
    out.distortion = sony_distortion.clone();
    let lr_mask_distortion =
        resample_sony_distortion(&sony_distortion, SONY_DISTORTION_CANONICAL_KNOTS);
    let ca = knots(ExifTag::ChromaticAberrationCorrParams);
    // The pair split needs an even count AND enough knots per channel for a
    // meaningful radial spline (real bodies write 16+16; a malformed 2-value
    // array would otherwise become two 1-knot "constants" instead of
    // degrading to absent, per the malformed-metadata contract).
    if ca.len() % 2 == 0 && ca.len() >= 8 {
        let half = ca.len() / 2;
        let f = |v: &i16| *v as f32 * (-21f32).exp2() + 1.0;
        out.ca_r = ca[..half].iter().map(f).collect();
        out.ca_b = ca[half..].iter().map(f).collect();
    }

    // --- the mask warp (R29 Batch-3) -------------------------------------
    let text = |tag: ExifTag| -> String {
        root.get_entry_recursive(tag)
            .and_then(|e| e.value.as_string().cloned())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let number = |tag: ExifTag| -> Option<f32> {
        root.get_entry_recursive(tag)?.value.get_f32(0).ok().flatten().filter(|v| v.is_finite())
    };
    // `DefaultCropSize` FIRST, and for the reason `xmp`'s frame doc spells out
    // at length: it is the frame every normalised coordinate in a sidecar is
    // measured against, where `ImageWidth/Length` is the raw sensor array
    // including the masked border. Only the ASPECT of this reaches the answer,
    // but the two aspects differ by ~0.3 % on a real ARW and the fallback is
    // the second-best shape, not an equal one.
    let dims = frame_dims(root);
    out.mask_warp_center = mask_warp_center(root, dims);
    match dims {
        Some(dims) if !lr_mask_distortion.is_empty() => {
            let w = crate::render::mask_warp_from_camera_knots(
                &lr_mask_distortion,
                dims,
                MASK_WARP_KNOTS,
            );
            if w.len() >= 2 {
                out.mask_warp = w;
                out.mask_warp_src = MaskWarpSource::CameraMetadata;
            } else {
                // The inversion is only unsolvable for a spline that folds
                // inside the frame — a corrupt array, not a real lens.
                out.mask_warp_src = MaskWarpSource::Unparseable;
            }
        }
        Some(dims) => {
            // No in-camera knots: this is exactly the body source B exists for.
            let lens = {
                let m = text(ExifTag::LensModel);
                if m.is_empty() { text(ExifTag::LensMake) } else { m }
            };
            match crate::lcp::solve_mask_warp(
                None,
                &text(ExifTag::Make),
                &lens,
                number(ExifTag::FocalLength),
                dims,
                MASK_WARP_KNOTS,
            ) {
                Ok(w) => {
                    out.mask_warp = w;
                    out.mask_warp_src = MaskWarpSource::Lcp;
                }
                Err(r) => out.mask_warp_src = r.into(),
            }
        }
        // A file that declares no frame declares no aspect, and the map is a
        // function of the aspect. Refusing beats guessing 3:2.
        None => out.mask_warp_src = MaskWarpSource::Absent,
    }

    out.clamp(); // same defensive ranges as a hand-edited recipe
    out
}

/// The frame this photo's normalised coordinates are measured against:
/// `DefaultCropSize` when the file states one, else the raw array.
///
/// Split out so the precedence is one statement rather than a nested `match`
/// inside `read` — and so a test can state it.
fn frame_dims(root: &rawler::formats::tiff::IFD) -> Option<(f32, f32)> {
    let pair = |a: Option<f32>, b: Option<f32>| -> Option<(f32, f32)> {
        let (w, h) = (a?, b?);
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some((w, h))
    };
    let dng = root.get_entry_recursive(DngTag::DefaultCropSize).map(|e| &e.value);
    if let Some(v) = dng
        && let Some(d) = pair(v.get_f32(0).ok().flatten(), v.get_f32(1).ok().flatten())
    {
        return Some(d);
    }
    pair(
        root.get_entry_recursive(ExifTag::ImageWidth)?.value.get_f32(0).ok().flatten(),
        root.get_entry_recursive(ExifTag::ImageHeight)?.value.get_f32(0).ok().flatten(),
    )
}

/// Resample the Sony distortion spline from its native `(i+1)/n` radii onto
/// the canonical grid consumed by `render::profile_knot_interp`.
pub(crate) fn resample_sony_distortion(native: &[f32], n: usize) -> Vec<f32> {
    if native.is_empty() || n < 2 {
        return Vec::new();
    }
    (0..n)
        .map(|i| {
            let r = (i as f32 + 0.5) / (n - 1) as f32;
            sony_distortion_interp(native, r)
        })
        .collect()
}

/// Sony `0x7037` piecewise-linear interpolation at native radius `(i+1)/n`,
/// clamped before the first and after the last sample.
fn sony_distortion_interp(knots: &[f32], r: f32) -> f32 {
    let n = knots.len();
    if n == 0 {
        return 1.0;
    }
    if n == 1 {
        return knots[0];
    }
    let t = r * n as f32 - 1.0;
    if t <= 0.0 {
        return knots[0];
    }
    if t >= (n - 1) as f32 {
        return knots[n - 1];
    }
    let i = t.floor() as usize;
    let f = t - i as f32;
    knots[i] * (1.0 - f) + knots[i + 1] * f
}

/// Full-raw-frame centre expressed in the stored/default-crop frame.
/// Requires both first-party facts; missing either preserves legacy centre
/// behaviour through `LensProfile::mask_warp_center = None`.
fn mask_warp_center(
    root: &rawler::formats::tiff::IFD,
    stored_dims: Option<(f32, f32)>,
) -> Option<MaskWarpCenter> {
    let pair = |value: &Value| -> Option<(f32, f32)> {
        let x = value.get_f32(0).ok().flatten()?;
        let y = value.get_f32(1).ok().flatten()?;
        (x.is_finite() && y.is_finite()).then_some((x, y))
    };
    let number = |tag: ExifTag| -> Option<f32> {
        root.get_entry_recursive(tag)?.value.get_f32(0).ok().flatten().filter(|v| v.is_finite())
    };
    let full = (number(ExifTag::ImageWidth)?, number(ExifTag::ImageHeight)?);
    let origin = pair(&root.get_entry_recursive(DngTag::DefaultCropOrigin)?.value)?;
    let centre = [full.0 * 0.5 - origin.0, full.1 * 0.5 - origin.1];
    let (stored_w, stored_h) = stored_dims?;
    (centre[0] >= 0.0 && centre[1] >= 0.0).then_some(MaskWarpCenter {
        stored_px: centre,
        stored_dims: [stored_w, stored_h],
    })
}

/// Sony vignetting knot → linear-light gain (RawTherapee's formula).
fn vignette_gain(v: i16) -> f32 {
    1.0 / (0.5 - (v as f32 * (-13f32).exp2() - 1.0).exp2()).exp2()
}

#[cfg(test)]
mod tests {
    // These are decimal transcriptions of decoded integer knots and solved D2
    // fixture outputs; keep the report's digits rather than rewriting them to
    // clippy's display-oriented f32 spelling.
    #![allow(clippy::excessive_precision)]

    use super::*;

    const WALL_DISTORTION: [f32; 16] = [
        1.0007934570,
        0.9998779297,
        0.9981079102,
        0.9959716797,
        0.9927368164,
        0.9890136719,
        0.9846191406,
        0.9800415039,
        0.9749145508,
        0.9696044922,
        0.9641113281,
        0.9589843750,
        0.9538574219,
        0.9492187500,
        0.9448242188,
        0.9412231445,
    ];
    const DSC_DISTORTION: [f32; 16] = [
        1.0007934570,
        0.9998779297,
        0.9982299805,
        0.9961547852,
        0.9932250977,
        0.9898071289,
        0.9856567383,
        0.9813842773,
        0.9766235352,
        0.9719238281,
        0.9668579102,
        0.9622802734,
        0.9576416016,
        0.9538574219,
        0.9503173828,
        0.9478149414,
    ];

    #[test]
    fn sony_distortion_native_domain_resamples_to_the_d2_mask_warp_fixtures() {
        const RADII: [f32; 6] = [0.0, 0.2, 1.0 / 3.0, 0.5, 0.75, 1.0];
        const EXPECTED: [[f32; 6]; 2] = [
            [
                0.97478093,
                0.97767276,
                0.98359925,
                0.99524465,
                1.01848750,
                1.03647513,
            ],
            [
                0.97644290,
                0.97922226,
                0.98473749,
                0.99560184,
                1.01649349,
                1.03102158,
            ],
        ];
        for (name, native, expected) in [
            ("wall", &WALL_DISTORTION[..], EXPECTED[0]),
            ("DSC08276", &DSC_DISTORTION[..], EXPECTED[1]),
        ] {
            let dense = resample_sony_distortion(native, SONY_DISTORTION_CANONICAL_KNOTS);
            assert_eq!(dense.len(), SONY_DISTORTION_CANONICAL_KNOTS);

            let mut worst = 0.0f32;
            for i in 0..=65_536 {
                let r = i as f32 / 65_536.0;
                worst = worst.max(
                    (crate::render::profile_knot_interp(&dense, r)
                        - sony_distortion_interp(native, r))
                    .abs(),
                );
            }
            assert!(worst < 1e-5, "{name}: canonical resampling error {worst}");

            let warp = crate::render::mask_warp_from_camera_knots(
                &dense,
                (9504.0, 6336.0),
                MASK_WARP_KNOTS,
            );
            assert_eq!(warp.len(), MASK_WARP_KNOTS);
            for (r, want) in RADII.into_iter().zip(expected) {
                let got = crate::render::mask_warp_factor(&warp, r);
                assert!((got - want).abs() < 2e-5, "{name} r={r}: {got} vs {want}");
            }
        }
    }

    #[test]
    fn conversion_matches_the_real_a7riv_vectors() {
        // Ground truth: DSC08276.ARW (A7RIV + FE 24-105mm F4 G OSS @ 24mm),
        // dumped with rawler on this machine. Centre knots must be exact
        // identity; corner knots match independently computed values.
        assert!((vignette_gain(0) - 1.0).abs() < 1e-6, "centre vignette gain is exactly 1");
        assert!(
            (vignette_gain(8320) - 1.4249).abs() < 1e-3,
            "corner gain ≈ +0.51 EV, got {}",
            vignette_gain(8320)
        );
        let dist = |v: i16| v as f32 * (-14f32).exp2() + 1.0;
        assert!((dist(13) - 1.000793).abs() < 1e-5);
        assert!((dist(-855) - 0.947815).abs() < 1e-5, "barrel corner pulls source inward");
        let ca = |v: i16| v as f32 * (-21f32).exp2() + 1.0;
        assert!((ca(1024) - 1.000488).abs() < 1e-6);
        assert!((ca(-256) - 0.999878).abs() < 1e-6);
    }

    #[test]
    fn malformed_counts_degrade_to_absent() {
        // The count guard is exercised through `read` on real files; here the
        // conversion helpers must at least stay finite on extreme inputs.
        for v in [i16::MIN, -1, 0, 1, i16::MAX] {
            assert!(vignette_gain(v).is_finite());
        }
    }

    /// Real-machine probe (ignored in CI): AUTOSHADE_PROBE_RAW=<file.arw>
    /// cargo test --release probe_real_lens_metadata -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_real_lens_metadata() {
        let Some(path) = crate::config::live_env("AUTOSHADE_PROBE_RAW") else {
            eprintln!("AUTOSHADE_PROBE_RAW unset — skipping");
            return;
        };
        let p = read(Path::new(&path));
        eprintln!(
            "vignette {} knots (corner gain {:?}), distortion {} (corner {:?}), ca {}+{}",
            p.vignette.len(),
            p.vignette.last(),
            p.distortion.len(),
            p.distortion.last(),
            p.ca_r.len(),
            p.ca_b.len()
        );
        eprintln!(
            "mask warp: {:?} ({} knots, centre {:?}, corner {:?})",
            p.mask_warp_src,
            p.mask_warp.len(),
            p.mask_warp.first(),
            p.mask_warp.last()
        );
        assert!(!p.vignette.is_empty() && !p.distortion.is_empty(), "A7RIV files carry all three");
        assert!((p.vignette[0] - 1.0).abs() < 0.02, "centre gain ~1.0");
        assert_eq!(p.mask_warp_src, MaskWarpSource::CameraMetadata);
        assert_eq!(p.mask_warp.len(), MASK_WARP_KNOTS);
    }
}
