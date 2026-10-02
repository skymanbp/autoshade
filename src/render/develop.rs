//! The develop pipeline proper: previews, the per-pixel and spatial passes, the colour field, vignettes and dehaze.

use super::*;

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
/// the exception is real: the GUI's range REFERENCE builds and its Point
/// Color eyedropper develop a recipe that still carries a lens profile and
/// then apply NO geometry, because they exist to sample pixel VALUES rather
/// than to be looked at. They pass
/// [`MaskFrame::without_downstream`] so LINEAR still receives its handle-only
/// raw-frame rule.
///
/// `film_short_edge` is [`develop_preview_film`]'s: a caller reproducing what
/// the canvas shows (the GUI's Range-mask references and Point Color samples,
/// the fill's picture of a card) passes the source's own edge so its pixels
/// are the canvas's pixels;
/// `None` treats the preview itself as the film. `raw_source` is
/// [`develop_preview_film`]'s too: the kind of source these pixels came from.
pub fn develop_preview_framed(
    preview: &DynamicImage,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
    frame: MaskFrame<'_>,
    film_short_edge: Option<u32>,
    raw_source: bool,
) -> DynamicImage {
    develop_preview_inner(preview, recipe, diag, Some(frame), film_short_edge, raw_source)
}

/// [`develop_preview_with`] for a VIEWING surface that knows how large the
/// photo really is: `film_short_edge` is the full-resolution develop's short
/// edge (`decode::film_short_edge`), so the Detail panel's pixel radii land
/// at the scale the export will show them once it is downscaled to this
/// preview — see [`FilmScale`]. The GUI canvas (`util::build_preview`) and the
/// web preview call this, and every surface that must see the canvas's own
/// pixels passes the same edge to [`develop_preview_framed`]; the analysis
/// surfaces (the reverse fit, the judge) keep the forms above, which treat the
/// preview itself as the film and compare like with like.
///
/// `raw_source` (v1.6.0) is the KIND of the source (`decode::is_raw`): an
/// absent Sharpening amount renders at Lightroom's own default for it — 40
/// on a RAW negative, none on a baked raster
/// (`EditRecipe::capture_sharpening`). The analysis forms above develop as
/// baked, which is what they did before the default existed.
pub fn develop_preview_film(
    preview: &DynamicImage,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
    film_short_edge: Option<u32>,
    raw_source: bool,
) -> DynamicImage {
    develop_preview_inner(preview, recipe, diag, None, film_short_edge, raw_source)
}

/// [`develop_preview`] with the caller's own diagnostics channel — the injected
/// form of the preview arm. `diag` states whose pixels these are (or that
/// nobody's are), and where the mask loader's refusals go.
pub fn develop_preview_with(
    preview: &DynamicImage,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
) -> DynamicImage {
    develop_preview_inner(preview, recipe, diag, None, None, false)
}

