//! The canvas's base develop (v1.17.0): the ONE develop every surface that
//! shows, fits or bakes a photograph starts from.
//!
//! A recipe renders in two halves. The PROFILE STAGE — the camera profile's
//! matrices, hue/sat maps, look table, tone and baseline exposure, with the
//! creative Look's colour table on top of the profile's own (`profile::Stage`)
//! — runs INSIDE the RAW develop, on camera-space pixels, before the white
//! balance. Everything after it (white balance, tone, colour, detail, masks,
//! geometry) runs on the developed frame, and the preview develop
//! ([`develop_preview`] and its forms) runs exactly that second half on pixels
//! it is handed. So pixels handed to a preview develop must already carry the
//! first half, or the canvas shows a different picture from the export: they
//! are the base develop of the recipe's [`BasePart`].
//!
//! Until v1.17.0 every base was the NEUTRAL develop, and that was right while
//! no fresh recipe named a profile. A fresh open now starts where Lightroom
//! starts — 「Adobe Standard」 with the 「Adobe Color」 Look ([`canvas_recipe`])
//! — so the GUI and web canvases, the retouch / denoise / stack / fill masters
//! and the reverse fit's source all develop through this module.
//!
//! The profile stage also takes the white balance (v1.17.0): Camera Raw applies
//! a Custom white's multipliers to the camera triple and blends the profile's
//! matrices and hue/sat maps at that white's temperature, BEFORE the tables
//! (two synthetic-DNG experiments, `docs/ARCHITECTURE.md`). So a base that is
//! to show a moved white must be developed at it, which a frame already past
//! the profile cannot be. A [`CameraBase`] keeps the RAW's camera-native frame
//! at the working edge and re-runs the profile stage on it — once per white,
//! not once per decode — and a canvas's [`BasePart::of_canvas`] names the white
//! with the profile. The reverse fit renders its candidates on a [`Plate`],
//! whose preview develop does the same for every white a candidate names.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::*;

/// The camera profile a fresh develop names (v1.17.0) — Lightroom's own.
pub const CANVAS_PROFILE: &str = "Adobe Standard";

/// The part of a recipe the BASE develop renders: which camera profile, and
/// which creative Look's colour table. Two recipes with equal parts develop
/// from the same base pixels; a surface holding a base compares parts to know
/// whether the base still belongs to the recipe it is about to show.
///
/// The Look is kept WITHOUT its baked half (tone curves, baked sliders, the
/// monochrome switch): those are tone and colour work the preview develop
/// renders from the full recipe, and baking them into the base would apply
/// them twice.
///
/// The lens profile's VIGNETTING belongs here too (v1.17.0): under a camera
/// profile it is applied on the sensor's linear light before the profile
/// stage (`render::source`), so a base without it could not be corrected
/// later at the right place. Its knots and strength, empty when the recipe
/// renders none.
///
/// So does the lens profile's GEOMETRY when the camera stage resamples it
/// (v1.17.0, [`lens_in_camera`]): the composed distortion and CA, with the
/// vignetting left to the two fields above. A base of such a part is in the
/// lens-CORRECTED frame, and switching the profile's corrections re-develops
/// it as switching the camera profile does. So is a master baked from it
/// ([`BasePart::master_recipe`]), which carries what it baked in its own file
/// ([`BakedGeometry`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BasePart {
    camera_profile: String,
    look: Option<crate::recipe::CreativeLook>,
    vignette: Vec<f32>,
    vignette_scale: f32,
    lens: Option<Box<crate::recipe::LensProfile>>,
    /// The lens profile the vignetting and geometry above come from
    /// ([`BakedGeometry::lens_profile`]); empty when there are neither.
    lens_id: String,
    /// The white the base is developed at when it is not As Shot — only on a
    /// canvas's part ([`Self::of_canvas`]); `None` is As Shot.
    white: Option<BaseWhite>,
}

/// A recipe's white balance as the profile stage reads it: the Temperature
/// and Tint, and the as-shot pair they are measured from
/// ([`super::wb::camera_raw_white`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct BaseWhite {
    temperature_k: Option<f32>,
    tint: f32,
    as_shot_k: Option<f32>,
    as_shot_tint: Option<f32>,
}

