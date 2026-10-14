//! Pixel layers (2026-10-01, user decision): a generative fill lands as a
//! LAYER on the card it was made on, over that card's develop, instead of as a
//! new ✨ card — 「如何让生成式填充的部分作为图层，非破坏式的覆盖到原图上（并保存）」,
//! answered 「盖在调色上面」.
//!
//! A layer is one RGBA file covering the whole frame in the develop's own
//! (pre-geometry) coordinates: its colour is the generated pixels as the card
//! looked when they were made, its alpha the feathered painted area
//! (`generative::FillJob::layer`). It is composited LAST in the develop chain
//! (`apply_develop_with_rasters`, after the SDR rendition), because what the
//! model was shown was that finished picture, and the geometry stage that
//! follows carries it with the rest of the photograph. The recipe holds only
//! the path, a switch and an opacity, so the card's own pixels never change:
//! switching a layer off or deleting it gives the photograph back.
//!
//! The trade the user chose, stated where it lives: the layer is the LOOK of
//! the moment it was made. Sliders moved afterwards change the photograph
//! under it and not the layer itself, so a large later edit shows a seam —
//! regenerate the layer then.

use super::*;

/// One generated patch over a card's develop. See the module docs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PixelLayer {
    /// The RGBA file (a `./out` master, like every generated card's origin).
    pub path: String,
    /// Off = carried and saved, not drawn.
    pub enabled: bool,
    /// 0..=1, multiplying the file's own alpha.
    pub opacity: f32,
}

impl Default for PixelLayer {
    fn default() -> Self {
        PixelLayer { path: String::new(), enabled: true, opacity: 1.0 }
    }
}

impl PixelLayer {
    /// Does this layer put any pixel on the frame?
    pub fn draws(&self) -> bool {
        self.enabled && self.opacity > 0.0 && !self.path.is_empty()
    }
}

/// What a whole-frame raster loader names in its messages: `what` before the
/// path ("pixel layer"), and the consequence of an unreadable file after it
/// ("the layer is not drawn").
pub(super) struct RasterKind {
    pub(super) what: &'static str,
    pub(super) unloaded: &'static str,
}

const LAYER: RasterKind = RasterKind { what: "pixel layer", unloaded: "the layer is not drawn" };

/// One slot of a [`decode_cached`] cache: the path, the file's (mtime, length)
/// when it was decoded, and the decode (or `None` for a cached failure).
pub(super) type DecodeSlot<T> =
    (String, Option<(Option<std::time::SystemTime>, u64)>, Option<std::sync::Arc<T>>);

/// Decode one whole-frame raster (a pixel layer, the lens blur's depth map)
/// through a process-wide cache keyed by the file's (mtime, length) — the
/// preview re-develops on every slider tick, and these files never change once
/// written (a new fill or a new estimate writes a new name), so the decode is
/// paid once. A failure is cached too, so its warning fires once rather than
/// per tick; the bounded header probe runs before any decode, as for every
/// mask raster ([`open_mask_bounded`]). `cap` bounds the entries without LRU
/// bookkeeping on the hot path.
pub(super) fn decode_cached<T>(
    slots: &std::sync::Mutex<Vec<DecodeSlot<T>>>,
    cap: usize,
    path: &str,
    kind: &RasterKind,
    decode: impl FnOnce(image::DynamicImage) -> T,
    diag: &crate::diag::Diag<'_>,
) -> Option<std::sync::Arc<T>> {
    let ident = std::fs::metadata(path).ok().map(|m| (m.modified().ok(), m.len()));
    let hit = slots
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(p, i, _)| p == path && *i == ident)
        .map(|(_, _, img)| img.clone());
    if let Some(img) = hit {
        return img;
    }
    let decoded = match open_mask_bounded(std::path::Path::new(path)) {
        Ok(img) => Some(std::sync::Arc::new(decode(img))),
        Err(e) => {
            diag.warn(format!("{} '{path}' could not be loaded ({e}) — {}", kind.what, kind.unloaded));
            None
        }
    };
    let mut held = slots.lock().unwrap_or_else(|p| p.into_inner());
    held.retain(|(p, _, _)| p != path);
    if held.len() >= cap {
        held.remove(0);
    }
    held.push((path.to_owned(), ident, decoded.clone()));
    decoded
}

fn load_layer_image(path: &str, diag: &crate::diag::Diag<'_>) -> Option<std::sync::Arc<image::RgbaImage>> {
    static DECODED: std::sync::OnceLock<std::sync::Mutex<Vec<DecodeSlot<image::RgbaImage>>>> =
        std::sync::OnceLock::new();
    // A card holds a handful of layers; eight entries bound the cache at a
    // few full-resolution frames.
    decode_cached(DECODED.get_or_init(Default::default), 8, path, &LAYER, |img| img.to_rgba8(), diag)
}