/// The preview develop. `frame` is `None` for the two entry points that let the
/// RECIPE answer "will geometry follow?" and `Some` for the caller that knows
/// better — see [`develop_preview_framed`].
fn develop_preview_inner(
    preview: &DynamicImage,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
    frame: Option<MaskFrame<'_>>,
    film_short_edge: Option<u32>,
    raw_source: bool,
) -> DynamicImage {
    // Entry-point sanitisation: ONE construction, ONE disclosure — the
    // ValidatedRecipe token (arch item c) replaces four hand-rolled
    // clone+clamp+eprintln triplets that had already drifted apart.
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    validated.disclose(diag);
    let recipe = &*validated;
    let rgb = preview.to_rgb8();
    let (w, h) = rgb.dimensions();
    let mut data: Vec<[f32; 3]> = rgb
        .as_raw()
        .par_chunks(3)
        .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
        .collect();
    apply_recipe_wb(&mut data, recipe);
    // The pixels come first here (they used to come after the profile) for one
    // reason: the auto-CA solver reads them, and `geometry_profile` below must
    // already carry its answer — on this raster as on the export's, because the
    // estimate is a ratio and it is rounded to the manual slider's own step.
    let solved = lens::with_auto_lateral_ca(recipe, &data, w as usize, h as usize);
    let recipe = &*solved;
    // Derived from the CLAMPED recipe, and from the same composed profile the
    // preview surfaces hand `apply_lens_geometry` — `geometry_profile`, not the
    // raw one, because the manual CA pair rides those knots (R25 B3).
    let geom = geometry_profile(recipe);
    let frame = frame.unwrap_or_else(|| MaskFrame::downstream(&geom, recipe.lens_distortion));
    let film = FilmScale::of(film_short_edge, w as usize, h as usize);
    apply_develop(&mut data, w as usize, h as usize, recipe, diag, frame, film, raw_source);
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
/// only thing in here that can say anything. `raw_source` is the kind of the
/// source (see [`develop_preview_film`]).
#[allow(clippy::too_many_arguments)] // one value each of the develop's own frame: pixels, size, recipe, channel, mask frame, film scale, source kind
fn apply_develop(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
    frame: MaskFrame<'_>,
    film: FilmScale,
    raw_source: bool,
) {
    let rasters = best_effort_mask_raster_snapshot(r, diag);
    apply_develop_with_rasters(data, w, h, r, &rasters, frame, film, raw_source);
}

/// [`apply_develop`] on pixels with no owner and no caller to route to — the
/// pixel-math tests, which construct a `[[f32; 3]]` by hand and have neither.
/// `Subject::PixelOnly` on the default sink: the same thing production's
/// un-injected preview arm does, said once here instead of at ~40 call sites.
#[cfg(test)]
pub(super) fn apply_develop_anon(data: &mut [[f32; 3]], w: usize, h: usize, r: &EditRecipe) {
    // `AsRendered`: these fixtures construct a raw pixel buffer and inspect it
    // directly — no geometry stage runs after them, so every mask belongs at
    // its stored coordinates (`MaskFrame`).
    apply_develop(data, w, h, r, &crate::diag::pixels(), MaskFrame::AsRendered, FilmScale::NATIVE, false);
}

/// The recipe's retouch areas as heal spots for a `w`×`h` working frame
/// (v1.5.0 F9) — the one place Lightroom's units become this engine's.
///
/// Two conversions happen here and nowhere else, because both are frame facts
/// rather than operator facts:
///
///  * **Radius.** Adobe states a half-extent in WIDTH units on both axes (the
///    convention `BrushDab::r` records for `crs:Radius`, and the one the
///    library's ellipses measure to: 161.5 px / 9504 = 0.016994 against a
///    stored 0.016938). A [`HealSpot`]'s radius is a fraction of the SHORT
///    side. One factor, `w / min(w, h)`, applied once.
///  * **Donor.** `crs:SourceX`/`crs:OffsetY` are ABSOLUTE normalised
///    coordinates of the donor's centre; `HealSpot::source` is an OFFSET from
///    the spot centre, because `find_donor` returns one. The centre comes off
///    here rather than inside the operator, so the operator has one meaning
///    for the field no matter who filled it.
///
/// A brush area goes through the mask side's own rasteriser and the retouch
/// side's own planner — no third implementation of "dabs into a shape".
///
/// [`HealSpot`]: crate::retouch::HealSpot
pub fn retouch_spots(r: &EditRecipe, w: usize, h: usize) -> Vec<crate::retouch::HealSpot> {
    use crate::retouch::{HealSpot, RetouchShape};
    if r.retouch.is_empty() || w == 0 || h == 0 {
        return Vec::new();
    }
    let to_short = w as f32 / w.min(h) as f32;
    let mut out = Vec::new();
    for a in &r.retouch {
        // Everything the raster cannot say about this area, in one value that
        // both arms below build from.
        let template = HealSpot {
            feather: a.feather,
            origin: a.origin,
            ..Default::default()
        };
        match &a.shape {
            RetouchShape::Ellipse { cx, cy, size_x, size_y } => out.push(HealSpot {
                cx: *cx,
                cy: *cy,
                // `max`, although all 84 measured ellipses are circles: the
                // larger half-extent is the one that must be covered, and a
                // file that ever states two is not a reason to under-heal.
                radius: size_x.max(*size_y) * to_short,
                source: a.donor.map(|[dx, dy]| [dx - *cx, dy - *cy]),
                ..template
            }),
            RetouchShape::Brush(strokes) => {
                if let Some(alpha) = rasterise_brush_group(strokes, w as u32, h as u32) {
                    // 127: a dab-stamped alpha is 0..255 and this is the
                    // half-way mark, the same place `plan_from_mask`'s own
                    // `< 128` brush convention puts it.
                    out.extend(crate::retouch::plan_from_alpha(&alpha, 127, &template).0);
                }
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)] // `apply_develop`'s frame with the rasters already loaded
pub(super) fn apply_develop_with_rasters(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    r: &EditRecipe,
    rasters: &MaskRasterSnapshot,
    frame: MaskFrame<'_>,
    film: FilmScale,
    raw_source: bool,
) {
    // 0-) spot removal (v1.5.0 F9) — Lightroom's `crs:RetouchAreas`, which
    //    this engine re-solves from the frame's own pixels. FIRST, for two
    //    reasons that both say "before", and one measurement that says "here".
    //
    //    Before every SPATIAL stage: de-fringe, clarity, noise reduction and
    //    sharpening all read a neighbourhood, and every one of them would
    //    otherwise be handed an object the photographer deleted — sharpening
    //    the patch seam, and letting a removed power line vote in the local
    //    contrast around it.
    //
    //    At STORED coordinates, with no unwarp, and that is the measured part.
    //    `apply_lens_geometry` runs after this whole chain and resamples the
    //    frame, so a geometry stored POST-correction has to be pre-compensated
    //    (what `MaskFrame` exists for, and what a RADIAL mask needs). Retouch
    //    does not: `crs:pm_whole_image_*` is stated in PIXELS, and across the
    //    library its extent is exactly the native sensor rectangle every time
    //    — 9504x6336 on 114 areas and 6240x4160 on 6, the two bodies' full
    //    frames, never a lens-corrected one. So these coordinates live
    //    pre-correction, the same frame `render`'s own measurement puts brush
    //    dabs in, and the repaired pixels ride the geometry resample with the
    //    rest of the photograph. Unwarping here would apply the field twice.
    if !r.retouch.is_empty() {
        crate::retouch::heal_planar(data, w, h, &retouch_spots(r, w, h));
    }
    // 00) camera calibration (v1.5.0). Lightroom's Calibration panel belongs
    //    to the camera profile, so it is the first thing that happens to the
    //    white-balanced frame and every later stage sees the moved primaries —
    //    the order Lightroom's own profile has under all of its sliders.
    apply_calibration(data, r);

    // 00b) de-fringe (v1.5.0, `render/lens.rs`). A lens DEFECT, so it is
    //    corrected before the picture is made: at the top of the chain the
    //    edge that carries the fringe still has the contrast the sensor
    //    recorded, and no later stage is asked to sharpen or saturate a
    //    colour the lens invented. (Its geometric siblings — profile
    //    distortion, the CA scales — cannot join it here: they resample the
    //    frame, and every mask's geometry is stored in this one.)
    lens::defringe(data, w, h, r);
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
    //    A camera-matched base curve is tone work too — it must not be skipped,
    //    and since v1.5.0 F7 neither must a creative profile's baked curve or
    //    its baked Highlights/Shadows. The question is asked ONCE, on the
    //    recipe (`EditRecipe::has_tone_work`), against the same inputs
    //    `build_tone_lut` reads: this condition listing its own copy of them is
    //    exactly how the profile came to render nothing.
    let tone_neutral = !r.has_tone_work();
    if !tone_neutral {
        let lut = build_tone_lut(r);
        data.par_iter_mut().for_each(|px| {
            let l_old = luma601(px);
            let l_new = sample_lut(&lut, l_old);
            scale_chroma(px, l_old, l_new);
        });
    }
    // 1a) the B&W treatment (v1.5.0) replaces the colour mixer, and it runs
    //    BEFORE the per-channel curves: a red-channel curve on a black-and-white
    //    photo is how Lightroom photographers tone one, so the curves have to
    //    meet the grey rather than the colour it came from. Colour grading
    //    tints the grey further down, in its ordinary place.
    // Either switch: the photographer's own, or the one a monochrome creative
    // profile bakes ([`EditRecipe::renders_grayscale`]).
    let monochrome = r.renders_grayscale();
    if monochrome {
        apply_gray_mix(data, &r.gray_mixer());
    }
    // 1b) per-channel RGB curves (red/green/blue), right after the master curve.
    apply_rgb_curves(data, r);
    // 2) per-colour HSL (the 8 ACR bands): rotate/scale each colour family,
    //    after global tone and before clarity/saturation (ACR ordering) — then
    //    the point colours (v1.5.0), the Color Mixer's other half. Neither has
    //    a colour to act on in black and white, where Lightroom's panel offers
    //    the B&W mixer in their place.
    if !monochrome {
        apply_hsl(data, &r.hsl);
        apply_point_colors(data, &r.point_colors);
    }
    // 2b) colour grading wheels (shadow/midtone/highlight/global toning + lum).
    apply_color_grade(data, &r.color_grade);
    // 3) clarity — large-radius, midtone-masked local contrast.
    if r.clarity != 0.0 {
        unsharp_luma(data, w, h, clarity_radius(w, h), r.clarity / 100.0, true);
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
    // 3) saturation / vibrance — not in black and white, where the only
    //    colour left is the grade's tint: a Saturation slider that also
    //    re-strengthened the toning would make the grade's own saturation mean
    //    two things.
    let (sat, vib) = (r.saturation / 100.0, r.vibrance / 100.0);
    if (sat != 0.0 || vib != 0.0) && !monochrome {
        data.par_iter_mut().for_each(|px| {
            *px = apply_sat_vibrance(px[0], px[1], px[2], sat, vib);
        });
    }
    // 4) the Detail panel (v1.5.0, `detail.rs`): colour noise, then luminance
    //    noise, then sharpening. Noise reduction runs BEFORE sharpening (the
    //    order that matters most), and colour before luminance so the
    //    luminance pass smooths a frame whose chroma blotches are already
    //    gone. Every radius is stated in FILM pixels and converted through
    //    `film`, which retires the V2 §4c/§4d rules (sharpening σ =
    //    clamp(0.0008·min(w,h), 0.7, 2.0), NR at a raster-pixel radius) under
    //    which one slider value meant two structures at preview and at export.
    if let Some(p) = detail::ChromaNrParams::global(r) {
        detail::chroma_nr(data, w, h, &p, film);
    }
    if let Some(p) = detail::LumaNrParams::global(r) {
        detail::luma_nr(data, w, h, &p, film, |_, _, _| 1.0);
    }
    if let Some(p) = detail::SharpenParams::global(r, raw_source) {
        detail::sharpen(data, w, h, &p, film, |_, _, _| 1.0);
    }
    // 6) local masked adjustments (linear/radial gradients).
    if !r.masks.is_empty() {
        apply_masks(data, w, h, r, rasters, frame, film);
    }
    // 7) the colour field, after the masks on purpose: it is the residual the
    //    mask-shaped controls above could not reach, so it must read the
    //    frame they produced. Its guide is the render's own smoothed luma at
    //    the render's own resolution, which is what makes it a pure function
    //    of the pixels in front of it at any size.
    apply_colour_field(data, w, h, r.colour_field.as_ref());
    // 8) the SDR rendition (v1.5.0 F8), the last EDIT because that is what it is: not
    //    another edit but the mapping of the finished edit into the range this
    //    engine can publish. Lightroom derives it from the completed HDR
    //    develop, and a control that ran BEFORE the colour field or the masks
    //    would be one they could then undo.
    //
    //    Absent unless the photograph is in HDR edit mode, so every SDR
    //    photograph reaches the packer through exactly the code it always did.
    if let Some(p) = hdr::SdrRendition::global(r) {
        hdr::apply(data, w, h, &p);
    }
    // 9) pixel layers (2026-10-01, `layers.rs`): generated patches over the
    //    FINISHED develop — what the model was shown when it made them — in
    //    the order they were added. Stored coordinates, like the retouch
    //    areas above: the geometry stage that follows carries them.
    for layer in r.pixel_layers.iter().filter(|l| l.draws()) {
        if let Some(img) = rasters.layer(&layer.path) {
            layers::composite(data, w, h, img, layer.opacity);
        }
    }
}

/// Render one [`ColourField`] over the frame it is handed, in place.
///
/// ONE implementation, shared with [`crate::fit_field`]: the analyzer's own
/// render is a call into this function, so the field the solver measures and
/// the field the engine ships cannot drift apart. A field that is absent,
/// switched off, at zero amount, or whose grid length disagrees with its own
/// declared shape leaves the frame untouched — a mis-shaped grid is not a
/// field to be guessed at.
///
/// Per pixel: a trilinear read of the (x, y, luma-bin) grid, then
/// `delta_c = ln2·c·EV + c·gain_c + (c - guide)·slope`, scaled by `amount`,
/// then a display clamp. The guide is [`field_guide_luma`], the same
/// separable 3-tap the analyzer builds.
///
/// **Scale.** The grid's spatial axes are NORMALISED to the frame, so a field
/// solved on the 384×256 analysis raster renders correctly at 2048 or at
/// full sensor resolution: cell (i, j) covers the same fraction of the picture
/// either way. What does NOT rescale is the guide's 3-tap kernel, which is
/// three pixels wide whatever those pixels are. That difference is
/// second-order here because the guide's only job is to pick a LUMA BIN out of
/// eight: a 1/8-wide bin is 32 code values, and the two kernels disagree by
/// far less than that except on hard edges, where the trilinear read blends
/// the two bins anyway. It is measured rather than asserted — see the report
/// for the 2048-vs-384 discrepancy on the reference pair.
pub(crate) fn apply_colour_field(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    field: Option<&crate::recipe::ColourField>,
) {
    let Some(field) = field.filter(|f| f.renderable()) else { return };
    if w == 0 || h == 0 || data.len() != w * h {
        return;
    }
    let guide = field_guide_luma(data, w, h);
    let (xs, ys) = (field_axis(w, field.x), field_axis(h, field.y));
    let amount = field.amount.clamp(0.0, 1.0);
    for (i, c) in data.iter_mut().enumerate() {
        let coords = [
            xs[i % w],
            ys[i / w],
            guide[i].clamp(0.0, 1.0) * (field.b - 1) as f32,
        ];
        let limits = [field.x, field.y, field.b];
        let (mut low, mut high, mut frac) = ([0usize; 3], [0usize; 3], [0.0f32; 3]);
        for (axis, &limit) in limits.iter().enumerate() {
            let floor = (coords[axis].floor() as i64).clamp(0, limit as i64 - 1) as usize;
            (low[axis], high[axis]) = (floor, (floor + 1).min(limit - 1));
            frac[axis] = coords[axis] - floor as f32;
        }
        let mut p = [0.0f64; 5];
        for slot in 0..8 {
            let up = [slot >> 2 & 1, slot >> 1 & 1, slot & 1];
            let at = |a: usize| if up[a] == 1 { high[a] } else { low[a] };
            let mass = |a: usize| if up[a] == 1 { frac[a] } else { 1.0 - frac[a] };
            let weight = (mass(0) * mass(1) * mass(2)) as f64;
            let vertex = &field.grid[(at(1) * field.x + at(0)) * field.b + at(2)];
            for (q, slot) in p.iter_mut().enumerate() {
                *slot += weight * vertex[q] as f64;
            }
        }
        let g = guide[i];
        for ch in 0..3 {
            let d = std::f64::consts::LN_2 * c[ch] as f64 * p[0]
                + c[ch] as f64 * p[1 + ch]
                + (c[ch] - g) as f64 * p[4];
            c[ch] = (c[ch] + amount * d as f32).clamp(0.0, 1.0);
        }
    }
}

/// The colour field's guide: `grid_experiment.smooth_3tap` on `luma601`,
/// edge-padded 3-tap along x then y.
///
/// It lives HERE, in the engine, rather than in the analyzer that used to own
/// it (as `fit_field::smooth_3tap_luma`, which is now a re-export of this
/// name), because the engine is the side that has to render a stored field.
/// One implementation is not a tidiness point: the field's whole contract is
/// that what the solver measured is what the engine ships, and a guide that
/// padded one way here and another way there would move every border pixel's
/// luma BIN.
pub(crate) fn field_guide_luma(px: &[[f32; 3]], width: usize, height: usize) -> Vec<f32> {
    let luma: Vec<f32> = px.iter().map(luma601).collect();
    let mut horizontal = vec![0.0f32; luma.len()];
    for (i, slot) in horizontal.iter_mut().enumerate() {
        let (row, x) = (i - i % width, i % width);
        let (left, right) = (row + x.saturating_sub(1), row + (x + 1).min(width - 1));
        *slot = (luma[left] + luma[i] + luma[right]) / 3.0;
    }
    let mut out = vec![0.0f32; luma.len()];
    for (i, slot) in out.iter_mut().enumerate() {
        let (y, x) = (i / width, i % width);
        let (up, down) = (y.saturating_sub(1) * width + x, (y + 1).min(height - 1) * width + x);
        *slot = (horizontal[up] + horizontal[i] + horizontal[down]) / 3.0;
    }
    out
}

/// The colour field's spatial axis: `numpy.linspace(0, limit - 1, n)` in f64,
/// cast once at the end, the last sample pinned exactly on the stop value so
/// the grid spans the frame at any resolution. Shared with the analyzer's
/// splat table for the same reason the guide is.
pub(crate) fn field_axis(n: usize, limit: usize) -> Vec<f32> {
    if n <= 1 {
        return vec![0.0; n];
    }
    let (stop, step) = ((limit - 1) as f64, (limit - 1) as f64 / (n - 1) as f64);
    let mut out: Vec<f32> = (0..n).map(|i| (i as f64 * step) as f32).collect();
    out[n - 1] = stop as f32;
    out
}

pub(super) fn luma601(p: &[f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

/// Manual lens-vignette compensation LUT: gain = 1 + k·rⁿ on the normalised
/// corner-radius, in linear light. `amount` -100..=100 (positive brightens
/// corners); `midpoint` 0..=100 shapes WHERE it lands via the radius exponent
/// (0.6..3.0, ACR-default 50 → 1.8): low reaches toward the centre, high
/// confines the correction to the corners. The exact LR falloff model is
/// proprietary — this is our documented approximation (XMP carries the raw
/// slider values, so Lightroom re-renders with its own model).
pub(super) fn manual_vignette_lut(amount: f32, midpoint: f32) -> Vec<f32> {
    let gamma = 0.6 + 2.4 * (midpoint.clamp(0.0, 100.0) / 100.0);
    let k = amount.clamp(-100.0, 100.0) / 100.0;
    (0..LUT_N)
        .map(|i| 1.0 + k * (i as f32 / (LUT_N - 1) as f32).powf(gamma))
        .collect()
}

/// Profile vignetting LUT: per-knot linear-light GAINS over the normalised
/// corner radius (knot placement (i+0.5)/(n−1) — see `lensmeta`), linearly
/// interpolated. Gains come from the PROFILE — the camera's own metadata, or
/// an Adobe `.lcp` on this machine — not from a slider model.
///
/// `scale` is Lightroom's profile Vignetting strength, 0..200 with 100 = the
/// profile's own gains. Like the distortion strength in [`geometry_profile`]
/// it scales the correction's DEPARTURE FROM IDENTITY (`1 + (g − 1)·s`), and
/// 100 takes the branch that returns the gains untouched, so every render made
/// before v1.5.0 stays bit-identical.
pub(super) fn profile_vignette_lut(knots: &[f32], scale: f32) -> Vec<f32> {
    let g = |i: usize| profile_knot_interp(knots, i as f32 / (LUT_N - 1) as f32);
    let s = (scale / 100.0).clamp(0.0, 2.0);
    if s == 1.0 {
        return (0..LUT_N).map(g).collect();
    }
    (0..LUT_N).map(|i| 1.0 + (g(i) - 1.0) * s).collect()
}

/// The single radial-gain LUT for whichever vignette stages are active —
/// `None` when neither is. Both active compose by MULTIPLYING the gains, so
/// the one clamp in `apply_radial_gain` runs on the true combined gain and
/// inverse corrections genuinely cancel (L01-6).
pub(super) fn vignette_gain_lut(r: &EditRecipe) -> Option<Vec<f32>> {
    let profile = r
        .lens_profile
        .vignette_active()
        .then(|| profile_vignette_lut(&r.lens_profile.vignette, r.lens_profile_vignetting_scale));
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
pub(super) fn apply_radial_gain(data: &mut [[f32; 3]], w: usize, h: usize, gain_lut: &[f32]) {
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
pub(super) fn apply_dehaze(data: &mut [[f32; 3]], w: usize, amount: f32) {
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
pub(super) fn dehaze_airlight(data: &[[f32; 3]], w: usize) -> f32 {
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
pub(super) fn dehaze_px(px: &[f32; 3], a: f32, s: f32, dec: &[f32], enc: &[f32]) -> [f32; 3] {
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