impl BasePart {
    /// The base part of `r`. A recipe that names no profile has the NEUTRAL
    /// part, whatever else its Look carries — the profile stage cannot run
    /// without a profile ([`EditRecipe::names_profile`]).
    pub fn of(r: &EditRecipe) -> Self {
        if !r.names_profile() {
            return Self::default();
        }
        let vignette =
            if r.lens_profile.vignette_active() { r.lens_profile.vignette.clone() } else { Vec::new() };
        let vignette_scale = if vignette.is_empty() { 0.0 } else { r.lens_profile_vignetting_scale };
        let lens = lens_in_camera(r).then(|| {
            Box::new(crate::recipe::LensProfile {
                vignette_on: false,
                vignette: Vec::new(),
                ..geometry_profile(r).into_owned()
            })
        });
        let lens_id = if lens.is_none() && vignette.is_empty() {
            String::new()
        } else {
            match &r.lens_profile.adobe {
                Some(a) => format!("adobe:{}", a.name),
                None => "camera".to_string(),
            }
        };
        Self {
            camera_profile: r.camera_profile.clone(),
            look: r.look.as_ref().map(|l| crate::recipe::CreativeLook {
                name: l.name.clone(),
                amount: l.amount,
                base_profile: l.base_profile.clone(),
                table: l.table.clone(),
                ..Default::default()
            }),
            vignette,
            vignette_scale,
            lens,
            lens_id,
            white: None,
        }
    }

    /// The base part of `r` as a CANVAS shows it: [`Self::of`] and, when `r`
    /// moves its white from As Shot on Camera Raw's law, that white too —
    /// Camera Raw renders a Custom white inside the profile stage, so the
    /// base is developed at it and the preview develop, handed such a base by
    /// a RAW source's canvas, does not apply the white a second time
    /// ([`develop_preview_film`]'s `raw_source`).
    ///
    /// A MASTER (a heal, denoise, stack or fill bakes the base) keeps
    /// [`Self::of`]: it is rendered later under whatever white the recipe has
    /// by then, through the baked door, which applies the white itself.
    pub fn of_canvas(r: &EditRecipe) -> Self {
        let mut part = Self::of(r);
        if !part.is_neutral() && super::wb::camera_raw_white(r).is_some() {
            part.white = Some(BaseWhite {
                temperature_k: r.temperature_k,
                tint: r.tint,
                as_shot_k: r.as_shot_k,
                as_shot_tint: r.as_shot_tint,
            });
        }
        part
    }

    /// Is this base developed at a white other than As Shot?
    pub fn carries_white(&self) -> bool {
        self.white.is_some()
    }

    /// This part at As Shot.
    pub fn at_as_shot(&self) -> Self {
        Self { white: None, ..self.clone() }
    }

    /// This part with the lens geometry left to the tail: a base in the SENSOR
    /// frame, for the surface that still works there (the reverse fit's
    /// source).
    pub fn without_lens(&self) -> Self {
        let lens_id = if self.vignette.is_empty() { String::new() } else { self.lens_id.clone() };
        Self { lens: None, lens_id, ..self.clone() }
    }

    /// The composed lens geometry a base of this part is resampled through
    /// (vignetting cleared), when the camera stage resamples one.
    pub fn lens(&self) -> Option<&crate::recipe::LensProfile> {
        self.lens.as_deref()
    }

    /// The profile vignetting knots a base of this part carries.
    pub fn vignette(&self) -> &[f32] {
        &self.vignette
    }

    /// The vignetting strength those knots are applied at.
    pub fn vignette_scale(&self) -> f32 {
        self.vignette_scale
    }

    /// [`BakedGeometry::lens_profile`] for this part.
    pub fn lens_id(&self) -> &str {
        &self.lens_id
    }

    /// The lens geometry a master of this part has in its pixels.
    pub fn baked_geometry(&self) -> Option<BakedGeometry> {
        BakedGeometry::of_part(self)
    }

    /// Is a base of this part in the lens-corrected frame?
    pub fn corrects_lens(&self) -> bool {
        self.lens.is_some()
    }

    /// The neutral develop: no profile named.
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// The recipe that develops exactly this part and nothing else — with
    /// Sharpening at an explicit 0: the RAW default capture sharpening belongs
    /// to the display and the export, which run it over the base (a base
    /// sharpened too was sharpened twice, and the pass clipped the over-range
    /// light the float base keeps).
    ///
    /// The part's lens geometry too: the composed profile as it is, so
    /// `geometry_profile` reads it back unchanged (no manual pair,
    /// Distortion strength 100).
    pub fn recipe(&self) -> EditRecipe {
        let mut r = self.master_recipe();
        r.set_resolved("sharpening", 0.0);
        if let Some(w) = self.white {
            (r.temperature_k, r.tint, r.as_shot_k, r.as_shot_tint) =
                (w.temperature_k, w.tint, w.as_shot_k, w.as_shot_tint);
        }
        r
    }

