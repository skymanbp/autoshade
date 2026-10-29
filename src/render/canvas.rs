//! The canvas's base develop (v1.17.0): the ONE develop every surface that
//! shows, fits or bakes a photograph starts from.
//!
//! A recipe renders in two halves. The PROFILE STAGE — the camera profile's
//! matrices, hue/sat maps, look table, tone and baseline exposure, with the
//! creative Look's colour table in place of the profile's own (`profile::Stage`)
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BasePart {
    camera_profile: String,
    look: Option<crate::recipe::CreativeLook>,
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
        Self {
            camera_profile: r.camera_profile.clone(),
            look: r.look.as_ref().map(|l| crate::recipe::CreativeLook {
                name: l.name.clone(),
                amount: l.amount,
                base_profile: l.base_profile.clone(),
                table: l.table.clone(),
                ..Default::default()
            }),
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

    /// The neutral develop: no profile named.
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// The recipe that develops exactly this part and nothing else.
    pub fn recipe(&self) -> EditRecipe {
        let mut r = EditRecipe {
            camera_profile: self.camera_profile.clone(),
            look: self.look.clone(),
            ..Default::default()
        };
        if let Some(w) = self.white {
            (r.temperature_k, r.tint, r.as_shot_k, r.as_shot_tint) =
                (w.temperature_k, w.tint, w.as_shot_k, w.as_shot_tint);
        }
        r
    }
}

/// Develop `raw`'s base for `part` — the profile stage and the neutral rest —
/// at `max_edge` (the cap runs right after the profile stage, as in
/// [`render_to_image`]). The neutral part is byte-identical to the neutral
/// develop every base was before v1.17.0, and a part at As Shot to the RAW
/// render of its recipe at that edge.
///
/// A part that names a profile is developed from the photo's [`CameraBase`]
/// for it (kept for the process, so a canvas moving its white re-runs the
/// profile stage at the working edge, not the decode). Full resolution
/// (`None`), and a sensor the camera base does not take, develop through
/// [`render_to_image`].
pub fn base_develop(raw: &Path, part: &BasePart, max_edge: Option<u32>) -> Result<DynamicImage> {
    if let Some(camera) = camera_base_of(raw, part, max_edge)?
        && let Some(img) = camera.develop(part, &[])?
    {
        return Ok((*img).clone());
    }
    render_to_image(raw, &part.recipe(), None, max_edge)
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
/// cropped to the default crop, oriented and capped (an area average in
/// linear light), with no white balance and no profile — the input of the
/// profile stage. And the As Shot frame: the profile stage at As Shot run at
/// FULL resolution and then capped — exactly what [`render_to_image`] makes
/// of the frame, which caps after the profile stage.
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
        let (mut rawimage, orientation, daylight) = source::decode_sensor(raw, 0, None)?;
        if source::non_bayer_rgb_cfa(&rawimage).is_some() {
            return Ok(None);
        }
        let meta = camera::CameraMeta::of(&rawimage, daylight);
        let Ok(xyz2cam) = meta.matrix.clone() else { return Ok(None) };
        // The render's own order: the stage resolved, the calibration
        // validated, the hot sites mapped before anything reads the mosaic.
        let diag = crate::diag::Diag::about(crate::diag::stderr(), raw);
        let recipe = part.recipe();
        let Some((stage, None)) = camera::resolve_camera_profile(&meta, &recipe, ExportColorSpace::Srgb, &diag)
        else {
            return Ok(None);
        };
        validate_calibration(&xyz2cam, rawimage.wb_coeffs, ExportColorSpace::Srgb, raw)?;
        let hot_sites = crate::denoise::hot_pixels::map_for(&mut rawimage)?;
        if hot_sites > 0 {
            println!("mapped {hot_sites} isolated hot pixels stronger than 20 local sigma");
        }
        let mut dev = RawDevelop::default();
        dev.steps.retain(|s| {
            !matches!(s, ProcessingStep::WhiteBalance | ProcessingStep::Calibrate | ProcessingStep::SRgb)
        });
        let (data, w, h) = source::develop_raw_buffer(&rawimage, &dev, None, None, ExportColorSpace::Srgb, None)?;
        drop(rawimage);
        // Oriented first: the stage is per pixel, so staging the oriented
        // frame is staging the frame and orienting it, as the render does.
        let (data, w, h) = orient_f32(data, w, h, orientation);
        let film_short = u32::try_from(w.min(h)).ok();
        let as_shot = normalise_wb(meta.wb_coeffs);
        let mut full = data.clone();
        calibrate_camera_buffer(&mut full, &xyz2cam, as_shot, ExportColorSpace::Srgb, Some(&stage));
        let (exact, _, _) = downscale_f32(full, w, h, max_edge);
        let (camera, w, h) = downscale_f32(data, w, h, max_edge);
        let mut staged = camera.clone();
        calibrate_camera_buffer(&mut staged, &xyz2cam, as_shot, ExportColorSpace::Srgb, Some(&stage));
        Ok(Some(Self {
            path: raw.to_path_buf(),
            part,
            meta,
            xyz2cam,
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
                Some(parent) => parent.thumbnail(*edge, *edge),
                None => return Ok(None),
            },
            None => {
                let validated = crate::recipe::ValidatedRecipe::new(&part.recipe());
                let recipe = &*validated;
                // Said when the base was decoded; not again for every white.
                let diag = crate::diag::Diag::about(crate::diag::silent(), &self.path);
                let (data, white_in_camera) = if part.carries_white() {
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
                            calibrate_camera_buffer(&mut moved, &self.xyz2cam, wb, ExportColorSpace::Srgb, Some(&stage));
                            moved
                                .par_iter_mut()
                                .zip(self.exact.par_iter().zip(self.staged.par_iter()))
                                .for_each(|(m, (e, s))| *m = std::array::from_fn(|c| m[c] - s[c] + e[c]));
                            (moved, true)
                        }
                        // A stage that does not own the camera leaves the
                        // white to the adaptation after it, on the As Shot
                        // frame — the render's own fallback.
                        None => (self.exact.clone(), false),
                    }
                } else {
                    (self.exact.clone(), false)
                };
                let rasters = load_mask_raster_snapshot(recipe, &diag)?;
                source::finish_develop(
                    data,
                    self.w,
                    self.h,
                    self.film,
                    recipe,
                    &rasters,
                    white_in_camera,
                    ExportColorSpace::Srgb,
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
/// its own pass. Any other pixels — a plate of a baked source, a copy, a crop
/// — are what they always were.
pub struct Plate {
    image: Arc<DynamicImage>,
    registered: bool,
}

struct PlateEntry {
    at: (usize, usize),
    camera: Arc<CameraBase>,
    part: BasePart,
    thumbs: Vec<u32>,
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
    /// Pixels with no camera base behind them (a baked source, a master).
    pub fn baked(image: DynamicImage) -> Self {
        Self { image: Arc::new(image), registered: false }
    }

    fn register(image: Arc<DynamicImage>, camera: Arc<CameraBase>, part: BasePart, thumbs: Vec<u32>) -> Self {
        plates().lock().unwrap_or_else(|p| p.into_inner()).push(PlateEntry {
            at: pixels_at(&image),
            camera,
            part,
            thumbs,
        });
        Self { image, registered: true }
    }

    /// `src.thumbnail(edge, edge)` — and, when `src` is a registered plate's
    /// pixels, a plate that knows its camera base and the chain that made it.
    pub fn thumbnail_of(src: &DynamicImage, edge: u32) -> Self {
        let image = Arc::new(src.thumbnail(edge, edge));
        let parent = {
            let plates = plates().lock().unwrap_or_else(|p| p.into_inner());
            plates.iter().find(|e| e.at == pixels_at(src)).map(|e| {
                let mut thumbs = e.thumbs.clone();
                thumbs.push(edge);
                (e.camera.clone(), e.part.clone(), thumbs)
            })
        };
        match parent {
            Some((camera, part, thumbs)) => Self::register(image, camera, part, thumbs),
            None => Self { image, registered: false },
        }
    }

    /// The pixels, shared (the registration lasts as long as this plate does).
    pub fn shared(&self) -> Arc<DynamicImage> {
        self.image.clone()
    }

    /// Does a camera base stand behind these pixels?
    pub fn has_camera(&self) -> bool {
        self.registered
    }
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
        return Ok(Plate::register(img, camera, part.clone(), Vec::new()));
    }
    Ok(Plate::baked(render_to_image(raw, &part.recipe(), None, Some(edge))?))
}

/// What the preview develop renders `recipe` from when it is handed
/// `preview`: `Some((pixels, white_in_pixels))` when `preview` is a registered
/// plate's — `None` for the plate itself when the recipe's canvas part is the
/// plate's own, else that part developed from the camera base along the
/// plate's chain — and `None` for any other pixels.
pub(super) fn plate_pixels(
    preview: &DynamicImage,
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
) -> Option<(Option<Arc<DynamicImage>>, bool)> {
    let (camera, own, thumbs) = {
        let plates = plates().lock().unwrap_or_else(|p| p.into_inner());
        let e = plates.iter().find(|e| e.at == pixels_at(preview))?;
        (e.camera.clone(), e.part.clone(), e.thumbs.clone())
    };
    let want = BasePart::of_canvas(recipe);
    if want == own {
        return Some((None, own.carries_white()));
    }
    match camera.develop(&want, &thumbs) {
        Ok(Some(img)) => Some((Some(img), want.carries_white())),
        Ok(None) => None,
        Err(e) => {
            diag.warn(format!(
                "the camera stage could not re-develop this source ({e:#}); its white balance is                  adapted after the profile instead"
            ));
            None
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
