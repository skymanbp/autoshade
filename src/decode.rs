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

/// Every camera-RAW extension the app opens — THE list `is_raw` is, and the
/// one every hand-copied extension list in the tree derives from (the GUI file
/// dialog, the web `accept` attribute, the library scanners). A second copy is
/// drift waiting to happen: `.orf`/`.rw2`/`.raw` were openable for four
/// releases while the file dialog refused them.
///
/// **What is in it.** One entry per rawler 0.7.2 decoder that has a real
/// filename extension. rawler dispatches on CONTENT (magic bytes, then the
/// TIFF `Make` string — `decoders/mod.rs:847-994`), never on the extension, so
/// this list only has to name files worth handing it; the decoder choice is
/// rawler's. `3fr`/`fff` are Hasselblad (the `tfr` decoder), `mos` is Leaf,
/// `nrw` is Nikon's compact line, `mrw` Minolta, `ari` ARRI.
///
/// **What is deliberately NOT in it.**
///   * `x3f` (Sigma Foveon) — PERMANENTLY excluded. rawler 0.7.2's
///     `decoders/x3f.rs:138` (`format_dump`) and `:146` (`raw_metadata`) are
///     literal `todo!()`, i.e. a guaranteed panic, and we call `raw_metadata`
///     directly rather than through rawler's own `catch_unwind` wrapper. The
///     CLI guard added alongside this list ([`guard_parser_panic`]) turns that
///     into a named error instead of an abort, but a format whose metadata
///     reader cannot run at all has nothing to offer a photographer — listing
///     it would only promise support that provably does not exist. Revisit
///     when the upstream `todo!()`s become real code, not before.
///   * `nkd` / "unwrapped" — rawler decoders with no extension of their own
///     (CHDK-style naked dumps matched by FILE SIZE, `mod.rs:989-992`).
///   * `qtk` — QuickTake, matched by magic bytes; the extension is `.qtk` but
///     the format is a 1994 Apple curiosity with no develop path worth
///     claiming.
pub const RAW_EXTS: [&str; 24] = [
    "arw", "dng", "raw", "raf", "nef", "cr2", "cr3", "orf", "rw2", "pef", "srw", "3fr", "fff",
    "iiq", "mef", "mos", "erf", "kdc", "dcr", "dcs", "crw", "nrw", "mrw", "ari",
];

/// Does this path look like a camera RAW (vs an already-baked raster like a
/// LR/PS-exported PNG/TIFF/JPEG)? Drives the raw-vs-baked dispatch.
///
/// Accepting an extension is NOT a promise that the body decodes: rawler
/// carries 725 camera models and refuses anything outside them
/// ([`describe_decoder_failure`] turns that into a sentence a photographer can
/// act on). It IS a promise that the file reaches the RAW engine rather than
/// the baked one, which is the only decision this predicate makes.
pub fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RAW_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
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
/// buffer at its peak (`render_baked_to_image`). The f32 planes
/// (`Vec<[f32; 3]>`, 12 B/px) are alive from the transcode to the end of the
/// function; what rides ALONGSIDE them is what varies, and the peak is the
/// worst of those moments — enumerated as data (not prose) by
/// `develop_peak_accounts_for_every_pass`, which is where a new stage lands.
///
/// Three of those moments tie at 24: a spatial pass (12 + the luma plane 4 +
/// `blur_plane`'s two chained planes 8), a geometric resample (12 + the Rgb16
/// frame 6 + the resampler's fresh output 6) and the AI-denoise round trip.
/// The chain has been tuned to that ceiling deliberately (the A7 batch), so
/// "12 + 6 + 6" is one path to 24 rather than the whole account.
///
/// A further full-frame stage raises this constant only if it is alive AT THE
/// SAME TIME as the planes: three SEQUENTIAL unsharp passes (clarity, texture,
/// sharpening) each drop their two planes before the next allocates
/// (`render.rs`'s memory note on `apply_masks`), so adding one costs nothing
/// here. A stage that holds a new buffer concurrently costs its full width.
///
/// **R27 R7 — re-derived, not re-assumed, when the RAW format list went from 9
/// extensions to 24.** The worry raised was "this was tuned on a 61 MP Bayer
/// A7; what about a 100 MP GFX or a Phase One back?". Two facts answer it:
///
/// 1. **It never applies to a RAW at all.** The only consumer is
///    [`develop_peak_bytes`], reached only from `load_image_gated(develop =
///    true)` — and that gate REFUSES a camera RAW by name a few lines in. A
///    RAW's develop is budgeted by the decode-scope lifetimes in
///    `render::render_to_image_in`, not here. So sensor size and CFA layout
///    (Bayer vs X-Trans vs 4-colour) are outside this constant's world; what
///    it budgets is a baked PNG/TIFF/JPEG, which has no CFA by definition.
/// 2. **It is per-SOURCE-PIXEL, so it does not carry a resolution
///    assumption.** Every term above is a fixed number of bytes for one pixel
///    (12 B of `[f32; 3]`, 4 B luma, 6 B of Rgb16, …); the pixel COUNT is the
///    other multiplicand in `develop_peak_bytes` and comes from the file's own
///    header. Doubling the megapixels doubles the product, which is the
///    intended behaviour — it is what makes the 4 GiB ceiling bite on a
///    200 MP panorama and not on a 24 MP export.
///
/// What "tuned to that ceiling deliberately (the A7 batch)" refers to is WHICH
/// STAGES exist and which of them overlap — the enumeration in
/// `develop_peak_accounts_for_every_pass` — not a pixel count. That
/// enumeration is what a new stage must update; the 61 MP number never enters
/// the arithmetic.
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

/// Bytes of COMMIT charge one SOURCE pixel of a camera RAW costs at the PEAK of
/// `render::render_to_image_in` — the RAW twin of [`PIPELINE_BYTES_PER_PIXEL`],
/// and what lets the same 4 GiB per-file ceiling be charged to a RAW at all.
///
/// **MEASURED, not derived.** 1,771 MB of peak process commit over a
/// 9504x6336 (60.2 MP) A7R V ARW = 30.8 B/px, read as `PeakPagefileUsage` by
/// `jobs::tests::probe_per_photo_peak_commit` (release profile, one stage per
/// process — that probe's own docs carry the method, `jobs`' module docs the
/// stage table). Rounded UP to 31: this number multiplies a pixel count into a
/// REFUSAL, and rounding a peak DOWN is how a file that really does blow the
/// ceiling gets admitted.
///
/// It is the WHOLE develop's peak, not one buffer's width. The demosaiced f32
/// frame (`Vec<[f32; 3]>`, 12 B/px) is only its largest single term; rawler's
/// own sensor buffers, the mapped file, the orientation transient and the
/// 16-bit pack are all inside the measured figure. That is deliberate — the
/// baked side can enumerate its overlapping stages because it OWNS them
/// (`PIPELINE_BYTES_PER_PIXEL`'s doc does exactly that), while a RAW's peak is
/// mostly inside rawler, where an enumeration would be a guess about somebody
/// else's allocations. A measurement of the whole is the honest form.
///
/// **Corpus caveat, stated rather than hidden**: one body, one CFA, one
/// compression mode. A format whose decoder holds more than rawler's ARW path
/// does would peak higher per pixel, and this constant would then admit a file
/// it should refuse. Re-run the probe when the RAW format list grows a decoder
/// with a different shape (the same discipline `PIPELINE_BYTES_PER_PIXEL`'s
/// R27 R7 note records for the baked side).
pub const RAW_DEVELOP_BYTES_PER_PIXEL: u64 = 31;

/// What developing a `pixels`-pixel camera RAW peaks at. Pure, so the boundary
/// test can pin the accounting the way it pins the baked one.
fn raw_develop_peak_bytes(pixels: u64) -> u64 {
    pixels.saturating_mul(RAW_DEVELOP_BYTES_PER_PIXEL)
}

/// THE RAW DEVELOP CEILING — the twin of the baked gate inside
/// [`load_image_gated`], and the closing of what the R28 adjudication (F2)
/// called the deeper root: the baked path has refused an over-ceiling file
/// since L02, while **the RAW path had no per-file limit at all**. A 150 MP
/// IIQ on the default `batch --jobs 3` was therefore a worse instance of the
/// same defect than the constructed TIFF scenario that raised it, with nothing
/// opt-in about it.
///
/// ONE call site, `render::render_to_image_in` — the single funnel every RAW's
/// pixels pass through (`render_to_file`'s RAW arm, `source_pixels`' RAW arm,
/// `render_to_image`, and through those three the CLI, the GUI and `serve`).
/// Patching the callers instead would have been the shape this file already
/// refuses for `load_image` (see that gate's doc): a per-caller copy is a
/// per-caller chance to forget.
///
/// REFUSE, never degrade (user ruling 2026-08-20, plan D2): the alternative —
/// silently developing at a reduced resolution — would hand back a deliverable
/// that is not the one asked for.
fn refuse_raw_develop_over_ceiling(path: &Path, pixels: u64) -> Result<()> {
    let need = raw_develop_peak_bytes(pixels);
    if allocation_over_ceiling(need) {
        // Everything a person can act on: the measured basis (so the estimate
        // is auditable rather than an oracle), and — because the obvious guess
        // is wrong — that the concurrency flag is not the answer.
        anyhow::bail!(
            "{} is a {pixels}-pixel camera RAW; developing it peaks at about {need} bytes \
             ({pixels} px x {RAW_DEVELOP_BYTES_PER_PIXEL} B/px, measured) — at or over the \
             {MAX_ALLOC}-byte ceiling this build will commit for ONE file. `--jobs 1` does not \
             help: this is a single file's own peak, not a concurrency budget. This build cannot \
             develop a photo this large without paging, so it refuses instead of stalling the \
             machine or aborting on a failed allocation",
            path.display()
        );
    }
    Ok(())
}

/// [`refuse_raw_develop_over_ceiling`] against a decoder's DUMMY `RawImage` —
/// the metadata-only probe `render::render_to_image_in` takes before it
/// decompresses a single sensor row.
///
/// The frame rule ([`default_crop`]) stays on THIS side of the wall so the
/// ceiling is charged against the same rectangle the develop will actually
/// produce, decided in the one place that owns that rule — rather than the
/// render re-deriving "which pixels are the picture" for the purpose of a
/// budget and drifting from the answer it uses for pixels.
pub(crate) fn refuse_raw_develop_over_ceiling_for(
    path: &Path,
    probe: &rawler::RawImage,
) -> Result<()> {
    let d = default_crop(probe).d;
    refuse_raw_develop_over_ceiling(path, (d.w as u64).saturating_mul(d.h as u64))
}

/// The per-file develop peak in MB for a source whose HEADER is CHEAP to read,
/// or `None` when it is not — the admission-time half of the same accounting
/// the ceilings above enforce.
///
/// **Baked**: `image`'s reader parses a header and decodes no pixel, so the
/// real per-file peak is available for the price of an `open` +
/// `into_decoder`. `jobs.rs`' "reading each RAW's dimensions to size the
/// budget would cost a decode per photo" is simply not true here, which is why
/// the planner may consult this.
///
/// **Camera RAW**: `None`. Answering would cost `RawSource::new` — the WHOLE
/// file mapped (~120 MB for a 61 MP ARW) — per photo before the pool even
/// starts, which is exactly the cost that reasoning was about. The RAW side is
/// bounded instead by [`refuse_raw_develop_over_ceiling`] at the develop door,
/// plus the corpus constant in the planner.
///
/// Best-effort by construction: an unreadable header answers `None` rather than
/// failing, because a PLAN must not be the thing that fails a run. The photo's
/// own develop will surface the real error, loudly, where the diagnosis lives.
pub fn cheap_develop_peak_mb(path: &Path) -> Option<u64> {
    if is_raw(path) {
        return None;
    }
    // baked-by-construction: the !is_raw arm, decided one line up.
    Some(baked_header_peak_bytes(path).ok()?.div_ceil(1024 * 1024))
}

