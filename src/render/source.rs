//! Source pixels: RAW decode over the sensor's CFA geometry, the baked-image door, and the top-level develop-to-image entry.

use super::*;

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
        Some(edge) if img.width().max(img.height()) > edge => Ok(super::thumbnail(&img, edge, edge)),
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
    render_frame(raw_path, recipe, denoise, max_edge, working, sink, BasePart::of(recipe).colour_nr()).map(|(img, _)| img)
}

/// [`render_to_image_in`]'s body. `colour_nr` is the strength of the RAW's
/// colour noise reduction, which runs on the linear mosaic inside the demosaic
/// ([`crate::denoise::linear_chroma`]); 0 is off. Beside the frame it reports
/// whether that stage ran: `false` on a mosaic the stage cannot read, whose
/// colour noise the display-frame pass then takes, as for any source without
/// one.
#[allow(clippy::too_many_arguments)] // the develop's frame plus the one switch of its first half
pub(super) fn render_frame(
    raw_path: &Path,
    recipe: &EditRecipe,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    max_edge: Option<u32>,
    working: ExportColorSpace,
    sink: &dyn crate::diag::Sink,
    colour_nr: f32,
) -> Result<(DynamicImage, bool)> {
    let diag = crate::diag::Diag::about(sink, raw_path);
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose(&diag);
    let recipe = &*validated;
    let rasters = load_mask_raster_snapshot(recipe, &diag)?;
    let (mut rawimage, orientation, daylight) = decode_sensor(raw_path, recipe.quarter_turns, denoise)?;

    let wide = working != ExportColorSpace::Srgb;
    // v1.5.0 F7 — the camera profile the sidecar names, resolved against the
    // Adobe profiles installed on THIS machine. `None` is the ordinary case
    // (no profile named, or none installed that matches) and costs nothing.
    // The profile is colour data: its tables, never tone.
    let profile_stage = resolve_camera_profile(&CameraMeta::of(&rawimage, daylight), recipe, working, &diag);
    // A profile is rendered by the engine's OWN calibrate path whatever the
    // export space asked for. The sRGB path normally leaves white balance and
    // calibration to rawler and stays byte-identical to it; that byte identity
    // is worth keeping when nothing else changes, but a profile changes the
    // colour by design, and an sRGB export that ignored the profile while the
    // ProPhoto one obeyed it would be two renderings of one photograph.
    let own_calibrate = wide || profile_stage.is_some();
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
    } else if own_calibrate {
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
    let calibration = if own_calibrate {
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
    let strength = denoise.map_or(1.0, |opts| opts.strength);
    // Strong isolated hot sites are mapped on EVERY Bayer develop (user ruling
    // 2026-09-21; until then only a positive-strength AI denoise did it), before
    // anything reads the mosaic: the grain source, the cleaner and the develop
    // itself. The helper leaves any sensor without a 2×2 integer mosaic alone.
    let hot_sites = crate::denoise::hot_pixels::map_for(&mut rawimage)?;
    if hot_sites > 0 {
        println!("mapped {hot_sites} isolated hot pixels stronger than 20 local sigma");
    }
    // The exposure's noise reading for the colour stage (its model and its
    // `k`): a property of the exposure, read ONCE per source off the sensor's
    // own mosaic — here, before the AI denoise below replaces that mosaic
    // with one whose noise is no longer the sensor's — and kept with the
    // source frame, so a second develop runs the stage's pass alone.
    let reading = (colour_nr > 0.0 && crate::denoise::linear_chroma::reads(&rawimage))
        .then(|| SourceFrame::noise_reading(raw_path, || read_sensor(&rawimage)))
        .flatten();
    let grain_weights = denoise_grain::weights(working);
    let original_y = denoise_grain::capture_original(
        strength,
        crate::denoise::mosaic_args_for(&rawimage).is_ok(),
        grain_weights,
        || develop_raw_buffer(&rawimage, &dev, geometry_cfa.as_ref(), (colour_nr, reading.as_ref(), max_edge), calibration, working, profile_stage.as_ref())
            .map(|(rgb, _, _)| rgb),
    )?;
    // --- AI denoise on the MOSAIC (opt-in), BEFORE demosaic (2026-09-15).
    // The noise on the sensor mosaic is white per CFA plane and follows
    // var = a·x + b, which a non-blind model removes with the texture left in
    // place; after demosaic it is spatially correlated and no sRGB-domain
    // model separated it from texture (the 2026-09-15 probe: SCUNet 1.0's
    // detail blocks scored BELOW the noisy input on a ground-truth window).
    // The cleaner always returns its whole mosaic. Luminance grain returns
    // after the same develop and calibration as the original. A sensor without a 2×2
    // Bayer mosaic (X-Trans, four-colour, linear DNG) is disclosed and takes
    // the older developed-frame path below.
    let mut mosaic_denoised = false;
    if let Some(opts) = denoise {
        println!("AI denoise (RAW mosaic, DRUNet) on {}x{} ...", rawimage.width, rawimage.height);
        match crate::denoise::denoise_mosaic(opts, &mut rawimage).context("AI denoise")? {
            crate::denoise::MosaicDenoise::Denoised => mosaic_denoised = true,
            crate::denoise::MosaicDenoise::NotApplicable(why) => diag.warn(format!(
                "AI denoise: {why} — denoising the developed frame instead (the SCUNet path)"
            )),
        }
    }

    // The develop in its two halves with the mosaic dropped between them
    // (`develop_raw_buffer` is the same two halves back to back).
    let (data, w, h, linear_colour) = develop_raw_linear(&rawimage, &dev, geometry_cfa.as_ref(), (colour_nr, reading.as_ref(), max_edge))?;
    // The clean RGB owns the remaining work. Drop the mosaic before returning
    // grain; the original RGB already died when capture_original kept only Y.
    drop(rawimage);
    let mut data =
        settle_camera_buffer(data, calibration, geometry_cfa.is_some(), working, profile_stage.as_ref());
    if let Some(original_y) = original_y {
        denoise_grain::return_luminance(&mut data, &original_y, 1.0 - strength, grain_weights);
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
    // The develop's own full-resolution short edge, read BEFORE the cap: the
    // Detail panel states its radii in pixels of THIS frame (`FilmScale`).
    let film_short = u32::try_from(w.min(h)).ok();
    // Working-resolution cap: downscale-then-develop, the same order the GUI
    // preview path uses — masks/sharpen/geometry are resolution-normalised.
    let (mut data, w, h) = match max_edge {
        Some(edge) => downscale_f32(data, w, h, edge),
        None => (data, w, h),
    };
    let film = FilmScale::of(film_short, w, h);

    // --- AI denoise on the demosaiced pixels: the FALLBACK for a sensor the
    // mosaic path could not take (see above), before tone/sharpen.
    if let Some(opts) = denoise.filter(|_| !mosaic_denoised) {
        println!("AI denoise ({}) on {}x{} ...", opts.model, w, h);
        crate::denoise::denoise_buffer(opts, &mut data, w, h).context("AI denoise")?;
    }

    Ok((finish_develop(data, w, h, film, recipe, &rasters, linear_colour)?, linear_colour))
}

/// Open `raw_path` and read its sensor: the guards, the metadata, the per-file
/// memory ceiling, the full decode and the default crop — everything up to
/// the develop — for [`render_frame`]. Returns the decoded image, its display
/// orientation composed with the photographer's `quarter_turns`, and (for a
/// Sony body) the file's Daylight preset.
///
/// The decode scope: the RawSource holds the entire RAW file in memory
/// (~60–120 MB for a 61 MP lossless ARW), and neither it nor the decoder
/// outlives the sensor read — so the file bytes drop when this returns
/// instead of sitting under the whole ~720 MB-per-plane develop chain (A7
/// buffer-lifetime queue).
pub(super) fn decode_sensor(
    raw_path: &Path,
    quarter_turns: u8,
    denoise: Option<&crate::denoise::DenoiseOpts>,
) -> Result<(rawler::RawImage, Orientation, Option<[f32; 3]>)> {
    crate::decode::guard_tiff_chain(raw_path)?;
    crate::decode::guard_raw_plane_extent(raw_path)?;
    let src = RawSource::new(raw_path)
        .with_context(|| format!("open RAW {}", raw_path.display()))?;
    let decoder = crate::decode::decoder_for(raw_path, &src)?;
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
        compose_orientation(crate::decode::raw_orientation_of(&md), quarter_turns);
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
    if denoise.is_some_and(|o| o.strength > 0.0 && o.strength < 1.0)
        && crate::denoise::mosaic_args_for(&probe).is_ok()
    {
        crate::decode::refuse_raw_grain_return_over_ceiling_for(raw_path, &probe)?;
    }
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
    // A Sony file's Daylight preset, read while its bytes are mapped: with
    // the body's reference it is the calibration Camera Raw puts in front
    // of the profile's matrices (`as_shot::camera_calibration`).
    let daylight = if raw.clean_make.eq_ignore_ascii_case("sony") {
        super::as_shot::sony_daylight_preset(src.buf())
    } else {
        None
    };
    Ok((raw, orientation, daylight))
}

/// The develop after the camera stage, on a frame already oriented, capped and
/// calibrated (the sRGB transfer, `working`'s primaries): the recipe's white
/// balance, the shared tone/colour/detail/mask develop, the 16-bit pack and
/// the geometry and finishing tail. `linear_colour`: the develop's first half
/// ran the RAW's linear colour stage, so the display-frame pass must not.
pub(super) fn finish_develop(
    mut data: Vec<[f32; 3]>,
    w: usize,
    h: usize,
    film: FilmScale,
    recipe: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    linear_colour: bool,
) -> Result<DynamicImage> {
    // --- white balance (target Kelvin/tint) in linear light -------------------
    apply_recipe_wb(&mut data, recipe);

    // --- 「Remove Chromatic Aberration」 (v1.5.0): an INSTRUCTION, so
    // rendering it means running the solver it names and handing the answer to
    // the manual pair below. Here, because `geometry_profile` is what folds
    // that pair onto the camera's knots and it is read on the next line.
    let solved = lens::with_auto_lateral_ca(recipe, &data, w, h);
    let recipe = &*solved;

    // --- tone + clarity + sat/vibrance + NR + sharpen (shared pipeline) -------
    // ONE value decides both the mask chain's frame adaptation and whether the
    // geometry stage runs below (`MaskFrame`). RADIAL keeps its pointwise
    // Lightroom/engine composition. LINEAR uses only the engine inverse when
    // geometry follows, or transports its two handles once when none follows.
    let geom = geometry_profile(recipe);
    let frame = MaskFrame::of_base(&geom, recipe.lens_distortion, false);
    // A RAW negative: an absent Sharpening amount renders at Lightroom's own
    // RAW default (`EditRecipe::capture_sharpening`, v1.6.0).
    apply_develop_with_rasters(&mut data, w, h, recipe, rasters, frame, film, true, linear_colour);

    // --- pack to 16-bit (highest precision; JPEG downconverts at encode).
    // Orientation was applied BEFORE develop (see orient_f32 above), so the
    // buffer is already in the display frame — no tail rotation.
    let dynimg = {
        let mut buf: Vec<u16> = vec![0u16; w * h * 3];
        buf.par_chunks_mut(3).zip(data.par_iter()).for_each(|(o, px)| {
            o[0] = to_u16(px[0]);
            o[1] = to_u16(px[1]);
            o[2] = to_u16(px[2]);
        });
        let img: ImageBuffer<Rgb<u16>, _> = ImageBuffer::from_raw(w as u32, h as u32, buf)
            .ok_or_else(|| anyhow!("pixel buffer size mismatch"))?;
        DynamicImage::ImageRgb16(img)
    };

    // --- lens geometry → straighten → crop → the finishing pass, all four in
    // `frame_and_finish` (`render/finish.rs`) because they are ONE order and
    // four surfaces need it: the geometric chain is original → corrected →
    // view, the user crop is defined on the straightened frame (Lightroom's
    // CropAngle + crop rect), and the post-crop vignette and grain can only
    // act once that rectangle exists.
    //
    // `geometry_profile`, not `recipe.lens_profile`: the manual CA pair rides
    // the same per-channel knots (R25 B3), and reading the raw profile here
    // would skip it on a photo with no in-camera CA data. Hoisted above the
    // develop so the mask chain and this resample are ONE decision — the gate
    // inside is the one `frame.warps()` answers with.
    //
    // Orientation was applied BEFORE the develop, so no tail rotation.
    Ok(frame_and_finish(dynimg, recipe, &geom, film, CropPolicy::Cut))
}

/// Both original and clean RAW buffers go through this exact develop and
/// calibration path. Every returned buffer has the sRGB transfer, including
/// wide working primaries; orientation and recipe work happen afterwards.
/// `colour_nr`: the colour stage's strength and the exposure's reading
/// ([`develop_raw_linear`]).
///
/// The two halves, [`develop_raw_linear`] then [`settle_camera_buffer`], back
/// to back: `render_frame` calls them itself with the mosaic dropped between.
pub(super) fn develop_raw_buffer(
    rawimage: &rawler::RawImage,
    dev: &RawDevelop,
    geometry_cfa: Option<&rawler::cfa::CFA>,
    colour_nr: ColourNr<'_>,
    calibration: Option<([[f32; 3]; 3], [f32; 3])>,
    working: ExportColorSpace,
    profile_stage: Option<&profile::Stage>,
) -> Result<(Vec<[f32; 3]>, usize, usize)> {
    let (data, w, h, _) = develop_raw_linear(rawimage, dev, geometry_cfa, colour_nr)?;
    let data = settle_camera_buffer(data, calibration, geometry_cfa.is_some(), working, profile_stage);
    Ok((data, w, h))
}

/// The develop's first half: the demosaic (with its default crop), on
/// camera-native LINEAR light when the engine calibrates or demosaiced the
/// CFA itself, and on rawler's own sRGB develop otherwise. `colour_nr` is the
/// colour noise reduction's strength ([`render_frame`]) and the exposure's
/// reading the stage runs on ([`ColourNr`]); the last value reports whether
/// the linear colour stage ran (`develop_cfa_signed`).
pub(super) fn develop_raw_linear(
    rawimage: &rawler::RawImage,
    dev: &RawDevelop,
    geometry_cfa: Option<&rawler::cfa::CFA>,
    colour_nr: ColourNr<'_>,
) -> Result<(Vec<[f32; 3]>, usize, usize, bool)> {
    let (inter, linear_colour) = match develop_cfa_signed(rawimage, dev, colour_nr) {
        Some(developed) => developed,
        None => (dev.develop_intermediate(rawimage).map_err(|e| anyhow!("develop: {e}"))?, false),
    };
    let inter = match (geometry_cfa, inter) {
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
    Ok((rgb.data, w, h, linear_colour))
}

/// The develop's second half: the white balance, calibration and profile
/// stage ([`calibrate_camera_buffer`]) — or, with no calibration, the sRGB
/// transfer a CFA the engine demosaiced still needs (`own_cfa`).
pub(super) fn settle_camera_buffer(
    data: Vec<[f32; 3]>,
    calibration: Option<([[f32; 3]; 3], [f32; 3])>,
    own_cfa: bool,
    working: ExportColorSpace,
    profile_stage: Option<&profile::Stage>,
) -> Vec<[f32; 3]> {
    let mut data = data;
    if let Some((xyz2cam, wb)) = calibration {
        calibrate_camera_buffer(&mut data, &xyz2cam, wb, working, profile_stage);
    } else if own_cfa {
        // A camera with no colour matrix at all: rawler skips its `Calibrate`
        // step there but still applies `SRgb` (`imgop/develop.rs:199-233`).
        // This path stripped both, so the working encoding is applied here or
        // the frame would publish as linear light.
        data.par_iter_mut().for_each(|px| {
            *px = [linear_to_srgb(px[0]), linear_to_srgb(px[1]), linear_to_srgb(px[2])];
        });
    }
    data
}

/// The colour stage's strength (0 is off), the exposure's reading it runs
/// on (`None`: read off the mosaic the develop holds, which is then the
/// sensor's) and the long edge the develop is capped at (`None`: full size),
/// whose pixels the stage's pass reads its scale from
/// (`linear_chroma::preview_level`).
pub(super) type ColourNr<'a> = (f32, Option<&'a crate::denoise::linear_chroma::SourceReading>, Option<u32>);

/// The mosaic normalised and SIGNED: rawler's rescale `(v − black) / (white −
/// black)` per 2×2 site in its pairing of lines, no clamp
/// ([`develop_cfa_signed`]).
fn normalised_mosaic(raw: &rawler::RawImage) -> Vec<f32> {
    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();
    let max: [f32; 4] = std::array::from_fn(|i| white[i] - black[i]);
    let mut v = raw.data.as_f32().into_owned();
    let width = raw.width;
    v.par_chunks_exact_mut(width * 2).for_each(|lines| {
        let (line0, line1) = lines.split_at_mut(width);
        line0.chunks_exact_mut(2).zip(line1.chunks_exact_mut(2)).for_each(|(a, b)| {
            a[0] = (a[0] - black[0]) / max[0];
            a[1] = (a[1] - black[1]) / max[1];
            b[0] = (b[0] - black[2]) / max[2];
            b[1] = (b[1] - black[3]) / max[3];
        });
    });
    v
}

/// The camera white balance the colour stage reads its colour differences
/// by (unit gains when the file's are not finite and positive).
fn stage_wb(raw: &rawler::RawImage) -> [f64; 3] {
    if raw.wb_coeffs[..3].iter().all(|c| c.is_finite() && *c > 0.0) {
        [raw.wb_coeffs[0], raw.wb_coeffs[1], raw.wb_coeffs[2]].map(f64::from)
    } else {
        [1.0; 3]
    }
}

/// The exposure's noise reading for the colour stage, off `raw`'s own sensor
/// mosaic ([`crate::denoise::linear_chroma::read_source`]).
fn read_sensor(raw: &rawler::RawImage) -> Option<crate::denoise::linear_chroma::SourceReading> {
    let rawler::rawimage::RawPhotometricInterpretation::Cfa(config) = &raw.photometric else { return None };
    crate::denoise::linear_chroma::read_source(&normalised_mosaic(raw), raw.width, raw.height, &config.cfa, stage_wb(raw))
}

/// rawler's `develop_intermediate` for a CFA mosaic with the black subtracted
/// SIGNED (v1.17.0). rawler 0.7.2 clips every value below the black to 0
/// before the demosaic (`imgop/raw.rs`, `correct_blacklevel_cfa`), which turns
/// the zero-mean read noise of a dark frame into a positive bias: on the night
/// pair the sensor noise sd is about 40 DN on a signal of 10–50 DN, the
/// darkest band's colour read u/v = +1.606/−0.288 against Camera Raw's
/// +1.261/−0.405 (+1.486/−0.405 signed). Here the values stay signed through
/// the demosaic (PPG and [`demosaic_over_cfa_geometry`] are differences and
/// weighted means, no clamp) and the white balance; the first clamp is where a
/// stage needs non-negative input: rawler's calibrate rule
/// (`clip_euclidean_norm_avg`, after the white balance and the matrix) on the
/// plain sRGB develop, the profile stage's guard before its hue/sat tables
/// (`profile::Stage::render_ref`, after the camera matrix) on a profiled one,
/// and the final pack otherwise.
///
/// Everything else is rawler's, step for step and in its order: the rescale
/// `(v − black) / (white − black)` per 2×2 site in its pairing of lines (a
/// value above the black is bit-identical), PPG over the active area, the
/// calibrate (matrix choice, white balance, `normalize`/`pseudo_inverse`), the
/// default crop and the sRGB transfer, all from rawler's public pieces. It is
/// done here rather than by handing rawler a pre-scaled mosaic because rawler
/// clones its input: a float mosaic beside the clone cost 3.9 B per source
/// pixel of peak commit (1745 → 1979 MB on a 60 MP frame). `None` — rawler's own
/// develop — when the develop does not rescale, the frame is not a CFA
/// mosaic, or rawler would demosaic it as four colours.
///
/// The colour noise reduction of a RAW runs here, on the normalised signed
/// mosaic before the demosaic (`colour_nr` > 0, [`crate::denoise::linear_chroma`]):
/// the one place of the develop where the noise is the mosaic's own and no
/// per-pixel nonlinear stage has read it yet. The flag beside the frame
/// reports whether it ran (the mosaic is one the stage reads,
/// `linear_chroma::reads`).
fn develop_cfa_signed(raw: &rawler::RawImage, dev: &RawDevelop, (colour_nr, reading, max_edge): ColourNr<'_>) -> Option<(Intermediate, bool)> {
    use rawler::imgop::sensor::bayer::{ppg::PPGDemosaic, Demosaic};
    use rawler::pixarray::PixF32;
    use rawler::rawimage::RawPhotometricInterpretation as Photo;
    let Photo::Cfa(config) = &raw.photometric else { return None };
    let demosaic = dev.steps.contains(&ProcessingStep::Demosaic);
    if !dev.steps.contains(&ProcessingStep::Rescale) || raw.cpp != 1 || (demosaic && !config.cfa.is_rgb()) {
        return None;
    }
    let mut v = normalised_mosaic(raw);
    let width = raw.width;
    let stage = crate::denoise::linear_chroma::scale_of(colour_nr).filter(|_| crate::denoise::linear_chroma::reads(raw));
    let linear_colour = stage.is_some();
    let mut cell_chroma = None;
    if let Some(scale) = stage {
        let rho = |k| crate::denoise::linear_chroma::window(colour_nr, k);
        cell_chroma = crate::denoise::linear_chroma::filter_mosaic(&mut v, width, raw.height, &config.cfa, stage_wb(raw), scale, rho, reading, crate::denoise::linear_chroma::preview_level(raw.width, raw.height, max_edge))
            .map(|(_, _, chroma)| chroma);
    }
    let pixels = PixF32::new_with(v, raw.width, raw.height);
    if !demosaic {
        return Some((Intermediate::Monochrome(pixels), linear_colour));
    }
    let crop_active = dev.steps.contains(&ProcessingStep::CropActiveArea);
    let roi = if crop_active { raw.active_area.unwrap_or(pixels.rect()) } else { pixels.rect() };
    let mut rgb = PPGDemosaic::new().demosaic(&pixels, &config.cfa, &config.colors, roi);
    drop(pixels);
    // The sensor measures colour once per cell; pixel-scale chroma is the
    // demosaic's, so the stage's cell chroma is the pixel's.
    if let Some(chroma) = &cell_chroma {
        let width = rgb.width;
        chroma.rebuild(&mut rgb.data, width, roi.p.x, roi.p.y);
    }
    if dev.steps.contains(&ProcessingStep::Calibrate) {
        use rawler::imgop::matrix::{multiply, normalize, pseudo_inverse};
        let found = raw
            .color_matrix
            .iter()
            .find(|(illuminant, _)| **illuminant == rawler::imgop::xyz::Illuminant::D65)
            .or_else(|| raw.color_matrix.iter().next());
        if let Some((_, m)) = found.filter(|(_, m)| m.len() % 3 == 0) {
            let mut xyz2cam = [[0.0f32; 3]; 4];
            for i in 0..m.len() / 3 {
                for j in 0..3 {
                    xyz2cam[i][j] = m[i * 3 + j];
                }
            }
            let wb = if raw.wb_coeffs[0].is_nan() || !dev.steps.contains(&ProcessingStep::WhiteBalance) {
                [1.0f32; 4]
            } else {
                raw.wb_coeffs
            };
            let cam2rgb = pseudo_inverse(normalize(multiply(&xyz2cam, &rawler::imgop::xyz::SRGB_TO_XYZ_D65)));
            rgb.data.par_iter_mut().for_each(|px| {
                let (r, g, b) = (px[0] * wb[0], px[1] * wb[1], px[2] * wb[2]);
                let out = [
                    cam2rgb[0][0] * r + cam2rgb[0][1] * g + cam2rgb[0][2] * b,
                    cam2rgb[1][0] * r + cam2rgb[1][1] * g + cam2rgb[1][2] * b,
                    cam2rgb[2][0] * r + cam2rgb[2][1] * g + cam2rgb[2][2] * b,
                ];
                *px = rawler::imgop::raw::clip_euclidean_norm_avg(&out);
            });
        }
    }
    if dev.steps.contains(&ProcessingStep::CropDefault)
        && let Some(mut crop) = raw.crop_area.or(raw.active_area)
    {
        if crop_active {
            crop = crop.adapt(&raw.active_area.unwrap_or(crop));
        }
        if rgb.dim().w == raw.active_area.map(|area| area.d).unwrap_or(raw.dim()).w / 2 {
            crop.scale(0.5);
        }
        if crop.d != rgb.dim() {
            rgb = rgb.crop(crop);
        }
    }
    if dev.steps.contains(&ProcessingStep::SRgb) {
        rgb.for_each(rawler::imgop::srgb::srgb_apply_gamma_n);
    }
    Some((Intermediate::ThreeColor(rgb), linear_colour))
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
pub(super) fn cfa_needs_geometry_demosaic(cfa: &rawler::cfa::CFA) -> bool {
    cfa.is_rgb() && (cfa.width, cfa.height) != (2, 2)
}

pub(super) fn non_bayer_rgb_cfa(raw: &rawler::RawImage) -> Option<&rawler::cfa::CFA> {
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
///
/// Raised through [`crate::diag`] like every other develop-chain disclosure, so
/// the line carries its photograph as DATA and a pooled `batch` worker can put
/// it in that worker's own transcript block. It is bound to the process default
/// rather than to a caller's sink, and that is the decision: this sits under
/// `render_to_image`, whose call sites do not carry a channel, and threading one
/// through all of them to reach a single disclosure would buy nothing today.
/// The line's own text no longer spells the path — the sink renders the
/// attribution — so it reads `⚠ <stem>: this file comes from a …` where it used
/// to read `⚠ <full path> comes from a …`.
fn disclose_approximate_demosaic(raw: &rawler::RawImage, path: &Path) {
    let Some(cfa) = non_bayer_rgb_cfa(raw) else { return };
    crate::diag::photo(path).warn(format!(
        "this file comes from a {}×{} non-Bayer colour filter array ({}), which this build \
         demosaics over the array's own geometry instead of with an algorithm written for this \
         sensor family. Every channel is interpolated from the photosites that actually measured \
         it, so colour, tone and framing are correct; fine detail is softer than a dedicated \
         converter would resolve",
        cfa.width, cfa.height, cfa
    ));
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
pub(super) struct CfaTaps {
    /// Window half-width the offsets below are BIASED by, so both index
    /// straight into [`wrap_table`]'s tables with no signed arithmetic in the
    /// inner loop. Wide enough for the whole-tile fallback, not just
    /// [`CFA_TAP_RADIUS`].
    pub(super) radius: usize,
    /// `[(phase_row * cfa.width + phase_col) * 3 + channel]` → that channel's
    /// taps as `(biased_row_offset, biased_col_offset, weight)`.
    pub(super) per_phase: Vec<Vec<(usize, usize, f32)>>,
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

pub(super) fn cfa_taps(cfa: &rawler::cfa::CFA) -> CfaTaps {
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
pub(super) fn wrap_table(n: usize, period: usize, radius: usize) -> Vec<usize> {
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
pub(super) fn demosaic_over_cfa_geometry(
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
pub(super) fn apply_crop(img: DynamicImage, crop: Option<&Crop>) -> DynamicImage {
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
pub(super) fn rgb16_source(img: &DynamicImage) -> Cow<'_, ImageBuffer<Rgb<u16>, Vec<u16>>> {
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
///
/// `baked` is the lens geometry the pixels carry (v1.17.0, the file's marker,
/// [`SourceFrame::of`]): a lens-corrected master is masked in the corrected
/// frame and takes no second resample in the tail. The caller has already
/// refused a recipe that develops other geometry ([`SourceFrame::check`]).
/// `colour_nr_in_pixels`: the pixels already carry the develop's one colour
/// noise reduction (a marked master, [`SourceFrame::colour_nr_in_pixels`]),
/// so the display-frame pass does not run again.
pub fn render_baked_to_image(
    img: &DynamicImage,
    recipe: &EditRecipe,
    denoise: Option<&crate::denoise::DenoiseOpts>,
    max_edge: Option<u32>,
    baked: Option<&BakedGeometry>,
    colour_nr_in_pixels: bool,
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
    // The source's own short edge, before the cap (`FilmScale`).
    let film_short = Some(img.width().min(img.height()));
    // Downscale-only, before anything else allocates a plane. `Cow` so the
    // uncapped path (every shipped export) still borrows and copies nothing.
    let capped: Cow<'_, DynamicImage> = match max_edge {
        Some(edge) if img.width().max(img.height()) > edge => {
            Cow::Owned(super::thumbnail(img, edge, edge))
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
    // The auto-CA solver, before the profile that carries its answer — the RAW
    // arm's rule above, for the same reason.
    let solved = lens::with_auto_lateral_ca(recipe, &data, w, h);
    let recipe = &*solved;
    // Same ONE-value rule as the RAW arm above (`MaskFrame`): the mask chain's
    // frame adaptation and the geometry gate below are the same decision.
    let geom = geometry_profile(recipe);
    let corrected = baked.is_some_and(BakedGeometry::corrects_frame);
    let frame = MaskFrame::of_base(&geom, recipe.lens_distortion, corrected);
    let film = FilmScale::of(film_short, w, h);
    // A baked raster: Lightroom gives a JPEG / TIFF no sharpening by default,
    // so an absent amount renders none (`EditRecipe::capture_sharpening`).
    apply_develop_with_rasters(&mut data, w, h, recipe, &rasters, frame, film, false, colour_nr_in_pixels);

    let mut buf: Vec<u16> = vec![0u16; w * h * 3];
    buf.par_chunks_mut(3).zip(data.par_iter()).for_each(|(o, px)| {
        o[0] = to_u16(px[0]);
        o[1] = to_u16(px[1]);
        o[2] = to_u16(px[2]);
    });
    let out: ImageBuffer<Rgb<u16>, _> = ImageBuffer::from_raw(w as u32, h as u32, buf)
        .ok_or_else(|| anyhow!("baked pixel buffer size mismatch"))?;
    let dynimg = DynamicImage::ImageRgb16(out);

    // The same tail as the RAW arm above, from the same function: lens
    // geometry → straighten → crop → post-crop vignette and grain, with the
    // same composed profile (manual CA included). Orientation is already baked
    // into the source here. A lens-corrected master takes no second resample
    // ([`tail_lens`]).
    Ok(frame_and_finish(dynimg, recipe, tail_lens(&geom, corrected), film, CropPolicy::Cut))
}
