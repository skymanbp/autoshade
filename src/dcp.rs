//! Adobe `.dcp` camera profiles → the engine's COLOUR rendering (v1.5.0, F7).
//!
//! # What this is for
//!
//! `crs:CameraProfile` names the camera profile Lightroom developed the
//! photograph through — "Adobe Standard" on 160 of the 175 sidecars in the
//! library this was measured against. Until v1.5.0 the engine carried that name
//! to the sidecar and rendered its colour from the RAW's own embedded matrix
//! instead, so the name was a label on a rendering that did not obey it. This
//! module reads Adobe's profile file and makes the name mean something.
//!
//! **Read from the user's install, never bundled.** Same rule as [`crate::lcp`]
//! and for the same reason: these are Adobe's files, shipped with Camera Raw.
//! The machine this was built on holds 4,352 of them — 1,464 under
//! `Adobe Standard/` and 2,888 under `Camera/<model>/` — and shipping a copy of
//! any of them would be redistributing someone else's work.
//!
//! # The format, and how its layout was settled
//!
//! A `.dcp` is a plain TIFF container: an `II`/`MM` header and ONE IFD whose
//! tags are the DNG specification's profile tags. The two interesting ones are
//! three-dimensional HSV correction tables — `ProfileHueSatMapData1`/`…2` and
//! `ProfileLookTableData` — each entry three floats:
//!
//! ```text
//! (hue shift in DEGREES, saturation SCALE, value SCALE)
//! ```
//!
//! Measured rather than recalled: across the installed pool the first component
//! spans [-180, +180] and is centred on 0, the other two are positive and
//! centred on 1, and `Sony ILCE-7RM5 Camera BW`'s mean saturation scale is
//! 0.0039 — a table that desaturates to grey, which is what a black-and-white
//! profile has to do.
//!
//! The STORAGE ORDER is
//!
//! ```text
//! index = (val * hueDivisions + hue) * satDivisions + sat
//! ```
//!
//! and that was the one thing here worth measuring twice. Two-dimensional
//! tables — `valDivisions == 1`, which is every `ProfileHueSatMap` Adobe ships
//! — cannot tell this reading apart from "hue slowest, value fastest": both
//! collapse to `hue * satDivisions + sat` there. On the 60 installed profiles
//! that DO carry a three-dimensional table the discriminator is the achromatic
//! plane — a colour with no saturation must not have its saturation changed —
//! and the saturation scale over the `sat == 0` plane is EXACTLY 1.0 on 60/60
//! profiles under the reading above and on 0/60 under either alternative.
//!
//! # What this module does NOT do
//!
//! The creative profiles — "Adobe Color", "Adobe Landscape", "Adobe
//! Monochrome", the ones `crs:Look` names — are not `.dcp` files at all. They
//! live under `CameraRaw/Settings` as XMP, and their colour table is an opaque
//! `crs:Table_<HASH>` payload: base85 over an 85-character alphabet (printable
//! ASCII 33..125 less `"&,;<>\_`) whose entropy is 6.4082 bits per character of
//! a possible 6.4094 — a compressed or encrypted payload, and it does not
//! decompress under zlib, raw deflate, gzip, bzip2, xz, lzma, zstd or lz4 at
//! any byte offset up to 256 under four base85 conventions. Those profiles'
//! BAKED half — their tone curve and sliders, which the sidecar carries in full
//! on 161 of 161 — IS rendered; the table is disclosed as unrendered.

use std::path::PathBuf;
use std::sync::OnceLock;

/// A three-dimensional HSV correction table, as the file stores it.
///
/// `data` is `hue * sat * val` entries of `(hue shift °, sat scale, val scale)`
/// in the order documented at the module head. Held flat because the lookup is
/// the hot path: a render asks this question once per pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Divisions of the 360° hue circle. The axis is CYCLIC: division
    /// `hue - 1` interpolates back into division 0.
    pub hue: u32,
    /// Divisions of saturation over [0, 1] inclusive. The axis CLAMPS.
    pub sat: u32,
    /// Divisions of value over [0, 1] inclusive, or 1 for a flat table.
    pub val: u32,
    /// `true` when the file's encoding tag says the VALUE coordinate is
    /// sRGB-encoded — the lookup then encodes before indexing and decodes after
    /// scaling, which is the whole purpose of that tag.
    pub srgb_value: bool,
    pub data: Vec<[f32; 3]>,
}

