//! The canvas's base develop: the ONE develop every surface that shows, fits
//! or bakes a photograph starts from.
//!
//! A recipe renders in two halves. The PROFILE STAGE — a named camera
//! profile's colour tables (matrices, hue/sat map, look table) with the
//! creative Look's colour table on top (`profile::Stage`) — runs INSIDE the
//! RAW develop. Everything after it (white balance, tone, colour, detail,
//! masks, geometry) runs on the developed frame, and the preview develop
//! ([`develop_preview`] and its forms) runs exactly that second half on pixels
//! it is handed. So pixels handed to a preview develop must already carry the
//! first half: they are the base develop of the recipe's [`BasePart`]. A
//! recipe that names no profile — every fresh canvas — has the neutral part.

use std::sync::{Arc, Mutex};

use super::*;
use super::develop::{apply_radial_gain, apply_radial_gain_open, vignette_gain_lut};
use super::luma::scale_chroma_open;

/// The part of a recipe the BASE develop renders: which camera profile, and
/// which creative Look's colour table. Two recipes with equal parts develop
/// from the same base pixels; a surface holding a base compares parts to know
/// whether the base still belongs to the recipe it is about to show.
///
/// The Look is kept WITHOUT its baked half: a profile and a Look are colour
/// data, never tone.
///
/// The part also carries the strength of the RAW's colour noise reduction (E3
/// of v1.18.0). That stage runs on the linear mosaic inside the demosaic, the
/// develop's first half (`denoise::linear_chroma`), so it belongs to the part:
/// one law decides it for every sensor develop — the export, the canvas's
/// base, a master's bake, the fit's plate — and that law is the colour-NR
/// control's own (`detail::ChromaNrParams::global`): the stage runs exactly
/// when the recipe the part is taken from asks for colour NR, at its slider.
/// A master is baked through the part of the photo's recipe, so a master of a
/// photo whose colour NR is on carries the stage, as the export of that photo
/// does, and says so in its file (`BakedColourNr`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BasePart {
    camera_profile: String,
    look: Option<crate::recipe::CreativeLook>,
    colour_nr: f32,
}

impl BasePart {
    /// The base part of `r`. A recipe that names no profile has the NEUTRAL
    /// part, whatever else its Look carries — the profile stage cannot run
    /// without a profile ([`EditRecipe::names_profile`]).
    pub fn of(r: &EditRecipe) -> Self {
        let colour_nr = super::detail::ChromaNrParams::global(r).map_or(0.0, |_| r.color_nr);
        if !r.names_profile() {
            return Self { colour_nr, ..Self::default() };
        }
        Self {
            colour_nr,
            camera_profile: r.camera_profile.clone(),
            look: r.look.as_ref().map(|l| crate::recipe::CreativeLook {
                name: l.name.clone(),
                amount: l.amount,
                base_profile: l.base_profile.clone(),
                table: l.table.clone(),
                ..Default::default()
            }),
        }
    }

    /// The neutral develop: no profile named, and the mosaic as the sensor
    /// wrote it.
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// The strength the RAW's colour noise reduction runs at in this part's
    /// develop (the slider; 0 = off). See [`BasePart`].
    pub fn colour_nr(&self) -> f32 {
        self.colour_nr
    }

    /// The recipe that develops exactly this part and nothing else — with
    /// Sharpening at an explicit 0: the RAW default capture sharpening belongs
    /// to the display and the export, which run it over the base.
    pub fn recipe(&self) -> EditRecipe {
        let mut r = self.master_recipe();
        r.set_resolved("sharpening", 0.0);
        r
    }

    /// The recipe a MASTER of this part is baked through ([`master_develop`]):
    /// [`BasePart::recipe`] with the RAW default capture sharpening kept. A
    /// master is a baked file, and a baked file is rendered with no default
    /// sharpening (`EditRecipe::capture_sharpening`), so the master carries the
    /// one sharpening its RAW is owed.
    pub fn master_recipe(&self) -> EditRecipe {
        EditRecipe { camera_profile: self.camera_profile.clone(), look: self.look.clone(), ..EditRecipe::default() }
    }
}

