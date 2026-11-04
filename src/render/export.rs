//! The output pipeline: export options, delivery colour spaces and their matrices, ICC tagging, staged writes and `render_to_file`.

use super::*;

/// The output pipeline — Lightroom's export page distilled to the controls
/// that matter for delivery: resize to a long edge, output sharpening applied
/// AFTER the resize (detail lost to downscaling can only be compensated
/// post-resize), JPEG quality, and the delivery color space. `None` /
/// `Default` reproduce the classic full-resolution q95 sRGB behaviour exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportOpts {
    /// Resize so the LONG edge equals this many pixels (aspect kept, Lanczos3).
    /// Never upscales. `None` = full resolution.
    pub long_edge: Option<u32>,
    /// Output sharpening 0..=100: small-radius luma unsharp on the (resized)
    /// output. 0 = off. Screen-oriented (radius 1).
    pub sharpen: f32,
    /// JPEG quality 1..=100 (ignored by TIFF/PNG, which stay lossless).
    pub jpeg_quality: u8,
    /// Delivery color space — a REAL gamut transform + matching embedded
    /// profile, not a tag swap (gap batch D2).
    pub color_space: ExportColorSpace,
    /// Write TIFF/PNG at 8 bits per channel instead of the 16-bit default
    /// (round-12 阶段4 export normalisation: the extension alone cannot say
    /// which depth a .png/.tif should carry). JPEG is 8-bit regardless.
    pub eight_bit: bool,
}

impl Default for ExportOpts {
    fn default() -> Self {
        Self {
            long_edge: None,
            sharpen: 0.0,
            jpeg_quality: 95,
            color_space: ExportColorSpace::Srgb,
            eight_bit: false,
        }
    }
}

// --- Delivery color spaces: a real gamut transform (gap batch D2) ------------
//
// The whole pipeline works in sRGB. Choosing a wider export space converts the
// pixel NUMBERS (linearise → 3×3 primaries change → target TRC) and embeds the
// matching profile, so a color-managed viewer shows the *same* colors — that
// is the point of color management. What you gain is a valid Display P3 /
// Adobe RGB deliverable (wide-gamut web, print workflows). sRGB is a subset of
// both targets, so the conversion never clips.
//
// The matrices are DERIVED from primary chromaticities at runtime instead of
// hand-typing 7-digit constants from a table: build each space's RGB→XYZ from
// its primaries + white point (all three spaces share the D65 white, so no
// chromatic adaptation is involved), then sRGB→target = inv(M_target)·M_srgb.
// The white-preservation unit test pins the derivation end to end.

/// Output color space for exports. `Srgb` is the pipeline's native space
/// (identity). Display P3 uses the sRGB transfer curve on P3-D65 primaries;
/// Adobe RGB (1998) uses its pure 563/256 gamma on its own primaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportColorSpace {
    #[default]
    Srgb,
    DisplayP3,
    AdobeRgb,
}

/// CIE xy chromaticities (D65 white shared by all three spaces).
pub(super) const D65_XY: [f32; 2] = [0.3127, 0.3290];
pub(super) const SRGB_PRIM: [[f32; 2]; 3] = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
pub(super) const P3_PRIM: [[f32; 2]; 3] = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];
pub(super) const ADOBE_PRIM: [[f32; 2]; 3] = [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]];
/// Adobe RGB (1998) transfer gamma, exact per the spec (= 2.19921875, a
/// dyadic rational that f32 represents exactly).
const ADOBE_GAMMA: f32 = 563.0 / 256.0;

/// A row-major 3×3 read out of a flat slice, or `None` when the slice is short.
///
/// Two readers of camera calibration build this same array: the RAW's own
/// `ColorMatrix` ([`camera_matrix`]) and Adobe's `.dcp` ([`crate::dcp`]). The
/// unpacking is the only thing they share — what counts as a USABLE matrix
/// differs (the RAW's is judged by [`validate_calibration`] against the space
/// it will be inverted into; the profile's is judged on its own finiteness), so
/// this answers the shape question and leaves the judgement to each caller.
pub(crate) fn mat3_from_slice(v: &[f32]) -> Option<[[f32; 3]; 3]> {
    let v = v.get(..9)?;
    let mut m = [[0.0f32; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = v[i * 3 + j];
        }
    }
    Some(m)
}