/// [`develop_peak_bytes`] from a HEADER alone — no pixel decoded.
///
/// Reads through [`baked_reader`], the same raised limits the pixel path uses:
/// a probe running under the crate's 512 MB default `max_alloc` would refuse
/// to build a decoder for exactly the big exports this estimate exists for,
/// and answering "unreadable" for them would silently plan as if they were
/// small.
fn baked_header_peak_bytes(path: &Path) -> Result<u64> {
    use image::ImageDecoder as _;
    let mut decoder = baked_reader(path)?
        .into_decoder()
        .with_context(|| format!("read the header of {}", path.display()))?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let (dw, dh) = decoder.dimensions();
    Ok(develop_peak_bytes(
        decoder.total_bytes(),
        orientation,
        u64::from(dw).saturating_mul(u64::from(dh)),
    ))
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
///
/// BAKED ONLY: a camera RAW is REFUSED here (see [`load_image_gated`]) — it has
/// no `image`-crate decoder, so the honest gate is a named error, not a probe
/// failure. Callers that may hold either kind of source want the one dispatch,
/// [`crate::render::source_pixels`].
// not-a-consumer-call: the gate's own declaration.
pub fn load_image(path: &Path) -> Result<DynamicImage> {
    load_image_gated(path, false) // not-a-consumer-call: dispatch inside the gate
}

/// [`load_image`] for the full-frame baked DEVELOP path
/// (`render_baked_to_image`): charges each source pixel the develop chain's
/// downstream footprint ([`PIPELINE_BYTES_PER_PIXEL`]) on top of the decode
/// buffer, so the ceiling bounds the true pipeline peak — the plain gate
/// admitted an L8 source whose develop then peaked at ~25× the ceiling (L02).
/// Thumbnail consumers (GUI open, denoise/retouch/fit pre-shrink) stay on
/// [`load_image`]: they never build those planes, and charging them would
/// refuse sources they legitimately shrink.
// not-a-consumer-call: the gate's develop-charged twin.
pub fn load_image_for_develop(path: &Path) -> Result<DynamicImage> {
    load_image_gated(path, true) // not-a-consumer-call: dispatch inside the gate
}

/// A baked raster's reader, opened with the RAISED (not lifted) decoder limits
/// and its format already probed.
///
/// ONE construction, because there are now TWO consumers and they must agree:
/// the pixel path ([`load_image_gated`]) and the header-only peak estimate
/// ([`baked_header_peak_bytes`]). Under the crate's own `Limits::default()`
/// the TIFF codec's `set_limits` refuses to build a decoder whose frame is
/// larger than the default 512 MB `max_alloc` — so an estimate that opened its
/// own plain reader would answer "unreadable" for precisely the 60 MP-plus
/// exports it exists to size, and the caller would plan as if they were small.
/// The 65,536-px dimension bounds and the 4 GiB allocation bound below are the
/// ones that gate's doc argues for; they live here so neither consumer can
/// drift off them.
fn baked_reader(
    path: &Path,
) -> Result<image::ImageReader<std::io::BufReader<std::fs::File>>> {
    let mut reader = image::ImageReader::open(path)
        .with_context(|| format!("open image {}", path.display()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(65_536);
    limits.max_image_height = Some(65_536);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    reader
        .with_guessed_format()
        .with_context(|| format!("probe image {}", path.display()))
}

/// THE ROUTING TABLE for the refusal below, kept in the doc rather than in the
/// error text (R24 batch 2): a RAW that arrived here belongs in
/// [`crate::render::render_to_image`] or [`crate::render::source_pixels`] (the
/// one raw-vs-baked dispatch), or in [`decode_any`] when the caller wants the
/// sensor data rather than a picture.
// not-a-consumer-call: the gate's body — where a camera RAW is refused by name.
fn load_image_gated(path: &Path, develop: bool) -> Result<DynamicImage> {
    use image::ImageDecoder as _;
    // The "RAW → develop engine / baked → here" dispatch, enforced at the
    // GATE instead of trusting every caller to hand-copy an `is_raw` branch.
    // A missed branch used to reach `ImageReader` with a .ARW and surface as
    // an unrelated format/probe error (v0.22's mask-refine worker: "The image
    // format could not be determined" for a photo the app had just developed
    // on screen) — the class this refuses by name, wherever it happens.
    if is_raw(path) {
        // The sentence a USER may read. It used to end in three Rust paths
        // (`render::render_to_image / render::source_pixels`, `decode::
        // decode_any`), which is a developer's routing table shown in a
        // desktop toast and a web error body — the two surfaces this gate
        // actually reaches (the CLI routes RAWs before they arrive here). The
        // routing table now lives one screen up, in this function's own doc,
        // where the developer who needs it is already reading; what stays here
        // is the fact and the way out, in words the person holding the photo
        // can act on.
        anyhow::bail!(
            "{} is a camera RAW, and this step reads finished images \
             (PNG/TIFF/JPEG) only — a RAW has to be developed before anything \
             can read it as a picture",
            path.display()
        );
    }
    let reader = baked_reader(path)?;
    let format = reader.format();
    // R11: a camera RAW wearing a `.tif` extension reaches HERE, because
    // `is_raw` is extension-based and a DNG (or a CR2, or a NEF) really is a
    // TIFF container. The `image` crate would then decode whichever IFD it
    // finds first — on a DNG that is the small embedded THUMBNAIL, so the
    // photo would open, look right at a glance, and be developed at a few
    // hundred pixels. That is worse than a refusal, so it IS a refusal, and it
    // names the way out. Placed BEFORE `into_decoder` so the answer is this
    // sentence rather than whatever the TIFF codec makes of a sensor plane.
    // Costs one header parse on a baked TIFF open, next to nothing beside the
    // pixel decode below.
    if format == Some(image::ImageFormat::Tiff)
        && let Some(kind) = raw_in_tiff_clothing(path)
    {
        anyhow::bail!(
            "{} is named .tif but is really a camera RAW ({kind}) — rename it to its real \
             extension (e.g. .dng) so Autoshop develops the sensor instead of reading the \
             thumbnail the `image` crate would find first",
            path.display()
        );
    }
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
    let decoded_color = decoder.color_type();
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
            // The re-probe obeys apply_icc_profile's own rule (its doc):
            // unreadable colour management is a HARD error, never a silent
            // assume-sRGB fall-through — "that fall-through IS the bug".
            // The old .ok() pair folded a failed profile READ into "no
            // profile" (L05-2). Ok(None) is the real no-profile case.
            image::codecs::tiff::TiffDecoder::new(std::io::BufReader::new(
                std::fs::File::open(path)
                    .with_context(|| format!("open image {}", path.display()))?,
            ))
            .and_then(|mut d| d.icc_profile())
            .with_context(|| format!("read the ICC profile of {}", path.display()))?
        }
        None => None,
    };
    // R10: the assume-sRGB fall-through `apply_icc_profile` refuses is only
    // refusable when a profile EXISTS. An UNTAGGED file has nothing to refuse,
    // and is read as sRGB — right for essentially every 8-bit JPEG (the web's
    // default and what phones write), and a real risk for 16-bit, which is
    // what an editor produces: Lightroom's "Edit in…" hands over ProPhoto,
    // Photoshop happily saves untagged AdobeRGB. So the disclosure is aimed at
    // exactly that population instead of warning on every ordinary snapshot —
    // a warning that fires on the common correct case is one nobody reads.
    if icc_profile.is_none()
        && matches!(
            decoded_color,
            image::ColorType::Rgb16 | image::ColorType::Rgba16 | image::ColorType::La16
                | image::ColorType::L16
        )
    {
        eprintln!(
            "⚠ {} is 16-bit but carries no ICC profile, so its pixels are being read as sRGB. \
             If it was exported as ProPhoto or Adobe RGB (Lightroom's \"Edit in…\" default is \
             ProPhoto), every tone and colour decision below is computed on the wrong numbers — \
             re-export it with the profile embedded",
            path.display()
        );
    }
    let mut img = DynamicImage::from_decoder(decoder)
        .with_context(|| format!("decode image {}", path.display()))?;
    if let Some(profile) = icc_profile {
        apply_icc_profile(&mut img, &profile, path)?;
    }
    img.apply_orientation(orientation);
    Ok(img)
}

/// Is this TIFF-container file actually a camera RAW (R11)? Names the marker
/// that gave it away, or `None` for an ordinary baked TIFF.
///
/// Header-only, and deliberately conservative: it looks for the two tags that
/// no photo editor writes into a delivery TIFF — `DNGVersion` (0xC612), which
/// only a DNG carries, and `SubIFDs` combined with a `Make` string, which is
/// the shape every TIFF-based RAW (CR2/NEF/ARW/ORF/PEF/SRW/…) uses to hang the
/// sensor plane off the root IFD. A file that will not even parse as TIFF is
/// not our problem here — the decoder below reports it.
fn raw_in_tiff_clothing(path: &Path) -> Option<&'static str> {
    /// `DNGVersion`, the tag that DEFINES a DNG (rawler dispatches on it —
    /// `decoders/mod.rs:919-921`).
    const DNG_VERSION: u16 = 0xC612;
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let tiff = GenericTiffReader::new(&mut reader, 0, 0, Some(16), &[]).ok()?;
    // Same panic-shaped `root_ifd()` as `baked_exif`; a metadata probe must
    // never be the thing that ends the process.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let root = tiff.root_ifd();
        if root.get_entry(DNG_VERSION).is_some() {
            return Some("it carries the DNGVersion tag");
        }
        let has_make = root
            .get_entry(TiffCommonTag::Make)
            .and_then(|e| e.value.as_string().cloned())
            .is_some_and(|s| !s.trim().is_empty());
        if has_make && root.get_sub_ifd(TiffCommonTag::SubIFDs).is_some() {
            return Some("it names a camera and hangs its sensor data off a SubIFD");
        }
        None
    }))
    .ok()
    .flatten()
}

/// Decode any supported source — a camera RAW or an already-baked image. The
/// baked path (the "PNG source" mode: edit an LR/PS-denoised export) has no
/// sensor metadata, so [`Meta`] is filled with neutral defaults and the
/// histogram is computed from the pixels.
pub fn decode_any(path: &Path) -> Result<Decoded> {
    decode_any_turned(path, 0)
}

/// [`decode_any`] in the frame the photographer's quarter turns produce — the
/// dispatch twin of [`decode_raw_turned`]. The baked arm turns the decoded
/// pixels directly: `load_image` has already applied their EXIF orientation,
/// so only the user's half is left, and `Meta`'s dims are re-read from the
/// turned image rather than swapped by hand.
pub fn decode_any_turned(path: &Path, quarter_turns: u8) -> Result<Decoded> {
    if is_raw(path) {
        return decode_raw_turned(path, quarter_turns);
    }
    let mut d = decode_baked(path)?;
    match crate::render::quarter_turn_orientation(quarter_turns) {
        rawler::Orientation::Normal | rawler::Orientation::Unknown => {}
        o => {
            d.preview = crate::render::oriented(d.preview, o);
            d.meta.width = d.preview.width() as usize;
            d.meta.height = d.preview.height() as usize;
        }
    }
    Ok(d)
}

/// The seven capture facts [`Meta`] carries that come from EXIF rather than
/// from the sensor — ONE extraction shared by the RAW arm and the baked arm
/// (P2). Splitting them was the old bug in miniature: the RAW side grew an
/// APEX `ApertureValue` fallback and two finiteness filters that a
/// hand-written baked copy would not have had, so the same JPEG would have
/// reported a different f-number depending on which door it came through.
struct ExifFacts {
    lens: Option<String>,
    iso: Option<u32>,
    shutter: Option<String>,
    aperture: Option<f32>,
    focal_length_mm: Option<f32>,
    exposure_bias_ev: Option<f32>,
    date_time: Option<String>,
}

fn exif_facts(exif: &rawler::exif::Exif) -> ExifFacts {
    ExifFacts {
        lens: exif.lens_model.clone().or_else(|| exif.lens_make.clone()),
        iso: exif.iso_speed_ratings.map(u32::from).or(exif.iso_speed),
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
    }
}

/// Largest EXIF block this reader will take from a baked file. A JPEG APP1
/// segment cannot exceed 65533 payload bytes by construction (the length field
/// is 16-bit); a TIFF's header chain is walked, not buffered. 1 MiB is
/// therefore pure belt-and-braces against a hand-built file, and it is a
/// REFUSAL rather than a truncation — half an IFD parses to different numbers,
/// which is the failure mode this whole module refuses everywhere else.
const MAX_BAKED_EXIF: usize = 1024 * 1024;

/// How far into a JPEG this reader will hunt for the EXIF segment before
/// giving up. EXIF is required to be the FIRST APP segment, so anything past
/// a few hundred KB of headers is not a file that follows the spec; the cap
/// stops a crafted file from turning a metadata read into a whole-file scan.
const MAX_JPEG_HEADER_SCAN: u64 = 4 * 1024 * 1024;

/// The TIFF/EXIF block of a JPEG: the payload of the first `APP1` segment
/// whose six leading bytes are `Exif\0\0`, with that introducer stripped so
/// what comes back starts at the TIFF header (`II`/`MM`) rawler's reader
/// expects.
///
/// Markers are walked, not searched for: scanning the file for the byte pair
/// would hit `FFE1` inside compressed scan data. `D0..=D7` (restart), `01` and
/// `D8` are the standalone markers that carry no length; `DA` (start of scan)
/// ends the header region — everything after it is entropy-coded.
/// Read-only by construction — no `Seek`. Skipping a segment copies its bytes
/// to a sink rather than seeking past them, so the walk cannot depend on how
/// `Take`/`BufReader` compose their cursors; the cost is reading header bytes
/// that were about to be read anyway, bounded by [`MAX_JPEG_HEADER_SCAN`].
fn jpeg_exif_block(file: &mut std::fs::File) -> Result<Option<Vec<u8>>> {
    use std::io::Read as _;
    let mut r = std::io::BufReader::new(file.by_ref().take(MAX_JPEG_HEADER_SCAN));
    let mut soi = [0u8; 2];
    if r.read_exact(&mut soi).is_err() || soi != [0xFF, 0xD8] {
        return Ok(None);
    }
    loop {
        // Marker prefixes may be padded with any number of 0xFF fill bytes.
        let mut b = [0u8; 1];
        if r.read_exact(&mut b).is_err() {
            return Ok(None);
        }
        if b[0] != 0xFF {
            return Ok(None); // desynchronised — not our business to repair
        }
        while b[0] == 0xFF {
            if r.read_exact(&mut b).is_err() {
                return Ok(None);
            }
        }
        let marker = b[0];
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue; // standalone, no length field
        }
        if marker == 0xDA || marker == 0xD9 {
            return Ok(None); // scan data / end of image — no EXIF in this file
        }
        let mut len = [0u8; 2];
        if r.read_exact(&mut len).is_err() {
            return Ok(None);
        }
        let payload = u64::from(u16::from_be_bytes(len)).saturating_sub(2);
        if marker != 0xE1 {
            // A short copy (truncated file) is not an error here — the next
            // read_exact fails and the walk ends with "no EXIF", which is the
            // honest answer for a file that stops mid-header.
            if std::io::copy(&mut r.by_ref().take(payload), &mut std::io::sink()).is_err() {
                return Ok(None);
            }
            continue;
        }
        if payload > MAX_BAKED_EXIF as u64 {
            anyhow::bail!("the EXIF segment is larger than the {MAX_BAKED_EXIF}-byte limit");
        }
        let mut buf = vec![0u8; payload as usize];
        if r.read_exact(&mut buf).is_err() {
            return Ok(None);
        }
        // An APP1 that is not EXIF is almost always the XMP packet
        // (`http://ns.adobe.com/xap/1.0/`), which is not ours to read here.
        if buf.starts_with(b"Exif\0\0") {
            return Ok(Some(buf.split_off(6)));
        }
    }
}

/// The XMP packet of a JPEG: the `http://ns.adobe.com/xap/1.0/\0` APP1
/// segment, with any **ExtendedXMP** continuation chunks reassembled onto it.
///
/// **Why the second half is not optional** (R27, `P5-cropped-mask-frame.md`
/// §8). A JPEG APP1 segment holds at most 65533 payload bytes, and a Lightroom
/// develop block with masks routinely exceeds that: **13 exports in the user's
/// own library** are split this way, including `DSC09024_1.jpg` — one of the
/// seven photographs P3's crop model rests on. The continuation lives in
/// further APP1 segments introduced by `http://ns.adobe.com/xmp/extension/\0`,
/// each carrying the 32-hex GUID of the extension it belongs to, the total
/// length, and its own byte offset. A reader that simply concatenates
/// `<x:xmpmeta>…</x:xmpmeta>` out of the raw bytes bridges the segment headers
/// and produces XML that is not well-formed; a reader that takes only the
/// standard segment gets a truncated document. Both were observed on this
/// library, in P5's own first pass.
///
/// The chunks are placed BY OFFSET, not by arrival order, and a chain with a
/// hole in it is `Err` rather than a quietly short document — the same rule the
/// caller applies to a packet it cannot decode. Only the GUID the standard
/// packet names (`xmpNote:HasExtendedXMP`) is accepted, so a second edit
/// generation's leftover chunks cannot splice themselves into this one.
///
/// The standard packet comes back FIRST and whole: `crs:` settings live in it,
/// and the extension carries the overflow (usually the AI-mask rasters).
fn jpeg_xmp_packet(file: &mut std::fs::File) -> Result<Option<Vec<u8>>> {
    use std::io::Read as _;
    const STD: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
    const EXT: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
    /// GUID (32) + total length (4) + offset (4).
    const EXT_HEADER: usize = 40;
    let mut r = std::io::BufReader::new(file.by_ref().take(MAX_JPEG_HEADER_SCAN));
    let mut soi = [0u8; 2];
    if r.read_exact(&mut soi).is_err() || soi != [0xFF, 0xD8] {
        return Ok(None);
    }
    let mut standard: Option<Vec<u8>> = None;
    let mut chunks: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut total: Option<u32> = None;
    loop {
        let mut b = [0u8; 1];
        if r.read_exact(&mut b).is_err() {
            break;
        }
        if b[0] != 0xFF {
            break; // desynchronised — not our business to repair
        }
        while b[0] == 0xFF {
            if r.read_exact(&mut b).is_err() {
                return Ok(standard);
            }
        }
        let marker = b[0];
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue; // standalone, no length field
        }
        if marker == 0xDA || marker == 0xD9 {
            break; // scan data / end of image — the header region is over
        }
        let mut len = [0u8; 2];
        if r.read_exact(&mut len).is_err() {
            break;
        }
        let payload = u64::from(u16::from_be_bytes(len)).saturating_sub(2);
        if marker != 0xE1 {
            if std::io::copy(&mut r.by_ref().take(payload), &mut std::io::sink()).is_err() {
                break;
            }
            continue;
        }
        let mut buf = vec![0u8; payload as usize];
        if r.read_exact(&mut buf).is_err() {
            break;
        }
        if buf.starts_with(STD) {
            standard.get_or_insert_with(|| buf[STD.len()..].to_vec());
        } else if buf.starts_with(EXT) && buf.len() >= EXT.len() + EXT_HEADER {
            let head = &buf[EXT.len()..EXT.len() + EXT_HEADER];
            let guid = head[..32].to_ascii_uppercase();
            // The standard packet NAMES the extension it owns; anything else
            // belongs to some other generation of this file.
            let owned = standard.as_deref().is_some_and(|s| {
                twoway_contains(s, &guid)
            });
            if !owned {
                continue;
            }
            let len = u32::from_be_bytes([head[32], head[33], head[34], head[35]]);
            let off = u32::from_be_bytes([head[36], head[37], head[38], head[39]]);
            if total.is_some_and(|t| t != len) {
                anyhow::bail!("its ExtendedXMP chunks disagree about the total length");
            }
            total = Some(len);
            chunks.push((off, buf[EXT.len() + EXT_HEADER..].to_vec()));
        }
    }
    let Some(std_packet) = standard else { return Ok(None) };
    if chunks.is_empty() {
        return Ok(Some(std_packet));
    }
    chunks.sort_by_key(|(off, _)| *off);
    let mut ext = Vec::new();
    for (off, body) in chunks {
        if off as usize != ext.len() {
            anyhow::bail!(
                "its ExtendedXMP chain has a gap at byte {off} (had {} bytes)",
                ext.len()
            );
        }
        ext.extend_from_slice(&body);
    }
    if total.is_some_and(|t| t as usize != ext.len()) {
        anyhow::bail!("its ExtendedXMP chain is short of the length it declares");
    }
    // Two packets, one after the other: each is a complete `<x:xmpmeta>`
    // document, and every reader in this crate scans for the settings rather
    // than assuming one root (the same shape a RAW's own multi-packet files
    // have — `P3-cropangle-model.md` §3.1 found a develop block in the SECOND
    // packet of a real export).
    let mut out = std_packet;
    out.push(b'\n');
    out.extend_from_slice(&ext);
    Ok(Some(out))
}