    /// The recipe a MASTER of this part is baked through ([`master_develop`]):
    /// [`BasePart::recipe`] with the RAW default capture sharpening kept. A
    /// master is a baked file, and a baked file is rendered with no default
    /// sharpening (`EditRecipe::capture_sharpening`), so the master carries the
    /// one sharpening its RAW is owed. In the frame the base is in: with the
    /// part's lens geometry, a master is lens-corrected, and its file says so
    /// ([`BasePart::baked_geometry`], [`stamp_baked_geometry`]). At As Shot,
    /// whatever white the part carries: a master is rendered later under the
    /// white its recipe has by then, through the baked door, which applies it
    /// ([`BasePart::of_canvas`]).
    pub fn master_recipe(&self) -> EditRecipe {
        let dflt = EditRecipe::default();
        let mut lens_profile = crate::recipe::LensProfile {
            vignette_on: !self.vignette.is_empty(),
            vignette: self.vignette.clone(),
            ..Default::default()
        };
        if let Some(lens) = &self.lens {
            lens_profile = crate::recipe::LensProfile {
                vignette_on: lens_profile.vignette_on,
                vignette: std::mem::take(&mut lens_profile.vignette),
                ..(**lens).clone()
            };
        }
        EditRecipe {
            camera_profile: self.camera_profile.clone(),
            look: self.look.clone(),
            lens_profile,
            lens_profile_vignetting_scale: if self.vignette.is_empty() {
                dflt.lens_profile_vignetting_scale
            } else {
                self.vignette_scale
            },
            ..dflt
        }
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
        let wanted = part.baked_geometry();
        let baked = match frame {
            SourceFrame::Raw => wanted.clone(),
            SourceFrame::Baked(None) => return Ok(Self::sensor(image)),
            SourceFrame::Baked(Some(g)) => {
                if wanted.as_ref() != Some(g) {
                    let w = wanted.map_or_else(|| "no lens profile".to_string(), |w| w.to_string());
                    bail!(
                        "this master has {g} in its pixels, but the develop asks for {w} — bake \
                         the master again from the RAW, or set the lens corrections back"
                    );
                }
                Some(g.clone())
            }
        };
        let frame_lens = baked
            .as_ref()
            .filter(|g| g.corrects_frame())
            .and_then(|_| part.lens().cloned());
        Ok(Self { image, baked, frame_lens })
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
/// [`render_to_image`]). The neutral part is the neutral develop every base
/// was before v1.17.0, unsharpened ([`BasePart::recipe`]); a part at As Shot
/// is the RAW render of its recipe at that edge. A part that names a profile
/// is a 32-bit float frame that keeps its over-range light
/// ([`OverRange::Keep`]), so the canvas's Exposure brings it back as the
/// export's does; a surface that converts it to 8 or 16 bits clips it there.
///
/// A part that names a profile is developed from the photo's [`CameraBase`]
/// for it (kept for the process, so a canvas moving its white re-runs the
/// profile stage at the working edge, not the decode). Full resolution
/// (`None`), and a sensor the camera base does not take, develop through
/// [`render_base`].
pub fn base_develop(raw: &Path, part: &BasePart, max_edge: Option<u32>) -> Result<DynamicImage> {
    if let Some(camera) = camera_base_of(raw, part, max_edge)?
        && let Some(img) = camera.develop(part, &[])?
    {
        return Ok((*img).clone());
    }
    render_base(raw, part, max_edge)
}

/// [`base_develop`] through the render itself, with no camera base: the part's
/// recipe rendered at `max_edge`, a float frame unless the part is neutral.
fn render_base(raw: &Path, part: &BasePart, max_edge: Option<u32>) -> Result<DynamicImage> {
    let over_range = if part.is_neutral() { OverRange::Clip } else { OverRange::Keep };
    render_frame(raw, &part.recipe(), None, max_edge, ExportColorSpace::Srgb, crate::diag::stderr(), over_range)
}

/// A master's pixels for `part` (a heal, a fill, a denoise): the base part
/// developed with the RAW default sharpening a baked master is owed
/// ([`BasePart::master_recipe`]), 16-bit as the master file is, in the frame
/// [`BasePart::baked_geometry`] names.
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

/// The camera base [`base_develop`] develops `part` from at `max_edge`, or
/// `None` when the part is neutral, the edge is full resolution or the file
/// is no camera RAW.
fn camera_base_of(raw: &Path, part: &BasePart, max_edge: Option<u32>) -> Result<Option<Arc<CameraBase>>> {
    match max_edge {
        Some(edge) if !part.is_neutral() && crate::decode::is_raw(raw) => CameraBase::shared(raw, part, edge),
        _ => Ok(None),
    }
}

/// A RAW's frame at a working edge for one camera profile (v1.17.0), kept so
/// the profile stage can run again at another white without decoding the
/// file again.
///
/// It holds two frames of the same pixels. The CAMERA frame: demosaiced,
/// cropped to the default crop, through the camera stage's lens work (the
/// vignetting gain and, when the stage resamples it, the lens geometry —
/// [`source::camera_stage_lens`]), oriented and capped (an area average in
/// linear light), with no white balance and no profile — the input of the
/// profile stage. And the As Shot frame: the profile stage at As Shot run at
/// FULL resolution and then capped — exactly what the render makes of the
/// frame, which caps after the profile stage.
///
/// The order matters because the profile stage is not linear (its tables,
/// exposure ramp and tone curve): staged on the capped camera frame it reads
/// each block's average where the render reads the average of the staged
/// pixels, which on p39 at 1000 px left the base 0.42 ΔE76 off the render on
/// average (median 0, 99th percentile 5.4 — edges and highlights; 0.26 at
/// 2048 px). So a white is developed as the As Shot frame plus what the white
/// changes on the capped camera frame:
/// `staged(W) − staged(As Shot) + exact(As Shot)`. The block-average error
/// of the two stagings cancels to the extent it does not depend on the white,
/// and at As Shot the base IS the render's, byte for byte.
///
/// Only the develop that owns its calibration takes one: a 2×2 Bayer sensor
/// with a colour matrix, developed through a profile this machine renders.
/// Everything else (the neutral develop, which is rawler's own; a sensor this
/// build demosaics itself; a profile not installed) answers `None` and keeps
/// [`render_to_image`].
pub struct CameraBase {
    path: std::path::PathBuf,
    /// The part this base renders, at As Shot.
    part: BasePart,
    meta: camera::CameraMeta,
    xyz2cam: [[f32; 3]; 3],
    /// The camera start of the stage at As Shot ([`EditRecipe::camera_start`]).
    start: crate::recipe::CameraStart,
    /// The camera stage resampled the lens: the frames are lens-corrected.
    corrected: bool,
    /// The camera stage keeps the over-range light ([`calibrate_camera_buffer`]).
    defer_over_range: bool,
    w: usize,
    h: usize,
    film: FilmScale,
    /// The camera frame at the edge (linear camera RGB).
    camera: Vec<[f32; 3]>,
    /// The profile stage at As Shot, staged at full resolution and capped.
    exact: Vec<[f32; 3]>,
    /// The profile stage at As Shot, staged on [`Self::camera`].
    staged: Vec<[f32; 3]>,
    /// Recently developed bases, keyed by part and by the chain of thumbnail
    /// edges a [`Plate`] took from it — newest last.
    developed: Mutex<VecDeque<(DevelopedKey, Arc<DynamicImage>)>>,
}

type DevelopedKey = (BasePart, Vec<u32>);

/// How many bytes of developed bases one [`CameraBase`] keeps.
const DEVELOPED_BUDGET_BYTES: usize = 96 << 20;

/// The largest camera base the process keeps (pixels at the edge): three f32
/// frames of 16 MP are 576 MB, and a full-resolution 61 MP frame is left to
/// [`render_to_image`], which streams it.
const SHARED_MAX_PIXELS: f64 = 16.0e6;

impl CameraBase {
    /// Decode `raw` for `part` (its white is ignored: the base is As Shot's)
    /// at `max_edge`. `Ok(None)`: this develop is not the camera base's (see
    /// the type).
    pub fn decode(raw: &Path, part: &BasePart, max_edge: u32) -> Result<Option<Self>> {
        let part = part.at_as_shot();
        if part.is_neutral() {
            return Ok(None);
        }
        let diag = crate::diag::Diag::about(crate::diag::stderr(), raw);
        let (mut rawimage, orientation, daylight) = source::decode_sensor(raw, 0, None, &diag)?;
        if source::non_bayer_rgb_cfa(&rawimage).is_some() {
            return Ok(None);
        }
        let meta = camera::CameraMeta::of(&rawimage, daylight);
        let Ok(xyz2cam) = meta.matrix.clone() else { return Ok(None) };
        // The render's own order: the stage resolved, the calibration
        // validated, the hot sites mapped before anything reads the mosaic.
        let recipe = part.recipe();
        let Some((stage, None)) = camera::resolve_camera_profile(&meta, &recipe, ExportColorSpace::Srgb, &diag)
        else {
            return Ok(None);
        };
        let (lens_gain, lens, defer_over_range) = source::camera_stage_lens(&recipe);
        validate_calibration(&xyz2cam, rawimage.wb_coeffs, ExportColorSpace::Srgb, raw)?;
        let hot_sites = crate::denoise::hot_pixels::map_for(&mut rawimage)?;
        if hot_sites > 0 {
            println!("mapped {hot_sites} isolated hot pixels stronger than 20 local sigma");
        }
        let mut dev = RawDevelop::default();
        dev.steps.retain(|s| {
            !matches!(s, ProcessingStep::WhiteBalance | ProcessingStep::Calibrate | ProcessingStep::SRgb)
        });
        // The render's first half: the demosaic and the vignetting gain on
        // linear camera light, then the lens resample — the camera stage's
        // part of `source::settle_camera_buffer` before its calibrate, which
        // runs below once per white.
        let (data, w, h) = source::develop_raw_linear(&rawimage, &dev, None, true, lens_gain.as_deref())?;
        drop(rawimage);
        let data = match lens.as_deref() {
            Some(p) => apply_lens_geometry_linear(data, w, h, p),
            None => data,
        };
        // Oriented first: the stage is per pixel, so staging the oriented
        // frame is staging the frame and orienting it, as the render does.
        let (data, w, h) = orient_f32(data, w, h, orientation);
        let film_short = u32::try_from(w.min(h)).ok();
        let as_shot = normalise_wb(meta.wb_coeffs);
        let mut full = data.clone();
        calibrate_camera_buffer(&mut full, &xyz2cam, as_shot, ExportColorSpace::Srgb, Some(&stage), defer_over_range);
        let (exact, _, _) = downscale_f32(full, w, h, max_edge);
        let (camera, w, h) = downscale_f32(data, w, h, max_edge);
        let mut staged = camera.clone();
        calibrate_camera_buffer(&mut staged, &xyz2cam, as_shot, ExportColorSpace::Srgb, Some(&stage), defer_over_range);
        Ok(Some(Self {
            path: raw.to_path_buf(),
            part,
            meta,
            xyz2cam,
            start: stage.start(),
            corrected: lens.is_some(),
            defer_over_range,
            w,
            h,
            film: FilmScale::of(film_short, w, h),
            camera,
            exact,
            staged,
            developed: Mutex::new(VecDeque::new()),
        }))
    }

    /// [`Self::decode`], kept for the process: the last two camera bases
    /// decoded (a canvas and a fit at once), keyed by the file's path, size
    /// and modification time, the part at As Shot and the edge. A frame over
    /// [`SHARED_MAX_PIXELS`] is neither built nor kept — `Ok(None)`.
    pub fn shared(raw: &Path, part: &BasePart, max_edge: u32) -> Result<Option<Arc<Self>>> {
        type Key = (std::path::PathBuf, u64, Option<std::time::SystemTime>, BasePart, u32);
        type Kept = Mutex<VecDeque<(Key, Option<Arc<CameraBase>>)>>;
        static KEPT: std::sync::OnceLock<Kept> = std::sync::OnceLock::new();
        let md = std::fs::metadata(raw).with_context(|| format!("stat {}", raw.display()))?;
        let key: Key = (raw.to_path_buf(), md.len(), md.modified().ok(), part.at_as_shot(), max_edge);
        let kept = KEPT.get_or_init(|| Mutex::new(VecDeque::new()));
        if let Some((_, hit)) = kept.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(k, _)| *k == key) {
            return Ok(hit.clone());
        }
        // The area-average cap leaves at most this edge.
        if let Ok((w, h)) = crate::decode::frame_size(raw) {
            let (w, h) = (w as f64, h as f64);
            let scale = (f64::from(max_edge) / w.max(h)).min(1.0);
            if (w * scale) * (h * scale) > SHARED_MAX_PIXELS {
                return Ok(None);
            }
        }
        // Decoded outside the lock: a second photo need not wait for this one.
        let base = Self::decode(raw, part, max_edge)?.map(Arc::new);
        let mut kept = kept.lock().unwrap_or_else(|p| p.into_inner());
        kept.retain(|(k, _)| *k != key);
        kept.push_back((key, base.clone()));
        while kept.len() > 2 {
            kept.pop_front();
        }
        Ok(base)
    }

    /// The base `part` develops to — the profile stage at the part's white
    /// (see the type for how), then the neutral rest of the RAW develop
    /// ([`source::finish_develop`]) — thumbnailed down each edge of `thumbs`
    /// in turn (a [`Plate`]'s chain). `Ok(None)` when `part` is not this
    /// base's profile, or its white cannot be staged.
    fn develop(&self, part: &BasePart, thumbs: &[u32]) -> Result<Option<Arc<DynamicImage>>> {
        if part.at_as_shot() != self.part {
            return Ok(None);
        }
        let key: DevelopedKey = (part.clone(), thumbs.to_vec());
        if let Some((_, hit)) =
            self.developed.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(k, _)| *k == key)
        {
            return Ok(Some(hit.clone()));
        }
        let img = match thumbs.split_last() {
            Some((edge, rest)) => match self.develop(part, rest)? {
                Some(parent) => thumbnail(&parent, *edge, *edge),
                None => return Ok(None),
            },
            None => {
                let validated = crate::recipe::ValidatedRecipe::new(&part.recipe());
                let recipe = &*validated;
                // Said when the base was decoded; not again for every white.
                let diag = crate::diag::Diag::about(crate::diag::silent(), &self.path);
                let (data, white_in_camera, start) = if part.carries_white() {
                    let Some((stage, white)) =
                        camera::resolve_camera_profile(&self.meta, recipe, ExportColorSpace::Srgb, &diag)
                    else {
                        return Ok(None);
                    };
                    match white {
                        // The white staged on the camera frame, carried onto
                        // the As Shot frame as the change it makes.
                        Some(wb) => {
                            let mut moved = self.camera.clone();
                            calibrate_camera_buffer(
                                &mut moved,
                                &self.xyz2cam,
                                wb,
                                ExportColorSpace::Srgb,
                                Some(&stage),
                                self.defer_over_range,
                            );
                            moved
                                .par_iter_mut()
                                .zip(self.exact.par_iter().zip(self.staged.par_iter()))
                                .for_each(|(m, (e, s))| *m = std::array::from_fn(|c| m[c] - s[c] + e[c]));
                            (moved, true, stage.start())
                        }
                        // A stage that does not own the camera leaves the
                        // white to the adaptation after it, on the As Shot
                        // frame — the render's own fallback.
                        None => (self.exact.clone(), false, stage.start()),
                    }
                } else {
                    (self.exact.clone(), false, self.start)
                };
                // The render's restart: the develop takes the light back under
                // the camera start the stage rendered at (`render_frame`).
                let restarted = EditRecipe { camera_start: Some(start), ..recipe.clone() };
                let rasters = load_mask_raster_snapshot(&restarted, &diag)?;
                source::finish_develop(
                    data,
                    self.w,
                    self.h,
                    self.film,
                    &restarted,
                    &rasters,
                    white_in_camera,
                    ExportColorSpace::Srgb,
                    self.corrected,
                    OverRange::Keep,
                )?
            }
        };
        let img = Arc::new(img);
        let mut kept = self.developed.lock().unwrap_or_else(|p| p.into_inner());
        kept.retain(|(k, _)| *k != key);
        kept.push_back((key, img.clone()));
        while kept.len() > 1
            && kept.iter().map(|(_, i)| i.as_bytes().len()).sum::<usize>() > DEVELOPED_BUDGET_BYTES
        {
            kept.pop_front();
        }
        Ok(Some(img))
    }
}

