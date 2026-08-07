//! RAW decode + feature extraction (Milestone M1, decode half).
//!
//! Backed by `rawler` 0.7.2 — chosen for Sony A7R IV/IVA coverage, embedded
//! preview extraction, and full EXIF (see `docs/M1_PLAN.md` §1 and §9; the
//! older pure-Rust `rawloader` froze its camera DB before these bodies). One
//! backend for now: a `Decoder` trait abstraction is deferred until a second
//! backend is actually needed (the user shoots a single camera family).
//!
//! All `rawler` calls here were written against the crate's real source
//! (`RawSource::new`, `get_decoder`, the `Decoder` trait, `RawMetadata.exif`),
//! not from memory.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, GenericImageView};
use rawler::decoders::RawDecodeParams;
use rawler::formats::tiff::{Rational, SRational};
use rawler::get_decoder;
use rawler::rawsource::RawSource;

/// Camera + capture metadata pulled from the RAW, for display and for feeding
/// the AI advisor later.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Meta {
    pub make: String,
    pub model: String,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    /// Human shutter string, e.g. "1/1250" or "2s".
    pub shutter: Option<String>,
    /// f-number, e.g. 4.0.
    pub aperture: Option<f32>,
    pub focal_length_mm: Option<f32>,
    pub exposure_bias_ev: Option<f32>,
    pub date_time: Option<String>,
    /// Full sensor dimensions (from the raw image, not the preview).
    pub width: usize,
    pub height: usize,
    /// As-shot white-balance multipliers [R, G1, B, G2].
    pub as_shot_wb_coeffs: [f32; 4],
}

/// 256-bin per-channel + luma histogram with clipping fractions.
///
/// Computed from the camera-processed embedded preview (tone-mapped), so it is
/// a *display-referred* histogram — good for framing/clipping hints, not a
/// linear raw histogram. A raw-linear version can replace this in a later
/// milestone if exposure decisions need it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Histogram {
    pub luma: Vec<u32>,
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
    /// % of pixels with luma in 0..=1 (crushed blacks).
    pub clip_black_pct: f32,
    /// % of pixels with luma in 254..=255 (blown highlights).
    pub clip_white_pct: f32,
    pub sample_pixels: u64,
}

/// Everything `decode_raw` produces for one RAW file.
pub struct Decoded {
    /// Full-resolution embedded preview (already white-balanced by the camera).
    pub preview: DynamicImage,
    pub meta: Meta,
    pub histogram: Histogram,
    /// Embedded XMP packet, if the RAW carries one.
    pub embedded_xmp: Option<String>,
}

fn ratio(r: &Rational) -> f32 {
    if r.d == 0 { 0.0 } else { r.n as f32 / r.d as f32 }
}
fn sratio(r: &SRational) -> f32 {
    if r.d == 0 { 0.0 } else { r.n as f32 / r.d as f32 }
}

/// Format a shutter-speed Rational as "1/x" for fast speeds, "Ns" otherwise.
fn fmt_shutter(r: &Rational) -> String {
    let v = ratio(r);
    if v > 0.0 && v < 1.0 {
        format!("1/{}", (1.0 / v).round() as i64)
    } else {
        format!("{v}s")
    }
}

/// Does this path look like a camera RAW (vs an already-baked raster like a
/// LR/PS-exported PNG/TIFF/JPEG)? Drives the raw-vs-baked dispatch.
pub fn is_raw(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "arw" | "dng" | "raw" | "raf" | "nef" | "cr2" | "cr3" | "orf" | "rw2"
        )
    })
}

