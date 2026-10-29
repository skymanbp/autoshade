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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BasePart {
    camera_profile: String,
    look: Option<crate::recipe::CreativeLook>,
    vignette: Vec<f32>,
    vignette_scale: f32,
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
        }
    }

    /// The neutral develop: no profile named.
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// The recipe that develops exactly this part and nothing else.
    pub fn recipe(&self) -> EditRecipe {
        let dflt = EditRecipe::default();
        EditRecipe {
            camera_profile: self.camera_profile.clone(),
            look: self.look.clone(),
            lens_profile: crate::recipe::LensProfile {
                vignette_on: !self.vignette.is_empty(),
                vignette: self.vignette.clone(),
                ..Default::default()
            },
            lens_profile_vignetting_scale: if self.vignette.is_empty() {
                dflt.lens_profile_vignetting_scale
            } else {
                self.vignette_scale
            },
            ..dflt
        }
    }
}

/// Develop `raw`'s base for `part` — the profile stage and the neutral rest —
/// at `max_edge` (the cap runs right after demosaic, as in
/// [`render_to_image`]). The neutral part is byte-identical to the neutral
/// develop every base was before v1.17.0.
pub fn base_develop(raw: &Path, part: &BasePart, max_edge: Option<u32>) -> Result<DynamicImage> {
    render_to_image(raw, &part.recipe(), None, max_edge)
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