/// Pixels the reverse fit renders its candidates on (v1.17.0): an image that
/// may know the [`CameraBase`] it was developed from.
///
/// The fit renders every candidate with the preview develop over its source,
/// and a candidate that moves the white names a white the profile stage must
/// render ([`BasePart::of_canvas`]) — over a frame already past the profile,
/// the preview develop could only adapt it afterwards, which is not what the
/// export (or Camera Raw) does. A plate developed from a camera base is
/// registered for as long as it lives: the preview develop handed exactly its
/// pixels develops the candidate's white from the camera frame instead, along
/// the same chain of thumbnails the plate took, and leaves the white out of
/// its own pass. Any other pixels — a copy, a crop — are what they always were.
///
/// A plate also says which FRAME its pixels are in (v1.17.0): the reverse fit
/// works in the lens-corrected frame when the calibration's camera stage
/// resamples the lens ([`lens_in_camera`]), and a master can be baked in it
/// ([`SourceFrame`]). The preview develop handed a corrected plate's pixels
/// reads its masks in that frame ([`MaskFrame::Corrected`]), as the canvas
/// over the same base does — so a plate of a corrected baked source is
/// registered too, with no camera behind it.
pub struct Plate {
    image: Arc<DynamicImage>,
    registered: bool,
    camera: bool,
}

