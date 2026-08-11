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
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, Rational, SRational, Value};
use rawler::get_decoder;
use rawler::rawsource::RawSource;
use rawler::tags::TiffCommonTag;

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

/// Transform profiled pixels into the sRGB working space the whole pipeline
/// assumes. A baked import that carries an ICC profile (LR "Edit in…" exports
/// ProPhoto 16-bit TIFFs by default) used to be read as if it were sRGB, so
/// the histogram, tone and HSL decisions were all computed on wrong numbers.
/// Unsupported layouts and unparseable profiles are hard errors, not a silent
/// fall-through to "assume sRGB" — that fall-through IS the bug, and in a
/// batch run it would pay for a grade computed from incorrect colors.
fn apply_icc_profile(img: &mut DynamicImage, profile: &[u8], path: &Path) -> Result<()> {
    let input = qcms::Profile::new_from_slice(profile, false)
        .ok_or_else(|| anyhow!("invalid ICC profile in {}", path.display()))?;
    let output = qcms::Profile::new_sRGB();
    let color_type = img.color();
    let (pixels, data_type): (&mut [u8], qcms::DataType) = match img {
        DynamicImage::ImageRgb8(rgb) => {
            (rgb.as_flat_samples_mut().samples, qcms::DataType::RGB8)
        }
        DynamicImage::ImageRgba8(rgba) => {
            (rgba.as_flat_samples_mut().samples, qcms::DataType::RGBA8)
        }
        // 16-bit is the MAIN customer: LR "Edit in…" hands over ProPhoto
        // 16-bit TIFFs, and refusing them here reintroduced the exact
        // workflow this function exists to serve (they opened fine — with
        // wrong colours — before the ICC pass landed).
        DynamicImage::ImageRgb16(_) | DynamicImage::ImageRgba16(_) => {
            return apply_icc_profile_16(img, &input, &output, path);
        }
        _ => anyhow::bail!(
            "ICC profile in {} accompanies {color_type:?} pixels, but qcms has no matching \
             transform that preserves this image's channel layout and bit depth",
            path.display()
        ),
    };
    let transform = qcms::Transform::new(
        &input,
        &output,
        data_type,
        qcms::Intent::Perceptual,
    )
    .ok_or_else(|| {
        anyhow!(
            "ICC profile in {} cannot transform {color_type:?} pixels into sRGB",
            path.display()
        )
    })?;
    transform.apply(pixels);
    Ok(())
}