/// `haystack.contains(needle)` for bytes, without pulling in a dependency.
fn twoway_contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len()
        && haystack.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

/// Capture metadata for an already-baked raster (P2). Returns the maker, the
/// model and rawler's own parsed [`rawler::exif::Exif`] — the SAME struct the
/// RAW arm reads, so [`exif_facts`] serves both.
///
/// No new dependency, and no hand-rolled tag decoding: rawler's
/// [`GenericTiffReader`] already parses a TIFF header chain (this module uses
/// it for `embedded_xmp` and `lensmeta` uses it for the Sony spline tags), and
/// `Exif::new` already walks the root IFD plus the `ExifIFD` sub-IFD that
/// `wellknown_sub_ifd_tags` parses by default (tag 0x8769, spelled
/// `ExifIFDPointer` there and `ExifOffset` in the EXIF table — verified the
/// same number in `rawler/src/tags.rs:100` and `:369`). A JPEG needs one extra
/// step, [`jpeg_exif_block`], because its TIFF block is wrapped in an APP1
/// segment; a TIFF IS the container already.
///
/// PNG is deliberately NOT read: its EXIF lives in an `eXIf` chunk that needs
/// its own chunk walk, and the format's own metadata story (`tEXt`) is not
/// EXIF at all. A PNG import keeps the neutral stub, exactly as before.
///
/// Absent metadata is `Ok(None)`, not an error — an untagged export is the
/// normal case, not a defect.
fn baked_exif(path: &Path) -> Result<Option<(String, String, rawler::exif::Exif)>> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open image {}", path.display()))?;
    let mut magic = [0u8; 2];
    if file.read_exact(&mut magic).is_err() {
        return Ok(None);
    }
    use std::io::Seek as _;
    // Both arms re-read from byte 0: the magic probe above consumed two
    // bytes, and both readers below expect to start at the file header (the
    // JPEG walker re-reads SOI itself, the TIFF reader wants `II`/`MM`).
    file.rewind().with_context(|| format!("rewind {}", path.display()))?;
    let tiff = if magic == [0xFF, 0xD8] {
        // JPEG: the TIFF block rides inside APP1.
        let Some(block) = jpeg_exif_block(&mut file)
            .with_context(|| format!("read the EXIF segment of {}", path.display()))?
        else {
            return Ok(None);
        };
        GenericTiffReader::new_with_buffer(&block, 0, 0, Some(16)).ok()
    } else if magic == *b"II" || magic == *b"MM" {
        let mut reader = std::io::BufReader::new(&mut file);
        // Same chain cap as `embedded_xmp` and `lensmeta::read` — a header
        // parse, no pixel decode.
        GenericTiffReader::new(&mut reader, 0, 0, Some(16), &[]).ok()
    } else {
        return Ok(None); // PNG and anything else: no EXIF route (see the doc)
    };
    let Some(tiff) = tiff else { return Ok(None) };
    // `root_ifd()` PANICS on an empty chain (rawler's own `unwrap`-shaped
    // guard, `formats/tiff/reader.rs:36-38`) and `Exif::new` walks
    // third-party-parsed IFDs — both are exactly what `guard_parser_panic`
    // exists for, and metadata is never worth aborting a batch over.
    guard_parser_panic(path, "baked EXIF", || {
        let root = tiff.root_ifd();
        let text = |t: TiffCommonTag| {
            root.get_entry(t)
                .and_then(|e| e.value.as_string().cloned())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let exif = rawler::exif::Exif::new(root)
            .map_err(|e| anyhow!("parse the EXIF of {}: {e}", path.display()))?;
        Ok(Some((
            text(TiffCommonTag::Make).unwrap_or_default(),
            text(TiffCommonTag::Model).unwrap_or_default(),
            exif,
        )))
    })
}

fn decode_baked(path: &Path) -> Result<Decoded> {
    // baked-by-construction: decode_any sends a RAW to decode_raw; this is the baked arm.
    let preview = load_image(path)?;
    let (w, h) = preview.dimensions();
    // P2: a phone or Lightroom JPEG carries ISO, shutter, aperture, focal
    // length and capture time in plain EXIF, and this arm used to throw all of
    // it away and hand the AI advisor `model: "imported image"` and nothing
    // else. A read FAILURE is disclosed and degrades to the old stub — the
    // pixels are fine and refusing the photo over its metadata would be the
    // wrong trade — but it is never folded into silence.
    let facts = match baked_exif(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("⚠ {e:#}");
            None
        }
    };
    let (make, model, exif) = match facts {
        Some((make, model, exif)) => {
            let f = exif_facts(&exif);
            // "imported image" stays the answer for a file that names no
            // camera: it is what every downstream consumer already reads as
            // "no body", and an empty model string would print as a blank.
            (make, if model.is_empty() { "imported image".to_string() } else { model }, Some(f))
        }
        None => (String::new(), "imported image".to_string(), None),
    };
    let meta = Meta {
        make,
        model,
        lens: exif.as_ref().and_then(|f| f.lens.clone()),
        iso: exif.as_ref().and_then(|f| f.iso),
        shutter: exif.as_ref().and_then(|f| f.shutter.clone()),
        aperture: exif.as_ref().and_then(|f| f.aperture),
        focal_length_mm: exif.as_ref().and_then(|f| f.focal_length_mm),
        exposure_bias_ev: exif.as_ref().and_then(|f| f.exposure_bias_ev),
        date_time: exif.as_ref().and_then(|f| f.date_time.clone()),
        // DISPLAY-frame dims: `load_image` has already applied the EXIF
        // orientation, so these describe the pixels actually delivered.
        width: w as usize,
        height: h as usize,
        // A baked raster is not camera-referred — the develop anchors on the
        // recipe's 5500 K instead (see `render::render_baked_to_image`).
        as_shot_wb_coeffs: [1.0, 1.0, 1.0, 1.0],
    };
    let histogram = compute_histogram(&hist_copy(&preview));
    Ok(Decoded { preview, meta, histogram, embedded_xmp: None })
}

/// The RAW's EXIF orientation (IFD0 tag 0x0112) — the ONE source of truth for
/// which way is up, and the only value any consumer may read.
///
/// **Not** `RawImage.orientation`: rawler 0.7.2 hard-codes that field to
/// `Orientation::Normal` in both `RawImage` constructors
/// (`rawimage.rs:389` and `rawimage.rs:478`, verbatim
/// `orientation: Orientation::Normal, //cam.orientation, // TODO fixme`), so
/// every decoder except DNG and QTK — ARW included — reported "Normal" for a
/// portrait frame and the whole chain rendered it sideways. The real value
/// rides in `RawMetadata.exif`, which rawler DOES populate from tag 0x0112
/// (`exif.rs:16` `pub orientation: Option<u16>`, filled at `exif.rs:124`);
/// the A7RIV portrait samples read `Some(8)` = `Rotate270`.
///
/// An absent tag answers `Normal`. rawler's own `Orientation::from_tiff`
/// answers `Unknown` for that case, but the two are indistinguishable to
/// every consumer: `to_flips()` and [`crate::render::oriented`] treat
/// `Unknown` exactly as `Normal`. That equivalence is asserted, not assumed —
/// see `unknown_and_normal_are_the_same_no_op`.
pub fn raw_orientation_of(md: &rawler::decoders::RawMetadata) -> rawler::Orientation {
    md.exif
        .orientation
        .map(rawler::Orientation::from_u16)
        .unwrap_or(rawler::Orientation::Normal)
}

/// [`raw_orientation_of`] for a path we have not decoded yet: metadata only,
/// no sensor decompression and no preview extraction. Errors only when the
/// file cannot be opened or has no decoder — a RAW without the tag is
/// `Normal`, not a failure.
pub fn raw_orientation(path: &Path) -> Result<rawler::Orientation> {
    guard_tiff_chain(path)?;
    let src = RawSource::new(path).with_context(|| format!("open RAW {}", path.display()))?;
    let decoder = decoder_for(path, &src)?;
    let md = guard_parser_panic(path, "raw_metadata", || {
        decoder
            .raw_metadata(&src, &RawDecodeParams { image_index: 0 })
            .map_err(|e| anyhow!("raw_metadata: {e}"))
    })?;
    Ok(raw_orientation_of(&md))
}

/// The rectangle of the sensor that IS the picture — the DNG
/// `DefaultCropOrigin` / `DefaultCropSize` pair, which is what the camera, the
/// DNG spec and Lightroom all call the frame. Falls back to the active area
/// and then to the whole sensor for a RAW that declares neither.
fn default_crop(raw: &rawler::RawImage) -> rawler::imgop::Rect {
    raw.crop_area.or(raw.active_area).unwrap_or_else(|| {
        rawler::imgop::Rect::new(
            rawler::imgop::Point::new(0, 0),
            rawler::imgop::Dim2::new(raw.width, raw.height),
        )
    })
}

/// Move a decoded `RawImage`'s develop window onto that rectangle. Returns
/// [`CropAlignment`] — which of the three outcomes happened — and DISCLOSES
/// the refusal on stderr (A5).
///
/// **The defect this closes.** Block registration of eight Autoshop renders
/// against their Lightroom exports put every one of them
/// `(+31 ± 6, +20 ± 1)` full-resolution pixels off, a pure translation with no
/// scale component (`PROBE2-VERDICT.md` §9, re-confirmed at `(+34.4, +19.9)`
/// in `PROBE4-FINAL.md` §3). The ARWs carry
/// `DefaultCropOrigin = (32, 20)`, `DefaultCropSize = (9504, 6336)` inside a
/// `9600 × 6376` raw frame: Autoshop emitted the right SIZE from the wrong
/// ORIGIN, so recipe coordinates and Lightroom coordinates disagreed by 0.34 %
/// of the width at the edges — and every mask this batch teaches the importer
/// to place correctly would have landed 32 px right of where Lightroom draws
/// it.
///
/// **Why rawler does not do it.** Two facts in the dependency compose, and
/// neither is wrong on its own (rawler 0.7.2, read at
/// `~/.cargo/registry/src/…/rawler-0.7.2/`):
///   * `decoders/arw.rs:707-713` builds `active_area` from the
///     `SonyRawImageSize` tag as `Rect::new(Point::default(), …)` — the tag
///     carries a SIZE, so the origin is pinned at `(0, 0)`;
///   * `imgop/develop.rs:216` applies the default crop only
///     `if crop.d != intermediate.dim()`, and the demosaic ROI has already
///     taken `active_area.d` pixels, so a default crop that is a pure
///     TRANSLATION has exactly the same size and is skipped.
///
/// **The fix, and why it is this one.** Moving the demosaic ROI (i.e.
/// `active_area`) to the default-crop origin costs nothing — the developed
/// buffer is the same size, just read from the right place — where cropping
/// afterwards would pay a second full-frame copy (~720 MB at 61 MP). rawler
/// shifts the CFA pattern by the ROI origin itself
/// (`imgop/sensor/bayer/ppg.rs:50`, `cfa.shift(roi.p.x, roi.p.y)`), so the
/// Bayer phase follows; `apply_scaling` has already run over the whole frame
/// in raw coordinates and is untouched by this.
///
/// Deliberately NARROW: it fires only when the two rectangles are the same
/// SIZE and differ in origin, which is exactly the case rawler skips. When
/// they differ in size, rawler's own `CropDefault` step does the right thing
/// and this leaves it alone. A rectangle that would run off the sensor is
/// refused rather than clamped — a RAW whose tags disagree with its own
/// dimensions is not a frame to guess at.
pub fn align_default_crop(raw: &mut rawler::RawImage) -> CropAlignment {
    let verdict = aligned_demosaic_roi(raw.crop_area, raw.active_area, raw.width, raw.height);
    match verdict {
        CropAlignment::Moved(moved) => raw.active_area = Some(moved),
        // A5: the refusal used to be an indistinguishable `None`, discarded at
        // the call site — so a non-Sony file with inconsistent tags rendered
        // from the un-aligned window and said NOTHING, leaving any later
        // diagnosis with zero telemetry. It is now a sentence on the same
        // stderr channel every other render disclosure uses
        // (`ValidatedRecipe::disclose`, the embedded-XMP read warning).
        CropAlignment::OffSensor { crop, width, height } => eprintln!(
            "⚠ this RAW's DefaultCrop rectangle ({}×{} at {},{}) runs off its own {width}×{height} \
             sensor — developing from the un-aligned window instead. The frame is still the size \
             the camera declares; its ORIGIN may differ from Lightroom's by the tag disagreement",
            crop.d.w, crop.d.h, crop.p.x, crop.p.y
        ),
        CropAlignment::NothingToMove => {}
    }
    verdict
}

/// What [`align_default_crop`] decided. Three outcomes, not two: "moved" and
/// "nothing to move" are both normal, and "the tags run off the sensor" is a
/// fact about the FILE that a photographer chasing a misplaced mask needs to
/// hear (A5). Carried as a value as well as printed so the `AUTOSHOP_RAW_ZOO`
/// probe can record a verdict per make without scraping stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropAlignment {
    /// The demosaic window moved onto the default-crop origin.
    Moved(rawler::imgop::Rect),
    /// Sizes differ (rawler's own `CropDefault` handles it), origins already
    /// agree, or the RAW declares no default crop. Every non-Sony make in the
    /// zoo lands here — see the table in
    /// `the_demosaic_window_moves_onto_the_default_crop_rectangle`.
    NothingToMove,
    /// The rectangle would index past the sensor buffer. Refused, never
    /// clamped — a RAW whose tags disagree with its own dimensions is not a
    /// frame to guess at.
    OffSensor { crop: rawler::imgop::Rect, width: usize, height: usize },
}

impl CropAlignment {
    /// The origin the window moved to, for the callers that only wanted that.
    pub fn moved_to(self) -> Option<(usize, usize)> {
        match self {
            CropAlignment::Moved(r) => Some((r.p.x, r.p.y)),
            _ => None,
        }
    }
}

/// The decision half of [`align_default_crop`], as a function of the numbers
/// alone — a `RawImage` needs a whole `Camera` and a sensor buffer to build,
/// and the thing worth pinning is which rectangles move and which do not.
fn aligned_demosaic_roi(
    crop: Option<rawler::imgop::Rect>,
    active: Option<rawler::imgop::Rect>,
    width: usize,
    height: usize,
) -> CropAlignment {
    let (Some(crop), Some(active)) = (crop, active) else {
        return CropAlignment::NothingToMove;
    };
    if crop.d != active.d || crop.p == active.p {
        return CropAlignment::NothingToMove;
    }
    if crop.p.x + crop.d.w > width || crop.p.y + crop.d.h > height {
        return CropAlignment::OffSensor { crop, width, height };
    }
    CropAlignment::Moved(crop)
}

/// The pixel frame a develop of `path` lands in — the frame every normalised
/// coordinate in a recipe, and in a Lightroom sidecar, is measured against.
///
/// For a RAW that is [`default_crop`]'s rectangle, turned into the DISPLAY
/// frame by the EXIF orientation, which is `render::render_to_image`'s own
/// order (orientation first, everything else after). For a baked image it is
/// the oriented header dimensions — `load_image` applies the orientation the
/// same way.
///
/// Metadata only: `dummy = true` on the RAW arm means no sensor
/// decompression, and the baked arm decodes no pixel at all.
pub fn frame_size(path: &Path) -> Result<(usize, usize)> {
    frame_size_turned(path, 0)
}

/// [`frame_size`] in the frame the photographer's own quarter turns produce.
pub fn frame_size_turned(path: &Path, quarter_turns: u8) -> Result<(usize, usize)> {
    let ((w, h), exif) = source_frame(path)?;
    let orientation = crate::render::compose_orientation(exif, quarter_turns);
    Ok(if orientation_transposes(orientation) { (h, w) } else { (w, h) })
}