/// Load a baked raster (PNG/TIFF/JPEG) with a RAISED (not lifted) decoder
/// memory limit — a 60 MP export trips the image crate's default cap, but
/// `no_limits()` let a corrupt header with absurd declared dimensions drive an
/// unbounded allocation straight into OOM. 61 MP 16-bit RGBA is ~0.5 GiB;
/// 4 GiB leaves headroom without trusting arbitrary headers.
/// Also applies the EXIF orientation: phone/Lightroom JPEGs store rotation as
/// metadata the decoder does NOT apply — imported photos rendered sideways
/// (the RAW path already orients via the sensor metadata).
pub fn load_image(path: &Path) -> Result<DynamicImage> {
    use image::ImageDecoder as _;
    let mut reader = image::ImageReader::open(path)
        .with_context(|| format!("open image {}", path.display()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(65_536);
    limits.max_image_height = Some(65_536);
    limits.max_alloc = Some(4 * 1024 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .with_guessed_format()
        .with_context(|| format!("probe image {}", path.display()))?
        .into_decoder()
        .with_context(|| format!("decode image {}", path.display()))?;
    // into_decoder enforces the DIMENSION limits but skips decode()'s
    // total_bytes reservation — without this check max_alloc never bounded
    // the OUTPUT buffer (a 65536² 16-bit RGBA header passes the dimension
    // gate yet decodes to ~32 GiB).
    let need = decoder.total_bytes();
    const MAX_ALLOC: u64 = 4 * 1024 * 1024 * 1024;
    if need > MAX_ALLOC {
        anyhow::bail!(
            "image {} decodes to {need} bytes — over the {MAX_ALLOC}-byte ceiling",
            path.display()
        );
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)
        .with_context(|| format!("decode image {}", path.display()))?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Decode any supported source — a camera RAW or an already-baked image. The
/// baked path (the "PNG source" mode: edit an LR/PS-denoised export) has no
/// sensor metadata, so [`Meta`] is filled with neutral defaults and the
/// histogram is computed from the pixels.
pub fn decode_any(path: &Path) -> Result<Decoded> {
    if is_raw(path) {
        decode_raw(path)
    } else {
        decode_baked(path)
    }
}

fn decode_baked(path: &Path) -> Result<Decoded> {
    let preview = load_image(path)?;
    let (w, h) = preview.dimensions();
    let meta = Meta {
        make: String::new(),
        model: "imported image".to_string(),
        lens: None,
        iso: None,
        shutter: None,
        aperture: None,
        focal_length_mm: None,
        exposure_bias_ev: None,
        date_time: None,
        width: w as usize,
        height: h as usize,
        as_shot_wb_coeffs: [1.0, 1.0, 1.0, 1.0],
    };
    let histogram = compute_histogram(&hist_copy(&preview));
    Ok(Decoded { preview, meta, histogram, embedded_xmp: None })
}

/// Does this EXIF orientation swap width and height in the display frame?
fn orientation_transposes(o: rawler::Orientation) -> bool {
    matches!(
        o,
        rawler::Orientation::Rotate90
            | rawler::Orientation::Rotate270
            | rawler::Orientation::Transpose
            | rawler::Orientation::Transverse
    )
}

/// Histogram working copy: DOWNSCALE-only to ≤1024 (`resize` also UPSCALES a
/// smaller input, and interpolating a small thumbnail up distorted clipping
/// percentages before histogramming).
fn hist_copy(img: &DynamicImage) -> DynamicImage {
    if img.width().max(img.height()) > 1024 {
        img.resize(1024, 1024, image::imageops::FilterType::Triangle)
    } else {
        img.clone()
    }
}

/// Decode a RAW file: embedded preview + metadata + histogram. Reads the file
/// only; never writes near the source.
pub fn decode_raw(path: &Path) -> Result<Decoded> {
    let src = RawSource::new(path).with_context(|| format!("open RAW {}", path.display()))?;
    let decoder = get_decoder(&src)
        .map_err(|e| anyhow!("no rawler decoder for {}: {e}", path.display()))?;
    let params = RawDecodeParams { image_index: 0 };

    // Prefer the mid-size embedded preview; fall back to the thumbnail, then
    // to the embedded FULL-SIZE camera JPEG (`full_image` extracts the
    // JPEGInterchangeFormat blob — it never develops the sensor; rawler's ARW
    // decoder in fact implements ONLY this level, so Sony files always
    // resolve here). Evaluated lazily to skip the larger JPEG decodes when a
    // cheaper level exists.
    let preview = match decoder
        .preview_image(&src, &params)
        .map_err(|e| anyhow!("preview_image: {e}"))?
    {
        Some(p) => p,
        None => match decoder
            .thumbnail_image(&src, &params)
            .map_err(|e| anyhow!("thumbnail_image: {e}"))?
        {
            Some(t) => t,
            None => decoder
                .full_image(&src, &params)
                .map_err(|e| anyhow!("full_image: {e}"))?
                .ok_or_else(|| anyhow!("no embedded preview/thumbnail/full image in {}", path.display()))?,
        },
    };

    let md = decoder
        .raw_metadata(&src, &params)
        .map_err(|e| anyhow!("raw_metadata: {e}"))?;

    // `dummy = true`: populate dimensions / WB / levels without decompressing
    // the full sensor data — we only need the structural metadata here.
    let raw = decoder
        .raw_image(&src, &params, true)
        .map_err(|e| anyhow!("raw_image(dummy): {e}"))?;

    let exif = &md.exif;
    let meta = Meta {
        make: md.make.trim().to_string(),
        model: md.model.trim().to_string(),
        lens: exif.lens_model.clone().or_else(|| exif.lens_make.clone()),
        iso: exif.iso_speed_ratings.map(|v| v as u32).or(exif.iso_speed),
        shutter: exif.exposure_time.as_ref().map(fmt_shutter),
        aperture: exif
            .fnumber
            .as_ref()
            .map(ratio)
            // An INVALID FNumber (0-denominator → 0.0, or non-finite) must
            // fall through to the Av fallback, not suppress it: filtering
            // only at the end let f/0 shadow a perfectly valid ApertureValue.
            .filter(|v| v.is_finite() && *v > 0.0)
            // ApertureValue is an APEX Av, not an f-number: N = 2^(Av/2)
            // (Av 4 ⇒ f/4, Av 5 ⇒ f/5.7). Feeding it raw overstated fast
            // lenses in metadata + the AI prompt whenever FNumber was absent.
            .or_else(|| exif.aperture_value.as_ref().map(|v| (ratio(v) / 2.0).exp2()))
            // Same physical-validity rule for the fallback (huge Av → ∞;
            // serde_json refuses non-finite floats — the WB-coeff rule).
            .filter(|v| v.is_finite() && *v > 0.0),
        focal_length_mm: exif
            .focal_length
            .as_ref()
            .map(ratio)
            .filter(|v| v.is_finite() && *v > 0.0),
        exposure_bias_ev: exif.exposure_bias.as_ref().map(sratio).filter(|v| v.is_finite()),
        date_time: exif.date_time_original.clone(),
        // DISPLAY-frame dims, agreeing with the oriented preview below:
        // sensor width/height made every rotated (portrait) shot's aspect —
        // and the style index's portrait feature — read as landscape.
        width: if orientation_transposes(raw.orientation) { raw.height } else { raw.width },
        height: if orientation_transposes(raw.orientation) { raw.width } else { raw.height },
        // Sony stores only 3 WB multipliers, so rawler leaves the 4th (second
        // green) as NaN. Replace any non-finite coeff with the neutral 1.0 —
        // otherwise serde_json refuses to serialise Meta when we hand it to the
        // advisor (JSON has no NaN).
        as_shot_wb_coeffs: {
            let mut wb = raw.wb_coeffs;
            for c in wb.iter_mut() {
                if !c.is_finite() {
                    *c = 1.0;
                }
            }
            wb
        },
    };

    // Orient the preview into the display frame (embedded previews are stored
    // in sensor orientation; see preview_only for the full rationale). Uses
    // the dummy raw's orientation, which is already decoded above for Meta.
    let preview = crate::render::oriented(preview, raw.orientation);

    // Histogram on a downscale-only copy of the preview — representative and
    // fast even for a 60 MP embedded JPEG (see hist_copy).
    let histogram = compute_histogram(&hist_copy(&preview));

    let embedded_xmp = decoder
        .xpacket(&src, &params)
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok());

    Ok(Decoded { preview, meta, histogram, embedded_xmp })
}

/// Just the embedded preview, skipping metadata/histogram — for the UI grid and
/// before/after, where only the image is needed.
pub fn preview_only(path: &Path) -> Result<DynamicImage> {
    if !is_raw(path) {
        return load_image(path);
    }
    camera_rendition(path)?
        .ok_or_else(|| anyhow!("no preview/thumbnail/full image in {}", path.display()))
}

/// The camera's own rendition of a RAW, oriented — `None` when it carries
/// none. The 3-level fallback (embedded preview → thumbnail → the embedded
/// full-size camera JPEG) shared by `preview_only` and `embedded_preview`,
/// which differ only in whether "none" is an error.
///
/// All three levels are camera-baked EXTRACTIONS, never a sensor develop
/// (see `decode_raw` for the rawler-source verification), so the base-look
/// estimator's camera-vs-neutral CDF comparison stays meaningful.
///
/// Embedded previews come back in SENSOR orientation (rawler's ARW path
/// decodes the JPEG bytes verbatim — verified in the crate source), so they
/// are oriented here with the render engine's own function: masks, crop and
/// straighten are all defined against the displayed preview and must mean the
/// same thing in the full-res render. The dummy `raw_image` that carries the
/// orientation decodes metadata only — no sensor decompression.
fn camera_rendition(path: &Path) -> Result<Option<DynamicImage>> {
    let src = RawSource::new(path).with_context(|| format!("open RAW {}", path.display()))?;
    let decoder =
        get_decoder(&src).map_err(|e| anyhow!("no decoder for {}: {e}", path.display()))?;
    let params = RawDecodeParams { image_index: 0 };
    let img = if let Some(p) = decoder
        .preview_image(&src, &params)
        .map_err(|e| anyhow!("preview_image: {e}"))?
    {
        p
    } else if let Some(t) = decoder
        .thumbnail_image(&src, &params)
        .map_err(|e| anyhow!("thumbnail_image: {e}"))?
    {
        t
    } else if let Some(f) = decoder
        .full_image(&src, &params)
        .map_err(|e| anyhow!("full_image: {e}"))?
    {
        f
    } else {
        return Ok(None);
    };
    let orientation = decoder
        .raw_image(&src, &params, true)
        .map_err(|e| anyhow!("raw_image(dummy): {e}"))?
        .orientation;
    Ok(Some(crate::render::oriented(img, orientation)))
}

/// The camera's OWN rendition only — `None` for a non-RAW or a RAW that
/// carries none.
///
/// `full_image` reads the JPEGInterchangeFormat blob for ARW (and the
/// analogous embedded image for CR2/CR3/DNG), and its DEFAULT impl returns
/// `Ok(None)` — verified in the rawler 0.7.2 source. That level-3 call is
/// essential in practice: rawler's ARW decoder implements ONLY `full_image`
/// (no preview/thumbnail overrides), so every A7RIV file answers there alone.
/// Callers (the base-look estimator) rely on this "camera pixels or nothing"
/// contract — a neutral develop standing in would make the camera-vs-neutral
/// CDF comparison meaningless.
pub fn embedded_preview(path: &Path) -> Result<Option<DynamicImage>> {
    if !is_raw(path) {
        return Ok(None);
    }
    camera_rendition(path)
}

fn compute_histogram(img: &DynamicImage) -> Histogram {
    let rgb = img.to_rgb8();
    let (mut r, mut g, mut b, mut luma) = (vec![0u32; 256], vec![0u32; 256], vec![0u32; 256], vec![0u32; 256]);
    let (mut clip_black, mut clip_white, mut n) = (0u64, 0u64, 0u64);
    for px in rgb.pixels() {
        let (rr, gg, bb) = (px[0], px[1], px[2]);
        r[rr as usize] += 1;
        g[gg as usize] += 1;
        b[bb as usize] += 1;
        let y = (0.299 * rr as f32 + 0.587 * gg as f32 + 0.114 * bb as f32)
            .round()
            .clamp(0.0, 255.0) as usize;
        luma[y] += 1;
        if y <= 1 {
            clip_black += 1;
        }
        if y >= 254 {
            clip_white += 1;
        }
        n += 1;
    }
    let pct = |c: u64| if n > 0 { 100.0 * c as f32 / n as f32 } else { 0.0 };
    Histogram {
        luma,
        r,
        g,
        b,
        clip_black_pct: pct(clip_black),
        clip_white_pct: pct(clip_white),
        sample_pixels: n,
    }
}

impl Decoded {
    /// The preview downscaled so its long edge is at most `max_edge` px, for
    /// saving / sending to the advisor. Returns a borrow-free owned image.
    pub fn preview_resized(&self, max_edge: u32) -> DynamicImage {
        let (w, h) = self.preview.dimensions();
        if w.max(h) <= max_edge {
            self.preview.clone()
        } else {
            self.preview
                .resize(max_edge, max_edge, image::imageops::FilterType::Lanczos3)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `preview_only` and `embedded_preview` share `camera_rendition` and
    /// differ in EXACTLY two ways. Both differences are load-bearing and
    /// neither had a test — this module is the first in `decode.rs`.
    #[test]
    fn the_two_camera_rendition_callers_differ_only_where_they_must() {
        let dir = std::env::temp_dir().join(format!("autoshop-decode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // (1) A NON-RAW: `preview_only` loads the raster; `embedded_preview`
        // answers "no camera rendition" WITHOUT touching it — a baked image
        // has no camera JPEG to compare a neutral develop against, and
        // standing in with the file's own pixels would make the base-look CDF
        // comparison meaningless. Every production caller happens to gate on
        // `is_raw` itself (`pipeline::photo_base_knots_checked` and
        // `serve::fresh_base_knots` at their tops; the GUI open worker's
        // non-RAW arm never reaches its `embedded_preview` call), so this
        // branch is reached only from here: pinning it is what keeps the
        // guarantee a CONTRACT rather than an accident the next caller has no
        // right to rely on. (Named, not line-numbered: the old line citations
        // had already drifted onto unrelated functions.)
        let png = dir.join("baked.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::new(7, 5)).save(&png).unwrap();
        let p = preview_only(&png).expect("a baked raster loads");
        assert_eq!((p.width(), p.height()), (7, 5));
        assert!(
            embedded_preview(&png).expect("not an error").is_none(),
            "a non-RAW carries no CAMERA rendition"
        );

        // (2) A RAW that cannot be decoded is an INABILITY, not "carries
        // none": both must Err. TWO of the three callers act on that
        // difference — `pipeline::photo_base_knots_checked` and
        // `serve::fresh_base_knots` print a diagnostic on Err and stay silent
        // on Ok(None) — so collapsing them would hide a broken file behind a
        // silent skip. The third, the GUI open worker's knots match,
        // deliberately reads both as "no base look" (its `_` arm): an open
        // never fails over a missing camera rendition.
        let missing = dir.join("nope.arw");
        assert!(preview_only(&missing).is_err(), "an absent RAW is an error");
        assert!(embedded_preview(&missing).is_err(), "…and never a silent None");
        let garbage = dir.join("garbage.arw");
        std::fs::write(&garbage, b"not a raw file at all").unwrap();
        assert!(preview_only(&garbage).is_err(), "an undecodable RAW is an error");
        assert!(embedded_preview(&garbage).is_err(), "…and never a silent None");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