/// A master's pixels with the frame they are in: what a heal, a clone, a fill
/// or a stack bakes over, and what it stamps on the file it writes.
pub struct Master {
    /// The pixels.
    pub image: DynamicImage,
    /// The lens geometry in them, to stamp on a master made from them.
    pub baked: Option<BakedGeometry>,
    /// The geometry that maps the SENSOR frame onto them, when they are
    /// lens-corrected: painted masks, picked points and stored spots live in
    /// the sensor frame.
    frame_lens: Option<crate::recipe::LensProfile>,
}

impl Master {
    /// Pixels in the sensor frame, carrying no geometry.
    pub fn sensor(image: DynamicImage) -> Self {
        Self { image, baked: None, frame_lens: None }
    }

    /// `image`, a develop of a source whose frame is `frame` under `part`.
    /// `Err` when a marked source's geometry is not `part`'s
    /// ([`SourceFrame::check`]'s rule).
    pub fn of(image: DynamicImage, frame: &SourceFrame, part: &BasePart) -> Result<Self> {
        let _ = part;
        match frame {
            SourceFrame::Raw { .. } | SourceFrame::Baked(None, _) => Ok(Self::sensor(image)),
            SourceFrame::Baked(Some(g), _) => bail!(
                "this master has {g} in its pixels, but the develop bakes no lens geometry — bake \
                 the master again from the RAW"
            ),
        }
    }

    /// Are the pixels lens-corrected?
    pub fn corrected(&self) -> bool {
        self.frame_lens.is_some()
    }

    /// The mask-frame marker of these pixels' frame ([`BakedGeometry::frame_of`]'s
    /// form): `None` in the sensor frame.
    fn frame_marker(&self) -> Option<BakedGeometry> {
        let p = self.frame_lens.as_ref()?;
        BakedGeometry::frame_marker(p, 0.0, self.baked.as_ref().map_or_else(String::new, |b| b.lens_profile.clone()))
    }

    /// Is `raster` (read at `path` through [`open_mask_raster`]) already in
    /// these pixels' frame? A sensor-frame raster is not, and is mapped; one
    /// drawn in a corrected frame is when that frame is these pixels', and is
    /// refused when it is not — never read a lens field away from where it was
    /// drawn.
    fn mask_in_frame(&self, raster: &MaskRaster, path: &Path) -> Result<bool> {
        let Some(g) = &raster.frame else { return Ok(false) };
        let mine = self.frame_marker();
        if mine.as_ref() == Some(g) {
            return Ok(true);
        }
        let mine = mine.map_or_else(|| "the sensor frame".to_string(), |m| format!("the frame of {m}"));
        bail!("the mask raster {} is drawn in the frame of {g}, but these pixels are in {mine}", path.display())
    }

    /// An RGBA mask raster (a painted heal or fill mask) read through THE
    /// entry and put in these pixels' frame ([`Master::rgba_to_frame`] for a
    /// sensor-frame one).
    pub fn rgba_mask_to_frame(&self, raster: &MaskRaster, path: &Path) -> Result<image::RgbaImage> {
        let rgba = raster.image.to_rgba8();
        Ok(if self.mask_in_frame(raster, path)? { rgba } else { self.rgba_to_frame(&rgba) })
    }

    /// [`Master::rgba_mask_to_frame`] for a grey raster (a limit mask).
    pub fn gray_mask_to_frame(&self, raster: &MaskRaster, path: &Path) -> Result<image::GrayImage> {
        let gray = raster.image.to_luma8();
        Ok(if self.mask_in_frame(raster, path)? { gray } else { self.gray_to_frame(&gray) })
    }

    /// A SENSOR-frame RGBA raster (a painted heal or fill mask) mapped onto
    /// these pixels' frame.
    pub fn rgba_to_frame(&self, raster: &image::RgbaImage) -> image::RgbaImage {
        match &self.frame_lens {
            Some(p) => sensor_rgba_to_corrected(raster, p),
            None => raster.clone(),
        }
    }

    /// A SENSOR-frame grey raster (a limit mask) mapped onto these pixels'
    /// frame.
    pub fn gray_to_frame(&self, raster: &image::GrayImage) -> image::GrayImage {
        match &self.frame_lens {
            Some(p) => sensor_raster_to_corrected(raster, p),
            None => raster.clone(),
        }
    }

