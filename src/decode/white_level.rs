//! Camera Raw's raw white level (v1.17.0, measured).
//!
//! The raw is scaled to light as `(value − black) / (white − black)`, so the
//! white level is an exposure: a render that divides by a smaller white than
//! Camera Raw's is brighter than Camera Raw's by `log2` of the ratio, on every
//! pixel. rawler takes the level the file states; Adobe DNG Converter 18.7
//! writes its own into the DNG it makes, and the two differ on the bodies we
//! hold RAWs for:
//!
//! | body | rawler (file) | Camera Raw (DNG `WhiteLevel`) |
//! |---|---|---|
//! | ILCE-7RM4A / ILCE-7RM4, 14-bit | 15360 (SR2 tag 0x787F) | 16383; 15000 at ISO 160 |
//! | ILCE-7RM4A / ILCE-7RM4, 12-bit | 15360 | 16380 |
//! | ILCE-7M3 | 15360 | 16300 |
//! | Canon EOS R6 | 14008 | 16384 |
//! | Nikon D700 | 15892 | 15892 |
//!
//! The 7RM4 rows are a census of 50 files (42 ILCE-7RM4A, 8 ILCE-7RM4) plus three
//! corpus frames: ISO 50 to 25600, 1/4000 s to 25 s, both bit
//! depths. The rule fits every one exactly: 12-bit files 16380, 14-bit files
//! 16383, except ISO 160, where all 8 files (both bodies, 1/4000 s to 15 s)
//! say 15000. It is not the file's data maximum (an underexposed 14-bit frame
//! peaking at 3706 still gets 16383; ISO 160 frames reach 16383 and get
//! 15000), not the vendor tag (15360 on every file), and not a linearization
//! table (none is written). The 12-bit files seen are all long exposures; a
//! 12-bit ISO 160 file was not in the census, so that pair takes the 12-bit
//! row. The 7M3 and R6 rows are one file each.
//!
//! A body not in the table keeps the file's own level: there is no measured
//! Camera Raw number to use, and the render discloses that its brightness then
//! rests on the file's level (the D700 row is why that is often right).

/// Where the white level a render scales by came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WhiteLevelSource {
    /// Camera Raw's own level for this body (the table above).
    CameraRaw { level: u32, file: u32 },
    /// The file's level, kept: the body is not in the table.
    File { level: u32 },
}

/// Camera Raw's white level for a body, as DNG Converter 18.7 writes it; `iso`
/// and `bits` are the file's ISO and its raw IFD's `BitsPerSample`. `None`
/// for a body not measured.
pub(crate) fn camera_raw_white_level(make: &str, model: &str, iso: Option<u32>, bits: Option<u16>) -> Option<u32> {
    let make = make.to_ascii_lowercase();
    match (make.as_str(), model) {
        ("sony", "ILCE-7RM4A" | "ILCE-7RM4") => Some(match (bits, iso) {
            (Some(12), _) => 16380,
            (_, Some(160)) => 15000,
            _ => 16383,
        }),
        ("sony", "ILCE-7M3") => Some(16300),
        ("canon", "EOS R6") => Some(16384),
        _ => None,
    }
}

