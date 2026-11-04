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
use super::develop::{apply_radial_gain, vignette_gain_lut};

/// The part of a recipe the BASE develop renders: which camera profile, and
/// which creative Look's colour table. Two recipes with equal parts develop
/// from the same base pixels; a surface holding a base compares parts to know
/// whether the base still belongs to the recipe it is about to show.
///
/// The Look is kept WITHOUT its baked half: a profile and a Look are colour
/// data, never tone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BasePart {
    camera_profile: String,
    look: Option<crate::recipe::CreativeLook>,
}

impl BasePart {
    /// The base part of `r`. A recipe that names no profile has the NEUTRAL
    /// part, whatever else its Look carries — the profile stage cannot run
    /// without a profile ([`EditRecipe::names_profile`]).
    pub fn of(r: &EditRecipe) -> Self {
        if !r.names_profile() {
            return Self::default();
        }
        Self {
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

    /// The neutral develop: no profile named.
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
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
            SourceFrame::Raw | SourceFrame::Baked(None) => Ok(Self::sensor(image)),
            SourceFrame::Baked(Some(g)) => bail!(
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
    render_frame(raw, &part.recipe(), None, max_edge, ExportColorSpace::Srgb, crate::diag::stderr())
}

/// A master's pixels for `part` (a heal, a fill, a denoise): the base part
/// developed with the RAW default sharpening a baked master is owed
/// ([`BasePart::master_recipe`]), 16-bit as the master file is.
pub fn master_develop(raw: &Path, part: &BasePart, max_edge: Option<u32>) -> Result<DynamicImage> {
    render_to_image(raw, &part.master_recipe(), None, max_edge)
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

    /// The base's pointwise stages, as the develop runs them: vignette gain,
    /// then the tone on the pixel's luma with RGB scaled by the ratio.
    fn encode(&self, img: &DynamicImage) -> DynamicImage {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let mut data = pixels_f32(img);
        if let Some(lut) = &self.vignette {
            apply_radial_gain(&mut data, w, h, lut);
        }
        if let Some(lut) = &self.tone {
            data.par_iter_mut().for_each(|px| {
                let l_old = luma601(px);
                scale_chroma_in(px, l_old, sample_lut(lut, l_old), PixelRange::Float);
            });
        }
        image_f32(data, w, h)
    }

    /// [`Self::encode`]'s inverse, stage by stage in reverse order: the tone's
    /// inverse on the luma, then the vignette gain divided out in linear
    /// light. Both run in `Float` range (R13a): a channel the base's tone
    /// carried past 1 is carried back, so a plate developed in `Float`
    /// inverts exactly.
    fn decode(&self, img: &DynamicImage) -> DynamicImage {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let mut data = pixels_f32(img);
        if let Some(lut) = &self.tone_inverse {
            data.par_iter_mut().for_each(|px| {
                let l_old = luma601(px);
                scale_chroma_in(px, l_old, sample_lut(lut, l_old), PixelRange::Float);
            });
        }
        if let Some(lut) = &self.vignette {
            let inverse: Vec<f32> = lut.iter().map(|g| 1.0 / g.max(1e-6)).collect();
            apply_radial_gain(&mut data, w, h, &inverse);
        }
        image_f32(data, w, h)
    }

    /// `resample` run where the export runs it: on the encoded pixels. A
    /// `Float` plate's encoded channels run past 1 (R13a), and the engine's
    /// geometry samplers read a frame as 16-bit (`rgb16_source`), which would
    /// clip them there; a resample is linear in the pixel values, so the
    /// frame is scaled into range by its own peak and back, and the headroom
    /// rides the resample at 16-bit precision.
    fn resample(&self, img: &DynamicImage, resample: impl FnOnce(&DynamicImage) -> DynamicImage) -> DynamicImage {
        let encoded = self.encode(img);
        let peak = encoded.as_rgb32f().map_or(1.0, |b| b.as_raw().iter().copied().fold(1.0f32, f32::max));
        if peak <= 1.0 {
            return self.decode(&resample(&encoded));
        }
        let scaled = |img: &DynamicImage, k: f32| {
            let (w, h) = (img.width() as usize, img.height() as usize);
            image_f32(pixels_f32(img).into_iter().map(|p| p.map(|v| v * k)).collect(), w, h)
        };
        self.decode(&scaled(&resample(&scaled(&encoded, 1.0 / peak)), peak))
    }
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
/// pointwise stages ([`BaseEncoding`]), which the preview develop re-applies.
/// Its candidates are therefore rendered on the pixels the export develops,
/// and every resample of it runs where the export's does.
pub fn source_plate(raw: &Path, base: &EditRecipe, edge: u32) -> Result<Plate> {
    let mut develop = base.clone();
    (develop.lens_profile.distortion_on, develop.lens_profile.ca_on) = (false, false);
    let developed =
        render_frame_in(raw, &develop, None, Some(edge), ExportColorSpace::Srgb, crate::diag::stderr(), PixelRange::Float)?;
    let pixels = match BaseEncoding::of(base) {
        Some(e) => e.decode(&developed),
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