    /// A SENSOR-frame normalised point mapped onto these pixels' frame.
    pub fn point_to_frame(&self, (nx, ny): (f32, f32)) -> (f32, f32) {
        match &self.frame_lens {
            Some(p) => {
                let dims = (self.image.width() as f32, self.image.height() as f32);
                lens_ungeom_norm(nx, ny, dims, p, 0.0)
            }
            None => (nx, ny),
        }
    }
}

/// Develop `raw`'s base for `part` — the profile stage and the neutral rest —
/// at `max_edge` (the cap runs right after the profile stage, as in
/// [`render_to_image`]), unsharpened ([`BasePart::recipe`]).
pub fn base_develop(raw: &Path, part: &BasePart, max_edge: Option<u32>) -> Result<DynamicImage> {
    render_frame(raw, &part.recipe(), None, max_edge, ExportColorSpace::Srgb, crate::diag::stderr(), part.colour_nr()).map(|(img, _)| img)
}

/// A master's pixels for `part` (a heal, a fill, a denoise): the base part
/// developed with the RAW default sharpening a baked master is owed
/// ([`BasePart::master_recipe`]), 16-bit as the master file is.
pub fn master_develop(raw: &Path, part: &BasePart, max_edge: Option<u32>) -> Result<DynamicImage> {
    render_frame(raw, &part.master_recipe(), None, max_edge, ExportColorSpace::Srgb, crate::diag::stderr(), part.colour_nr()).map(|(img, _)| img)
}

/// [`source_pixels_for`] for a master: a RAW through [`master_develop`], a
/// baked source its own pixels — each with the frame it is in ([`Master`]).
pub fn master_pixels_for(path: &Path, part: &BasePart, cap: Option<u32>) -> Result<Master> {
    let frame = SourceFrame::of(path)?;
    let image = if frame.is_raw() { master_develop(path, part, cap)? } else { source_pixels(path, cap)? };
    Master::of(image, &frame, part)
}

/// Pixels the reverse fit renders its candidates on: an image that may say
/// which FRAME it is in. The reverse fit works in the lens-corrected frame
/// when the target was exported with the profile on (`pipeline`), and a
/// master can be marked as baked in it ([`SourceFrame`]). The preview develop
/// handed a corrected plate's pixels reads its masks in that frame
/// ([`MaskFrame::Corrected`]). Any other pixels — a copy, a crop — are in the
/// sensor frame they were always read in.
pub struct Plate {
    image: Arc<DynamicImage>,
    registered: bool,
}

/// One registered plate: where its pixels live, which frame they are in, and
/// the base encoding its resamples go through ([`BaseEncoding`]).
struct PlateEntry {
    at: (usize, usize),
    corrected: bool,
    encoding: Option<Arc<BaseEncoding>>,
}

/// The POINTWISE stages of a fit's base recipe that the export runs on every
/// pixel before it resamples: the lens profile's vignette gain and the base's
/// tone (the camera-matched base curve and any tone a profile bakes), in the
/// develop's own order and with the develop's own operators
/// ([`apply_radial_gain`], the luma-ratio tone pass of `apply_develop`).
///
/// Why a plate needs it (v1.18.0, measured on the self-consistency gate's
/// identical pairs): the export develops at its working edge and resamples
/// afterwards — the lens geometry, then whoever reads it at the analysis
/// raster averages pixels that already carry the base curve. The fit's plate
/// was resampled first (the geometry warp in `pipeline::fit_target`, the
/// analysis thumbnail) and had the base curve applied to the averages. A
/// curve does not commute with an average: on the corpus's base curves,
/// steep in the shadows, the identical pair read 0.00235 (p39) and 0.00285
/// (p40) against `fit::FIT_QUANT` 0.0018, and the fit corrected a gap that
/// was not in the photograph. Every resample of a plate that carries an
/// encoding therefore runs between [`Self::encode`] and its pointwise
/// inverse [`Self::decode`], so the pixels a candidate is rendered on are the
/// pixels the export would have averaged.
///
/// THE WHITE CLAMP (v1.18.0, J151). The develop clamps every channel at
/// white after each of the two stages, and what a clamp cut no inverse gives
/// back. Until this version the plate was decoded from the clamped frame, so
/// wherever the base's curve lifts a channel past white the plate held less
/// of that channel than the pixels the export develops. The export never
/// makes that cut — it runs the photo's white balance BEFORE these stages
/// and clamps what its OWN tone leaves — so a candidate that pulls the
/// channel down was rendered on less light than the export has. Round V14,
/// the lantern street (p38, 0.85): the base's develop holds the blue of
/// 5.74 % of the fit's frame at white, the overcast sky; on those
/// pixels the fit's own preview of the recipe it shipped read blue
/// 244.3 in the mean and the export of that recipe
/// 253.0 (the colour field off in both; over the rest of the
/// frame the two agree to 0.2 of a code in each channel's mean), and the sky
/// the fit had matched was rendered with more blue than the target has. The
/// two stages modelled on that photograph's own pixels (rel1180
/// fieldproto/plate_model.py, 2026-11-13) put the old plate
/// 11.0 codes of blue under them where the curve cuts, and the
/// plate as it is made now within 0.02 of a code.
///
/// So the encoding keeps two forms of its stages — the develop's own
/// ([`Self::develop`], clamped as the develop clamps) and the OPEN one
/// ([`Self::open`], nothing cut at white, invertible) — and their
/// difference is what the clamp cut ([`Self::encode`]). A plate is decoded
/// from the develop's frame with that light put back ([`Self::restore`]),
/// and a resample carries it through the same resampler beside the frame
/// ([`Self::resample`]): a resampler is linear, so the parts sum to the
/// resample of the open frame, through a 16-bit resampler too. Wherever the
/// clamp cut nothing the difference is exactly zero.
pub(crate) struct BaseEncoding {
    vignette: Option<Vec<f32>>,
    tone: Option<Vec<f32>>,
    tone_inverse: Option<Vec<f32>>,
}