/// The frame the FILE STORES, un-turned, together with the turn its own
/// metadata asks for — the two halves [`frame_size_turned`] folds together,
/// and the pair `xmp::FrameAspect` needs whole (R27 A7/A8).
///
/// For a RAW that is [`default_crop`]'s rectangle in SENSOR orientation (the
/// DNG `DefaultCropSize`, long axis left→right on an A7R IV whichever way the
/// camera was held); for a baked image it is the header's own dimensions
/// before its EXIF orientation is applied. **This — not the display frame — is
/// what every normalised `crs:` coordinate is measured against**
/// (`P1-portrait-mask-frame.md` §1 for the mask block,
/// `P3-cropangle-model.md` §2 for the crop rectangle, both HIGH), and it is
/// what a Lightroom sidecar's `tiff:ImageWidth/ImageLength` declare (F3's
/// public census, 72/72).
///
/// ONE metadata read for both answers: the caller that needs the aspect needs
/// the orientation too, and asking twice paid for the same header walk twice.
///
/// Metadata only: `dummy = true` on the RAW arm means no sensor
/// decompression, and the baked arm decodes no pixel at all.
pub fn source_frame(path: &Path) -> Result<((usize, usize), rawler::Orientation)> {
    if !is_raw(path) {
        use image::ImageDecoder as _;
        use image::metadata::Orientation as ImgO;
        use rawler::Orientation as O;
        // baked-by-construction: the `is_raw` gate above is this function's
        // own dispatch, the one `load_image_gated` enforces for pixels.
        let reader = image::ImageReader::open(path)
            .with_context(|| format!("open image {}", path.display()))?
            .with_guessed_format()
            .with_context(|| format!("probe image {}", path.display()))?;
        let mut decoder = reader
            .into_decoder()
            .with_context(|| format!("read the header of {}", path.display()))?;
        // `image`'s eight states ARE the EXIF eight, named after the motion
        // that CORRECTS the file rather than after the tag number; the pairing
        // below is that table (`Rotate90` = tag 6, `Rotate270` = tag 8), which
        // is exactly how `rawler::Orientation::from_u16` spells them too.
        let exif = match decoder.orientation().unwrap_or(ImgO::NoTransforms) {
            ImgO::NoTransforms => O::Normal,
            ImgO::FlipHorizontal => O::HorizontalFlip,
            ImgO::Rotate180 => O::Rotate180,
            ImgO::FlipVertical => O::VerticalFlip,
            ImgO::Rotate90FlipH => O::Transpose,
            ImgO::Rotate90 => O::Rotate90,
            ImgO::Rotate270FlipH => O::Transverse,
            ImgO::Rotate270 => O::Rotate270,
        };
        let (w, h) = decoder.dimensions();
        return Ok(((w as usize, h as usize), exif));
    }
    guard_tiff_chain(path)?;
    let src = RawSource::new(path).with_context(|| format!("open RAW {}", path.display()))?;
    let decoder = decoder_for(path, &src)?;
    let params = RawDecodeParams { image_index: 0 };
    let (md, raw) = guard_parser_panic(path, "frame size", || {
        let md = decoder
            .raw_metadata(&src, &params)
            .map_err(|e| anyhow!("raw_metadata: {e}"))?;
        let raw = decoder
            .raw_image(&src, &params, true)
            .map_err(|e| anyhow!("raw_image(dummy): {e}"))?;
        Ok((md, raw))
    })?;
    let d = default_crop(&raw).d;
    Ok(((d.w, d.h), raw_orientation_of(&md)))
}

/// Does this orientation swap width and height in the display frame?
/// `pub(crate)`: the render side's quarter-turn contract test asserts that
/// THIS predicate and `render::oriented`'s actual output agree for all eight
/// states — two modules, one answer, checked rather than assumed.
pub(crate) fn orientation_transposes(o: rawler::Orientation) -> bool {
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

/// The one sentence that turns "your camera is not supported" into something a
/// photographer can DO (A8, the DNG on-ramp). Every body Autoshop cannot read
/// natively has this route, and it is not a downgrade: rawler's DNG decoder
/// builds its whole `Camera` — colour matrices included — from the file's own
/// tags (`decoders/dng.rs:270-289`), so a converted file needs no entry in the
/// 725-model camera database at all. Kept as ONE constant so the CLI, the GUI
/// toast and the web error body cannot drift into three different offers.
pub const DNG_ONRAMP: &str = "Adobe DNG Converter (free, from Adobe) rewrites it as a .dng, \
                              which Autoshop develops from the file's own colour tags — that \
                              route works for any body";

/// Turn rawler's `get_decoder` refusal into a sentence that names WHICH of the
/// three quite different failures happened (A7). They used to arrive as one
/// opaque `no rawler decoder for {path}: {e}` line, and the three want three
/// different actions from the user:
///
///   * **unknown make** — the file IS a TIFF-shaped RAW, but its `Make` string
///     matches none of rawler's table (`decoders/mod.rs:963-971`). Usually a
///     body newer than the decoder crate.
///   * **unknown model** — the make is known and the container parsed, but the
///     exact model is missing from the 725-file camera database
///     (`check_supported_with_everything`, `mod.rs:1003-1009`). This is the
///     common case for a recent body, and the one the DNG on-ramp fixes best.
///   * **no decoder at all** — nothing matched: not a RAW, or a container this
///     rawler does not know. A `.tif`-named RAW lands here too (see
///     [`load_image_gated`]'s DNG-in-TIFF refusal for the reverse direction).
///
/// The path is always named because a batch scan reports many files at once.
fn describe_decoder_failure(path: &Path, e: &rawler::RawlerError) -> anyhow::Error {
    let rawler::RawlerError::Unsupported { what, make, model, .. } = e else {
        // DecoderFailed — the container was recognised and then would not
        // read. That is a file-integrity story, not a camera-support one, so
        // it keeps rawler's own words and gains no on-ramp advice.
        return anyhow!("{} could not be decoded: {e}", path.display());
    };
    let (make, model) = (make.trim(), model.trim());
    if !make.is_empty() && !model.is_empty() {
        anyhow!(
            "{} is a {make} {model}, and this build's RAW decoder has no calibration for that \
             exact model (it carries 725). {DNG_ONRAMP}",
            path.display()
        )
    } else if !make.is_empty() {
        anyhow!(
            "{} names its maker as \"{make}\", which this build's RAW decoder does not know at \
             all — usually a body newer than the decoder. {DNG_ONRAMP}",
            path.display()
        )
    } else {
        anyhow!(
            "{} does not read as any RAW format this build knows ({what}) — if it really is a \
             camera RAW, {DNG_ONRAMP}",
            path.display()
        )
    }
}

/// [`get_decoder`] with [`describe_decoder_failure`]'s wording — the ONE place
/// a decoder refusal is turned into a sentence, so `decode_raw`, the preview
/// path, `frame_size`, `raw_orientation` and the full-res render cannot answer
/// the same file three different ways (they did: "no rawler decoder for …",
/// "no decoder for …", and rawler's raw Display).
pub(crate) fn decoder_for<'a>(
    path: &Path,
    src: &'a RawSource,
) -> Result<Box<dyn rawler::decoders::Decoder + 'a>> {
    get_decoder(src).map_err(|e| describe_decoder_failure(path, &e))
}

/// Run a THIRD-PARTY parser call so that a panic inside it becomes a named
/// error instead of an aborted process.
///
/// The GUI has had this since v0.22 — every worker body runs inside
/// `catch_unwind` (`src/bin/gui/workers.rs:29-38`) — and the CLI had nothing:
/// the same malformed file that showed a toast in the app killed
/// `autoshop batch` mid-library, losing the run. Widening [`RAW_EXTS`] to 24
/// formats widens the surface exactly where the parsers are least exercised,
/// so the guard goes in at the same time as the extensions.
///
/// `AssertUnwindSafe` is correct and not a shortcut here: the closure borrows
/// a `RawSource` and a decoder that are DROPPED on the error path — nothing
/// observes a half-updated value after a caught panic, because nothing
/// survives the call. What crosses the boundary is the returned `T` alone.
///
/// This does NOT make a panicking decoder supported (see [`RAW_EXTS`] on
/// `.x3f`); it makes one file's defect one file's error.
pub(crate) fn guard_parser_panic<T>(
    path: &Path,
    what: &str,
    call: impl FnOnce() -> Result<T>,
) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(anyhow!(
                "the RAW parser panicked reading {} ({what}: {msg}) — that is a defect in the \
                 third-party decoder for this format, not something wrong with the photograph. \
                 {DNG_ONRAMP}",
                path.display()
            ))
        }
    }
}

/// The three-level embedded-rendition fallback: a level that ERRORS is a
/// level that cannot answer — the chain's whole purpose is "this one can't,
/// try the next" (L05-4/5: a corrupt mid-size preview used to abort files
/// whose thumbnail or full-size JPEG was intact). Lazy, so a cheaper hit
/// skips the larger decodes; the terminal failure names every reason.
type RenditionLevel<'a, T> = (&'static str, &'a mut dyn FnMut() -> Result<Option<T>>);