impl Table {
    fn at(&self, h: u32, s: u32, v: u32) -> [f32; 3] {
        self.data[(v as usize * self.hue as usize + h as usize) * self.sat as usize + s as usize]
    }

    /// The correction at `(hue in degrees, saturation, value)`, interpolated
    /// trilinearly — cyclically in hue, clamped in saturation and value.
    ///
    /// Returns the three factors rather than applying them, so the caller owns
    /// the order the profile's pieces compose in.
    pub fn lookup(&self, hue_deg: f32, sat: f32, val: f32) -> [f32; 3] {
        // Hue: `hue` divisions span the WHOLE circle, so the step is 360/hue
        // and the last division's partner is the first one. Saturation and
        // value divisions span [0, 1] INCLUSIVE, so their step is 1/(n-1) and
        // the last sample sits exactly at 1.
        let hs = hue_deg.rem_euclid(360.0) * self.hue as f32 / 360.0;
        let h0 = (hs.floor() as i64).clamp(0, self.hue as i64 - 1) as u32;
        let hf = (hs - h0 as f32).clamp(0.0, 1.0);
        let h1 = if h0 + 1 >= self.hue { 0 } else { h0 + 1 };
        let (s0, s1, sf) = axis(sat, self.sat);
        let (v0, v1, vf) = axis(if self.srgb_value { encode_srgb(val) } else { val }, self.val);

        let mut out = [0.0f32; 3];
        for (hi, hw) in [(h0, 1.0 - hf), (h1, hf)] {
            for (si, sw) in [(s0, 1.0 - sf), (s1, sf)] {
                for (vi, vw) in [(v0, 1.0 - vf), (v1, vf)] {
                    let w = hw * sw * vw;
                    if w == 0.0 {
                        continue;
                    }
                    let e = self.at(hi, si, vi);
                    out[0] += e[0] * w;
                    out[1] += e[1] * w;
                    out[2] += e[2] * w;
                }
            }
        }
        out
    }

    /// Apply this table to one HSV triple, in place.
    ///
    /// Value scaling happens in the table's OWN value encoding: when the file
    /// flags the axis as sRGB-encoded, the scale multiplies the ENCODED value
    /// and the result is decoded back. Scaling the linear value instead would
    /// make one stored table mean two different pictures.
    pub fn apply(&self, hsv: &mut [f32; 3]) {
        let d = self.lookup(hsv[0], hsv[1], hsv[2]);
        hsv[0] = (hsv[0] + d[0]).rem_euclid(360.0);
        hsv[1] = (hsv[1] * d[1]).clamp(0.0, 1.0);
        hsv[2] = if self.srgb_value {
            decode_srgb((encode_srgb(hsv[2]) * d[2]).clamp(0.0, 1.0))
        } else {
            (hsv[2] * d[2]).max(0.0)
        };
    }
}

/// One clamped axis's bracketing samples and the weight between them.
///
/// A single division is a CONSTANT axis and answers `(0, 0, 0)` — which is what
/// makes every `ProfileHueSatMap` Adobe ships (valDivisions = 1) a plain
/// two-dimensional lookup without a second code path.
fn axis(x: f32, n: u32) -> (u32, u32, f32) {
    if n < 2 {
        return (0, 0, 0.0);
    }
    let scaled = x.clamp(0.0, 1.0) * (n - 1) as f32;
    let i0 = (scaled.floor() as u32).min(n - 2);
    (i0, i0 + 1, (scaled - i0 as f32).clamp(0.0, 1.0))
}