impl BaseEncoding {
    /// The encoding of `base`, or `None` when its pointwise stages are the
    /// identity (no vignette, no tone work).
    pub(crate) fn of(base: &EditRecipe) -> Option<Arc<Self>> {
        let vignette = vignette_gain_lut(base);
        let tone = base.has_tone_work().then(|| build_tone_lut(base));
        if vignette.is_none() && tone.is_none() {
            return None;
        }
        // The tone LUT is monotone with a pinned white (`build_tone_lut`), so
        // its inverse on the same grid is read by bisection.
        let tone_inverse = tone.as_ref().map(|lut| {
            let n = lut.len();
            (0..n)
                .map(|i| {
                    let y = i as f32 / (n - 1) as f32;
                    let (mut lo, mut hi) = (0.0f32, 1.0f32);
                    for _ in 0..32 {
                        let mid = 0.5 * (lo + hi);
                        if sample_lut(lut, mid) < y { lo = mid } else { hi = mid }
                    }
                    0.5 * (lo + hi)
                })
                .collect()
        });
        Some(Arc::new(Self { vignette, tone, tone_inverse }))
    }

    /// The base's pointwise stages as the develop runs them, in place:
    /// vignette gain, then the tone on the pixel's luma with RGB scaled by the
    /// ratio — every channel clamped at white after each, by the develop's
    /// own operators.
    fn develop(&self, data: &mut [[f32; 3]], w: usize, h: usize) {
        if let Some(lut) = &self.vignette {
            apply_radial_gain(data, w, h, lut);
        }
        if let Some(lut) = &self.tone {
            data.par_iter_mut().for_each(|px| {
                let l_old = luma601(px);
                scale_chroma(px, l_old, sample_lut(lut, l_old));
            });
        }
    }

    /// [`Self::develop`] with nothing cut at white, in place: the same two
    /// stages through their open operators. Past the luma of white the tone
    /// is the straight line through its own white point (the identity, under
    /// the LUT's pinned white), so the map has an inverse everywhere
    /// ([`Self::decode`]). On a pixel the develop's clamps cut nothing of, it
    /// is the develop's own value bit for bit.
    fn open(&self, data: &mut [[f32; 3]], w: usize, h: usize) {
        if let Some(lut) = &self.vignette {
            apply_radial_gain_open(data, w, h, lut);
        }
        if let Some(lut) = &self.tone {
            let (white, top) = (luma601(&[1.0; 3]), white_point(lut));
            data.par_iter_mut().for_each(|px| {
                let l_old = luma601(px);
                let l_new = if l_old <= white { sample_lut(lut, l_old) } else { l_old * top };
                scale_chroma_open(px, l_old, l_new);
            });
        }
    }