struct PlateEntry {
    at: (usize, usize),
    camera: Option<Arc<CameraBase>>,
    part: BasePart,
    thumbs: Vec<u32>,
    corrected: bool,
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
    /// Pixels in the sensor frame with no camera base behind them (a baked
    /// source, a master).
    pub fn baked(image: DynamicImage) -> Self {
        Self { image: Arc::new(image), registered: false, camera: false }
    }

    /// Pixels with no camera base behind them, in the lens-corrected frame
    /// when `corrected` (a master whose marker says so, [`SourceFrame::corrected`]).
    pub fn baked_in(image: DynamicImage, corrected: bool) -> Self {
        if !corrected {
            return Self::baked(image);
        }
        Self::register(Arc::new(image), None, BasePart::default(), Vec::new(), true)
    }

    fn register(
        image: Arc<DynamicImage>,
        camera: Option<Arc<CameraBase>>,
        part: BasePart,
        thumbs: Vec<u32>,
        corrected: bool,
    ) -> Self {
        let has_camera = camera.is_some();
        plates().lock().unwrap_or_else(|p| p.into_inner()).push(PlateEntry {
            at: pixels_at(&image),
            camera,
            part,
            thumbs,
            corrected,
        });
        Self { image, registered: true, camera: has_camera }
    }