/// The sRGB transfer, for the value axis of a table that asks for it.
///
/// Spelled out here rather than borrowed from the render's own transfer because
/// this one belongs to the FILE FORMAT: it is what Adobe's encoding flag means,
/// and it must not follow the working space's transfer if that ever changes.
fn encode_srgb(x: f32) -> f32 {
    if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.max(0.0).powf(1.0 / 2.4) - 0.055 }
}

fn decode_srgb(x: f32) -> f32 {
    if x <= 0.040_45 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

/// A parsed `.dcp`.
///
/// Every field is what the file said; nothing is defaulted into existence. A
/// profile with no tables and no tone curve is a colour-matrix-only profile and
/// renders as one.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// `ProfileName` — what `crs:CameraProfile` has to match.
    pub name: String,
    /// `UniqueCameraModel` — the body this profile was calibrated for.
    pub unique_model: String,
    /// `CalibrationIlluminant1`/`2` as EXIF light-source codes.
    pub illuminant: [Option<u16>; 2],
    /// `ColorMatrix1`/`2`: XYZ → camera.
    pub color_matrix: [Option<[[f32; 3]; 3]>; 2],
    /// `ForwardMatrix1`/`2`: white-balanced camera → XYZ D50.
    pub forward_matrix: [Option<[[f32; 3]; 3]>; 2],
    /// `ProfileHueSatMapData1`/`2`, one per calibration illuminant.
    pub hue_sat_map: [Option<Table>; 2],
    /// `ProfileLookTableData` — one table, illuminant-independent.
    pub look_table: Option<Table>,
    /// `ProfileToneCurve` as (x, y) knots in [0, 1].
    pub tone_curve: Vec<[f32; 2]>,
    /// `BaselineExposureOffset` in stops (0 when absent).
    pub baseline_exposure_offset: f32,
    /// `ProfileHueSatMapData3` is the tri-illuminant extension. Recorded so the
    /// render can DISCLOSE that the profile is being read with two of its three
    /// calibrations rather than silently dropping the third.
    pub has_third_illuminant: bool,
}

/// Why a `.dcp` could not be turned into a [`Profile`].
///
/// Named cases, because "profile not applied" with no reason is exactly the
/// silent degradation this project refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No Camera Raw profile directory exists on this machine.
    NoRoots,
    /// No file under the roots matches this body and profile name.
    NotFound,
    /// The bytes are not a TIFF container, or the IFD runs off the end.
    NotAProfile(String),
    /// A table's entry count disagrees with its own declared dimensions.
    TableMismatch(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRoots => write!(f, "no Adobe Camera Raw profile directory on this machine"),
            Self::NotFound => write!(f, "no installed profile matches this camera and name"),
            Self::NotAProfile(why) => write!(f, "not a readable .dcp: {why}"),
            Self::TableMismatch(why) => write!(f, "profile table is inconsistent: {why}"),
        }
    }
}

// --- the container ---------------------------------------------------------

/// The DNG tags this module reads. Numbers from the DNG specification, and
/// cross-checked against `rawler-0.7.2/src/tags.rs`'s `DngTag`.
mod tag {
    pub const UNIQUE_CAMERA_MODEL: u16 = 50708;
    pub const COLOR_MATRIX_1: u16 = 50721;
    pub const COLOR_MATRIX_2: u16 = 50722;
    pub const CALIBRATION_ILLUMINANT_1: u16 = 50778;
    pub const CALIBRATION_ILLUMINANT_2: u16 = 50779;
    pub const PROFILE_NAME: u16 = 50936;
    pub const HUE_SAT_MAP_DIMS: u16 = 50937;
    pub const HUE_SAT_MAP_DATA_1: u16 = 50938;
    pub const HUE_SAT_MAP_DATA_2: u16 = 50939;
    pub const PROFILE_TONE_CURVE: u16 = 50940;
    pub const FORWARD_MATRIX_1: u16 = 50964;
    pub const FORWARD_MATRIX_2: u16 = 50965;
    pub const LOOK_TABLE_DIMS: u16 = 50981;
    pub const LOOK_TABLE_DATA: u16 = 50982;
    pub const HUE_SAT_MAP_ENCODING: u16 = 51107;
    pub const LOOK_TABLE_ENCODING: u16 = 51108;
    pub const BASELINE_EXPOSURE_OFFSET: u16 = 51109;
    pub const HUE_SAT_MAP_DATA_3: u16 = 52537;
}