    /// `img` through [`Self::develop`], and beside it what that develop's
    /// white clamps cut: [`Self::open`]'s result less the develop's, channel
    /// by channel — `None` when they cut nothing anywhere.
    fn encode(&self, img: &DynamicImage) -> (Vec<[f32; 3]>, Option<Vec<[f32; 3]>>) {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let mut developed = pixels_f32(img);
        let mut cut = developed.clone();
        self.develop(&mut developed, w, h);
        self.open(&mut cut, w, h);
        cut.par_iter_mut().zip(developed.par_iter()).for_each(|(o, d)| *o = [o[0] - d[0], o[1] - d[1], o[2] - d[2]]);
        let any = cut.par_iter().any(|p| p.iter().any(|v| *v != 0.0));
        (developed, any.then_some(cut))
    }

    /// [`Self::open`]'s inverse, in place, stage by stage in reverse order —
    /// the tone's inverse on the luma, then the vignette gain divided out in
    /// linear light — and the result brought inside 0..1, where the pixels a
    /// develop reads are. The light the develop's clamps cut is the caller's
    /// to put back first ([`Self::restore`], [`Self::resample`]); on a frame
    /// without it this is the inverse it always was, exact wherever the
    /// develop cut nothing.
    fn decode(&self, data: &mut [[f32; 3]], w: usize, h: usize) {
        if let (Some(lut), Some(inverse)) = (&self.tone, &self.tone_inverse) {
            let (white, top) = (luma601(&[1.0; 3]), white_point(lut));
            data.par_iter_mut().for_each(|px| {
                let l_old = luma601(px);
                let l_new = if l_old <= white * top { sample_lut(inverse, l_old) } else { l_old / top };
                scale_chroma_open(px, l_old, l_new);
            });
        }
        if let Some(lut) = &self.vignette {
            let inverse: Vec<f32> = lut.iter().map(|g| 1.0 / g.max(1e-6)).collect();
            apply_radial_gain_open(data, w, h, &inverse);
        }
        data.par_iter_mut().for_each(|px| *px = px.map(|v| v.clamp(0.0, 1.0)));
    }

    /// A plate's pixels from the export's develop of the base (`developed`)
    /// and that develop before these stages, unsharpened (`bare`, the same
    /// size): the develop's frame with the light its white clamps cut put
    /// back, decoded. What the develop did after these stages — the capture
    /// sharpening — rides `developed` and is decoded with it.
    pub(super) fn restore(&self, developed: &DynamicImage, bare: &DynamicImage) -> DynamicImage {
        let (w, h) = (developed.width() as usize, developed.height() as usize);
        let mut data = pixels_f32(developed);
        if let (_, Some(cut)) = self.encode(bare) {
            data.par_iter_mut().zip(cut.par_iter()).for_each(|(d, c)| *d = [d[0] + c[0], d[1] + c[1], d[2] + c[2]]);
        }
        self.decode(&mut data, w, h);
        image_f32(data, w, h)
    }

    /// `resample` run where the export runs it — on the encoded pixels — and
    /// decoded. The light the develop's white clamps cut ([`Self::encode`])
    /// goes through the same resampler beside the frame, what the open form
    /// holds above the develop's and what it holds below apart and each
    /// scaled to its own peak, so a resampler that works in 16 bits carries
    /// both.
    fn resample(&self, img: &DynamicImage, resample: impl Fn(&DynamicImage) -> DynamicImage) -> DynamicImage {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let (developed, cut) = self.encode(img);
        let out = resample(&image_f32(developed, w, h));
        let (ow, oh) = (out.width() as usize, out.height() as usize);
        let mut data = pixels_f32(&out);
        if let Some(cut) = &cut {
            for sign in [1.0f32, -1.0] {
                let peak = cut.par_iter().map(|p| (p[0] * sign).max(p[1] * sign).max(p[2] * sign)).reduce(|| 0.0, f32::max);
                if peak <= 0.0 {
                    continue;
                }
                let part: Vec<[f32; 3]> = cut.par_iter().map(|p| p.map(|v| (v * sign).max(0.0) / peak)).collect();
                let moved = pixels_f32(&resample(&image_f32(part, w, h)));
                let k = sign * peak;
                data.par_iter_mut().zip(moved.par_iter()).for_each(|(d, m)| *d = [d[0] + k * m[0], d[1] + k * m[1], d[2] + k * m[2]]);
            }
        }
        self.decode(&mut data, ow, oh);
        image_f32(data, ow, oh)
    }
}