pub(super) fn mat_vec3(m: &[[f32; 3]; 3], v: &[f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

pub(super) fn mat_mul3(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

/// 3×3 inverse by adjugate / determinant, no conditioning guard of its own.
/// The built-in primaries matrices are far from singular (their determinants
/// are the gamut volumes); the FILE-SUPPLIED camera matrix that also flows
/// through here is validated by [`validate_calibration`] before the render
/// path calls in, and the metadata-only `as_shot_wb` path catches a
/// non-finite inverse behind its own is_finite gate (L04-1).
pub(super) fn inv3(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let c00 = m[1][1] * m[2][2] - m[1][2] * m[2][1];
    let c01 = m[1][2] * m[2][0] - m[1][0] * m[2][2];
    let c02 = m[1][0] * m[2][1] - m[1][1] * m[2][0];
    let det = m[0][0] * c00 + m[0][1] * c01 + m[0][2] * c02;
    let d = 1.0 / det;
    [
        [c00 * d, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d],
        [c01 * d, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d],
        [c02 * d, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d],
    ]
}

/// RGB→XYZ from primary + white chromaticities (textbook derivation: primary
/// XYZ columns at Y=1, scaled so R=G=B=1 lands exactly on the white point).
pub(super) fn rgb_to_xyz(prim: [[f32; 2]; 3], white: [f32; 2]) -> [[f32; 3]; 3] {
    let col = |p: [f32; 2]| [p[0] / p[1], 1.0, (1.0 - p[0] - p[1]) / p[1]];
    let (r, g, b) = (col(prim[0]), col(prim[1]), col(prim[2]));
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let s = mat_vec3(&inv3(&m), &col(white));
    [
        [m[0][0] * s[0], m[0][1] * s[1], m[0][2] * s[2]],
        [m[1][0] * s[0], m[1][1] * s[1], m[1][2] * s[2]],
        [m[2][0] * s[0], m[2][1] * s[1], m[2][2] * s[2]],
    ]
}

/// Linear-light sRGB → linear-light target primaries. `None` for sRGB itself.
pub(super) fn srgb_to_space_matrix(space: ExportColorSpace) -> Option<[[f32; 3]; 3]> {
    let m_srgb = rgb_to_xyz(SRGB_PRIM, D65_XY);
    match space {
        ExportColorSpace::Srgb => None,
        ExportColorSpace::DisplayP3 => Some(mat_mul3(&inv3(&rgb_to_xyz(P3_PRIM, D65_XY)), &m_srgb)),
        ExportColorSpace::AdobeRgb => Some(mat_mul3(&inv3(&rgb_to_xyz(ADOBE_PRIM, D65_XY)), &m_srgb)),
    }
}

/// Convert a rendered (sRGB-encoded) image into the requested delivery space:
/// decode the sRGB TRC → change primaries in linear light → encode the
/// target's TRC (P3 shares sRGB's curve; Adobe RGB is a pure 563/256 gamma).
/// 16-bit throughout; takes the image BY VALUE so the sRGB identity and the
/// already-16-bit export path move instead of cloning a ~366 MB frame.
/// The u16 input makes the decode a 65536-entry EXACT table (the same
/// function precomputed per representable input — bit-identical, zero
/// interpolation); the encode keeps its exact powf, and rows run in parallel.
pub fn convert_export_color_space(img: DynamicImage, space: ExportColorSpace) -> DynamicImage {
    let Some(m) = srgb_to_space_matrix(space) else {
        return img;
    };
    let mut rgb = match img {
        DynamicImage::ImageRgb16(b) => b,
        other => other.to_rgb16(),
    };
    let dec: Vec<f32> = (0..=65535u32).map(|v| srgb_to_linear(v as f32 / 65535.0)).collect();
    let buf: &mut [u16] = &mut rgb;
    buf.par_chunks_mut(3).for_each(|px| {
        let lin = [dec[px[0] as usize], dec[px[1] as usize], dec[px[2] as usize]];
        let t = mat_vec3(&m, &lin);
        let enc = |c: f32| -> u16 {
            let c = c.clamp(0.0, 1.0);
            let e = match space {
                ExportColorSpace::AdobeRgb => c.powf(1.0 / ADOBE_GAMMA),
                _ => linear_to_srgb(c),
            };
            (e.clamp(0.0, 1.0) * 65535.0).round() as u16
        };
        px[0] = enc(t[0]);
        px[1] = enc(t[1]);
        px[2] = enc(t[2]);
    });
    DynamicImage::ImageRgb16(rgb)
}

/// An Adobe RGB deliverable developed NATIVELY in Adobe primaries still
/// carries the working sRGB transfer — swap the per-channel TRANSFER only
/// (no primary change): decode the sRGB TRC, encode the pure 563/256 gamma.
/// Same exact-table scheme as `convert_export_color_space`.
pub(super) fn transcode_srgb_trc_to_adobe(img: DynamicImage) -> DynamicImage {
    let mut rgb = match img {
        DynamicImage::ImageRgb16(b) => b,
        other => other.to_rgb16(),
    };
    let lut: Vec<u16> = (0..=65535u32)
        .map(|v| {
            let lin = srgb_to_linear(v as f32 / 65535.0).clamp(0.0, 1.0);
            (lin.powf(1.0 / ADOBE_GAMMA) * 65535.0).round() as u16
        })
        .collect();
    let buf: &mut [u16] = &mut rgb;
    buf.par_iter_mut().for_each(|v| *v = lut[*v as usize]);
    DynamicImage::ImageRgb16(rgb)
}

/// Compact v2 ICC profiles embedded in exports — an UNTAGGED file makes
/// wide-gamut displays guess (typically stretching colors to the panel gamut).
/// All three from saucecontrol/Compact-ICC-Profiles, licensed CC0-1.0 (public
/// domain, repo license verified) — redistribution in this public repo is fine.
/// `acsp` signature + header size field validated at download time.
/// `SRGB_ICC` also tags the stack, heal and clone masters and a baked
/// source's 16-bit denoise master ([`write_working_space`]), and the reader
/// takes it as the working space itself (`decode::apply_icc_profile`).
pub(crate) const SRGB_ICC: &[u8] = include_bytes!("../../assets/sRGB-v2-magic.icc");
pub(super) const DISPLAY_P3_ICC: &[u8] = include_bytes!("../../assets/DisplayP3-v2-magic.icc");
pub(super) const ADOBE_RGB_ICC: &[u8] = include_bytes!("../../assets/AdobeCompat-v2.icc");

fn staging_path(out: &Path) -> std::path::PathBuf {
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("");
    out.with_extension(format!(
        "{ext}.tmp.{}.{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ))
}

fn publish_staged(out: &Path, staged: &Path, written: Result<()>) -> Result<()> {
    if let Err(error) = written {
        let _ = std::fs::remove_file(staged);
        return Err(error);
    }
    // durable_replace, not bare rename (L03): staged bytes + dir entry are
    // fsynced around the rename — pixels.json commits durably, so the
    // master it names must not be able to vanish with the page cache.
    if let Err(error) = crate::store::durable_replace(staged, out) {
        let _ = std::fs::remove_file(staged);
        return Err(error).with_context(|| format!("publish {}", out.display()));
    }
    Ok(())
}

pub fn stage_and_publish(
    out: &Path,
    write: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let staged = staging_path(out);
    let written = write(&staged);
    publish_staged(out, &staged, written)
}

/// Tag an encoder's output with the export space's profile. Never fails on
/// jpeg/png/tiff in image 0.25 (their `set_icc_profile` impls store the
/// profile unconditionally — verified in the crate source); if a future
/// version regresses, the pixels are still correctly encoded, just untagged —
/// so warn instead of failing the whole export.
///
/// `diag` is the export's diagnostics channel, and it exists for ONE reason:
/// the warning below is ungated and used to land on a shared stderr that a
/// parallel `batch` interleaved in completion order (R28 Batch-5 5c stamped it;
/// R29-1 routes it). Threaded as a parameter rather than read from anywhere,
/// because nothing here knows a photo — that is the honest shape of a leaf.
fn tag_icc<E: ImageEncoder>(
    enc: &mut E,
    space: ExportColorSpace,
    diag: &crate::diag::Diag<'_>,
) {
    let profile = match space {
        ExportColorSpace::Srgb => SRGB_ICC,
        ExportColorSpace::DisplayP3 => DISPLAY_P3_ICC,
        ExportColorSpace::AdobeRgb => ADOBE_RGB_ICC,
    };
    if let Err(e) = enc.set_icc_profile(profile.to_vec()) {
        diag.warn(format!("could not embed the {space:?} ICC profile: {e:?}"));
    }
}

/// Encode WORKING-SPACE pixels, this pipeline's sRGB, to `path` as `fmt`,
/// tagged with the engine's sRGB profile wherever the format carries one
/// (JPEG, PNG, TIFF), as an sRGB export is ([`tag_icc`]); another format is
/// written untagged. The encoders are the ones `save_with_format` picks, so
/// the file differs from an untagged write by the tag alone.
///
/// The one encoder for the pixel masters (`pipeline::save_master`: stack,
/// heal, clone) and for the deep working copy a baked denoise hands its
/// sidecar, whose tag the product inherits (`denoise::denoise_active`). The
/// reader takes this tag as the working space and keeps the numbers as they
/// are (`decode::apply_icc_profile`). Written untagged, a 16-bit master
/// re-read as "16-bit but carries no ICC profile" (the warning meant for an
/// editor's ProPhoto export that lost its tag) and any other editor had to
/// guess its space (2026-09-26).
pub(crate) fn write_working_space(path: &Path, img: &DynamicImage, fmt: image::ImageFormat) -> Result<()> {
    use std::io::Write as _;
    // `set_icc_profile` cannot fail on these three encoders in image 0.25
    // (see `tag_icc`); if it ever does, the write fails rather than publish
    // an untagged master.
    fn tagged(img: &DynamicImage, mut enc: impl ImageEncoder) -> image::ImageResult<()> {
        enc.set_icc_profile(SRGB_ICC.to_vec()).map_err(image::ImageError::Unsupported)?;
        img.write_with_encoder(enc)
    }
    let file = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut wr = std::io::BufWriter::new(file);
    match fmt {
        image::ImageFormat::Jpeg => tagged(img, image::codecs::jpeg::JpegEncoder::new(&mut wr)),
        image::ImageFormat::Png => tagged(img, image::codecs::png::PngEncoder::new(&mut wr)),
        image::ImageFormat::Tiff => tagged(img, image::codecs::tiff::TiffEncoder::new(&mut wr)),
        _ => img.write_to(&mut wr, fmt),
    }
    .with_context(|| format!("encode {}", path.display()))?;
    // Flushed explicitly: BufWriter's drop-time flush swallows its error.
    wr.flush().with_context(|| format!("flush {}", path.display()))
}

/// Render and save to `out` at the highest fidelity the format allows:
/// `.tif`/`.png` keep the full **16-bit** depth; `.jpg` downconverts to 8-bit.
/// Every export is transformed into and TAGGED with the selected delivery
/// color space (sRGB by default — see [`ExportColorSpace`] / [`tag_icc`]).
/// Extension picks the format. Dispatches RAW (demosaic engine) vs baked
/// image (the PNG-source engine) automatically. `export` adds the delivery
/// pipeline (resize / output sharpen / JPEG quality / color space); `None` =
/// full-res q95 sRGB as always. Returns the SAVED dimensions (post-resize).
///
/// `sink` is where this export's disclosures go (R29-1) — the clamp summary,
/// the ICC tagging failure, the mask-raster budget refusals. Bound to
/// `src_path` here, so the identity cannot disagree with the file being
/// rendered; `batch` passes a [`crate::diag::Collector`] and prints the result
/// in its own order.
pub fn render_to_file(
    src_path: &Path,
    recipe: &EditRecipe,
    out: &Path,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    export: Option<&ExportOpts>,
    sink: &dyn crate::diag::Sink,
) -> Result<(u32, u32)> {
    render_to_file_marked(src_path, recipe, out, denoise, export, sink, None, BasePart::of(recipe).colour_nr(), None)
}

/// A MASTER of `raw` for `part`, rendered to `out` through
/// [`BasePart::master_recipe`]: the full-resolution AI denoise's master. Its
/// develop runs the part's colour noise reduction ([`BasePart::colour_nr`]),
/// the stage the export of the photo the part is taken from runs, and the
/// master says so in its file ([`BakedColourNr`]) so that its card runs no
/// second one — when, and only when, the develop reports that the stage ran
/// (a mosaic it reads, [`render_to_file_marked`]).
pub fn render_master_to_file(
    raw: &Path,
    part: &BasePart,
    out: &Path,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    sink: &dyn crate::diag::Sink,
) -> Result<(u32, u32)> {
    let nr = BakedColourNr::of(part.colour_nr()).filter(|_| crate::decode::is_raw(raw));
    render_to_file_marked(raw, &part.master_recipe(), out, denoise, None, sink, None, part.colour_nr(), nr.as_ref())
}

/// [`render_to_file`], stamping `marker` on the staged file before the
/// publish when it is given. A RAW develops at colour NR `colour_nr` (the
/// part's, [`BasePart::colour_nr`]); `nr_mark` is stamped beside `marker`
/// (a master of a RAW baked with colour NR on, [`render_master_to_file`])
/// only when the develop reports that the linear colour stage ran: a RAW
/// whose mosaic it cannot read took the display-frame pass, and its master
/// must keep that pass on its card.
#[allow(clippy::too_many_arguments)] // the export's frame plus the part's one switch
fn render_to_file_marked(
    src_path: &Path,
    recipe: &EditRecipe,
    out: &Path,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    export: Option<&ExportOpts>,
    sink: &dyn crate::diag::Sink,
    marker: Option<&BakedGeometry>,
    colour_nr: f32,
    nr_mark: Option<&BakedColourNr>,
) -> Result<(u32, u32)> {
    let diag = crate::diag::Diag::about(sink, src_path);
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose(&diag);
    let recipe = &*validated;
    let opts = export.copied().unwrap_or_default();

    let ext = out
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // The gamut transform only runs for formats that can carry the matching
    // profile: pixels re-encoded for P3/AdobeRGB but saved UNTAGGED would
    // display wrong everywhere — sRGB is the only space safe to leave untagged.
    let taggable = matches!(ext.as_str(), "jpg" | "jpeg" | "jfif" | "tif" | "tiff" | "png");
    let space = if taggable { opts.color_space } else { ExportColorSpace::Srgb };
    let is_raw_src = crate::decode::is_raw(src_path);
    // RAW + wide delivery develops DIRECTLY in the delivery primaries — the
    // only route that carries camera colours beyond sRGB into the file (the
    // sRGB working develop gamut-clips at decode, making the conversion
    // below a relabelling that can never ADD colour). A baked source IS
    // sRGB pixels — for it the conversion below is complete by construction.
    let native_wide = is_raw_src && space != ExportColorSpace::Srgb;
    let (mut img, linear_colour) = if is_raw_src {
        let working = if native_wide { space } else { ExportColorSpace::Srgb };
        render_frame(src_path, recipe, denoise, None, working, sink, colour_nr)?
    } else {
        // The frame the file is in (v1.17.0): a master marked lens-corrected
        // takes no second resample, and a recipe that develops other lens
        // geometry than the one in its pixels is refused.
        let frame = SourceFrame::of(src_path)?;
        frame.check(recipe)?;
        // baked-by-construction: the !is_raw_src arm (decided just above).
        let src = crate::decode::load_image_for_develop(src_path)?;
        // `None`: this is the DELIVERY render. `opts.long_edge` below resizes
        // the finished pixels, which is not the same thing as developing at a
        // bounded working resolution and must not be quietly swapped for it —
        // the RAW arm above passes `None` for exactly the same reason.
        (render_baked_to_image(&src, recipe, denoise, None, frame.baked(), frame.colour_nr_in_pixels(), &diag)?, false)
    };
    let nr_mark = nr_mark.filter(|_| linear_colour);
    if let Some(le) = opts.long_edge
        && le > 0
        && img.width().max(img.height()) > le
    {
        // resize() fits within the box while keeping aspect → long edge == le.
        img = img.resize(le, le, image::imageops::FilterType::Lanczos3);
    }
    if opts.sharpen > 0.0 {
        // Same luma-unsharp the develop uses, run on the delivery-size pixels.
        // The 16-bit export path MOVES its buffer here (no to_rgb16 clone).
        let rgb = match img {
            DynamicImage::ImageRgb16(b) => b,
            other => other.to_rgb16(),
        };
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);
        let mut data: Vec<[f32; 3]> = rgb
            .as_raw()
            .par_chunks(3)
            .map(|p| [p[0] as f32 / 65535.0, p[1] as f32 / 65535.0, p[2] as f32 / 65535.0])
            .collect();
        // `data` carries the pixels now — the u16 source (~366 MB at 61 MP)
        // must not sit under the unsharp + repack below (A7).
        drop(rgb);
        unsharp_luma(&mut data, w, h, 1, (opts.sharpen / 100.0).clamp(0.0, 1.0), false);
        let mut buf: Vec<u16> = vec![0u16; w * h * 3];
        buf.par_chunks_mut(3).zip(data.par_iter()).for_each(|(o, px)| {
            for c in 0..3 {
                o[c] = (px[c].clamp(0.0, 1.0) * 65535.0).round() as u16;
            }
        });
        img = DynamicImage::ImageRgb16(
            ImageBuffer::from_raw(w as u32, h as u32, buf).expect("sharpen buffer size matches"),
        );
    }
    let (w, h) = (img.width(), img.height());
    if space != ExportColorSpace::Srgb {
        if native_wide {
            // Already IN the delivery primaries. Adobe RGB still swaps to
            // its own transfer; P3's native transfer IS the sRGB curve.
            if space == ExportColorSpace::AdobeRgb {
                img = transcode_srgb_trc_to_adobe(img);
            }
        } else {
            img = convert_export_color_space(img, space);
        }
    }
    // STAGE, then publish. `File::create` truncates the delivery path, so an
    // encode that failed half-way (disk full, a killed process) left a partial
    // file sitting at the name the user was told to hand over, and a repeat
    // export destroyed the previous deliverable before knowing the new one
    // would even encode. Every other artifact in this app stages and renames;
    // the one the photographer actually delivers was the exception.
    let staged = staging_path(out);
    let create = |p: &Path| {
        std::fs::File::create(p)
            .map(std::io::BufWriter::new)
            .with_context(|| format!("create {}", p.display()))
    };
    // The encode writes to `staged`; `out` stays untouched until it succeeds.
    let encoded = (|| -> Result<()> {
    // Every buffered arm FLUSHES explicitly before success: BufWriter's
    // drop-time flush SWALLOWS its error, so a full disk could report a
    // successful export over a truncated file.
    use std::io::Write as _;
    match ext.as_str() {
        // "jfif" belongs HERE: `ImageFormat::from_extension` maps it to Jpeg,
        // so it used to fall through to the generic arm, which constructs the
        // encoder with the library's default quality and silently ignored
        // opts.jpeg_quality — an export typed as out.jfif came out at 75 no
        // matter what the Export panel said. (Before the generic arm existed
        // it failed loudly, which was at least honest.)
        "jpg" | "jpeg" | "jfif" => {
            // JPEG is 8-bit only — downconvert from 16-bit.
            let rgb8 = img.to_rgb8();
            let mut wr = create(&staged)?;
            let mut enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut wr, opts.jpeg_quality.clamp(1, 100));
            tag_icc(&mut enc, space, &diag);
            enc.write_image(rgb8.as_raw(), rgb8.width(), rgb8.height(), image::ExtendedColorType::Rgb8)
                .with_context(|| format!("encode jpeg {}", out.display()))?;
            wr.flush().with_context(|| format!("flush {}", out.display()))?;
        }
        "tif" | "tiff" => {
            let mut wr = create(&staged)?;
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut wr);
            tag_icc(&mut enc, space, &diag);
            // 8-bit on request (阶段4): the depth is an EXPORT SETTING, not
            // an extension property — a .tif says nothing about bits.
            if opts.eight_bit {
                DynamicImage::ImageRgb8(img.to_rgb8())
                    .write_with_encoder(enc)
                    .with_context(|| format!("encode tiff {}", out.display()))?;
            } else {
                img.write_with_encoder(enc)
                    .with_context(|| format!("encode tiff {}", out.display()))?;
            }
            wr.flush().with_context(|| format!("flush {}", out.display()))?;
        }
        "png" => {
            let mut wr = create(&staged)?;
            let mut enc = image::codecs::png::PngEncoder::new(&mut wr);
            tag_icc(&mut enc, space, &diag);
            if opts.eight_bit {
                DynamicImage::ImageRgb8(img.to_rgb8())
                    .write_with_encoder(enc)
                    .with_context(|| format!("encode png {}", out.display()))?;
            } else {
                img.write_with_encoder(enc)
                    .with_context(|| format!("encode png {}", out.display()))?;
            }
            wr.flush().with_context(|| format!("flush {}", out.display()))?;
        }
        // Unknown extensions keep the generic 16-bit save (no ICC tag, so the
        // pixels above were deliberately left in sRGB). `save` infers the
        // format from the EXTENSION, and the staged name ends in the temp
        // sequence number — so the format has to come from the real target
        // instead, or every such export failed with "the file extension `.7`
        // was not recognized" naming a path the user never typed (R12).
        _ => {
            let fmt = image::ImageFormat::from_path(out)
                .with_context(|| format!("unsupported output format {}", out.display()))?;
            img.save_with_format(&staged, fmt)
                .with_context(|| format!("save render {}", out.display()))?
        }
    }
        Ok(())
    })();
    let encoded = encoded.and_then(|()| marker.map_or(Ok(()), |g| stamp_baked_geometry(&staged, g)));
    let encoded = encoded.and_then(|()| nr_mark.map_or(Ok(()), |m| stamp_colour_nr(&staged, m)));
    publish_staged(out, &staged, encoded)?;

    Ok((w, h))
}