/// One IFD entry's payload, still in the file's byte order.
struct Field<'a> {
    kind: u16,
    count: u32,
    bytes: &'a [u8],
    big: bool,
}

impl Field<'_> {
    fn scalars<const N: usize, T>(&self, f: impl Fn([u8; N]) -> T) -> Vec<T> {
        (0..self.count as usize)
            .filter_map(|i| self.bytes.get(i * N..i * N + N))
            .filter_map(|b| <[u8; N]>::try_from(b).ok())
            .map(f)
            .collect()
    }

    fn u16s(&self) -> Vec<u16> {
        let big = self.big;
        self.scalars(move |a| if big { u16::from_be_bytes(a) } else { u16::from_le_bytes(a) })
    }

    fn u32s(&self) -> Vec<u32> {
        let big = self.big;
        self.scalars(move |a| if big { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) })
    }

    /// Every numeric shape this module meets, as `f32`: FLOAT straight through,
    /// RATIONAL and SRATIONAL divided out, SHORT and LONG widened.
    ///
    /// A zero denominator answers 0 rather than an infinity: a matrix with an
    /// infinite entry would reach the render's inverse and publish a black
    /// frame, and the finiteness checks downstream should be judging the file's
    /// numbers, not this function's arithmetic.
    fn floats(&self) -> Vec<f32> {
        let big = self.big;
        match self.kind {
            11 => self.scalars(move |a| if big { f32::from_be_bytes(a) } else { f32::from_le_bytes(a) }),
            5 | 10 => self.scalars(move |a: [u8; 8]| {
                let rd = |s: [u8; 4]| if big { i32::from_be_bytes(s) } else { i32::from_le_bytes(s) };
                let n = rd([a[0], a[1], a[2], a[3]]);
                let d = rd([a[4], a[5], a[6], a[7]]);
                if d == 0 { 0.0 } else { n as f32 / d as f32 }
            }),
            3 => self.u16s().into_iter().map(f32::from).collect(),
            4 => self.u32s().into_iter().map(|v| v as f32).collect(),
            _ => Vec::new(),
        }
    }

    fn ascii(&self) -> String {
        let end = self.bytes.iter().position(|b| *b == 0).unwrap_or(self.bytes.len());
        String::from_utf8_lossy(&self.bytes[..end]).trim().to_string()
    }
}

fn type_size(kind: u16) -> usize {
    match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

/// The ONE IFD a `.dcp` carries, as tag → field.
///
/// Every offset is bounds-checked against the file. These are files the USER
/// installed, not files we wrote: a truncated or crafted one has to refuse,
/// not panic inside a render.
fn read_ifd(data: &[u8]) -> Result<Vec<(u16, Field<'_>)>, Refusal> {
    let big = match data.get(..2) {
        Some(b"II") => false,
        Some(b"MM") => true,
        _ => return Err(Refusal::NotAProfile("no II/MM byte-order mark".into())),
    };
    let rd16 = |at: usize| -> Option<u16> {
        let b: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
        Some(if big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
    };
    let rd32 = |at: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    };
    let off =
        rd32(4).ok_or_else(|| Refusal::NotAProfile("header shorter than 8 bytes".into()))? as usize;
    let n = rd16(off).ok_or_else(|| Refusal::NotAProfile("IFD offset past the end".into()))?;
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        let base = off + 2 + i * 12;
        let (Some(tag), Some(kind), Some(count)) = (rd16(base), rd16(base + 2), rd32(base + 4))
        else {
            return Err(Refusal::NotAProfile(format!("entry {i} runs past the end")));
        };
        let size = type_size(kind).saturating_mul(count as usize);
        if size == 0 {
            continue;
        }
        let bytes = if size <= 4 {
            data.get(base + 8..base + 8 + size)
        } else {
            let at = rd32(base + 8).unwrap_or(0) as usize;
            data.get(at..at.saturating_add(size))
        };
        let Some(bytes) = bytes else {
            return Err(Refusal::NotAProfile(format!("tag {tag} points outside the file")));
        };
        out.push((tag, Field { kind, count, bytes, big }));
    }
    Ok(out)
}

