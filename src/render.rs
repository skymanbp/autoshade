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
//! [`apply_dehaze`]). NOT applied here: LOCAL-mask clarity/dehaze/texture
//! (deferred — the XMP→Lightroom path renders those, see [`apply_masks`];
//! local temperature/tint ARE engine-rendered since batch #2-B).

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use image::{DynamicImage, ImageBuffer, ImageEncoder, Rgb, RgbImage};
use rawler::decoders::RawDecodeParams;
use rawler::get_decoder;
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::rawsource::RawSource;
use rawler::Orientation;
use rayon::prelude::*;

use crate::recipe::{EditRecipe, MaskGeometry, RangeMask};

const LUT_N: usize = 4096;

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
    // Decode scope: the RawSource holds the entire RAW file in memory
    // (~60–120 MB for a 61 MP lossless ARW), and neither it nor the decoder
    // outlives the sensor read — so the file bytes drop HERE instead of
    // sitting under the whole ~720 MB-per-plane develop chain below (A7
    // buffer-lifetime queue).
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
    let inter = dev
        .develop_intermediate(&rawimage)
        .map_err(|e| anyhow!("develop: {e}"))?;
    // The wide path still needs the sensor container's calibration metadata —
    // copy it out now, so the mosaic itself can go before the float chain.
    let calibration = if wide {
        let xyz2cam = camera_matrix(&rawimage)?;
        let wb = if rawimage.wb_coeffs[0].is_nan() {
            [1.0, 1.0, 1.0]
        } else {
            [rawimage.wb_coeffs[0], rawimage.wb_coeffs[1], rawimage.wb_coeffs[2]]
        };
        Some((xyz2cam, wb))
    } else {
        None
    };
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
    apply_develop(&mut data, w, h, recipe);

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

    // --- crop (normalised [0,1] on the displayed frame) ----------------------
    if let Some(c) = &recipe.crop {
        let (iw, ih) = (dynimg.width() as f32, dynimg.height() as f32);
        let x = (c.left.clamp(0.0, 1.0) * iw).round() as u32;
        let y = (c.top.clamp(0.0, 1.0) * ih).round() as u32;
        let cw = (((c.right - c.left).clamp(0.0, 1.0)) * iw).round() as u32;
        let ch = (((c.bottom - c.top).clamp(0.0, 1.0)) * ih).round() as u32;
        if cw > 0 && ch > 0 {
            dynimg = dynimg.crop_imm(x, y, cw, ch);
        }
    }

    Ok(dynimg)
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
    apply_develop(&mut data, w, h, recipe);

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

    // Crop (normalised [0,1]) — orientation is already baked into the source.
    if let Some(c) = &recipe.crop {
        let (iw, ih) = (dynimg.width() as f32, dynimg.height() as f32);
        let x = (c.left.clamp(0.0, 1.0) * iw).round() as u32;
        let y = (c.top.clamp(0.0, 1.0) * ih).round() as u32;
        let cw = (((c.right - c.left).clamp(0.0, 1.0)) * iw).round() as u32;
        let ch = (((c.bottom - c.top).clamp(0.0, 1.0)) * ih).round() as u32;
        if cw > 0 && ch > 0 {
            dynimg = dynimg.crop_imm(x, y, cw, ch);
        }
    }
    Ok(dynimg)
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
    /// JPEG quality 1..=100 (ignored by TIFF/PNG, which stay 16-bit lossless).
    pub jpeg_quality: u8,
    /// Delivery color space — a REAL gamut transform + matching embedded
    /// profile, not a tag swap (gap batch D2).
    pub color_space: ExportColorSpace,
}