/// Load one whole-frame raster, charging its header-projected footprint
/// (`bytes_per_px`) to the SAME aggregate budget the mask rasters are charged
/// to (`held` carries what they already hold). `strict` (an export) refuses an
/// unreadable or over-budget file outright — a file that silently left out
/// what the canvas showed would be the wrong file; the preview skips it with
/// one line on `diag` and gets `None`.
#[allow(clippy::too_many_arguments)] // the budget's three, the policy's two, and the decode
pub(super) fn load_budgeted<T>(
    path: &str,
    kind: &RasterKind,
    bytes_per_px: usize,
    budget_bytes: usize,
    held: &mut usize,
    strict: bool,
    diag: &crate::diag::Diag<'_>,
    decode: impl FnOnce() -> Option<(std::sync::Arc<T>, usize)>,
) -> Result<Option<std::sync::Arc<T>>> {
    let what = kind.what;
    let projected = image::ImageReader::open(path)
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .map_or(0, |(w, h)| (w as usize).saturating_mul(h as usize).saturating_mul(bytes_per_px));
    if held.saturating_add(projected) > budget_bytes {
        if strict {
            bail!("{what} '{path}' exceeds the {budget_bytes}-byte raster budget — no pixels were rendered");
        }
        diag.warn(format!("{what} '{path}' skipped: the raster set exceeds the {budget_bytes}-byte budget"));
        return Ok(None);
    }
    let Some((img, bytes)) = decode() else {
        if strict {
            bail!("{what} '{path}' is unreadable — no pixels were rendered");
        }
        return Ok(None);
    };
    *held = held.saturating_add(bytes);
    Ok(Some(img))
}

/// Load every drawing layer of `recipe` into `out` through [`load_budgeted`].
pub(super) fn load_layers(
    recipe: &EditRecipe,
    budget_bytes: usize,
    held: &mut usize,
    strict: bool,
    diag: &crate::diag::Diag<'_>,
    out: &mut std::collections::HashMap<String, std::sync::Arc<image::RgbaImage>>,
) -> Result<()> {
    for layer in recipe.pixel_layers.iter().filter(|l| l.draws()) {
        let path = layer.path.as_str();
        if out.contains_key(path) {
            continue;
        }
        let decode = || load_layer_image(path, diag).map(|img| {
            let bytes = img.as_raw().len();
            (img, bytes)
        });
        if let Some(img) = load_budgeted(path, &LAYER, 4, budget_bytes, held, strict, diag, decode)? {
            out.insert(path.to_owned(), img);
        }
    }
    Ok(())
}

/// Composite one layer over the develop's `w`×`h` frame, in place, in the
/// develop's own display-referred [0, 1] values. The layer covers the whole
/// frame whatever its own size: a same-size layer is read texel for texel,
/// any other size bilinearly at texel centres (the mask rasters' convention,
/// [`sample_gray_norm`]), so a 2048 px fill lands where it was painted on a
/// full-resolution export too.
pub(super) fn composite(data: &mut [[f32; 3]], w: usize, h: usize, layer: &image::RgbaImage, opacity: f32) {
    let (lw, lh) = (layer.width() as usize, layer.height() as usize);
    if lw == 0 || lh == 0 || w == 0 || h == 0 {
        return;
    }
    let texel = |x: usize, y: usize| -> [f32; 4] {
        let p = layer.get_pixel(x as u32, y as u32).0;
        [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]
    };
    let same = lw == w && lh == h;
    let axis = |i: usize, n: usize, ln: usize| {
        let s = (((i as f32 + 0.5) / n as f32) * ln as f32 - MASK_SAMPLE_CENTRE).clamp(0.0, (ln - 1) as f32);
        let i0 = s.floor() as usize;
        (i0, (i0 + 1).min(ln - 1), s - i0 as f32)
    };
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (y0, y1, fy) = if same { (y, y, 0.0) } else { axis(y, h, lh) };
        for (x, px) in row.iter_mut().enumerate() {
            let v = if same {
                texel(x, y)
            } else {
                let (x0, x1, fx) = axis(x, w, lw);
                let (a, b, c, d) = (texel(x0, y0), texel(x1, y0), texel(x0, y1), texel(x1, y1));
                std::array::from_fn(|k| {
                    (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy
                })
            };
            let a = v[3] * opacity;
            if a > 0.0 {
                for k in 0..3 {
                    px[k] = px[k] * (1.0 - a) + v[k] * a;
                }
            }
        }
    });
}