/// The 16-bit arm of [`apply_icc_profile`]. qcms transforms 8-bit samples
/// only (DataType is RGB8/RGBA8/BGRA8/Gray8/GrayA8 — checked against qcms
/// 0.3's source), and rounding the IMAGE to 8 bits would trade the
/// colour-space error for permanent banding in every later tone move. So:
/// run the profile pair ONCE over a 33³ RGB lattice at qcms's native 8-bit
/// precision, then map the 16-bit samples through that lattice by trilinear
/// interpolation in f32. The colour mapping is 8-bit-accurate (≤1/255 per
/// lattice value — the transform's own output precision) while the DATA
/// keeps its 16-bit smoothness, because interpolation is continuous between
/// lattice points. ICC display-class transforms are smooth by construction,
/// so 33 points per axis track them closely.
fn apply_icc_profile_16(
    img: &mut DynamicImage,
    input: &qcms::Profile,
    output: &qcms::Profile,
    path: &Path,
) -> Result<()> {
    const N: usize = 33;
    let mut lattice = vec![0u8; N * N * N * 3];
    for r in 0..N {
        for g in 0..N {
            for b in 0..N {
                let i = ((r * N + g) * N + b) * 3;
                lattice[i] = (r * 255 / (N - 1)) as u8;
                lattice[i + 1] = (g * 255 / (N - 1)) as u8;
                lattice[i + 2] = (b * 255 / (N - 1)) as u8;
            }
        }
    }
    let transform = qcms::Transform::new(
        input,
        output,
        qcms::DataType::RGB8,
        qcms::Intent::Perceptual,
    )
    .ok_or_else(|| {
        anyhow!(
            "ICC profile in {} cannot transform RGB pixels into sRGB",
            path.display()
        )
    })?;
    transform.apply(&mut lattice);
    let sample = |rgb: [u16; 3]| -> [u16; 3] {
        let mut idx = [0usize; 3];
        let mut frac = [0f32; 3];
        for (c, v) in rgb.iter().enumerate() {
            let t = f32::from(*v) / 65535.0 * (N - 1) as f32;
            let i = (t as usize).min(N - 2);
            idx[c] = i;
            frac[c] = t - i as f32;
        }
        let at = |dr: usize, dg: usize, db: usize, ch: usize| -> f32 {
            let i = (((idx[0] + dr) * N + idx[1] + dg) * N + idx[2] + db) * 3 + ch;
            f32::from(lattice[i])
        };
        let mut out = [0u16; 3];
        for (ch, o) in out.iter_mut().enumerate() {
            let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
            let c00 = lerp(at(0, 0, 0, ch), at(1, 0, 0, ch), frac[0]);
            let c10 = lerp(at(0, 1, 0, ch), at(1, 1, 0, ch), frac[0]);
            let c01 = lerp(at(0, 0, 1, ch), at(1, 0, 1, ch), frac[0]);
            let c11 = lerp(at(0, 1, 1, ch), at(1, 1, 1, ch), frac[0]);
            let c0 = lerp(c00, c10, frac[1]);
            let c1 = lerp(c01, c11, frac[1]);
            let v = lerp(c0, c1, frac[2]) / 255.0;
            *o = (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
        out
    };
    match img {
        DynamicImage::ImageRgb16(rgb) => {
            for px in rgb.as_flat_samples_mut().samples.chunks_exact_mut(3) {
                let [r, g, b] = sample([px[0], px[1], px[2]]);
                px.copy_from_slice(&[r, g, b]);
            }
        }
        DynamicImage::ImageRgba16(rgba) => {
            // Alpha is coverage, not colour — it rides through untouched.
            for px in rgba.as_flat_samples_mut().samples.chunks_exact_mut(4) {
                let [r, g, b] = sample([px[0], px[1], px[2]]);
                px[..3].copy_from_slice(&[r, g, b]);
            }
        }
        _ => unreachable!("routed here only for Rgb16/Rgba16"),
    }
    Ok(())
}

const MAX_ALLOC: u64 = 4 * 1024 * 1024 * 1024;

/// The accept/reject decision for a decode's peak allocation, extracted so a
/// test can pin the BOUNDARY: equality refuses (`>` instead of `>=` would
/// admit an exact-ceiling allocation and abort the process).
fn allocation_over_ceiling(need: u64) -> bool {
    need >= MAX_ALLOC
}

/// Peak output-buffer bytes for a decode followed by `apply_orientation`: the
/// four rotating variants allocate a SECOND full buffer (`rotate90()` /
/// `rotate270()` return new images; 180°/flips are in-place), so the old
/// `total_bytes()`-only check let peak storage reach twice the 4 GiB ceiling.
fn decode_peak_bytes(need: u64, orientation: image::metadata::Orientation) -> u64 {
    if matches!(
        orientation,
        image::metadata::Orientation::Rotate90
            | image::metadata::Orientation::Rotate270
            | image::metadata::Orientation::Rotate90FlipH
            | image::metadata::Orientation::Rotate270FlipH
    ) {
        need.saturating_mul(2)
    } else {
        need
    }
}

/// Bytes per SOURCE pixel the baked develop chain holds ON TOP of the decoded
/// buffer at its peak (`render_baked_to_image`): the f32 planes
/// (`Vec<[f32; 3]>`, 12 B/px, alive to the end of the fn), the packed 16-bit
/// frame (6 B/px) and the fresh full-size output the geometric resamplers
/// allocate (6 B/px) — all while the decoded source is still borrowed
/// underneath. 12 + 6 + 6 = 24. If a further full-frame stage joins that
/// chain, this constant must grow with it.
const PIPELINE_BYTES_PER_PIXEL: u64 = 24;

/// [`decode_peak_bytes`] plus the develop chain's own full-frame buffers —
/// what the baked develop entry point really allocates for a source of
/// `pixels` px. Pure, so the boundary test can pin the accounting.
fn develop_peak_bytes(
    decoded_bytes: u64,
    orientation: image::metadata::Orientation,
    pixels: u64,
) -> u64 {
    decode_peak_bytes(decoded_bytes, orientation)
        .saturating_add(pixels.saturating_mul(PIPELINE_BYTES_PER_PIXEL))
}

/// Load a baked raster (PNG/TIFF/JPEG) with a RAISED (not lifted) decoder
/// memory limit — a 60 MP export trips the image crate's default cap, but
/// `no_limits()` let a corrupt header with absurd declared dimensions drive an
/// unbounded allocation straight into OOM. 61 MP 16-bit RGBA is ~0.5 GiB;
/// 4 GiB leaves headroom without trusting arbitrary headers; guided-mask
/// refinement keeps its derived planes tile-bounded, so the two limits no
/// longer stack into an unbudgeted full-frame peak.
/// Also applies the EXIF orientation: phone/Lightroom JPEGs store rotation as
/// metadata the decoder does NOT apply — imported photos rendered sideways
/// (the RAW path already orients via the sensor metadata).
pub fn load_image(path: &Path) -> Result<DynamicImage> {
    load_image_gated(path, false)
}

/// [`load_image`] for the full-frame baked DEVELOP path
/// (`render_baked_to_image`): charges each source pixel the develop chain's
/// downstream footprint ([`PIPELINE_BYTES_PER_PIXEL`]) on top of the decode
/// buffer, so the ceiling bounds the true pipeline peak — the plain gate
/// admitted an L8 source whose develop then peaked at ~25× the ceiling (L02).
/// Thumbnail consumers (GUI open, denoise/retouch/fit pre-shrink) stay on
/// [`load_image`]: they never build those planes, and charging them would
/// refuse sources they legitimately shrink.
pub fn load_image_for_develop(path: &Path) -> Result<DynamicImage> {
    load_image_gated(path, true)
}

fn load_image_gated(path: &Path, develop: bool) -> Result<DynamicImage> {
    use image::ImageDecoder as _;
    let mut reader = image::ImageReader::open(path)
        .with_context(|| format!("open image {}", path.display()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(65_536);
    limits.max_image_height = Some(65_536);
    limits.max_alloc = Some(4 * 1024 * 1024 * 1024);
    reader.limits(limits);
    let reader = reader
        .with_guessed_format()
        .with_context(|| format!("probe image {}", path.display()))?;
    let format = reader.format();
    let mut decoder = reader
        .into_decoder()
        .with_context(|| format!("decode image {}", path.display()))?;
    // into_decoder enforces the DIMENSION limits but skips decode()'s
    // total_bytes reservation — without this check max_alloc never bounded
    // the OUTPUT buffer (a 65536² 16-bit RGBA header passes the dimension
    // gate yet decodes to ~32 GiB).
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let decoded_bytes = decoder.total_bytes();
    let (dw, dh) = decoder.dimensions();
    let need = if develop {
        develop_peak_bytes(
            decoded_bytes,
            orientation,
            u64::from(dw).saturating_mul(u64::from(dh)),
        )
    } else {
        decode_peak_bytes(decoded_bytes, orientation)
    };
    if allocation_over_ceiling(need) {
        anyhow::bail!(
            "image {} needs {need} bytes at peak (decode {decoded_bytes}, then orientation{}) \
             — at or over the {MAX_ALLOC}-byte ceiling",
            path.display(),
            if develop { ", then the develop chain's full-frame buffers" } else { "" }
        );
    }
    let icc_profile = decoder
        .icc_profile()
        .with_context(|| format!("read ICC profile {}", path.display()))?;
    // image 0.25's TiffDecoder::set_limits breaks IFD tag-value reads: the
    // tiff crate accounts a tag read as count × size_of::<Value>() (~32 bytes
    // per profile BYTE) against `decoding_buffer_size`, which set_limits pins
    // to the image's own byte size — probed empirically ("decoder limits
    // exceeded", swallowed by `.ok()` inside the codec into a silent None).
    // Big LR exports clear the budget; small profiled TIFFs would silently
    // skip the transform — the exact assume-sRGB bug this function fixes. Ask
    // a fresh, header-only decoder instead; its tag reads run under the tiff
    // crate's own 1 MiB per-value default, and NO pixel is ever decoded here.
    let icc_profile = match icc_profile {
        Some(p) => Some(p),
        None if format == Some(image::ImageFormat::Tiff) => {
            image::codecs::tiff::TiffDecoder::new(std::io::BufReader::new(
                std::fs::File::open(path)
                    .with_context(|| format!("open image {}", path.display()))?,
            ))
            .ok()
            .and_then(|mut d| d.icc_profile().ok().flatten())
        }
        None => None,
    };
    let mut img = DynamicImage::from_decoder(decoder)
        .with_context(|| format!("decode image {}", path.display()))?;
    if let Some(profile) = icc_profile {
        apply_icc_profile(&mut img, &profile, path)?;
    }
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

/// Process-wide cap on CONCURRENT full-res embedded-preview decodes. A 61 MP
/// ARW preview lands as a 9504×6336 Rgb8 frame (~181 MB), and a portrait
/// rotation holds a second copy while it turns. `StyleIndex::build`'s worker
/// pool bounds ONE build; two builds in the same process (the web server's
/// request threads — see `StyleIndex::save`'s tmp-name comment) used to
/// stack 8 decodes ≈ 1.4 GB. This bounds them together. Taken at the BUILD
/// call site only: gating inside `decode_raw` would silently throttle the
/// GUI thumbnail path, whose own cap deliberately keeps RAW thumbs
/// concurrent.
pub const MAX_CONCURRENT_DECODES: usize = 4;
const _: () = assert!(MAX_CONCURRENT_DECODES > 0);

fn decode_gate() -> &'static (std::sync::Mutex<usize>, std::sync::Condvar) {
    static GATE: std::sync::OnceLock<(std::sync::Mutex<usize>, std::sync::Condvar)> =
        std::sync::OnceLock::new();
    GATE.get_or_init(Default::default)
}

/// One decode slot, released on DROP — mandatory, not tidiness: rawler runs
/// third-party parsers over untrusted files, and a permit leaked by an
/// unwind inside `thread::scope` would park every other worker forever
/// instead of letting the scope join and re-panic. Poison is recovered,
/// never re-panicked — a panic inside Drop during an unwind aborts the
/// process.
pub struct DecodePermit(());

impl DecodePermit {
    pub fn acquire() -> Self {
        let (lock, cv) = decode_gate();
        let mut n = lock.lock().unwrap_or_else(|p| p.into_inner());
        while *n >= MAX_CONCURRENT_DECODES {
            n = cv.wait(n).unwrap_or_else(|p| p.into_inner());
        }
        *n += 1;
        DecodePermit(())
    }
}

impl Drop for DecodePermit {
    fn drop(&mut self) {
        let (lock, cv) = decode_gate();
        let mut n = lock.lock().unwrap_or_else(|p| p.into_inner());
        *n = n.saturating_sub(1);
        cv.notify_one();
    }
}

/// Refuse a TIFF-container RAW whose top-level IFD chain CYCLES, before
/// rawler ever walks it.
///
/// Upstream defect (rawler 0.7.2, `formats/tiff/reader.rs:164-179`): the
/// chain walker keeps no visited-offset set, and its only `break` is inside
/// `if let Some(max) = max_chained` — which never fires for `None`. Every
/// path into `get_decoder` passes `None` (`decoders/mod.rs:909`, hard-coded
/// and unreachable from here), so an IFD whose `next_ifd` points at itself
/// spins forever pushing a fresh IFD per iteration: unbounded time AND
/// memory. Nothing downstream can contain it — there is no panic for
/// `spawn_worker`'s `catch_unwind` to catch, allocation failure aborts
/// rather than unwinds, and Rust cannot kill a spinning thread.
///
/// CONTAINMENT, NOT A TERMINATION PROOF. This walk mirrors the straight
/// `count × 12` entry stride; rawler's `IFD::new` can abandon its entry loop
/// early on a malformed entry and then read `next_ifd` from wherever its
/// reader stopped, so a file crafted against that divergence could follow a
/// different chain and still hang. It stops the natural crafted cycle. The
/// complete cure is upstream (a visited-offset set, plus a sane default cap
/// for `None`) or decoding in a subprocess.
///
/// NOT COVERED, same upstream file class: `Entry::parse`
/// (`formats/tiff/entry.rs:106-112` and its siblings) sizes
/// `vec![0; count as usize]` from a file-supplied u32 BEFORE reading, so a
/// SINGLE entry can demand up to 4 GiB (34 GiB for DOUBLE). No app-side
/// interception exists short of vendoring rawler; a chain of length one
/// reaches it, and this guard does nothing about it.
///
/// Called from EVERY `get_decoder` caller in the crate — `decode_raw`,
/// `camera_rendition`, the full-res sensor render, and `as_shot_wb`. A new
/// `get_decoder` call site needs this line too.
///
/// Rejects ONLY on proof; any IO error or non-TIFF magic passes through, so
/// no file that decodes today changes behaviour (truncation is end-of-chain
/// to rawler, not an error, and CR3/RAF/X3F never reach the TIFF probe).
pub(crate) fn guard_tiff_chain(path: &Path) -> Result<()> {
    use std::io::{Read as _, Seek as _};
    /// Far above any real file (a RAW carries ≤4) and above rawler's own
    /// internal caps of 10/16 — a chain this long is already pathological.
    const MAX_IFDS: usize = 64;
    fn u16at(r: &mut impl std::io::Read, le: bool) -> std::io::Result<u16> {
        let mut b = [0u8; 2];
        r.read_exact(&mut b)?;
        Ok(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    }
    fn u32at(r: &mut impl std::io::Read, le: bool) -> std::io::Result<u32> {
        let mut b = [0u8; 4];
        r.read_exact(&mut b)?;
        Ok(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }
    let Ok(file) = std::fs::File::open(path) else { return Ok(()) };
    let mut r = std::io::BufReader::new(file);
    let mut bom = [0u8; 2];
    if r.read_exact(&mut bom).is_err() {
        return Ok(());
    }
    let le = match &bom {
        b"II" => true,
        b"MM" => false,
        _ => return Ok(()), // not a TIFF container — rawler probes it elsewhere
    };
    // The magic word is READ but not enforced: rawler's own check is
    // commented out (reader.rs:151-154), which is how Panasonic RW2's 0x55
    // reaches the same walker. Enforcing 42 here would leave RW2 unguarded.
    if u16at(&mut r, le).is_err() {
        return Ok(());
    }
    // base = corr = 0 at every call site we guard (decoders/mod.rs:909), so
    // there is no offset correction to model.
    let Ok(mut next) = u32at(&mut r, le) else { return Ok(()) };
    let mut seen = std::collections::HashSet::new();
    let mut walked = 0usize;
    while next != 0 {
        if !seen.insert(next) {
            anyhow::bail!(
                "{} has a cyclic TIFF IFD chain (offset {next} repeats) — refusing to decode",
                path.display()
            );
        }
        walked += 1;
        if walked > MAX_IFDS {
            anyhow::bail!(
                "{} has a TIFF IFD chain longer than {MAX_IFDS} entries — refusing to decode",
                path.display()
            );
        }
        if r.seek(std::io::SeekFrom::Start(u64::from(next))).is_err() {
            return Ok(());
        }
        let Ok(entries) = u16at(&mut r, le) else { return Ok(()) };
        if r.seek(std::io::SeekFrom::Current(i64::from(entries) * 12)).is_err() {
            return Ok(());
        }
        let Ok(n) = u32at(&mut r, le) else { return Ok(()) };
        next = n;
    }
    Ok(())
}

/// Decode a RAW file: embedded preview + metadata + histogram. Reads the file
/// only; never writes near the source.
pub fn decode_raw(path: &Path) -> Result<Decoded> {
    guard_tiff_chain(path)?;
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

    // ONE implementation ([`embedded_xmp`]): the per-decoder `xpacket` this
    // call used covers only CR2/CR3/RAF/TFR (rawler 0.7.2's default is
    // `Ok(None)`), so a DNG — the format that actually bakes develops into
    // the file — always reported "none" here. A read failure is disclosed,
    // never folded into absent (the old `.ok()` did exactly that).
    let embedded_xmp = match embedded_xmp(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("⚠ {e}");
            None
        }
    };

    Ok(Decoded { preview, meta, histogram, embedded_xmp })
}

/// The RAW's embedded XMP packet — the develop Lightroom bakes INTO a DNG
/// ("Store presets with this catalog" workflows write it for CR2/RAF too) —
/// bounded and honest: `Ok(None)` means no packet this reader can prove
/// exists (including a container that does not walk as TIFF — such a photo
/// fails loudly at decode, where the real diagnosis lives); a packet that
/// provably EXISTS but cannot be used (over-cap, non-text, unrecognised
/// type) is `Err`, never folded into "absent".
///
/// TIFF containers (DNG/ARW/NEF/ORF/RW2/CR2/…) are read through the SAME
/// bounded header walk the lens-metadata reader uses (`lensmeta::read`): a
/// `BufReader` + [`GenericTiffReader`] with a chain cap — roughly a root-IFD
/// parse, cheap enough for the open path. rawler's per-format `xpacket`
/// overrides cover only CR2/CR3/RAF/TFR, which would MISS the DNG tag
/// 0x02BC entirely; only the non-TIFF containers (CR3's BMFF, RAF's
/// proprietary header) go through the decoder — that branch maps the whole
/// file (`RawSource::new`), the cost `decode_raw` itself already pays, and
/// is reached for those two extensions alone.
pub fn embedded_xmp(path: &Path) -> Result<Option<String>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let bytes: Option<Vec<u8>> = if matches!(ext.as_deref(), Some("cr3" | "raf")) {
        guard_tiff_chain(path)?;
        let src =
            RawSource::new(path).with_context(|| format!("open RAW {}", path.display()))?;
        let decoder = get_decoder(&src)
            .map_err(|e| anyhow!("no rawler decoder for {}: {e}", path.display()))?;
        decoder
            .xpacket(&src, &RawDecodeParams { image_index: 0 })
            .map_err(|e| anyhow!("read the XMP packet in {}: {e}", path.display()))?
    } else {
        let file = std::fs::File::open(path)
            .with_context(|| format!("open RAW {}", path.display()))?;
        let mut reader = std::io::BufReader::new(file);
        // Same chain cap and rationale as `lensmeta::read` — see that call
        // site. A file whose header does not walk as TIFF has no TIFF tag to
        // hold a packet: that is absence, not unreadability (the photo
        // itself will fail loudly elsewhere if it is genuinely corrupt).
        let Ok(tiff) = GenericTiffReader::new(&mut reader, 0, 0, Some(16), &[]) else {
            return Ok(None);
        };
        match tiff.root_ifd().get_entry(TiffCommonTag::Xmp) {
            // BYTE is the spelling the XMP spec prescribes and rawler's own
            // tfr decoder reads; UNDEFINED is the one other shape real
            // writers use. Anything else is a packet we cannot account for —
            // said, not skipped.
            Some(entry) => match &entry.value {
                Value::Byte(b) | Value::Undefined(b) => Some(b.clone()),
                other => {
                    return Err(anyhow!(
                        "the XMP packet in {} has an unrecognised TIFF type ({})",
                        path.display(),
                        other.value_type()
                    ));
                }
            },
            None => None,
        }
    };
    let Some(buf) = bytes else { return Ok(None) };
    // The same 16 MiB ceiling every sidecar read enforces — refused, not
    // truncated (a partial packet would parse to a DIFFERENT develop).
    if buf.len() as u64 > crate::store::MAX_STORE_JSON {
        return Err(anyhow!(
            "the XMP packet in {} is larger than the {}-byte limit",
            path.display(),
            crate::store::MAX_STORE_JSON
        ));
    }
    let text = String::from_utf8(buf)
        .map_err(|_| anyhow!("the XMP packet in {} is not UTF-8 text", path.display()))?;
    // Packets are xpacket-padded with trailing whitespace; blank-after-trim
    // carries nothing to restore.
    Ok((!text.trim().is_empty()).then_some(text))
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
    guard_tiff_chain(path)?;
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

    /// A minimal ICC v2 RGB matrix/TRC profile, built byte-by-byte so the
    /// test needs no binary asset (Adobe's sRGB profile is not
    /// redistributable). sRGB D50-adapted primaries, linear TRCs (count=0
    /// curv) — enough for qcms to build an RGB8 transform.
    fn build_test_icc() -> Vec<u8> {
        let mut p = vec![0u8; 128];
        let be = |v: u32| v.to_be_bytes();
        p[8] = 0x02; // version 2.0; bytes 10..12 stay 0 (qcms checks them)
        p[12..16].copy_from_slice(b"mntr");
        p[16..20].copy_from_slice(b"RGB ");
        p[20..24].copy_from_slice(b"XYZ ");
        p[36..40].copy_from_slice(b"acsp");
        // rendering intent @64 already 0 = Perceptual
        // D50 illuminant @68 (s15Fixed16), read by some CMMs; harmless if not
        p[68..72].copy_from_slice(&be(0x0000F6D6));
        p[72..76].copy_from_slice(&be(0x00010000));
        p[76..80].copy_from_slice(&be(0x0000D32D));

        let xyz = |x: u32, y: u32, z: u32| {
            let mut t = Vec::new();
            t.extend_from_slice(b"XYZ ");
            t.extend_from_slice(&be(0));
            for v in [x, y, z] {
                t.extend_from_slice(&be(v));
            }
            t
        };
        // linear TRC: 'curv' + reserved + count=0
        let curv: Vec<u8> = [b"curv".as_slice(), &be(0), &be(0)].concat();
        // sRGB primaries adapted to D50, in s15Fixed16 (value * 65536)
        let tags: [(&[u8; 4], Vec<u8>); 6] = [
            (b"rXYZ", xyz(28573, 14582, 911)),
            (b"gXYZ", xyz(25238, 46982, 6364)),
            (b"bXYZ", xyz(9378, 3972, 46786)),
            (b"rTRC", curv.clone()),
            (b"gTRC", curv.clone()),
            (b"bTRC", curv),
        ];
        p.extend_from_slice(&be(tags.len() as u32));
        let mut offset = 128 + 4 + 12 * tags.len() as u32;
        let mut data = Vec::new();
        for (sig, body) in &tags {
            p.extend_from_slice(*sig);
            p.extend_from_slice(&be(offset));
            p.extend_from_slice(&be(body.len() as u32));
            offset += body.len() as u32;
            data.extend_from_slice(body);
        }
        p.extend_from_slice(&data);
        let len = be(p.len() as u32);
        p[0..4].copy_from_slice(&len);
        p
    }

    #[test]
    fn embedded_icc_profiles_are_processed_or_rejected_loudly() {
        use image::ImageEncoder as _;

        let dir =
            std::env::temp_dir().join(format!("autoshop-decode-icc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // PNG: the one format whose image-crate encoder round-trips an ICC
        // blob (the TIFF encoder writes a tag its own decoder reads as None —
        // probed; see the tiff dev-dependency note in Cargo.toml).
        let write = |path: &std::path::Path, profile: &[u8]| {
            let file = std::fs::File::create(path).unwrap();
            let mut encoder = image::codecs::png::PngEncoder::new(file);
            encoder.set_icc_profile(profile.to_vec()).unwrap();
            encoder
                .write_image(
                    &[32, 128, 240],
                    1,
                    1,
                    image::ExtendedColorType::Rgb8,
                )
                .unwrap();
        };

        let valid = dir.join("valid-profile.png");
        write(&valid, &build_test_icc());
        let img = load_image(&valid).expect("a supported embedded profile is transformed");
        // The transform must ACTUALLY run, not merely parse: this profile's
        // TRCs are linear, so encoding (32,128,240) back to sRGB visibly
        // brightens it — a no-op `transform.apply` loaded the raw samples as
        // sRGB with silently wrong colour (16-lane scan L14).
        let px = img.to_rgb8().get_pixel(0, 0).0;
        assert_ne!(
            px,
            [32, 128, 240],
            "the RGB8 ICC pass left the pixels untransformed"
        );

        let invalid = dir.join("invalid-profile.png");
        write(&invalid, b"not an ICC profile");
        assert!(
            load_image(&invalid).is_err(),
            "an invalid profile must not fall through to assumed sRGB"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The TIFF path is the one README's LR/Topaz round-trip actually takes,
    /// and it needs the fallback query: image's `TiffDecoder::set_limits`
    /// makes IFD tag-value reads fail ("decoder limits exceeded" — the tiff
    /// crate charges ~32 bytes of budget per profile byte against the image's
    /// own byte size), and the codec swallows that into `None`. Without the
    /// fallback this fixture loads Ok with the profile silently ignored.
    #[test]
    fn a_profiled_tiff_reaches_the_transform_despite_the_limits_bug() {
        let dir =
            std::env::temp_dir().join(format!("autoshop-decode-icctiff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let write = |path: &std::path::Path, profile: &[u8]| {
            let f = std::fs::File::create(path).unwrap();
            let mut enc =
                tiff::encoder::TiffEncoder::new(std::io::BufWriter::new(f)).unwrap();
            let mut img = enc
                .new_image::<tiff::encoder::colortype::RGB8>(1, 1)
                .unwrap();
            img.encoder()
                .write_tag(tiff::tags::Tag::IccProfile, profile)
                .unwrap();
            img.write_data(&[32u8, 128, 240]).unwrap();
        };

        let valid = dir.join("valid-profile.tiff");
        write(&valid, &build_test_icc());
        load_image(&valid).expect("a profiled TIFF is transformed via the fallback query");

        // The load-bearing half: an unparseable profile must ERROR. If the
        // fallback is removed, the profile is never seen and this loads Ok.
        let invalid = dir.join("invalid-profile.tiff");
        write(&invalid, b"not an ICC profile");
        assert!(
            load_image(&invalid).is_err(),
            "a TIFF profile must reach the parser, not vanish into the limits bug"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The LR "Edit in…" shape itself: a PROFILED 16-BIT TIFF must open,
    /// stay 16-bit, and come back transformed (the fixture's linear TRC is
    /// far from sRGB's curve, so mid-tones must move visibly). Refusing the
    /// layout — the first ICC pass did — failed the exact workflow the
    /// module docs promise.
    #[test]
    fn a_profiled_16bit_image_transforms_and_keeps_its_depth() {
        let dir = std::env::temp_dir()
            .join(format!("autoshop-decode-icc16-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // PNG carrier: its encoder round-trips both 16-bit samples and the
        // ICC blob (the tiff encoder writes a corrupt file for RGB16 plus an
        // extra tag — probed: the result fails to decode at all). The
        // format-specific TIFF limits fallback has its own test above; the
        // code under test HERE (the 16-bit LUT transform) is format-blind.
        use image::ImageEncoder as _;
        let p = dir.join("profiled-16bit.png");
        let f = std::fs::File::create(&p).unwrap();
        let mut enc = image::codecs::png::PngEncoder::new(f);
        enc.set_icc_profile(build_test_icc()).unwrap();
        let mid: [u16; 3] = [8224, 32896, 61680]; // 32/128/240 at 16-bit
        let bytes: Vec<u8> = mid.iter().flat_map(|v| v.to_be_bytes()).collect();
        enc.write_image(&bytes, 1, 1, image::ExtendedColorType::Rgb16)
            .unwrap();

        let loaded = load_image(&p).expect("a profiled 16-bit TIFF must open");
        let DynamicImage::ImageRgb16(rgb) = &loaded else {
            panic!("bit depth must survive the transform, got {:?}", loaded.color());
        };
        let got = rgb.get_pixel(0, 0).0;
        assert_ne!(
            got, mid,
            "linear-TRC input through the sRGB transform must move the values"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copying_orientations_fit_two_buffers_strictly_below_the_ceiling() {
        use image::metadata::Orientation;

        assert_eq!(
            decode_peak_bytes(MAX_ALLOC / 2, Orientation::Rotate90),
            MAX_ALLOC
        );
        assert!(
            decode_peak_bytes(MAX_ALLOC / 2 - 1, Orientation::Rotate270FlipH) < MAX_ALLOC
        );
        assert_eq!(
            decode_peak_bytes(MAX_ALLOC - 1, Orientation::Rotate180),
            MAX_ALLOC - 1
        );
        assert_eq!(
            decode_peak_bytes(MAX_ALLOC, Orientation::NoTransforms),
            MAX_ALLOC
        );
    }

    /// The PREDICATE, not just the arithmetic: `>` instead of `>=` admits an
    /// exact-4-GiB allocation the ceiling exists to refuse (16-lane scan L14).
    #[test]
    fn the_allocation_ceiling_refuses_equality_and_admits_one_byte_below() {
        assert!(allocation_over_ceiling(MAX_ALLOC));
        assert!(!allocation_over_ceiling(MAX_ALLOC - 1));
    }

    /// L02: the cyclic-IFD guard — a self-referential chain is refused, a
    /// straight two-IFD chain passes, and a non-TIFF file passes through
    /// untouched (rejecting there would break CR3/RAF/X3F).
    #[test]
    fn guard_tiff_chain_refuses_a_cycle_and_admits_a_straight_chain() {
        fn tiff(ifds: &[(u32, u32)]) -> Vec<u8> {
            // Little-endian header pointing at the first listed IFD, then one
            // 1-entry IFD written at each (offset, next_ifd) pair.
            let end = ifds.iter().map(|(o, _)| *o).max().unwrap_or(8) as usize + 32;
            let mut b = vec![0u8; end];
            b[..2].copy_from_slice(b"II");
            b[2..4].copy_from_slice(&42u16.to_le_bytes());
            b[4..8].copy_from_slice(&ifds[0].0.to_le_bytes());
            for (off, next) in ifds {
                let o = *off as usize;
                b[o..o + 2].copy_from_slice(&1u16.to_le_bytes()); // one entry
                b[o + 2..o + 14].copy_from_slice(&[0u8; 12]); // its 12 bytes
                b[o + 14..o + 18].copy_from_slice(&next.to_le_bytes());
            }
            b
        }
        let dir = std::env::temp_dir().join("autoshop-tiff-guard-test");
        std::fs::create_dir_all(&dir).unwrap();
        let selfloop = dir.join("selfloop.tif");
        std::fs::write(&selfloop, tiff(&[(8, 8)])).unwrap();
        let err = guard_tiff_chain(&selfloop).unwrap_err().to_string();
        assert!(err.contains("cyclic"), "{err}");
        let two_cycle = dir.join("two.tif");
        std::fs::write(&two_cycle, tiff(&[(8, 40), (40, 8)])).unwrap();
        assert!(guard_tiff_chain(&two_cycle).is_err(), "A→B→A must be refused");
        let straight = dir.join("straight.tif");
        std::fs::write(&straight, tiff(&[(8, 40), (40, 0)])).unwrap();
        guard_tiff_chain(&straight).expect("a straight chain must pass");
        let foreign = dir.join("foreign.bin");
        std::fs::write(&foreign, b"ftypcrx not a tiff at all").unwrap();
        guard_tiff_chain(&foreign).expect("non-TIFF must pass through");
    }

    /// A minimal little-endian TIFF whose root IFD carries ONE entry: tag
    /// 0x02BC (XMP), type BYTE, the given payload. Word-aligned data offset,
    /// no image data — exactly what the packet reader consults.
    fn tiff_with_xmp(payload: &[u8]) -> Vec<u8> {
        let mut f: Vec<u8> = Vec::new();
        f.extend(b"II");
        f.extend(42u16.to_le_bytes());
        f.extend(8u32.to_le_bytes()); // root IFD at byte 8
        f.extend(1u16.to_le_bytes()); // one entry
        f.extend(0x02BCu16.to_le_bytes());
        f.extend(1u16.to_le_bytes()); // type BYTE
        f.extend((payload.len() as u32).to_le_bytes());
        if payload.len() <= 4 {
            let mut v = [0u8; 4];
            v[..payload.len()].copy_from_slice(payload);
            f.extend(v);
        } else {
            f.extend(26u32.to_le_bytes()); // 8 header + 2 count + 12 entry + 4 next
        }
        f.extend(0u32.to_le_bytes()); // no next IFD
        if payload.len() > 4 {
            f.extend(payload);
        }
        f
    }

    /// L05#6: the packet lives in TIFF tag 0x02BC for every TIFF-container
    /// RAW — DNG included, where rawler 0.7.2's per-format `xpacket` answers
    /// None (only CR2/CR3/RAF/TFR override it). The reader is honest at both
    /// edges: over the cap is REFUSED (a truncated packet would parse to a
    /// different develop), and a non-text packet is an error, never "absent".
    #[test]
    fn an_embedded_xmp_packet_is_read_from_the_tiff_tag() {
        let dir = std::env::temp_dir().join("autoshop-embedded-xmp-test");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = crate::xmp::recipe_to_xmp(&crate::recipe::EditRecipe {
            exposure_ev: 0.8,
            ..Default::default()
        });
        let dng = dir.join("packet.dng");
        std::fs::write(&dng, tiff_with_xmp(doc.as_bytes())).unwrap();
        let text = embedded_xmp(&dng).expect("readable").expect("present");
        assert_eq!(crate::xmp::xmp_to_recipe(&text).exposure_ev, 0.8);

        // No 0x02BC entry → genuinely no packet.
        let plain = dir.join("plain.dng");
        std::fs::write(&plain, {
            let mut f = tiff_with_xmp(b"x");
            f[10] = 0x00; // retag the entry: 0x02BC → 0x0200 (not XMP)
            f
        })
        .unwrap();
        assert!(embedded_xmp(&plain).expect("readable").is_none());

        // A non-TIFF container (that is also not CR3/RAF) has no TIFF tag to
        // hold a packet: absence, not an error.
        let junk = dir.join("junk.dng");
        std::fs::write(&junk, b"not a tiff").unwrap();
        assert!(embedded_xmp(&junk).expect("no packet, no error").is_none());

        // Non-UTF-8 packet bytes: an ERROR naming the file, never absence.
        let bad = dir.join("badtext.dng");
        std::fs::write(&bad, tiff_with_xmp(&[0xFF, 0xFE, 0x00, 0x01, 0x02])).unwrap();
        let err = embedded_xmp(&bad).unwrap_err().to_string();
        assert!(err.contains("not UTF-8"), "{err}");

        // Over the cap: REFUSED with the limit named, not truncated.
        let big = dir.join("big.dng");
        let payload = vec![b'x'; crate::store::MAX_STORE_JSON as usize + 1];
        std::fs::write(&big, tiff_with_xmp(&payload)).unwrap();
        let err = embedded_xmp(&big).unwrap_err().to_string();
        assert!(err.contains("larger than"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L02: the develop entry charges downstream bytes-per-pixel — the same
    /// source clears the plain decode gate yet refuses the develop gate, and
    /// the documented 61 MP RGBA16 target still clears it with headroom.
    #[test]
    fn develop_peak_accounting_charges_downstream_bpp() {
        use image::metadata::Orientation;
        let px: u64 = 200_000_000; // 200 MP L8: decode 200 MB, develop peak 5 GB
        assert!(!allocation_over_ceiling(decode_peak_bytes(px, Orientation::NoTransforms)));
        assert!(allocation_over_ceiling(develop_peak_bytes(
            px,
            Orientation::NoTransforms,
            px
        )));
        let target: u64 = 61_000_000; // 61 MP RGBA16, even rotated
        assert!(!allocation_over_ceiling(develop_peak_bytes(
            target * 8,
            Orientation::Rotate90,
            target
        )));
    }
}