impl Default for ExportOpts {
    fn default() -> Self {
        Self { long_edge: None, sharpen: 0.0, jpeg_quality: 95, color_space: ExportColorSpace::Srgb }
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

/// 3×3 inverse by adjugate / determinant. The primaries matrices are far from
/// singular (their determinants are the gamut volumes), so plain f32 is fine.
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
        let s = row[0] + row[1] + row[2];
        if s.abs() > 1e-6 {
            for v in row {
                *v /= s;
            }
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
    let opts = export.copied().unwrap_or_default();
    // DELIVERABLE gate (every export surface funnels through here): an
    // unloadable Bitmap raster renders INERT, so this file would "succeed"
    // minus an edit the user made — and nothing would say so (A6). Refuse
    // with the mask named; the remedies (delete the mask / restore the
    // raster) are one step away in the app.
    let broken = unreadable_mask_rasters(recipe);
    if !broken.is_empty() {
        bail!(
            "mask raster(s) unreadable: {} — the export would silently drop those edits; \
             delete the mask(s) or restore the raster file(s), then export again",
            broken.join(", ")
        );
    }
    let ext = out
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // The gamut transform only runs for formats that can carry the matching
    // profile: pixels re-encoded for P3/AdobeRGB but saved UNTAGGED would
    // display wrong everywhere — sRGB is the only space safe to leave untagged.
    let taggable = matches!(ext.as_str(), "jpg" | "jpeg" | "tif" | "tiff" | "png");
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
        let src = crate::decode::load_image(src_path)?;
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
    let staged = out.with_extension(format!(
        "{ext}.tmp.{}.{}",
        std::process::id(),
        crate::store::next_tmp_seq()
    ));
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
        "jpg" | "jpeg" => {
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
            img.write_with_encoder(enc)
                .with_context(|| format!("encode tiff {}", out.display()))?;
            wr.flush().with_context(|| format!("flush {}", out.display()))?;
        }
        "png" => {
            let mut wr = create(&staged)?;
            let mut enc = image::codecs::png::PngEncoder::new(&mut wr);
            tag_icc(&mut enc, space);
            img.write_with_encoder(enc)
                .with_context(|| format!("encode png {}", out.display()))?;
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
    if let Err(e) = encoded {
        let _ = std::fs::remove_file(&staged); // never leave a partial behind
        return Err(e);
    }
    // fs::rename REPLACES the destination on every platform we support, so the
    // previous deliverable survives right up to the instant the new one lands.
    if let Err(e) = std::fs::rename(&staged, out) {
        let _ = std::fs::remove_file(&staged);
        return Err(e).with_context(|| format!("publish {}", out.display()));
    }
    Ok((w, h))
}

/// Fast "after" render for the UI: apply the recipe's WB + tonal + colour ops
/// to an already-demosaiced preview image (no full-res develop, no demosaic).
/// White balance runs through the SAME `apply_recipe_wb` stage as the exports,
/// so the Temp/Tint sliders and the WB eyedropper are live in the preview.
/// Crop is intentionally NOT applied here so sliders give immediate full-frame
/// feedback; the full-res `render_to_image` path applies crop on export.
pub fn develop_preview(preview: &DynamicImage, recipe: &EditRecipe) -> DynamicImage {
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
    // 0a) in-camera lens profile vignetting (lensmeta): the manufacturer's own
    //     falloff map for THIS shot, a radial gain in LINEAR light. Runs before
    //     the manual stage — both are multiplicative gains, so order between
    //     them is cosmetic, but profile-as-base reads as "calibration first".
    if r.lens_profile.vignette_active() {
        apply_profile_vignette(data, w, h, &r.lens_profile.vignette);
    }
    // 0) lens vignette compensation — a radial gain in LINEAR light (falloff is
    //    multiplicative on sensor irradiance), before any tonal work so the tone
    //    curve sees evenly-lit pixels. Preview and export share this stage.
    if r.lens_vignette != 0.0 {
        apply_vignette(data, w, h, r.lens_vignette, r.lens_vignette_mid);
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
        apply_masks(data, w, h, r);
    }
}

fn luma601(p: &[f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

/// Manual lens-vignette compensation: gain = 1 + k·rⁿ on the normalised
/// corner-radius, applied in linear light. `amount` -100..=100 (positive
/// brightens corners); `midpoint` 0..=100 shapes WHERE it lands via the radius
/// exponent (0.6..3.0, ACR-default 50 → 1.8): low reaches toward the centre,
/// high confines the correction to the corners. The exact LR falloff model is
/// proprietary — this is our documented approximation (XMP carries the raw
/// slider values, so Lightroom re-renders with its own model).
fn apply_vignette(data: &mut [[f32; 3]], w: usize, h: usize, amount: f32, midpoint: f32) {
    if w == 0 || h == 0 {
        return; // par_chunks_mut(0) asserts even on an empty slice (U14)
    }
    let (cx, cy) = ((w as f32 - 1.0) * 0.5, (h as f32 - 1.0) * 0.5);
    let rmax = (cx * cx + cy * cy).sqrt().max(1.0);
    let gamma = 0.6 + 2.4 * (midpoint.clamp(0.0, 100.0) / 100.0);
    let k = amount.clamp(-100.0, 100.0) / 100.0;
    // This stage used to cost 7 powf per pixel (rn^gamma + two transfer curves
    // × 3 channels) on every preview tick / export. Three LUTs replace them:
    // the radial gain over rn ∈ [0,1], and the shared transfer pair. Rows are
    // independent, so the pass is also row-parallel.
    let gain_lut: Vec<f32> = (0..LUT_N)
        .map(|i| 1.0 + k * (i as f32 / (LUT_N - 1) as f32).powf(gamma))
        .collect();
    let (dec, enc) = transfer_luts();
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let dy = y as f32 - cy;
        for (x, px) in row.iter_mut().enumerate() {
            let dx = x as f32 - cx;
            let rn = ((dx * dx + dy * dy).sqrt() / rmax).clamp(0.0, 1.0);
            let gain = sample_lut(&gain_lut, rn);
            if (gain - 1.0).abs() < 1e-6 {
                continue;
            }
            for c in px.iter_mut() {
                *c = sample_lut(enc, (sample_lut(dec, *c) * gain).clamp(0.0, 1.0));
            }
        }
    });
}

/// In-camera profile vignetting: per-knot linear-light GAINS over the
/// normalised corner radius (knot placement (i+0.5)/(n−1) — see `lensmeta`),
/// linearly interpolated. Same LUT + row-parallel skeleton as the manual
/// stage below; gains come from the camera, not a slider model.
fn apply_profile_vignette(data: &mut [[f32; 3]], w: usize, h: usize, knots: &[f32]) {
    if w == 0 || h == 0 {
        return; // par_chunks_mut(0) asserts even on an empty slice (U14)
    }
    let (cx, cy) = ((w as f32 - 1.0) * 0.5, (h as f32 - 1.0) * 0.5);
    let rmax = (cx * cx + cy * cy).sqrt().max(1.0);
    let gain_lut: Vec<f32> = (0..LUT_N)
        .map(|i| profile_knot_interp(knots, i as f32 / (LUT_N - 1) as f32))
        .collect();
    let (dec, enc) = transfer_luts();
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let dy = y as f32 - cy;
        for (x, px) in row.iter_mut().enumerate() {
            let dx = x as f32 - cx;
            let rn = ((dx * dx + dy * dy).sqrt() / rmax).clamp(0.0, 1.0);
            let gain = sample_lut(&gain_lut, rn);
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
fn apply_dehaze(data: &mut [[f32; 3]], w: usize, amount: f32) {
    let s = amount.clamp(-100.0, 100.0) / 100.0;
    if s.abs() < 1e-4 {
        return;
    }
    /// Full-slider strength: at +100 a pure-airlight pixel reaches t = T_MIN.
    const K: f32 = 0.75;
    /// Transmission floor — caps amplification at 1/T_MIN ≈ 3.3× so deep
    /// shadows darken decisively but cannot explode to noise.
    const T_MIN: f32 = 0.30;

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
    let a = (a_bin as f32 / 1023.0).clamp(0.10, 1.0);

    // Per-pixel loop: 6 powf/px replaced by the shared transfer LUTs (the
    // airlight histogram above keeps the exact powf — see transfer_luts), and
    // pixels are independent so the pass is parallel.
    let (dec, enc) = transfer_luts();
    data.par_iter_mut().for_each(|px| {
        let lin = [sample_lut(dec, px[0]), sample_lut(dec, px[1]), sample_lut(dec, px[2])];
        let out = if s > 0.0 {
            let w = (lin[0].min(lin[1]).min(lin[2]) / a).clamp(0.0, 1.0);
            let t = (1.0 - K * s * w).max(T_MIN);
            let b = a * (1.0 - t);
            [(lin[0] - b) / t, (lin[1] - b) / t, (lin[2] - b) / t]
        } else {
            let v = K * (-s);
            [
                lin[0] * (1.0 - v) + a * v,
                lin[1] * (1.0 - v) + a * v,
                lin[2] * (1.0 - v) + a * v,
            ]
        };
        for (c, o) in px.iter_mut().zip(out) {
            *c = sample_lut(enc, o.clamp(0.0, 1.0));
        }
    });
}

/// Apply each local masked adjustment: blend the masked region toward a locally
/// re-adjusted version, weighted by the gradient mask × amount. Applies local
/// white balance (temperature/tint — the same [`wb_gains`] model as the global
/// stage, see [`local_temp_to_kelvin`]), then local tone (exposure/contrast/
/// highlights/shadows/whites/blacks) + saturation — WB → tone → sat, mirroring
/// the global pipeline order — then local **noise reduction** (smooth luma
/// toward its neighbourhood, inside the mask — for "this region is noisy"
/// requests). Local clarity/dehaze/texture are deferred (the XMP→Lightroom
/// path renders those). Mask coords are normalised so this works at any
/// resolution.
fn apply_masks(data: &mut [[f32; 3]], w: usize, h: usize, r: &EditRecipe) {
    if w == 0 || h == 0 {
        return; // both passes below chunk by w; rayon asserts chunk_size != 0
    }
    for m in &r.masks {
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
        // Bitmap geometry: decode the raster ONCE per mask per develop (never
        // inside the pixel loop); both the tone and the NR pass share it.
        let bmp = load_mask_bitmap(&m.mask);
        // An unloadable raster carries NO coverage, so its weight must never
        // reach the inversion below: 0 with `inverted` would apply this
        // adjustment to the WHOLE frame at full strength. Skipping the whole
        // adjustment is the inert contract (recipe.rs `MaskGeometry::Bitmap`).
        if bmp.is_none() && matches!(m.mask, MaskGeometry::Bitmap { .. }) {
            continue;
        }
        // mask coverage × master amount at a pixel (with optional inversion).
        let weight_at = |x: usize, y: usize| -> f32 {
            let mut wgt = mask_weight(&m.mask, x as f32 / w as f32, y as f32 / h as f32, bmp.as_deref());
            if m.inverted {
                wgt = 1.0 - wgt;
            }
            wgt * amount
        };

        // An adjustment whose tone/sat/colour stages are ALL identity blends
        // each pixel with itself — skip the full-frame scan (a real cost at
        // 61 MP for an NR-only or freshly parked mask); the NR pass below
        // still runs on its own gate.
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
        MaskGeometry::Radial { top, left, bottom, right, feather, roundness: _, flipped } => {
            let cx = (left + right) / 2.0;
            let cy = (top + bottom) / 2.0;
            let rx = ((right - left) / 2.0).abs().max(1e-4);
            let ry = ((bottom - top) / 2.0).abs().max(1e-4);
            let d = (((nx - cx) / rx).powi(2) + ((ny - cy) / ry).powi(2)).sqrt();
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

/// Display names of ENABLED Bitmap masks whose raster does not decode right
/// now (missing or corrupt file). The engine contract for those is "render
/// inert" — right for a live preview (recoverable, warned once by the
/// loader) — but a DELIVERABLE rendered that way "succeeds" minus an edit
/// the user made and never says so; `render_to_file` refuses on a non-empty
/// answer. Callers must have resolved mask paths first (every render path
/// already does).
pub fn unreadable_mask_rasters(recipe: &EditRecipe) -> Vec<String> {
    recipe
        .masks
        .iter()
        .filter_map(|m| {
            let MaskGeometry::Bitmap { path } = &m.mask else { return None };
            // The ENGINE's own activity rule (apply_masks): identity
            // tone/sat + no local WB/recolour + no local NR renders nothing
            // even with a healthy raster — local clarity/dehaze/texture are
            // XMP-only. A PARKED mask (default amount 1, sliders neutral)
            // whose raster is lost drops no edit and must not block export.
            let engine_active = m.exposure_ev != 0.0
                || m.contrast != 0.0
                || m.highlights != 0.0
                || m.shadows != 0.0
                || m.whites != 0.0
                || m.blacks != 0.0
                || m.saturation != 0.0
                || m.temperature != 0.0
                || m.tint != 0.0
                || m.noise_reduction != 0.0
                || m.color_gains.is_some_and(|g| g != [1.0, 1.0, 1.0]);
            if m.amount == 0.0 || !engine_active || load_mask_bitmap(&m.mask).is_some() {
                return None;
            }
            Some(if m.name.is_empty() { path.clone() } else { m.name.clone() })
        })
        .collect()
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
    let decoded = match image::open(path) {
        Ok(img) => Some(Arc::new(img.to_luma8())),
        Err(e) => {
            eprintln!("⚠ bitmap mask '{path}' could not be loaded ({e}) — mask is inert");
            None
        }
    };
    {
        let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
        // A recipe holds a handful of masks — a rare hard reset beats
        // LRU bookkeeping on this hot path. Budgeted in BYTES as well
        // as entries: sixteen full-res 61 MP rasters would otherwise
        // pin ~1 GB for the life of the process.
        const MASK_CACHE_BUDGET_BYTES: usize = 256 * 1024 * 1024;
        let held: usize =
            map.values().filter_map(|(_, i)| i.as_ref()).map(|i| i.as_raw().len()).sum();
        let incoming = decoded.as_ref().map_or(0, |i| i.as_raw().len());
        if map.len() > 16 || held + incoming > MASK_CACHE_BUDGET_BYTES {
            map.clear();
        }
        map.insert(path.clone(), (ident, decoded.clone()));
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
    let bmp = load_mask_bitmap(&m.mask);
    // Same load-failure contract as `apply_masks` (inert, inversion included),
    // so the overlay never advertises coverage the render will not apply.
    if bmp.is_none() && matches!(m.mask, MaskGeometry::Bitmap { .. }) {
        return image::GrayImage::new(w, h);
    }
    let amount = m.amount.clamp(0.0, 1.0);
    let mut out = image::GrayImage::new(w, h);
    for (x, y, px) in out.enumerate_pixels_mut() {
        // Same normalisation as apply_masks' weight_at (x/w, not x/(w-1)).
        let mut wgt = mask_weight(&m.mask, x as f32 / w as f32, y as f32 / h as f32, bmp.as_deref());
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
    let luma: Vec<f32> = data.par_iter().map(luma601).collect();
    let blurred = blur_plane(&luma, w, h, radius);
    data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let l = luma[i];
        let detail = l - blurred[i];
        let m = if midtone { 1.0 - (2.0 * l - 1.0).powi(2) } else { 1.0 };
        let new_l = (l + amount * detail * m).clamp(0.0, 1.0);
        scale_chroma(px, l, new_l);
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
/// R²>0.987]. Valid 1000–40000 K.
fn kelvin_to_rgb(k: f32) -> [f32; 3] {
    let t = k.clamp(1000.0, 40000.0) / 100.0;
    let red = if t <= 66.0 {
        255.0
    } else {
        (329.698_73 * (t - 60.0).powf(-0.133_204_76)).clamp(0.0, 255.0)
    };
    let green = if t <= 66.0 {
        (99.470_8 * t.ln() - 161.119_57).clamp(0.0, 255.0)
    } else {
        (288.122_16 * (t - 60.0).powf(-0.075_514_846)).clamp(0.0, 255.0)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        (138.517_73 * (t - 10.0).ln() - 305.044_8).clamp(0.0, 255.0)
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
pub(crate) fn local_temp_to_kelvin(t: f32) -> f32 {
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
    for px in data.iter_mut() {
        for c in 0..3 {
            px[c] = sample_lut(&luts[c], px[c]);
        }
    }
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

    let mut ys = [0.0f32; 8];
    for (idx, &x) in TONE_KNOTS_X.iter().enumerate() {
        let b = tone_slider_basis(x);
        ys[idx] = tone_exposure_curve(x, r.exposure_ev)
            + b[0] * contrast
            + b[1] * highlights
            + b[2] * shadows
            + b[3] * whites
            + b[4] * blacks;
    }
    // Force the knot outputs non-decreasing (a tone curve cannot invert) then clamp.
    // Fritsch–Carlson on monotone data ⇒ the whole spline is monotone, so there is
    // NO running-max pass over the sampled LUT — monotonicity is structural.
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
/// Returns EMPTY (= no base look) when near-identity or degenerate.
pub fn camera_base_knots(neutral: &DynamicImage, camera: &DynamicImage) -> Vec<[f32; 2]> {
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
        return Vec::new();
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
        return Vec::new();
    }
    knots
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
    // Borrow an already-16-bit source instead of cloning it (the export path
    // arrives here as ImageRgb16 — to_rgb16() would copy ~366 MB at 61 MP).
    let owned;
    let src = match img.as_rgb16() {
        Some(b) => b,
        None => {
            owned = img.to_rgb16();
            &owned
        }
    };
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
    // Borrow an already-16-bit source (same policy as rotate_straighten).
    let owned;
    let src = match img.as_rgb16() {
        Some(b) => b,
        None => {
            owned = img.to_rgb16();
            &owned
        }
    };
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
/// active. Green map only — an overlay needs no chromatic refinement.
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
    if !dist_on && amount.abs() < 1e-3 {
        return src.clone();
    }
    let (w, h) = (src.width() as f32, src.height() as f32);
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
            let f = lens_geom_factor(rn, dist_knots, s_p, k, s);
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
/// the COMPOSED geometry (profile distortion + manual amount — the green map;
/// CA is a render-only chromatic refinement the GUI never needs). Falls back
/// to [`distort_norm`]'s exact math when the profile is inactive.
pub fn lens_geom_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    amount: f32,
) -> (f32, f32) {
    let dist_on = profile.distortion_on && !profile.distortion.is_empty();
    if !dist_on {
        return distort_norm(nx, ny, dims, amount);
    }
    let (w, h) = dims;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let s_p = profile_fill_scale(&profile.distortion, dims);
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let (dx, dy) = ((nx - 0.5) * w, (ny - 0.5) * h);
    let rn = (dx * dx + dy * dy).sqrt() / rr;
    let f = lens_geom_factor(rn, &profile.distortion, s_p, k, s);
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
    if !dist_on {
        return undistort_norm(nx, ny, dims, amount);
    }
    let (w, h) = dims;
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let s_p = profile_fill_scale(&profile.distortion, dims);
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
    let fwd = |rn: f32| rn * lens_geom_factor(rn, &profile.distortion, s_p, k, s);
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
    let owned;
    let src = match img.as_rgb16() {
        Some(b) => b,
        None => {
            owned = img.to_rgb16();
            &owned
        }
    };
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (k, s) = if amount.abs() < 1e-3 { (0.0, 1.0) } else { distort_params(amount) };
    let dist_knots: &[f32] = if dist_on { &profile.distortion } else { &[] };
    let s_p = if dist_on { profile_fill_scale(&profile.distortion, (w, h)) } else { 1.0 };
    // Per-channel radial factor LUTs over rn ∈ [0,1]: one lookup per channel
    // per pixel instead of spline walks. CA multiplies the green map.
    let luts: [Vec<f32>; 3] = {
        let base: Vec<f32> = (0..LUT_N)
            .map(|i| lens_geom_factor(i as f32 / (LUT_N - 1) as f32, dist_knots, s_p, k, s))
            .collect();
        let chan = |knots: &[f32]| -> Vec<f32> {
            if !ca_on || knots.is_empty() {
                return base.clone();
            }
            (0..LUT_N)
                .map(|i| {
                    let rn = i as f32 / (LUT_N - 1) as f32;
                    base[i] * profile_knot_interp(knots, rn)
                })
                .collect()
        };
        [chan(&profile.ca_r), base.clone(), chan(&profile.ca_b)]
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
        let knots = camera_base_knots(&n, &c);
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
        assert!(camera_base_knots(&n, &n).is_empty(), "identity map → empty (no base look)");
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
        let knots =
            camera_base_knots(&DynamicImage::ImageRgb8(n), &DynamicImage::ImageRgb8(c));
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
        let knots =
            camera_base_knots(&DynamicImage::ImageRgb8(n), &DynamicImage::ImageRgb8(c));
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
        apply_vignette(&mut up, w, h, 60.0, 50.0);
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
        apply_vignette(&mut down, w, h, -60.0, 50.0);
        assert!(down[0][0] < 0.38, "negative amount darkens the corner: {}", down[0][0]);

        // Higher midpoint confines the effect to the corners: the halfway
        // pixel moves LESS than with the default midpoint.
        let mut tight = flat.clone();
        apply_vignette(&mut tight, w, h, 60.0, 100.0);
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
        let (w, h) = (64usize, 1usize);
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
        let _ = h;
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
}
