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
        }
    }

    /// This part with the lens geometry left to the tail: a base in the SENSOR
    /// frame, for the surface that still works there (the reverse fit's
    /// source).
    pub fn without_lens(&self) -> Self {
        Self { lens: None, ..self.clone() }
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
        r
    }

    /// The recipe a MASTER of this part is baked through ([`master_develop`]):
    /// [`BasePart::recipe`] with the RAW default capture sharpening kept. A
    /// master is a baked file, and a baked file is rendered with no default
    /// sharpening (`EditRecipe::capture_sharpening`), so the master carries the
    /// one sharpening its RAW is owed. In the frame the base is in: with the
    /// part's lens geometry, a master is lens-corrected, and its file says so
    /// ([`BasePart::baked_geometry`], [`stamp_baked_geometry`]).
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
                        "this master has {g} in its pixels, but the develop asks for {w} — bake                          the master again from the RAW, or set the lens corrections back"
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
/// at `max_edge` (the cap runs right after demosaic, as in
/// [`render_to_image`]). The neutral part is the neutral develop every base
/// was before v1.17.0, unsharpened ([`BasePart::recipe`]). A part that names a profile is a
/// 32-bit float frame that keeps its over-range light ([`OverRange::Keep`]),
/// so the canvas's Exposure brings it back as the export's does; a surface
/// that converts it to 8 or 16 bits clips it there.
pub fn base_develop(raw: &Path, part: &BasePart, max_edge: Option<u32>) -> Result<DynamicImage> {
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