/// `BitsPerSample` of the full-resolution raw IFD of a little-endian TIFF-based
/// RAW (an ARW: IFD0's SubIFDs, `NewSubfileType` 0, a compression other than
/// the JPEG previews' 6/7). `None` when the file does not parse that way.
pub(crate) fn raw_bits_per_sample(file: &[u8]) -> Option<u16> {
    let le16 = |at: usize| file.get(at..at.checked_add(2)?).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let le32 = |at: usize| file.get(at..at.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if file.get(..4)? != b"II*\0" {
        return None;
    }
    // (tag → (type, count, value-or-offset field position)) of one IFD.
    let tags = |ifd: usize| -> Option<Vec<(u16, u16, u32, usize)>> {
        let n = usize::from(le16(ifd)?);
        (0..n.min(512)).map(|i| {
            let e = ifd.checked_add(2 + 12 * i)?;
            Some((le16(e)?, le16(e + 2)?, le32(e + 4)?, e + 8))
        }).collect()
    };
    let value = |kind: u16, at: usize| -> Option<u32> {
        match kind {
            3 => le16(at).map(u32::from),
            4 => le32(at),
            _ => None,
        }
    };
    let ifd0 = le32(4)? as usize;
    let subs = tags(ifd0)?.into_iter().find(|t| t.0 == 0x014A)?;
    let offsets: Vec<usize> = if subs.2 == 1 {
        vec![le32(subs.3)? as usize]
    } else {
        let at = le32(subs.3)? as usize;
        (0..subs.2.min(16) as usize).map(|i| le32(at + 4 * i).map(|v| v as usize)).collect::<Option<_>>()?
    };
    offsets.into_iter().find_map(|ifd| {
        let t = tags(ifd)?;
        let get = |tag: u16| t.iter().find(|e| e.0 == tag).and_then(|e| value(e.1, e.3));
        let full = get(0x00FE).unwrap_or(0) == 0;
        let compression = get(0x0103)?;
        (full && compression != 6 && compression != 7).then_some(())?;
        get(0x0102).and_then(|b| u16::try_from(b).ok())
    })
}

/// Put Camera Raw's white level on a decoded RAW when its body is measured;
/// otherwise leave the file's. Returns where the level now comes from.
pub(crate) fn use_camera_raw_white_level(
    raw: &mut rawler::RawImage,
    iso: Option<u32>,
    bits: Option<u16>,
) -> Option<WhiteLevelSource> {
    let file = *raw.whitelevel.0.first()?;
    Some(match camera_raw_white_level(&raw.clean_make, &raw.clean_model, iso, bits) {
        Some(level) => {
            raw.whitelevel = rawler::rawimage::WhiteLevel(vec![level; raw.whitelevel.0.len()]);
            WhiteLevelSource::CameraRaw { level, file }
        }
        None => WhiteLevelSource::File { level: file },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row pinned to the `WhiteLevel` DNG Converter 18.7 wrote for a
    /// file of that body (census in the module note).
    #[test]
    fn the_white_level_is_camera_raws_per_body() {
        for model in ["ILCE-7RM4A", "ILCE-7RM4"] {
            for iso in [50, 100, 125, 200, 250, 400, 800, 1600, 2500, 6400, 25600] {
                assert_eq!(camera_raw_white_level("Sony", model, Some(iso), Some(14)), Some(16383), "{model} ISO {iso}");
            }
            assert_eq!(camera_raw_white_level("Sony", model, Some(160), Some(14)), Some(15000), "{model} ISO 160");
        }
        for iso in [50, 100, 200, 800, 1600, 2000, 2500, 3200] {
            assert_eq!(camera_raw_white_level("SONY", "ILCE-7RM4A", Some(iso), Some(12)), Some(16380), "12-bit ISO {iso}");
        }
        assert_eq!(camera_raw_white_level("Sony", "ILCE-7M3", Some(100), Some(14)), Some(16300));
        assert_eq!(camera_raw_white_level("Canon", "EOS R6", Some(100), None), Some(16384));
        // Not measured: the file's level stays (D700: Camera Raw's equals it).
        assert_eq!(camera_raw_white_level("Nikon", "D700", Some(200), Some(14)), None);
    }

    /// A minimal little-endian TIFF: IFD0 with one SubIFD holding the raw
    /// (`NewSubfileType` 0, uncompressed, `bits` per sample) and a JPEG
    /// preview SubIFD ahead of it that must be skipped.
    fn tiff_with_raw_bits(bits: u16) -> Vec<u8> {
        let mut f = b"II*\0".to_vec();
        f.extend(8u32.to_le_bytes());
        let ifd = |entries: &[(u16, u16, u32, u32)]| {
            let mut b = (entries.len() as u16).to_le_bytes().to_vec();
            for (tag, kind, count, v) in entries {
                b.extend(tag.to_le_bytes());
                b.extend(kind.to_le_bytes());
                b.extend(count.to_le_bytes());
                b.extend(v.to_le_bytes());
            }
            b.extend(0u32.to_le_bytes());
            b
        };
        // IFD0 at 8: SubIFDs (2 offsets) stored at 8 + 18 = 26.
        let ifd0_len = 2 + 12 + 4;
        let list_at = 8 + ifd0_len as u32;
        let preview_at = list_at + 8;
        let preview = ifd(&[(0x00FE, 4, 1, 1), (0x0103, 3, 1, 6), (0x0102, 3, 1, 8)]);
        let raw_at = preview_at + preview.len() as u32;
        f.extend(ifd(&[(0x014A, 4, 2, list_at)]));
        f.extend(preview_at.to_le_bytes());
        f.extend(raw_at.to_le_bytes());
        f.extend(preview);
        f.extend(ifd(&[(0x00FE, 4, 1, 0), (0x0103, 3, 1, 1), (0x0102, 3, 1, u32::from(bits))]));
        f
    }

    #[test]
    fn the_raw_ifds_bit_depth_is_read_past_the_preview() {
        assert_eq!(raw_bits_per_sample(&tiff_with_raw_bits(12)), Some(12));
        assert_eq!(raw_bits_per_sample(&tiff_with_raw_bits(14)), Some(14));
        assert_eq!(raw_bits_per_sample(b"MM\0*\0\0\0\x08"), None);
        assert_eq!(raw_bits_per_sample(b"II*"), None);
    }
}
