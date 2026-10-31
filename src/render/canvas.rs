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

/// One registered plate: where its pixels live.
struct PlateEntry {
    at: (usize, usize),
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
        if !corrected {
            return Self::baked(image);
        }
        Self::register(Arc::new(image))
    }

    fn register(image: Arc<DynamicImage>) -> Self {
        plates().lock().unwrap_or_else(|p| p.into_inner()).push(PlateEntry { at: pixels_at(&image) });
        Self { image, registered: true }
    }

    /// [`thumbnail`]`(src, edge, edge)` — and, when `src` is a corrected
    /// plate's pixels, a corrected plate too.
    pub fn thumbnail_of(src: &DynamicImage, edge: u32) -> Self {
        let image = Arc::new(thumbnail(src, edge, edge));
        if plate_corrected(src) { Self::register(image) } else { Self { image, registered: false } }
    }

    /// The pixels, shared (the registration lasts as long as this plate does).
    pub fn shared(&self) -> Arc<DynamicImage> {
        self.image.clone()
    }

    /// Are these pixels in the lens-corrected frame?
    pub fn corrected(&self) -> bool {
        self.registered
    }
}

/// Are `img`'s pixels a registered plate's, in the lens-corrected frame? Any
/// pixels that are not a plate's answer `false`, the sensor frame they were
/// always read in.
pub fn plate_corrected(img: &DynamicImage) -> bool {
    let plates = plates().lock().unwrap_or_else(|p| p.into_inner());
    plates.iter().any(|e| e.at == pixels_at(img))
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

/// The reverse fit's source as a [`Plate`]: [`base_develop`] of `part` at
/// `edge`, in the sensor frame.
pub fn source_plate(raw: &Path, part: &BasePart, edge: u32) -> Result<Plate> {
    Ok(Plate::baked(base_develop(raw, part, Some(edge))?))
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