/// What a tone LUT maps white to: 1.0 under `build_tone_lut`'s pinned white.
fn white_point(lut: &[f32]) -> f32 {
    lut.last().copied().unwrap_or(1.0).max(1e-6)
}

/// `r` before the stages a [`BaseEncoding`] holds, and unsharpened: the recipe
/// whose develop is the pixels those stages read. It names what
/// [`BaseEncoding::of`] reads — the vignette's two gains and every input of
/// `EditRecipe::has_tone_work` — and [`source_plate`] refuses a recipe this
/// leaves an encoding on, so an input added there is not missed here in
/// silence.
fn before_pointwise(r: &EditRecipe) -> EditRecipe {
    let mut bare = r.clone();
    bare.lens_profile.vignette_on = false;
    bare.lens_vignette = 0.0;
    (bare.exposure_ev, bare.contrast, bare.highlights) = (0.0, 0.0, 0.0);
    (bare.shadows, bare.whites, bare.blacks) = (0.0, 0.0, 0.0);
    (bare.param_shadows, bare.param_darks, bare.param_lights, bare.param_highlights) = (0.0, 0.0, 0.0, 0.0);
    bare.tone_curve = Vec::new();
    bare.base_curve = Vec::new();
    bare.set_resolved("sharpening", 0.0);
    bare
}

fn pixels_f32(img: &DynamicImage) -> Vec<[f32; 3]> {
    img.to_rgb32f().as_raw().chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect()
}

fn image_f32(data: Vec<[f32; 3]>, w: usize, h: usize) -> DynamicImage {
    let flat: Vec<f32> = data.into_iter().flatten().collect();
    DynamicImage::ImageRgb32F(ImageBuffer::from_raw(w as u32, h as u32, flat).expect("plate size"))
}

fn plates() -> &'static Mutex<Vec<PlateEntry>> {
    static PLATES: std::sync::OnceLock<Mutex<Vec<PlateEntry>>> = std::sync::OnceLock::new();
    PLATES.get_or_init(|| Mutex::new(Vec::new()))
}

/// The identity of an image's pixels: where its buffer lives, and how long it is.
fn pixels_at(img: &DynamicImage) -> (usize, usize) {
    let b = img.as_bytes();
    (b.as_ptr() as usize, b.len())
}

impl Plate {
    /// Pixels in the sensor frame (a baked source, a master, a base).
    pub fn baked(image: DynamicImage) -> Self {
        Self { image: Arc::new(image), registered: false }
    }

    /// Pixels in the lens-corrected frame when `corrected` (a source the fit
    /// moved there, or a master whose marker says so,
    /// [`SourceFrame::corrected`]).
    pub fn baked_in(image: DynamicImage, corrected: bool) -> Self {
        Self::register(Arc::new(image), corrected, None)
    }

    /// A fit source's pixels in `corrected`'s frame whose resamples run after
    /// `base`'s pointwise stages ([`BaseEncoding`]): the reverse fit's plate.
    pub fn fit_plate(image: DynamicImage, corrected: bool, base: &EditRecipe) -> Self {
        Self::register(Arc::new(image), corrected, BaseEncoding::of(base))
    }

    fn register(image: Arc<DynamicImage>, corrected: bool, encoding: Option<Arc<BaseEncoding>>) -> Self {
        if !corrected && encoding.is_none() {
            return Self { image, registered: false };
        }
        plates().lock().unwrap_or_else(|p| p.into_inner()).push(PlateEntry { at: pixels_at(&image), corrected, encoding });
        Self { image, registered: true }
    }

    /// [`thumbnail`]`(src, edge, edge)` — and, when `src` is a plate's pixels,
    /// a plate in the same frame with the same encoding, thumbnailed after its
    /// base's pointwise stages as the export's pixels are averaged.
    pub fn thumbnail_of(src: &DynamicImage, edge: u32) -> Self {
        let (corrected, encoding) = plate_entry(src);
        let image = match &encoding {
            Some(e) => e.resample(src, |img| thumbnail(img, edge, edge)),
            None => thumbnail(src, edge, edge),
        };
        Self::register(Arc::new(image), corrected, encoding)
    }

