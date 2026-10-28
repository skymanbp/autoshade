//! The camera's as-shot white balance, read the way Camera Raw names it: a Temperature and a Tint on the DNG SDK's scale.

use super::*;

/// The camera's AS-SHOT white balance as Camera Raw reads it: (Temperature K,
/// Tint). Metadata-only rawler decode (`dummy` — no pixel data, no demosaic;
/// wb_coeffs and the colour matrices come from the file/camera definition
/// either way, verified in rawler 0.7.2 arw.rs:184 + rawimage.rs:390), then
/// [`camera_raw_as_shot`]. `None` when the file is not a RAW, has no colour
/// matrix, or carries damaged coefficients — callers keep the engine's
/// historical 5500 K anchor.
pub fn as_shot_wb(raw_path: &Path) -> Option<(f32, f32)> {
    if !crate::decode::is_raw(raw_path) {
        return None;
    }
    // A cyclic-IFD file degrades to None here — the documented
    // no-metadata path (callers keep the historical 5500 K anchor).
    crate::decode::guard_tiff_chain(raw_path).ok()?;
    // Same degradation as the line above, for the same documented reason:
    // an over-ceiling frame has no WB to offer and the caller keeps the
    // historical 5500 K anchor. The refusal a person reads comes from the
    // develop funnel, which cannot degrade.
    crate::decode::guard_raw_plane_extent(raw_path).ok()?;
    let src = RawSource::new(raw_path).ok()?;
    let decoder = get_decoder(&src).ok()?;
    // A6: the last `get_decoder` caller in the crate, and the one that can
    // least afford to abort — it is consulted on OPEN, for the WB anchor, and
    // its whole contract is already "None on any trouble".
    let rawimage = crate::decode::guard_parser_panic(raw_path, "as-shot WB", || {
        decoder
            .raw_image(&src, &RawDecodeParams { image_index: 0 }, true)
            .map_err(|e| anyhow!("raw_image(dummy): {e}"))
    })
    .ok()?;
    let matrices = calibration_matrices(&rawimage)?;
    let wb = rawimage.wb_coeffs;
    let daylight = if rawimage.clean_make.eq_ignore_ascii_case("sony") {
        sony_daylight_preset(src.buf())
    } else {
        None
    };
    camera_raw_as_shot([wb[0], wb[1], wb[2]], daylight, &rawimage.clean_model, &matrices)
}