    /// [`thumbnail`]`(src, edge, edge)` — and, when `src` is a registered
    /// plate's pixels, a plate that knows its camera base and the chain that
    /// made it.
    pub fn thumbnail_of(src: &DynamicImage, edge: u32) -> Self {
        let image = Arc::new(thumbnail(src, edge, edge));
        let parent = {
            let plates = plates().lock().unwrap_or_else(|p| p.into_inner());
            plates.iter().find(|e| e.at == pixels_at(src)).map(|e| {
                let mut thumbs = e.thumbs.clone();
                thumbs.push(edge);
                (e.camera.clone(), e.part.clone(), thumbs, e.corrected)
            })
        };
        match parent {
            Some((camera, part, thumbs, corrected)) => Self::register(image, camera, part, thumbs, corrected),
            None => Self { image, registered: false, camera: false },
        }
    }

    /// The pixels, shared (the registration lasts as long as this plate does).
    pub fn shared(&self) -> Arc<DynamicImage> {
        self.image.clone()
    }

    /// Does a camera base stand behind these pixels?
    pub fn has_camera(&self) -> bool {
        self.camera
    }

    /// Are these pixels in the lens-corrected frame?
    pub fn corrected(&self) -> bool {
        plate_corrected(&self.image)
    }
}

/// Are `img`'s pixels a registered plate's in the lens-corrected frame? Any
/// pixels that are not a plate's answer `false`, the sensor frame they were
/// always read in.
pub fn plate_corrected(img: &DynamicImage) -> bool {
    let plates = plates().lock().unwrap_or_else(|p| p.into_inner());
    plates.iter().find(|e| e.at == pixels_at(img)).is_some_and(|e| e.corrected)
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
/// `edge`, registered with its camera base when the camera base takes it.
pub fn source_plate(raw: &Path, part: &BasePart, edge: u32) -> Result<Plate> {
    if let Some(camera) = camera_base_of(raw, part, Some(edge))?
        && let Some(img) = camera.develop(part, &[])?
    {
        return Ok(Plate::register(img, Some(camera), part.clone(), Vec::new(), part.corrects_lens()));
    }
    Ok(Plate::baked_in(render_base(raw, part, Some(edge))?, part.corrects_lens()))
}

/// What the preview develop renders `recipe` from when it is handed
/// `preview`: `Some((pixels, white_in_pixels, corrected))` when `preview` is a
/// registered plate's — `None` for the plate itself when the recipe's canvas
/// part is the plate's own (or no camera base stands behind it), else that
/// part developed from the camera base along the plate's chain, with whether
/// those pixels are lens-corrected — and `None` for any other pixels.
pub(super) fn plate_pixels(
    preview: &DynamicImage,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
) -> Option<(Option<Arc<DynamicImage>>, bool, bool)> {
    let (camera, own, thumbs, corrected) = {
        let plates = plates().lock().unwrap_or_else(|p| p.into_inner());
        let e = plates.iter().find(|e| e.at == pixels_at(preview))?;
        (e.camera.clone(), e.part.clone(), e.thumbs.clone(), e.corrected)
    };
    let Some(camera) = camera else { return Some((None, false, corrected)) };
    // In the plate's own frame: a source developed in the sensor frame (a
    // calibration whose lens the tail resamples) re-develops a candidate's
    // white in that frame too; a source in the corrected frame keeps the
    // candidate's own lens geometry, so a candidate that switches the profile
    // off is developed in the sensor frame and read there.
    let want = BasePart::of_canvas(recipe);
    let want = if own.corrects_lens() { want } else { want.without_lens() };
    if want == own {
        return Some((None, own.carries_white(), corrected));
    }
    match camera.develop(&want, &thumbs) {
        Ok(Some(img)) => Some((Some(img), want.carries_white(), want.corrects_lens())),
        // The plate's own pixels, the white adapted after the profile — in
        // the frame they are in.
        Ok(None) => Some((None, false, corrected)),
        Err(e) => {
            diag.warn(format!(
                "the camera stage could not re-develop this source ({e:#}); its white balance is \
                 adapted after the profile instead"
            ));
            Some((None, false, corrected))
        }
    }
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

/// The recipe a fresh develop of a RAW starts from (v1.17.0): 「Adobe
/// Standard」 with the 「Adobe Color」 Look, every slider at zero — what an
/// all-zero Lightroom sidecar says ([`crate::xmp::canvas_look`]).
pub fn canvas_recipe() -> EditRecipe {
    EditRecipe {
        camera_profile: CANVAS_PROFILE.to_string(),
        look: Some(crate::xmp::canvas_look()),
        ..Default::default()
    }
}

/// Can this machine render the canvas for `raw`: is it a camera RAW, and does
/// the Camera Raw install hold 「Adobe Standard」 for its body and the 「Adobe
/// Color」 colour table? `Err` says why not, as a phrase.
///
/// Answered once per body for the process (the install does not change under
/// a running process — [`crate::dcp`]'s stance), after a metadata-only read of
/// the file's make and model.
pub fn canvas_renders(raw: &Path) -> std::result::Result<(), String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Cache = Mutex<HashMap<(String, String), std::result::Result<(), String>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    if !crate::decode::is_raw(raw) {
        return Err("it is not a camera RAW".to_string());
    }
    let Some(body) = camera_body(raw) else {
        return Err("its camera make and model could not be read".to_string());
    };
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().ok().and_then(|m| m.get(&body).cloned()) {
        return hit;
    }
    let look = crate::xmp::canvas_look();
    let answer = crate::dcp::find(&body.0, &body.1, CANVAS_PROFILE)
        .map(|_| ())
        .map_err(|e| format!("Camera Raw's \"{CANVAS_PROFILE}\" profile for this camera: {e}"))
        .and_then(|()| {
            crate::dcp::find_look_table(&look.table)
                .map(|_| ())
                .map_err(|e| format!("the \"{}\" colour table: {e}", look.name))
        });
    if let Ok(mut m) = cache.lock() {
        m.insert(body, answer.clone());
    }
    answer
}

/// The RAW's camera make and model, from a metadata-only read (no sensor
/// decode) — the two names [`crate::dcp::find`] matches a profile file on.
fn camera_body(raw: &Path) -> Option<(String, String)> {
    crate::decode::guard_tiff_chain(raw).ok()?;
    crate::decode::guard_raw_plane_extent(raw).ok()?;
    let src = RawSource::new(raw).ok()?;
    let decoder = crate::decode::decoder_for(raw, &src).ok()?;
    let image = crate::decode::guard_parser_panic(raw, "camera body", || {
        decoder
            .raw_image(&src, &RawDecodeParams { image_index: 0 }, true)
            .map_err(|e| anyhow!("raw_image(dummy): {e}"))
    })
    .ok()?;
    Some((image.camera.make.clone(), image.camera.model.clone()))
}
