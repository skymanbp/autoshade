//! Render engine v1 — apply an [`EditRecipe`] to the full-resolution RAW and
//! produce a developed image (no Lightroom needed).
//!
//! Pipeline: `rawler` demosaics + colour-calibrates the sensor data to a
//! full-res sRGB-gamma float image (`RawDevelop::develop_intermediate`), then we
//! apply the recipe. The tonal ops (exposure, contrast, whites/blacks,
//! highlights/shadows, tone curve) are all 1-D functions of a channel value, so
//! they collapse into a single per-channel lookup table; saturation/vibrance run
//! per pixel; then orientation + crop.
//!
//! HONEST SCOPE: these ops are tasteful **approximations**, not bit-exact
//! Lightroom — clarity/sharpening are luma unsharp masks, noise reduction is a
//! bilateral-lite, dehaze is a pointwise scattering inversion (see
//! [`apply_dehaze`]). LOCAL-mask clarity/dehaze/texture ARE engine-rendered
//! since R22 (local temperature/tint since batch #2-B) — see [`apply_masks`]
//! for the pass order and the two documented residues vs the global chain.
//! Local `texture` has no global counterpart to align with, so its radius is
//! our own calibration.

use std::borrow::Cow;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use image::{DynamicImage, GenericImageView, ImageBuffer, ImageEncoder, Rgb, RgbImage};
use rawler::decoders::RawDecodeParams;
use rawler::get_decoder;
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::rawsource::RawSource;
use rawler::Orientation;
use rayon::prelude::*;

use crate::recipe::{Crop, EditRecipe, MaskGeometry, RangeMask};

const LUT_N: usize = 4096;
const MASK_RASTER_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// Shared, parameter-free transfer-curve LUTs: `[0]` = sRGB→linear, `[1]` =
/// linear→sRGB. Built once per process (OnceLock). Dehaze and vignette used to
/// evaluate both `powf` curves for every pixel — the identical 6-powf/px
/// pattern the v0.8.1 colour-gain fix removed (measured there: 609 ms vs 53 ms
/// per 1280×853 frame). 4096-entry linear interpolation keeps the error in the
/// same sub-8-bit-quantisation envelope `colour_gain_luts` pinned with a test;
/// dehaze's airlight HISTOGRAM keeps the exact powf (a 1e-5 shift across a bin
/// edge could move the whole-frame airlight estimate — not worth the few ms).
fn transfer_luts() -> &'static ([f32; LUT_N], [f32; LUT_N]) {
    use std::sync::OnceLock;
    static LUTS: OnceLock<([f32; LUT_N], [f32; LUT_N])> = OnceLock::new();
    LUTS.get_or_init(|| {
        let mut dec = [0.0f32; LUT_N];
        let mut enc = [0.0f32; LUT_N];
        for i in 0..LUT_N {
            let x = i as f32 / (LUT_N - 1) as f32;
            dec[i] = srgb_to_linear(x);
            enc[i] = linear_to_srgb(x);
        }
        (dec, enc)
    })
}

/// The ONE raw-vs-baked dispatch for "give me this source's pixels, neutrally".
///
/// A camera RAW has no `image`-crate decoder and a baked raster has no
/// demosaic, so every consumer of *source pixels* needs the same two-armed
/// branch — and it was hand-copied at six call sites, one of which (the GUI's
/// v0.22 mask-refine worker) simply forgot it and fed a .ARW to
/// [`crate::decode::load_image`]. This is that branch, once: RAW →
/// [`render_to_image`] with a NEUTRAL recipe (the engine's own develop, never
/// the camera's baked 8-bit preview — see `retouch::heal`); baked →
/// `decode::load_image` (which applies the EXIF orientation).
///
/// `cap` bounds the LONG EDGE. The RAW arm develops AT that edge (the cap runs
/// before tone/geometry, so a preview-size caller never pays a 61 MP develop);
/// a baked source is thumbnailed to it and only ever DOWN — plain `thumbnail`
/// UPSCALES a smaller source, which would inflate a small image instead of
/// bounding a large one. `None` = the source's own full resolution.
pub fn source_pixels(path: &Path, cap: Option<u32>) -> Result<DynamicImage> {
    if crate::decode::is_raw(path) {
        return render_to_image(path, &EditRecipe::default(), None, cap);
    }
    // baked-by-construction: the !is_raw arm of THE dispatch itself.
    let img = crate::decode::load_image(path)?;
    match cap {
        Some(edge) if img.width().max(img.height()) > edge => Ok(img.thumbnail(edge, edge)),
        _ => Ok(img),
    }
}

/// Develop `raw_path` and apply `recipe`, returning the finished image. When
/// `denoise` is set, the demosaiced buffer is AI-denoised (via the Python
/// sidecar) before any tonal/colour work — i.e. denoise-before-sharpen.
/// `max_edge`: develop at a bounded working resolution — the oriented f32
/// buffer is downscaled right after demosaic, BEFORE denoise/tone/geometry,
/// so a preview-resolution caller (retouch base, web preview, base-look
/// estimation) stops paying a 61 MP develop + 16-bit pack + geometry chain
/// only to thumbnail the result at the end. `None` = full resolution (export).
pub fn render_to_image(
    raw_path: &Path,
    recipe: &EditRecipe,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    max_edge: Option<u32>,
) -> Result<DynamicImage> {
    render_to_image_in(raw_path, recipe, denoise, max_edge, ExportColorSpace::Srgb)
}

/// [`render_to_image`] with a chosen WORKING space. `Srgb` is the exact
/// historical pipeline (rawler's own calibrated develop, byte-identical). A
/// wide space develops DIRECTLY in the delivery primaries: rawler's own
/// calibrate gamut-clips at the sRGB boundary (its `map_3ch_to_rgb` ends in
/// a negative clip, where every colour outside sRGB dies — verified in the
/// 0.7.2 source), so the wide path runs the develop WITHOUT
/// WhiteBalance/Calibrate/SRgb and performs the DNG-spec calibration
/// itself, into the delivery primaries, with no gamut clip. The working
/// ENCODING stays the sRGB transfer in every space (shared D65 white +
/// shared transfer → the neutral axis renders identically to sRGB), and
/// colours outside the DELIVERY gamut clip only at the final 16-bit pack —
/// the honest boundary of the chosen deliverable.
pub fn render_to_image_in(
    raw_path: &Path,
    recipe: &EditRecipe,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    max_edge: Option<u32>,
    working: ExportColorSpace,
) -> Result<DynamicImage> {
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose();
    let recipe = &*validated;
    let rasters = load_mask_raster_snapshot(recipe)?;
    // Decode scope: the RawSource holds the entire RAW file in memory
    // (~60–120 MB for a 61 MP lossless ARW), and neither it nor the decoder
    // outlives the sensor read — so the file bytes drop HERE instead of
    // sitting under the whole ~720 MB-per-plane develop chain below (A7
    // buffer-lifetime queue).
    crate::decode::guard_tiff_chain(raw_path)?;
    let rawimage = {
        let src = RawSource::new(raw_path)
            .with_context(|| format!("open RAW {}", raw_path.display()))?;
        let decoder =
            get_decoder(&src).map_err(|e| anyhow!("no decoder for {}: {e}", raw_path.display()))?;
        let params = RawDecodeParams { image_index: 0 };
        // Full sensor data (dummy = false) → demosaic + colour pipeline → float.
        decoder
            .raw_image(&src, &params, false)
            .map_err(|e| anyhow!("raw_image: {e}"))?
    };
    let orientation = rawimage.orientation;

    let wide = working != ExportColorSpace::Srgb;
    let mut dev = RawDevelop::default();
    if wide {
        dev.steps.retain(|s| {
            !matches!(
                s,
                ProcessingStep::WhiteBalance | ProcessingStep::Calibrate | ProcessingStep::SRgb
            )
        });
    }
    // Calibration metadata is validated BEFORE any pixel work (L04-1): a
    // singular/non-finite ColorMatrix or a corrupt AsShotNeutral (a zero
    // component reaches here as an INFINITE coefficient — rawler's dng.rs
    // builds wb as 1/levels) used to sail through, and `to_u16` then
    // saturated the damage into a silently published all-black or all-white
    // deliverable. Refuse-not-degrade: the render errors with the cause and
    // no file is staged. The sRGB path validates too — rawler's own develop
    // carries the identical `[0].is_nan()` blind spot — but only when a
    // matrix is PRESENT: absence is the normal matrix-less-camera case and
    // rawler then skips calibration entirely.
    let calibration = if wide {
        let xyz2cam = camera_matrix(&rawimage)?;
        validate_calibration(&xyz2cam, rawimage.wb_coeffs, working, raw_path)?;
        Some((xyz2cam, normalise_wb(rawimage.wb_coeffs)))
    } else {
        if rawimage.color_matrix.iter().next().is_some() {
            let xyz2cam = camera_matrix(&rawimage)?;
            // rawler's own develop targets sRGB — validate the matrix it
            // effectively inverts.
            validate_calibration(
                &xyz2cam,
                rawimage.wb_coeffs,
                ExportColorSpace::Srgb,
                raw_path,
            )?;
        }
        None
    };
    let inter = dev
        .develop_intermediate(&rawimage)
        .map_err(|e| anyhow!("develop: {e}"))?;
    // The demosaiced float frame owns everything the pipeline needs from here
    // on; the ~120 MB u16 sensor mosaic would otherwise survive to the end of
    // the function, under denoise/tone/pack/geometry (A7).
    drop(rawimage);
    let rgb = match inter {
        Intermediate::ThreeColor(c) => c,
        Intermediate::Monochrome(_) => bail!("monochrome RAW not supported by render v1"),
        Intermediate::FourColor(_) => bail!("4-colour develop output not supported by render v1"),
    };
    let (w, h) = (rgb.width, rgb.height);
    // sRGB path: sRGB-gamma ~[0,1] straight from rawler (owned, no copy).
    // Wide path: camera-native LINEAR until the calibrate below.
    let mut data: Vec<[f32; 3]> = rgb.data;
    if let Some((xyz2cam, wb)) = calibration {
        calibrate_camera_buffer(&mut data, &xyz2cam, wb, working);
    }
    let data = data;

    // --- EXIF orientation FIRST, so the whole pipeline works in the DISPLAY
    // frame. Masks / crop / straighten are all defined against what the user
    // sees (the C2 coordinate contract's "original" frame); orienting at the
    // end — as this pipeline once did — made portrait RAWs apply crop and
    // straighten in the wrong axis vs the un-oriented GUI preview (the decode
    // side now orients too, see decode.rs). Identity for landscape shots.
    //
    // The A7 queue asked whether the cap below could run BEFORE orientation
    // (a capped portrait render would then orient a preview-sized buffer
    // instead of paying a second ~720 MB full-res frame for the rotation).
    // Probed and REJECTED: `image::thumbnail`'s integer binning commutes
    // only with pure axis swaps (Normal/Transpose) — every orientation with
    // a REVERSAL component diverges by one source bin (mirrored bin edges
    // of a non-integer ratio don't line up; measured on a 97×61 frame,
    // edge 40). The first probe's 0.48 Transpose figure was contaminated
    // by the Rgba<u8> flip adapter (fixed in U14) — the corrected probe
    // still forbids the swap for six of eight states. The portrait-preview
    // rotation transient is the accepted price of preview pixels that
    // match the export path exactly.
    let (data, w, h) = orient_f32(data, w, h, orientation);
    // Working-resolution cap: downscale-then-develop, the same order the GUI
    // preview path uses — masks/sharpen/geometry are resolution-normalised.
    let (mut data, w, h) = match max_edge {
        Some(edge) => downscale_f32(data, w, h, edge),
        None => (data, w, h),
    };

    // --- AI denoise (opt-in) on the clean demosaiced pixels, before tone/sharpen
    if let Some(opts) = denoise {
        println!("AI denoise ({}) on {}x{} ...", opts.model, w, h);
        crate::denoise::denoise_buffer(opts, &mut data, w, h).context("AI denoise")?;
    }

    // --- white balance (target Kelvin/tint) in linear light -------------------
    apply_recipe_wb(&mut data, recipe);

    // --- tone + clarity + sat/vibrance + NR + sharpen (shared pipeline) -------
    apply_develop_with_rasters(&mut data, w, h, recipe, &rasters);

    // --- pack to 16-bit (highest precision; JPEG downconverts at encode) ------
    let mut buf: Vec<u16> = vec![0u16; w * h * 3];
    buf.par_chunks_mut(3).zip(data.par_iter()).for_each(|(o, px)| {
        o[0] = to_u16(px[0]);
        o[1] = to_u16(px[1]);
        o[2] = to_u16(px[2]);
    });
    let img: ImageBuffer<Rgb<u16>, _> = ImageBuffer::from_raw(w as u32, h as u32, buf)
        .ok_or_else(|| anyhow!("pixel buffer size mismatch"))?;
    // Orientation was applied BEFORE develop (see orient_f32 above), so the
    // buffer is already in the display frame — no tail rotation.
    let mut dynimg = DynamicImage::ImageRgb16(img);

    // --- lens geometry (profile distortion/CA + manual amount): radial
    // resample FIRST in the geometric chain (masks were applied above, in the
    // original frame). The map depends only on the radius normalised by the
    // half-diagonal, so it is orientation-invariant and identical between the
    // small preview and this full render.
    if recipe.lens_profile.geometry_active() || recipe.lens_distortion != 0.0 {
        dynimg = apply_lens_geometry(&dynimg, &recipe.lens_profile, recipe.lens_distortion);
    }

    // --- straighten: rotate + auto-crop BEFORE the user crop, in display
    // space (after orientation) so the slider means what the user sees. The
    // user crop below is therefore defined on the straightened frame — same
    // composition order as Lightroom's CropAngle + crop rect.
    if recipe.straighten_deg != 0.0 {
        dynimg = rotate_straighten(&dynimg, recipe.straighten_deg);
    }

    Ok(apply_crop(dynimg, recipe.crop.as_ref()))
}

/// The user crop — normalised [0,1] on the DISPLAYED frame, i.e. after
/// orientation, lens geometry and straighten. Shared by the RAW and the baked
/// paths so ONE rounding rule serves both (they must agree: the same recipe
/// exports the same rectangle whichever source it rides). A degenerate
/// rectangle is a no-op rather than a zero-size image.
fn apply_crop(img: DynamicImage, crop: Option<&Crop>) -> DynamicImage {
    let Some(c) = crop else { return img };
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let x = (c.left.clamp(0.0, 1.0) * iw).round() as u32;
    let y = (c.top.clamp(0.0, 1.0) * ih).round() as u32;
    let cw = ((c.right - c.left).clamp(0.0, 1.0) * iw).round() as u32;
    let ch = ((c.bottom - c.top).clamp(0.0, 1.0) * ih).round() as u32;
    if cw == 0 || ch == 0 {
        return img;
    }
    img.crop_imm(x, y, cw, ch)
}

/// Borrow an already-16-bit source, converting only when it is not one: the
/// export path arrives as `ImageRgb16`, where `to_rgb16()` would copy ~366 MB
/// at 61 MP (A7). Every resampler that reads 16-bit pixels goes through here.
fn rgb16_source(img: &DynamicImage) -> Cow<'_, ImageBuffer<Rgb<u16>, Vec<u16>>> {
    match img.as_rgb16() {
        Some(b) => Cow::Borrowed(b),
        None => Cow::Owned(img.to_rgb16()),
    }
}

/// Develop an already-baked image (the "PNG source" mode: edit an LR/PS-denoised
/// export). Runs the SAME pipeline as the RAW engine on the loaded pixels — no
/// demosaic, and white balance uses the same anchored shift the RAW path
/// uses — the anchor rides IN the recipe (`as_shot_k` when stamped, the
/// 5500 K default otherwise), so a baked sRGB image needs no raw WB
/// coefficients of its own.
/// Optional AI denoise runs first; output is 16-bit.
pub fn render_baked_to_image(
    img: &DynamicImage,
    recipe: &EditRecipe,
    denoise: Option<&crate::denoise::DenoiseOpts>,
) -> Result<DynamicImage> {
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose();
    let recipe = &*validated;
    let rasters = load_mask_raster_snapshot(recipe)?;
    let rgb = img.to_rgb16();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut data: Vec<[f32; 3]> = rgb
        .as_raw()
        .par_chunks(3)
        .map(|p| [p[0] as f32 / 65535.0, p[1] as f32 / 65535.0, p[2] as f32 / 65535.0])
        .collect();
    // The 16-bit staging copy (~366 MB at 61 MP) is fully transcribed into
    // `data` — freed here rather than after the whole develop below (A7).
    drop(rgb);

    if let Some(opts) = denoise {
        println!("AI denoise ({}) on {}x{} ...", opts.model, w, h);
        crate::denoise::denoise_buffer(opts, &mut data, w, h).context("AI denoise")?;
    }

    apply_recipe_wb(&mut data, recipe);
    apply_develop_with_rasters(&mut data, w, h, recipe, &rasters);

    let mut buf: Vec<u16> = vec![0u16; w * h * 3];
    buf.par_chunks_mut(3).zip(data.par_iter()).for_each(|(o, px)| {
        o[0] = to_u16(px[0]);
        o[1] = to_u16(px[1]);
        o[2] = to_u16(px[2]);
    });
    let out: ImageBuffer<Rgb<u16>, _> = ImageBuffer::from_raw(w as u32, h as u32, buf)
        .ok_or_else(|| anyhow!("baked pixel buffer size mismatch"))?;
    let mut dynimg = DynamicImage::ImageRgb16(out);

    // Lens geometry, then straighten, before the user crop — same order as
    // the RAW path (the geometric chain is original → corrected → view).
    if recipe.lens_profile.geometry_active() || recipe.lens_distortion != 0.0 {
        dynimg = apply_lens_geometry(&dynimg, &recipe.lens_profile, recipe.lens_distortion);
    }
    if recipe.straighten_deg != 0.0 {
        dynimg = rotate_straighten(&dynimg, recipe.straighten_deg);
    }

    // Orientation is already baked into the source here.
    Ok(apply_crop(dynimg, recipe.crop.as_ref()))
}

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
const D65_XY: [f32; 2] = [0.3127, 0.3290];
const SRGB_PRIM: [[f32; 2]; 3] = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
const P3_PRIM: [[f32; 2]; 3] = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];
const ADOBE_PRIM: [[f32; 2]; 3] = [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]];
/// Adobe RGB (1998) transfer gamma, exact per the spec (= 2.19921875, a
/// dyadic rational that f32 represents exactly).
const ADOBE_GAMMA: f32 = 563.0 / 256.0;

fn mat_vec3(m: &[[f32; 3]; 3], v: &[f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn mat_mul3(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
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
fn inv3(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
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
fn rgb_to_xyz(prim: [[f32; 2]; 3], white: [f32; 2]) -> [[f32; 3]; 3] {
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
fn srgb_to_space_matrix(space: ExportColorSpace) -> Option<[[f32; 3]; 3]> {
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

/// The camera's xyz→cam matrix (3-colour), by rawler's own selection rule:
/// the D65-illuminant matrix first, else whichever exists.
fn camera_matrix(rawimage: &rawler::RawImage) -> Result<[[f32; 3]; 3]> {
    let cm = rawimage
        .color_matrix
        .iter()
        .find(|(i, _)| **i == rawler::imgop::xyz::Illuminant::D65)
        .or_else(|| rawimage.color_matrix.iter().next())
        .map(|(_, m)| m)
        .ok_or_else(|| anyhow!("no camera colour matrix — the wide-gamut develop needs one"))?;
    if cm.len() < 9 {
        bail!("camera colour matrix has {} entries (need 9)", cm.len());
    }
    let mut xyz2cam = [[0.0f32; 3]; 3];
    for (i, row) in xyz2cam.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = cm[i * 3 + j];
        }
    }
    Ok(xyz2cam)
}

/// rawler's AsShotNeutral convention: `wb[0]` NaN ⇒ WB unknown ⇒ neutral
/// [1,1,1]. Anything else passes through for [`validate_calibration`] to
/// judge — an INFINITE coefficient (1/0 from a zero AsShotNeutral, rawler
/// dng.rs), a negative, or a partial NaN is corrupt metadata, not
/// "unknown", and the old `[0].is_nan()`-only guard let all three through.
fn normalise_wb(wb: [f32; 4]) -> [f32; 3] {
    if wb[0].is_nan() { [1.0, 1.0, 1.0] } else { [wb[0], wb[1], wb[2]] }
}

/// Validate the FILE-SUPPLIED calibration before it reaches [`inv3`] and
/// the pixel chain (L04-1). `to_u16` clamps silently — NaN quantises to 0,
/// inf saturates to 65535 — so a singular matrix or corrupt WB published a
/// committed all-black/all-white deliverable with `Ok` status. Every bail
/// names the file and the measured value; no output is written.
///
/// The conditioning check runs on the matrix the render ACTUALLY inverts
/// (Codex AL-review F3): `space2cam = xyz2cam × rgb_to_xyz(space)`, row-
/// normalised exactly as [`camera_to_space_matrix`] does — validating the
/// raw xyz2cam rows alone passed a matrix whose D65-weighted product row
/// sums to ~0 and divides into infinities at render time. The raw-row
/// degeneracy check is kept as well: no physical matrix has a zero raw row
/// sum, and refusing garbage early is the cheap direction.
fn validate_calibration(
    xyz2cam: &[[f32; 3]; 3],
    wb: [f32; 4],
    space: ExportColorSpace,
    src: &Path,
) -> Result<()> {
    for row in xyz2cam {
        for v in row {
            if !v.is_finite() {
                bail!(
                    "camera colour matrix of {} has a non-finite entry ({v}) — \
                     the file's calibration metadata is corrupt; no output was written",
                    src.display()
                );
            }
        }
    }
    let mut norm = mat_mul3(xyz2cam, &rgb_to_xyz(space_primaries(space), D65_XY));
    for (i, row) in norm.iter_mut().enumerate() {
        let raw = xyz2cam[i][0] + xyz2cam[i][1] + xyz2cam[i][2];
        let s = row[0] + row[1] + row[2];
        if raw.abs() <= 1e-6 || s.abs() <= 1e-6 {
            bail!(
                "camera colour matrix of {} has a degenerate row (raw sum {raw:e}, \
                 white-weighted sum {s:e}) — the file's calibration metadata is \
                 corrupt; no output was written",
                src.display()
            );
        }
        for v in row.iter_mut() {
            *v /= s;
        }
    }
    let c00 = norm[1][1] * norm[2][2] - norm[1][2] * norm[2][1];
    let c01 = norm[1][2] * norm[2][0] - norm[1][0] * norm[2][2];
    let c02 = norm[1][0] * norm[2][1] - norm[1][1] * norm[2][0];
    let det = norm[0][0] * c00 + norm[0][1] * c01 + norm[0][2] * c02;
    // Real camera matrices land at O(0.1–1) here (a sweep of rawler 0.7.2's
    // 1331 bundled matrices bottoms out around 0.22), so 1e-4 leaves ~3
    // decades of headroom.
    if det.abs() < 1e-4 {
        bail!(
            "camera colour matrix of {} is singular or near-singular \
             (row-normalised determinant {det:e}) — inverting it would render \
             the whole frame black; no output was written",
            src.display()
        );
    }
    let wb3 = normalise_wb(wb);
    for v in wb3 {
        if !v.is_finite() || v <= 0.0 {
            bail!(
                "AsShotNeutral/WB coefficients of {} are corrupt ({wb3:?}) — \
                 a zero AsShotNeutral component becomes an infinite multiplier \
                 (all-white frame); no output was written",
                src.display()
            );
        }
    }
    // A REAL fourth coefficient (rawler leaves wb[3] NaN for 3-channel
    // cameras) is consumed by rawler's own develop on 4-colour sensors —
    // validate it the same way (Codex AL-review F4, the shallow half; a
    // true 4-colour intermediate is refused post-develop as before).
    if !wb[3].is_nan() && (!wb[3].is_finite() || wb[3] <= 0.0) {
        bail!(
            "the fourth WB coefficient of {} is corrupt ({}) — \
             no output was written",
            src.display(),
            wb[3]
        );
    }
    Ok(())
}

/// Does the composed lens geometry MOVE the shared frame? THE predicate
/// every geometry consumer must use (Codex AL-review F1): distortion-off +
/// amount-0 is NOT identity once CA overshoots, because the composite fill
/// (L04-2) zooms every channel. Dims-free and conservatively OVER-inclusive
/// (any ca knot above 1 counts, whether or not the band max survives): a
/// false positive routes through maps that are then identity anyway; a
/// false negative desyncs masks/overlays/selections from the pixels.
pub fn geometry_moves_frame(profile: &crate::recipe::LensProfile, amount: f32) -> bool {
    (profile.distortion_on && !profile.distortion.is_empty())
        || amount.abs() >= 1e-3
        || (profile.ca_on
            && !profile.ca_r.is_empty()
            && !profile.ca_b.is_empty()
            && profile.ca_r.iter().chain(&profile.ca_b).any(|k| *k > 1.0))
}

/// The delivery space's primaries.
fn space_primaries(space: ExportColorSpace) -> [[f32; 2]; 3] {
    match space {
        ExportColorSpace::Srgb => SRGB_PRIM,
        ExportColorSpace::DisplayP3 => P3_PRIM,
        ExportColorSpace::AdobeRgb => ADOBE_PRIM,
    }
}

/// The camera's AS-SHOT white balance as absolute chromaticity: (CCT Kelvin,
/// tint in the recipe's ±100 scale). Metadata-only rawler decode (`dummy` —
/// no pixel data, no demosaic; wb_coeffs and the colour matrix come from the
/// file/camera definition either way, verified in rawler 0.7.2 arw.rs:184 +
/// rawimage.rs:390), then [`wb_to_kelvin_tint`]. `None` when the file is not
/// a RAW, has no colour matrix, or carries damaged coefficients — callers
/// keep the engine's historical 5500 K anchor.
pub fn as_shot_wb(raw_path: &Path) -> Option<(f32, f32)> {
    if !crate::decode::is_raw(raw_path) {
        return None;
    }
    // A cyclic-IFD file degrades to None here — the documented
    // no-metadata path (callers keep the historical 5500 K anchor).
    crate::decode::guard_tiff_chain(raw_path).ok()?;
    let src = RawSource::new(raw_path).ok()?;
    let decoder = get_decoder(&src).ok()?;
    let rawimage = decoder.raw_image(&src, &RawDecodeParams { image_index: 0 }, true).ok()?;
    let xyz2cam = camera_matrix(&rawimage).ok()?;
    let wb = rawimage.wb_coeffs;
    wb_to_kelvin_tint(&xyz2cam, [wb[0], wb[1], wb[2]])
}

/// (CCT, tint) of the scene illuminant implied by camera WB gains. The gains
/// NEUTRALISE the illuminant, so the illuminant's camera-space colour is
/// their reciprocal; through the camera matrix that becomes XYZ → (x, y) →
/// McCamy's cubic CCT approximation [verified: McCamy 1992,
/// n=(x−0.3320)/(0.1858−y), CCT=449n³+3525n²+6823.3n+5520.33 — reproduces
/// illuminant A at 2856 K and D65 at 6504 K, pinned by test] and a Duv-based
/// tint: the signed CIE-1960 distance from the Planckian locus (Krystek 1985
/// rational fits; above the locus = green), mapped at 3000 tint units per
/// Duv — the scale that lands D65 (Duv ≈ +0.0032) on ≈ +10, ACR's own
/// Daylight-preset tint, which pins both sign and magnitude. ACR's exact
/// model is proprietary — this is a documented approximation (the
/// `local_temp_to_kelvin` stance) used for anchoring and display, never for
/// pixel math.
pub(crate) fn wb_to_kelvin_tint(xyz2cam: &[[f32; 3]; 3], wb: [f32; 3]) -> Option<(f32, f32)> {
    if wb.iter().any(|c| !c.is_finite() || *c <= 0.0) {
        return None;
    }
    let neutral = [1.0 / wb[0], 1.0 / wb[1], 1.0 / wb[2]];
    let xyz = mat_vec3(&inv3(xyz2cam), &neutral);
    let sum = xyz[0] + xyz[1] + xyz[2];
    if !sum.is_finite() || sum <= 0.0 {
        return None;
    }
    let (x, y) = (xyz[0] / sum, xyz[1] / sum);
    let d = 0.1858 - y;
    if d.abs() < 1e-6 {
        return None; // McCamy's pole — no real illuminant lives there
    }
    let n = (x - 0.3320) / d;
    let cct = 449.0 * n * n * n + 3525.0 * n * n + 6823.3 * n + 5520.33;
    // McCamy + Krystek's mutual comfort zone (McCamy degrades toward the
    // extremes; Krystek's stated fit ends at 15000 K). Every WB a camera
    // plausibly meters — tungsten 2500 K through deep blue shade ~12000 K —
    // lives well inside. Outside it the METADATA is junk: refuse, keeping
    // the legacy unknown anchor, rather than stamp a wrong absolute label.
    if !cct.is_finite() || !(1667.0..=15_000.0).contains(&cct) {
        return None;
    }
    let (u, v) = uv1960(x, y);
    let (up, vp) = planck_uv1960(cct);
    let dist = ((u - up).powi(2) + (v - vp).powi(2)).sqrt();
    let duv = if v >= vp { dist } else { -dist };
    let tint = (duv * 3000.0).clamp(-100.0, 100.0);
    // Whole-Kelvin quantisation AT THE SOURCE: the XMP serialises integer
    // Kelvin, so a fractional anchor would round-trip as a target a fraction
    // off the anchor — a shift where none was intended (and below ~2500 K
    // one that escapes apply_wb's 1e-3 neutral short-circuit).
    Some((cct.clamp(2000.0, 40000.0).round(), tint))
}

/// CIE 1960 (u, v) from chromaticity (x, y) — the space Duv is defined in.
fn uv1960(x: f32, y: f32) -> (f32, f32) {
    let den = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / den, 6.0 * y / den)
}

/// Planckian locus in CIE 1960 (u, v): Krystek 1985 rational approximation
/// (stated fit range 1000–15000 K; beyond that it extrapolates smoothly —
/// acceptable for a tint DISPLAY value, and daylight lives well inside).
// The literals are Krystek's PUBLISHED coefficients verbatim — truncating to
// f32-representable digits parses to the same bits but breaks checkability
// against the source.
#[allow(clippy::excessive_precision)]
fn planck_uv1960(t: f32) -> (f32, f32) {
    let t2 = t * t;
    let u = (0.860_117_757 + 1.541_182_54e-4 * t + 1.286_412_12e-7 * t2)
        / (1.0 + 8.424_202_35e-4 * t + 7.081_451_63e-7 * t2);
    let v = (0.317_398_726 + 4.228_062_45e-5 * t + 4.204_816_91e-8 * t2)
        / (1.0 - 2.897_418_16e-5 * t + 1.614_560_53e-7 * t2);
    (u, v)
}

/// cam→space by the DNG white-preservation rule: space→cam =
/// xyz2cam · M(space→XYZ) with each row normalised to sum 1 (a
/// white-balanced grey then maps to the SAME grey in every space), inverted.
fn camera_to_space_matrix(xyz2cam: &[[f32; 3]; 3], space: ExportColorSpace) -> [[f32; 3]; 3] {
    let mut space2cam = mat_mul3(xyz2cam, &rgb_to_xyz(space_primaries(space), D65_XY));
    for row in &mut space2cam {
        // Unconditional (L04-1): the sole production caller is the
        // calibrate path, whose matrix `validate_calibration` has already
        // refused when any row is degenerate — so the old silent
        // `if s.abs() > 1e-6` skip, which quietly dropped the DNG
        // white-preservation rule for exactly the broken inputs, no longer
        // has a case to hide. (A zero row here would now divide to
        // inf/NaN — loud downstream — instead of passing un-normalised
        // as a plausible-but-wrong calibration.)
        let s = row[0] + row[1] + row[2];
        for v in row {
            *v /= s;
        }
    }
    inv3(&space2cam)
}

/// White-balance + calibrate a camera-native LINEAR buffer into `space`'s
/// primaries and encode the sRGB transfer — WITHOUT a gamut clip: a colour
/// outside sRGB but inside the delivery gamut is exactly what a wide-gamut
/// export exists to carry (rawler's own calibrate kills it). Components
/// outside the DELIVERY gamut go negative here and clip at the final
/// 16-bit pack. Highlights (any component > 1 after white balance) get the
/// same desaturating treatment rawler's develop applies — scale-to-max
/// averaged with the euclidean norm — so wide and sRGB renders treat blown
/// areas alike. The transfer's linear segment covers negatives (no NaN).
fn calibrate_camera_buffer(
    data: &mut [[f32; 3]],
    xyz2cam: &[[f32; 3]; 3],
    wb: [f32; 3],
    space: ExportColorSpace,
) {
    let m = camera_to_space_matrix(xyz2cam, space);
    data.par_iter_mut().for_each(|px| {
        let v = [px[0] * wb[0], px[1] * wb[1], px[2] * wb[2]];
        let mut t = mat_vec3(&m, &v);
        let max = t[0].max(t[1]).max(t[2]);
        if max > 1.0 {
            // Blown pixels take EXACTLY rawler's treatment, including its
            // negative pre-clip: keeping negatives inside this formula let
            // the positive euclidean term drag an out-of-gamut component
            // back into gamut with a hue shift. Unblown pixels (the branch
            // NOT taken) keep their negatives — the wide-gamut win lives
            // there.
            let t0 = t.map(|c| c.max(0.0));
            let eucl = ((t0[0] * t0[0] + t0[1] * t0[1] + t0[2] * t0[2]) / 3.0).sqrt();
            t = t0.map(|c| (c / max + eucl) / 2.0);
        }
        *px = [linear_to_srgb(t[0]), linear_to_srgb(t[1]), linear_to_srgb(t[2])];
    });
}

/// An Adobe RGB deliverable developed NATIVELY in Adobe primaries still
/// carries the working sRGB transfer — swap the per-channel TRANSFER only
/// (no primary change): decode the sRGB TRC, encode the pure 563/256 gamma.
/// Same exact-table scheme as `convert_export_color_space`.
fn transcode_srgb_trc_to_adobe(img: DynamicImage) -> DynamicImage {
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
const SRGB_ICC: &[u8] = include_bytes!("../assets/sRGB-v2-magic.icc");
const DISPLAY_P3_ICC: &[u8] = include_bytes!("../assets/DisplayP3-v2-magic.icc");
const ADOBE_RGB_ICC: &[u8] = include_bytes!("../assets/AdobeCompat-v2.icc");

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
fn tag_icc<E: ImageEncoder>(enc: &mut E, space: ExportColorSpace) {
    let profile = match space {
        ExportColorSpace::Srgb => SRGB_ICC,
        ExportColorSpace::DisplayP3 => DISPLAY_P3_ICC,
        ExportColorSpace::AdobeRgb => ADOBE_RGB_ICC,
    };
    if let Err(e) = enc.set_icc_profile(profile.to_vec()) {
        eprintln!("⚠ could not embed the {space:?} ICC profile: {e:?}");
    }
}

/// Render and save to `out` at the highest fidelity the format allows:
/// `.tif`/`.png` keep the full **16-bit** depth; `.jpg` downconverts to 8-bit.
/// Every export is transformed into and TAGGED with the selected delivery
/// color space (sRGB by default — see [`ExportColorSpace`] / [`tag_icc`]).
/// Extension picks the format. Dispatches RAW (demosaic engine) vs baked
/// image (the PNG-source engine) automatically. `export` adds the delivery
/// pipeline (resize / output sharpen / JPEG quality / color space); `None` =
/// full-res q95 sRGB as always. Returns the SAVED dimensions (post-resize).
pub fn render_to_file(
    src_path: &Path,
    recipe: &EditRecipe,
    out: &Path,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    export: Option<&ExportOpts>,
) -> Result<(u32, u32)> {
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose();
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
    let mut img = if is_raw_src {
        let working = if native_wide { space } else { ExportColorSpace::Srgb };
        render_to_image_in(src_path, recipe, denoise, None, working)?
    } else {
        // baked-by-construction: the !is_raw_src arm (decided just above).
        let src = crate::decode::load_image_for_develop(src_path)?;
        render_baked_to_image(&src, recipe, denoise)?
    };
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
            tag_icc(&mut enc, space);
            enc.write_image(rgb8.as_raw(), rgb8.width(), rgb8.height(), image::ExtendedColorType::Rgb8)
                .with_context(|| format!("encode jpeg {}", out.display()))?;
            wr.flush().with_context(|| format!("flush {}", out.display()))?;
        }
        "tif" | "tiff" => {
            let mut wr = create(&staged)?;
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut wr);
            tag_icc(&mut enc, space);
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
            tag_icc(&mut enc, space);
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
    publish_staged(out, &staged, encoded)?;

    Ok((w, h))
}

/// Fast "after" render for the UI: apply the recipe's WB + tonal + colour ops
/// to an already-demosaiced preview image (no full-res develop, no demosaic).
/// White balance runs through the SAME `apply_recipe_wb` stage as the exports,
/// so the Temp/Tint sliders and the WB eyedropper are live in the preview.
/// Crop is intentionally NOT applied here so sliders give immediate full-frame
/// feedback; the full-res `render_to_image` path applies crop on export.
pub fn develop_preview(preview: &DynamicImage, recipe: &EditRecipe) -> DynamicImage {
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose();
    let recipe = &*validated;
    let rgb = preview.to_rgb8();
    let (w, h) = rgb.dimensions();
    let mut data: Vec<[f32; 3]> = rgb
        .as_raw()
        .par_chunks(3)
        .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
        .collect();
    apply_recipe_wb(&mut data, recipe);
    apply_develop(&mut data, w as usize, h as usize, recipe);
    let mut buf = vec![0u8; (w * h * 3) as usize];
    buf.par_chunks_mut(3).zip(data.par_iter()).for_each(|(o, px)| {
        o[0] = to_u8(px[0]);
        o[1] = to_u8(px[1]);
        o[2] = to_u8(px[2]);
    });
    DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, buf).expect("preview buffer size matches"))
}

/// The full per-pixel + spatial develop pipeline (everything except WB, crop,
/// orientation), shared by full-res render and the UI preview. Order follows
/// ACR: tone → clarity → saturation/vibrance → noise reduction → sharpening.
/// Operates in place on sRGB-gamma RGB in [0,1].
fn apply_develop(data: &mut [[f32; 3]], w: usize, h: usize, r: &EditRecipe) {
    let rasters = best_effort_mask_raster_snapshot(r);
    apply_develop_with_rasters(data, w, h, r, &rasters);
}

fn apply_develop_with_rasters(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
) {
    // 0/0a) vignette — the in-camera profile falloff map and the manual
    //    slider compensation, both radial gains in LINEAR light, applied as
    //    ONE composed pass. Two sequential passes were NOT equivalent: each
    //    pass clamps to [0,1], so a profile gain and a manual correction
    //    that should cancel multiplicatively could not cancel on clipped
    //    pixels (the old "order between them is cosmetic" comment was only
    //    true of un-clamped math — L01-6).
    if let Some(lut) = vignette_gain_lut(r) {
        apply_radial_gain(data, w, h, &lut);
    }
    // 0b) dehaze — pointwise atmospheric-veil removal in LINEAR light, before
    //    any tonal work: the airlight estimate then depends only on the capture
    //    (plus WB, which ran before apply_develop), never on the user's tone
    //    sliders — dragging Exposure cannot re-estimate the haze. The
    //    pinned-white tone LUT afterwards cannot blow what dehaze protected,
    //    and saturation/vibrance stay downstream so the user can trim dehaze's
    //    chroma restoration.
    if r.dehaze != 0.0 {
        apply_dehaze(data, w, r.dehaze);
    }
    // 1) tonal ops via the LUT (exposure/contrast/whites/blacks/highlights/
    //    shadows/tone-curve). Tone the pixel's LUMINANCE and scale RGB by the
    //    ratio (scale_chroma) so hue + saturation are preserved — NOT per-channel.
    //    Running each channel through the curve independently lets opposing pushes
    //    (e.g. strong −highlights + +shadows) converge the channels, desaturating
    //    saturated colour to grey. The LUT itself is monotone with a pinned white
    //    point (see build_tone_lut), so no per-channel greying and no flat/inverted
    //    midtones — the tone model is correct by construction, not patched.
    //    A fully-neutral tone recipe skips the pass outright: sampling an
    //    identity LUT is the identity map up to interpolation rounding, and this
    //    pass used to run unconditionally over the full sensor on every open.
    //    A camera-matched base curve is tone work too — it must not be skipped.
    let tone_neutral = r.exposure_ev == 0.0
        && r.contrast == 0.0
        && r.highlights == 0.0
        && r.shadows == 0.0
        && r.whites == 0.0
        && r.blacks == 0.0
        && r.tone_curve.is_empty()
        && r.base_curve.is_empty();
    if !tone_neutral {
        let lut = build_tone_lut(r);
        data.par_iter_mut().for_each(|px| {
            let l_old = luma601(px);
            let l_new = sample_lut(&lut, l_old);
            scale_chroma(px, l_old, l_new);
        });
    }
    // 1b) per-channel RGB curves (red/green/blue), right after the master curve.
    apply_rgb_curves(data, r);
    // 2) per-colour HSL (the 8 ACR bands): rotate/scale each colour family,
    //    after global tone and before clarity/saturation (ACR ordering).
    apply_hsl(data, &r.hsl);
    // 2b) colour grading wheels (shadow/midtone/highlight/global toning + lum).
    apply_color_grade(data, &r.color_grade);
    // 3) clarity — large-radius, midtone-masked local contrast.
    if r.clarity != 0.0 {
        let radius = ((0.02 * w.min(h) as f32).round() as usize).max(8);
        unsharp_luma(data, w, h, radius, r.clarity / 100.0, true);
    }
    // 3) saturation / vibrance.
    let (sat, vib) = (r.saturation / 100.0, r.vibrance / 100.0);
    if sat != 0.0 || vib != 0.0 {
        data.par_iter_mut().for_each(|px| {
            *px = apply_sat_vibrance(px[0], px[1], px[2], sat, vib);
        });
    }
    // 4) noise reduction — BEFORE sharpening (the order that matters most).
    //    Radius stays pixel-scale by design (V2 spec §4d: noise grain lives at
    //    sensor-pixel scale) — so NR judged on a downscaled preview reads
    //    stronger than the full-res export delivers; a known perception gap.
    if r.noise_reduction > 0.0 {
        noise_reduce_luma(data, w, h, r.noise_reduction / 100.0);
    }
    // 5) sharpening — small-radius unsharp mask. Radius follows the V2 spec
    //    σ = clamp(0.0008·min(w,h), 0.7, 2.0) (docs/V2_PLAN.md §4c) instead of
    //    a hard-coded 1 px: one slider value used to mean structurally
    //    different sharpening at 1280px preview vs a 61 MP export. Three box
    //    passes of radius r ≈ Gaussian σ of √(r(r+1)), so σ rounds to the
    //    box radius directly (preview ≤1536px → 1, larger frames cap at 2 —
    //    clarity already scales with resolution the same way).
    if r.sharpening > 0.0 {
        let sigma = (0.0008 * w.min(h) as f32).clamp(0.7, 2.0);
        let radius = (sigma.round() as usize).max(1);
        unsharp_luma(data, w, h, radius, r.sharpening / 100.0, false);
    }
    // 6) local masked adjustments (linear/radial gradients).
    if !r.masks.is_empty() {
        apply_masks(data, w, h, r, rasters);
    }
}

fn luma601(p: &[f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

/// Manual lens-vignette compensation LUT: gain = 1 + k·rⁿ on the normalised
/// corner-radius, in linear light. `amount` -100..=100 (positive brightens
/// corners); `midpoint` 0..=100 shapes WHERE it lands via the radius exponent
/// (0.6..3.0, ACR-default 50 → 1.8): low reaches toward the centre, high
/// confines the correction to the corners. The exact LR falloff model is
/// proprietary — this is our documented approximation (XMP carries the raw
/// slider values, so Lightroom re-renders with its own model).
fn manual_vignette_lut(amount: f32, midpoint: f32) -> Vec<f32> {
    let gamma = 0.6 + 2.4 * (midpoint.clamp(0.0, 100.0) / 100.0);
    let k = amount.clamp(-100.0, 100.0) / 100.0;
    (0..LUT_N)
        .map(|i| 1.0 + k * (i as f32 / (LUT_N - 1) as f32).powf(gamma))
        .collect()
}

/// In-camera profile vignetting LUT: per-knot linear-light GAINS over the
/// normalised corner radius (knot placement (i+0.5)/(n−1) — see `lensmeta`),
/// linearly interpolated. Gains come from the camera, not a slider model.
fn profile_vignette_lut(knots: &[f32]) -> Vec<f32> {
    (0..LUT_N)
        .map(|i| profile_knot_interp(knots, i as f32 / (LUT_N - 1) as f32))
        .collect()
}

/// The single radial-gain LUT for whichever vignette stages are active —
/// `None` when neither is. Both active compose by MULTIPLYING the gains, so
/// the one clamp in `apply_radial_gain` runs on the true combined gain and
/// inverse corrections genuinely cancel (L01-6).
fn vignette_gain_lut(r: &EditRecipe) -> Option<Vec<f32>> {
    let profile = r
        .lens_profile
        .vignette_active()
        .then(|| profile_vignette_lut(&r.lens_profile.vignette));
    let manual =
        (r.lens_vignette != 0.0).then(|| manual_vignette_lut(r.lens_vignette, r.lens_vignette_mid));
    match (profile, manual) {
        (Some(p), Some(m)) => Some(p.iter().zip(&m).map(|(a, b)| a * b).collect()),
        (Some(p), None) => Some(p),
        (None, Some(m)) => Some(m),
        (None, None) => None,
    }
}

/// Apply a radial gain LUT — indexed by the normalised corner radius — in
/// LINEAR light. The two vignette stages above differ ONLY in how they build
/// that LUT, so the geometry, the transfer pair and the traversal live here
/// once and cannot drift apart.
///
/// The stage used to cost 7 powf per pixel (rⁿ + two transfer curves × 3
/// channels) on every preview tick and export; three LUTs replace them. Rows
/// are independent, so the pass is row-parallel.
fn apply_radial_gain(data: &mut [[f32; 3]], w: usize, h: usize, gain_lut: &[f32]) {
    if w == 0 || h == 0 {
        return; // par_chunks_mut(0) asserts even on an empty slice (U14)
    }
    let (cx, cy) = ((w as f32 - 1.0) * 0.5, (h as f32 - 1.0) * 0.5);
    let rmax = (cx * cx + cy * cy).sqrt().max(1.0);
    let (dec, enc) = transfer_luts();
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let dy = y as f32 - cy;
        for (x, px) in row.iter_mut().enumerate() {
            let dx = x as f32 - cx;
            let rn = ((dx * dx + dy * dy).sqrt() / rmax).clamp(0.0, 1.0);
            let gain = sample_lut(gain_lut, rn);
            if (gain - 1.0).abs() < 1e-6 {
                continue;
            }
            for c in px.iter_mut() {
                *c = sample_lut(enc, (sample_lut(dec, *c) * gain).clamp(0.0, 1.0));
            }
        }
    });
}

/// Dehaze: pointwise atmospheric-scattering inversion, `amount` -100..=100.
///
/// Model: `I = J·t + A·(1−t)` (observed = true radiance through transmission
/// `t`, veiled by airlight `A`). Solved per pixel with the pixel's OWN
/// min-channel as the haze-density proxy — deliberately NOT the spatial
/// dark-channel min-filter: a pointwise op is O(N) per slider tick and its
/// statistics stay CDF-identifiable (the constraint the reverse-fit design
/// documents in fit.rs). Airlight `A` = P99 of the min channel in linear
/// light (the brightest neutral-ish region — the hazy sky), via a histogram
/// over strided samples so full-res export and 384px analysis agree.
///
/// Positive `amount` removes haze: `ω = min(R,G,B)/A` (haze density),
/// `t = max(1 − K·s·ω, T_MIN)`, `out = (in − A(1−t))/t`. All three channels
/// of a pixel share one affine map, so channel ORDER is preserved (no
/// magenta/cyan inversions), `v = A` is a fixed point (an airlight-bright sky
/// does not move or blow out), and the map is monotone in luma. Scaling the
/// channel DIFFERENCES by 1/t ≥ 1 is the point — haze removal must deepen
/// tone AND restore chroma together, which is why this deliberately does not
/// use the luma-preserving `scale_chroma` convention of the tone stages.
///
/// Negative `amount` adds a uniform veil toward the airlight (`ω ≡ 1`, the
/// exact inverse family): a convex blend, mathematically clip-free.
///
/// Split into [`dehaze_airlight`] (the frame-level estimate) and [`dehaze_px`]
/// (the per-pixel affine map) so the MASKED dehaze in [`apply_masks`] can
/// share the exact same two halves — one model, two call sites, no second
/// implementation to drift. The split is a pure factoring: test
/// `dehaze_split_is_bit_identical_to_the_pre_split_golden` pins the output
/// bit-for-bit against values captured before it.
fn apply_dehaze(data: &mut [[f32; 3]], w: usize, amount: f32) {
    let s = amount.clamp(-100.0, 100.0) / 100.0;
    if s.abs() < 1e-4 {
        return;
    }
    let a = dehaze_airlight(data, w);
    let (dec, enc) = transfer_luts();
    data.par_iter_mut().for_each(|px| {
        *px = dehaze_px(px, a, s, dec, enc);
    });
}

/// Full-slider dehaze strength: at +100 a pure-airlight pixel reaches
/// `t = DEHAZE_T_MIN`.
const DEHAZE_K: f32 = 0.75;
/// Dehaze transmission floor — caps amplification at 1/T_MIN ≈ 3.3× so deep
/// shadows darken decisively but cannot explode to noise.
const DEHAZE_T_MIN: f32 = 0.30;

/// Estimate the airlight `A` for [`apply_dehaze`] / [`dehaze_px`]: P99 of the
/// linear min-channel, clamped away from black. Depends ONLY on the frame it
/// is handed — no slider value reaches it, which is what keeps the haze model
/// from re-estimating itself when the user drags Exposure (and, in
/// [`apply_masks`], what makes the estimate independent of mask stacking).
fn dehaze_airlight(data: &[[f32; 3]], w: usize) -> f32 {
    // Airlight: histogram of the linear min-channel over ≤ ~262k strided
    // samples (resolution-stable), P99, clamped away from black so a frame
    // with no bright region cannot produce a degenerate divisor.
    //
    // Each row's sampling phase comes from a HASH of the row index, not from
    // the row index itself. Two weaker schemes failed first: a flat
    // `step_by(stride)` phase-locked to column parity (a one-pixel shift of a
    // striped frame flipped the airlight between the 0.10 floor and the
    // bright bin, U14), and a +1-per-row shear fixed that but still locked to
    // a DIAGONAL of the same period — with stride 2 it samples exactly the
    // pixels where x ≡ y (mod 2), i.e. one checkerboard phase (R12). A
    // hashed phase is deterministic (same frame → same estimate, preview and
    // export agree) yet correlates with no small period, so a periodic frame
    // contributes every phase to the histogram. The stride needs no parity
    // or coprimality, so the sample count stays exactly the budget.
    let mut hist = [0u32; 1024];
    let mut n = 0u32;
    let stride = (data.len() / 262_144).max(1);
    let mut add = |px: &[f32; 3]| {
        let m = srgb_to_linear(px[0]).min(srgb_to_linear(px[1])).min(srgb_to_linear(px[2]));
        hist[(m.clamp(0.0, 1.0) * 1023.0) as usize] += 1;
        n += 1;
    };
    if stride == 1 {
        data.iter().for_each(&mut add);
    } else {
        // stride > 1 implies len ≥ 524288, so w ≥ 1 and chunks(w) is safe.
        for (y, row) in data.chunks(w).enumerate() {
            // splitmix-style multiply-shift: cheap, deterministic, and its
            // low bits do not follow the row index.
            let phase =
                (((y as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33) as usize) % stride;
            row.iter().skip(phase).step_by(stride).for_each(&mut add);
        }
    }
    let mut acc = 0u32;
    let mut a_bin = 1023usize;
    for (i, c) in hist.iter().enumerate() {
        acc += c;
        if acc as f32 >= 0.99 * n as f32 {
            a_bin = i;
            break;
        }
    }
    (a_bin as f32 / 1023.0).clamp(0.10, 1.0)
}

/// One pixel through the dehaze affine map: `a` = airlight from
/// [`dehaze_airlight`], `s` = signed slider strength (`amount/100`, already
/// clamped), `dec`/`enc` = the shared transfer LUTs.
///
/// 6 powf/px are replaced by those LUTs (the airlight histogram keeps the exact
/// powf — see `transfer_luts`); pixels are independent, so both callers run the
/// map in parallel.
#[inline]
fn dehaze_px(px: &[f32; 3], a: f32, s: f32, dec: &[f32], enc: &[f32]) -> [f32; 3] {
    let lin = [sample_lut(dec, px[0]), sample_lut(dec, px[1]), sample_lut(dec, px[2])];
    let out = if s > 0.0 {
        let w = (lin[0].min(lin[1]).min(lin[2]) / a).clamp(0.0, 1.0);
        let t = (1.0 - DEHAZE_K * s * w).max(DEHAZE_T_MIN);
        let b = a * (1.0 - t);
        [(lin[0] - b) / t, (lin[1] - b) / t, (lin[2] - b) / t]
    } else {
        let v = DEHAZE_K * (-s);
        [
            lin[0] * (1.0 - v) + a * v,
            lin[1] * (1.0 - v) + a * v,
            lin[2] * (1.0 - v) + a * v,
        ]
    };
    [
        sample_lut(enc, out[0].clamp(0.0, 1.0)),
        sample_lut(enc, out[1].clamp(0.0, 1.0)),
        sample_lut(enc, out[2].clamp(0.0, 1.0)),
    ]
}

/// Apply each local masked adjustment: blend the masked region toward a locally
/// re-adjusted version, weighted by the gradient mask × amount. Mask coords are
/// normalised so this works at any resolution.
///
/// Per mask, in pass order: local **dehaze** → the fused local **WB**
/// (temperature/tint — the same [`wb_gains`] model as the global stage, see
/// [`local_temp_to_kelvin`]) + **tone** (exposure/contrast/highlights/shadows/
/// whites/blacks) + **saturation** pass → local **clarity** → local **texture**
/// → local **noise reduction** (smooth luma toward its neighbourhood, inside
/// the mask — for "this region is noisy" requests).
///
/// Clarity/dehaze/texture are ENGINE-RENDERED since R22 (they were XMP-only
/// before, so a mask that moved only those three appeared to do nothing in-app
/// — user feedback #15a/#10B; recipes saved before R22 that carry them now
/// re-render with the local effect applied, which the user signed off on).
/// Clarity and texture are unsharp masks at two different radii
/// ([`unsharp_luma_weighted`], weighted by the mask instead of blending against
/// an RGB copy); dehaze reuses the exact global pair
/// [`dehaze_airlight`] + [`dehaze_px`].
///
/// **Pass order vs the global chain** (WB → dehaze → tone → … → clarity →
/// saturation → NR): the local WB/tone/saturation stages are ONE fused
/// single-weight blend, and splitting them apart to interleave the spatial ops
/// would change the output of every existing partial-weight mask (for `0 < w <
/// 1`, one blend of the composed transform ≠ three chained blends). So the two
/// achievable orderings are kept and the residue is documented: dehaze runs
/// BEFORE the fused pass — preserving the two properties the global order
/// exists for (the monotone pinned-white tone LUT cannot blow what dehaze
/// protected, and saturation stays downstream so the user can trim dehaze's
/// chroma restoration) at the cost of local Temp/Tint landing after local
/// dehaze rather than before it; clarity/texture run after the fused pass, so
/// local saturation precedes them (globally clarity precedes saturation).
/// Both residues are second-order: local Temp/Tint is a relative nudge on a
/// frame that was already globally white-balanced before the airlight was
/// estimated, and clarity/texture scale luma while saturation moves chroma.
///
/// **Memory** (full-resolution export, 61 MP → 244 MB per f32 plane): the
/// spatial passes each hold one luma plane + one blurred plane and run
/// SEQUENTIALLY, dropping both before the next starts, so the resident
/// increment is the same two planes (~488 MB) the local NR pass has always
/// cost — not three passes' worth. `blur_plane` transiently holds two more of
/// its own, the existing global-clarity/NR peak. The `!= 0.0` gate on each
/// pass means a mask that does not use an op allocates nothing for it.
fn apply_masks(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
) {
    if w == 0 || h == 0 {
        return; // both passes below chunk by w; rayon asserts chunk_size != 0
    }
    // Airlight for every masked dehaze in this frame, estimated ONCE and only
    // when some mask actually asks for dehaze (the estimate is a full-frame
    // histogram — a real cost at 61 MP). Estimating it here, from the frame as
    // the global develop left it, is what makes it independent of MASK STACKING
    // ORDER: reordering or toggling masks cannot re-estimate the haze, exactly
    // as dragging Exposure cannot re-estimate the global one. A per-mask
    // estimate would also make two masks disagree about the same sky.
    let dehaze_a = r
        .masks
        .iter()
        .any(|m| m.enabled && m.amount.clamp(0.0, 1.0) != 0.0 && m.dehaze != 0.0)
        .then(|| dehaze_airlight(data, w));
    for m in &r.masks {
        // The eye toggle: a disabled mask renders nothing at any Amount —
        // the lossless mute (recipe.rs `LocalAdjustment::enabled`).
        if !m.enabled {
            continue;
        }
        let local = EditRecipe {
            exposure_ev: m.exposure_ev,
            contrast: m.contrast,
            highlights: m.highlights,
            shadows: m.shadows,
            whites: m.whites,
            blacks: m.blacks,
            ..EditRecipe::default()
        };
        let lut = build_tone_lut(&local);
        let sat = m.saturation / 100.0;
        // Local colour transform, computed ONCE per mask (never inside the
        // pixel loop): compose Temp/Tint WB with the zoned recolour gains,
        // then compile the exact linear-light formula into channel LUTs.
        // None when neutral, so tone-only masks pay no colour-stage cost.
        let colour_luts =
            (m.temperature != 0.0 || m.tint != 0.0 || m.color_gains.is_some()).then(|| {
                let g = wb_gains(5500.0, local_temp_to_kelvin(m.temperature), m.tint);
                let cg = m.color_gains.unwrap_or([1.0; 3]);
                colour_gain_luts([g[0] * cg[0], g[1] * cg[1], g[2] * cg[2]])
            });
        let amount = m.amount.clamp(0.0, 1.0);
        // Amount 0 zeroes every weight below (inverted or not) — skip the
        // full-frame tone scan and a possible NR blur that would all be
        // multiplied away (a real cost at 61 MP for a merely parked mask).
        if amount == 0.0 {
            continue;
        }
        // Bitmap geometry: decode each raster ONCE per mask per develop
        // (never inside the pixel loop); both the tone and the NR pass share
        // them. Components load alongside the base.
        let bmp = rasters.get(&m.mask);
        let comp_bmps: Vec<Option<&image::GrayImage>> =
            m.components.iter().map(|c| rasters.get(&c.geometry)).collect();
        // An unloadable raster carries NO coverage, so its weight must never
        // reach the inversion below: 0 with `inverted` would apply this
        // adjustment to the WHOLE frame at full strength. Skipping the whole
        // adjustment is the inert contract (recipe.rs `MaskGeometry::Bitmap`)
        // — and it covers COMPONENTS for the same reason: a lost Subtract
        // raster contributes 0 and silently WIDENS the effect area.
        if (bmp.is_none() && matches!(m.mask, MaskGeometry::Bitmap { .. }))
            || m.components
                .iter()
                .zip(&comp_bmps)
                .any(|(c, b)| b.is_none() && matches!(c.geometry, MaskGeometry::Bitmap { .. }))
        {
            continue;
        }
        // combined mask coverage × master amount at a pixel (with inversion).
        let weight_at = |x: usize, y: usize| -> f32 {
            let (nx, ny) = (x as f32 / w as f32, y as f32 / h as f32);
            let mut wgt = combined_mask_weight(m, nx, ny, bmp, &comp_bmps);
            if m.inverted {
                wgt = 1.0 - wgt;
            }
            wgt * amount
        };

        // --- local dehaze pass (runs FIRST — see the pass-order note on this
        //     function for why, and for the two residues vs the global chain) ---
        // The airlight is the frame-level one estimated above; only the affine
        // map is per-pixel, so this is the same model as the global stage with
        // the mask weight blending the two ends. `|s| < 1e-4` mirrors
        // `apply_dehaze`'s own floor, so a hair-off-zero slider costs no pass.
        let dehaze_s = m.dehaze.clamp(-100.0, 100.0) / 100.0;
        if dehaze_s.abs() >= 1e-4
            && let Some(a) = dehaze_a
        {
            let (dec, enc) = transfer_luts();
            data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, out_px) in row.iter_mut().enumerate() {
                    let mut wgt = weight_at(x, y);
                    if wgt <= 0.001 {
                        continue;
                    }
                    let p = *out_px;
                    // Range Mask intersection, same convention as the tone pass.
                    if let Some(rm) = &m.range {
                        wgt *= range_weight(rm, &p);
                        if wgt <= 0.001 {
                            continue;
                        }
                    }
                    let t = dehaze_px(&p, a, dehaze_s, dec, enc);
                    for c in 0..3 {
                        out_px[c] = p[c] * (1.0 - wgt) + t[c] * wgt;
                    }
                }
            });
        }

        // An adjustment whose tone/sat/colour stages are ALL identity blends
        // each pixel with itself — skip the full-frame scan (a real cost at
        // 61 MP for an NR-only or freshly parked mask); the clarity, texture
        // and NR passes below each still run on their own `!= 0.0` gate, so a
        // mask that moves ONLY one of those reaches it (before R22 a
        // clarity-only mask fell through every gate and rendered nothing).
        let tone_identity = m.exposure_ev == 0.0
            && m.contrast == 0.0
            && m.highlights == 0.0
            && m.shadows == 0.0
            && m.whites == 0.0
            && m.blacks == 0.0
            && m.saturation == 0.0
            && colour_luts.is_none();

        // --- tone + saturation pass (rows independent → parallel) ---
        if !tone_identity {
        data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, out_px) in row.iter_mut().enumerate() {
                let mut wgt = weight_at(x, y);
                if wgt <= 0.001 {
                    continue;
                }
                let p = *out_px;
                // Range Mask refinement: intersect the geometric weight with the
                // per-pixel range weight, evaluated on the pixel as it stands when
                // this mask runs (post-global develop, pre-this-mask — masks stack
                // sequentially, so a later mask's range sees earlier masks' output;
                // documented approximation vs LR's fixed reference image).
                if let Some(rm) = &m.range {
                    wgt *= range_weight(rm, &p);
                    if wgt <= 0.001 {
                        continue;
                    }
                }
                // Local WB/recolour first (the same exact linear-light model
                // as global apply_wb, sampled through its 4096-entry LUT), then
                // luminance-preserving local tone and saturation. The fully-
                // shifted pixel `t` is blended with the original by mask weight.
                let mut t = p;
                if let Some(luts) = &colour_luts {
                    for c in 0..3 {
                        t[c] = sample_lut(&luts[c], t[c]);
                    }
                }
                let l_old = luma601(&t);
                let l_new = sample_lut(&lut, l_old);
                scale_chroma(&mut t, l_old, l_new);
                let t = apply_sat_vibrance(t[0], t[1], t[2], sat, 0.0);
                for c in 0..3 {
                    out_px[c] = p[c] * (1.0 - wgt) + t[c] * wgt;
                }
            }
        });
        }

        // --- local clarity / texture (SPATIAL: full-frame luma plane → blur →
        //     detail weighted by the mask, exactly like the NR pass below) ---
        //     Neither can ride the per-pixel loop above: a spatial operator
        //     needs neighbours, and that loop is a pure per-pixel LUT.
        let spatial_weight = |x: usize, y: usize, px: &[f32; 3]| -> f32 {
            let mut wgt = weight_at(x, y);
            // Range Mask intersection, same convention as the tone pass — the
            // pixel state here already carries this mask's own tone move (the
            // same documented drift the NR pass notes).
            if wgt > 0.001
                && let Some(rm) = &m.range
            {
                wgt *= range_weight(rm, px);
            }
            wgt
        };
        if m.clarity != 0.0 {
            // The global clarity radius model, verbatim (render.rs stage 3):
            // large-radius midtone-masked local contrast, 2% of the short edge
            // floored at 8 px so a preview and a 61 MP export mean the same
            // thing by "Clarity 30".
            let radius = ((0.02 * w.min(h) as f32).round() as usize).max(8);
            unsharp_luma_weighted(data, w, h, radius, m.clarity / 100.0, true, spatial_weight);
        }
        if m.texture != 0.0 {
            // Texture = the same unsharp operator at a SMALL radius and with no
            // midtone mask, so it works fine detail across the whole tonal
            // range where clarity works midtone volume. There is no global
            // Texture stage to align with and Adobe's model is proprietary, so
            // 0.5% of the short edge (floored at 2 px) is OUR calibration — the
            // same honesty stance as `manual_vignette_lut`: the XMP carries the
            // raw slider value, so Lightroom re-renders it with its own model.
            let radius = ((0.005 * w.min(h) as f32).round() as usize).max(2);
            unsharp_luma_weighted(data, w, h, radius, m.texture / 100.0, false, spatial_weight);
        }

        // --- local noise reduction pass (only where the mask covers) ---
        let nr = (m.noise_reduction / 100.0).clamp(0.0, 1.0);
        // Gate matches the per-pixel `nw <= 0.001` skip below: with nr at or
        // under it every weight is rejected anyway, and the two full-frame
        // f32 planes (~488 MB at 61 MP) were allocated for nothing.
        if nr > 0.001 {
            let luma: Vec<f32> = data.par_iter().map(luma601).collect();
            let blur = blur_plane(&luma, w, h, 2);
            data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, px) in row.iter_mut().enumerate() {
                    let i = y * w + x;
                    let mut nw = weight_at(x, y) * nr;
                    if let Some(rm) = &m.range {
                        // Same intersection as the tone pass (pixel state here
                        // includes this mask's own tone move — acceptable drift,
                        // NR is the subtler effect).
                        nw *= range_weight(rm, px);
                    }
                    if nw <= 0.001 {
                        continue;
                    }
                    let l = luma[i];
                    let new_l = l + (blur[i] - l) * nw;
                    scale_chroma(px, l, new_l);
                }
            });
        }
    }
}

/// Mask coverage [0,1] at normalised frame coordinate (nx, ny).
fn mask_weight(g: &MaskGeometry, nx: f32, ny: f32, bmp: Option<&image::GrayImage>) -> f32 {
    match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let (vx, vy) = (full_x - zero_x, full_y - zero_y);
            let len2 = vx * vx + vy * vy;
            if len2 < 1e-9 {
                return 1.0;
            }
            (((nx - zero_x) * vx + (ny - zero_y) * vy) / len2).clamp(0.0, 1.0)
        }
        // `roundness` is carried but deliberately NOT rendered — pure ellipse,
        // see `MaskGeometry::Radial` in recipe.rs. Nothing in the repo fixes its
        // scale or sign (the advisor schema declares a bare number; docs/
        // V2_PLAN.md §7 item 1 lists the radial ranges as unverified; every
        // radial in the reference Lightroom sidecars carries Roundness="0").
        // The sibling `feather` HAD the same guessing bug — Lightroom writes it
        // 0..100 and xmp.rs used to import the value raw, so Feather="72"
        // clamped to fully feathered; both XMP directions now convert on the
        // boundary (xmp.rs). Test radial_roundness_is_a_documented_no_op pins
        // the roundness no-op until a real sidecar fixes the mapping.
        MaskGeometry::Radial { top, left, bottom, right, feather, roundness: _, flipped, angle } => {
            let cx = (left + right) / 2.0;
            let cy = (top + bottom) / 2.0;
            let rx = ((right - left) / 2.0).abs().max(1e-4);
            let ry = ((bottom - top) / 2.0).abs().max(1e-4);
            // Rotation (engine convention, recipe.rs `MaskGeometry::Radial`):
            // rotate the SAMPLE POINT about the bbox centre by −angle, in
            // normalised frame coords — equivalent to rotating the ellipse
            // by +angle (counter-clockwise, y-down screen sense).
            let (mut px, mut py) = (nx - cx, ny - cy);
            if *angle != 0.0 {
                let (s, c) = (-angle.to_radians()).sin_cos();
                (px, py) = (px * c - py * s, px * s + py * c);
            }
            let d = ((px / rx).powi(2) + (py / ry).powi(2)).sqrt();
            let f = feather.clamp(0.0, 1.0);
            // Guarded ramp, not raw `smoothstep`: feather 0 makes the edges
            // equal, and 0/0 would be NaN exactly on the ellipse boundary
            // (NaN survives the `wgt <= 0.001` early-out and casts to black).
            let wgt = 1.0 - ramp(1.0 - f, 1.0, d);
            if *flipped {
                1.0 - wgt
            } else {
                wgt
            }
        }
        // Raster mask: bilinear lookup in the pre-decoded bitmap (normalised
        // coords, so the mask's own resolution is independent of the render's).
        // No bitmap = the load failed → inert, warned once by the loader.
        MaskGeometry::Bitmap { .. } => match bmp {
            Some(b) => sample_gray_norm(b, nx, ny),
            None => 0.0,
        },
    }
}

// --- bitmap-mask BAKE operations (the GUI's raster editing) -----------------
// Every op returns a NEW image; the GUI writes it under a freshly CLAIMED
// raster name and repoints the recipe — the input file is never mutated, so
// saved recipes and version snapshots referencing it keep rendering what they
// rendered (the same immutability rule start_segment follows).

/// Soften a mask's boundary: gaussian blur of the raster. `sigma` in mask
/// pixels.
pub fn feather_mask(g: &image::GrayImage, sigma: f32) -> image::GrayImage {
    image::imageops::blur(g, sigma.max(0.1))
}

/// Grow (`radius > 0`, dilate) or shrink (`radius < 0`, erode) a mask by
/// `|radius|` pixels — separable max/min filter, the morphological pair
/// behind Lightroom's Expand/Contract. Square structuring element: visually
/// indistinguishable from a disc at the small radii the GUI uses, and O(r)
/// cheaper to reason about.
pub fn morph_mask(g: &image::GrayImage, radius: i32) -> image::GrayImage {
    if radius == 0 || g.width() == 0 || g.height() == 0 {
        return g.clone();
    }
    let r = radius.unsigned_abs() as usize;
    let dilate = radius > 0;
    let (w, h) = (g.width() as usize, g.height() as usize);
    let src = g.as_raw();
    let pick = |acc: u8, v: u8| if dilate { acc.max(v) } else { acc.min(v) };
    let seed = if dilate { 0u8 } else { 255u8 };
    // Horizontal pass…
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        let row = &src[y * w..y * w + w];
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = row[lo..=hi].iter().fold(seed, |a, &v| pick(a, v));
        }
    }
    // …then vertical.
    let mut out = vec![0u8; w * h];
    for x in 0..w {
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
            let mut v = seed;
            for yy in lo..=hi {
                v = pick(v, tmp[yy * w + x]);
            }
            out[y * w + x] = v;
        }
    }
    image::GrayImage::from_raw(w as u32, h as u32, out).expect("dims preserved")
}

// Seven f32 planes are live at the peak over an input tile whose side is at
// most 1024 + 12r: 7 × (1024 + 12r)² × 4 bytes, plus one column scratch row
// and the required u8 output. That is about 42 MiB at the GUI's usual r=19.
const GUIDED_REFINE_TILE_EDGE: usize = 1024;

/// Upsample `mask` to the guide image's resolution and snap its soft
/// boundary onto the guide's real edges — He et al.'s guided filter, box
/// means via the NR pass's own `blur_plane`. This is the honest fix for
/// AI-segmentation rasters baked at preview resolution: bilinear upsampling
/// keeps their PLACEMENT resolution-independent but not their DETAIL (a
/// 1280 px mask on a 9504 px export smears every boundary ~7 px), while the
/// guide-driven output follows hair/foliage/architecture edges at the
/// guide's own resolution.
pub fn refine_mask_guided(
    mask: &image::GrayImage,
    guide: &DynamicImage,
    radius: usize,
    eps: f32,
) -> image::GrayImage {
    // A zero / negative / non-finite eps divides by (near-)zero variance and
    // quantises NaN to black pixels — floor it here, this fn is pub.
    let eps = if eps.is_finite() { eps.max(1e-6) } else { 1e-6 };
    refine_mask_guided_tiled(mask, guide, radius, eps, GUIDED_REFINE_TILE_EDGE)
}

fn refine_mask_guided_tiled(
    mask: &image::GrayImage,
    guide: &DynamicImage,
    radius: usize,
    eps: f32,
    tile_edge: usize,
) -> image::GrayImage {
    let (w, h) = (guide.width() as usize, guide.height() as usize);
    if w == 0 || h == 0 {
        return mask.clone();
    }

    let r = radius.max(1);
    let support = r.saturating_mul(3);
    let tile_edge = tile_edge.max(1);
    let mut out = image::GrayImage::new(guide.width(), guide.height());

    let guide_luma = |x: usize, y: usize| {
        let p = guide.get_pixel(x as u32, y as u32);
        (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0
    };
    // Global pixel-centre coordinates preserve Triangle resize's continuous
    // mapping; tile-local coordinates never enter, so adjoining tiles sample
    // exactly the same low-resolution mask function.
    let mask_value = |x: usize, y: usize| {
        let u = (x as f32 + 0.5) / w as f32;
        let v = (y as f32 + 0.5) / h as f32;
        image::imageops::sample_bilinear(mask, u, v).map_or(0.0, |p| p[0] as f32 / 255.0)
    };

    for tile_y in (0..h).step_by(tile_edge) {
        let tile_y1 = tile_y.saturating_add(tile_edge).min(h);
        for tile_x in (0..w).step_by(tile_edge) {
            let tile_x1 = tile_x.saturating_add(tile_edge).min(w);

            // The coefficient blur needs a 3r halo around the output tile.
            // Computing those coefficients needs another 3r halo from the
            // guide and mask, so neither serial blur sees an artificial edge.
            let coeff_x0 = tile_x.saturating_sub(support);
            let coeff_y0 = tile_y.saturating_sub(support);
            let coeff_x1 = tile_x1.saturating_add(support).min(w);
            let coeff_y1 = tile_y1.saturating_add(support).min(h);
            let input_x0 = coeff_x0.saturating_sub(support);
            let input_y0 = coeff_y0.saturating_sub(support);
            let input_x1 = coeff_x1.saturating_add(support).min(w);
            let input_y1 = coeff_y1.saturating_add(support).min(h);
            let input_w = input_x1 - input_x0;
            let input_h = input_y1 - input_y0;

            let mut i_plane = Vec::with_capacity(input_w * input_h);
            let mut p_plane = Vec::with_capacity(input_w * input_h);
            for y in input_y0..input_y1 {
                for x in input_x0..input_x1 {
                    i_plane.push(guide_luma(x, y));
                    p_plane.push(mask_value(x, y));
                }
            }

            let ip: Vec<f32> = i_plane.iter().zip(&p_plane).map(|(i, p)| i * p).collect();
            let ii: Vec<f32> = i_plane.iter().map(|i| i * i).collect();
            let mut a = blur_plane(&ip, input_w, input_h, r);
            drop(ip);
            let mut b = blur_plane(&ii, input_w, input_h, r);
            drop(ii);
            let mean_i = blur_plane(&i_plane, input_w, input_h, r);
            let mean_p = blur_plane(&p_plane, input_w, input_h, r);

            for k in 0..a.len() {
                let var = (b[k] - mean_i[k] * mean_i[k]).max(0.0);
                let cov = a[k] - mean_i[k] * mean_p[k];
                a[k] = cov / (var + eps);
                b[k] = mean_p[k] - a[k] * mean_i[k];
            }
            drop(i_plane);
            drop(p_plane);
            drop(mean_i);
            drop(mean_p);

            let coeff_w = coeff_x1 - coeff_x0;
            let coeff_h = coeff_y1 - coeff_y0;
            let coeff_offset_x = coeff_x0 - input_x0;
            let coeff_offset_y = coeff_y0 - input_y0;
            let mut a_inner = Vec::with_capacity(coeff_w * coeff_h);
            let mut b_inner = Vec::with_capacity(coeff_w * coeff_h);
            for y in 0..coeff_h {
                let start = (coeff_offset_y + y) * input_w + coeff_offset_x;
                a_inner.extend_from_slice(&a[start..start + coeff_w]);
                b_inner.extend_from_slice(&b[start..start + coeff_w]);
            }
            drop(a);
            drop(b);

            let mean_a = blur_plane(&a_inner, coeff_w, coeff_h, r);
            drop(a_inner);
            let mean_b = blur_plane(&b_inner, coeff_w, coeff_h, r);
            drop(b_inner);

            let tile_offset_x = tile_x - coeff_x0;
            let tile_offset_y = tile_y - coeff_y0;
            for (local_y, y) in (tile_y..tile_y1).enumerate() {
                for (local_x, x) in (tile_x..tile_x1).enumerate() {
                    let k = (tile_offset_y + local_y) * coeff_w + tile_offset_x + local_x;
                    let value = ((mean_a[k] * guide_luma(x, y) + mean_b[k]).clamp(0.0, 1.0)
                        * 255.0)
                        .round() as u8;
                    out.put_pixel(x as u32, y as u32, image::Luma([value]));
                }
            }
        }
    }

    out
}

/// The ENGINE's own activity rule for one local adjustment: does
/// [`apply_masks`] have anything to do for it? Every `!= 0.0` gate inside that
/// function is mirrored here — identity tone/sat + no local WB/recolour + no
/// local dehaze/clarity/texture/NR renders nothing even with a healthy raster.
///
/// Two consumers depend on it and must never disagree with the render: the
/// GUI's mask-list activity marker (so the ● the user sees IS the rule the
/// render applies) and `load_mask_raster_snapshot_with_budget`, which spends
/// the raster budget only on masks that will actually render. Adding the
/// clarity/dehaze/texture terms in R22 fixed both at once: a clarity-only
/// bitmap mask used to read "parked" AND have its raster left unloaded.
pub fn engine_active(m: &crate::recipe::LocalAdjustment) -> bool {
    m.exposure_ev != 0.0
        || m.contrast != 0.0
        || m.highlights != 0.0
        || m.shadows != 0.0
        || m.whites != 0.0
        || m.blacks != 0.0
        || m.clarity != 0.0
        || m.dehaze != 0.0
        || m.texture != 0.0
        || m.saturation != 0.0
        || m.temperature != 0.0
        || m.tint != 0.0
        || m.noise_reduction != 0.0
        || m.color_gains.is_some_and(|g| g != [1.0, 1.0, 1.0])
}

#[derive(Debug, Default)]
struct MaskRasterSnapshot {
    images: std::collections::HashMap<String, std::sync::Arc<image::GrayImage>>,
}

impl MaskRasterSnapshot {
    fn get(&self, geometry: &MaskGeometry) -> Option<&image::GrayImage> {
        let MaskGeometry::Bitmap { path } = geometry else { return None };
        self.images.get(path).map(std::sync::Arc::as_ref)
    }
}

fn load_mask_raster_snapshot(recipe: &EditRecipe) -> Result<MaskRasterSnapshot> {
    load_mask_raster_snapshot_with_budget(recipe, MASK_RASTER_BUDGET_BYTES, true)
}

fn best_effort_mask_raster_snapshot(recipe: &EditRecipe) -> MaskRasterSnapshot {
    load_mask_raster_snapshot_with_budget(recipe, MASK_RASTER_BUDGET_BYTES, false)
        .unwrap_or_default()
}

/// Which of this adjustment's bitmap geometries (base or component) currently
/// have NO loadable raster. The preview path renders such a geometry inert
/// with only a stderr warning ([`load_mask_bitmap`]) — the GUI mask list uses
/// this to put the fact ON THE ROW (L08: the list said "enabled" while the
/// engine skipped the mask). Cache-hot: the answer comes from
/// `load_mask_bitmap`'s (mtime, size)-keyed cache, so a per-frame call costs
/// one metadata stat per path, not a decode.
pub fn dead_bitmap_rasters(m: &crate::recipe::LocalAdjustment) -> Vec<String> {
    std::iter::once(&m.mask)
        .chain(m.components.iter().map(|c| &c.geometry))
        .filter_map(|g| {
            let MaskGeometry::Bitmap { path } = g else { return None };
            load_mask_bitmap(g).is_none().then(|| path.clone())
        })
        .collect()
}

/// The ONE bounded gate for decoding a standalone mask/overlay raster (L02):
/// a header-only dimension probe refuses anything whose worst-case decoded
/// footprint (w×h×4, the decoder's native intermediate) exceeds the mask
/// budget BEFORE the decoder allocates. Every mask decode outside the
/// snapshot loader routes through here — GUI brush/edit/refine bases,
/// heal/clone plan masks, generative fill masks, reverse-fit sky masks.
/// (The probe and the decode are two opens; a file swapped between them is
/// the develop-store TOCTOU boundary, not this size gate's.)
pub fn open_mask_bounded(path: &std::path::Path) -> anyhow::Result<image::DynamicImage> {
    let (w, h) = image::ImageReader::open(path)?.into_dimensions()?;
    check_mask_dims(w, h, &path.display().to_string())?;
    Ok(image::open(path)?)
}

/// [`open_mask_bounded`] for in-memory bytes (the web UI's uploaded masks).
pub fn mask_from_memory_bounded(bytes: &[u8]) -> anyhow::Result<image::DynamicImage> {
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()?;
    check_mask_dims(w, h, "uploaded mask")?;
    Ok(image::load_from_memory(bytes)?)
}

/// Would a mask raster of these dimensions ever be loadable ON ITS OWN? The
/// per-file half of the budget, which is all [`check_mask_dims`] can judge at a
/// single READ. Deliberately NOT public: a WRITER that asked this question
/// alone shipped the R22 H1 defect (the mask-refine precheck passed a raster
/// the loader then refused for the AGGREGATE) — the writer-side question is
/// [`mask_raster_write_fits_budget`].
fn mask_raster_fits_budget(w: u32, h: u32) -> bool {
    raster_bytes(w, h) <= MASK_RASTER_BUDGET_BYTES
}

/// The ONE worst-case footprint every budget arm charges a raster: ×4 = the
/// decoder's native RGBA intermediate ahead of the grayscale conversion a
/// snapshot retains.
fn raster_bytes(w: u32, h: u32) -> usize {
    (w as usize).saturating_mul(h as usize).saturating_mul(4)
}

/// Header-only projection of ONE mask raster already on disk — [`raster_bytes`]
/// of its stored dimensions, without decoding it. `None` = the dimensions could
/// not be read (a missing/unreadable file), which is the loader's own
/// "find out at decode time" case.
///
/// Charged BEFORE the decode on purpose: the budget used to be spent only after
/// `image::open` had allocated the full raster, so one compressed large file
/// drove peak memory far past the advertised cap before being rejected.
fn raster_projected_bytes(path: &str) -> Option<usize> {
    let reader = image::ImageReader::open(std::path::Path::new(path)).ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    Some(raster_bytes(w, h))
}

/// Every bitmap geometry the develop pipeline would LOAD for this recipe, in
/// the loader's own order: `(mask, geometry, path)` for each Bitmap base or
/// component of a mask that will actually render.
///
/// ONE definition of "active", used by both the loader
/// ([`load_mask_raster_snapshot_with_budget`]) and the writer-side precheck
/// ([`mask_raster_write_fits_budget`]) — R22 H1: the precheck judged the
/// incoming raster ALONE while the loader charges the whole active set, so a
/// second full-resolution refine passed the precheck and was then refused by
/// the aggregate (strict bail at export, silent skip in the preview).
///
/// NOT de-duplicated: the loader skips a path it already HOLDS (its `held_bytes`
/// is the truth about what is charged), while the precheck de-duplicates by
/// path — two different, correct answers to "have I counted this already?".
fn active_bitmap_rasters(
    recipe: &EditRecipe,
) -> impl Iterator<Item = (&crate::recipe::LocalAdjustment, &MaskGeometry, &str)> {
    recipe
        .masks
        .iter()
        .filter(|m| m.enabled && m.amount != 0.0 && engine_active(m))
        .flat_map(|m| {
            std::iter::once(&m.mask)
                .chain(m.components.iter().map(|c| &c.geometry))
                .filter_map(move |g| match g {
                    MaskGeometry::Bitmap { path } => Some((m, g, path.as_str())),
                    _ => None,
                })
        })
}

/// Would a raster of `w`×`h` be loadable again ALONGSIDE the rest of this
/// recipe's active rasters? The writer-side twin of the loader's aggregate
/// charge, asked with the loader's own filter and the loader's own header-only
/// projection — so a full-resolution mask refine cannot publish a raster that
/// every later open/export drops (strictly: `render_to_file` bails and
/// `develop_preview` skips it with a stderr line, i.e. one mask silently stops
/// rendering).
///
/// `replacing` = the raster path this write will REPLACE (the refine target's
/// current raster), excluded from the sum because it will not be loaded again.
/// A raster whose header cannot be read is charged 0 — the same thing the
/// loader can know at this point, and it finds out for real at decode time.
pub fn mask_raster_write_fits_budget(
    recipe: &EditRecipe,
    replacing: Option<&str>,
    w: u32,
    h: u32,
) -> bool {
    let mut projected = raster_bytes(w, h);
    let mut counted: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (_, _, path) in active_bitmap_rasters(recipe) {
        if Some(path) == replacing || !counted.insert(path) {
            continue;
        }
        projected = projected.saturating_add(raster_projected_bytes(path).unwrap_or(0));
    }
    projected <= MASK_RASTER_BUDGET_BYTES
}

fn check_mask_dims(w: u32, h: u32, what: &str) -> anyhow::Result<()> {
    if !mask_raster_fits_budget(w, h) {
        anyhow::bail!(
            "{what} is {w}x{h} — its decoded footprint exceeds the \
             {MASK_RASTER_BUDGET_BYTES}-byte mask budget"
        );
    }
    Ok(())
}

fn load_mask_raster_snapshot_with_budget(
    recipe: &EditRecipe,
    budget_bytes: usize,
    strict: bool,
) -> Result<MaskRasterSnapshot> {
    let mut snapshot = MaskRasterSnapshot::default();
    let mut held_bytes = 0usize;

    // The active set and its per-file projection come from
    // `active_bitmap_rasters` / `raster_projected_bytes` — the same two the
    // writer-side precheck asks (R22 H1), so neither side can drift into a
    // different idea of what is charged.
    for (mask, geometry, path) in active_bitmap_rasters(recipe) {
        if snapshot.images.contains_key(path) {
            continue;
        }
        let label = if mask.name.is_empty() { path } else { mask.name.as_str() };
        // Header-only dimension precheck BEFORE the decode: the budget
        // used to be charged only after image::open had allocated the
        // full raster (plus its native RGBA intermediate), so one
        // compressed large file drove peak memory far past the
        // advertised cap before being rejected.
        if let Some(incoming) = raster_projected_bytes(path) {
            let projected = incoming.saturating_add(held_bytes);
            if projected > budget_bytes {
                if strict {
                    bail!(
                        "mask raster set exceeds the {budget_bytes}-byte aggregate budget while \
                         loading '{path}' for mask '{label}' — no pixels were rendered"
                    );
                }
                eprintln!(
                    "mask raster '{path}' skipped: the active raster set exceeds the \
                     {budget_bytes}-byte aggregate budget"
                );
                continue;
            }
        }
        let Some(bitmap) = load_mask_bitmap(geometry) else {
            if strict {
                bail!(
                    "mask raster '{path}' for mask '{label}' is unreadable — no pixels were rendered"
                );
            }
            continue;
        };
        let incoming = bitmap.as_raw().len();
        let Some(next_bytes) = held_bytes.checked_add(incoming) else {
            if strict {
                bail!(
                    "mask raster set exceeds the {budget_bytes}-byte aggregate budget while \
                     loading '{path}' for mask '{label}' — no pixels were rendered"
                );
            }
            eprintln!(
                "mask raster '{path}' skipped: the active raster set exceeds the \
                 {budget_bytes}-byte aggregate budget"
            );
            continue;
        };
        if next_bytes > budget_bytes {
            if strict {
                bail!(
                    "mask raster set exceeds the {budget_bytes}-byte aggregate budget while \
                     loading '{path}' for mask '{label}' — no pixels were rendered"
                );
            }
            eprintln!(
                "mask raster '{path}' skipped: the active raster set exceeds the \
                 {budget_bytes}-byte aggregate budget"
            );
            continue;
        }
        held_bytes = next_bytes;
        snapshot.images.insert(path.to_string(), bitmap);
    }
    Ok(snapshot)
}

/// The adjustment's COMBINED coverage at normalised (nx, ny): the base
/// geometry's weight folded with each component in list order (Lightroom's
/// Add / Subtract / Intersect grammar — the algebra is documented on
/// [`crate::recipe::MaskCombine`]). Inversion / amount / range are the
/// caller's layers, exactly as with the single-geometry `mask_weight`.
fn combined_mask_weight(
    m: &crate::recipe::LocalAdjustment,
    nx: f32,
    ny: f32,
    base: Option<&image::GrayImage>,
    comp_bmps: &[Option<&image::GrayImage>],
) -> f32 {
    let mut w = mask_weight(&m.mask, nx, ny, base);
    for (c, bmp) in m.components.iter().zip(comp_bmps) {
        let cw = mask_weight(&c.geometry, nx, ny, *bmp);
        w = match c.mode {
            crate::recipe::MaskCombine::Add => 1.0 - (1.0 - w) * (1.0 - cw),
            crate::recipe::MaskCombine::Subtract => w * (1.0 - cw),
            crate::recipe::MaskCombine::Intersect => w * cw,
        };
    }
    w
}

/// Decode the raster of a Bitmap mask geometry, greyscale — through a
/// process-wide (path, mtime)-keyed cache. The GUI re-develops the preview on
/// every slider tick, and decoding the segmentation PNG from DISK per tick
/// dominated the develop whenever a bitmap mask was present. Keyed by mtime
/// because re-running a segmentation OVERWRITES the same file (one raster per
/// photo+target, see the GUI's start_segment) — a path-only key would serve
/// the stale mask forever. Failure warns and returns None (the mask renders
/// inert instead of killing the develop).
fn load_mask_bitmap(g: &MaskGeometry) -> Option<std::sync::Arc<image::GrayImage>> {
    use std::sync::{Arc, Mutex, OnceLock};
    // Keyed by Option<(mtime, size)>: mtime alone misses a same-length-of-
    // time overwrite on coarse-timestamp filesystems (the thumb cache already
    // carries size for the same reason). `None` payload = FAILED to decode —
    // cached so the warning fires once, not on every slider tick. The
    // identity itself is an Option: a MISSING file (no metadata) caches under
    // `None` too — it used to bypass the cache entirely and re-open + re-warn
    // every refresh; the identity flips to Some the moment the file appears,
    // which misses and loads it.
    // Outer None = file MISSING; inner None = mtime unavailable on this
    // filesystem (a distinct, existing-file identity — collapsing the two
    // made a formerly missing mask that appears mtime-less keep hitting the
    // cached negative forever).
    type Key = Option<(Option<std::time::SystemTime>, u64)>;
    type Cache = Mutex<std::collections::HashMap<String, (Key, Option<Arc<image::GrayImage>>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let MaskGeometry::Bitmap { path } = g else { return None };
    let cache = CACHE.get_or_init(Default::default);
    let ident: Key = std::fs::metadata(path)
        .ok()
        .map(|m| (m.modified().ok(), m.len()));
    {
        // No user code runs under the lock, so poisoning is not reachable —
        // recover anyway rather than turning a past panic into a new one.
        let map = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cached_t, img)) = map.get(path.as_str())
            && *cached_t == ident
        {
            return img.clone();
        }
    }
    // Header-only dimension precheck BEFORE the decode (L02): the snapshot
    // loader guards its own calls, but every OTHER path through here (the
    // mask list's ⚠-badge probe, a future direct call) hit image::open
    // unbounded — the decoder allocates the full raster plus its native
    // intermediate before any byte count exists. The refusal is cached under
    // the file's identity below, exactly like a failed decode.
    let over_budget = image::ImageReader::open(path.as_str())
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_some_and(|(w, h)| {
            (w as usize).saturating_mul(h as usize).saturating_mul(4) > MASK_RASTER_BUDGET_BYTES
        });
    let decoded = if over_budget {
        eprintln!(
            "⚠ bitmap mask '{path}' exceeds the {MASK_RASTER_BUDGET_BYTES}-byte mask budget — mask is inert"
        );
        None
    } else {
        match image::open(path) {
            Ok(img) => Some(Arc::new(img.to_luma8())),
            Err(e) => {
                eprintln!("⚠ bitmap mask '{path}' could not be loaded ({e}) — mask is inert");
                None
            }
        }
    };
    {
        let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
        // A recipe holds a handful of masks — a rare hard reset beats
        // LRU bookkeeping on this hot path. Budgeted in BYTES as well
        // as entries: sixteen full-res 61 MP rasters would otherwise
        // pin ~1 GB for the life of the process.
        let held: usize =
            map.values().filter_map(|(_, i)| i.as_ref()).map(|i| i.as_raw().len()).sum();
        let incoming = decoded.as_ref().map_or(0, |i| i.as_raw().len());
        if incoming <= MASK_RASTER_BUDGET_BYTES {
            if map.len() > 16
                || held.saturating_add(incoming) > MASK_RASTER_BUDGET_BYTES
            {
                map.clear();
            }
            map.insert(path.clone(), (ident, decoded.clone()));
        } else {
            map.clear();
        }
    }
    decoded
}

/// Bilinear weight lookup in an 8-bit greyscale mask at normalised (nx, ny).
pub(crate) fn sample_gray_norm(b: &image::GrayImage, nx: f32, ny: f32) -> f32 {
    let (w, h) = (b.width() as f32, b.height() as f32);
    // EXTENT scaling (`* w`), not endpoint scaling (`* (w - 1)`): every
    // producer normalises with `x / w` (apply_masks' weight_at and the overlay
    // builder both say so in their own comments), so mapping onto 0..=size-1
    // here was a DIFFERENT convention. A frame-sized mask then never reached
    // its last row/column — a 2-wide mask holding [0,255] rendered [0, 0.5]
    // instead of [0, 1] — and because the shortfall is one source pixel out of
    // `w`, the same mask landed differently in a 1280 px preview than in a
    // 9504 px export. With extent scaling and nx = x/w the sample index is
    // exactly x for a same-size mask (no interpolation blur at all), and a
    // smaller mask scales proportionally as intended.
    let sx = (nx.clamp(0.0, 1.0) * w).max(0.0).min(w - 1.0);
    let sy = (ny.clamp(0.0, 1.0) * h).max(0.0).min(h - 1.0);
    let x0 = sx.floor().min(w - 1.0);
    let y0 = sy.floor().min(h - 1.0);
    let x1 = (x0 + 1.0).min(w - 1.0);
    let y1 = (y0 + 1.0).min(h - 1.0);
    let (fx, fy) = (sx - x0, sy - y0);
    let g = |x: f32, y: f32| b.get_pixel(x as u32, y as u32)[0] as f32 / 255.0;
    let top = g(x0, y0) * (1.0 - fx) + g(x1, y0) * fx;
    let bot = g(x0, y1) * (1.0 - fx) + g(x1, y1) * fx;
    top * (1.0 - fy) + bot * fy
}

/// Coverage map of ONE local adjustment for on-screen display: geometry ×
/// inversion × amount × range, evaluated with the SAME primitives
/// `apply_masks` uses (`mask_weight` / `range_weight`), so the overlay the
/// GUI paints is the weight the render actually applies. `reference`
/// supplies the pixels the range mask is judged on — pass the develop as it
/// stands when THIS mask runs (its PREFIX: earlier masks applied, matching
/// apply_masks' sequential stacking; the GUI's overlay and range sampler
/// both do). Output is an 8-bit map at the reference's size
/// (255 = full effect), in the ORIGINAL frame like every mask.
pub fn mask_coverage(
    m: &crate::recipe::LocalAdjustment,
    reference: &DynamicImage,
) -> image::GrayImage {
    let rgb = reference.to_rgb8();
    let (w, h) = rgb.dimensions();
    // A muted (eye-toggled) mask applies nothing — advertise nothing.
    if !m.enabled {
        return image::GrayImage::new(w, h);
    }
    let bmp = load_mask_bitmap(&m.mask);
    let comp_bmps: Vec<Option<std::sync::Arc<image::GrayImage>>> =
        m.components.iter().map(|c| load_mask_bitmap(&c.geometry)).collect();
    // Same load-failure contract as `apply_masks` (inert, inversion included,
    // components included), so the overlay never advertises coverage the
    // render will not apply.
    if (bmp.is_none() && matches!(m.mask, MaskGeometry::Bitmap { .. }))
        || m.components
            .iter()
            .zip(&comp_bmps)
            .any(|(c, b)| b.is_none() && matches!(c.geometry, MaskGeometry::Bitmap { .. }))
    {
        return image::GrayImage::new(w, h);
    }
    let comp_refs: Vec<Option<&image::GrayImage>> =
        comp_bmps.iter().map(|bmp| bmp.as_deref()).collect();
    let amount = m.amount.clamp(0.0, 1.0);
    let mut out = image::GrayImage::new(w, h);
    for (x, y, px) in out.enumerate_pixels_mut() {
        // Same normalisation as apply_masks' weight_at (x/w, not x/(w-1)).
        let mut wgt = combined_mask_weight(
            m,
            x as f32 / w as f32,
            y as f32 / h as f32,
            bmp.as_deref(),
            &comp_refs,
        );
        if m.inverted {
            wgt = 1.0 - wgt;
        }
        wgt *= amount;
        if wgt > 0.001
            && let Some(rm) = &m.range
        {
            let p = rgb.get_pixel(x, y);
            wgt *= range_weight(
                rm,
                &[p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0],
            );
        }
        *px = image::Luma([(wgt.clamp(0.0, 1.0) * 255.0).round() as u8]);
    }
    out
}

/// Per-pixel Range Mask weight [0,1] — Lightroom's 范围蒙版, multiplied into the
/// geometric mask weight (intersection).
///
/// * `Luminance`: trapezoid over `LumRange` — smooth ramp lo_outer→lo, hold 1
///   across lo..hi, ramp down hi→hi_outer. Degenerate edges (outer == inner,
///   e.g. ACR's real `"… 1.000000 1.000000"`) become hard steps.
/// * `Color`: falloff on the luminance-invariant chromaticity distance to the
///   reference colour (each colour divided by its own luma), so a darker patch
///   of the same hue still matches. `amount` widens tolerance: at the LR-default
///   0.5 a saturated reference keeps same-hue pixels (d=0), rejects neutral grey
///   (d≈0.8) and opposite hues (d≳2); at 1.0 grey gains partial weight. Very
///   dark pixels (luma < 1e-4) have no reliable chroma and get weight 0.
pub fn range_weight(rm: &RangeMask, px: &[f32; 3]) -> f32 {
    match rm {
        RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => {
            let l = luma601(px);
            // The upper edge must stay INCLUSIVE at l == hi (the trapezoid
            // holds 1 across lo..=hi): ramp's degenerate step counts x == e0
            // as already past, so a full-range {0,0,1,1} mask silently
            // rejected pure white. The lower edge needs no twin — ramp's
            // step already includes l == lo on the hold side.
            let up = if *hi_outer - *hi < 1e-6 {
                if l > *hi { 1.0 } else { 0.0 }
            } else {
                ramp(*hi, *hi_outer, l)
            };
            ramp(*lo_outer, *lo, l) * (1.0 - up)
        }
        RangeMask::Color { r, g, b, amount, .. } => {
            // Documented: very dark pixels have no reliable chroma and get
            // weight 0 — clamping instead let a black pixel match a black
            // reference at full weight through arbitrary 1e-4/1e-4 ratios.
            // The REFERENCE is held to the same rule: flooring a near-black
            // reference and normalising it made it read as ordinary grey and
            // select bright neutral regions at full weight.
            if luma601(px) < 1e-4 || luma601(&[*r, *g, *b]) < 1e-4 {
                return 0.0;
            }
            let rl = luma601(&[*r, *g, *b]).max(1e-4);
            let pl = luma601(px).max(1e-4);
            let mut d2 = 0.0;
            for (rc, pc) in [(*r, px[0]), (*g, px[1]), (*b, px[2])] {
                let diff = rc / rl - pc / pl;
                d2 += diff * diff;
            }
            let d = d2.sqrt();
            let d_max = 0.15 + 0.9 * amount.clamp(0.0, 1.0);
            1.0 - ramp(0.5 * d_max, d_max, d)
        }
    }
}

/// Scale a pixel's chroma so its luma moves `l_old`→`l_new` while preserving hue.
fn scale_chroma(px: &mut [f32; 3], l_old: f32, l_new: f32) {
    if l_old > 1e-4 {
        let k = l_new / l_old;
        px[0] = (px[0] * k).clamp(0.0, 1.0);
        px[1] = (px[1] * k).clamp(0.0, 1.0);
        px[2] = (px[2] * k).clamp(0.0, 1.0);
    } else {
        *px = [l_new, l_new, l_new];
    }
}

/// Unsharp mask on luminance (chroma-preserving). `amount` scales the detail;
/// `midtone` weights the effect toward midtones (for clarity).
fn unsharp_luma(data: &mut [[f32; 3]], w: usize, h: usize, radius: usize, amount: f32, midtone: bool) {
    unsharp_luma_weighted(data, w, h, radius, amount, midtone, |_, _, _| 1.0);
}

/// [`unsharp_luma`] with a per-pixel weight — the LOCAL-mask form (clarity /
/// texture inside a mask). `weight(x, y, px)` receives the pixel as it stands
/// when the pass reaches it, so the caller can fold a Range Mask into the
/// geometric coverage; weights ≤ 0.001 skip the pixel untouched.
///
/// **The weighting is EXACT, not an approximation.** `unsharp_luma` scales a
/// pixel's RGB by `k = new_l/l` (`scale_chroma`), so the filtered pixel is
/// `p·k`. Mixing the original and the filtered result by weight `w` gives
/// `p·(1−w) + p·k·w = p·(1 + w(k−1))`, i.e. the pixel scaled by the
/// weight-interpolated ratio `1 + w(k−1)`. Attenuating the LUMA DIFFERENCE by
/// `w` before `scale_chroma` produces `new_l' = l + w·(new_l − l)`, whose ratio
/// is `k' = new_l'/l = 1 + w(k−1)` — the same number. So "weight the detail"
/// and "filter the whole frame, then blend by weight" are the same operation,
/// and this needs only the two f32 planes the filter already builds instead of
/// a full RGB copy of the frame to blend against (~732 MB at 61 MP).
/// (Exactness holds up to the two clamps `scale_chroma` and the `new_l` clamp
/// apply; at `w = 1` the arithmetic is bit-identical to `unsharp_luma` — test
/// `unsharp_weighted_at_weight_one_is_bit_identical`.)
fn unsharp_luma_weighted(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    radius: usize,
    amount: f32,
    midtone: bool,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
) {
    if w == 0 || h == 0 {
        return; // par_chunks_mut(0) asserts; a 0-dim frame has no pixels anyway
    }
    let luma: Vec<f32> = data.par_iter().map(luma601).collect();
    let blurred = blur_plane(&luma, w, h, radius);
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let wgt = weight(x, y, px);
            if wgt <= 0.001 {
                continue;
            }
            let i = y * w + x;
            let l = luma[i];
            let detail = l - blurred[i];
            let m = if midtone { 1.0 - (2.0 * l - 1.0).powi(2) } else { 1.0 };
            let new_l = (l + amount * detail * m * wgt).clamp(0.0, 1.0);
            scale_chroma(px, l, new_l);
        }
    });
}

/// Bilateral-lite luminance denoise: smooth flat areas, keep edges. `t` in 0..1.
/// `denoised = l − t·w_edge·detail`, w_edge≈1 in flat regions, ≈0 at edges.
fn noise_reduce_luma(data: &mut [[f32; 3]], w: usize, h: usize, t: f32) {
    let luma: Vec<f32> = data.par_iter().map(luma601).collect();
    let radius = (1.0 + 2.0 * t).round().max(1.0) as usize;
    let blurred = blur_plane(&luma, w, h, radius);
    let range = 0.05_f32;
    data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let l = luma[i];
        let detail = l - blurred[i];
        let w_edge = (-(detail / range) * (detail / range)).exp();
        let new_l = (l - t * w_edge * detail).clamp(0.0, 1.0);
        scale_chroma(px, l, new_l);
    });
}

/// Approximate a Gaussian blur with 3 separable box-blur passes. Box blur uses a
/// running sum, so cost is O(N) regardless of `radius` — essential for clarity's
/// large radius on a 60 MP frame.
fn blur_plane(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    // Zero-dim guard: the box-blur seeds use `Ord::clamp(0, w-1)`, which PANICS
    // when w or h is 0 (min > max). No caller produces a 0-dim buffer today —
    // this turns a future one into a no-op instead of a crash.
    if radius == 0 || w == 0 || h == 0 {
        return src.to_vec();
    }
    // The first pass reads the caller's plane directly — the old
    // `src.to_vec()` seed was a full extra plane (~240 MB at 61 MP) copied
    // only to be replaced by the first horizontal pass (A7). Bit-identical:
    // that pass reads exactly the same values either way.
    let mut buf = box_blur_h(src, w, h, radius);
    buf = box_blur_v(&buf, w, h, radius);
    for _ in 0..2 {
        buf = box_blur_h(&buf, w, h, radius);
        buf = box_blur_v(&buf, w, h, radius);
    }
    buf
}

fn box_blur_h(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    debug_assert_eq!(src.len(), w * h);
    let mut out = vec![0.0f32; src.len()];
    let r = radius as isize;
    let win = (2 * radius + 1) as f32;
    // Rows are independent → parallel (row count now comes from the chunking,
    // not `h`); the per-row arithmetic order is exactly the serial version's,
    // so the result is bit-identical.
    out.par_chunks_mut(w).enumerate().for_each(|(y, orow)| {
        let base = y * w;
        let mut sum = 0.0f32;
        for k in -r..=r {
            sum += src[base + k.clamp(0, w as isize - 1) as usize];
        }
        orow[0] = sum / win;
        for (x, o) in orow.iter_mut().enumerate().skip(1) {
            let add = (x as isize + r).min(w as isize - 1) as usize;
            let sub = (x as isize - 1 - r).max(0) as usize;
            sum += src[base + add] - src[base + sub];
            *o = sum / win;
        }
    });
    out
}

fn box_blur_v(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; src.len()];
    let r = radius as isize;
    let win = (2 * radius + 1) as f32;
    // Row-major with one running sum PER COLUMN: the old column-major walk
    // strode 4·w bytes on all three access streams, so export-sized planes
    // fell out of cache into DRAM latency for most of the pass. Every access
    // below is sequential, and each column's adds/subs happen in the exact
    // order of the old per-column walk — the result is bit-identical.
    let mut sums = vec![0.0f32; w];
    for k in -r..=r {
        let row = &src[k.clamp(0, h as isize - 1) as usize * w..][..w];
        for (s, v) in sums.iter_mut().zip(row) {
            *s += v;
        }
    }
    for (o, s) in out[..w].iter_mut().zip(&sums) {
        *o = s / win;
    }
    for y in 1..h {
        let add = &src[(y as isize + r).min(h as isize - 1) as usize * w..][..w];
        let sub = &src[(y as isize - 1 - r).max(0) as usize * w..][..w];
        let orow = &mut out[y * w..][..w];
        for x in 0..w {
            sums[x] += add[x] - sub[x];
            orow[x] = sums[x] / win;
        }
    }
    out
}

// ---------------------------------------------------------------------------

/// Blackbody colour at temperature `k` Kelvin as RGB in [0,1].
/// Tanner-Helland piecewise fit [verified: tannerhelland.com/2012/09/18,
/// R²>0.987], with the cool-side branches RESCALED so every branch seam is
/// continuous. Valid 1000–40000 K.
///
/// The published constants leave cliffs at the 6600 K seam — green jumps
/// 1.31 % and blue 0.96 % — and the cool red branch starts at 259.7, which
/// the clamp holds flat at 255 until 6688 K. `wb_gains` divides one of these
/// curves by another, so at the seam two near-identical temperatures produced
/// visibly different gains, and inside the 88 K red plateau the r/b ratio —
/// the eyedropper's temperature signal — did not move at all. Each cool
/// branch keeps the published EXPONENT (the fit's shape) and gets its
/// coefficient(s) recalibrated on the seam values instead:
///   red   329.69873 → 323.73796  (branch(66) = 255 exactly, plateau gone)
///   green 288.12216 → 291.94575  (branch(66) = 255 = warm branch, clamped)
///   blue  a·ln(t−10)−b with a 138.51773 → 139.48702, b 305.0448 → 306.48430
///         (two-point: blue(6600 K) = 255 AND blue(1900 K) = 0, so repairing
///         the top seam does not open one at the bottom).
/// Worst mid-range deviation from the published fit is under 2 % of
/// full-scale — smaller than the discontinuities it removes.
fn kelvin_to_rgb(k: f32) -> [f32; 3] {
    let t = k.clamp(1000.0, 40000.0) / 100.0;
    let red = if t <= 66.0 {
        255.0
    } else {
        (323.737_96 * (t - 60.0).powf(-0.133_204_76)).clamp(0.0, 255.0)
    };
    let green = if t <= 66.0 {
        (99.470_8 * t.ln() - 161.119_57).clamp(0.0, 255.0)
    } else {
        (291.945_75 * (t - 60.0).powf(-0.075_514_846)).clamp(0.0, 255.0)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        (139.487_02 * (t - 10.0).ln() - 306.484_3).clamp(0.0, 255.0)
    };
    [red / 255.0, green / 255.0, blue / 255.0]
}

/// Per-channel gains to move WB from `as_shot_k` to `target_k` (+ tint), green
/// normalised to 1.0 (WB changes colour, not brightness). Lightroom convention:
/// higher target K = warmer result (boosts red, cuts blue). `pub(crate)` so the
/// zoned fit can INVERT the engine's own model instead of duplicating it.
pub(crate) fn wb_gains(as_shot_k: f32, target_k: f32, tint: f32) -> [f32; 3] {
    let a = kelvin_to_rgb(as_shot_k);
    let t = kelvin_to_rgb(target_k);
    let g1 = a[1] / t[1].max(1e-4);
    let gr = (a[0] / t[0].max(1e-4)) / g1;
    let gb = (a[2] / t[2].max(1e-4)) / g1;
    // Tint: positive = magenta (less green), negative = green.
    let gg = 1.0 - 0.20 * (tint / 100.0);
    [gr, gg, gb]
}

/// Map the local mask Temp slider — a RELATIVE warm/cool shift, ±100 (see
/// `LocalAdjustment::temperature`) — to the target Kelvin the shared
/// [`wb_gains`] model expects: a linear shift in MIRED (1e6/K, the unit
/// photographic conversion gels are specified in, ~perceptually uniform for
/// WB) around a FIXED 5500 K anchor. (The global stage anchored there too
/// until batch 29 taught it the photo's stamped as-shot Kelvin; a local
/// slider is a relative gel, so it keeps the fixed anchor and the two
/// deliberately differ — R12.) Full scale ±100
/// ⇒ ∓80 mired (≈ half a CTO/CTB gel): +100 → ~9823 K (warmer — matching
/// wb_gains' "higher target K = warmer" convention), −100 → ~3820 K. Both
/// endpoints sit inside kelvin_to_rgb's 1000–40000 K validity. ACR's exact
/// local-temp model is proprietary — this is our documented approximation
/// (same stance as [`apply_vignette`]); the XMP carries the raw slider value,
/// so Lightroom re-renders with its own model.
// `pub`, not `pub(crate)`: the mask panel's Temp-shift tooltip states the
// equivalent Kelvin for the value on the slider, and it must be THIS
// function's answer — a number retyped into the GUI would drift the moment
// the anchor or the mired scale moves.
pub fn local_temp_to_kelvin(t: f32) -> f32 {
    const ANCHOR_K: f32 = 5500.0;
    const MIRED_FULL_SCALE: f32 = 80.0;
    let mired = 1e6 / ANCHOR_K - (t.clamp(-100.0, 100.0) / 100.0) * MIRED_FULL_SCALE;
    1e6 / mired
}

/// Build one LUT per channel for a linear-light RGB gain. The exact transform
/// is `linear_to_srgb(srgb_to_linear(x) * gain)`, but evaluating both transfer
/// curves (`powf`) for every pixel/channel dominated v0.8 zoned preview time:
/// measured on the production-shaped 1280×853 probe, one colour-gain bitmap
/// mask cost 609 ms vs 53 ms for the SAME mask without colour gains; the
/// sky+land pair cost 1188 ms vs 92 ms. A 4096-entry LUT evaluates the exact
/// formula only 12k times per adjustment, then the existing linear sampler
/// handles millions of pixels. `LUT_N=4096` keeps interpolation error below
/// the engine's 8/16-bit output quantisation (pinned by the test below).
fn colour_gain_luts(g: [f32; 3]) -> [Vec<f32>; 3] {
    std::array::from_fn(|ch| {
        (0..LUT_N)
            .map(|i| {
                let x = i as f32 / (LUT_N - 1) as f32;
                linear_to_srgb((srgb_to_linear(x) * g[ch]).clamp(0.0, 1.0))
            })
            .collect()
    })
}

/// Apply white-balance gains in linear light. No-op when gains are ~neutral.
/// Uses [`colour_gain_luts`] so preview cost scales with pixels, not with six
/// transcendental operations per pixel.
fn apply_wb(data: &mut [[f32; 3]], as_shot_k: f32, target_k: f32, tint: f32) {
    let g = wb_gains(as_shot_k, target_k, tint);
    if (g[0] - 1.0).abs() < 1e-3 && (g[1] - 1.0).abs() < 1e-3 && (g[2] - 1.0).abs() < 1e-3 {
        return;
    }
    let luts = colour_gain_luts(g);
    // Row-parallel like every other per-pixel stage (the v0.11.0 sweep took
    // the tone, HSL, grading, curve, dehaze and vignette passes; this one was
    // left serial and is the stage EVERY Temp/Tint tick runs through, at full
    // sensor resolution on export). Each pixel reads only itself and the
    // read-only LUTs, so the result is bit-identical — no accumulation and no
    // order dependence to change.
    data.par_iter_mut().for_each(|px| {
        for c in 0..3 {
            px[c] = sample_lut(&luts[c], px[c]);
        }
    });
}

/// The ONE recipe→WB stage, shared by the full-res render, the baked-image
/// render and the UI preview so they can never disagree. The buffer arrives
/// at as-shot WB; the shift is anchored at the photo's STAMPED as-shot
/// Kelvin (`as_shot_k`, engine-only — [`as_shot_wb`]), so `temperature_k`
/// finally speaks ABSOLUTE Kelvin: target == as-shot is a true no-op, and
/// the number agrees with what Lightroom shows for the same XMP. A legacy
/// recipe (`None`) keeps the historical 5500 K daylight anchor —
/// byte-identical rendering of every old archive. `temperature_k = None`
/// only means "no Kelvin shift" (the target IS the anchor) — tint still
/// applies on its own, matching the recipe contract (tint 0 = neutral) and
/// what the GUI slider promises.
fn apply_recipe_wb(data: &mut [[f32; 3]], r: &EditRecipe) {
    if r.temperature_k.is_some() || r.tint != 0.0 {
        let anchor = r.as_shot_k.unwrap_or(5500.0);
        apply_wb(data, anchor, r.temperature_k.unwrap_or(anchor), r.tint);
    }
}

/// Inverse white balance — the WB eyedropper's solver. Given an sRGB pixel the
/// user says SHOULD be neutral, find the (target Kelvin, tint) whose
/// [`wb_gains`] neutralise it, using the exact forward model the render then
/// applies — anchored at `as_shot_k` (the photo's stamped as-shot Kelvin, or
/// the legacy 5500 K), so the solved Kelvin lands in the same absolute scale
/// the Temp slider now speaks. Target K is scanned on a log grid (400 steps
/// over the recipe's legal 2000–40000 K) to equalise the red/blue channels;
/// tint then falls analytically out of the green residual
/// (gg = 1 − 0.20·tint/100). Returns (kelvin, tint clamped to ±100).
pub fn solve_wb_from_neutral(px: [f32; 3], as_shot_k: f32) -> (f32, f32) {
    let lr = srgb_to_linear(px[0]).max(1e-5);
    let lg = srgb_to_linear(px[1]).max(1e-5);
    let lb = srgb_to_linear(px[2]).max(1e-5);
    const N: usize = 400;
    let (lo, hi) = ((2000.0f32).ln(), (40000.0f32).ln());
    let mut best = (as_shot_k, f32::INFINITY);
    for i in 0..=N {
        let k = (lo + (hi - lo) * i as f32 / N as f32).exp();
        let g = wb_gains(as_shot_k, k, 0.0);
        let e = (lr * g[0] - lb * g[2]).abs();
        if e < best.1 {
            best = (k, e);
        }
    }
    let k = best.0;
    let g = wb_gains(as_shot_k, k, 0.0);
    // Green gain that lands green on the (now equal) red/blue level → tint.
    // Bounded to the gg range tint can actually express (tint ±100 ⇒ gg 0.8–1.2).
    let level = 0.5 * (lr * g[0] + lb * g[2]);
    let gg = (level / lg).clamp(0.8, 1.2);
    let tint = ((1.0 - gg) / 0.20 * 100.0).clamp(-100.0, 100.0);
    (k, tint)
}

// `pub(crate)`: the zoned fit computes zone moments in linear light with the
// engine's exact transfer curve (a duplicated constant would drift).
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `smoothstep` with a degenerate-edge guard: equal edges are a hard step
/// instead of the 0/0 NaN `smoothstep` returns there (and `clamp` propagates).
/// Identical to `smoothstep` whenever `e1 - e0 >= 1e-6`.
fn ramp(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 - e0 < 1e-6 {
        if x < e0 { 0.0 } else { 1.0 }
    } else {
        smoothstep(e0, e1, x)
    }
}

/// Tone-model knot inputs. 0.66 is an explicit vertex so mid-bright water (≈0.66)
/// stays separated from the midtone (0.50) under a strong −Highlights; 0.82 shapes
/// the highlight shoulder; 0.92 is where whites concentrate. Shared with the
/// reverse-fit (fit.rs), which solves slider values against this same model.
pub(crate) const TONE_KNOTS_X: [f32; 8] = [0.0, 0.10, 0.25, 0.50, 0.66, 0.82, 0.92, 1.0];

/// Per-slider knot-output basis at input `x`: how far a fully-pushed +100 slider
/// moves the knot, in order `[contrast, highlights, shadows, whites, blacks]`.
/// The knot output is `tone_exposure_curve(x, ev) + basis · sliders/100`; keeping
/// this the ONLY definition means render and reverse-fit cannot drift apart.
pub(crate) fn tone_slider_basis(x: f32) -> [f32; 5] {
    // Authority: how far a fully-pushed ±100 slider moves its knot(s).
    const A_SHADOW: f32 = 0.33;
    const A_HIGHLIGHT: f32 = 0.34;
    const A_CONTRAST: f32 = 0.20;
    const A_WB: f32 = 0.32; // whites & blacks share it

    // Region basis functions over knot input x (each ∈ [0,1]).
    let w_shadow = smoothstep(0.0, 0.25, x) * (1.0 - smoothstep(0.25, 0.50, x));
    // highlights: peak 0.82, ZERO at 0.50 and PINNED to 0 at 1.0 — so highlights can
    // never move the white point; specular foam near white is never dragged down.
    let w_high = smoothstep(0.60, 0.82, x) * (1.0 - smoothstep(0.82, 1.0, x));
    // contrast: shoulder lobe minus toe lobe → antisymmetric, 0 at the ends and 0.50.
    let w_contrast = smoothstep(0.50, 0.75, x) * (1.0 - smoothstep(0.75, 1.0, x)) - w_shadow;
    // whites/blacks own the literal end knots (+ a touch of the adjacent knot).
    let w_white = if x >= 0.999 {
        1.0
    } else if (x - 0.92).abs() < 1e-3 {
        0.45
    } else {
        0.0
    };
    let w_black = if x <= 0.001 {
        1.0
    } else if (x - 0.10).abs() < 1e-3 {
        0.45
    } else {
        0.0
    };
    [
        A_CONTRAST * w_contrast,
        A_HIGHLIGHT * w_high,
        A_SHADOW * w_shadow,
        A_WB * w_white,
        A_WB * w_black,
    ]
}

/// The exposure component of a knot output: a linear-light gain of `ev` stops
/// applied under the sRGB transfer curve (the identity curve when ev = 0).
pub(crate) fn tone_exposure_curve(x: f32, ev: f32) -> f32 {
    linear_to_srgb((srgb_to_linear(x) * 2.0_f32.powf(ev)).clamp(0.0, 1.0))
}

/// Per-knot slider AUTHORITY under the current exposure — the tone-model
/// repair for the residual `limit_tone_sliders` documents below.
///
/// The knot model adds `basis(x)·sliders` at knot inputs in the ORIGINAL
/// tonal axis, so the basis does not know when exposure has already collapsed
/// the base curve around a knot: at +1.5 EV every knot from x = 0.66 up sits
/// at base 1.0, `contrast: -100` then writes 1.0 − 0.141 = 0.859 at 0.66 and
/// a DEEPER dip at 0.82, and the monotone backstop flattens the whole
/// [0.66, 0.82] interval at an interior grey — the measured 197-input plateau
/// at code 56304. The λ limiter cannot help: those intervals have
/// `base_gap ≤ 1e-6` and are rightly its skip case (exposure clipped them;
/// that is what exposure means).
///
/// So authority follows the base curve's own local separation: a knot keeps
/// full weight while at least one adjacent base interval is still open
/// (`ramp(0, 0.01, healthiest adjacent gap)`), and fades to zero where
/// exposure has saturated BOTH sides. A muted knot stays on the base curve,
/// the saturated run stays at the ceiling/floor, and a strong slider there
/// now yields honest clipping instead of an interior flat band — Lightroom's
/// own semantics for a slider aimed at a region exposure already clipped.
///
/// Exactness matters twice: at ev = 0 every gap is ≥ 0.08, `ramp` saturates
/// to exactly 1.0, and every render is bit-for-bit unchanged; the boundary
/// knot of a saturated run keeps weight through its healthy side, so the
/// slider's effect on the still-alive interval below is preserved, not
/// chopped at the run's edge. Endpoint knots have one neighbour — the
/// missing side counts as closed, so a fully-clipped end loses authority
/// with the run it belongs to.
///
/// Shared by all three model sites (`build_tone_lut`, `limit_tone_sliders`,
/// `fit.rs::fit_tone_sliders`); weights depend only on `ev`, so the model
/// stays LINEAR in the sliders and the reverse-fit still inverts it
/// analytically.
pub(crate) fn tone_knot_weights(ev: f32) -> [f32; 8] {
    // Full authority once the healthiest adjacent interval separates by 1 %
    // of the axis — comfortably under the 0.08 minimum an ev = 0 grid has,
    // comfortably over the 1e-6 the λ limiter treats as closed.
    const GAP_FULL: f32 = 0.01;
    let base: [f32; 8] = std::array::from_fn(|i| tone_exposure_curve(TONE_KNOTS_X[i], ev));
    std::array::from_fn(|i| {
        let left = if i > 0 { base[i] - base[i - 1] } else { 0.0 };
        let right = if i < 7 { base[i + 1] - base[i] } else { 0.0 };
        ramp(0.0, GAP_FULL, left.max(right))
    })
}

/// Scale a slider vector `[contrast, highlights, shadows, whites, blacks]`
/// (each already in −1..1) down to the strongest version of ITSELF that no
/// longer collapses a tonal band. A slider must SATURATE, never annihilate.
///
/// The knots sit only 0.08–0.25 apart and nothing used to check that a
/// slider's offset fitted in the gap. Past a threshold a knot overtook its
/// neighbour and the repair in [`build_tone_lut`] — snap to `prev + 1e-4` —
/// turned that whole interval FLAT; Fritsch–Carlson then zeroed both tangents,
/// making it exactly flat, so every input tone in the interval rendered to one
/// output value. Clamping to 1.0 did the same at the top end. These are
/// ORDINARY edits, not abuse — measured on the pre-fix engine through the real
/// export path on a 16-bit ramp:
///
///   * `whites: -50` mapped input 0.9568–0.9731 to a single code and cut the
///     top decade from 411 distinct codes to 75.
///   * `highlights: +60` mapped everything above 0.8195 to pure white — 18 %
///     of the range, 740 of 4096 sampled inputs.
///
/// Detail destroyed here is not recoverable by any later stage. Four rounds of
/// tests missed it because a flat band is still monotone and still pins its
/// endpoints, which is all they asserted.
///
/// Returning scaled SLIDERS rather than a repaired curve is deliberate: the
/// knot model stays linear in the sliders, which is what lets `fit.rs` invert
/// it analytically. ONE caller applies this — `build_tone_lut`. The reverse
/// fit deliberately does not (see the note at the end of `fit_tone_sliders`):
/// it scores candidates by rendering them, so it already measures whatever
/// the engine does, and pre-applying the limiter perturbed a solve that was
/// tuned against a knife-edge acceptance test.
///
/// λ = 1 whenever nothing binds, so every edit inside the thresholds renders
/// bit-for-bit as before; only the region that was being destroyed changes.
///
/// Both of this design's measured gaps are CLOSED, by different halves of
/// the model (grid = 18 recipes × 15 exposures on the real engine, counting
/// only INTERIOR plateaus — a run at 0 or 65535 is clipping, which is what a
/// strong slider on a bright frame is meant to do):
///
///   * The high-exposure residual (worst cell `contrast: -100` at `+1.5 EV`,
///     a 197-input plateau at code 56304; grid 13 cells > 96) was the knot
///     BASIS not knowing exposure had saturated the base curve around a knot
///     — fixed in the model by [`tone_knot_weights`], not here. Four
///     knot-level repairs measured before it (including a pre-weights
///     per-slider λ: 13 cells → 9 but worst 197 → 317) all traded the tail
///     for a worse one; the model fix took the grid to 6 cells > 96.
///   * The collateral gap — ONE λ scaled the whole vector, so pinning
///     shadows +50 while dragging whites −45 → −100 rendered the shadows at
///     22.5 — is closed by the per-slider iteration below: only the sliders
///     that CLOSE the worst-violated interval shrink, the single-λ pass
///     stays as the unconditional backstop, and the grid worst is now 100 —
///     the same level the ev = 0 design holds
///     (`a_slider_that_binds_an_interval_no_longer_drags_the_innocent_ones`).
pub(crate) fn limit_tone_sliders(ev: f32, s: [f32; 5]) -> [f32; 5] {
    // The share of an interval's EXISTING separation the sliders must leave
    // behind. Two calibration notes, both learned the hard way:
    //
    //   * Phrased against the base curve's own gap, NOT against the identity
    //     slope. An absolute floor has a cliff wherever exposure has already
    //     narrowed an interval to just above it; there λ collapses to ~0 and
    //     silently zeroes every slider at once.
    //   * Small on purpose. This exists to stop a band COLLAPSING, not to
    //     reserve a fixed share of every interval. At 0.05 it bound on an
    //     ordinary reverse-fit result (contrast +40.4 with shadows −42.8 puts
    //     λ at 0.990 for the 0.10–0.25 interval), which perturbed the solve
    //     that fit.rs had already tuned to that fixture. At 0.01 the same
    //     recipe is unconstrained (λ = 1.03) while `highlights: +60` is still
    //     cut to ~47 and `whites: -50` to ~45 — the settings that were
    //     destroying detail. 1 % of a 0.147-wide interval is still 96 distinct
    //     16-bit codes, which is a gradient, not a flat patch.
    const KEEP: f32 = 0.01;
    let weights = tone_knot_weights(ev);
    // Per-slider differential contribution to each interval: how much slider
    // k (at full value s[k]) changes the separation of interval i. Weighted —
    // an unweighted λ would limit against offsets the engine no longer adds.
    let mut base = [0.0f32; 8];
    let mut wb = [[0.0f32; 5]; 8];
    for (i, &x) in TONE_KNOTS_X.iter().enumerate() {
        let b = tone_slider_basis(x);
        base[i] = tone_exposure_curve(x, ev);
        for k in 0..5 {
            wb[i][k] = weights[i] * b[k] * s[k];
        }
    }

    // PER-SLIDER λ, iteratively: for the worst-violated interval, shrink only
    // the sliders whose contribution CLOSES it, by exactly the factor that
    // interval needs; repeat. Sliders that open the interval — or act
    // elsewhere — keep their authority (the old single λ scaled the whole
    // vector, so pinning shadows +50 while dragging whites −45 → −100 pulled
    // the rendered shadows down to 22.5 with it). A shrink here can deepen a
    // violation in ANOTHER interval that the shrunk slider was helping to
    // hold open (contrast is antisymmetric), so this iterates to a fixpoint —
    // and the single-λ pass below remains as the unconditional backstop, so
    // the hard guarantee never rests on convergence.
    let mut lam = [1.0f32; 5];
    for _ in 0..8 {
        // Worst violation under the CURRENT per-slider scales.
        let mut worst: Option<(usize, f32)> = None; // (interval, allowed/actual)
        for i in 1..8 {
            let gap = base[i] - base[i - 1];
            if gap <= 1e-6 {
                continue; // exposure's own clipping — see tone_knot_weights
            }
            let d: f32 = (0..5).map(|k| (wb[i][k] - wb[i - 1][k]) * lam[k]).sum();
            let allowed = -(1.0 - KEEP) * gap;
            if d < allowed {
                let ratio = allowed / d; // in (0,1): fraction of d that fits
                if worst.is_none_or(|(_, r)| ratio < r) {
                    worst = Some((i, ratio));
                }
            }
        }
        let Some((i, _)) = worst else { break };
        let gap = base[i] - base[i - 1];
        let allowed = -(1.0 - KEEP) * gap;
        let (mut open, mut close) = (0.0f32, 0.0f32);
        for k in 0..5 {
            let c = (wb[i][k] - wb[i - 1][k]) * lam[k];
            if c < 0.0 { close += c } else { open += c }
        }
        // f·close + open ≥ allowed  ⇒  f = (allowed − open) / close, in [0,1):
        // `close < allowed − open ≤ 0` here, since the interval is violated
        // and `allowed − open ≤ allowed < 0`.
        let f = ((allowed - open) / close).clamp(0.0, 1.0);
        for k in 0..5 {
            if (wb[i][k] - wb[i - 1][k]) * lam[k] < 0.0 {
                lam[k] *= f;
            }
        }
    }

    // Unconditional single-λ backstop over whatever the iteration left: the
    // band-collapse guarantee is enforced HERE, not by convergence above.
    // λ = 1 whenever nothing binds, and then every knot — and so every
    // rendered pixel — is bit-for-bit what the per-slider scales produced.
    let mut lambda = 1.0f32;
    for i in 1..8 {
        let gap = base[i] - base[i - 1];
        if gap <= 1e-6 {
            continue;
        }
        let d: f32 = (0..5).map(|k| (wb[i][k] - wb[i - 1][k]) * lam[k]).sum();
        if d < 0.0 {
            lambda = lambda.min((1.0 - KEEP) * gap / -d);
        }
    }
    let lambda = lambda.clamp(0.0, 1.0);
    std::array::from_fn(|k| s[k] * lam[k] * lambda)
}

/// Build the develop tone curve as a [`LUT_N`]-entry LUT over input gamma [0,1].
///
/// It is an 8-knot control-point curve fit by a MONOTONE cubic Hermite spline
/// (Fritsch–Carlson), so it is monotone *by construction* (no post-hoc clamp) and
/// the endpoints are pinned. Exposure is a linear-light gain applied before the
/// curve; contrast is an antisymmetric S; shadows/highlights shape the toe/shoulder
/// WITHOUT reaching the midtones or the white point (so a strong −Highlights can't
/// drag specular foam to grey — that is the white point's job, owned by whites);
/// whites/blacks move the end knots. The recipe's own `tone_curve` is composed on
/// top. This replaces a summed-region-hump model that could go non-monotonic and
/// crush mid-bright water / near-white foam (which had needed ad-hoc patches).
pub(crate) fn build_tone_lut(r: &EditRecipe) -> Vec<f32> {
    // Knot OUTPUTS: exposure-mapped identity, then the slider offsets — all from
    // the shared basis below so the reverse-fit (fit.rs) solves against the SAME
    // model the engine renders.
    let contrast = (r.contrast / 100.0).clamp(-1.0, 1.0);
    let highlights = (r.highlights / 100.0).clamp(-1.0, 1.0);
    let shadows = (r.shadows / 100.0).clamp(-1.0, 1.0);
    let whites = (r.whites / 100.0).clamp(-1.0, 1.0);
    let blacks = (r.blacks / 100.0).clamp(-1.0, 1.0);

    // Saturate the slider vector BEFORE using it, so the knot model below
    // stays exactly what it always was: base + basis·sliders, linear in the
    // sliders. That linearity is load-bearing — `fit_tone_sliders` inverts it
    // analytically — so the limit is applied to the SLIDERS, once, here and in
    // the fit, rather than to the curve afterwards.
    let [contrast, highlights, shadows, whites, blacks] =
        limit_tone_sliders(r.exposure_ev, [contrast, highlights, shadows, whites, blacks]);


    let mut ys = [0.0f32; 8];
    let weights = tone_knot_weights(r.exposure_ev);
    for (idx, &x) in TONE_KNOTS_X.iter().enumerate() {
        let b = tone_slider_basis(x);
        // Knot authority fades where exposure saturated BOTH adjacent base
        // intervals (see tone_knot_weights): a strong slider aimed at a
        // region exposure already clipped yields honest clipping, not the
        // interior flat band the backstop below used to manufacture.
        ys[idx] = tone_exposure_curve(x, r.exposure_ev)
            + weights[idx]
                * (b[0] * contrast
                    + b[1] * highlights
                    + b[2] * shadows
                    + b[3] * whites
                    + b[4] * blacks);
    }
    // Backstop only. λ above already keeps the SLIDERS from closing an
    // interval, so this now fires just where exposure itself saturated the
    // knots (the `base_gap <= need` skip), which is exposure's prerogative.
    // Kept unchanged and deliberately minimal: with the limiter in place a
    // minimum-slope version of this loop changes nothing a test can see
    // (verified by mutation), and monotonicity is all it owes.
    // Fritsch–Carlson on monotone data ⇒ the whole spline is monotone, so
    // there is NO running-max pass over the sampled LUT — it is structural.
    const EPS: f32 = 1e-4;
    for i in 1..ys.len() {
        if ys[i] < ys[i - 1] + EPS {
            ys[i] = ys[i - 1] + EPS;
        }
    }
    for v in &mut ys {
        *v = v.clamp(0.0, 1.0);
    }

    let m = fc_tangents(&TONE_KNOTS_X, &ys);
    let curve = curve_lut(&r.tone_curve); // the recipe's own tone_curve, composed on top
    let user: Vec<f32> = (0..LUT_N)
        .map(|i| {
            let x = i as f32 / (LUT_N - 1) as f32;
            sample_lut(&curve, hermite_eval(&TONE_KNOTS_X, &ys, &m, x))
        })
        .collect();
    if r.base_curve.is_empty() {
        return user;
    }
    // Camera-matched base look: composed UNDER the user controls — sliders act
    // on the camera-like base, the same profile-then-sliders order Lightroom
    // uses. final(x) = user(base(x)); one LUT, still zero extra per-pixel cost.
    let base = base_curve_lut(&r.base_curve);
    (0..LUT_N).map(|i| sample_lut(&user, base[i])).collect()
}

/// LUT for the recipe's camera-matched base curve (`EditRecipe::base_curve`).
/// Knot hygiene mirrors `build_tone_lut`: sort by x, drop non-increasing x,
/// force non-decreasing y (a tone curve cannot invert), pin the (0,0)/(1,1)
/// endpoints — a hand-edited recipe must not unpin black/white through the
/// base stage — then the same monotone Fritsch–Carlson Hermite as the tone
/// model, so the base look inherits its no-inversion/no-overshoot guarantees.
fn base_curve_lut(knots: &[[f32; 2]]) -> Vec<f32> {
    const EPS: f32 = 1e-4;
    let mut pts: Vec<(f32, f32)> = knots
        .iter()
        .map(|p| (p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut xs = vec![0.0f32];
    let mut ys = vec![0.0f32];
    for (x, y) in pts {
        if x <= *xs.last().expect("seeded") + EPS || x >= 1.0 - EPS {
            continue; // endpoint pins own x=0 / x=1
        }
        xs.push(x);
        ys.push(y);
    }
    xs.push(1.0);
    ys.push(1.0);
    for i in 1..ys.len() {
        if ys[i] < ys[i - 1] + EPS {
            ys[i] = ys[i - 1] + EPS;
        }
    }
    for v in &mut ys {
        *v = v.clamp(0.0, 1.0);
    }
    let m = fc_tangents(&xs, &ys);
    (0..LUT_N)
        .map(|i| hermite_eval(&xs, &ys, &m, i as f32 / (LUT_N - 1) as f32))
        .collect()
}

/// Estimate a photo's camera base curve: `[x, y]` knots mapping the NEUTRAL
/// develop's luma toward the camera's embedded rendition by CDF match.
///
/// Knots are QUANTILE-anchored — `x = Q_neutral(p), y = Q_camera(p)` over a
/// shared probability grid — so they only ever sit where the neutral
/// histogram HAS mass. A fixed input grid (the first design) planted pairs
/// of equal-y knots inside empty luma bands (night sky vs street lamps),
/// which the monotone LUT builder flattened into ~30-level posterised
/// plateaus; and on any frame darker than the grid its top knots hit
/// `quantile(1.0)` and latched to 1.0, pinning whole upper bands to pure
/// white in the export (adversarial review, reproduced on synthetic gapped
/// histograms). The probability grid stops at p = 0.98 and the pinned (1,1)
/// endpoint carries the tail smoothly instead.
///
/// Both inputs go through the SAME ≤1024px box-thumbnail + 1024-bin
/// histogram before matching: resampling narrows a luma distribution, so
/// comparing a small thumbnail against a native-size preview used to read as
/// phantom camera contrast (a spurious S on an identity pair, dependent on
/// the GUI preview-size dropdown). Symmetric processing removes that
/// asymmetry; residual sensitivity to a caller's own pre-thumbnailing is
/// sub-bin. Field-measured on A7RIV ARWs: the neutral develop sits
/// 0.6–1.4 EV under the camera JPEG with a consistent S shape that is NOT a
/// single gain (midtones move ~3× more than the toe) — hence a curve.
/// Returns `None` when it cannot JUDGE (degenerate input: too few pixels on
/// either side — an inability the pre-era repair must never mistake for a
/// verdict), `Some(empty)` for the identity verdict (= no base look), and
/// `Some(knots)` otherwise.
pub fn camera_base_knots(
    neutral: &DynamicImage,
    camera: &DynamicImage,
) -> Option<Vec<[f32; 2]>> {
    const BINS: usize = 1024;
    const EST_EDGE: u32 = 1024;
    fn luma_hist(img: &DynamicImage) -> (Vec<u64>, u64) {
        let small;
        let img = if img.width().max(img.height()) > EST_EDGE {
            small = img.thumbnail(EST_EDGE, EST_EDGE);
            &small
        } else {
            img
        };
        let rgb = img.to_rgb8();
        let px = rgb.as_raw();
        let n_px = px.len() / 3;
        // Every pixel of the ≤1024px thumbnail (≤~0.7 MP — cheap). A regular
        // stride here could phase-lock onto periodic image structure, and the
        // two sides would alias DIFFERENTLY (their strides derive from their
        // own sizes), distorting the CDF match.
        let mut h = vec![0u64; BINS];
        for i in 0..n_px {
            let p = &px[i * 3..i * 3 + 3];
            let l = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
            h[((l / 255.0) * (BINS - 1) as f32) as usize] += 1;
        }
        (h, n_px as u64)
    }
    let (hist_n, count_n) = luma_hist(neutral);
    let (hist_c, count_c) = luma_hist(camera);
    if count_n < 10_000 || count_c < 10_000 {
        // Not enough mass to compare CDFs — an INABILITY, distinct from the
        // identity verdict below. Sharing its empty return meant a tiny
        // embedded thumbnail read as "this photo needs no base look", and
        // once the repair adopted empty answers it permanently cleared saved
        // curves over an estimate that never judged anything.
        return None;
    }
    let cumulate = |h: &[u64]| -> Vec<u64> {
        let mut acc = 0u64;
        h.iter().map(|&v| { acc += v; acc }).collect()
    };
    let cum_n = cumulate(&hist_n);
    let cum_c = cumulate(&hist_c);
    // Smallest bin whose cumulative mass reaches p — the same rule on both
    // sides, so an identical pair maps every p to the identical bin and the
    // guard below sees an exact identity.
    let quantile = |cum: &[u64], count: u64, p: f64| -> f32 {
        let target = (p * count as f64).ceil() as u64;
        let bin = cum.partition_point(|&c| c < target).min(BINS - 1);
        bin as f32 / (BINS - 1) as f32
    };
    // Denser toward the toe where the S bends hardest; capped at 0.98 (see
    // the doc comment — the (1,1) pin owns the tail). Quantiles that land in
    // the SAME neutral bin (spiky / posterised neutrals) are merged by
    // averaging their camera side: base_curve_lut's strictly-increasing-x
    // pass would otherwise keep only the FIRST duplicate, biasing that tone
    // toward the spike's lowest camera quantile.
    const PS: [f64; 11] = [0.02, 0.05, 0.10, 0.20, 0.32, 0.45, 0.58, 0.70, 0.82, 0.92, 0.98];
    let mut groups: Vec<(f32, f32, u32)> = Vec::with_capacity(PS.len()); // (x, Σy, n)
    for &p in &PS {
        let x = quantile(&cum_n, count_n, p);
        let y = quantile(&cum_c, count_c, p);
        match groups.last_mut() {
            Some(g) if (x - g.0).abs() < 0.5 / (BINS - 1) as f32 => {
                g.1 += y;
                g.2 += 1;
            }
            _ => groups.push((x, y, 1)),
        }
    }
    let mut knots: Vec<[f32; 2]> = Vec::with_capacity(groups.len() + 2);
    knots.push([0.0, 0.0]);
    for (x, y_sum, n) in groups {
        knots.push([x, y_sum / n as f32]);
    }
    knots.push([1.0, 1.0]);
    // Identity guard: a baked source (or an already camera-matched render)
    // maps onto itself — return empty so the recipe stays clean.
    let max_dev = knots.iter().map(|p| (p[1] - p[0]).abs()).fold(0.0f32, f32::max);
    if max_dev < 0.02 {
        return Some(Vec::new());
    }
    Some(knots)
}

/// Monotone cubic Hermite tangents (Fritsch–Carlson). With `xs` strictly increasing
/// and `ys` non-decreasing, the resulting Hermite spline is monotone everywhere.
fn fc_tangents(xs: &[f32], ys: &[f32]) -> Vec<f32> {
    let n = xs.len();
    let d: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        if d[i - 1] * d[i] <= 0.0 {
            m[i] = 0.0; // local extremum → flat tangent (keeps monotonicity)
        } else {
            let w1 = 2.0 * (xs[i + 1] - xs[i]) + (xs[i] - xs[i - 1]);
            let w2 = (xs[i + 1] - xs[i]) + 2.0 * (xs[i] - xs[i - 1]);
            m[i] = (w1 + w2) / (w1 / d[i - 1] + w2 / d[i]); // weighted harmonic mean
        }
    }
    // Monotonicity limiter: keep each (α,β) inside the circle α²+β² ≤ 9.
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let a = m[i] / d[i];
            let b = m[i + 1] / d[i];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[i] = t * a * d[i];
                m[i + 1] = t * b * d[i];
            }
        }
    }
    m
}

/// Evaluate the monotone cubic Hermite spline at `x` (clamped to the knot range).
fn hermite_eval(xs: &[f32], ys: &[f32], m: &[f32], x: f32) -> f32 {
    let n = xs.len();
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    let mut i = 0;
    while i + 1 < n && x > xs[i + 1] {
        i += 1;
    }
    let h = xs[i + 1] - xs[i];
    let t = (x - xs[i]) / h;
    let (t2, t3) = (t * t, t * t * t);
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    h00 * ys[i] + h10 * h * m[i] + h01 * ys[i + 1] + h11 * h * m[i + 1]
}

/// Curve control points → a 256-entry [0,1]→[0,1] LUT; identity when empty.
/// The ONE curve sampler shared by the master tone curve, the per-channel RGB
/// curves, and the GUI curve editor's on-screen preview — public so what the
/// editor draws is exactly what the engine applies (same sort + linear interp).
pub fn curve_lut(points: &[crate::recipe::CurvePoint]) -> Vec<f32> {
    if points.is_empty() {
        return (0..256).map(|i| i as f32 / 255.0).collect();
    }
    let mut pts: Vec<(f32, f32)> = points
        .iter()
        .map(|p| (p.input as f32 / 255.0, p.output as f32 / 255.0))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // Duplicate inputs (possible via a hand-edited / imported recipe; the GUI
    // editor keeps inputs strictly increasing) resolve FIRST-point-wins —
    // now actually, by DROPPING the later twins. Merely relying on the stable
    // sort gave the first twin's output AT that code and the second twin's
    // output one code later: a one-LUT-bin cliff that shows up as a hard
    // contour line across a smooth gradient.
    // Pin the endpoints Lightroom-style: a curve that places no point of its
    // own AT an end keeps (0,0)/(1,1) authoritative there. Without the pins,
    // interp's flat clamp beyond the first/last point turns a single mid-curve
    // click into a constant image and flattens everything past any inner
    // endpoint into crushed/blown bands. A user (or AI) point at exactly
    // x=0 / x=1 still overrides the pin — lifted blacks stay expressible.
    // Drop the later twins so first-point-wins is literally true.
    pts.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-6);
    if pts[0].0 > 0.0 {
        pts.insert(0, (0.0, 0.0));
    }
    if pts[pts.len() - 1].0 < 1.0 {
        pts.push((1.0, 1.0));
    }
    (0..256).map(|i| interp(&pts, i as f32 / 255.0)).collect()
}

/// Piecewise-linear interpolation over sorted (x,y) control points, clamped at
/// the ends.
fn interp(pts: &[(f32, f32)], x: f32) -> f32 {
    if pts.is_empty() {
        return x;
    }
    if x <= pts[0].0 {
        return pts[0].1;
    }
    if x >= pts[pts.len() - 1].0 {
        return pts[pts.len() - 1].1;
    }
    for w in pts.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if x >= x0 && x <= x1 {
            let t = if (x1 - x0).abs() < 1e-6 { 0.0 } else { (x - x0) / (x1 - x0) };
            return y0 + (y1 - y0) * t;
        }
    }
    x
}

/// Sample a LUT (any length) at a normalised [0,1] position with linear interp.
pub(crate) fn sample_lut(lut: &[f32], x: f32) -> f32 {
    let n = lut.len();
    if n == 0 {
        return x;
    }
    let pos = x.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = pos.floor() as usize;
    if i >= n - 1 {
        return lut[n - 1];
    }
    let t = pos - i as f32;
    lut[i] * (1.0 - t) + lut[i + 1] * t
}

/// Apply the per-channel RGB curves (red/green/blue) in place — the colour
/// companion to the master tone curve. No-op when all three are empty.
fn apply_rgb_curves(data: &mut [[f32; 3]], r: &EditRecipe) {
    let curves = [&r.red_curve, &r.green_curve, &r.blue_curve];
    if curves.iter().all(|c| c.is_empty()) {
        return;
    }
    let luts: [Vec<f32>; 3] =
        [curve_lut(curves[0]), curve_lut(curves[1]), curve_lut(curves[2])];
    let active = [!curves[0].is_empty(), !curves[1].is_empty(), !curves[2].is_empty()];
    data.par_iter_mut().for_each(|px| {
        for ch in 0..3 {
            if active[ch] {
                px[ch] = sample_lut(&luts[ch], px[ch]);
            }
        }
    });
}

/// Saturation + vibrance around the pixel's luma. Vibrance boosts low-saturation
/// pixels more (so already-vivid colours don't blow out).
fn apply_sat_vibrance(r: f32, g: f32, b: f32, sat: f32, vib: f32) -> [f32; 3] {
    let l = 0.299 * r + 0.587 * g + 0.114 * b;
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let pixel_sat = if mx > 1e-4 { (mx - mn) / mx } else { 0.0 };
    let factor = (1.0 + sat + vib * (1.0 - pixel_sat)).max(0.0);
    [
        (l + (r - l) * factor).clamp(0.0, 1.0),
        (l + (g - l) * factor).clamp(0.0, 1.0),
        (l + (b - l) * factor).clamp(0.0, 1.0),
    ]
}

/// Per-colour HSL (the 8 ACR bands). For each pixel: find which colour band(s)
/// its hue falls in (triangular partition of unity over the band centres), then
/// rotate hue / scale saturation / scale luminance by the band-weighted amounts.
/// Achromatic pixels (no hue) are untouched. Runs in sRGB-gamma space — a
/// tasteful approximation; the XMP→Lightroom path renders the exact ACR model.
fn apply_hsl(data: &mut [[f32; 3]], hsl: &crate::recipe::Hsl) {
    if hsl.is_neutral() {
        return;
    }
    data.par_iter_mut().for_each(|px| {
        let (h, s, l) = rgb_to_hsl(px[0], px[1], px[2]);
        // Fade the WHOLE HSL effect out on low-CHROMA pixels. Gate on chroma
        // (max−min), NOT HSL saturation: HSL `s` is ill-conditioned near white and
        // black — a bright, faintly-blue sea-foam pixel has chroma ≈ 0.12 yet HSL
        // s ≈ 1.0, so an HSL-`s` gate hits specular highlights at FULL strength and
        // a Blue-band luminance push crushes white foam to grey. Chroma is a true
        // colourfulness measure: ≈0 for near-grey (the overcast-sky blotch case)
        // AND for near-white foam, ramping to full only on genuinely saturated
        // colour, so both are protected while real colours are still adjusted.
        let chroma = px[0].max(px[1]).max(px[2]) - px[0].min(px[1]).min(px[2]);
        let satw = smoothstep(0.05, 0.22, chroma);
        if satw <= 0.0 {
            return; // (per-pixel closure: this pixel is untouched)
        }
        let (b0, b1, w1) = bracket_bands(h * 360.0, &HSL_CENTERS);
        let w0 = 1.0 - w1;
        let hue_adj = (w0 * hsl.hue[b0] + w1 * hsl.hue[b1]) * satw;
        let sat_adj = (w0 * hsl.saturation[b0] + w1 * hsl.saturation[b1]) * satw;
        let lum_adj = (w0 * hsl.luminance[b0] + w1 * hsl.luminance[b1]) * satw;
        // hue: ±100 → ±30° rotation; sat: ±100 → ±100%; lum gentler (×0.5).
        let new_h = (h + (hue_adj / 100.0) * (30.0 / 360.0)).rem_euclid(1.0);
        let new_s = (s * (1.0 + sat_adj / 100.0)).clamp(0.0, 1.0);
        let new_l = (l * (1.0 + 0.5 * lum_adj / 100.0)).clamp(0.0, 1.0);
        let (r, g, b) = hsl_to_rgb(new_h, new_s, new_l);
        *px = [r, g, b];
    });
}

/// ACR band centres in degrees (red..magenta), matching recipe::HSL_BANDS.
/// Shared with the reverse-fit so its per-band statistics use the SAME partition.
pub(crate) const HSL_CENTERS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];

/// The two band indices bracketing hue `deg` and the blend weight toward the
/// second (partition of unity). Centres are non-uniform and wrap (magenta 300°
/// → red 360°/0°), so the last segment spans 300..360 back to red.
pub(crate) fn bracket_bands(deg: f32, centers: &[f32; 8]) -> (usize, usize, f32) {
    let d = deg.rem_euclid(360.0);
    for i in 0..8 {
        let lo = centers[i];
        let hi = if i + 1 < 8 { centers[i + 1] } else { 360.0 };
        if d >= lo && d < hi {
            let upper = if i + 1 < 8 { i + 1 } else { 0 };
            return (i, upper, (d - lo) / (hi - lo));
        }
    }
    (0, 1, 0.0) // unreachable: the segments tile [0,360)
}

/// sRGB-gamma RGB → HSL, all in [0,1] (hue normalised to turns).
pub(crate) fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l); // achromatic
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h.rem_euclid(1.0), s, l)
}

/// HSL → sRGB-gamma RGB (inverse of [`rgb_to_hsl`]).
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s < 1e-6 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue2rgb = |mut t: f32| -> f32 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 1.0 / 2.0 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (hue2rgb(h + 1.0 / 3.0), hue2rgb(h), hue2rgb(h - 1.0 / 3.0))
}

/// Lightroom-style colour grading: tint + lift the shadow / midtone / highlight
/// tonal regions (and a global wheel) by their hue/sat/lum. Region membership is a
/// smoothstep split on luma; `blending` scales the regional effect, `balance`
/// shifts the shadow/highlight split. Approximation; XMP→Lightroom is exact.
fn apply_color_grade(data: &mut [[f32; 3]], cg: &crate::recipe::ColorGrade) {
    if cg.is_neutral() {
        return;
    }
    // balance shifts the shadow/highlight midpoint: positive leans toward highlights.
    let mid = (0.5 - 0.25 * (cg.balance / 100.0)).clamp(0.05, 0.95);
    // `blending` sets how much the tonal regions OVERLAP — the schema's own
    // words (recipe.rs) and Lightroom's. It used to scale the regional
    // AMPLITUDE instead, with two consequences: a legal Blending of 0 silently
    // erased all three regional wheels (only the global wheel survived, which
    // read as "grading is half broken"), and ACR's DEFAULT of 50 rendered
    // every graded photo at half strength — so our render disagreed with the
    // Lightroom render the XMP hands off, for essentially every graded photo.
    // 100 reproduces the previous weights EXACTLY (ramps spanning mid..1 and
    // 0..mid); lower values tighten the ramps around `mid` instead of fading
    // the effect out.
    let overlap = (cg.blending / 100.0).clamp(0.0, 1.0);
    // A floor keeps the tightest split one smoothstep wide: a true step would
    // band visibly on a smooth gradient.
    const MIN_SPAN: f32 = 0.02;
    let hi_end = (mid + ((1.0 - mid) * overlap).max(MIN_SPAN)).min(1.0);
    let sh_start = (mid - (mid * overlap).max(MIN_SPAN)).max(0.0);
    data.par_iter_mut().for_each(|px| {
        let l = luma601(px);
        let w_hi = smoothstep(mid, hi_end, l);
        let w_sh = 1.0 - smoothstep(sh_start, mid, l);
        let w_mid = (1.0 - w_hi - w_sh).clamp(0.0, 1.0);
        apply_wheel(px, cg.shadow_hue, cg.shadow_sat, cg.shadow_lum, w_sh);
        apply_wheel(px, cg.midtone_hue, cg.midtone_sat, cg.midtone_lum, w_mid);
        apply_wheel(px, cg.highlight_hue, cg.highlight_sat, cg.highlight_lum, w_hi);
        apply_wheel(px, cg.global_hue, cg.global_sat, cg.global_lum, 1.0); // global: all tones
    });
}

/// Apply one colour-grade wheel to a pixel: shift chroma toward the wheel's hue
/// (scaled by sat × weight) and scale brightness by its luminance — both gentle.
fn apply_wheel(px: &mut [f32; 3], hue_deg: f32, sat: f32, lum: f32, weight: f32) {
    if weight <= 1e-4 {
        return;
    }
    if sat.abs() > 1e-4 {
        // Tint toward the pure hue AT THIS PIXEL'S OWN LUMINANCE (not a fixed
        // 0.5-grey anchor) and blend — this keeps luma roughly constant, so deep
        // shadows / bright highlights aren't crushed past [0,1] the way a fixed
        // additive push does. Closer to ACR's luma-aware toning.
        let l = luma601(px);
        let tint = hsl_to_rgb((hue_deg / 360.0).rem_euclid(1.0), 1.0, l);
        let amt = (sat / 100.0) * weight * 0.4;
        px[0] = (px[0] + (tint.0 - px[0]) * amt).clamp(0.0, 1.0);
        px[1] = (px[1] + (tint.1 - px[1]) * amt).clamp(0.0, 1.0);
        px[2] = (px[2] + (tint.2 - px[2]) * amt).clamp(0.0, 1.0);
    }
    if lum.abs() > 1e-4 {
        let k = (1.0 + (lum / 100.0) * weight * 0.5).max(0.0);
        for c in px.iter_mut() {
            *c = (*c * k).clamp(0.0, 1.0);
        }
    }
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn to_u16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// Apply the RAW's stored orientation so portraits/flips display correctly.
/// pub(crate): the decode side orients the embedded previews with the SAME
/// function, so GUI display and render pipeline can never disagree about
/// which way is up.
pub(crate) fn oriented(img: DynamicImage, o: Orientation) -> DynamicImage {
    match o {
        Orientation::Normal | Orientation::Unknown => img,
        Orientation::HorizontalFlip => img.fliph(),
        Orientation::Rotate180 => img.rotate180(),
        Orientation::VerticalFlip => img.flipv(),
        Orientation::Rotate90 => img.rotate90(),
        Orientation::Rotate270 => img.rotate270(),
        // The rotate must allocate (dims swap) but the flip runs IN PLACE:
        // the old rotate().fliph() chain held a THIRD full frame while the
        // source was still alive (~2.2 GB transient on a 61 MP Rgb32F frame).
        Orientation::Transpose => {
            let mut r = img.rotate90();
            drop(img);
            flip_h_in_place(&mut r);
            r
        }
        Orientation::Transverse => {
            let mut r = img.rotate270();
            drop(img);
            flip_h_in_place(&mut r);
            r
        }
    }
}

/// In-place horizontal flip that stays in the image's OWN pixel type.
/// Calling `flip_horizontal_in_place(&mut DynamicImage)` goes through the
/// GenericImage adapter, whose Pixel is Rgba<u8> — that QUANTIZED f32/u16
/// frames to 8 bits (and clamped f32, killing wide-gamut negatives) on every
/// Transpose/Transverse-oriented photo since the A7 in-place rewrite; the
/// eight-state orientation test caught it (U14).
fn flip_h_in_place(img: &mut DynamicImage) {
    use image::imageops::flip_horizontal_in_place as flip;
    match img {
        DynamicImage::ImageLuma8(b) => flip(b),
        DynamicImage::ImageLumaA8(b) => flip(b),
        DynamicImage::ImageRgb8(b) => flip(b),
        DynamicImage::ImageRgba8(b) => flip(b),
        DynamicImage::ImageLuma16(b) => flip(b),
        DynamicImage::ImageLumaA16(b) => flip(b),
        DynamicImage::ImageRgb16(b) => flip(b),
        DynamicImage::ImageRgba16(b) => flip(b),
        DynamicImage::ImageRgb32F(b) => flip(b),
        DynamicImage::ImageRgba32F(b) => flip(b),
        // DynamicImage is #[non_exhaustive]: a future variant falls back to
        // the adapter path (quantizing, but never silently skipped).
        other => flip(other),
    }
}

/// Orient the demosaiced f32 buffer BEFORE develop, so masks / crop /
/// straighten all live in the display frame (the C2 contract's "original").
/// Implemented by round-tripping through [`oriented`] on a lossless Rgb32F
/// image — one function owns the orientation semantics, no hand-derived
/// index math to drift. Identity (no copy) for Normal/Unknown.
fn orient_f32(
    data: Vec<[f32; 3]>,
    w: usize,
    h: usize,
    o: Orientation,
) -> (Vec<[f32; 3]>, usize, usize) {
    if matches!(o, Orientation::Normal | Orientation::Unknown) {
        return (data, w, h);
    }
    // [f32;3] and 3×f32 share layout, so both casts are zero-copy — the old
    // flatten/collect + to_rgb32f() + pixels().collect() chain made THREE full
    // copies of a ~732 MB frame for every portrait RAW. try_cast_vec only
    // falls back to a real copy when the vec's capacity isn't an exact
    // multiple of the element ratio.
    let flat: Vec<f32> = bytemuck::cast_vec(data);
    let img = ImageBuffer::<Rgb<f32>, Vec<f32>>::from_raw(w as u32, h as u32, flat)
        .expect("orient_f32: buffer size matches dims");
    let out = match oriented(DynamicImage::ImageRgb32F(img), o) {
        DynamicImage::ImageRgb32F(b) => b, // rotations/flips keep the variant
        other => other.to_rgb32f(),
    };
    let (ow, oh) = out.dimensions();
    let data: Vec<[f32; 3]> = bytemuck::try_cast_vec(out.into_raw())
        .unwrap_or_else(|(_, v)| v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect());
    (data, ow as usize, oh as usize)
}

/// Downscale the oriented f32 buffer to fit `max_edge` (aspect preserved).
/// The averaging is hand-rolled — a fresh `Vec<[f32; 3]>` filled through an
/// f64 accumulator, NOT a cast-and-delegate — for the bias reason spelled out
/// in the body. It must be the ORIENTED buffer: this binning only commutes
/// with pure axis swaps — reversing orientations shift the bin edges by one
/// source bin, so capping in the sensor frame would change preview pixels
/// (see the probe note in `render_to_image_in`). No-op when
/// the frame already fits. Backs `render_to_image`'s working-resolution
/// cap: developing 61 MP only to thumbnail the result wasted a ~1.5 GB
/// transient chain on every preview-resolution retouch base.
fn downscale_f32(
    data: Vec<[f32; 3]>,
    w: usize,
    h: usize,
    max_edge: u32,
) -> (Vec<[f32; 3]>, usize, usize) {
    // Clamped ONCE and used for the bin ratios too — a raw 0 would produce a
    // zero-dimensional working image.
    let edge = max_edge.max(1);
    if w.max(h) <= edge as usize || w == 0 || h == 0 {
        return (data, w, h);
    }
    // The averaging is hand-rolled because `image::imageops::thumbnail` adds
    // an INTEGER rounding term before dividing: `(sum + n/2) / n`. For u8/u16
    // that is round-to-nearest; for f32 (whose `Enlargeable::Larger` is f64)
    // `n/2` stays a float, so every channel of every capped frame came back
    // exactly +0.5 too bright — GUI previews, retouch/generative bases, the
    // web preview and the camera-base-curve estimate all washed to white.
    //
    // An export's own pixels never went through here (deliverables pass
    // max_edge = None) — but saying exports "were never affected" was WRONG,
    // and this comment said it. `photo_base_knots` estimates the camera base
    // curve from a CAPPED develop and that curve is PERSISTED, so every
    // recipe saved by a build with the bias carries a curve fitted to a
    // washed frame, and the full-resolution render composes it. Fixing the
    // sampler made those saved curves worse, not better — they now sit over a
    // correct develop. See `pipeline::repair_pre_era_base_curve`.
    // The BIN GEOMETRY below is a faithful replica of that function (the same
    // aspect-preserving output dims and the same ceil-based windows), so the
    // orientation behaviour the canary test pins is unchanged — only the bias
    // is gone. Downscale-only means both ratios are ≥ 1, so every window
    // holds at least one source pixel and the fractional-edge cases of the
    // original cannot arise.
    let ratio = (edge as f64 / w as f64).min(edge as f64 / h as f64);
    let nw = ((w as f64 * ratio).round() as usize).max(1).min(w);
    let nh = ((h as f64 * ratio).round() as usize).max(1).min(h);
    let (x_ratio, y_ratio) = (w as f32 / nw as f32, h as f32 / nh as f32);
    let mut out = vec![[0.0f32; 3]; nw * nh];
    out.par_chunks_mut(nw).enumerate().for_each(|(oy, row)| {
        let bottomf = oy as f32 * y_ratio;
        let bottom = (bottomf.ceil() as usize).min(h - 1);
        let top = ((bottomf + y_ratio).ceil() as usize).clamp(bottom + 1, h);
        for (ox, px) in row.iter_mut().enumerate() {
            let leftf = ox as f32 * x_ratio;
            let left = (leftf.ceil() as usize).min(w - 1);
            let right = ((leftf + x_ratio).ceil() as usize).clamp(left + 1, w);
            // f64 accumulation: a 61 MP frame capped to 1280 sums ~2800
            // samples per output pixel, where f32 addition would drift.
            let mut sum = [0.0f64; 3];
            for y in bottom..top {
                for x in left..right {
                    let p = &data[y * w + x];
                    for c in 0..3 {
                        sum[c] += p[c] as f64;
                    }
                }
            }
            let n = ((top - bottom) * (right - left)) as f64;
            for c in 0..3 {
                px[c] = (sum[c] / n) as f32;
            }
        }
    });
    (out, nw, nh)
}

/// The largest axis-aligned rectangle (same aspect freedom as Lightroom's
/// auto-constrain) inscribed in a `w`×`h` rectangle rotated by `deg` degrees —
/// the closed-form solution, so a straightened image never shows black
/// corners. Public: the GUI shares this exact formula to map interaction
/// coordinates between the straightened view and the original frame.
pub fn inscribed_dims(w: f32, h: f32, deg: f32) -> (f32, f32) {
    let a = deg.abs().to_radians();
    if w <= 0.0 || h <= 0.0 {
        return (0.0, 0.0);
    }
    if a < 1e-6 {
        return (w, h);
    }
    let (s, c) = (a.sin(), a.cos());
    let (long, short) = (w.max(h), w.min(h));
    let cos2 = c * c - s * s;
    // At exactly 45° a square lands in the general branch with 0/0 → NaN →
    // a 1×1 output; cos2 ≈ 0 always means the half-diagonal fit applies.
    if short <= 2.0 * s * c * long || cos2.abs() < 1e-6 {
        // Thin case: the short side limits both dimensions (half-diagonal fit).
        let x = 0.5 * short;
        if w >= h { (x / s, x / c) } else { (x / c, x / s) }
    } else {
        ((w * c - h * s) / cos2, (h * c - w * s) / cos2)
    }
}

/// Straighten: rotate the image `deg` degrees CLOCKWISE about its centre
/// (bilinear resample) and auto-crop to the largest inscribed axis-aligned
/// rectangle ([`inscribed_dims`]) so no black corners survive. Identity when
/// `deg` rounds to zero. Works in 16-bit so the export path loses nothing;
/// the preview's 8-bit input survives the round-trip exactly.
pub fn rotate_straighten(img: &DynamicImage, deg: f32) -> DynamicImage {
    if deg.abs() < 1e-3 {
        return img.clone();
    }
    // A zero-size frame has no geometry to rotate, and the inscribed-rect math
    // below would hand the bilinear sampler an upper bound of -1 and panic.
    // The lens resamplers already guard this; this one did not.
    if img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
    let src = rgb16_source(img);
    let src = &*src; // the samplers take a plain &ImageBuffer
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (cw, ch) = inscribed_dims(w, h, deg);
    let (ow, oh) = ((cw.floor() as u32).max(1), (ch.floor() as u32).max(1));
    let rad = deg.to_radians();
    // Content rotates clockwise ⇒ inverse-map each dest pixel by the
    // counter-clockwise matrix (y-down screen coords): [c, s; -s, c].
    let (s, c) = (rad.sin(), rad.cos());
    let (cx_src, cy_src) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let (cx_dst, cy_dst) = ((ow as f32 - 1.0) * 0.5, (oh as f32 - 1.0) * 0.5);
    let mut out: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(ow, oh);
    let obuf: &mut [u16] = &mut out;
    // Output rows are independent → parallel; per-pixel math is unchanged.
    obuf.par_chunks_mut(ow as usize * 3).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy_dst;
        for x in 0..ow as usize {
            let dx = x as f32 - cx_dst;
            let sx = c * dx + s * dy + cx_src;
            let sy = -s * dx + c * dy + cy_src;
            // Bilinear sample, clamped to the frame (the inscribed crop keeps
            // samples in-bounds up to float rounding at the very edge).
            orow[x * 3..x * 3 + 3].copy_from_slice(&sample_bilinear_rgb16(src, sx, sy).0);
        }
    });
    DynamicImage::ImageRgb16(out)
}

/// Clamped bilinear lookup in a 16-bit RGB buffer — the shared resampling core
/// of the geometric ops ([`rotate_straighten`], [`apply_lens_distortion`]).
/// Single-channel bilinear fetch — SAME per-channel math as
/// [`sample_bilinear_rgb16`] (bit-identical result), for the CA path where
/// each channel samples at its own radius and the other two would be wasted.
fn sample_bilinear_ch(src: &ImageBuffer<Rgb<u16>, Vec<u16>>, sx: f32, sy: f32, ch: usize) -> u16 {
    let (w, h) = (src.width() as f32, src.height() as f32);
    let x0 = sx.floor().clamp(0.0, w - 1.0);
    let y0 = sy.floor().clamp(0.0, h - 1.0);
    let x1 = (x0 + 1.0).min(w - 1.0);
    let y1 = (y0 + 1.0).min(h - 1.0);
    let (fx, fy) = ((sx - x0).clamp(0.0, 1.0), (sy - y0).clamp(0.0, 1.0));
    let p00 = src.get_pixel(x0 as u32, y0 as u32)[ch] as f32;
    let p10 = src.get_pixel(x1 as u32, y0 as u32)[ch] as f32;
    let p01 = src.get_pixel(x0 as u32, y1 as u32)[ch] as f32;
    let p11 = src.get_pixel(x1 as u32, y1 as u32)[ch] as f32;
    let top = p00 * (1.0 - fx) + p10 * fx;
    let bot = p01 * (1.0 - fx) + p11 * fx;
    (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 65535.0) as u16
}

fn sample_bilinear_rgb16(src: &ImageBuffer<Rgb<u16>, Vec<u16>>, sx: f32, sy: f32) -> Rgb<u16> {
    let (w, h) = (src.width() as f32, src.height() as f32);
    let x0 = sx.floor().clamp(0.0, w - 1.0);
    let y0 = sy.floor().clamp(0.0, h - 1.0);
    let x1 = (x0 + 1.0).min(w - 1.0);
    let y1 = (y0 + 1.0).min(h - 1.0);
    let (fx, fy) = ((sx - x0).clamp(0.0, 1.0), (sy - y0).clamp(0.0, 1.0));
    let p00 = src.get_pixel(x0 as u32, y0 as u32);
    let p10 = src.get_pixel(x1 as u32, y0 as u32);
    let p01 = src.get_pixel(x0 as u32, y1 as u32);
    let p11 = src.get_pixel(x1 as u32, y1 as u32);
    let mut v = [0u16; 3];
    for (ch_i, out_v) in v.iter_mut().enumerate() {
        let top = p00[ch_i] as f32 * (1.0 - fx) + p10[ch_i] as f32 * fx;
        let bot = p01[ch_i] as f32 * (1.0 - fx) + p11[ch_i] as f32 * fx;
        *out_v = (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 65535.0) as u16;
    }
    Rgb(v)
}

// --- Manual lens distortion (gap batch C, 第二片) ----------------------------
//
// Coordinate-space contract (the C2 design). The geometric pipeline is
//
//   original ──apply_lens_distortion──▶ corrected ──rotate_straighten──▶ view
//
// Masks / brush strokes / droppers / clone points live in the ORIGINAL frame
// (`apply_develop` runs before this remap); `recipe.crop` lives in the VIEW
// frame. The GUI maps every interaction through
// view → (un-rotate) → corrected → [`distort_norm`] → original, and displays
// stored original-frame geometry via [`undistort_norm`] → (rotate) → view, so
// a mask painted on screen lands on the same CONTENT in the export regardless
// of the slider values.
//
// Model: a pure radial resample about the frame centre, radius normalised by
// the half-diagonal (r = 1 exactly at the corners — invariant to the EXIF
// orientation step and identical between the 1280 px preview and the 61 MP
// export). Every corrected-frame point at radius r samples the original at
//
//   r_src = s · r · (1 + k · (s·r)²),      k = −amount/100 · DISTORT_STRENGTH
//
// Sign: ACR's Distortion slider is "+ straightens barrel", which must push
// edge content OUTWARD, i.e. pull samples INWARD ⇒ k < 0 for amount > 0
// (derived twice independently: pinhole magnification recovery, and the
// bow-direction of a mapped straight line — both agree). |k| ≤ 0.25 keeps
// d(r_src)/dr = s(1 + 3k(sr)²) > 0 on the frame, so the map stays monotonic
// and invertible. `s` is a fill scale: for k > 0 (pincushion fix) the Newton
// root of k·s³ + s − 1 = 0 zooms in just enough that corner samples stay
// inside the source (no black corners — the same auto-fill policy as
// `rotate_straighten`); for k ≤ 0 the map fills the frame as-is (s = 1) and
// the outermost source corners crop away instead, like LR's constrained crop.
// The amount → k gain is our calibration, not Adobe's published one (they
// don't publish it); ±100 ⇒ up to 25 % radial remap at the corners.

/// Slider-to-curvature gain: |k| at amount = ±100. Must stay < 1/3 or the
/// radial map loses monotonicity inside the frame (see module notes above).
const DISTORT_STRENGTH: f32 = 0.25;

/// amount → (k, fill scale s). See the coordinate-space contract above.
fn distort_params(amount: f32) -> (f32, f32) {
    let k = -amount.clamp(-100.0, 100.0) / 100.0 * DISTORT_STRENGTH;
    let s = if k > 0.0 {
        // Newton on f(s) = k·s³ + s − 1: strictly increasing ⇒ unique root,
        // convex ⇒ monotone convergence from s = 1.
        let mut s = 1.0f32;
        for _ in 0..8 {
            s -= (k * s * s * s + s - 1.0) / (3.0 * k * s * s + 1.0);
        }
        s
    } else {
        1.0
    };
    (k, s)
}

/// Corrected-frame normalised point → ORIGINAL-frame normalised point: the
/// forward sampling map of the manual distortion correction. Identity when
/// the amount rounds to zero. Public — the GUI composes it into its
/// view→original interaction mapping.
pub fn distort_norm(nx: f32, ny: f32, dims: (f32, f32), amount: f32) -> (f32, f32) {
    if amount.abs() < 1e-3 {
        return (nx, ny);
    }
    let (w, h) = dims;
    let (k, s) = distort_params(amount);
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rn = (dx * dx + dy * dy).sqrt() / rr;
    let f = s * (1.0 + k * (s * rn) * (s * rn));
    ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5)
}

/// ORIGINAL-frame normalised point → corrected-frame normalised point (Newton
/// inverse of [`distort_norm`]). Original content the correction crops away
/// (a barrel fix pulls the outermost corners out of frame) has no preimage;
/// those points clamp to the map's monotonic limit and land OUTSIDE the unit
/// square, where the GUI's overlay painter clips them — honestly off-screen.
pub fn undistort_norm(nx: f32, ny: f32, dims: (f32, f32), amount: f32) -> (f32, f32) {
    if amount.abs() < 1e-3 {
        return (nx, ny);
    }
    let (w, h) = dims;
    let (k, s) = distort_params(amount);
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rho = (dx * dx + dy * dy).sqrt() / rr;
    if rho < 1e-6 {
        return (nx, ny); // centre is a fixed point
    }
    // Solve u(1 + k·u²) = ρ for u = s·r_corrected. g is concave-increasing up
    // to u_max for k < 0 (monotone Newton from the left, never overshoots) and
    // convex-increasing for k > 0 (monotone from the right); ρ beyond the k<0
    // reachable maximum clamps to u_max (the cropped-away case above).
    let u_max = if k < 0.0 { (1.0 / (3.0 * -k)).sqrt() } else { f32::INFINITY };
    let mut u = rho.min(u_max);
    for _ in 0..12 {
        let g = k * u * u * u + u - rho;
        let dg = 3.0 * k * u * u + 1.0;
        if dg.abs() < 1e-6 {
            break;
        }
        u = (u - g / dg).clamp(0.0, u_max);
    }
    let f = (u / s) / rho; // radial scale: r_corrected / r_original
    ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5)
}

/// Resample the frame through the manual distortion correction (bilinear,
/// 16-bit — the same precision policy as [`rotate_straighten`], so the export
/// path loses nothing and the 8-bit preview survives exactly). Output has the
/// SAME dimensions: the fill scale inside the map guarantees every output
/// pixel has an in-frame source sample. Identity when the amount rounds to 0.
pub fn apply_lens_distortion(img: &DynamicImage, amount: f32) -> DynamicImage {
    if amount.abs() < 1e-3 {
        return img.clone();
    }
    // Degenerate input: par_chunks_mut(0) panics on a zero-size chunk — the
    // same guard apply_lens_geometry carries; an empty frame maps to itself.
    if img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
    let src = rgb16_source(img);
    let src = &*src; // the samplers take a plain &ImageBuffer
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (k, s) = distort_params(amount);
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (cx, cy) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let ow = src.width() as usize;
    let mut out: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(src.width(), src.height());
    let obuf: &mut [u16] = &mut out;
    // Output rows are independent → parallel; per-pixel math is unchanged.
    obuf.par_chunks_mut(ow * 3).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy;
        for x in 0..ow {
            let dx = x as f32 - cx;
            let rn = (dx * dx + dy * dy).sqrt() / rr;
            let f = s * (1.0 + k * (s * rn) * (s * rn));
            orow[x * 3..x * 3 + 3]
                .copy_from_slice(&sample_bilinear_rgb16(src, cx + dx * f, cy + dy * f).0);
        }
    });
    DynamicImage::ImageRgb16(out)
}

// --- In-camera lens profile geometry (lensmeta knots) -----------------------
//
// Same coordinate contract as the manual correction above: pure radial maps
// about the frame centre, radius normalised by the half-diagonal. The profile
// spline (knot i at (i+0.5)/(n−1), linear interpolation, clamped ends —
// RawTherapee's placement) runs FIRST, the manual amount composes on top, and
// CA multiplies the resulting map per channel (red/blue sample at a slightly
// different radius than green). Composition happens in ONE resample pass so
// the frame is only softened by a single bilinear step.

/// Linear interpolation over profile knots at (i+0.5)/(n−1), clamped outside.
fn profile_knot_interp(knots: &[f32], r: f32) -> f32 {
    let n = knots.len();
    if n == 0 {
        return 1.0;
    }
    if n == 1 {
        return knots[0];
    }
    let t = r * (n - 1) as f32 - 0.5;
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

/// The profile's fill scale s_p = max g over the frame EDGE (Stannum's `s`):
/// dividing the map by it is the minimal zoom that keeps every edge source
/// sample inside the frame — no black borders, minimal crop. Edge radii sweep
/// [min(w,h)/diagonal, 1] continuously, so a max over that interval suffices.
fn profile_fill_scale(knots: &[f32], dims: (f32, f32)) -> f32 {
    if knots.is_empty() {
        return 1.0;
    }
    let (w, h) = dims;
    let rmin = (w.min(h) / (w * w + h * h).sqrt().max(1e-6)).clamp(0.0, 1.0);
    // The spline is piecewise LINEAR (knot i at r = (i+0.5)/(n−1)), so its
    // maximum over [rmin, 1] sits at an interval endpoint or an interior
    // knot — evaluate those EXACTLY. The old 257-point uniform sweep could
    // undershoot a peak that fell between samples, and a factor above the
    // true edge maximum sends edge samples outside the source (clamped and
    // smeared by the RGB sampler).
    let mut s = profile_knot_interp(knots, rmin).max(profile_knot_interp(knots, 1.0));
    let n = knots.len();
    if n > 1 {
        let denom = (n - 1) as f32;
        for (j, k) in knots.iter().enumerate() {
            let rj = (j as f32 + 0.5) / denom;
            if rj >= rmin && rj <= 1.0 {
                s = s.max(*k);
            }
        }
    }
    s.max(1e-3)
}

/// Composed forward radial factor at normalised radius `rn` (green/base
/// channel): profile spline (over its fill scale) then the manual amount.
fn lens_geom_factor(rn: f32, dist_knots: &[f32], s_p: f32, k: f32, s: f32) -> f32 {
    let f1 = if dist_knots.is_empty() { 1.0 } else { profile_knot_interp(dist_knots, rn) / s_p };
    let r1 = rn * f1;
    let f2 = s * (1.0 + k * (s * r1) * (s * r1));
    f1 * f2
}

/// Composite fill scale over ALL channels (L04-2): the minimal extra zoom
/// that keeps every channel's edge source sample inside the frame. The
/// GREEN map is bounded on the edge band by construction
/// ([`profile_fill_scale`] divides the spline; the manual term is exactly 1
/// at r=1), but CA MULTIPLIES red/blue past it — a ca knot above 1 sent
/// edge samples outside the source, where [`sample_bilinear_ch`] clamps and
/// smears them into a radial plateau along the border (worst in the CA-only
/// case, where s_p was hard-wired to 1 and no fill existed at all; present
/// with distortion ON too, since the fill drives green to exactly 1 at the
/// worst edge radius and CA multiplies past it).
///
/// Evaluated on the SAME [`LUT_N`] node grid the render interpolates over —
/// the rendered per-channel factor is piecewise linear with node values
/// `base[i]·ca(rn_i)`, so its band maximum sits AT a node and this bound is
/// exact for the resampler. Returns ≥ 1.0, and exactly 1.0 whenever CA is
/// off or no channel overshoots — those paths divide by 1.0 and stay
/// bit-identical. All four map consumers (RGB render, RGBA overlay,
/// forward/inverse norm) divide by the SAME value, so masks, the colour
/// dropper and clone points cannot drift against the pixels (C2).
fn geometry_fill_scale(
    profile: &crate::recipe::LensProfile,
    amount: f32,
    dims: (f32, f32),
) -> f32 {
    let ca_on = profile.ca_on && !profile.ca_r.is_empty() && !profile.ca_b.is_empty();
    if !ca_on {
        return 1.0;
    }
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    let (w, h) = dims;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, dims) } else { 1.0 };
    let rmin = (w.min(h) / (w * w + h * h).sqrt().max(1e-6)).clamp(0.0, 1.0);
    // Start at the last node ≤ rmin: linear interpolation between nodes
    // means the band maximum is covered by the nodes bracketing it.
    let start = ((rmin * (LUT_N - 1) as f32).floor() as usize).min(LUT_N - 1);
    let mut m = 1.0f32;
    for i in start..LUT_N {
        let rn = i as f32 / (LUT_N - 1) as f32;
        let g = lens_geom_factor(rn, dist_knots, s_p, k, s);
        let ca = profile_knot_interp(&profile.ca_r, rn)
            .max(profile_knot_interp(&profile.ca_b, rn));
        m = m.max(g * ca);
    }
    m
}

/// The base image `camera_base_knots` should be fed for a photo whose canvas
/// starts from a stamped lens profile: the neutral develop with the profile
/// VIGNETTE applied (the camera JPEG the estimator matches against already
/// contains that correction — estimating on the uncorrected neutral bakes
/// the corner lift into the global curve a second time). Pre-thumbnailed to
/// the estimator's own working size, so the extra develop pass is a LUT walk
/// over ≤1 MP, not the full frame. Geometry is skipped on purpose: it moves
/// pixels, not their luma histogram.
pub fn estimation_base(
    neutral: &DynamicImage,
    lens: &crate::recipe::LensProfile,
) -> DynamicImage {
    let small = if neutral.width().max(neutral.height()) > 1024 {
        neutral.thumbnail(1024, 1024)
    } else {
        neutral.clone()
    };
    if !lens.vignette_active() {
        return small;
    }
    let vig_only = EditRecipe {
        lens_profile: crate::recipe::LensProfile {
            vignette: lens.vignette.clone(),
            vignette_on: true,
            ..Default::default()
        },
        ..Default::default()
    };
    develop_preview(&small, &vig_only)
}

/// Bilinear sample of an RGBA8 buffer; out-of-frame reads are TRANSPARENT —
/// an overlay raster must vanish where the remap leaves the source, not
/// smear its edge pixels (the RGB16 paths clamp instead, correct for photos).
fn sample_bilinear_rgba8(src: &image::RgbaImage, x: f32, y: f32) -> image::Rgba<u8> {
    let (w, h) = (src.width() as i32, src.height() as i32);
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let px = |xi: i32, yi: i32| -> [f32; 4] {
        if xi < 0 || yi < 0 || xi >= w || yi >= h {
            [0.0; 4]
        } else {
            let p = src.get_pixel(xi as u32, yi as u32).0;
            // PREMULTIPLIED components: interpolating straight RGBA drags the
            // colour toward transparent neighbours' arbitrary (zero) RGB and
            // then attenuates AGAIN at composite time — dark fringes on every
            // overlay edge under geometry.
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
        }
    };
    let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
    let mut acc = [0f32; 4];
    for i in 0..4 {
        let top = a[i] * (1.0 - fx) + b[i] * fx;
        let bot = c[i] * (1.0 - fx) + d[i] * fx;
        acc[i] = top * (1.0 - fy) + bot * fy;
    }
    let alpha = acc[3];
    let mut o = [0u8; 4];
    if alpha > 0.0 {
        for i in 0..3 {
            // Un-premultiply by the interpolated alpha (255·a normalisation
            // cancels) so straight-alpha consumers see the true colour.
            o[i] = (acc[i] * 255.0 / alpha).round().clamp(0.0, 255.0) as u8;
        }
        o[3] = alpha.round().clamp(0.0, 255.0) as u8;
    }
    image::Rgba(o)
}

/// Alpha-preserving twin of [`apply_lens_geometry`] for UI overlay rasters
/// (the paint canvas): the RGB16 photo path flattens transparency to opaque,
/// which turned the whole canvas into a red wash the moment any geometry was
/// active. Green map only — an overlay needs no chromatic refinement, but it
/// MUST carry the composite CA fill scale (L04-2): the render's green map is
/// zoomed by 1/fill, and an overlay skipping that drifts off the pixels.
pub fn apply_lens_geometry_rgba(
    src: &image::RgbaImage,
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> image::RgbaImage {
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    // Degenerate frame: par_chunks_mut(0) below would panic — same guard the
    // RGB16 twin (apply_lens_geometry) carries.
    if src.width() == 0 || src.height() == 0 {
        return src.clone();
    }
    let (w, h) = (src.width() as f32, src.height() as f32);
    let fill = geometry_fill_scale(profile, amount, (w, h));
    // fill > 1 means the RGB render moved every pixel even with distortion
    // off — the overlay must move with it, so the early-out gains the
    // fill==1 condition.
    if !dist_on && amount.abs() < 1e-3 && fill == 1.0 {
        return src.clone();
    }
    let inv_fill = 1.0 / fill;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, (w, h)) } else { 1.0 };
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (cx, cy) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let ow = src.width() as usize;
    let mut out = image::RgbaImage::new(src.width(), src.height());
    let obuf: &mut [u8] = &mut out;
    obuf.par_chunks_mut(ow * 4).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy;
        for x in 0..ow {
            let dx = x as f32 - cx;
            let rn = ((dx * dx + dy * dy).sqrt() / rr).clamp(0.0, 1.0);
            let f = lens_geom_factor(rn, dist_knots, s_p, k, s) * inv_fill;
            orow[x * 4..x * 4 + 4]
                .copy_from_slice(&sample_bilinear_rgba8(src, cx + dx * f, cy + dy * f).0);
        }
    });
    out
}

/// Alpha-preserving twin of [`rotate_straighten`] for UI overlay rasters —
/// same inverse rotation matrix and inscribed-crop output size.
pub fn rotate_straighten_rgba(src: &image::RgbaImage, deg: f32) -> image::RgbaImage {
    if deg.abs() < 1e-3 {
        return src.clone();
    }
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (cw, ch) = inscribed_dims(w, h, deg);
    let (ow, oh) = ((cw.floor() as u32).max(1), (ch.floor() as u32).max(1));
    let rad = deg.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    let (cx_src, cy_src) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let (cx_dst, cy_dst) = ((ow as f32 - 1.0) * 0.5, (oh as f32 - 1.0) * 0.5);
    let ow_px = ow as usize;
    let mut out = image::RgbaImage::new(ow, oh);
    let obuf: &mut [u8] = &mut out;
    obuf.par_chunks_mut(ow_px * 4).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy_dst;
        for x in 0..ow_px {
            let dx = x as f32 - cx_dst;
            let sx = cx_src + c * dx + s * dy;
            let sy = cy_src - s * dx + c * dy;
            orow[x * 4..x * 4 + 4].copy_from_slice(&sample_bilinear_rgba8(src, sx, sy).0);
        }
    });
    out
}

/// Corrected-frame normalised point → ORIGINAL-frame normalised point through
/// the COMPOSED geometry (profile distortion + manual amount — the green map).
/// CA's chromatic split stays render-only, but its composite FILL SCALE moves
/// the shared map by a scalar (L04-2) — skipping it here drifted masks, the
/// dropper and clone points off the rendered pixels. Falls back to
/// [`distort_norm`]'s exact math when the whole map is the manual one.
pub fn lens_geom_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> (f32, f32) {
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    let fill = geometry_fill_scale(profile, amount, dims);
    if !dist_on && fill == 1.0 {
        return distort_norm(nx, ny, dims, amount);
    }
    let (w, h) = dims;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, dims) } else { 1.0 };
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rn = (dx * dx + dy * dy).sqrt() / rr;
    let f = lens_geom_factor(rn, dist_knots, s_p, k, s) / fill;
    ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5)
}

/// ORIGINAL-frame normalised point → corrected-frame point: numeric inverse of
/// [`lens_geom_norm`] by bisection on the forward radial map (monotone for
/// every real profile — factors live in `clamp()`'s 0.7..1.3 band and the
/// spline slopes are gentle; a crafted zigzag would merely land on ONE valid
/// preimage). Falls back to [`undistort_norm`] when the profile is inactive.
pub fn lens_ungeom_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> (f32, f32) {
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    // Same composite fill as the forward map (L04-2) — inverting a map the
    // render did not draw would un-roundtrip every C2 consumer.
    let fill = geometry_fill_scale(profile, amount, dims);
    if !dist_on && fill == 1.0 {
        return undistort_norm(nx, ny, dims, amount);
    }
    let (w, h) = dims;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, dims) } else { 1.0 };
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rho = (dx * dx + dy * dy).sqrt() / rr;
    if rho < 1e-6 {
        return (nx, ny);
    }
    // Bisection over output radius on the RISING PREFIX of the forward map:
    // fwd(rn) = rn · factor(rn) increases and then — under a strong manual
    // barrel fix — folds back (the same shape undistort_norm's u_max clamp
    // handles). Scan for the peak first; originals beyond the reachable
    // maximum clamp there and land honestly off-screen, like undistort_norm.
    let fwd = |rn: f32| rn * lens_geom_factor(rn, dist_knots, s_p, k, s) / fill;
    let mut hi = 2.0f32;
    let mut peak = 0.0f32;
    for i in 1..=256 {
        let rn = 2.0 * i as f32 / 256.0;
        let v = fwd(rn);
        if v < peak {
            hi = 2.0 * (i - 1) as f32 / 256.0;
            break;
        }
        peak = v;
    }
    if fwd(hi) <= rho {
        let f = hi / rho;
        return ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5);
    }
    let mut lo = 0.0f32;
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if fwd(mid) < rho {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let rn = 0.5 * (lo + hi);
    let f = rn / rho; // r_corrected / r_original
    ((dx * f) / w.max(1e-6) + 0.5, (dy * f) / h.max(1e-6) + 0.5)
}

/// View-frame normalised point (the straightened, auto-cropped frame the user
/// SEES, before the user crop) → ORIGINAL-frame normalised point: un-rotate
/// (view → corrected, the counter-clockwise matrix), then the forward sampling
/// map (corrected → original). Clamped once, at the end. This is the ONE
/// shared implementation of the C2 interaction mapping — the GUI wraps it and
/// the web server maps analyze region boxes through it, so they cannot drift.
pub fn view_to_original_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    deg: f32,
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> (f32, f32) {
    let (w, h) = dims;
    // Same identity threshold as BOTH raster rotators (rotate_straighten /
    // rotate_straighten_rgba use |deg| < 1e-3): an exact-zero test here made
    // the maps rotate for a sub-threshold angle the pixels never got.
    let (cx, cy) = if deg.abs() < 1e-3 {
        (nx, ny)
    } else {
        let (cw, ch) = inscribed_dims(w, h, deg);
        let rad = deg.to_radians();
        let (s, c) = (rad.sin(), rad.cos());
        let (dx, dy) = ((nx - 0.5) * cw, (ny - 0.5) * ch);
        // Content was rotated clockwise; undo with the counter-clockwise matrix.
        (((c * dx + s * dy) / w) + 0.5, ((-s * dx + c * dy) / h) + 0.5)
    };
    let (ox, oy) = lens_geom_norm(cx, cy, dims, profile, amount);
    (ox.clamp(0.0, 1.0), oy.clamp(0.0, 1.0))
}

/// ORIGINAL-frame normalised point → view normalised point: the inverse
/// geometry map (original → corrected), then the forward rotation. NOT
/// clamped: an original point can legitimately fall outside the view window
/// (content a barrel fix crops away lands just outside the unit square);
/// callers clip.
pub fn original_to_view_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    deg: f32,
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> (f32, f32) {
    let (nx, ny) = lens_ungeom_norm(nx, ny, dims, profile, amount);
    // Same 1e-3 identity threshold as the raster rotators — see
    // view_to_original_norm.
    if deg.abs() < 1e-3 {
        return (nx, ny);
    }
    let (w, h) = dims;
    let (cw, ch) = inscribed_dims(w, h, deg);
    let rad = deg.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rx = c * dx - s * dy; // clockwise forward
    let ry = s * dx + c * dy;
    (rx / cw.max(1e-3) + 0.5, ry / ch.max(1e-3) + 0.5)
}

/// Resample the frame through the COMPOSED lens geometry: profile distortion
/// (+ per-channel CA) and the manual amount in one bilinear pass. Identity →
/// clone. Same 16-bit precision policy as [`apply_lens_distortion`], which
/// remains as the manual-only special case this generalises.
pub fn apply_lens_geometry(
    img: &DynamicImage,
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> DynamicImage {
    // Degenerate input: rayon's par_chunks_mut(0) panics on a zero-size
    // chunk — an empty frame maps to itself.
    if img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    let ca_on = profile.ca_on && !profile.ca_r.is_empty() && !profile.ca_b.is_empty();
    if !dist_on && !ca_on {
        return apply_lens_distortion(img, amount);
    }
    let src = rgb16_source(img);
    let src = &*src; // the samplers take a plain &ImageBuffer
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, (w, h)) } else { 1.0 };
    // Composite CA fill (L04-2): every channel's LUT divides by ONE scalar
    // (≥ 1; exactly 1 when no channel overshoots, keeping those paths
    // bit-identical), so a ca knot above 1 zooms the whole frame in by up
    // to that knot instead of sending red/blue edge samples outside the
    // source, where the clamping sampler smeared them into a radial band.
    // Per-channel renormalisation is NOT an option — dividing ca_r by its
    // own max would cancel the near-constant correction it encodes.
    let fill = geometry_fill_scale(profile, amount, (w, h));
    // Per-channel radial factor LUTs over rn ∈ [0,1]: one lookup per channel
    // per pixel instead of spline walks. CA multiplies the green map.
    let luts: [Vec<f32>; 3] = {
        let base: Vec<f32> = (0..LUT_N)
            .map(|i| lens_geom_factor(i as f32 / (LUT_N - 1) as f32, dist_knots, s_p, k, s))
            .collect();
        let chan = |knots: &[f32]| -> Vec<f32> {
            if !ca_on || knots.is_empty() {
                return base.iter().map(|f| f / fill).collect();
            }
            (0..LUT_N)
                .map(|i| {
                    let rn = i as f32 / (LUT_N - 1) as f32;
                    base[i] * profile_knot_interp(knots, rn) / fill
                })
                .collect()
        };
        [chan(&profile.ca_r), chan(&[]), chan(&profile.ca_b)]
    };
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (cx, cy) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let ow = src.width() as usize;
    let mut out: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(src.width(), src.height());
    let obuf: &mut [u16] = &mut out;
    obuf.par_chunks_mut(ow * 3).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy;
        for x in 0..ow {
            let dx = x as f32 - cx;
            let rn = ((dx * dx + dy * dy).sqrt() / rr).clamp(0.0, 1.0);
            if ca_on {
                // Red and blue sample at their own CA-refined radius — one
                // channel per fetch (the full-RGB sampler computed all three
                // channels only to keep one, tripling the interpolation work
                // across a 61 MP frame).
                for (c, lut) in luts.iter().enumerate() {
                    let f = sample_lut(lut, rn);
                    orow[x * 3 + c] = sample_bilinear_ch(src, cx + dx * f, cy + dy * f, c);
                }
            } else {
                let f = sample_lut(&luts[1], rn);
                orow[x * 3..x * 3 + 3]
                    .copy_from_slice(&sample_bilinear_rgb16(src, cx + dx * f, cy + dy * f).0);
            }
        }
    });
    DynamicImage::ImageRgb16(out)
}

#[cfg(test)]
mod tests {

    /// [`source_pixels`] is the ONE raw-vs-baked dispatch, so both arms and the
    /// cap contract are pinned here.
    ///
    /// The RAW arm cannot be exercised end-to-end without a real sensor file
    /// (the repo carries no RAW fixture), but the DISPATCH can: a .ARW must
    /// reach the develop engine — proven by the failure it produces being the
    /// raw decoder's, never `load_image`'s "is a camera RAW" refusal. That
    /// refusal appearing here would mean the gate had sent a RAW down the baked
    /// arm, which is exactly the v0.22 mask-refine bug.
    #[test]
    fn the_source_dispatch_sends_each_kind_down_its_own_arm() {
        let dir = std::env::temp_dir().join(format!("autoshop-source-px-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let raw = dir.join("fake.arw");
        std::fs::write(&raw, b"not really a raw").unwrap();
        let e = format!("{:#}", source_pixels(&raw, None).unwrap_err());
        assert!(
            !e.contains("is a camera RAW"),
            "a RAW must be DEVELOPED, not sent to the baked decoder: {e}"
        );

        // Baked arm: full resolution when uncapped...
        let big = dir.join("big.png");
        image::RgbImage::from_fn(400, 200, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, 7]))
            .save(&big)
            .unwrap();
        assert_eq!(source_pixels(&big, None).unwrap().dimensions(), (400, 200));
        // ...bounded by the cap's LONG edge, aspect kept...
        assert_eq!(source_pixels(&big, Some(100)).unwrap().dimensions(), (100, 50));
        // ...and NEVER upsampled: `thumbnail` alone inflates a small source,
        // which would hand a heal/denoise/refine consumer invented pixels (and
        // save them as the delivered master).
        let small = dir.join("small.png");
        image::RgbImage::from_pixel(64, 48, image::Rgb([3, 4, 5])).save(&small).unwrap();
        assert_eq!(source_pixels(&small, Some(2048)).unwrap().dimensions(), (64, 48));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Patrol (the `find_raws_accepts_every_raw_format_the_app_can_decode`
    /// pattern: ONE predicate, app-wide): every line in the tree that names the
    /// baked decoder must say, ON THE LINE, why it is allowed to. A new consumer
    /// of *source* pixels must go through [`source_pixels`] instead — the whole
    /// point of having one dispatch.
    ///
    /// Per CALL SITE, not per file (R22 M2). The old form asserted a sorted FILE
    /// allow-list, so any file already on it could grow a new hand-rolled decode
    /// of source pixels and stay green — including `bin/gui/workers.rs`, the
    /// exact site of the v0.22 "AI mask refine failed" accident this patrol was
    /// written for. Two markers, either on the line or on the line above it:
    ///
    /// * `// baked-by-construction: <why this path can never be a camera RAW>`
    ///   — a consumer's call.
    /// * `// not-a-consumer-call: <why the line is not a consumer at all>` — the
    ///   gate's own declaration / dispatch / unit tests, and this patrol's own
    ///   extractor literal. It exists so those lines stay HONEST instead of
    ///   claiming a baked path they do not have.
    ///
    /// Lexical on purpose: the drift this catches is someone typing
    /// `load_image` in a new worker, which no type can prevent (both arms
    /// return `DynamicImage`). Known and accepted limit, unchanged from the
    /// file-granular form: a `//`-prefixed line is skipped (that is where the
    /// doc references live), and a mention inside a string literal is scanned
    /// like a call — the marker is then the honest answer.
    #[test]
    fn every_baked_decode_line_says_why_it_is_allowed() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).expect("source dir listable") {
                let p = e.expect("dir entry").path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        files.sort();
        assert!(files.len() >= 20, "only {} source files walked — the patrol is broken", files.len());

        const MARKERS: [&str; 2] = ["baked-by-construction:", "not-a-consumer-call:"];
        let mut scanned = 0usize;
        let mut unmarked: Vec<String> = Vec::new();
        for p in &files {
            let text = std::fs::read_to_string(p).expect("source readable");
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                // Comment lines carry the DOC references, which are exactly what
                // this patrol must not count.
                // not-a-consumer-call: the patrol's own extractor literal.
                if line.trim_start().starts_with("//") || !line.contains("load_image") {
                    continue;
                }
                scanned += 1;
                let above = i.checked_sub(1).map(|j| lines[j]).unwrap_or("");
                if MARKERS.iter().any(|m| line.contains(m) || above.contains(m)) {
                    continue;
                }
                unmarked.push(format!(
                    "{}:{}: {}",
                    p.strip_prefix(&src).unwrap_or(p).to_string_lossy().replace('\\', "/"),
                    i + 1,
                    line.trim()
                ));
            }
        }
        assert!(scanned >= 20, "only {scanned} lines scanned — the extractor is broken");
        assert!(
            unmarked.is_empty(),
            "these lines name the baked decoder with no reason on them — route the source through \
             render::source_pixels, or mark the line `// baked-by-construction: <why>` (a path that \
             cannot be a RAW) / `// not-a-consumer-call: <why>`:\n{}",
            unmarked.join("\n")
        );
    }

    /// L01-6: two active vignette stages compose into ONE clamped pass — the
    /// per-pass clamp made mathematically inverse corrections irreversible on
    /// bright pixels.
    #[test]
    fn opposing_vignette_stages_compose_into_one_clamped_pass() {
        let knots = vec![2.0f32; 4];
        let p_lut = profile_vignette_lut(&knots);
        let m_lut = manual_vignette_lut(-50.0, 50.0);
        let r = EditRecipe {
            lens_vignette: -50.0,
            lens_profile: crate::recipe::LensProfile {
                vignette: knots,
                vignette_on: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let lut = vignette_gain_lut(&r).expect("both stages active");
        let last = *lut.last().unwrap();
        assert!(
            (last - p_lut.last().unwrap() * m_lut.last().unwrap()).abs() < 1e-6,
            "the composed LUT is the product of the stage gains"
        );
        assert!((last - 1.0).abs() < 1e-3, "2.0 × 0.5 cancels at the corner: {last}");

        // A bright corner pixel survives the composed pass untouched, where
        // the clamped two-pass order permanently darkened it. 9×9: the
        // radial geometry floors rmax at one PIXEL, so a frame this small
        // is needed for the corner to actually reach rn = 1.0.
        let mut composed = vec![[0.9f32; 3]; 81];
        apply_radial_gain(&mut composed, 9, 9, &lut);
        let mut two_pass = vec![[0.9f32; 3]; 81];
        apply_radial_gain(&mut two_pass, 9, 9, &p_lut);
        apply_radial_gain(&mut two_pass, 9, 9, &m_lut);
        assert!((composed[0][0] - 0.9).abs() < 1e-3, "composed: {}", composed[0][0]);
        assert!(two_pass[0][0] < 0.85, "the clamp loses the highlight: {}", two_pass[0][0]);
    }

    use super::*;
    use crate::recipe::{EditRecipe, LocalAdjustment};

    #[test]
    fn base_curve_composes_under_user_tone_and_is_not_skipped() {
        // Mid-grey through a lifting base curve must brighten even with a
        // fully-neutral user recipe — the neutral short-circuit must treat the
        // base look as tone work — and user exposure must act ON TOP of it.
        let base =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([100, 100, 100])));
        let mut lifted = EditRecipe {
            base_curve: vec![[0.0, 0.0], [0.4, 0.62], [1.0, 1.0]],
            ..Default::default()
        };
        let neutral_px = develop_preview(&base, &EditRecipe::default()).to_rgb8()[(0, 0)][0];
        assert_eq!(neutral_px, 100, "a truly neutral recipe is the identity");
        let lifted_px = develop_preview(&base, &lifted).to_rgb8()[(0, 0)][0];
        assert!(lifted_px > 130, "base curve must lift mid-grey: {lifted_px}");
        lifted.exposure_ev = 1.0;
        let more_px = develop_preview(&base, &lifted).to_rgb8()[(0, 0)][0];
        assert!(more_px > lifted_px, "exposure composes on top of the base look: {more_px}");
        // Endpoints stay pinned through the composition: black in → black out.
        let black = DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, image::Rgb([0, 0, 0])));
        lifted.exposure_ev = 0.0;
        assert_eq!(develop_preview(&black, &lifted).to_rgb8()[(0, 0)][0], 0);
    }

    #[test]
    fn camera_base_knots_recovers_the_map_and_identity_is_empty() {
        // neutral = a luma gradient; camera = the SAME frame through a known
        // pointwise lift (x^0.6). The CDF match must recover that map at the
        // interior knots, and a self-match must collapse to empty (no-op).
        let (w, h) = (512u32, 64u32);
        let mut n = RgbImage::new(w, h);
        let mut c = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let v = (t * 255.0).round() as u8;
                n.put_pixel(x, y, image::Rgb([v, v, v]));
                let l = (t.powf(0.6) * 255.0).round() as u8;
                c.put_pixel(x, y, image::Rgb([l, l, l]));
            }
        }
        let n = DynamicImage::ImageRgb8(n);
        let c = DynamicImage::ImageRgb8(c);
        let knots =
            camera_base_knots(&n, &c).expect("512x64 clears the degenerate-input guard");
        assert!(!knots.is_empty(), "a real lift must be detected");
        assert_eq!(knots.first(), Some(&[0.0, 0.0]), "black endpoint pinned");
        assert_eq!(knots.last(), Some(&[1.0, 1.0]), "white endpoint pinned");
        for p in &knots {
            if p[0] > 0.05 && p[0] < 0.95 {
                assert!(
                    (p[1] - p[0].powf(0.6)).abs() < 0.04,
                    "knot {p:?} vs expected {}",
                    p[0].powf(0.6)
                );
            }
        }
        assert!(
            camera_base_knots(&n, &n).expect("same pair — still judgeable").is_empty(),
            "identity map → Some(empty) (no base look)"
        );
        // Degenerate input is an INABILITY, not a verdict: the pre-era repair
        // keys on exactly this distinction (None retries later; Some(empty)
        // clears a saved curve and stamps the era).
        let tiny =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(50, 50, image::Rgb([128, 128, 128])));
        assert!(
            camera_base_knots(&tiny, &tiny).is_none(),
            "too few pixels is an inability, not an identity verdict"
        );
    }

    #[test]
    fn base_curve_bridges_histogram_gaps_without_plateaus_or_white_pinning() {
        // Night-street-like pair: luma mass in two bands (0..0.30 and
        // 0.62..0.95) with NOTHING between, both sides the same frame through
        // a known lift (x^0.75). The first estimator planted equal-y knots
        // inside the empty band (→ ~30-level posterised plateaus) and latched
        // its top knots to 1.0 on frames darker than its fixed grid (→ whole
        // upper bands pinned to pure white). Quantile-anchored knots must
        // bridge the gap monotonically and keep the tail off white.
        // 512×64 keeps the sample count above the degenerate-input guard.
        let (w, h) = (512u32, 64u32);
        let mut n = RgbImage::new(w, h);
        let mut c = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let nv = if x % 2 == 0 { 0.30 * t } else { 0.62 + 0.33 * t };
                let cv = nv.powf(0.75).min(1.0);
                n.put_pixel(x, y, image::Rgb([(nv * 255.0).round() as u8; 3]));
                c.put_pixel(x, y, image::Rgb([(cv * 255.0).round() as u8; 3]));
            }
        }
        let knots = camera_base_knots(&DynamicImage::ImageRgb8(n), &DynamicImage::ImageRgb8(c))
            .expect("512x64 clears the degenerate-input guard");
        assert!(!knots.is_empty(), "a real lift must be detected");
        // Apply through the real render path over a full ramp and measure.
        let mut ramp = RgbImage::new(256, 1);
        for x in 0..256 {
            ramp.put_pixel(x, 0, image::Rgb([x as u8; 3]));
        }
        let r = EditRecipe { base_curve: knots, ..Default::default() };
        let out = develop_preview(&DynamicImage::ImageRgb8(ramp), &r).to_rgb8();
        let vals: Vec<u8> = (0..256u32).map(|x| out[(x, 0)][0]).collect();
        let (mut longest, mut run) = (1usize, 1usize);
        for i in 1..256 {
            if vals[i] == vals[i - 1] {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 1;
            }
        }
        assert!(longest <= 12, "posterised plateau of {longest} identical output levels");
        assert!(vals[229] < 250, "input 0.9 must not latch to white: {}", vals[229]);
        assert!(vals[250] > vals[235], "the tail keeps rising toward the (1,1) pin");
    }

    #[test]
    fn camera_base_knots_merges_same_bin_quantiles_with_a_mean() {
        // A posterised neutral (one constant tone) against a camera side whose
        // mass splits across two levels: every probability lands on the same
        // neutral bin, and keeping only the first duplicate would map that
        // tone to the LOWER camera level. The merged knot must sit between.
        let (w, h) = (512u32, 64u32);
        let mut n = RgbImage::new(w, h);
        let mut c = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                n.put_pixel(x, y, image::Rgb([77; 3])); // luma 0.302
                let cv = if x % 2 == 0 { 128 } else { 179 }; // 0.502 / 0.702
                c.put_pixel(x, y, image::Rgb([cv; 3]));
            }
        }
        let knots = camera_base_knots(&DynamicImage::ImageRgb8(n), &DynamicImage::ImageRgb8(c))
            .expect("512x64 clears the degenerate-input guard");
        assert_eq!(knots.len(), 3, "one merged mid knot between the pins: {knots:?}");
        let mid = knots[1];
        assert!((mid[0] - 0.302).abs() < 0.01, "x = the neutral spike: {mid:?}");
        assert!(
            mid[1] > 0.52 && mid[1] < 0.68,
            "y = a mean over the camera split, not its floor: {mid:?}"
        );
    }

    /// Real-machine probe, never run in CI: point AUTOSHOP_PROBE_RAW at an
    /// ARW and run with `--ignored` to check the whole base-look chain on a
    /// real photo — estimator knots + the luma median of the base-curved
    /// render vs the camera's own preview (they must land close).
    #[test]
    #[ignore = "real-machine probe: set AUTOSHOP_PROBE_RAW to an ARW path"]
    fn probe_real_raw_base_look() {
        let Ok(raw) = std::env::var("AUTOSHOP_PROBE_RAW") else {
            panic!("set AUTOSHOP_PROBE_RAW to a RAW path");
        };
        let raw = std::path::PathBuf::from(raw);
        let cam_probe = crate::decode::embedded_preview(&raw);
        println!(
            "embedded_preview: {:?}",
            cam_probe.as_ref().map(|o| o.as_ref().map(|i| (i.width(), i.height())))
        );
        let knots = crate::pipeline::photo_base_knots(&raw);
        println!("knots: {knots:?}");
        assert!(!knots.is_empty(), "expected a base look on a camera RAW");
        let neutral =
            render_to_image(&raw, &EditRecipe::default(), None, None).unwrap().thumbnail(1536, 1536);
        let based = develop_preview(
            &neutral,
            &EditRecipe { base_curve: knots, ..Default::default() },
        );
        let cam = crate::decode::embedded_preview(&raw).unwrap().unwrap();
        let median = |img: &DynamicImage| -> f32 {
            let rgb = img.to_rgb8();
            let mut v: Vec<f32> = rgb
                .as_raw()
                .chunks(3)
                .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
                .collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2] / 255.0
        };
        let (mn, mb, mc) = (median(&neutral), median(&based), median(&cam));
        println!("median neutral={mn:.3} based={mb:.3} camera={mc:.3}");
        assert!(
            (mb - mc).abs() < 0.06,
            "base-curved render should sit near the camera preview: based={mb:.3} camera={mc:.3}"
        );
    }

    #[test]
    fn the_crop_rectangle_is_one_rule_for_both_source_paths() {
        use crate::recipe::Crop;
        // `apply_crop` exists BECAUSE the RAW path and the baked path must
        // agree on the rectangle — but nothing pinned that rule, so the shared
        // helper's arithmetic was verified by reading only. Pin it three ways.
        //
        // (a) The exact rectangle. Origin from left/top, SIZE from the width
        // and height — an implementation that computed the size from the
        // right/bottom EDGES instead (a natural slip) yields 80x40 here, not
        // 60x30, and a swapped x/y yields a 30-tall crop starting at x=20.
        let img =
            DynamicImage::ImageRgb8(RgbImage::from_fn(100, 50, |x, y| {
                image::Rgb([x as u8, y as u8, 0])
            }));
        let c = Crop { left: 0.2, top: 0.1, right: 0.8, bottom: 0.7 };
        let out = apply_crop(img.clone(), Some(&c)).to_rgb8();
        assert_eq!(out.dimensions(), (60, 30), "size comes from the crop's extent");
        assert_eq!(out.get_pixel(0, 0).0, [20, 5, 0], "origin = (left, top) of the frame");

        // (b) Degenerate and absent rectangles are no-ops, never a zero-size
        // image (a zero-size frame reaches par_chunks_mut(0) downstream).
        let dims = |i: DynamicImage| (i.width(), i.height());
        assert_eq!(dims(apply_crop(img.clone(), None)), (100, 50));
        let flat = Crop { left: 0.5, top: 0.1, right: 0.5, bottom: 0.9 };
        assert_eq!(dims(apply_crop(img.clone(), Some(&flat))), (100, 50));
        // Out-of-range components clamp instead of overflowing the cast.
        let wild = Crop { left: -1.0, top: -1.0, right: 2.0, bottom: 2.0 };
        assert_eq!(dims(apply_crop(img.clone(), Some(&wild))), (100, 50));

        // (c) End to end through the REAL baked pipeline (`render_to_file`
        // dispatches a non-RAW source to `render_baked_to_image`): the
        // deliverable's dimensions must equal what the helper predicts, or the
        // shared rule is not the rule the export actually applies.
        let dir = std::env::temp_dir().join(format!("autoshop-crop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.png");
        img.save(&src).unwrap();
        let out_p = dir.join("cropped.png");
        let r = EditRecipe { crop: Some(c), ..Default::default() };
        let (w, h) = render_to_file(&src, &r, &out_p, None, None).unwrap();
        assert_eq!((w, h), (60, 30), "the baked export applies the SAME rectangle");
        assert_eq!(image::image_dimensions(&out_p).unwrap(), (60, 30));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_publishes_atomically_and_leaves_no_staging_file() {
        let dir = std::env::temp_dir().join("autoshop-export-atomic");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.png");
        DynamicImage::ImageRgb8(image::RgbImage::new(4, 3)).save(&src).unwrap();
        let out = dir.join("shot.developed.png");
        let r = EditRecipe::default();
        render_to_file(&src, &r, &out, None, None).unwrap();
        assert!(out.exists(), "the deliverable must be published");
        // The staged copy is consumed on EVERY path — a leftover would mean a
        // partial file could survive beside a delivery.
        let residue: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(residue.is_empty(), "staging residue left behind: {residue:?}");
        // A re-export replaces it in place, still with no residue.
        render_to_file(&src, &r, &out, None, None).unwrap();
        assert!(out.exists());

        // A PRE-STAGING failure: an unknown extension is rejected at format
        // resolution before any file is created — the target must survive
        // and no staging litter may appear. (The old comment claimed this
        // failed "after staging"; it never did — the REAL post-staging case
        // follows below, R12.)
        let keeper = dir.join("keeper.unknownext");
        std::fs::write(&keeper, b"a previous deliverable").unwrap();
        let err = render_to_file(&src, &r, &keeper, None, None);
        assert!(err.is_err(), "an unknown extension must fail the export");
        assert_eq!(
            std::fs::read(&keeper).unwrap(),
            b"a previous deliverable",
            "a failed export must not touch the file it was going to replace"
        );
        let residue: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(residue.is_empty(), "a failed export must clean its staging file: {residue:?}");

        // THE ATOMICITY PROPERTY, exercised after staging actually began: a
        // read-only target makes the PUBLISH rename fail on Windows, so the
        // encode has succeeded and the staging file exists at the moment of
        // failure — the previous deliverable must survive byte-for-byte and
        // the staging file must be cleaned (R12; the old failure aborted
        // before any file handle was opened, so atomicity was never tested).
        #[cfg(windows)]
        {
            let ro = dir.join("keeper.png");
            std::fs::write(&ro, b"a previous deliverable").unwrap();
            let mut perm = std::fs::metadata(&ro).unwrap().permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(&ro, perm.clone()).unwrap();
            let err = render_to_file(&src, &r, &ro, None, None);
            assert!(err.is_err(), "publishing over a read-only file must fail");
            assert_eq!(
                std::fs::read(&ro).unwrap(),
                b"a previous deliverable",
                "a failed PUBLISH must leave the previous deliverable untouched"
            );
            let residue: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains(".tmp."))
                .collect();
            assert!(residue.is_empty(), "publish failure must clean staging: {residue:?}");
            // This whole block is #[cfg(windows)], so the lint's "world
            // writable on Unix" concern cannot apply — we are only restoring
            // writability so the temp dir can be removed.
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            std::fs::set_permissions(&ro, perm).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bitmap_mask_sampling_matches_the_producers_convention() {
        // Producers normalise with x / w; the sampler must agree, or a
        // frame-sized mask loses its last row/column and its placement drifts
        // with resolution.
        let mut m = image::GrayImage::new(2, 1);
        m.put_pixel(0, 0, image::Luma([0]));
        m.put_pixel(1, 0, image::Luma([255]));
        // The two pixel positions a 2-wide FRAME produces.
        assert_eq!(sample_gray_norm(&m, 0.0 / 2.0, 0.0), 0.0);
        assert_eq!(sample_gray_norm(&m, 1.0 / 2.0, 0.0), 1.0, "last texel must be reachable");
        // Resolution independence: an 8-wide frame over the same 2-wide mask
        // must still end at full coverage.
        assert_eq!(sample_gray_norm(&m, 7.0 / 8.0, 0.0), 1.0);
    }

    #[test]
    fn straighten_survives_a_zero_size_frame() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(0, 0));
        let out = rotate_straighten(&img, 5.0);
        assert_eq!((out.width(), out.height()), (0, 0));
    }

    #[test]
    fn develop_survives_a_zero_size_frame() {
        // rayon asserts chunk_size != 0 even on an EMPTY slice, so every
        // `par_chunks_mut(w)` in the develop needs a zero-dim guard. Batch 40
        // guarded the two vignette passes and claimed they were the last of
        // the family; `apply_masks` in fact had two more (its tone pass and
        // its local-NR pass), so this recipe carries a MASK as well — the
        // case that still panicked (R12).
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(0, 0));
        let r = EditRecipe {
            lens_vignette: 60.0,
            lens_profile: crate::recipe::LensProfile {
                vignette: vec![1.0, 1.2, 1.4, 1.6],
                vignette_on: true,
                ..Default::default()
            },
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.0, full_x: 1.0, full_y: 1.0 },
                amount: 1.0,
                exposure_ev: 1.0,
                noise_reduction: 50.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = develop_preview(&img, &r);
        assert_eq!((out.width(), out.height()), (0, 0));
    }

    #[test]
    fn grading_blending_controls_overlap_not_amplitude() {
        use crate::recipe::ColorGrade;
        // A legal Blending of 0 used to zero every regional wheel. It must
        // now still grade — only the region split gets tighter.
        let mut cg = ColorGrade { blending: 0.0, shadow_sat: 100.0, shadow_hue: 210.0, ..Default::default() };
        let mut dark = [[0.08f32, 0.08, 0.08]];
        apply_color_grade(&mut dark, &cg);
        assert!(
            (dark[0][2] - dark[0][0]).abs() > 0.01,
            "blending=0 must still apply the shadow wheel, got {:?}",
            dark[0]
        );
        // And 100 must keep the shadow wheel OFF the midtone: at l = 0.5,
        // mid = 0.5, w_sh = 1 − smoothstep(0, 0.5, 0.5) = 0 exactly, and the
        // midtone/global wheels carry no sat/lum here — the pixel must come
        // back UNTOUCHED. (The old form compared two applications with
        // field-for-field identical ColorGrade values — f(x) == f(x) could
        // never fail, so a shadow-into-midtone leak passed, U14.)
        cg.blending = 100.0;
        let mut a = [[0.5f32, 0.5, 0.5]];
        apply_color_grade(&mut a, &cg);
        assert_eq!(
            a[0],
            [0.5f32, 0.5, 0.5],
            "blending=100: the shadow wheel must not reach the midtone"
        );
        // AMPLITUDE INVARIANCE on a fully-owned deep shadow: blending shapes
        // the region SPLIT only, so a pixel every split assigns to the
        // shadow wheel must grade the same at 0 / 50 / 100. An engine that
        // additionally scaled the regional weights by any blending factor
        // passes the two probes above (the deep probe only ran at 0, the
        // midpoint at 100) but fails this sweep (R12). l = 0.02 sits below
        // every sh_start; the b=100 ramp contributes only ~5e-3 of weight
        // there, far under the mutation's 2x amplitude swing.
        let mut sweep = Vec::new();
        for b in [0.0f32, 50.0, 100.0] {
            let mut px = [[0.02f32, 0.02, 0.02]];
            apply_color_grade(
                &mut px,
                &ColorGrade { blending: b, shadow_sat: 100.0, shadow_hue: 210.0, ..Default::default() },
            );
            sweep.push(px[0]);
        }
        // ENDPOINTS, not consecutive pairs. An amplitude multiplier moves
        // this probe monotonically across the sweep, so each STEP is only
        // ~4e-3 — under a 5e-3 tolerance — while end to end it moves ~8e-3.
        // The consecutive form therefore passed on the exact mutation this
        // block names (measured against the real engine, which stays within
        // 3.7e-5 end to end, so the endpoint form keeps 100x+ of headroom).
        for (a, b) in sweep[0].iter().zip(&sweep[sweep.len() - 1]) {
            assert!(
                (a - b).abs() < 5e-3,
                "blending changed the shadow AMPLITUDE: {sweep:?}"
            );
        }
    }

    #[test]
    fn duplicate_curve_points_do_not_cliff() {
        use crate::recipe::CurvePoint;
        // Two outputs at ONE input is not a function; the documented rule is
        // first-point-wins, which must hold at the code AND just after it.
        let lut = curve_lut(&[
            CurvePoint { input: 0, output: 0 },
            CurvePoint { input: 128, output: 200 },
            CurvePoint { input: 128, output: 50 },
            CurvePoint { input: 255, output: 255 },
        ]);
        let step = (lut[129] - lut[128]).abs();
        assert!(step < 0.05, "one-bin cliff at the duplicate: {step} ({} -> {})", lut[128], lut[129]);
        assert!(lut[128] > 0.7, "first point must win at the duplicate code: {}", lut[128]);
    }

    #[test]
    fn white_point_is_invariant_to_highlights_and_bright_stays_bright() {
        // The engine renders faithfully: Highlights shapes the shoulder but must NOT
        // move the white point, so the brightest tone stays pinned at white. (Keeping
        // bright FOAM bright under an over-cooked recipe is the recipe layer's job —
        // EditRecipe::temper — not an engine override.)
        for h in [-100.0, -78.81, -30.0, 30.0, 100.0] {
            let lut = build_tone_lut(&EditRecipe { highlights: h, ..Default::default() });
            assert!(
                (sample_lut(&lut, 1.0) - 1.0).abs() < 1e-3,
                "highlights {h} moved the white point: {}",
                sample_lut(&lut, 1.0)
            );
        }
        // A neutral recipe must leave bright near-white foam bright.
        let mut foam = vec![[0.90_f32, 0.93, 0.96]];
        apply_develop(&mut foam, 1, 1, &EditRecipe::default());
        let lum = 0.299 * foam[0][0] + 0.587 * foam[0][1] + 0.114 * foam[0][2];
        assert!(lum > 0.90, "neutral recipe dimmed bright foam: {lum}");
    }

    #[test]
    fn tempered_recipe_renders_foam_light_and_water_saturated() {
        // End-to-end: the over-cooked AI recipe (the one that greyed the foam),
        // after clamp + temper, rendered through the monotone curve. Foam must be
        // LIGHT (not crushed to the muddy ~0.6 grey it was) and water must stay
        // turquoise — the engine + recipe layers compose, no engine override.
        let mut r = EditRecipe {
            highlights: -78.81,
            shadows: 36.56,
            whites: 10.27,
            blacks: -14.59,
            contrast: 4.68,
            exposure_ev: -0.177,
            vibrance: 11.19,
            saturation: 2.9,
            ..Default::default()
        };
        r.clamp();
        r.temper();
        let lum = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        let mut foam = vec![[0.90_f32, 0.93, 0.96]];
        apply_develop(&mut foam, 1, 1, &r);
        assert!(lum(foam[0]) > 0.80, "foam crushed (should stay light): luma {}", lum(foam[0]));
        let mut water = vec![[0.35_f32, 0.62, 0.66]];
        apply_develop(&mut water, 1, 1, &r);
        let [rr, gg, bb] = water[0];
        assert!(gg > rr + 0.10 && bb > rr + 0.10, "water lost its turquoise: [{rr}, {gg}, {bb}]");
    }

    #[test]
    fn region_tones_pin_endpoints_and_stay_monotonic() {
        // Highlights/shadows/contrast must never move the endpoints (only whites/
        // blacks may), and the curve must stay monotone under any extreme combo.
        let recipes = [
            EditRecipe::default(),
            EditRecipe { highlights: -100.0, shadows: 100.0, contrast: 100.0, ..Default::default() },
            EditRecipe { highlights: 100.0, shadows: -100.0, contrast: -100.0, ..Default::default() },
        ];
        for r in recipes {
            let lut = build_tone_lut(&r);
            for i in 1..lut.len() {
                assert!(lut[i] >= lut[i - 1] - 1e-6, "non-monotonic at {i}");
            }
            assert!(sample_lut(&lut, 0.0) < 1e-3, "black point moved by hi/sh/contrast");
            assert!((sample_lut(&lut, 1.0) - 1.0).abs() < 1e-3, "white point moved by hi/sh/contrast");
        }
    }

    /// A slider must run out of authority, never destroy detail.
    ///
    /// Monotonicity and pinned endpoints — the two things the tone tests
    /// asserted for four rounds — are both TRUE of a perfectly flat band, so
    /// they were blind to the worst thing this curve can do. Measured on the
    /// pre-fix engine through the real export path: `whites: -50` mapped input
    /// 0.9568–0.9731 to one 16-bit code and left the top decade with 75
    /// distinct codes out of 411; `highlights: +60` mapped everything above
    /// 0.8195 to pure white. Both are ordinary edits.
    ///
    /// So this pins the property those tests missed: no input band survives
    /// the curve as a single output value.
    #[test]
    fn no_slider_setting_collapses_a_tonal_band() {
        const N: usize = 4096;
        // The onset of the old collapse for each slider, and past it.
        let recipes = [
            ("neutral", EditRecipe::default()),
            ("whites -50", EditRecipe { whites: -50.0, ..Default::default() }),
            ("whites -100", EditRecipe { whites: -100.0, ..Default::default() }),
            ("highlights +60", EditRecipe { highlights: 60.0, ..Default::default() }),
            ("highlights +100", EditRecipe { highlights: 100.0, ..Default::default() }),
            ("blacks +60", EditRecipe { blacks: 60.0, ..Default::default() }),
            ("blacks +100", EditRecipe { blacks: 100.0, ..Default::default() }),
            ("shadows +76", EditRecipe { shadows: 76.0, ..Default::default() }),
            ("shadows +100", EditRecipe { shadows: 100.0, ..Default::default() }),
            ("contrast +100", EditRecipe { contrast: 100.0, ..Default::default() }),
            (
                "the old extreme combo",
                EditRecipe { highlights: -100.0, shadows: 100.0, contrast: 100.0, ..Default::default() },
            ),
            (
                "everything at once",
                EditRecipe {
                    whites: -100.0,
                    blacks: 100.0,
                    highlights: 100.0,
                    shadows: 100.0,
                    contrast: 100.0,
                    ..Default::default()
                },
            ),
        ];
        // A run this long is a visibly flat patch, not quantisation: the worst
        // measured pre-fix run was 740 of 4096 and the neutral curve's is 1.
        const MAX_RUN: usize = 96;
        for (name, r) in recipes {
            let lut = build_tone_lut(&r);
            let out: Vec<u16> = (0..N)
                .map(|i| {
                    let x = i as f32 / (N - 1) as f32;
                    (sample_lut(&lut, x).clamp(0.0, 1.0) * 65535.0).round() as u16
                })
                .collect();
            let (mut run, mut worst, mut worst_at) = (1usize, 1usize, 0usize);
            for i in 1..out.len() {
                run = if out[i] == out[i - 1] { run + 1 } else { 1 };
                if run > worst {
                    worst = run;
                    worst_at = i;
                }
            }
            assert!(
                worst <= MAX_RUN,
                "{name}: {worst} consecutive inputs (around x={:.4}) all render to {} — \
                 a slider flattened a tonal band instead of saturating",
                worst_at as f32 / (N - 1) as f32,
                out[worst_at]
            );
        }
    }

    /// The same property with EXPOSURE in play — the dimension the test above
    /// never varies, and the one where this design's guarantee actually ends.
    ///
    /// Two corrections to how the band is measured, both from re-deriving the
    /// numbers rather than trusting the earlier write-up:
    ///
    ///   * A run at 0 or 65535 is CLIPPING, which is what a strong slider on
    ///     an already-bright frame is supposed to do; a run at an interior
    ///     code is destroyed detail. Counting both together made
    ///     `contrast: +100` at `+0.5 EV` look like a 161-input collapse when
    ///     every one of those inputs renders to pure white — and the same
    ///     measurement said the pre-fix `highlights: +60` was harmless, which
    ///     it was not. Only interior runs are counted here.
    ///   * Measured that way, the v0.18.0 limiter's win is still real and
    ///     larger than it looked: `whites: -50` goes from a 157-input
    ///     interior plateau (no limiter) to 43.
    ///
    /// The threshold is what the shipped design HOLDS, not what would be
    /// nice — see the measured-grid note ahead of the loop below.
    #[test]
    fn no_slider_collapses_an_interior_band_at_any_exposure() {
        const N: usize = 4096;
        // What the shipped design HOLDS on this WHOLE grid, with margin: the
        // worst cell measures 100 (weighted-knot model + per-slider λ). Not a
        // wish — measured.
        const MAX_RUN: usize = 128;
        type Case = (&'static str, fn(&mut EditRecipe));
        let sliders: [Case; 18] = [
            ("neutral", |_r| {}),
            ("whites -50", |r| r.whites = -50.0),
            ("whites -100", |r| r.whites = -100.0),
            ("highlights +60", |r| r.highlights = 60.0),
            ("highlights +100", |r| r.highlights = 100.0),
            ("blacks +60", |r| r.blacks = 60.0),
            ("blacks +100", |r| r.blacks = 100.0),
            ("shadows +76", |r| r.shadows = 76.0),
            ("shadows +100", |r| r.shadows = 100.0),
            ("contrast +100", |r| r.contrast = 100.0),
            ("old extreme combo", |r| {
                r.contrast = 100.0;
                r.highlights = -100.0;
                r.shadows = 100.0;
            }),
            ("everything at once", |r| {
                r.contrast = 100.0;
                r.highlights = 100.0;
                r.shadows = 100.0;
                r.whites = -100.0;
                r.blacks = 100.0;
            }),
            ("highlights -100", |r| r.highlights = -100.0),
            ("highlights -60", |r| r.highlights = -60.0),
            ("shadows -100", |r| r.shadows = -100.0),
            ("contrast -100", |r| r.contrast = -100.0),
            ("whites +50", |r| r.whites = 50.0),
            ("blacks -60", |r| r.blacks = -60.0),
        ];
        let interior_run = |r: &EditRecipe| -> (usize, usize, u16) {
            let lut = build_tone_lut(r);
            let out: Vec<u16> = (0..N)
                .map(|i| {
                    let x = i as f32 / (N - 1) as f32;
                    (sample_lut(&lut, x).clamp(0.0, 1.0) * 65535.0).round() as u16
                })
                .collect();
            let (mut run, mut worst, mut worst_at) = (1usize, 1usize, 0usize);
            for i in 1..out.len() {
                // Clipping is the user's own request; an interior plateau is
                // detail that no later stage can recover.
                run = if out[i] == out[i - 1] && out[i] > 0 && out[i] < u16::MAX {
                    run + 1
                } else {
                    1
                };
                if run > worst {
                    worst = run;
                    worst_at = i;
                }
            }
            (worst, worst_at, out[worst_at])
        };

        // ONE bound for the WHOLE grid. Until the weighted-knot model
        // (`tone_knot_weights`, M-T1) this test carried a 220-input carve-out
        // for `ev > 1.0`: the basis added slider offsets at knots whose base
        // intervals exposure had already saturated (`base_gap <= 1e-6`, the λ
        // limiter's rightful skip case), a strong negative slider dipped
        // below the ceiling, and the monotone backstop flattened a whole
        // interval at an interior grey — `contrast: -100` at `+1.5 EV`
        // flattened 197 inputs at code 56304. Four knot-LEVEL repairs were
        // measured and rejected (each traded the tail for a worse one; the
        // pre-weights per-slider λ took the worst from 197 to 317) before the
        // tone-MODEL fix landed: knot authority now follows the base curve's
        // own local separation, a slider aimed at a clipped region yields
        // honest clipping, and the measured grid is 6 cells > 96 with a
        // global worst of 100 — the same level the ev = 0 design holds, three
        // of those six sitting one code below pure white (65534, the
        // quantisation edge of clipping, not a band).
        for ev in [-3.0f32, -2.0, -1.5, -1.0, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0, 1.28, 1.5, 2.0, 3.0] {
            for (name, apply) in sliders {
                let mut r = EditRecipe { exposure_ev: ev, ..Default::default() };
                apply(&mut r);
                let (worst, at, code) = interior_run(&r);
                assert!(
                    worst <= MAX_RUN,
                    "{name} at {ev:+} EV: {worst} consecutive inputs (around x={:.4}) all render                      to the interior code {code} — a slider flattened a tonal band instead of                      saturating",
                    at as f32 / (N - 1) as f32
                );
            }
        }
    }

    /// M-T2: the per-slider λ iteration removed (every `lam` left at 1, only
    /// the single-λ backstop applied) — the slider that binds an interval
    /// must saturate ALONE; the sliders that did not close it keep their
    /// authority. This was the model's second known gap: pinning shadows +50
    /// while dragging whites −100 rendered the shadows at 22.5.
    #[test]
    fn a_slider_that_binds_an_interval_no_longer_drags_the_innocent_ones() {
        // whites −100 alone closes the 0.92–1.0 interval and must saturate
        // near −0.45 (the value the interval can absorb); shadows was not
        // involved and keeps exactly what the caller asked.
        let out = limit_tone_sliders(0.0, [0.0, 0.0, 0.5, -1.0, 0.0]);
        assert_eq!(out[2], 0.5, "shadows were scaled for a violation they did not cause");
        assert!(
            (-0.46..=-0.44).contains(&out[3]),
            "whites did not saturate at the interval's own capacity: {}",
            out[3]
        );
        // blacks −100 OPENS the bottom interval (black point down) — no limit
        // applies to it at all, even alongside the binding whites.
        let out = limit_tone_sliders(0.0, [0.0, 0.0, 0.5, -1.0, -1.0]);
        assert_eq!(out[4], -1.0, "blacks bind nothing and must pass through untouched");
        assert_eq!(out[2], 0.5);
        // And a genuinely unconstrained vector is bit-for-bit untouched.
        let s = [0.3, -0.2, 0.4, 0.1, -0.25];
        assert_eq!(limit_tone_sliders(0.0, s), s);
    }

    #[test]
    fn tone_lut_is_monotonic_and_keeps_midtone_separation() {
        // The reported "flat muddy water": strong opposing highlights/shadows made
        // the per-region tone curve non-monotonic and collapsed mid-bright tones
        // into one dark band. The curve must stay monotonic and keep midtones apart.
        let r = EditRecipe {
            highlights: -73.89,
            shadows: 33.28,
            whites: 6.99,
            blacks: -12.94,
            contrast: 4.68,
            ..Default::default()
        };
        let lut = build_tone_lut(&r);
        for i in 1..lut.len() {
            assert!(lut[i] >= lut[i - 1] - 1e-6, "tone LUT inverts at {i}: {} < {}", lut[i], lut[i - 1]);
        }
        // mid-bright water tones (0.50 vs 0.66) must NOT collapse to one value.
        let (a, b) = (sample_lut(&lut, 0.50), sample_lut(&lut, 0.66));
        assert!(b - a > 0.05, "midtone separation crushed flat: {a}..{b}");
        // and a true midtone (0.5) is no longer crushed deep into shadow.
        assert!(a > 0.45, "midtone water still crushed dark: {a}");
    }

    #[test]
    fn aggressive_highlights_keep_saturated_water_from_greying() {
        // Reported bug: strong −highlights + +shadows turned bright turquoise water
        // flat grey, because the tone LUT ran per-channel and the channels converged.
        // Luminance-preserving tone must keep the cyan recognizably cyan (just darker).
        let r = EditRecipe {
            highlights: -73.89,
            shadows: 33.28,
            whites: 6.99,
            blacks: -12.94,
            contrast: 4.68,
            ..Default::default()
        };
        let cyan = [0.35_f32, 0.62, 0.66]; // mid-bright sunlit turquoise
        let mut data = vec![cyan];
        apply_develop(&mut data, 1, 1, &r);
        let [rr, gg, bb] = data[0];
        // green & blue stay clearly above red → still cyan, not neutral grey.
        assert!(gg > rr + 0.08 && bb > rr + 0.08, "water greyed out: [{rr}, {gg}, {bb}]");
        // channel spread preserved (not converged toward equal = grey).
        let spread = rr.max(gg).max(bb) - rr.min(gg).min(bb);
        assert!(spread > 0.12, "channels converged toward grey: spread {spread}");
    }

    #[test]
    fn linear_mask_affects_only_the_full_end() {
        // Linear mask: zero at top (ny=0), full at bottom (ny=1) + strong darken.
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0 },
                amount: 1.0,
                exposure_ev: -4.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let (w, h) = (1usize, 4usize);
        let mut data = vec![[0.6_f32, 0.6, 0.6]; w * h];
        apply_develop(&mut data, w, h, &r);
        assert!((data[0][0] - 0.6).abs() < 0.03, "top should be ~unchanged: {}", data[0][0]);
        assert!(data[3][0] < 0.5, "bottom should darken: {}", data[3][0]);
        // The interior rows carry the actual gradient — the endpoint checks
        // alone let a "positive weight ⇒ full coverage" mutation render the
        // ramp as a hard step (row 0 has weight EXACTLY 0 and stays pinned;
        // row 3 only darkens further) (U14).
        assert!(
            data[1][0] > data[2][0] + 0.05 && data[2][0] > data[3][0] + 0.05,
            "linear ramp collapsed to a step: {:?}",
            [data[1][0], data[2][0], data[3][0]]
        );
    }

    #[test]
    fn local_noise_reduction_smooths_only_inside_the_mask() {
        // 8x1 strip of alternating luma (= noise). A linear mask covering the
        // RIGHT half with full local NR should flatten the right; left untouched.
        let (w, h) = (8usize, 1usize);
        let mut data: Vec<[f32; 3]> =
            (0..w).map(|x| { let v = if x % 2 == 0 { 0.3 } else { 0.7 }; [v, v, v] }).collect();
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 1.0, full_y: 0.5 },
                amount: 1.0,
                noise_reduction: 100.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let var = |d: &[[f32; 3]], rng: std::ops::Range<usize>| {
            let v: Vec<f32> = rng.map(|i| d[i][0]).collect();
            let m = v.iter().sum::<f32>() / v.len() as f32;
            v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32
        };
        // Control render (mask-less, same global stages): "untouched" must
        // mean BIT-FOR-BIT equal — the convention the range-mask tests use.
        // The old probe compared the left half's red-channel VARIANCE, which
        // is blind to a constant offset (translation-invariant) and to
        // green/blue-only leaks (it never read those channels) (U14).
        let mut control = data.clone();
        apply_develop(&mut control, w, h, &EditRecipe::default());
        let right0 = var(&data, 4..8);
        apply_develop(&mut data, w, h, &r);
        assert!(var(&data, 4..8) < right0 * 0.8, "right half should smooth");
        assert_eq!(&data[0..4], &control[0..4], "left half untouched, bit for bit");
    }

    #[test]
    fn luminance_range_mask_gates_by_pixel_brightness() {
        // Full-coverage geometry (degenerate linear = weight 1 everywhere) so
        // ONLY the luminance range decides where the −2 EV darken lands. The
        // trapezoid uses a degenerate top edge (hi == hi_outer == 1.0), exactly
        // like the real ACR sidecars' `LumRange="… 1.000000 1.000000"`.
        let full = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 };
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: full,
                range: Some(RangeMask::Luminance { lo_outer: 0.55, lo: 0.7, hi: 1.0, hi_outer: 1.0 }),
                amount: 1.0,
                exposure_ev: -2.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let dark = [0.15_f32; 3];
        let mid = [0.625_f32; 3]; // ramp midpoint between lo_outer and lo
        let bright = [0.85_f32; 3];
        // Control: identical pipeline WITHOUT the mask. The global stages run
        // either way (the neutral tone LUT still costs ~1 ULP of interpolation
        // noise), so "untouched by the mask" means equal to the CONTROL, not to
        // the raw input.
        let mut control = vec![dark, mid, bright];
        apply_develop(&mut control, 3, 1, &EditRecipe::default());
        let mut data = vec![dark, mid, bright];
        apply_develop(&mut data, 3, 1, &r);
        assert_eq!(data[0], control[0], "below the range: the mask must skip it");
        assert!(data[2][0] < 0.6, "bright pixel must darken: {}", data[2][0]);
        // The ramp midpoint moves, but less than the fully-selected pixel.
        let (d_mid, d_bright) = (control[1][0] - data[1][0], control[2][0] - data[2][0]);
        assert!(d_mid > 0.01 && d_mid < d_bright, "feathered ramp: mid {d_mid} vs bright {d_bright}");
    }

    #[test]
    fn color_range_mask_selects_chroma_not_brightness() {
        // Desaturate through a colour range keyed to orange: both bright and
        // dark orange collapse to grey (luminance-invariant match), while blue
        // and neutral grey pass through bit-exact.
        let full = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 };
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: full,
                range: Some(RangeMask::Color { r: 0.9, g: 0.6, b: 0.2, amount: 0.5, px: 0.5, py: 0.5 }),
                amount: 1.0,
                saturation: -100.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let orange = [0.9_f32, 0.6, 0.2];
        let dark_orange = [0.45_f32, 0.3, 0.1]; // same chromaticity, half as bright
        let blue = [0.2_f32, 0.3, 0.9];
        let grey = [0.6_f32; 3];
        // Same control-render comparison as the luminance test: out-of-range
        // pixels must match a mask-less render exactly (the mask pass skips them).
        let mut control = vec![orange, dark_orange, blue, grey];
        apply_develop(&mut control, 4, 1, &EditRecipe::default());
        let mut data = vec![orange, dark_orange, blue, grey];
        apply_develop(&mut data, 4, 1, &r);
        let spread = |p: [f32; 3]| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        assert!(spread(data[0]) < 0.05, "orange must desaturate: {:?}", data[0]);
        assert!(spread(data[1]) < 0.05, "dark orange (same hue) must desaturate: {:?}", data[1]);
        assert_eq!(data[2], control[2], "opposite hue: the mask must skip it");
        assert_eq!(data[3], control[3], "neutral grey: the mask must skip it");
        // Desaturation must land at the pixel's LUMA, not at black — spread
        // alone cannot tell grey from destroyed (a `c * factor` rewrite of
        // the sat formula yields [0,0,0] with spread 0 and passed) (U14).
        let lum = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        assert!(
            (data[0][0] - lum(control[0])).abs() < 0.03,
            "orange must keep its brightness: {:?} vs luma {}",
            data[0],
            lum(control[0])
        );
        assert!(
            (data[1][0] - lum(control[1])).abs() < 0.03,
            "dark orange must keep its brightness: {:?} vs luma {}",
            data[1],
            lum(control[1])
        );
    }

    #[test]
    fn local_temperature_warms_the_masked_region_only() {
        // Feedback batch #2-B prerequisite: LocalAdjustment carried Temp/Tint
        // since v1 and the XMP writer exports them, but the ENGINE ignored
        // them (render.rs listed them as "deferred") — so the GUI's mask
        // Temp/Tint sliders did nothing in-app, and the zoned reverse-fit
        // would have nothing to drive. A warm local temperature must boost
        // red / cut blue inside the mask; the uncovered end must stay equal
        // to a mask-less control render (the mask pass skips weight 0).
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0 },
                amount: 1.0,
                temperature: 100.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let grey = [0.5_f32; 3];
        let (w, h) = (1usize, 4usize);
        let mut control = vec![grey; w * h];
        apply_develop(&mut control, w, h, &EditRecipe::default());
        let mut data = vec![grey; w * h];
        apply_develop(&mut data, w, h, &r);
        assert_eq!(data[0], control[0], "zero end of the gradient: the mask must skip it");
        let px = data[3];
        assert!(
            px[0] > grey[0] + 0.02 && px[2] < grey[2] - 0.02,
            "full end must warm (red up, blue down): {px:?}"
        );
    }

    #[test]
    fn colour_gain_lut_matches_the_exact_linear_light_formula() {
        // Pin the optimization's fidelity independently of apply_wb: compare
        // LUT interpolation against the old exact formula over dark values
        // (where the sRGB knee is hardest), midtones and highlights, across
        // both sub-unity and strong zoned-fit gains. The tolerance is below
        // one 16-bit code value (1/65535 ≈ 1.53e-5).
        for gains in [[0.41f32, 0.91, 1.45], [1.65, 0.76, 0.38], [1.0, 1.0, 1.0]] {
            let luts = colour_gain_luts(gains);
            for x in [0.0f32, 0.001, 0.003, 0.01, 0.04, 0.1, 0.25, 0.5, 0.8, 0.99, 1.0] {
                for ch in 0..3 {
                    let exact =
                        linear_to_srgb((srgb_to_linear(x) * gains[ch]).clamp(0.0, 1.0));
                    let fast = sample_lut(&luts[ch], x);
                    assert!(
                        (fast - exact).abs() < 1.5e-5,
                        "gain {} x {x}: LUT {fast} vs exact {exact}",
                        gains[ch]
                    );
                }
            }
        }
    }

    #[test]
    fn full_frame_local_wb_matches_the_global_wb_stage() {
        // The local Temp/Tint must MIRROR apply_recipe_wb's semantics — same
        // wb_gains model, same 5500 K anchor, WB→tone→sat order — so a
        // weight-1 full-frame mask must land within LUT-quantization of a
        // global render whose absolute Kelvin is the mired-mapped target.
        // This pins local_temp_to_kelvin AND the tint sign end to end.
        let full = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 };
        let (t_rel, tint) = (60.0_f32, -25.0_f32);
        let src = [[0.7_f32, 0.55, 0.35], [0.2, 0.35, 0.6], [0.5, 0.5, 0.5]];
        let mut global = src.to_vec();
        apply_recipe_wb(
            &mut global,
            &EditRecipe {
                temperature_k: Some(local_temp_to_kelvin(t_rel)),
                tint,
                ..Default::default()
            },
        );
        apply_develop(&mut global, 3, 1, &EditRecipe::default());
        let mut local = src.to_vec();
        apply_develop(
            &mut local,
            3,
            1,
            &EditRecipe {
                masks: vec![LocalAdjustment {
                    mask: full,
                    amount: 1.0,
                    temperature: t_rel,
                    tint,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        for (a, b) in global.iter().zip(&local) {
            for c in 0..3 {
                assert!(
                    (a[c] - b[c]).abs() < 3e-3,
                    "full-frame local WB drifted from the global stage: {a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn exports_are_tagged_srgb_in_all_three_formats() {
        // Every export format must carry the sRGB profile: JPEG in an APP2
        // "ICC_PROFILE" segment, PNG in an iCCP chunk, TIFF as the raw profile
        // (tag 34675) whose header signature is "acsp".
        std::fs::create_dir_all("out").ok();
        let src_p = std::path::Path::new("out/_icc_src.png");
        RgbImage::from_fn(32, 16, |x, y| Rgb([(x * 8) as u8, (y * 16) as u8, 128]))
            .save(src_p)
            .unwrap();
        let neutral = EditRecipe::default();
        for (name, needle) in [
            ("out/_icc.jpg", &b"ICC_PROFILE"[..]),
            ("out/_icc.png", &b"iCCP"[..]),
            ("out/_icc.tif", &b"acsp"[..]),
        ] {
            render_to_file(src_p, &neutral, std::path::Path::new(name), None, None).unwrap();
            let bytes = std::fs::read(name).unwrap();
            assert!(
                bytes.windows(needle.len()).any(|win| win == needle),
                "{name} must carry the sRGB ICC marker"
            );
            // The markers above match ANY ICC payload — pin the PROFILE:
            // tag_icc's Srgb arm shipping the P3/Adobe bytes passed every
            // marker check (U14). JPEG/TIFF store the profile verbatim; PNG
            // deflate-compresses it, so compare the DECODED profile.
            if name.ends_with(".png") {
                let mut d = image::codecs::png::PngDecoder::new(std::io::BufReader::new(
                    std::fs::File::open(name).unwrap(),
                ))
                .unwrap();
                assert_eq!(
                    image::ImageDecoder::icc_profile(&mut d).unwrap().unwrap(),
                    SRGB_ICC.to_vec(),
                    "{name} must embed the sRGB profile (decompressed iCCP)"
                );
            } else {
                assert!(
                    bytes.windows(SRGB_ICC.len()).any(|win| win == SRGB_ICC),
                    "{name} must embed the sRGB profile bytes"
                );
            }
        }
    }

    #[test]
    fn gamut_transform_is_colorimetric_not_a_tag_swap() {
        // (a) White preservation pins the whole matrix derivation: every row of
        // sRGB→target must sum to 1 (R=G=B=1 stays exactly white — all three
        // spaces share the D65 white point, so no adaptation term may appear).
        for space in [ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb] {
            let m = srgb_to_space_matrix(space).unwrap();
            for (i, row) in m.iter().enumerate() {
                let s: f32 = row.iter().sum();
                assert!((s - 1.0).abs() < 1e-3, "{space:?} row {i} sums to {s}");
            }
            // (b) Invertibility: a color grid survives forward → inverse.
            let inv = inv3(&m);
            for c in [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.7, 0.2, 0.55]] {
                let back = mat_vec3(&inv, &mat_vec3(&m, &c));
                for k in 0..3 {
                    assert!((back[k] - c[k]).abs() < 1e-3, "{space:?} roundtrip {c:?} → {back:?}");
                }
            }
        }

        // (c) Full pixel path on a mid-grey: P3 shares sRGB's TRC, so a neutral
        // pixel is numerically UNCHANGED; Adobe RGB's pure gamma encodes the
        // same grey darker — while staying exactly neutral. That difference is
        // the transform actually running (a tag swap would leave both equal).
        let grey = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(2, 2, Rgb([32896u16, 32896, 32896])));
        let p3 = convert_export_color_space(grey.clone(), ExportColorSpace::DisplayP3).to_rgb16();
        let (pr, pg, pb) = (p3.get_pixel(0, 0)[0], p3.get_pixel(0, 0)[1], p3.get_pixel(0, 0)[2]);
        assert!(pr == pg && pg == pb, "P3 grey must stay neutral: {pr},{pg},{pb}");
        assert!((pr as i32 - 32896).abs() <= 4, "P3 grey must keep its value: {pr}");
        let ad = convert_export_color_space(grey.clone(), ExportColorSpace::AdobeRgb).to_rgb16();
        let (ar, ag, ab) = (ad.get_pixel(0, 0)[0], ad.get_pixel(0, 0)[1], ad.get_pixel(0, 0)[2]);
        assert!(ar == ag && ag == ab, "AdobeRGB grey must stay neutral: {ar},{ag},{ab}");
        assert!((ar as i32) < pr as i32 - 64, "AdobeRGB gamma must encode grey darker: {ar} vs {pr}");

        // (d) Saturated sRGB red. P3's red primary sits further out, so sRGB
        // red lands strictly INSIDE (dominant red, positive green/blue).
        // Adobe RGB shares sRGB's red CHROMATICITY, so sRGB red stays a pure
        // red there — just rescaled (Adobe's red carries a larger luminance
        // share): g = b = 0 with red below full scale. Both derive from the
        // primaries table, so both directions pin the matrix.
        let red = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(1, 1, Rgb([65535u16, 0, 0])));
        let p3r = convert_export_color_space(red.clone(), ExportColorSpace::DisplayP3).to_rgb16();
        let p = p3r.get_pixel(0, 0);
        assert!(
            p[0] > 55000 && p[1] > 0 && p[2] > 0 && p[1] < p[0] && p[2] < p[0],
            "DisplayP3: sRGB red must land inside the gamut, got {p:?}"
        );
        let adr = convert_export_color_space(red, ExportColorSpace::AdobeRgb).to_rgb16();
        let q = adr.get_pixel(0, 0);
        assert!(
            q[0] > 50000 && q[0] < 62000 && q[1] <= 300 && q[2] <= 300,
            "AdobeRGB: sRGB red must stay a rescaled pure red, got {q:?}"
        );

        // (d2) Green and blue primaries pin the REMAINING columns: the red
        // probe alone survives a green/blue column swap of the matrix — row
        // sums, invertibility, grey and red are all invariant under it, and
        // so is the calibration cross-check, which derives from the same
        // primaries table (U14). All three spaces share the same blue
        // CHROMATICITY, so sRGB blue stays a pure blue in both targets;
        // under the swap each primary would receive the other's column.
        let green = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(1, 1, Rgb([0u16, 65535, 0])));
        let blue = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(1, 1, Rgb([0u16, 0, 65535])));
        for space in [ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb] {
            let g = convert_export_color_space(green.clone(), space).to_rgb16();
            let p = g.get_pixel(0, 0);
            assert!(
                p[1] > 50000 && p[0] < p[1] && p[2] < p[1],
                "{space:?}: sRGB green must stay dominant-green, got {p:?}"
            );
            let b = convert_export_color_space(blue.clone(), space).to_rgb16();
            let p = b.get_pixel(0, 0);
            assert!(
                p[2] > 55000 && p[0] < 1000 && p[1] < 1000,
                "{space:?}: sRGB blue shares the target's blue primary — must stay pure, got {p:?}"
            );
        }

        // (e) sRGB is the identity (now a MOVE, not a clone).
        let same = convert_export_color_space(grey, ExportColorSpace::Srgb).to_rgb16();
        assert_eq!(same.get_pixel(1, 1)[0], 32896);
    }

    #[test]
    fn wide_develop_calibration_agrees_with_the_export_matrix() {
        // A synthetic camera whose native space IS sRGB: xyz2cam =
        // inv(sRGB→XYZ). The DNG calibration into a target space must then
        // equal the sRGB→target export matrix — the two derivations meet.
        let xyz2cam = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
        for space in [ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb] {
            let cam2space = camera_to_space_matrix(&xyz2cam, space);
            let reference = srgb_to_space_matrix(space).unwrap();
            for i in 0..3 {
                for j in 0..3 {
                    assert!(
                        (cam2space[i][j] - reference[i][j]).abs() < 2e-3,
                        "{space:?} [{i}][{j}]: {} vs {}",
                        cam2space[i][j],
                        reference[i][j]
                    );
                }
            }
        }
    }

    #[test]
    fn wide_develop_keeps_out_of_srgb_camera_colors_and_neutral_parity() {
        // A camera whose native space IS Display P3: its pure red lies
        // OUTSIDE sRGB. Developed INTO DisplayP3 it must survive as ~[1,0,0]
        // — the colour the old clip-at-sRGB pipeline destroyed.
        let xyz2cam = inv3(&rgb_to_xyz(P3_PRIM, D65_XY));
        let mut px = [[1.0f32, 0.0, 0.0]];
        calibrate_camera_buffer(&mut px, &xyz2cam, [1.0, 1.0, 1.0], ExportColorSpace::DisplayP3);
        let p = px[0];
        assert!(
            p[0] > 0.99 && p[1].abs() < 0.02 && p[2].abs() < 0.02,
            "P3-native red must survive a P3 develop, got {p:?}"
        );
        // The same HUE at 80% (an unblown saturated colour — full-scale 1.0
        // is a clipped sensor reading and legitimately takes the highlight
        // desaturation, same as rawler) into sRGB goes out of gamut: a
        // NEGATIVE component must reach the caller (the final pack clips it
        // — not the decode).
        let mut px = [[0.8f32, 0.0, 0.0]];
        calibrate_camera_buffer(&mut px, &xyz2cam, [1.0, 1.0, 1.0], ExportColorSpace::Srgb);
        assert!(
            px[0][1] < -0.001 || px[0][2] < -0.001,
            "out-of-gamut components must SURVIVE to the pack, got {:?}",
            px[0]
        );
        // Neutral parity: a white-balanced grey encodes to the SAME value in
        // every space (shared D65 white + shared working transfer) — the
        // whole reason the wide develop may share the sRGB tone pipeline.
        let wb = [2.0f32, 1.0, 1.5];
        let grey_cam = [[0.4 / 2.0, 0.4, 0.4 / 1.5]];
        let mut out = [[0.0f32; 3]; 3];
        for (i, space) in
            [ExportColorSpace::Srgb, ExportColorSpace::DisplayP3, ExportColorSpace::AdobeRgb]
                .into_iter()
                .enumerate()
        {
            let mut px = grey_cam;
            calibrate_camera_buffer(&mut px, &xyz2cam, wb, space);
            out[i] = px[0];
        }
        let want = linear_to_srgb(0.4);
        for (i, o) in out.iter().enumerate() {
            for c in o {
                assert!((c - want).abs() < 2e-3, "space {i}: grey drifted to {o:?} (want {want})");
            }
        }
    }

    #[test]
    fn adobe_trc_transcode_matches_the_conversion_path_on_neutrals() {
        // A grey through the native-wide path (primaries already Adobe,
        // transfer swap only) must land where the matrix path lands it —
        // on neutrals the matrix is a no-op, isolating the TRC.
        let grey = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(1, 1, Rgb([32896u16, 32896, 32896])));
        let via_matrix =
            convert_export_color_space(grey.clone(), ExportColorSpace::AdobeRgb).to_rgb16();
        let via_transcode = transcode_srgb_trc_to_adobe(grey).to_rgb16();
        let (a, b) = (via_matrix.get_pixel(0, 0), via_transcode.get_pixel(0, 0));
        for c in 0..3 {
            assert!(
                (a[c] as i32 - b[c] as i32).abs() <= 1,
                "TRC transcode must agree with the conversion path: {a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn exports_embed_the_selected_wide_gamut_profile() {
        // JPEG (APP2, one segment at 736 B) and TIFF (tag 34675) store the raw
        // profile — the ENTIRE profile bytes must appear in the file. PNG
        // deflate-compresses inside iCCP, so its profile is compared
        // DECOMPRESSED via the decoder — the old claim that the sRGB test's
        // chunk check covered it was FALSE: that check only proves an iCCP
        // chunk exists, so a PNG arm shipping sRGB bytes under a P3
        // selection passed everything (U14).
        std::fs::create_dir_all("out").ok();
        let src_p = std::path::Path::new("out/_gamut_src.png");
        RgbImage::from_fn(24, 12, |x, y| Rgb([(x * 10) as u8, (y * 20) as u8, 90]))
            .save(src_p)
            .unwrap();
        let neutral = EditRecipe::default();
        for (space, profile) in [
            (ExportColorSpace::DisplayP3, DISPLAY_P3_ICC),
            (ExportColorSpace::AdobeRgb, ADOBE_RGB_ICC),
        ] {
            let opts = ExportOpts { color_space: space, ..Default::default() };
            for name in ["out/_gamut.jpg", "out/_gamut.tif"] {
                render_to_file(src_p, &neutral, std::path::Path::new(name), None, Some(&opts)).unwrap();
                let bytes = std::fs::read(name).unwrap();
                assert!(
                    bytes.windows(profile.len()).any(|win| win == profile),
                    "{name} must embed the full {space:?} profile ({} B)",
                    profile.len()
                );
            }
            let png_name = "out/_gamut.png";
            render_to_file(src_p, &neutral, std::path::Path::new(png_name), None, Some(&opts)).unwrap();
            let mut d = image::codecs::png::PngDecoder::new(std::io::BufReader::new(
                std::fs::File::open(png_name).unwrap(),
            ))
            .unwrap();
            assert_eq!(
                image::ImageDecoder::icc_profile(&mut d).unwrap().unwrap(),
                profile.to_vec(),
                "{png_name} must embed the full {space:?} profile"
            );
        }
    }

    #[test]
    fn vignette_gain_is_radial_and_linear_light() {
        // A flat mid-grey field: +60 compensation must leave the exact centre
        // untouched, brighten the corner the most, and increase monotonically
        // with radius. Negative amount darkens the corner instead.
        let (w, h) = (9usize, 9usize);
        let flat = vec![[0.5_f32; 3]; w * h];
        let mut up = flat.clone();
        apply_radial_gain(&mut up, w, h, &manual_vignette_lut(60.0, 50.0));
        let centre = up[4 * w + 4][0];
        let mid = up[2 * w + 2][0]; // halfway toward the corner
        let corner = up[0][0];
        assert!((centre - 0.5).abs() < 1e-4, "centre must not move: {centre}");
        assert!(corner > mid && mid > centre, "radial monotone: {centre} < {mid} < {corner}");
        assert!(corner > 0.62, "corner must clearly brighten: {corner}");
        // Pin the NAMED linear-light formula at the exact corner (rn = 1 →
        // gain = 1.6): decode → gain → encode. Gamma-space multiplication
        // (0.5·1.6 = 0.8) clears every shape assertion here yet misses this
        // by 0.18 (U14).
        let expect = linear_to_srgb((srgb_to_linear(0.5) * 1.6).min(1.0));
        assert!(
            (corner - expect).abs() < 2e-3,
            "corner must follow the linear-light gain: {corner} vs {expect}"
        );
        // Grey in, grey out: the gain applies to all three channels — every
        // assertion above reads channel 0 only, so a red-only regression of
        // the per-channel loop passed with a strong colour cast (R12).
        for p in [&up[0], &up[2 * w + 2]] {
            assert!(
                (p[0] - p[1]).abs() < 1e-6 && (p[1] - p[2]).abs() < 1e-6,
                "vignette must stay neutral on grey: {p:?}"
            );
        }

        let mut down = flat.clone();
        apply_radial_gain(&mut down, w, h, &manual_vignette_lut(-60.0, 50.0));
        assert!(down[0][0] < 0.38, "negative amount darkens the corner: {}", down[0][0]);

        // Higher midpoint confines the effect to the corners: the halfway
        // pixel moves LESS than with the default midpoint.
        let mut tight = flat.clone();
        apply_radial_gain(&mut tight, w, h, &manual_vignette_lut(60.0, 100.0));
        assert!(tight[2 * w + 2][0] < mid, "midpoint 100 must spare the mid-field");
    }

    #[test]
    fn export_opts_resize_sharpen_quality() {
        // Synthetic 200×100 gradient source (baked path), rendered through the
        // delivery pipeline. Long edge 50 → 50×25 saved AND reported; a long
        // edge larger than the source never upscales; lower JPEG quality
        // produces a smaller file than higher quality.
        std::fs::create_dir_all("out").ok();
        let src_p = std::path::Path::new("out/_export_src.png");
        let img = RgbImage::from_fn(200, 100, |x, y| {
            Rgb([(x % 256) as u8, (y * 2 % 256) as u8, ((x + y) % 256) as u8])
        });
        img.save(src_p).unwrap();
        let neutral = EditRecipe::default();

        let small = ExportOpts { long_edge: Some(50), sharpen: 25.0, ..Default::default() };
        let (w, h) =
            render_to_file(src_p, &neutral, std::path::Path::new("out/_export_le50.png"), None, Some(&small))
                .unwrap();
        assert_eq!((w, h), (50, 25), "long edge 50 must fit 200×100 to 50×25");
        let saved = image::image_dimensions("out/_export_le50.png").unwrap();
        assert_eq!(saved, (50, 25), "saved file dims must match the report");

        let big = ExportOpts { long_edge: Some(400), ..Default::default() };
        let (w, h) =
            render_to_file(src_p, &neutral, std::path::Path::new("out/_export_le400.png"), None, Some(&big))
                .unwrap();
        assert_eq!((w, h), (200, 100), "long edge beyond source must NOT upscale");

        for (q, name) in [(30u8, "out/_export_q30.jpg"), (95u8, "out/_export_q95.jpg")] {
            let opts = ExportOpts { jpeg_quality: q, ..Default::default() };
            render_to_file(src_p, &neutral, std::path::Path::new(name), None, Some(&opts)).unwrap();
        }
        let (s30, s95) = (
            std::fs::metadata("out/_export_q30.jpg").unwrap().len(),
            std::fs::metadata("out/_export_q95.jpg").unwrap().len(),
        );
        assert!(s30 < s95, "q30 ({s30} B) must be smaller than q95 ({s95} B)");

        // Output sharpening must be OBSERVABLE: same source, same size, only
        // `sharpen` differs — the sharpened file needs strictly more edge
        // energy. Deleting the whole post-resize stage changed no previously
        // asserted quantity (dims and JPEG sizes are blind to it) (U14).
        let edge_p = std::path::Path::new("out/_export_edge.png");
        RgbImage::from_fn(200, 100, |x, _| Rgb([if x < 100 { 40 } else { 215 }; 3]))
            .save(edge_p)
            .unwrap();
        let export = |sharpen: f32, name: &str| {
            let opts = ExportOpts { long_edge: Some(100), sharpen, ..Default::default() };
            render_to_file(edge_p, &neutral, std::path::Path::new(name), None, Some(&opts)).unwrap();
            let px = image::open(name).unwrap().to_rgb8();
            let (w, h) = px.dimensions();
            // Per channel: a red-only sharpen raised the summed energy too,
            // so each channel gets its own comparison (R12).
            let mut energy = [0u64; 3];
            for y in 0..h {
                for x in 1..w {
                    for (c, e) in energy.iter_mut().enumerate() {
                        *e += (px[(x, y)][c] as i64 - px[(x - 1, y)][c] as i64).unsigned_abs();
                    }
                }
            }
            ((w, h), energy)
        };
        let (dim_flat, e_flat) = export(0.0, "out/_export_sharp0.png");
        let (dim_sharp, e_sharp) = export(100.0, "out/_export_sharp100.png");
        assert_eq!(dim_flat, dim_sharp, "only the sharpen knob differs");
        for c in 0..3 {
            assert!(
                e_sharp[c] > e_flat[c],
                "output sharpening must raise edge energy on channel {c}: {e_sharp:?} vs {e_flat:?}"
            );
        }
    }

    #[test]
    fn kelvin_to_rgb_warm_is_redder_than_cool() {
        let warm = kelvin_to_rgb(3000.0);
        let cool = kelvin_to_rgb(9000.0);
        // STRICT: a kelvin_to_rgb that ignored its argument satisfied the
        // old >= / <= forms (R12).
        assert!(warm[0] > cool[0], "warm red {} > cool red {}", warm[0], cool[0]);
        assert!(warm[2] < cool[2], "warm blue {} < cool blue {}", warm[2], cool[2]);
    }

    /// The published Tanner-Helland constants have a cliff at the 6600 K
    /// branch seam (green +1.31 %, blue +0.96 %) and a red plateau to 6688 K
    /// where the r/b ratio — the eyedropper's temperature signal — does not
    /// move at all. The recalibrated branches must be continuous at the seam
    /// and alive inside the formerly dead band.
    #[test]
    fn the_6600k_branch_seam_is_continuous_and_carries_a_temperature_signal() {
        // C0 at the seam: 2 K apart may differ by slope (≈2e-4 per channel),
        // never by a branch cliff (the old green cliff alone was 1.3e-2).
        let below = kelvin_to_rgb(6599.0);
        let above = kelvin_to_rgb(6601.0);
        for c in 0..3 {
            assert!(
                (below[c] - above[c]).abs() < 2e-3,
                "channel {c} jumps across the seam: {} vs {}",
                below[c],
                above[c]
            );
        }
        // Inside 6600–6688 K the old red branch sat clamped at 255 while blue
        // was 255 by definition — r/b pinned at 1.0, so every temperature in
        // the band solved identically. The ratio must move now.
        let a = kelvin_to_rgb(6610.0);
        let b = kelvin_to_rgb(6680.0);
        let (ra, rb) = (a[0] / a[2], b[0] / b[2]);
        assert!(
            (ra - rb).abs() > 1e-3,
            "the r/b temperature signal is still dead in-band: {ra} vs {rb}"
        );
    }

    #[test]
    fn wb_warmer_target_boosts_red_cuts_blue() {
        // Target warmer (higher K) than as-shot ⇒ Lightroom warms: red gain > 1, blue < 1.
        let g = wb_gains(5000.0, 7000.0, 0.0);
        assert!(g[0] > 1.0, "red gain {}", g[0]);
        assert!(g[2] < 1.0, "blue gain {}", g[2]);
        // Neutral (same K, no tint) ⇒ all gains ~1.
        let n = wb_gains(5500.0, 5500.0, 0.0);
        assert!((n[0] - 1.0).abs() < 1e-3 && (n[2] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn wb_eyedropper_neutralizes_a_synthetic_cast() {
        // Build the pixel a grey card shows under a known wrong WB: linear grey
        // L divided by the gains a (k0, tint0) correction WOULD apply — so that
        // correction is exactly what neutralises it. The solver must recover a
        // (k, tint) whose gains bring the pixel back to r≈g≈b, judged by the
        // same forward model (parameter identity is NOT required — nearby K
        // can neutralise equally well; neutrality is the contract).
        for (k0, tint0) in [(3200.0f32, 12.0f32), (7500.0, -18.0), (5500.0, 0.0)] {
            let g0 = wb_gains(5500.0, k0, tint0);
            let l = 0.18f32;
            let cast = [
                linear_to_srgb(l / g0[0]),
                linear_to_srgb(l / g0[1]),
                linear_to_srgb(l / g0[2]),
            ];
            let (k, tint) = solve_wb_from_neutral(cast, 5500.0);
            let g = wb_gains(5500.0, k, tint);
            let out = [
                srgb_to_linear(cast[0]) * g[0],
                srgb_to_linear(cast[1]) * g[1],
                srgb_to_linear(cast[2]) * g[2],
            ];
            let (mx, mn) = (
                out[0].max(out[1]).max(out[2]),
                out[0].min(out[1]).min(out[2]),
            );
            assert!(
                (mx - mn) / mx < 0.02,
                "cast for ({k0},{tint0}) not neutralised: solved ({k:.0},{tint:.1}) → {out:?}"
            );
        }
        // An already-neutral pixel solves to ~as-shot, ~zero tint.
        let (k, tint) = solve_wb_from_neutral([0.5, 0.5, 0.5], 5500.0);
        assert!((k - 5500.0).abs() < 300.0 && tint.abs() < 2.0, "neutral → ({k:.0},{tint:.1})");
    }

    #[test]
    fn as_shot_math_lands_on_canonical_illuminants() {
        // Identity camera matrix ⇒ camera space IS XYZ; the WB gains
        // neutralise the illuminant, so wb = 1/XYZ reconstructs it exactly.
        let id = [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        // D65 (X 0.95047, Y 1, Z 1.08883): McCamy ⇒ ~6504 K, and D65 sits
        // Duv ≈ +0.0032 ABOVE the Planckian locus ⇒ tint ≈ +10 — ACR's own
        // Daylight-preset tint, which pins the sign convention AND the scale.
        let (k, t) = wb_to_kelvin_tint(&id, [1.0 / 0.95047, 1.0, 1.0 / 1.08883]).unwrap();
        assert!((6400.0..=6600.0).contains(&k), "D65 → {k:.0} K");
        assert!((5.0..=15.0).contains(&t), "D65 → tint {t:+.1}");
        // Illuminant A (X 1.09850, Z 0.35585) lies ON the locus: ~2856 K, ~0.
        let (k, t) = wb_to_kelvin_tint(&id, [1.0 / 1.0985, 1.0, 1.0 / 0.35585]).unwrap();
        assert!((2790.0..=2920.0).contains(&k), "A → {k:.0} K");
        assert!(t.abs() < 6.0, "A → tint {t:+.1}");
        // Damaged coefficients refuse instead of anchoring nonsense.
        assert_eq!(wb_to_kelvin_tint(&id, [f32::NAN, 1.0, 1.0]), None);
        assert_eq!(wb_to_kelvin_tint(&id, [0.0, 1.0, 1.0]), None);
    }

    #[test]
    fn stamped_as_shot_anchor_moves_only_kelvin_edits() {
        // tint-only renders identically stamped or legacy — the tint gain
        // never depends on the anchor — so no old archive can move.
        let grey = [[0.5f32, 0.5, 0.5]; 4];
        let mut legacy_px = grey;
        apply_recipe_wb(&mut legacy_px, &EditRecipe { tint: 20.0, ..Default::default() });
        let mut stamped_px = grey;
        apply_recipe_wb(
            &mut stamped_px,
            &EditRecipe { tint: 20.0, as_shot_k: Some(4000.0), ..Default::default() },
        );
        assert_eq!(legacy_px, stamped_px, "tint-only must ignore the anchor");
        // An ABSOLUTE target equal to the stamped as-shot is a true no-op —
        // the honest semantic the 5500-anchored model could not express…
        let mut at_as_shot = grey;
        apply_recipe_wb(
            &mut at_as_shot,
            &EditRecipe {
                temperature_k: Some(4000.0),
                as_shot_k: Some(4000.0),
                ..Default::default()
            },
        );
        assert_eq!(at_as_shot, grey, "target == as-shot must not shift");
        // …while a LEGACY recipe with the same numbers still takes its tuned
        // 5500-anchored shift, byte-identical to the old engine.
        let mut legacy_shift = grey;
        apply_recipe_wb(
            &mut legacy_shift,
            &EditRecipe { temperature_k: Some(4000.0), ..Default::default() },
        );
        assert_ne!(legacy_shift, grey, "legacy 5500-anchored shift still applies");
    }

    #[test]
    fn export_refuses_a_recipe_whose_mask_raster_is_unreadable() {
        use crate::recipe::LocalAdjustment;
        let dir = std::env::temp_dir().join(format!("autoshop_maskgate_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("base.png");
        image::DynamicImage::new_rgb8(8, 8).save(&src).unwrap();
        let broken = LocalAdjustment {
            name: "sky".into(),
            amount: 1.0,
            exposure_ev: -0.5,
            mask: MaskGeometry::Bitmap { path: dir.join("gone.png").display().to_string() },
            ..Default::default()
        };
        let r = EditRecipe { masks: vec![broken.clone()], ..Default::default() };
        let out = dir.join("out.png");
        let err = render_to_file(&src, &r, &out, None, None).unwrap_err().to_string();
        assert!(err.contains("sky"), "the refusal names the mask: {err}");
        assert!(!out.exists(), "a refused export writes nothing");
        // amount = 0 is inert BY the recipe — nothing is being dropped.
        let mut disabled = broken;
        disabled.amount = 0.0;
        let r = EditRecipe { masks: vec![disabled], ..Default::default() };
        render_to_file(&src, &r, &out, None, None).expect("disabled mask exports fine");
        // A PARKED mask (default amount 1, every adjustment neutral) renders
        // nothing even with a healthy raster — its lost raster must not
        // block the export either.
        let parked = LocalAdjustment {
            name: "parked".into(),
            mask: MaskGeometry::Bitmap { path: dir.join("gone.png").display().to_string() },
            ..Default::default()
        };
        let r = EditRecipe { masks: vec![parked], ..Default::default() };
        render_to_file(&src, &r, &out, None, None).expect("engine-inert mask exports fine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn straighten_rotation_geometry_and_direction() {
        // (a) 0° is the identity (dims + pixels untouched). A non-uniform
        // frame and a byte comparison: the old uniform frame + dims-only
        // check passed even if the zero-angle branch returned cleared or
        // arbitrary pixels (R12).
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(40, 30, |x, y| {
            image::Rgb([(x * 6) as u8, (y * 8) as u8, ((x + y) % 256) as u8])
        }));
        let same = rotate_straighten(&img, 0.0);
        assert_eq!((same.width(), same.height()), (40, 30));
        assert_eq!(
            same.to_rgb8().as_raw(),
            img.to_rgb8().as_raw(),
            "0° must return the pixels untouched"
        );

        // (b) inscribed_dims: identity at 0°, symmetric in ±θ, strictly smaller
        // than the frame for any real tilt.
        assert_eq!(inscribed_dims(120.0, 80.0, 0.0), (120.0, 80.0));
        let (w1, h1) = inscribed_dims(120.0, 80.0, 7.0);
        let (w2, h2) = inscribed_dims(120.0, 80.0, -7.0);
        assert!((w1 - w2).abs() < 1e-4 && (h1 - h2).abs() < 1e-4);
        assert!(w1 < 120.0 && h1 < 80.0 && w1 > 90.0 && h1 > 60.0, "({w1},{h1})");

        // (c) No black corners: an all-white frame stays all-white after any
        // tilt — the auto-crop must keep every sample inside the source.
        let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(120, 80, image::Rgb([255, 255, 255])));
        for deg in [3.0, 7.0, -12.0, 30.0] {
            let r = rotate_straighten(&white, deg).to_rgb8();
            let min = r.pixels().flat_map(|p| p.0).min().unwrap();
            assert!(min >= 250, "black bleed at {deg}°: min channel {min}");
        }

        // (d) Direction: positive = CLOCKWISE (the recipe contract). A vertical
        // red|blue split rotated clockwise tilts its divider top-to-the-right,
        // so just right of centre at the TOP row the red half now covers it.
        let mut split = RgbImage::new(100, 100);
        for (x, _y, p) in split.enumerate_pixels_mut() {
            *p = if x < 50 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) };
        }
        let rot = rotate_straighten(&DynamicImage::ImageRgb8(split), 10.0).to_rgb8();
        let (rw, _rh) = rot.dimensions();
        let probe = rot.get_pixel(rw / 2 + 3, 1);
        assert!(probe[0] > probe[2], "clockwise tilt must move red over top-centre-right: {probe:?}");
    }

    #[test]
    fn distortion_maps_are_inverse_and_directionally_correct() {
        let dims = (120.0, 80.0);
        // (a) amount = 0 is the exact identity, both directions.
        assert_eq!(distort_norm(0.31, 0.77, dims, 0.0), (0.31, 0.77));
        assert_eq!(undistort_norm(0.31, 0.77, dims, 0.0), (0.31, 0.77));
        // (b) the centre is a fixed point at any amount.
        for amt in [-100.0f32, -45.0, 60.0, 100.0] {
            let (cx, cy) = distort_norm(0.5, 0.5, dims, amt);
            assert!((cx - 0.5).abs() < 1e-5 && (cy - 0.5).abs() < 1e-5, "centre moved at {amt}");
        }
        // (c) Round-trips. view→orig→view must hold everywhere in the frame;
        // orig→view→orig only for content the correction keeps (interior
        // points — a +100 barrel fix legitimately crops the outermost corners,
        // and those originals have no preimage by design).
        for amt in [-100.0f32, -45.0, 60.0, 100.0] {
            for (nx, ny) in [(0.0, 0.0), (1.0, 0.0), (0.1, 0.9), (0.3, 0.4), (0.62, 0.85), (0.5, 0.5)] {
                let (ox, oy) = distort_norm(nx, ny, dims, amt);
                let (bx, by) = undistort_norm(ox, oy, dims, amt);
                assert!(
                    (bx - nx).abs() < 2e-3 && (by - ny).abs() < 2e-3,
                    "view roundtrip @{amt}: ({nx},{ny}) → ({ox},{oy}) → ({bx},{by})"
                );
            }
            for (nx, ny) in [(0.3, 0.4), (0.6, 0.35), (0.25, 0.7), (0.45, 0.52)] {
                let (vx, vy) = undistort_norm(nx, ny, dims, amt);
                let (bx, by) = distort_norm(vx, vy, dims, amt);
                assert!(
                    (bx - nx).abs() < 2e-3 && (by - ny).abs() < 2e-3,
                    "orig roundtrip @{amt}: ({nx},{ny}) → ({vx},{vy}) → ({bx},{by})"
                );
            }
        }
        // (d) Direction, via the radial sampling ratio f = r_src/r_dst probed
        // along the x-axis: a barrel fix (+) pulls samples INWARD, harder at
        // the edge (f < 1, decreasing); a pincushion fix (−) samples RELATIVELY
        // further out at the edge than at the centre (f increasing).
        let ratio = |nx: f32, amt: f32| {
            let (ox, _) = distort_norm(nx, 0.5, dims, amt);
            (ox - 0.5) / (nx - 0.5)
        };
        assert!(
            ratio(0.95, 100.0) < ratio(0.6, 100.0) && ratio(0.6, 100.0) < 1.0,
            "barrel fix direction: f(edge)={} f(mid)={}",
            ratio(0.95, 100.0),
            ratio(0.6, 100.0)
        );
        assert!(
            ratio(0.95, -100.0) > ratio(0.6, -100.0),
            "pincushion fix direction: f(edge)={} f(mid)={}",
            ratio(0.95, -100.0),
            ratio(0.6, -100.0)
        );
    }

    #[test]
    fn view_original_norm_maps_roundtrip_and_identity() {
        let dims = (1200.0, 800.0);
        let off = crate::recipe::LensProfile::default();
        // Identity when every control is zero.
        assert_eq!(view_to_original_norm(0.31, 0.77, dims, 0.0, &off, 0.0), (0.31, 0.77));
        assert_eq!(original_to_view_norm(0.31, 0.77, dims, 0.0, &off, 0.0), (0.31, 0.77));
        // Round-trip through straighten + manual distortion for interior
        // points (the composed map the web region box and every GUI mask
        // gesture ride on).
        for (deg, amt) in [(4.5f32, 0.0f32), (0.0, 35.0), (-3.0, -60.0), (7.0, 80.0)] {
            for (nx, ny) in [(0.3, 0.4), (0.55, 0.6), (0.42, 0.35), (0.5, 0.5)] {
                let (ox, oy) = view_to_original_norm(nx, ny, dims, deg, &off, amt);
                let (bx, by) = original_to_view_norm(ox, oy, dims, deg, &off, amt);
                assert!(
                    (bx - nx).abs() < 3e-3 && (by - ny).abs() < 3e-3,
                    "roundtrip deg={deg} amt={amt}: ({nx},{ny}) → ({ox},{oy}) → ({bx},{by})"
                );
                // A round trip alone proves only that the two maps invert
                // EACH OTHER: replace both with the identity and every
                // assertion above still passes while masks, brush strokes and
                // region boxes quietly stop tracking the displayed geometry.
                // So also require that active geometry actually MOVES an
                // off-centre point. (The frame centre is a fixed point of
                // both rotation and radial distortion — it is excluded.)
                if (deg != 0.0 || amt != 0.0) && (nx, ny) != (0.5, 0.5) {
                    assert!(
                        (ox - nx).abs() > 1e-4 || (oy - ny).abs() > 1e-4,
                        "deg={deg} amt={amt} left ({nx},{ny}) unmoved — the map is inert"
                    );
                }
            }
        }
    }

    #[test]
    fn lens_profile_vignette_lifts_corners_only_and_geometry_roundtrips() {
        use crate::recipe::LensProfile;
        // Real A7RIV-shaped data (DSC08276 conversions): rising corner gains,
        // falling distortion factors (barrel), near-unity CA.
        let profile = LensProfile {
            vignette: (0..16).map(|i| 1.0 + 0.42 * (i as f32 / 15.0).powi(2)).collect(),
            distortion: (0..16).map(|i| 1.0008 - 0.053 * (i as f32 / 15.0).powi(2)).collect(),
            ca_r: vec![1.0005; 16],
            ca_b: vec![0.9995; 16],
            vignette_on: true,
            distortion_on: true,
            ca_on: true,
        };
        // (a) Vignette: corners brighten, the centre stays put.
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(120, 80, image::Rgb([100, 100, 100])));
        let vig_only = EditRecipe {
            lens_profile: LensProfile { distortion_on: false, ca_on: false, ..profile.clone() },
            ..Default::default()
        };
        let out = develop_preview(&base, &vig_only).to_rgb8();
        assert_eq!(out[(60, 40)][0], 100, "centre untouched (gain 1.0)");
        assert!(out[(0, 0)][0] > 110, "corner lifted, got {}", out[(0, 0)][0]);

        // (b) Geometry: forward/inverse round-trip through the composed map.
        let dims = (1200.0, 800.0);
        for (nx, ny) in [(0.1, 0.1), (0.5, 0.2), (0.85, 0.7), (0.5, 0.5)] {
            let (ox, oy) = lens_geom_norm(nx, ny, dims, &profile, 20.0);
            let (bx, by) = lens_ungeom_norm(ox, oy, dims, &profile, 20.0);
            assert!(
                (bx - nx).abs() < 2e-3 && (by - ny).abs() < 2e-3,
                "roundtrip ({nx},{ny}) → ({ox},{oy}) → ({bx},{by})"
            );
        }
        // Inactive profile must be EXACTLY the manual path.
        let off = LensProfile::default();
        assert_eq!(lens_geom_norm(0.3, 0.8, dims, &off, 33.0), distort_norm(0.3, 0.8, dims, 33.0));

        // (c) Resample: a barrel profile pulls samples inward (like the manual
        // barrel fix) and leaves no unfilled pixels; identity-profile resample
        // with CA stays within a hair of the plain image.
        let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(121, 81, image::Rgb([255, 255, 255])));
        let r = apply_lens_geometry(&white, &profile, 0.0).to_rgb16();
        let min = r.pixels().flat_map(|p| p.0).min().unwrap();
        assert!(min >= 65000, "unfilled pixels through the profile map: min {min}");
        // A flat frame cannot see WHERE samples come from — an identity
        // resampler passes the white probe (U14). Encode x into the value:
        // the barrel profile must pull the left edge inward (nonzero source
        // x → nonzero value), and CA must sample R and B at their own radii
        // (ca_r > 1 reaches farther out than green; ca_b < 1 less far).
        let ramp = DynamicImage::ImageRgb16(ImageBuffer::from_fn(121, 81, |x, _| {
            Rgb([(x as u16) * 500; 3])
        }));
        let g = apply_lens_geometry(&ramp, &profile, 0.0).to_rgb16();
        let p = g.get_pixel(0, 40).0;
        assert!(p[1] > 300, "profile distortion inert at the left edge: {p:?}");
        assert!(
            p[0] < p[1] && p[1] < p[2],
            "CA directions: ca_r > 1 samples farther out (smaller ramp value \
             at the left edge), ca_b < 1 nearer (larger) — a symmetric split \
             also passed a swapped R/B correction (Codex batch 40): {p:?}"
        );
    }

    #[test]
    fn apply_lens_distortion_fills_the_frame_and_moves_content_radially() {
        // (a) 0 is the identity (pixels untouched); dims always preserved.
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(121, 81, image::Rgb([9, 200, 30])));
        assert_eq!(apply_lens_distortion(&img, 0.0).to_rgb8().as_raw(), img.to_rgb8().as_raw());
        let out = apply_lens_distortion(&img, 70.0);
        assert_eq!((out.width(), out.height()), (121, 81));

        // (b) No un-sampled (black) pixels for EITHER sign: k ≤ 0 fills by
        // construction, k > 0 relies on the Newton fill scale.
        let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(121, 81, image::Rgb([255, 255, 255])));
        for amt in [100.0f32, -100.0, 55.0, -55.0] {
            let r = apply_lens_distortion(&white, amt).to_rgb16();
            let min = r.pixels().flat_map(|p| p.0).min().unwrap();
            assert!(min >= 65000, "unfilled pixels at amount {amt}: min {min}");
        }

        // (c) The exact centre is a fixed point of the resample.
        let mut cdot = RgbImage::from_pixel(121, 81, image::Rgb([0, 0, 0]));
        cdot.put_pixel(60, 40, image::Rgb([255, 255, 255]));
        for amt in [100.0f32, -100.0] {
            let m = apply_lens_distortion(&DynamicImage::ImageRgb8(cdot.clone()), amt).to_rgb16();
            assert!(m.get_pixel(60, 40)[0] > 30000, "centre must be a fixed point at {amt}");
        }

        // (d) A +100 barrel fix (fill scale = 1) pushes content OUTWARD: a
        // white 3×3 dot centred at x=30 on the horizontal centreline (frame
        // centre x=60) must land further LEFT (predicted ≈ x 28.6).
        let mut dot = RgbImage::from_pixel(121, 81, image::Rgb([0, 0, 0]));
        for yy in 39..=41 {
            for xx in 29..=31 {
                dot.put_pixel(xx, yy, image::Rgb([255, 255, 255]));
            }
        }
        let moved = apply_lens_distortion(&DynamicImage::ImageRgb8(dot), 100.0).to_rgb16();
        let bright_x = (0..121u32).max_by_key(|&x| moved.get_pixel(x, 40)[0]).unwrap();
        assert!(
            moved.get_pixel(bright_x, 40)[0] > 30000 && bright_x <= 29,
            "barrel fix must move the dot outward (x<30), got x={bright_x}"
        );
    }

    #[test]
    fn bitmap_masks_gate_by_the_raster_and_fail_inert() {
        use crate::recipe::{LocalAdjustment, MaskGeometry};
        // A left-white / right-black raster driving an exposure-up local mask:
        // the white half must brighten vs a control render through the SAME
        // pipeline, the black half must stay byte-identical to the control.
        std::fs::create_dir_all("out").ok();
        let mask_p = "out/_bitmap_mask.png";
        image::GrayImage::from_fn(40, 20, |x, _| image::Luma([if x < 20 { 255u8 } else { 0 }]))
            .save(mask_p)
            .unwrap();
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(40, 20, image::Rgb([100, 100, 100])));
        let control = develop_preview(&base, &EditRecipe::default()).to_rgb8();
        let masked = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: mask_p.into() },
                exposure_ev: 1.5,
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = develop_preview(&base, &masked).to_rgb8();
        let (white_side, ctrl_w) = (out.get_pixel(5, 10)[0], control.get_pixel(5, 10)[0]);
        let (black_side, ctrl_b) = (out.get_pixel(35, 10)[0], control.get_pixel(35, 10)[0]);
        assert!(
            white_side as i32 > ctrl_w as i32 + 25,
            "white half must brighten: {white_side} vs control {ctrl_w}"
        );
        assert_eq!(black_side, ctrl_b, "black half must be untouched by the mask");

        // A missing raster renders the mask INERT (weight 0, stderr warning),
        // never a crash and never a stuck full-frame adjustment.
        let missing = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: "out/_no_such_mask_xyz.png".into() },
                exposure_ev: 1.5,
                ..Default::default()
            }],
            ..Default::default()
        };
        let inert = develop_preview(&base, &missing).to_rgb8();
        assert_eq!(inert.get_pixel(5, 10)[0], ctrl_w, "missing raster ⇒ mask inert");
        assert_eq!(inert.get_pixel(35, 10)[0], ctrl_b);
    }

    #[test]
    fn missing_bitmap_raster_is_inert_even_when_inverted() {
        use crate::recipe::MaskGeometry;
        // An unloadable raster carries no coverage, so `inverted` must NOT turn
        // its zero weight into full-frame coverage — render AND overlay have to
        // match a no-mask control for both inversion states.
        let base =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(24, 16, image::Rgb([100, 100, 100])));
        let control = develop_preview(&base, &EditRecipe::default()).to_rgb8();
        for inverted in [false, true] {
            let adj = LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: "out/_no_such_mask_inverted.png".into() },
                exposure_ev: 2.0,
                saturation: -100.0,
                inverted,
                ..Default::default()
            };
            let r = EditRecipe { masks: vec![adj.clone()], ..Default::default() };
            let out = develop_preview(&base, &r).to_rgb8();
            assert_eq!(
                out.as_raw(),
                control.as_raw(),
                "missing raster (inverted={inverted}) must render byte-identically to no mask"
            );
            let cov = mask_coverage(&adj, &base);
            assert!(
                cov.as_raw().iter().all(|&v| v == 0),
                "overlay must show zero coverage (inverted={inverted})"
            );
        }
    }

    #[test]
    fn radial_feather_zero_stays_finite_on_the_boundary() {
        use crate::recipe::MaskGeometry;
        // Full-frame radial, feather 0: the normalised distance is EXACTLY 1.0
        // at (0.0, 0.5), where the unguarded smoothstep divided 0/0 → NaN.
        let g = MaskGeometry::Radial {
            top: 0.0,
            left: 0.0,
            bottom: 1.0,
            right: 1.0,
            feather: 0.0,
            roundness: 0.0,
            flipped: false,
            angle: 0.0,
        };
        assert_eq!(mask_weight(&g, 0.0, 0.5, None), 0.0, "hard edge: boundary is outside");
        assert_eq!(mask_weight(&g, 0.5, 0.5, None), 1.0, "hard edge: centre is inside");
        for i in 0..=40 {
            for j in 0..=40 {
                let w = mask_weight(&g, i as f32 / 40.0, j as f32 / 40.0, None);
                assert!(w.is_finite() && (0.0..=1.0).contains(&w), "weight {w} at ({i},{j})");
            }
        }
        // User-visible symptom: a NaN weight blends to NaN and casts to 0 (a
        // black pixel). 40×20 puts pixel (0,10) exactly on the boundary.
        let base =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(40, 20, image::Rgb([120, 120, 120])));
        let r = EditRecipe {
            masks: vec![LocalAdjustment { mask: g, exposure_ev: 1.0, ..Default::default() }],
            ..Default::default()
        };
        let out = develop_preview(&base, &r).to_rgb8();
        assert!(
            out.as_raw().iter().all(|&v| v > 100),
            "feather 0 must brighten or leave pixels alone, never produce black"
        );
    }

    #[test]
    fn radial_roundness_is_a_documented_no_op() {
        use crate::recipe::MaskGeometry;
        // CONTRACT (see `MaskGeometry::Radial` in recipe.rs): roundness is
        // carried by recipe/XMP/AI schema but NOT rendered, because its scale
        // and sign are unverified. Pinning the no-op so any future
        // implementation lands together with the doc and the XMP round-trip.
        let radial = |roundness: f32| MaskGeometry::Radial {
            top: 0.2,
            left: 0.1,
            bottom: 0.8,
            right: 0.7,
            feather: 0.5,
            roundness,
            flipped: false,
            angle: 0.0,
        };
        for i in 0..=10 {
            for j in 0..=10 {
                let (nx, ny) = (i as f32 / 10.0, j as f32 / 10.0);
                let base = mask_weight(&radial(0.0), nx, ny, None);
                for r in [-100.0, -35.0, -1.0, 1.0, 35.0, 100.0] {
                    assert_eq!(
                        mask_weight(&radial(r), nx, ny, None),
                        base,
                        "roundness {r} must not change the weight at ({nx},{ny})"
                    );
                }
            }
        }
    }

    #[test]
    fn mask_components_compose_add_subtract_intersect() {
        use crate::recipe::{LocalAdjustment, MaskCombine, MaskComponent, MaskGeometry};
        // Two orthogonal linear gradients give exact hand-computable weights:
        // base = nx (horizontal ramp), component = ny (vertical ramp). The
        // algebra is the MaskCombine doc contract — union without a seam,
        // subtract carves, intersect restricts.
        let with = |mode| LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.5, full_x: 1.0, full_y: 0.5 },
            components: vec![MaskComponent {
                geometry: MaskGeometry::Linear {
                    zero_x: 0.5,
                    zero_y: 0.0,
                    full_x: 0.5,
                    full_y: 1.0,
                },
                mode,
            }],
            ..Default::default()
        };
        for i in 0..=4 {
            for j in 0..=4 {
                let (nx, ny) = (i as f32 / 4.0, j as f32 / 4.0);
                let (b, c) = (nx, ny);
                for (mode, want) in [
                    (MaskCombine::Add, 1.0 - (1.0 - b) * (1.0 - c)),
                    (MaskCombine::Subtract, b * (1.0 - c)),
                    (MaskCombine::Intersect, b * c),
                ] {
                    let got = combined_mask_weight(&with(mode), nx, ny, None, &[None]);
                    assert!(
                        (got - want).abs() < 1e-6,
                        "{mode:?} at ({nx},{ny}): got {got}, want {want}"
                    );
                }
            }
        }
        // No components = exactly the base geometry (v1 compatibility).
        let plain = LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.5, full_x: 1.0, full_y: 0.5 },
            ..Default::default()
        };
        assert_eq!(combined_mask_weight(&plain, 0.3, 0.9, None, &[]), 0.3);
        // Components fold IN LIST ORDER: subtract-then-add differs from
        // add-then-subtract, so a reorder is a real semantic change.
        let vertical = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0 };
        let sub_then_add = LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.5, full_x: 1.0, full_y: 0.5 },
            components: vec![
                MaskComponent { geometry: vertical.clone(), mode: MaskCombine::Subtract },
                MaskComponent { geometry: vertical.clone(), mode: MaskCombine::Add },
            ],
            ..Default::default()
        };
        let (nx, ny) = (0.5, 0.75);
        let want = {
            let w = nx * (1.0 - ny);
            1.0 - (1.0 - w) * (1.0 - ny)
        };
        let got = combined_mask_weight(&sub_then_add, nx, ny, None, &[None, None]);
        assert!((got - want).abs() < 1e-6, "sequential fold: got {got}, want {want}");
    }

    #[test]
    fn morph_mask_grows_and_shrinks_by_the_given_radius() {
        // A single white pixel in an 9×9 black field: dilate(+2) must produce
        // a 5×5 white block (square element), erode(−1) of that block must
        // shrink it back to 3×3, and radius 0 is the identity.
        let mut g = image::GrayImage::new(9, 9);
        g.put_pixel(4, 4, image::Luma([255]));
        let grown = morph_mask(&g, 2);
        let white = |img: &image::GrayImage| {
            img.enumerate_pixels().filter(|(_, _, p)| p[0] == 255).count()
        };
        assert_eq!(white(&grown), 25, "dilate r=2: 5×5 block");
        let shrunk = morph_mask(&grown, -1);
        assert_eq!(white(&shrunk), 9, "erode r=1: back to 3×3");
        assert_eq!(morph_mask(&g, 0), g, "radius 0 is the identity");
        // Erode of the single dot wipes it — no wraparound resurrection.
        assert_eq!(white(&morph_mask(&g, -1)), 0, "erode kills a 1px dot");
    }

    #[test]
    fn refine_mask_guided_snaps_a_soft_boundary_onto_the_guide_edge() {
        // Guide: hard vertical edge (left black, right white) at full res.
        // Mask: LOW-RES version of the same selection whose upsampled
        // boundary would smear across ~8 px. The guided output must be
        // decisively dark left of the edge and bright right of it — i.e.
        // the boundary re-attaches to the guide's edge.
        let (w, h) = (64u32, 32u32);
        let guide = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, _| {
            if x < w / 2 { image::Rgb([10, 10, 10]) } else { image::Rgb([245, 245, 245]) }
        }));
        // 8× smaller mask of the same half-split (boundary lands between
        // texels — the upsample alone leaves a wide gray ramp).
        let small = image::GrayImage::from_fn(w / 8, h / 8, |x, _| {
            if x < w / 16 { image::Luma([0]) } else { image::Luma([255]) }
        });
        let refined = refine_mask_guided(&small, &guide, 4, 1e-4);
        assert_eq!(refined.dimensions(), (w, h), "output at guide resolution");
        // Sample rows away from borders; 4 px from the edge on each side.
        let y = h / 2;
        let far_l = refined.get_pixel(w / 2 - 4, y)[0];
        let far_r = refined.get_pixel(w / 2 + 4, y)[0];
        assert!(
            far_l < 40,
            "left of the guide edge must read as deselected, got {far_l}"
        );
        assert!(
            far_r > 215,
            "right of the guide edge must read as selected, got {far_r}"
        );
        // …and feather_mask is a smoke-checked smoothing: the hard edge
        // gains intermediate values without moving the extremes.
        let feathered = feather_mask(&refined, 2.0);
        assert_eq!(feathered.dimensions(), (w, h));
        assert!(feathered.get_pixel(2, y)[0] < 40 && feathered.get_pixel(w - 3, y)[0] > 215);
    }

    #[test]
    fn a_missing_component_raster_makes_the_whole_adjustment_inert() {
        use crate::recipe::{EditRecipe, LocalAdjustment, MaskCombine, MaskComponent, MaskGeometry};
        // A lost Subtract raster contributes 0 coverage — folding that in
        // would WIDEN the effect area, so the whole adjustment must go inert
        // (apply_masks / mask_coverage) and the strict raster snapshot must
        // refuse the deliverable, exactly like a lost BASE raster.
        let m = LocalAdjustment {
            exposure_ev: 1.0, // engine-active, so the export gate cares
            mask: MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.5, full_x: 1.0, full_y: 0.5 },
            components: vec![MaskComponent {
                geometry: MaskGeometry::Bitmap {
                    path: "Z:/__autoshop_definitely_missing__/raster.png".into(),
                },
                mode: MaskCombine::Subtract,
            }],
            name: "carved".into(),
            ..Default::default()
        };
        let r = EditRecipe { masks: vec![m], ..Default::default() };
        let err = load_mask_raster_snapshot(&r)
            .expect_err("a component raster counts for the deliverable refusal");
        assert!(
            err.to_string().contains("carved"),
            "the refusal names the mask whose edit would be dropped: {err:#}"
        );
        let cov = mask_coverage(&r.masks[0], &DynamicImage::new_rgb8(8, 8));
        assert!(
            cov.pixels().all(|p| p[0] == 0),
            "the overlay must not advertise coverage the render will not apply"
        );
    }

    /// L08: the mask list's ⚠ badge — only geometries whose raster cannot
    /// load are reported; a readable one is not.
    #[test]
    fn dead_bitmap_rasters_reports_only_unloadable_geometries() {
        use crate::recipe::{LocalAdjustment, MaskCombine, MaskComponent, MaskGeometry};
        let dir = std::env::temp_dir().join("autoshop-dead-raster-probe");
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good-raster.png");
        image::GrayImage::from_pixel(4, 4, image::Luma([200])).save(&good).unwrap();
        let missing = "Z:/__autoshop_definitely_missing__/raster.png";
        let m = LocalAdjustment {
            exposure_ev: 1.0,
            mask: MaskGeometry::Bitmap { path: good.to_string_lossy().into_owned() },
            components: vec![MaskComponent {
                geometry: MaskGeometry::Bitmap { path: missing.into() },
                mode: MaskCombine::Subtract,
            }],
            name: "badge".into(),
            ..Default::default()
        };
        assert_eq!(dead_bitmap_rasters(&m), vec![missing.to_string()]);
        let all_good = LocalAdjustment {
            exposure_ev: 1.0,
            mask: MaskGeometry::Bitmap { path: good.to_string_lossy().into_owned() },
            name: "clean".into(),
            ..Default::default()
        };
        assert!(dead_bitmap_rasters(&all_good).is_empty());
    }

    /// L02: the bounded mask-decode gate — a header claiming absurd
    /// dimensions is refused BEFORE the decoder allocates (the fixture is a
    /// header-only PNG with no pixel data: if the gate did not fire first,
    /// the decode would fail with a non-budget error and the assert catches
    /// the difference). A real small raster passes.
    #[test]
    fn open_mask_bounded_refuses_oversized_headers() {
        let dir = std::env::temp_dir().join("autoshop-mask-bounded-test");
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("small.png");
        image::GrayImage::from_pixel(4, 4, image::Luma([1])).save(&good).unwrap();
        assert!(open_mask_bounded(&good).is_ok());
        // A PNG signature + one IHDR chunk claiming 100000×100000 (a 40 GB
        // decode) and nothing else. The IHDR CRC must be real — the png
        // reader verifies it before yielding dimensions.
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
                }
            }
            crc ^ 0xFFFF_FFFF
        }
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(b"IHDR");
        ihdr.extend_from_slice(&100_000u32.to_be_bytes()); // width
        ihdr.extend_from_slice(&100_000u32.to_be_bytes()); // height
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit greyscale, no interlace
        let mut png: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&crc32(&ihdr).to_be_bytes());
        // The dimension probe reads chunks up to the first IDAT header —
        // give it an empty IDAT (and IEND) so it succeeds with zero pixel
        // data on disk.
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&crc32(b"IDAT").to_be_bytes());
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&crc32(b"IEND").to_be_bytes());
        let huge = dir.join("huge.png");
        std::fs::write(&huge, &png).unwrap();
        let err = open_mask_bounded(&huge).unwrap_err();
        assert!(err.to_string().contains("budget"), "{err}");
        let err = mask_from_memory_bounded(&png).unwrap_err();
        assert!(err.to_string().contains("budget"), "{err}");
    }

    #[test]
    fn orient_f32_matches_the_display_orientation_semantics() {
        // A 3×2 buffer whose pixels carry their own index: every one of the
        // EIGHT states must produce the exact hand-derived EXIF mapping
        // (Rotate90 = clockwise, so top-left → top-right; Transpose =
        // main-diagonal mirror; Transverse = anti-diagonal). Only Normal and
        // Rotate90 were pinned before — the other six arms of `oriented`,
        // including the two in-place A7 compositions, had no coverage, and
        // orient_f32 round-trips through `oriented`, so the table below is
        // derived from the EXIF definitions, NOT from the image crate (a
        // crate-op reference would compare the function against itself)
        // (U14).
        let src: Vec<[f32; 3]> = (0..6).map(|i| [i as f32, 0.0, 0.0]).collect();
        let cases: [(Orientation, (usize, usize), [usize; 6]); 8] = [
            (Orientation::Normal, (3, 2), [0, 1, 2, 3, 4, 5]),
            (Orientation::HorizontalFlip, (3, 2), [2, 1, 0, 5, 4, 3]),
            (Orientation::Rotate180, (3, 2), [5, 4, 3, 2, 1, 0]),
            (Orientation::VerticalFlip, (3, 2), [3, 4, 5, 0, 1, 2]),
            (Orientation::Rotate90, (2, 3), [3, 0, 4, 1, 5, 2]),
            (Orientation::Rotate270, (2, 3), [2, 5, 1, 4, 0, 3]),
            (Orientation::Transpose, (2, 3), [0, 3, 1, 4, 2, 5]),
            (Orientation::Transverse, (2, 3), [5, 2, 4, 1, 3, 0]),
        ];
        for (o, dims, map) in cases {
            let (out, w, h) = orient_f32(src.clone(), 3, 2, o);
            assert_eq!((w, h), dims, "{o:?} dims");
            let got: Vec<usize> = out.iter().map(|p| p[0] as usize).collect();
            assert_eq!(&got[..], &map[..], "{o:?} pixel mapping");
        }
    }

    #[test]
    fn thumbnail_binning_commutes_only_with_non_reversing_orientations() {
        // The probe behind the A7 decision to keep the working-resolution
        // cap AFTER orientation (see `render_to_image_in`) — RE-RUN with the
        // type-exact flip after the eight-state test exposed that the first
        // measurement's Transpose figure (0.48) was entirely the Rgba<u8>
        // flip adapter's quantization (U14). The true shape: `thumbnail`'s
        // integer binning uses forward bin edges, so it commutes EXACTLY
        // with pure axis swaps (Normal, Transpose) and diverges by one
        // source bin (≈1/97 on this gradient) under every orientation with
        // a REVERSAL component — mirrored bin edges of a non-integer ratio
        // don't line up. Cap-before-orientation therefore STAYS forbidden
        // (six of eight states would change preview pixels). The exact arm
        // also pins the type-exact flip: a quantizing Transpose flip shows
        // up here as ~0.5. If the reversing arm ever FAILS, the crate's
        // binning became mirror-symmetric — re-probe all eight states
        // before touching the pipeline order.
        let (w, h) = (97usize, 61usize);
        let data: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                [x as f32 / w as f32, y as f32 / h as f32, 0.0]
            })
            .collect();
        let diff = |o: Orientation| -> f32 {
            let (a, aw, ah) = {
                let (d, dw, dh) = orient_f32(data.clone(), w, h, o);
                downscale_f32(d, dw, dh, 40)
            };
            let (b, bw, bh) = {
                let (d, dw, dh) = downscale_f32(data.clone(), w, h, 40);
                orient_f32(d, dw, dh, o)
            };
            assert_eq!((aw, ah), (bw, bh), "{o:?}: dims always agree");
            a.iter()
                .zip(&b)
                .flat_map(|(p, q)| (0..3).map(move |c| (p[c] - q[c]).abs()))
                .fold(0.0f32, f32::max)
        };
        // Normal is excluded: orient_f32 is a passthrough there, so both
        // sides of the comparison would be the SAME call on equal inputs —
        // an assertion that cannot fail (R12). Transpose is the real
        // content: a pure axis swap that must commute with the binning, and
        // the arm that catches a quantizing flip (~0.5 here).
        let d = diff(Orientation::Transpose);
        assert!(d <= 1e-6, "Transpose must commute exactly, diff {d}");
        for o in [
            Orientation::HorizontalFlip,
            Orientation::VerticalFlip,
            Orientation::Rotate90,
            Orientation::Rotate180,
            Orientation::Rotate270,
            Orientation::Transverse,
        ] {
            let d = diff(o);
            assert!(
                d > 1e-4,
                "{o:?}: binning became mirror-symmetric (diff {d}) — re-probe \
                 all eight states before considering cap-before-orientation"
            );
        }
    }

    #[test]
    fn downscale_f32_is_an_unbiased_average() {
        // The working-resolution cap must not shift LEVELS. A flat field must
        // survive exactly, and a gradient's mean must be preserved: every GUI
        // preview, every retouch base and the camera-base-curve estimation all
        // run through this path, so a per-pixel offset here would wash out the
        // whole application (R12).
        let (w, h) = (97usize, 61usize);
        let flat: Vec<[f32; 3]> = vec![[0.25, 0.5, 0.75]; w * h];
        let (small, sw, sh) = downscale_f32(flat, w, h, 40);
        assert!(sw <= 40 && sh <= 40, "capped to {sw}x{sh}");
        for p in &small {
            for (c, want) in [0.25f32, 0.5, 0.75].iter().enumerate() {
                assert!(
                    (p[c] - want).abs() < 1e-4,
                    "flat field must survive the cap unchanged: {p:?}"
                );
            }
        }
        let ramp: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = (i % w) as f32 / (w - 1) as f32;
                [v; 3]
            })
            .collect();
        let mean_in = ramp.iter().map(|p| p[0] as f64).sum::<f64>() / (w * h) as f64;
        let (small, sw, sh) = downscale_f32(ramp, w, h, 40);
        let mean_out = small.iter().map(|p| p[0] as f64).sum::<f64>() / (sw * sh) as f64;
        assert!(
            (mean_out - mean_in).abs() < 0.02,
            "the cap must preserve the mean level: {mean_in} -> {mean_out}"
        );
        // AVERAGING, not merely level. Both arms above pass for a
        // nearest-neighbour sampler (measured: top-left 0.0023 and
        // bottom-right 0.0125 off the mean, inside the 0.02 budget, and a
        // flat field survives point sampling exactly) — so "optimising" the
        // inner loop into `*px = data[bottom * w + left]` would keep the
        // suite green while every preview started aliasing.
        //
        // One lit source pixel in a dark field: a box filter spreads it over
        // its whole window, so the output lands at 1/window_area — never at 0
        // (a point sampler that missed it) and never at 1 (one that hit it).
        let mut spike = vec![[0.0f32; 3]; w * h];
        spike[(h / 2) * w + w / 2] = [1.0; 3];
        let (small, _sw, _sh) = downscale_f32(spike, w, h, 40);
        let peak = small.iter().map(|p| p[0]).fold(0.0f32, f32::max);
        let lit = small.iter().filter(|p| p[0] > 0.0).count();
        assert_eq!(lit, 1, "exactly one output window contains the lit pixel");
        // Bin windows are ceil-based, so this fixture (97x61 -> 40x25, ratios
        // 2.425 and 2.44) gives each output pixel 2 or 3 source columns and
        // rows: the lit sample is therefore averaged over 4..=9 of them.
        assert!(
            (1.0 / 9.0..=1.0 / 4.0).contains(&peak),
            "one lit pixel must be AVERAGED over its 4..=9 sample window, got {peak}"
        );
    }

    #[test]
    fn mask_coverage_reports_the_engine_weight() {
        use crate::recipe::{LocalAdjustment, MaskGeometry, RangeMask};
        // (a) A top→bottom linear gradient over a flat grey reference: zero at
        // the top row, ~full at the bottom, ~half in the middle.
        let grey = DynamicImage::ImageRgb8(RgbImage::from_pixel(20, 20, image::Rgb([120, 120, 120])));
        let grad = LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.5, full_y: 1.0 },
            ..Default::default()
        };
        let cov = mask_coverage(&grad, &grey);
        assert_eq!(cov.get_pixel(10, 0)[0], 0, "zero end must be 0");
        assert!(cov.get_pixel(10, 19)[0] > 235, "full end: {}", cov.get_pixel(10, 19)[0]);
        let mid = cov.get_pixel(10, 10)[0];
        assert!((mid as i32 - 128).abs() < 15, "midpoint ≈ half: {mid}");

        // (b) amount halves the whole map; inversion flips its direction.
        let half = LocalAdjustment { amount: 0.5, ..grad.clone() };
        assert!((mask_coverage(&half, &grey).get_pixel(10, 19)[0] as i32 - 128).abs() < 15);
        let inv = LocalAdjustment { inverted: true, ..grad.clone() };
        let icov = mask_coverage(&inv, &grey);
        assert!(icov.get_pixel(10, 0)[0] > 235 && icov.get_pixel(10, 19)[0] < 20);

        // (c) A luminance range gates the map by the REFERENCE pixels: with a
        // bright-only range, the dark half of the reference reads 0 even where
        // the geometry is at full strength.
        let split = DynamicImage::ImageRgb8(RgbImage::from_fn(20, 20, |x, _| {
            if x < 10 { image::Rgb([30, 30, 30]) } else { image::Rgb([220, 220, 220]) }
        }));
        let ranged = LocalAdjustment {
            // Degenerate linear (zero == full) = weight 1 everywhere.
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 },
            range: Some(RangeMask::Luminance { lo_outer: 0.5, lo: 0.6, hi: 1.0, hi_outer: 1.0 }),
            ..Default::default()
        };
        let rcov = mask_coverage(&ranged, &split);
        assert_eq!(rcov.get_pixel(3, 10)[0], 0, "dark side gated out");
        assert!(rcov.get_pixel(16, 10)[0] > 235, "bright side kept: {}", rcov.get_pixel(16, 10)[0]);
    }

    #[test]
    fn preview_wb_is_live_and_matches_the_shared_stage() {
        // develop_preview must run the SAME apply_recipe_wb as the exports:
        // a warmer Kelvin target raises red vs blue on a grey preview, and a
        // tint-only recipe (temperature_k = None) is NOT a no-op.
        let grey = DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 2, image::Rgb([128, 128, 128])));
        let warm = EditRecipe { temperature_k: Some(8000.0), ..Default::default() };
        let w = develop_preview(&grey, &warm).to_rgb8();
        let p = w.get_pixel(0, 0);
        assert!(p[0] > p[2] + 5, "warm target must warm the preview: {p:?}");

        let tinted = EditRecipe { tint: 60.0, ..Default::default() };
        let t = develop_preview(&grey, &tinted).to_rgb8();
        let q = t.get_pixel(0, 0);
        assert!(q[1] < 126, "positive (magenta) tint must cut green: {q:?}");

        // EQUALITY with the shared stage, not just the right lean: a
        // preview-specific WB with the wrong anchor or magnitude satisfied
        // both directions above while preview and export visibly disagreed
        // (R12). With a neutral develop the preview is exactly
        // to_u8(apply_recipe_wb(pixels)).
        for recipe in [&warm, &tinted] {
            let mut manual = vec![[128.0f32 / 255.0; 3]; 4];
            apply_recipe_wb(&mut manual, recipe);
            let want = [to_u8(manual[0][0]), to_u8(manual[0][1]), to_u8(manual[0][2])];
            let got = develop_preview(&grey, recipe).to_rgb8();
            assert_eq!(
                got.get_pixel(0, 0).0,
                want,
                "preview WB must be the shared apply_recipe_wb stage, exactly"
            );
        }
    }

    #[test]
    fn specular_white_handling_diagnosis() {
        // Push one pixel through the full per-pixel develop (1x1 → spatial ops are
        // no-ops) to learn: is bright near-white "foam" greyed by a render BUG, or
        // only by aggressive recipe values? Run with `--nocapture` to read numbers.
        fn run(px: [f32; 3], r: &EditRecipe) -> [f32; 3] {
            let mut d = vec![px];
            apply_develop(&mut d, 1, 1, r);
            d[0]
        }
        let white = [1.0_f32, 1.0, 1.0];
        let foam = [0.88_f32, 0.93, 1.00]; // sky-lit foam: bright, slightly blue
        let lum = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        let hsv_sat = |p: [f32; 3]| {
            let mx = p[0].max(p[1]).max(p[2]);
            let mn = p[0].min(p[1]).min(p[2]);
            if mx > 1e-4 { (mx - mn) / mx } else { 0.0 }
        };

        // (1) NEUTRAL must preserve white — guards against a standalone render bug.
        let wn = run(white, &EditRecipe::default());
        eprintln!("neutral white -> {wn:?}");
        assert!(wn[0] > 0.99 && wn[1] > 0.99 && wn[2] > 0.99, "neutral greyed white: {wn:?}");

        let (_h, hsl_s, _l) = rgb_to_hsl(foam[0], foam[1], foam[2]);
        eprintln!("foam HSL-sat={hsl_s:.3}  HSV-sat={:.3}", hsv_sat(foam));

        let mut hsl_lum = crate::recipe::Hsl::default();
        hsl_lum.luminance[5] = -60.0; // Blue band
        let blue_lum = EditRecipe { hsl: hsl_lum, ..Default::default() };

        // THE FIX: a Blue-band luminance push must NOT crush near-white foam to
        // grey (chroma ≈ 0.12 → gate ≈ 0.37), yet MUST still darken a genuinely
        // vivid blue (chroma ≈ 0.65 → gate ≈ 1.0). Pre-fix the HSL-`s` gate (s≈1.0)
        // hit foam at full strength and it landed at luma 0.71 (a blue-grey).
        let foam_out = run(foam, &blue_lum);
        let vivid = [0.20_f32, 0.45, 0.85];
        let vivid_out = run(vivid, &blue_lum);
        eprintln!("foam  + blue lum-60 -> {foam_out:?} luma {:.2}", lum(foam_out));
        eprintln!("vivid + blue lum-60 -> {vivid_out:?} luma {:.2}", lum(vivid_out));
        assert!(lum(foam_out) > 0.80, "near-white foam must stay bright, got luma {:.2}", lum(foam_out));
        assert!(lum(vivid_out) < 0.90 * lum(vivid), "vivid blue must still darken (HSL still works)");
    }

    #[test]
    fn box_blur_preserves_uniform_plane() {
        // A flat plane must stay flat (DC preserved) after blurring.
        let (w, h) = (40usize, 30usize);
        let plane = vec![0.4_f32; w * h];
        let blurred = blur_plane(&plane, w, h, 5);
        assert!(blurred.iter().all(|&v| (v - 0.4).abs() < 1e-4));
    }

    #[test]
    fn neutral_recipe_is_near_identity() {
        // All-zero recipe ⇒ no clarity/sat/NR/sharpen, near-identity tone LUT.
        let mut data = vec![[0.2_f32, 0.5, 0.8], [0.9, 0.1, 0.4]];
        let orig = data.clone();
        apply_develop(&mut data, 2, 1, &EditRecipe::default());
        for (a, b) in data.iter().zip(orig.iter()) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 0.02, "channel drift {} vs {}", a[c], b[c]);
            }
        }
    }

    #[test]
    fn scurve_contrast_pins_ends_and_steepens_midtones() {
        // Positive contrast must keep 0→0 and 1→1 (pinned endpoints), darken a
        // shadow value, brighten a highlight value (the S shape), and stay
        // monotonic — the old linear stretch clipped instead of pinning.
        let lut = build_tone_lut(&EditRecipe { contrast: 80.0, ..Default::default() });
        assert!(sample_lut(&lut, 0.0) < 0.01, "black pinned: {}", sample_lut(&lut, 0.0));
        assert!(sample_lut(&lut, 1.0) > 0.99, "white pinned: {}", sample_lut(&lut, 1.0));
        assert!(sample_lut(&lut, 0.25) < 0.25, "shadow darkened: {}", sample_lut(&lut, 0.25));
        assert!(sample_lut(&lut, 0.75) > 0.75, "highlight brightened: {}", sample_lut(&lut, 0.75));
        let mut prev = -1.0;
        for &y in &lut {
            assert!(y >= prev - 1e-4, "non-monotonic: {y} after {prev}");
            prev = y;
        }
    }

    #[test]
    fn region_tones_target_four_different_zones() {
        // Each region owns a DISTINCT tonal zone, and — the muddy-water fix —
        // highlights/shadows act on the UPPER/LOWER tones and leave the MIDTONES
        // alone (the old wide bands gave highlights 0.6–1.0 authority at v≈0.5–0.65,
        // crushing mid-bright water). Gentle ±30 pushes keep the curve unclamped
        // except very near white.
        let base = build_tone_lut(&EditRecipe::default());
        let d = |r: &EditRecipe, x: f32| sample_lut(&build_tone_lut(r), x) - sample_lut(&base, x);
        let whites = EditRecipe { whites: 30.0, ..Default::default() };
        let highs = EditRecipe { highlights: 30.0, ..Default::default() };
        let shadows = EditRecipe { shadows: 30.0, ..Default::default() };
        let blacks = EditRecipe { blacks: 30.0, ..Default::default() };
        // The fix: neither highlights nor shadows may touch the midtone (0.5).
        assert!(d(&highs, 0.5).abs() < 0.01, "highlights must NOT touch the midtone: {}", d(&highs, 0.5));
        assert!(d(&shadows, 0.5).abs() < 0.01, "shadows must NOT touch the midtone: {}", d(&shadows, 0.5));
        // Each region still owns its zone (upper / white-point / lower / black-point).
        assert!(d(&highs, 0.75) > 0.03, "highlights lift the upper tones: {}", d(&highs, 0.75));
        assert!(d(&whites, 0.92) > 0.03, "whites lift the white point: {}", d(&whites, 0.92));
        assert!(d(&shadows, 0.25) > 0.03, "shadows lift the lower tones: {}", d(&shadows, 0.25));
        assert!(d(&blacks, 0.08) > 0.03, "blacks lift the black point: {}", d(&blacks, 0.08));
        // Differentiation: highlights concentrate BELOW white; whites concentrate AT white.
        assert!(d(&highs, 0.75) > d(&highs, 0.97), "highlights concentrate below the white point");
        assert!(d(&whites, 0.95) > d(&whites, 0.70), "whites concentrate at the white point");
    }

    #[test]
    fn sharpening_raises_local_contrast_at_an_edge() {
        // A vertical edge: sharpening should push the dark side darker / bright
        // side brighter (overshoot), increasing the edge step. The flat ends
        // (outside the ±3 px unsharp support at radius 1) must NOT move — a
        // global pointwise contrast curve also grows the step, and only the
        // flat-field control tells the two apart (U14).
        let (w, h) = (12usize, 1usize);
        let mut data: Vec<[f32; 3]> = (0..w)
            .map(|x| { let v = if x < 6 { 0.3 } else { 0.7 }; [v, v, v] })
            .collect();
        let before = data[6][0] - data[5][0];
        let r = EditRecipe { sharpening: 120.0, ..Default::default() };
        apply_develop(&mut data, w, h, &r);
        let after = data[6][0] - data[5][0];
        assert!(after > before, "edge step {after} should exceed {before}");
        // The unsharp is a LUMA op scaling all channels by one ratio — a
        // grey edge must stay grey. Every probe above reads channel 0, so a
        // red-only sharpen (chromatic halos, unsharpened green/blue) passed
        // (R12).
        for p in [&data[5], &data[6]] {
            assert!(
                (p[0] - p[1]).abs() < 1e-6 && (p[1] - p[2]).abs() < 1e-6,
                "sharpened grey must stay grey: {p:?}"
            );
        }
        for x in [0usize, 1, 10, 11] {
            let want = if x < 6 { 0.3 } else { 0.7 };
            assert!(
                (data[x][0] - want).abs() < 1e-6,
                "flat field at x={x} must not move: {} vs {want}",
                data[x][0]
            );
        }
    }

    // ---- dehaze -----------------------------------------------------------

    /// A colourful test frame under a synthetic atmospheric veil built with
    /// the actual scattering physics — `I = J·t + A·(1−t)` in LINEAR light
    /// (haze is additive in radiance, not in gamma): t=0.55, airlight 0.9.
    /// Lifted black, compressed contrast, desaturated.
    fn hazy_frame() -> (Vec<[f32; 3]>, usize, usize) {
        let (w, h) = (64usize, 32usize);
        let (t0, a0) = (0.55f32, 0.90f32);
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let l = x as f32 / (w - 1) as f32;
                let p = match y * 4 / h {
                    0 => [l, l, l],
                    1 => [l, l * 0.6, l * 0.2],
                    2 => [l * 0.2, l * 0.7, l],
                    _ => [l * 0.3, l, l * 0.4],
                };
                data.push(p.map(|c| {
                    linear_to_srgb(srgb_to_linear(c) * t0 + a0 * (1.0 - t0))
                }));
            }
        }
        (data, w, h)
    }

    fn mean_chroma(px: &[[f32; 3]]) -> f32 {
        px.iter().map(|p| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])).sum::<f32>()
            / px.len() as f32
    }

    fn luma_quantile_spread(px: &[[f32; 3]]) -> f32 {
        let mut lum: Vec<f32> = px.iter().map(luma601).collect();
        lum.sort_by(f32::total_cmp);
        lum[(lum.len() * 9) / 10] - lum[lum.len() / 10]
    }

    #[test]
    fn dehaze_zero_is_exact_identity() {
        // Like neutral_recipe_is_near_identity but bit-exact: the stage is
        // gated on != 0.0 and must not run at all.
        let (data, w, _) = hazy_frame();
        let mut out = data.clone();
        apply_dehaze(&mut out, w, 0.0);
        assert_eq!(data, out, "dehaze 0 must be a bit-exact no-op");
    }

    #[test]
    fn dehaze_positive_recovers_a_hazy_ramp() {
        // Haze removal must jointly deepen tone AND restore chroma — the
        // signature no combination of tone sliders (luma-preserving chroma)
        // reproduces.
        let (mut data, w, _) = hazy_frame();
        let spread0 = luma_quantile_spread(&data);
        let chroma0 = mean_chroma(&data);
        apply_dehaze(&mut data, w, 50.0);
        let spread1 = luma_quantile_spread(&data);
        let chroma1 = mean_chroma(&data);
        assert!(
            spread1 > spread0 * 1.15,
            "q90-q10 luma spread must grow ≥15%: {spread0:.3} → {spread1:.3}"
        );
        assert!(chroma1 > chroma0 * 1.10, "mean chroma must grow: {chroma0:.3} → {chroma1:.3}");
    }

    #[test]
    fn dehaze_protects_bright_sky_channel_order() {
        // A bright pale-blue sky pixel near the airlight sits at the model's
        // fixed point: strong dehaze must not blow it out, flip its channel
        // order (no magenta/cyan inversions), or move it far.
        let (mut data, w, _) = hazy_frame();
        let sky = [0.80f32, 0.85, 0.92];
        data[w / 2] = sky;
        apply_dehaze(&mut data, w, 75.0);
        let p = data[w / 2];
        assert!(p[2] > p[1] && p[1] > p[0], "channel order flipped: {p:?}");
        assert!(p[2] < 0.999, "sky blew out: {p:?}");
        let moved = (0..3).map(|c| (p[c] - sky[c]).abs()).fold(0.0f32, f32::max);
        assert!(moved < 0.15, "near-airlight pixel moved {moved:.3}: {p:?}");
    }

    #[test]
    fn dehaze_negative_adds_a_veil_without_clipping() {
        // Adding haze is a convex blend toward the airlight: black lifts,
        // chroma drops, a neutral ramp stays strictly monotone, nothing clips.
        let w = 64usize;
        let mut data: Vec<[f32; 3]> = (0..w)
            .map(|x| {
                let v = x as f32 / (w - 1) as f32;
                [v, v, v]
            })
            .collect();
        let (mut colour, cw, _) = hazy_frame();
        let chroma0 = mean_chroma(&colour);
        apply_dehaze(&mut colour, cw, -50.0);
        assert!(mean_chroma(&colour) < chroma0, "a veil must desaturate");
        apply_dehaze(&mut data, w, -50.0);
        assert!(data[0][0] > 0.05, "black point must lift under a veil: {}", data[0][0]);
        for i in 1..w {
            assert!(
                data[i][0] > data[i - 1][0],
                "veiled ramp must stay strictly increasing at {i}"
            );
        }
        for p in &data {
            assert!(p[0] < 1.0 && p[0] > 0.0, "veil must not clip: {p:?}");
        }
    }

    #[test]
    fn dehaze_is_gentle_on_a_clean_image() {
        // On an already-clean frame (deep blacks present → low airlight-relative
        // haze density on colourful pixels) positive dehaze must be a light
        // touch, not a re-grade: a saturated midtone probe barely moves.
        let (w, h) = (64usize, 4usize);
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let l = x as f32 / (w - 1) as f32;
                data.push(if y == 0 { [l, l, l] } else { [l, l * 0.5, l * 0.15] });
            }
        }
        let probe_idx = w + w / 2; // saturated orange, mid ramp
        let before = data[probe_idx];
        apply_dehaze(&mut data, w, 50.0);
        let after = data[probe_idx];
        let moved = (0..3).map(|c| (after[c] - before[c]).abs()).fold(0.0f32, f32::max);
        assert!(moved < 0.05, "clean saturated midtone moved {moved:.3}: {before:?} → {after:?}");
    }

    #[test]
    fn dehaze_airlight_does_not_phase_lock_to_any_small_period() {
        // 1024×512 = 524288 px → stride 2, i.e. the sampler sees half the
        // frame. Two periodic frames caught two different lock-ups: COLUMN
        // stripes locked a flat `step_by` sampler to one parity (U14), and a
        // CHECKERBOARD locks a +1-per-row shear to one diagonal phase (R12).
        // In both cases a one-pixel shift flipped the estimated airlight
        // between the 0.10 floor and the bright bin, so preview and export
        // disagreed on the same photo.
        //
        // The probe must be a BRIGHT pixel: for a dark one the model's
        // b = a·(1−t) = K·s·min cancels the airlight almost exactly (the two
        // estimates differ by ~4e-4 there, so a dark probe would have passed
        // with the broken sampler — R12).
        let (w, h) = (1024usize, 512usize);
        let stripes = |phase: usize, i: usize| (i % w + phase).is_multiple_of(2);
        let checker = |phase: usize, i: usize| ((i % w) + (i / w) + phase).is_multiple_of(2);
        for (name, pattern) in [
            ("columns", &stripes as &dyn Fn(usize, usize) -> bool),
            ("checkerboard", &checker as &dyn Fn(usize, usize) -> bool),
        ] {
            let frame = |phase: usize| -> Vec<[f32; 3]> {
                (0..w * h)
                    .map(|i| {
                        let v = if pattern(phase, i) { 0.05 } else { 0.85 };
                        [v, v, v]
                    })
                    .collect()
            };
            let mut a = frame(0);
            let mut b = frame(1);
            apply_dehaze(&mut a, w, 50.0);
            apply_dehaze(&mut b, w, 50.0);
            // Value-to-value: the frames are one-pixel-shifted copies, so the
            // same INPUT value must map to the same output in both. Index 1
            // of `a` and index 0 of `b` were both the BRIGHT 0.85.
            let (pa, pb) = (a[1][0], b[0][0]);
            assert!(
                (pa - pb).abs() < 1e-3,
                "{name}: airlight phase-locked to the pattern: {pa} vs {pb}"
            );
        }
    }

    /// Deterministic synthetic frame for the dehaze golden: a hazy sky band
    /// over a colourful ground, generated by pure integer arithmetic so the
    /// bytes are identical on every platform and compiler.
    fn dehaze_golden_frame() -> (Vec<[f32; 3]>, usize, usize) {
        let (w, h) = (16usize, 8usize);
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / (w - 1) as f32;
                let v = y as f32 / (h - 1) as f32;
                // Top rows: bright, low-contrast veil (the airlight source).
                // Bottom rows: saturated ground with a horizontal ramp.
                data.push(if y < 3 {
                    [0.62 + 0.30 * u, 0.66 + 0.28 * u, 0.74 + 0.24 * u]
                } else {
                    [0.10 + 0.70 * u, (0.08 + 0.55 * u) * (1.0 - 0.3 * v), 0.06 + 0.40 * u * v]
                });
            }
        }
        (data, w, h)
    }

    /// FNV-1a over the raw IEEE-754 bits of every channel — a one-`u64`
    /// witness that pins the output BIT-exactly (a 0.5-ULP drift changes it).
    fn frame_bits_fnv64(data: &[[f32; 3]]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for px in data {
            for c in px {
                for b in c.to_bits().to_le_bytes() {
                    hash ^= b as u64;
                    hash = hash.wrapping_mul(0x100_0000_01b3);
                }
            }
        }
        hash
    }

    #[test]
    fn dehaze_split_is_bit_identical_to_the_pre_split_golden() {
        // R22 split `apply_dehaze` into `dehaze_airlight` + `dehaze_px` so the
        // MASKED dehaze could reuse the exact same model. These two hashes were
        // captured from the PRE-split implementation on this frame; the split is
        // a refactor only if they still hold bit-for-bit (the tone/chroma
        // assertions in the tests above would survive a 1-ULP drift, this will
        // not). Golden: 2026-08-17, before the split landed.
        for (amount, want) in [(60.0f32, 0xb0ae_c36c_5a0d_6123u64), (-40.0, 0x74e8_603b_e639_28e7)]
        {
            let (mut data, w, _) = dehaze_golden_frame();
            apply_dehaze(&mut data, w, amount);
            assert_eq!(
                frame_bits_fnv64(&data),
                want,
                "dehaze {amount} drifted from the pre-split golden; px0 = {:?}",
                data[0]
            );
        }
    }

    #[test]
    fn unsharp_weighted_at_weight_one_is_bit_identical() {
        // `unsharp_luma` now DELEGATES to `unsharp_luma_weighted` with a
        // constant-1 weight, so comparing the two functions would be circular.
        // The reference here is the original formula written out longhand: the
        // weighted form adds `* wgt`, and float multiplication by exactly 1.0
        // is exact, so weight 1 must reproduce it to the bit.
        let (data, w, h) = detail_frame();
        for (radius, amount, midtone) in [(8usize, 0.5f32, true), (2, -0.35, false)] {
            let mut reference = data.clone();
            {
                let luma: Vec<f32> = reference.iter().map(luma601).collect();
                let blurred = blur_plane(&luma, w, h, radius);
                for (i, px) in reference.iter_mut().enumerate() {
                    let l = luma[i];
                    let detail = l - blurred[i];
                    let m = if midtone { 1.0 - (2.0 * l - 1.0).powi(2) } else { 1.0 };
                    let new_l = (l + amount * detail * m).clamp(0.0, 1.0);
                    scale_chroma(px, l, new_l);
                }
            }
            let mut weighted = data.clone();
            unsharp_luma_weighted(&mut weighted, w, h, radius, amount, midtone, |_, _, _| 1.0);
            assert_eq!(
                frame_bits_fnv64(&reference),
                frame_bits_fnv64(&weighted),
                "radius {radius} amount {amount} midtone {midtone}: weight-1 must be bit-identical"
            );
            assert_ne!(
                frame_bits_fnv64(&data),
                frame_bits_fnv64(&weighted),
                "the probe frame must actually be sharpened, or the test proves nothing"
            );
        }
    }

    #[test]
    fn mask_clarity_at_full_coverage_equals_global_clarity() {
        // The #15a/#10B fix, pinned at its strongest: a Linear mask whose zero
        // and full points COINCIDE carries weight 1 everywhere (mask_weight's
        // len2 < 1e-9 arm), so at Amount 1 a local Clarity +50 must render
        // EXACTLY what the global Clarity +50 stage renders — same radius model
        // (2% of the short edge, floored at 8 px), same midtone mask, same
        // operator. Bit-exact, measured: 0 differing channels of 9216.
        // Before R22 the local path rendered NOTHING at all.
        let (data, w, h) = detail_frame();
        let mut global = data.clone();
        apply_develop(&mut global, w, h, &EditRecipe { clarity: 50.0, ..Default::default() });
        let mut local = data.clone();
        apply_develop(
            &mut local,
            w,
            h,
            &EditRecipe {
                masks: vec![LocalAdjustment {
                    mask: MaskGeometry::Linear {
                        zero_x: 0.5,
                        zero_y: 0.5,
                        full_x: 0.5,
                        full_y: 0.5,
                    },
                    amount: 1.0,
                    clarity: 50.0,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        assert_ne!(
            frame_bits_fnv64(&data),
            frame_bits_fnv64(&global),
            "global clarity must move this frame, or the comparison is vacuous"
        );
        assert_eq!(
            frame_bits_fnv64(&global),
            frame_bits_fnv64(&local),
            "full-coverage local clarity must equal global clarity bit-for-bit"
        );
    }

    #[test]
    fn mask_texture_at_full_coverage_equals_the_unweighted_operator() {
        // Texture has NO global counterpart to compare against (EditRecipe has
        // no `texture` field — only LocalAdjustment does), so the reference is
        // the bare operator at texture's own calibration: small radius
        // (0.5% of the short edge, floored at 2 px) and NO midtone mask.
        // Bit-exact, measured: 0 differing channels of 9216.
        let (data, w, h) = detail_frame();
        let radius = ((0.005 * w.min(h) as f32).round() as usize).max(2);
        assert_eq!(radius, 2, "the 48px short edge must land on the 2px floor");
        let mut reference = data.clone();
        unsharp_luma(&mut reference, w, h, radius, 0.5, false);
        let mut local = data.clone();
        apply_develop(
            &mut local,
            w,
            h,
            &EditRecipe {
                masks: vec![LocalAdjustment {
                    mask: MaskGeometry::Linear {
                        zero_x: 0.5,
                        zero_y: 0.5,
                        full_x: 0.5,
                        full_y: 0.5,
                    },
                    amount: 1.0,
                    texture: 50.0,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        assert_ne!(
            frame_bits_fnv64(&data),
            frame_bits_fnv64(&reference),
            "the reference operator must move this frame, or the comparison is vacuous"
        );
        assert_eq!(
            frame_bits_fnv64(&reference),
            frame_bits_fnv64(&local),
            "full-coverage local texture must equal the unweighted operator bit-for-bit"
        );
    }

    #[test]
    fn mask_texture_halo_is_narrower_than_mask_clarity_halo() {
        // The two radii are the whole reason both sliders exist: clarity is
        // midtone VOLUME at a large radius, texture is fine DETAIL at a small
        // one. On a 128×64 step edge that is 8 px vs 2 px of box radius, and
        // three box passes spread each to ≈3×. Measured: 20 px vs 6 px of
        // half-width. Swapping the two radius formulas fails this.
        let (w, h) = (128usize, 64usize);
        let edge: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                // 0.35/0.65 keeps both plateaus well inside the midtone mask,
                // which would zero clarity's effect at 0.0 and 1.0.
                let v = if i % w < w / 2 { 0.35f32 } else { 0.65 };
                [v, v, v]
            })
            .collect();
        let halo_of = |m: LocalAdjustment| -> usize {
            let mut out = edge.clone();
            apply_develop(&mut out, w, h, &EditRecipe { masks: vec![m], ..Default::default() });
            let row = (h / 2) * w;
            (0..w)
                .filter(|x| (out[row + x][0] - edge[row + x][0]).abs() > 1e-3)
                .map(|x| x.abs_diff(w / 2))
                .max()
                .unwrap_or(0)
        };
        let full = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 };
        let clarity = halo_of(LocalAdjustment {
            mask: full.clone(),
            amount: 1.0,
            clarity: 60.0,
            ..Default::default()
        });
        let texture = halo_of(LocalAdjustment {
            mask: full,
            amount: 1.0,
            texture: 60.0,
            ..Default::default()
        });
        assert!(texture >= 2, "texture must actually reach the edge: {texture}");
        assert!(
            texture * 2 < clarity,
            "texture halo ({texture}px) must be far narrower than clarity's ({clarity}px)"
        );
    }

    #[test]
    fn mask_dehaze_renders_only_inside_the_mask() {
        // A left-half Linear mask (weight 1 at nx=0, ramping to 0 at nx=0.5 and
        // clamped past it) with Dehaze +100: every column left of the midpoint
        // must move, every column at or right of it must be BYTE-identical —
        // the local dehaze may not leak the frame-wide airlight inversion
        // outside its coverage. Measured: columns 0..=31 changed, 32..=63 not.
        let (w, h) = (64usize, 16usize);
        let base: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                // A hazy, low-contrast, slightly blue ramp — the case dehaze is for.
                let u = (i % w) as f32 / (w - 1) as f32;
                [0.30 + 0.5 * u, 0.34 + 0.45 * u, 0.42 + 0.40 * u]
            })
            .collect();
        let mut out = base.clone();
        apply_develop(
            &mut out,
            w,
            h,
            &EditRecipe {
                masks: vec![LocalAdjustment {
                    mask: MaskGeometry::Linear {
                        zero_x: 0.5,
                        zero_y: 0.0,
                        full_x: 0.0,
                        full_y: 0.0,
                    },
                    amount: 1.0,
                    dehaze: 100.0,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let changed = (0..3).any(|c| out[i][c].to_bits() != base[i][c].to_bits());
                if x >= w / 2 {
                    assert!(
                        !changed,
                        "uncovered column {x} moved: {:?} → {:?}",
                        base[i], out[i]
                    );
                } else {
                    assert!(changed, "covered column {x} did not move: {:?}", base[i]);
                }
            }
        }
        // Positive dehaze deepens tone: the fully-covered edge must darken.
        assert!(out[0][0] < base[0][0] - 0.05, "dehazed pixel must darken: {:?}", out[0]);
    }

    #[test]
    fn engine_active_counts_local_clarity_dehaze_texture() {
        // The activity rule feeds the GUI's ● marker AND the mask-raster budget
        // loader. Before R22 a clarity-only mask read "parked" and its bitmap
        // was never loaded — so even after the engine learned to render it, the
        // raster would have been missing. All three must count.
        assert!(!engine_active(&LocalAdjustment::default()), "a bare mask is inert");
        for (name, m) in [
            ("clarity", LocalAdjustment { clarity: 50.0, ..Default::default() }),
            ("dehaze", LocalAdjustment { dehaze: -30.0, ..Default::default() }),
            ("texture", LocalAdjustment { texture: 15.0, ..Default::default() }),
        ] {
            assert!(engine_active(&m), "local {name} alone must count as active");
        }
    }

    /// Deterministic mid-tone frame with fine and coarse structure — enough
    /// detail for an unsharp mask to bite, no values near 0 or 1 where the
    /// midtone weight or the clamps would mask a real difference.
    fn detail_frame() -> (Vec<[f32; 3]>, usize, usize) {
        let (w, h) = (64usize, 48usize);
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let coarse = if (x / 8 + y / 8) % 2 == 0 { 0.08 } else { -0.08 };
                let fine = if (x + y) % 2 == 0 { 0.03 } else { -0.03 };
                let base = 0.45 + coarse + fine;
                data.push([base, base * 0.95 + 0.02, base * 0.9 + 0.04]);
            }
        }
        (data, w, h)
    }

    #[test]
    fn hsl_adjusts_only_the_targeted_colour_band() {
        use crate::recipe::Hsl;
        // Red-band saturation -100 desaturates a red pixel toward grey but leaves
        // a blue pixel (a different band) untouched.
        let mut hsl = Hsl::default();
        hsl.saturation[0] = -100.0; // red band
        let mut data = vec![[0.8_f32, 0.1, 0.1], [0.1, 0.1, 0.8]];
        apply_hsl(&mut data, &hsl);
        let red = data[0];
        assert!(
            (red[0] - red[1]).abs() < 0.05 && (red[1] - red[2]).abs() < 0.05,
            "red pixel desaturated toward grey: {red:?}"
        );
        let blue = data[1];
        // ALL three channels — hsl_to_rgb derives them from three different
        // hue offsets, so a green-only defect is a real failure class, and
        // green is exactly the channel a blue→cyan cast moves first (U14).
        for (c, want) in [0.1f32, 0.1, 0.8].iter().enumerate() {
            assert!(
                (blue[c] - want).abs() < 0.02,
                "blue pixel untouched on every channel: {blue:?}"
            );
        }
        // EVERY band that is not red or one of its feathered neighbours
        // (orange, magenta) is a control: the single blue probe above let a
        // routing leak into yellow/green/aqua/purple pass (R12). Probe
        // pixels are generated by the engine's own hue converter at each
        // band's centre hue, saturated and mid-bright.
        for (name, hue) in
            [("yellow", 60.0f32), ("green", 120.0), ("aqua", 180.0), ("purple", 280.0)]
        {
            let (r0, g0, b0) = hsl_to_rgb(hue / 360.0, 0.7, 0.45);
            let mut probe = vec![[r0, g0, b0]];
            apply_hsl(&mut probe, &hsl);
            for c in 0..3 {
                assert!(
                    (probe[0][c] - [r0, g0, b0][c]).abs() < 0.02,
                    "red-band sat must not reach the {name} band: {:?} vs {:?}",
                    probe[0],
                    (r0, g0, b0)
                );
            }
        }
    }

    #[test]
    fn hsl_neutral_is_identity_and_grey_is_untouched() {
        use crate::recipe::Hsl;
        // A neutral HSL is an exact no-op.
        let mut data = vec![[0.6_f32, 0.2, 0.2], [0.5, 0.5, 0.5]];
        let orig = data.clone();
        apply_hsl(&mut data, &Hsl::default());
        assert_eq!(data, orig);
        // A grey pixel has no hue, so even a strong all-band push leaves it alone.
        let hsl = Hsl { saturation: [100.0; 8], ..Hsl::default() };
        let mut grey = vec![[0.5_f32, 0.5, 0.5]];
        apply_hsl(&mut grey, &hsl);
        assert!(
            (grey[0][0] - 0.5).abs() < 1e-4
                && (grey[0][1] - 0.5).abs() < 1e-4
                && (grey[0][2] - 0.5).abs() < 1e-4,
            "grey untouched: {:?}",
            grey[0]
        );
        // A LUMINANCE push probes the chroma gate itself: the saturation
        // case is nearly vacuous for grey (s = 0 short-circuits both hue
        // converters), while a grey that slipped the gate would be SCALED
        // by the luminance term on all three channels (U14).
        let hsl_lum = Hsl { luminance: [100.0; 8], ..Hsl::default() };
        let mut grey2 = vec![[0.5_f32, 0.5, 0.5]];
        apply_hsl(&mut grey2, &hsl_lum);
        assert_eq!(grey2[0], [0.5, 0.5, 0.5], "grey must not respond to band luminance");
    }

    #[test]
    fn hsl_does_not_blotch_a_near_grey_sky() {
        use crate::recipe::Hsl;
        // A near-grey overcast "sky": alternating pixels lean faintly blue vs
        // faintly aqua (s ≈ 3%), the way real demosaiced sky noise does. With
        // OPPOSITE luminance on the blue and aqua bands, the un-weighted code
        // would slam adjacent pixels to wildly different luma (a checkerboard
        // blotch). The saturation fade must keep the patch smooth.
        let mut data: Vec<[f32; 3]> = (0..64)
            .map(|i| if i % 2 == 0 { [0.71, 0.715, 0.726] } else { [0.71, 0.726, 0.722] })
            .collect();
        let hsl = Hsl { luminance: [0.0, 0.0, 0.0, 0.0, 60.0, -80.0, 0.0, 0.0], ..Hsl::default() };
        apply_hsl(&mut data, &hsl);
        let lumas: Vec<f32> = data.iter().map(luma601).collect();
        let spread = lumas.iter().cloned().fold(f32::MIN, f32::max)
            - lumas.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread < 0.04, "near-grey sky must not blotch — luma spread {spread}");
    }

    #[test]
    fn color_grade_tints_the_targeted_tonal_region() {
        use crate::recipe::ColorGrade;
        // A blue shadow wheel pushes a DARK pixel toward blue; neutral is a no-op.
        let cg = ColorGrade { shadow_hue: 240.0, shadow_sat: 100.0, blending: 100.0, ..Default::default() };
        let mut data = vec![[0.15_f32, 0.15, 0.15]]; // dark grey
        apply_color_grade(&mut data, &cg);
        let p = data[0];
        assert!(p[2] > p[0] && p[2] > p[1], "shadow tinted blue: {p:?}");

        let mut d2 = vec![[0.4_f32, 0.3, 0.2]];
        let orig = d2.clone();
        apply_color_grade(&mut d2, &ColorGrade::default()); // neutral
        assert_eq!(d2, orig);
    }

    #[test]
    fn rgb_curves_shape_each_channel_independently() {
        use crate::recipe::CurvePoint;
        // Each per-channel curve lifts ITS channel only, via the full
        // pipeline. Only the red curve used to be exercised — an engine that
        // ignored green_curve/blue_curve entirely stayed green (R12).
        let lift = || vec![
            CurvePoint { input: 0, output: 60 },
            CurvePoint { input: 255, output: 255 },
        ];
        let cases: [(&str, EditRecipe, usize); 3] = [
            ("red", EditRecipe { red_curve: lift(), ..Default::default() }, 0),
            ("green", EditRecipe { green_curve: lift(), ..Default::default() }, 1),
            ("blue", EditRecipe { blue_curve: lift(), ..Default::default() }, 2),
        ];
        for (name, r, ch) in cases {
            let mut data = vec![[0.0_f32, 0.0, 0.0]];
            apply_develop(&mut data, 1, 1, &r);
            let p = data[0];
            assert!(p[ch] > 0.15, "{name} channel lifted: {p:?}");
            for c in (0..3).filter(|c| *c != ch) {
                assert!(p[c] < 0.02, "{name} curve must not move channel {c}: {p:?}");
            }
        }
    }

    #[test]
    fn curve_lut_pins_missing_endpoints_but_keeps_explicit_ones() {
        use crate::recipe::CurvePoint;
        // One mid-curve click must NOT flatten the image to a constant: the
        // missing (0,0)/(1,1) endpoints are pinned, so the LUT stays a real
        // ramp through the clicked point.
        let one = curve_lut(&[CurvePoint { input: 128, output: 128 }]);
        assert!(one[0].abs() < 1e-6 && (one[255] - 1.0).abs() < 1e-6, "ends pinned");
        assert!((one[128] - 128.0 / 255.0).abs() < 1e-2, "clicked point honoured");
        assert!(one[64] > 0.1 && one[64] < 0.4, "shadows still a ramp, not clamped flat");
        assert!(one[192] > 0.6 && one[192] < 0.9, "highlights still a ramp");

        // Two inner points: everything outside them ramps to the pins instead
        // of freezing into crushed/blown bands.
        let two = curve_lut(&[
            CurvePoint { input: 64, output: 64 },
            CurvePoint { input: 192, output: 192 },
        ]);
        assert!(two[32] > 0.05 && two[32] < 0.2, "below-first ramps from (0,0)");
        assert!(two[224] > 0.8 && two[224] < 0.95, "above-last ramps to (1,1)");

        // An explicit endpoint stays authoritative — lifted blacks survive.
        let lifted = curve_lut(&[
            CurvePoint { input: 0, output: 40 },
            CurvePoint { input: 255, output: 255 },
        ]);
        assert!((lifted[0] - 40.0 / 255.0).abs() < 1e-3, "explicit (0,40) wins over the pin");
    }

    /// Manual, machine-relative regression probe for the GUI's engine hot path.
    /// Ignored in normal CI because wall-clock budgets are hardware-dependent;
    /// run release-only and compare same-machine ratios:
    /// `cargo test --release --lib preview_mask_perf_probe -- --ignored --nocapture`.
    /// The checksum prevents a future fast path from "winning" by skipping work.
    #[test]
    #[ignore]
    fn preview_mask_perf_probe() {
        use std::time::Instant;

        let (w, h) = (1280u32, 853u32);
        let base = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let fx = x as f32 / (w - 1) as f32;
            let fy = y as f32 / (h - 1) as f32;
            Rgb([
                (255.0 * (0.15 + 0.75 * fx)).round() as u8,
                (255.0 * (0.12 + 0.65 * fy)).round() as u8,
                (255.0 * (0.18 + 0.55 * (1.0 - fx * fy))).round() as u8,
            ])
        }));
        // Process-unique (the ./out fixture race — see fit_zoned's
        // fixture_mask_path): a concurrent `cargo test` deleted this mask
        // mid-run, turning the zone inert and the measurement meaningless.
        let mask_path =
            std::env::temp_dir().join(format!("autoshop-preview-perf-mask-{}.png", std::process::id()));
        image::GrayImage::from_fn(w / 4, h / 4, |x, _| {
            image::Luma([((x as f32 / (w / 4 - 1) as f32) * 255.0).round() as u8])
        })
        .save(&mask_path)
        .unwrap();
        let zone = |inverted| LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: mask_path.to_string_lossy().into_owned() },
            inverted,
            exposure_ev: if inverted { -0.35 } else { 0.45 },
            contrast: if inverted { 8.0 } else { -6.0 },
            saturation: if inverted { -4.0 } else { 9.0 },
            color_gains: Some(if inverted { [1.35, 0.88, 0.62] } else { [1.15, 0.96, 0.78] }),
            ..Default::default()
        };
        let no_colour_zone = |inverted| LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: mask_path.to_string_lossy().into_owned() },
            inverted,
            exposure_ev: if inverted { -0.35 } else { 0.45 },
            contrast: if inverted { 8.0 } else { -6.0 },
            saturation: if inverted { -4.0 } else { 9.0 },
            ..Default::default()
        };
        let recipes = [
            ("zero", EditRecipe { exposure_ev: 0.2, saturation: 8.0, ..Default::default() }),
            ("one_no_colour", EditRecipe {
                exposure_ev: 0.2,
                saturation: 8.0,
                masks: vec![no_colour_zone(false)],
                ..Default::default()
            }),
            ("one", EditRecipe {
                exposure_ev: 0.2,
                saturation: 8.0,
                masks: vec![zone(false)],
                ..Default::default()
            }),
            ("shared_pair_no_colour", EditRecipe {
                exposure_ev: 0.2,
                saturation: 8.0,
                masks: vec![no_colour_zone(false), no_colour_zone(true)],
                ..Default::default()
            }),
            ("shared_pair", EditRecipe {
                exposure_ev: 0.2,
                saturation: 8.0,
                masks: vec![zone(false), zone(true)],
                ..Default::default()
            }),
        ];
        for (name, recipe) in recipes {
            let _ = develop_preview(&base, &recipe); // warm bitmap decode cache
            let start = Instant::now();
            let mut checksum = 0u64;
            const N: usize = 5;
            for _ in 0..N {
                let out = develop_preview(&base, &recipe).to_rgb8();
                checksum = out
                    .as_raw()
                    .iter()
                    .step_by(997)
                    .fold(checksum, |acc, &v| acc.wrapping_mul(16777619) ^ v as u64);
            }
            eprintln!(
                "PERF preview/{name}: {:.2} ms/frame checksum={checksum}",
                start.elapsed().as_secs_f64() * 1000.0 / N as f64,
            );
        }
        std::fs::remove_file(&mask_path).ok();
    }

    #[test]
    fn render_to_file_clamps_extreme_finite_mask_geometry_before_pixel_work() {
        use crate::recipe::MaskGeometry;

        let dir = std::env::temp_dir().join(format!(
            "autoshop-render-clamp-{}-{}",
            std::process::id(),
            crate::store::next_tmp_seq()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.png");
        DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 4, image::Rgb([120, 120, 120])))
            .save(&src)
            .unwrap();

        let wild = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear {
                    zero_x: 1e30,
                    zero_y: 1e30,
                    full_x: -1e30,
                    full_y: -1e30,
                },
                exposure_ev: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut clamped = wild.clone();
        clamped.clamp();
        let wild_out = dir.join("wild.png");
        let clamped_out = dir.join("clamped.png");
        render_to_file(&src, &wild, &wild_out, None, None).unwrap();
        render_to_file(&src, &clamped, &clamped_out, None, None).unwrap();

        let got = image::open(&wild_out).unwrap().to_rgb16();
        let expected = image::open(&clamped_out).unwrap().to_rgb16();
        assert_eq!(got.as_raw(), expected.as_raw());
        assert!(got.as_raw().iter().all(|&channel| channel != 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PNG that carries ONLY its header (signature + IHDR + an empty IDAT),
    /// so a fixture can claim 61 MP dimensions in 45 bytes.
    ///
    /// Legitimate for the budget projection under test because that projection
    /// is header-only BY DESIGN (`raster_projected_bytes` never decodes), and
    /// necessary because the honest alternative is not free: encoding a real
    /// 9504×6336 grayscale PNG measured 3.16 s and a 60 MB allocation per
    /// fixture (probed on this machine), twice over in the scenario below.
    fn write_header_only_png(path: &std::path::Path, w: u32, h: u32) {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for &b in bytes {
                crc ^= b as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
                }
            }
            !crc
        }
        fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            let mut crc_over = kind.to_vec();
            crc_over.extend_from_slice(data);
            out.extend_from_slice(&crc32(&crc_over).to_be_bytes());
            out
        }
        // IHDR: width, height, 8-bit, colour type 0 (grayscale), deflate,
        // adaptive filtering, no interlace — the same shape a mask raster has.
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        let mut file = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        file.extend_from_slice(&chunk(b"IHDR", &ihdr));
        // The dimension reader stops at the first image-data chunk HEADER, so
        // an empty IDAT is enough to make the header complete and parsable.
        file.extend_from_slice(&chunk(b"IDAT", &[]));
        std::fs::write(path, &file).expect("fixture written");
    }

    /// R22 H1: the mask-refine precheck must ask the question the LOADER asks —
    /// the aggregate one. The refined raster is charged next to the recipe's
    /// other active rasters, so the SECOND full-resolution refine on a 61 MP
    /// photo is refused instead of publishing a raster that `render_to_file`
    /// then bails on and `develop_preview` silently drops.
    ///
    /// The arithmetic, spelled out (61 MP Sony A7R: 9504×6336 = 60,217,344 px):
    /// one raster projects 60,217,344 × 4 = 240,869,376 B, which fits the
    /// 268,435,456 B (256 MiB) budget; two project 481,738,752 B, which does
    /// not. The old single-raster judgement said yes to both.
    #[test]
    fn a_second_full_resolution_refine_is_refused_by_the_aggregate_budget() {
        use crate::recipe::MaskGeometry;

        let dir = std::env::temp_dir().join(format!(
            "autoshop-refine-budget-{}-{}",
            std::process::id(),
            crate::store::next_tmp_seq()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Two masks, both active, each with its own segmentation raster; `sky`
        // is the one that gets refined to full resolution first.
        let sky_full = dir.join("mask-sky-refined.png");
        let sky_small = dir.join("mask-sky.png");
        let ground_small = dir.join("mask-ground.png");
        write_header_only_png(&sky_full, 9504, 6336);
        for p in [&sky_small, &ground_small] {
            image::GrayImage::from_pixel(64, 48, image::Luma([255])).save(p).unwrap();
        }
        let mask = |name: &str, path: &std::path::Path| LocalAdjustment {
            name: name.into(),
            mask: MaskGeometry::Bitmap { path: path.display().to_string() },
            exposure_ev: 1.0,
            ..Default::default()
        };

        // Leg 1 — refining `sky`: its own small raster is the one being
        // REPLACED, so the sum is `ground`'s 12,288 B plus the incoming
        // 240,869,376 B. Fits.
        let pre_refine = EditRecipe {
            masks: vec![mask("sky", &sky_small), mask("ground", &ground_small)],
            ..Default::default()
        };
        assert!(
            mask_raster_write_fits_budget(
                &pre_refine,
                Some(&sky_small.display().to_string()),
                9504,
                6336
            ),
            "the FIRST full-resolution refine fits: 240,881,664 B ≤ {MASK_RASTER_BUDGET_BYTES} B"
        );

        // Leg 2 — now refining `ground` while `sky` holds its 61 MP raster:
        // 240,869,376 B already committed + 240,869,376 B incoming. Refused.
        let recipe = EditRecipe {
            masks: vec![mask("sky", &sky_full), mask("ground", &ground_small)],
            ..Default::default()
        };
        assert!(
            !mask_raster_write_fits_budget(
                &recipe,
                Some(&ground_small.display().to_string()),
                9504,
                6336
            ),
            "the SECOND full-resolution refine must be refused: 481,738,752 B > \
             {MASK_RASTER_BUDGET_BYTES} B"
        );
        // And the refusal is the AGGREGATE's, not this file's: the same raster
        // alone is still fine (which is exactly why the single-raster judgement
        // said yes).
        assert!(
            mask_raster_write_fits_budget(&EditRecipe::default(), None, 9504, 6336),
            "one 61 MP raster on its own fits — the aggregate is what refuses"
        );
        // A mask the engine will not render is not charged (the loader's own
        // filter): muting `sky` frees its raster's share.
        let mut muted = recipe.clone();
        muted.masks[0].enabled = false;
        assert!(
            mask_raster_write_fits_budget(
                &muted,
                Some(&ground_small.display().to_string()),
                9504,
                6336
            ),
            "an inactive mask's raster is never loaded, so it must not be charged"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_active_raster_set_over_its_budget_is_refused_before_pixel_work() {
        use crate::recipe::MaskGeometry;

        let dir = std::env::temp_dir().join(format!(
            "autoshop-raster-budget-{}-{}",
            std::process::id(),
            crate::store::next_tmp_seq()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.png");
        let second = dir.join("second.png");
        image::GrayImage::from_pixel(1, 1, image::Luma([255])).save(&first).unwrap();
        image::GrayImage::from_pixel(1, 1, image::Luma([255])).save(&second).unwrap();
        let recipe = EditRecipe {
            masks: vec![
                LocalAdjustment {
                    name: "first".into(),
                    mask: MaskGeometry::Bitmap { path: first.display().to_string() },
                    exposure_ev: 1.0,
                    ..Default::default()
                },
                LocalAdjustment {
                    name: "second".into(),
                    mask: MaskGeometry::Bitmap { path: second.display().to_string() },
                    exposure_ev: 1.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let Err(error) = load_mask_raster_snapshot_with_budget(&recipe, 1, true) else {
            panic!("two decoded bytes must exceed a one-byte snapshot budget");
        };
        assert!(
            error.to_string().contains("1-byte aggregate budget"),
            "{error:#}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_raster_after_snapshot_construction_does_not_change_the_render() {
        use crate::recipe::MaskGeometry;

        let dir = std::env::temp_dir().join(format!(
            "autoshop-raster-snapshot-{}-{}",
            std::process::id(),
            crate::store::next_tmp_seq()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mask = dir.join("mask.png");
        image::GrayImage::from_pixel(2, 2, image::Luma([255])).save(&mask).unwrap();
        let recipe = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: mask.display().to_string() },
                exposure_ev: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let snapshot = load_mask_raster_snapshot(&recipe).unwrap();
        let untouched = vec![[0.25, 0.25, 0.25]; 4];
        let mut before_delete = untouched.clone();
        apply_develop_with_rasters(&mut before_delete, 2, 2, &recipe, &snapshot);
        std::fs::remove_file(&mask).unwrap();
        let mut after_delete = untouched.clone();
        apply_develop_with_rasters(&mut after_delete, 2, 2, &recipe, &snapshot);
        assert_eq!(after_delete, before_delete);
        assert_ne!(after_delete, untouched, "the retained white mask must still apply");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_staged_encode_leaves_the_existing_target_intact() {
        let dir = std::env::temp_dir().join(format!(
            "autoshop-staged-failure-{}-{}",
            std::process::id(),
            crate::store::next_tmp_seq()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("preview.jpg");
        std::fs::write(&target, b"previous deliverable").unwrap();

        let result = stage_and_publish(&target, |staged| {
            std::fs::write(staged, b"partial new bytes")?;
            Err(anyhow::anyhow!("encoder failed"))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"previous deliverable");
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp."))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tiled_guided_refine_matches_the_whole_frame_result_across_every_seam() {
        let (w, h) = (53u32, 47u32);
        let tile_edge = 19usize;
        assert!(
            w as usize > 2 * tile_edge && h as usize > 2 * tile_edge,
            "the fixture must cross two tile seams in each axis"
        );

        let guide = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let base: u8 = if x + y / 3 < w / 2 { 28 } else { 210 };
            image::Rgb([
                base,
                base.saturating_add(((x * 5 + y * 3) % 17) as u8),
                base.saturating_sub(((x * 2 + y * 7) % 13) as u8),
            ])
        }));
        let small = image::GrayImage::from_fn(9, 7, |x, y| {
            image::Luma([((x * 37 + y * 53 + (x * y % 7) * 19) % 256) as u8])
        });

        let tiled = refine_mask_guided_tiled(&small, &guide, 2, 1e-2, tile_edge);
        let reference =
            refine_mask_guided_tiled(&small, &guide, 2, 1e-2, w.max(h) as usize);

        for (k, (&got, &want)) in tiled.as_raw().iter().zip(reference.as_raw().iter()).enumerate() {
            let x = k % w as usize;
            let y = k / w as usize;
            let delta = (got as i16 - want as i16).abs();
            let on_seam = (x != 0 && x.is_multiple_of(tile_edge))
                || (y != 0 && y.is_multiple_of(tile_edge));
            assert!(
                delta <= 1,
                "pixel ({x}, {y}), seam={on_seam}: tiled {got}, whole-frame {want}"
            );
        }
    }

    /// L14#7: the PRODUCTION tile geometry. The public entry hard-wires
    /// GUIDED_REFINE_TILE_EDGE and every real caller crosses it (the GUI
    /// refines at full decode resolution), yet the seam oracle above only
    /// drives the parameterised internal — from its point of view the
    /// shipped constant is dead code. The guide here exceeds the edge in
    /// ONE axis (two real seams for ~100 K pixels; growing both axes would
    /// square the cost for no added coverage), and the width is DERIVED
    /// from the constant so the test keeps crossing two seams if the edge
    /// ever changes. Same |delta| ≤ 1 tolerance: tiled and whole-frame
    /// differ in f32 summation order, and bit-equality would be flaky
    /// across targets.
    #[test]
    fn the_public_guided_refine_crosses_its_production_tile_seams_cleanly() {
        let w = (GUIDED_REFINE_TILE_EDGE * 2 + 52) as u32;
        let h = 48u32;
        // Slanted high-contrast edge sweeping through the first seam column,
        // plus per-channel dither — same generator family as the oracle test.
        let guide = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let base: u8 = if x + y * 11 < w / 2 { 28 } else { 210 };
            image::Rgb([
                base,
                base.saturating_add(((x * 5 + y * 3) % 17) as u8),
                base.saturating_sub(((x * 2 + y * 7) % 13) as u8),
            ])
        }));
        let small = image::GrayImage::from_fn(17, 5, |x, y| {
            image::Luma([((x * 37 + y * 53 + (x * y % 7) * 19) % 256) as u8])
        });
        // eps matches the sole production caller (gui/masks.rs).
        let public = refine_mask_guided(&small, &guide, 2, 1e-4);
        let whole = refine_mask_guided_tiled(&small, &guide, 2, 1e-4, w.max(h) as usize);
        for (k, (&got, &want)) in public.as_raw().iter().zip(whole.as_raw().iter()).enumerate() {
            let x = k % w as usize;
            let y = k / w as usize;
            let delta = (got as i16 - want as i16).abs();
            let on_seam = x != 0 && x.is_multiple_of(GUIDED_REFINE_TILE_EDGE);
            assert!(
                delta <= 1,
                "pixel ({x}, {y}), production-seam-column={on_seam}: public {got}, \
                 whole-frame {want}"
            );
        }
    }

    /// The public entry's OWN guards, unreachable through the internal fn
    /// the seam tests drive: a hostile eps (NaN / negative / zero) is
    /// floored to 1e-6 — not divided by (near-)zero variance and quantised
    /// to black — and a 0×0 guide returns the mask unchanged.
    #[test]
    fn the_public_guided_refine_floors_a_hostile_eps() {
        let (w, h) = (64u32, 32u32);
        let guide = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _| {
            if x < w / 2 { image::Rgb([20, 20, 20]) } else { image::Rgb([220, 220, 220]) }
        }));
        let small = image::GrayImage::from_fn(9, 5, |x, y| {
            image::Luma([((x * 41 + y * 59) % 256) as u8])
        });
        let floored = refine_mask_guided(&small, &guide, 2, 1e-6);
        assert!(
            floored.as_raw().iter().any(|&p| p > 0),
            "premise: the floored reference is not all black"
        );
        for bad in [f32::NAN, -1.0, 0.0] {
            let got = refine_mask_guided(&small, &guide, 2, bad);
            assert_eq!(
                got.as_raw(),
                floored.as_raw(),
                "eps {bad} must be floored to 1e-6, not quantise NaN to black"
            );
        }
        let empty = refine_mask_guided(&small, &DynamicImage::ImageRgb8(RgbImage::new(0, 0)), 2, 1e-4);
        assert_eq!(empty.as_raw(), small.as_raw(), "a 0x0 guide returns the mask unchanged");
    }

    /// L04-1: a file-supplied camera matrix that cannot be inverted is
    /// REFUSED with the measured determinant — never rendered into a
    /// silently-black frame — while a real, well-conditioned matrix passes.
    #[test]
    fn singular_camera_matrix_is_refused_not_rendered_black() {
        let good = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
        assert!(
            validate_calibration(
                &good,
                [1.9, 1.0, 1.6, f32::NAN],
                ExportColorSpace::Srgb,
                Path::new("x.dng"),
            )
            .is_ok(),
            "a real matrix + real WB validates"
        );
        let mut twin = good;
        twin[1] = twin[0]; // two identical rows ⇒ det == 0 after row-norm
        let e = validate_calibration(&twin, [1.0, 1.0, 1.0, f32::NAN], ExportColorSpace::Srgb, Path::new("x.dng"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("determinant"), "the refusal quotes the measurement: {e}");
        assert!(e.contains("x.dng"), "the refusal names the file: {e}");
        let mut nan = good;
        nan[2][1] = f32::NAN;
        let e = validate_calibration(&nan, [1.0, 1.0, 1.0, f32::NAN], ExportColorSpace::Srgb, Path::new("x.dng"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("non-finite"), "{e}");
    }

    /// L04-1: a matrix row summing to zero used to be SILENTLY left
    /// un-normalised by camera_to_space_matrix (the DNG white-preservation
    /// rule quietly dropped) — the validator refuses it instead.
    #[test]
    fn degenerate_matrix_row_is_disclosed_not_silently_unnormalised() {
        let mut degen = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
        degen[0] = [0.5, -1.0, 0.5]; // sums to 0
        let e = validate_calibration(&degen, [1.0, 1.0, 1.0, f32::NAN], ExportColorSpace::Srgb, Path::new("x.dng"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("degenerate row"), "{e}");
        // Codex AL-review F3: a row can be healthy RAW but near-orthogonal
        // to the white-weighted PRODUCT the render actually inverts - the
        // validator must judge that product, not the raw rows alone.
        let m = rgb_to_xyz(SRGB_PRIM, D65_XY);
        let col_sums = [
            m[0][0] + m[0][1] + m[0][2],
            m[1][0] + m[1][1] + m[1][2],
            m[2][0] + m[2][1] + m[2][2],
        ];
        let mut ortho = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
        // row = [1, t, 0] with 1*col0 + t*col1 = 0  =>  raw sum 1+t != 0.
        let t = -col_sums[0] / col_sums[1];
        ortho[0] = [1.0, t, 0.0];
        let e = validate_calibration(&ortho, [1.0, 1.0, 1.0, f32::NAN], ExportColorSpace::Srgb, Path::new("x.dng"))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("degenerate row") || e.contains("determinant"),
            "a white-orthogonal row must refuse even with a healthy raw sum: {e}"
        );
    }

    /// L04-1: rawler turns a zero AsShotNeutral component into an INFINITE
    /// wb coefficient (dng.rs builds 1/levels), which the old
    /// `wb[0].is_nan()`-only guard let straight into the pixel chain — as
    /// did a partial NaN and a negative. All refuse now; the documented
    /// "unknown" convention (wb[0] NaN ⇒ neutral) still validates.
    #[test]
    fn zero_as_shot_neutral_becomes_an_infinite_coefficient_and_is_refused() {
        let good = inv3(&rgb_to_xyz(SRGB_PRIM, D65_XY));
        for bad in [
            [f32::INFINITY, 1.0, 1.0, f32::NAN], // the DNG 1/0 case
            [1.0, f32::NAN, 1.0, f32::NAN],      // partial NaN — invisible to a [0]-only check
            [1.0, -0.5, 1.0, f32::NAN],
            [1.0, 0.0, 1.0, f32::NAN],
        ] {
            let e = validate_calibration(&good, bad, ExportColorSpace::Srgb, Path::new("x.dng"))
                .unwrap_err()
                .to_string();
            assert!(e.contains("AsShotNeutral"), "{bad:?} must refuse: {e}");
        }
        // A REAL (non-NaN) fourth coefficient is validated too (Codex F4):
        // rawler consumes it on 4-colour sensors.
        let e = validate_calibration(&good, [1.0, 1.0, 1.0, f32::INFINITY], ExportColorSpace::Srgb, Path::new("x.dng"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("fourth WB coefficient"), "{e}");
        assert!(validate_calibration(&good, [1.0, 1.0, 1.0, 1.2], ExportColorSpace::Srgb, Path::new("x.dng")).is_ok());
        assert_eq!(
            normalise_wb([f32::NAN, 9.0, 9.0, 9.0]),
            [1.0, 1.0, 1.0],
            "wb[0] NaN is rawler's documented UNKNOWN — neutral, not corrupt"
        );
        assert!(validate_calibration(&good, [f32::NAN; 4], ExportColorSpace::Srgb, Path::new("x")).is_ok());
        assert_eq!(
            normalise_wb([f32::INFINITY, 1.0, 1.0, f32::NAN]),
            [f32::INFINITY, 1.0, 1.0],
            "an infinite coefficient is NOT collapsed to unknown — it must reach the refusal"
        );
    }

    /// L04-1: WHY the refusal exists — the packer saturates silently. NaN
    /// quantises to black, inf to white, and render_to_file would return Ok.
    /// Pinned so a future refactor cannot re-open the silent path.
    #[test]
    fn nan_calibration_never_reaches_the_packer() {
        assert_eq!(to_u16(f32::NAN), 0, "NaN clamps to black");
        assert_eq!(to_u16(f32::INFINITY), 65535, "inf saturates to white");
        assert_eq!(to_u16(f32::NEG_INFINITY), 0);
    }

    /// L04-2: a CA-only profile (ca knots past 1, distortion off) used to
    /// sample red OUTSIDE the frame along the whole border — the clamping
    /// sampler smeared a radial plateau there (red[(0,mid)] == red[(1,mid)]
    /// on any ramp), because the fill scale was hard-wired to 1.0 whenever
    /// distortion was off. The composite fill zooms all channels in by the
    /// overshoot, so every source sample stays inside the frame.
    #[test]
    fn ca_only_profile_never_samples_outside_the_frame() {
        use crate::recipe::LensProfile;
        let profile = LensProfile {
            ca_r: vec![1.02; 16],
            ca_b: vec![0.98; 16],
            ca_on: true,
            ..Default::default()
        };
        let ramp = DynamicImage::ImageRgb16(ImageBuffer::from_fn(200, 100, |x, _| {
            Rgb([(x as u16) * 300; 3])
        }));
        let out = apply_lens_geometry(&ramp, &profile, 0.0).to_rgb16();
        let a = out.get_pixel(0, 50).0[0];
        let b = out.get_pixel(1, 50).0[0];
        assert_ne!(
            a, b,
            "the border red plateau is gone — no clamped out-of-frame samples"
        );
        // …and the zoom leaves no unfilled pixels: a white frame stays white.
        let white =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(201, 101, image::Rgb([255; 3])));
        let w = apply_lens_geometry(&white, &profile, 0.0).to_rgb16();
        let min = w.pixels().flat_map(|p| p.0).min().unwrap();
        assert!(min >= 65000, "unfilled pixels through the CA fill: min {min}");
    }

    /// L04-2: the fill is exactly 1.0 whenever no channel overshoots — real
    /// profiles below unity (and CA-off entirely) cost nothing and stay
    /// bit-identical: the green channel of a sub-unity-CA render equals the
    /// CA-off render byte for byte (base LUT divided by exactly 1.0; the
    /// two pixel branches use samplers with identical math).
    #[test]
    fn ca_fill_scale_is_identity_when_no_channel_overshoots() {
        use crate::recipe::LensProfile;
        let dims = (1200.0, 800.0);
        let sub = LensProfile {
            ca_r: vec![0.999; 16],
            ca_b: vec![0.998; 16],
            ca_on: true,
            ..Default::default()
        };
        assert_eq!(geometry_fill_scale(&sub, 0.0, dims), 1.0);
        assert_eq!(
            geometry_fill_scale(&LensProfile::default(), 25.0, dims),
            1.0,
            "CA off is always exactly 1.0 — the manual path never pays"
        );
        let dist: Vec<f32> =
            (0..16).map(|i| 1.0008 - 0.02 * (i as f32 / 15.0).powi(2)).collect();
        let with_ca = LensProfile {
            distortion: dist.clone(),
            distortion_on: true,
            ca_r: vec![0.999; 16],
            ca_b: vec![0.999; 16],
            ca_on: true,
            ..Default::default()
        };
        let no_ca =
            LensProfile { distortion: dist, distortion_on: true, ..Default::default() };
        let ramp = DynamicImage::ImageRgb16(ImageBuffer::from_fn(160, 90, |x, y| {
            Rgb([(x as u16) * 300, (x as u16) * 300 + (y as u16), (y as u16) * 500])
        }));
        let a = apply_lens_geometry(&ramp, &with_ca, 0.0).to_rgb16();
        let b = apply_lens_geometry(&ramp, &no_ca, 0.0).to_rgb16();
        assert!(
            a.pixels().zip(b.pixels()).all(|(p, q)| p.0[1] == q.0[1]),
            "green must be byte-identical when the fill is exactly 1"
        );
    }

    /// L04-2: the C2 coordinate contract — the GUI's normalised maps carry
    /// the SAME composite fill as the render, so masks/dropper/clone points
    /// stay on the pixels; the fill-adjusted forward/inverse pair still
    /// round-trips; and a CA-only profile no longer short-circuits the
    /// shared map to the manual path.
    #[test]
    fn ca_fill_keeps_the_gui_map_in_step_with_the_render() {
        use crate::recipe::LensProfile;
        let profile = LensProfile {
            distortion: (0..16).map(|i| 1.0008 - 0.053 * (i as f32 / 15.0).powi(2)).collect(),
            ca_r: vec![1.004; 16],
            ca_b: vec![0.997; 16],
            distortion_on: true,
            ca_on: true,
            ..Default::default()
        };
        let dims = (1200.0, 800.0);
        let fill = geometry_fill_scale(&profile, 0.0, dims);
        assert!(fill > 1.0, "premise: this profile's red channel overshoots");
        // The normalised map's radial factor at an edge point equals the
        // render's green factor (base / fill) at the same rn.
        let (nx, ny) = (0.98, 0.5);
        let (ox, _) = lens_geom_norm(nx, ny, dims, &profile, 0.0);
        let f_norm = (ox - 0.5) / (nx - 0.5);
        let (w, h) = dims;
        let rn = ((nx - 0.5) * w).abs() / (0.5 * (w * w + h * h).sqrt());
        let s_p = profile_fill_scale(&profile.distortion, dims);
        let expect = lens_geom_factor(rn, &profile.distortion, s_p, 0.0, 1.0) / fill;
        assert!(
            (f_norm - expect).abs() < 1e-4,
            "the GUI map factor {f_norm} drifted from the render's {expect}"
        );
        // Forward/inverse still round-trip through the fill-adjusted pair.
        for (px, py) in [(0.1, 0.2), (0.9, 0.85), (0.5, 0.05)] {
            let (ax, ay) = lens_geom_norm(px, py, dims, &profile, 12.0);
            let (bx, by) = lens_ungeom_norm(ax, ay, dims, &profile, 12.0);
            assert!(
                (bx - px).abs() < 2e-3 && (by - py).abs() < 2e-3,
                "roundtrip ({px},{py}) → ({bx},{by})"
            );
        }
        // Distortion OFF + overshooting CA: the shared map must move (the
        // old early-return to the manual path skipped the fill entirely).
        let ca_only = LensProfile {
            ca_r: vec![1.02; 16],
            ca_b: vec![0.98; 16],
            ca_on: true,
            ..Default::default()
        };
        let (mx, _) = lens_geom_norm(0.9, 0.5, dims, &ca_only, 0.0);
        assert!(
            (mx - 0.9).abs() > 1e-4,
            "the composite fill moves the shared map even with distortion off"
        );
        let (fx, fy) = lens_geom_norm(0.8, 0.3, dims, &ca_only, 0.0);
        let (ux, uy) = lens_ungeom_norm(fx, fy, dims, &ca_only, 0.0);
        assert!(
            (ux - 0.8).abs() < 2e-3 && (uy - 0.3).abs() < 2e-3,
            "CA-only roundtrip ({ux},{uy})"
        );
    }

    /// 阶段4: the export DEPTH is an ExportOpts setting, not an extension
    /// property — a .png/.tif carries 16 bits by default and 8 on request,
    /// and the staged publish keeps the contract for both.
    #[test]
    fn export_depth_follows_the_option_not_the_extension() {
        let dir = std::env::temp_dir().join(format!(
            "autoshop-export-depth-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let src_dir = dir.join("library");
        let out_dir = dir.join("exports");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&out_dir).unwrap();
        let src = src_dir.join("in.png");
        RgbImage::from_fn(12, 8, |x, y| image::Rgb([x as u8 * 20, y as u8 * 30, 90]))
            .save(&src)
            .unwrap();
        let recipe = EditRecipe::default();
        for (name, eight, want8) in [
            ("d16.png", false, false),
            ("d8.png", true, true),
            ("d16.tif", false, false),
            ("d8.tif", true, true),
        ] {
            let out = out_dir.join(name);
            let opts = ExportOpts { eight_bit: eight, ..Default::default() };
            render_to_file(&src, &recipe, &out, None, Some(&opts)).unwrap();
            let img = image::open(&out).unwrap();
            let got8 = matches!(img.color(), image::ColorType::Rgb8 | image::ColorType::Rgba8);
            assert_eq!(
                got8, want8,
                "{name}: eight_bit={eight} must decide the stored depth, got {:?}",
                img.color()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