    /// The plate moved into the lens-corrected frame by the engine's own
    /// geometry stage, after its base's pointwise stages, as the export moves
    /// the developed frame.
    pub fn lens_corrected(&self, geom: &crate::recipe::LensProfile, amount: f32) -> Self {
        let (_, encoding) = plate_entry(&self.image);
        let image = match &encoding {
            Some(e) => e.resample(&self.image, |img| apply_lens_geometry(img, geom, amount)),
            None => apply_lens_geometry(&self.image, geom, amount),
        };
        Self::register(Arc::new(image), true, encoding)
    }

    /// The pixels, shared (the registration lasts as long as this plate does).
    pub fn shared(&self) -> Arc<DynamicImage> {
        self.image.clone()
    }

    /// Are these pixels in the lens-corrected frame?
    pub fn corrected(&self) -> bool {
        plate_entry(&self.image).0
    }
}

/// Are `img`'s pixels a registered plate's, in the lens-corrected frame? Any
/// pixels that are not a plate's answer `false`, the sensor frame they were
/// always read in.
pub fn plate_corrected(img: &DynamicImage) -> bool {
    plate_entry(img).0
}

/// A registered plate's frame and encoding; (false, None) for any other pixels.
fn plate_entry(img: &DynamicImage) -> (bool, Option<Arc<BaseEncoding>>) {
    let plates = plates().lock().unwrap_or_else(|p| p.into_inner());
    plates
        .iter()
        .find(|e| e.at == pixels_at(img))
        .map_or((false, None), |e| (e.corrected, e.encoding.clone()))
}

impl std::ops::Deref for Plate {
    type Target = DynamicImage;
    fn deref(&self) -> &DynamicImage {
        &self.image
    }
}

impl Drop for Plate {
    fn drop(&mut self) {
        if self.registered {
            let at = pixels_at(&self.image);
            let mut plates = plates().lock().unwrap_or_else(|p| p.into_inner());
            if let Some(i) = plates.iter().position(|e| e.at == at) {
                plates.swap_remove(i);
            }
        }
    }
}

/// The reverse fit's source as a [`Plate`], in the sensor frame at `edge`: the
/// export's own develop of `base` with the lens geometry left for later — so
/// it carries the RAW's capture sharpening where the export applies it, after
/// the base's tone and at the working edge — decoded back through `base`'s
/// pointwise stages ([`BaseEncoding`]), which the preview develop re-applies,
/// with the light those stages' white clamps cut put back
/// ([`BaseEncoding::restore`]; read on a second develop, the same one before
/// those stages). Its candidates are therefore rendered on the pixels the
/// export develops, and every resample of it runs where the export's does.
pub fn source_plate(raw: &Path, base: &EditRecipe, edge: u32) -> Result<Plate> {
    let mut develop = base.clone();
    (develop.lens_profile.distortion_on, develop.lens_profile.ca_on) = (false, false);
    let colour_nr = BasePart::of(base).colour_nr();
    let render = |recipe: &EditRecipe, sink: &dyn crate::diag::Sink| {
        render_frame(raw, recipe, None, Some(edge), ExportColorSpace::Srgb, sink, colour_nr).map(|(img, _)| img)
    };
    let developed = render(&develop, crate::diag::stderr())?;
    let pixels = match BaseEncoding::of(base) {
        Some(e) => {
            let bare = before_pointwise(&develop);
            if BaseEncoding::of(&bare).is_some() {
                bail!("the fit source's base still carries a pointwise stage after they were taken out of it");
            }
            // It says nothing the develop above has not said.
            let bare = render(&bare, crate::diag::silent())?;
            if (bare.width(), bare.height()) != (developed.width(), developed.height()) {
                bail!("the fit source's two develops differ in size");
            }
            e.restore(&developed, &bare)
        }
        None => developed,
    };
    Ok(Plate::fit_plate(pixels, false, base))
}

/// [`source_pixels`] for a recipe's base part: a RAW develops through
/// [`base_develop`], a baked source is its own pixels (a profile it was
/// developed through is already in them).
pub fn source_pixels_for(path: &Path, part: &BasePart, cap: Option<u32>) -> Result<DynamicImage> {
    if crate::decode::is_raw(path) {
        return base_develop(path, part, cap);
    }
    source_pixels(path, cap)
}