fn field<'a, 'b>(fields: &'b [(u16, Field<'a>)], tag: u16) -> Option<&'b Field<'a>> {
    fields.iter().find(|(t, _)| *t == tag).map(|(_, f)| f)
}

/// A calibration matrix, or `None` when it is short or holds a non-finite
/// entry. A profile with a broken matrix is not applied at all — half a
/// calibration is worse than the RAW's own.
fn matrix(fields: &[(u16, Field<'_>)], tag: u16) -> Option<[[f32; 3]; 3]> {
    let m = crate::render::mat3_from_slice(&field(fields, tag)?.floats())?;
    m.iter().flatten().all(|x| x.is_finite()).then_some(m)
}

fn table(
    fields: &[(u16, Field<'_>)],
    dims_tag: u16,
    data_tag: u16,
    encoding_tag: u16,
) -> Result<Option<Table>, Refusal> {
    let (Some(dims), Some(data)) = (field(fields, dims_tag), field(fields, data_tag)) else {
        return Ok(None);
    };
    let d = dims.u32s();
    if d.len() < 3 || d[0] == 0 || d[1] == 0 || d[2] == 0 {
        return Err(Refusal::TableMismatch(format!("tag {dims_tag} dims {d:?}")));
    }
    let want = (d[0] as usize)
        .checked_mul(d[1] as usize)
        .and_then(|n| n.checked_mul(d[2] as usize))
        .and_then(|n| n.checked_mul(3));
    let Some(want) = want else {
        return Err(Refusal::TableMismatch(format!("tag {dims_tag} dims {d:?} overflow")));
    };
    let v = data.floats();
    if v.len() != want {
        return Err(Refusal::TableMismatch(format!(
            "tag {data_tag}: {} floats for dims {:?} (want {want})",
            v.len(),
            &d[..3]
        )));
    }
    if !v.iter().all(|x| x.is_finite()) {
        return Err(Refusal::TableMismatch(format!("tag {data_tag} holds a non-finite entry")));
    }
    Ok(Some(Table {
        hue: d[0],
        sat: d[1],
        val: d[2],
        srgb_value: field(fields, encoding_tag).and_then(|f| f.u32s().first().copied()) == Some(1),
        data: v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
    }))
}

/// Parse the bytes of a `.dcp`.
pub fn parse(data: &[u8]) -> Result<Profile, Refusal> {
    let fields = read_ifd(data)?;
    let curve: Vec<[f32; 2]> = field(&fields, tag::PROFILE_TONE_CURVE)
        .map(|f| f.floats())
        .map(|v| v.chunks_exact(2).map(|c| [c[0], c[1]]).collect())
        .unwrap_or_default();
    Ok(Profile {
        name: field(&fields, tag::PROFILE_NAME).map(Field::ascii).unwrap_or_default(),
        unique_model: field(&fields, tag::UNIQUE_CAMERA_MODEL)
            .map(Field::ascii)
            .unwrap_or_default(),
        illuminant: [tag::CALIBRATION_ILLUMINANT_1, tag::CALIBRATION_ILLUMINANT_2]
            .map(|t| field(&fields, t).and_then(|f| f.u16s().first().copied())),
        color_matrix: [
            matrix(&fields, tag::COLOR_MATRIX_1),
            matrix(&fields, tag::COLOR_MATRIX_2),
        ],
        forward_matrix: [
            matrix(&fields, tag::FORWARD_MATRIX_1),
            matrix(&fields, tag::FORWARD_MATRIX_2),
        ],
        hue_sat_map: [
            table(
                &fields,
                tag::HUE_SAT_MAP_DIMS,
                tag::HUE_SAT_MAP_DATA_1,
                tag::HUE_SAT_MAP_ENCODING,
            )?,
            table(
                &fields,
                tag::HUE_SAT_MAP_DIMS,
                tag::HUE_SAT_MAP_DATA_2,
                tag::HUE_SAT_MAP_ENCODING,
            )?,
        ],
        look_table: table(
            &fields,
            tag::LOOK_TABLE_DIMS,
            tag::LOOK_TABLE_DATA,
            tag::LOOK_TABLE_ENCODING,
        )?,
        tone_curve: curve.into_iter().filter(|k| k[0].is_finite() && k[1].is_finite()).collect(),
        baseline_exposure_offset: field(&fields, tag::BASELINE_EXPOSURE_OFFSET)
            .and_then(|f| f.floats().first().copied())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0),
        has_third_illuminant: field(&fields, tag::HUE_SAT_MAP_DATA_3).is_some(),
    })
}

// --- discovery -------------------------------------------------------------

/// Where Adobe keeps camera profiles. `%AUTOSHADE_DCP_DIR%` leads when set —
/// see [`crate::adobe::camera_raw_roots`] for why none of this is a literal.
pub fn roots() -> Vec<PathBuf> {
    crate::adobe::camera_raw_roots("AUTOSHADE_DCP_DIR", "CameraProfiles")
}

/// Every `.dcp` under [`roots`], walked ONCE per process — the same contract,
/// and the same deliberate non-invalidation, as [`crate::lcp`]'s index: a
/// profile installed mid-run and expected to be noticed is not a workflow,
/// restarting is.
fn index() -> &'static [PathBuf] {
    static INDEX: OnceLock<Vec<PathBuf>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut out = crate::adobe::walk_extension(roots(), "dcp", 20_000);
        out.sort();
        out
    })
}