/// The camera's XYZ → camera matrices with the colour temperature each is
/// stated at, from rawler's camera definition (Standard A and D65 for the
/// bodies it ships both for), coolest first. A matrix at an illuminant with
/// no stated temperature is left out; `None` when none is left.
fn calibration_matrices(rawimage: &rawler::RawImage) -> Option<Vec<(f64, [[f64; 3]; 3])>> {
    let mut out: Vec<(f64, [[f64; 3]; 3])> = rawimage
        .color_matrix
        .iter()
        .filter_map(|(i, m)| {
            let k = profile::illuminant_kelvin(*i as u16)?;
            let m = mat3_from_slice(m)?;
            Some((f64::from(k), m.map(|row| row.map(f64::from))))
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    (!out.is_empty()).then_some(out)
}

/// Camera Raw's own white-balance multipliers (R/G, B/G) for a Sony body's
/// Daylight preset, by model.
///
/// Sony folds a colour compensation for the mounted lens into every white
/// balance a body records: the As Shot levels and the fourteen preset levels
/// of the same file scale together by lens (ILCE-7RM4A, Daylight R and B
/// ×1024 at G 1024: 2564/1728 on the FE 24–105 F4, 2596/1688 on the FE 85
/// F1.8, 2668/1664 on the Sigma 14–24 DG DN). Camera Raw takes that
/// compensation back out: it reads the As Shot levels RELATIVE to the file's
/// Daylight preset and puts that preset at the body's one white, these
/// multipliers. Measured with Camera Raw itself (v1.17.0): on seven frames
/// over the three lenses its As Shot render is pixel-identical to a Custom
/// render at one grid point each, and these multipliers reproduce all seven
/// (each within 1 mired and 1 tint unit when left out of the fit); a copy of
/// one frame with ONLY its Daylight preset rewritten to the Sigma's levels
/// moved Camera Raw's As Shot from 5400 K / +14 to the 5100 K / +12 predicted
/// here. The values are the centre of the region that reproduces all seven
/// (R/G 2.5098–2.5132, B/G 1.6274–1.6301).
///
/// A body with no entry reads its levels as they are: its reference is not
/// in its files, and a guessed one would be a cast of its own.
const SONY_DAYLIGHT_REFERENCE: [(&str, [f32; 2]); 1] = [("ILCE-7RM4A", [2.5115, 1.6288])];

/// The camera's As Shot white as Camera Raw reads it, from its white-balance
/// multipliers `wb` (rawler's convention: the reciprocal of the camera
/// neutral, green 1), the file's Daylight preset multipliers when the body
/// records them, its model, and its calibration `matrices`.
///
/// The neutral goes to a chromaticity by the DNG SDK's `NeutralToXY` (the
/// matrices interpolated in reciprocal temperature at the white's own
/// temperature, iterated from D50, [`profile::neutral_to_xy`]) and to a
/// Temperature and Tint by `dng_temperature::Set_xy_coord`
/// ([`profile::xy_to_temp_tint`], the inverse of the
/// [`profile::temp_tint_to_xy`] the white-balance law moves from). On a Sony
/// body with a [`SONY_DAYLIGHT_REFERENCE`] the levels are read relative to
/// the Daylight preset first. The answer is quantised to Camera Raw's own
/// grid, 50 K and whole tint units: its As Shot render is pixel-identical to
/// a Custom render at the rounded reading and differs from one 25 K or one
/// tint unit away (measured on seven frames, 3800–7650 K). `None` for damaged
/// multipliers or a white outside 1667–15000 K, where the metadata is junk
/// rather than a light.
pub(super) fn camera_raw_as_shot(
    wb: [f32; 3],
    daylight: Option<[f32; 3]>,
    model: &str,
    matrices: &[(f64, [[f64; 3]; 3])],
) -> Option<(f32, f32)> {
    if wb.iter().any(|c| !c.is_finite() || *c <= 0.0) {
        return None;
    }
    let mut mult = wb.map(f64::from);
    let reference = SONY_DAYLIGHT_REFERENCE.iter().find(|(m, _)| m.eq_ignore_ascii_case(model));
    let daylight = daylight.filter(|d| d.iter().all(|c| c.is_finite() && *c > 0.0));
    if let (Some((_, r)), Some(d)) = (reference, daylight) {
        mult[0] *= f64::from(r[0]) / f64::from(d[0] / d[1]);
        mult[2] *= f64::from(r[1]) / f64::from(d[2] / d[1]);
    }
    let xy = profile::neutral_to_xy(mult.map(|m| 1.0 / m), |k| matrix_at(matrices, k))?;
    let (kelvin, tint) = profile::xy_to_temp_tint(xy);
    if !kelvin.is_finite() || !tint.is_finite() || !(1667.0..=15_000.0).contains(&kelvin) {
        return None;
    }
    let kelvin = (kelvin / 50.0).round() * 50.0;
    Some((kelvin as f32, tint.round().clamp(-100.0, 100.0) as f32))
}

/// The XYZ → camera matrix at `kelvin`: the two stated either side weighted
/// in reciprocal temperature (the SDK's `FindXYZtoCamera`), the nearest end
/// beyond them.
fn matrix_at(matrices: &[(f64, [[f64; 3]; 3])], kelvin: f64) -> Option<[[f64; 3]; 3]> {
    let (first, last) = (matrices.first()?, matrices.last()?);
    if !kelvin.is_finite() || kelvin <= first.0 {
        return Some(first.1);
    }
    if kelvin >= last.0 {
        return Some(last.1);
    }
    let j = matrices.iter().position(|(k, _)| *k >= kelvin)?;
    let ((k1, a), (k2, b)) = (matrices[j - 1], matrices[j]);
    let w = (1.0 / kelvin - 1.0 / k1) / (1.0 / k2 - 1.0 / k1);
    Some([0, 1, 2].map(|i| [0, 1, 2].map(|c| a[i][c] + (b[i][c] - a[i][c]) * w)))
}

/// The Daylight preset multipliers (R, G, B; green 1) a Sony ARW records in
/// its encrypted SR2 block (tag 0x7820, `WB_RGBLevelsDaylight`), or `None`
/// when the file has no such block or it does not parse. The block is found
/// as rawler finds it for the As Shot levels (arw.rs `get_params`):
/// DNGPrivateData in IFD0 → the SR2 private IFD → its offset (0x7200),
/// length (0x7201) and key (0x7221) → Sony's keystream cipher. Every read is
/// bounds-checked: this is a file the user brought, and a bad one answers
/// `None`.
pub(super) fn sony_daylight_preset(file: &[u8]) -> Option<[f32; 3]> {
    if file.get(..4)? != b"II*\0" {
        return None;
    }
    let ifd0 = le32(file, 4)? as usize;
    let private = le32(file, entry(file, ifd0, 0xC634, 0)?.value_at)? as usize;
    let offset = le32(file, entry(file, private, 0x7200, 0)?.value_at)? as usize;
    let length = le32(file, entry(file, private, 0x7201, 0)?.value_at)? as usize;
    let key = le32(file, entry(file, private, 0x7221, 0)?.value_at)?;
    let block = sony_decrypt(file.get(offset..offset.checked_add(length / 4 * 4)?)?, key);
    let e = entry(&block, 0, 0x7820, offset)?;
    if e.kind != 8 || e.count != 3 {
        return None;
    }
    let level = |i: usize| -> Option<f32> {
        let at = e.value_at + 2 * i;
        Some(f32::from(i16::from_le_bytes(block.get(at..at + 2)?.try_into().ok()?)))
    };
    let (r, g, b) = (level(0)?, level(1)?, level(2)?);
    (g > 0.0).then(|| [r / g, 1.0, b / g])
}

/// One IFD entry: its type, its count, and where its value sits.
struct Entry {
    kind: u16,
    count: u32,
    value_at: usize,
}

/// The entry for `tag` in the little-endian IFD at `at` in `data`. A value
/// longer than four bytes lives at its stated offset less `base`, the file
/// offset `data` was cut from (0 for the file itself).
fn entry(data: &[u8], at: usize, tag: u16, base: usize) -> Option<Entry> {
    let n = usize::from(le16(data, at)?);
    (0..n).find_map(|i| {
        let e = at.checked_add(2 + 12 * i)?;
        if le16(data, e)? != tag {
            return None;
        }
        let (kind, count) = (le16(data, e + 2)?, le32(data, e + 4)?);
        let unit: usize = match kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        };
        let size = unit.checked_mul(count as usize)?;
        let value_at = if size <= 4 { e + 8 } else { (le32(data, e + 8)? as usize).checked_sub(base)? };
        data.get(value_at..value_at.checked_add(size)?)?;
        Some(Entry { kind, count, value_at })
    })
}

fn le16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn le32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// Sony's SR2 cipher (rawler arw.rs `sony_decrypt`, after dcraw's): a
/// 127-word keystream from the key, XORed onto the block word by word. The
/// pad's byte swap is rawler's `u32::from_be` on the little-endian hosts it
/// runs on, written out so every host reads the same bytes.
pub(super) fn sony_decrypt(data: &[u8], key: u32) -> Vec<u8> {
    let mut pad = [0u32; 128];
    let mut k = key;
    for p in pad.iter_mut().take(4) {
        k = k.wrapping_mul(48_828_125).wrapping_add(1);
        *p = k;
    }
    pad[3] = (pad[3] << 1) | ((pad[0] ^ pad[2]) >> 31);
    for p in 4..127 {
        pad[p] = ((pad[p - 4] ^ pad[p - 2]) << 1) | ((pad[p - 3] ^ pad[p - 1]) >> 31);
    }
    for p in pad.iter_mut().take(127) {
        *p = p.swap_bytes();
    }
    let mut out = Vec::with_capacity(data.len());
    for (i, word) in data.chunks_exact(4).enumerate() {
        let p = i + 127;
        pad[p & 127] = pad[(p + 1) & 127] ^ pad[(p + 65) & 127];
        let w = u32::from_le_bytes([word[0], word[1], word[2], word[3]]) ^ pad[p & 127];
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}
