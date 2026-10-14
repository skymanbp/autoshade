//! A reference's SETTINGS, when they can be trusted (user ruling 2026-10-14).
//!
//! A finished photo carries no settings, and its RAW's `.xmp` may describe a
//! different version (stale, one of several, or followed by a Photoshop
//! pass). So an `.xmp` is attached to a library entry only when it PROVES
//! itself: the RAW whose name the finished photo carries is found under the
//! RAW folder the photographer named, its sidecar is developed by this engine
//! at the library's frame size, and the result must look like the finished
//! photo ([`MATCH_DE_MAX`]). What is kept is a line of the settings the
//! advisor reads beside the reference ([`settings_line`]).
//!
//! AI masks in the sidecar stay inert for the match: a build must not run a
//! segmentation model per photo, and a mask is local — the global look the
//! match tests is carried by the global controls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::Stamp;
use crate::recipe::EditRecipe;

/// The most a matched develop may differ from the finished photo: mean CIELAB
/// ΔE76 over a 96-px-wide grid. Calibrated by eye on the photographer's own
/// album (2026-10-14, 63 photos with a candidate sidecar): every pair up to
/// 10.7 was the same picture; from 11.4 up the develop was a different one
/// (an older grade, a later Photoshop pass, a darker or bluer version), the
/// one borderline pair at 11.7 included. 17 of the 63 fall under the line.
pub const MATCH_DE_MAX: f32 = 11.0;

/// Width of the grid the match is measured on: coarse enough that this
/// engine's small rendering differences from Lightroom's average out, fine
/// enough that a different frame or a different grade does not.
const GRID_W: u32 = 96;

/// The longest settings line an entry may carry, in characters.
pub const MAX_SETTINGS_CHARS: usize = 400;

/// A sidecar that proved itself against this entry's finished photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// The RAW and its sidecar, absolute.
    pub raw: String,
    pub xmp: String,
    /// The sidecar as it was when matched: an edited `.xmp` is matched again.
    pub xmp_stamp: Stamp,
    /// The measured mean ΔE76 of the match (lower is closer).
    pub match_de: f32,
    /// The global controls that differ from neutral, as `name=value` in the
    /// recipe's own field names ([`settings_line`]).
    pub line: String,
}

/// Every RAW under `root`, keyed by lower-case file stem. A camera's counter
/// wraps, so one stem may name several RAWs; the match decides between them.
pub(super) fn raw_index(root: &Path) -> Result<HashMap<String, Vec<PathBuf>>> {
    let mut out: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut pending = vec![root.to_path_buf()];
    let mut seen = std::collections::HashSet::new();
    while let Some(dir) = pending.pop() {
        if !std::fs::canonicalize(&dir).map_or(true, |c| seen.insert(c)) {
            continue;
        }
        for item in std::fs::read_dir(&dir).with_context(|| format!("scan {}", dir.display()))? {
            let p = item?.path();
            if p.is_dir() {
                pending.push(p);
            } else if crate::decode::is_raw(&p)
                && let Some(stem) = p.file_stem()
            {
                out.entry(stem.to_string_lossy().to_lowercase()).or_default().push(p);
            }
        }
    }
    for v in out.values_mut() {
        v.sort();
    }
    Ok(out)
}

/// The RAWs a finished photo may have been made from: those whose stem is a
/// PREFIX of the finished photo's stem, with no digit right after it
/// (`frame0264F`, `frame0268-Enhanced-NR`, `frame9274_trip` all name
/// `frame0264`/`frame0268`/`frame9274`; `frame02641` does not name `frame0264`).
/// Longest prefix first, so a merged `X-HDR.dng` is tried before `X.ARW`.
pub(super) fn candidates<'a>(finished: &Path, index: &'a HashMap<String, Vec<PathBuf>>) -> Vec<&'a Path> {
    let Some(stem) = finished.file_stem().map(|s| s.to_string_lossy().to_lowercase()) else {
        return Vec::new();
    };
    let chars: Vec<(usize, char)> = stem.char_indices().collect();
    let mut out = Vec::new();
    for k in (1..=chars.len()).rev() {
        let end = chars.get(k).map_or(stem.len(), |&(i, _)| i);
        let next_is_digit = chars.get(k).is_some_and(|&(_, c)| c.is_ascii_digit());
        if next_is_digit {
            continue;
        }
        if let Some(raws) = index.get(&stem[..end]) {
            out.extend(raws.iter().map(PathBuf::as_path));
        }
    }
    out
}

/// The recipe a sidecar develops to here: the import every surface uses, plus
/// the photo's own calibration (camera base look, lens profile, as-shot WB) —
/// what the desktop app applies when it opens a RAW with a sidecar.
fn sidecar_recipe(raw: &Path, xmp_text: &str) -> EditRecipe {
    let mut r = crate::xmp::xmp_to_recipe_for_photo(xmp_text, raw);
    let quiet = crate::diag::Collector::default();
    r.base_curve = crate::pipeline::photo_base_knots_in(raw, &crate::diag::Diag::about(&quiet, raw));
    r.lens_profile = crate::pipeline::fresh_lens_profile_for_sidecar(raw, Some(xmp_text));
    let (k, t) = crate::pipeline::fresh_as_shot_wb(raw);
    r.as_shot_k = r.as_shot_k.or(k);
    r.as_shot_tint = r.as_shot_tint.or(t);
    r
}

