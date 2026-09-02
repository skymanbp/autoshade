//! Render engine v1 — apply an [`EditRecipe`] to the full-resolution RAW and
//! produce a developed image (no Lightroom needed).
//!
//! Pipeline: `rawler` demosaics + colour-calibrates the sensor data to a
//! full-res sRGB-gamma float image (`RawDevelop::develop_intermediate`), then we
//! apply the recipe. A non-2×2 RGB colour filter array (X-Trans) is the one
//! exception: rawler's demosaic is Bayer-only, so this module demosaics and
//! calibrates that class itself — see [`demosaic_over_cfa_geometry`]. The tonal
//! ops (exposure, contrast, whites/blacks, highlights/shadows, tone curve) are
//! all 1-D functions of a channel value, so they collapse into a single
//! per-channel lookup table; saturation/vibrance run per pixel; then
//! orientation + crop.
//!
//! HONEST SCOPE: these ops are tasteful **approximations**, not bit-exact
//! Lightroom — clarity/sharpening are luma unsharp masks, noise reduction is a
//! bilateral-lite, dehaze is a pointwise scattering inversion (see
//! [`apply_dehaze`]). LOCAL-mask clarity/dehaze/texture ARE engine-rendered
//! since R22 (local temperature/tint since batch #2-B) — see [`apply_masks`]
//! for the pass order and the two documented residues vs the global chain.
//! `texture` gained its GLOBAL stage in R25 B2 and the two share one radius
//! model (0.5% of the short edge, floored at 2 px) — still our own
//! calibration, Adobe's being unpublished, but now one calibration instead of
//! a local-only one with nothing to align against.

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
pub(crate) const MASK_RASTER_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// The linear-gradient profile selected by the Lightroom falloff measurement.
///
/// `Eased` is shipped after the Lightroom probe measured C1-softened handles.
const LINEAR_FALLOFF: LinearFalloff = LinearFalloff::Eased;

#[allow(dead_code)] // Clamped remains pinned by the historical-ramp test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinearFalloff {
    /// The historical piecewise-linear ramp.
    Clamped,
    /// C1 Hermite smoothstep, with zero slope at both handles.
    Eased,
}

/// Reshape the existing handle-axis parameter without changing handle
/// transport, coordinate frames, or geometry metrics.
fn linear_coverage(t: f32, profile: LinearFalloff) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match profile {
        LinearFalloff::Clamped => t,
        LinearFalloff::Eased => t * t * (3.0 - 2.0 * t),
    }
}

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
///
/// **The un-injected door.** This wrapper sends its disclosures (the clamp
/// summary, the mask-raster loader's refusals) to [`crate::diag::stderr`],
/// attributed to `raw_path` — exactly what they did before [`crate::diag`]
/// existed. It stays that way because the surfaces on it (the decoder's
/// self-checks, `generative`, the GUI's full-res fetch) have nowhere else to
/// put a line and no ordering to defend. A caller that DOES want the lines
/// routed calls [`render_to_image_in`], which takes the sink.
pub fn render_to_image(
    raw_path: &Path,
    recipe: &EditRecipe,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    max_edge: Option<u32>,
) -> Result<DynamicImage> {
    render_to_image_in(
        raw_path,
        recipe,
        denoise,
        max_edge,
        ExportColorSpace::Srgb,
        crate::diag::stderr(),
    )
}

/// [`render_to_image`] with a chosen WORKING space. `Srgb` is the exact
/// historical pipeline (rawler's own calibrated develop, byte-identical) for
/// every 2×2 Bayer CFA — since v0.34.0 a non-2×2 RGB CFA takes the
/// [`demosaic_over_cfa_geometry`] path in BOTH working spaces and is
/// deliberately not byte-identical to v0.33.0, which rendered it through a
/// Bayer demosaic that left two of three channels partly unwritten. A
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
///
/// `sink` is where this develop's disclosures go (R29-1). The SUBJECT is bound
/// here from `raw_path`, so a caller cannot attribute one photo's warnings to
/// another; what it CAN do is decide where they land and in what order.
pub fn render_to_image_in(
    raw_path: &Path,
    recipe: &EditRecipe,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    max_edge: Option<u32>,
    working: ExportColorSpace,
    sink: &dyn crate::diag::Sink,
) -> Result<DynamicImage> {
    let diag = crate::diag::Diag::about(sink, raw_path);
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose(&diag);
    let recipe = &*validated;
    let rasters = load_mask_raster_snapshot(recipe, &diag)?;
    // Decode scope: the RawSource holds the entire RAW file in memory
    // (~60–120 MB for a 61 MP lossless ARW), and neither it nor the decoder
    // outlives the sensor read — so the file bytes drop HERE instead of
    // sitting under the whole ~720 MB-per-plane develop chain below (A7
    // buffer-lifetime queue).
    crate::decode::guard_tiff_chain(raw_path)?;
    let (rawimage, orientation) = {
        let src = RawSource::new(raw_path)
            .with_context(|| format!("open RAW {}", raw_path.display()))?;
        let decoder =
            get_decoder(&src).map_err(|e| anyhow!("no decoder for {}: {e}", raw_path.display()))?;
        let params = RawDecodeParams { image_index: 0 };
        // Which way is up comes from the EXIF metadata, NOT `RawImage
        // .orientation` — rawler 0.7.2 hard-codes that field to `Normal` for
        // every decoder but DNG/QTK, which is why every portrait ARW rendered
        // and exported sideways. See `decode::raw_orientation_of`. Read
        // INSIDE this scope so the RawSource still lives; metadata only, no
        // second sensor read.
        let md = crate::decode::guard_parser_panic(raw_path, "raw_metadata", || {
            decoder.raw_metadata(&src, &params).map_err(|e| anyhow!("raw_metadata: {e}"))
        })?;
        // …composed with the photographer's own quarter turns (R27), so the
        // whole pipeline still sees ONE orientation and no second rotation
        // stage exists to disagree with this one.
        let orientation =
            compose_orientation(crate::decode::raw_orientation_of(&md), recipe.quarter_turns);
        // THE PER-FILE MEMORY CEILING, charged BEFORE a single sensor row is
        // decompressed (R28 Batch-4 4a; adjudication F2's deeper root). The
        // baked door has refused an over-ceiling file since L02 while this one
        // — the door every RAW's pixels come through — had no per-file limit
        // at all. `dummy = true` is the same metadata-only read
        // `decode::source_frame` takes: dimensions and levels, no
        // decompression, so the refusal costs a header parse and the admission
        // costs one too. Gating AFTER the real decode instead would already
        // have committed the ~2 B/px sensor mosaic, and gating at the top of
        // the function would have paid a SECOND `RawSource::new` (the whole
        // file mapped) for dimensions the open decoder can already answer.
        //
        // The price is measured, not asserted: `decode::frame_size` on the
        // same 61 MP ARW — which opens the file, maps it, builds a decoder AND
        // takes this read — costs 110 ms against a 3.5 s full-resolution
        // render (`jobs::tests::probe_per_photo_peak_commit`, release). That is
        // an UPPER BOUND on what this line adds, since the first three of those
        // four are already paid above.
        let probe = crate::decode::guard_parser_panic(raw_path, "raw_image(dummy)", || {
            decoder
                .raw_image(&src, &params, true)
                .map_err(|e| anyhow!("raw_image(dummy): {e}"))
        })?;
        crate::decode::refuse_raw_develop_over_ceiling_for(raw_path, &probe)?;
        drop(probe);
        // Full sensor data (dummy = false) → demosaic + colour pipeline → float.
        let mut raw = crate::decode::guard_parser_panic(raw_path, "raw_image", || {
            decoder.raw_image(&src, &params, false).map_err(|e| anyhow!("raw_image: {e}"))
        })?;
        // A9: the sensor kinds render v1 cannot deliver are refused HERE, off
        // the decoded RawImage's own declaration, instead of after the whole
        // demosaic + colour pipeline has run and produced an `Intermediate`
        // this function then throws away. On a 100 MP achromatic back that
        // was tens of seconds and a full-frame float buffer spent to reach a
        // verdict the metadata already contained.
        refuse_unsupported_sensor(&raw, raw_path)?;
        // …and for the sensors we DO render, say when the demosaic is only an
        // approximation of this CFA (R6 — a non-2×2 RGB array, X-Trans today).
        disclose_approximate_demosaic(&raw, raw_path);
        // …developed from the frame the camera and Lightroom call the picture,
        // not from the sensor's top-left corner. v0.32.0 — see
        // `decode::align_default_crop` for the measurement (every Sony ARW
        // render sat 32 px right and 20 px down of Lightroom's) and for why
        // rawler 0.7.2 skips this crop on its own. The verdict is disclosed
        // inside (A5) — an out-of-bounds refusal used to be a silent `None`.
        crate::decode::align_default_crop(&mut raw);
        (raw, orientation)
    };

    let wide = working != ExportColorSpace::Srgb;
    // R28 — a non-2×2 RGB CFA is demosaiced HERE, not by rawler, so its
    // develop keeps only the black/white-level rescale: rawler's `Demosaic`
    // step is Bayer-only (`imgop/develop.rs:145-147` hands every `is_rgb`
    // pattern to `PPGDemosaic`), and it owns the active-area ROI and the
    // default crop along with it (`develop.rs:140-144`, `:204-224`), so taking
    // the demosaic means taking both crops too. Cloned because the frame it
    // describes outlives `rawimage`, which is dropped right after the develop.
    let geometry_cfa = non_bayer_rgb_cfa(&rawimage).cloned();
    let mut dev = RawDevelop::default();
    if geometry_cfa.is_some() {
        dev.steps = vec![ProcessingStep::Rescale];
    } else if wide {
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
    } else if rawimage.color_matrix.iter().next().is_some() {
        let xyz2cam = camera_matrix(&rawimage)?;
        // rawler's own develop targets sRGB — validate the matrix it
        // effectively inverts.
        validate_calibration(
            &xyz2cam,
            rawimage.wb_coeffs,
            ExportColorSpace::Srgb,
            raw_path,
        )?;
        // The CFA-geometry path stripped rawler's WhiteBalance/Calibrate/SRgb
        // together with its demosaic, so it performs them here; the Bayer sRGB
        // path leaves all three to rawler and stays byte-identical.
        geometry_cfa.is_some().then_some((xyz2cam, normalise_wb(rawimage.wb_coeffs)))
    } else {
        None
    };
    let inter = dev
        .develop_intermediate(&rawimage)
        .map_err(|e| anyhow!("develop: {e}"))?;
    let inter = match (&geometry_cfa, inter) {
        (Some(cfa), Intermediate::Monochrome(plane)) => {
            let roi = rawimage.active_area.unwrap_or_else(|| plane.rect());
            let rgb = demosaic_over_cfa_geometry(&plane.data, plane.dim(), cfa, roi);
            let mut out =
                rawler::pixarray::Color2D::<f32, 3>::new_with(rgb, roi.width(), roi.height());
            // rawler's `CropDefault` measures the default crop against the
            // window the demosaic actually read (`develop.rs:204-216`); the
            // master here is that ROI rather than `active_area`, which is the
            // same rectangle whenever the file declares one and the correct
            // one when it does not.
            if let Some(crop) = rawimage.crop_area.or(rawimage.active_area) {
                let crop = crop.adapt(&roi);
                if crop.d != out.dim() {
                    out = out.crop(crop);
                }
            }
            Intermediate::ThreeColor(out)
        }
        (_, other) => other,
    };
    // The demosaiced float frame owns everything the pipeline needs from here
    // on; the ~120 MB u16 sensor mosaic would otherwise survive to the end of
    // the function, under denoise/tone/pack/geometry (A7).
    drop(rawimage);
    // `refuse_unsupported_sensor` above has already answered this off the
    // metadata (A9), so these two arms are now a BACKSTOP against rawler
    // changing which `Intermediate` a given declaration produces — not the
    // primary gate. They stay: an engine that silently rendered a monochrome
    // frame as if it were RGB would be the worse failure.
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
    } else if geometry_cfa.is_some() {
        // A camera with no colour matrix at all: rawler skips its `Calibrate`
        // step there but still applies `SRgb` (`imgop/develop.rs:199-233`).
        // This path stripped both, so the working encoding is applied here or
        // the frame would publish as linear light.
        data.par_iter_mut().for_each(|px| {
            *px = [linear_to_srgb(px[0]), linear_to_srgb(px[1]), linear_to_srgb(px[2])];
        });
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
    // ONE value decides both the mask chain's frame adaptation and whether the
    // geometry stage runs below (`MaskFrame`). RADIAL keeps its pointwise
    // Lightroom/engine composition. LINEAR uses only the engine inverse when
    // geometry follows, or transports its two handles once when none follows.
    let geom = geometry_profile(recipe);
    let frame = MaskFrame::downstream(&geom, recipe.lens_distortion);
    apply_develop_with_rasters(&mut data, w, h, recipe, &rasters, frame);

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
    // `geometry_profile`, not `recipe.lens_profile`: the manual CA pair rides
    // the same per-channel knots (R25 B3), and reading the raw profile here
    // would skip it on a photo with no in-camera CA data. Hoisted above the
    // develop so the mask chain and this resample are ONE decision.
    if frame.warps() {
        dynimg = apply_lens_geometry(&dynimg, &geom, recipe.lens_distortion);
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

/// What `RawDevelop::default().develop_intermediate` WILL produce for this
/// sensor, decided from the decoded `RawImage`'s own declaration instead of by
/// running the develop and looking (A9).
///
/// The rule is rawler 0.7.2's, transcribed from `imgop/develop.rs:121-160`:
/// `cpp` picks the starting `Intermediate` (1 → Monochrome, 3 → ThreeColor,
/// 4 → FourColor, anything else → a literal `todo!()`), and then the
/// `Demosaic` step — which `RawDevelop::default()` always carries
/// (`develop.rs:84-92`) — promotes a Monochrome CFA frame to ThreeColor when
/// `cfa.is_rgb()`, to FourColor when the pattern has four unique colours, and
/// otherwise hits a second `todo!()`.
///
/// `Err` here therefore covers BOTH "render v1 has no path for these pixels"
/// and "rawler itself would panic", which is why it is checked before the
/// develop rather than after: the second class never reached the old
/// post-develop `bail!` at all — it aborted.
///
/// Since v0.34.0 the `is_rgb` arm has TWO producers, not one: a non-2×2 repeat
/// drops rawler's `Demosaic` step for [`demosaic_over_cfa_geometry`], which
/// yields a three-channel frame by the same declaration. The verdict is
/// unchanged either way, so this stays a single predicate.
fn refuse_unsupported_sensor(raw: &rawler::RawImage, path: &Path) -> Result<()> {
    use rawler::rawimage::RawPhotometricInterpretation as Photo;
    let kind = match (raw.cpp, &raw.photometric) {
        (3, _) => return Ok(()),
        (1, Photo::Cfa(c)) if c.cfa.is_rgb() => return Ok(()),
        (1, Photo::Cfa(c)) if c.cfa.unique_colors() == 4 => {
            format!("a 4-colour {} sensor", c.cfa)
        }
        (1, Photo::Cfa(c)) => format!(
            "a colour-filter array render v1 cannot demosaic ({}) — rawler has no path for it \
             either",
            c.cfa
        ),
        (1, _) => "a monochrome sensor".to_string(),
        (4, _) => "4-colour sensor data".to_string(),
        (n, _) => format!("{n} components per pixel, which no develop in this build handles"),
    };
    bail!(
        "{} comes from {kind}, and AutoShade's develop engine produces three-channel colour only. \
         Nothing was rendered. {}",
        path.display(),
        crate::decode::DNG_ONRAMP
    )
}

/// The colour filter array this build must demosaic ITSELF — an RGB pattern
/// whose repeat is anything other than 2×2. The test is geometric, not
/// nominal: `CFA::is_rgb` (`cfa.rs:193-195`) only checks that the pattern NAME
/// is spelled out of R, G and B, so X-Trans's 36-char 6×6 string satisfies it
/// as readily as `"RGGB"` does, and `CFA::new` admits 2×8 and 12×12 repeats on
/// the same terms (`cfa.rs:116-123`).
///
/// ONE predicate for the dispatch and for the disclosure, so the sentence a
/// photographer reads cannot describe a path the pixels did not take.
fn cfa_needs_geometry_demosaic(cfa: &rawler::cfa::CFA) -> bool {
    cfa.is_rgb() && (cfa.width, cfa.height) != (2, 2)
}

fn non_bayer_rgb_cfa(raw: &rawler::RawImage) -> Option<&rawler::cfa::CFA> {
    use rawler::rawimage::RawPhotometricInterpretation as Photo;
    let Photo::Cfa(c) = &raw.photometric else { return None };
    cfa_needs_geometry_demosaic(&c.cfa).then_some(&c.cfa)
}

/// Say so when the demosaic that ran is not one written for this sensor's
/// colour filter array — R27's answer to the format map's R6, which asked
/// whether rawler 0.7.2 handles Fuji X-Trans properly and could not tell
/// offline. It does not (see [`demosaic_over_cfa_geometry`] for the defect and
/// the measurement), so since v0.34.0 this build does not use rawler's
/// demosaic on such a file at all — it reconstructs the frame over the array's
/// real geometry instead.
///
/// The disclosure survives the fix because what it discloses has changed, not
/// gone away: colour, tone and framing are now correct — that clause was
/// measurably FALSE before the fix and is true only because of it — while fine
/// detail is still reconstructed by a general rule rather than by an algorithm
/// built for this array (Markesteijn for X-Trans), and is correspondingly
/// softer. Saying nothing would leave someone comparing against Fujifilm's own
/// converter with no explanation for the difference.
fn disclose_approximate_demosaic(raw: &rawler::RawImage, path: &Path) {
    let Some(cfa) = non_bayer_rgb_cfa(raw) else { return };
    eprintln!(
        "⚠ {} comes from a {}×{} non-Bayer colour filter array ({}), which this build demosaics \
         over the array's own geometry instead of with an algorithm written for this sensor \
         family. Every channel is interpolated from the photosites that actually measured it, so \
         colour, tone and framing are correct; fine detail is softer than a dedicated converter \
         would resolve",
        path.display(),
        cfa.width,
        cfa.height,
        cfa
    );
}

/// Half-width, in photosites, of the window the two missing channels are
/// interpolated from. 2 (a 5×5 window) is the smallest that pins a PLANE
/// through every one of the X-S10 tile's 108 (phase, channel) sample sets:
/// enumerated at radius 1, **56 of the 108 hold fewer than three samples** and
/// fall back to a plain mean, whose per-phase chroma error on a gradient is
/// the thing the plane fit exists to remove
/// (see [`demosaic_over_cfa_geometry`]). At radius 2 none do — the sets run
/// 4-6 samples for R and B, 13-17 for G.
const CFA_TAP_RADIUS: usize = 2;

/// Per-(phase, channel) interpolation weights for one CFA, built once per
/// render — the pattern repeats, so the tap set does too, and the per-pixel
/// cost collapses to a dot product.
struct CfaTaps {
    /// Window half-width the offsets below are BIASED by, so both index
    /// straight into [`wrap_table`]'s tables with no signed arithmetic in the
    /// inner loop. Wide enough for the whole-tile fallback, not just
    /// [`CFA_TAP_RADIUS`].
    radius: usize,
    /// `[(phase_row * cfa.width + phase_col) * 3 + channel]` → that channel's
    /// taps as `(biased_row_offset, biased_col_offset, weight)`.
    per_phase: Vec<Vec<(usize, usize, f32)>>,
}

/// Offsets of every photosite of colour `ch` within `radius` of CFA phase
/// `(pr, pc)`. `color_at` wraps its argument at 48 (`cfa.rs:165-167`) and 48
/// is a multiple of every repeat `CFA::new` accepts (2, 6, 8, 12), so biasing
/// a negative offset by 48 preserves its colour exactly.
fn cfa_samples(
    cfa: &rawler::cfa::CFA,
    pr: usize,
    pc: usize,
    ch: usize,
    radius: usize,
) -> Vec<(isize, isize)> {
    let r = radius as isize;
    let mut pts = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let row = (pr as isize + dy + 48) as usize;
            let col = (pc as isize + dx + 48) as usize;
            if cfa.color_at(row, col) == ch {
                pts.push((dy, dx));
            }
        }
    }
    pts
}

/// Weights that reproduce a locally PLANAR signal exactly: the constant-term
/// row of the least-squares pseudo-inverse of the design matrix `[1, dx, dy]`.
/// `None` when the samples cannot pin a plane (fewer than three, or collinear)
/// — the caller falls back to a plain mean, which is exact on flat colour but
/// not on a gradient.
fn plane_fit_weights(pts: &[(isize, isize)]) -> Option<Vec<(isize, isize, f32)>> {
    if pts.len() < 3 {
        return None;
    }
    let (mut sx, mut sy, mut sxx, mut sxy, mut syy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for &(dy, dx) in pts {
        let (x, y) = (dx as f32, dy as f32);
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
        syy += y * y;
    }
    let a = [[pts.len() as f32, sx, sy], [sx, sxx, sxy], [sy, sxy, syy]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    // Scale-free rank test: for a Gram matrix the product of the diagonal
    // bounds |det| from above (Hadamard), so the ratio IS the conditioning and
    // needs no absolute threshold in photosite units.
    if det.abs() <= 1e-6 * a[0][0] * a[1][1] * a[2][2] {
        return None;
    }
    let m = inv3(&a);
    Some(
        pts.iter()
            .map(|&(dy, dx)| (dy, dx, m[0][0] + m[0][1] * dx as f32 + m[0][2] * dy as f32))
            .collect(),
    )
}

fn cfa_taps(cfa: &rawler::cfa::CFA) -> CfaTaps {
    // A window of half-width max(width, height) spans strictly more than one
    // full repeat on both axes, so it contains every colour the pattern has —
    // the guarantee [`CFA_TAP_RADIUS`] cannot make for an arbitrary geometry.
    let full = cfa.width.max(cfa.height);
    let radius = CFA_TAP_RADIUS.max(full);
    let mut per_phase = Vec::with_capacity(cfa.height * cfa.width * 3);
    for pr in 0..cfa.height {
        for pc in 0..cfa.width {
            for ch in 0..3 {
                let weights = plane_fit_weights(&cfa_samples(cfa, pr, pc, ch, CFA_TAP_RADIUS))
                    .unwrap_or_else(|| {
                        let pts = cfa_samples(cfa, pr, pc, ch, full);
                        // Unreachable through `non_bayer_rgb_cfa`, whose
                        // `is_rgb` requires R, G and B all to appear in the
                        // pattern name — and a full-repeat window sees every
                        // cell of the tile. Loud rather than silent: an empty
                        // tap list would leave the channel at 0.0, which is
                        // the exact defect this function exists to remove.
                        assert!(
                            !pts.is_empty(),
                            "CFA {cfa} has no colour-{ch} photosite in a full repeat, but \
                             is_rgb() promised one"
                        );
                        let w = 1.0 / pts.len() as f32;
                        pts.into_iter().map(|(dy, dx)| (dy, dx, w)).collect()
                    });
                let bias = radius as isize;
                per_phase.push(
                    weights
                        .into_iter()
                        .map(|(dy, dx, w)| ((dy + bias) as usize, (dx + bias) as usize, w))
                        .collect(),
                );
            }
        }
    }
    CfaTaps { radius, per_phase }
}

/// Source index for every logical index in `-radius .. n + radius`.
///
/// Out-of-frame taps come back INTO the frame by whole CFA repeats, never by
/// mirroring or clamping: a mirror puts a different colour under the offset
/// and would reintroduce the very channel error this demosaic exists to
/// remove. Folding by the largest whole number of repeats that fits inside
/// the frame preserves each index's residue — i.e. its colour — exactly.
fn wrap_table(n: usize, period: usize, radius: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let p = period.max(1);
    let span = ((n / p) * p) as isize;
    (0..n + 2 * radius)
        .map(|i| {
            let v = i as isize - radius as isize;
            // A frame narrower than one repeat has no whole span to fold by;
            // colour-correct interpolation is impossible there in any case, so
            // clamp rather than loop.
            if span == 0 { v.clamp(0, n as isize - 1) as usize } else { v.rem_euclid(span) as usize }
        })
        .collect()
}

/// Demosaic a rescaled sensor plane over the CFA's REAL geometry, returning
/// the `roi`-sized camera-native linear frame. R28 Batch-1 1a.
///
/// **The defect this replaces.** rawler 0.7.2 routes every `is_rgb` pattern
/// into `PPGDemosaic`, whose chroma pass fills a green photosite's two missing
/// channels from *exactly* the neighbour to its right and the neighbour below
/// (`imgop/sensor/bayer/ppg.rs:185-203`), on the Bayer axiom that those two
/// carry the two different chroma colours. Inside X-Trans's four 2×2
/// all-green blocks per tile the axiom is false: the write lands back on the
/// GREEN channel, and no later pass revisits a green site
/// (`ppg.rs:220-252` is gated on `color_at != G`), so the chroma channel keeps
/// the `Color2D::new` zero fill (`pixarray.rs:376-378`). Measured on the zoo's
/// X-S10 RAF (`GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG`, 6252×4176 demosaic ROI,
/// binned by `(row mod 6, col mod 6)`): **8 of the 36 phases carry R = 0.0 at
/// 99.8 % of their pixels and a different 8 carry B = 0.0; green has no hole**
/// — the remaining 0.2 % is the 3-px border ring, which upstream fills with a
/// CFA-correct rule it never applies to the interior (`ppg.rs:74-110`).
/// Camera-native whole-frame means came out R 0.03434 / G 0.08810 / B 0.02051,
/// i.e. G/R = 2.57 before white balance and the 1.55 the README reported after
/// it. White balance cannot repair this: a per-channel gain and a per-channel
/// deficiency are both diagonal, so they commute — and the loss is not even a
/// scalar, it is a 6×6-periodic pattern of exact zeros.
///
/// **The rule here.** Every output pixel keeps its OWN photosite's channel
/// exactly; each missing channel is a fixed linear combination of the real
/// photosites of that colour inside a `(2·CFA_TAP_RADIUS + 1)²` window, with
/// the weights taken per (phase, channel) from [`plane_fit_weights`]. Two
/// consequences, both measured on a synthetic 60×60 mosaic of the X-S10 tile
/// (interior pixels, versus the ground truth the mosaic was sampled from):
///
///   * **flat colour — exact.** Max abs error 1.1e-16, and the spread of the
///     per-phase R/G ratio across all 36 phases is 3.9e-16. The zero holes and
///     the fixed-pattern chroma are gone, not attenuated.
///   * **linear gradient — exact.** Max abs error 2.2e-16, per-phase R/G
///     spread 3.3e-16. A plain distance-weighted mean is exact only on the
///     flat case; on the gradient its per-phase R/G spread is 7.2e-3, a 1.8 %
///     chroma modulation at the tile period — visible fixed-pattern chroma on
///     a sky, and a milder form of the defect above. That is why the weights
///     fit a plane and not a mean.
///
/// Detail stays APPROXIMATE and the render says so
/// ([`disclose_approximate_demosaic`]): chroma is reconstructed over a 5×5
/// window with no directional decision, so a hard edge smears across it
/// (measured max abs error 0.304 on a 0.5-amplitude step). It does not RING:
/// for this tile all 108 (phase, channel) tap sets came out non-negative
/// (worst tap +0.056), so each estimate is a convex combination of real
/// samples of that colour and cannot leave their range — measured overshoot
/// on the step is exactly 0.0000, and interpolated noise sits at or below a
/// distance-weighted mean's (σ 0.0117/0.0152/0.0119 versus 0.0122/0.0153/
/// 0.0122 for an input σ of 0.0200).
///
/// **Bayer files never reach this function** — [`non_bayer_rgb_cfa`] gates it,
/// and a 2×2 CFA keeps rawler's own develop untouched, byte for byte.
fn demosaic_over_cfa_geometry(
    plane: &[f32],
    dim: rawler::imgop::Dim2,
    cfa: &rawler::cfa::CFA,
    roi: rawler::imgop::Rect,
) -> Vec<[f32; 3]> {
    // The ROI moves the pattern under the frame exactly as rawler's own
    // expansion does (`imgop/sensor/bayer/mod.rs:26`), so a develop window
    // that does not start on a tile boundary — the X-S10's active area starts
    // at y = 5 — keeps its true phase.
    let cfa = cfa.shift(roi.x(), roi.y());
    let taps = cfa_taps(&cfa);
    let (rw, rh) = (roi.width(), roi.height());
    let ymap = wrap_table(rh, cfa.height, taps.radius);
    let xmap = wrap_table(rw, cfa.width, taps.radius);
    let mut out = vec![[0.0f32; 3]; rw * rh];
    out.par_chunks_exact_mut(rw).enumerate().for_each(|(row, line)| {
        let phase = (row % cfa.height) * cfa.width;
        for (col, px) in line.iter_mut().enumerate() {
            let own = cfa.color_at(row, col);
            let base = (phase + col % cfa.width) * 3;
            for (ch, out) in px.iter_mut().enumerate() {
                if ch == own {
                    // This photosite MEASURED this channel; nothing is
                    // interpolated over a real sample.
                    *out = plane[(roi.y() + row) * dim.w + roi.x() + col];
                    continue;
                }
                let mut acc = 0.0f32;
                for &(dy, dx, w) in &taps.per_phase[base + ch] {
                    acc += w * plane[(roi.y() + ymap[row + dy]) * dim.w + roi.x() + xmap[col + dx]];
                }
                *out = acc;
            }
        }
    });
    out
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
///
/// `max_edge` bounds the LONG EDGE of the working buffer, mirroring
/// [`render_to_image`]'s parameter of the same name and obeying the same two
/// rules: the shrink happens BEFORE denoise/tone/geometry (so a
/// preview-resolution caller never pays a full-size develop), and it only ever
/// goes DOWN — `thumbnail` would otherwise UPSCALE a source smaller than the
/// cap, inflating a small image instead of bounding a large one. `None` = the
/// source's own resolution, which is what the export path passes: a delivery
/// render must not be developed at preview size.
///
/// Until this parameter existed the baked arm had no cap at all while the RAW
/// arm had one — the asymmetry the format-support map filed as B2. The
/// resolution-normalised stages (masks, sharpen, geometry) are what make the
/// capped result meaningful rather than merely smaller; they are the same
/// shared functions the RAW path calls, so the two arms cap identically.
///
/// `diag` is the caller's diagnostics channel, and it carries WHOSE pixels
/// these are (R28 Batch-5 5c threaded the path for the stamp; R29-1 threads the
/// channel). A caller really holding anonymous pixels says so with
/// [`crate::diag::Subject::PixelOnly`] rather than a `None` whose meaning lived
/// in this comment; `render_to_file`'s baked arm has the path and binds it,
/// which is what puts the baked half of a parallel `batch` on the same footing
/// as the RAW half.
pub fn render_baked_to_image(
    img: &DynamicImage,
    recipe: &EditRecipe,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    max_edge: Option<u32>,
    diag: &crate::diag::Diag<'_>,
) -> Result<DynamicImage> {
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose(diag);
    let recipe = &*validated;
    let rasters = load_mask_raster_snapshot(recipe, diag)?;
    // The photographer's quarter turns FIRST — before the cap and before every
    // develop stage, exactly where `orient_f32` sits on the RAW path, and for
    // the same reason: masks / crop / straighten are defined against what the
    // user sees. Only the USER's half is applied here; the EXIF half is
    // already in these pixels (`decode::load_image` applies it at decode).
    // `Cow` so an un-rotated baked export still copies nothing.
    let turned: Cow<'_, DynamicImage> = match quarter_turn_orientation(recipe.quarter_turns) {
        Orientation::Normal | Orientation::Unknown => Cow::Borrowed(img),
        o => Cow::Owned(oriented(img.clone(), o)),
    };
    let img = turned.as_ref();
    // Downscale-only, before anything else allocates a plane. `Cow` so the
    // uncapped path (every shipped export) still borrows and copies nothing.
    let capped: Cow<'_, DynamicImage> = match max_edge {
        Some(edge) if img.width().max(img.height()) > edge => {
            Cow::Owned(img.thumbnail(edge, edge))
        }
        _ => Cow::Borrowed(img),
    };
    let img = capped.as_ref();
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
    // Same ONE-value rule as the RAW arm above (`MaskFrame`): the mask chain's
    // frame adaptation and the geometry gate below are the same decision.
    let geom = geometry_profile(recipe);
    let frame = MaskFrame::downstream(&geom, recipe.lens_distortion);
    apply_develop_with_rasters(&mut data, w, h, recipe, &rasters, frame);

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
    // the RAW path (the geometric chain is original → corrected → view), and
    // the same composed profile (manual CA included).
    if frame.warps() {
        dynimg = apply_lens_geometry(&dynimg, &geom, recipe.lens_distortion);
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

/// Radial scale one unit of `ca_r` / `ca_b` asks for (R25 B3).
///
/// ±100 therefore means ±0.2 % of the half-diagonal — about ±7 px at the
/// corner of a 6000×4000 frame, where real lateral CA is one to three. That
/// is OUR calibration and it is stated as such: Adobe never published what
/// its own ±100 means, and no sidecar in the user's library carries a
/// non-zero `crs:ChromaticAberrationR/B` to measure one from (the PV2012
/// panel replaced the pair with de-fringe + the auto switch). It is sized to
/// cover the artefact with headroom rather than to match an unknown, and it
/// stays inside the ±2 % band [`crate::recipe::LensProfile::clamp`] holds
/// profile CA knots to, so a manual value can never ask for a scale the
/// engine would refuse from a camera.
pub const MANUAL_CA_PER_UNIT: f32 = 2.0e-5;

/// THE lens profile every geometry consumer must use: the in-camera one with
/// the recipe's MANUAL CA folded into its per-channel radius knots.
///
/// Manual CA is not a second operator — a lateral CA scales a channel
/// linearly with radius, so its factor is CONSTANT in radius, which is
/// exactly what one knot means to [`profile_knot_interp`]. Folding it in here
/// rather than threading two more arguments through
/// [`apply_lens_geometry`] / [`lens_geom_norm`] / [`geometry_moves_frame`] is
/// what keeps the C2 contract intact for free: every consumer divides by the
/// SAME composite fill ([`geometry_fill_scale`]), so masks, overlays and the
/// colour dropper cannot drift against the pixels when a manual value pushes
/// a channel past 1.
///
/// BORROWED when the pair is at rest — the common case allocates nothing and
/// renders bit-identically to the pre-B3 engine.
///
/// When the profile's own CA is switched OFF, its knots do NOT participate:
/// the manual pair stands alone (a user who unticked 「Chromatic aberration」
/// asked for the camera's correction to stop, not to be scaled).
pub fn geometry_profile(r: &EditRecipe) -> Cow<'_, crate::recipe::LensProfile> {
    if r.ca_r == 0.0 && r.ca_b == 0.0 {
        return Cow::Borrowed(&r.lens_profile);
    }
    let p = &r.lens_profile;
    let profile_ca_on = p.ca_on && !p.ca_r.is_empty() && !p.ca_b.is_empty();
    let fold = |knots: &[f32], slider: f32| -> Vec<f32> {
        let f = 1.0 + slider * MANUAL_CA_PER_UNIT;
        if profile_ca_on { knots.iter().map(|k| k * f).collect() } else { vec![f] }
    };
    Cow::Owned(crate::recipe::LensProfile {
        ca_r: fold(&p.ca_r, r.ca_r),
        ca_b: fold(&p.ca_b, r.ca_b),
        ca_on: true,
        ..p.clone()
    })
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
    // A6: the last `get_decoder` caller in the crate, and the one that can
    // least afford to abort — it is consulted on OPEN, for the WB anchor, and
    // its whole contract is already "None on any trouble".
    let rawimage = crate::decode::guard_parser_panic(raw_path, "as-shot WB", || {
        decoder
            .raw_image(&src, &RawDecodeParams { image_index: 0 }, true)
            .map_err(|e| anyhow!("raw_image(dummy): {e}"))
    })
    .ok()?;
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
    let mut img = if is_raw_src {
        let working = if native_wide { space } else { ExportColorSpace::Srgb };
        render_to_image_in(src_path, recipe, denoise, None, working, sink)?
    } else {
        // baked-by-construction: the !is_raw_src arm (decided just above).
        let src = crate::decode::load_image_for_develop(src_path)?;
        // `None`: this is the DELIVERY render. `opts.long_edge` below resizes
        // the finished pixels, which is not the same thing as developing at a
        // bounded working resolution and must not be quietly swapped for it —
        // the RAW arm above passes `None` for exactly the same reason.
        render_baked_to_image(&src, recipe, denoise, None, &diag)?
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
    publish_staged(out, &staged, encoded)?;

    Ok((w, h))
}

/// Fast "after" render for the UI: apply the recipe's WB + tonal + colour ops
/// to an already-demosaiced preview image (no full-res develop, no demosaic).
/// White balance runs through the SAME `apply_recipe_wb` stage as the exports,
/// so the Temp/Tint sliders and the WB eyedropper are live in the preview.
/// Crop is intentionally NOT applied here so sliders give immediate full-frame
/// feedback; the full-res `render_to_image` path applies crop on export.
///
/// **The pure-pixel arm, typed.** This entry is handed a buffer, a width and a
/// height: there is no photograph behind them, and since R29-1 that is a state
/// in the type ([`crate::diag::Subject::PixelOnly`]) rather than a `None` whose
/// meaning lived in a comment beside the call. Its disclosures go to
/// [`crate::diag::stderr`] with no stem — which is exactly what they printed
/// before. A caller that DOES know which photograph these pixels came from, or
/// that wants the lines routed somewhere other than the console, calls
/// [`develop_preview_with`] and says so.
pub fn develop_preview(preview: &DynamicImage, recipe: &EditRecipe) -> DynamicImage {
    develop_preview_with(preview, recipe, &crate::diag::pixels())
}

/// [`develop_preview_with`] for a caller that states its own [`MaskFrame`].
///
/// The two forms above assume the caller runs the geometry stage when the
/// recipe's geometry is active, because the three surfaces that look at a
/// preview all do (`bin/gui/util.rs`'s `build_preview`, `serve.rs`'s preview
/// route, and the GUI coverage overlay). This form is for the exception, and
/// the exception is real: the GUI's range REFERENCE builds develop a recipe
/// that still carries a lens profile and then apply NO geometry, because they
/// exist to sample pixel VALUES rather than to be looked at. They pass
/// [`MaskFrame::without_downstream`] so LINEAR still receives its handle-only
/// raw-frame rule.
pub fn develop_preview_framed(
    preview: &DynamicImage,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
    frame: MaskFrame<'_>,
) -> DynamicImage {
    develop_preview_inner(preview, recipe, diag, Some(frame))
}

/// [`develop_preview`] with the caller's own diagnostics channel — the injected
/// form of the preview arm. `diag` states whose pixels these are (or that
/// nobody's are), and where the mask loader's refusals go.
pub fn develop_preview_with(
    preview: &DynamicImage,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
) -> DynamicImage {
    develop_preview_inner(preview, recipe, diag, None)
}

/// The preview develop. `frame` is `None` for the two entry points that let the
/// RECIPE answer "will geometry follow?" and `Some` for the caller that knows
/// better — see [`develop_preview_framed`].
fn develop_preview_inner(
    preview: &DynamicImage,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
    frame: Option<MaskFrame<'_>>,
) -> DynamicImage {
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose(diag);
    let recipe = &*validated;
    // Derived from the CLAMPED recipe, and from the same composed profile the
    // preview surfaces hand `apply_lens_geometry` — `geometry_profile`, not the
    // raw one, because the manual CA pair rides those knots (R25 B3).
    let geom = geometry_profile(recipe);
    let frame = frame.unwrap_or_else(|| MaskFrame::downstream(&geom, recipe.lens_distortion));
    let rgb = preview.to_rgb8();
    let (w, h) = rgb.dimensions();
    let mut data: Vec<[f32; 3]> = rgb
        .as_raw()
        .par_chunks(3)
        .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
        .collect();
    apply_recipe_wb(&mut data, recipe);
    apply_develop(&mut data, w as usize, h as usize, recipe, diag, frame);
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
///
/// `diag` is the caller's channel for the mask-raster loader's refusals — the
/// only thing in here that can say anything.
fn apply_develop(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
    frame: MaskFrame<'_>,
) {
    let rasters = best_effort_mask_raster_snapshot(r, diag);
    apply_develop_with_rasters(data, w, h, r, &rasters, frame);
}

/// [`apply_develop`] on pixels with no owner and no caller to route to — the
/// pixel-math tests, which construct a `[[f32; 3]]` by hand and have neither.
/// `Subject::PixelOnly` on the default sink: the same thing production's
/// un-injected preview arm does, said once here instead of at ~40 call sites.
#[cfg(test)]
fn apply_develop_anon(data: &mut [[f32; 3]], w: usize, h: usize, r: &EditRecipe) {
    // `AsRendered`: these fixtures construct a raw pixel buffer and inspect it
    // directly — no geometry stage runs after them, so every mask belongs at
    // its stored coordinates (`MaskFrame`).
    apply_develop(data, w, h, r, &crate::diag::pixels(), MaskFrame::AsRendered);
}

fn apply_develop_with_rasters(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    frame: MaskFrame<'_>,
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
    // 3b) texture — a small-radius detail operator with no midtone mask, so it
    //     works fine detail across the whole tonal range where clarity works
    //     midtone volume (R25 B2). Placed between clarity and saturation for
    //     ACR's Basic-panel order, and sharing the mask path's operator
    //     VERBATIM (`apply_masks`, the `m.texture` arm) — one calibration, so
    //     "Texture +30" means the same structure globally and inside a mask, at
    //     a 1280 px preview and at 61 MP.
    //     The radius model, the positive branch and the negative one all live
    //     in `texture_pass` now (R28 Batch-5 5a): the two arms used to hold two
    //     copies of the same three lines, which is how a one-sided fix to the
    //     −100 endpoint would have split the calibration in half. At weight 1
    //     the positive branch is still exactly `unsharp_luma`, so this adds no
    //     new mechanism there, and it runs and DROPS its planes before the next
    //     stage like the other two spatial passes. The NEGATIVE branch is the
    //     measured two-lowpass mix since R29 B8-2 (`texture_negative_pass`) —
    //     a rendering change for every negative value, here and in the mask.
    if r.texture != 0.0 {
        texture_pass(data, w, h, r.texture / 100.0, |_, _, _| 1.0);
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
        apply_masks(data, w, h, r, rasters, frame);
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
/// bit-for-bit against values captured before it — on the platform they were
/// captured on (see that test for why `powf` makes the last bits
/// libm-specific, and for what covers the others).
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
/// whites/blacks) + **saturation** + **hue** pass → local **clarity** → local
/// **texture** → local **sharpness** → local **noise reduction** (smooth luma
/// toward its neighbourhood, inside the mask — for "this region is noisy"
/// requests).
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
    frame: MaskFrame<'_>,
) {
    if w == 0 || h == 0 {
        return; // both passes below chunk by w; rayon asserts chunk_size != 0
    }
    // The frame adaptation, built ONCE per frame rather than per mask or per
    // pixel (see `MaskUnwarp`). `None` whenever nothing downstream moves these
    // pixels, which is what keeps a photo with no active geometry byte-identical
    // to what this function produced before R29 Batch-3.
    let unwarp = frame.unwarp((w as f32, h as f32));
    let unwarp = unwarp.as_ref();
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
    for stored_mask in &r.masks {
        // The eye toggle: a disabled mask renders nothing at any Amount —
        // the lossless mute (recipe.rs `LocalAdjustment::enabled`).
        if !stored_mask.enabled {
            continue;
        }
        // H2's only geometry rewrite happens here, once for the base plus once
        // per LINEAR component. The pixel closures below see one reconstructed
        // straight raw-frame gradient and never call the camera map.
        let framed_mask =
            frame.linear_handles_to_raw(stored_mask, (w as f32, h as f32));
        let m = framed_mask.as_ref();
        let local = EditRecipe {
            exposure_ev: m.exposure_ev,
            contrast: m.contrast,
            highlights: m.highlights,
            shadows: m.shadows,
            whites: m.whites,
            blacks: m.blacks,
            // The mask's own master point curve (R25 P6, `crs:MainCurve`)
            // rides in as this synthetic recipe's `tone_curve`: `build_tone_lut`
            // already composes that curve on top of the slider knots, so the
            // local curve costs no new pass, no new LUT and no new curve model
            // — it is the SAME builder the global master curve goes through.
            tone_curve: m.main_curve.clone(),
            ..EditRecipe::default()
        };
        let lut = build_tone_lut(&local);
        // The three per-channel local curves (`crs:{Red,Green,Blue}Curve`),
        // compiled ONCE per mask like `colour_luts` below and applied inside
        // the fused pixel loop right after the master curve — the global
        // chain's own order (`apply_develop` stage 1 then 1b). `None` when all
        // three are empty, so a curve-free mask pays nothing.
        let rgb_curve_luts = (!m.red_curve.is_empty()
            || !m.green_curve.is_empty()
            || !m.blue_curve.is_empty())
        .then(|| {
            (
                [
                    curve_lut(&m.red_curve),
                    curve_lut(&m.green_curve),
                    curve_lut(&m.blue_curve),
                ],
                [
                    !m.red_curve.is_empty(),
                    !m.green_curve.is_empty(),
                    !m.blue_curve.is_empty(),
                ],
            )
        });
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
        //
        // A BRUSH group arrives through the same slot (R29 Batch-6b). It has
        // no file, so the snapshot has nothing for it; `brush_raster` stamps
        // its dab stream instead — once per mask per develop, memoised across
        // develops, and at THIS frame's size because a dab is a circle in
        // pixels. The two sources cannot collide: `rasters.get` answers only
        // for a geometry with a raster PATH (`geometry_raster_path`), which a
        // brush never has.
        let brush_base = brush_raster(&m.mask, w as u32, h as u32);
        let brush_comps: Vec<Option<std::sync::Arc<image::GrayImage>>> =
            m.components.iter().map(|c| brush_raster(&c.geometry, w as u32, h as u32)).collect();
        let bmp = rasters.get(&m.mask).or(brush_base.as_deref());
        let comp_bmps: Vec<Option<&image::GrayImage>> = m
            .components
            .iter()
            .zip(&brush_comps)
            .map(|(c, brush)| rasters.get(&c.geometry).or(brush.as_deref()))
            .collect();
        // An unloadable raster carries NO coverage, so its weight must never
        // reach the inversion below: 0 with `inverted` would apply this
        // adjustment to the WHOLE frame at full strength. Skipping the whole
        // adjustment is the inert contract (recipe.rs `MaskGeometry::Bitmap`)
        // — and it covers COMPONENTS for the same reason: a lost Subtract
        // raster contributes 0 and silently WIDENS the effect area.
        if (bmp.is_none() && is_raster_backed(&m.mask))
            || m.components
                .iter()
                .zip(&comp_bmps)
                .any(|(c, b)| b.is_none() && is_raster_backed(&c.geometry))
        {
            continue;
        }
        // combined mask coverage × master amount at a pixel (with inversion).
        //
        // PIXEL CENTRES, `(x + 0.5)/w` — see `MASK_SAMPLE_CENTRE` for the
        // measurement, the derivation, and the render-behaviour change.
        let weight_at = |x: usize, y: usize| -> f32 {
            let (nx, ny) = (
                (x as f32 + MASK_SAMPLE_CENTRE) / w as f32,
                (y as f32 + MASK_SAMPLE_CENTRE) / h as f32,
            );
            let mut wgt = combined_mask_weight(
                m,
                nx,
                ny,
                bmp,
                &comp_bmps,
                unwarp,
                (w as f32, h as f32),
            );
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
            && m.hue == 0.0
            && colour_luts.is_none()
            // A mask whose ONLY move is a point curve reaches the pass through
            // these two terms — the same trap a clarity-only mask fell into
            // before R22 (it fell through every gate and rendered nothing).
            && m.main_curve.is_empty()
            && rgb_curve_luts.is_none();
        // ±100 → ±30°, the same scale `apply_hsl` gives the mixer's hue axis —
        // one meaning for "hue 40" wherever the user sets it. No chroma gate
        // here (the mixer needs one because its per-BAND weights are
        // ill-conditioned on near-greys; a uniform rotation has no band to pick
        // and `hsl_to_rgb` returns an achromatic pixel unchanged).
        let hue_turns = m.hue / 100.0 * (30.0 / 360.0);

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
                // Per-channel curves right after the master one, before
                // saturation — `apply_develop`'s stage 1 → 1b → 3 order, so a
                // full-coverage mask carrying a curve lands where the same
                // curve set globally would, to within one 8-bit code.
                // APPROXIMATELY, and measured (R25 P8): the two paths compose
                // the same LUTs but not the same arithmetic — this one fuses
                // the stages per pixel and always finishes through
                // `apply_sat_vibrance`, whose factor-1 identity
                // (`l + (c - l)`) is not bit-exact in the deep shadows. Worst
                // observed 1.5e-5 of a code over 2686 of 17751 channels
                // (`mask_curves_at_full_coverage_match_the_global_curves_
                // within_one_code`, which owns the tolerance). The clarity and
                // texture twins ARE bit-exact; this one is not, and the
                // sentence used to claim it was.
                if let Some((luts, active)) = &rgb_curve_luts {
                    for ch in 0..3 {
                        if active[ch] {
                            t[ch] = sample_lut(&luts[ch], t[ch]);
                        }
                    }
                }
                let mut t = apply_sat_vibrance(t[0], t[1], t[2], sat, 0.0);
                // Local hue rotation, LAST in the fused transform: it turns the
                // colour this mask's WB/tone/saturation stages produced, which
                // is the order the sliders read in (Temp shift → Saturation →
                // Hue). Blended by the same single weight as the rest.
                if hue_turns != 0.0 {
                    let (hh, ss, ll) = rgb_to_hsl(t[0], t[1], t[2]);
                    let (r2, g2, b2) = hsl_to_rgb((hh + hue_turns).rem_euclid(1.0), ss, ll);
                    t = [r2, g2, b2];
                }
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
            // Texture = a SMALL-radius detail operator with no midtone mask, so
            // it works fine detail across the whole tonal range where clarity
            // works midtone volume. The GLOBAL texture stage (R25 B2,
            // `apply_develop` stage 3b) calls the SAME function, so the two are
            // one calibration — 0.5% of the short edge floored at 2 px on the
            // positive half, and since R29 B8-2 a MEASURED two-lowpass mix on
            // the negative one (`texture_negative_pass`). Positive is ours
            // (Adobe's model is proprietary); negative is fitted to controlled
            // Lightroom ladders and carries its own residuals. Same honesty
            // stance as `manual_vignette_lut` either way: the XMP carries the
            // raw slider value, so Lightroom re-renders it with its own model.
            texture_pass(data, w, h, m.texture / 100.0, spatial_weight);
        }
        if m.sharpness != 0.0 {
            // The GLOBAL sharpening stage's own radius model (stage 5,
            // docs/V2_PLAN.md §4c: σ = clamp(0.0008·min(w,h), 0.7, 2.0)) — not
            // a third calibration. One slider value therefore means the same
            // structure globally and inside a mask, and the same at 1280 px
            // preview as at 61 MP.
            //
            // SIGNED, unlike the global stage (which is 0..150): ACR's local
            // Sharpness band runs -100..100 and the negative half is the point
            // — `unsharp_luma_weighted` with a negative amount subtracts the
            // detail plane, which softens. That is how a background is thrown
            // back without touching the subject.
            let sigma = (0.0008 * w.min(h) as f32).clamp(0.7, 2.0);
            let radius = (sigma.round() as usize).max(1);
            unsharp_luma_weighted(data, w, h, radius, m.sharpness / 100.0, false, spatial_weight);
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

/// WHERE a mask chain's output will be sampled — the value that keeps
/// parametric mask geometry and the pixel warp travelling together.
///
/// # The defect this closes (R29 Batch-3, user ruling 2026-08-20)
///
/// Lightroom stores mask geometry in different frames and, for LINEAR, uses a
/// different transport topology. Brush dabs live PRE-lens-correction. RADIAL
/// points live POST-correction. LINEAR stores two corrected-frame handles, then
/// either evaluates the reconstructed straight gradient in that corrected frame
/// or transports only those handles to the raw frame and reconstructs it there.
/// The `D` adjudication measured the RADIAL frame at pixel level on the 105 mm pair: the
/// PIXELS move +87.5 px at r ≈ 3250 (the `.lcp` model at 2.69 px rms over 30
/// NCC points, tangential rms 1.22 px) while the radial mask itself measures a
/// similarity of 0.99956 — the identity to 0.05 %, and **88.7 px away from the
/// pixel field**.
///
/// This engine evaluates every mask BEFORE its geometry stage (`apply_masks`
/// runs inside `apply_develop`, `apply_lens_geometry` after it) and then
/// resamples the whole frame. Let `T_engine` map an original pixel to the
/// corrected output and `m_lr` map Lightroom's stored parametric geometry to
/// its exported point. The sample adapter therefore has this table:
///
/// * a BRUSH is then already right — Lightroom rasterises it in that same
///   pre-correction frame, so the two agree with nothing applied. Warping a
///   dab here would apply the field twice.
/// * a RADIAL was **wrong by the whole field** — up to 186 px at 24 mm and
///   88 px at 105 mm — because Lightroom does not move it and this engine did.
/// * a LINEAR needs neither RADIAL's pointwise Lightroom inverse nor a
///   pointwise forward warp. With downstream geometry it is sampled only at
///   `lens_ungeom_norm(p)`; without downstream geometry its two handles are
///   mapped once by `lr_mask_unwarp_norm` and one straight raw-frame gradient
///   is rebuilt in the raw pixel metric.
///
/// RADIAL maps each sample point first through the inverse description of where
/// engine geometry will put it, then through the inverse Lightroom mask
/// transport. [`MaskUnwarp::at`] is that exact-once composition. LINEAR uses
/// [`MaskUnwarp::engine_at`] for the first half only, or the handle-only path
/// carried by [`MaskFrame::LinearHandlesToRaw`].
///
/// **Precision disclosure (D2 LINEAR, 2026-08-24): this is not 1 px closed.**
/// Against the three wall contours, the active/corrected arm's stored-line RMS
/// residual is 9.748/7.025/6.336 px; the inactive/raw H2 arm's absolute RMS is
/// 12.449/9.943/4.979 px. A fitted anisotropic aspect term is diagnostic only
/// and is deliberately not implemented here.
///
/// # Why this is a parameter and not derived from the recipe
///
/// Because "does the geometry stage run?" is the CALLER's fact, not the
/// recipe's. Five surfaces compose develop-then-geometry (`render_to_file`'s
/// two arms, the GUI preview, the GUI coverage overlay, the web preview) and
/// all five gate on the same expression — but the GUI's range REFERENCE builds
/// (`canvas.rs`) deliberately develop a recipe that still carries a lens
/// profile and then apply NO geometry, because they exist to sample pixel
/// values, not to be looked at. They state that with
/// [`MaskFrame::without_downstream`], which keeps RADIAL off the engine sampler
/// while still carrying LINEAR's separate handle fact.
///
/// So the invariant is stated as a value: **whoever runs the geometry stage
/// builds this from the same profile and amount it will pass to
/// [`apply_lens_geometry`], and uses [`MaskFrame::warps`] to decide whether to
/// run it at all; whoever omits it passes the profile to
/// [`MaskFrame::without_downstream`].**
#[derive(Clone, Copy, Debug)]
pub enum MaskFrame<'a> {
    /// The caller WILL resample this buffer through [`apply_lens_geometry`]
    /// with exactly this profile and manual amount.
    WarpedDownstream { profile: &'a crate::recipe::LensProfile, amount: f32 },
    /// No geometry resample follows, but a camera map is available for LINEAR's
    /// corrections-off rule. Only its two handles take this map; RADIAL and all
    /// raster geometry remain at stored coordinates.
    LinearHandlesToRaw { profile: &'a crate::recipe::LensProfile },
    /// Nothing downstream moves these pixels: the mask chain's output IS what
    /// will be looked at, so every geometry is evaluated at its stored
    /// coordinates.
    AsRendered,
}

impl<'a> MaskFrame<'a> {
    /// What a caller that runs the geometry stage passes.
    ///
    /// Answers [`MaskFrame::LinearHandlesToRaw`] when no resample follows but a
    /// camera map is available, and [`MaskFrame::AsRendered`] when neither fact
    /// exists. The SAME geometry condition gates `apply_lens_geometry`.
    pub fn downstream(profile: &'a crate::recipe::LensProfile, amount: f32) -> Self {
        if profile.geometry_active() || amount != 0.0 {
            MaskFrame::WarpedDownstream { profile, amount }
        } else {
            Self::without_downstream(profile)
        }
    }

    /// What a caller that deliberately omits the geometry stage passes. RADIAL
    /// and raster geometry stay stored; LINEAR retains its H2 handle transport
    /// whenever the profile carries a solved camera map.
    pub fn without_downstream(profile: &'a crate::recipe::LensProfile) -> Self {
        if profile.linear_handle_warp().len() >= 2 {
            MaskFrame::LinearHandlesToRaw { profile }
        } else {
            MaskFrame::AsRendered
        }
    }

    /// Will the geometry stage run? The caller's gate, so that the gate and the
    /// mask map are one decision.
    pub fn warps(self) -> bool {
        matches!(self, MaskFrame::WarpedDownstream { .. })
    }

    /// The inverse map for this frame, or `None` when nothing moves.
    fn unwarp(self, dims: (f32, f32)) -> Option<MaskUnwarp> {
        match self {
            MaskFrame::WarpedDownstream { profile, amount } => {
                MaskUnwarp::new(profile, amount, dims)
            }
            MaskFrame::LinearHandlesToRaw { .. } | MaskFrame::AsRendered => None,
        }
    }

    /// Apply LINEAR's corrections-off H2 rule once per local adjustment. The
    /// returned owned value exists only when at least one base/component LINEAR
    /// geometry was transported; every pixel then evaluates the resulting
    /// straight gradient with no camera map in its sample path.
    fn linear_handles_to_raw<'b>(
        self,
        mask: &'b crate::recipe::LocalAdjustment,
        dims: (f32, f32),
    ) -> Cow<'b, crate::recipe::LocalAdjustment> {
        let MaskFrame::LinearHandlesToRaw { profile } = self else {
            return Cow::Borrowed(mask);
        };
        let knots = profile.linear_handle_warp();
        if knots.len() < 2 {
            return Cow::Borrowed(mask);
        }
        let mut out = mask.clone();
        let mut moved = transport_linear_handles(&mut out.mask, dims, profile, knots);
        for component in &mut out.components {
            moved |= transport_linear_handles(&mut component.geometry, dims, profile, knots);
        }
        if moved { Cow::Owned(out) } else { Cow::Borrowed(mask) }
    }
}

/// Map a LINEAR component's two handles in the camera map's forward direction
/// (`D_fwd` / `lr_mask_unwarp_norm`) and nothing else. Returning `true` lets the
/// caller avoid cloning adjustments with no LINEAR geometry.
fn transport_linear_handles(
    geometry: &mut MaskGeometry,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    knots: &[f32],
) -> bool {
    let MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } = geometry else {
        return false;
    };
    (*zero_x, *zero_y) = linear_handle_unwarp_norm(*zero_x, *zero_y, dims, profile, knots);
    (*full_x, *full_y) = linear_handle_unwarp_norm(*full_x, *full_y, dims, profile, knots);
    true
}

/// RADIAL-only: ORIGINAL-frame point → the Lightroom-stored sample point
/// whose effect will occupy the right Lightroom export point after this
/// engine's geometry stage.
///
/// This is the composition `m_lr^-1(T_engine(p))`: first ask where original
/// pixel `p` will land under the exact downstream engine resample, then pull
/// that output coordinate back through Lightroom's corrected mask transport.
/// `T_engine` is built by calling [`lens_ungeom_norm`], not by reimplementing
/// it. `m_lr^-1` is built by calling [`lr_mask_unwarp_norm`]. Each appears
/// exactly once; the downstream resample supplies the corresponding one
/// `T_engine` application after mask rasterisation.
///
/// **The manual `lens_distortion` amount is covered with no residue** in the
/// first half. The second half deliberately uses the Lightroom mask map, not
/// the engine map a second time: D2's 41-vector radial fixture closes this
/// point law. LINEAR, brush, bitmap and AI never call this full adapter; the
/// explicit match in `mask_weight_in` is the type boundary.
///
/// A LUT because `lens_ungeom_norm` costs a 256-step peak scan plus 40
/// bisection steps per call, and this is a per-pixel question on frames up to
/// 61 MP. The map is radial, so [`LUT_N`] nodes over the normalised radius
/// carry it exactly as the resampler's own per-channel LUTs carry the forward
/// map, at the same node density.
struct MaskUnwarp {
    /// Factor `r_corrected / r_original` at node `i` = radius `i/(LUT_N−1)` of
    /// the half-diagonal.
    lut: Vec<f32>,
    /// Factor `r_stored / r_exported` for Lightroom's mask transport, sampled
    /// about `lr_cx,lr_cy`. `None` is Lightroom identity.
    lr_lut: Option<Vec<f32>>,
    rr: f32,
    w: f32,
    h: f32,
    lr_cx: f32,
    lr_cy: f32,
    lr_rmax: f32,
}

impl MaskUnwarp {
    fn new(profile: &crate::recipe::LensProfile, amount: f32, dims: (f32, f32)) -> Option<Self> {
        let (w, h) = dims;
        if !(w > 0.0 && h > 0.0) {
            return None;
        }
        let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
        // Sampled along the +x axis: the map is radial, so one ray carries it,
        // and going through the public entry point keeps this honest.
        let lut: Vec<f32> = (0..LUT_N)
            .map(|i| {
                let rho = i as f32 / (LUT_N - 1) as f32;
                let dx = rho * rr;
                if dx <= 1e-6 {
                    // The centre is a fixed point of every radial map; the
                    // ratio there is 0/0 and the limit is the next node's.
                    return f32::NAN;
                }
                let (ox, _) = lens_ungeom_norm(dx / w + 0.5, 0.5, dims, profile, amount);
                (ox - 0.5) * w / dx
            })
            .collect();
        let mut lut = lut;
        if lut.len() > 1 {
            lut[0] = lut[1];
        }
        let [lr_cx, lr_cy] = lr_mask_center_px(dims, profile);
        let lr_rmax = [
            lr_cx.hypot(lr_cy),
            (w - lr_cx).hypot(lr_cy),
            lr_cx.hypot(h - lr_cy),
            (w - lr_cx).hypot(h - lr_cy),
        ]
        .into_iter()
        .fold(1.0f32, f32::max)
            / rr;
        let lr_lut = if profile.mask_warp.is_empty() {
            None
        } else {
            let mut lr_lut: Vec<f32> = (0..LUT_N)
                .map(|i| {
                    let rho = lr_rmax * i as f32 / (LUT_N - 1) as f32;
                    let dx = rho * rr;
                    if dx <= 1e-6 {
                        return f32::NAN;
                    }
                    let (sx, _) = lr_mask_unwarp_norm(
                        (lr_cx + dx) / w,
                        lr_cy / h,
                        dims,
                        profile,
                    );
                    (sx * w - lr_cx) / dx
                })
                .collect();
            lr_lut[0] = lr_lut[1];
            Some(lr_lut)
        };
        // Identity map = nothing to do, and saying so here is what keeps a
        // distortion-free profile bit-identical rather than merely close.
        //
        // The threshold is in PIXELS, not in factor units, because that is the
        // question: a factor of 1 ± 1e-7 on a 9504 px frame moves a mask by
        // 6e-4 px, which is not a displacement, it is the bisection's own
        // residue (`lens_ungeom_norm` does not short-circuit for an active
        // profile whose knots are all 1.0 — it solves, and lands a few ulps
        // off). Anything a real lens produces is four orders larger: the
        // gentlest frame measured in this batch moves 0.6 % at the centre.
        //
        // MUTATION THIS KILLS: dropping this guard makes every coordinate on a
        // distortion-free profile take a float round trip through `at`, and
        // `with_the_geometry_stage_inactive_the_mask_chain_is_untouched` goes red.
        let engine_identity = lut.iter().all(|f| (f - 1.0).abs() * rr < 0.01);
        let lr_identity = lr_lut
            .as_ref()
            .is_none_or(|v| v.iter().all(|f| (f - 1.0).abs() * rr < 0.01));
        if engine_identity && lr_identity {
            return None;
        }
        Some(MaskUnwarp { lut, lr_lut, rr, w, h, lr_cx, lr_cy, lr_rmax })
    }

    /// Exact inverse of the downstream engine geometry, without Lightroom's
    /// point-transport half. LINEAR uses this arm so its stored straight line
    /// lands in the corrected output frame without acquiring RADIAL's map.
    fn engine_at(&self, nx: f32, ny: f32) -> (f32, f32) {
        let (dx, dy) = ((nx - 0.5) * self.w, (ny - 0.5) * self.h);
        let rho = ((dx * dx + dy * dy).sqrt() / self.rr).clamp(0.0, 1.0);
        let t = rho * (LUT_N - 1) as f32;
        let i = (t.floor() as usize).min(LUT_N - 2);
        let f = t - i as f32;
        let k = self.lut[i] * (1.0 - f) + self.lut[i + 1] * f;
        ((dx * k) / self.w + 0.5, (dy * k) / self.h + 0.5)
    }

    /// The point `(nx, ny)` will occupy after the geometry stage. RADIAL's
    /// settled point law remains byte-for-byte on this method; LINEAR calls the
    /// separate engine-only half above.
    fn at(&self, nx: f32, ny: f32) -> (f32, f32) {
        let (dx, dy) = ((nx - 0.5) * self.w, (ny - 0.5) * self.h);
        let rho = ((dx * dx + dy * dy).sqrt() / self.rr).clamp(0.0, 1.0);
        let t = rho * (LUT_N - 1) as f32;
        let i = (t.floor() as usize).min(LUT_N - 2);
        let f = t - i as f32;
        let k = self.lut[i] * (1.0 - f) + self.lut[i + 1] * f;
        let (nx, ny) = ((dx * k) / self.w + 0.5, (dy * k) / self.h + 0.5);
        let Some(lr_lut) = &self.lr_lut else { return (nx, ny) };
        let (dx, dy) = (nx * self.w - self.lr_cx, ny * self.h - self.lr_cy);
        let rho = ((dx * dx + dy * dy).sqrt() / self.rr).clamp(0.0, self.lr_rmax);
        let t = rho / self.lr_rmax * (LUT_N - 1) as f32;
        let i = (t.floor() as usize).min(LUT_N - 2);
        let f = t - i as f32;
        let k = lr_lut[i] * (1.0 - f) + lr_lut[i + 1] * f;
        (
            (dx * k + self.lr_cx) / self.w,
            (dy * k + self.lr_cy) / self.h,
        )
    }
}

/// Historical classification assertion for the unchanged regression below.
///
/// Production routing deliberately does NOT use this union: H2 requires RADIAL
/// point transport and LINEAR handle transport to take separate match arms.
/// The test-only helper keeps the older classification test byte-for-byte while
/// the reasons for the non-parametric types remain registered beside it:
///
/// * `Radial` / `Linear` — YES. Measured post-correction (`MaskFrame`).
/// * `Brush` — no. Measured PRE-correction, which is the frame this engine
///   already evaluates in; the geometry stage carries it correctly untouched.
/// * `Bitmap` / `AiMask` — no, and not for want of measuring. These rasters are
///   ENGINE-generated (our segmenter, the GUI's own paint), so there is no
///   Lightroom rendering for them to agree with; they are authored in the frame
///   the engine draws them in and stay there.
/// * A colour / luminance `RangeMask` is not here at all because it selects by
///   pixel VALUE, not position, and a pixel keeps its value through a resample
///   — approximately frame-invariant, so nothing to map. (Approximately: the
///   resampler interpolates, so a value on a steep edge shifts slightly. That
///   residue is sub-pixel and is registered here rather than modelled.)
#[cfg(test)]
fn is_lr_post_correction_geometry(g: &MaskGeometry) -> bool {
    matches!(g, MaskGeometry::Radial { .. } | MaskGeometry::Linear { .. })
}

/// Lightroom's radial-mask falloff α(ρ), READ OUT OF THE MEASUREMENT.
///
/// `feather` is the recipe's 0..1 fraction, `d` the normalised elliptical
/// radius (1.0 = on the ellipse). Returns coverage BEFORE `flipped` flips it.
///
/// # Why a table and not a law
///
/// Three successive closed forms were wrong on this arm, and R29 Batch-7 plus
/// its supplement Batch-7-2 (`~/.claude/plans/r29-materials/b7-analysis.md`,
/// `…/b7-analysis-2.md`) closed the question rather than proposing a fourth:
/// across all EIGHT rungs those batches measured, no two-parameter closed form
/// reaches the 0.003 measurement floor. The best is a Beta CDF in `1 − ρ/1.4335`
/// at 3.1× the floor; the free-endpoint smoothstep this engine shipped scores
/// 4.5×, `exp(−(ρ/s)^k)` 4.0×, a logistic 9.5× (B7-2 §4). The one candidate law
/// the four-rung batch had spotted, `a ≈ 1.9/f`, is refuted outright by the
/// supplement — it holds on f ∈ [25, 100] and misses by 58× at f = 1. So the
/// adjudicated landing shape is the measured α(ρ) itself.
///
/// # What it replaces, and by how much
///
/// `1 − smoothstep(1 − f, 1 + f/2, d)`. Scored on the batch's own grid (Δρ =
/// 0.005 bins of ≥400 px, ρ ≤ 1.45, green channel) that law reads rms(α)
/// 0.0093 / 0.0104 / 0.0285 / 0.0929 / 0.0974 / 0.1197 / 0.1403 / 0.1557 at
/// feather 1 / 5 / 10 / 25 / 50 / 75 / 90 / 100, and renders the α ≥ 0.5 region
/// 1.105× / 1.247× / 1.387× / 1.690× / 2.077× too large from f = 25 up. This
/// table reads 0.0009 / 0.0005 / 0.0005 / 0.0005 / 0.0004 / 0.0001 / 0.0000 /
/// 0.0000, and its α = 0.5 contour lands on the RAW measurement's to four
/// decimals in ρ on every rung, so the same area ratio is 1.000 across the
/// board — measured against the unconditioned profile, not against the table's
/// own conditioned copy. The three columns me3 inserted later (f = 15/35/65)
/// score the same way: the old law reads rms(α) 0.0567 / 0.1124 / 0.1073 there,
/// with the α ≥ 0.5 region 1.043× / 1.188× / 1.307× too large, and the table
/// carrying those columns reads 0.0000 (`a_08`).
///
/// Better on EVERY rung is the requirement, not a bonus: the old law was
/// already CORRECT for f ≤ 5 (rms 0.009–0.010, area ratio 0.995–0.997, B7-2
/// §6) and a replacement that only fixed the wide end would have broken the one
/// segment that worked.
///
/// # The table
///
/// Rows are the measurement's OWN ρ bins — centres `0.0025 + 0.005 i` — so the
/// eleven columns are reproduced exactly where they were measured rather than
/// resampled onto a rounder grid. Columns are Lightroom's feather 1 / 5 / 10 /
/// 15 / 25 / 35 / 50 / 65 / 75 / 90 / 100. Sources: `dense9.npz` from
/// `b7b_12_dense.py`, tabulated in `b7-analysis-2.md` §3.1, for eight of them
/// (f = 25/50/75/100 reproduce B7 §3.1 bit for bit; f = 1/5/10/90 are that
/// supplement's rungs), and the me3 package's own f = 15/35/65 exports for the
/// other three (`~/.claude/plans/r29-materials/me3-a-report.md` §0-Q1 and §1,
/// generator `scripts-archive/me3-a/a_09_table.py`).
///
/// The three me3 columns are INSERTED, not refitted. The eight B7-2 columns
/// come across bit for bit — max |Δ| = 0.000000 over all 2320 of their entries
/// (`a_09`) — and the insertion lands because BETWEEN columns is where the
/// table was still wrong: scored against the new exports the eight-column
/// version read rms(α) 0.0209 (max 0.0799) at f = 15 and 0.0212 (max 0.0490) at
/// f = 35, which is its α = 0.5 contour sitting 14.8 px and 24.9 px outside
/// Lightroom's on the measured frame's major axis (`a_07` §4). f = 65 was
/// already right at rms 0.0004 (0.2 px) and comes along only because the export
/// existed. Held-out over the whole ladder — drop each rung, predict it from
/// its neighbours — the mean rms goes 0.0268 → 0.0140 (`a_07` §2).
///
/// Two conditionings, both small enough to name outright:
///
/// * α is regressed non-increasing in ρ (pool-adjacent-violators, weighted by
///   bin count) and clamped to [0, 1]. Cost ≤ 0.0073 anywhere (≤ 0.0061 on the
///   eight B7-2 columns, 0.0073 / 0.0072 / 0.0060 on the inserted f = 15/35/65,
///   `a_09`), rms ≤ 0.0009 — the raw wiggle is 8-bit quantisation, 1 DN ≈ 0.004
///   in α.
/// * α is forced non-increasing in f for ρ ≤ 1 (running minimum across the
///   columns). Cost ≤ 0.000061, on 17 entries — all of them in the eight
///   columns that carried over unchanged; `a_09` reports the running-minimum
///   cost on the inserted f = 15/35 as 0.000000 and does not print it
///   separately for f = 65. OUTSIDE the ellipse the order genuinely reverses —
///   more feather reaches further — so the running minimum stops at ρ = 1, and
///   the f = 50 tail really is fatter than f = 75's and f = 100's out there
///   (B7 §3.1, independently reproduced in the raw DN profile).
///
/// `α(0) = 1` at every feather is measured, not fitted or normalised in: mask
/// centres are pixel-identical to the feather-0 frame on all eight rungs B7-2
/// measured (§3.4), and the me3 rungs land on the same near-centre rows. That is the fact that killed the free-endpoint refit, which wanted
/// `d_in = −0.228` and a 6 %-wrong centre at f = 100. Rows inside ρ = 0.0425
/// hold that 1 outright — those bins carry too few pixels to measure and the
/// disc they cover is 0.18 % of the ellipse.
///
/// # Feather 0 is ANALYTIC, not measured
///
/// The measured f = 0 column has a transition 0.0084 wide in ρ, but that is the
/// JPEG-plus-capture-sharpening blur floor (8.7 px on this frame's major axis),
/// not Lightroom's edge — at Feather 0 Lightroom draws a hard edge. So f = 0 is
/// a hard step here, exactly, with `d == 1.0` counting as OUTSIDE: the
/// behaviour the old degenerate-`ramp` guard produced and the one
/// `radial_feather_zero_stays_finite_on_the_boundary` pins.
///
/// # Interpolation
///
/// Linear in ρ between rows, linear in f between columns. Exact on all eleven
/// columns, and a convex combination throughout — so α stays inside [0, 1] and
/// stays monotone on both axes by construction, not by assertion.
///
/// Linear in f is the abscissa the DATA picks, not a default. The measured
/// transition width `W(f) = ρ(α=.05) − ρ(α=.95)` (B7-2 §3.2) spreads only 1.97×
/// as `W/f` across the whole ladder, against 7.5× as `W/√f` and 6.0× as
/// `W/log f`. A held-out check — drop a column, predict it from its neighbours
/// — puts linear-in-f at mean rms 0.027 against 0.018 for the best curved rival
/// (PCHIP in log f), the sign of the difference flipping rung to rung, and
/// log f is undefined at the f = 0 end this function has to reach anyway. A
/// 1.5× edge with mixed sign does not buy curvature that nothing measured.
///
/// me3 then settled it on rungs nothing had fitted, f = 15/35/65 (`a_08`):
/// linear-in-f scores mean rms 0.0141 there, PCHIP-in-log-f 0.0092, cubic in
/// log f 0.0093, Akima 0.0105, plain PCHIP 0.0113 — and INSERTING those three
/// measured columns scores 0.0000. Changing the family buys at most 1.5×;
/// carrying the measurement buys the whole residual, so the family stays and
/// the columns land.
///
/// The residual is what it is, and this is it: ON a column the table is within 0.0009 of
/// the measurement; BETWEEN two columns it is still unmeasured. The two widest
/// gaps of the eight-column version were probed and closed rather than
/// estimated (f = 15 and f = 35, above), which leaves the f ≤ 10 end as the
/// coarsest remaining seam — dropping f = 5 and predicting it from f = 1/10
/// costs rms 0.0253 (`a_07` §2). That seam stays open by decision: no export sits inside
/// that gap, and its transition is narrow enough (W ≤ 0.17 in ρ) that the same
/// α error is a far smaller contour displacement than at f = 15/35
/// (`me3-a-report.md` §4).
///
/// # Carried as measurement, not as a formula
///
/// * `d_out` deliberately does NOT land as a constant — it is baked into the
///   column tails, so the value below costs zero pixels either way. It is
///   **√2**, and me3 excluded `1.43` and B7's `1.4335` outright (`me3-a-report`
///   §0-Q2). Four SHAPE-FREE instruments agree: the sector-block correction
///   endpoint reads 1.41480 ± 0.00046 on the major axis across all eleven rungs
///   (`a_19`); a sign census puts the excess darkening below 2σ from ρ = 1.4160
///   and pure dither (exactly 0.500) over [1.418, 1.424) (`a_18`); the strict
///   all-darkened block bound is 1.41367 (`a_16`); and the last ring-mean band
///   significantly darker than nomask ends at 1.4142 (`a_10`). Forward check:
///   √2 predicts sector endpoints 1.4219 major / 1.4335 minor against measured
///   1.4231 / 1.4386, while 1.43 predicts 1.4377 and 1.4335 predicts 1.4412 —
///   both PAST what the pixels show. B7's ±0.002 was measuring JPEG 8×8 block
///   spill rather than mask support (twelve mod-8 alignment tests, p ≤ 1e−23;
///   B7-2 §3.3), and B7-2's own `1.43 ± 0.015` was the honest width of that
///   contaminated estimate. Residual systematic ±0.001, declared rather than
///   polished away.
/// * The f = 1 FAR TAIL is UNRESOLVED (B7-2 §8-2): its darkening is significant
///   out to ρ ≈ 1.25 and indistinguishable from zero past that, decaying too
///   slowly to separate "same support, small amplitude" from "smaller support".
///   The column carries what was measured and rounds to zero where the 8-bit
///   floor did.
/// * ASPECT INVARIANCE is SAMPLED, once: the shipped table scored against a
///   held-out aspect 1.2 export reads rms(α) 0.0009, max |dev| 0.0031, and its
///   α = 0.5 contour lands 0.04 px from the measurement (against 0.0004 on the
///   fitted aspect 2.5) — and the best single radial rescale between the two
///   geometries is k = 1.00076, so no part of the falloff is anchored in pixels
///   (`me3-b-report.md` H1/A1-A2). Scope, kept rather than generalised: ONE
///   extra aspect ratio, at f = 50 only, still centred. Every other rung is
///   still one geometry (aspect 2.5, centred, Angle 0).
/// * Every column carries the same residual measurement blur that makes f = 0's
///   own column 0.0084 wide, so the narrow rungs are marginally softer here
///   than Lightroom's truth. Deconvolving it would be inventing a kernel.
/// * `roundness` still does not enter — a measured no-op at +100 with feather
///   both 0 and 50 (B7-2 §5). See `MaskGeometry::Radial` in `mask_weight`.
fn radial_falloff(feather: f32, d: f32) -> f32 {
    // Lightroom's own 0..100 feather units — the axis the columns sit on. The
    // recipe carries the same number as a 0..1 fraction; `xmp.rs` converts on
    // the boundary in both directions.
    //
    // NaN is spelled out rather than left to `clamp`, which PROPAGATES it: a
    // NaN feather would otherwise pass `f <= 0.0`, survive the `d` guard on any
    // finite sample point, and come back out as a NaN weight — which survives
    // `wgt <= 0.001` and casts to black. It degrades to the hard edge, the same
    // stance `brush_kernel_exponents` takes for a NaN hardness, and a
    // hand-edited `recipe.json` is the only way to produce one.
    let f = if feather.is_nan() { 0.0 } else { feather.clamp(0.0, 1.0) * 100.0 };
    if f <= 0.0 {
        // The analytic hard edge (see above).
        return if d < 1.0 { 1.0 } else { 0.0 };
    }
    // Past the last row every column is already 0, so this is the table's
    // extent and NOT a claim about `d_out`. The `is_finite` half is not
    // decoration: a NaN `d` would otherwise index row 0 (NaN casts to 0) and
    // blend with a NaN weight, and a NaN mask weight survives the `wgt <= 0.001`
    // early-out and casts to black — the same trap `brush_kernel_at` guards and
    // the one the old degenerate-`ramp` comment described.
    let last = RADIAL_FALLOFF.len() - 1;
    if !d.is_finite() || d >= RADIAL_FALLOFF_RHO0 + RADIAL_FALLOFF_DRHO * last as f32 {
        return 0.0;
    }
    let x = ((d - RADIAL_FALLOFF_RHO0) / RADIAL_FALLOFF_DRHO).max(0.0);
    let k = (x as usize).min(last - 1);
    let u = x - k as f32;
    // One column, interpolated in ρ. Row 0 is all 1, so clamping `x` at 0 IS
    // α(0) = 1 and needs no second branch.
    let col = |j: usize| RADIAL_FALLOFF[k][j] * (1.0 - u) + RADIAL_FALLOFF[k + 1][j] * u;
    let hi = RADIAL_FALLOFF_F
        .iter()
        .position(|&c| f <= c)
        .unwrap_or(RADIAL_FALLOFF_F.len() - 1);
    let (a_lo, f_lo) = if hi == 0 {
        // 0 < f < 1: the gap between Lightroom's hard edge and its first
        // feathered rung, which nothing sampled — Lightroom only ever writes
        // whole feather units, but this engine's own slider is continuous. The
        // lower end is the hard edge read ON THE SAME GRID, which keeps the
        // family continuous in `d` for every f > 0; f == 0 itself is still the
        // exact step above, and the two differ only across one row (0.005 in ρ,
        // ~5 px on the measured frame's major axis).
        let step = |i: usize| {
            if RADIAL_FALLOFF_RHO0 + RADIAL_FALLOFF_DRHO * i as f32 <= 1.0 { 1.0 } else { 0.0 }
        };
        (step(k) * (1.0 - u) + step(k + 1) * u, 0.0)
    } else {
        (col(hi - 1), RADIAL_FALLOFF_F[hi - 1])
    };
    let t = (f - f_lo) / (RADIAL_FALLOFF_F[hi] - f_lo);
    a_lo * (1.0 - t) + col(hi) * t
}

/// [`RADIAL_FALLOFF`]'s feather columns, in Lightroom's own 0..100 units.
const RADIAL_FALLOFF_F: [f32; 11] = [1.0, 5.0, 10.0, 15.0, 25.0, 35.0, 50.0, 65.0, 75.0, 90.0, 100.0];

/// ρ of [`RADIAL_FALLOFF`]'s first row and the row spacing — the measurement's
/// own bin centres (`b7b_12_dense.py` bins ρ at 0.005 from 0).
const RADIAL_FALLOFF_RHO0: f32 = 0.0025;
const RADIAL_FALLOFF_DRHO: f32 = 0.005;

/// The measured α(ρ; feather) itself: rows are ρ = `0.0025 + 0.005 i`, columns
/// are [`RADIAL_FALLOFF_F`]. Provenance and conditioning: [`radial_falloff`].
///
/// `approx_constant` is silenced because three of these 3190 measurements land
/// on a mathematical constant by coincidence: 0.4342 at (ρ = 0.7425, f = 75)
/// and 0.4343 at (ρ = 0.8225, f = 50) near `LOG10_E`, 0.3010 at (ρ = 0.7975,
/// f = 90) near `LOG10_2` — the three the lint reports with the `allow` lifted,
/// all of them in columns me3's insertion did not touch. They are photographs
/// of a mask edge, not logarithms, and substituting the constant would corrupt
/// the data the lint is pointing at.
#[rustfmt::skip]
#[allow(clippy::approx_constant)]
const RADIAL_FALLOFF: [[f32; 11]; 290] = [
    // rho = 0.0025
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9970, 0.9955],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9986, 0.9951, 0.9915, 0.9883],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9956, 0.9926, 0.9861, 0.9815],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9944, 0.9882, 0.9801, 0.9743],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9899, 0.9833, 0.9726, 0.9656],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9884, 0.9781, 0.9656, 0.9574],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9884, 0.9781, 0.9629, 0.9537],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9884, 0.9781, 0.9629, 0.9529],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9859, 0.9741, 0.9562, 0.9452],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9822, 0.9701, 0.9513, 0.9387],
    // rho = 0.1025
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9811, 0.9670, 0.9469, 0.9335],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9794, 0.9648, 0.9430, 0.9284],
    [1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 0.9771, 0.9614, 0.9383, 0.9226],
    [1.0000, 0.9996, 0.9996, 0.9996, 0.9996, 0.9995, 0.9982, 0.9736, 0.9574, 0.9327, 0.9162],
    [1.0000, 0.9993, 0.9993, 0.9993, 0.9993, 0.9993, 0.9981, 0.9718, 0.9546, 0.9284, 0.9109],
    [1.0000, 0.9991, 0.9991, 0.9991, 0.9991, 0.9990, 0.9972, 0.9695, 0.9514, 0.9238, 0.9056],
    [1.0000, 0.9991, 0.9991, 0.9991, 0.9990, 0.9990, 0.9969, 0.9677, 0.9486, 0.9195, 0.9003],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9988, 0.9967, 0.9664, 0.9460, 0.9155, 0.8957],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9986, 0.9962, 0.9643, 0.9433, 0.9111, 0.8902],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9960, 0.9624, 0.9401, 0.9067, 0.8847],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9953, 0.9604, 0.9371, 0.9023, 0.8794],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9953, 0.9587, 0.9348, 0.8987, 0.8747],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9950, 0.9570, 0.9318, 0.8944, 0.8693],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9947, 0.9555, 0.9295, 0.8901, 0.8644],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9944, 0.9538, 0.9264, 0.8858, 0.8590],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9944, 0.9520, 0.9239, 0.8822, 0.8545],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9983, 0.9939, 0.9505, 0.9213, 0.8778, 0.8491],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9982, 0.9933, 0.9480, 0.9180, 0.8733, 0.8442],
    [1.0000, 0.9989, 0.9989, 0.9989, 0.9988, 0.9982, 0.9930, 0.9463, 0.9154, 0.8693, 0.8386],
    [1.0000, 0.9988, 0.9988, 0.9988, 0.9988, 0.9979, 0.9925, 0.9441, 0.9123, 0.8648, 0.8336],
    // rho = 0.2025
    [1.0000, 0.9988, 0.9987, 0.9986, 0.9986, 0.9976, 0.9915, 0.9420, 0.9092, 0.8604, 0.8278],
    [1.0000, 0.9988, 0.9987, 0.9986, 0.9986, 0.9976, 0.9913, 0.9404, 0.9066, 0.8561, 0.8229],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9985, 0.9970, 0.9901, 0.9379, 0.9030, 0.8516, 0.8173],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9985, 0.9970, 0.9900, 0.9362, 0.9006, 0.8476, 0.8124],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9984, 0.9968, 0.9893, 0.9340, 0.8976, 0.8431, 0.8070],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9984, 0.9966, 0.9887, 0.9319, 0.8944, 0.8386, 0.8016],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9984, 0.9966, 0.9879, 0.9300, 0.8915, 0.8344, 0.7966],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9983, 0.9960, 0.9872, 0.9277, 0.8885, 0.8297, 0.7912],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9983, 0.9960, 0.9868, 0.9261, 0.8855, 0.8257, 0.7861],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9983, 0.9955, 0.9863, 0.9237, 0.8825, 0.8210, 0.7805],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9953, 0.9854, 0.9216, 0.8794, 0.8167, 0.7753],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9950, 0.9846, 0.9195, 0.8765, 0.8122, 0.7702],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9949, 0.9839, 0.9176, 0.8736, 0.8082, 0.7649],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9947, 0.9834, 0.9155, 0.8706, 0.8040, 0.7599],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9945, 0.9823, 0.9131, 0.8673, 0.7994, 0.7546],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9939, 0.9811, 0.9110, 0.8643, 0.7950, 0.7492],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9982, 0.9934, 0.9807, 0.9087, 0.8610, 0.7905, 0.7439],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9980, 0.9930, 0.9796, 0.9063, 0.8578, 0.7860, 0.7386],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9979, 0.9925, 0.9785, 0.9038, 0.8548, 0.7815, 0.7333],
    [1.0000, 0.9988, 0.9986, 0.9986, 0.9979, 0.9922, 0.9777, 0.9017, 0.8516, 0.7773, 0.7282],
    // rho = 0.3025
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9975, 0.9913, 0.9762, 0.8992, 0.8482, 0.7725, 0.7226],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9975, 0.9910, 0.9755, 0.8969, 0.8450, 0.7682, 0.7177],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9972, 0.9901, 0.9740, 0.8941, 0.8414, 0.7634, 0.7119],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9970, 0.9896, 0.9727, 0.8918, 0.8381, 0.7588, 0.7065],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9970, 0.9889, 0.9716, 0.8893, 0.8351, 0.7546, 0.7015],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9970, 0.9884, 0.9706, 0.8871, 0.8321, 0.7504, 0.6963],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9970, 0.9879, 0.9690, 0.8844, 0.8285, 0.7456, 0.6909],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9968, 0.9871, 0.9678, 0.8820, 0.8252, 0.7411, 0.6858],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9968, 0.9867, 0.9668, 0.8797, 0.8222, 0.7371, 0.6810],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9968, 0.9859, 0.9655, 0.8770, 0.8189, 0.7325, 0.6756],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9967, 0.9851, 0.9641, 0.8746, 0.8156, 0.7281, 0.6705],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9961, 0.9840, 0.9624, 0.8717, 0.8118, 0.7234, 0.6650],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9961, 0.9831, 0.9609, 0.8691, 0.8086, 0.7190, 0.6598],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9956, 0.9821, 0.9591, 0.8662, 0.8051, 0.7143, 0.6547],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9956, 0.9811, 0.9577, 0.8634, 0.8016, 0.7099, 0.6495],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9954, 0.9803, 0.9559, 0.8608, 0.7983, 0.7055, 0.6443],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9950, 0.9790, 0.9540, 0.8581, 0.7948, 0.7010, 0.6392],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9947, 0.9779, 0.9523, 0.8552, 0.7911, 0.6965, 0.6340],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9945, 0.9767, 0.9506, 0.8523, 0.7876, 0.6919, 0.6290],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9942, 0.9757, 0.9487, 0.8496, 0.7844, 0.6878, 0.6240],
    // rho = 0.4025
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9940, 0.9742, 0.9466, 0.8465, 0.7807, 0.6832, 0.6188],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9935, 0.9730, 0.9444, 0.8435, 0.7769, 0.6786, 0.6138],
    [1.0000, 0.9988, 0.9986, 0.9984, 0.9930, 0.9715, 0.9423, 0.8404, 0.7733, 0.6740, 0.6087],
    [1.0000, 0.9988, 0.9986, 0.9980, 0.9924, 0.9698, 0.9395, 0.8371, 0.7693, 0.6693, 0.6033],
    [1.0000, 0.9988, 0.9986, 0.9979, 0.9918, 0.9680, 0.9372, 0.8337, 0.7655, 0.6645, 0.5982],
    [1.0000, 0.9988, 0.9986, 0.9979, 0.9912, 0.9663, 0.9349, 0.8304, 0.7616, 0.6600, 0.5930],
    [1.0000, 0.9988, 0.9986, 0.9979, 0.9908, 0.9649, 0.9324, 0.8271, 0.7578, 0.6552, 0.5877],
    [1.0000, 0.9988, 0.9986, 0.9979, 0.9905, 0.9633, 0.9302, 0.8242, 0.7543, 0.6511, 0.5831],
    [1.0000, 0.9988, 0.9985, 0.9978, 0.9893, 0.9611, 0.9272, 0.8207, 0.7503, 0.6464, 0.5779],
    [1.0000, 0.9988, 0.9982, 0.9973, 0.9885, 0.9588, 0.9242, 0.8167, 0.7461, 0.6415, 0.5725],
    [1.0000, 0.9988, 0.9982, 0.9973, 0.9880, 0.9569, 0.9215, 0.8135, 0.7423, 0.6369, 0.5677],
    [1.0000, 0.9988, 0.9982, 0.9973, 0.9874, 0.9551, 0.9188, 0.8098, 0.7383, 0.6322, 0.5626],
    [1.0000, 0.9988, 0.9982, 0.9973, 0.9865, 0.9530, 0.9156, 0.8064, 0.7342, 0.6278, 0.5575],
    [1.0000, 0.9988, 0.9982, 0.9971, 0.9856, 0.9504, 0.9126, 0.8027, 0.7302, 0.6231, 0.5525],
    [1.0000, 0.9988, 0.9982, 0.9970, 0.9848, 0.9483, 0.9093, 0.7988, 0.7260, 0.6184, 0.5474],
    [1.0000, 0.9988, 0.9981, 0.9968, 0.9836, 0.9457, 0.9061, 0.7950, 0.7219, 0.6139, 0.5425],
    [1.0000, 0.9988, 0.9981, 0.9967, 0.9826, 0.9431, 0.9026, 0.7911, 0.7176, 0.6091, 0.5375],
    [1.0000, 0.9988, 0.9981, 0.9967, 0.9819, 0.9408, 0.8992, 0.7873, 0.7137, 0.6047, 0.5327],
    [1.0000, 0.9988, 0.9979, 0.9963, 0.9804, 0.9378, 0.8954, 0.7831, 0.7092, 0.5997, 0.5276],
    [1.0000, 0.9988, 0.9979, 0.9963, 0.9794, 0.9353, 0.8919, 0.7793, 0.7048, 0.5952, 0.5226],
    // rho = 0.5025
    [1.0000, 0.9988, 0.9979, 0.9960, 0.9781, 0.9322, 0.8879, 0.7749, 0.7005, 0.5903, 0.5177],
    [1.0000, 0.9988, 0.9977, 0.9958, 0.9768, 0.9292, 0.8838, 0.7705, 0.6958, 0.5856, 0.5127],
    [1.0000, 0.9988, 0.9977, 0.9956, 0.9754, 0.9262, 0.8798, 0.7663, 0.6915, 0.5807, 0.5077],
    [1.0000, 0.9988, 0.9977, 0.9956, 0.9740, 0.9230, 0.8757, 0.7619, 0.6869, 0.5762, 0.5028],
    [1.0000, 0.9988, 0.9977, 0.9954, 0.9727, 0.9198, 0.8716, 0.7575, 0.6824, 0.5715, 0.4980],
    [1.0000, 0.9988, 0.9977, 0.9950, 0.9709, 0.9164, 0.8670, 0.7529, 0.6779, 0.5667, 0.4932],
    [1.0000, 0.9988, 0.9977, 0.9950, 0.9697, 0.9130, 0.8627, 0.7485, 0.6733, 0.5622, 0.4885],
    [1.0000, 0.9988, 0.9977, 0.9950, 0.9682, 0.9096, 0.8582, 0.7440, 0.6690, 0.5577, 0.4839],
    [1.0000, 0.9988, 0.9977, 0.9950, 0.9663, 0.9060, 0.8535, 0.7393, 0.6642, 0.5530, 0.4791],
    [1.0000, 0.9988, 0.9977, 0.9948, 0.9647, 0.9020, 0.8485, 0.7342, 0.6592, 0.5480, 0.4741],
    [1.0000, 0.9988, 0.9971, 0.9936, 0.9619, 0.8975, 0.8430, 0.7289, 0.6540, 0.5428, 0.4690],
    [1.0000, 0.9988, 0.9971, 0.9935, 0.9599, 0.8935, 0.8379, 0.7239, 0.6488, 0.5379, 0.4642],
    [1.0000, 0.9988, 0.9971, 0.9934, 0.9579, 0.8893, 0.8326, 0.7188, 0.6441, 0.5330, 0.4595],
    [1.0000, 0.9988, 0.9970, 0.9929, 0.9556, 0.8848, 0.8271, 0.7135, 0.6389, 0.5281, 0.4546],
    [1.0000, 0.9988, 0.9970, 0.9928, 0.9535, 0.8805, 0.8216, 0.7083, 0.6339, 0.5234, 0.4500],
    [1.0000, 0.9988, 0.9970, 0.9923, 0.9512, 0.8761, 0.8162, 0.7030, 0.6287, 0.5185, 0.4453],
    [1.0000, 0.9988, 0.9970, 0.9923, 0.9490, 0.8718, 0.8106, 0.6979, 0.6238, 0.5137, 0.4407],
    [1.0000, 0.9988, 0.9967, 0.9916, 0.9458, 0.8667, 0.8045, 0.6922, 0.6183, 0.5086, 0.4357],
    [1.0000, 0.9988, 0.9965, 0.9911, 0.9431, 0.8618, 0.7984, 0.6865, 0.6130, 0.5036, 0.4312],
    [1.0000, 0.9988, 0.9965, 0.9907, 0.9405, 0.8570, 0.7924, 0.6811, 0.6078, 0.4987, 0.4265],
    // rho = 0.6025
    [1.0000, 0.9988, 0.9965, 0.9903, 0.9375, 0.8520, 0.7863, 0.6752, 0.6023, 0.4939, 0.4219],
    [1.0000, 0.9988, 0.9962, 0.9896, 0.9345, 0.8467, 0.7798, 0.6694, 0.5968, 0.4888, 0.4173],
    [1.0000, 0.9988, 0.9960, 0.9891, 0.9312, 0.8411, 0.7731, 0.6634, 0.5912, 0.4836, 0.4125],
    [1.0000, 0.9988, 0.9960, 0.9885, 0.9281, 0.8358, 0.7668, 0.6575, 0.5858, 0.4789, 0.4082],
    [1.0000, 0.9988, 0.9960, 0.9881, 0.9248, 0.8301, 0.7598, 0.6515, 0.5802, 0.4739, 0.4036],
    [1.0000, 0.9988, 0.9958, 0.9872, 0.9211, 0.8244, 0.7529, 0.6453, 0.5745, 0.4688, 0.3991],
    [1.0000, 0.9988, 0.9956, 0.9864, 0.9173, 0.8185, 0.7459, 0.6390, 0.5686, 0.4638, 0.3945],
    [1.0000, 0.9988, 0.9955, 0.9857, 0.9135, 0.8125, 0.7387, 0.6328, 0.5629, 0.4587, 0.3901],
    [1.0000, 0.9988, 0.9955, 0.9853, 0.9096, 0.8065, 0.7317, 0.6265, 0.5572, 0.4538, 0.3857],
    [1.0000, 0.9988, 0.9955, 0.9844, 0.9056, 0.8003, 0.7243, 0.6201, 0.5513, 0.4487, 0.3811],
    [1.0000, 0.9987, 0.9952, 0.9833, 0.9016, 0.7939, 0.7169, 0.6136, 0.5455, 0.4436, 0.3767],
    [1.0000, 0.9987, 0.9950, 0.9825, 0.8971, 0.7876, 0.7094, 0.6071, 0.5395, 0.4386, 0.3723],
    [1.0000, 0.9986, 0.9948, 0.9813, 0.8927, 0.7809, 0.7017, 0.6005, 0.5334, 0.4336, 0.3681],
    [1.0000, 0.9984, 0.9943, 0.9801, 0.8878, 0.7740, 0.6939, 0.5936, 0.5273, 0.4285, 0.3634],
    [1.0000, 0.9984, 0.9943, 0.9789, 0.8830, 0.7672, 0.6861, 0.5869, 0.5212, 0.4233, 0.3590],
    [1.0000, 0.9984, 0.9940, 0.9778, 0.8782, 0.7603, 0.6782, 0.5802, 0.5153, 0.4184, 0.3549],
    [1.0000, 0.9984, 0.9938, 0.9766, 0.8730, 0.7533, 0.6703, 0.5733, 0.5090, 0.4133, 0.3505],
    [1.0000, 0.9984, 0.9935, 0.9750, 0.8676, 0.7462, 0.6623, 0.5663, 0.5029, 0.4083, 0.3463],
    [1.0000, 0.9984, 0.9935, 0.9738, 0.8625, 0.7390, 0.6542, 0.5596, 0.4967, 0.4033, 0.3421],
    [1.0000, 0.9984, 0.9933, 0.9722, 0.8569, 0.7317, 0.6460, 0.5525, 0.4905, 0.3984, 0.3378],
    // rho = 0.7025
    [1.0000, 0.9984, 0.9930, 0.9706, 0.8512, 0.7242, 0.6377, 0.5455, 0.4842, 0.3933, 0.3335],
    [1.0000, 0.9984, 0.9929, 0.9690, 0.8455, 0.7168, 0.6295, 0.5384, 0.4781, 0.3884, 0.3295],
    [1.0000, 0.9984, 0.9927, 0.9671, 0.8393, 0.7092, 0.6213, 0.5315, 0.4720, 0.3835, 0.3255],
    [1.0000, 0.9984, 0.9924, 0.9652, 0.8332, 0.7014, 0.6128, 0.5244, 0.4658, 0.3785, 0.3212],
    [1.0000, 0.9984, 0.9921, 0.9630, 0.8268, 0.6937, 0.6044, 0.5173, 0.4593, 0.3735, 0.3171],
    [1.0000, 0.9984, 0.9920, 0.9611, 0.8203, 0.6856, 0.5960, 0.5101, 0.4530, 0.3684, 0.3130],
    [1.0000, 0.9984, 0.9920, 0.9590, 0.8138, 0.6780, 0.5877, 0.5030, 0.4470, 0.3639, 0.3091],
    [1.0000, 0.9983, 0.9912, 0.9560, 0.8067, 0.6697, 0.5790, 0.4956, 0.4404, 0.3587, 0.3048],
    [1.0000, 0.9983, 0.9910, 0.9534, 0.7996, 0.6615, 0.5705, 0.4886, 0.4342, 0.3538, 0.3008],
    [1.0000, 0.9983, 0.9907, 0.9504, 0.7922, 0.6531, 0.5616, 0.4811, 0.4276, 0.3488, 0.2968],
    [1.0000, 0.9983, 0.9901, 0.9473, 0.7848, 0.6449, 0.5533, 0.4741, 0.4217, 0.3440, 0.2929],
    [1.0000, 0.9983, 0.9895, 0.9441, 0.7771, 0.6364, 0.5446, 0.4667, 0.4152, 0.3391, 0.2888],
    [1.0000, 0.9983, 0.9892, 0.9408, 0.7692, 0.6280, 0.5360, 0.4597, 0.4092, 0.3344, 0.2851],
    [1.0000, 0.9981, 0.9883, 0.9368, 0.7612, 0.6193, 0.5274, 0.4523, 0.4028, 0.3294, 0.2810],
    [1.0000, 0.9980, 0.9876, 0.9327, 0.7527, 0.6107, 0.5187, 0.4451, 0.3965, 0.3247, 0.2771],
    [1.0000, 0.9978, 0.9867, 0.9284, 0.7442, 0.6019, 0.5099, 0.4377, 0.3901, 0.3198, 0.2729],
    [1.0000, 0.9978, 0.9860, 0.9241, 0.7358, 0.5933, 0.5016, 0.4310, 0.3842, 0.3151, 0.2693],
    [1.0000, 0.9978, 0.9852, 0.9192, 0.7269, 0.5844, 0.4929, 0.4237, 0.3780, 0.3103, 0.2654],
    [1.0000, 0.9978, 0.9841, 0.9142, 0.7178, 0.5755, 0.4845, 0.4167, 0.3719, 0.3056, 0.2614],
    [1.0000, 0.9978, 0.9832, 0.9089, 0.7087, 0.5668, 0.4761, 0.4098, 0.3659, 0.3010, 0.2578],
    // rho = 0.8025
    [1.0000, 0.9978, 0.9821, 0.9033, 0.6995, 0.5580, 0.4677, 0.4028, 0.3600, 0.2965, 0.2542],
    [1.0000, 0.9977, 0.9806, 0.8970, 0.6899, 0.5490, 0.4593, 0.3958, 0.3540, 0.2917, 0.2503],
    [1.0000, 0.9977, 0.9792, 0.8906, 0.6800, 0.5399, 0.4508, 0.3888, 0.3479, 0.2871, 0.2466],
    [1.0000, 0.9975, 0.9773, 0.8835, 0.6701, 0.5307, 0.4424, 0.3819, 0.3420, 0.2825, 0.2428],
    [1.0000, 0.9975, 0.9754, 0.8763, 0.6599, 0.5216, 0.4343, 0.3752, 0.3362, 0.2780, 0.2392],
    [1.0000, 0.9974, 0.9731, 0.8685, 0.6494, 0.5125, 0.4260, 0.3682, 0.3302, 0.2734, 0.2355],
    [1.0000, 0.9974, 0.9705, 0.8603, 0.6390, 0.5033, 0.4179, 0.3616, 0.3245, 0.2690, 0.2318],
    [1.0000, 0.9974, 0.9682, 0.8517, 0.6283, 0.4941, 0.4099, 0.3550, 0.3188, 0.2645, 0.2282],
    [1.0000, 0.9974, 0.9647, 0.8424, 0.6173, 0.4848, 0.4018, 0.3483, 0.3129, 0.2599, 0.2245],
    [1.0000, 0.9973, 0.9613, 0.8325, 0.6062, 0.4756, 0.3939, 0.3418, 0.3073, 0.2556, 0.2209],
    [1.0000, 0.9973, 0.9574, 0.8225, 0.5950, 0.4664, 0.3861, 0.3353, 0.3016, 0.2512, 0.2174],
    [1.0000, 0.9973, 0.9530, 0.8117, 0.5836, 0.4571, 0.3783, 0.3288, 0.2960, 0.2468, 0.2139],
    [1.0000, 0.9971, 0.9480, 0.8002, 0.5719, 0.4479, 0.3707, 0.3225, 0.2906, 0.2425, 0.2104],
    [1.0000, 0.9971, 0.9425, 0.7883, 0.5602, 0.4387, 0.3632, 0.3162, 0.2851, 0.2383, 0.2069],
    [1.0000, 0.9970, 0.9360, 0.7755, 0.5482, 0.4295, 0.3557, 0.3100, 0.2797, 0.2340, 0.2035],
    [1.0000, 0.9967, 0.9287, 0.7618, 0.5358, 0.4201, 0.3482, 0.3037, 0.2742, 0.2297, 0.1999],
    [1.0000, 0.9963, 0.9206, 0.7476, 0.5234, 0.4108, 0.3408, 0.2976, 0.2688, 0.2255, 0.1965],
    [1.0000, 0.9961, 0.9117, 0.7327, 0.5108, 0.4014, 0.3334, 0.2914, 0.2634, 0.2212, 0.1931],
    [1.0000, 0.9954, 0.9015, 0.7170, 0.4982, 0.3921, 0.3262, 0.2853, 0.2581, 0.2171, 0.1898],
    [1.0000, 0.9953, 0.8905, 0.7009, 0.4854, 0.3830, 0.3192, 0.2795, 0.2530, 0.2131, 0.1865],
    // rho = 0.9025
    [1.0000, 0.9941, 0.8778, 0.6836, 0.4724, 0.3737, 0.3121, 0.2734, 0.2477, 0.2089, 0.1830],
    [1.0000, 0.9934, 0.8640, 0.6659, 0.4593, 0.3647, 0.3052, 0.2677, 0.2427, 0.2049, 0.1797],
    [1.0000, 0.9922, 0.8489, 0.6473, 0.4462, 0.3557, 0.2983, 0.2619, 0.2376, 0.2009, 0.1763],
    [1.0000, 0.9901, 0.8315, 0.6278, 0.4329, 0.3465, 0.2915, 0.2562, 0.2325, 0.1970, 0.1731],
    [1.0000, 0.9873, 0.8124, 0.6075, 0.4194, 0.3374, 0.2849, 0.2504, 0.2274, 0.1929, 0.1698],
    [1.0000, 0.9836, 0.7913, 0.5862, 0.4058, 0.3282, 0.2781, 0.2447, 0.2224, 0.1889, 0.1664],
    [1.0000, 0.9787, 0.7683, 0.5644, 0.3924, 0.3194, 0.2716, 0.2392, 0.2176, 0.1850, 0.1633],
    [1.0000, 0.9720, 0.7427, 0.5417, 0.3786, 0.3104, 0.2651, 0.2338, 0.2128, 0.1812, 0.1601],
    [1.0000, 0.9630, 0.7149, 0.5183, 0.3649, 0.3015, 0.2589, 0.2284, 0.2080, 0.1774, 0.1570],
    [1.0000, 0.9511, 0.6846, 0.4940, 0.3513, 0.2926, 0.2525, 0.2230, 0.2034, 0.1737, 0.1538],
    [1.0000, 0.9347, 0.6515, 0.4692, 0.3374, 0.2838, 0.2463, 0.2178, 0.1987, 0.1700, 0.1508],
    [1.0000, 0.9132, 0.6157, 0.4437, 0.3237, 0.2750, 0.2402, 0.2126, 0.1940, 0.1663, 0.1476],
    [1.0000, 0.8845, 0.5774, 0.4177, 0.3102, 0.2664, 0.2341, 0.2074, 0.1896, 0.1627, 0.1447],
    [1.0000, 0.8468, 0.5363, 0.3913, 0.2965, 0.2577, 0.2283, 0.2024, 0.1851, 0.1591, 0.1416],
    [1.0000, 0.7978, 0.4926, 0.3647, 0.2832, 0.2494, 0.2226, 0.1975, 0.1808, 0.1556, 0.1388],
    [0.9990, 0.7351, 0.4465, 0.3377, 0.2696, 0.2409, 0.2168, 0.1926, 0.1764, 0.1522, 0.1358],
    [0.9929, 0.6559, 0.3984, 0.3106, 0.2561, 0.2325, 0.2112, 0.1878, 0.1722, 0.1487, 0.1330],
    [0.9719, 0.5582, 0.3486, 0.2833, 0.2426, 0.2241, 0.2056, 0.1829, 0.1679, 0.1451, 0.1300],
    [0.8960, 0.4415, 0.2982, 0.2562, 0.2293, 0.2159, 0.2000, 0.1782, 0.1637, 0.1417, 0.1271],
    [0.6457, 0.3112, 0.2477, 0.2293, 0.2161, 0.2078, 0.1946, 0.1737, 0.1596, 0.1384, 0.1243],
    // rho = 1.0025
    [0.0980, 0.1784, 0.1976, 0.2025, 0.2030, 0.1996, 0.1892, 0.1689, 0.1554, 0.1351, 0.1215],
    [0.0054, 0.0953, 0.1557, 0.1782, 0.1905, 0.1917, 0.1839, 0.1645, 0.1514, 0.1318, 0.1187],
    [0.0019, 0.0525, 0.1228, 0.1567, 0.1786, 0.1840, 0.1789, 0.1600, 0.1475, 0.1286, 0.1160],
    [0.0006, 0.0297, 0.0968, 0.1376, 0.1674, 0.1766, 0.1737, 0.1556, 0.1435, 0.1253, 0.1133],
    [0.0004, 0.0177, 0.0766, 0.1209, 0.1569, 0.1696, 0.1689, 0.1515, 0.1398, 0.1224, 0.1106],
    [0.0002, 0.0109, 0.0606, 0.1060, 0.1468, 0.1625, 0.1640, 0.1473, 0.1360, 0.1191, 0.1079],
    [0.0002, 0.0073, 0.0484, 0.0933, 0.1375, 0.1558, 0.1593, 0.1432, 0.1324, 0.1161, 0.1053],
    [0.0002, 0.0052, 0.0386, 0.0817, 0.1284, 0.1491, 0.1546, 0.1390, 0.1287, 0.1130, 0.1026],
    [0.0002, 0.0038, 0.0309, 0.0716, 0.1199, 0.1427, 0.1500, 0.1349, 0.1250, 0.1100, 0.1000],
    [0.0002, 0.0030, 0.0250, 0.0630, 0.1120, 0.1365, 0.1455, 0.1312, 0.1216, 0.1071, 0.0975],
    [0.0001, 0.0024, 0.0203, 0.0552, 0.1046, 0.1306, 0.1411, 0.1273, 0.1180, 0.1042, 0.0949],
    [0.0001, 0.0019, 0.0168, 0.0486, 0.0976, 0.1249, 0.1369, 0.1236, 0.1148, 0.1013, 0.0925],
    [0.0001, 0.0016, 0.0139, 0.0428, 0.0911, 0.1194, 0.1327, 0.1199, 0.1115, 0.0986, 0.0901],
    [0.0001, 0.0013, 0.0117, 0.0378, 0.0849, 0.1141, 0.1286, 0.1164, 0.1082, 0.0959, 0.0877],
    [0.0001, 0.0013, 0.0100, 0.0333, 0.0793, 0.1091, 0.1246, 0.1129, 0.1050, 0.0932, 0.0854],
    [0.0001, 0.0010, 0.0084, 0.0294, 0.0739, 0.1039, 0.1206, 0.1093, 0.1019, 0.0905, 0.0831],
    [0.0001, 0.0010, 0.0074, 0.0261, 0.0688, 0.0992, 0.1168, 0.1060, 0.0988, 0.0879, 0.0807],
    [0.0001, 0.0009, 0.0065, 0.0232, 0.0640, 0.0945, 0.1130, 0.1027, 0.0958, 0.0854, 0.0785],
    [0.0001, 0.0009, 0.0058, 0.0206, 0.0596, 0.0902, 0.1094, 0.0994, 0.0927, 0.0827, 0.0761],
    [0.0001, 0.0007, 0.0050, 0.0182, 0.0554, 0.0858, 0.1057, 0.0961, 0.0897, 0.0802, 0.0739],
    // rho = 1.1025
    [0.0001, 0.0007, 0.0046, 0.0164, 0.0516, 0.0818, 0.1022, 0.0930, 0.0870, 0.0778, 0.0717],
    [0.0001, 0.0007, 0.0042, 0.0148, 0.0480, 0.0779, 0.0989, 0.0901, 0.0842, 0.0754, 0.0696],
    [0.0001, 0.0007, 0.0039, 0.0132, 0.0447, 0.0741, 0.0955, 0.0871, 0.0814, 0.0730, 0.0673],
    [0.0001, 0.0007, 0.0035, 0.0120, 0.0415, 0.0704, 0.0922, 0.0841, 0.0787, 0.0706, 0.0652],
    [0.0001, 0.0007, 0.0033, 0.0110, 0.0387, 0.0669, 0.0890, 0.0812, 0.0761, 0.0684, 0.0632],
    [0.0001, 0.0006, 0.0028, 0.0099, 0.0359, 0.0635, 0.0857, 0.0783, 0.0734, 0.0660, 0.0610],
    [0.0001, 0.0006, 0.0027, 0.0090, 0.0334, 0.0602, 0.0827, 0.0756, 0.0709, 0.0638, 0.0590],
    [0.0001, 0.0005, 0.0024, 0.0083, 0.0309, 0.0570, 0.0797, 0.0728, 0.0683, 0.0616, 0.0569],
    [0.0001, 0.0005, 0.0023, 0.0077, 0.0289, 0.0543, 0.0768, 0.0703, 0.0660, 0.0594, 0.0552],
    [0.0001, 0.0005, 0.0022, 0.0071, 0.0269, 0.0515, 0.0740, 0.0677, 0.0636, 0.0573, 0.0532],
    [0.0001, 0.0005, 0.0020, 0.0066, 0.0250, 0.0487, 0.0713, 0.0652, 0.0612, 0.0552, 0.0512],
    [0.0001, 0.0005, 0.0017, 0.0060, 0.0232, 0.0460, 0.0684, 0.0627, 0.0589, 0.0531, 0.0493],
    [0.0001, 0.0005, 0.0017, 0.0056, 0.0216, 0.0435, 0.0657, 0.0602, 0.0565, 0.0510, 0.0474],
    [0.0001, 0.0005, 0.0016, 0.0053, 0.0201, 0.0412, 0.0633, 0.0580, 0.0545, 0.0490, 0.0457],
    [0.0001, 0.0005, 0.0015, 0.0049, 0.0188, 0.0390, 0.0608, 0.0557, 0.0524, 0.0472, 0.0439],
    [0.0001, 0.0005, 0.0014, 0.0045, 0.0176, 0.0369, 0.0583, 0.0535, 0.0503, 0.0454, 0.0421],
    [0.0001, 0.0004, 0.0014, 0.0044, 0.0163, 0.0348, 0.0559, 0.0513, 0.0482, 0.0435, 0.0404],
    [0.0001, 0.0004, 0.0014, 0.0041, 0.0154, 0.0330, 0.0538, 0.0493, 0.0463, 0.0419, 0.0389],
    [0.0001, 0.0004, 0.0012, 0.0038, 0.0144, 0.0311, 0.0514, 0.0472, 0.0443, 0.0400, 0.0372],
    [0.0001, 0.0004, 0.0011, 0.0035, 0.0133, 0.0293, 0.0493, 0.0451, 0.0424, 0.0382, 0.0355],
    // rho = 1.2025
    [0.0001, 0.0004, 0.0011, 0.0033, 0.0125, 0.0276, 0.0471, 0.0432, 0.0406, 0.0366, 0.0340],
    [0.0001, 0.0004, 0.0011, 0.0033, 0.0117, 0.0261, 0.0451, 0.0413, 0.0388, 0.0349, 0.0324],
    [0.0001, 0.0004, 0.0010, 0.0030, 0.0110, 0.0245, 0.0431, 0.0394, 0.0370, 0.0333, 0.0309],
    [0.0001, 0.0003, 0.0009, 0.0026, 0.0101, 0.0231, 0.0411, 0.0376, 0.0352, 0.0317, 0.0293],
    [0.0001, 0.0003, 0.0009, 0.0026, 0.0096, 0.0218, 0.0392, 0.0358, 0.0335, 0.0302, 0.0280],
    [0.0001, 0.0003, 0.0009, 0.0023, 0.0090, 0.0205, 0.0374, 0.0341, 0.0319, 0.0287, 0.0265],
    [0.0001, 0.0003, 0.0008, 0.0023, 0.0084, 0.0192, 0.0355, 0.0324, 0.0304, 0.0272, 0.0251],
    [0.0001, 0.0003, 0.0008, 0.0021, 0.0079, 0.0181, 0.0339, 0.0309, 0.0289, 0.0258, 0.0238],
    [0.0001, 0.0003, 0.0008, 0.0020, 0.0075, 0.0171, 0.0324, 0.0294, 0.0274, 0.0245, 0.0225],
    [0.0001, 0.0003, 0.0008, 0.0019, 0.0071, 0.0160, 0.0307, 0.0278, 0.0260, 0.0232, 0.0213],
    [0.0001, 0.0003, 0.0007, 0.0017, 0.0066, 0.0151, 0.0291, 0.0264, 0.0246, 0.0219, 0.0200],
    [0.0001, 0.0002, 0.0007, 0.0017, 0.0063, 0.0142, 0.0276, 0.0250, 0.0233, 0.0206, 0.0189],
    [0.0000, 0.0002, 0.0007, 0.0015, 0.0059, 0.0133, 0.0262, 0.0236, 0.0219, 0.0193, 0.0177],
    [0.0000, 0.0002, 0.0006, 0.0014, 0.0055, 0.0125, 0.0247, 0.0224, 0.0206, 0.0182, 0.0165],
    [0.0000, 0.0002, 0.0006, 0.0014, 0.0052, 0.0118, 0.0234, 0.0211, 0.0195, 0.0171, 0.0155],
    [0.0000, 0.0001, 0.0005, 0.0012, 0.0048, 0.0109, 0.0220, 0.0197, 0.0181, 0.0159, 0.0144],
    [0.0000, 0.0001, 0.0005, 0.0012, 0.0045, 0.0103, 0.0208, 0.0186, 0.0170, 0.0149, 0.0135],
    [0.0000, 0.0001, 0.0005, 0.0011, 0.0042, 0.0096, 0.0197, 0.0174, 0.0160, 0.0139, 0.0125],
    [0.0000, 0.0001, 0.0004, 0.0010, 0.0039, 0.0089, 0.0183, 0.0163, 0.0149, 0.0128, 0.0114],
    [0.0000, 0.0001, 0.0004, 0.0010, 0.0036, 0.0083, 0.0173, 0.0153, 0.0139, 0.0119, 0.0105],
    // rho = 1.3025
    [0.0000, 0.0001, 0.0004, 0.0009, 0.0033, 0.0078, 0.0162, 0.0143, 0.0130, 0.0110, 0.0097],
    [0.0000, 0.0001, 0.0004, 0.0008, 0.0031, 0.0072, 0.0151, 0.0133, 0.0120, 0.0102, 0.0088],
    [0.0000, 0.0001, 0.0004, 0.0008, 0.0030, 0.0069, 0.0142, 0.0126, 0.0112, 0.0094, 0.0081],
    [0.0000, 0.0001, 0.0003, 0.0008, 0.0026, 0.0063, 0.0131, 0.0114, 0.0103, 0.0085, 0.0072],
    [0.0000, 0.0001, 0.0003, 0.0008, 0.0026, 0.0059, 0.0123, 0.0105, 0.0095, 0.0078, 0.0067],
    [0.0000, 0.0001, 0.0003, 0.0007, 0.0022, 0.0054, 0.0114, 0.0097, 0.0086, 0.0071, 0.0060],
    [0.0000, 0.0001, 0.0003, 0.0007, 0.0022, 0.0050, 0.0106, 0.0090, 0.0079, 0.0064, 0.0054],
    [0.0000, 0.0001, 0.0003, 0.0006, 0.0019, 0.0046, 0.0097, 0.0082, 0.0072, 0.0057, 0.0048],
    [0.0000, 0.0001, 0.0003, 0.0006, 0.0018, 0.0043, 0.0090, 0.0074, 0.0065, 0.0052, 0.0043],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0016, 0.0038, 0.0081, 0.0067, 0.0058, 0.0044, 0.0036],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0013, 0.0034, 0.0074, 0.0061, 0.0052, 0.0040, 0.0030],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0013, 0.0030, 0.0068, 0.0054, 0.0046, 0.0033, 0.0026],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0011, 0.0027, 0.0060, 0.0048, 0.0040, 0.0029, 0.0021],
    [0.0000, 0.0001, 0.0002, 0.0004, 0.0010, 0.0024, 0.0054, 0.0043, 0.0036, 0.0026, 0.0018],
    [0.0000, 0.0001, 0.0002, 0.0003, 0.0008, 0.0020, 0.0048, 0.0037, 0.0030, 0.0021, 0.0014],
    [0.0000, 0.0000, 0.0002, 0.0003, 0.0007, 0.0018, 0.0041, 0.0033, 0.0027, 0.0018, 0.0012],
    [0.0000, 0.0000, 0.0002, 0.0002, 0.0006, 0.0015, 0.0036, 0.0028, 0.0022, 0.0014, 0.0009],
    [0.0000, 0.0000, 0.0002, 0.0002, 0.0006, 0.0013, 0.0030, 0.0023, 0.0019, 0.0013, 0.0007],
    [0.0000, 0.0000, 0.0002, 0.0002, 0.0006, 0.0011, 0.0025, 0.0019, 0.0015, 0.0010, 0.0006],
    [0.0000, 0.0000, 0.0001, 0.0001, 0.0004, 0.0008, 0.0019, 0.0014, 0.0011, 0.0007, 0.0003],
    // rho = 1.4025
    [0.0000, 0.0000, 0.0001, 0.0001, 0.0002, 0.0006, 0.0013, 0.0010, 0.0008, 0.0004, 0.0002],
    [0.0000, 0.0000, 0.0000, 0.0001, 0.0002, 0.0003, 0.0007, 0.0005, 0.0004, 0.0003, 0.0001],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0001, 0.0003, 0.0002, 0.0001, 0.0001, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000],
];

/// Where inside a pixel a mask is sampled, as a fraction of the pixel's own
/// width — `0.5`, its CENTRE.
///
/// **The whole mask family samples through this one constant** (R29 C2): the
/// frame loops that ask for a weight ([`apply_masks`]' `weight_at`,
/// [`mask_coverage`]'s overlay, `fit_zoned`'s analysis moments), the texel grid
/// [`rasterise_brush_group`] stamps onto, and the texel grid
/// [`sample_gray_norm`] reads back from. Radial, Linear, Brush, Bitmap and
/// AiMask all land on it, because all five reach the frame through those.
///
/// **MEASURED, on two different negatives.** R29 B7 fitted the hard edge
/// (`Feather = 0`) of a Lightroom-exported radial mask on a 6240 × 4160 frame
/// whose sidecar geometry is `Left = 0.333333`, `Right = 0.666667`,
/// `Top = 0.4`, `Bottom = 0.6` — a centre at normalised `(0.5, 0.5)`, i.e.
/// `(3120.0, 2080.0)` in continuous frame units. The fit, in PIXEL-INDEX
/// coordinates, put it at **(3119.46, 2079.50)** (`b7_03`, edge rms 0.24 px);
/// R29 B7-2 re-fitted a SECOND capture — a different body, lens, day and
/// `OriginalDocumentID` (`b7b_18`, D6) — and got **(3119.49, 2079.51)**. An
/// ellipse fitted over pixel INDICES returns `p − 0.5` for a true continuous
/// centre `p`, so both readings say `p = 3119.96 / 3119.99 ≈ 3120` and
/// `2080.00 / 2080.01 ≈ 2080`: Lightroom maps the stored fraction `u` to the
/// continuous position `u·W`, and pixel `i` — whose centre is at `i + 0.5` —
/// therefore carries the mask value of `u = (i + 0.5)/W`.
///
/// **It is also what the rest of this engine already assumed.**
/// [`apply_lens_geometry`] measures every pixel's radius from
/// `cx = (w − 1)/2`, and `x − (w − 1)/2` IS `x + 0.5 − w/2` — the pixel-centre
/// offset. [`MaskUnwarp::at`] takes `(nx − 0.5)·w`, which equals that same
/// offset only under this convention; with `nx = x/w` the mask un-warp sat
/// half a pixel off the resampler it is defined to invert. The two now agree
/// exactly rather than nearly.
///
/// **⚠ RENDER-BEHAVIOUR CHANGE**, the seventh on the v0.35.0 list: every mask
/// of every type now lands half a pixel up and to the left of where this
/// engine used to put it. That is the direction the measurement demands — the
/// old `x/w` gave pixel `x` the value belonging to continuous position `x`,
/// which is its own top-left CORNER — and the size of the correction is
/// exactly 0.5 px on each axis, everywhere, with no dependence on feather,
/// geometry or frame size.
///
/// Not a tunable: it is `0.5` because a pixel's centre is at its middle. It is
/// named only so the four sites that must agree can be seen to agree, and so
/// that a mutation of any one of them is a mutation of a shared constant.
pub(crate) const MASK_SAMPLE_CENTRE: f32 = 0.5;

/// [`mask_weight`] with the frame adaptation applied — the ONE place a stored
/// coordinate becomes the coordinate this engine's pre-geometry rasteriser
/// should be asked about.
///
/// Split from `mask_weight` rather than folded into it so that `mask_weight`
/// keeps meaning exactly "the weight of this geometry at this STORED point",
/// which is what every unit test in this file asserts and what the XMP
/// boundary's byte fidelity is defined against.
fn mask_weight_in(
    g: &MaskGeometry,
    nx: f32,
    ny: f32,
    bmp: Option<&image::GrayImage>,
    unwarp: Option<&MaskUnwarp>,
    dims: (f32, f32),
) -> f32 {
    // The explicit split is the H2 boundary. RADIAL retains the landed
    // `m_lr^-1 ∘ T_engine` point sampler byte-for-byte. LINEAR uses only
    // `T_engine`; its corrections-off camera map has already moved the two
    // handles in `MaskFrame::linear_handles_to_raw`, never this sample.
    let (nx, ny) = match (g, unwarp) {
        (MaskGeometry::Radial { .. }, Some(u)) => u.at(nx, ny),
        (MaskGeometry::Linear { .. }, Some(u)) => u.engine_at(nx, ny),
        _ => (nx, ny),
    };
    mask_weight_metric(g, nx, ny, bmp, dims)
}

/// Evaluate a geometry using the metric Lightroom uses for its stored frame.
///
/// Linear endpoints are stored as normalized coordinates, but their dot
/// product is a pixel-space measurement. On a non-square frame the two are
/// not equivalent: the pixel vector is `(vx * w, vy * h)`. Axis-aligned and
/// square-frame gradients deliberately retain the old normalized arithmetic,
/// keeping those render bytes stable while making the angled case exact.
fn mask_weight_metric(
    g: &MaskGeometry,
    nx: f32,
    ny: f32,
    bmp: Option<&image::GrayImage>,
    dims: (f32, f32),
) -> f32 {
    match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let (vx, vy) = (full_x - zero_x, full_y - zero_y);
            let len2 = vx * vx + vy * vy;
            if len2 < 1e-9 {
                return 1.0;
            }
            let (w, h) = dims;
            if vx == 0.0 || vy == 0.0 || w == h || !(w > 0.0 && h > 0.0) {
                return linear_coverage(
                    ((nx - zero_x) * vx + (ny - zero_y) * vy) / len2,
                    LINEAR_FALLOFF,
                );
            }
            let dx = (nx - zero_x) * w;
            let dy = (ny - zero_y) * h;
            let px = vx * w;
            let py = vy * h;
            linear_coverage((dx * px + dy * py) / (px * px + py * py), LINEAR_FALLOFF)
        }
        _ => mask_weight(g, nx, ny, bmp),
    }
}

/// Mask coverage [0,1] at normalized frame coordinate (nx, ny).
fn mask_weight(g: &MaskGeometry, nx: f32, ny: f32, bmp: Option<&image::GrayImage>) -> f32 {
    match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let (vx, vy) = (full_x - zero_x, full_y - zero_y);
            let len2 = vx * vx + vy * vy;
            if len2 < 1e-9 {
                return 1.0;
            }
            linear_coverage(
                ((nx - zero_x) * vx + (ny - zero_y) * vy) / len2,
                LINEAR_FALLOFF,
            )
        }
        // `roundness` is carried but deliberately NOT rendered — pure ellipse,
        // see `MaskGeometry::Radial` in recipe.rs. Its DOMAIN is known
        // (Lightroom's ±100 integer slider — v0.31.1 widened the importer and
        // the clamp to match), and since R29 B7 (2026-08-20) the no-op is a
        // MEASURED fact, not a guess: a hand-authored Roundness="100" probe
        // renders geometry within 0.1 px of its Roundness=0 reference (edge
        // rms 0.31 px over 1440 angles — no circularisation, no superellipse)
        // and is pixel-identical to it where the two masks overlap. R29 B7-2
        // (2026-08-21) then closed the biggest hole with a Roundness=100 &
        // Feather=50 cross probe: the no-op HOLDS with feather active
        // (|Δα| ≤ 0.006 on both falloff branches, same support endpoint), so
        // Roundness does not modulate the falloff shape either. R29 me3-b
        // (2026-08-21) closed the two scope caveats this comment used to carry.
        // NEGATIVES are no longer untested: a hand-authored Roundness="-100"
        // probe renders the same geometry as its Roundness="+100" sibling on
        // the same frame at Feather=0 (ellipse parameters within 0.03 px,
        // |Δα| ≤ 0.0024 — me3-b §4), and Lightroom accepts and writes the
        // value back verbatim, so the ±100 domain gate in xmp.rs matches what
        // was measured. The "minor-axis sector only" caveat is GONE too: the
        // Roundness 0 vs +100 pair at Feather=50 is byte-identical over the
        // WHOLE exported frame — same entropy-coded segment, max|Δ| = 0 across
        // 26 M pixels (me3-b §5, re-verified first-hand at adjudication) — so
        // every sector is covered, not just the one the earlier fit sampled.
        // What genuinely stays open (docs/V2_PLAN.md §7 item 11): all four
        // probes are the SAME box (Top 0.4 / Left 0.333333 / Bottom 0.6 /
        // Right 0.666667, aspect 2.5, centred, Angle 0), so a second geometry
        // is still zero-sample; only {−100, 0, +100} were exported, so an
        // implementation that acts strictly INSIDE the endpoints would be
        // invisible here; and Roundness × Angle≠0 is untested.
        // Registered, not claimed (me3-b §4.3, H8): the −100 and +100 exports
        // differ by a whole-frame, zero-mean dither rearrangement of ≤ ±4 DN
        // with no spatial structure and no far-field asymmetry. Mechanism
        // unresolved. It is written down so a later batch that meets the same
        // ±1 DN wash on these probe negatives checks for THIS before reading
        // it as a signal.
        // The sibling `feather` HAD the same guessing bug — Lightroom writes it
        // 0..100 and xmp.rs used to import the value raw, so Feather="72"
        // clamped to fully feathered; both XMP directions now convert on the
        // boundary (xmp.rs). Test radial_roundness_is_a_documented_no_op pins
        // the roundness no-op, now as the measured behaviour.
        // `midpoint: _` joins it for the same reason (R25 P5): Lightroom's
        // second falloff knob, carried through the recipe and the sidecar
        // unchanged, with no published mapping onto this engine's `feather`.
        // `mask_version: _` is Lightroom's own schema stamp and has no pixel
        // meaning at all. Both are spelled out rather than swept into `..` so
        // a field added to the geometry cannot reach the renderer unnoticed.
        MaskGeometry::Radial {
            top, left, bottom, right, feather, roundness: _, flipped, angle, midpoint: _,
            mask_version: _,
        } => {
            let cx = (left + right) / 2.0;
            let cy = (top + bottom) / 2.0;
            let rx = ((right - left) / 2.0).abs().max(1e-4);
            let ry = ((bottom - top) / 2.0).abs().max(1e-4);
            // Rotation (engine convention, recipe.rs `MaskGeometry::Radial`):
            // rotate the SAMPLE POINT about the bbox centre by −angle, in
            // normalised frame coords — equivalent to rotating the ELLIPSE by
            // +angle, which on screen (x right, y DOWN) is CLOCKWISE.
            // MEASURED, not derived: a synthetic one-radial recipe rendered by
            // the released binary at angle +30 puts the darkest row at
            // x = 150 → 450 at rows 147 → 249, i.e. the band descends — right
            // end DOWN — and at −30 it ascends (E1-verdict §4a, R25 P9).
            // The matrix below is `R(+θ)` applied to the point, which IS
            // counter-clockwise in a y-UP maths frame; this comment used to
            // carry that reading while explicitly claiming the y-down screen
            // sense, so it named the direction backwards. Code unchanged.
            let (mut px, mut py) = (nx - cx, ny - cy);
            if *angle != 0.0 {
                let (s, c) = (-angle.to_radians()).sin_cos();
                (px, py) = (px * c - py * s, px * s + py * c);
            }
            let d = ((px / rx).powi(2) + (py / ry).powi(2)).sqrt();
            // The falloff is Lightroom's MEASURED α(ρ), read out of the eleven
            // measured columns plus the analytic f = 0 edge — see
            // `radial_falloff` for the table, its provenance, and the three
            // successive laws it buries. NOTHING else on this arm
            // moves: the frame adaptation (`mask_weight_in`), the rotation
            // convention above, the sample point, and the `flipped` polarity
            // below all predate this batch and keep their own pins.
            //
            // The history, because it took four rounds of measurement:
            //
            // * v0.32.0 replaced `d_out = 1` — the effect reaching zero exactly
            //   ON the ellipse — with `d_out = 1 + f/2`, recovered from an
            //   11-rung exposure ladder over five frames spanning aspect
            //   1.03 … 7.46. That the OUTER boundary moves with feather at all
            //   was the load-bearing find (the sourced claim said only the inner
            //   one does), and the old edge was a 29 % under-sized mask at
            //   Feather 50 (`PROBE3-ADDENDUM.md` §3.1). The LUT keeps that half.
            // * R27 Batch-8/10 then refuted BOTH endpoints as written: `d_out`
            //   appeared to SATURATE near 1.41 rather than reach 1.5, and `d_in`
            //   read `0.79 − 0.94 f`, negative at f = 1. Left standing at the
            //   time — a two-branch replacement needed its own adjudication.
            // * R29 B7 found out why, with the nomask reference both earlier
            //   batches lacked: both readings were artefacts of forcing ONE
            //   smoothstep across a profile that is not one. `d_out` is CONSTANT
            //   in feather, and α(0) = 1 at every feather, so there is no inner
            //   knee to fit in the first place.
            // * R29 B7-2 supplied the f = 1/5/10/90 rungs and closed it: the
            //   f ∈ (0, 25) opening is continuous, no closed form reaches the
            //   measurement floor, and the shipped ramp was already RIGHT for
            //   f ≤ 5. That last one is why `radial_falloff` degenerates to an
            //   EXACT hard edge at f = 0 rather than to a table row.
            // * R29 me3 measured f = 15/35/65 and INSERTED them as columns
            //   rather than trusting the interpolation between the old ones,
            //   which was reading 14.8 px and 24.9 px wide on the α = 0.5
            //   contour at f = 15 and f = 35.
            //
            // ⚠ RENDER-BEHAVIOUR CHANGE. Every radial mask with feather ≥ 10
            // renders differently from this version on — at Feather 100 the
            // α ≥ 0.5 region was 2.08× Lightroom's and its α = 0.5 contour sat
            // 239 px out on the measured frame's major axis. f = 1 and f = 5
            // move too, by the old law's own residual there (rms 0.009–0.010 in
            // α, concentrated in the annulus within a few percent of ρ = 1), and
            // f = 0 is byte-identical: it takes the analytic branch, which
            // reproduces the old degenerate `ramp` exactly. The me3 insertion
            // moves f ∈ (10, 75) a SECOND time, relative to the eight-column
            // table itself: on a column (15/35/65) by that column's own
            // held-out error, between columns by the interpolation now running
            // through measured neighbours. f ≤ 10, f = 25, f = 50 and f ≥ 75
            // are untouched by it — those columns carried over bit for bit.
            let wgt = radial_falloff(*feather, d);
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
        // Brush group: RENDERED since R29 Batch-6b, from a MEASURED model —
        // sampled exactly like a raster mask, because by the time it reaches
        // here it IS one. `brush_raster` stamps the dab stream into an 8-bit
        // grey alpha at develop time (a render-time artefact: no schema field,
        // no `schema_era` gate, nothing in `recipe.json` moved) and the caller
        // hands it in through the same `bmp` slot `Bitmap` and `AiMask` use.
        //
        // `None` is the SAME inert contract an unreadable raster gets, and it
        // has exactly two causes: the group yielded no drawable dab (an empty
        // or state-token-only stream, a zero radius, flow 0 — Lightroom draws
        // nothing there either), or the caller never asked for a raster (the
        // unit tests below, which assert `mask_weight`'s meaning at a stored
        // coordinate). Inert is also inert in both compositions — an `Add`
        // component folds in as `1−(1−w)(1−0) = w` and a `Subtract` as
        // `w·(1−0) = w`.
        //
        // THE MODEL, and every number in it is measured, not chosen (R29
        // Batch-6, `~/.claude/plans/r29-materials/b6-analysis.md`; the two
        // laws re-confirmed out-of-sample against R27 Batch-8/10):
        //
        //     ρ       = |p − dab| / (Radius·W)          dab is a circle in PIXELS
        //     k(ρ;h)  = (1 − ρ^m(h))^n(h)               0 ≤ ρ < 1, else exactly 0
        //     D(f)    = κf / (1 − f + κf)               κ = 0.1284, D(1) = 1 exact
        //     α       = 1 − Π_i (1 − value·D(f_i)·k(ρ_i; h_i))        SCREEN
        //
        // with `ln m(h)` and `ln n(h)` cubics in the hardness (§4.4). See
        // `brush_kernel` / `brush_flow_deposit` for the coefficients and their
        // provenance, and `brush_kernel_matches_the_measured_nine_rungs` /
        // `brush_flow_law_matches_the_measured_deposit` for the pins.
        //
        // WHAT THIS REPLACES, and it is a HARD render-behaviour change: this
        // arm answered `0.0` for every pixel from R27 Batch-4 until now, so a
        // brush-only correction rendered NOTHING while the sidecar carried it
        // whole. Every recipe holding a brush mask therefore renders
        // DIFFERENTLY from here on — that is the point, and it is why the
        // disclosure variants moved with it (`MaskImportReason::BrushRendered`
        // / `MaskLossReason::BrushRendered`, which now say「drawn from our
        // measured model, not Adobe's rasteriser」rather than「not drawn」).
        //
        // The frame half was closed first, by R29 Batch-3. Lightroom rasterises
        // a brush in its pre-lens-correction frame, and so does this engine:
        // `apply_masks` runs before `apply_lens_geometry` and the geometry
        // stage carries the mask exactly as it carries the photograph. So the
        // dab coordinates are ALREADY in the right frame and get no warp — see
        // the mask-warp block header for the measurements and the frame table,
        // and `the_engine_evaluates_masks_before_the_geometry_stage` for the
        // pin. Applying a warp here would apply the field TWICE.
        //
        // Production routing is the explicit `mask_weight_in` match over
        // `MaskFrame`: the RADIAL arm uses `MaskUnwarp::at`, the LINEAR arm uses
        // `MaskUnwarp::engine_at`, and this brush arm stays on its stored point.
        // `is_lr_post_correction_geometry` is retained only as a historical
        // classification assertion in tests.
        //
        // The two group-level fields are spelled out rather than swept into
        // `..`, the same discipline `Radial` follows, so a field added to the
        // geometry cannot reach the renderer unnoticed:
        //
        //  * `inverted` (`crs:MaskInverted` on the Aggregate) IS rendered —
        //    `1 − α` — but only where an alpha exists. Measured `true` on 1 of
        //    39 real groups (F2 anatomy). It is deliberately NOT lifted into
        //    `LocalAdjustment::inverted` at import (xmp.rs: one bit, one home),
        //    so this arm is the only place it can be honoured at all.
        //  * `value` (`crs:MaskValue` on the Aggregate) is NOT a strength and
        //    must never scale anything here: it is the other half of the
        //    subtract pair, measured `(blend_mode, value)` as `(1, 0)` ×23 and
        //    `(0, 1)` ×16, and reading it as a density neutralises every
        //    subtract brush in the library (`recipe::MaskGeometry::Brush`).
        //    The per-STROKE `BrushStroke::value` is the genuine density, and it
        //    scales each dab BEFORE the screen, inside `brush_raster`.
        MaskGeometry::Brush { inverted, name: _, blend_mode: _, value: _, strokes: _ } => {
            match bmp {
                Some(b) => {
                    let w = sample_gray_norm(b, nx, ny);
                    if *inverted { 1.0 - w } else { w }
                }
                // No alpha = no coverage, and NO inversion either. `1 − 0` here
                // would turn a group that drew nothing into a WHOLE-FRAME
                // adjustment at full strength — the identical failure
                // `is_raster_backed` exists to keep a dead bitmap out of, said
                // locally because a brush needs no file and so cannot use that
                // gate (a dab-less group is inert, not broken).
                None => 0.0,
            }
        }
        // AI mask: the RECOMPUTED alpha, sampled exactly like a raster mask —
        // because that is what it is. `segment::resolve_ai_masks` runs our own
        // segmenter at the reference point the sidecar names and caches the
        // 8-bit grey PNG beside the develop; `raster: None` (not resolved yet,
        // or the model declined) takes the same inert path a `Bitmap` with an
        // unreadable file takes, and both disclosure channels say so
        // (`MaskImportReason::AiMaskRecomputed` / `MaskLossReason::…`).
        //
        // The honest sentence, which the disclosures also carry: these pixels
        // are NOT Adobe's. The sidecar holds no raster, so the alpha here comes
        // from a different model with a different edge behaviour — an
        // approximation of the photographer's intent, never a reproduction of
        // their mask.
        MaskGeometry::AiMask { .. } => match bmp {
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
        || m.sharpness != 0.0
        || m.saturation != 0.0
        || m.hue != 0.0
        || m.temperature != 0.0
        || m.tint != 0.0
        || m.noise_reduction != 0.0
        // The four local point curves (R25 P6). Empty = identity, exactly as
        // `apply_develop`'s own `tone_neutral` reads the global curves — a
        // non-empty curve is an edit even if its points happen to trace the
        // diagonal, which is the same latitude the global stage takes.
        || !m.main_curve.is_empty()
        || !m.red_curve.is_empty()
        || !m.green_curve.is_empty()
        || !m.blue_curve.is_empty()
        || m.color_gains.is_some_and(|g| g != [1.0, 1.0, 1.0])
}

#[derive(Debug, Default)]
struct MaskRasterSnapshot {
    images: std::collections::HashMap<String, std::sync::Arc<image::GrayImage>>,
}

impl MaskRasterSnapshot {
    fn get(&self, geometry: &MaskGeometry) -> Option<&image::GrayImage> {
        self.images.get(geometry_raster_path(geometry)?).map(std::sync::Arc::as_ref)
    }
}

/// The raster FILE one geometry renders from, if it currently has one — the
/// ONE place "where does this geometry's pixels live" is answered.
///
/// Two carriers since R27 Batch-5: an explicit [`MaskGeometry::Bitmap`], and a
/// [`MaskGeometry::AiMask`] whose alpha `segment::resolve_ai_masks` has already
/// recomputed. `None` for an AiMask means *not resolved yet, or the segmenter
/// declined* — which is a real state, and [`is_raster_backed`] is the question
/// that separates "has no raster" from "needs no raster".
pub(crate) fn geometry_raster_path(g: &MaskGeometry) -> Option<&str> {
    match g {
        MaskGeometry::Bitmap { path } => Some(path.as_str()),
        MaskGeometry::AiMask { raster, .. } => raster.as_deref(),
        _ => None,
    }
}

/// Does this geometry draw from a raster at all?
///
/// Distinct from `geometry_raster_path(g).is_some()`, and the distinction is
/// the whole point: an AI mask with no resolved alpha answers `true` here and
/// `None` there, which is exactly the "this mask NEEDS pixels and has none"
/// state the weight loop must SKIP rather than render at weight 0 — a 0 under
/// `inverted` applies the adjustment to the entire frame.
pub(crate) fn is_raster_backed(g: &MaskGeometry) -> bool {
    matches!(g, MaskGeometry::Bitmap { .. } | MaskGeometry::AiMask { .. })
}

/// `diag` rides along ONLY to carry the loader's warnings to the caller,
/// attributed to the photograph they belong to (R28 Batch-5 5c stamped them;
/// R29-1 routes them) — a `batch --jobs 3` interleaves three photos' warnings
/// on one stderr in completion order, and "mask raster '…/mask-1.png' is inert"
/// names a file inside a hashed store directory, not a picture the
/// photographer can find. It changes nothing about what loads.
fn load_mask_raster_snapshot(
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
) -> Result<MaskRasterSnapshot> {
    load_mask_raster_snapshot_with_budget(recipe, MASK_RASTER_BUDGET_BYTES, true, diag)
}

/// The PREVIEW arm — the one place that genuinely has no photograph, and since
/// R29-1 the one place that SAYS SO in the type.
///
/// `apply_develop` is handed pixels, a width and a height. Under 5c this arm
/// passed a bare `None` and the registration admitted what that cost: `None`
/// meant "I have no photo" here and "the caller did not bother" three call
/// sites away, and neither the destination nor the order of the resulting
/// stderr line could be chosen by anyone but the process (adjudication F6). It
/// now takes the caller's `Diag`, whose subject is
/// [`crate::diag::Subject::PixelOnly`] when the pixels really are anonymous —
/// a state a sink can match on rather than a missing value it has to guess at.
/// The interactive surfaces this arm serves still have a window and a mask list
/// to say it in ([`dead_bitmap_rasters`]); that remains the better channel
/// there, and a GUI is now free to select it.
fn best_effort_mask_raster_snapshot(
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
) -> MaskRasterSnapshot {
    load_mask_raster_snapshot_with_budget(recipe, MASK_RASTER_BUDGET_BYTES, false, diag)
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
            if !is_raster_backed(g) {
                return None;
            }
            // An AI mask with NO resolved alpha is dead in exactly the sense
            // this list means — the row must say so — and it has no path to
            // name, so it names the intent instead (R27 Batch-5).
            let name = geometry_raster_path(g)
                .map(str::to_string)
                .unwrap_or_else(|| "AI mask (not yet recomputed)".to_string());
            // GUI mask list, no photograph in scope and none needed: this is
            // a UI probe, and the row it feeds IS the disclosure. So the
            // channel is DROPPED, deliberately and in the type (R29-1) — the
            // loader's stderr copy would fire once per frame per mask and say
            // nothing the row does not already say. Under 5c this was a `None`
            // that only suppressed the STEM; the line still printed.
            load_mask_bitmap(g, &crate::diag::dropped()).is_none().then_some(name)
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
                .filter_map(move |g| geometry_raster_path(g).map(|p| (m, g, p)))
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
    diag: &crate::diag::Diag<'_>,
) -> Result<MaskRasterSnapshot> {
    let mut snapshot = MaskRasterSnapshot::default();
    let mut held_bytes = 0usize;
    // Every disclosure below is ungated and worker-reachable, so each travels
    // the caller's channel carrying its subject (R28 Batch-5 5c stamped them;
    // R29-1 routes them). The `bail!`s do not: an error message travels up a
    // `Result` to a caller that names the photo itself.
    //
    // `Mark::Bare`: these three have never worn the ⚠ glyph, and the default
    // sink reproduces that. The mark is data precisely so a sink rendering
    // into a per-photo block can drop it.

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
                diag.emit(
                    crate::diag::Mark::Bare,
                    format!(
                        "mask raster '{path}' skipped: the active raster set exceeds the \
                         {budget_bytes}-byte aggregate budget"
                    ),
                );
                continue;
            }
        }
        let Some(bitmap) = load_mask_bitmap(geometry, diag) else {
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
            diag.emit(
                crate::diag::Mark::Bare,
                format!(
                    "mask raster '{path}' skipped: the active raster set exceeds the \
                     {budget_bytes}-byte aggregate budget"
                ),
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
            diag.emit(
                crate::diag::Mark::Bare,
                format!(
                    "mask raster '{path}' skipped: the active raster set exceeds the \
                     {budget_bytes}-byte aggregate budget"
                ),
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
///
/// `unwarp` is the frame adaptation ([`MaskFrame`]) — applied PER COMPONENT,
/// through [`mask_weight_in`], because a correction can hold a post-correction
/// radial and a pre-correction brush side by side and each must be asked about
/// in its own frame. Folding the map in at this level instead would have moved
/// the brush too.
fn combined_mask_weight(
    m: &crate::recipe::LocalAdjustment,
    nx: f32,
    ny: f32,
    base: Option<&image::GrayImage>,
    comp_bmps: &[Option<&image::GrayImage>],
    unwarp: Option<&MaskUnwarp>,
    dims: (f32, f32),
) -> f32 {
    let mut w = mask_weight_in(&m.mask, nx, ny, base, unwarp, dims);
    for (c, bmp) in m.components.iter().zip(comp_bmps) {
        let cw = mask_weight_in(&c.geometry, nx, ny, *bmp, unwarp, dims);
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
///
/// `diag` carries those two warnings to the caller with the photograph they
/// belong to (R28 Batch-5 5c stamped them; R29-1 routes them), and carries ONE
/// caveat worth stating: the negative result is CACHED, so the warning fires
/// once per (path, mtime) — on the channel, and with the subject, of whoever
/// hit it FIRST. A mask raster lives in its own photo's develop directory, so
/// in practice that is the only photo it can belong to; two recipes pointing at
/// one raster would see the first name stick.
///
/// R29-1 sharpened that caveat rather than removing it: the GUI mask list's
/// `dead_bitmap_rasters` probe now passes a SILENT channel (its row is the
/// disclosure), so if the probe reaches a dead raster first, the console line
/// for that (path, mtime) is the one that does not print. That is the intended
/// trade — the alternative (warn per call) is the per-tick flood this cache
/// exists to stop, and the surface that suppressed it is the surface that
/// already shows the fact.
fn load_mask_bitmap(
    g: &MaskGeometry,
    diag: &crate::diag::Diag<'_>,
) -> Option<std::sync::Arc<image::GrayImage>> {
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
    let path = geometry_raster_path(g)?;
    let cache = CACHE.get_or_init(Default::default);
    let ident: Key = std::fs::metadata(path)
        .ok()
        .map(|m| (m.modified().ok(), m.len()));
    {
        // No user code runs under the lock, so poisoning is not reachable —
        // recover anyway rather than turning a past panic into a new one.
        let map = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cached_t, img)) = map.get(path)
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
    let over_budget = image::ImageReader::open(path)
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_some_and(|(w, h)| {
            (w as usize).saturating_mul(h as usize).saturating_mul(4) > MASK_RASTER_BUDGET_BYTES
        });
    let decoded = if over_budget {
        diag.warn(format!(
            "bitmap mask '{path}' exceeds the {MASK_RASTER_BUDGET_BYTES}-byte mask budget — mask is inert"
        ));
        None
    } else {
        match image::open(path) {
            Ok(img) => Some(Arc::new(img.to_luma8())),
            Err(e) => {
                diag.warn(format!(
                    "bitmap mask '{path}' could not be loaded ({e}) — mask is inert"
                ));
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
            map.insert(path.to_string(), (ident, decoded.clone()));
        } else {
            map.clear();
        }
    }
    decoded
}

/// Bilinear weight lookup in an 8-bit greyscale mask at normalised (nx, ny).
pub(crate) fn sample_gray_norm(b: &image::GrayImage, nx: f32, ny: f32) -> f32 {
    let (w, h) = (b.width() as f32, b.height() as f32);
    // EXTENT scaling (`* w`), not endpoint scaling (`* (w - 1)`): a texel owns
    // a SLICE of the frame, `[i/w, (i+1)/w]`, so mapping onto 0..=size-1 here
    // was a DIFFERENT convention. A frame-sized mask then never reached its
    // last row/column — a 2-wide mask holding [0,255] rendered [0, 0.5]
    // instead of [0, 1] — and because the shortfall is one source pixel out of
    // `w`, the same mask landed differently in a 1280 px preview than in a
    // 9504 px export.
    //
    // The `− MASK_SAMPLE_CENTRE` is the other half of that slice reading, and
    // it is what makes this the TEXEL-CENTRE lookup its producers stamp for
    // (R29 C2): texel `i` owns `[i/w, (i+1)/w]`, so its centre is the
    // normalised `(i + 0.5)/w` and the texel coordinate of a normalised `nx`
    // is `nx·w − 0.5`. Bilinear then interpolates between the two texel
    // CENTRES that bracket the sample, which is the only reading under which a
    // raster and the frame it covers agree about where a given physical point
    // is. Without it the interpolation is anchored on texel top-left corners
    // and every raster mask sits half a texel out.
    //
    // The exactness the old comment claimed is KEPT, and by construction: a
    // frame-sized raster read from a frame loop that also samples at pixel
    // centres gives `nx·w − 0.5 = (x + 0.5) − 0.5 = x`, an exact texel hit
    // with no interpolation at all. `rasterise_brush_group` sizes and stamps
    // its raster to land on this same grid.
    //
    // Clamping to `0 ..= size-1` is clamp-to-edge, and it is what the outer
    // half-texel band on each side gets: a sample at `nx = 0` asks for texel
    // −0.5, which is outside the first texel's centre, and the honest answer
    // for a mask that says nothing beyond its own edge is the edge value.
    let sx = (nx.clamp(0.0, 1.0) * w - MASK_SAMPLE_CENTRE).clamp(0.0, w - 1.0);
    let sy = (ny.clamp(0.0, 1.0) * h - MASK_SAMPLE_CENTRE).clamp(0.0, h - 1.0);
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

// --- Lightroom's brush, rasterised ------------------------------------------
//
// Everything from here to `brush_raster` is ONE measurement made executable:
// R29 Batch-6 (`~/.claude/plans/r29-materials/b6-analysis.md`), 29 controlled
// Lightroom exports on one capture — a nine-rung hardness ladder (class 07,
// Δh = 0.125, one dab at the exact frame centre) and a 5 × 2 × 2 flow × radius
// × hardness grid of drags (class 06) — read back through batch-10's `par`
// composition law and un-warped into Lightroom's own pre-correction frame.
//
//     ρ       = |p − dab| / (Radius·W)          a dab is a circle in PIXELS
//     k(ρ;h)  = (1 − ρ^m(h))^n(h)               0 ≤ ρ < 1, else exactly 0
//     D(f)    = κf / (1 − f + κf)               κ = 0.1284 ± 0.0029, D(1) = 1
//     α       = 1 − Π_i (1 − value·D(f_i)·k(ρ_i; h_i))        SCREEN
//
// NOTHING here is a schema field. The raster is a render-time artefact keyed by
// the dab stream and the frame size, exactly as a `Bitmap` mask's decode is
// keyed by (path, mtime, size) — `recipe.json` gained no member and no
// `schema_era` gate moved.

/// Kernel exponents `(m, n)` of `k(ρ;h) = (1 − ρ^m)^n` at hardness `h`
/// (`crs:CenterWeight`, and the dab stream's `h` token — one quantity, two
/// spellings, agreeing to 4 dp on the census).
///
/// **Measured, not chosen** (B6 §4.4). Nine rungs, each fitted independently
/// with a profile-likelihood interval 5–15 % wide, so both parameters are
/// separately IDENTIFIED at every rung — which is what batch-10's rival
/// `disc ⊛ gaussian` was not (its disc radius jumped 0.002 → 0.503 between
/// adjacent rungs). `ln m` and `ln n` are then cubics in `h` fitted against all
/// nine at once: 8 numbers total, pooled rms 0.0102, **held-out 0.0109** (fit 5
/// rungs, predict the 4 interleaved ones) against 0.0180 for interpolating the
/// measured table itself. That is why this ships as a FORM and not as a table.
///
/// The physical reading (B6 §4.5): re-parametrised as `exp(−cρ^m)` the exponent
/// runs 1.95–2.15 at `h ≤ 0.25` — an actual gaussian — and 20.8 at `h = 1`, a
/// super-gaussian barely distinguishable from a top-hat. Hardness is the ORDER
/// of the falloff, not a plateau radius.
///
/// `h` is CLAMPED to Lightroom's own 0..1 before the cubics are evaluated. They
/// are empirical fits over exactly that interval and diverge outside it — at
/// `h = 2` they return `m = 5×10⁻⁴`, which would paint the whole frame. The
/// clamp is the guard, and it is the only one needed: on [0, 1] the cubics are
/// bounded at `m ∈ [1.66, 18.0]`, `n ∈ [1.40, 6.43]`.
///
/// Evaluated in f64 and returned as f32: the cubic coefficients carry six
/// decimals and `exp` of a sum near 2.9 loses the last of them in f32.
fn brush_kernel_exponents(h: f32) -> (f32, f32) {
    // NaN clamps to NEITHER end — `f32::clamp` propagates it — and a NaN
    // exponent turns the whole raster into `0 as u8` further down, silently.
    // A hand-edited `recipe.json` is the only way to get one, and Lightroom's
    // own default `CenterWeight` is what it degrades to.
    let h = if h.is_nan() { 0.0 } else { f64::from(h).clamp(0.0, 1.0) };
    // Horner, from B6 §4.4:
    //   ln m(h) = −5.272996 h³ + 9.420690 h² − 1.864655 h + 0.605303
    //   ln n(h) = −5.976385 h³ + 12.872100 h² − 7.987943 h + 1.861162
    let ln_m = ((-5.272996 * h + 9.420690) * h - 1.864655) * h + 0.605303;
    let ln_n = ((-5.976385 * h + 12.872100) * h - 7.987943) * h + 1.861162;
    (ln_m.exp() as f32, ln_n.exp() as f32)
}

/// One dab's alpha profile: `k(ρ; h)`, the falloff at normalised radius `ρ`.
///
/// **Support ends EXACTLY at ρ = 1**, re-confirmed at nine hardnesses: α is
/// ≤ 5×10⁻⁴ in magnitude at every ρ ≥ 1.002 on every rung, so `crs:Radius` is
/// the outer support and not a half-width (B6 §4.1). Returning a hard 0 there
/// is therefore the measurement, not a convenience — and it is what keeps a
/// dab's cost proportional to its own area instead of the frame's.
///
/// `k(0) = 1` is likewise measured, not normalised in by hand: the
/// un-normalised peak reads 1.00288 ± 0.00229 across the nine rungs with no
/// trend in `h`, which is also an independent confirmation of `D(1) = 1`.
///
/// TESTS ONLY, and deliberately: the rasteriser hoists `brush_kernel_exponents`
/// out of its inner loop, so composing them per call is a convenience no pixel
/// path wants. The falloff ITSELF is [`brush_kernel_at`], which both this and
/// the rasteriser go through — so a mutation to the closed form still reaches
/// production, and the nine-rung pin still bites it.
#[cfg(test)]
fn brush_kernel(rho: f32, h: f32) -> f32 {
    let (m, n) = brush_kernel_exponents(h);
    brush_kernel_at(rho * rho, m * 0.5, n)
}

/// [`brush_kernel`] with ρ² in hand and the exponents already resolved — the
/// form the rasteriser's inner loop wants, and the ONE place the falloff is
/// actually computed, so the pinned closed form and the pixels cannot drift.
///
/// `ρ^m = (ρ²)^(m/2)`: one exponentiation per texel, and no square root at all.
fn brush_kernel_at(rho2: f32, half_m: f32, n: f32) -> f32 {
    // NaN takes this branch too, deliberately: a NaN weight survives
    // `wgt <= 0.001` and casts to black (the same trap the radial arm's guarded
    // ramp documents), and an unreachable NaN is still a NaN once someone
    // hand-edits a `recipe.json`.
    if rho2.is_nan() || rho2 >= 1.0 {
        return 0.0;
    }
    if rho2 <= 0.0 {
        return 1.0;
    }
    (1.0 - rho2.powf(half_m)).max(0.0).powf(n)
}

/// The per-dab DEPOSIT at flow `f` — how much alpha one stamp lays down before
/// the screen accumulation folds the stamps together.
///
/// **A one-parameter ODDS law**, `D/(1−D) = κ·f/(1−f)` (B6 §5.3). Identified,
/// not fitted: over the sixteen unsaturated cells it scores rms 0.0030 against
/// 0.0207 for `1−(1−f)ⁿ` (6.9×) and 0.0383 for the naive linear `D = κf`
/// (12.7×), and giving the law a second free exponent returns n = 1.024 and
/// buys 0.0004. κ is UNIVERSAL to 2.24 % across a 3× radius change and both
/// hardness ends (0.12496 / 0.12804 / 0.12752 / 0.13293).
///
/// `D(1) = 1` is EXACT and free — `κ/(1−1+κ)` — which is also exact in f32
/// here, so a flow-1 dab deposits its full density with no epsilon. Pinning it
/// at 1 rather than fitting it costs at most 0.0037 on the four saturated
/// frames, below the α quantum (B6 §5.2).
///
/// **Registered wart, not swept** (B6 §5.4, §9): the per-rung κ rises ~11 %
/// from f = 0.10 to f = 0.75 in all four cells, so the law carries a small real
/// curvature no better one-parameter form absorbed. It is also why this κ
/// (four cells, 0.1284 ± 0.0029) sits 5.3 % above batch-10's single-cell
/// 0.12189 ± 0.00270 — about 1.6σ. **The two must not be quoted as agreeing to
/// better than 5 %.**
fn brush_flow_deposit(f: f32) -> f32 {
    /// B6 §5.4, four cells: 0.12836 ± 0.00288. Supersedes batch-10's 0.12189.
    const KAPPA: f32 = 0.1284;
    let f = f.clamp(0.0, 1.0);
    let kf = KAPPA * f;
    let denom = 1.0 - f + kf;
    // Unreachable on [0, 1] — `denom ≥ κ > 0` — but a division that can only be
    // reasoned about is a division that eventually is not.
    if denom <= 0.0 { 1.0 } else { (kf / denom).clamp(0.0, 1.0) }
}

/// One stamp, fully resolved out of the token stream — position and radius in
/// the stored normalised frame, and the deposit and falloff in force when the
/// `d` token was reached.
///
/// The state tokens are resolved HERE and not in the pixel loop: `a` folds the
/// stroke density into `D(flow)` and `(half_m, n)` folds the hardness through
/// [`brush_kernel_exponents`], so the rasteriser's per-texel work is one
/// exponentiation and a multiply, and its per-(row, dab) work is a bounds test.
#[derive(Clone, Copy)]
struct BrushDab {
    /// `d <x>` — fraction of the frame WIDTH.
    x: f32,
    /// `d <y>` — fraction of the frame HEIGHT.
    y: f32,
    /// The current `r`, in WIDTH units on BOTH axes (a dab is a circle in
    /// pixels, so the y half-extent is `r·W/H` in normalised coordinates).
    r: f32,
    /// `BrushStroke::value · D(flow)` — the density-scaled deposit. Density
    /// scales the DAB, pre-screen (R27 Batch-8: the rival `min(MaskValue, ·)`
    /// cap reading is refuted at 13×).
    a: f32,
    /// `m(h)/2`, ready for the `ρ^m = (ρ²)^(m/2)` form.
    half_m: f32,
    /// `n(h)`.
    n: f32,
}

/// Total dabs one brush group may stamp. Four times the ENTIRE reference
/// library (15,964 dabs over 382 components) and ~100× its largest single
/// stroke (645). It is not reachable from a sidecar — `xmp::parse_dabs` caps a
/// stroke at 65,536 TOKENS and `recipe::clamp_strings` caps its stream at
/// 256 KiB — so only a hand-written `recipe.json` can meet it, and what it buys
/// is a hard ceiling on the rasteriser's work that does not depend on the
/// resolution search below.
const BRUSH_MAX_DABS: usize = 65_536;

/// Long edge of a brush alpha raster, before the work budget. Batch-10 §7.4's
/// own figure: at 2048 the 8-bit alpha quantum is 1/255 = 0.0039, which is
/// below the 0.0085 measurement quantum the whole model was fitted against, and
/// a dab's own transition width is ~5 raster px even at h = 1 (m = 18, so the
/// 10–90 % edge spans Δρ ≈ 0.05).
///
/// A raster is never UPSCALED past the frame it serves, so a 1280 px preview
/// gets a 1280 px raster and `sample_gray_norm`'s extent scaling then makes the
/// lookup an exact texel hit with no interpolation at all. Only a full-res
/// export downsamples (9504 → 2048, 4.6×), and what that costs is edge
/// sharpness on a dab far smaller than the ones this model was measured on.
const BRUSH_RASTER_MAX_EDGE: u32 = 2048;

/// Floor for the same edge. Below this the raster stops describing the mask at
/// all, so the work budget is allowed to be exceeded rather than the mask
/// destroyed — and `BRUSH_MAX_DABS` is what bounds the overrun.
const BRUSH_RASTER_MIN_EDGE: u32 = 32;

/// Kernel evaluations one group may spend, which is what actually sets the
/// raster's resolution for a heavy stroke.
///
/// The work is `Σ_dabs min(π·(r·W_r)², W_r·H_r)` — each dab pays for its own
/// disc, clipped to the raster — and it scales as `s²` with the raster scale,
/// so the largest `s` meeting this budget is a closed form, not a search.
///
/// **The budget SELF-BALANCES, which is why one constant is enough.** A dab's
/// cost grows as `r²` while the resolution it NEEDS grows as `1/r`: coarsening
/// only ever blurs the dabs that were cheap. A 645-dab stroke at r = 0.05 (the
/// library's largest) costs 21 M evaluations and keeps the full 2048 raster; a
/// 645-dab stroke at r = 0.58 would ask for 1.8 G, and the 236 px raster the
/// budget hands it is ample for a mask whose every feature is 0.58 frame-widths
/// across.
///
/// **Sized against a measurement, not a guess.** The reference library's
/// largest real stroke — 645 dabs at r = 0.05 — rasterises for a 9504 × 6336
/// frame in **445 ms in a DEBUG build** on this machine (2048 × 1365 raster,
/// 2.13e7 evaluations, so ~21 ns each wall-clock across the row-parallel loop).
/// Straight-line proportionality puts a group that spends the whole budget at
/// ~0.5 s debug, once per (group, frame size) and then memoised.
///
/// **RELEASE, now measured — and the guess it replaces was wrong (R29 C3/C4).**
/// This line used to end "a release build is several times quicker again, and
/// it is not measured here". It is **~1.3×, not several times**: a synthetic
/// stroke of the same shape (645 dabs, r = 0.05, the same 0.2 r densification,
/// reproducing the documented 2.1e7 evaluations and the same 2048 × 1365
/// raster) rasterises in **416 ms release** (5 builds, 388–426 ms) against
/// **528 ms debug** (5 builds, 504–619 ms) in the SAME harness on the same
/// machine — each build a fresh dab stream, since `brush_raster` memoises on
/// (content, frame) and a repeat of one geometry would time the cache instead.
/// So the budget's headroom is the DEBUG figure's, and sizing this constant
/// against a hoped-for optimiser win would have been sizing it against nothing.
/// `-O` buys little here because the row loop is a bounds test and a multiply
/// over an 11 MB `prod` buffer spread across every core: it is bandwidth-bound,
/// not instruction-bound, which is also why the ratio is stated rather than
/// extrapolated to other machines.
///
/// The `BRUSH_MAX_DABS` × `BRUSH_RASTER_MIN_EDGE` corner — the only way past
/// this number, and reachable only from a hand-written `recipe.json` — bounds
/// out at 44 M evaluations.
const BRUSH_RASTER_MAX_WORK: f64 = 24_000_000.0;

/// Process-wide cache budget for finished brush alphas, in bytes. Small on
/// purpose: at the 2048 edge one raster is 2.8 MB for a 3:2 frame (4.2 MB for a
/// square one), so this holds a handful and hard-resets rather than keeping LRU
/// books — the same trade `load_mask_bitmap`'s cache makes, and the same
/// reasoning, since a recipe holds a handful of masks.
///
/// **The memory accounting, stated rather than assumed.** Peak added by this
/// whole feature is `cache + one build transient` = 16 MB + (4·1 + 1)·4.2 Mpx
/// ≈ 37 MB, against the 256 MB `MASK_RASTER_BUDGET_BYTES` already reserves for
/// a SINGLE bitmap-mask decode. It rides inside that envelope with room to
/// spare and does not move `jobs::PER_PHOTO_PEAK_COMMIT_MB` (1800), which
/// budgets the develop's own f32 planes and never counted mask rasters.
const BRUSH_RASTER_CACHE_BYTES: usize = 16 * 1024 * 1024;

/// The brush raster's size for a frame of `fw × fh`, given `cost` = the
/// kernel evaluations stamping this group would take at FULL frame resolution
/// (`Σ min(π·(r·fw)², fw·fh)`).
///
/// Three bounds, in this order, and the third wins:
///
/// 1. [`BRUSH_RASTER_MAX_EDGE`], never upscaling past the frame itself;
/// 2. [`BRUSH_RASTER_MAX_WORK`] — work goes as `s²`, so the largest scale that
///    fits the budget is `sqrt(budget / cost)`, a closed form and not a search;
/// 3. [`BRUSH_RASTER_MIN_EDGE`], which OVERRIDES the budget: below it the
///    raster stops describing the mask at all, and `BRUSH_MAX_DABS` is what
///    bounds the overrun instead.
///
/// Split out of [`rasterise_brush_group`] so the policy can be asserted at a
/// cost the assertion does not have to pay — exercising the work budget by
/// actually rasterising costs, by construction, exactly the budget.
fn brush_raster_dims(cost: f64, fw: u32, fh: u32) -> (u32, u32) {
    let long = f64::from(fw.max(fh)).max(1.0);
    let by_edge = (f64::from(BRUSH_RASTER_MAX_EDGE) / long).min(1.0);
    let by_work = if cost > 0.0 { (BRUSH_RASTER_MAX_WORK / cost).sqrt() } else { 1.0 };
    let floor = (f64::from(BRUSH_RASTER_MIN_EDGE) / long).min(1.0);
    let scale = by_edge.min(by_work).max(floor);
    (
        (f64::from(fw) * scale).round().max(1.0) as u32,
        (f64::from(fh) * scale).round().max(1.0) as u32,
    )
}

/// `<num>` off a dab token, or `None` — finite only.
fn brush_token_num(it: &mut std::str::SplitWhitespace<'_>) -> Option<f32> {
    it.next().and_then(|t| t.parse::<f32>().ok()).filter(|v| v.is_finite())
}

/// The `crs:Dabs` state machine, run (`recipe::BrushStroke::dabs`). Four token
/// forms and no others — `r <f>` / `f <f>` / `h <f>` set the current state,
/// `d <x> <y>` stamps at it — with the stroke's own attributes as the INITIAL
/// state (measured: 102 components carry no `r` token at all and every one of
/// them has a non-zero `Radius` attribute).
///
/// **Nothing is interpolated.** Lightroom has already densified the polyline at
/// 0.2000·r (15,582 steps, IQR [0.1998, 0.2001], zero pen-lifts), so a renderer
/// stamps exactly the dabs it is given — which is also what makes the screen
/// accumulation's `N_eff` come out flat across the flow ladder (B6 §5.1).
///
/// Malformed tokens are SKIPPED, not refused. The XMP boundary already rejects
/// anything outside the grammar (`xmp::dab_token_is_known`, which is what makes
/// the round trip lossless); this parser also has to survive a hand-edited
/// `recipe.json`, where refusing the stroke would mean a silent whole-mask
/// change and skipping one token means a missing stamp.
fn brush_dabs(strokes: &[crate::recipe::BrushStroke], out: &mut Vec<BrushDab>) {
    for s in strokes {
        let value = s.value.clamp(0.0, 1.0);
        let (mut r, mut f, mut h) = (s.radius, s.flow, s.center_weight);
        for token in s.dabs.split('\n') {
            if out.len() >= BRUSH_MAX_DABS {
                return;
            }
            let mut it = token.split_whitespace();
            match it.next() {
                Some("r") => {
                    if let Some(v) = brush_token_num(&mut it) {
                        r = v;
                    }
                }
                Some("f") => {
                    if let Some(v) = brush_token_num(&mut it) {
                        f = v;
                    }
                }
                Some("h") => {
                    if let Some(v) = brush_token_num(&mut it) {
                        h = v;
                    }
                }
                Some("d") => {
                    let (Some(x), Some(y)) =
                        (brush_token_num(&mut it), brush_token_num(&mut it))
                    else {
                        continue;
                    };
                    let a = value * brush_flow_deposit(f);
                    // A zero radius or a zero deposit stamps nothing — and
                    // Lightroom draws nothing there either, so this is the
                    // model and not a shortcut. Dropping them here is what
                    // lets `rasterise_brush_group` answer `None` for a group
                    // that genuinely has no coverage.
                    if r.is_finite() && r > 0.0 && a > 0.0 {
                        let (m, n) = brush_kernel_exponents(h);
                        out.push(BrushDab { x, y, r, a, half_m: m * 0.5, n });
                    }
                }
                _ => {}
            }
        }
    }
}

/// Stamp one brush group's dab stream into an 8-bit grey alpha for a frame of
/// `fw × fh` pixels, or `None` when the group has no drawable dab.
///
/// **Pre-rasterised, not evaluated per pixel**, and the reason is arithmetic:
/// `mask_weight` runs per pixel and is called by up to five passes per mask, so
/// stamping N dabs inside it would be O(pixels × dabs × passes) — 5×10⁹ kernel
/// evaluations for a 90-dab stroke at 61 MP, before the passes multiply it.
/// Rasterising once costs `Σ` each dab's OWN disc and turns every later lookup
/// into the bilinear read a `Bitmap` mask already pays (batch-10 §7.4).
///
/// The accumulation carries the PRODUCT `Π(1 − a·k)` in f32 and converts once at
/// the end, so the screen law is applied in the form it was measured in and the
/// dab order cannot matter (it does not: the product is commutative, which is
/// itself part of why screen beat `max` by 3.4× and sum-clamp by 1.8×). That
/// commutativity is also what makes the row-parallel loop below sound without a
/// single lock — each row owns its slice, every dab is read-only, and no two
/// threads can disagree about a texel because none of them share one.
///
/// Coordinates are the STORED ones and are not warped — see the `Brush` arm of
/// `mask_weight` and `the_engine_evaluates_masks_before_the_geometry_stage`.
fn rasterise_brush_group(
    strokes: &[crate::recipe::BrushStroke],
    fw: u32,
    fh: u32,
) -> Option<image::GrayImage> {
    if fw == 0 || fh == 0 {
        return None;
    }
    let mut dabs: Vec<BrushDab> = Vec::new();
    brush_dabs(strokes, &mut dabs);
    if dabs.is_empty() {
        return None;
    }
    // The cost of stamping this group at FULL frame resolution: each dab pays
    // for its own disc, clipped to the frame, which is what makes one absurd
    // radius cost a frame and not a universe. Saturating by construction.
    let frame_cells = f64::from(fw) * f64::from(fh);
    let cost: f64 = dabs
        .iter()
        .map(|d| {
            let disc = std::f64::consts::PI * (f64::from(d.r) * f64::from(fw)).powi(2);
            if disc.is_finite() { disc.min(frame_cells) } else { frame_cells }
        })
        .sum();
    let (rw, rh) = brush_raster_dims(cost, fw, fh);
    let (rwf, rhf) = (rw as f32, rh as f32);
    // A dab is a circle in PIXELS, so its y half-extent in normalised
    // coordinates is `r·W/H`. Read off the FRAME, not off the raster: rounding
    // rw and rh independently moves their ratio by up to a texel's worth.
    let aspect = fw as f32 / fh as f32;

    let (rwu, rhu) = (rw as usize, rh as usize);
    let mut prod = vec![1.0f32; rwu * rhu];
    // BY ROW, in parallel. Each row owns its slice and every dab is read-only,
    // so the accumulation needs no synchronisation at all — and because the
    // screen product is commutative, a row's result does not depend on which
    // thread reached it or in what order the dabs are folded in. The scan is
    // over ALL dabs per row (a bounds test each, ~10 flops), which is cheaper
    // than building and holding a per-row index for a list this size.
    prod.par_chunks_mut(rwu).enumerate().for_each(|(j, row)| {
        let jf = j as f32;
        for d in &dabs {
            // Centre and half-extents in TEXEL index space. Texel `i` owns the
            // normalised slice `[i/rw, (i+1)/rw]`, so its CENTRE is
            // `(i + MASK_SAMPLE_CENTRE)/rw` and a dab stored at the normalised
            // `d.x` sits at texel coordinate `d.x·rw − MASK_SAMPLE_CENTRE`.
            // That is the same grid `sample_gray_norm` reads back on, so a
            // same-size raster still costs no interpolation at all — and it is
            // the same grid the frame loop samples in, so the dab lands where
            // Lightroom puts it rather than half a pixel down and right
            // (R29 C2; both halves move, and their derivations are one).
            let (cx, cy) =
                (d.x * rwf - MASK_SAMPLE_CENTRE, d.y * rhf - MASK_SAMPLE_CENTRE);
            let (ex, ey) = (d.r * rwf, d.r * aspect * rhf);
            // A degenerate or overflowed extent has no disc to stamp. NaN is
            // caught by `is_finite`, so the comparisons only ever see a number.
            if !(ex.is_finite() && ey.is_finite()) || ex <= 0.0 || ey <= 0.0 {
                continue;
            }
            let dy = (jf - cy) / ey;
            let dy2 = dy * dy;
            if dy2 >= 1.0 {
                continue; // this row misses the dab entirely
            }
            // Clamped in f64 BEFORE the cast: an absurd hand-edited radius
            // makes these ±inf, and the range is clearer closed than saturated.
            let clamp_i = |v: f32| f64::from(v).clamp(0.0, f64::from(rw - 1)) as u32;
            let half = ex * (1.0 - dy2).sqrt(); // the chord, not the bbox
            let (i0, i1) = (clamp_i((cx - half).ceil()), clamp_i((cx + half).floor()));
            let sx = 1.0 / ex;
            for i in i0..=i1 {
                let dx = (i as f32 - cx) * sx;
                let rho2 = dx * dx + dy2;
                if rho2 >= 1.0 {
                    continue; // support ends EXACTLY at ρ = 1 (B6 §4.1)
                }
                let k = brush_kernel_at(rho2, d.half_m, d.n);
                row[i as usize] *= 1.0 - d.a * k;
            }
        }
    });
    let buf: Vec<u8> = prod
        .iter()
        .map(|p| ((1.0 - p).clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    image::GrayImage::from_raw(rw, rh, buf)
}

/// The brush alpha for one geometry at one frame size, through a process-wide
/// cache — the brush twin of [`load_mask_bitmap`], and cached for the same
/// reason: the GUI re-develops the preview on every slider tick, and stamping a
/// 645-dab stroke per tick would dominate the develop exactly as decoding the
/// segmentation PNG per tick used to.
///
/// `None` for a non-brush geometry (the caller asks about every geometry and
/// lets this one answer), and for a brush group with no drawable dab — which
/// renders inert, which is what Lightroom renders for the same group.
///
/// **The key is `(content hash, stroke count, stream bytes, fw, fh)`.** The
/// frame size belongs in it because a dab is a circle in pixels, so the same
/// stream is a different raster at a different aspect — and because the preview
/// and the export legitimately want different resolutions. The identity is a
/// 64-bit hash reinforced by two structural counts rather than a byte compare:
/// the alternative is holding a second copy of every dab stream (256 KiB per
/// stroke) for the life of the process. That is a weaker identity than a
/// content compare and it is stated as such; `load_mask_bitmap` accepts the
/// same shape of trade with (mtime, size).
fn brush_raster(g: &MaskGeometry, fw: u32, fh: u32) -> Option<std::sync::Arc<image::GrayImage>> {
    use std::hash::{Hash, Hasher};
    use std::sync::{Arc, Mutex, OnceLock};
    let MaskGeometry::Brush { strokes, .. } = g else {
        return None;
    };
    if strokes.is_empty() {
        return None;
    }
    type Key = (u64, usize, usize, u32, u32);
    type Cache = Mutex<std::collections::HashMap<Key, Option<Arc<image::GrayImage>>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut bytes = 0usize;
    for s in strokes {
        // `to_bits`, not the float: f32 is not `Hash`, and the bit pattern is
        // the right identity here anyway — two streams that differ only in the
        // sign of a zero are two different streams to the sidecar writer.
        s.value.to_bits().hash(&mut hasher);
        s.radius.to_bits().hash(&mut hasher);
        s.flow.to_bits().hash(&mut hasher);
        s.center_weight.to_bits().hash(&mut hasher);
        s.dabs.hash(&mut hasher);
        bytes += s.dabs.len();
    }
    let key: Key = (hasher.finish(), strokes.len(), bytes, fw, fh);
    let cache = CACHE.get_or_init(Default::default);
    {
        // No user code runs under the lock, so poisoning is not reachable —
        // recover anyway rather than turning a past panic into a new one.
        let map = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(hit) = map.get(&key) {
            return hit.clone();
        }
    }
    let built = rasterise_brush_group(strokes, fw, fh).map(Arc::new);
    {
        let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
        let held: usize =
            map.values().filter_map(|i| i.as_ref()).map(|i| i.as_raw().len()).sum();
        let incoming = built.as_ref().map_or(0, |i| i.as_raw().len());
        // A rare hard reset beats LRU bookkeeping for a handful of entries —
        // `load_mask_bitmap`'s cache makes the same call, in bytes as well as
        // entries, and for the same reason. The entry bound is what bounds the
        // `None` results (no drawable dabs): they weigh zero bytes, so without
        // it a stream of unique non-drawing groups would grow the map forever.
        if map.len() > 16 || held.saturating_add(incoming) > BRUSH_RASTER_CACHE_BYTES {
            map.clear();
        }
        map.insert(key, built.clone());
    }
    built
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
///
/// `frame` must be the SAME [`MaskFrame`] the caller will use on the pixels
/// this overlay is painted over. The GUI's overlay applies
/// [`apply_lens_geometry`] to the coverage raster so the red wash follows the
/// rendered pixels; with the R29 Batch-3 wiring the render now puts a
/// parametric mask back on its STORED coordinates, so an overlay built without
/// the same adaptation would advertise coverage the render does not apply —
/// by the full field, up to 186 px at 24 mm.
pub fn mask_coverage(
    m: &crate::recipe::LocalAdjustment,
    reference: &DynamicImage,
    frame: MaskFrame<'_>,
) -> image::GrayImage {
    let rgb = reference.to_rgb8();
    let (w, h) = rgb.dimensions();
    // A muted (eye-toggled) mask applies nothing — advertise nothing.
    if !m.enabled {
        return image::GrayImage::new(w, h);
    }
    // Same one-time H2 preparation as `apply_masks`; keeping it above the
    // pixel loop makes the overlay and render share both topology and metric.
    let framed_mask = frame.linear_handles_to_raw(m, (w as f32, h as f32));
    let m = framed_mask.as_ref();
    let unwarp = frame.unwarp((w as f32, h as f32));
    let unwarp = unwarp.as_ref();
    // DROPPED channel, for the same reason as `dead_bitmap_rasters` (R29-1):
    // this is the overlay probe, it runs per frame, and a dead raster's
    // disclosure is the empty coverage it returns plus the ⚠ on the mask row —
    // not a console line the windowed surface cannot show anyway. Under 5c
    // this was a `None` that suppressed only the STEM.
    let probe = crate::diag::dropped();
    // `or_else`, not a second branch: a brush group has no file for
    // `load_mask_bitmap` to find and a bitmap has no dab stream to stamp, so
    // exactly one of the two answers for any geometry. Same frame size the
    // caller will paint this overlay over, so the wash is the weight the
    // render applies — which is what
    // `the_gui_coverage_overlay_matches_what_the_render_applies` asserts.
    let bmp = load_mask_bitmap(&m.mask, &probe).or_else(|| brush_raster(&m.mask, w, h));
    let comp_bmps: Vec<Option<std::sync::Arc<image::GrayImage>>> = m
        .components
        .iter()
        .map(|c| load_mask_bitmap(&c.geometry, &probe).or_else(|| brush_raster(&c.geometry, w, h)))
        .collect();
    // Same load-failure contract as `apply_masks` (inert, inversion included,
    // components included), so the overlay never advertises coverage the
    // render will not apply.
    if (bmp.is_none() && is_raster_backed(&m.mask))
        || m.components
            .iter()
            .zip(&comp_bmps)
            .any(|(c, b)| b.is_none() && is_raster_backed(&c.geometry))
    {
        return image::GrayImage::new(w, h);
    }
    let comp_refs: Vec<Option<&image::GrayImage>> =
        comp_bmps.iter().map(|bmp| bmp.as_deref()).collect();
    let amount = m.amount.clamp(0.0, 1.0);
    let mut out = image::GrayImage::new(w, h);
    for (x, y, px) in out.enumerate_pixels_mut() {
        // Same normalisation as apply_masks' weight_at — pixel CENTRES,
        // through the shared [`MASK_SAMPLE_CENTRE`], so the wash cannot drift
        // half a pixel from the weight the render applies.
        let mut wgt = combined_mask_weight(
            m,
            (x as f32 + MASK_SAMPLE_CENTRE) / w as f32,
            (y as f32 + MASK_SAMPLE_CENTRE) / h as f32,
            bmp.as_deref(),
            &comp_refs,
            unwarp,
            (w as f32, h as f32),
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

/// Weight of the COARSE arm in the negative texture mix, and the fraction of
/// the render raster's short edge its Gaussian σ binds to (B8-2 §6-1, five-step
/// joint fit, 890 residuals, rms 0.0048).
///
/// Landmarks rather than bare digits: σ₁ = 12.99 px on a 4160 px short edge,
/// a half-power period of 69.3 px, and 36.1 % of the total depth.
const TEXTURE_COARSE_AMPLITUDE: f32 = 0.172_443;
const TEXTURE_COARSE_SIGMA_FRAC: f32 = 0.003_123_5;

/// The FINE arm — the one B8 missed entirely and B8-2 found under the capture
/// sharpening: σ₂ = 1.174 px at a 4160 px short edge, half-power period 6.3 px,
/// and **63.9 % of the total depth**. Losing it is why the R28 band form kept
/// 0.9992 of a 4 px pattern where Lightroom keeps 0.57.
const TEXTURE_FINE_AMPLITUDE: f32 = 0.304_888;
const TEXTURE_FINE_SIGMA_FRAC: f32 = 0.000_282_2;

/// The one free parameter of the DEPTH law, `w(t) = t(1+d)/(1+d·t)` (B8-2 §1
/// ruling 3). `w(1) = 1` is exact and free, so the endpoint is the plateau
/// `1 − (A₁+A₂) = 0.52267` with no epsilon.
const TEXTURE_DEPTH_D: f32 = 0.558_583;

/// Below this σ (in RENDER-raster pixels) an arm is dropped rather than
/// approximated — the ruling of 2026-08-21 (`r29-rulings-2026-08-20.md`
/// 拍板三), and the threshold is where a sampled kernel stops representing the
/// continuous Gaussian it stands for rather than a round number.
///
/// Measured, at the frequencies that matter: at σ = 0.49 the 4σ-truncated FIR
/// transfers 0.601 at Nyquist where the continuous `G` is 0.306, and at the
/// σ₂ = 0.2407 a 1280 px preview actually asks for (`gui/model.rs:296`,
/// short edge ≈ 853) the kernel collapses to `[1.8e−4, 1, 1.8e−4]` and
/// transfers 0.9993 — an identity wearing a Gaussian's name. At σ₂ = 1.174
/// (a 4160 px raster) the discrete and continuous responses agree to 4 dp.
/// So the clamp is not a behaviour cliff — it makes explicit what a sub-pixel
/// spatial kernel was going to do anyway, and stops the pass paying for it.
const TEXTURE_MIN_SIGMA_PX: f32 = 0.5;

/// `w(t) = t(1+d)/(1+d·t)` — the negative half's DEPTH against the slider,
/// evaluated in f64 and returned in f32 (the constants are quoted to 6 digits;
/// f32 division would spend two of them for nothing).
///
/// **The linear reading is refuted, not merely improved on.** The engine's old
/// `strength = -amount` is exactly `w(t) = t` — bit-verified at
/// D(−50)/D(−100) = 0.5000 — where the five-step Lightroom ladder gives 0.605.
/// Even with the endpoint matched, linear under-depths −50 by 18 % and −10 by
/// 32 % (`w(0.5) = 0.609`, `w(0.1) = 0.148`). A single power law is refuted
/// too: the local exponent drifts 0.85 → 0.67 across the ladder, the best fit
/// `t^0.778` misses ±0.024 at the ends, and this one-parameter hyperbolic form
/// holds them to ±0.008 (B8-2 §6-4 items 1-2).
fn texture_depth(t: f32) -> f32 {
    let t = f64::from(t.clamp(0.0, 1.0));
    let d = f64::from(TEXTURE_DEPTH_D);
    (t * (1.0 + d) / (1.0 + d * t)) as f32
}

/// The `(σ_coarse, σ_fine)` the negative half runs at on a `w × h` raster.
///
/// **`min(w, h)` is the RENDER raster's short edge, and that is adjudicated
/// rather than assumed.** A two-resolution export pair — 6240 × 4160 and
/// 3120 × 2080, sidecars byte-identical but for the delivery size — separates
/// "σ is a fixed pixel count" from "σ is a fixed fraction of the short edge" by
/// **16×** (rms 0.0886 vs 0.0054 across 4 ≤ k ≤ 96), and a leave-out check
/// carries the full-size fit onto the half-size file with σ scaled by the
/// short-edge ratio for rms 0.0048 — its own in-sample residual (B8-2 §1
/// ruling 4). This engine is the architecture that makes that reading
/// unambiguous: the develop runs at FULL resolution and `--long-edge` resamples
/// the FINISHED pixels as the last stage (`src/main.rs:891-894`, the resize at
/// `src/render.rs:1569-1575`), so the `(w, h)` handed to this pass IS the
/// render raster and never the delivery size.
///
/// What the two-resolution pair does NOT decide is whether σ tracks the FILM's
/// resolution or a fixed pixel count — every fixture came off one ARW, so both
/// readings predict the same numbers (B8-2 §7-1). The proportional form is kept
/// because at a single film resolution it introduces no known error and it is
/// what the pass already did.
fn texture_sigmas(w: usize, h: usize) -> (f32, f32) {
    let short = w.min(h) as f32;
    (TEXTURE_COARSE_SIGMA_FRAC * short, TEXTURE_FINE_SIGMA_FRAC * short)
}

/// The integer box³ radius whose equivalent σ = √(r(r+1)) sits nearest `sigma`.
///
/// Closed form, not a search: `σ² = r(r+1)` inverts to
/// `r = (√(1+4σ²) − 1)/2`, and only the two integers around it can win.
fn box3_radius_for_sigma(sigma: f32) -> usize {
    // `is_finite` FIRST so a NaN σ leaves by this door rather than through a
    // comparison that is false either way.
    if !sigma.is_finite() || sigma <= 0.0 {
        return 0;
    }
    let s = f64::from(sigma);
    // The clamp bounds a hand-written frame size out of an overflowing `+ 1`
    // below; no raster reaches it (σ = 1e9 needs a 3.2e11 px short edge).
    let lo = (((1.0 + 4.0 * s * s).sqrt() - 1.0) * 0.5).floor().clamp(0.0, 1e9) as usize;
    let err = |r: usize| {
        let r = r as f64;
        ((r * (r + 1.0)).sqrt() - s).abs()
    };
    if err(lo) <= err(lo + 1) { lo } else { lo + 1 }
}

/// **The texture operator** — the ONE calibration the global stage
/// ([`apply_develop`] 3b) and the mask arm ([`apply_masks`]) both call, so
/// "Texture −40" cannot come to mean two different things depending on whether
/// a mask is in the way. `amount` is the slider ÷ 100 (−1..=1).
///
/// **POSITIVE — unchanged, and measured.** A plain unsharp mask at the
/// resolution-normalised radius both arms have shared since R25 B2 (0.5 % of
/// the short edge, floored at 2 px), with no midtone weighting:
/// `l + amount·(l − blur)`. R27 P2 measured this half against Lightroom and not
/// one character of it is touched here.
///
/// **NEGATIVE — measured against Lightroom and rebuilt to the measurement
/// (R29 B8-2, landed 2026-08-21).** See [`texture_negative_pass`] for the
/// model, the kernels and the evidence; this function only picks the σ pair.
///
/// **RENDER-BEHAVIOUR HARD CHANGE, every negative Texture value, global and
/// per mask.** The R28 Batch-5 band form this replaces was a notch —
/// `1 − |t|·(G_f − G_c)`, returning to 1 at BOTH spectral ends — designed with
/// no Lightroom ground truth in the tree. Two controlled ladders now say the
/// shape itself was wrong: Lightroom's negative Texture is a monotone
/// HIGH-SHELF. Recipes re-render; version snapshots keep the old pixels. The
/// sidecar still carries the raw slider value so Lightroom re-renders it with
/// its own model — the same stance [`manual_vignette_lut`] takes.
fn texture_pass(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    amount: f32,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
) {
    if amount >= 0.0 {
        let radius = ((0.005 * w.min(h) as f32).round() as usize).max(2);
        unsharp_luma_weighted(data, w, h, radius, amount, false, weight);
        return;
    }
    let (sigma_coarse, sigma_fine) = texture_sigmas(w, h);
    texture_negative_pass(data, w, h, -amount, sigma_coarse, sigma_fine, weight);
}

/// The negative half, at an explicit σ pair — **two low-passes mixed in
/// PARALLEL, scaled by a hyperbolic depth law**:
///
/// ```text
///   l' = l − w(t)·[ A₁·(l − G_σ₁∗l) + A₂·(l − G_σ₂∗l) ]
/// ```
///
/// `t = |slider|/100`. Parallel, NOT cascaded: the two high-passes are summed,
/// not composed, and the arms carry 36 % / 64 % of the depth. Free-refitting
/// the old cascade band form against the same ground truth lands 8.2× worse
/// (rms 0.0392 vs 0.0048) — a wrong function family, not mistuned constants
/// (B8-2 §1 ruling 5).
///
/// **Why σ is a parameter here and not read off `(w, h)`.** The acceptance
/// grid is defined on a 4160 px short edge; a test that had to build a
/// 4160 × 4160 frame to reach it would cost 200 MB to assert nine numbers.
/// Splitting the σ choice ([`texture_sigmas`]) from the filter lets the anchor
/// test drive the real filter at the real σ on a 2048 × 64 strip.
///
/// **The kernels, and why they are not the same kernel.** The anchor grid is
/// the arbiter, and it rejected the cheap answer:
///
/// | scheme | max dev vs the closed form, 45 anchors |
/// |---|---|
/// | box³ both arms, integer radius | **0.0443 — fails** |
/// | box³ both arms, fractional (extended-box) radius | **0.0373 — fails** |
/// | box³ coarse + true Gaussian FIR fine | 0.0088 |
/// | **as shipped** (below) | **0.0037** |
///
/// σ₂ = 1.174 px is simply not on the box³ grid — the nearest integer radius
/// (r = 1) is σ = 1.414, and no fractional-radius box³ has the right SHAPE at
/// that support either (its sinc³ transfer reads 0.037 at a 4 px period where
/// the Gaussian reads 0.183). So the fine arm is a real separable Gaussian FIR
/// ([`gauss_blur_plane`]), and the coarse arm stays on the O(N) box³ the whole
/// file already uses, where at σ₁ ≈ 13 px the shape error is a rounding
/// difference.
///
/// **`coarse` is still grown FROM `fine`, and that is the parallel model, not a
/// cascade.** Gaussians compose — `G_σ₁ = G_σ₂ ∗ G_√(σ₁²−σ₂²)` — so blurring
/// the fine plane by the residual σ′ = 12.941 px produces exactly the coarse
/// arm the formula asks for, while the luma plane dies before the coarse blur
/// starts. The pass therefore holds the same TWO f32 planes the old band form
/// did (`jobs::PER_PHOTO_PEAK_COMMIT_MB` unmoved), and `l` is recomputed from
/// the pixel — the same number the dropped luma plane held, since every pixel
/// is read before it is written.
///
/// **Cost of the FIR arm**, stated rather than hoped: the kernel is 2⌈4σ₂⌉+1
/// taps, so 11 at a 4160 px short edge and 17 at 61 MP — 0.57 and 2.05 G
/// multiply-adds across both separable passes, against ~0.7 G for the entire
/// box³ chain. It is row-parallel like every other plane pass here.
///
/// **The domain is load-bearing.** The fit holds in the sRGB-gamma domain and
/// diverges by 0.041 at a 4 px period in linear light (B8-2 §6-3), so this pass
/// must run on gamma-encoded pixels. It does: the develop's buffer is
/// sRGB-encoded before `apply_develop` is ever called (`src/render.rs:326`, the
/// baked path; `src/render.rs:1417`, `calibrate_camera_buffer`'s last line).
///
/// **Where the model is honest about not applying.** Lightroom's operator is
/// amplitude-adaptive — not LTI: H spans 0.33 → 0.85 with detail amplitude
/// inside one octave on the clean base, against ≤ 0.009 for an LTI control. A
/// fixed kernel can only match the ENSEMBLE, which is what the anchor grid is
/// (512-block cross-spectrum over one 6240 × 4160 frame). Edge-preserving
/// behaviour is not modelled here and is registered, not silently claimed.
///
/// **Preview fidelity, and the promise that does not hold on this branch.**
/// R25 B2 promised one slider value means one structure at a 1280 px preview
/// and at 61 MP. On the negative half it still does not — the reason has
/// changed from "`fine_radius = radius/4` degenerates to 1" to "σ₂ is
/// sub-pixel". At the GUI preview raster (`gui/model.rs:296`, short edge ≈ 853)
/// σ₂ = 0.241 px, so [`TEXTURE_MIN_SIGMA_PX`] drops the fine arm and the
/// preview's negative Texture is WEAKER than the export's by up to 0.021 in
/// transfer at a 4 px preview period, 0.036 at 3 px and 0.076 at its Nyquist.
/// User ruling of 2026-08-21: clamp and disclose, no approximation and no 1:1
/// patch render. The export is exact; the preview is honestly weaker.
///
/// Below a 228 px short edge the coarse arm's own box³ radius rounds to 0 and
/// the whole negative half becomes a no-op — a thumbnail has no mid band left
/// to take out, and a radius-0 box³ would have silently applied the FINE arm's
/// high-pass at the COARSE arm's amplitude.
fn texture_negative_pass(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    t: f32,
    sigma_coarse: f32,
    sigma_fine: f32,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
) {
    if w == 0 || h == 0 {
        return; // par_chunks_mut(0) asserts; a 0-dim frame has no pixels anyway
    }
    let fine_on = sigma_fine >= TEXTURE_MIN_SIGMA_PX;
    // The coarse arm is grown from the fine plane, so what it must supply is the
    // RESIDUAL σ′ = √(σ₁²−σ₂²) — and the whole σ₁ when the fine arm was clamped
    // out and the plane is still the raw luma.
    let residual = if fine_on {
        (sigma_coarse * sigma_coarse - sigma_fine * sigma_fine).max(0.0).sqrt()
    } else {
        sigma_coarse
    };
    let coarse_r =
        if sigma_coarse >= TEXTURE_MIN_SIGMA_PX { box3_radius_for_sigma(residual) } else { 0 };
    let coarse_on = coarse_r >= 1;
    let depth = texture_depth(t);
    if (!fine_on && !coarse_on) || depth <= 0.0 {
        return;
    }
    // Either the fine arm's plane or — when it is clamped out — the luma itself.
    // Either way it is what the coarse arm is grown from, and either way the
    // luma plane is gone by the time the coarse blur allocates.
    let fine = {
        let luma: Vec<f32> = data.par_iter().map(luma601).collect();
        if fine_on { gauss_blur_plane(&luma, w, h, sigma_fine) } else { luma }
    };
    let coarse = if coarse_on { Some(blur_plane(&fine, w, h, coarse_r)) } else { None };
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let wgt = weight(x, y, px);
            if wgt <= 0.001 {
                continue;
            }
            let i = y * w + x;
            let l = luma601(px);
            // `fine` holding the unblurred luma would make this term exactly
            // zero on its own (same function, same pixel, read before written);
            // the guard states the arm is OFF rather than leaving a reader to
            // rediscover that.
            let hp_fine = if fine_on { l - fine[i] } else { 0.0 };
            let hp_coarse = coarse.as_ref().map_or(0.0, |c| l - c[i]);
            let mix = TEXTURE_COARSE_AMPLITUDE * hp_coarse + TEXTURE_FINE_AMPLITUDE * hp_fine;
            let new_l = (l - depth * mix * wgt).clamp(0.0, 1.0);
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

/// A TRUE separable Gaussian blur — the one place in this file that cannot use
/// [`blur_plane`], because the σ it is asked for is small enough that box³'s
/// shape stops being an approximation of a Gaussian and starts being a
/// different filter.
///
/// [`texture_negative_pass`] is the caller and its doc carries the arbitration:
/// at σ = 1.174 px a fractional-radius box³ transfers 0.037 at a 4 px period
/// where the Gaussian transfers 0.183, which alone misses the acceptance grid
/// by 0.037 against a ±0.02 budget. Above ~5 px the two agree to a rounding
/// difference and `blur_plane`'s O(N) running sums are the right tool; this is
/// for the other end.
///
/// O(taps) per pixel per axis rather than O(1), so the kernel truncation is
/// also the cost: 2⌈4σ⌉+1 taps, the tail beyond 4σ being `exp(−8) = 3.4e−4` of
/// the peak and renormalised away. Both passes are row-parallel and the
/// vertical one accumulates row-major, for the same cache reason
/// [`box_blur_v`] gives.
fn gauss_blur_plane(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if w == 0 || h == 0 {
        return src.to_vec();
    }
    // A kernel wider than the plane buys nothing: past the edge every tap reads
    // the same clamped sample.
    let Some(kernel) = gauss_kernel(sigma, w.max(h)) else {
        return src.to_vec();
    };
    let r = kernel.len() / 2;
    let mut mid = vec![0.0f32; src.len()];
    mid.par_chunks_mut(w).enumerate().for_each(|(y, orow)| {
        let row = &src[y * w..][..w];
        for (x, o) in orow.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            if x >= r && x + r < w {
                // Interior — no clamp in the hot loop. Same tap ORDER as the
                // border arm, so the two are bit-identical where they meet.
                for (k, wk) in kernel.iter().enumerate() {
                    acc += row[x + k - r] * wk;
                }
            } else {
                for (k, wk) in kernel.iter().enumerate() {
                    acc += row[(x + k).saturating_sub(r).min(w - 1)] * wk;
                }
            }
            *o = acc;
        }
    });
    let mut out = vec![0.0f32; src.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, orow)| {
        for (k, wk) in kernel.iter().enumerate() {
            let row = &mid[(y + k).saturating_sub(r).min(h - 1) * w..][..w];
            for (o, v) in orow.iter_mut().zip(row) {
                *o += v * wk;
            }
        }
    });
    out
}

/// The normalised 1-D Gaussian taps for `sigma`, or `None` when there is no
/// kernel to build. Summed and normalised in f64: the taps are quoted to f32 in
/// the end, but a kernel whose weights do not sum to 1 is a DC gain error, and
/// that is the one error a blur must not have.
fn gauss_kernel(sigma: f32, max_radius: usize) -> Option<Vec<f32>> {
    if !sigma.is_finite() || sigma <= 0.0 {
        return None;
    }
    let s = f64::from(sigma);
    let r = ((4.0 * s).ceil() as usize).clamp(1, max_radius.max(1));
    let mut k: Vec<f64> = (0..=2 * r)
        .map(|i| {
            let d = i as f64 - r as f64;
            (-0.5 * (d / s) * (d / s)).exp()
        })
        .collect();
    let sum: f64 = k.iter().sum();
    if !sum.is_finite() || sum <= 0.0 {
        return None;
    }
    for v in k.iter_mut() {
        *v /= sum;
    }
    Some(k.into_iter().map(|v| v as f32).collect())
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
// `pub(crate)` for the same reason as its inverse above: the zoned fit's
// joint value-range family reads its bucket means back OUT of linear light
// (R23-6), and a second copy of this curve would be a second thing to keep
// in step with the engine.
pub(crate) fn linear_to_srgb(c: f32) -> f32 {
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
    let (ys, m) = tone_model_knots(
        r.exposure_ev,
        [contrast, highlights, shadows, whites, blacks],
    );
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

/// The knot outputs and spline tangents of the engine's slider-tone model
/// for `(ev, sliders)` — the ONE definition `build_tone_lut` renders and the
/// reverse fit scores candidates against ([`sample_tone_model`]). Limiter,
/// authority weights, monotone snap and Fritsch–Carlson exactly as rendered;
/// no residual tone_curve and no base curve composed.
pub(crate) fn tone_model_knots(ev: f32, sliders: [f32; 5]) -> ([f32; 8], Vec<f32>) {
    let [contrast, highlights, shadows, whites, blacks] = limit_tone_sliders(ev, sliders);
    let mut ys = [0.0f32; 8];
    let weights = tone_knot_weights(ev);
    for (idx, &x) in TONE_KNOTS_X.iter().enumerate() {
        let b = tone_slider_basis(x);
        // Knot authority fades where exposure saturated BOTH adjacent base
        // intervals (see tone_knot_weights): a strong slider aimed at a
        // region exposure already clipped yields honest clipping, not the
        // interior flat band the backstop below used to manufacture.
        ys[idx] = tone_exposure_curve(x, ev)
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
    (ys, m)
}

/// The engine's slider-tone response at one input — [`tone_model_knots`]
/// evaluated through the same Hermite the LUT samples.
pub(crate) fn sample_tone_model(knots: &([f32; 8], Vec<f32>), x: f32) -> f32 {
    hermite_eval(&TONE_KNOTS_X, &knots.0, &knots.1, x)
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
/// The part of `neutral` the camera's embedded rendition shows: the whole
/// develop when the two share the sensor frame, else the centred crop at the
/// rendition's aspect. A body set to an in-camera aspect writes a centred
/// crop (a Sony 4:3 preview over the 3:2 sensor measured centred at NCC 0.987
/// against 0.83 for either side, v1.2.2); pairing the full frame against it
/// put the edge strips' histogram on one side of the CDF match only.
pub fn camera_frame_of(neutral: &DynamicImage, camera: &DynamicImage) -> DynamicImage {
    let (nw, nh) = (neutral.width(), neutral.height());
    let (cw, ch) = (camera.width(), camera.height());
    if cw == 0 || ch == 0 || crate::fit::same_frame_plausible_dims((nw, nh), (cw, ch)) {
        return neutral.clone();
    }
    let target = cw as f64 / ch as f64;
    let (w, h) = if nw as f64 / nh as f64 > target {
        ((nh as f64 * target).round() as u32, nh)
    } else {
        (nw, (nw as f64 / target).round() as u32)
    };
    let (w, h) = (w.clamp(1, nw), h.clamp(1, nh));
    neutral.crop_imm((nw - w) / 2, (nh - h) / 2, w, h)
}

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

/// [`oriented`]'s public door, for the photographer's own quarter turns.
///
/// The GUI is a separate crate and cannot reach `oriented` (deliberately
/// `pub(crate)`: the EIGHT-state transform belongs to the decode/render pair,
/// and a UI that could pass an arbitrary `Orientation` could mirror a photo by
/// accident). A quarter turn is the one rotation a UI legitimately asks for,
/// so that is the shape of the door. Lossless — a pure axis swap on the
/// image's own pixel type.
pub fn turn_image(img: DynamicImage, quarter_turns: u8) -> DynamicImage {
    oriented(img, quarter_turn_orientation(quarter_turns))
}

/// [`oriented`]'s coordinate twin: where a NORMALISED point of the sensor
/// frame lands in the display frame.
///
/// Derived from [`oriented`] itself, state by state, so the two can never
/// disagree — `image`'s `rotate90` is CLOCKWISE, mapping pixel `(x, y)` of a
/// `W×H` frame to `(H−1−y, x)` of the `H×W` result, i.e. `(u, v) → (1−v, u)`
/// normalised; `Transpose`/`Transverse` compose that with the horizontal flip
/// exactly as `oriented` does. Every state is its own bijection of the unit
/// square, which is what makes the era-0 → era-1 recipe migration lossless
/// and round-trippable (`orient_point_round_trips_through_its_inverse`).
///
/// Points OUTSIDE [0,1] are mapped by the same affine rule — mask gradients
/// legitimately live off-frame (ACR geometry), and clamping them here would
/// silently shorten a gradient's falloff.
pub fn orient_point(o: Orientation, u: f32, v: f32) -> (f32, f32) {
    match o {
        Orientation::Normal | Orientation::Unknown => (u, v),
        Orientation::HorizontalFlip => (1.0 - u, v),
        Orientation::Rotate180 => (1.0 - u, 1.0 - v),
        Orientation::VerticalFlip => (u, 1.0 - v),
        Orientation::Transpose => (v, u),
        Orientation::Rotate90 => (1.0 - v, u),
        Orientation::Transverse => (1.0 - v, 1.0 - u),
        Orientation::Rotate270 => (v, 1.0 - u),
    }
}

/// The orientation of `quarter_turns` CLOCKWISE quarter turns on their own —
/// the photographer's half of [`compose_orientation`].
///
/// Clockwise because [`oriented`] is: `image`'s `rotate90` turns clockwise
/// (its derivation is spelled out on [`orient_point`]), so `Rotate90` here and
/// a click on the 「turn right」 button mean the same motion. Values outside
/// 0..=3 fold (`% 4`), the same residue-class rule
/// `EditRecipe::clamp` applies to the field itself.
pub fn quarter_turn_orientation(quarter_turns: u8) -> Orientation {
    match quarter_turns % 4 {
        1 => Orientation::Rotate90,
        2 => Orientation::Rotate180,
        3 => Orientation::Rotate270,
        _ => Orientation::Normal,
    }
}

/// The ONE orientation every consumer reads: the camera's EXIF state followed
/// by the photographer's `quarter_turns` clockwise quarter turns, composed
/// into a single [`Orientation`].
///
/// This is the skeleton's root insight (ROADMAP 7.2): the eight EXIF states
/// ARE the dihedral group of the square, which is CLOSED under composition, so
/// a user turn on top of a `Transpose` file lands on a state [`oriented`],
/// [`orient_point`] and [`orient_recipe_coords`] already handle exactly. No
/// second rotation stage anywhere in the pipeline, no new geometry code.
///
/// **Composition order is `exif` FIRST**: `orient_point(compose(e, k), p) ==
/// orient_point(R90^k, orient_point(e, p))`. That is the order the pixels take
/// — `render_to_image_in` orients the sensor buffer into the display frame and
/// the user's turn is a turn OF that display frame — and it is asserted
/// exhaustively over all 9×4 (state, turn) pairs by
/// `compose_orientation_is_the_composition_of_the_two_coordinate_maps`.
///
/// **Implementation.** Each state is `(swap, flip_h, flip_v)` with the flips
/// taken in the SOURCE frame and the swap last — rawler's own `to_flips`
/// contract ("flipping must be done before transposing"), which is
/// bit-for-bit the convention [`orient_point`] was independently derived in
/// (checked state by state in the test above). Composing two such triples:
/// the flips XOR, and when the first map swaps, the second map's flips arrive
/// on exchanged axes and cross over. `Unknown` is [`Orientation::Normal`]'s
/// twin on the way in and never appears on the way out — the group has eight
/// elements, not nine.
pub fn compose_orientation(exif: Orientation, quarter_turns: u8) -> Orientation {
    compose_two(exif, quarter_turn_orientation(quarter_turns))
}

/// `b ∘ a` in coordinate terms — apply `a`, then `b`. Private because the only
/// composition the pipeline needs is [`compose_orientation`]'s; exposing a
/// general group operation would invite a second place to decide the order.
fn compose_two(a: Orientation, b: Orientation) -> Orientation {
    let (t1, h1, v1) = a.to_flips();
    let (t2, h2, v2) = b.to_flips();
    // `a` did not swap: `b`'s flips act on the same axes, so they simply XOR
    // and `b`'s swap is the composed swap. `a` DID swap: `b`'s horizontal flip
    // now lands on what was the vertical source axis (and vice versa), so the
    // two cross before XOR-ing, and the swaps XOR.
    let (h, v) = if t1 { (h1 ^ v2, v1 ^ h2) } else { (h1 ^ h2, v1 ^ v2) };
    Orientation::from_flips((t1 ^ t2, h, v))
}

/// Does this orientation MIRROR the frame (an odd number of reflections)?
/// The four reversing states are the ones whose `to_flips` triple has an odd
/// parity, and they are the only ones that flip the SIGN of a rotation angle
/// — the ellipse-`angle` half of [`orient_recipe_coords`].
fn orientation_mirrors(o: Orientation) -> bool {
    matches!(
        o,
        Orientation::HorizontalFlip
            | Orientation::VerticalFlip
            | Orientation::Transpose
            | Orientation::Transverse
    )
}

/// The frame a recipe's coordinates are CURRENTLY measured against, reduced to
/// the one number a turn needs from it: `W/H`.
///
/// Every other geometry in a recipe is normalised TWICE — `x` against the width
/// and `y` against the height — so turning the unit square carries it with no
/// knowledge of the frame's shape at all ([`orient_point`] is aspect-free by
/// construction). A BRUSH is the exception: `crs:Radius` and the dab stream's
/// `r` token are in WIDTH units while a dab is a circle in PIXELS
/// (`rasterise_brush_group`'s `aspect`), so a quarter turn — which exchanges
/// `W` and `H` — has to rescale every radius by `W/H` or the strokes come back
/// elliptical. That missing input is what R29 Batch-6b had to register and what
/// this type supplies.
///
/// **Which frame.** The one the coordinates are in BEFORE the turn, always:
/// the SENSOR rectangle for the `coord_era` migration and for an XMP import
/// (era-0 numbers and `crs:` numbers are both source-frame), the CURRENT
/// display rectangle for a photographer's rotate and for the export projection
/// back into the source frame.
///
/// `None` from [`new`](Self::new) for anything that is not a positive, finite
/// rectangle: a zero dimension makes the rescale singular, and guessing at one
/// would move a mask by an unbounded factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoordFrame {
    /// `W/H` of the frame the coordinates are in before the turn.
    aspect: f32,
}

impl CoordFrame {
    /// The frame of a `w × h` pixel rectangle. `f64` in, because every caller
    /// has pixel counts (`decode::source_frame`, `xmp::FrameAspect`) and
    /// narrowing them at the boundary is what loses a 61 MP dimension's last
    /// digits before the division rather than after.
    pub fn new(w: f64, h: f64) -> Option<Self> {
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0)
            .then(|| CoordFrame { aspect: (w / h) as f32 })
            .filter(|f| f.aspect.is_finite() && f.aspect > 0.0)
    }

    /// What a brush radius must be multiplied by so its dab stays the SAME
    /// circle in pixels after the turn.
    ///
    /// A dab's pixel radius is `r · W`. The turned frame's width is `H` for the
    /// four transposing states and `W` for the other four, so `r' = r · W / W'`
    /// is `W/H` and `1` respectively. Nothing else in the stroke is spatial —
    /// `f` (flow) and `h` (hardness) are deposit laws, and `MaskValue` is a
    /// density.
    fn brush_radius_scale(self, o: Orientation) -> f32 {
        if crate::decode::orientation_transposes(o) { self.aspect } else { 1.0 }
    }
}

/// Lightroom's own precision for a brush number: six decimal places, FIXED.
///
/// Measured on the user's library rather than assumed — `P49.xmp` writes
/// `r 0.218584`, `d 0.067120 0.096097` and `crs:Radius="0.216487"`, trailing
/// zeros included, and every token in the 22,966-token census has that shape.
/// Re-emitting a TURNED stream in the same form is what keeps the rewrite
/// invisible in the file, and on the pure rotations it is also what makes the
/// rewrite invertible in DECIMAL: `1 − 0.096097` is `0.903903` to six places
/// and back again, so a portrait capture's import → export round trip hands
/// Lightroom its own digits instead of an f32's shortest form.
const LR_DAB_DECIMALS: usize = 6;

/// One brush number as the sidecar spells it, or `None` for a value that has
/// overflowed out of the grammar (`xmp::dab_token_is_known` refuses a
/// non-finite token, so writing one would make the stream un-importable).
fn lr_dab_str(v: f32) -> Option<String> {
    v.is_finite().then(|| format!("{v:.prec$}", prec = LR_DAB_DECIMALS))
}

/// The same six-place grid applied to a stored `f32` — `BrushStroke::radius`,
/// which is a FIELD and not text, so it is quantised rather than formatted.
///
/// One rule for both halves of a stroke: the attribute and the `r` tokens are
/// two spellings of the same quantity, and letting them drift apart by the
/// width of a formatter would make a re-imported sidecar disagree with the
/// `recipe.json` it came from. Falls back to the un-quantised value when the
/// scaling by `1e6` overflows, which only a hand-written recipe can reach.
fn lr_dab_round(v: f32) -> f32 {
    let q = (v * 1e6).round() / 1e6;
    if q.is_finite() { q } else { v }
}

/// One `crs:Dabs` token rewritten into the turned frame — or `None` when it is
/// not one of the two SPATIAL forms, in which case the caller carries the token
/// through unchanged.
///
/// `d <x> <y>` moves through [`orient_point`] like every other coordinate in
/// this file. `r <f>` is rescaled ONLY when the turn exchanges the axes, so a
/// half turn or a mirror leaves every radius token byte-identical. `f` and `h`
/// are never spatial and never touched.
///
/// A malformed token is `None` and therefore VERBATIM, matching `brush_dabs`'
/// own rule: the XMP boundary already refuses anything outside the grammar, and
/// a hand-edited `recipe.json` must not have its stream silently shortened by a
/// migration. An overflowed result is `None` for the same reason.
fn turned_dab_token(token: &str, o: Orientation, radius_scale: f32) -> Option<String> {
    let mut it = token.split_whitespace();
    match it.next()? {
        "d" => {
            let (x, y) = (brush_token_num(&mut it)?, brush_token_num(&mut it)?);
            if it.next().is_some() {
                return None;
            }
            let (x, y) = orient_point(o, x, y);
            Some(format!("d {} {}", lr_dab_str(x)?, lr_dab_str(y)?))
        }
        "r" if radius_scale != 1.0 => {
            let v = brush_token_num(&mut it)?;
            if it.next().is_some() {
                return None;
            }
            Some(format!("r {}", lr_dab_str(v * radius_scale)?))
        }
        _ => None,
    }
}

/// Turn a brush's strokes: every dab coordinate through [`orient_point`], every
/// radius through [`CoordFrame::brush_radius_scale`].
///
/// **The stream is REBUILT, not edited.** `split('\n')` is the exact inverse of
/// the join `xmp::parse_dabs` performs (a token carrying a newline is refused
/// there), so an EMPTY stream comes back empty, a stream of nothing but `f`/`h`
/// state comes back byte for byte, and the token count cannot change.
fn turn_brush_strokes(
    strokes: &mut [crate::recipe::BrushStroke],
    o: Orientation,
    frame: CoordFrame,
) {
    let scale = frame.brush_radius_scale(o);
    for s in strokes.iter_mut() {
        // The stroke ATTRIBUTE is the stream's initial state (102 real
        // components carry no `r` token at all), so it rides the same scale.
        if scale != 1.0 {
            s.radius = lr_dab_round(s.radius * scale);
        }
        let mut out = String::with_capacity(s.dabs.len() + 16);
        for (i, token) in s.dabs.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            match turned_dab_token(token, o, scale) {
                Some(t) => out.push_str(&t),
                None => out.push_str(token),
            }
        }
        s.dabs = out;
    }
}

/// Rewrite a recipe's stored GEOMETRY from the sensor frame into the display
/// frame — the deterministic, bijective half of the `coord_era` 0 → 1
/// migration (`pipeline::migrate_recipe_coord_frame` owns the gating).
///
/// Moves the crop rectangle, every mask geometry (base + components) and the
/// Range-Mask colour sample point through [`orient_point`]. Returns `false`
/// for the identity orientations, so the caller can tell "nothing to do" from
/// "moved".
///
/// **Ellipse angle.** `MaskGeometry::Radial`'s `top/left/bottom/right` is a
/// centre+radii carrier, not a true bounding box, and `angle` rotates the
/// ellipse inside it (see `mask_weight`). Mapping the two corners through
/// `orient_point` already swaps the radii for the four transposing states,
/// which is exactly equivalent to leaving them alone and adding ±90° to the
/// angle — an ellipse rotated a quarter turn IS the same ellipse with its
/// axes exchanged. So a pure ROTATION needs no angle change at all; a
/// MIRROR needs the angle negated, because a reflection reverses the sense of
/// rotation. Verified algebraically against `mask_weight`'s own quadratic
/// form and pinned by `rotated_radial_mask_covers_the_rotated_pixels`.
///
/// **Not migrated: `MaskGeometry::Bitmap`.** A raster mask is a FILE of
/// pixels sampled in normalised coordinates, not a coordinate — turning it
/// would mean rewriting an image on disk that version snapshots and other
/// recipes may share. The caller discloses this instead of pretending. It is
/// now the ONLY member of that disclosure ([`recipe_has_raster_masks`]).
///
/// **Migrated since R29 C1: `MaskGeometry::Brush`, by NUMERICALLY REWRITING its
/// dab stream** (and an `AiMask`'s `crs:Gesture` strokes, which are the same
/// payload under a different parent). Until this batch the stream was carried
/// verbatim so a republished sidecar was byte-faithful to Lightroom's, and the
/// brush rendered nothing, so an un-turned stream was invisible; R29 Batch-6b
/// made the brush DRAW, which turned that verbatim carry into a mask left at
/// its old coordinates while every parametric shape beside it moved. The user's
/// ruling (2026-08-21) is that the render is what must be right: coordinates
/// turn, radii rescale by the frame aspect, and a rotated photo's republished
/// dab stream is no longer byte-identical to the one Lightroom wrote — it is
/// still legal, still six decimal places, and still says the same mask about
/// the frame the document declares. An UNROTATED photo is untouched: the
/// identity orientations return before any of this, so their streams cannot
/// change even by a formatter.
///
/// This is the one arm that needs `frame`, and the reason is a unit mismatch,
/// not the coordinates: see [`CoordFrame`].
///
/// **The straighten angle** (R27, closing the R24 registration
/// 「`straighten≠0` 时 crop 迁移一阶近似」). `Crop` is normalised against the
/// STRAIGHTENED frame — `render_pipeline` runs `rotate_straighten` before
/// `apply_crop` — so migrating the rectangle correctly means knowing what the
/// straighten does under the same turn. Two facts settle it:
///
/// * [`inscribed_dims`] is SWAP-EQUIVARIANT: `inscribed_dims(h, w, deg)` is
///   `inscribed_dims(w, h, deg)` with its two outputs exchanged. The general
///   branch is `((w·c − h·s)/cos2, (h·c − w·s)/cos2)`, visibly so; the thin
///   branch's `if w >= h` looks asymmetric but is unreachable at `w == h`,
///   because that branch needs `short ≤ sin(2a)·long`, i.e. `sin 2a ≥ 1`, i.e.
///   exactly 45°, where `s == c` makes its two outputs equal anyway. So the
///   inscribed rectangle of the TURNED frame is the turn of the inscribed
///   rectangle, and normalised coordinates inside it map by `orient_point`
///   with nothing left over.
/// * Rotations commute (`rot(deg) ∘ R90 == R90 ∘ rot(deg)`), so for the four
///   pure rotations the migration was ALREADY exact. Reflections do not:
///   `rot(deg) ∘ M == M ∘ rot(−deg)`. Leaving the angle alone through a
///   MIRROR therefore straightened the frame the wrong way by `2·deg` — the
///   approximation R24 registered — and every crop coordinate then indexed
///   content that had been rotated out from under it.
///
/// The fix is the rule already applied to the ellipse `angle` just below: a
/// reflection reverses the sense of a rotation, so negate it. With that, all
/// eight states are exact and the migration stays the bijection its
/// round-trip test claims.
///
/// **Not migrated, and correctly so: `Radial::midpoint`** (R25 P5). It is a
/// ratio along the ellipse's own falloff axis, not a point in the frame — the
/// same status a tone-curve point has — so turning the frame leaves it
/// meaning exactly what it meant. Said out loud because the next reader will
/// scan this function for "every geometry field" and find one it skips.
///
/// **Not migrated, and correctly so: the four local point curves** (R25 P6,
/// `LocalAdjustment::main_curve` …). Their points are `{input, output}` pairs
/// on the 0..255 TONE axis, not positions in the frame: rotating a photo
/// changes which pixels a mask covers, never what value 128 maps to. Stated
/// here for the same reason as `midpoint` — they are fields on a mask, and a
/// reader auditing "did the migration cover every mask field?" must find the
/// answer rather than a silence.
///
/// **`frame`** is the shape of the rectangle the coordinates are in BEFORE the
/// turn, and only the brush arm reads it ([`CoordFrame`]). `None` means "not
/// known here", and the honest consequence is that a brush's dabs are left
/// where they were — every production caller supplies one, and the only one
/// that can pass `None` (`pipeline::rotate_recipe`, which reads the photo's
/// header lazily) does so exactly when the recipe holds no brush stroke at all
/// ([`recipe_has_brush_strokes`]).
pub fn orient_recipe_coords(
    r: &mut EditRecipe,
    o: Orientation,
    frame: Option<CoordFrame>,
) -> bool {
    if matches!(o, Orientation::Normal | Orientation::Unknown) {
        return false;
    }
    let mirrors = orientation_mirrors(o);
    // The straighten rides the same sign rule as an ellipse angle, and for the
    // same reason (see the doc comment): a reflection reverses the sense of a
    // rotation, and `rotate_straighten` runs BEFORE `apply_crop`, so getting
    // this wrong moves the content under every crop coordinate below.
    if mirrors {
        r.straighten_deg = -r.straighten_deg;
    }
    if let Some(c) = r.crop.as_mut() {
        let (x0, y0) = orient_point(o, c.left, c.top);
        let (x1, y1) = orient_point(o, c.right, c.bottom);
        *c = Crop {
            left: x0.min(x1),
            right: x0.max(x1),
            top: y0.min(y1),
            bottom: y0.max(y1),
        };
    }
    let turn = |g: &mut MaskGeometry| match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            (*zero_x, *zero_y) = orient_point(o, *zero_x, *zero_y);
            (*full_x, *full_y) = orient_point(o, *full_x, *full_y);
        }
        MaskGeometry::Radial { top, left, bottom, right, angle, .. } => {
            let (x0, y0) = orient_point(o, *left, *top);
            let (x1, y1) = orient_point(o, *right, *bottom);
            (*left, *right) = (x0.min(x1), x0.max(x1));
            (*top, *bottom) = (y0.min(y1), y0.max(y1));
            if mirrors {
                *angle = -*angle;
            }
        }
        // Raster masks carry no coordinates — see the doc comment.
        MaskGeometry::Bitmap { .. } => {}
        // A brush group carries thousands of coordinates inside `crs:Dabs`,
        // and since R29 C1 they TURN — numerically, token by token. The
        // registration this arm used to hold ("un-turned, because the function
        // is handed an `Orientation` and nothing else") is closed by `frame`;
        // the doc comment above carries the ruling and the cost.
        MaskGeometry::Brush { strokes, .. } => {
            if let Some(f) = frame {
                turn_brush_strokes(strokes, o, f);
            }
        }
        // An AI mask carries a reference coordinate and may carry gesture dab
        // coordinates, so both turn like every other point in the frame. Its
        // cached alpha does not: that raster was segmented in the OLD frame, so
        // rotating it is not a coordinate migration, it is a re-render. The
        // cache is DROPPED and the next develop recomputes it at the turned
        // point (`segment::resolve_ai_masks`), which is why this geometry is
        // not a member of `recipe_has_raster_masks` — nothing here fails to be
        // turned, and claiming it did would be the wrong disclosure.
        //
        // The `gesture` strokes are `BrushStroke`s under a different parent and
        // ride the SAME rewrite (R29 C1). The renderer does not composite them;
        // subtype 0 sends their `d` points to the segmenter. They are also
        // written back, so leaving them in the old frame would hand Lightroom a
        // refinement stroke beside a moved reference point.
        MaskGeometry::AiMask { ref_x, ref_y, raster, gesture, .. } => {
            (*ref_x, *ref_y) = orient_point(o, *ref_x, *ref_y);
            *raster = None;
            if let Some(f) = frame {
                turn_brush_strokes(gesture, o, f);
            }
        }
    };
    for m in r.masks.iter_mut() {
        turn(&mut m.mask);
        for c in m.components.iter_mut() {
            turn(&mut c.geometry);
        }
        // The colour Range Mask's `(px, py)` is Lightroom's sample MARKER —
        // cosmetic, but it is a point in the original frame like any other,
        // and leaving it behind would put the marker on the wrong subject.
        if let Some(RangeMask::Color { px, py, .. }) = m.range.as_mut() {
            (*px, *py) = orient_point(o, *px, *py);
        }
    }
    true
}

/// Does this recipe hold any geometry the `coord_era` migration would move?
/// Used for the disclosure: a recipe with nothing but global sliders is
/// re-stamped in silence, because nothing about it changed.
///
/// A mask counts for its GEOMETRY only. Its point curves (R25 P6) are
/// frame-independent tone values and do not qualify — a mask carrying nothing
/// but a curve still counts here through its geometry, so the distinction is
/// invisible in the answer and easy to misread as an omission. It is not one;
/// see [`orient_recipe_coords`].
///
/// `straighten_deg` is deliberately NOT counted (R27 L-16c), even though the
/// migration now reverses it under a mirror. It moves for FOUR of the eight
/// states and this predicate cannot see which one is coming, so counting it
/// would raise the note on every rotated photo that has a tilt and nothing
/// else — an alarm that is wrong three times in four. The states that do move
/// it (`HorizontalFlip`/`VerticalFlip`/`Transpose`/`Transverse`) are ones no
/// camera writes; a recipe that also holds a crop or a mask — i.e. any recipe
/// where the tilt has something to be wrong about — is disclosed by that.
pub fn recipe_has_frame_coords(r: &EditRecipe) -> bool {
    r.crop.is_some()
        || r.masks.iter().any(|m| {
            // Bitmap is the ONE geometry that does not move (its pixels are a
            // file). Brush left this exclusion in R29 C1: its dab stream is
            // rewritten numerically now, so counting it here is the true
            // statement, not the flattering one.
            let turnable = |g: &MaskGeometry| !matches!(g, MaskGeometry::Bitmap { .. });
            turnable(&m.mask)
                || m.components.iter().any(|c| turnable(&c.geometry))
                || matches!(m.range, Some(RangeMask::Color { .. }))
        })
}

/// Does this recipe carry a brush stroke anywhere — a [`MaskGeometry::Brush`]
/// group or an [`MaskGeometry::AiMask`]'s `crs:Gesture` refinement?
///
/// The predicate exists for ONE caller: `pipeline::rotate_recipe` needs the
/// photo's frame shape ([`CoordFrame`]) only for these, and reading it costs a
/// metadata walk of the RAW (a `RawSource` slurp, 60–120 MB for a 61 MP ARW).
/// Asking this first is what keeps a rotate of an ordinary develop as cheap as
/// it was — and what makes `orient_recipe_coords`' `None` arm unreachable with
/// a brush in hand instead of merely unlikely.
///
/// A group with an EMPTY stroke list counts as nothing: there is no coordinate
/// to move, so no frame is needed to move it.
pub fn recipe_has_brush_strokes(r: &EditRecipe) -> bool {
    let brushed = |g: &MaskGeometry| match g {
        MaskGeometry::Brush { strokes, .. } => !strokes.is_empty(),
        MaskGeometry::AiMask { gesture, .. } => !gesture.is_empty(),
        _ => false,
    };
    r.masks
        .iter()
        .any(|m| brushed(&m.mask) || m.components.iter().any(|c| brushed(&c.geometry)))
}

/// Does this recipe carry a geometry the `coord_era` migration cannot turn
/// (see [`orient_recipe_coords`])? Drives the honest half of the migration's
/// disclosure.
///
/// ONE member since R29 C1: a raster [`MaskGeometry::Bitmap`], whose pixels are
/// a FILE — rewriting someone's PNG is not a coordinate migration, and version
/// snapshots or another saved recipe may point at that same file. The NAME is
/// exact again, which it had stopped being: R27 Batch-4 put
/// [`MaskGeometry::Brush`] in here too (its dabs were carried verbatim for the
/// sidecar round trip and there was no frame aspect to rescale their radii
/// with), so the function meant "cannot be turned" while it said "raster". The
/// brush turns now — numerically, see the `Brush` arm of
/// [`orient_recipe_coords`] — and is counted by [`recipe_has_frame_coords`]
/// with every other geometry that moves.
///
/// An [`MaskGeometry::AiMask`] has never been a member and still is not: its
/// cached alpha is DROPPED rather than left behind, so nothing about it fails
/// to be turned.
pub fn recipe_has_raster_masks(r: &EditRecipe) -> bool {
    let unturnable = |g: &MaskGeometry| matches!(g, MaskGeometry::Bitmap { .. });
    r.masks
        .iter()
        .any(|m| unturnable(&m.mask) || m.components.iter().any(|c| unturnable(&c.geometry)))
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
pub(crate) fn profile_knot_interp(knots: &[f32], r: f32) -> f32 {
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

// --- the MASK WARP: Lightroom's stored mask frame → Lightroom's export frame -
//
// Everything in this block is about LIGHTROOM's frames, not this engine's. The
// two are not the same and the difference is the whole reason the block needs a
// header rather than a one-line doc.
//
// WHAT WAS MEASURED (R27 Batches 8-10, R29 `D`, and D2 LINEAR through
// 2026-08-24). Lightroom stores geometry by mask type and LINEAR adds a
// topology distinction:
//
//   * BRUSH dabs are stored PRE-lens-correction. Eleven disjoint dabs on the
//     24 mm frame are displaced from their stored coordinates by exactly the
//     lens-profile distortion field: the `.lcp` model with ZERO free parameters
//     scores 4.63 px rms against a 4.19 px tangential noise floor, and the same
//     field read off the camera's own knots scores 5.13 px. On 138 pixel
//     patches of a `LensProfileEnable` 0→1 pair the two score 2.11 / 2.30 px.
//   * RADIAL points are stored POST-correction. On the 105 mm `D` pair the
//     PIXELS move +87.5 px at r ≈ 3250 (the `.lcp` model at 2.69 px rms,
//     30 NCC points, tangential rms 1.22 px) while the radial mask itself
//     measures a similarity of 0.99956 — identity to within 0.05 %, and
//     88.7 px away from the pixel field.
//   * LINEAR stores corrected-frame Zero/Full handles. With correction ON it
//     reconstructs the straight gradient in that corrected frame. With
//     correction OFF it maps only those two handles through D_fwd and rebuilds
//     one straight gradient in the raw pixel metric. Pointwise H1 is rejected:
//     its predicted full-contour sag is 10.8–24.4 px with the wrong sign.
//
// So `m(r)` below is one number per radius: where a stored radius LANDS in
// Lightroom's export. `LensProfile::mask_warp` holds it as knots.
//
// WHAT THIS ENGINE DOES WITH IT — and the part that is NOT the same question.
// This pipeline evaluates every mask in the PRE-lens-correction frame and then
// resamples the whole frame through the geometry stage (`develop` applies masks
// at line ~379 and `apply_lens_geometry` at ~404; the comment there states the
// order outright). A mask this engine draws is therefore carried by the
// distortion field exactly as the pixels are, without anyone applying `m`:
//
//   | mask kind   | downstream geometry | engine frame operation              |
//   |-------------|---------------------|-------------------------------------|
//   | brush       | either              | IDENTITY                            |
//   | radial      | active              | m_lr⁻¹ composed with T_engine      |
//   | radial      | inactive            | IDENTITY (stored coordinates)       |
//   | linear      | active              | sample at T_engine(p), no m_lr      |
//   | linear      | inactive            | D_fwd(z), D_fwd(f), rebuild straight|
//   | bitmap / AI | either              | IDENTITY                            |
//
// The brush row is why nothing here is wired into `mask_weight` for a dab:
// applying `m` to a dab centre AND letting the geometry stage move it would
// apply the field twice, putting a 24 mm corner dab ~186 px past where
// Lightroom puts it.
//
// The radial row WAS a live mismatch — every imported radial on a
// profile-corrected photo rendered up to 186 px (24 mm) / 88 px (105 mm) away
// from where Lightroom puts it. `MaskFrame` + `MaskUnwarp` now compose the two
// independent maps exactly once: `T_engine` cancels the downstream engine
// resample at rasterisation time, while `m_lr⁻¹` asks the stored Lightroom
// geometry at the exported point its own model predicts. Omitting either map
// leaves a whole correction field; repeating either double-counts it.
// The LINEAR rows are H2. Their zero-parameter absolute residual remains
// 9.748/7.025/6.336 px RMS for the active/stored line and
// 12.449/9.943/4.979 px for the inactive/transported line. They are topology
// evidence, not 1 px closure; the fitted anisotropic aspect candidate is not
// implemented. The named LINEAR tests pin active placement, all three wall
// handle pairs, and straightness independently of the RADIAL tests below.
//
// THE RADIUS, decided here because there is nowhere better. A brush dab is a
// circle of radius `r` and the map is not conformal, so a warped dab is an
// ELLIPSE: the local Jacobian's tangential eigenvalue is `m(r)` and its radial
// one is `d(r·m)/dr`, and they differ by up to 6.15 % at 24 mm and 7.66 % at
// 105 mm — MORE than the isotropic part a radius scale would fix (3.25 % and
// 4.25 %). No scalar radius can represent that, so no scalar is the right
// answer, and nothing in the measurement set observes a dab RADIUS at all (the
// eleven-dab ladder measures centres). The map below therefore takes POINTS and
// has no radius argument: the exact way to warp a brush mask is to rasterise
// the stroke in its stored frame and resample the resulting PLANE through
// `m` — which is precisely what this engine's geometry stage already does to
// every mask it draws.

/// [`LensProfile::mask_warp`]'s interpolator: the radial magnification at
/// normalised radius `rn` (1 = the corner half-diagonal).
///
/// The SAME spline the in-camera `distortion` knots are read with, deliberately
/// — the two sources write one field and a second interpolator is how two
/// conventions get in.
///
/// [`LensProfile::mask_warp`]: crate::recipe::LensProfile::mask_warp
pub fn mask_warp_factor(knots: &[f32], rn: f32) -> f32 {
    if knots.is_empty() {
        return 1.0;
    }
    profile_knot_interp(knots, rn)
}

/// Solve the mask warp from the IN-CAMERA knots — source A.
///
/// The camera's `distortion` spline is a BACKWARD map like Adobe's: at
/// corrected radius `rn` the source sample sits at `rn · g(rn)/s_p`. A mask
/// stored in the source frame needs the other direction, so this inverts that
/// map at each knot radius by bisection on its rising prefix — the same
/// construction (and the same fold guard) [`lens_ungeom_norm`] uses, because it
/// is the same inversion.
///
/// Deliberately NOT composed with the manual `lens_distortion` slider or the
/// CA fill scale. Those are this engine's own edit and this engine's own
/// resampling artefact; `mask_warp` models what LIGHTROOM's correction did, and
/// folding our slider into it would make the answer depend on the user's later
/// choices.
///
/// Empty in ⇒ empty out: no knots is not a warp of 1.0, it is no answer, and
/// `MaskWarpSource` is where that difference is stated.
pub fn mask_warp_from_camera_knots(distortion: &[f32], dims: (f32, f32), n: usize) -> Vec<f32> {
    if distortion.is_empty() || n < 2 {
        return Vec::new();
    }
    let s_p = profile_fill_scale(distortion, dims);
    let fwd = |rn: f32| rn * profile_knot_interp(distortion, rn) / s_p;
    // Peak scan first: past the fold the map is no longer injective and a
    // bisection would land on an arbitrary preimage.
    let mut hi_max = 2.0f32;
    let mut peak = 0.0f32;
    for i in 1..=256 {
        let rn = 2.0 * i as f32 / 256.0;
        let v = fwd(rn);
        if v < peak {
            hi_max = 2.0 * (i - 1) as f32 / 256.0;
            break;
        }
        peak = v;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let rho = (i as f32 + 0.5) / (n - 1) as f32;
        if fwd(hi_max) <= rho {
            // Beyond what the map reaches: clamp at the peak, exactly as
            // `lens_ungeom_norm` does, rather than extrapolate a factor.
            out.push(hi_max / rho);
            continue;
        }
        let (mut lo, mut hi) = (0.0f32, hi_max);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if fwd(mid) < rho {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        out.push(0.5 * (lo + hi) / rho);
    }
    out
}

/// STORED mask point → the point Lightroom EXPORTED it at, both normalised
/// 0..1 on the frame corner origin.
///
/// Identity when the profile carries no solved warp, which is the honest answer
/// for a photo whose frame nobody could model — see
/// [`crate::recipe::MaskWarpSource`] for which of the five "no warp" states
/// applies and why they are not one state.
///
/// Read the block header above before wiring this into a render path: which
/// mask kinds need it, and in which direction, depends on where that path
/// evaluates masks, and for THIS engine's own `mask_weight` the answer for a
/// brush is identity.
pub fn lr_mask_warp_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
) -> (f32, f32) {
    if profile.mask_warp.is_empty() {
        return (nx, ny);
    }
    let (w, h) = dims;
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let [cx, cy] = lr_mask_center_px(dims, profile);
    let (dx, dy) = (nx * w - cx, ny * h - cy);
    let f = mask_warp_factor(&profile.mask_warp, (dx * dx + dy * dy).sqrt() / rr);
    ((dx * f + cx) / w.max(1e-6), (dy * f + cy) / h.max(1e-6))
}

/// EXPORTED point → the point Lightroom STORED it as: the numeric inverse of
/// [`lr_mask_warp_norm`], by the same rising-prefix bisection
/// [`lens_ungeom_norm`] uses.
///
/// This is the Lightroom half of the sample composition for a radial or
/// linear geometry. [`MaskUnwarp`] calls it after the engine-map half; brush,
/// bitmap and AI geometry never take that arm.
pub fn lr_mask_unwarp_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
) -> (f32, f32) {
    unwarp_norm_over(nx, ny, dims, profile, &profile.mask_warp)
}

/// The numeric inverse itself, over WHICHEVER spline the caller names.
///
/// The radial and LINEAR arms are the same 45 lines — same centre, same fold
/// guard, same 40-step bisection law — differing only in where the knots come
/// from, so the knots are the parameter. They were a copy while the linear arm
/// was being settled (D2's handle-transport rule could not take a second knot
/// source without touching the byte-for-byte settled radial path); the copy
/// outlived that reason, and a fold guard that exists twice is a fold guard
/// that can be fixed once.
fn unwarp_norm_over(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    knots: &[f32],
) -> (f32, f32) {
    if knots.is_empty() {
        return (nx, ny);
    }
    let (w, h) = dims;
    let rr = (0.5 * (w * w + h * h).sqrt()).max(1e-6);
    let [cx, cy] = lr_mask_center_px(dims, profile);
    let (dx, dy) = (nx * w - cx, ny * h - cy);
    let rho = (dx * dx + dy * dy).sqrt() / rr;
    if rho < 1e-6 {
        return (nx, ny);
    }
    let fwd = |rn: f32| rn * mask_warp_factor(knots, rn);
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
        return ((dx * f + cx) / w.max(1e-6), (dy * f + cy) / h.max(1e-6));
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
    let f = 0.5 * (lo + hi) / rho;
    ((dx * f + cx) / w.max(1e-6), (dy * f + cy) / h.max(1e-6))
}

/// LINEAR handle-only numeric inverse over the RETAINED camera spline.
///
/// Separate from the radial arm by its knot SOURCE, not by a copy: both go
/// through [`unwarp_norm_over`], so centre, radius, fold guard and bisection
/// law cannot drift apart. What stays distinct is which spline a handle is
/// transported over — the whole point of D2's H2 rule.
fn linear_handle_unwarp_norm(
    nx: f32,
    ny: f32,
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
    knots: &[f32],
) -> (f32, f32) {
    unwarp_norm_over(nx, ny, dims, profile, knots)
}

/// Lightroom's full-raw centre in the dimensions currently being rendered.
/// Legacy recipes carry no frame fact and retain stored-frame-centre behaviour.
fn lr_mask_center_px(
    dims: (f32, f32),
    profile: &crate::recipe::LensProfile,
) -> [f32; 2] {
    let (w, h) = dims;
    profile.mask_warp_center.map_or([w * 0.5, h * 0.5], |c| {
        [c.stored_px[0] * w / c.stored_dims[0], c.stored_px[1] * h / c.stored_dims[1]]
    })
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
        let dir = std::env::temp_dir().join(format!("autoshade-source-px-{}", std::process::id()));
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

    /// R27 P3 — the baked develop takes a working-resolution cap, and obeys
    /// the RAW arm's two rules for it: bound the LONG edge, and only ever go
    /// DOWN.
    ///
    /// Before this the baked arm had no cap while the RAW arm had one (the
    /// format map's B2), so a 60 MP TIFF developed at full size wherever it
    /// was developed at all. The downscale-only half matters as much as the
    /// bound: plain `thumbnail` UPSCALES a source smaller than the cap, which
    /// would invent pixels and then hand them on as a developed master.
    ///
    /// MUTATION THIS CATCHES: use `resize` instead of `thumbnail`'s
    /// downscale-only guard and the third assertion inflates the 64×48 source
    /// to 2048×1536; cap AFTER the develop instead of before and the first
    /// assertion still passes while the memory saving — the entire point —
    /// silently disappears, which is why the fourth assertion pins that the
    /// tone stage saw the SMALL frame.
    #[test]
    fn the_baked_develop_caps_its_working_resolution_downward_only() {
        let big = DynamicImage::ImageRgb8(image::RgbImage::from_fn(400, 200, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 7])
        }));
        let r = EditRecipe::default();

        // Uncapped = the source's own resolution (what the export path asks).
        assert_eq!(
            render_baked_to_image(&big, &r, None, None, &crate::diag::pixels()).unwrap().dimensions(),
            (400, 200)
        );
        // Capped on the LONG edge, aspect kept.
        assert_eq!(
            render_baked_to_image(&big, &r, None, Some(100), &crate::diag::pixels()).unwrap().dimensions(),
            (100, 50)
        );
        // Never upsampled.
        let small = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 48, image::Rgb([3, 4, 5])));
        assert_eq!(
            render_baked_to_image(&small, &r, None, Some(2048), &crate::diag::pixels()).unwrap().dimensions(),
            (64, 48)
        );

        // The cap runs BEFORE the develop, not after: a crop is expressed in
        // normalised coordinates on the developed frame, so if the shrink
        // happened last the crop would be taken from the 400-wide frame and
        // then shrunk, landing on 100×50 either way. Ask instead for a frame
        // whose SIZE reveals the order — a half-width crop of a capped
        // develop is 50 px; of an uncapped one shrunk afterwards it would be
        // 200 px before the shrink and the function returns the crop, not the
        // cap, so the two disagree.
        let cropped = EditRecipe {
            crop: Some(crate::recipe::Crop { left: 0.0, top: 0.0, right: 0.5, bottom: 1.0 }),
            ..Default::default()
        };
        assert_eq!(
            render_baked_to_image(&big, &cropped, None, Some(100), &crate::diag::pixels()).unwrap().dimensions(),
            (50, 50),
            "the crop must be taken from the CAPPED frame — i.e. the cap ran first"
        );
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

    /// v1.2.2: the base-look estimator pairs the rendition against the frame
    /// it SHOWS. A 300x200 neutral with dark side strips and a "camera"
    /// rendition that is its centred 4:3 crop, pixel for pixel: paired whole,
    /// the strips' mass sits on one side of the CDF match only and a curve
    /// appears where there is none; paired on the camera's frame the pair is
    /// the identity it is. A same-frame pair passes through untouched.
    #[test]
    fn the_base_look_is_estimated_on_the_frame_the_camera_shows() {
        let neutral = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(300, 200, |x, _| {
            if !(16..283).contains(&x) {
                image::Rgb([20, 20, 20])
            } else {
                let v = (x * 255 / 300) as u8;
                image::Rgb([v, v, v])
            }
        }));
        let camera = neutral.crop_imm(16, 0, 267, 200);
        assert!(
            !camera_base_knots(&neutral, &camera).expect("judgeable").is_empty(),
            "paired whole, the edge strips read as a camera curve"
        );
        let paired = camera_frame_of(&neutral, &camera);
        assert_eq!((paired.width(), paired.height()), (267, 200));
        assert!(
            camera_base_knots(&paired, &camera).expect("judgeable").is_empty(),
            "paired on the frame the camera shows, an identical crop is the identity"
        );
        let same = camera_frame_of(&neutral, &neutral.thumbnail(150, 100));
        assert_eq!((same.width(), same.height()), (300, 200), "a same-frame pair is untouched");
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

    /// Real-machine probe, never run in CI: point AUTOSHADE_PROBE_RAW at an
    /// ARW and run with `--ignored` to check the whole base-look chain on a
    /// real photo — estimator knots + the luma median of the base-curved
    /// render vs the camera's own preview (they must land close).
    #[test]
    #[ignore = "real-machine probe: set AUTOSHADE_PROBE_RAW to an ARW path"]
    fn probe_real_raw_base_look() {
        let Some(raw) = crate::config::live_env("AUTOSHADE_PROBE_RAW") else {
            panic!("set AUTOSHADE_PROBE_RAW to a RAW path");
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
        let dir = std::env::temp_dir().join(format!("autoshade-crop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.png");
        img.save(&src).unwrap();
        let out_p = dir.join("cropped.png");
        let r = EditRecipe { crop: Some(c), ..Default::default() };
        let (w, h) = render_to_file(&src, &r, &out_p, None, None, crate::diag::stderr()).unwrap();
        assert_eq!((w, h), (60, 30), "the baked export applies the SAME rectangle");
        assert_eq!(image::image_dimensions(&out_p).unwrap(), (60, 30));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_publishes_atomically_and_leaves_no_staging_file() {
        let dir = std::env::temp_dir().join(format!("autoshade-export-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.png");
        DynamicImage::ImageRgb8(image::RgbImage::new(4, 3)).save(&src).unwrap();
        let out = dir.join("shot.developed.png");
        let r = EditRecipe::default();
        render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).unwrap();
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
        render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).unwrap();
        assert!(out.exists());

        // A PRE-STAGING failure: an unknown extension is rejected at format
        // resolution before any file is created — the target must survive
        // and no staging litter may appear. (The old comment claimed this
        // failed "after staging"; it never did — the REAL post-staging case
        // follows below, R12.)
        let keeper = dir.join("keeper.unknownext");
        std::fs::write(&keeper, b"a previous deliverable").unwrap();
        let err = render_to_file(&src, &r, &keeper, None, None, crate::diag::stderr());
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
            let err = render_to_file(&src, &r, &ro, None, None, crate::diag::stderr());
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
        // Producers normalise at PIXEL CENTRES, `(x + MASK_SAMPLE_CENTRE)/w`;
        // the sampler must read on the matching TEXEL-CENTRE grid, or a
        // frame-sized mask loses its last row/column, its placement drifts
        // with resolution, and the whole family sits half a texel out.
        let mut m = image::GrayImage::new(2, 1);
        m.put_pixel(0, 0, image::Luma([0]));
        m.put_pixel(1, 0, image::Luma([255]));
        // The two pixel positions a 2-wide FRAME produces — 0.5/2 and 1.5/2 —
        // are exactly the two TEXEL centres, so both are exact hits with no
        // interpolation: sx = nx·2 − 0.5 = 0 and 1.
        assert_eq!(sample_gray_norm(&m, 0.5 / 2.0, 0.0), 0.0);
        assert_eq!(sample_gray_norm(&m, 1.5 / 2.0, 0.0), 1.0, "last texel must be reachable");
        // Resolution independence: an 8-wide frame over the same 2-wide mask.
        // Pixel 7 sits at nx = 7.5/8 → sx = 1.875 − 0.5 = 1.375, past the last
        // texel centre → clamp-to-edge → full coverage. Pixel 0 sits at
        // nx = 0.5/8 → sx = −0.375 → clamp → nothing.
        assert_eq!(sample_gray_norm(&m, 7.5 / 8.0, 0.0), 1.0);
        assert_eq!(sample_gray_norm(&m, 0.5 / 8.0, 0.0), 0.0);
        // …and the interpolated interior is SYMMETRIC about the frame centre,
        // which is the half-pixel convention made visible on this arm. Every
        // value here is dyadic, so the arithmetic is exact in f32:
        //   pixel 3 → nx = 3.5/8 = 0.4375 → sx = 0.875 − 0.5 = 0.375 → 0.375
        //   pixel 4 → nx = 4.5/8 = 0.5625 → sx = 1.125 − 0.5 = 0.625 → 0.625
        // Under the refuted `x/w` reading they are 0.75 and 1.0 — a pair that
        // is neither symmetric nor even distinct at the top end.
        let p3 = sample_gray_norm(&m, 3.5 / 8.0, 0.0);
        let p4 = sample_gray_norm(&m, 4.5 / 8.0, 0.0);
        assert_eq!(p3, 0.375);
        assert_eq!(p4, 0.625);
        assert_eq!(p3 + p4, 1.0, "the ramp must be symmetric about the frame centre");
    }

    /// **The half-pixel convention itself, on every arm of the family that can
    /// carry one** — the R29 C2 pin, and the one test built so that reverting
    /// [`MASK_SAMPLE_CENTRE`] to 0 fails it four separate ways.
    ///
    /// The measurement is in that constant's doc (two Lightroom captures, both
    /// putting a nominally centred radial's centre at pixel-index 3119.5 on a
    /// 6240-wide frame). What is pinned HERE is that the engine's own arms all
    /// implement it, and each fixture is chosen so the two readings disagree by
    /// a whole feature rather than by a rounding:
    ///
    /// | arm | fixture | at pixel centres | at `x/w` |
    /// |---|---|---|---|
    /// | Radial | ellipse = the middle half of a 4 × 4 frame | a centred 2 × 2 block | ONE off-centre pixel (measured) |
    /// | Linear | ramp from row 0's centre to row 3's centre | 0, ⅓, ⅔, 1 | 0, ⅙, ½, ⅚ — never reaches full |
    /// | Bitmap | a 2-wide raster over a 4-wide frame | 0, ¼, ¾, 1 (symmetric) | 0, ½, 1, 1 (saturates early) |
    /// | Brush | one dab at `d 0.5 0.5` on a 16 × 16 frame | mirror-symmetric alpha | the dab lands ON texel 8 |
    ///
    /// Both frame producers are exercised: [`mask_coverage`]'s loop reads the
    /// weights directly, and `apply_masks`' own `weight_at` is pinned through a
    /// real develop at the end — they are separate lines of code and a mutation
    /// of either one alone must be caught.
    #[test]
    fn every_mask_family_samples_at_pixel_centres() {
        use crate::recipe::{EditRecipe, LocalAdjustment, MaskGeometry};
        let flat = |n: u32| {
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(n, n, image::Rgb([128, 128, 128])))
        };

        // --- (a) RADIAL -----------------------------------------------------
        // Ellipse centred at (0.5, 0.5) with rx = ry = 0.25, hard edge. On a
        // 4 × 4 frame the pixel centres are nx ∈ {0.125, 0.375, 0.625, 0.875},
        // so (nx − 0.5)/0.25 ∈ {−1.5, −0.5, +0.5, +1.5} and
        // d² ∈ {0.5, 2.5, 4.5}: the four pixels with d = √0.5 = 0.707 are
        // inside and the twelve with d ≥ 1.58 are outside. A centred 2 × 2
        // block, and `radial_falloff(0, d)` is the exact step `d < 1`, so the
        // coverage bytes are exactly 255 and 0 with nothing to round.
        let ell = LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: 0.25,
                left: 0.25,
                bottom: 0.75,
                right: 0.75,
                feather: 0.0,
                roundness: 0.0,
                flipped: false,
                angle: 0.0,
                midpoint: 50.0,
                mask_version: 2,
            },
            ..Default::default()
        };
        let cov = mask_coverage(&ell, &flat(4), MaskFrame::AsRendered);
        let inside: Vec<(u32, u32)> = (0..4)
            .flat_map(|y| (0..4).map(move |x| (x, y)))
            .filter(|&(x, y)| cov.get_pixel(x, y)[0] == 255)
            .collect();
        // At the refuted `x/w` the offsets are {−2, −1, 0, +1} instead, so
        // d² ∈ {0, 1, 2, …} and the four neighbours land at d = 1 EXACTLY,
        // which the strict `d < 1` hard edge excludes: the whole mask collapses
        // to the single pixel (2, 2). Measured, not predicted — reverting
        // `MASK_SAMPLE_CENTRE` prints `left: [(2, 2)]` here.
        assert_eq!(
            inside,
            vec![(1, 1), (2, 1), (1, 2), (2, 2)],
            "a centred ellipse must cover a CENTRED block, not one corner-anchored pixel"
        );
        for y in 0..4 {
            for x in 0..4 {
                let v = cov.get_pixel(x, y)[0];
                assert!(v == 0 || v == 255, "a hard edge has no partial texel: ({x},{y}) = {v}");
            }
        }

        // --- (b) LINEAR -----------------------------------------------------
        // Zero end on row 0's centre (ny = 0.125), full end on row 3's centre
        // (ny = 0.875). vy = 0.75, len2 = 0.5625, so the weights are
        // (ny − 0.125)·0.75/0.5625 = 0, 1/3, 2/3, 1 — every operand dyadic, and
        // the last one EXACTLY 1. Eased bytes: 0, round(66.0) = 66,
        // round(189.0) = 189, 255.
        let ramp = LocalAdjustment {
            mask: MaskGeometry::Linear {
                zero_x: 0.5, zero_y: 0.125, full_x: 0.5, full_y: 0.875,
            },
            ..Default::default()
        };
        let lcov = mask_coverage(&ramp, &flat(4), MaskFrame::AsRendered);
        let column: Vec<u8> = (0..4).map(|y| lcov.get_pixel(2, y)[0]).collect();
        assert_eq!(column, vec![0, 66, 189, 255], "the eased ramp must span its ends exactly");

        // --- (c) BITMAP -----------------------------------------------------
        // A 2-wide raster [0, 255] read by a 4-wide frame. Texel centres sit at
        // nx = 0.25 and 0.75; the frame's pixel centres at 0.125/0.375/0.625/
        // 0.875 give sx = nx·2 − 0.5 = −0.25, 0.25, 0.75, 1.25, which clamp to
        // 0, 0.25, 0.75, 1 — symmetric about the frame centre, and reaching
        // BOTH ends. The `Bitmap` arm ignores its path when a raster is handed
        // in, so this needs no file.
        let mut ras = image::GrayImage::new(2, 1);
        ras.put_pixel(0, 0, image::Luma([0]));
        ras.put_pixel(1, 0, image::Luma([255]));
        let bmp = MaskGeometry::Bitmap { path: "unused — the raster is passed in".into() };
        let row: Vec<f32> = (0..4u32)
            .map(|x| {
                mask_weight(&bmp, (x as f32 + MASK_SAMPLE_CENTRE) / 4.0, 0.5, Some(&ras))
            })
            .collect();
        assert_eq!(row, vec![0.0, 0.25, 0.75, 1.0]);
        assert_eq!(row[0] + row[3], 1.0, "the two ends must mirror");
        assert_eq!(row[1] + row[2], 1.0, "…and so must the interior");

        // --- (d) BRUSH ------------------------------------------------------
        // One dab at the exact frame centre of a 16 × 16 frame. `rasterise_
        // brush_group` stamps it at texel coordinate 0.5·16 − 0.5 = 7.5, i.e.
        // BETWEEN texels 7 and 8, so the alpha is mirror-symmetric about the
        // frame centre; the frame then reads texel x exactly (16 is a power of
        // two, so (x + 0.5)/16 · 16 − 0.5 = x with no rounding at all). At
        // `x/w` the dab would land ON texel 8 and the mirror would break —
        // texel 4 falls exactly on ρ = 1 and reads 0 while its partner texel 11
        // is still lit.
        let dab = probe_brush(&[(1.0, 0.25, 1.0, 0.5, "d 0.5 0.5")]);
        let braster = brush_raster(&dab, 16, 16).expect("one dab");
        assert_eq!(braster.dimensions(), (16, 16), "small frame = 1:1 raster");
        let at = |x: u32, y: u32| {
            mask_weight(
                &dab,
                (x as f32 + MASK_SAMPLE_CENTRE) / 16.0,
                (y as f32 + MASK_SAMPLE_CENTRE) / 16.0,
                Some(&braster),
            )
        };
        for x in 0..16u32 {
            assert_eq!(at(x, 7), at(15 - x, 7), "the dab is not mirrored in x at column {x}");
            assert_eq!(at(7, x), at(7, 15 - x), "the dab is not mirrored in y at row {x}");
        }
        // The mirror is only meaningful if the dab is actually THERE and the
        // pair straddling the centre share the peak (a single peak texel is the
        // `x/w` signature).
        assert!(at(7, 7) > 0.9, "premise: the dab covers the centre: {}", at(7, 7));
        assert_eq!(at(7, 7), at(8, 8), "the centre must be shared, not owned by one texel");
        assert_eq!(at(0, 7), 0.0, "…and the dab still ends: {}", at(0, 7));

        // --- both frame producers -------------------------------------------
        // `mask_coverage` above is the OVERLAY's loop. `apply_masks`' own
        // `weight_at` is a separate line, so pin it on the same radial: exactly
        // the centred 2 × 2 may move.
        let r = EditRecipe {
            masks: vec![LocalAdjustment { exposure_ev: -4.0, ..ell }],
            ..Default::default()
        };
        let mut data = vec![[0.6_f32; 3]; 16];
        apply_develop_anon(&mut data, 4, 4, &r);
        let moved: Vec<usize> =
            (0..16).filter(|&i| (data[i][0] - 0.6).abs() > 1e-4).collect();
        assert_eq!(
            moved,
            vec![5, 6, 9, 10],
            "the render's own producer must agree with the overlay's"
        );
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
        apply_develop_anon(&mut foam, 1, 1, &EditRecipe::default());
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
        r.temper(crate::recipe::GradeStrength::calibrated());
        let lum = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        let mut foam = vec![[0.90_f32, 0.93, 0.96]];
        apply_develop_anon(&mut foam, 1, 1, &r);
        assert!(lum(foam[0]) > 0.80, "foam crushed (should stay light): luma {}", lum(foam[0]));
        let mut water = vec![[0.35_f32, 0.62, 0.66]];
        apply_develop_anon(&mut water, 1, 1, &r);
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
        apply_develop_anon(&mut data, 1, 1, &r);
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
        apply_develop_anon(&mut data, w, h, &r);
        // The four rows sample at their CENTRES, ny = (y + 0.5)/4, so the
        // weights are exactly 1/8, 3/8, 5/8, 7/8 — the top row is no longer
        // AT the gradient's zero end, it is an eighth of the way past it, and
        // it darkens according to the shipped eased profile (R29 C2's
        // half-pixel move, visible here). Pinned EXACTLY rather than bounded:
        // a degenerate linear (zero == full) renders weight 1 everywhere, so
        // this comparison carries the identical eased weight through every
        // stage below it.
        let top_weight = linear_coverage(0.125, LinearFalloff::Eased);
        let eighth = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 },
                amount: top_weight,
                exposure_ev: -4.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut eighth_px = vec![[0.6_f32, 0.6, 0.6]; 1];
        apply_develop_anon(&mut eighth_px, 1, 1, &eighth);
        assert_eq!(data[0], eighth_px[0], "top row carries exactly its eased coverage");
        assert!(data[0][0] < 0.6, "…which is a real darkening: {}", data[0][0]);
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
        apply_develop_anon(&mut control, w, h, &EditRecipe::default());
        let right0 = var(&data, 4..8);
        apply_develop_anon(&mut data, w, h, &r);
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
        apply_develop_anon(&mut control, 3, 1, &EditRecipe::default());
        let mut data = vec![dark, mid, bright];
        apply_develop_anon(&mut data, 3, 1, &r);
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
        apply_develop_anon(&mut control, 4, 1, &EditRecipe::default());
        let mut data = vec![orange, dark_orange, blue, grey];
        apply_develop_anon(&mut data, 4, 1, &r);
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
        // red / cut blue inside the mask; a row of weight EXACTLY 0 must stay
        // equal to a mask-less control render (the mask pass skips it).
        //
        // The gradient's zero end sits at `zero_y = 0.125`, which is the TOP
        // ROW'S CENTRE on this 4-row frame (R29 C2: rows sample at
        // ny = (y + 0.5)/4). It has to, for the skip to be exercised at all —
        // under pixel-centre sampling no row of a 0→1 gradient carries weight
        // 0, and the old `zero_y = 0.0` fixture stopped testing the skip the
        // moment the convention was corrected. The weights here are exactly
        // 0, 1/3, 2/3, 1: `(ny − 0.125)·0.75 / 0.5625`, all dyadic.
        let r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear {
                    zero_x: 0.5, zero_y: 0.125, full_x: 0.5, full_y: 0.875,
                },
                amount: 1.0,
                temperature: 100.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let grey = [0.5_f32; 3];
        let (w, h) = (1usize, 4usize);
        let mut control = vec![grey; w * h];
        apply_develop_anon(&mut control, w, h, &EditRecipe::default());
        let mut data = vec![grey; w * h];
        apply_develop_anon(&mut data, w, h, &r);
        assert_eq!(data[0], control[0], "zero end of the gradient: the mask must skip it");
        let px = data[3];
        assert!(
            px[0] > grey[0] + 0.02 && px[2] < grey[2] - 0.02,
            "full end must warm (red up, blue down): {px:?}"
        );
        // …and the SAME geometry read on the old `y/h` grid would give the top
        // row weight (0 − 0.125)·0.75/0.5625 < 0 → clamped to 0 as well, so the
        // skip alone cannot separate the two conventions. The bottom row can:
        // at ny = 0.875 the weight is exactly 1, while `y/h` puts row 3 at
        // ny = 0.75 → weight 5/6, short of the full end. Assert the full end is
        // REACHED by comparing against an amount-1 whole-frame mask.
        let full = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 },
                amount: 1.0,
                temperature: 100.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut full_px = vec![grey; 1];
        apply_develop_anon(&mut full_px, 1, 1, &full);
        assert_eq!(data[3], full_px[0], "the last row's centre IS the gradient's full end");
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
        apply_develop_anon(&mut global, 3, 1, &EditRecipe::default());
        let mut local = src.to_vec();
        apply_develop_anon(
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
            render_to_file(src_p, &neutral, std::path::Path::new(name), None, None, crate::diag::stderr()).unwrap();
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
                render_to_file(src_p, &neutral, std::path::Path::new(name), None, Some(&opts), crate::diag::stderr()).unwrap();
                let bytes = std::fs::read(name).unwrap();
                assert!(
                    bytes.windows(profile.len()).any(|win| win == profile),
                    "{name} must embed the full {space:?} profile ({} B)",
                    profile.len()
                );
            }
            let png_name = "out/_gamut.png";
            render_to_file(src_p, &neutral, std::path::Path::new(png_name), None, Some(&opts), crate::diag::stderr()).unwrap();
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
            render_to_file(src_p, &neutral, std::path::Path::new("out/_export_le50.png"), None, Some(&small), crate::diag::stderr())
                .unwrap();
        assert_eq!((w, h), (50, 25), "long edge 50 must fit 200×100 to 50×25");
        let saved = image::image_dimensions("out/_export_le50.png").unwrap();
        assert_eq!(saved, (50, 25), "saved file dims must match the report");

        let big = ExportOpts { long_edge: Some(400), ..Default::default() };
        let (w, h) =
            render_to_file(src_p, &neutral, std::path::Path::new("out/_export_le400.png"), None, Some(&big), crate::diag::stderr())
                .unwrap();
        assert_eq!((w, h), (200, 100), "long edge beyond source must NOT upscale");

        for (q, name) in [(30u8, "out/_export_q30.jpg"), (95u8, "out/_export_q95.jpg")] {
            let opts = ExportOpts { jpeg_quality: q, ..Default::default() };
            render_to_file(src_p, &neutral, std::path::Path::new(name), None, Some(&opts), crate::diag::stderr()).unwrap();
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
            render_to_file(edge_p, &neutral, std::path::Path::new(name), None, Some(&opts), crate::diag::stderr()).unwrap();
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

    /// R29 Batch-2 acceptance ①: WHAT `--long-edge` is, stated as an equality.
    ///
    /// The CLI's new `--long-edge N` reaches [`ExportOpts::long_edge`], which
    /// `render_to_file` applies as its LAST pixel stage — after the whole
    /// develop, after output sharpening's own input is chosen, before the
    /// encode. So the delivered file is exactly the full-resolution render
    /// resampled, and the equality below says so with no tolerance at all:
    /// `long_edge = k` is Lanczos3 downscale of the k-less render, to the
    /// 16-bit code, at every k.
    ///
    /// That is a REAL claim and not a tautology, because three plausible
    /// implementations break it and one of them was on the table:
    ///
    /// * resampling with a different kernel (`serve`'s preview arm uses
    ///   `Triangle`, `src/serve.rs:860`, while `decode::preview_resized` uses
    ///   `Lanczos3`, `src/decode.rs:2146` — the two spellings already in this
    ///   tree). The CLI flag inherits the EXPORT path's `Lanczos3`
    ///   (`src/render.rs:1564`) because it is that path; no third choice was
    ///   added.
    /// * developing at the capped resolution instead of capping the developed
    ///   pixels — see acceptance ② below for how far apart those two are.
    /// * doing the resize before output sharpening's measurement, or after the
    ///   colour-space transform.
    ///
    /// MEASURED, on the 800×600 fixture below with sharpening 60 / clarity 40 /
    /// texture 50: mean |Δ| = 0.000000 and worst |Δ| = 0 sixteen-bit codes at
    /// both k = 400 and k = 200. Pinned at 0, since anything else means one of
    /// the three above happened.
    #[test]
    fn export_at_size_is_exactly_a_downscale_of_the_full_render() {
        std::fs::create_dir_all("out").ok();
        let src_p = std::path::Path::new("out/_le_src.png");
        // Detail at all three scales the normalised operators work on: a
        // deterministic hash for pixel-scale grain (texture/sharpening), the
        // sinusoids for mid-band structure, the ramp for the tonal range
        // clarity's midtone mask needs. A flat fixture would satisfy this test
        // with every stage deleted.
        RgbImage::from_fn(800, 600, |x, y| {
            let hash = x.wrapping_mul(2_654_435_761u32).wrapping_add(y.wrapping_mul(40_503)) >> 13;
            let grain = (hash & 31) as f32 - 15.5;
            let mid = 40.0 * ((x as f32 / 9.0).sin() + (y as f32 / 7.0).cos());
            let ramp = 90.0 + 100.0 * x as f32 / 800.0;
            let v = |off: f32| (ramp + mid + grain + off).clamp(0.0, 255.0) as u8;
            Rgb([v(0.0), v(-8.0), v(12.0)])
        })
        .save(src_p)
        .unwrap();
        let recipe =
            EditRecipe { sharpening: 60.0, clarity: 40.0, texture: 50.0, ..Default::default() };

        let full_p = std::path::Path::new("out/_le_full.png");
        let (fw, fh) =
            render_to_file(src_p, &recipe, full_p, None, None, crate::diag::stderr()).unwrap();
        assert_eq!((fw, fh), (800, 600), "no export opts = the source's own resolution");
        // 16-bit PNG, so the round trip through the file is lossless and the
        // comparison below measures the RESIZE and nothing else.
        let full = image::open(full_p).unwrap();

        for k in [400u32, 200] {
            let opts = ExportOpts { long_edge: Some(k), ..Default::default() };
            let p = format!("out/_le_{k}.png");
            let (w, h) = render_to_file(
                src_p,
                &recipe,
                std::path::Path::new(&p),
                None,
                Some(&opts),
                crate::diag::stderr(),
            )
            .unwrap();
            assert_eq!(w.max(h), k, "the LONG edge is what the flag bounds");
            assert_eq!((w, h), (k, k * 3 / 4), "…and the aspect ratio is kept");
            let got = image::open(&p).unwrap().to_rgb16();
            let want = full.resize(k, k, image::imageops::FilterType::Lanczos3).to_rgb16();
            assert_eq!(got.dimensions(), want.dimensions());
            let worst = got
                .as_raw()
                .iter()
                .zip(want.as_raw())
                .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
                .max()
                .unwrap();
            assert_eq!(
                worst, 0,
                "long_edge {k} must BE the Lanczos3 downscale of the full render — \
                 worst channel difference {worst} sixteen-bit codes"
            );
        }

        // `Some(0)` is FULL RESOLUTION, not "resize to nothing": the guard is
        // `le > 0` (this file, the `opts.long_edge` block), and the CLI folds
        // its own `--long-edge 0` to `None` on top of that so the two surfaces
        // cannot disagree. Pinned here because a `saturating`-flavoured
        // rewrite of that guard would produce a 1×1 deliverable in silence.
        let zero = ExportOpts { long_edge: Some(0), ..Default::default() };
        let z_p = std::path::Path::new("out/_le_zero.png");
        let (zw, zh) =
            render_to_file(src_p, &recipe, z_p, None, Some(&zero), crate::diag::stderr()).unwrap();
        assert_eq!((zw, zh), (800, 600), "long_edge 0 = full resolution");
        assert_eq!(
            image::open(z_p).unwrap().to_rgb16().as_raw(),
            full.to_rgb16().as_raw(),
            "long_edge 0 must not touch a single pixel"
        );
    }

    /// R29 Batch-2 acceptance ②: the R25 B2 RESOLUTION-NORMALISATION promise,
    /// measured — and the boundary of what acceptance ① above buys.
    ///
    /// The promise (`apply_develop` stages 3/3b/5, and the `texture_pass` doc)
    /// is that clarity, texture and sharpening have radii expressed as a
    /// FRACTION of the frame, so one slider value means the same structure on a
    /// 1280 px preview and on a 61 MP export. Nothing in the tree measured it;
    /// the promise lived in four comments.
    ///
    /// This measures it where it is checkable: clarity's radius is 2 % of the
    /// short edge (`src/render.rs:1833`), and a three-pass box blur of radius r
    /// reaches 3r, so a step edge's halo must be 3 × 0.02 × short-edge px wide —
    /// i.e. the SAME fraction of the frame at both resolutions. Doubling the
    /// working resolution must double the halo in pixels.
    ///
    /// MUTATION THIS KILLS: replacing the radius with any constant (the shape
    /// the code had before R25 B2 — `unsharp_luma(data, w, h, 8, …)`), which
    /// makes the ratio 1.0 instead of 2.0 while every other clarity test in
    /// this file still passes, because they all work at ONE resolution.
    ///
    /// **What this does NOT say, and it matters for `--long-edge`.** Acceptance
    /// ① shows the export resize happens AFTER the develop, so `--long-edge`
    /// never exercises this promise at all: the develop runs at full sensor
    /// resolution and the resampler then averages its halos down with
    /// everything else. Developing at the capped resolution instead is a
    /// visibly different picture, and it is worth knowing by how much.
    ///
    /// The five figures below are PROVENANCED but not gate-checked: they come
    /// from a throwaway harness run once during R29 Batch-2 over the
    /// acceptance-① fixture and functions named here, and no test re-derives
    /// them, so treat them as a recorded observation rather than a pinned
    /// quantity. `render_baked_to_image(max_edge = k)` differs from
    /// the same-resampler downscale of the full develop by a mean 5.7 codes8 at
    /// k = 400 and 19.8 codes8 at k = 200, against 19.1 / 30.0 codes8 for the
    /// entire effect of that recipe (neutral vs graded at the same size), with
    /// a resampler-only control of 0.25 codes8. That is not a defect in either
    /// path — normalised operators are SUPPOSED to place their halos at the
    /// working resolution's scale, so the two disagree by construction — but it
    /// is exactly why the delivery flag resizes finished pixels instead of
    /// quietly reusing the preview path.
    #[test]
    fn the_develop_radius_is_a_fraction_of_the_frame_not_a_pixel_count() {
        // Clarity alone: its radius is the largest of the three, so the halo is
        // the easiest to measure, and its midtone weight is ~0.75 at both
        // plateau levels below (m = 1 − (2l − 1)²), so neither side is starved.
        let recipe = EditRecipe { clarity: 60.0, ..Default::default() };
        // Halo width in PIXELS: walk left from the step and count how far the
        // overshoot is still visible against the far-field plateau.
        let halo = |w: usize, h: usize| -> usize {
            let mut data: Vec<[f32; 3]> = (0..w * h)
                .map(|i| if i % w < w / 2 { [0.25; 3] } else { [0.75; 3] })
                .collect();
            apply_develop_anon(&mut data, w, h, &recipe);
            let row = h / 2;
            // The plateau as DEVELOPED (x = 0 is beyond any halo at these
            // radii), not the literal 0.25 — the tone stage is free to move it.
            let plateau = data[row * w][0];
            (0..w / 2)
                .rev()
                .take_while(|x| (data[row * w + x][0] - plateau).abs() > 1e-3)
                .count()
        };
        // 2 % of the short edge: 24 px at 1200, 12 px at 600 — both clear of
        // the 8 px floor, which would otherwise flatten the ratio by itself.
        let big = halo(1600, 1200);
        let small = halo(800, 600);
        assert!(big > 0 && small > 0, "clarity must produce a halo at all: {big} / {small}");
        // MEASURED: 59 px at 1600×1200, 30 px at 800×600 — ratio 1.967.
        let ratio = big as f64 / small as f64;
        assert!(
            (ratio - 2.0).abs() < 0.15,
            "clarity's radius must scale with the frame: halo {big} px at 1600×1200 vs \
             {small} px at 800×600 (ratio {ratio:.3}, expected 2.0 ± 0.15)"
        );
        // …and each halo really is the reach of a 2 %-of-short-edge blur, not
        // some other quantity that happens to double. Three box passes of
        // radius r reach 3r, so the ceiling is exact; the floor is 70 % of it
        // because the 1e-3 detection threshold above cuts the blur's thin outer
        // tail short (measured 59 of a possible 72, and 30 of 36 — the same
        // fraction at both sizes, which is itself the shape being preserved).
        // A constant radius fails this at 1600×1200 whatever it is set to.
        for (short, measured) in [(1200usize, big), (600, small)] {
            let radius = (0.02 * short as f64).round() as usize;
            let reach = 3 * radius;
            assert!(
                measured <= reach && measured * 10 >= reach * 7,
                "a {short} px short edge gives clarity a {radius} px radius, so the halo must \
                 land in {}..={reach} px — measured {measured}",
                reach * 7 / 10
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
        let dir = std::env::temp_dir().join(format!("autoshade_maskgate_{}", std::process::id()));
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
        let err = render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).unwrap_err().to_string();
        assert!(err.contains("sky"), "the refusal names the mask: {err}");
        assert!(!out.exists(), "a refused export writes nothing");
        // amount = 0 is inert BY the recipe — nothing is being dropped.
        let mut disabled = broken;
        disabled.amount = 0.0;
        let r = EditRecipe { masks: vec![disabled], ..Default::default() };
        render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).expect("disabled mask exports fine");
        // A PARKED mask (default amount 1, every adjustment neutral) renders
        // nothing even with a healthy raster — its lost raster must not
        // block the export either.
        let parked = LocalAdjustment {
            name: "parked".into(),
            mask: MaskGeometry::Bitmap { path: dir.join("gone.png").display().to_string() },
            ..Default::default()
        };
        let r = EditRecipe { masks: vec![parked], ..Default::default() };
        render_to_file(&src, &r, &out, None, None, crate::diag::stderr()).expect("engine-inert mask exports fine");
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
        // Real A7RIV-shaped data (P26 conversions): rising corner gains,
        // falling distortion factors (barrel), near-unity CA.
        let profile = LensProfile {
            vignette: (0..16).map(|i| 1.0 + 0.42 * (i as f32 / 15.0).powi(2)).collect(),
            distortion: (0..16).map(|i| 1.0008 - 0.053 * (i as f32 / 15.0).powi(2)).collect(),
            ca_r: vec![1.0005; 16],
            ca_b: vec![0.9995; 16],
            vignette_on: true,
            distortion_on: true,
            ca_on: true,
            // The mask warp plays no part in the PIXEL maps this test scores —
            // it is a mask-frame quantity and no resampler reads it.
            ..Default::default()
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

    /// The A7RIV frame every mask-frame measurement in R27 Batches 8-10 and
    /// R29's `D` adjudication was made on.
    const MASK_WARP_DIMS: (f32, f32) = (9504.0, 6336.0);

    /// The 24 mm mask warp, from the SAME `.lcp` node the recon solved — one
    /// fixture, so a change to either solver shows up as a disagreement rather
    /// than as two independently drifting sets of numbers.
    fn lcp_24mm_warp() -> Vec<f32> {
        let n = crate::lcp::PerspectiveModel {
            focal_mm: Some(24.0),
            focus_distance: Some(10000.0),
            scale: 1.027391,
            k: [-0.127336, 0.087661, -0.019675],
            focal_x: None,
            sensor_format_factor: 1.0,
        };
        n.mask_warp_knots(MASK_WARP_DIMS, crate::recipe::MASK_WARP_KNOTS).expect("solvable")
    }

    /// Source A and source B are two readings of ONE physical field, so they
    /// have to agree — and the recon says by how much: the camera's own knot
    /// map and Adobe's polynomial score 2.30 px and 2.11 px against the same
    /// 138-patch pixel field, and differ from each other by 6.74 px rms over a
    /// warp that reaches 185.7 px at the corner.
    ///
    /// Asserted as a BAND, both ends. Too-large means one solver drifted; the
    /// too-small end is the one that matters, because a source A that silently
    /// became a copy of source B (or of the identity) would pass every other
    /// test in this file.
    #[test]
    #[allow(clippy::excessive_precision)] // exact decoded 0x7037 fixture decimals
    fn the_two_mask_warp_sources_agree_to_the_measured_tolerance() {
        // `exp_C_ref.ARW`'s OWN `0x7037` array, converted by `lensmeta`'s
        // `v·2⁻¹⁴ + 1` and dumped on this machine — the real A7RIV @ 24 mm
        // barrel profile, not a curve shaped like one.
        let camera_native: Vec<f32> = vec![
            1.0007934570,
            0.9998779297,
            0.9981079102,
            0.9959716797,
            0.9927368164,
            0.9890136719,
            0.9846191406,
            0.9800415039,
            0.9749145508,
            0.9696044922,
            0.9641113281,
            0.9589843750,
            0.9538574219,
            0.9492187500,
            0.9448242188,
            0.9412231445,
        ];
        let camera = crate::lensmeta::resample_sony_distortion(
            &camera_native,
            crate::lensmeta::SONY_DISTORTION_CANONICAL_KNOTS,
        );
        // The engine's own fill scale on that array, against the value the
        // recon computed independently: `profile_fill_scale` is the s_p this
        // whole inversion divides by, so a drift there is a silent drift in
        // every mask-warp knot below.
        assert!(
            (profile_fill_scale(&camera, MASK_WARP_DIMS) - 0.9755544).abs() < 1e-5,
            "fill scale {} vs the zero-parameter model's 0.9755544",
            profile_fill_scale(&camera, MASK_WARP_DIMS)
        );
        let a = mask_warp_from_camera_knots(
            &camera,
            MASK_WARP_DIMS,
            crate::recipe::MASK_WARP_KNOTS,
        );
        let b = lcp_24mm_warp();
        assert_eq!(a.len(), crate::recipe::MASK_WARP_KNOTS);
        assert_eq!(b.len(), crate::recipe::MASK_WARP_KNOTS);
        let half_diag = 0.5 * (MASK_WARP_DIMS.0.hypot(MASK_WARP_DIMS.1));
        let mut worst = 0.0f32;
        for i in 0..crate::recipe::MASK_WARP_KNOTS {
            let r = (i as f32 + 0.5) / (crate::recipe::MASK_WARP_KNOTS - 1) as f32 * half_diag;
            worst = worst.max(((a[i] - b[i]) * r).abs());
        }
        assert!(worst < 40.0, "the two sources diverged by {worst:.1} px");
        // PREMISE: both really are a warp, not the identity dressed up as one.
        for (name, k) in [("camera", &a), ("lcp", &b)] {
            let corner = (k[k.len() - 1] - 1.0) * half_diag;
            assert!(corner.abs() > 100.0, "{name} corner displacement {corner:.1} px is not a warp");
            assert!(k[0] < 0.995, "{name} centre magnification {} is not a warp", k[0]);
        }
    }

    /// ACCEPTANCE ⑦ (engine half). The forward map and its bisection inverse
    /// compose to the identity across the frame — the property every consumer
    /// of a coordinate map in this file is held to.
    #[test]
    fn the_mask_warp_point_map_round_trips() {
        let profile = crate::recipe::LensProfile {
            mask_warp: lcp_24mm_warp(),
            mask_warp_src: crate::recipe::MaskWarpSource::Lcp,
            ..Default::default()
        };
        let mut moved = 0.0f32;
        for i in 0..=10 {
            for j in 0..=10 {
                let (nx, ny) = (i as f32 / 10.0, j as f32 / 10.0);
                let (wx, wy) = lr_mask_warp_norm(nx, ny, MASK_WARP_DIMS, &profile);
                let (bx, by) = lr_mask_unwarp_norm(wx, wy, MASK_WARP_DIMS, &profile);
                assert!((bx - nx).abs() < 1e-4 && (by - ny).abs() < 1e-4, "({nx},{ny})");
                moved = moved.max(((wx - nx) * MASK_WARP_DIMS.0).abs());
            }
        }
        // PREMISE: a round trip through two identities also round-trips.
        assert!(moved > 50.0, "the map moved at most {moved:.1} px — it is not a warp");
        // The frame CENTRE is a fixed point of a radial map, exactly.
        assert_eq!(lr_mask_warp_norm(0.5, 0.5, MASK_WARP_DIMS, &profile), (0.5, 0.5));
    }

    fn d2_camera_profile(native: &[f32]) -> crate::recipe::LensProfile {
        let distortion = crate::lensmeta::resample_sony_distortion(
            native,
            crate::lensmeta::SONY_DISTORTION_CANONICAL_KNOTS,
        );
        crate::recipe::LensProfile {
            mask_warp: mask_warp_from_camera_knots(
                &distortion,
                MASK_WARP_DIMS,
                crate::recipe::MASK_WARP_KNOTS,
            ),
            mask_warp_src: crate::recipe::MaskWarpSource::CameraMetadata,
            mask_warp_center: Some(crate::recipe::MaskWarpCenter {
                stored_px: [4768.0, 3168.0],
                stored_dims: [9504.0, 6336.0],
            }),
            ..Default::default()
        }
    }

    #[allow(clippy::excessive_precision)]
    fn d2_linear_wall_native() -> [f32; 16] {
        [
            1.0007934570,
            0.9998779297,
            0.9981079102,
            0.9959716797,
            0.9927368164,
            0.9890136719,
            0.9846191406,
            0.9800415039,
            0.9749145508,
            0.9696044922,
            0.9641113281,
            0.9589843750,
            0.9538574219,
            0.9492187500,
            0.9448242188,
            0.9412231445,
        ]
    }

    fn d2_linear_probe(
        zero: (f32, f32),
        full: (f32, f32),
    ) -> crate::recipe::LocalAdjustment {
        crate::recipe::LocalAdjustment {
            mask: MaskGeometry::Linear {
                zero_x: zero.0,
                zero_y: zero.1,
                full_x: full.0,
                full_y: full.1,
            },
            ..Default::default()
        }
    }

    fn d2_disabled_linear_profile(
        downstream_geometry: bool,
    ) -> (crate::recipe::LensProfile, crate::recipe::LensProfile) {
        let native = d2_linear_wall_native();
        let camera = d2_camera_profile(&native);
        let mut disabled = camera.clone();
        disabled.distortion = native.to_vec();
        disabled.distortion_on = downstream_geometry;
        disabled.linear_handle_warp = std::mem::take(&mut disabled.mask_warp);
        disabled.mask_warp_src = crate::recipe::MaskWarpSource::DisabledInSidecar;
        disabled.clamp();
        (camera, disabled)
    }

    fn d2_linear_handles(mask: &crate::recipe::LocalAdjustment) -> [(f32, f32); 2] {
        let MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } = &mask.mask else {
            panic!("expected LINEAR fixture")
        };
        [(*zero_x, *zero_y), (*full_x, *full_y)]
    }

    fn d2_midline_x_at_y(handles: [(f32, f32); 2], y: f32, dims: (f32, f32)) -> f32 {
        let [(zx, zy), (fx, fy)] = handles.map(|(x, y)| (x * dims.0, y * dims.1));
        let (mx, my) = ((zx + fx) * 0.5, (zy + fy) * 0.5);
        mx - (y - my) * (fy - zy) / (fx - zx)
    }

    fn d2_midline_y_at_x(handles: [(f32, f32); 2], x: f32, dims: (f32, f32)) -> f32 {
        let [(zx, zy), (fx, fy)] = handles.map(|(x, y)| (x * dims.0, y * dims.1));
        let (mx, my) = ((zx + fx) * 0.5, (zy + fy) * 0.5);
        my - (x - mx) * (fx - zx) / (fy - zy)
    }

    fn d2_coverage_crossing_x(
        mask: &crate::recipe::LocalAdjustment,
        y: f32,
        dims: (f32, f32),
    ) -> f32 {
        let ny = y / dims.1;
        let end = |nx| combined_mask_weight(mask, nx, ny, None, &[], None, dims);
        let increasing = end(1.0) > end(0.0);
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..40 {
            let mid = (lo + hi) * 0.5;
            if (end(mid) < 0.5) == increasing {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) * 0.5 * dims.0
    }

    fn d2_coverage_crossing_y(
        mask: &crate::recipe::LocalAdjustment,
        x: f32,
        dims: (f32, f32),
    ) -> f32 {
        let nx = x / dims.0;
        let end = |ny| combined_mask_weight(mask, nx, ny, None, &[], None, dims);
        let increasing = end(1.0) > end(0.0);
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..40 {
            let mid = (lo + hi) * 0.5;
            if (end(mid) < 0.5) == increasing {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) * 0.5 * dims.1
    }

    fn d2_gray_crossing_x(image: &image::GrayImage, y: u32) -> f32 {
        for x in 0..image.width() - 1 {
            let a = image.get_pixel(x, y)[0] as f32 - 127.5;
            let b = image.get_pixel(x + 1, y)[0] as f32 - 127.5;
            if a != b && a.signum() != b.signum() {
                return x as f32 + 0.5 + (-a) / (b - a);
            }
        }
        panic!("coverage row {y} never crossed 50%")
    }

    fn d2_rgb16_crossing_x_at(image: &DynamicImage, y: u32, target: f32) -> f32 {
        let image = image.to_rgb16();
        let mut range = (u16::MAX, u16::MIN);
        for x in 0..image.width() - 1 {
            range.0 = range.0.min(image.get_pixel(x, y)[1]);
            range.1 = range.1.max(image.get_pixel(x, y)[1]);
            let a = image.get_pixel(x, y)[1] as f32 - target;
            let b = image.get_pixel(x + 1, y)[1] as f32 - target;
            if a != b && a.signum() != b.signum() {
                return x as f32 + 0.5 + (-a) / (b - a);
            }
        }
        panic!("coverage row {y} range {range:?} never crossed target {target}")
    }

    #[test]
    fn linear_with_active_camera_profile_lands_on_the_stored_corrected_frame_line() {
        let (w, h) = (1920u32, 1280u32);
        let dims = (w as f32, h as f32);
        let mut profile = d2_camera_profile(&d2_linear_wall_native());
        profile.distortion = d2_linear_wall_native().to_vec();
        profile.distortion_on = true;
        let mut mask = d2_linear_probe((0.33, 0.5), (0.27, 0.5));
        mask.exposure_ev = -4.0;
        let frame = MaskFrame::downstream(&profile, 0.0);
        assert!(frame.warps(), "premise: camera geometry must be active");

        // Engine-math pin: the output midline q asks the pre-geometry coverage
        // at its source p, and LINEAR's adapter must return q with no LR half.
        let q = (0.30f32, 0.50f32);
        let p = lens_geom_norm(q.0, q.1, dims, &profile, 0.0);
        let unwarp = frame.unwarp(dims).expect("active non-identity engine map");
        let weight = mask_weight_in(&mask.mask, p.0, p.1, None, Some(&unwarp), dims);
        let math_error_px = (weight - 0.5).abs() * 0.06 * dims.0;
        assert!(math_error_px < 0.1, "active LINEAR midline math is {math_error_px:.4}px off");

        // Raster pin: run the exact coverage-then-geometry composition used by
        // the GUI overlay and measure its 50% line in corrected output pixels.
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
        let coverage = DynamicImage::ImageLuma8(mask_coverage(&mask, &base, frame));
        let rendered = apply_lens_geometry(&coverage, &profile, 0.0);
        let got = d2_rgb16_crossing_x_at(&rendered, h / 2, 32767.5);
        let expected = q.0 * dims.0;
        // The public coverage raster is 8-bit, so its rounded 0.5 code adds a
        // measured 0.16 px crossing bias; the float engine-law assertion above
        // is the sub-0.1 px contract, while this pins end-to-end wiring.
        assert!((got - expected).abs() < 0.3, "rendered {got:.4}px vs stored {expected:.4}px");

        // The actual local-adjustment renderer uses the same frame preparation,
        // independently of the coverage-overlay entry point above.
        let recipe = EditRecipe {
            masks: vec![mask],
            lens_profile: profile.clone(),
            ..Default::default()
        };
        let effect = apply_lens_geometry(&develop_preview(&base, &recipe), &profile, 0.0);
        let effect_rgb = effect.to_rgb16();
        let target = 0.5
            * (effect_rgb.get_pixel(0, h / 2)[1] as f32
                + effect_rgb.get_pixel(w - 1, h / 2)[1] as f32);
        let effect_crossing = d2_rgb16_crossing_x_at(&effect, h / 2, target);
        assert!(
            (effect_crossing - expected).abs() < 0.35,
            "active render {effect_crossing:.4}px vs stored {expected:.4}px"
        );
    }

    #[test]
    fn linear_without_downstream_geometry_transports_all_wall_handle_pairs_forward() {
        let (camera, disabled) = d2_disabled_linear_profile(false);
        let (_, active_but_omitted) = d2_disabled_linear_profile(true);
        assert!(disabled.mask_warp.is_empty(), "RADIAL map must be disabled");
        assert_eq!(disabled.linear_handle_warp, camera.mask_warp);
        let frame = MaskFrame::downstream(&disabled, 0.0);
        let omitted_frame = MaskFrame::without_downstream(&active_but_omitted);
        assert!(!frame.warps(), "corrections-off fixture must have no downstream geometry");
        assert!(!omitted_frame.warps(), "the caller explicitly omitted downstream geometry");

        let fixtures = [
            ("L1", (0.33, 0.50), (0.27, 0.50), true, 3168.0),
            ("L2", (0.69, 0.50), (0.75, 0.50), true, 3168.0),
            ("L3", (0.50, 0.27), (0.50, 0.21), false, 4752.0),
        ];
        for (name, zero, full, vertical, along) in fixtures {
            let stored = d2_linear_probe(zero, full);
            let rendered = frame.linear_handles_to_raw(&stored, MASK_WARP_DIMS);
            let got_handles = d2_linear_handles(rendered.as_ref());
            let omitted = omitted_frame.linear_handles_to_raw(&stored, MASK_WARP_DIMS);
            assert_eq!(
                d2_linear_handles(omitted.as_ref()),
                got_handles,
                "{name}: explicit no-downstream path disagrees with inactive profile"
            );
            let expected_handles = [zero, full].map(|(x, y)| {
                lr_mask_unwarp_norm(x, y, MASK_WARP_DIMS, &camera)
            });
            for (which, (got, expected)) in got_handles.iter().zip(expected_handles).enumerate() {
                let error = ((got.0 - expected.0) * MASK_WARP_DIMS.0)
                    .hypot((got.1 - expected.1) * MASK_WARP_DIMS.1);
                assert!(error < 0.01, "{name} handle {which} is {error:.5}px off D_fwd");
            }

            let (got, expected, stored_midline) = if vertical {
                (
                    d2_coverage_crossing_x(rendered.as_ref(), along, MASK_WARP_DIMS),
                    d2_midline_x_at_y(expected_handles, along, MASK_WARP_DIMS),
                    (zero.0 + full.0) * 0.5 * MASK_WARP_DIMS.0,
                )
            } else {
                (
                    d2_coverage_crossing_y(rendered.as_ref(), along, MASK_WARP_DIMS),
                    d2_midline_y_at_x(expected_handles, along, MASK_WARP_DIMS),
                    (zero.1 + full.1) * 0.5 * MASK_WARP_DIMS.1,
                )
            };
            assert!((got - expected).abs() < 0.1, "{name}: render {got:.4}px vs H2 {expected:.4}px");
            let delta = got - stored_midline;
            let expected_delta = match name {
                "L1" => -29.882,
                "L2" => 28.743,
                "L3" => -30.713,
                _ => unreachable!(),
            };
            assert!(
                (delta - expected_delta).abs() < 1.5,
                "{name}: displacement {delta:.3}px, expected {expected_delta:.3}±1.5px"
            );
        }
    }

    #[test]
    fn linear_off_path_rendered_boundary_stays_straight_instead_of_h1_bowing() {
        let (camera, disabled) = d2_disabled_linear_profile(false);
        let (w, h) = (1188u32, 792u32);
        let dims = (w as f32, h as f32);
        let mut mask = d2_linear_probe((0.33, 0.5), (0.27, 0.5));
        mask.exposure_ev = -4.0;
        let frame = MaskFrame::downstream(&disabled, 0.0);
        let coverage = mask_coverage(&mask, &DynamicImage::new_rgb8(w, h), frame);
        let rows = [h / 10, h / 2, h - h / 10 - 1];
        let crossings = rows.map(|y| d2_gray_crossing_x(&coverage, y));
        let sag = crossings[1] - 0.5 * (crossings[0] + crossings[2]);
        assert!(sag.abs() < 0.5, "H2 rendered boundary sagged {sag:.3}px: {crossings:?}");

        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
        let recipe = EditRecipe {
            masks: vec![mask],
            lens_profile: disabled.clone(),
            ..Default::default()
        };
        let effect = develop_preview(&base, &recipe);
        let effect_rgb = effect.to_rgb16();
        let effect_crossings = rows.map(|y| {
            let target = 0.5
                * (effect_rgb.get_pixel(0, y)[1] as f32
                    + effect_rgb.get_pixel(w - 1, y)[1] as f32);
            d2_rgb16_crossing_x_at(&effect, y, target)
        });
        for (y, (got, coverage_got)) in rows.into_iter().zip(effect_crossings.into_iter().zip(crossings)) {
            assert!(
                (got - coverage_got).abs() < 0.5,
                "row {y}: local render {got:.3}px vs coverage {coverage_got:.3}px"
            );
        }

        // Adversarial control: pointwise H1 on this same fixture bows by
        // multiple working-frame pixels, so the straightness gate is not an
        // axis-aligned identity test that both topologies can pass.
        let h1_crossing = |raw_y: f32| {
            let target_y = raw_y / dims.1;
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..40 {
                let mid = (lo + hi) * 0.5;
                if lr_mask_unwarp_norm(0.30, mid, dims, &camera).1 < target_y {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            lr_mask_unwarp_norm(0.30, (lo + hi) * 0.5, dims, &camera).0 * dims.0
        };
        let h1 = rows.map(|y| h1_crossing(y as f32 + 0.5));
        let h1_sag = h1[1] - 0.5 * (h1[0] + h1[2]);
        assert!(
            h1_sag.abs() > 1.5,
            "premise: pointwise H1 sag is only {h1_sag:.3}px on {h1:?}"
        );
    }

    #[test]
    fn radial_with_disabled_profile_and_retained_linear_map_stays_at_stored_coordinates() {
        let (_, disabled) = d2_disabled_linear_profile(true);
        assert!(disabled.geometry_active(), "premise: downstream geometry is active");
        assert!(disabled.mask_warp.is_empty(), "disabled RADIAL path must be identity");
        assert!(!disabled.linear_handle_warp.is_empty(), "premise: LINEAR map was retained");

        let (w, h) = (1920u32, 1280u32);
        let (cx, cy) = (0.30f32, 0.50f32);
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
        let recipe = EditRecipe {
            masks: vec![probe_radial(cx, cy, 0.025)],
            lens_profile: disabled.clone(),
            ..Default::default()
        };
        let rendered = apply_lens_geometry(&develop_preview(&base, &recipe), &disabled, 0.0);
        let got = effect_centroid(&rendered);
        let expected = (cx as f64 * w as f64, cy as f64 * h as f64);
        let error = (got.0 - expected.0).hypot(got.1 - expected.1);
        assert!(error < 1.0, "disabled RADIAL moved {error:.3}px: {got:?} vs {expected:?}");
    }

    #[test]
    #[allow(clippy::excessive_precision)] // exact D2 knot/vector fixture decimals
    fn d2_zero_parameter_camera_model_closes_all_41_measured_vectors() {
        const WALL_NATIVE: [f32; 16] = [
            1.0007934570,
            0.9998779297,
            0.9981079102,
            0.9959716797,
            0.9927368164,
            0.9890136719,
            0.9846191406,
            0.9800415039,
            0.9749145508,
            0.9696044922,
            0.9641113281,
            0.9589843750,
            0.9538574219,
            0.9492187500,
            0.9448242188,
            0.9412231445,
        ];
        const DSC_NATIVE: [f32; 16] = [
            1.0007934570,
            0.9998779297,
            0.9982299805,
            0.9961547852,
            0.9932250977,
            0.9898071289,
            0.9856567383,
            0.9813842773,
            0.9766235352,
            0.9719238281,
            0.9668579102,
            0.9622802734,
            0.9576416016,
            0.9538574219,
            0.9503173828,
            0.9478149414,
        ];
        type Row = (&'static str, (f32, f32), (f32, f32));
        const WALL: [Row; 20] = [
            ("G1", (1710.72, 1267.20), (-20.897, -12.931)),
            ("G2", (4752.00, 1267.20), (-0.011, 31.992)),
            ("G3", (7793.28, 1267.20), (19.025, -12.135)),
            ("G4", (1710.72, 3168.00), (4.924, -0.025)),
            ("G5", (4752.00, 3168.00), (0.244, -0.019)),
            ("G6", (7793.28, 3168.00), (-6.669, 0.026)),
            ("G7", (1710.72, 5068.80), (-20.836, 12.932)),
            ("G8", (4752.00, 5068.80), (0.097, -31.970)),
            ("G9", (7793.28, 5068.80), (19.105, 12.141)),
            ("R1", (2851.20, 2027.52), (24.874, 14.890)),
            ("R2", (6652.80, 3991.68), (-28.390, -12.427)),
            ("centre_S", (4752.00, 3168.00), (0.535, 0.035)),
            ("centre_M", (4752.00, 3168.00), (0.393, 0.009)),
            ("centre_L", (4752.00, 3168.00), (0.303, 0.010)),
            ("edge_S", (1710.72, 3168.00), (4.962, -0.040)),
            ("edge_M", (1710.72, 3168.00), (5.085, -0.083)),
            ("edge_L", (1710.72, 3168.00), (4.990, 0.037)),
            ("corner_S", (1710.72, 1267.20), (-20.927, -12.969)),
            ("corner_M", (1710.72, 1267.20), (-20.710, -12.895)),
            ("corner_L", (1710.72, 1267.20), (-20.880, -12.903)),
        ];
        const DSC: [Row; 21] = [
            ("G1", (1710.72, 1267.20), (-18.900, -11.770)),
            ("G2", (4752.00, 1267.20), (0.140, 29.250)),
            ("G3", (7793.28, 1267.20), (17.480, -11.020)),
            ("G4", (1710.72, 3168.00), (4.610, -0.010)),
            ("G5", (4752.00, 3168.00), (0.300, -0.070)),
            ("G6", (7793.28, 3168.00), (-5.920, -0.020)),
            ("G7", (1710.72, 5068.80), (-18.360, 11.960)),
            ("G8", (4752.00, 5068.80), (0.230, -29.280)),
            ("G9", (7793.28, 5068.80), (17.570, 11.000)),
            ("original", (2283.03, 951.41), (-5.820, -5.100)),
            ("R1", (2851.20, 2027.52), (22.950, 13.600)),
            ("R2", (6652.80, 3991.68), (-26.060, -11.330)),
            ("centre_S", (4752.00, 3168.00), (0.620, -0.080)),
            ("centre_M", (4752.00, 3168.00), (0.420, -0.030)),
            ("centre_L", (4752.00, 3168.00), (0.400, 0.040)),
            ("edge_S", (1710.72, 3168.00), (4.760, -0.050)),
            ("edge_M", (1710.72, 3168.00), (4.380, -0.030)),
            ("edge_L", (1710.72, 3168.00), (4.550, -0.070)),
            ("corner_S", (1710.72, 1267.20), (-18.790, -11.690)),
            ("corner_M", (1710.72, 1267.20), (-18.740, -11.730)),
            ("corner_L", (1710.72, 1267.20), (-18.930, -11.730)),
        ];

        let check = |label: &str, native: &[f32], rows: &[Row], expected_rms: f32| {
            let profile = d2_camera_profile(native);
            let mut sum_sq = 0.0f32;
            let mut worst = 0.0f32;
            for &(cell, (x, y), (mx, my)) in rows {
                let (wx, wy) =
                    lr_mask_warp_norm(x / MASK_WARP_DIMS.0, y / MASK_WARP_DIMS.1, MASK_WARP_DIMS, &profile);
                let (px, py) = (wx * MASK_WARP_DIMS.0 - x, wy * MASK_WARP_DIMS.1 - y);
                let err = (px - mx).hypot(py - my);
                sum_sq += err * err;
                worst = worst.max(err);
                assert!(err <= 1.0, "{label}/{cell}: predicted ({px},{py}), measured ({mx},{my}), error {err}");
            }
            let rms = (sum_sq / rows.len() as f32).sqrt();
            assert!((rms - expected_rms).abs() < 0.02, "{label}: rms {rms}, max {worst}");
        };
        check("wall", &WALL_NATIVE, &WALL, 0.568);
        check("P26", &DSC_NATIVE, &DSC, 0.243);
    }

    #[test]
    fn mask_warp_uses_the_full_raw_centre_in_stored_coordinates() {
        let shifted = crate::recipe::LensProfile {
            mask_warp: vec![1.05; crate::recipe::MASK_WARP_KNOTS],
            mask_warp_src: crate::recipe::MaskWarpSource::CameraMetadata,
            mask_warp_center: Some(crate::recipe::MaskWarpCenter {
                stored_px: [4768.0, 3168.0],
                stored_dims: [9504.0, 6336.0],
            }),
            ..Default::default()
        };
        let fixed = (4768.0 / MASK_WARP_DIMS.0, 3168.0 / MASK_WARP_DIMS.1);
        assert_eq!(lr_mask_warp_norm(fixed.0, fixed.1, MASK_WARP_DIMS, &shifted), fixed);
        let stored_centre = lr_mask_warp_norm(0.5, 0.5, MASK_WARP_DIMS, &shifted);
        assert!(stored_centre.0 < 0.5, "shifted centre was ignored: {stored_centre:?}");

        // The same stored-pixel centre scales with a working-resolution
        // preview instead of remaining thousands of pixels off-frame.
        let preview_dims = (950.4, 633.6);
        let preview_fixed = (476.8 / preview_dims.0, 316.8 / preview_dims.1);
        assert_eq!(
            lr_mask_warp_norm(preview_fixed.0, preview_fixed.1, preview_dims, &shifted),
            preview_fixed
        );

        let legacy = crate::recipe::LensProfile { mask_warp_center: None, ..shifted };
        assert_eq!(lr_mask_warp_norm(0.5, 0.5, MASK_WARP_DIMS, &legacy), (0.5, 0.5));
    }

    #[test]
    fn mask_frame_composes_the_engine_map_with_the_lr_inverse_exactly_once() {
        let profile = crate::recipe::LensProfile {
            distortion: (0..16).map(|i| 1.0 - 0.12 * (i as f32 / 15.0).powi(2)).collect(),
            distortion_on: true,
            mask_warp: (0..crate::recipe::MASK_WARP_KNOTS)
                .map(|i| 0.98 + 0.05 * i as f32 / (crate::recipe::MASK_WARP_KNOTS - 1) as f32)
                .collect(),
            mask_warp_src: crate::recipe::MaskWarpSource::CameraMetadata,
            mask_warp_center: Some(crate::recipe::MaskWarpCenter {
                stored_px: [4768.0, 3168.0],
                stored_dims: [9504.0, 6336.0],
            }),
            ..Default::default()
        };
        let dims = MASK_WARP_DIMS;
        let u = MaskUnwarp::new(&profile, 0.0, dims).expect("both maps are active");
        for (nx, ny) in [(0.15, 0.2), (0.5, 0.5), (0.82, 0.74)] {
            let engine = lens_ungeom_norm(nx, ny, dims, &profile, 0.0);
            let expected = lr_mask_unwarp_norm(engine.0, engine.1, dims, &profile);
            let got = u.at(nx, ny);
            assert!((got.0 - expected.0).abs() < 2e-5 && (got.1 - expected.1).abs() < 2e-5);
        }
    }

    /// ACCEPTANCE ⑤. With no solved warp the map is the IDENTITY — bit-for-bit,
    /// not approximately — and the reason is available by name rather than
    /// inferred from an empty vector.
    #[test]
    fn an_absent_profile_is_an_identity_warp_with_a_named_reason() {
        use crate::recipe::MaskWarpSource as S;
        let none = crate::recipe::LensProfile::default();
        assert_eq!(none.mask_warp_src, S::Absent);
        for i in 0..=7 {
            for j in 0..=7 {
                let (nx, ny) = (i as f32 / 7.0, j as f32 / 7.0);
                assert_eq!(lr_mask_warp_norm(nx, ny, MASK_WARP_DIMS, &none), (nx, ny));
                assert_eq!(lr_mask_unwarp_norm(nx, ny, MASK_WARP_DIMS, &none), (nx, ny));
            }
        }
        // Every "no warp" state names itself, and none of them is silent.
        for s in S::ALL {
            assert!(!s.en().is_empty(), "{s:?} has no prose");
            let mut p = crate::recipe::LensProfile { mask_warp_src: s, ..Default::default() };
            // A tag that is not SOLVED cannot keep knots — `clamp` enforces it,
            // so a hand-edited recipe cannot claim "fisheye refused" and warp.
            p.mask_warp = vec![1.02; 16];
            p.clamp();
            if s.is_solved() {
                assert_eq!(p.mask_warp.len(), 16, "{s:?} is a solved source");
            } else {
                assert!(p.mask_warp.is_empty(), "{s:?} kept knots it has no claim to");
                assert_eq!(lr_mask_warp_norm(0.3, 0.7, MASK_WARP_DIMS, &p), (0.3, 0.7));
            }
        }
        // Empty knots in, empty out: "no data" is not a warp of 1.0.
        assert!(mask_warp_from_camera_knots(&[], MASK_WARP_DIMS, 16).is_empty());
    }

    /// A strong, real-shaped barrel profile for the frame-wiring tests:
    /// falling radius factors, the shape every Sony `0x7037` array has.
    fn barrel_profile() -> crate::recipe::LensProfile {
        crate::recipe::LensProfile {
            distortion: (0..16).map(|i| 1.0 - 0.12 * (i as f32 / 15.0).powi(2)).collect(),
            distortion_on: true,
            ..Default::default()
        }
    }

    /// A hard-edged radial at a KNOWN stored centre, dark enough to find.
    fn probe_radial(cx: f32, cy: f32, r: f32) -> crate::recipe::LocalAdjustment {
        crate::recipe::LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: cy - r,
                left: cx - r,
                bottom: cy + r,
                right: cx + r,
                feather: 0.0,
                roundness: 0.0,
                flipped: false,
                angle: 0.0,
                midpoint: 50.0,
                mask_version: 0,
            },
            exposure_ev: -4.0,
            ..Default::default()
        }
    }

    /// Centroid of the darkened pixels, in output-frame pixels.
    fn effect_centroid(img: &DynamicImage) -> (f64, f64) {
        let g = img.to_rgb8();
        let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for (x, y, p) in g.enumerate_pixels() {
            if p.0[1] < 90 {
                sx += x as f64;
                sy += y as f64;
                n += 1.0;
            }
        }
        assert!(n > 40.0, "the mask must darken a real region, got {n} px");
        (sx / n, sy / n)
    }


    /// ACCEPTANCE ⑥, REWRITTEN by the 2026-08-20 user ruling — the
    /// PARAMETRIC-LANDS-ON-STORED-COORDINATES property.
    ///
    /// # What this replaces, and why
    ///
    /// Its predecessor pinned the opposite: that a radial's rendered weight is
    /// computed at its stored coordinates in the PRE-geometry frame and never
    /// touched. That was the shipped behaviour and it was wrong, because this
    /// engine then resampled the whole frame and carried the mask with it —
    /// while Lightroom does not move a parametric shape at all. The `D`
    /// adjudication measured the gap on the 105 mm pair: the PIXELS move
    /// +87.5 px at r ≈ 3250 (the `.lcp` model at 2.69 px rms, 30 NCC points,
    /// tangential rms 1.22 px) while the radial mask measures a similarity of
    /// 0.99956 — the identity to 0.05 %, and **88.7 px away from the pixel
    /// field** (rms; 89.56 px max). That 88.7 px now supports THIS direction.
    ///
    /// # The property, end to end
    ///
    /// A radial at a known stored centre, developed and then resampled through
    /// an active geometry stage — the real composition, `develop_preview` then
    /// `apply_lens_geometry`, which is the same order `render_to_file` runs —
    /// must put its darkened region back on the STORED centre.
    ///
    /// The control in the same test provenances the tolerance: the identical
    /// chain with [`MaskFrame::AsRendered`] (i.e. the behaviour before this
    /// wiring) displaces the effect by the field, and that displacement is
    /// asserted to be an order larger than the tolerance — so a passing test
    /// cannot be a test of nothing.
    ///
    /// MUTATION THIS KILLS: dropping or retargeting the explicit RADIAL/LINEAR
    /// arms in `mask_weight_in` so RADIAL no longer uses `MaskUnwarp::at` or
    /// LINEAR no longer uses `MaskUnwarp::engine_at`.
    #[test]
    fn a_parametric_mask_lands_on_its_stored_coordinates_under_lens_geometry() {
        let (w, h) = (960u32, 640u32);
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
        // A stronger barrel than `barrel_profile`, so the field is many pixels
        // rather than one: corner factor 0.75, fill scale 0.923.
        let profile = crate::recipe::LensProfile {
            distortion: (0..16).map(|i| 1.0 - 0.25 * (i as f32 / 15.0).powi(2)).collect(),
            distortion_on: true,
            ..Default::default()
        };
        // TWO placements at different radii. The frame centre is a fixed point
        // of every radial map, so one off-centre mask could pass by accident of
        // where it sat; two at different radii cannot.
        //
        // MEASURED on this fixture (the numbers the tolerances below come from,
        // scanned over frame size x barrel strength x placement):
        //
        //   | stored cx | wired error | UNWIRED drift |
        //   |-----------|-------------|---------------|
        //   | 0.10      | 0.30 px     | 23.84 px      |
        //   | 0.32      | 0.09 px     |  8.39 px      |
        //
        // The sub-pixel residue is resampling, not geometry: the effect is a
        // filled disc whose centroid is recovered from 8-bit thresholded
        // pixels, and the bilinear resample softens its rim.
        for (cx, min_drift) in [(0.10f32, 15.0f64), (0.32f32, 5.0f64)] {
            let cy = 0.5f32;
            let recipe = EditRecipe {
                masks: vec![probe_radial(cx, cy, 0.05)],
                lens_profile: profile.clone(),
                ..Default::default()
            };
            let want = (cx as f64 * w as f64, cy as f64 * h as f64);

            // WIRED: `develop_preview` derives `WarpedDownstream` from the
            // recipe, exactly as the GUI and web preview surfaces do before
            // they warp, and the same decision `render_to_file` makes.
            let wired = apply_lens_geometry(&develop_preview(&base, &recipe), &profile, 0.0);
            let got = effect_centroid(&wired);

            // CONTROL: the identical chain with the adaptation switched off —
            // the behaviour before this wiring, and where the tolerance's
            // provenance comes from.
            let unwired = apply_lens_geometry(
                &develop_preview_framed(
                    &base,
                    &recipe,
                    &crate::diag::pixels(),
                    MaskFrame::AsRendered,
                ),
                &profile,
                0.0,
            );
            let drifted = effect_centroid(&unwired);

            let err = ((got.0 - want.0).powi(2) + (got.1 - want.1).powi(2)).sqrt();
            let drift = ((drifted.0 - want.0).powi(2) + (drifted.1 - want.1).powi(2)).sqrt();
            // PREMISE: the field really does move this mask, or the assertion
            // below is satisfied by an identity map and proves nothing.
            assert!(
                drift > min_drift,
                "cx={cx}: the control must displace by the field; it moved \
                 {drift:.2} px (centroid {drifted:?} vs stored {want:?})"
            );
            assert!(
                err < 1.5,
                "cx={cx}: the wired chain must land on the STORED centre: \
                 {err:.2} px off (centroid {got:?} vs stored {want:?}; \
                 control drifts {drift:.2} px)"
            );
            assert!(
                err * 5.0 < drift,
                "cx={cx}: the fix must be an order better: {err:.2} vs {drift:.2}"
            );
        }
    }

    /// D2 continuation of the wired-centroid test above: when the Lightroom
    /// transport is non-identity, the same exact-once engine cancellation must
    /// land on `m_lr(stored)`, not on the stored point and not on a second copy
    /// of either lens field.
    #[test]
    #[allow(clippy::excessive_precision)] // exact decoded 0x7037 fixture decimals
    fn d2_wired_camera_mask_lands_at_the_lr_transported_coordinate() {
        const WALL_NATIVE: [f32; 16] = [
            1.0007934570,
            0.9998779297,
            0.9981079102,
            0.9959716797,
            0.9927368164,
            0.9890136719,
            0.9846191406,
            0.9800415039,
            0.9749145508,
            0.9696044922,
            0.9641113281,
            0.9589843750,
            0.9538574219,
            0.9492187500,
            0.9448242188,
            0.9412231445,
        ];
        let (w, h) = (1920u32, 1280u32);
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
        let mut profile = d2_camera_profile(&WALL_NATIVE);
        // The render-side adjudication retained this established 16-knot
        // calibration; only the mask solve uses the corrected dense spline.
        profile.distortion = WALL_NATIVE.to_vec();
        profile.distortion_on = true;

        for cx in [0.10f32, 0.32f32] {
            let cy = 0.5f32;
            let recipe = EditRecipe {
                masks: vec![probe_radial(cx, cy, 0.025)],
                lens_profile: profile.clone(),
                ..Default::default()
            };
            let target = lr_mask_warp_norm(cx, cy, (w as f32, h as f32), &profile);
            let want = (target.0 as f64 * w as f64, target.1 as f64 * h as f64);
            let got = effect_centroid(&apply_lens_geometry(
                &develop_preview(&base, &recipe),
                &profile,
                0.0,
            ));
            let err = (got.0 - want.0).hypot(got.1 - want.1);
            let transport = (want.0 - cx as f64 * w as f64)
                .hypot(want.1 - cy as f64 * h as f64);
            eprintln!(
                "D2 wired centroid cx={cx:.2}: error={err:.3}px, Lightroom transport={transport:.3}px"
            );
            assert!(transport > 1.0, "premise: D2 transport is only {transport:.3}px");
            assert!(err < 1.0, "cx={cx}: centroid {got:?}, target {want:?}, error {err:.3}px");
        }
    }

    /// The COMPANION pin: with the geometry stage inactive, the mask chain is
    /// unchanged — bit for bit, not approximately.
    ///
    /// This is the other half of the "mask map and pixel warp travel together"
    /// invariant, and the half that protects every photograph that has no lens
    /// profile and no manual distortion (every non-Sony frame, and every Sony
    /// frame whose photographer switched the correction off). The wiring must
    /// cost them nothing at all.
    ///
    /// MUTATION THIS KILLS: making `MaskFrame::downstream` return
    /// `WarpedDownstream` unconditionally, or dropping `MaskUnwarp::new`'s
    /// identity check.
    #[test]
    fn with_the_geometry_stage_inactive_the_mask_chain_is_untouched() {
        let (w, h) = (240u32, 160u32);
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
        let masks = vec![probe_radial(0.24, 0.5, 0.1)];
        // (a) No profile at all, and a profile whose data is present but
        //     TOGGLED OFF — both are inert, and the second is the one a naive
        //     `!profile.distortion.is_empty()` gate would get wrong.
        let none = EditRecipe { masks: masks.clone(), ..Default::default() };
        let toggled_off = EditRecipe {
            lens_profile: crate::recipe::LensProfile {
                distortion_on: false,
                ..barrel_profile()
            },
            ..none.clone()
        };
        let a = develop_preview(&base, &none);
        let b = develop_preview(&base, &toggled_off);
        assert_eq!(a.to_rgb8().as_raw(), b.to_rgb8().as_raw(), "an inert profile moved a mask");
        // …and both equal the explicitly-unadapted chain, which is what
        // "unchanged from before this batch" means.
        let c = develop_preview_framed(&base, &none, &crate::diag::pixels(), MaskFrame::AsRendered);
        assert_eq!(a.to_rgb8().as_raw(), c.to_rgb8().as_raw());

        // (a2) The case the identity short-circuit in `MaskUnwarp::new` exists
        //      for, and the one `downstream` cannot catch: a profile that IS
        //      active by the gate — sixteen knots, toggle on — whose map is
        //      nevertheless the identity, because the lens has no distortion to
        //      correct. `geometry_active()` is true, so the frame says
        //      `WarpedDownstream`; the map must then recognise itself as inert
        //      and return `None` rather than push every mask coordinate through
        //      a float round trip.
        let flat = EditRecipe {
            lens_profile: crate::recipe::LensProfile {
                distortion: vec![1.0; 16],
                distortion_on: true,
                ..Default::default()
            },
            ..none.clone()
        };
        assert!(
            MaskFrame::downstream(&flat.lens_profile, 0.0).warps(),
            "premise: a flat profile is still ACTIVE by the gate"
        );
        assert!(
            MaskUnwarp::new(&flat.lens_profile, 0.0, (w as f32, h as f32)).is_none(),
            "an identity map must short-circuit, not round-trip every coordinate"
        );
        let d = develop_preview(&base, &flat);
        assert_eq!(
            a.to_rgb8().as_raw(),
            d.to_rgb8().as_raw(),
            "a distortion-free lens profile moved a mask"
        );

        // (b) The decision itself, at the source: an inert profile answers
        //     `AsRendered`, so the geometry stage is not run either.
        let inert = crate::recipe::LensProfile::default();
        assert!(!MaskFrame::downstream(&inert, 0.0).warps());
        assert!(MaskFrame::downstream(&inert, 0.0).unwarp((240.0, 160.0)).is_none());
        assert!(!MaskFrame::downstream(&toggled_off.lens_profile, 0.0).warps());
        // …and an ACTIVE one answers the other way, in both of the two ways it
        // can be active (profile knots, and the manual amount alone).
        assert!(MaskFrame::downstream(&barrel_profile(), 0.0).warps());
        assert!(MaskFrame::downstream(&inert, 25.0).warps(), "the manual amount alone warps");
        assert!(
            MaskFrame::downstream(&inert, 25.0).unwarp((240.0, 160.0)).is_some(),
            "the manual lens_distortion must be covered, not half-covered"
        );
    }

    /// The GUI's red coverage wash must advertise exactly what the render
    /// applies — including the frame adaptation.
    ///
    /// The overlay is built by `mask_coverage` and then warped by the caller
    /// (`bin/gui/canvas.rs`) so it follows the rendered pixels. Both halves take
    /// the SAME [`MaskFrame`]; if the coverage build skipped the adaptation
    /// while the render performed it, the wash would sit a whole field away
    /// from the effect it claims to show — up to 186 px at 24 mm.
    ///
    /// Asserted as agreement between the two, not against a hand-computed
    /// expectation: the property is that the overlay and the render say the
    /// same thing, whatever that thing is.
    ///
    /// MUTATION THIS KILLS: dropping the `unwarp` from `mask_coverage`.
    #[test]
    fn the_gui_coverage_overlay_matches_what_the_render_applies() {
        let (w, h) = (480u32, 320u32);
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255])));
        let profile = crate::recipe::LensProfile {
            distortion: (0..16).map(|i| 1.0 - 0.25 * (i as f32 / 15.0).powi(2)).collect(),
            distortion_on: true,
            ..Default::default()
        };
        let adj = probe_radial(0.20, 0.5, 0.08);
        let recipe =
            EditRecipe { masks: vec![adj.clone()], lens_profile: profile.clone(), ..Default::default() };
        let frame = MaskFrame::downstream(&profile, 0.0);
        assert!(frame.warps(), "premise: this fixture's geometry is active");

        // The overlay, exactly as the GUI builds it: coverage then the same warp.
        let cov = DynamicImage::ImageLuma8(mask_coverage(&adj, &base, frame));
        let cov = apply_lens_geometry(&cov, &profile, 0.0).to_luma8();

        // The render, exactly as a preview surface builds it.
        let rendered = apply_lens_geometry(&develop_preview(&base, &recipe), &profile, 0.0).to_rgb8();

        // Where the overlay claims full coverage, the render must have applied
        // the effect (-4 EV on white); where it claims none, it must not have.
        let (mut claimed, mut agreed, mut clear, mut clean) = (0u32, 0u32, 0u32, 0u32);
        for (x, y, p) in cov.enumerate_pixels() {
            let lit = rendered.get_pixel(x, y).0[1];
            if p.0[0] > 200 {
                claimed += 1;
                if lit < 160 {
                    agreed += 1;
                }
            } else if p.0[0] < 20 {
                clear += 1;
                if lit > 200 {
                    clean += 1;
                }
            }
        }
        assert!(claimed > 200 && clear > 200, "premise: {claimed} covered / {clear} clear px");
        assert!(
            agreed * 100 >= claimed * 97,
            "the overlay claims coverage the render does not apply: {agreed}/{claimed}"
        );
        assert!(
            clean * 100 >= clear * 97,
            "the render applies an effect the overlay does not show: {clean}/{clear}"
        );
    }

    /// The per-TYPE split, at the predicate that owns it: only the two shapes
    /// Lightroom stores post-correction are adapted.
    ///
    /// A brush would be moved TWICE if it were included here (Lightroom
    /// rasterises it pre-correction and so does this engine); an engine-authored
    /// raster has no Lightroom rendering to agree with; a range mask selects by
    /// pixel value and is frame-invariant, so it is not a geometry at all.
    #[test]
    fn only_lightrooms_post_correction_shapes_are_frame_adapted() {
        assert!(is_lr_post_correction_geometry(&MaskGeometry::Linear {
            zero_x: 0.1,
            zero_y: 0.2,
            full_x: 0.8,
            full_y: 0.9
        }));
        assert!(is_lr_post_correction_geometry(&probe_radial(0.3, 0.3, 0.1).mask));
        assert!(!is_lr_post_correction_geometry(&MaskGeometry::Brush {
            name: "Brush 1".into(),
            blend_mode: 0,
            value: 1.0,
            inverted: false,
            strokes: Vec::new(),
        }));
        assert!(!is_lr_post_correction_geometry(&MaskGeometry::Bitmap { path: String::new() }));
        assert!(!is_lr_post_correction_geometry(&MaskGeometry::AiMask {
            name: String::new(),
            subtype: 0,
            ref_x: 0.5,
            ref_y: 0.5,
            blend_mode: 0,
            value: 1.0,
            inverted: false,
            mask_version: 0,
            gesture: Vec::new(),
            provenance: Vec::new(),
            raster: None,
        }));
        // And the adapter honours it: with the SAME unwarp in hand, a brush
        // and a bitmap are asked at the un-adapted point.
        let u = MaskUnwarp::new(&barrel_profile(), 0.0, (480.0, 320.0)).expect("active");
        let moved = u.at(0.2, 0.5);
        assert!((moved.0 - 0.2).abs() > 1e-4, "premise: the map really moves this point");
        // A brush that actually PAINTS at the sample point (R29 Batch-6b). With
        // the empty stroke list this test used to carry, both sides of the
        // equality were the inert 0 and it held for the wrong reason. The
        // sample sits on the RIM of a hard dab (h = 1, so the falloff is a
        // near-step) — the one place a fraction of a per-cent of frame width
        // changes the weight by more than the 8-bit raster quantum.
        let brush = probe_brush(&[(1.0, 0.05, 1.0, 1.0, "d 0.2 0.5")]);
        let braster = brush_raster(&brush, 480, 320).expect("one dab");
        let rim = (0.2 + 0.05 * 0.94, 0.5);
        let rim_moved = u.at(rim.0, rim.1);
        assert!(
            mask_weight(&brush, rim.0, rim.1, Some(&braster)) > 0.2,
            "premise: the brush really paints at the rim sample"
        );
        assert_eq!(
            mask_weight_in(&brush, rim.0, rim.1, Some(&braster), Some(&u), (480.0, 320.0)),
            mask_weight(&brush, rim.0, rim.1, Some(&braster)),
            "a brush must not be frame-adapted"
        );
        assert!(
            (mask_weight_in(&brush, rim.0, rim.1, Some(&braster), Some(&u), (480.0, 320.0))
                - mask_weight(&brush, rim_moved.0, rim_moved.1, Some(&braster)))
            .abs()
                > 1e-3,
            "…and the adapted point is a DIFFERENT weight, so the equality is not vacuous"
        );
        let rad = probe_radial(0.24, 0.5, 0.1).mask;
        assert_eq!(
            mask_weight_in(&rad, 0.2, 0.5, None, Some(&u), (480.0, 320.0)),
            mask_weight(&rad, moved.0, moved.1, None),
            "a radial must be asked at the adapted point"
        );
    }

    /// The frame table in the mask-warp block header, asserted rather than
    /// asserted-in-prose: this engine evaluates masks BEFORE the geometry
    /// stage, so a mask it draws is carried by the distortion field exactly as
    /// the pixels are.
    ///
    /// That is what makes an extra warp on the brush arm a DOUBLE application
    /// (Lightroom rasterises brush dabs pre-correction too), and what makes the
    /// radial arm — whose coordinates Lightroom stores POST-correction — a
    /// mismatch this batch exposed rather than created.
    ///
    /// Measured here rather than cited, in the two halves that make an order:
    /// the mask stage's OUTPUT does not depend on the lens profile at all (it
    /// runs first and cannot see it), and running the geometry stage over that
    /// output MOVES the mask's footprint (so it runs second, over pixels the
    /// mask is already baked into). That composition — `apply_develop` then
    /// `apply_lens_geometry` — is exactly the one `render_to_file` performs.
    #[test]
    fn the_engine_evaluates_masks_before_the_geometry_stage() {
        // 480 px wide, not 240: the displacement this measures is a FRACTION of
        // the frame (~0.6 % of the width for this profile), so a small fixture
        // frame puts the whole effect inside a pixel and the test would pass on
        // rounding rather than on the property.
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(480, 320, image::Rgb([128, 128, 128])));
        let mask = crate::recipe::LocalAdjustment {
            mask: MaskGeometry::Radial {
                top: 0.30,
                left: 0.05,
                bottom: 0.70,
                right: 0.28,
                feather: 0.0,
                roundness: 0.0,
                flipped: false,
                angle: 0.0,
                midpoint: 50.0,
                mask_version: 0,
            },
            exposure_ev: -4.0,
            ..Default::default()
        };
        // A strong barrel profile, shaped like a real one (falling factors).
        let profile = crate::recipe::LensProfile {
            distortion: (0..16).map(|i| 1.0 - 0.12 * (i as f32 / 15.0).powi(2)).collect(),
            distortion_on: true,
            ..Default::default()
        };
        let off = EditRecipe { masks: vec![mask.clone()], ..Default::default() };
        let on = EditRecipe { lens_profile: profile.clone(), ..off.clone() };
        let dark_centroid = |img: &DynamicImage| -> (f64, f64) {
            let g = img.to_rgb8();
            let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
            for (x, y, p) in g.enumerate_pixels() {
                if p.0[1] < 90 {
                    sx += x as f64;
                    sy += y as f64;
                    n += 1.0;
                }
            }
            assert!(n > 50.0, "the mask must darken a real region, got {n} px");
            (sx / n, sy / n)
        };
        // HALF ONE: the mask stage runs FIRST — it rasterises into the
        // PRE-geometry buffer and nothing it does moves a pixel. Asked with
        // `AsRendered` (no geometry downstream) it renders the same pixels
        // whether or not a profile is present, because the profile is not a
        // tonal control and the frame adaptation is switched off.
        //
        // Deliberately NOT `develop_preview` here any more: since R29 Batch-3
        // that entry point DOES read the profile — to adapt a parametric mask's
        // frame for the resample it expects to follow (`MaskFrame`). That is
        // the fix, not a counter-example to the ordering, and asking with the
        // frame pinned is how the ordering stays measurable.
        let anon = crate::diag::pixels();
        let masked_off = develop_preview_framed(&base, &off, &anon, MaskFrame::AsRendered);
        let masked_on = develop_preview_framed(&base, &on, &anon, MaskFrame::AsRendered);
        assert_eq!(
            masked_off.to_rgb8().as_raw(),
            masked_on.to_rgb8().as_raw(),
            "the mask stage moved pixels by itself - it is not a pure rasteriser"
        );
        // HALF TWO: the geometry stage runs SECOND, over pixels the mask is
        // already baked into, so it CARRIES the mask exactly as it carries the
        // photograph. If masks were evaluated after geometry this would be
        // zero, and the brush arm would need the warp the block header
        // describes instead of already having it.
        let before = dark_centroid(&masked_off);
        let after = dark_centroid(&apply_lens_geometry(&masked_off, &profile, 0.0));
        assert!(
            (before.0 - after.0).abs() > 1.5,
            "the geometry stage did not carry the mask: centroids {before:?} vs {after:?}"
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

    /// R29-1 acceptance ②: the PURE-PIXEL preview arm carries typed identity —
    /// or, here, typed ABSENCE of it.
    ///
    /// `develop_preview` is handed a buffer, a width and a height. Under R28
    /// Batch-5 5c its mask-raster loader was passed a bare `None` and the
    /// resulting stderr line named nothing, with the reason living only in a
    /// comment; the registration called it "the residue of 5c". The disclosure
    /// now arrives as a `diag::Line` whose subject IS `Subject::PixelOnly` —
    /// a state a sink can match on — and the injected form lets a caller that
    /// DOES know the photograph say so instead.
    #[test]
    fn the_pure_pixel_preview_arm_states_that_it_has_no_photograph() {
        use crate::diag::{Collector, Diag, Subject};
        use crate::recipe::MaskGeometry;
        let base =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([120, 120, 120])));
        // A path nothing else in the suite touches: `load_mask_bitmap` caches
        // its negative result per (path, mtime), so a shared fixture would let
        // a neighbouring test's first hit swallow the line under test.
        let dead = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap {
                    path: "out/_r29_pixel_only_probe_no_such_mask.png".into(),
                },
                exposure_ev: 1.5,
                ..Default::default()
            }],
            ..Default::default()
        };
        let sink = Collector::new();
        let _ = develop_preview_with(&base, &dead, &Diag::pixels_only(&sink));
        let lines = sink.take();
        assert_eq!(lines.len(), 1, "the dead raster must disclose exactly once: {lines:?}");
        assert_eq!(
            lines[0].subject,
            Subject::PixelOnly,
            "the preview arm must state that it has no photograph, not pass a bare None"
        );
        assert!(
            lines[0].text.contains("could not be loaded"),
            "unexpected line: {}",
            lines[0].text
        );
        // …and the shipped rendering of a PixelOnly line carries no stem, which
        // is what this arm has always printed.
        assert!(
            lines[0].shipped().starts_with("⚠ bitmap mask '"),
            "PixelOnly must render without an attribution: {}",
            lines[0].shipped()
        );
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
            let cov = mask_coverage(&adj, &base, MaskFrame::AsRendered);
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
            midpoint: 50.0,
            mask_version: 2,
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

    /// The α(ρ) table published as `b7-analysis-2.md` §3.1 and, for the three
    /// rungs me3 added, `me3-a-report.md`'s dense profiles — asserted against
    /// what `radial_falloff` actually renders. That is the whole point of the
    /// R29 Batch-7-2 landing and of me3's insertion on top of it, and the pin
    /// that makes the LUT a MEASUREMENT rather than 3190 numbers nobody can
    /// trace.
    ///
    /// These rows are the reports' own printed tables (green channel, Δρ = 0.005
    /// bins of ≥400 px, normalised by the f = 0 interior), transcribed here for
    /// all eleven measured feather rungs: eight from `b7-analysis-2.md` §3.1,
    /// and f = 15/35/65 from me3's `a_05_dense` (printed in
    /// `scripts-archive/me3-a/a_all_outputs.log`), which reproduces the eight
    /// B7-2 columns bit for bit where the two overlap — checked here, since
    /// those eight values are the ones this test already asserted. The f = 0
    /// column is deliberately NOT among them: it is analytic here, and
    /// `radial_feather_zero_is_an_exact_hard_edge` owns it.
    ///
    /// TOLERANCE, and where it comes from: 0.0025, the cost of the two
    /// conditionings `radial_falloff` documents. Eleven of these 187 published
    /// values exceed 1.0 (up to 1.0023 — the α calibration's own overshoot) and
    /// are clamped; the rest sit within 0.0013, which is the ρ-monotone
    /// regression flattening the f = 1 column's noisy near-unity plateau. It is
    /// NOT a slack budget: at 0.0025 a one-row or one-column shift of the table
    /// fails by two orders of magnitude.
    ///
    /// MUTATION THIS CATCHES: shift any column by one rung, transpose the two
    /// interpolation axes, drop the `B0` normalisation, or regenerate the table
    /// off a different ρ grid — every one of them moves rows here by ≫ 0.0025.
    #[test]
    fn the_radial_falloff_reproduces_the_measured_alpha_table() {
        // Columns f = 1 / 5 / 10 / 15 / 25 / 35 / 50 / 65 / 75 / 90 / 100.
        #[rustfmt::skip]
        const PUBLISHED: [(f32, [f32; 11]); 17] = [
            (0.10, [1.0022, 1.0022, 1.0022, 1.0022, 1.0022, 1.0023, 1.0015, 0.9816, 0.9685, 0.9491, 0.9361]),
            (0.20, [0.9987, 0.9986, 0.9987, 0.9987, 0.9986, 0.9977, 0.9920, 0.9430, 0.9107, 0.8626, 0.8307]),
            (0.30, [0.9987, 0.9986, 0.9986, 0.9986, 0.9977, 0.9917, 0.9770, 0.9004, 0.8499, 0.7749, 0.7254]),
            (0.40, [0.9992, 0.9991, 0.9989, 0.9985, 0.9941, 0.9750, 0.9477, 0.8481, 0.7825, 0.6855, 0.6214]),
            (0.50, [0.9990, 0.9987, 0.9979, 0.9962, 0.9788, 0.9338, 0.8899, 0.7771, 0.7027, 0.5927, 0.5201]),
            (0.60, [0.9998, 0.9989, 0.9965, 0.9905, 0.9390, 0.8545, 0.7893, 0.6782, 0.6051, 0.4963, 0.4242]),
            (0.70, [1.0003, 0.9984, 0.9931, 0.9714, 0.8540, 0.7279, 0.6419, 0.5490, 0.4874, 0.3958, 0.3357]),
            (0.80, [1.0010, 0.9979, 0.9827, 0.9061, 0.7041, 0.5624, 0.4719, 0.4063, 0.3629, 0.2987, 0.2560]),
            (0.90, [1.0012, 0.9947, 0.8842, 0.6923, 0.4789, 0.3784, 0.3156, 0.2764, 0.2504, 0.2110, 0.1847]),
            (0.95, [1.0008, 0.9429, 0.6680, 0.4816, 0.3443, 0.2882, 0.2494, 0.2204, 0.2010, 0.1719, 0.1523]),
            (1.00, [0.3718, 0.2448, 0.2226, 0.2159, 0.2096, 0.2037, 0.1919, 0.1713, 0.1575, 0.1368, 0.1229]),
            (1.05, [0.0002, 0.0027, 0.0226, 0.0591, 0.1083, 0.1336, 0.1433, 0.1292, 0.1198, 0.1056, 0.0962]),
            (1.10, [0.0001, 0.0007, 0.0048, 0.0173, 0.0535, 0.0838, 0.1040, 0.0946, 0.0884, 0.0790, 0.0728]),
            (1.20, [0.0001, 0.0004, 0.0011, 0.0034, 0.0129, 0.0285, 0.0482, 0.0441, 0.0415, 0.0374, 0.0347]),
            (1.30, [0.0000, 0.0002, 0.0004, 0.0009, 0.0035, 0.0080, 0.0167, 0.0148, 0.0134, 0.0115, 0.0101]),
            (1.40, [0.0000, 0.0000, 0.0001, 0.0001, 0.0003, 0.0007, 0.0016, 0.0012, 0.0009, 0.0005, 0.0002]),
            (1.45, [0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000]),
        ];
        let mut worst = 0.0f32;
        for (rho, row) in PUBLISHED {
            for (col, want) in RADIAL_FALLOFF_F.iter().zip(row) {
                let got = radial_falloff(col / 100.0, rho);
                let dev = (got - want).abs();
                worst = worst.max(dev);
                assert!(
                    dev <= 0.0025,
                    "feather {col} at ρ = {rho}: rendered {got}, §3.1 says {want} \
                     (off by {dev})"
                );
            }
        }
        // A floor as well as a ceiling: if the table ever became EXACT the
        // conditioning would have been silently dropped, and with it the
        // monotonicity the renderer relies on.
        assert!(worst > 0.001, "the documented clamp/regression cost vanished: {worst}");
    }

    /// The constant on disk IS the generated artefact, entry for entry — the
    /// drift gate that came in with me3's f = 15/35/65 columns.
    ///
    /// The measurement test above is a TOLERANCE check (0.0025) sampled at 17 ρ
    /// values: it cannot see one entry drifting by a few thousandths, it never
    /// looks at the 273 rows in between, and it reads the feather ladder rather
    /// than pinning it. This one hashes every `f32` in the table and in the
    /// ladder, so any edit to any of the 3190 entries fails — inserted column
    /// or carried-over one.
    ///
    /// The digests are FNV-1a-64 over each value's little-endian `to_bits()`,
    /// row-major, computed from the generator's own output file
    /// `…/r29-materials/scripts-archive/me3-a/cache-out/RADIAL_FALLOFF_11col.rs.txt`
    /// (27 725 B, sha256 `cd993fc7d73cf6f5302bcfaa8384f0325e51344ac9d0ee6af93b9a5d118ad7bb`,
    /// written by `a_09_table.py`). The eight B7-2 columns inside that file are
    /// the previously shipped ones bit for bit (`a_09`: max |Δ| = 0.000000 over
    /// their 2320 entries), so this pins B7-2's landing as well as me3's.
    ///
    /// MUTATION THIS CATCHES: change one value in an inserted column — which
    /// the tolerance test above cannot — or reorder, shift or mistype the
    /// feather ladder, or regenerate the table from a different analysis run.
    #[test]
    fn the_radial_falloff_table_is_the_generated_artefact() {
        fn fnv1a64(values: impl IntoIterator<Item = f32>) -> u64 {
            let mut h = 0xcbf2_9ce4_8422_2325_u64;
            for v in values {
                for byte in v.to_bits().to_le_bytes() {
                    h = (h ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            h
        }
        assert_eq!(
            fnv1a64(RADIAL_FALLOFF.iter().flatten().copied()),
            0x5b52_0111_2f58_37c5,
            "RADIAL_FALLOFF is no longer the table `a_09_table.py` generated"
        );
        assert_eq!(
            fnv1a64(RADIAL_FALLOFF_F),
            0x00a9_21c9_2f45_e96f,
            "the feather ladder is no longer [1, 5, 10, 15, 25, 35, 50, 65, 75, 90, 100]"
        );
    }

    /// The requirement that decided the shape of this landing: the LUT must be
    /// better than the law it replaces on EVERY feather rung, not just the wide
    /// ones.
    ///
    /// `1 − smoothstep(1 − f, 1 + f/2, d)` was already CORRECT for f ≤ 5 —
    /// rms(α) 0.009–0.010 against the measurement, α ≥ 0.5 area within 0.5 %
    /// (`b7-analysis-2.md` §6) — so a replacement fitted only where the gap was
    /// visible could have shipped a REGRESSION at the narrow end and nobody
    /// would have noticed. Scored on §3.1's own seventeen rows the ratio runs
    /// 31× at f = 1 and 26× at f = 5, up to 5448× at f = 100; the three rungs
    /// me3 inserted score 107× / 187× / 2783× at f = 15/35/65 on the same rows.
    ///
    /// MUTATION THIS CATCHES: putting the ramp back (every rung fails), or
    /// building the LUT from a fit that trades the narrow rungs for the wide
    /// ones — f = 1 and f = 5 fail first, which is exactly the failure this
    /// batch was told to avoid.
    #[test]
    fn the_radial_falloff_beats_the_refuted_ramp_on_every_feather() {
        #[rustfmt::skip]
        const PUBLISHED: [(f32, [f32; 11]); 8] = [
            (0.30, [0.9987, 0.9986, 0.9986, 0.9986, 0.9977, 0.9917, 0.9770, 0.9004, 0.8499, 0.7749, 0.7254]),
            (0.60, [0.9998, 0.9989, 0.9965, 0.9905, 0.9390, 0.8545, 0.7893, 0.6782, 0.6051, 0.4963, 0.4242]),
            (0.80, [1.0010, 0.9979, 0.9827, 0.9061, 0.7041, 0.5624, 0.4719, 0.4063, 0.3629, 0.2987, 0.2560]),
            (0.95, [1.0008, 0.9429, 0.6680, 0.4816, 0.3443, 0.2882, 0.2494, 0.2204, 0.2010, 0.1719, 0.1523]),
            (1.00, [0.3718, 0.2448, 0.2226, 0.2159, 0.2096, 0.2037, 0.1919, 0.1713, 0.1575, 0.1368, 0.1229]),
            (1.05, [0.0002, 0.0027, 0.0226, 0.0591, 0.1083, 0.1336, 0.1433, 0.1292, 0.1198, 0.1056, 0.0962]),
            (1.20, [0.0001, 0.0004, 0.0011, 0.0034, 0.0129, 0.0285, 0.0482, 0.0441, 0.0415, 0.0374, 0.0347]),
            (1.40, [0.0000, 0.0000, 0.0001, 0.0001, 0.0003, 0.0007, 0.0016, 0.0012, 0.0009, 0.0005, 0.0002]),
        ];
        for (j, col) in RADIAL_FALLOFF_F.iter().enumerate() {
            let f = col / 100.0;
            let (mut lut, mut old) = (0.0f32, 0.0f32);
            for (rho, row) in PUBLISHED {
                lut += (radial_falloff(f, rho) - row[j]).powi(2);
                // The refuted law, spelled out here rather than referenced, so
                // deleting it from the renderer cannot silently weaken this.
                old += (1.0 - ramp(1.0 - f, 1.0 + f / 2.0, rho) - row[j]).powi(2);
            }
            let n = PUBLISHED.len() as f32;
            let (lut, old) = ((lut / n).sqrt(), (old / n).sqrt());
            assert!(
                lut * 10.0 < old,
                "feather {col}: the LUT must beat the refuted ramp by an order of \
                 magnitude — rms {lut} against {old}"
            );
        }
        // The two rungs the old law got right, pinned as absolute numbers: a
        // regression here is the one this batch was specifically told to avoid.
        for (col, want) in [(1.0f32, 0.00088f32), (5.0, 0.00055)] {
            let mut acc = 0.0f32;
            for (rho, row) in PUBLISHED {
                acc += (radial_falloff(col / 100.0, rho)
                    - row[RADIAL_FALLOFF_F.iter().position(|c| *c == col).unwrap()])
                .powi(2);
            }
            let rms = (acc / PUBLISHED.len() as f32).sqrt();
            assert!(rms < want * 3.0, "feather {col}: rms {rms} against the landed {want}");
        }
    }

    /// Feather 0 is a HARD EDGE, exactly — the one place this LUT is analytic
    /// rather than measured.
    ///
    /// The measured f = 0 column is 0.0084 wide in ρ, but that width is the
    /// JPEG-plus-capture-sharpening blur floor (8.7 px on the measured frame's
    /// major axis), not Lightroom's edge: at Feather 0 Lightroom draws a step.
    /// Using the measured column would have smeared every hard-edged radial by
    /// ~9 px, so f = 0 takes its own branch and `d == 1.0` counts as OUTSIDE —
    /// byte-identical to the degenerate-`ramp` guard this replaces, which is
    /// what keeps `radial_feather_zero_stays_finite_on_the_boundary` and the
    /// four polarity cells green without re-pinning.
    ///
    /// MUTATION THIS CATCHES: drop the `f <= 0.0` branch and feather 0 reads
    /// the f = 1 column instead — 0.372 on the boundary rather than 0, and a
    /// soft rim on every hard radial.
    #[test]
    fn radial_feather_zero_is_an_exact_hard_edge() {
        for d in [0.0f32, 0.5, 0.9, 0.99, 0.999, 0.9999] {
            assert_eq!(radial_falloff(0.0, d), 1.0, "solid inside the ellipse at d = {d}");
        }
        for d in [1.0f32, 1.0001, 1.01, 1.5, 3.0] {
            assert_eq!(radial_falloff(0.0, d), 0.0, "nothing at or outside d = {d}");
        }
        // …and it is a LIMIT, not a cliff: the family stays continuous in
        // feather across the analytic/measured seam. Lightroom only ever writes
        // whole feather units, but this app's own slider is continuous, so a
        // discontinuity here would be a visible ring nobody asked for.
        for d in [0.97f32, 0.99, 1.0, 1.02, 1.05] {
            let (a, b) = (radial_falloff(0.0001, d), radial_falloff(0.0002, d));
            assert!(
                (a - b).abs() < 0.02,
                "feather 0.01 → 0.02 must not jump at d = {d}: {a} vs {b}"
            );
        }
    }

    /// The four structural properties the measurement establishes independently
    /// of any curve fit, asserted on the shipped table rather than on the
    /// report: α(0) = 1 at every feather, zero past the support, non-increasing
    /// in ρ, and non-increasing in feather INSIDE the ellipse.
    ///
    /// Each one has its own provenance. α(0) = 1 is measured, not normalised —
    /// mask centres are pixel-identical to the feather-0 frame on all eight
    /// rungs (`b7-analysis-2.md` §3.4), which is what refuted the free-endpoint
    /// refit's `d_in = −0.228` at f = 100. The support is baked into the column
    /// tails deliberately, with no `d_out` constant anywhere in this file: B7's
    /// `1.4335 ± 0.002` was measuring JPEG 8×8 block spill (§3.3), me3's four
    /// shape-free instruments put the value at √2 and exclude both 1.43 and
    /// 1.4335 (`me3-a-report.md` §0-Q2), and the tails carry it either way.
    ///
    /// The feather monotonicity is asserted only for ρ < 1, and that scope is
    /// the measurement's: OUTSIDE the ellipse the order genuinely reverses (more
    /// feather reaches further), and the f = 50 tail is measurably FATTER than
    /// f = 75's and f = 100's out there — a real non-monotonicity reproduced
    /// independently in the raw DN profile (B7 §3.1). Asserting it globally
    /// would be asserting a tidiness the data does not have.
    ///
    /// MUTATION THIS CATCHES: drop the pool-adjacent-violators pass and the ρ
    /// sweep fails on the noisy near-unity plateaux; drop the running-minimum
    /// pass and the feather sweep fails; let any column tail short of zero and
    /// the support check fails.
    #[test]
    fn the_radial_falloff_holds_its_structural_invariants() {
        let feathers: Vec<f32> = (0..=200).map(|i| i as f32 / 200.0).collect();
        for &f in &feathers {
            assert_eq!(radial_falloff(f, 0.0), 1.0, "α(0) must be 1 at feather {f}");
            // 1.42 / 1.43 / 1.44 are INSIDE the table — every column has already
            // reached zero by ρ = 1.4175 — so these exercise the tail itself and
            // not the past-the-end early return. Both matter: a tail that never
            // quite reaches zero paints the whole frame at 0.2 %, which is
            // invisible in a preview and wrong in an export.
            for d in [1.42f32, 1.43, 1.44, 1.45, 1.5, 2.0, 10.0] {
                assert_eq!(radial_falloff(f, d), 0.0, "feather {f} must be spent by d = {d}");
            }
            // Non-increasing in ρ, swept finer than the table's own rows so an
            // interpolation bug shows up too.
            let mut prev = f32::INFINITY;
            for i in 0..=1500 {
                let a = radial_falloff(f, i as f32 / 1000.0);
                assert!((0.0..=1.0).contains(&a), "α = {a} out of range at feather {f}");
                assert!(a <= prev + 1e-6, "feather {f} rises at ρ = {}: {prev} → {a}", i as f32 / 1000.0);
                prev = a;
            }
        }
        // Non-increasing in feather, INSIDE the ellipse only (see above).
        for i in 0..100 {
            let rho = i as f32 / 100.0;
            let mut prev = f32::INFINITY;
            for &f in &feathers {
                let a = radial_falloff(f, rho);
                assert!(a <= prev + 1e-6, "ρ = {rho} rises at feather {f}: {prev} → {a}");
                prev = a;
            }
        }
        // The reverse, outside: at ρ = 1.2 more feather really does reach
        // further, which is why the sweep above stops at the ellipse.
        assert!(
            radial_falloff(0.25, 1.2) > radial_falloff(0.10, 1.2) * 5.0,
            "the outer branch must GROW with feather"
        );
        // A non-finite sample point must not become a non-finite WEIGHT: NaN
        // casts to row 0, blends to NaN, survives the `wgt <= 0.001` early-out
        // and lands as a black pixel. The old degenerate-`ramp` guard existed
        // for the 0/0 half of this; the LUT has no division to go degenerate,
        // so the guard moved to the input.
        for f in [0.0f32, 0.005, 0.5, 1.0] {
            for d in [f32::NAN, f32::INFINITY] {
                let w = radial_falloff(f, d);
                assert_eq!(w, 0.0, "feather {f} at d = {d} must be inert, got {w}");
            }
        }
        // The other half, and the one `f32::clamp` gets wrong by propagating:
        // a NaN FEATHER on a perfectly ordinary sample point. It degrades to the
        // hard edge rather than to a NaN — reachable only from a hand-edited
        // `recipe.json`, the same threat model `brush_kernel_exponents` names.
        for d in [0.0f32, 0.5, 0.999, 1.0, 1.2, 2.0] {
            let w = radial_falloff(f32::NAN, d);
            assert!(w.is_finite(), "a NaN feather must not become a NaN weight at d = {d}");
            assert_eq!(w, if d < 1.0 { 1.0 } else { 0.0 }, "…and degrades to the hard edge");
        }
    }

    /// v0.32.0 — the polarity truth table, all four cells, closed on the pixel.
    ///
    /// This engine spells "which side gets the effect" as the XOR of TWO flags
    /// (`Radial::flipped` in `mask_weight`, `LocalAdjustment::inverted` in the
    /// weight loop); Lightroom spells it once. Both of Lightroom's observed
    /// spellings are rendered here as the flags the importer produces for them,
    /// and both were measured on real exports: `crs:Flipped="true"` +
    /// `crs:MaskInverted="false"` darkens the ellipse INTERIOR (8 frames,
    /// `#6` at +4.4 stops inside), `crs:Flipped="false"` +
    /// `crs:MaskInverted="true"` darkens the EXTERIOR (`P23`, +3.40 stops
    /// exterior-minus-interior, with the level sets GROWING as the threshold
    /// rises). `PROBE2-VERDICT.md` §6.
    ///
    /// The other two cells are engine-only — 201/201 real radials are
    /// anti-correlated, so Lightroom writes neither — and they are pinned
    /// because the recipe can hold them and a user's Flip checkbox produces
    /// one of them.
    ///
    /// MUTATION THIS CATCHES: change either XOR arm (drop the `1.0 - wgt` in
    /// `mask_weight`, or the `1.0 - wgt` in the weight loop) and two rows flip.
    #[test]
    fn the_radial_polarity_truth_table_is_closed_on_all_four_cells() {
        use crate::recipe::MaskGeometry;
        // A small centred ellipse, hard-edged so "inside" and "outside" are
        // unambiguous at the two sample points.
        let g = |flipped: bool| MaskGeometry::Radial {
            top: 0.3,
            left: 0.3,
            bottom: 0.7,
            right: 0.7,
            feather: 0.0,
            roundness: 0.0,
            flipped,
            angle: 0.0,
            midpoint: 50.0,
            mask_version: 2,
        };
        // (flipped, inverted) → does the effect land INSIDE?
        for (flipped, inverted, inside) in [
            // Lightroom's `Flipped="true" MaskInverted="false"`, as the
            // importer reads it (the bit comes from MaskInverted alone).
            (false, false, true),
            // Lightroom's `Flipped="false" MaskInverted="true"` — `P23`.
            (false, true, false),
            // Engine-only, from this app's own Flip checkbox.
            (true, false, false),
            (true, true, true),
        ] {
            let m = LocalAdjustment {
                mask: g(flipped),
                inverted,
                exposure_ev: -3.0,
                ..Default::default()
            };
            let base =
                DynamicImage::ImageRgb8(RgbImage::from_pixel(40, 40, image::Rgb([160, 160, 160])));
            let r = EditRecipe { masks: vec![m], ..Default::default() };
            let out = develop_preview(&base, &r).to_rgb8();
            let centre = out.get_pixel(20, 20)[0];
            let corner = out.get_pixel(1, 1)[0];
            assert!(
                if inside { centre < corner } else { corner < centre },
                "flipped={flipped} inverted={inverted}: expected the effect \
                 {} — centre {centre}, corner {corner}",
                if inside { "INSIDE" } else { "OUTSIDE" }
            );
        }
    }

    #[test]
    fn radial_roundness_is_a_documented_no_op() {
        use crate::recipe::MaskGeometry;
        // CONTRACT (see `MaskGeometry::Radial` in recipe.rs): roundness is
        // carried by recipe/XMP/AI schema but NOT rendered. Its DOMAIN is no
        // longer the gap — v0.31.1 measured it as Lightroom's ±100 integer
        // slider (24/24 real radials write a bare signed integer) and both the
        // clamp and the importer's gate moved to that band. R29 B7 then
        // measured what the number DOES at +100 / Feather=0: nothing, to
        // 0.1 px and JPEG noise — so this no-op is Lightroom's measured
        // behaviour there, and carrying the value verbatim stays right.
        // The negatives this loop has always exercised (`-100/-35/-1`) are no
        // longer a guess either: R29 B7-2 measured the Roundness×Feather cross
        // term at +100/Feather=50 and R29 me3-b measured −100 at Feather=0 and
        // the whole-frame Roundness×Feather identity at Feather=50, so the
        // assertions below now pin MEASURED behaviour rather than a documented
        // assumption — the values did not have to change for that. Still open
        // (docs/V2_PLAN.md §7 item 11): a second geometry, the values strictly
        // between the endpoints, and Roundness × Angle≠0. Pinning the no-op so
        // any future falloff-shape implementation lands together with the doc
        // and the XMP round-trip.
        let radial = |roundness: f32| MaskGeometry::Radial {
            top: 0.2,
            left: 0.1,
            bottom: 0.8,
            right: 0.7,
            feather: 0.5,
            roundness,
            flipped: false,
            angle: 0.0,
            midpoint: 50.0,
            mask_version: 2,
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
        // Two orthogonal linear gradients give exact hand-computable weights
        // after the shipped profile: base = Eased(nx) (horizontal ramp),
        // component = Eased(ny) (vertical ramp). The
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
                let (b, c) = (
                    linear_coverage(nx, LinearFalloff::Eased),
                    linear_coverage(ny, LinearFalloff::Eased),
                );
                for (mode, want) in [
                    (MaskCombine::Add, 1.0 - (1.0 - b) * (1.0 - c)),
                    (MaskCombine::Subtract, b * (1.0 - c)),
                    (MaskCombine::Intersect, b * c),
                ] {
                    let got = combined_mask_weight(&with(mode), nx, ny, None, &[None], None, (1.0, 1.0));
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
        assert_eq!(
            combined_mask_weight(&plain, 0.3, 0.9, None, &[], None, (1.0, 1.0)),
            linear_coverage(0.3, LinearFalloff::Eased)
        );
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
            let b = linear_coverage(nx, LinearFalloff::Eased);
            let c = linear_coverage(ny, LinearFalloff::Eased);
            let w = b * (1.0 - c);
            1.0 - (1.0 - w) * (1.0 - c)
        };
        let got = combined_mask_weight(&sub_then_add, nx, ny, None, &[None, None], None, (1.0, 1.0));
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
                    path: "Z:/__autoshade_definitely_missing__/raster.png".into(),
                },
                mode: MaskCombine::Subtract,
            }],
            name: "carved".into(),
            ..Default::default()
        };
        let r = EditRecipe { masks: vec![m], ..Default::default() };
        let err = load_mask_raster_snapshot(&r, &crate::diag::pixels())
            .expect_err("a component raster counts for the deliverable refusal");
        assert!(
            err.to_string().contains("carved"),
            "the refusal names the mask whose edit would be dropped: {err:#}"
        );
        let cov = mask_coverage(&r.masks[0], &DynamicImage::new_rgb8(8, 8), MaskFrame::AsRendered);
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
        let dir = std::env::temp_dir().join(format!("autoshade-dead-raster-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good-raster.png");
        image::GrayImage::from_pixel(4, 4, image::Luma([200])).save(&good).unwrap();
        let missing = "Z:/__autoshade_definitely_missing__/raster.png";
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
        let dir = std::env::temp_dir().join(format!("autoshade-mask-bounded-test-{}", std::process::id()));
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

    /// THE SEAM the whole coordinate migration rests on: `orient_point` must
    /// be the exact coordinate twin of `oriented`'s PIXEL transform, for all
    /// eight states. Derived from the pixel map above (which is itself derived
    /// from the EXIF definitions, never from the image crate), so a future
    /// edit to either function that drifts from the other fails HERE rather
    /// than silently displacing every saved mask.
    #[test]
    fn orient_point_is_the_coordinate_twin_of_the_pixel_transform() {
        const W: u32 = 5;
        const H: u32 = 3;
        let src = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(W, H, |x, y| {
            image::Rgb([x as u8, y as u8, 0])
        }));
        for o in [
            Orientation::Normal,
            Orientation::HorizontalFlip,
            Orientation::Rotate180,
            Orientation::VerticalFlip,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Transverse,
            Orientation::Rotate270,
        ] {
            let dst = oriented(src.clone(), o).to_rgb8();
            let (dw, dh) = (dst.width(), dst.height());
            for y in 0..H {
                for x in 0..W {
                    // Pixel CENTRES: the only points whose normalised image is
                    // unambiguous under a bin edge.
                    let (u, v) =
                        orient_point(o, (x as f32 + 0.5) / W as f32, (y as f32 + 0.5) / H as f32);
                    let (dx, dy) = ((u * dw as f32) as u32, (v * dh as f32) as u32);
                    let px = dst.get_pixel(dx.min(dw - 1), dy.min(dh - 1));
                    assert_eq!(
                        (px[0] as u32, px[1] as u32),
                        (x, y),
                        "{o:?}: source ({x},{y}) should land at ({dx},{dy})"
                    );
                }
            }
        }
    }

    /// The nine `Orientation` values under `to_flips`/`from_flips` and the nine
    /// under [`orient_point`] are the SAME group, so [`compose_orientation`]'s
    /// bit algebra and the geometry it claims to describe cannot drift apart.
    ///
    /// Checked exhaustively: for every (EXIF state, quarter turn) pair and
    /// four probe points, `orient_point(compose(e, k), p)` equals
    /// `orient_point(R90^k, orient_point(e, p))` — composition IS "apply the
    /// EXIF state, then turn the display frame", which is the order the pixels
    /// take. The probe set includes an off-frame point, because mask gradients
    /// legitimately live outside the unit square.
    ///
    /// MUTATION THIS CATCHES: swap the two arms of `compose_two`'s
    /// `if t1 { … } else { … }` (i.e. cross the flips on the wrong side) and
    /// every transposing EXIF state composes to its mirror — the 「竖图横躺」
    /// failure, one composition step upstream of where it used to live.
    #[test]
    fn compose_orientation_is_the_composition_of_the_two_coordinate_maps() {
        const STATES: [Orientation; 9] = [
            Orientation::Normal,
            Orientation::HorizontalFlip,
            Orientation::Rotate180,
            Orientation::VerticalFlip,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Transverse,
            Orientation::Rotate270,
            Orientation::Unknown,
        ];
        for e in STATES {
            for k in 0u8..4 {
                let composed = compose_orientation(e, k);
                // The group is CLOSED: nine values in, never `Unknown` out
                // (it is Normal's twin, and a composition that produced it
                // would make `to_u16` write 0 into a sidecar one day).
                assert_ne!(composed, Orientation::Unknown, "{e:?} + {k} quarter turns");
                for (u, v) in [(0.0f32, 0.0f32), (0.13, 0.87), (1.0, 0.25), (-0.4, 1.6)] {
                    let (a, b) = orient_point(e, u, v);
                    let want = orient_point(quarter_turn_orientation(k), a, b);
                    let got = orient_point(composed, u, v);
                    assert!(
                        (got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6,
                        "{e:?} then {k} quarter turns = {composed:?}: ({u},{v}) → {got:?}, \
                         but applying the two in order gives {want:?}"
                    );
                }
            }
        }
        // …and the identity/period facts the field's 0..3 domain rests on.
        assert_eq!(compose_orientation(Orientation::Rotate90, 0), Orientation::Rotate90);
        assert_eq!(compose_orientation(Orientation::Rotate90, 2), Orientation::Rotate270);
        assert_eq!(compose_orientation(Orientation::Rotate270, 1), Orientation::Normal);
        assert_eq!(compose_orientation(Orientation::Normal, 4), Orientation::Normal);
    }

    /// The 竖图横躺 regression net (R27 A10), stated as the property that
    /// actually matters: the composed orientation TRANSPOSES exactly when the
    /// rendered frame does.
    ///
    /// Pure code — the pixel side is [`oriented`] on a deliberately
    /// non-square frame, which is the same function `orient_f32` round-trips
    /// through, so no RAW is needed to pin the chain. The real-file arm lives
    /// in `decode::portrait_raw_reaches_the_pipeline_as_rotate270` and the RAW
    /// zoo probe.
    ///
    /// MUTATION THIS CATCHES: drop `Rotate270` from `decode`'s
    /// `orientation_transposes` list (or add `Rotate180` to it) and the
    /// declared dims part company with the pixels for half the states — the
    /// exact shape of the v0.30 root fix, now with the user's turn on top.
    #[test]
    fn a_quarter_turn_on_any_exif_state_transposes_the_dims_iff_it_transposes_the_pixels() {
        const STATES: [Orientation; 8] = [
            Orientation::Normal,
            Orientation::HorizontalFlip,
            Orientation::Rotate180,
            Orientation::VerticalFlip,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Transverse,
            Orientation::Rotate270,
        ];
        // 7×5: both dims distinct AND distinct from each other's, so a
        // transpose is visible and a square frame cannot hide a bug.
        let src = DynamicImage::ImageRgb8(RgbImage::new(7, 5));
        for e in STATES {
            for k in 0u8..4 {
                let composed = compose_orientation(e, k);
                let (w, h) = oriented(src.clone(), composed).dimensions();
                let transposed = (w, h) == (5, 7);
                assert!(
                    transposed || (w, h) == (7, 5),
                    "{e:?} + {k}: a rotation/flip produced {w}×{h} from 7×5"
                );
                assert_eq!(
                    transposed,
                    crate::decode::orientation_transposes(composed),
                    "{e:?} + {k} = {composed:?}: pixels {w}×{h} but the dims predicate disagrees"
                );
                // The user's turn applied to the ALREADY-oriented pixels must
                // give the same frame as the composed one — the property that
                // lets `render_to_image_in` keep exactly one orientation stage.
                let two_step = turn_image(oriented(src.clone(), e), k);
                assert_eq!(
                    two_step.dimensions(),
                    (w, h),
                    "{e:?} + {k}: one composed turn and two sequential turns disagree"
                );
            }
        }
    }

    /// Every state is a BIJECTION of the plane, and the migration is therefore
    /// reversible — the property that lets an era-0 recipe be turned exactly
    /// once with no accumulated drift.
    #[test]
    fn orient_point_round_trips_through_its_inverse() {
        // Six of the eight are involutions; the quarter turns are each
        // other's inverse.
        let pairs = [
            (Orientation::Normal, Orientation::Normal),
            (Orientation::Unknown, Orientation::Unknown),
            (Orientation::HorizontalFlip, Orientation::HorizontalFlip),
            (Orientation::VerticalFlip, Orientation::VerticalFlip),
            (Orientation::Rotate180, Orientation::Rotate180),
            (Orientation::Transpose, Orientation::Transpose),
            (Orientation::Transverse, Orientation::Transverse),
            (Orientation::Rotate90, Orientation::Rotate270),
            (Orientation::Rotate270, Orientation::Rotate90),
        ];
        for (o, inv) in pairs {
            // Off-frame points included: mask gradients legitimately live
            // outside [0,1] and must survive the round trip too.
            for (u, v) in [(0.0f32, 0.0f32), (0.13, 0.87), (1.0, 0.0), (-0.4, 1.6)] {
                let (a, b) = orient_point(o, u, v);
                let back = orient_point(inv, a, b);
                assert!(
                    (back.0 - u).abs() < 1e-6 && (back.1 - v).abs() < 1e-6,
                    "{o:?} then {inv:?} moved ({u},{v}) to {back:?}"
                );
            }
        }
    }

    /// The A7R IV's own frame — `DefaultCropSize = (9504, 6336)`, aspect
    /// exactly 1.5 — as `orient_recipe_coords`' third argument.
    ///
    /// A round number on purpose: `1.5` and its reciprocal are exact in binary,
    /// so a radius that survives a four-turn circle in these tests survives it
    /// because the algebra is right, not because the aspect happened to cancel
    /// its own rounding.
    fn probe_frame() -> Option<CoordFrame> {
        CoordFrame::new(9504.0, 6336.0)
    }

    /// The `angle` half of the radial rule, checked against `mask_weight`
    /// itself rather than against the derivation: a turned ELLIPSE must cover
    /// exactly the turned PIXELS. This is what makes "rotate the two corners,
    /// negate the angle only for mirrors" more than an assertion.
    #[test]
    fn rotated_radial_mask_covers_the_rotated_pixels() {
        use crate::recipe::LocalAdjustment;
        let base = MaskGeometry::Radial {
            top: 0.15,
            left: 0.30,
            bottom: 0.55,
            right: 0.90,
            feather: 0.4,
            roundness: 0.0,
            flipped: false,
            angle: 37.0,
            midpoint: 50.0,
            mask_version: 2,
        };
        for o in [
            Orientation::HorizontalFlip,
            Orientation::Rotate180,
            Orientation::VerticalFlip,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Transverse,
            Orientation::Rotate270,
        ] {
            let mut r = EditRecipe {
                masks: vec![LocalAdjustment { mask: base.clone(), ..Default::default() }],
                ..Default::default()
            };
            assert!(orient_recipe_coords(&mut r, o, probe_frame()));
            let turned = &r.masks[0].mask;
            for i in 0..=20 {
                for j in 0..=20 {
                    let (u, v) = (i as f32 / 20.0, j as f32 / 20.0);
                    let (u2, v2) = orient_point(o, u, v);
                    let before = mask_weight(&base, u, v, None);
                    let after = mask_weight(turned, u2, v2, None);
                    assert!(
                        (before - after).abs() < 1e-4,
                        "{o:?}: weight at ({u},{v}) was {before}, at the turned point ({u2},{v2}) it is {after}"
                    );
                }
            }
        }
    }

    /// The recipe-level migration: crop and every parametric geometry move,
    /// the round trip is exact, and a Normal photo is untouched.
    #[test]
    fn orient_recipe_coords_moves_geometry_and_round_trips() {
        use crate::recipe::{LocalAdjustment, MaskComponent};
        let seed = || EditRecipe {
            crop: Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 }),
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Linear {
                    zero_x: 0.5,
                    zero_y: 0.0,
                    full_x: 0.5,
                    full_y: 0.45,
                },
                components: vec![MaskComponent {
                    geometry: MaskGeometry::Radial {
                        top: 0.1,
                        left: 0.2,
                        bottom: 0.6,
                        right: 0.7,
                        feather: 0.5,
                        roundness: 0.0,
                        flipped: false,
                        angle: 0.0,
                        midpoint: 50.0,
                        mask_version: 2,
                    },
                    ..Default::default()
                }],
                range: Some(RangeMask::Color {
                    r: 0.2,
                    g: 0.4,
                    b: 0.9,
                    amount: 0.5,
                    px: 0.25,
                    py: 0.75,
                }),
                ..Default::default()
            }],
            ..Default::default()
        };
        // Normal / Unknown: not a single field moves, and the caller is told
        // nothing happened.
        for o in [Orientation::Normal, Orientation::Unknown] {
            let mut r = seed();
            assert!(!orient_recipe_coords(&mut r, o, probe_frame()), "{o:?} must report no move");
            assert_eq!(r, seed(), "{o:?} must not touch a single coordinate");
        }
        // The portrait ARW case, and back.
        let mut r = seed();
        assert!(orient_recipe_coords(&mut r, Orientation::Rotate270, probe_frame()));
        assert_ne!(r, seed(), "a quarter turn must actually move the geometry");
        // Hand-derived: Rotate270 maps (u,v) -> (v, 1-u), so the crop's
        // left/right come from top/bottom and its top/bottom from 1-right,
        // 1-left.
        let t = r.crop.expect("crop survives");
        for (got, want, what) in [
            (t.left, 0.2, "left"),
            (t.right, 0.9, "right"),
            (t.top, 1.0 - 0.8, "top"),
            (t.bottom, 1.0 - 0.1, "bottom"),
        ] {
            assert!((got - want).abs() < 1e-6, "crop {what}: {got} != {want} ({t:?})");
        }
        assert!(orient_recipe_coords(&mut r, Orientation::Rotate90, probe_frame()));
        let back = seed();
        let (c, c0) = (r.crop.unwrap(), back.crop.unwrap());
        assert!(
            (c.left - c0.left).abs() < 1e-6
                && (c.top - c0.top).abs() < 1e-6
                && (c.right - c0.right).abs() < 1e-6
                && (c.bottom - c0.bottom).abs() < 1e-6,
            "crop round trip: {c:?} vs {c0:?}"
        );
        let (
            MaskGeometry::Linear { zero_x, zero_y, full_x, full_y },
            MaskGeometry::Linear { zero_x: a, zero_y: b, full_x: cx, full_y: cy },
        ) = (&r.masks[0].mask, &back.masks[0].mask)
        else {
            panic!("linear geometry survives")
        };
        assert!(
            (zero_x - a).abs() < 1e-6
                && (zero_y - b).abs() < 1e-6
                && (full_x - cx).abs() < 1e-6
                && (full_y - cy).abs() < 1e-6,
            "linear round trip"
        );
        let Some(RangeMask::Color { px, py, .. }) = r.masks[0].range else {
            panic!("range survives")
        };
        assert!((px - 0.25).abs() < 1e-6 && (py - 0.75).abs() < 1e-6, "range point round trip");
    }

    /// R27 L-16c, half one. The `coord_era` migration's crop arm is exact ONLY
    /// if the frame the crop is normalised against turns with the frame — and
    /// that frame is `inscribed_dims`'s output, not the sensor rectangle,
    /// because `render_pipeline` straightens before it crops.
    ///
    /// Swapping `w` and `h` must swap the two answers and change nothing else.
    /// The general branch shows it by inspection; this pins the branch
    /// boundary too, which is where the `if w >= h` inside the thin case could
    /// have made the claim false.
    ///
    /// MUTATION THIS CATCHES: collapse the thin branch's
    /// `if w >= h { (x/s, x/c) } else { (x/c, x/s) }` to either arm alone and
    /// the sliver rows go red (verified). Note what does NOT catch it, because
    /// it looks like it should: SWAPPING the general branch's two expressions
    /// keeps the property, since exchanging both the inputs and the outputs of
    /// a swap-equivariant pair is still swap-equivariant. This test pins the
    /// symmetry, not the formula — `the_straighten_angle_reverses_only_under_
    /// a_mirror` and the existing crop round trip pin the values.
    #[test]
    fn the_straightened_frame_turns_with_the_photo() {
        // Real ARW dims, their transpose, a square, and a sliver.
        for (w, h) in
            [(9504.0f32, 6336.0f32), (6336.0, 9504.0), (4000.0, 4000.0), (100.0, 3000.0)]
        {
            for deg in [0.5f32, 2.5, -2.5, 30.0, -44.0, 44.0, 45.0, -45.0] {
                let (a, b) = inscribed_dims(w, h, deg);
                let (c, d) = inscribed_dims(h, w, deg);
                let close = |x: f32, y: f32| (x - y).abs() <= 1e-3 * x.abs().max(1.0);
                assert!(
                    close(a, d) && close(b, c),
                    "inscribed_dims({w},{h},{deg}) = ({a},{b}) but ({h},{w}) = ({c},{d}) \
                     — the turned frame must be the turn of the frame"
                );
            }
        }
    }

    /// R27 L-16c, half two. R24 registered 「`straighten≠0` 时 crop 迁移一阶
    /// 近似」 without a code site. The residue is the SIGN: rotations commute
    /// with a quarter turn, so the four pure rotations were already exact, but
    /// `rot(deg) ∘ mirror == mirror ∘ rot(−deg)`, so a mirrored photo was
    /// straightened the wrong way by `2·deg` and every crop coordinate then
    /// indexed content that had moved out from under it.
    ///
    /// Both angles the registration is quoted against: a routine horizon
    /// (2.5°) and the extreme end of the ±45 clamp (−44°).
    ///
    /// MUTATION THIS CATCHES: delete the `if mirrors { r.straighten_deg = … }`
    /// block in `orient_recipe_coords` and the four mirror rows go red; negate
    /// on EVERY orientation instead and the three rotation rows go red.
    #[test]
    fn the_straighten_angle_reverses_only_under_a_mirror() {
        let seed = |deg: f32| EditRecipe {
            straighten_deg: deg,
            crop: Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 }),
            ..Default::default()
        };
        for deg in [2.5f32, -44.0] {
            // A quarter or half turn carries the tilt unchanged: the content
            // and the frame turned together.
            for o in [Orientation::Rotate90, Orientation::Rotate180, Orientation::Rotate270] {
                let mut r = seed(deg);
                assert!(orient_recipe_coords(&mut r, o, probe_frame()));
                assert_eq!(r.straighten_deg, deg, "{o:?} must not touch the tilt");
            }
            // A reflection reverses it — and these four are involutions, so
            // applying the same one twice is the identity (the round trip the
            // migration's bijectivity claim rests on).
            for o in [
                Orientation::HorizontalFlip,
                Orientation::VerticalFlip,
                Orientation::Transpose,
                Orientation::Transverse,
            ] {
                let mut r = seed(deg);
                assert!(orient_recipe_coords(&mut r, o, probe_frame()));
                assert_eq!(r.straighten_deg, -deg, "{o:?} must reverse the tilt");
                assert!(orient_recipe_coords(&mut r, o, probe_frame()));
                assert_eq!(r.straighten_deg, deg, "{o:?} twice is the identity");
                // …and the crop came home with it (float tolerance, not `==`:
                // `1 − (1 − 0.1)` is 0.10000002 in f32).
                let (c, c0) = (r.crop.unwrap(), seed(deg).crop.unwrap());
                assert!(
                    (c.left - c0.left).abs() < 1e-6
                        && (c.top - c0.top).abs() < 1e-6
                        && (c.right - c0.right).abs() < 1e-6
                        && (c.bottom - c0.bottom).abs() < 1e-6,
                    "{o:?}: crop round trip {c:?} vs {c0:?}"
                );
            }
        }
        // And a photo with no tilt is untouched whatever the state.
        for o in [Orientation::Transverse, Orientation::Rotate90] {
            let mut r = seed(0.0);
            assert!(orient_recipe_coords(&mut r, o, probe_frame()));
            assert_eq!(r.straighten_deg, 0.0);
        }
    }

    /// A raster mask is an image FILE: the migration must leave its path alone
    /// and REPORT it, never quietly claim to have turned it.
    #[test]
    fn raster_masks_are_reported_not_turned() {
        use crate::recipe::LocalAdjustment;
        let mut r = EditRecipe {
            masks: vec![LocalAdjustment {
                mask: MaskGeometry::Bitmap { path: "sky.png".into() },
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(recipe_has_raster_masks(&r));
        assert!(!recipe_has_frame_coords(&r), "a raster-only recipe has no turnable coordinate");
        let before = r.clone();
        orient_recipe_coords(&mut r, Orientation::Rotate270, probe_frame());
        assert_eq!(r, before, "the raster path must survive byte-for-byte");
    }

    /// One brush group with `n` strokes built from `(value, radius, flow,
    /// hardness, dabs)` — the fixture every brush test below stands on.
    fn probe_brush(strokes: &[(f32, f32, f32, f32, &str)]) -> MaskGeometry {
        MaskGeometry::Brush {
            name: "Brush 1".into(),
            blend_mode: 0,
            // The AGGREGATE's MaskValue: the subtract pair's other half, never
            // a strength — `mask_weight`'s `Brush` arm spells out why.
            value: 1.0,
            inverted: false,
            strokes: strokes
                .iter()
                .map(|&(value, radius, flow, center_weight, dabs)| crate::recipe::BrushStroke {
                    value,
                    radius,
                    flow,
                    center_weight,
                    sync_id: "FA7459A9F5626F4881D7B730C3093F95".into(),
                    dabs: dabs.into(),
                })
                .collect(),
        }
    }

    /// R27 Batch-4 (L-08) → **R29 Batch-6b: a carried brush group draws its
    /// DABS.** Rewritten, not deleted: this test was mutation-lined against
    /// exactly this change, and what it pins now is the other side of it.
    ///
    /// Until R29 Batch-6b `mask_weight`'s `Brush` arm was the literal `=> 0.0`
    /// and this test asserted that zero at six points. The zero was honest
    /// while the alpha kernel was unmeasured; R29 Batch-6 measured it (29
    /// controlled Lightroom exports), so the zero became the invention it had
    /// been guarding against.
    ///
    /// MUTATION-LINED, in both directions:
    ///  * restoring `=> 0.0` fails the「paints」asserts;
    ///  * answering `1.0` (the "just treat it as fully painted" shortcut) fails
    ///    the far-corner asserts;
    ///  * dropping `brush_raster` from `apply_masks`/`mask_coverage` leaves the
    ///    `bmp` slot `None`, which fails the same「paints」asserts.
    #[test]
    fn a_carried_brush_group_draws_its_dabs() {
        // `P12` Mask 7 -> Brush 1, stroke 1: two dabs near the bottom-left
        // corner, radius 0.5818 in WIDTH units, density 0.4398.
        let g = probe_brush(&[(
            0.439815,
            0.582157,
            1.0,
            0.0,
            "r 0.581835\nd 0.100684 0.840004\nr 0.581172\nd 0.213862 0.887261",
        )]);
        let (fw, fh) = (480u32, 320u32);
        let raster = brush_raster(&g, fw, fh).expect("a group with two dabs rasterises");
        // ON the dabs it paints; three frame-widths away it does not. Both
        // halves matter: the first was `0.0` before this batch, and the second
        // is what a blanket `1.0` would break.
        for (nx, ny) in [(0.100684f32, 0.840004f32), (0.213862, 0.887261)] {
            let w = mask_weight(&g, nx, ny, Some(&raster));
            assert!(w > 0.3, "a brush must paint on its own dab at ({nx}, {ny}): {w}");
        }
        for (nx, ny) in [(0.0f32, 0.0f32), (0.99, 0.02)] {
            let w = mask_weight(&g, nx, ny, Some(&raster));
            assert!(w < 0.02, "and nothing at ({nx}, {ny}), ρ > 1 from every dab: {w}");
        }
        // The raster is a RENDER-time artefact, so `mask_weight` still means
        // "the weight of this geometry at this STORED point" and answers the
        // inert 0 when no alpha was built — which is also the contract every
        // other test in this file asserts `mask_weight` against.
        assert_eq!(
            mask_weight(&g, 0.100684, 0.840004, None),
            0.0,
            "no alpha in hand = inert, never a guess"
        );
        // And since R29 C1 the migration treats it like every other geometry:
        // a brush group is a TURNABLE coordinate, not a raster the disclosure
        // has to apologise for. (The algebra itself is
        // `a_quarter_turn_rewrites_the_dab_stream_and_rescales_its_radii`.)
        use crate::recipe::LocalAdjustment;
        let mut r = EditRecipe {
            masks: vec![LocalAdjustment { mask: g, ..Default::default() }],
            ..Default::default()
        };
        assert!(!recipe_has_raster_masks(&r), "a brush group is not a raster mask FILE");
        assert!(recipe_has_frame_coords(&r), "and its dabs ARE frame coordinates");
        let before = r.clone();
        orient_recipe_coords(&mut r, Orientation::Rotate270, probe_frame());
        assert_ne!(r, before, "a quarter turn must move the dab stream");
    }

    /// The dab stream of the first stroke of the first mask — the thing every
    /// R29 C1 assertion below is about.
    fn only_stream(r: &EditRecipe) -> &str {
        let MaskGeometry::Brush { strokes, .. } = &r.masks[0].mask else { panic!("a brush") };
        &strokes[0].dabs
    }

    /// …and its `crs:Radius`, the stream's initial state.
    fn only_radius(r: &EditRecipe) -> f32 {
        let MaskGeometry::Brush { strokes, .. } = &r.masks[0].mask else { panic!("a brush") };
        strokes[0].radius
    }

    /// A one-mask recipe around [`probe_brush`], with `radius` on the stroke.
    fn brushed_recipe(radius: f32, dabs: &str) -> EditRecipe {
        use crate::recipe::LocalAdjustment;
        EditRecipe {
            masks: vec![LocalAdjustment {
                mask: probe_brush(&[(1.0, radius, 1.0, 0.0, dabs)]),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// R29 C1 (the 2026-08-21 ruling) — a turn REWRITES the dab stream instead
    /// of carrying it. Three independent claims, each a different way to get it
    /// wrong:
    ///
    ///  * a `d` token moves through [`orient_point`], the same function the
    ///    crop corners and the radial box beside it use;
    ///  * an `r` token AND `BrushStroke::radius` are rescaled by the frame
    ///    aspect, but ONLY for the four states that exchange the axes —
    ///    `crs:Radius` is in width units while a dab is a circle in pixels, so
    ///    a quarter turn has to exchange the unit too, and a half turn must
    ///    not;
    ///  * `f` (flow) and `h` (hardness) are deposit laws, not positions, and
    ///    are never touched by anything.
    ///
    /// The expected TEXT is hand-derived from `orient_point`'s own table
    /// against the A7R IV's 3:2 frame, so this asserts the output rather than a
    /// re-derivation of it. The six-decimal form is Lightroom's own
    /// ([`LR_DAB_DECIMALS`]).
    ///
    /// MUTATIONS THIS CATCHES:
    ///  * turning `(x, y)` by anything but `orient_point(o, …)` (e.g. the
    ///    inverse state, or `(y, x)`) — the `Rotate90`/`Rotate270` rows;
    ///  * dropping the radius rescale (`radius_scale` pinned at 1.0) — the
    ///    `r 0.300000` and `0.375` asserts;
    ///  * applying it on every state instead of the transposing four — the
    ///    `Rotate180` row, which must keep `r 0.200000` and `0.25`;
    ///  * rescaling `f`/`h` along with `r` — the `f 0.500000` / `h 1.000000`
    ///    asserts.
    #[test]
    fn a_quarter_turn_rewrites_the_dab_stream_and_rescales_its_radii() {
        const SEED: &str =
            "r 0.200000\nf 0.500000\nh 1.000000\nd 0.100000 0.800000\nd 0.300000 0.400000";
        for (o, want_stream, want_radius) in [
            // (u,v) -> (1-v, u); the axes swap, so every radius takes W/H = 1.5.
            (
                Orientation::Rotate90,
                "r 0.300000\nf 0.500000\nh 1.000000\nd 0.200000 0.100000\nd 0.600000 0.300000",
                0.375f32,
            ),
            // (u,v) -> (1-u, 1-v); the frame keeps its shape, so no radius moves.
            (
                Orientation::Rotate180,
                "r 0.200000\nf 0.500000\nh 1.000000\nd 0.900000 0.200000\nd 0.700000 0.600000",
                0.25,
            ),
            // (u,v) -> (v, 1-u); axes swap again.
            (
                Orientation::Rotate270,
                "r 0.300000\nf 0.500000\nh 1.000000\nd 0.800000 0.900000\nd 0.400000 0.700000",
                0.375,
            ),
        ] {
            let mut r = brushed_recipe(0.25, SEED);
            assert!(orient_recipe_coords(&mut r, o, probe_frame()));
            assert_eq!(only_stream(&r), want_stream, "{o:?}: the rewritten stream");
            assert!(
                (only_radius(&r) - want_radius).abs() < 1e-6,
                "{o:?}: crs:Radius is {} , not {want_radius}",
                only_radius(&r)
            );
        }

        // FOUR quarter turns are the identity — and the frame handed in has to
        // turn with them, because after the first one the photo is 6336 × 9504.
        // The radius alternates 0.25 / 0.375 and comes home.
        let mut r = brushed_recipe(0.25, SEED);
        let portrait = CoordFrame::new(6336.0, 9504.0);
        for k in 0..4 {
            let frame = if k % 2 == 0 { probe_frame() } else { portrait };
            assert!(orient_recipe_coords(&mut r, Orientation::Rotate90, frame));
        }
        assert!(
            (only_radius(&r) - 0.25).abs() < 1e-6,
            "a full circle moved the radius: {}",
            only_radius(&r)
        );
        // Exact TEXT equality is not claimed — six decimals is a grid, and four
        // trips over it are four roundings — so the tokens are compared as
        // numbers. On this frame they do in fact come back byte-identical; the
        // tolerance is what the claim is worth, not what today's build does.
        for (got, want) in only_stream(&r).split('\n').zip(SEED.split('\n')) {
            let nums = |t: &str| {
                t.split_whitespace().skip(1).map(|v| v.parse::<f32>().unwrap()).collect::<Vec<_>>()
            };
            assert_eq!(got.split_whitespace().next(), want.split_whitespace().next());
            for (a, b) in nums(got).into_iter().zip(nums(want)) {
                assert!((a - b).abs() < 1e-5, "a full circle moved a token: {got} vs {want}");
            }
        }
    }

    /// The other side of the same ruling: a photo that is NOT turned keeps its
    /// dab stream byte for byte — the identity orientations return before the
    /// rewrite can reach a formatter.
    ///
    /// The fixture stream is deliberately NOT in Lightroom's six-decimal form
    /// (`d 0 0`, `r .5`): if the identity arm ever went through
    /// `turn_brush_strokes`, those would come back as `d 0.000000 0.000000` and
    /// `r 0.500000` — legal, identical in value, and a silent rewrite of a
    /// file the photographer never rotated.
    ///
    /// The second half pins the honest `None` arm: a caller that cannot supply
    /// the frame leaves the stream alone rather than guessing an aspect, and
    /// everything else in the recipe still moves.
    ///
    /// MUTATIONS THIS CATCHES: drop the `Normal | Unknown` early return;
    /// rewrite the stream unconditionally in the `Brush` arm instead of under
    /// `if let Some(f) = frame`.
    #[test]
    fn an_unturned_photo_keeps_every_dab_byte() {
        const RAW_FORM: &str = "r .5\nd 0 0\nd 1 1";
        for o in [Orientation::Normal, Orientation::Unknown] {
            let mut r = brushed_recipe(0.25, RAW_FORM);
            let before = r.clone();
            assert!(!orient_recipe_coords(&mut r, o, probe_frame()), "{o:?} must report no move");
            assert_eq!(r, before, "{o:?} must not touch a single dab byte");
        }
        // No frame in hand: the dabs stay put, and the migration says so by
        // leaving them rather than by inventing an aspect.
        let mut r = brushed_recipe(0.25, RAW_FORM);
        r.crop = Some(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 });
        assert!(orient_recipe_coords(&mut r, Orientation::Rotate90, None));
        assert_eq!(only_stream(&r), RAW_FORM, "no frame, no rewrite");
        assert!((only_radius(&r) - 0.25).abs() < 1e-9, "and no rescale either");
        assert!(r.crop.unwrap().left != 0.1, "while the aspect-free geometry still turned");
    }

    /// SAME FRAME, checked against a parametric shape rather than against the
    /// derivation: a brush dab and a radial centred on the same point must
    /// still be centred on the same point after the turn.
    ///
    /// This is what「渲染永远正确」means operationally — the dab stream is not
    /// merely moved, it is moved by the ONE map every other geometry in the
    /// recipe uses, so a photographer's brush stroke and the gradient they
    /// aligned it with do not drift apart on a rotate.
    ///
    /// MUTATION THIS CATCHES: turn the dabs with the INVERSE orientation (a
    /// plausible sign slip, since `in_source_frame` really does hand this
    /// function an inverse) — the radial still lands correctly and the dab does
    /// not, which no test that only looks at the brush could see.
    #[test]
    fn a_turned_dab_lands_where_the_turned_radial_beside_it_does() {
        use crate::recipe::LocalAdjustment;
        let (px, py) = (0.30f32, 0.65f32);
        // A radial whose box is centred on the dab. Half-extents differ so the
        // centre is not recoverable by accident from a symmetric box.
        let radial = MaskGeometry::Radial {
            top: py - 0.05,
            left: px - 0.12,
            bottom: py + 0.05,
            right: px + 0.12,
            feather: 0.5,
            roundness: 0.0,
            flipped: false,
            angle: 0.0,
            midpoint: 50.0,
            mask_version: 2,
        };
        for o in [
            Orientation::Rotate90,
            Orientation::Rotate180,
            Orientation::Rotate270,
            Orientation::Transpose,
            Orientation::HorizontalFlip,
        ] {
            let mut r = EditRecipe {
                masks: vec![
                    LocalAdjustment {
                        mask: probe_brush(&[(1.0, 0.1, 1.0, 0.0, &format!("d {px} {py}"))]),
                        ..Default::default()
                    },
                    LocalAdjustment { mask: radial.clone(), ..Default::default() },
                ],
                ..Default::default()
            };
            assert!(orient_recipe_coords(&mut r, o, probe_frame()));
            let MaskGeometry::Brush { strokes, .. } = &r.masks[0].mask else { panic!("a brush") };
            let dab: Vec<f32> = strokes[0]
                .dabs
                .split_whitespace()
                .skip(1)
                .map(|v| v.parse::<f32>().unwrap())
                .collect();
            let MaskGeometry::Radial { top, left, bottom, right, .. } = r.masks[1].mask else {
                panic!("a radial")
            };
            let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
            assert!(
                (dab[0] - cx).abs() < 1e-6 && (dab[1] - cy).abs() < 1e-6,
                "{o:?}: the dab landed at ({}, {}) and the radial at ({cx}, {cy})",
                dab[0],
                dab[1]
            );
        }
    }

    /// The RENDER, which is the claim the ruling actually bought: after a
    /// quarter turn a dab is still a CIRCLE in pixels, not an ellipse.
    ///
    /// Radius is the whole reason this batch needed a new input. A dab of
    /// `r = 0.1` on a 480 × 320 frame is 48 px across both axes; turn the photo
    /// and the frame is 320 × 480, so the SAME 48 px is `r = 0.15` in width
    /// units. Sampling the alpha 40 px from the centre along each axis is what
    /// separates the two readings: with the rescale both samples sit inside the
    /// disc and agree, without it the x-extent has shrunk to 32 px and the
    /// horizontal sample falls outside the dab entirely.
    ///
    /// MUTATION THIS CATCHES: pin `CoordFrame::brush_radius_scale` at 1.0. The
    /// coordinate half of the migration stays perfect and the mask still draws
    /// — in the wrong shape, which is exactly the failure a coordinates-only
    /// fix would have shipped.
    #[test]
    fn a_turned_dab_is_still_a_circle_in_pixels() {
        let (fw, fh) = (480u32, 320u32);
        let mut r = brushed_recipe(0.1, "h 1.000000\nd 0.500000 0.500000");
        let before = brush_raster(&r.masks[0].mask, fw, fh).expect("one dab");
        let flat = |g: &MaskGeometry, ras: &image::GrayImage, w: u32, h: u32| {
            // 40 px from the centre along each axis, in that frame's own
            // normalised coordinates.
            let x = mask_weight(g, 0.5 + 40.0 / w as f32, 0.5, Some(ras));
            let y = mask_weight(g, 0.5, 0.5 + 40.0 / h as f32, Some(ras));
            (x, y)
        };
        let (bx, by) = flat(&r.masks[0].mask, &before, fw, fh);
        assert!(bx > 0.5 && (bx - by).abs() < 0.05, "premise: the dab starts round ({bx}, {by})");

        assert!(orient_recipe_coords(&mut r, Orientation::Rotate90, CoordFrame::new(480.0, 320.0)));
        let after = brush_raster(&r.masks[0].mask, fh, fw).expect("still one dab");
        let (ax, ay) = flat(&r.masks[0].mask, &after, fh, fw);
        assert!(
            ax > 0.5 && (ax - ay).abs() < 0.05,
            "the turned dab must still be round ({ax}, {ay})"
        );
        assert!(
            (ax - bx).abs() < 0.05 && (ay - by).abs() < 0.05,
            "and the same size: was ({bx}, {by}), now ({ax}, {ay})"
        );
    }

    /// The WIRING, end to end: `apply_develop` really hands the brush arm a
    /// raster, and `mask_coverage` really advertises the same one.
    ///
    /// Every other brush test in this file builds the alpha itself and passes
    /// it to `mask_weight`, which pins the MODEL and would stay green if the
    /// two production call sites forgot to ask for it. This is the test that
    /// fails when they do — and「forgot to ask」is exactly the shape the arm's
    /// old `=> 0.0` had.
    ///
    /// MUTATION-LINED: dropping either `brush_raster` call — the one in
    /// `apply_masks` or the one in `mask_coverage` — turns one of the two
    /// halves below into the frame it started from.
    #[test]
    fn the_develop_hands_the_brush_arm_its_raster() {
        use crate::recipe::LocalAdjustment;
        let (w, h) = (240usize, 160usize);
        // Hard dab (h = 1) at the frame centre, radius 0.2 of the width, at
        // −3 EV: the centre must go dark and the corner must not move at all.
        let mask = LocalAdjustment {
            mask: probe_brush(&[(1.0, 0.2, 1.0, 1.0, "d 0.5 0.5")]),
            exposure_ev: -3.0,
            ..Default::default()
        };
        let r = EditRecipe { masks: vec![mask.clone()], ..Default::default() };
        let mut data = vec![[0.5f32; 3]; w * h];
        apply_develop_anon(&mut data, w, h, &r);
        let at = |x: usize, y: usize| data[y * w + x][1];
        assert!(at(w / 2, h / 2) < 0.2, "the dab centre must darken: {}", at(w / 2, h / 2));
        assert!(
            (at(2, 2) - 0.5).abs() < 1e-6,
            "and the far corner must not move: {}",
            at(2, 2)
        );
        // The GUI's red wash reads the SAME alpha through a different call
        // site, so it gets its own half of the assertion (the overlay agreeing
        // with the render is `the_gui_coverage_overlay_matches_what_the_render_applies`).
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(
            w as u32,
            h as u32,
            image::Rgb([128, 128, 128]),
        ));
        let cov = mask_coverage(&mask, &base, MaskFrame::AsRendered);
        assert!(
            cov.get_pixel(w as u32 / 2, h as u32 / 2)[0] > 200,
            "the overlay must show the coverage the render applied"
        );
        assert_eq!(cov.get_pixel(2, 2)[0], 0, "and none where the render applied none");
    }

    /// The kernel closed form against the MEASURED nine-rung table.
    ///
    /// Provenance for every number below: R29 Batch-6 §4.1 (the table, nine
    /// hardness rungs × thirteen ρ rows, each rung normalised by its own
    /// ρ < 0.05 core, zero point +0.00199) and §4.4 (the two cubics and the
    /// `(m, n)` they reproduce) —
    /// `~/.claude/plans/r29-materials/b6-analysis.md`.
    ///
    /// **The tolerances are the report's own numbers, not a bar tuned to pass.**
    /// The deg-3 law scores pooled rms 0.0102 against this table (B6 §4.4's
    /// "pooled 0.01020"), and its single largest cell deviation over the 99
    /// cells with ρ < 1 is 0.0297 — at (ρ = 0.792, h = 0.125), which is the
    /// worst rung in the report's own per-rung residual list. So the pooled bar
    /// is 0.012 and the per-cell bar 0.035; anything that moves either
    /// coefficient moves the pooled figure well past its bar.
    ///
    /// MUTATION-LINED: flipping the sign of any cubic coefficient, or swapping
    /// `m` and `n`, blows the pooled rms by more than an order of magnitude.
    #[test]
    fn brush_kernel_reproduces_the_measured_nine_rungs() {
        const HS: [f32; 9] = [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0];
        // B6 §4.1, verbatim. The last two rows of the printed table (ρ = 1.002
        // and 1.042) are the ZERO check and are asserted separately below.
        const TABLE: [(f32, [f32; 9]); 11] = [
            (0.092, [0.9350, 0.9643, 0.9814, 0.9908, 0.9959, 0.9975, 0.9972, 0.9977, 0.9973]),
            (0.193, [0.7326, 0.8494, 0.9235, 0.9657, 0.9869, 0.9948, 0.9975, 0.9981, 0.9979]),
            (0.292, [0.4790, 0.6831, 0.8321, 0.9232, 0.9710, 0.9904, 0.9980, 0.9991, 0.9989]),
            (0.393, [0.2646, 0.5000, 0.7138, 0.8630, 0.9467, 0.9840, 0.9964, 0.9992, 0.9995]),
            (0.492, [0.1302, 0.3359, 0.5775, 0.7794, 0.9073, 0.9697, 0.9935, 0.9989, 0.9999]),
            (0.593, [0.0645, 0.2084, 0.4298, 0.6609, 0.8353, 0.9360, 0.9806, 0.9948, 0.9975]),
            (0.693, [0.0339, 0.1244, 0.2875, 0.5030, 0.7102, 0.8603, 0.9461, 0.9842, 0.9966]),
            (0.792, [0.0151, 0.0685, 0.1671, 0.3153, 0.4978, 0.6772, 0.8207, 0.9159, 0.9679]),
            (0.893, [0.0023, 0.0232, 0.0672, 0.1366, 0.2316, 0.3494, 0.4832, 0.6214, 0.7505]),
            (0.943, [0.0002, 0.0063, 0.0230, 0.0522, 0.0956, 0.1530, 0.2249, 0.3109, 0.4094]),
            (0.972, [-0.0007, 0.0007, 0.0046, 0.0126, 0.0246, 0.0424, 0.0656, 0.0954, 0.1318]),
        ];
        let (mut sq, mut cells, mut worst) = (0.0f64, 0usize, 0.0f32);
        for (rho, row) in TABLE {
            for (h, want) in HS.into_iter().zip(row) {
                let got = brush_kernel(rho, h);
                let d = got - want;
                sq += f64::from(d) * f64::from(d);
                cells += 1;
                worst = worst.max(d.abs());
                assert!(
                    d.abs() <= 0.035,
                    "k({rho}, {h}) = {got}, measured {want} (B6 §4.1); the law's own worst \
                     cell on this table is 0.0297"
                );
            }
        }
        let rms = (sq / cells as f64).sqrt();
        assert_eq!(cells, 99, "the whole ρ < 1 table, not a corner of it");
        assert!(rms <= 0.012, "pooled rms {rms} against B6 §4.4's own 0.01020");
        assert!(worst <= 0.035, "worst cell {worst}");

        // The two structural facts, which are measurements and not conventions:
        // the core is exactly 1 and the support ends exactly at ρ = 1 (B6 §4.1
        // reads |α| ≤ 5e-4 at every ρ ≥ 1.002 on every rung).
        for h in HS {
            assert_eq!(brush_kernel(0.0, h), 1.0, "k(0) = 1 at h = {h}");
            for rho in [1.0f32, 1.002, 1.042, 4.0] {
                assert_eq!(brush_kernel(rho, h), 0.0, "k({rho}) = 0 at h = {h}");
            }
        }
        // Monotone in h at EVERY ρ: B6 counted 0 inversions against batch-10's
        // 3 failures of 80, and it is the property that makes「harder = more
        // covered」true rather than approximately true.
        for (rho, _) in TABLE {
            let ks: Vec<f32> = HS.into_iter().map(|h| brush_kernel(rho, h)).collect();
            for w in ks.windows(2) {
                assert!(w[1] >= w[0] - 1e-6, "k is not monotone in h at ρ = {rho}: {ks:?}");
            }
        }
        // And the exponents themselves, against B6 §4.4's "Reproduced values"
        // (3 dp, so the bar is one unit in the last place plus rounding).
        const MN: [(f32, f32); 9] = [
            (1.832, 6.431),
            (1.664, 2.864),
            (1.907, 1.778),
            (2.593, 1.434),
            (3.932, 1.402),
            (6.249, 1.549),
            (9.790, 1.803),
            (14.210, 2.061),
            (17.964, 2.157),
        ];
        for (h, (m_w, n_w)) in HS.into_iter().zip(MN) {
            let (m, n) = brush_kernel_exponents(h);
            assert!((m - m_w).abs() <= 0.001, "m({h}) = {m}, B6 §4.4 prints {m_w}");
            assert!((n - n_w).abs() <= 0.001, "n({h}) = {n}, B6 §4.4 prints {n_w}");
        }
        // `h` outside Lightroom's own 0..1 is CLAMPED, not extrapolated: the
        // cubics are fits over exactly that interval and at h = 2 they return
        // m = 5e-4, which would paint the frame.
        assert_eq!(brush_kernel_exponents(-3.0), brush_kernel_exponents(0.0));
        assert_eq!(brush_kernel_exponents(9.0), brush_kernel_exponents(1.0));
        // NaN clamps to NEITHER end (`f32::clamp` propagates it), and a NaN
        // exponent turns the whole raster into black without a word.
        assert_eq!(brush_kernel_exponents(f32::NAN), brush_kernel_exponents(0.0));
    }

    /// The flow law against the MEASURED deposit — R29 Batch-6 §5.3.
    ///
    /// `D(f) = κf/(1−f+κf)`, κ = 0.1284 (§5.4, four cells, 0.12836 ± 0.00288).
    /// The four `mean` column values below are the deposits fitted per frame
    /// under screen accumulation over the 5 × 2 × 2 drag grid; the law's
    /// residual against them is +0.00099 / +0.00168 / +0.00193 / −0.00137, so
    /// the bar is 0.0025.
    ///
    /// MUTATION-LINED: the linear rival `D = 0.31252·f` — the best straight
    /// line through these very points — misses them by rms 0.0382, which is the
    /// 25× the last assert insists on (B6 quotes 12.7× over all sixteen cells).
    #[test]
    fn brush_flow_law_matches_the_measured_deposit() {
        const LADDER: [(f32, f32); 4] =
            [(0.10, 0.01307), (0.25, 0.03935), (0.50, 0.11182), (0.75, 0.27937)];
        let (mut odds_sq, mut lin_sq) = (0.0f64, 0.0f64);
        for (f, measured) in LADDER {
            let d = brush_flow_deposit(f);
            assert!(
                (d - measured).abs() <= 0.0025,
                "D({f}) = {d}, measured {measured} (B6 §5.3, max residual 0.00193)"
            );
            odds_sq += f64::from(d - measured).powi(2);
            lin_sq += f64::from(0.31252 * f - measured).powi(2);
        }
        // `D(1) = 1` EXACTLY and with no free parameter — κ/(1−1+κ) — which is
        // also exact in f32, so a flow-1 dab deposits its full density with no
        // epsilon (B6 §5.2 pins it at ≤ 0.0037 cost against a free fit).
        assert_eq!(brush_flow_deposit(1.0), 1.0, "D(1) must be exactly 1");
        assert_eq!(brush_flow_deposit(0.0), 0.0, "a flow-0 dab deposits nothing");
        // Off-domain flows clamp rather than run the odds law negative.
        assert_eq!(brush_flow_deposit(-1.0), 0.0);
        assert_eq!(brush_flow_deposit(7.0), 1.0);
        let (odds, lin) = ((odds_sq / 4.0).sqrt(), (lin_sq / 4.0).sqrt());
        assert!(
            lin > odds * 10.0,
            "the linear rival must lose decisively: odds {odds}, linear {lin}"
        );
    }

    /// Dabs accumulate by SCREEN — not by sum, not by max.
    ///
    /// R29 Batch-6 §5.2 re-adjudicated this out-of-sample on 20 fresh drags:
    /// mean field rms 0.01583 for screen against 0.02880 for sum-clamp (1.8×)
    /// and 0.05343 for max (3.4×). Two overlapping dabs is the smallest fixture
    /// that separates the three, and it separates them by far more than the
    /// 8-bit raster quantum.
    ///
    /// The flow is 0.75 on purpose: at low flow all three laws agree to within
    /// a couple of quantisation steps, which is exactly how a sum-clamp
    /// implementation could pass a weaker test.
    #[test]
    fn brush_dabs_accumulate_by_screen_not_sum_or_max() {
        let (fw, fh) = (480u32, 320u32);
        // Two dabs 0.2 apart, radius 0.25, so the frame centre sits at ρ = 0.4
        // from BOTH — an exact texel in x (240) and in y (160), so no bilinear
        // blend stands between the assertion and the accumulation.
        let g = probe_brush(&[(1.0, 0.25, 0.75, 0.5, "d 0.4 0.5\nd 0.6 0.5")]);
        let raster = brush_raster(&g, fw, fh).expect("two dabs");
        let got = mask_weight(&g, 0.5, 0.5, Some(&raster));
        let a = brush_flow_deposit(0.75) * brush_kernel(0.4, 0.5);
        let screen = 1.0 - (1.0 - a) * (1.0 - a);
        let sum = (a + a).min(1.0);
        let max = a;
        assert!(
            (got - screen).abs() <= 0.006,
            "α = {got}, screen says {screen} (8-bit raster, so the bar is ~1.5 quanta)"
        );
        assert!((got - sum).abs() >= 0.05, "sum-clamp would say {sum}");
        assert!((got - max).abs() >= 0.15, "max would say {max}");
        // Screen is also what makes a single dab reach exactly its deposit —
        // the premise the two-dab comparison stands on.
        let one = probe_brush(&[(1.0, 0.25, 0.75, 0.5, "d 0.5 0.5")]);
        let r1 = brush_raster(&one, fw, fh).expect("one dab");
        let solo = mask_weight(&one, 0.4, 0.5, Some(&r1));
        assert!((solo - a).abs() <= 0.006, "one dab deposits {a}, got {solo}");
    }

    /// The RASTER is the closed form, texel for texel — the join between the
    /// measured model and the pixels.
    ///
    /// The frame here is small enough that the raster is built 1:1, so this
    /// reads the stamped texels DIRECTLY and the only error left is the 8-bit
    /// quantum (1/255 = 0.0039, so ≤ 1/510 after rounding). Direct rather than
    /// through `mask_weight`: since R29 C2 a texel's own normalised centre is
    /// `(i + MASK_SAMPLE_CENTRE)/rw`, and going through the lookup to assert a
    /// property OF the raster would only put a round trip between the claim and
    /// the evidence. The lookup's half of the convention is pinned by
    /// `bitmap_mask_sampling_matches_the_producers_convention` and
    /// `every_mask_family_samples_at_pixel_centres`.
    ///
    /// MUTATION-LINED: dropping the `value` (density) factor, or scaling by the
    /// GROUP's `MaskValue` instead of the stroke's, moves every sample by 30 %.
    #[test]
    fn brush_raster_stamps_the_closed_form() {
        let (fw, fh) = (480u32, 320u32);
        // Density 0.7, flow 1 (so D = 1 exactly), one dab at the frame centre:
        // α(ρ) must be 0.7·k(ρ; h) with nothing else in the way.
        let g = probe_brush(&[(0.7, 0.25, 1.0, 0.5, "d 0.5 0.5")]);
        let raster = brush_raster(&g, fw, fh).expect("one dab");
        assert_eq!(raster.dimensions(), (fw, fh), "small frame = 1:1 raster");
        let texel = |i: u32, j: u32| raster.get_pixel(i, j)[0] as f32 / 255.0;
        // The dab's centre is texel coordinate 0.5·480 − 0.5 = 239.5 in x and
        // 0.5·320 − 0.5 = 159.5 in y — BETWEEN texels, because the frame has an
        // even number of them — and its half-extent is 120 texels on both axes.
        // Reading row 160 (half a texel below the centre), texel 240+d sits at
        //     ρ = √((d + 0.5)² + 0.5²) / 120.
        for d in [0i32, 12, 30, 60, 90, 114, 118] {
            let i = 240 + d;
            let rho = ((d as f32 + 0.5).powi(2) + 0.25).sqrt() / 120.0;
            let got = texel(i as u32, 160);
            let want = 0.7 * brush_kernel(rho, 0.5);
            assert!(
                (got - want).abs() <= 0.005,
                "texel ({i}, 160) (ρ = {rho}) reads {got}, the closed form says {want}"
            );
        }
        // …and stops. ρ = 1 is the outer support: on row 160 the support ends at
        // |i − 239.5| = √(120² − 0.5²) = 119.99896, so texel 359 is the last lit
        // one and 360 is blank — not faint.
        assert!(texel(359, 160) > 0.0, "texel 359 is inside the support");
        assert_eq!(texel(360, 160), 0.0, "support ends EXACTLY at ρ = 1 (B6 §4.1)");
        // A dab is a circle in PIXELS, not in normalised coordinates: at this
        // 3:2 frame the mask must reach 0.25 of the WIDTH on both axes, i.e.
        // 0.25·(480/320) = 0.375 of the HEIGHT. Both half-extents are therefore
        // 120 TEXELS, so ρ depends only on (Δi² + Δj²) and a pair of texels with
        // swapped offsets from (239.5, 159.5) must read EXACTLY equal.
        let horizontal = texel(347, 159); // Δ = (+107.5, −0.5)
        let vertical = texel(239, 267); //   Δ = (−0.5, +107.5)
        assert_eq!(horizontal, vertical, "the dab is not round in pixels");
        assert!(horizontal > 0.0, "premise: both probes are inside the dab");
    }

    /// A brush group with nothing drawable in it is INERT — including when the
    /// group's own `crs:MaskInverted` is set, which is the one way a mask with
    /// no coverage can become a whole-frame adjustment.
    ///
    /// The three ways a group can carry strokes and paint nothing, all of them
    /// states Lightroom also paints nothing for: no `d` token at all, a zero
    /// radius, and flow 0.
    #[test]
    fn a_brush_group_with_no_drawable_dab_is_inert() {
        let (fw, fh) = (240u32, 160u32);
        for (what, stream, radius, flow) in [
            ("state tokens only", "r 0.2\nf 1\nh 0.5", 0.2f32, 1.0f32),
            ("zero radius", "d 0.5 0.5", 0.0, 1.0),
            ("zero flow", "d 0.5 0.5", 0.2, 0.0),
            ("garbage", "wobble\nd\nd 0.5", 0.2, 1.0),
        ] {
            let g = probe_brush(&[(1.0, radius, flow, 0.5, stream)]);
            assert!(brush_raster(&g, fw, fh).is_none(), "{what} must not rasterise");
            assert_eq!(mask_weight(&g, 0.5, 0.5, None), 0.0, "{what} must draw nothing");
            // The hazard, said in a test: `1 − 0` on an inverted group would
            // apply the correction to the WHOLE frame at full strength.
            let MaskGeometry::Brush { name, blend_mode, value, strokes, .. } = g else {
                unreachable!()
            };
            let flipped =
                MaskGeometry::Brush { name, blend_mode, value, inverted: true, strokes };
            assert_eq!(
                mask_weight(&flipped, 0.5, 0.5, None),
                0.0,
                "{what}: an inverted group with no alpha must not cover the frame"
            );
        }
        // A group with no strokes at all takes the same path.
        assert!(brush_raster(&probe_brush(&[]), fw, fh).is_none());
        // And a non-brush geometry answers `None` here rather than being asked
        // to explain itself at the call site.
        assert!(brush_raster(&MaskGeometry::Bitmap { path: "x.png".into() }, fw, fh).is_none());
    }

    /// The GROUP's own `crs:MaskInverted` IS rendered — `1 − α` — and it is the
    /// only place it can be, because the importer deliberately does not lift it
    /// into `LocalAdjustment::inverted` (xmp.rs: one bit, one home).
    ///
    /// Measured `true` on 1 of 39 real groups (F2 anatomy), which is exactly
    /// the population size that makes this worth a test rather than a comment.
    #[test]
    fn a_brush_groups_own_inversion_is_rendered() {
        let (fw, fh) = (480u32, 320u32);
        let plain = probe_brush(&[(1.0, 0.25, 1.0, 1.0, "d 0.5 0.5")]);
        let MaskGeometry::Brush { name, blend_mode, value, strokes, .. } = plain.clone() else {
            unreachable!()
        };
        let inverted = MaskGeometry::Brush { name, blend_mode, value, inverted: true, strokes };
        let raster = brush_raster(&plain, fw, fh).expect("one dab");
        // The SAME raster serves both: inversion is a reading of the alpha, not
        // a different alpha (which is also why it costs no second rasterise).
        assert_eq!(brush_raster(&inverted, fw, fh).map(|r| r.dimensions()), Some((fw, fh)));
        for (nx, ny) in [(0.5f32, 0.5f32), (0.55, 0.5), (0.02, 0.02)] {
            let w = mask_weight(&plain, nx, ny, Some(&raster));
            let f = mask_weight(&inverted, nx, ny, Some(&raster));
            assert!((w + f - 1.0).abs() <= 1e-6, "({nx}, {ny}): {w} + {f} != 1");
        }
    }

    /// The raster's SIZE policy, asserted at a cost the assertion does not have
    /// to pay: exercising the work budget by really rasterising costs, by
    /// construction, exactly the budget.
    ///
    /// Three regimes, one function (`brush_raster_dims`): a small frame is 1:1,
    /// a 61 MP frame is capped at the 2048 long edge, and a group heavy enough
    /// to blow `BRUSH_RASTER_MAX_WORK` is shrunk until it fits — with
    /// `BRUSH_RASTER_MIN_EDGE` overriding the budget rather than the mask being
    /// destroyed.
    #[test]
    fn the_brush_raster_size_policy_is_bounded_three_ways() {
        // 1:1 while both bounds are slack — which is what makes a GUI preview's
        // lookup an exact texel hit rather than a bilinear blend.
        assert_eq!(brush_raster_dims(1_000.0, 480, 320), (480, 320));
        // The A7R IV frame, capped on the long edge and only there.
        assert_eq!(brush_raster_dims(1_000.0, 9504, 6336), (2048, 1365));
        // Heavy: 400 dabs each covering a 4000 × 3000 frame is 4.8e9 texels of
        // work at full size, so the budget takes the scale to sqrt(24e6/4.8e9).
        let heavy = 400.0 * 4000.0 * 3000.0;
        let (w, h) = brush_raster_dims(heavy, 4000, 3000);
        assert!(w < 2048, "the work budget must bite before the edge cap: {w}×{h}");
        assert!(w >= BRUSH_RASTER_MIN_EDGE, "and must not shrink past the floor: {w}");
        assert!(
            (f64::from(w) * f64::from(h) * 400.0) <= BRUSH_RASTER_MAX_WORK * 1.05,
            "the chosen size must actually meet the budget: {w}×{h}"
        );
        // The floor OVERRIDES the budget: an absurd cost cannot take the raster
        // to a single texel, because a 1 px mask is not a cheaper mask, it is a
        // wrong one. `BRUSH_MAX_DABS` is what bounds the overrun instead.
        let (w, h) = brush_raster_dims(f64::MAX, 4000, 3000);
        assert_eq!(w.max(h), BRUSH_RASTER_MIN_EDGE, "floor wins: {w}×{h}");
    }

    /// Real-machine probe, never run in CI (it allocates gigabytes): the
    /// PORTRAIT branch's transient on a 61 MP frame — the one orientation
    /// state that was unreachable for an ARW until v0.30.0, so its cost had
    /// never actually been paid on a Sony file.
    ///
    /// `orient_f32` casts to and from `Rgb32F` with zero copies
    /// (`bytemuck::cast_vec`), so the whole transient is `oriented`'s own
    /// `rotate270`: one fresh output frame while the source is still alive.
    /// 61 MP x 3 channels x 4 bytes = 732 MB per frame, so the accounting
    /// predicts a ~1.46 GB peak and no third copy. Measure with:
    ///
    /// ```text
    /// cargo test --lib -- --ignored --exact render::tests::portrait_rotation_peak_on_a_61mp_frame
    /// ```
    ///
    /// and read the printed before/after RSS, or wrap the test binary in
    /// `Start-Process -PassThru -Wait` and read `PeakWorkingSet64`.
    #[test]
    #[ignore = "real-machine probe: allocates ~1.5 GB"]
    fn portrait_rotation_peak_on_a_61mp_frame() {
        // 9504 x 6336 = the A7R IV sensor, landscape; Rotate270 turns it
        // portrait, which is exactly what an orientation-8 ARW now does.
        let (w, h) = (9504usize, 6336usize);
        let px = w * h;
        let frame_bytes = px * 3 * 4;
        let data: Vec<[f32; 3]> = vec![[0.5, 0.5, 0.5]; px];
        eprintln!(
            "one frame = {:.0} MB ({px} px); predicted peak = {:.0} MB (source + rotated copy)",
            frame_bytes as f64 / 1e6,
            2.0 * frame_bytes as f64 / 1e6
        );
        let (out, ow, oh) = orient_f32(data, w, h, Orientation::Rotate270);
        assert_eq!((ow, oh), (h, w), "the frame must come back portrait");
        assert_eq!(out.len(), px);
        assert_eq!(out.capacity() * 12, frame_bytes, "no third copy was made");
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
        let cov = mask_coverage(&grad, &grey, MaskFrame::AsRendered);
        // Rows sample at their CENTRES, ny = (y + 0.5)/20, so this 0→1
        // gradient reads t = 0.025 / 0.525 / 0.975 at rows 0 / 10 / 19; the
        // shipped Eased profile maps those to 0.0018 / 0.5375 / 0.9982 and the
        // map quantises to `round(w · 255)` = 0 / 137 / 255 (Clamped gave
        // 6 / 134 / 249). Pinned EXACTLY, both because the arithmetic is exact
        // and because rows 10 and 19 still separate the conventions: `y/h`
        // gives t = 0.0 / 0.5 / 0.95 → 0 / 128 / 253. Under Clamped the old
        // `assert_eq!(…, 0)` on row 0 was the whole reason this test caught
        // R29 C2 rather than sleeping through it; under Eased row 0 rounds to
        // 0 either way, so rows 10 and 19 carry that duty now.
        assert_eq!(cov.get_pixel(10, 0)[0], 0, "eased zero end is flat at the handle");
        assert_eq!(cov.get_pixel(10, 19)[0], 255, "eased full end reaches the plateau");
        assert_eq!(cov.get_pixel(10, 10)[0], 137, "eased midpoint sits past the linear centre");

        // (b) amount halves the whole map; inversion flips its direction.
        let half = LocalAdjustment { amount: 0.5, ..grad.clone() };
        assert!((mask_coverage(&half, &grey, MaskFrame::AsRendered).get_pixel(10, 19)[0] as i32 - 128).abs() < 15);
        let inv = LocalAdjustment { inverted: true, ..grad.clone() };
        let icov = mask_coverage(&inv, &grey, MaskFrame::AsRendered);
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
        let rcov = mask_coverage(&ranged, &split, MaskFrame::AsRendered);
        assert_eq!(rcov.get_pixel(3, 10)[0], 0, "dark side gated out");
        assert!(rcov.get_pixel(16, 10)[0] > 235, "bright side kept: {}", rcov.get_pixel(16, 10)[0]);
    }

    #[test]
    fn angled_linear_mask_matches_the_pixel_metric_closed_form() {
        let g = MaskGeometry::Linear {
            zero_x: 0.15,
            zero_y: 0.20,
            full_x: 0.85,
            full_y: 0.80,
        };
        let (w, h) = (300.0f32, 200.0f32);
        let (zx, zy, fx, fy) = match &g {
            MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => (*zero_x, *zero_y, *full_x, *full_y),
            _ => unreachable!(),
        };
        let (vx, vy) = (fx - zx, fy - zy);
        let (px, py) = (vx * w, vy * h);
        let den = px * px + py * py;
        for (nx, ny) in [(0.30, 0.25), (0.55, 0.50), (0.75, 0.65)] {
            let dx = (nx - zx) * w;
            let dy = (ny - zy) * h;
            let want = linear_coverage((dx * px + dy * py) / den, LinearFalloff::Eased);
            let got = mask_weight_in(&g, nx, ny, None, None, (w, h));
            assert!((got - want).abs() < 1e-6, "({nx},{ny}): got {got}, want {want}");
            let normalized = mask_weight(&g, nx, ny, None);
            assert!((got - normalized).abs() > 1e-3, "({nx},{ny}) did not expose aspect skew");
        }
    }

    #[test]
    fn axis_aligned_linear_coverage_is_byte_stable() {
        let (w, h) = (13u32, 9u32);
        let g = MaskGeometry::Linear {
            zero_x: 0.4,
            zero_y: 0.1,
            full_x: 0.4,
            full_y: 0.9,
        };
        let m = LocalAdjustment { mask: g.clone(), ..Default::default() };
        let reference = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([120, 120, 120])));
        let got = mask_coverage(&m, &reference, MaskFrame::AsRendered);
        let mut want = image::GrayImage::new(w, h);
        for (x, y, px) in want.enumerate_pixels_mut() {
            let weight = mask_weight(
                &g,
                (x as f32 + MASK_SAMPLE_CENTRE) / w as f32,
                (y as f32 + MASK_SAMPLE_CENTRE) / h as f32,
                None,
            );
            *px = image::Luma([(weight * 255.0).round() as u8]);
        }
        assert_eq!(got.as_raw(), want.as_raw(), "axis-aligned coverage changed byte-for-byte");
    }

    #[test]
    fn linear_coverage_clamped_is_byte_identical_to_head() {
        // Keep the pre-refactor ramp expression in the test itself. This is a
        // code snapshot, rather than a file fixture that could be regenerated
        // with the new implementation by mistake.
        for i in 0..=4096u32 {
            let t = (i as f32 - 512.0) / 3072.0;
            let head = t.clamp(0.0, 1.0);
            let got = linear_coverage(t, LinearFalloff::Clamped);
            // Exact f32 identity on purpose: the 16-bit form of this check
            // went green under hand mutation M-L2 (2026-08-28, the [0,1] clamp
            // removed) because a saturating integer cast turns negative and
            // above-one coverage into the same 0 / 65535 the head ramp gives.
            assert_eq!(got.to_bits(), head.to_bits(), "clamped coverage changed at t={t}");
        }
    }

    #[test]
    fn linear_coverage_eased_is_c1_at_both_ends() {
        let n = 2000usize;
        let values: Vec<u16> = (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                (linear_coverage(t, LinearFalloff::Eased) * 65535.0).round() as u16
            })
            .collect();
        let slopes: Vec<i32> = values.windows(2).map(|p| p[1] as i32 - p[0] as i32).collect();
        let through: f32 = slopes[400..1600].iter().map(|&v| v as f32).sum::<f32>() / 1200.0;
        assert!(through > 1.0, "the 16-bit ramp must have a measurable slope");
        let turnover = |part: &[i32]| {
            part.iter()
                .take(80)
                .take_while(|&&s| (s as f32 - through).abs() >= 0.25 * through)
                .count()
        };
        let max_edge_jump = |part: &[i32]| {
            part.windows(2).take(80).map(|p| (p[1] - p[0]).abs()).max().unwrap_or(0)
        };
        assert!(max_edge_jump(&slopes) <= 2, "eased full-end first difference is discontinuous");
        assert!(max_edge_jump(&slopes[slopes.len() - 81..]) <= 2, "eased zero-end first difference is discontinuous");
        assert!(turnover(&slopes) > 2, "eased full end turns over in one row");
        assert!(turnover(&slopes[slopes.len() - 80..]) > 2, "eased zero end turns over in one row");
        assert_eq!(linear_coverage(0.0, LinearFalloff::Eased), 0.0);
        assert_eq!(linear_coverage(1.0, LinearFalloff::Eased), 1.0);
    }

    #[test]
    fn linear_coverage_profiles_agree_at_the_handles() {
        for &t in &[0.0, 1.0] {
            assert_eq!(linear_coverage(t, LinearFalloff::Clamped), linear_coverage(t, LinearFalloff::Eased));
        }
    }

    #[test]
    fn shipped_linear_falloff_is_eased() {
        assert_eq!(LINEAR_FALLOFF, LinearFalloff::Eased);
    }

    #[test]
    fn linear_ramp_has_a_single_definition() {
        let src = include_str!("render.rs");
        let metric = &src[src.find("fn mask_weight_metric").unwrap()..src.find("/// Mask coverage").unwrap()];
        let weight = &src[src.find("fn mask_weight(g:").unwrap()..src.find("fn combined_mask_weight").unwrap()];
        let metric_linear = &metric[metric.find("MaskGeometry::Linear").unwrap()..metric.find("_ => mask_weight").unwrap()];
        let weight_linear = &weight[weight.find("MaskGeometry::Linear").unwrap()..weight.find("// `roundness`").unwrap()];
        assert_eq!(metric_linear.matches("linear_coverage(").count(), 2, "metric has a second ramp definition");
        assert_eq!(weight_linear.matches("linear_coverage(").count(), 1, "weight has a second ramp definition");
        assert!(!metric_linear.contains(".clamp(0.0, 1.0)"), "metric keeps an inline linear clamp");
        assert!(!weight_linear.contains(".clamp(0.0, 1.0)"), "weight keeps an inline linear clamp");
    }

    #[test]
    fn radial_mask_renders_byte_identical_to_the_clamped_baseline() {
        let (w, h) = (48u32, 32u32);
        let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 3 + y * 2) as u8, (x * 5) as u8, (y * 7) as u8])
        }));
        let mask = crate::recipe::LocalAdjustment {
            mask: MaskGeometry::Radial { top: 0.1, left: 0.1, bottom: 0.9, right: 0.9, feather: 0.4, roundness: 0.0, flipped: false, angle: 0.0, midpoint: 50.0, mask_version: 2 },
            ..Default::default()
        };
        let got = mask_coverage(&mask, &src, MaskFrame::AsRendered);
        let want = image::GrayImage::from_fn(w, h, |x, y| {
            let nx = (x as f32 + MASK_SAMPLE_CENTRE) / w as f32;
            let ny = (y as f32 + MASK_SAMPLE_CENTRE) / h as f32;
            let mut weight = mask_weight(&mask.mask, nx, ny, None);
            if mask.inverted {
                weight = 1.0 - weight;
            }
            image::Luma([(weight * 255.0).round() as u8])
        });
        assert_eq!(got.as_raw(), want.as_raw(), "mask coverage changed from the clamped baseline");
    }

    #[test]
    fn bitmap_mask_renders_byte_identical_to_the_clamped_baseline() {
        let (w, h) = (48u32, 32u32);
        let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 3 + y * 2) as u8, (x * 5) as u8, (y * 7) as u8])
        }));
        // Own directory, not the bare temp root (see `fixture_mask_path`).
        let raster_dir =
            std::env::temp_dir().join(format!("autoshade-linear-baseline-{}", std::process::id()));
        std::fs::create_dir_all(&raster_dir).unwrap();
        let raster_path = raster_dir.join("mask.png");
        let raster = image::GrayImage::from_fn(7, 5, |x, y| image::Luma([((x + y) * 20) as u8]));
        raster.save(&raster_path).unwrap();
        let mask = crate::recipe::LocalAdjustment {
            mask: MaskGeometry::Bitmap { path: raster_path.to_string_lossy().into_owned() },
            ..Default::default()
        };
        let got = mask_coverage(&mask, &src, MaskFrame::AsRendered);
        let bmp = load_mask_bitmap(&mask.mask, &crate::diag::dropped());
        let want = image::GrayImage::from_fn(w, h, |x, y| {
            let nx = (x as f32 + MASK_SAMPLE_CENTRE) / w as f32;
            let ny = (y as f32 + MASK_SAMPLE_CENTRE) / h as f32;
            let mut weight = mask_weight(&mask.mask, nx, ny, bmp.as_deref());
            if mask.inverted {
                weight = 1.0 - weight;
            }
            image::Luma([(weight * 255.0).round() as u8])
        });
        assert_eq!(got.as_raw(), want.as_raw(), "mask coverage changed from the clamped baseline");
        let _ = std::fs::remove_file(raster_path);
    }

    #[test]
    fn linear_mask_renders_the_eased_ramp() {
        let (w, h) = (48u32, 32u32);
        let src = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 3 + y * 2) as u8, (x * 5) as u8, (y * 7) as u8])
        }));
        let mask = crate::recipe::LocalAdjustment {
            mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 1.0, full_x: 0.5, full_y: 0.0 },
            ..Default::default()
        };
        let got = mask_coverage(&mask, &src, MaskFrame::AsRendered);
        let want = image::GrayImage::from_fn(w, h, |x, y| {
            let nx = (x as f32 + MASK_SAMPLE_CENTRE) / w as f32;
            let ny = (y as f32 + MASK_SAMPLE_CENTRE) / h as f32;
            let (zero_x, zero_y, full_x, full_y) = (0.5, 1.0, 0.5, 0.0);
            let t = (((nx - zero_x) * (full_x - zero_x) + (ny - zero_y) * (full_y - zero_y))
                / ((full_x - zero_x).powi(2) + (full_y - zero_y).powi(2)))
                .clamp(0.0, 1.0);
            image::Luma([(linear_coverage(t, LinearFalloff::Eased) * 255.0).round() as u8])
        });
        assert_eq!(got.as_raw(), want.as_raw(), "linear mask coverage did not use the eased ramp");

        let byte = |value: f32| (value * 255.0).round() as u8;
        assert_eq!(linear_coverage(0.25, LinearFalloff::Eased), 0.15625);
        assert_eq!(linear_coverage(0.25, LinearFalloff::Clamped), 0.25);
        assert_ne!(byte(linear_coverage(0.25, LinearFalloff::Eased)), byte(linear_coverage(0.25, LinearFalloff::Clamped)));
        for t in [-1.0, 0.0, 1.0, 2.0] {
            assert_eq!(linear_coverage(t, LinearFalloff::Eased), linear_coverage(t, LinearFalloff::Clamped));
        }
    }

    #[test]
    fn shipped_linear_ramp_is_eased_end_to_end() {
        // The probe geometry (vertical gradient, zero at 0.80, full at 0.35,
        // −2 EV) through the public f32 develop path. The tone pass blends
        // `p·(1 − w) + t·w`, so on a flat grey the mask weight is recovered
        // EXACTLY as `(base − row) / (base − full_plateau)` — no assumption
        // about the exposure model, and no 8-bit quantisation. The expected
        // profile is the LITERAL Hermite smoothstep, not `linear_coverage`,
        // so a mutation of the Eased arm cannot rewrite its own oracle.
        let (w, h) = (4usize, 1000usize);
        let base = 0.5_f32;
        let recipe = EditRecipe {
            masks: vec![crate::recipe::LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.80, full_x: 0.5, full_y: 0.35 },
                exposure_ev: -2.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut px = vec![[base; 3]; w * h];
        apply_develop_anon(&mut px, w, h, &recipe);
        let rows: Vec<f32> = (0..h).map(|y| px[y * w + w / 2][1]).collect();
        let full_plateau = rows[100];
        assert!(full_plateau < base - 0.2, "full end must darken by −2 EV: {full_plateau}");
        assert_eq!(rows[950].to_bits(), base.to_bits(), "zero end must be untouched");
        let coverage: Vec<f32> = rows.iter().map(|r| (base - r) / (base - full_plateau)).collect();
        let t_of = |y: usize| {
            let ny = (y as f32 + MASK_SAMPLE_CENTRE) / h as f32;
            ((0.80 - ny) / (0.80 - 0.35)).clamp(0.0, 1.0)
        };
        // (a) every row IS the literal smoothstep of its projected t (the
        // 0.001 work floor and the tone LUT leave ≤ 2e-3 of slack).
        for (y, &got) in coverage.iter().enumerate() {
            let t = t_of(y);
            let want = t * t * (3.0 - 2.0 * t);
            assert!((got - want).abs() < 2e-3, "row {y}: t {t:.4} rendered {got:.5}, smoothstep {want:.5}");
        }
        // (b) C1 at BOTH handles: the coverage slope over the ten rows just
        // inside each handle is ≤ 0.1 of the mid-ramp slope (Clamped: ≈ 1.0;
        // `t²`: ≈ 0.0 at the zero end but ≈ 2.0 at the full end).
        let full_row = (0.35 * h as f32) as usize;
        let zero_row = (0.80 * h as f32) as usize;
        let mid = (full_row + zero_row) / 2;
        let slope = |a: usize, b: usize| ((coverage[b] - coverage[a]) / (b - a) as f32).abs();
        let mid_slope = slope(mid - 5, mid + 5);
        let full_end = slope(full_row + 1, full_row + 11);
        let zero_end = slope(zero_row - 11, zero_row - 1);
        assert!(full_end < 0.1 * mid_slope, "full-end slope {full_end:.2e} is not eased vs mid {mid_slope:.2e}");
        assert!(zero_end < 0.1 * mid_slope, "zero-end slope {zero_end:.2e} is not eased vs mid {mid_slope:.2e}");
        // (c) the eased midpoint is 1.5× the linear slope of the same span.
        let linear_slope = 1.0 / (zero_row - full_row) as f32;
        let ratio = mid_slope / linear_slope;
        assert!((1.45..=1.55).contains(&ratio), "mid-ramp slope ratio {ratio:.4} is not ~1.5");
    }

    #[test]
    fn probe_fixture_round_trips_through_xmp() {
        let recipe = EditRecipe {
            masks: vec![crate::recipe::LocalAdjustment {
                mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.80, full_x: 0.5, full_y: 0.35 },
                exposure_ev: -2.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let xmp = crate::xmp::recipe_to_xmp(&recipe);
        let back = crate::xmp::xmp_to_recipe(&xmp);
        assert_eq!(back.masks.len(), 1);
        let MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } = back.masks[0].mask else { panic!("probe mask was not linear") };
        assert!((zero_x - 0.5).abs() < 1e-6 && (zero_y - 0.80).abs() < 1e-6);
        assert!((full_x - 0.5).abs() < 1e-6 && (full_y - 0.35).abs() < 1e-6);
        assert_eq!(back.masks[0].exposure_ev, -2.0);
        if crate::config::live_env_os("AUTOSHADE_GENERATE_LINEAR_PROBE").is_some() {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/linear-falloff/probe");
            std::fs::create_dir_all(&dir).unwrap();
            let encoded = (linear_to_srgb(0.18) * 65535.0).round() as u16;
            let image = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_pixel(3000, 2000, image::Rgb([encoded; 3]));
            image::DynamicImage::ImageRgb16(image).save(dir.join("probe.tif")).unwrap();
            std::fs::write(dir.join("probe.xmp"), xmp.as_bytes()).unwrap();
            std::fs::write(dir.join("probe-recipe.json"), serde_json::to_vec_pretty(&back).unwrap()).unwrap();
        }
    }

    #[test]
    fn gui_coverage_overlay_matches_an_angled_linear_render_weight() {
        let (w, h) = (96u32, 64u32);
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255])));
        let adj = LocalAdjustment {
            mask: MaskGeometry::Linear {
                zero_x: 0.08,
                zero_y: 0.15,
                full_x: 0.92,
                full_y: 0.85,
            },
            exposure_ev: -4.0,
            ..Default::default()
        };
        let recipe = EditRecipe { masks: vec![adj.clone()], ..Default::default() };
        let coverage = mask_coverage(&adj, &base, MaskFrame::AsRendered);
        let rendered = develop_preview_framed(&base, &recipe, &crate::diag::pixels(), MaskFrame::AsRendered).to_rgb8();
        let (mut claimed, mut agreed, mut clear, mut clean) = (0u32, 0u32, 0u32, 0u32);
        for (x, y, p) in coverage.enumerate_pixels() {
            let lit = rendered.get_pixel(x, y).0[1];
            if p[0] > 200 {
                claimed += 1;
                if lit < 160 {
                    agreed += 1;
                }
            } else if p[0] < 20 {
                clear += 1;
                if lit > 200 {
                    clean += 1;
                }
            }
        }
        assert!(claimed > 500 && clear > 200, "premise: {claimed} covered / {clear} clear px");
        assert!(agreed * 100 >= claimed * 97, "angled overlay coverage disagrees: {agreed}/{claimed}");
        assert!(clean * 100 >= clear * 97, "angled overlay shows a clean pixel as covered: {clean}/{clear}");
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
            apply_develop_anon(&mut d, 1, 1, r);
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
        apply_develop_anon(&mut data, 2, 1, &EditRecipe::default());
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
        apply_develop_anon(&mut data, w, h, &r);
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
    /// bytes are identical on every platform and compiler. (Its INPUT bytes;
    /// what `apply_dehaze` makes of them is not — see the golden test below,
    /// which is why this helper carries the same platform gate.)
    #[cfg(all(windows, target_arch = "x86_64"))]
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

    /// PLATFORM-GATED to the box the golden was captured on: Windows,
    /// x86_64. The goldens are frozen BITS, and bit-identity was proven
    /// against the pre-split implementation ON THAT BOX — it was never
    /// promised across libm implementations, and this test is the only thing
    /// in the suite that would have to be re-captured to claim it.
    ///
    /// Why the bits move: `apply_dehaze`'s two `powf` sites — the transfer
    /// LUTs built in `transfer_luts`, and the airlight histogram in
    /// `dehaze_airlight`, which deliberately keeps the exact `powf` — are not
    /// correctly rounded, and their last bit differs between the MSVC CRT,
    /// glibc and Apple's libm. Evidence from CI run 32398395462, where this
    /// test first met a non-Windows runner: ubuntu (x86_64, glibc) reproduced
    /// the +60 hash EXACTLY and missed only −40, while macOS (aarch64) missed
    /// +60 — so it is the libm, not the word size. And on both runners pixel
    /// 0 came back bit-identical to this box ([0.5183644, 0.57828087,
    /// 0.68996465] at +60; [0.72724235, 0.75048673, 0.7995405] at −40,
    /// re-measured here 2026-08-20), i.e. what drifted is the low bits of a
    /// few of the other 127 pixels — not the airlight estimate, which would
    /// have moved every pixel including that one.
    ///
    /// What still covers the OTHER platforms, all of them ungated and all in
    /// this module: `dehaze_zero_is_exact_identity` (bit-exact, but a no-op
    /// so it needs no libm), `dehaze_positive_recovers_a_hazy_ramp`,
    /// `dehaze_protects_bright_sky_channel_order`,
    /// `dehaze_negative_adds_a_veil_without_clipping`,
    /// `dehaze_is_gentle_on_a_clean_image` and
    /// `dehaze_airlight_does_not_phase_lock_to_any_small_period` pin the
    /// model's behaviour to tolerances a 1-ULP drift cannot break; and
    /// `mask_dehaze_renders_only_inside_the_mask` covers the reuse the split
    /// existed for. What none of those seven can see, and this one can, is a
    /// change to the model far below their tolerances — measured 2026-08-20
    /// by mutation: `DEHAZE_K` 0.75 → 0.7500001 (≈1.7 ULP) leaves all seven
    /// green and turns this one red. That is the size of a refactor
    /// regression, and a refactor is only ever authored on one machine.
    #[cfg(all(windows, target_arch = "x86_64"))]
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
        apply_develop_anon(&mut global, w, h, &EditRecipe { clarity: 50.0, ..Default::default() });
        let mut local = data.clone();
        apply_develop_anon(
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
        // The bare operator at texture's own calibration: small radius
        // (0.5% of the short edge, floored at 2 px) and NO midtone mask —
        // which since R25 B2 is also what the GLOBAL texture stage runs, and
        // `global_texture_at_full_coverage_equals_the_masked_operator` below
        // closes that loop directly.
        // Bit-exact, measured: 0 differing channels of 9216.
        let (data, w, h) = detail_frame();
        let radius = ((0.005 * w.min(h) as f32).round() as usize).max(2);
        assert_eq!(radius, 2, "the 48px short edge must land on the 2px floor");
        let mut reference = data.clone();
        unsharp_luma(&mut reference, w, h, radius, 0.5, false);
        let mut local = data.clone();
        apply_develop_anon(
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

    /// R25 B2: the strongest pin this batch can offer. R22 gave the mask
    /// texture slider an operator and had to compare it against a hand-rolled
    /// `unsharp_luma` call, because `EditRecipe` had no `texture` field to
    /// compare with — the comment on the test above said exactly that. Now it
    /// does, and a full-coverage mask must be BIT-IDENTICAL to the global
    /// stage: same radius model, same amount scale, same absent midtone mask.
    ///
    /// Change either radius formula and this fails, which is the point: the
    /// two are one calibration, not two that happen to agree today.
    #[test]
    fn global_texture_at_full_coverage_equals_the_masked_operator() {
        let (data, w, h) = detail_frame();
        let mut global = data.clone();
        apply_develop_anon(&mut global, w, h, &EditRecipe { texture: 50.0, ..Default::default() });
        let mut local = data.clone();
        apply_develop_anon(
            &mut local,
            w,
            h,
            &EditRecipe {
                masks: vec![LocalAdjustment {
                    // The degenerate Linear gradient every full-coverage
                    // comparison in this module uses: weight 1 everywhere.
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
            frame_bits_fnv64(&global),
            "global texture must move this frame, or the comparison is vacuous"
        );
        assert_eq!(
            frame_bits_fnv64(&global),
            frame_bits_fnv64(&local),
            "full-coverage local texture must equal global texture bit-for-bit"
        );
    }

    /// A frame of vertical SINUSOIDAL stripes at `period` px, ±0.06 around mid
    /// grey. Sinusoidal and not square: a square wave's edges are broadband, so
    /// its peak-to-peak measures every frequency at once and cannot say which
    /// BAND a filter took — which is the whole question below. One tone per
    /// probe, and the peak-to-peak reads that tone's transfer directly.
    ///
    /// `period` is fractional because two of the acceptance anchors are FFT bin
    /// centres (64/3 px and 2.9942 px), not round pixel counts. That costs
    /// nothing in accuracy: every kernel here is symmetric and therefore
    /// zero-phase, so the filtered samples are the input samples scaled by the
    /// transfer and the peak-to-peak RATIO is exact whatever the sampling
    /// phases land on.
    fn stripe_frame(w: usize, h: usize, period: f32) -> Vec<[f32; 3]> {
        let mut data = Vec::with_capacity(w * h);
        for _ in 0..h {
            for x in 0..w {
                let phase = std::f32::consts::TAU * x as f32 / period;
                let v = 0.5 + 0.06 * phase.sin();
                data.push([v, v, v]);
            }
        }
        data
    }

    /// The nine acceptance periods, and the closed form's own transfer at each
    /// of the five ladder steps — b8-analysis-2 §6-3, the RIGHT half of the
    /// table (the left half is the Lightroom measurement, quoted below as
    /// ground truth but deliberately NOT asserted: it carries this frame's
    /// scene dependence, and the operator is not LTI).
    ///
    /// ```text
    ///   period  ν c/px   LR −10  −25   −50   −75  −100 ‖ closed −10  −25   −50   −75  −100
    ///     256  0.00391  0.9993 0.9982 .9965 .9951 .9939 ‖ 0.9987 0.9970 .9947 .9929 .9913
    ///     128  0.00781  0.9957 0.9897 .9813 .9744 .9689 ‖ 0.9952 0.9890 .9804 .9734 .9678
    ///      64  0.01562  0.9853 0.9655 .9376 .9151 .8969 ‖ 0.9855 0.9665 .9403 .9192 .9020
    ///      32  0.03125  0.9762 0.9439 .8986 .8619 .8324 ‖ 0.9743 0.9406 .8941 .8568 .8262
    ///      21  0.04688  0.9728 0.9359 .8840 .8419 .8081 ‖ 0.9720 0.9350 .8842 .8435 .8100
    ///      16  0.06250  0.9694 0.9283 .8701 .8231 .7852 ‖ 0.9700 0.9305 .8762 .8326 .7968
    ///       8  0.12500  0.9611 0.9091 .8358 .7769 .7291 ‖ 0.9590 0.9049 .8306 .7709 .7220
    ///       4  0.25000  0.9354 0.8533 .7374 .6443 .5695 ‖ 0.9378 0.8558 .7431 .6526 .5783
    ///       3  0.33398  0.9297 0.8438 .7224 .6254 .5466 ‖ 0.9317 0.8418 .7182 .6188 .5373
    /// ```
    const TEXTURE_ANCHOR_PERIODS: [f32; 9] =
        [256.0, 128.0, 64.0, 32.0, 64.0 / 3.0, 16.0, 8.0, 4.0, 2.9942];
    const TEXTURE_ANCHOR_STEPS: [f32; 5] = [0.10, 0.25, 0.50, 0.75, 1.00];
    const TEXTURE_ANCHOR_CLOSED: [[f32; 5]; 9] = [
        [0.9987, 0.9970, 0.9947, 0.9929, 0.9913],
        [0.9952, 0.9890, 0.9804, 0.9734, 0.9678],
        [0.9855, 0.9665, 0.9403, 0.9192, 0.9020],
        [0.9743, 0.9406, 0.8941, 0.8568, 0.8262],
        [0.9720, 0.9350, 0.8842, 0.8435, 0.8100],
        [0.9700, 0.9305, 0.8762, 0.8326, 0.7968],
        [0.9590, 0.9049, 0.8306, 0.7709, 0.7220],
        [0.9378, 0.8558, 0.7431, 0.6526, 0.5783],
        [0.9317, 0.8418, 0.7182, 0.6188, 0.5373],
    ];

    /// Peak-to-peak luma of the middle row, sampled away from the borders so
    /// the box blur's clamped edge seeding cannot answer for the interior.
    fn stripe_contrast(data: &[[f32; 3]], w: usize, h: usize) -> f32 {
        let row = (h / 2) * w;
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for x in (w / 4)..(3 * w / 4) {
            let l = luma601(&data[row + x]);
            lo = lo.min(l);
            hi = hi.max(l);
        }
        hi - lo
    }

    /// R29 Batch-8-2 — **THE 45 ACCEPTANCE ANCHORS**, the arbiter of the
    /// negative half.
    ///
    /// Nine periods × five ladder steps against the closed form of
    /// b8-analysis-2 §6-1, at the σ pair a 6240 × 4160 render raster asks for
    /// (σ₁ = 12.9938 px, σ₂ = 1.1740 px) — the raster the ground truth was
    /// measured on. The probe is a SYNTHETIC sinusoid, not a photograph: the
    /// closed form is an LTI model and only an LTI probe can say whether this
    /// implementation realises it. Lightroom's own column is quoted beside it
    /// in [`TEXTURE_ANCHOR_CLOSED`]'s doc as ground truth and is NOT asserted —
    /// the operator is amplitude-adaptive, so the measured column carries that
    /// frame's scene dependence and belongs in a comment, not an assertion.
    ///
    /// **Tolerance ±0.02, and it is a budget, not a round number** (§6-3): the
    /// model's own rms residual 0.0048 and max residual 0.0163, the
    /// cross-resolution leave-out rms 0.0048, and the JPEG noise bias < 0.0040.
    /// **Do not widen it to admit an implementation** — the whole point of the
    /// grid is that it rejected three cheaper kernel schemes (the numbers are
    /// in [`texture_negative_pass`]'s doc). As shipped the worst anchor sits at
    /// 0.0037, a fifth of the budget.
    ///
    /// A 2048 × 64 strip rather than a 4160 px square: σ is a PARAMETER of
    /// `texture_negative_pass`, the stripes are constant down the frame so the
    /// vertical passes are exactly identity, and 2048 px carries eight cycles
    /// of the longest anchor with the sampled window kept clear of the clamped
    /// borders.
    #[test]
    fn texture_negative_hits_the_forty_five_lightroom_anchors() {
        let (w, h) = (2048usize, 64usize);
        // THE RASTER THE GROUND TRUTH WAS MEASURED ON — read through the
        // shipping function, so a change to the short-edge fractions or to the
        // `min(w, h)` normalisation moves the whole grid.
        let (sigma_coarse, sigma_fine) = texture_sigmas(6240, 4160);
        let mut worst = 0.0f32;
        let mut worst_at = (0.0f32, 0.0f32);
        for (pi, &period) in TEXTURE_ANCHOR_PERIODS.iter().enumerate() {
            let src = stripe_frame(w, h, period);
            let before = stripe_contrast(&src, w, h);
            for (si, &t) in TEXTURE_ANCHOR_STEPS.iter().enumerate() {
                let mut got = src.clone();
                texture_negative_pass(&mut got, w, h, t, sigma_coarse, sigma_fine, |_, _, _| 1.0);
                let transfer = stripe_contrast(&got, w, h) / before;
                let want = TEXTURE_ANCHOR_CLOSED[pi][si];
                let dev = (transfer - want).abs();
                if dev > worst {
                    worst = dev;
                    worst_at = (period, t);
                }
                assert!(
                    dev <= 0.02,
                    "anchor {period:.4} px @ t={t:.2}: got {transfer:.4}, closed form {want:.4}, \
                     off by {dev:.4} — the ±0.02 budget is b8-analysis-2 §6-3 and is not the \
                     thing to widen"
                );
            }
        }
        eprintln!("texture anchors: worst |dev| {worst:.4} at period {:.4} px, t={:.2}", worst_at.0, worst_at.1);
        // …and the grid must actually be TIGHT, or a future implementation
        // could drift most of the way across the budget unnoticed. 0.006 is
        // chosen against a measurement, not for roundness: the shipped kernels
        // sit at 0.0037, and dropping just the Gaussian-semigroup correction
        // (blurring the fine plane by the whole σ₁ instead of the residual σ′,
        // a 4 % σ error) takes the grid to 0.0092 while still inside ±0.02.
        // The arithmetic here is plain f32 with no reordering and no
        // contraction, so there is no platform drift for the margin to absorb.
        assert!(
            worst < 0.006,
            "the shipped kernels measured 0.0037 across this grid; {worst:.4} means the filter \
             changed, not that the budget was always this loose"
        );
    }

    /// R29 Batch-8-2 — the SHAPE, and the two operators it supersedes.
    ///
    /// The acceptance grid above pins the numbers; this pins what the numbers
    /// MEAN, which is the claim two previous designs got wrong:
    ///
    /// * **pre-R28** ran `unsharp_luma` at `amount = −1`, whose transfer is
    ///   `G` exactly — a full Gaussian blur that erased fine detail (measured
    ///   in the visual-inspection pack, σ −92 %). It is CALLED here, not
    ///   paraphrased, so the comparison is against the real thing.
    /// * **R28 Batch-5** replaced it with a NOTCH — `1 − |t|·(G_f − G_c)`,
    ///   returning to 1 at both spectral ends — and R29 B8-2 refuted the shape:
    ///   Lightroom's negative Texture is a monotone HIGH-SHELF, taking MOST out
    ///   of the finest scales, where the notch kept 0.9992 of a 4 px pattern.
    ///
    /// So monotonicity is the historical assertion: any notch — including the
    /// one this file shipped in v0.34.0 — turns back up at the fine end and
    /// fails it.
    #[test]
    fn texture_negative_is_a_monotone_high_shelf_not_a_notch_and_not_a_blur() {
        let (w, h) = (2048usize, 64usize);
        let (sigma_coarse, sigma_fine) = texture_sigmas(6240, 4160);
        // The pre-R28 operator's own radius on that raster: 0.5 % of 4160.
        let old_radius = ((0.005 * 4160.0_f32).round() as usize).max(2);
        let mut curve = Vec::new();
        for &period in TEXTURE_ANCHOR_PERIODS.iter() {
            let src = stripe_frame(w, h, period);
            let before = stripe_contrast(&src, w, h);
            let mut now = src.clone();
            texture_negative_pass(&mut now, w, h, 1.0, sigma_coarse, sigma_fine, |_, _, _| 1.0);
            let mut pre_r28 = src.clone();
            unsharp_luma(&mut pre_r28, w, h, old_radius, -1.0, false);
            curve.push((
                period,
                stripe_contrast(&now, w, h) / before,
                stripe_contrast(&pre_r28, w, h) / before,
            ));
        }
        for (p, now, old) in &curve {
            eprintln!("texture −100 @ {p:8.4} px: now {now:.4}, pre-R28 {old:.4}");
        }

        // 1) MONOTONE from coarse to fine. `TEXTURE_ANCHOR_PERIODS` runs long
        //    period → short, so the transfer must never rise.
        for pair in curve.windows(2) {
            let (pa, ha, _) = pair[0];
            let (pb, hb, _) = pair[1];
            assert!(
                hb <= ha + 1e-3,
                "a high shelf never turns back up: {pa:.4} px keeps {ha:.4} but {pb:.4} px \
                 keeps {hb:.4} — that is the notch shape B8-2 refuted"
            );
        }

        // 2) The fine end is where MOST is taken, and the plateau is the
        //    model's own `1 − (A₁+A₂) = 0.5227`, approached from above.
        let (_, fine_now, fine_old) = *curve.last().expect("nine anchors");
        assert!(
            (0.50..0.60).contains(&fine_now),
            "at −100 the finest anchor must sit on the plateau (0.5227), kept {fine_now:.4}"
        );
        assert!(
            fine_old < 0.05,
            "the pre-R28 branch really did erase it ({fine_old:.4}) — if this fails the \
             comparison is not measuring what the ledger says it measures"
        );

        // 3) …while the COARSE end is nearly untouched: H(ν→0) = 0.9996 on the
        //    clean base, and B8's +1.8 % low-frequency LIFT was pure capture
        //    sharpening, so a value above 1 here would be re-importing the
        //    confound the second batch removed.
        let (_, coarse_now, _) = curve[0];
        assert!(
            (0.98..=1.0).contains(&coarse_now),
            "a 256 px pattern must survive at −100 and must NOT be lifted above 1, kept \
             {coarse_now:.4}"
        );
    }

    /// R29 Batch-8-2 — the σ model and the preview clamp, in one test because
    /// they are one decision: σ binds to the RENDER raster's short edge, and an
    /// arm whose σ has gone sub-pixel on that raster is dropped rather than
    /// approximated (user ruling 2026-08-21).
    #[test]
    fn texture_sigmas_track_the_render_rasters_short_edge_and_clamp_sub_pixel_arms() {
        // The measurement raster, both ways round: `min(w, h)`, never the first
        // argument and never the delivery size.
        let (c, f) = texture_sigmas(6240, 4160);
        assert!((c - 12.9938).abs() < 1e-3, "σ₁ at short edge 4160 is 12.9938 px, got {c:.4}");
        assert!((f - 1.1740).abs() < 1e-3, "σ₂ at short edge 4160 is 1.1740 px, got {f:.4}");
        assert_eq!(texture_sigmas(4160, 6240), (c, f), "the SHORT edge, whichever axis it is on");
        // …and it is proportional, not a fixed pixel count: half the raster,
        // half the σ. (Which reading Lightroom uses was settled at 16×
        // separation — b8-analysis-2 §1 ruling 4.)
        let (c2, f2) = texture_sigmas(3120, 2080);
        assert!((c2 * 2.0 - c).abs() < 1e-3 && (f2 * 2.0 - f).abs() < 1e-3);

        // The GUI preview raster (`gui/model.rs:296`: 1280 long edge, so ≈ 853
        // short) puts σ₂ at 0.241 px — under half a pixel, so the fine arm goes.
        let (pc, pf) = texture_sigmas(1280, 853);
        assert!(pf < TEXTURE_MIN_SIGMA_PX, "preview σ₂ = {pf:.4} px is the clamped case");
        assert!(pc >= TEXTURE_MIN_SIGMA_PX, "preview σ₁ = {pc:.4} px still renders");
        // 2080 is the first common raster where the fine arm survives — the
        // half-size export of the fixture set.
        assert!(texture_sigmas(3120, 2080).1 >= TEXTURE_MIN_SIGMA_PX);

        // The clamp is visible in the pixels, not just in the constants: on the
        // preview raster the fine arm's share of the depth is simply absent, so
        // a 3 px pattern keeps MORE than the full model would leave it.
        let (w, h) = (1024usize, 64usize);
        let src = stripe_frame(w, h, 3.0);
        let before = stripe_contrast(&src, w, h);
        let transfer = |sc: f32, sf: f32| {
            let mut d = src.clone();
            texture_negative_pass(&mut d, w, h, 1.0, sc, sf, |_, _, _| 1.0);
            stripe_contrast(&d, w, h) / before
        };
        let preview = transfer(pc, pf);
        // The same coarse arm with a fine arm that is NOT sub-pixel, so the
        // comparison isolates the clamp and not the σ pair.
        let both_arms = transfer(pc, 1.0);
        eprintln!("preview clamp @ 3 px: fine arm off {preview:.4}, fine arm on {both_arms:.4}");
        assert!(
            preview > both_arms + 0.15,
            "the clamped preview must be visibly WEAKER (higher transfer) than a rendered fine \
             arm — off {preview:.4}, on {both_arms:.4}"
        );

        // Below a 228 px short edge even the coarse arm's box³ radius rounds to
        // zero, and the pass declines rather than mis-applying the fine arm's
        // high-pass at the coarse arm's amplitude.
        let tiny = texture_sigmas(300, 200);
        let mut small = stripe_frame(32, 32, 4.0);
        let untouched = small.clone();
        texture_negative_pass(&mut small, 32, 32, 1.0, tiny.0, tiny.1, |_, _, _| 1.0);
        assert_eq!(
            frame_bits_fnv64(&small),
            frame_bits_fnv64(&untouched),
            "a 200 px short edge has no representable arm left — the pass must be a no-op"
        );
    }

    /// ONE calibration: the negative half is the same operator inside a mask as
    /// outside it, bit for bit — the law the positive half is already pinned to
    /// two tests above, restated on the branch that was rebuilt.
    #[test]
    fn mask_negative_texture_at_full_coverage_is_the_global_one_bit_for_bit() {
        let (w, h) = (800usize, 800usize);
        let recipe = EditRecipe { texture: -100.0, ..Default::default() };
        let src = stripe_frame(w, h, 16.0);
        let mut global = src.clone();
        apply_develop_anon(&mut global, w, h, &recipe);
        assert_ne!(
            frame_bits_fnv64(&src),
            frame_bits_fnv64(&global),
            "global texture −100 must move this frame, or the comparison below is vacuous"
        );
        let mut local = src.clone();
        apply_develop_anon(
            &mut local,
            w,
            h,
            &EditRecipe {
                masks: vec![LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.5, full_x: 0.5, full_y: 0.5 },
                    amount: 1.0,
                    texture: -100.0,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        assert_eq!(
            frame_bits_fnv64(&global),
            frame_bits_fnv64(&local),
            "full-coverage local texture −100 must equal global texture −100 bit-for-bit"
        );
    }

    /// R25 P8: the honest version of the claim beside the fused curve stage.
    ///
    /// `render.rs`'s "a full-coverage mask carrying a curve lands where the
    /// same curve set globally would" was written as an ORDER argument (stage
    /// 1 -> 1b -> 3, mirrored) and reads as a bit-exactness one, like the
    /// clarity and texture twins above. It is not: the two paths compose the
    /// same LUTs through different arithmetic — the global chain runs the
    /// master curve over the whole frame and then the per-channel pass over it
    /// again, while the mask fuses both into one pixel step and blends the
    /// result at weight 1 — so the last bit of a deep-shadow code can differ.
    /// Measured on this frame: the numbers this test prints.
    ///
    /// The tolerance IS the claim now. One 8-bit code is the finest thing an
    /// export, a preview or a Lightroom comparison can show, so a difference
    /// under it is invisible everywhere the promise is made — and a difference
    /// OVER it means the two paths have really come apart, which is what this
    /// catches and a comment could not.
    #[test]
    fn mask_curves_at_full_coverage_match_the_global_curves_within_one_code() {
        use crate::recipe::CurvePoint;
        let pts = |v: &[(u8, u8)]| -> Vec<CurvePoint> {
            v.iter().map(|(i, o)| CurvePoint { input: *i, output: *o }).collect()
        };
        let main = pts(&[(0, 0), (64, 40), (192, 210), (255, 255)]);
        let red = pts(&[(0, 10), (255, 250)]);
        let green = pts(&[(0, 0), (128, 120), (255, 255)]);
        let blue = pts(&[(0, 5), (128, 140), (255, 255)]);
        // NOT `detail_frame`: its three channels sit within 0.06 of each
        // other, and the difference this test measures lives in the fused
        // path's unconditional `apply_sat_vibrance` at factor 1 — where
        // `l + (r - l)` returns `r` exactly whenever the channel is near the
        // luma. A frame with real chroma spread and real deep shadows is what
        // makes the two paths' arithmetic differ at all; on a flat one this
        // test would pass by measuring nothing.
        let (w, h) = (61usize, 97usize);
        let data: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let t = i as f32 / (w * h) as f32;
                [t * t, 1.0 - t, (0.5 - t).abs() * 1.8 + 0.002]
            })
            .collect();
        let mut global = data.clone();
        apply_develop_anon(
            &mut global,
            w,
            h,
            &EditRecipe {
                tone_curve: main.clone(),
                red_curve: red.clone(),
                green_curve: green.clone(),
                blue_curve: blue.clone(),
                ..Default::default()
            },
        );
        let mut local = data.clone();
        apply_develop_anon(
            &mut local,
            w,
            h,
            &EditRecipe {
                masks: vec![LocalAdjustment {
                    // The degenerate Linear gradient every full-coverage
                    // comparison in this module uses: weight 1 everywhere.
                    mask: MaskGeometry::Linear {
                        zero_x: 0.5,
                        zero_y: 0.5,
                        full_x: 0.5,
                        full_y: 0.5,
                    },
                    amount: 1.0,
                    main_curve: main,
                    red_curve: red,
                    green_curve: green,
                    blue_curve: blue,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        assert_ne!(
            frame_bits_fnv64(&data),
            frame_bits_fnv64(&global),
            "the curves must move this frame, or the comparison is vacuous"
        );
        let (mut worst, mut differing) = (0.0f32, 0usize);
        for (g, l) in global.iter().zip(&local) {
            for c in 0..3 {
                let d = (g[c] - l[c]).abs();
                if d > 0.0 {
                    differing += 1;
                }
                worst = worst.max(d);
            }
        }
        let channels = global.len() * 3;
        eprintln!(
            "curve equivalence: {differing}/{channels} channel(s) differ, worst {:.6} LSB",
            worst * 255.0
        );
        // The drift has to OCCUR, or the tolerance is agreed with vacuously —
        // which is exactly how the old comment survived: on a low-chroma frame
        // the two paths really are bit-identical and nothing contradicts a
        // claim of bit-exactness.
        assert!(differing > 0, "no channel differed — this frame does not exercise the fused path");
        assert!(
            worst <= 1.0 / 255.0,
            "the fused mask curve stage drifted past one 8-bit code: worst {:.6} LSB over              {differing}/{channels} channel(s)",
            worst * 255.0
        );
    }

    /// R25 B2, the global twin of `mask_texture_halo_is_narrower_than_mask_
    /// clarity_halo`: the two radii are the whole reason both sliders exist,
    /// and the global pair must keep the same separation the masked pair has.
    #[test]
    fn texture_halo_is_narrower_than_clarity_halo() {
        let (w, h) = (128usize, 64usize);
        // 0.35/0.65 keeps both plateaus inside the midtone mask, which would
        // zero clarity's effect at 0.0 and 1.0.
        let edge: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = if i % w < w / 2 { 0.35f32 } else { 0.65 };
                [v, v, v]
            })
            .collect();
        let halo_of = |r: EditRecipe| -> usize {
            let mut out = edge.clone();
            apply_develop_anon(&mut out, w, h, &r);
            let row = (h / 2) * w;
            (0..w)
                .filter(|x| (out[row + x][0] - edge[row + x][0]).abs() > 1e-3)
                .map(|x| x.abs_diff(w / 2))
                .max()
                .unwrap_or(0)
        };
        let clarity = halo_of(EditRecipe { clarity: 60.0, ..Default::default() });
        let texture = halo_of(EditRecipe { texture: 60.0, ..Default::default() });
        assert!(texture >= 2, "texture must actually reach the edge: {texture}");
        assert!(
            texture * 2 < clarity,
            "texture halo ({texture}px) must be far narrower than clarity's ({clarity}px)"
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
            apply_develop_anon(&mut out, w, h, &EditRecipe { masks: vec![m], ..Default::default() });
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
        apply_develop_anon(
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
                // The eased handle is below the existing 0.001 work floor at
                // the first pixel next to the zero edge (t = 1/64,
                // smoothstep(t) < 0.001), so column 31 is intentionally part
                // of the unchanged plateau under the shipped profile.
                if x >= w / 2 - 1 {
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
            // R23-1b: the same rule for the two new local controls — a
            // hue-only or sharpness-only bitmap mask must load its raster and
            // read as active, or it renders nothing for the same reason.
            ("hue", LocalAdjustment { hue: 40.0, ..Default::default() }),
            ("sharpness", LocalAdjustment { sharpness: -60.0, ..Default::default() }),
        ] {
            assert!(engine_active(&m), "local {name} alone must count as active");
        }
    }

    /// R25 P6. The registry's one-hot/zeroing probe
    /// (`catalogue::local_tiers_agree_with_the_engines_own_activity_gate`)
    /// cannot reach a `Shape::Curve` row — neither `1.0` nor `0.0` is a curve
    /// — so it skips all four and their `Rendered` claim would be a free
    /// declaration. This is the compensating test that probe's doc names, and
    /// it probes BOTH directions the same way the scalar arm does.
    #[test]
    fn engine_active_counts_local_point_curves() {
        use crate::recipe::CurvePoint;
        let lift = || vec![CurvePoint { input: 64, output: 96 }];
        assert!(!engine_active(&LocalAdjustment::default()), "premise: a bare mask is inert");
        for (name, m) in [
            ("main_curve", LocalAdjustment { main_curve: lift(), ..Default::default() }),
            ("red_curve", LocalAdjustment { red_curve: lift(), ..Default::default() }),
            ("green_curve", LocalAdjustment { green_curve: lift(), ..Default::default() }),
            ("blue_curve", LocalAdjustment { blue_curve: lift(), ..Default::default() }),
        ] {
            // ONE-HOT: neutral everywhere but this curve ⇒ the mask wakes up.
            assert!(engine_active(&m), "local {name} alone must count as active");
            // ZEROING: an already-active mask with this curve emptied still
            // renders (the other term holds it up) — so the term is additive,
            // not a gate that swallows the rest.
            let with_slider = LocalAdjustment { exposure_ev: 1.0, ..m.clone() };
            assert!(engine_active(&with_slider), "premise: the two-term mask is active");
            let mut without = with_slider;
            without.main_curve.clear();
            without.red_curve.clear();
            without.green_curve.clear();
            without.blue_curve.clear();
            assert!(engine_active(&without), "clearing {name} must not mute the exposure move");
        }
    }

    /// R25 P6, the render half: a mask whose ONLY move is a point curve must
    /// move the pixels it covers and leave every other pixel BIT-IDENTICAL.
    ///
    /// Both halves matter. Before this batch a curve-only mask fell through
    /// `tone_identity` exactly as a clarity-only mask fell through every gate
    /// before R22 — it rendered nothing while the sidecar carried it — and the
    /// bit-identical half is what proves the local curve is weighted by the
    /// mask instead of being applied to the frame.
    #[test]
    fn a_local_point_curve_darkens_only_inside_the_mask() {
        use crate::recipe::CurvePoint;
        let (w, h) = (16usize, 4usize);
        let base: Vec<[f32; 3]> = (0..w * h).map(|_| [0.60, 0.50, 0.40]).collect();
        // Full effect at x=0, zero from x=w/2 on (the ramp's own convention).
        let left_half = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 };
        let run = |m: LocalAdjustment| -> Vec<[f32; 3]> {
            let mut out = base.clone();
            apply_masks(
                &mut out,
                w,
                h,
                &EditRecipe { masks: vec![m], ..Default::default() },
                &MaskRasterSnapshot::default(),
                MaskFrame::AsRendered,
            );
            out
        };
        // A pull-down master curve: midtones map lower, ends pinned.
        let darken = vec![
            CurvePoint { input: 0, output: 0 },
            CurvePoint { input: 128, output: 64 },
            CurvePoint { input: 255, output: 255 },
        ];
        let out = run(LocalAdjustment {
            mask: left_half.clone(),
            amount: 1.0,
            main_curve: darken,
            ..Default::default()
        });
        assert!(
            out[0][0] < base[0][0] - 0.05,
            "the fully covered pixel must darken: {:?} → {:?}",
            base[0],
            out[0]
        );
        for x in w / 2..w {
            assert_eq!(out[x], base[x], "uncovered column {x} moved on a curve-only mask");
        }

        // The per-channel arm: a RED lift touches red and nothing else, so the
        // three channel curves are wired to three different fields (a copied
        // index would show up here as green or blue moving too).
        let out = run(LocalAdjustment {
            mask: left_half,
            amount: 1.0,
            red_curve: vec![
                CurvePoint { input: 0, output: 0 },
                CurvePoint { input: 128, output: 192 },
                CurvePoint { input: 255, output: 255 },
            ],
            ..Default::default()
        });
        assert!(out[0][0] > base[0][0] + 0.05, "red must lift: {:?} → {:?}", base[0], out[0]);
        // Green and blue keep their value to within LUT-sampling rounding.
        // Not `==`: the fused pass runs the identity master curve through
        // `sample_lut` + `scale_chroma` for every covered pixel, which is a
        // ~1e-7 round trip on ANY active mask (it predates this batch). The
        // claim being made is "the red curve touched one channel", and 1e-5 is
        // four orders below the 0.05 swing above.
        for (ch, name) in [(1usize, "green"), (2, "blue")] {
            assert!(
                (out[0][ch] - base[0][ch]).abs() < 1e-5,
                "a red curve must leave {name} untouched: {:?} → {:?}",
                base[0],
                out[0]
            );
        }
        for x in w / 2..w {
            assert_eq!(out[x], base[x], "uncovered column {x} moved on a red-curve-only mask");
        }
    }

    /// R23-1b: the two controls the XMP writer emitted as a literal `"0"` from
    /// the first sidecar on. Both must actually MOVE PIXELS, inside the mask
    /// only — a slider that exports but does not render is the #15a/#10B defect
    /// R22 fixed for clarity/dehaze/texture, reintroduced.
    #[test]
    fn local_hue_rotates_and_local_sharpness_signs_both_ways_inside_the_mask() {
        // A saturated red left half / right half, so a hue rotation is
        // measurable as a channel swing and the mask edge is unambiguous.
        let (w, h) = (16usize, 4usize);
        let base: Vec<[f32; 3]> = (0..w * h).map(|_| [0.80, 0.25, 0.20]).collect();
        // Covers the LEFT half only (the linear ramp reaches full at x=0).
        let left_half = MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 };
        let run = |m: LocalAdjustment| -> Vec<[f32; 3]> {
            let mut out = base.clone();
            apply_masks(
                &mut out,
                w,
                h,
                &EditRecipe { masks: vec![m], ..Default::default() },
                &MaskRasterSnapshot::default(),
                MaskFrame::AsRendered,
            );
            out
        };

        // HUE: +100 is +30°, so red → orange (green rises, blue barely moves).
        let hue = run(LocalAdjustment {
            mask: left_half.clone(),
            amount: 1.0,
            hue: 100.0,
            ..Default::default()
        });
        assert!(
            hue[0][1] > base[0][1] + 0.05,
            "a +30° rotation must swing red toward orange: {:?} → {:?}",
            base[0],
            hue[0]
        );
        let right = w - 1;
        assert_eq!(hue[right], base[right], "the uncovered half must not rotate");
        // …and the rotation is a rotation: -100 goes the other way (toward
        // magenta — blue rises), not "less of the same".
        let back = run(LocalAdjustment {
            mask: left_half.clone(),
            amount: 1.0,
            hue: -100.0,
            ..Default::default()
        });
        assert!(back[0][2] > base[0][2] + 0.02, "-30° must swing the other way: {:?}", back[0]);

        // SHARPNESS: signed. A flat patch has no detail to sharpen, so measure
        // on an edge — the frame's own left column against its neighbour after
        // a step is introduced.
        let (mut edged, ew, eh) = detail_frame();
        let flat = edged.clone();
        apply_masks(
            &mut edged,
            ew,
            eh,
            &EditRecipe {
                masks: vec![LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 },
                    amount: 1.0,
                    sharpness: 100.0,
                    ..Default::default()
                }],
                ..Default::default()
            },
            &MaskRasterSnapshot::default(),
            MaskFrame::AsRendered,
        );
        let energy = |d: &[[f32; 3]], lo: usize, hi: usize| -> f32 {
            let mut e = 0.0;
            for y in 0..eh {
                for x in lo..hi.saturating_sub(1) {
                    e += (luma601(&d[y * ew + x + 1]) - luma601(&d[y * ew + x])).abs();
                }
            }
            e
        };
        assert!(
            energy(&edged, 0, ew / 4) > energy(&flat, 0, ew / 4) * 1.01,
            "positive local sharpness must RAISE edge energy inside the mask"
        );
        assert!(
            (energy(&edged, 3 * ew / 4, ew) - energy(&flat, 3 * ew / 4, ew)).abs() < 1e-4,
            "…and leave the uncovered side alone"
        );
        // The negative half is the point of the signed band: it SOFTENS.
        let mut softened = flat.clone();
        apply_masks(
            &mut softened,
            ew,
            eh,
            &EditRecipe {
                masks: vec![LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.0, full_x: 0.0, full_y: 0.0 },
                    amount: 1.0,
                    sharpness: -100.0,
                    ..Default::default()
                }],
                ..Default::default()
            },
            &MaskRasterSnapshot::default(),
            MaskFrame::AsRendered,
        );
        assert!(
            energy(&softened, 0, ew / 4) < energy(&flat, 0, ew / 4) * 0.99,
            "negative local sharpness must LOWER edge energy (this is the blur half)"
        );
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
            apply_develop_anon(&mut data, 1, 1, &r);
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
            {
                // Own directory, not the bare temp root (see `fixture_mask_path`).
                let d = std::env::temp_dir()
                    .join(format!("autoshade-preview-perf-mask-{}", std::process::id()));
                std::fs::create_dir_all(&d).unwrap();
                d.join("mask.png")
            };
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
            "autoshade-render-clamp-{}-{}",
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
        render_to_file(&src, &wild, &wild_out, None, None, crate::diag::stderr()).unwrap();
        render_to_file(&src, &clamped, &clamped_out, None, None, crate::diag::stderr()).unwrap();

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
            "autoshade-refine-budget-{}-{}",
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
            "autoshade-raster-budget-{}-{}",
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
        let Err(error) = load_mask_raster_snapshot_with_budget(&recipe, 1, true, &crate::diag::pixels()) else {
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
            "autoshade-raster-snapshot-{}-{}",
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
        let snapshot = load_mask_raster_snapshot(&recipe, &crate::diag::pixels()).unwrap();
        let untouched = vec![[0.25, 0.25, 0.25]; 4];
        let mut before_delete = untouched.clone();
        apply_develop_with_rasters(&mut before_delete, 2, 2, &recipe, &snapshot, MaskFrame::AsRendered);
        std::fs::remove_file(&mask).unwrap();
        let mut after_delete = untouched.clone();
        apply_develop_with_rasters(&mut after_delete, 2, 2, &recipe, &snapshot, MaskFrame::AsRendered);
        assert_eq!(after_delete, before_delete);
        assert_ne!(after_delete, untouched, "the retained white mask must still apply");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_staged_encode_leaves_the_existing_target_intact() {
        let dir = std::env::temp_dir().join(format!(
            "autoshade-staged-failure-{}-{}",
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

    /// R25 B3: the manual CA pair really moves the red channel's sampling
    /// radius — and moves NOTHING else when it asks for a shrink.
    ///
    /// A NEGATIVE ca_r is the sharp case: every channel factor lands at or
    /// below 1, so [`geometry_fill_scale`] is exactly 1.0 and green and blue
    /// come out byte for byte identical to the CA-off render, leaving red as
    /// the only thing that moved. (The positive direction cannot make that
    /// claim and must not pretend to: an overshooting channel zooms the whole
    /// frame through the composite fill, which is the L04-2 contract, and it
    /// is asserted below rather than dodged.)
    #[test]
    fn manual_ca_shifts_the_red_channel_radius() {
        // Concentric rings: a pattern that is a function of RADIUS, so a
        // radial rescale of one channel is visible and a translation-only bug
        // could not fake it. All three channels identical at the source.
        let target = DynamicImage::ImageRgb16(ImageBuffer::from_fn(161, 161, |x, y| {
            let (dx, dy) = (x as f32 - 80.0, y as f32 - 80.0);
            let r = (dx * dx + dy * dy).sqrt();
            let v = (((r * 0.7).sin() * 0.5 + 0.5) * 60000.0) as u16;
            Rgb([v; 3])
        }));
        let off = EditRecipe::default();
        let shrink = EditRecipe { ca_r: -100.0, ..Default::default() };
        let grow = EditRecipe { ca_r: 100.0, ..Default::default() };

        // The composed profile is what carries it — a photo with no in-camera
        // CA data of its own still gets one.
        assert!(!geometry_profile(&off).geometry_active(), "premise: a rest recipe adds nothing");
        assert!(geometry_profile(&shrink).geometry_active(), "the manual pair alone is geometry");

        let base = apply_lens_geometry(&target, &geometry_profile(&off), 0.0).to_rgb16();
        let out = apply_lens_geometry(&target, &geometry_profile(&shrink), 0.0).to_rgb16();
        assert!(
            out.pixels().zip(base.pixels()).all(|(p, q)| p.0[1] == q.0[1] && p.0[2] == q.0[2]),
            "a shrinking manual CA must leave green and blue byte-identical"
        );
        assert!(
            out.pixels().zip(base.pixels()).any(|(p, q)| p.0[0] != q.0[0]),
            "…and it must actually move red"
        );
        // The DIRECTION, at one named pixel: ca_r < 0 means red samples at a
        // SMALLER radius, so the far-off-centre red is the source from nearer
        // the middle. SUB-PIXEL by construction — ±100 is 0.2 % of the radius
        // and no test frame makes that a whole pixel — so the expectation is
        // the source's own bilinear value at the shrunk coordinate, which on
        // the centre row (dy = 0) is a plain lerp along x.
        let src = target.to_rgb16();
        let f = 1.0 + (-100.0) * MANUAL_CA_PER_UNIT;
        let (x, y) = (158u32, 80u32);
        let sx = 80.0 + ((x as f32) - 80.0) * f;
        let (i0, frac) = (sx.floor() as u32, sx - sx.floor());
        let want = src.get_pixel(i0, y).0[0] as f32 * (1.0 - frac)
            + src.get_pixel(i0 + 1, y).0[0] as f32 * frac;
        let got = out.get_pixel(x, y).0[0] as f32;
        assert!(
            (got - want).abs() < 64.0,
            "red at x={x} should be the source at the shrunk radius {sx}: got {got}, want {want}"
        );
        // …and that really is a MOVE, not a rounding: the unshifted source
        // pixel is far away on this ring pattern.
        let unshifted = src.get_pixel(x, y).0[0] as f32;
        assert!(
            (got - unshifted).abs() > 1000.0,
            "the probe pixel must sit on a steep part of the ring pattern"
        );

        // The other direction is the documented frame zoom, not a silent one:
        // an overshooting channel makes `geometry_moves_frame` true, which is
        // what keeps every coordinate map in step (C2).
        assert!(
            geometry_moves_frame(&geometry_profile(&grow), 0.0),
            "a magnifying manual CA zooms the frame through the composite fill"
        );
        assert!(
            !geometry_moves_frame(&geometry_profile(&shrink), 0.0),
            "…and a shrinking one does not move the shared frame at all"
        );
    }

    /// R25 B3: the pair at rest costs NOTHING — no allocation, no engine
    /// change, and a render byte for byte what v0.30 produced.
    #[test]
    fn ca_zero_is_bit_identical_to_no_ca() {
        use crate::recipe::LensProfile;
        let profile = LensProfile {
            distortion: (0..16).map(|i| 1.0008 - 0.02 * (i as f32 / 15.0).powi(2)).collect(),
            distortion_on: true,
            ca_r: vec![0.999; 16],
            ca_b: vec![1.001; 16],
            ca_on: true,
            ..Default::default()
        };
        let r = EditRecipe { lens_profile: profile.clone(), ..Default::default() };
        assert!(
            matches!(geometry_profile(&r), std::borrow::Cow::Borrowed(_)),
            "a recipe with ca_r = ca_b = 0 must borrow the in-camera profile unchanged"
        );
        assert_eq!(*geometry_profile(&r), profile, "…and it is that profile, not a copy of one");
        let ramp = DynamicImage::ImageRgb16(ImageBuffer::from_fn(160, 90, |x, y| {
            Rgb([(x as u16) * 300, (x as u16) * 300 + (y as u16), (y as u16) * 500])
        }));
        let a = apply_lens_geometry(&ramp, &geometry_profile(&r), 0.0).to_rgb16();
        let b = apply_lens_geometry(&ramp, &profile, 0.0).to_rgb16();
        assert!(a.pixels().zip(b.pixels()).all(|(p, q)| p.0 == q.0), "the render must not move");
        // A profile whose CA the user switched OFF stays off: the manual pair
        // stands alone rather than scaling knots nobody asked for.
        let off = EditRecipe {
            lens_profile: LensProfile { ca_on: false, ..profile },
            ca_r: -50.0,
            ..Default::default()
        };
        let composed = geometry_profile(&off);
        assert_eq!(composed.ca_r.len(), 1, "the disabled profile knots must not participate");
        assert!((composed.ca_r[0] - (1.0 + -50.0 * MANUAL_CA_PER_UNIT)).abs() < 1e-9);
        assert_eq!(composed.ca_b, vec![1.0], "the untouched axis is exactly neutral");
    }

    /// R25 B3: every CARRIED control renders NOTHING — the whole claim
    /// `Tier::CarriedOnly` makes, re-derived from the registry so a row that
    /// changes tier without gaining an engine stage fails here.
    #[test]
    fn carried_detail_renders_nothing() {
        use crate::advisor::catalogue::{global_value, Shape, Tier, RECIPE_CONTROLS};
        let img = DynamicImage::ImageRgb16(ImageBuffer::from_fn(64, 48, |x, y| {
            Rgb([(x as u16) * 900, (y as u16) * 1200, ((x + y) as u16) * 500])
        }));
        let neutral_recipe = EditRecipe::default();
        let neutral = develop_preview(&img, &neutral_recipe).to_rgb16();
        let mut probed = 0usize;
        for c in RECIPE_CONTROLS.iter().filter(|c| c.tier == Some(Tier::CarriedOnly)) {
            // Through serde, so a renamed field cannot slip past — and BY
            // SHAPE, because B3 put a flag on this tier.
            let mut json = serde_json::to_value(&neutral_recipe).expect("recipe serialises");
            json[c.name] = match c.shape {
                Shape::Bool => serde_json::json!(true),
                _ => serde_json::json!(c.range.map(|(_, hi)| hi).unwrap_or(1.0)),
            };
            let mut r: EditRecipe = serde_json::from_value(json)
                .unwrap_or_else(|e| panic!("{}: probe value rejected: {e}", c.name));
            r.clamp();
            assert_ne!(
                global_value(&r, c.name),
                global_value(&neutral_recipe, c.name),
                "{}: the probe must actually move the control",
                c.name
            );
            let out = develop_preview(&img, &r).to_rgb16();
            assert!(
                out.pixels().zip(neutral.pixels()).all(|(p, q)| p.0 == q.0),
                "{}: a CarriedOnly control moved a pixel — it is not carried, it renders",
                c.name
            );
            // The GEOMETRIC stage runs after `develop_preview`, so inertness
            // there needs its own word: a carried control must not reach the
            // composed lens profile either (the manual CA pair is the only
            // thing on that path, and it is `Rendered`).
            assert_eq!(
                *geometry_profile(&r),
                *geometry_profile(&neutral_recipe),
                "{}: a CarriedOnly control changed the composed lens geometry",
                c.name
            );
            probed += 1;
        }
        assert!(probed >= 24, "premise: B2's nine plus B3's fifteen, got {probed}");
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
            "autoshade-export-depth-{}",
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
            render_to_file(&src, &recipe, &out, None, Some(&opts), crate::diag::stderr()).unwrap();
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

    /// An AI mask with no recomputed alpha must SKIP its adjustment, not
    /// render it at weight 0.
    ///
    /// The distinction is not academic — it is the difference between "this
    /// mask does nothing" and "this mask does everything". `LocalAdjustment`
    /// composes coverage with `inverted` as `1 - w`, so a zero-coverage mask
    /// under `inverted: true` applies the edit to the ENTIRE frame. That is
    /// exactly the silent-zero failure the AI arm is not allowed to have, and
    /// `is_raster_backed` is what separates "needs pixels and has none" from
    /// "needs no pixels".
    ///
    /// MUTATION-LINED. Verified red by reverting the two `is_raster_backed`
    /// call sites to `matches!(…, MaskGeometry::Bitmap { .. })` (transcript in
    /// the batch report): the inverted arm's assertion fails because the
    /// unresolved AI mask brightens the whole frame.
    #[test]
    fn an_unresolved_ai_mask_skips_its_adjustment_instead_of_covering_the_frame() {
        let ai = |inverted: bool| crate::recipe::LocalAdjustment {
            name: "Sky".into(),
            inverted,
            exposure_ev: 3.0,
            mask: MaskGeometry::AiMask {
                name: "Sky 1".into(),
                subtype: 2,
                ref_x: 0.5,
                ref_y: 0.3,
                blend_mode: 0,
                value: 1.0,
                inverted: false,
                mask_version: 1,
                provenance: Vec::new(),
                gesture: Vec::new(),
                // NOT resolved: the segmenter has not run, or declined.
                raster: None,
            },
            ..Default::default()
        };
        // The two questions the weight loop asks, answered directly — the same
        // pair `apply_masks` and `mask_coverage_preview` both consult.
        let g = &ai(false).mask;
        assert!(is_raster_backed(g), "an AI mask draws from a raster");
        assert!(geometry_raster_path(g).is_none(), "…and it has none yet");
        // With no bitmap, the weight is 0 everywhere — which is exactly why the
        // caller must skip rather than invert it.
        assert_eq!(mask_weight(g, 0.5, 0.3, None), 0.0);
        assert_eq!(mask_weight(g, 0.1, 0.9, None), 0.0);

        // The visible consequence, through the public coverage preview: an
        // unresolved AI mask advertises NO coverage in either polarity. Under
        // the old Bitmap-only test the inverted one would have advertised the
        // whole frame.
        let reference = DynamicImage::ImageRgb8(image::RgbImage::new(8, 8));
        for inverted in [false, true] {
            let cov = mask_coverage(&ai(inverted), &reference, MaskFrame::AsRendered);
            let lit = cov.pixels().filter(|p| p.0[0] > 0).count();
            assert_eq!(
                lit, 0,
                "an unresolved AI mask must advertise nothing (inverted={inverted})"
            );
        }

        // And once an alpha EXISTS the geometry samples it like any raster.
        let dir = std::env::temp_dir().join(format!("autoshade-ai-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("alpha.png");
        let mut img = image::GrayImage::new(4, 4);
        for px in img.pixels_mut() {
            px.0[0] = 255;
        }
        img.save(&p).unwrap();
        let mut resolved = ai(false);
        if let MaskGeometry::AiMask { raster, .. } = &mut resolved.mask {
            *raster = Some(p.to_string_lossy().into_owned());
        }
        assert_eq!(
            geometry_raster_path(&resolved.mask),
            Some(p.to_string_lossy().as_ref()),
            "the resolved alpha is the geometry's raster"
        );
        let bmp = load_mask_bitmap(&resolved.mask, &crate::diag::pixels()).expect("the alpha must load");
        assert_eq!(mask_weight(&resolved.mask, 0.5, 0.5, Some(&bmp)), 1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `coord_era` migration turns an AI mask's reference point and DROPS
    /// its cached alpha — the raster was segmented in the old frame, so
    /// rotating it is a re-render, not a coordinate migration.
    ///
    /// MUTATION: leave `*raster` alone in `orient_recipe_coords`' AiMask arm
    /// and the second assert fails — the mask would then render the OLD
    /// frame's selection over the turned pixels.
    #[test]
    fn turning_the_frame_moves_the_ai_click_and_invalidates_its_cached_alpha() {
        let mut r = EditRecipe {
            coord_era: 0, // the LEGACY era — what the migration acts on
            masks: vec![crate::recipe::LocalAdjustment {
                exposure_ev: 1.0,
                mask: MaskGeometry::AiMask {
                    name: "Sky 1".into(),
                    subtype: 2,
                    ref_x: 0.25,
                    ref_y: 0.10,
                    blend_mode: 0,
                    value: 1.0,
                    inverted: false,
                    mask_version: 1,
                    provenance: Vec::new(),
                    gesture: Vec::new(),
                    raster: Some("stale.png".into()),
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            recipe_has_frame_coords(&r),
            "an AI mask's reference point IS a frame coordinate the migration moves"
        );
        assert!(
            !recipe_has_raster_masks(&r),
            "and it is not 'unturnable' — nothing here fails to be turned"
        );
        let turned = orient_recipe_coords(&mut r, rawler::Orientation::Rotate90, probe_frame());
        assert!(turned, "the migration ran");
        let MaskGeometry::AiMask { ref_x, ref_y, raster, .. } = &r.masks[0].mask else {
            panic!("the geometry must survive the migration");
        };
        assert_ne!((*ref_x, *ref_y), (0.25, 0.10), "the click moved with the frame");
        assert!(raster.is_none(), "the alpha from the OLD frame must not be reused");
    }

    // ---- R28 Batch-1 1a: the CFA-geometry demosaic ------------------------

    /// The X-S10's 6×6 X-Trans tile, read back from the zoo RAF's OWN
    /// `XTransLayout` (0x0131) record rather than from rawler's camera DB —
    /// the RAF decoder prefers the file's copy (`decoders/raf.rs:257-263`),
    /// and for this body the two turned out identical (measured 2026-08-20,
    /// `AUTOSHADE_RAW_ZOO` probe, alongside `active_area = 6252×4176 @ (0,5)`
    /// and `crop_area = 6240×4160 @ (6,13)`).
    const XTRANS_XS10: &str = "GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG";

    /// 20 green / 8 red / 8 blue in four 2×2 all-green blocks plus four
    /// isolated greens — the structure every claim below rests on.
    #[test]
    fn the_x_trans_tile_is_four_green_blocks_and_four_isolated_greens() {
        use rawler::cfa::{CFA, CFA_COLOR_B, CFA_COLOR_G, CFA_COLOR_R};
        let cfa = CFA::new(XTRANS_XS10);
        assert_eq!((cfa.width, cfa.height), (6, 6));
        let count = |ch: usize| (0..36).filter(|i| cfa.color_at(i / 6, i % 6) == ch).count();
        assert_eq!((count(CFA_COLOR_R), count(CFA_COLOR_G), count(CFA_COLOR_B)), (8, 20, 8));
        // A green whose right AND down neighbours are both green is the
        // top-left corner of a 2×2 block; there are four of them.
        let blocks = (0..36)
            .filter(|i| {
                let (r, c) = (i / 6, i % 6);
                cfa.color_at(r, c) == CFA_COLOR_G
                    && cfa.color_at(r, c + 1) == CFA_COLOR_G
                    && cfa.color_at(r + 1, c) == CFA_COLOR_G
            })
            .count();
        assert_eq!(blocks, 4, "X-Trans is defined by its four 2×2 green blocks per tile");
    }

    /// WHY this file has its own demosaic, pinned from the pattern alone.
    ///
    /// rawler's `interpolate_rb_at_green` (`imgop/sensor/bayer/ppg.rs:185-203`)
    /// fills a green photosite's two missing channels from exactly
    /// `(row, col+1)` and `(row+1, col)`, on the Bayer axiom that those two
    /// carry the two DIFFERENT chroma colours. Every time a neighbour is green
    /// instead, the write lands back on the green channel and that chroma
    /// value is never written by any pass — `interpolate_rb_at_non_green`
    /// (`ppg.rs:220-252`) is gated on `color_at != G` and never revisits a
    /// green site.
    ///
    /// The four 2×2 blocks contribute 2 + 1 + 1 failures each (top-left corner
    /// both ways, top-right down, bottom-left right), so **16 chroma values
    /// per 36-pixel tile are lost**, split 8 R / 8 B by the tile's R↔B duality.
    /// Confirmed on the real X-S10 RAF before the fix: binning the 6252×4176
    /// camera-native demosaic by `(row mod 6, col mod 6)` gave R = 0.0 at
    /// 99.8 % of the pixels in 8 of the 36 phases and B = 0.0 in a different 8,
    /// green in none — the residual 0.2 % being the 3-px ring that upstream's
    /// CFA-correct border pass (`ppg.rs:74-110`) does reach.
    ///
    /// Kept as a PATTERN assertion rather than a call into `PPGDemosaic`
    /// deliberately: on a 6×6 CFA those passes read the buffer they are
    /// concurrently writing through `Color2DPtr` (`pixarray.rs:500-529`), so
    /// running them here would put a genuine data race in the suite to assert
    /// on its output.
    ///
    /// MUTATION THIS CATCHES: a future rawler that fixes X-Trans does not make
    /// this red — it makes [`demosaic_over_cfa_geometry`] redundant, which is
    /// a decision, not a regression. What it pins is the arithmetic behind the
    /// 16, so nobody re-derives it from memory.
    #[test]
    fn the_bayer_chroma_axiom_fails_sixteen_times_per_x_trans_tile() {
        use rawler::cfa::{CFA, CFA_COLOR_G};
        let cfa = CFA::new(XTRANS_XS10);
        let lost: usize = (0..36)
            .map(|i| {
                let (r, c) = (i / 6, i % 6);
                if cfa.color_at(r, c) != CFA_COLOR_G {
                    return 0;
                }
                usize::from(cfa.color_at(r, c + 1) == CFA_COLOR_G)
                    + usize::from(cfa.color_at(r + 1, c) == CFA_COLOR_G)
            })
            .sum();
        assert_eq!(lost, 16, "the Bayer chroma axiom must fail 16 times per X-Trans tile");
    }

    /// The fix, on the case the defect destroyed: a flat colour must survive
    /// EVERY CFA phase exactly. On the pre-fix path 16 of every 36 pixels came
    /// out with a channel at 0.0 or half its value.
    ///
    /// The ROI deliberately starts at neither the frame origin nor a tile
    /// boundary — the X-S10's active area starts at `y = 5` (measured) — so a
    /// demosaic that forgot to shift the pattern by the ROI origin writes the
    /// measured photosite into the wrong channel and this goes red at the
    /// first pixel.
    ///
    /// MUTATION THIS CATCHES: drop `cfa.shift(roi.x(), roi.y())`; swap the R
    /// and B tap sets; index the source plane from the frame origin instead of
    /// the ROI's; mirror instead of fold at the border (the outer ring then
    /// samples the wrong colour).
    #[test]
    fn the_cfa_geometry_demosaic_is_exact_on_flat_colour_at_every_phase() {
        use rawler::cfa::CFA;
        use rawler::imgop::{Dim2, Point, Rect};
        let cfa = CFA::new(XTRANS_XS10);
        let truth = [0.2f32, 0.5, 0.3];
        let (pw, ph) = (72usize, 72usize);
        let roi = Rect::new(Point::new(3, 5), Dim2::new(60, 60));
        let plane: Vec<f32> =
            (0..pw * ph).map(|i| truth[cfa.color_at(i / pw, i % pw)]).collect();
        let out = demosaic_over_cfa_geometry(&plane, Dim2::new(pw, ph), &cfa, roi);
        assert_eq!(out.len(), roi.width() * roi.height());
        let mut sums = [[0.0f64; 3]; 36];
        for (i, px) in out.iter().enumerate() {
            let (row, col) = (i / roi.width(), i % roi.width());
            for ch in 0..3 {
                assert!(
                    (px[ch] - truth[ch]).abs() < 1e-5,
                    "phase ({}, {}) channel {ch} came out {} instead of {}",
                    row % 6,
                    col % 6,
                    px[ch],
                    truth[ch]
                );
                sums[(row % 6) * 6 + col % 6][ch] += px[ch] as f64;
            }
        }
        // The whole-frame statement of the same thing, and the shape the
        // defect was originally measured in: 36 phase means per channel, which
        // must not spread at all on a flat field.
        let n = (out.len() / 36) as f64;
        for ch in 0..3 {
            let means: Vec<f64> = sums.iter().map(|s| s[ch] / n).collect();
            let spread = means.iter().cloned().fold(f64::MIN, f64::max)
                - means.iter().cloned().fold(f64::MAX, f64::min);
            assert!(spread < 1e-5, "channel {ch} spreads {spread} across the 36 CFA phases");
        }
    }

    /// …and on a LINEAR gradient, which is what makes the taps a plane fit
    /// rather than a distance-weighted mean. Measured on this same synthetic
    /// (60×60, interior): a mean's per-phase R/G ratio spreads by 7.2e-3 — a
    /// 1.8 % chroma modulation at the tile period, i.e. visible fixed-pattern
    /// chroma on a sky — where the plane fit spreads by 3.3e-16.
    ///
    /// Interior only: outside `CfaTaps::radius` of the ROI edge the window
    /// folds back into the frame by whole CFA repeats, which is colour-correct
    /// but not linear.
    #[test]
    fn the_cfa_geometry_demosaic_is_exact_on_a_linear_gradient() {
        use rawler::cfa::CFA;
        use rawler::imgop::{Dim2, Point, Rect};
        let cfa = CFA::new(XTRANS_XS10);
        let (pw, ph) = (72usize, 72usize);
        let roi = Rect::new(Point::new(3, 5), Dim2::new(60, 60));
        let truth = |row: usize, col: usize| {
            let t = 0.1 + 0.6 * col as f32 / 71.0 + 0.2 * row as f32 / 71.0;
            [0.4 * t, t, 0.6 * t]
        };
        let plane: Vec<f32> = (0..pw * ph)
            .map(|i| truth(i / pw, i % pw)[cfa.color_at(i / pw, i % pw)])
            .collect();
        let out = demosaic_over_cfa_geometry(&plane, Dim2::new(pw, ph), &cfa, roi);
        let guard = cfa.width.max(cfa.height);
        for (i, px) in out.iter().enumerate() {
            let (row, col) = (i / roi.width(), i % roi.width());
            if row < guard || col < guard || row + guard >= roi.height() || col + guard >= roi.width()
            {
                continue;
            }
            let want = truth(roi.y() + row, roi.x() + col);
            for ch in 0..3 {
                assert!(
                    (px[ch] - want[ch]).abs() < 2e-5,
                    "({row}, {col}) channel {ch}: {} vs {}",
                    px[ch],
                    want[ch]
                );
            }
        }
    }

    /// Every tap set is a normalised, NON-NEGATIVE combination — the two
    /// properties the doc comment's quality claims rest on. Sum = 1 is what
    /// makes flat colour exact; non-negativity makes each estimate a convex
    /// combination of real samples of that colour, so it cannot leave their
    /// range and cannot ring. The second is a MEASURED property of this tile
    /// at radius 2 (worst tap +0.056 over all 108 sets), not a theorem about
    /// least squares — a different geometry may well need the guarantee
    /// dropped, and this is where that would be noticed.
    #[test]
    fn every_cfa_tap_set_is_a_normalised_convex_combination() {
        use rawler::cfa::CFA;
        let cfa = CFA::new(XTRANS_XS10);
        let taps = cfa_taps(&cfa);
        assert_eq!(taps.per_phase.len(), 6 * 6 * 3);
        assert!(taps.radius >= 6, "the fallback window must span a whole repeat");
        for (i, set) in taps.per_phase.iter().enumerate() {
            let (phase, ch) = (i / 3, i % 3);
            assert!(!set.is_empty(), "phase {phase} channel {ch} has no sample at all");
            let sum: f32 = set.iter().map(|t| t.2).sum();
            assert!(
                (sum - 1.0).abs() < 1e-5,
                "phase {phase} channel {ch} weights sum to {sum}"
            );
            let worst = set.iter().map(|t| t.2).fold(f32::MAX, f32::min);
            assert!(worst >= 0.0, "phase {phase} channel {ch} has a negative tap {worst}");
        }
    }

    /// Out-of-frame taps fold by whole CFA repeats, so an offset keeps its
    /// COLOUR. A mirror or a clamp puts a different colour under it and
    /// reintroduces the channel error the whole path exists to remove.
    #[test]
    fn out_of_frame_taps_fold_by_whole_cfa_repeats() {
        let t = wrap_table(60, 6, 6);
        assert_eq!(t.len(), 60 + 12);
        for (i, &src) in t.iter().enumerate() {
            let logical = i as isize - 6;
            assert!(src < 60, "index {logical} folded to {src}, outside the frame");
            assert_eq!(
                src % 6,
                logical.rem_euclid(6) as usize,
                "index {logical} folded to {src} and changed colour"
            );
        }
        // A frame that is not a whole number of repeats folds by the largest
        // whole span inside it (54 of 57 rows here), never by the frame size.
        let t = wrap_table(57, 6, 6);
        for (i, &src) in t.iter().enumerate() {
            let logical = i as isize - 6;
            assert!(src < 57);
            assert_eq!(src % 6, logical.rem_euclid(6) as usize, "{logical} -> {src}");
        }
    }

    /// The dispatch is GEOMETRIC. Every Bayer spelling stays on rawler's own
    /// develop (byte-identical: all eight non-Fuji zoo renders hashed the same
    /// before and after this change), a 4-colour array is not ours to touch,
    /// and only a non-2×2 RGB repeat takes the new path.
    #[test]
    fn only_a_non_two_by_two_rgb_cfa_takes_the_geometry_path() {
        use rawler::cfa::CFA;
        for bayer in ["RGGB", "BGGR", "GRBG", "GBRG"] {
            assert!(
                !cfa_needs_geometry_demosaic(&CFA::new(bayer)),
                "{bayer} is a 2×2 quincunx and must keep rawler's PPG"
            );
        }
        assert!(!cfa_needs_geometry_demosaic(&CFA::new("RGBE")), "4-colour is refused earlier");
        assert!(cfa_needs_geometry_demosaic(&CFA::new(XTRANS_XS10)));
    }
}