/// [`first_usable_rendition`], with the two "no" answers kept APART (A4).
///
/// `Ok(None)` means every level said "I do not carry one" — a fact about the
/// FORMAT, not about this file. rawler 0.7.2 overrides none of the three
/// rendition methods for ORF, SRW, NRW, MEF, MOS, KDC, DCR, DCS, ERF, IIQ,
/// CRW or ARI (the trait defaults return `Ok(None)`, `decoders/mod.rs:340`,
/// `:346`, `:351`), so an Olympus file has no embedded rendition to find no
/// matter how healthy it is. Folding that into the same error as "the preview
/// is corrupt" is what made `.orf` fail outright on the CLI while the very
/// same file developed fine in the GUI.
///
/// `Err` still means the levels that COULD have answered tried and failed —
/// a corrupt preview, an unreadable JPEG blob — and every reason is named.
fn usable_rendition<T>(path: &Path, levels: &mut [RenditionLevel<'_, T>]) -> Result<Option<T>> {
    let mut why: Vec<String> = Vec::new();
    for (name, get) in levels.iter_mut() {
        match get() {
            Ok(Some(v)) => return Ok(Some(v)),
            Ok(None) => {}
            Err(e) => why.push(format!("{name}: {e:#}")),
        }
    }
    if why.is_empty() {
        return Ok(None);
    }
    anyhow::bail!("no usable embedded rendition in {} ({})", path.display(), why.join("; "))
}

/// Long edge for the stand-in develop [`neutral_rendition`] produces. 2048 px
/// is the working edge the rest of the tree already treats as "preview-sized"
/// (`pipeline`'s base-look estimation, `denoise`'s and `generative`'s
/// pre-shrink), and it is chosen for the same reason here: the histogram this
/// feeds is a 256-bin summary and the AI advisor is shown a downscaled image
/// regardless, so paying a full-resolution develop would buy nothing.
const NEUTRAL_PREVIEW_EDGE: u32 = 2048;

/// The stand-in for a camera rendition on a format that embeds none (A4): our
/// OWN neutral develop at working resolution, already in the display frame
/// (`render_to_image` orients before anything else).
///
/// It says so, on stderr, every time — the GUI/web route their own copy of
/// this line through their toast/warning channels. That disclosure is not
/// decoration: the base-look estimator's whole method is comparing the
/// CAMERA's rendering against a neutral one, so a neutral develop silently
/// posing as a camera JPEG would make it measure zero difference and report a
/// confident "no base look". [`embedded_preview`] — the estimator's own door —
/// therefore keeps the strict "camera pixels or nothing" contract and never
/// comes here.
fn neutral_rendition(path: &Path, quarter_turns: u8) -> Result<DynamicImage> {
    eprintln!(
        "⚠ {} carries no embedded preview or thumbnail (its format does not store one), so \
         Autoshop is showing its own neutral develop instead of the camera's rendering",
        path.display()
    );
    crate::render::render_to_image(
        path,
        // NEUTRAL except for the turn: the develop must land in the same frame
        // as an extracted rendition would have, and `render_to_image` is the
        // one place that applies the composed orientation.
        &crate::recipe::EditRecipe { quarter_turns, ..Default::default() },
        None,
        Some(NEUTRAL_PREVIEW_EDGE),
    )
    .with_context(|| {
        format!(
            "{} has no embedded rendition, and developing the sensor as a stand-in failed too",
            path.display()
        )
    })
}

/// Decode a RAW file: embedded preview + metadata + histogram. Reads the file
/// only; never writes near the source.
///
/// The EXIF orientation only — the door for a caller that holds no recipe (the
/// `decode` CLI command, the style-index scan of a foreign library). A caller
/// that DOES hold one takes [`decode_raw_turned`].
pub fn decode_raw(path: &Path) -> Result<Decoded> {
    decode_raw_turned(path, 0)
}

/// [`decode_raw`] with the photographer's own quarter turns folded in
/// ([`crate::render::compose_orientation`]), so `Meta`'s display dims and the
/// oriented preview describe the frame the render and the export will produce
/// — not the one the camera happened to write.
///
/// A SEPARATE entry point rather than a parameter on `decode_raw` because most
/// callers genuinely have no recipe to read a turn from, and defaulting them
/// to 0 at each site is how the pre-v0.30 orientation drift happened. Here the
/// turn is impossible to forget: the un-turned door SAYS 0 in one place.
pub fn decode_raw_turned(path: &Path, quarter_turns: u8) -> Result<Decoded> {
    guard_tiff_chain(path)?;
    let src = RawSource::new(path).with_context(|| format!("open RAW {}", path.display()))?;
    let decoder = decoder_for(path, &src)?;
    let params = RawDecodeParams { image_index: 0 };

    // Prefer the mid-size embedded preview; fall back to the thumbnail, then
    // to the embedded FULL-SIZE camera JPEG (`full_image` extracts the
    // JPEGInterchangeFormat blob — it never develops the sensor; rawler's ARW
    // decoder in fact implements ONLY this level, so Sony files always
    // resolve here). Evaluated lazily to skip the larger JPEG decodes when a
    // cheaper level exists. `None` = the FORMAT embeds none (the ORF class) —
    // degraded to our own develop below, never a hard failure (A4).
    let camera_preview = guard_parser_panic(path, "embedded rendition", || {
        usable_rendition(
            path,
            &mut [
                ("preview_image", &mut || {
                    decoder.preview_image(&src, &params).map_err(|e| anyhow!("{e}"))
                }),
                ("thumbnail_image", &mut || {
                    decoder.thumbnail_image(&src, &params).map_err(|e| anyhow!("{e}"))
                }),
                ("full_image", &mut || {
                    decoder.full_image(&src, &params).map_err(|e| anyhow!("{e}"))
                }),
            ],
        )
    })?;

    let md = guard_parser_panic(path, "raw_metadata", || {
        decoder.raw_metadata(&src, &params).map_err(|e| anyhow!("raw_metadata: {e}"))
    })?;
    // Which way is up: EXIF tag 0x0112 off the metadata already in hand, NOT
    // the dummy raw's hard-coded field (see [`raw_orientation_of`]), COMPOSED
    // with the photographer's quarter turns (R27). Free — `md` is decoded one
    // line above, and the composition is a bit-twiddle.
    let orientation = crate::render::compose_orientation(raw_orientation_of(&md), quarter_turns);

    // `dummy = true`: populate dimensions / WB / levels without decompressing
    // the full sensor data — we only need the structural metadata here.
    let raw = guard_parser_panic(path, "raw_image(dummy)", || {
        decoder.raw_image(&src, &params, true).map_err(|e| anyhow!("raw_image(dummy): {e}"))
    })?;

    // ONE EXIF extraction for both source kinds ([`exif_facts`]) — the APEX
    // aperture fallback and the finiteness filters live there now.
    let facts = exif_facts(&md.exif);
    let mut meta = Meta {
        make: md.make.trim().to_string(),
        model: md.model.trim().to_string(),
        lens: facts.lens,
        iso: facts.iso,
        shutter: facts.shutter,
        aperture: facts.aperture,
        focal_length_mm: facts.focal_length_mm,
        exposure_bias_ev: facts.exposure_bias_ev,
        date_time: facts.date_time,
        // DISPLAY-frame dims, agreeing with the oriented preview below:
        // sensor width/height made every rotated (portrait) shot's aspect —
        // and the style index's portrait feature — read as landscape.
        width: if orientation_transposes(orientation) { raw.height } else { raw.width },
        height: if orientation_transposes(orientation) { raw.width } else { raw.height },
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
    // the EXIF orientation resolved above — the same value Meta's dims used.
    // The A4 stand-in comes back from `render_to_image` ALREADY in the display
    // frame (it orients before the develop), so it must not be oriented twice.
    let preview = match camera_preview {
        Some(p) => crate::render::oriented(p, orientation),
        None => {
            // The decoder + source go first: `render_to_image` maps the whole
            // RAW again (~15-25 MB for the zoo's files, ~120 MB for a 61 MP
            // ARW), and holding this one underneath would double that peak for
            // no reason. Order matters — the decoder borrows the source.
            drop(decoder);
            drop(src);
            // The stand-in develops through `render_to_image`, which applies
            // the COMPOSED orientation itself — so it is handed the turn, not
            // oriented afterwards (that would turn it twice).
            neutral_rendition(path, quarter_turns)?
        }
    };
    // The DELIVERED pixels are the embedded preview, and its dims are the
    // camera's choice — they can differ from the sensor math above (a DNG
    // can report 4024×6048 while its preview is 4000×6000). These numbers
    // feed the AI prompt and the style index's aspect feature, and every
    // pixel this build ever serves comes from this preview — so they must
    // describe it (L05-6). The oriented image is already display-frame.
    meta.width = preview.width() as usize;
    meta.height = preview.height() as usize;

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
    let bytes: Option<Vec<u8>> = if matches!(ext.as_deref(), Some("jpg" | "jpeg" | "jpe")) {
        // A JPEG is not a TIFF container: its packet rides in APP1 segments,
        // and the walk below is the same marker walk `jpeg_exif_block` does
        // for EXIF. Before R27 this fell into the TIFF arm, `GenericTiffReader`
        // failed on the `FFD8` header and the answer came back as ABSENT — the
        // silent-absence this function's own contract forbids.
        let mut file = std::fs::File::open(path)
            .with_context(|| format!("open image {}", path.display()))?;
        jpeg_xmp_packet(&mut file)
            .with_context(|| format!("read the XMP packet in {}", path.display()))?
    } else if matches!(ext.as_deref(), Some("cr3" | "raf")) {
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
    preview_only_turned(path, 0)
}

/// [`preview_only`] in the frame the photographer's quarter turns produce —
/// what the gallery grid and the web thumbnails show, so a rotated photo reads
/// the way the export will.
pub fn preview_only_turned(path: &Path, quarter_turns: u8) -> Result<DynamicImage> {
    if !is_raw(path) {
        // baked-by-construction: the !is_raw arm of preview_only.
        let img = load_image(path)?;
        return Ok(crate::render::oriented(
            img,
            crate::render::quarter_turn_orientation(quarter_turns),
        ));
    }
    // A4: a format that embeds no rendition (the ORF class — see
    // `usable_rendition`) is DEGRADED to our own neutral develop, with a word
    // about it, instead of failing. This is the door the gallery grid and the
    // web thumbnails use, and the old error blanked their cells after three
    // silent retries (`bin/gui/workers.rs`), which reads as a broken file.
    match camera_rendition(path, quarter_turns)? {
        Some(img) => Ok(img),
        None => neutral_rendition(path, quarter_turns),
    }
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
/// same thing in the full-res render. The orientation comes from
/// [`raw_orientation_of`] on the RAW's metadata — strictly cheaper than the
/// dummy `raw_image` this used to build, and the only field of it we ever
/// read was the one rawler hard-codes.
fn camera_rendition(path: &Path, quarter_turns: u8) -> Result<Option<DynamicImage>> {
    guard_tiff_chain(path)?;
    let src = RawSource::new(path).with_context(|| format!("open RAW {}", path.display()))?;
    let decoder = decoder_for(path, &src)?;
    let params = RawDecodeParams { image_index: 0 };
    guard_parser_panic(path, "camera rendition", || {
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
        let md = decoder
            .raw_metadata(&src, &params)
            .map_err(|e| anyhow!("raw_metadata: {e}"))?;
        Ok(Some(crate::render::oriented(
            img,
            crate::render::compose_orientation(raw_orientation_of(&md), quarter_turns),
        )))
    })
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
    embedded_preview_turned(path, 0)
}

/// [`embedded_preview`] in the frame the photographer's quarter turns produce.
/// The base-look estimator pairs it with a develop of the same photo, and a
/// luma CDF is orientation-blind — but the two images are also compared for
/// SIZE, and handing it the un-turned twin of a turned develop would be a
/// silent frame disagreement in the one place this module exists to prevent.
pub fn embedded_preview_turned(path: &Path, quarter_turns: u8) -> Result<Option<DynamicImage>> {
    if !is_raw(path) {
        return Ok(None);
    }
    camera_rendition(path, quarter_turns)
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

    /// L05-4/5: a level that errors yields to the next level instead of
    /// aborting the file, and a fully-dry chain names every reason.
    ///
    /// R27 A4 adds the THIRD outcome the first two used to be confused with:
    /// every level answering "I carry none" is `Ok(None)` — a fact about the
    /// FORMAT (ORF, SRW, NRW, MEF, … override no rendition method in rawler
    /// 0.7.2) — and must not be reported as the corruption case. That
    /// conflation is what made `.orf` fail outright on the CLI.
    ///
    /// MUTATION THIS CATCHES: make the empty-`why` arm `bail!` again (the
    /// pre-R27 code) and the third assertion fails; make the errors arm return
    /// `Ok(None)` and the second fails, which would degrade a genuinely
    /// corrupt preview into a silent neutral develop.
    #[test]
    fn a_corrupt_rendition_level_yields_to_the_next() {
        let p = Path::new("x.arw");
        let got = usable_rendition::<u32>(
            p,
            &mut [
                ("preview_image", &mut || Err(anyhow!("corrupt JPEG stream"))),
                ("thumbnail_image", &mut || Ok(Some(7))),
                ("full_image", &mut || panic!("laziness: never reached")),
            ],
        )
        .expect("the intact thumbnail answers");
        assert_eq!(got, Some(7));

        let e = usable_rendition::<u32>(
            p,
            &mut [
                ("preview_image", &mut || Err(anyhow!("corrupt JPEG stream"))),
                ("thumbnail_image", &mut || Ok(None)),
                ("full_image", &mut || Err(anyhow!("truncated blob"))),
            ],
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("preview_image: corrupt JPEG stream") && e.contains("full_image: truncated blob"),
            "the terminal failure names each level's reason: {e}"
        );

        // The ORF class: nothing errored, nothing existed.
        assert_eq!(
            usable_rendition::<u32>(
                p,
                &mut [
                    ("preview_image", &mut || Ok(None)),
                    ("thumbnail_image", &mut || Ok(None)),
                    ("full_image", &mut || Ok(None)),
                ],
            )
            .expect("a format that embeds no rendition is not a failure"),
            None,
            "'this format carries none' must be Ok(None), never the corruption error"
        );
    }

    use super::*;

    /// The baked-only gate: `load_image` must refuse a camera RAW BY NAME, at
    /// the door, before any decoder is asked.
    ///
    /// This is the v0.22 mask-refine bug's root: the GUI worker handed a .ARW
    /// to `load_image`, whose `ImageReader` has no RAW decoder, so the user saw
    /// "The image format could not be determined" for a photo the app was
    /// developing on screen at that moment. A named refusal is what makes the
    /// next missed dispatch diagnosable in one read of the toast.
    ///
    /// R24 batch 2 also pins what the sentence may NOT contain. This gate's
    /// only two surfaces are the desktop toast and the web error body, and the
    /// text used to hand both of them three Rust paths. Naming the file and the
    /// condition is what makes a missed dispatch obvious; naming our functions
    /// is what makes a user think the app is broken in a way they caused.
    #[test]
    fn load_image_refuses_a_camera_raw_by_name() { // not-a-consumer-call: the gate's own test
        let dir = std::env::temp_dir().join(format!("autoshop-load-raw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Every RAW extension the app claims, upper and lower case — one
        // predicate app-wide (`is_raw`), so the gate must not care which.
        for name in ["a.arw", "b.ARW", "c.dng", "d.NEF", "e.cr3", "f.raf", "g.rw2", "h.orf"] {
            let p = dir.join(name);
            // Real bytes on disk: the refusal must come from the EXTENSION, not
            // from a missing file or an unparseable header.
            std::fs::write(&p, b"not really a raw").unwrap();
            for (what, e) in [
                // not-a-consumer-call: the gate's own refusal test.
                ("load_image", load_image(&p).unwrap_err()),
                // not-a-consumer-call: …and the develop-charged twin's.
                ("load_image_for_develop", load_image_for_develop(&p).unwrap_err()),
            ] {
                let e = format!("{e:#}");
                assert!(
                    e.contains("RAW") && e.contains("developed"),
                    "{what}({name}) must name the RAW and the way out: {e}"
                );
                assert!(
                    e.contains(name),
                    "{what}({name}) must name the FILE — a toast that omits it \
                     cannot be acted on: {e}"
                );
                for symbol in ["render_to_image", "source_pixels", "decode_any", "::"] {
                    assert!(
                        !e.contains(symbol),
                        "{what}({name}) leaks the internal symbol {symbol:?} into \
                         a message the desktop and web faces show verbatim: {e}"
                    );
                }
            }
        }
        // A baked raster still loads through the very same gate.
        let png = dir.join("baked.png");
        image::RgbImage::from_pixel(4, 3, image::Rgb([9, 9, 9])).save(&png).unwrap();
        // not-a-consumer-call: the gate's own baked-raster case.
        assert_eq!(load_image(&png).unwrap().dimensions(), (4, 3));
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        // not-a-consumer-call: the gate's own ICC fixture (a PNG written above).
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
            // not-a-consumer-call: the gate's own invalid-ICC case.
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
        // not-a-consumer-call: the gate's own TIFF-profile fixture.
        load_image(&valid).expect("a profiled TIFF is transformed via the fallback query");

        // The load-bearing half: an unparseable profile must ERROR. If the
        // fallback is removed, the profile is never seen and this loads Ok.
        let invalid = dir.join("invalid-profile.tiff");
        write(&invalid, b"not an ICC profile");
        assert!(
            // not-a-consumer-call: the gate's own invalid-TIFF-profile case.
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

        // not-a-consumer-call: the gate's own 16-bit ICC fixture.
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
        let dir = std::env::temp_dir().join(format!("autoshop-tiff-guard-test-{}", std::process::id()));
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

    /// A JPEG whose header carries the given APP1 payloads, in order. No scan
    /// data — the packet reader stops at `SOS` and never looks at pixels.
    fn jpeg_with_app1(segments: &[Vec<u8>]) -> Vec<u8> {
        let mut f: Vec<u8> = vec![0xFF, 0xD8];
        for payload in segments {
            f.extend([0xFF, 0xE1]);
            f.extend(((payload.len() + 2) as u16).to_be_bytes());
            f.extend(payload);
        }
        f.extend([0xFF, 0xD9]);
        f
    }

    /// R27 T3, `P5-cropped-mask-frame.md` §8. A JPEG's XMP rides in APP1, and
    /// a develop block with masks routinely exceeds the 65533-byte segment
    /// limit — **13 exports in the user's own library** are split into
    /// GUID-keyed ExtendedXMP chunks, `DSC09024_1.jpg` (one of P3's seven crop
    /// specimens) among them. Before R27 a JPEG fell into the TIFF arm,
    /// `GenericTiffReader` failed on the `FFD8` header, and the packet came
    /// back as ABSENT — the silent absence this reader's contract forbids
    /// everywhere else.
    ///
    /// MUTATION THIS CATCHES: return the standard segment alone (drop the
    /// chunk loop) and the second half of the document is gone; accept a chunk
    /// whose GUID the standard packet does not name and the foreign chunk
    /// splices itself in; place the chunks in arrival order rather than by
    /// offset and the two come back swapped.
    #[test]
    fn a_jpeg_xmp_packet_reassembles_its_extended_chunks() {
        const STD: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
        const EXT: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
        const GUID: &[u8; 32] = b"5CA1AB1E5CA1AB1E5CA1AB1E5CA1AB1E";
        let dir = std::env::temp_dir().join(format!("autoshop-jpeg-xmp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let standard = {
            let mut v = STD.to_vec();
            v.extend(
                format!(
                    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description \
                     xmpNote:HasExtendedXMP=\"{}\"/></rdf:RDF></x:xmpmeta>",
                    String::from_utf8_lossy(GUID)
                )
                .as_bytes(),
            );
            v
        };
        let chunk = |body: &str, off: u32, guid: &[u8; 32]| {
            let mut v = EXT.to_vec();
            v.extend(guid);
            v.extend(12u32.to_be_bytes()); // total length of the extension
            v.extend(off.to_be_bytes());
            v.extend(body.as_bytes());
            v
        };
        // Deliberately out of order, with a foreign chunk in the middle.
        let jpg = dir.join("split.jpg");
        std::fs::write(
            &jpg,
            jpeg_with_app1(&[
                standard.clone(),
                chunk("SECOND", 6, GUID),
                chunk("XXXXXX", 0, b"0BADC0DE0BADC0DE0BADC0DE0BADC0DE"),
                chunk("_FIRST", 0, GUID),
            ]),
        )
        .unwrap();
        let text = embedded_xmp(&jpg).expect("readable").expect("present");
        assert!(text.contains("HasExtendedXMP"), "the standard packet comes first: {text}");
        assert!(text.ends_with("_FIRSTSECOND"), "reassembled BY OFFSET: {text:?}");
        assert!(!text.contains("XXXXXX"), "a foreign GUID's chunk must not join: {text:?}");

        // A chain with a hole is an ERROR, never a quietly short document.
        let holed = dir.join("holed.jpg");
        std::fs::write(&holed, jpeg_with_app1(&[standard.clone(), chunk("SECOND", 6, GUID)]))
            .unwrap();
        let err = format!("{:#}", embedded_xmp(&holed).unwrap_err());
        assert!(err.contains("gap"), "{err}");

        // No XMP APP1 at all → absence, not an error.
        let bare = dir.join("bare.jpg");
        std::fs::write(&bare, jpeg_with_app1(&[b"Exif\0\0II".to_vec()])).unwrap();
        assert!(embedded_xmp(&bare).expect("readable").is_none());
    }

    /// L05#6: the packet lives in TIFF tag 0x02BC for every TIFF-container
    /// RAW — DNG included, where rawler 0.7.2's per-format `xpacket` answers
    /// None (only CR2/CR3/RAF/TFR override it). The reader is honest at both
    /// edges: over the cap is REFUSED (a truncated packet would parse to a
    /// different develop), and a non-text packet is an error, never "absent".
    #[test]
    fn an_embedded_xmp_packet_is_read_from_the_tiff_tag() {
        let dir = std::env::temp_dir().join(format!("autoshop-embedded-xmp-test-{}", std::process::id()));
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

    /// R25 P0: the accounting BEHIND [`PIPELINE_BYTES_PER_PIXEL`], written as
    /// data so a new full-frame stage has somewhere to land and the constant
    /// can never be a number without a derivation.
    ///
    /// Each row is one moment of `render::render_baked_to_image` and the
    /// per-SOURCE-pixel bytes it holds ON TOP of the decoded buffer (which
    /// [`decode_peak_bytes`] charges separately). Re-read from the render
    /// source, not from the old comment: `img.to_rgb16()` is a 6 B/px staging
    /// copy dropped right after the transcode; `unsharp_luma_weighted` /
    /// `noise_reduce_luma` hold one luma plane while `blur_plane` chains two
    /// more (`box_blur_v` allocates its output before the previous buffer is
    /// dropped); `rgb16_source` BORROWS an already-Rgb16 frame, and
    /// `rotate_straighten` writes an INSCRIBED (never larger) output, so a
    /// resampler costs exactly one fresh frame.
    ///
    /// The one 61 MP peak measured on real hardware in R24
    /// (`render::tests::portrait_rotation_peak_on_a_61mp_frame`, 1384 MB) is
    /// the RAW path's `orient_f32` — source + rotated copy, a different
    /// function this gate does not cover — and it landed BELOW its own
    /// 1464 MB prediction, so it neither raises nor lowers the number here.
    #[test]
    fn develop_peak_accounts_for_every_pass() {
        use image::metadata::Orientation;
        let stages: [(&str, u64); 6] = [
            ("transcode: to_rgb16 staging 6 + the f32 planes 12", 6 + 12),
            ("spatial pass: planes 12 + luma 4 + blur_plane's two chained planes 8", 12 + 4 + 8),
            ("pack: planes 12 + the packed u16 frame 6", 12 + 6),
            ("geometry: planes 12 + the frame 6 + the resampler's fresh output 6", 12 + 6 + 6),
            ("crop: planes 12 + the frame 6 + the (never larger) cropped copy 6", 12 + 6 + 6),
            ("AI denoise: planes 12 + the sidecar's decoded output 6 + its to_rgb16 6", 12 + 6 + 6),
        ];
        let (worst, peak) =
            stages.iter().max_by_key(|(_, b)| *b).copied().expect("the table is not empty");
        assert_eq!(
            PIPELINE_BYTES_PER_PIXEL, peak,
            "the constant must be the WORST moment of the chain — {worst}"
        );
        // …and the gate really charges it, per SOURCE pixel, on top of the
        // decode (which is 0 here so the arithmetic is visible).
        assert_eq!(
            develop_peak_bytes(0, Orientation::NoTransforms, 1_000_000),
            1_000_000 * peak
        );
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

    /// R28 Batch-4 4a — the RAW twin of the two tests above: until this batch
    /// the develop chain had a per-file ceiling on ONE side only, so a 150 MP
    /// back that the baked door would have refused outright went through the
    /// RAW door with nothing checked (adjudication F2's deeper root).
    ///
    /// MUTATION THIS KILLS: delete the `allocation_over_ceiling` branch in
    /// `refuse_raw_develop_over_ceiling` (or turn its `>=` into `>`, which the
    /// exact-ceiling row below catches) and the 200 MP frame is admitted —
    /// the exact silence the gate exists to end.
    #[test]
    fn a_raw_over_the_ceiling_is_refused_by_name_and_offers_the_way_out() {
        let p = std::path::Path::new("big.iiq");
        // 60.2 MP, the measured corpus frame: admitted, with room to spare.
        assert!(refuse_raw_develop_over_ceiling(p, 60_217_344).is_ok());
        // The BOUNDARY, both sides of it. 31 B/px x 138,547,332 px is
        // 4,294,967,292 B, still UNDER 4 GiB, so the first count at or over the
        // ceiling is `MAX_ALLOC.div_ceil(31)` = 138,547,333 — which is what the
        // line below computes, and what both docs already say.
        let at = MAX_ALLOC.div_ceil(RAW_DEVELOP_BYTES_PER_PIXEL);
        assert!(refuse_raw_develop_over_ceiling(p, at - 1).is_ok(), "just under must develop");
        let e = refuse_raw_develop_over_ceiling(p, at).unwrap_err().to_string();
        // The disclosure the user ruling asked for: WHICH file, the estimate
        // and its basis, and the correction of the obvious wrong guess.
        assert!(e.contains("big.iiq"), "must name the file: {e}");
        assert!(e.contains(&at.to_string()), "must name the pixel count: {e}");
        assert!(
            e.contains(&RAW_DEVELOP_BYTES_PER_PIXEL.to_string()) && e.contains("measured"),
            "must show the basis, not just a verdict: {e}"
        );
        assert!(e.contains("--jobs 1"), "must correct the concurrency guess: {e}");
        // A 200 MP back — the class F2 named as the worse, zero-opt-in
        // instance — is refused where the same count on the baked side is.
        assert!(refuse_raw_develop_over_ceiling(p, 200_000_000).is_err());
        // Absurd input saturates instead of wrapping into an accidental pass.
        assert!(refuse_raw_develop_over_ceiling(p, u64::MAX).is_err());
    }

    /// The RAW and baked ceilings must be the SAME ceiling — the user ruling
    /// (2026-08-20) was "the same 4 GiB per-file peak gate as baked", and two
    /// numbers that merely happen to agree today would drift.
    ///
    /// Also pins the per-pixel constant against the measurement that produced
    /// it: 1,771 MB over 60,217,344 px = 30.84 B/px, and the constant must be
    /// the CEILING of that (rounding a peak down admits files it should
    /// refuse). MUTATION THIS KILLS: lowering the constant to 30.
    #[test]
    fn the_raw_ceiling_is_the_baked_ceiling_and_its_constant_matches_the_probe() {
        let measured_bytes: u64 = 1771 * 1024 * 1024;
        let measured_px: u64 = 60_217_344;
        let per_px = measured_bytes.div_ceil(measured_px);
        assert_eq!(
            RAW_DEVELOP_BYTES_PER_PIXEL, per_px,
            "the constant is the ROUNDED-UP measurement (jobs.rs' stage table), not a guess"
        );
        // One ceiling, asserted rather than assumed: whatever `MAX_ALLOC` is,
        // both doors refuse at it.
        assert!(allocation_over_ceiling(raw_develop_peak_bytes(
            MAX_ALLOC.div_ceil(RAW_DEVELOP_BYTES_PER_PIXEL)
        )));
        assert!(allocation_over_ceiling(MAX_ALLOC));
    }

    /// A ceiling is only a ceiling if the ONE funnel charges it, and the pure
    /// tests above cannot see a call site. This is that half: a SOURCE SCAN,
    /// the idiom this repo already uses where the property is about WHERE code
    /// is called rather than what it computes (`store`'s capped-reader gate,
    /// the GUI font gate).
    ///
    /// MUTATION THIS KILLS: deleting the call from `render::render_to_image_in`
    /// — every RAW is admitted again and no unit test of the pure function
    /// would notice — or moving it AFTER the non-dummy sensor read, which
    /// would refuse only once the decompression it exists to prevent had
    /// already happened.
    #[test]
    fn the_one_raw_develop_funnel_charges_the_ceiling_before_it_decompresses() {
        // LF-normalised: this repo has MIXED line endings by design (per-file
        // `.gitattributes` + `core.autocrlf`).
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/render.rs"),
        )
        .expect("render.rs readable")
        .replace("\r\n", "\n");
        assert_eq!(
            text.matches("refuse_raw_develop_over_ceiling_for(").count(),
            1,
            "exactly ONE call site — a second would be the per-caller copy this design refuses"
        );
        let gate = text
            .find("refuse_raw_develop_over_ceiling_for(")
            .expect("the RAW develop funnel must charge the ceiling");
        // Non-vacuity AND the ordering: the gate exists to run before the
        // sensor is decompressed, which is the `dummy = false` read.
        let sensor = text
            .find("decoder.raw_image(&src, &params, false)")
            .expect("extractor anchor: the non-dummy sensor read moved — re-anchor this gate");
        assert!(
            gate < sensor,
            "the ceiling must be charged BEFORE the sensor decompression it bounds"
        );
    }

    /// The planner's half (R28 Batch-4 4a): a baked source answers its own
    /// peak from a HEADER, a RAW declines to answer at all, and the two
    /// answers come from the same accounting the ceilings enforce.
    ///
    /// MUTATION THIS KILLS: make `cheap_develop_peak_mb` answer for RAWs too
    /// (say by dropping the `is_raw` arm) — a 61 MP ARW would then pay
    /// `RawSource::new`, the whole file mapped, once per photo before the pool
    /// starts, which is the cost the corpus constant exists to avoid.
    #[test]
    fn the_cheap_peak_estimate_answers_for_baked_sources_and_declines_for_raw() {
        assert_eq!(cheap_develop_peak_mb(std::path::Path::new("a.arw")), None);
        assert_eq!(cheap_develop_peak_mb(std::path::Path::new("a.dng")), None);
        // An absent/unreadable baked file is `None` too — a PLAN must never be
        // the thing that fails a run.
        assert_eq!(cheap_develop_peak_mb(std::path::Path::new("nope.png")), None);

        let dir = std::env::temp_dir().join(format!("autoshop-peak-est-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let png = dir.join("small.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::new(400, 300))
            .save(&png)
            .expect("write fixture");
        let mb = cheap_develop_peak_mb(&png).expect("a readable PNG answers");
        // 400x300 RGB8: decode 360,000 B + 120,000 px x 24 B = 3,240,000 B.
        let want = develop_peak_bytes(
            400 * 300 * 3,
            image::metadata::Orientation::NoTransforms,
            400 * 300,
        );
        assert_eq!(mb, want.div_ceil(1024 * 1024), "the estimate IS develop_peak_bytes, in MB");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE INTERFACE that was broken: rawler 0.7.2 hard-codes
    /// `RawImage.orientation` to `Normal` for every decoder but DNG/QTK, so
    /// the value must come off the EXIF metadata. This pins the mapping
    /// itself — `Some(8)` is the portrait ARW's tag and it MUST become
    /// `Rotate270`, not `Normal`.
    #[test]
    fn raw_orientation_reads_the_exif_tag_not_rawlers_constant() {
        let md = |tag: Option<u16>| rawler::decoders::RawMetadata {
            exif: rawler::exif::Exif { orientation: tag, ..Default::default() },
            ..Default::default()
        };
        assert_eq!(raw_orientation_of(&md(Some(8))), rawler::Orientation::Rotate270);
        assert_eq!(raw_orientation_of(&md(Some(6))), rawler::Orientation::Rotate90);
        assert_eq!(raw_orientation_of(&md(Some(1))), rawler::Orientation::Normal);
        // No tag at all: our answer is Normal where rawler's own
        // `Orientation::from_tiff` would say Unknown — pinned as the SAME
        // no-op by `unknown_and_normal_are_the_same_no_op` below.
        assert_eq!(raw_orientation_of(&md(None)), rawler::Orientation::Normal);
        // An out-of-range tag value is rawler's Unknown, which is also a
        // no-op — a corrupt 0x0112 must never rotate anything.
        assert_eq!(raw_orientation_of(&md(Some(99))), rawler::Orientation::Unknown);
    }

    /// The DNG semantic difference, ASSERTED rather than assumed: rawler's
    /// `from_tiff` answers `Unknown` for a missing tag while
    /// [`raw_orientation_of`] answers `Normal`, and the whole chain is only
    /// safe because the two are the same NO-OP everywhere they are consumed —
    /// the pixel transform, the coordinate transform, and the width/height
    /// swap.
    #[test]
    fn unknown_and_normal_are_the_same_no_op() {
        use rawler::Orientation::{Normal, Unknown};
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(7, 3, |x, y| {
            image::Rgb([x as u8, y as u8, 0])
        }));
        assert_eq!(
            crate::render::oriented(img.clone(), Unknown).to_rgb8().into_raw(),
            crate::render::oriented(img, Normal).to_rgb8().into_raw(),
            "pixels"
        );
        for (u, v) in [(0.0f32, 0.0f32), (0.25, 0.75), (1.0, 1.0), (-0.5, 1.7)] {
            assert_eq!(
                crate::render::orient_point(Unknown, u, v),
                crate::render::orient_point(Normal, u, v),
                "coordinates at ({u}, {v})"
            );
        }
        assert_eq!(orientation_transposes(Unknown), orientation_transposes(Normal), "dims");
    }

    /// Real-machine probe, never run in CI: point AUTOSHOP_ORIENT_PROBE_RAW at
    /// a RAW whose IFD0 tag 0x0112 is 8 (a portrait Sony ARW) and this asserts
    /// the WHOLE chain — the accessor answers `Rotate270`, and `decode_raw`
    /// hands back a PORTRAIT frame (height > width). The two constant tests
    /// above hand-feed the enum and so cannot see a broken link between the
    /// file and the pipeline; this is the one that can.
    #[test]
    #[ignore = "real-machine probe: set AUTOSHOP_ORIENT_PROBE_RAW to a portrait RAW"]
    fn portrait_raw_reaches_the_pipeline_as_rotate270() {
        let Ok(path) = std::env::var("AUTOSHOP_ORIENT_PROBE_RAW") else {
            panic!("set AUTOSHOP_ORIENT_PROBE_RAW to a RAW with EXIF orientation 8");
        };
        let p = std::path::Path::new(&path);
        assert_eq!(
            raw_orientation(p).expect("read orientation"),
            rawler::Orientation::Rotate270,
            "EXIF tag 0x0112 = 8 must reach the pipeline as Rotate270"
        );
        let d = decode_raw(p).expect("decode");
        eprintln!(
            "{}: EXIF 0x0112 = 8 → Rotate270, decoded frame {}x{}",
            p.display(),
            d.meta.width,
            d.meta.height
        );
        assert!(
            d.meta.height > d.meta.width,
            "a portrait RAW must decode to a portrait frame, got {}x{}",
            d.meta.width,
            d.meta.height
        );
        assert_eq!(
            (d.preview.width() as usize, d.preview.height() as usize),
            (d.meta.width, d.meta.height),
            "Meta dims must describe the delivered preview"
        );
        // R27 A10 — the photographer's own quarter turns, on the one file
        // where the EXIF half is not `Normal`. `Rotate270 + 1 = Normal`, so
        // ONE clockwise turn of a portrait ARW is a LANDSCAPE frame, and two
        // turns bring the portrait back the other way up. This is the arm the
        // pure-code contract test cannot reach: it proves the composed value
        // travels from tag 0x0112 all the way to delivered pixels.
        for (k, want_portrait) in [(1u8, false), (2, true), (3, false), (4, true)] {
            let t = decode_raw_turned(p, k).unwrap_or_else(|e| panic!("decode +{k}: {e:#}"));
            eprintln!("  +{k} quarter turns → {}x{}", t.meta.width, t.meta.height);
            assert_eq!(
                t.meta.height > t.meta.width,
                want_portrait,
                "+{k} quarter turns gave {}x{}",
                t.meta.width,
                t.meta.height
            );
            assert_eq!(
                (t.preview.width() as usize, t.preview.height() as usize),
                (t.meta.width, t.meta.height),
                "+{k}: Meta dims must still describe the delivered preview"
            );
        }
        assert_eq!(
            frame_size_turned(p, 1).expect("frame_size +1"),
            {
                let (w, h) = frame_size(p).expect("frame_size");
                (h, w)
            },
            "one quarter turn must transpose the develop frame"
        );
    }

    /// v0.32.0 — the develop window sits on the DefaultCrop rectangle, not on
    /// the sensor's top-left corner.
    ///
    /// The first row is the real Sony A7R IV geometry the defect was measured
    /// on: `9600 × 6376` raw, `SonyRawImageSize` giving rawler an active area
    /// of `9504 × 6336` **at (0, 0)**, `DefaultCropOrigin = (32, 20)`. rawler
    /// 0.7.2 skips its own `CropDefault` there (equal sizes — see
    /// `align_default_crop`), which is why every ARW render landed 32 px right
    /// and 20 px down of Lightroom's frame.
    ///
    /// The other rows are the cases that must NOT move: rawler's own crop step
    /// already handles a size-reducing crop, an aligned pair has nothing to do,
    /// and a rectangle that would run off the sensor is refused rather than
    /// clamped.
    ///
    /// MUTATION THIS CATCHES: drop the `crop.d != active.d` guard and the
    /// size-reducing row starts double-cropping; drop the bounds check and the
    /// off-sensor row hands rawler a ROI it would index past the buffer with.
    #[test]
    fn the_demosaic_window_moves_onto_the_default_crop_rectangle() {
        use rawler::imgop::{Dim2, Point, Rect};
        let r = |x, y, w, h| Rect::new(Point::new(x, y), Dim2::new(w, h));
        let sony_crop = r(32, 20, 9504, 6336);
        assert_eq!(
            aligned_demosaic_roi(Some(sony_crop), Some(r(0, 0, 9504, 6336)), 9600, 6376),
            CropAlignment::Moved(sony_crop),
            "the A7R IV window starts at DefaultCropOrigin, not at the sensor corner"
        );
        assert_eq!(
            aligned_demosaic_roi(Some(r(32, 20, 9000, 6000)), Some(r(0, 0, 9504, 6336)), 9600, 6376),
            CropAlignment::NothingToMove,
            "a size-REDUCING default crop is rawler's own CropDefault step's job"
        );
        assert_eq!(
            aligned_demosaic_roi(Some(sony_crop), Some(sony_crop), 9600, 6376),
            CropAlignment::NothingToMove,
            "an already-aligned pair has nothing to move"
        );
        let off = r(200, 20, 9504, 6336);
        assert_eq!(
            aligned_demosaic_roi(Some(off), Some(r(0, 0, 9504, 6336)), 9600, 6376),
            CropAlignment::OffSensor { crop: off, width: 9600, height: 6376 },
            "a window that would run off the sensor is refused, never clamped — and SAID (A5)"
        );
        assert_eq!(
            aligned_demosaic_roi(None, Some(r(0, 0, 9504, 6336)), 9600, 6376),
            CropAlignment::NothingToMove,
            "a RAW that declares no default crop keeps the window it had"
        );
    }

    /// A little-endian TIFF block with an IFD0 (Make/Model/ExifIFDPointer) and
    /// an ExifIFD (ISO / FNumber / ExposureTime / FocalLength /
    /// DateTimeOriginal) — the exact shape a camera writes into a JPEG's APP1
    /// segment, built by hand so the P2 tests need no photograph and no new
    /// dependency.
    ///
    /// Values ≤ 4 bytes live inline in the entry; longer ones (the two ASCII
    /// strings and the three rationals) are appended after both IFDs and
    /// referenced by offset, which is what makes this a real exercise of the
    /// reader rather than a straight-line one.
    #[cfg(test)]
    fn exif_block_fixture() -> Vec<u8> {
        // Tag, type, count, then either the inline value or a pool entry.
        const ASCII: u16 = 2;
        const SHORT: u16 = 3;
        const LONG: u16 = 4;
        const RATIONAL: u16 = 5;
        let make = b"TestCam Industries\0";
        let model = b"TC-1\0";
        let date = b"2026:08:19 11:22:33\0";
        // header(8) + ifd0(2 + 4*12 + 4) + exif(2 + 5*12 + 4) = 8 + 54 + 66
        let ifd0_at = 8u32;
        let ifd0_len = 2 + 4 * 12 + 4;
        let exif_at = ifd0_at + ifd0_len as u32;
        let exif_len = 2 + 5 * 12 + 4;
        let pool_at = exif_at + exif_len as u32;

        let mut pool: Vec<u8> = Vec::new();
        let push = |bytes: &[u8], pool: &mut Vec<u8>| -> u32 {
            let at = pool_at + pool.len() as u32;
            pool.extend_from_slice(bytes);
            at
        };
        let make_at = push(make, &mut pool);
        let model_at = push(model, &mut pool);
        let date_at = push(date, &mut pool);
        // f/2.8 = 28/10, 1/250 s, 35 mm.
        let fnum_at = push(&[28, 0, 0, 0, 10, 0, 0, 0], &mut pool);
        let time_at = push(&[1, 0, 0, 0, 250, 0, 0, 0], &mut pool);
        let focal_at = push(&[35, 0, 0, 0, 1, 0, 0, 0], &mut pool);

        let entry = |tag: u16, ty: u16, count: u32, val: u32| {
            let mut e = Vec::with_capacity(12);
            e.extend_from_slice(&tag.to_le_bytes());
            e.extend_from_slice(&ty.to_le_bytes());
            e.extend_from_slice(&count.to_le_bytes());
            e.extend_from_slice(&val.to_le_bytes());
            e
        };
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"II");
        out.extend_from_slice(&42u16.to_le_bytes());
        out.extend_from_slice(&ifd0_at.to_le_bytes());
        // IFD0 — entries MUST be in ascending tag order.
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend(entry(0x010F, ASCII, make.len() as u32, make_at)); // Make
        out.extend(entry(0x0110, ASCII, model.len() as u32, model_at)); // Model
        out.extend(entry(0x0112, SHORT, 1, 1)); // Orientation = Normal
        out.extend(entry(0x8769, LONG, 1, exif_at)); // ExifIFDPointer
        out.extend_from_slice(&0u32.to_le_bytes()); // no IFD1
        // ExifIFD
        out.extend_from_slice(&5u16.to_le_bytes());
        out.extend(entry(0x829A, RATIONAL, 1, time_at)); // ExposureTime
        out.extend(entry(0x829D, RATIONAL, 1, fnum_at)); // FNumber
        out.extend(entry(0x8827, SHORT, 1, 640)); // ISOSpeedRatings
        out.extend(entry(0x9003, ASCII, date.len() as u32, date_at)); // DateTimeOriginal
        out.extend(entry(0x920A, RATIONAL, 1, focal_at)); // FocalLength
        out.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(out.len() as u32, pool_at, "the fixture's offset arithmetic must be exact");
        out.extend_from_slice(&pool);
        out
    }

    /// R27 P2 — a baked photo's real EXIF reaches [`Meta`], through the SAME
    /// extraction the RAW arm uses.
    ///
    /// Both containers are exercised, because they reach the TIFF block by
    /// different routes: a TIFF IS the block, a JPEG wraps it in an APP1
    /// segment that [`jpeg_exif_block`] has to walk the marker chain to find.
    /// The JPEG case deliberately puts a decoy APP0 (JFIF) and a decoy
    /// non-EXIF APP1 (an XMP packet, which is what really sits there in a
    /// Lightroom export) BEFORE the real one.
    ///
    /// MUTATION THIS CATCHES: search the JPEG for the `FFE1` byte pair instead
    /// of walking markers and the decoy APP1 is returned as the EXIF block;
    /// drop the `Exif\0\0` check and the XMP packet is handed to the TIFF
    /// reader; forget `split_off(6)` and the block starts six bytes early and
    /// parses as nothing. Give `decode_baked` its own copy of the aperture
    /// rule and the APEX fallback silently stops applying to baked photos.
    #[test]
    fn a_baked_photos_own_exif_reaches_its_metadata() {
        let block = exif_block_fixture();

        // --- TIFF: the block IS the file's header.
        let dir = std::env::temp_dir().join(format!("autoshop_baked_exif_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let tif = dir.join("export.tif");
        std::fs::write(&tif, &block).expect("write");
        let (make, model, exif) =
            baked_exif(&tif).expect("readable").expect("a TIFF header IS the EXIF block");
        assert_eq!(make, "TestCam Industries");
        assert_eq!(model, "TC-1");

        // --- JPEG: SOI, a decoy APP0, a decoy non-EXIF APP1, then the real
        // one. Nothing here has to DECODE — `baked_exif` reads headers only.
        let mut jpg: Vec<u8> = vec![0xFF, 0xD8];
        let seg = |marker: u8, payload: &[u8], out: &mut Vec<u8>| {
            out.extend_from_slice(&[0xFF, marker]);
            out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            out.extend_from_slice(payload);
        };
        seg(0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0", &mut jpg);
        seg(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>", &mut jpg);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&block);
        seg(0xE1, &app1, &mut jpg);
        jpg.extend_from_slice(&[0xFF, 0xD9]);
        let jpeg = dir.join("phone.jpg");
        std::fs::write(&jpeg, &jpg).expect("write");
        let (jmake, jmodel, jexif) =
            baked_exif(&jpeg).expect("readable").expect("the third segment is the EXIF one");
        assert_eq!((jmake.as_str(), jmodel.as_str()), ("TestCam Industries", "TC-1"));

        // --- The SAME extraction the RAW arm uses, on both.
        for (what, e) in [("tiff", &exif), ("jpeg", &jexif)] {
            let f = exif_facts(e);
            assert_eq!(f.iso, Some(640), "{what}: ISO");
            assert_eq!(f.shutter.as_deref(), Some("1/250"), "{what}: shutter");
            assert_eq!(f.aperture, Some(2.8), "{what}: f-number");
            assert_eq!(f.focal_length_mm, Some(35.0), "{what}: focal length");
            assert_eq!(
                f.date_time.as_deref(),
                Some("2026:08:19 11:22:33"),
                "{what}: capture time"
            );
        }

        // --- Absence is Ok(None), not an error: an untagged export is the
        // normal case. A PNG is the documented no-route container.
        let png = dir.join("plain.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n").expect("write");
        assert!(
            baked_exif(&png).expect("no EXIF route is not a failure").is_none(),
            "PNG has no EXIF route in this build — absence, not an error"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R27 P4 — the three codecs added to `pipeline::BAKED_EXTS` really
    /// DECODE, rather than merely being named in a list.
    ///
    /// The two halves can drift apart in the worst direction: an extension in
    /// `BAKED_EXTS` passes the file dialog, the drag-and-drop gate, the
    /// gallery scan and the upload endpoint, and only then fails at the
    /// decoder — a photo that appears to be accepted and then is not. Adding
    /// the `Cargo.toml` feature and the list entry are two separate edits, so
    /// this is the test that keeps them one change.
    ///
    /// MUTATION THIS CATCHES: remove `"webp"` (or `bmp`, or `gif`) from the
    /// `image` dependency's feature list while leaving `BAKED_EXTS` alone —
    /// the round trip stops finding an encoder and this fails, instead of a
    /// user discovering it by dropping a photo on the window.
    #[test]
    fn every_baked_extension_has_a_working_codec() {
        let dir = std::env::temp_dir().join(format!("autoshop_codecs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        // A gradient, not a flat fill: GIF quantises to 256 colours and BMP/
        // WebP have their own packings, so a uniform image could round-trip
        // through a broken path by accident.
        let src = image::RgbImage::from_fn(24, 16, |x, y| {
            image::Rgb([(x * 10) as u8, (y * 15) as u8, 60])
        });
        for ext in crate::pipeline::BAKED_EXTS {
            // "tif"/"tiff" and "jpg"/"jpeg" are the same codec twice; writing
            // both is the point (the extension is what the gates see).
            let at = dir.join(format!("probe.{ext}"));
            image::DynamicImage::ImageRgb8(src.clone())
                .save(&at)
                .unwrap_or_else(|e| panic!("{ext}: the image crate cannot ENCODE it — {e}"));
            // baked-by-construction: a file this test just wrote in a baked format.
            let back = load_image(&at)
                .unwrap_or_else(|e| panic!("{ext}: written, then not readable — {e:#}"));
            assert_eq!(
                back.dimensions(),
                (24, 16),
                "{ext}: round-tripped to the wrong size"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R27 R11 — a camera RAW wearing a `.tif` extension is REFUSED by name,
    /// not silently decoded.
    ///
    /// `is_raw` is extension-based, so a DNG named `.tif` takes the baked arm;
    /// a DNG really is a TIFF, so the `image` crate would decode its first IFD
    /// — the embedded thumbnail — and the photo would open, look plausible,
    /// and develop at a few hundred pixels. Opening wrong is worse than not
    /// opening.
    ///
    /// MUTATION THIS CATCHES: drop the `DNGVersion` probe and the fixture
    /// below opens as a 2×2 image with no complaint; move the check back after
    /// `into_decoder` and the message becomes whatever the TIFF codec says
    /// about a sensor plane.
    #[test]
    fn a_raw_named_tif_is_refused_by_name() {
        let dir = std::env::temp_dir().join(format!("autoshop_tif_raw_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        // A real 2×2 TIFF, so the ONLY thing separating the two files below is
        // the DNGVersion tag — not "one of them is malformed".
        let plain = dir.join("export.tif");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9])))
            .save(&plain)
            .expect("encode a plain TIFF");
        // baked-by-construction: a .tif this test just wrote with the image crate.
        assert!(load_image(&plain).is_ok(), "an ordinary baked TIFF still opens");

        // The same bytes plus a DNGVersion entry. Built by hand: IFD0 with the
        // minimum a TIFF reader needs, plus 0xC612.
        let dng_ish = dir.join("actually-a-raw.tif");
        let entry = |tag: u16, ty: u16, count: u32, val: u32| {
            let mut e = Vec::new();
            e.extend_from_slice(&tag.to_le_bytes());
            e.extend_from_slice(&ty.to_le_bytes());
            e.extend_from_slice(&count.to_le_bytes());
            e.extend_from_slice(&val.to_le_bytes());
            e
        };
        let mut t: Vec<u8> = Vec::new();
        t.extend_from_slice(b"II");
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&8u32.to_le_bytes());
        t.extend_from_slice(&2u16.to_le_bytes());
        t.extend(entry(0x010F, 2, 6, 0x0000_0000)); // Make (offset 0 — unread)
        t.extend(entry(0xC612, 1, 4, 0x0000_0401)); // DNGVersion 1.4.0.0, inline
        t.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(&dng_ish, &t).expect("write");

        // baked-by-construction: the .tif fixture this test just wrote.
        let e = format!("{:#}", load_image(&dng_ish).unwrap_err());
        assert!(
            e.contains("really a camera RAW") && e.contains("DNGVersion"),
            "the refusal must name what gave the file away: {e}"
        );
        assert!(
            e.contains("rename it"),
            "…and what to do about it: {e}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R27 A7 — "your camera is not supported" is three different failures
    /// wanting three different actions, and they used to arrive as one opaque
    /// line. Every branch also carries the DNG on-ramp (A8), because for the
    /// unknown-MODEL case — much the commonest — it is an actual remedy:
    /// rawler builds a DNG's whole camera profile from the file's own tags,
    /// so a converted file needs no database entry at all.
    ///
    /// MUTATION THIS CATCHES: collapse the make/model arms back into one and
    /// the unknown-make file starts being described as an unsupported model of
    /// a maker the decoder has never heard of; drop `DNG_ONRAMP` from an arm
    /// and that failure becomes a dead end again.
    #[test]
    fn an_unreadable_camera_is_told_apart_from_an_unreadable_maker() {
        let p = Path::new("D:/photos/DSC0001.cr3");
        let unsupported = |what: &str, make: &str, model: &str| rawler::RawlerError::Unsupported {
            what: what.to_string(),
            make: make.to_string(),
            model: model.to_string(),
            mode: String::new(),
        };

        let model = format!(
            "{:#}",
            describe_decoder_failure(p, &unsupported("Unknown camera", "Canon", "EOS R9"))
        );
        assert!(
            model.contains("Canon EOS R9") && model.contains("no calibration for that exact model"),
            "the model case names the body: {model}"
        );

        let make = format!(
            "{:#}",
            describe_decoder_failure(
                p,
                &unsupported("Couldn't find a decoder for make \"Zorki\"", "Zorki", "")
            )
        );
        assert!(
            make.contains("\"Zorki\"") && make.contains("does not know at all"),
            "the make case says the MAKER is unknown, not the model: {make}"
        );

        let none =
            format!("{:#}", describe_decoder_failure(p, &unsupported("No decoder found", "", "")));
        assert!(
            none.contains("does not read as any RAW format"),
            "the no-decoder case does not invent a camera: {none}"
        );

        for (what, msg) in [("model", &model), ("make", &make), ("none", &none)] {
            assert!(msg.contains("DNG Converter"), "{what} must offer the on-ramp: {msg}");
            assert!(msg.contains("DSC0001.cr3"), "{what} must name the file: {msg}");
        }

        // A DecoderFailed is a file-integrity story, not a camera-support one:
        // it keeps rawler's own words and gains no advice about converters.
        let corrupt = format!(
            "{:#}",
            describe_decoder_failure(
                p,
                &rawler::RawlerError::DecoderFailed("truncated strip".into())
            )
        );
        assert!(
            corrupt.contains("truncated strip") && !corrupt.contains("DNG Converter"),
            "a corrupt file is not an unsupported camera: {corrupt}"
        );
    }

    /// FORENSIC PROBE across MAKES, run against a directory of real camera
    /// files — the R27 counterpart to `xmp.rs`'s `AUTOSHOP_MB_FIXTURES`, and
    /// written to the same discipline for the same reason: the committed
    /// fixtures above are synthetic by policy, so they prove the RULES and not
    /// the FILES, and until this batch **not one non-Sony RAW had ever been
    /// through this tree** (nothing in the repo, the tests, or the ROADMAP
    /// recorded one; `docs/ARCHITECTURE.md` still scoped the app to Sony
    /// `.ARW`). Widening `RAW_EXTS` from 9 extensions to 24 without running a
    /// file from each make would have been a claim, not a feature.
    ///
    /// Point `AUTOSHOP_RAW_ZOO` at a directory (searched recursively) holding
    /// one RAW per make. For every file whose extension [`is_raw`] accepts it
    /// asserts:
    ///
    ///   * [`decode_raw`] succeeds — or, if it cannot, the reason is NAMED and
    ///     the file counted, never skipped;
    ///   * [`frame_size`] (metadata only) equals the dimensions
    ///     `render::render_to_image` actually produces. These are computed by
    ///     completely different routes — `default_crop().d` turned by the EXIF
    ///     orientation, versus rawler's real develop — and every normalised
    ///     coordinate in a recipe and in a Lightroom sidecar is measured
    ///     against the frame they are supposed to agree on;
    ///   * [`raw_orientation`] (no sensor read) equals the orientation
    ///     `decode_raw` resolved from the metadata it decoded;
    ///   * `render::as_shot_wb` is either `None` or inside the 1667–15000 K
    ///     band `wb_to_kelvin_tint` promises — a per-make colour-matrix or
    ///     WB-convention mismatch would show up here as a wild CCT;
    ///   * [`align_default_crop`]'s verdict, recorded per file (R2: the
    ///     v0.32.0 origin fix is narrow by design and this is what says which
    ///     makes it fires on).
    ///
    /// Unset, it is a silent no-op — these are photographs, they are not in
    /// this public repository, and no path to them appears in this test. SET
    /// and unreadable, or set and holding a RAW that will not read, it
    /// PANICS: a forensic probe whose files quietly stop arriving is a green
    /// test that measures nothing.
    ///
    /// MUTATION THIS CATCHES: make `frame_size`'s RAW arm report
    /// `active_area` instead of `default_crop` and the CR3 rows (whose two
    /// rectangles differ in size) mismatch the rendered dimensions
    /// immediately; drop the transpose in either function and every portrait
    /// file reports a landscape frame.
    #[test]
    fn every_make_in_the_raw_zoo_decodes_and_agrees_with_itself() {
        let Ok(dir) = std::env::var("AUTOSHOP_RAW_ZOO") else {
            return;
        };
        fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                panic!("AUTOSHOP_RAW_ZOO holds an unreadable directory: {}", dir.display());
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if is_raw(&p) {
                    out.push(p);
                }
            }
        }
        let root = Path::new(&dir);
        assert!(root.is_dir(), "AUTOSHOP_RAW_ZOO is set but is not a directory: {dir}");
        let mut files = Vec::new();
        walk(root, &mut files);
        files.sort();
        assert!(
            !files.is_empty(),
            "AUTOSHOP_RAW_ZOO ({dir}) holds no file this build calls a RAW — either the fixtures \
             went missing or RAW_EXTS no longer covers them"
        );

        let mut report = Vec::new();
        for path in &files {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            // NAMED, never skipped: an `if let Ok(..)` here would let a make
            // stop decoding and the suite stay green.
            let d = decode_raw(path)
                .unwrap_or_else(|e| panic!("{name}: decode_raw failed — {e:#}"));

            let orient = raw_orientation(path)
                .unwrap_or_else(|e| panic!("{name}: raw_orientation failed — {e:#}"));

            let (fw, fh) = frame_size(path)
                .unwrap_or_else(|e| panic!("{name}: frame_size failed — {e:#}"));
            let rendered = crate::render::render_to_image(
                path,
                &crate::recipe::EditRecipe::default(),
                None,
                None,
            )
            .unwrap_or_else(|e| panic!("{name}: render_to_image failed — {e:#}"));
            assert_eq!(
                (fw, fh),
                (rendered.width() as usize, rendered.height() as usize),
                "{name}: frame_size says {fw}x{fh} but the develop produced {}x{} — every \
                 normalised recipe/sidecar coordinate is measured against that frame",
                rendered.width(),
                rendered.height()
            );
            // The DISPLAY frame the metadata predicted must be the one that
            // came out: a transpose applied on one side only is the classic
            // portrait-renders-sideways defect.
            assert_eq!(
                orientation_transposes(orient),
                fh > fw || rendered.height() > rendered.width(),
                "{name}: orientation {orient:?} disagrees with the {fw}x{fh} frame's aspect"
            );
            // R27 A10 — and the photographer's own quarter turns compose with
            // whatever that make wrote into 0x0112. Metadata-only for all four
            // turns (no second develop), plus ONE capped turned render so the
            // claim is anchored in real pixels rather than in the predicate.
            for k in 0u8..4 {
                let (tw, th) = frame_size_turned(path, k)
                    .unwrap_or_else(|e| panic!("{name}: frame_size_turned({k}) failed — {e:#}"));
                let want = if k % 2 == 1 { (fh, fw) } else { (fw, fh) };
                assert_eq!(
                    (tw, th),
                    want,
                    "{name}: {orient:?} + {k} quarter turns says {tw}x{th}, but composing \
                     {orient:?} with {k} turns transposes {}",
                    if k % 2 == 1 { "once" } else { "not at all" }
                );
                assert_eq!(
                    orientation_transposes(crate::render::compose_orientation(orient, k)),
                    th > tw,
                    "{name}: the composed orientation and the {tw}x{th} frame disagree"
                );
            }
            let turned = crate::render::render_to_image(
                path,
                &crate::recipe::EditRecipe { quarter_turns: 1, ..Default::default() },
                None,
                Some(256),
            )
            .unwrap_or_else(|e| panic!("{name}: turned render failed — {e:#}"));
            let flat = crate::render::render_to_image(
                path,
                &crate::recipe::EditRecipe::default(),
                None,
                Some(256),
            )
            .unwrap_or_else(|e| panic!("{name}: capped render failed — {e:#}"));
            assert_eq!(
                (turned.width(), turned.height()),
                (flat.height(), flat.width()),
                "{name}: a quarter turn must transpose the rendered pixels, not just the \
                 declared dims"
            );

            let wb = crate::render::as_shot_wb(path);
            if let Some((k, tint)) = wb {
                assert!(
                    (1667.0..=15000.0).contains(&k) && tint.is_finite(),
                    "{name}: as-shot WB {k} K / tint {tint} is outside the band \
                     wb_to_kelvin_tint promises — a per-make colour-matrix or WB-convention \
                     mismatch"
                );
            }

            // The alignment verdict AND the two rectangles it was read from,
            // recorded per make (R2). The rectangles are the evidence: a
            // verdict alone cannot tell "sizes differ, so rawler's own
            // CropDefault owns it" from "origins already agree", and those are
            // different facts about a decoder. Re-decoded dummy so the numbers
            // are the ones the render saw.
            let (verdict, rects) = {
                guard_tiff_chain(path).unwrap_or_else(|e| panic!("{name}: {e:#}"));
                let src = RawSource::new(path)
                    .unwrap_or_else(|e| panic!("{name}: open failed — {e:#}"));
                let decoder = decoder_for(path, &src)
                    .unwrap_or_else(|e| panic!("{name}: {e:#}"));
                let mut raw = decoder
                    .raw_image(&src, &RawDecodeParams { image_index: 0 }, true)
                    .unwrap_or_else(|e| panic!("{name}: raw_image(dummy) failed — {e}"));
                let show = |r: Option<rawler::imgop::Rect>| match r {
                    Some(r) => format!("{}x{}@{},{}", r.d.w, r.d.h, r.p.x, r.p.y),
                    None => "none".to_string(),
                };
                let rects = format!(
                    "sensor {}x{} active {} crop {}",
                    raw.width,
                    raw.height,
                    show(raw.active_area),
                    show(raw.crop_area)
                );
                (align_default_crop(&mut raw), rects)
            };
            assert!(
                !matches!(verdict, CropAlignment::OffSensor { .. }),
                "{name}: the default-crop rectangle runs off its own sensor — that is a fact \
                 about this fixture the round report has to hear, not something to pass over"
            );

            report.push(format!(
                "{name} | {} {} | {fw}x{fh} | {orient:?} | wb={} | align={} | {rects}",
                d.meta.make,
                d.meta.model,
                match wb {
                    Some((k, t)) => format!("{k:.0}K/{t:+.2}"),
                    None => "none".into(),
                },
                match verdict {
                    CropAlignment::Moved(r) => format!("moved to ({},{})", r.p.x, r.p.y),
                    CropAlignment::NothingToMove => "unchanged".into(),
                    CropAlignment::OffSensor { .. } => "OFF-SENSOR".into(),
                }
            ));
        }
        // Printed, not asserted: the per-file table IS the deliverable of this
        // probe, and `cargo test -- --nocapture` is how it is read.
        println!("AUTOSHOP_RAW_ZOO — {} file(s)\n{}", files.len(), report.join("\n"));
    }

    /// R27 tier-1 fixture: the SHAPE of `(active_area, crop_area)` that each
    /// make's rawler decoder produces, as pure numbers, so widening
    /// [`RAW_EXTS`] from 9 extensions to 24 cannot silently change which files
    /// the v0.32.0 origin fix fires on. Risk R2 in the format-support map:
    /// that fix is narrow BY DESIGN, and the only way to know it stays narrow
    /// for a make nobody owns a camera from is to pin the arithmetic.
    ///
    /// Rows 2-9 are MEASURED, not guessed: they are the `(sensor, active,
    /// crop)` triples the `AUTOSHOP_RAW_ZOO` probe printed for nine real CC0
    /// files — one per FORMAT, eight makes, Canon covering both CR2 and CR3 —
    /// on 2026-08-19. That is why this test can exist in
    /// a public repository without the photographs — the numbers are the
    /// fixture. Row 1 (A7R IV) is the geometry the v0.32.0 defect was
    /// originally measured on and has no zoo file.
    ///
    /// **What the measurement CORRECTED.** Two claims that a source read alone
    /// had gotten wrong:
    ///   * `cr2.rs:284`'s `img.active_area = img.crop_area` is inside an
    ///     `if cpp == 3` — the sRAW/mRAW branch (`cr2.rs:281-285`). A normal
    ///     CFA CR2 keeps camera-DB borders for `active` and the file's own
    ///     rectangle for `crop`, and the EOS 40D measures them DIFFERENT
    ///     (3908×2602@30,18 vs 3888×2592@40,23).
    ///   * NEF and ORF do NOT leave `active_area` as `None`. Both get
    ///     camera-DB borders from `RawImage::new`; the D700 measures
    ///     4284×2844@2,0 and the E-M5 the whole 4640×3472@0,0.
    ///
    /// **What it CONFIRMED** — the point of the exercise: on all nine bodies
    /// the two rectangles differ in SIZE (or coincide exactly), so
    /// `align_default_crop` fires on NONE of them and rawler's own
    /// `CropDefault` owns the crop everywhere. Risk R2 is answered: widening
    /// the format list does not silently widen the v0.32.0 correction.
    ///
    /// And the A7 III is the sharpest instance — it is a SONY, and it does not
    /// move either, because `SonyRawImageSize` hands rawler the FULL
    /// 6048×4024 sensor there while on the A7R IV it hands over the
    /// already-trimmed 9504×6336. The v0.32.0 defect is therefore per-BODY,
    /// not per-make.
    ///
    /// MUTATION THIS CATCHES: widen the fix to fire when only the ORIGINS
    /// differ (drop `crop.d != active.d`) and seven of these nine rows start
    /// moving a window rawler already sized differently — a per-make offset
    /// with no symptom until someone measures a Canon render against
    /// Lightroom. Widen it to fire on `crop.p == active.p` and the RW2 row
    /// (whose rectangles are identical) moves by zero and pays a pointless
    /// re-index.
    #[test]
    fn every_makes_crop_and_active_rectangles_keep_their_shape() {
        use rawler::imgop::{Dim2, Point, Rect};
        let r = |x, y, w, h| Rect::new(Point::new(x, y), Dim2::new(w, h));

        // --- Row 1: ARW / Sony A7R IV. `arw.rs:190-197` — active from the
        // SonyRawImageSize tag at `Point::default()`, crop from
        // DefaultCropOrigin/Size. Here the tag reports the TRIMMED size, so
        // the two rectangles are the same size at different origins: the ONE
        // shape the v0.32.0 fix exists for. Not a zoo file.
        assert_eq!(
            aligned_demosaic_roi(Some(r(32, 20, 9504, 6336)), Some(r(0, 0, 9504, 6336)), 9600, 6376),
            CropAlignment::Moved(r(32, 20, 9504, 6336)),
            "ARW (A7R IV): equal sizes at different origins is the case rawler skips"
        );

        // --- Rows 2-9: measured. Every one must be NothingToMove.
        // (make, sensor w, sensor h, active, crop, why)
        let measured: [(&str, usize, usize, Rect, Rect, &str); 8] = [
            (
                "ARW (A7 III)", 6048, 4024, r(0, 0, 6048, 4024), r(12, 12, 6000, 4000),
                "SonyRawImageSize gives the FULL sensor here, so sizes differ and rawler's own \
                 CropDefault does the trim — the v0.32.0 defect is per-body, not per-make",
            ),
            (
                "CR2 (EOS 40D)", 3944, 2622, r(30, 18, 3908, 2602), r(40, 23, 3888, 2592),
                "camera-DB borders vs the file's own crop; cr2.rs:284 only aliases them for \
                 sRAW/mRAW (cpp == 3)",
            ),
            (
                "CR3 (EOS R6)", 5568, 3708, r(72, 38, 5494, 3662), r(84, 50, 5472, 3648),
                "cr3.rs:359-367 builds a separate active rect from IAD1 — different size",
            ),
            (
                "NEF (D700)", 4288, 2844, r(2, 0, 4284, 2844), r(15, 6, 4256, 2832),
                "nef.rs:302 sets crop only; active is camera-DB borders, and they differ",
            ),
            (
                "ORF (E-M5)", 4640, 3472, r(0, 0, 4640, 3472), r(8, 8, 4608, 3456),
                "orf.rs:222 sets crop only; active is the whole sensor",
            ),
            (
                "RW2 (DMC-GX85)", 4816, 3464, r(8, 8, 4592, 3448), r(8, 8, 4592, 3448),
                "IDENTICAL rectangles — the `crop.p == active.p` arm, nothing to move",
            ),
            (
                "PEF (K-5)", 4992, 3284, r(0, 0, 4960, 3284), r(22, 10, 4928, 3264),
                "camera-DB borders vs the file's crop",
            ),
            (
                "DNG (Ricoh GR II)", 4960, 3280, r(496, 328, 3952, 2624), r(504, 336, 3936, 2608),
                "dng.rs:249-252 pre-offsets the crop by the active origin; the sizes still differ",
            ),
        ];
        for (make, w, h, active, crop, why) in measured {
            assert_eq!(
                aligned_demosaic_roi(Some(crop), Some(active), w, h),
                CropAlignment::NothingToMove,
                "{make}: expected the window to stay put — {why}"
            );
            // The rectangles are also self-consistent: a crop that ran off the
            // sensor would mean the fixture itself was transcribed wrong.
            assert!(
                crop.p.x + crop.d.w <= w && crop.p.y + crop.d.h <= h,
                "{make}: the transcribed crop does not fit its own sensor"
            );
        }

        // --- The refusal arm, on the A7R IV geometry with an impossible
        // origin: refused, never clamped, and now SAID (A5).
        let off = r(200, 20, 9504, 6336);
        assert_eq!(
            aligned_demosaic_roi(Some(off), Some(r(0, 0, 9504, 6336)), 9600, 6376),
            CropAlignment::OffSensor { crop: off, width: 9600, height: 6376 },
            "a window that would run off the sensor is refused"
        );

        // --- TFR (Hasselblad, `.3fr`/`.fff`) — `tfr.rs:77` `Rect::from_tiff`
        // against camera-DB borders: the same SHAPE as the A7R IV, so the fix
        // is expected to fire there too, and that is correct (§3.1 of the
        // format map). No zoo file — this row is the source read, and it is
        // labelled as such rather than presented as a measurement.
        assert!(
            matches!(
                aligned_demosaic_roi(Some(r(4, 4, 8272, 6200)), Some(r(0, 0, 8272, 6200)), 8280, 6208),
                CropAlignment::Moved(_)
            ),
            "TFR: ARW-shaped, so the same correction applies"
        );
    }
}