/// Mean CIELAB ΔE76 between two pictures on a [`GRID_W`]-wide grid of `a`'s
/// aspect (`b` is resized onto it).
pub(super) fn mean_de(a: &image::DynamicImage, b: &image::DynamicImage) -> f32 {
    let h = ((GRID_W as f32) * a.height() as f32 / a.width().max(1) as f32).round().max(1.0) as u32;
    let f = image::imageops::FilterType::Triangle;
    let (la, lb) = (a.resize_exact(GRID_W, h, f).to_rgb8(), b.resize_exact(GRID_W, h, f).to_rgb8());
    let lab = |p: &image::Rgb<u8>| {
        let lin = |c: u8| {
            let c = f32::from(c) / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        let (r, g, b) = (lin(p[0]), lin(p[1]), lin(p[2]));
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
        let t = |v: f32| if v > 0.008856 { v.cbrt() } else { 7.787 * v + 16.0 / 116.0 };
        let (fx, fy, fz) = (t(x), t(y), t(z));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    };
    let n = (GRID_W * h) as f32;
    la.pixels()
        .zip(lb.pixels())
        .map(|(p, q)| {
            let (u, v) = (lab(p), lab(q));
            ((u[0] - v[0]).powi(2) + (u[1] - v[1]).powi(2) + (u[2] - v[2]).powi(2)).sqrt()
        })
        .sum::<f32>()
        / n
}

/// Develop `raw` with its sidecar at the library's frame size and measure it
/// against `finished` (already reduced to the frame): the settings it would
/// carry, with their measured ΔE. `None` when the RAW has no sidecar.
pub(super) fn try_candidate(raw: &Path, root: &Path, finished: &image::DynamicImage) -> Result<Option<Settings>> {
    let Some(xmp) = crate::xmp_pair::XmpPairing::new(root, None).find(raw) else {
        return Ok(None);
    };
    let text = crate::store::read_text_capped(&xmp, crate::store::MAX_STORE_JSON)
        .with_context(|| format!("read {}", xmp.display()))?;
    let recipe = sidecar_recipe(raw, &text);
    let quiet = crate::diag::Collector::default();
    let developed = crate::render::render_to_image_in(
        raw,
        &recipe,
        None,
        Some(super::frames::FRAME_EDGE),
        crate::render::ExportColorSpace::Srgb,
        &quiet,
    )?;
    let match_de = mean_de(&developed, finished);
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).display().to_string();
    Ok(Some(Settings {
        raw: abs(raw),
        xmp: abs(&xmp),
        xmp_stamp: Stamp::of(&xmp)?,
        match_de,
        line: settings_line(&recipe),
    }))
}

/// The global controls of `r` that differ from neutral, as `name=value` in
/// the recipe's own JSON field names — the names the advisor writes its
/// proposal in, so it reads a reference's settings in its own vocabulary.
/// Local masks are counted, not listed.
pub fn settings_line(r: &EditRecipe) -> String {
    const SCALARS: &[&str] = &[
        "exposure_ev", "contrast", "highlights", "shadows", "whites", "blacks", "temperature_k", "tint",
        "vibrance", "saturation", "clarity", "dehaze", "texture", "param_shadows", "param_darks", "param_lights",
        "param_highlights", "post_crop_vignette", "grain", "convert_to_grayscale",
    ];
    let have = serde_json::to_value(r).unwrap_or_default();
    let neutral = serde_json::to_value(EditRecipe::default()).unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    for k in SCALARS {
        let (Some(v), d) = (have.get(*k), neutral.get(*k)) else { continue };
        if Some(v) == d || v.is_null() {
            continue;
        }
        parts.push(match v {
            serde_json::Value::Number(n) => {
                let x = n.as_f64().unwrap_or(0.0);
                if *k == "temperature_k" { format!("{k}={x:.0}") } else { format!("{k}={x:+.2}") }
            }
            other => format!("{k}={other}"),
        });
    }
    for (name, curve) in [("tone_curve", &r.tone_curve), ("red_curve", &r.red_curve), ("green_curve", &r.green_curve), ("blue_curve", &r.blue_curve)] {
        if !curve.is_empty() {
            parts.push(format!("{name}={} points", curve.len()));
        }
    }
    for (key, label) in [("hsl", "hsl"), ("color_grade", "color_grade")] {
        if have.get(key) != neutral.get(key) {
            parts.push(format!("{label}=set"));
        }
    }
    if !r.masks.is_empty() {
        parts.push(format!("local_masks={}", r.masks.len()));
    }
    let line = parts.join(", ");
    line.chars().take(MAX_SETTINGS_CHARS).collect()
}
