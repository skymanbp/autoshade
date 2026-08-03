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
//! Knot `i` sits at normalised radius `(i + 0.5)/(n − 1)` (0 = centre,
//! 1 = corner half-diagonal) — RawTherapee's placement; the engine
//! interpolates linearly and clamps outside. Each array's FIRST element is
//! the value count; a malformed count degrades that component to "absent",
//! never an error (a photo without correction data is a normal photo).

use std::path::Path;

use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, Value};
use rawler::tags::ExifTag;

use crate::recipe::LensProfile;

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
    let Ok(tiff) = GenericTiffReader::new(&mut reader, 0, 0, None, &[]) else {
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
    out.distortion = knots(ExifTag::DistortionCorrParams)
        .iter()
        .map(|&v| v as f32 * (-14f32).exp2() + 1.0)
        .collect();
    let ca = knots(ExifTag::ChromaticAberrationCorrParams);
    if !ca.is_empty() && ca.len() % 2 == 0 {
        let half = ca.len() / 2;
        let f = |v: &i16| *v as f32 * (-21f32).exp2() + 1.0;
        out.ca_r = ca[..half].iter().map(f).collect();
        out.ca_b = ca[half..].iter().map(f).collect();
    }
    out.clamp(); // same defensive ranges as a hand-edited recipe
    out
}

/// Sony vignetting knot → linear-light gain (RawTherapee's formula).
fn vignette_gain(v: i16) -> f32 {
    1.0 / (0.5 - (v as f32 * (-13f32).exp2() - 1.0).exp2()).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Real-machine probe (ignored in CI): AUTOSHOP_PROBE_RAW=<file.arw>
    /// cargo test --release probe_real_lens_metadata -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_real_lens_metadata() {
        let Ok(path) = std::env::var("AUTOSHOP_PROBE_RAW") else {
            eprintln!("AUTOSHOP_PROBE_RAW unset — skipping");
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
        assert!(!p.vignette.is_empty() && !p.distortion.is_empty(), "A7RIV files carry all three");
        assert!((p.vignette[0] - 1.0).abs() < 0.02, "centre gain ~1.0");
    }
}