/// Fold a name to the form the file-name filter compares on: lower case, with
/// `_` read as a space.
///
/// Adobe's own naming is not quite consistent — the installed pool holds both
/// `Sony ILCE-7RM4A Adobe Standard.dcp` and `Leica D-Lux 7 Adobe_Standard.dcp`
/// — so a filter comparing bytes would miss the underscore spelling and report
/// "no profile installed" about a profile sitting in the directory.
fn fold(s: &str) -> String {
    s.to_ascii_lowercase().replace('_', " ")
}

/// The installed profile for this body and name, or why not.
///
/// The file NAME is the FILTER and the file's own tags are the VERDICT. The
/// pool is ~4,350 files of ~120 KB; parsing all of them per photograph would be
/// half a gigabyte of reads to answer a question the names already narrow to a
/// handful. A file whose name passes but whose `ProfileName` disagrees is
/// rejected on its tags, so the heuristic can cost a few needless reads — it
/// cannot produce the wrong profile.
pub fn find(make: &str, model: &str, profile_name: &str) -> Result<(PathBuf, Profile), Refusal> {
    let files = index();
    if files.is_empty() {
        return Err(Refusal::NoRoots);
    }
    let (want, model_f, make_f) = (fold(profile_name), fold(model), fold(make));
    if want.is_empty() || model_f.is_empty() {
        return Err(Refusal::NotFound);
    }
    let mut last = Refusal::NotFound;
    for p in files {
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(fold) else { continue };
        // The convention is "<Make> <Model> <ProfileName>", and the make is
        // already a prefix of the model on the bodies that repeat it.
        if !stem.contains(&model_f) || !stem.ends_with(&want) {
            continue;
        }
        if !make_f.is_empty() && !stem.contains(&make_f) && !model_f.contains(&make_f) {
            continue;
        }
        let Ok(bytes) = std::fs::read(p) else { continue };
        match parse(&bytes) {
            Ok(prof) if fold(&prof.name) == want => return Ok((p.clone(), prof)),
            Ok(_) => continue,
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests;
