//! Mask rasters: bake operations, raster paths and snapshots, the decode budgets and the bitmap loader.

use super::*;

// --- bitmap-mask BAKE operations (the GUI's raster editing) -----------------
// Every op returns a NEW image; the GUI writes it under a freshly CLAIMED
// raster name and repoints the recipe — the input file is never mutated, so
// saved recipes and version snapshots referencing it keep rendering what they
// rendered (the same immutability rule start_segment follows).

/// Soften a mask's boundary: gaussian blur of the raster. `sigma` in mask
/// pixels.
pub fn feather_mask(g: &image::GrayImage, sigma: f32) -> image::GrayImage {
    image::imageops::blur(g, sigma.max(0.1))
}

/// Grow (`radius > 0`, dilate) or shrink (`radius < 0`, erode) a mask by
/// `|radius|` pixels — separable max/min filter, the morphological pair
/// behind Lightroom's Expand/Contract. Square structuring element: visually
/// indistinguishable from a disc at the small radii the GUI uses, and O(r)
/// cheaper to reason about.
pub fn morph_mask(g: &image::GrayImage, radius: i32) -> image::GrayImage {
    if radius == 0 || g.width() == 0 || g.height() == 0 {
        return g.clone();
    }
    let r = radius.unsigned_abs() as usize;
    let dilate = radius > 0;
    let (w, h) = (g.width() as usize, g.height() as usize);
    let src = g.as_raw();
    let pick = |acc: u8, v: u8| if dilate { acc.max(v) } else { acc.min(v) };
    let seed = if dilate { 0u8 } else { 255u8 };
    // Horizontal pass…
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        let row = &src[y * w..y * w + w];
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = row[lo..=hi].iter().fold(seed, |a, &v| pick(a, v));
        }
    }
    // …then vertical.
    let mut out = vec![0u8; w * h];
    for x in 0..w {
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
            let mut v = seed;
            for yy in lo..=hi {
                v = pick(v, tmp[yy * w + x]);
            }
            out[y * w + x] = v;
        }
    }
    image::GrayImage::from_raw(w as u32, h as u32, out).expect("dims preserved")
}

// Seven f32 planes are live at the peak over an input tile whose side is at
// most 1024 + 12r: 7 × (1024 + 12r)² × 4 bytes, plus one column scratch row
// and the required u8 output. That is about 42 MiB at the GUI's usual r=19.
pub(super) const GUIDED_REFINE_TILE_EDGE: usize = 1024;

/// Upsample `mask` to the guide image's resolution and snap its soft
/// boundary onto the guide's real edges — He et al.'s guided filter, box
/// means via the NR pass's own `blur_plane`. This is the honest fix for
/// AI-segmentation rasters baked at preview resolution: bilinear upsampling
/// keeps their PLACEMENT resolution-independent but not their DETAIL (a
/// 1280 px mask on a 9504 px export smears every boundary ~7 px), while the
/// guide-driven output follows hair/foliage/architecture edges at the
/// guide's own resolution.
pub fn refine_mask_guided(
    mask: &image::GrayImage,
    guide: &DynamicImage,
    radius: usize,
    eps: f32,
) -> image::GrayImage {
    // A zero / negative / non-finite eps divides by (near-)zero variance and
    // quantises NaN to black pixels — floor it here, this fn is pub.
    let eps = if eps.is_finite() { eps.max(1e-6) } else { 1e-6 };
    refine_mask_guided_tiled(mask, guide, radius, eps, GUIDED_REFINE_TILE_EDGE)
}

pub(super) fn refine_mask_guided_tiled(
    mask: &image::GrayImage,
    guide: &DynamicImage,
    radius: usize,
    eps: f32,
    tile_edge: usize,
) -> image::GrayImage {
    let (w, h) = (guide.width() as usize, guide.height() as usize);
    if w == 0 || h == 0 {
        return mask.clone();
    }

    let r = radius.max(1);
    let support = r.saturating_mul(3);
    let tile_edge = tile_edge.max(1);
    let mut out = image::GrayImage::new(guide.width(), guide.height());

    let guide_luma = |x: usize, y: usize| {
        let p = guide.get_pixel(x as u32, y as u32);
        (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0
    };
    // Global pixel-centre coordinates preserve Triangle resize's continuous
    // mapping; tile-local coordinates never enter, so adjoining tiles sample
    // exactly the same low-resolution mask function.
    let mask_value = |x: usize, y: usize| {
        let u = (x as f32 + 0.5) / w as f32;
        let v = (y as f32 + 0.5) / h as f32;
        image::imageops::sample_bilinear(mask, u, v).map_or(0.0, |p| p[0] as f32 / 255.0)
    };

    for tile_y in (0..h).step_by(tile_edge) {
        let tile_y1 = tile_y.saturating_add(tile_edge).min(h);
        for tile_x in (0..w).step_by(tile_edge) {
            let tile_x1 = tile_x.saturating_add(tile_edge).min(w);

            // The coefficient blur needs a 3r halo around the output tile.
            // Computing those coefficients needs another 3r halo from the
            // guide and mask, so neither serial blur sees an artificial edge.
            let coeff_x0 = tile_x.saturating_sub(support);
            let coeff_y0 = tile_y.saturating_sub(support);
            let coeff_x1 = tile_x1.saturating_add(support).min(w);
            let coeff_y1 = tile_y1.saturating_add(support).min(h);
            let input_x0 = coeff_x0.saturating_sub(support);
            let input_y0 = coeff_y0.saturating_sub(support);
            let input_x1 = coeff_x1.saturating_add(support).min(w);
            let input_y1 = coeff_y1.saturating_add(support).min(h);
            let input_w = input_x1 - input_x0;
            let input_h = input_y1 - input_y0;

            let mut i_plane = Vec::with_capacity(input_w * input_h);
            let mut p_plane = Vec::with_capacity(input_w * input_h);
            for y in input_y0..input_y1 {
                for x in input_x0..input_x1 {
                    i_plane.push(guide_luma(x, y));
                    p_plane.push(mask_value(x, y));
                }
            }

            let ip: Vec<f32> = i_plane.iter().zip(&p_plane).map(|(i, p)| i * p).collect();
            let ii: Vec<f32> = i_plane.iter().map(|i| i * i).collect();
            let mut a = blur_plane(&ip, input_w, input_h, r);
            drop(ip);
            let mut b = blur_plane(&ii, input_w, input_h, r);
            drop(ii);
            let mean_i = blur_plane(&i_plane, input_w, input_h, r);
            let mean_p = blur_plane(&p_plane, input_w, input_h, r);

            for k in 0..a.len() {
                let var = (b[k] - mean_i[k] * mean_i[k]).max(0.0);
                let cov = a[k] - mean_i[k] * mean_p[k];
                a[k] = cov / (var + eps);
                b[k] = mean_p[k] - a[k] * mean_i[k];
            }
            drop(i_plane);
            drop(p_plane);
            drop(mean_i);
            drop(mean_p);

            let coeff_w = coeff_x1 - coeff_x0;
            let coeff_h = coeff_y1 - coeff_y0;
            let coeff_offset_x = coeff_x0 - input_x0;
            let coeff_offset_y = coeff_y0 - input_y0;
            let mut a_inner = Vec::with_capacity(coeff_w * coeff_h);
            let mut b_inner = Vec::with_capacity(coeff_w * coeff_h);
            for y in 0..coeff_h {
                let start = (coeff_offset_y + y) * input_w + coeff_offset_x;
                a_inner.extend_from_slice(&a[start..start + coeff_w]);
                b_inner.extend_from_slice(&b[start..start + coeff_w]);
            }
            drop(a);
            drop(b);

            let mean_a = blur_plane(&a_inner, coeff_w, coeff_h, r);
            drop(a_inner);
            let mean_b = blur_plane(&b_inner, coeff_w, coeff_h, r);
            drop(b_inner);

            let tile_offset_x = tile_x - coeff_x0;
            let tile_offset_y = tile_y - coeff_y0;
            for (local_y, y) in (tile_y..tile_y1).enumerate() {
                for (local_x, x) in (tile_x..tile_x1).enumerate() {
                    let k = (tile_offset_y + local_y) * coeff_w + tile_offset_x + local_x;
                    let value = ((mean_a[k] * guide_luma(x, y) + mean_b[k]).clamp(0.0, 1.0)
                        * 255.0)
                        .round() as u8;
                    out.put_pixel(x as u32, y as u32, image::Luma([value]));
                }
            }
        }
    }

    out
}

#[derive(Debug, Default)]
pub(super) struct MaskRasterSnapshot {
    images: std::collections::HashMap<String, std::sync::Arc<image::GrayImage>>,
    /// The recipe's drawing pixel layers (`layers.rs`), under the same budget.
    layers: std::collections::HashMap<String, std::sync::Arc<image::RgbaImage>>,
}

impl MaskRasterSnapshot {
    pub(super) fn get(&self, geometry: &MaskGeometry) -> Option<&image::GrayImage> {
        self.images.get(geometry_raster_path(geometry)?).map(std::sync::Arc::as_ref)
    }

    pub(super) fn layer(&self, path: &str) -> Option<&image::RgbaImage> {
        self.layers.get(path).map(std::sync::Arc::as_ref)
    }
}

/// The raster FILE one geometry renders from, if it currently has one — the
/// ONE place "where does this geometry's pixels live" is answered.
///
/// Two carriers since R27 Batch-5: an explicit [`MaskGeometry::Bitmap`], and a
/// [`MaskGeometry::AiMask`] whose alpha `segment::resolve_ai_masks` has already
/// recomputed — or, for a zone mask, the alpha the zoned reverse-fit rendered
/// itself ([`crate::recipe::MaskGeometry::select_sky`]). `None` for an AiMask
/// means *not resolved yet, or the segmenter declined* — which is a real
/// state, and [`is_raster_backed`] is the question that separates "has no
/// raster" from "needs no raster".
///
/// `pub` rather than `pub(crate)` since the zone masks became Select Sky
/// components: the GUI's raster tools (brush-edit, feather, expand/contract,
/// full-resolution refine) asked `matches!(.., Bitmap { .. })`, which is the
/// variant and not the question, and would have silently dropped all four
/// affordances off every reverse-fit zone row.
pub fn geometry_raster_path(g: &MaskGeometry) -> Option<&str> {
    match g {
        MaskGeometry::Bitmap { path } => Some(path.as_str()),
        MaskGeometry::AiMask { raster, .. } => raster.as_deref(),
        _ => None,
    }
}

/// [`geometry_raster_path`] for a caller that REPOINTS the geometry at a new
/// file — the GUI's raster bakes, which never mutate the input PNG and always
/// claim a fresh name for the result, so the geometry's KIND must survive the
/// repoint (a bake that rewrote a zone's Select Sky component as a `Bitmap`
/// would take the mask back out of the sidecar it now rides in).
///
/// `None` wherever the immutable twin answers `None`, including an AI mask
/// with no alpha yet: repointing a geometry that has no raster would be
/// inventing one, and every caller starts from a raster it just read.
pub fn geometry_raster_path_mut(g: &mut MaskGeometry) -> Option<&mut String> {
    match g {
        MaskGeometry::Bitmap { path } => Some(path),
        MaskGeometry::AiMask { raster: Some(path), .. } => Some(path),
        _ => None,
    }
}

/// Does this geometry draw from a raster at all?
///
/// Distinct from `geometry_raster_path(g).is_some()`, and the distinction is
/// the whole point: an AI mask with no resolved alpha answers `true` here and
/// `None` there, which is exactly the "this mask NEEDS pixels and has none"
/// state the weight loop must SKIP rather than render at weight 0 — a 0 under
/// `inverted` applies the adjustment to the entire frame.
pub fn is_raster_backed(g: &MaskGeometry) -> bool {
    matches!(g, MaskGeometry::Bitmap { .. } | MaskGeometry::AiMask { .. })
}

/// `diag` rides along ONLY to carry the loader's warnings to the caller,
/// attributed to the photograph they belong to (R28 Batch-5 5c stamped them;
/// R29-1 routes them) — a `batch --jobs 3` interleaves three photos' warnings
/// on one stderr in completion order, and "mask raster '…/mask-1.png' is inert"
/// names a file inside a hashed store directory, not a picture the
/// photographer can find. It changes nothing about what loads.
pub(super) fn load_mask_raster_snapshot(
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
) -> Result<MaskRasterSnapshot> {
    load_mask_raster_snapshot_with_budget(recipe, MASK_RASTER_BUDGET_BYTES, true, diag)
}

/// The PREVIEW arm — the one place that genuinely has no photograph, and since
/// R29-1 the one place that SAYS SO in the type.
///
/// `apply_develop` is handed pixels, a width and a height. Under 5c this arm
/// passed a bare `None` and the registration admitted what that cost: `None`
/// meant "I have no photo" here and "the caller did not bother" three call
/// sites away, and neither the destination nor the order of the resulting
/// stderr line could be chosen by anyone but the process (adjudication F6). It
/// now takes the caller's `Diag`, whose subject is
/// [`crate::diag::Subject::PixelOnly`] when the pixels really are anonymous —
/// a state a sink can match on rather than a missing value it has to guess at.
/// The interactive surfaces this arm serves still have a window and a mask list
/// to say it in ([`dead_bitmap_rasters`]); that remains the better channel
/// there, and a GUI is now free to select it.
pub(super) fn best_effort_mask_raster_snapshot(
    recipe: &EditRecipe,
    diag: &crate::diag::Diag<'_>,
) -> MaskRasterSnapshot {
    load_mask_raster_snapshot_with_budget(recipe, MASK_RASTER_BUDGET_BYTES, false, diag)
        .unwrap_or_default()
}

/// Which of this adjustment's bitmap geometries (base or component) currently
/// have NO loadable raster. The preview path renders such a geometry inert
/// with only a stderr warning ([`load_mask_bitmap`]) — the GUI mask list uses
/// this to put the fact ON THE ROW (L08: the list said "enabled" while the
/// engine skipped the mask). Cache-hot: the answer comes from
/// `load_mask_bitmap`'s (mtime, size)-keyed cache, so a per-frame call costs
/// one metadata stat per path, not a decode.
pub fn dead_bitmap_rasters(m: &crate::recipe::LocalAdjustment) -> Vec<String> {
    std::iter::once(&m.mask)
        .chain(m.components.iter().map(|c| &c.geometry))
        .filter_map(|g| {
            if !is_raster_backed(g) {
                return None;
            }
            // An AI mask with NO resolved alpha is dead in exactly the sense
            // this list means — the row must say so — and it has no path to
            // name, so it names the intent instead (R27 Batch-5).
            let name = geometry_raster_path(g)
                .map(str::to_string)
                .unwrap_or_else(|| "AI mask (not yet recomputed)".to_string());
            // GUI mask list, no photograph in scope and none needed: this is
            // a UI probe, and the row it feeds IS the disclosure. So the
            // channel is DROPPED, deliberately and in the type (R29-1) — the
            // loader's stderr copy would fire once per frame per mask and say
            // nothing the row does not already say. Under 5c this was a `None`
            // that only suppressed the STEM; the line still printed.
            load_mask_bitmap(g, &crate::diag::dropped()).is_none().then_some(name)
        })
        .collect()
}

/// The ONE bounded gate for decoding a standalone mask/overlay raster (L02):
/// a header-only dimension probe refuses anything whose worst-case decoded
/// footprint (w×h×4, the decoder's native intermediate) exceeds the mask
/// budget BEFORE the decoder allocates. Every mask decode outside the
/// snapshot loader routes through here — GUI brush/edit/refine bases,
/// heal/clone plan masks, generative fill masks, reverse-fit sky masks.
/// (The probe and the decode are two opens; a file swapped between them is
/// the develop-store TOCTOU boundary, not this size gate's.)
pub fn open_mask_bounded(path: &std::path::Path) -> anyhow::Result<image::DynamicImage> {
    let (w, h) = image::ImageReader::open(path)?.into_dimensions()?;
    check_mask_dims(w, h, &path.display().to_string())?;
    Ok(image::open(path)?)
}

/// [`open_mask_bounded`] for in-memory bytes (the web UI's uploaded masks).
pub fn mask_from_memory_bounded(bytes: &[u8]) -> anyhow::Result<image::DynamicImage> {
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()?;
    check_mask_dims(w, h, "uploaded mask")?;
    Ok(image::load_from_memory(bytes)?)
}

/// Would a mask raster of these dimensions ever be loadable ON ITS OWN? The
/// per-file half of the budget, which is all [`check_mask_dims`] can judge at a
/// single READ. Deliberately NOT public: a WRITER that asked this question
/// alone shipped the R22 H1 defect (the mask-refine precheck passed a raster
/// the loader then refused for the AGGREGATE) — the writer-side question is
/// [`mask_raster_write_fits_budget`].
fn mask_raster_fits_budget(w: u32, h: u32) -> bool {
    raster_bytes(w, h) <= MASK_RASTER_BUDGET_BYTES
}

/// The ONE worst-case footprint every budget arm charges a raster: ×4 = the
/// decoder's native RGBA intermediate ahead of the grayscale conversion a
/// snapshot retains.
fn raster_bytes(w: u32, h: u32) -> usize {
    (w as usize).saturating_mul(h as usize).saturating_mul(4)
}

/// Header-only projection of ONE mask raster already on disk — [`raster_bytes`]
/// of its stored dimensions, without decoding it. `None` = the dimensions could
/// not be read (a missing/unreadable file), which is the loader's own
/// "find out at decode time" case.
///
/// Charged BEFORE the decode on purpose: the budget used to be spent only after
/// `image::open` had allocated the full raster, so one compressed large file
/// drove peak memory far past the advertised cap before being rejected.
fn raster_projected_bytes(path: &str) -> Option<usize> {
    let reader = image::ImageReader::open(std::path::Path::new(path)).ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    Some(raster_bytes(w, h))
}

/// Every bitmap geometry the develop pipeline would LOAD for this recipe, in
/// the loader's own order: `(mask, geometry, path)` for each Bitmap base or
/// component of a mask that will actually render.
///
/// ONE definition of "active", used by both the loader
/// ([`load_mask_raster_snapshot_with_budget`]) and the writer-side precheck
/// ([`mask_raster_write_fits_budget`]) — R22 H1: the precheck judged the
/// incoming raster ALONE while the loader charges the whole active set, so a
/// second full-resolution refine passed the precheck and was then refused by
/// the aggregate (strict bail at export, silent skip in the preview).
///
/// NOT de-duplicated: the loader skips a path it already HOLDS (its `held_bytes`
/// is the truth about what is charged), while the precheck de-duplicates by
/// path — two different, correct answers to "have I counted this already?".
fn active_bitmap_rasters(
    recipe: &EditRecipe,
) -> impl Iterator<Item = (&crate::recipe::LocalAdjustment, &MaskGeometry, &str)> {
    recipe
        .masks
        .iter()
        .filter(|m| m.enabled && m.amount != 0.0 && engine_active(m))
        .flat_map(|m| {
            std::iter::once(&m.mask)
                .chain(m.components.iter().map(|c| &c.geometry))
                .filter_map(move |g| geometry_raster_path(g).map(|p| (m, g, p)))
        })
}

/// Would a raster of `w`×`h` be loadable again ALONGSIDE the rest of this
/// recipe's active rasters? The writer-side twin of the loader's aggregate
/// charge, asked with the loader's own filter and the loader's own header-only
/// projection — so a full-resolution mask refine cannot publish a raster that
/// every later open/export drops (strictly: `render_to_file` bails and
/// `develop_preview` skips it with a stderr line, i.e. one mask silently stops
/// rendering).
///
/// `replacing` = the raster path this write will REPLACE (the refine target's
/// current raster), excluded from the sum because it will not be loaded again.
/// A raster whose header cannot be read is charged 0 — the same thing the
/// loader can know at this point, and it finds out for real at decode time.
pub fn mask_raster_write_fits_budget(
    recipe: &EditRecipe,
    replacing: Option<&str>,
    w: u32,
    h: u32,
) -> bool {
    let mut projected = raster_bytes(w, h);
    let mut counted: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (_, _, path) in active_bitmap_rasters(recipe) {
        if Some(path) == replacing || !counted.insert(path) {
            continue;
        }
        projected = projected.saturating_add(raster_projected_bytes(path).unwrap_or(0));
    }
    projected <= MASK_RASTER_BUDGET_BYTES
}

fn check_mask_dims(w: u32, h: u32, what: &str) -> anyhow::Result<()> {
    if !mask_raster_fits_budget(w, h) {
        anyhow::bail!(
            "{what} is {w}x{h} — its decoded footprint exceeds the \
             {MASK_RASTER_BUDGET_BYTES}-byte mask budget"
        );
    }
    Ok(())
}

pub(super) fn load_mask_raster_snapshot_with_budget(
    recipe: &EditRecipe,
    budget_bytes: usize,
    strict: bool,
    diag: &crate::diag::Diag<'_>,
) -> Result<MaskRasterSnapshot> {
    let mut snapshot = MaskRasterSnapshot::default();
    let mut held_bytes = 0usize;
    // Every disclosure below is ungated and worker-reachable, so each travels
    // the caller's channel carrying its subject (R28 Batch-5 5c stamped them;
    // R29-1 routes them). The `bail!`s do not: an error message travels up a
    // `Result` to a caller that names the photo itself.
    //
    // `Mark::Bare`: these three have never worn the ⚠ glyph, and the default
    // sink reproduces that. The mark is data precisely so a sink rendering
    // into a per-photo block can drop it.

    // The active set and its per-file projection come from
    // `active_bitmap_rasters` / `raster_projected_bytes` — the same two the
    // writer-side precheck asks (R22 H1), so neither side can drift into a
    // different idea of what is charged.
    for (mask, geometry, path) in active_bitmap_rasters(recipe) {
        if snapshot.images.contains_key(path) {
            continue;
        }
        let label = if mask.name.is_empty() { path } else { mask.name.as_str() };
        // Header-only dimension precheck BEFORE the decode: the budget
        // used to be charged only after image::open had allocated the
        // full raster (plus its native RGBA intermediate), so one
        // compressed large file drove peak memory far past the
        // advertised cap before being rejected.
        if let Some(incoming) = raster_projected_bytes(path) {
            let projected = incoming.saturating_add(held_bytes);
            if projected > budget_bytes {
                if strict {
                    bail!(
                        "mask raster set exceeds the {budget_bytes}-byte aggregate budget while \
                         loading '{path}' for mask '{label}' — no pixels were rendered"
                    );
                }
                diag.emit(
                    crate::diag::Mark::Bare,
                    format!(
                        "mask raster '{path}' skipped: the active raster set exceeds the \
                         {budget_bytes}-byte aggregate budget"
                    ),
                );
                continue;
            }
        }
        let Some(bitmap) = load_mask_bitmap(geometry, diag) else {
            if strict {
                bail!(
                    "mask raster '{path}' for mask '{label}' is unreadable — no pixels were rendered"
                );
            }
            continue;
        };
        let incoming = bitmap.as_raw().len();
        let Some(next_bytes) = held_bytes.checked_add(incoming) else {
            if strict {
                bail!(
                    "mask raster set exceeds the {budget_bytes}-byte aggregate budget while \
                     loading '{path}' for mask '{label}' — no pixels were rendered"
                );
            }
            diag.emit(
                crate::diag::Mark::Bare,
                format!(
                    "mask raster '{path}' skipped: the active raster set exceeds the \
                     {budget_bytes}-byte aggregate budget"
                ),
            );
            continue;
        };
        if next_bytes > budget_bytes {
            if strict {
                bail!(
                    "mask raster set exceeds the {budget_bytes}-byte aggregate budget while \
                     loading '{path}' for mask '{label}' — no pixels were rendered"
                );
            }
            diag.emit(
                crate::diag::Mark::Bare,
                format!(
                    "mask raster '{path}' skipped: the active raster set exceeds the \
                     {budget_bytes}-byte aggregate budget"
                ),
            );
            continue;
        }
        held_bytes = next_bytes;
        snapshot.images.insert(path.to_string(), bitmap);
    }
    layers::load_layers(recipe, budget_bytes, &mut held_bytes, strict, diag, &mut snapshot.layers)?;
    Ok(snapshot)
}

/// Decode the raster of a Bitmap mask geometry, greyscale — through a
/// process-wide (path, mtime)-keyed cache. The GUI re-develops the preview on
/// every slider tick, and decoding the segmentation PNG from DISK per tick
/// dominated the develop whenever a bitmap mask was present. Keyed by mtime
/// because re-running a segmentation OVERWRITES the same file (one raster per
/// photo+target, see the GUI's start_segment) — a path-only key would serve
/// the stale mask forever. Failure warns and returns None (the mask renders
/// inert instead of killing the develop).
///
/// `diag` carries those two warnings to the caller with the photograph they
/// belong to (R28 Batch-5 5c stamped them; R29-1 routes them), and carries ONE
/// caveat worth stating: the negative result is CACHED, so the warning fires
/// once per (path, mtime) — on the channel, and with the subject, of whoever
/// hit it FIRST. A mask raster lives in its own photo's develop directory, so
/// in practice that is the only photo it can belong to; two recipes pointing at
/// one raster would see the first name stick.
///
/// R29-1 sharpened that caveat rather than removing it: the GUI mask list's
/// `dead_bitmap_rasters` probe now passes a SILENT channel (its row is the
/// disclosure), so if the probe reaches a dead raster first, the console line
/// for that (path, mtime) is the one that does not print. That is the intended
/// trade — the alternative (warn per call) is the per-tick flood this cache
/// exists to stop, and the surface that suppressed it is the surface that
/// already shows the fact.
pub(super) fn load_mask_bitmap(
    g: &MaskGeometry,
    diag: &crate::diag::Diag<'_>,
) -> Option<std::sync::Arc<image::GrayImage>> {
    use std::sync::{Arc, Mutex, OnceLock};
    // Keyed by Option<(mtime, size)>: mtime alone misses a same-length-of-
    // time overwrite on coarse-timestamp filesystems (the thumb cache already
    // carries size for the same reason). `None` payload = FAILED to decode —
    // cached so the warning fires once, not on every slider tick. The
    // identity itself is an Option: a MISSING file (no metadata) caches under
    // `None` too — it used to bypass the cache entirely and re-open + re-warn
    // every refresh; the identity flips to Some the moment the file appears,
    // which misses and loads it.
    // Outer None = file MISSING; inner None = mtime unavailable on this
    // filesystem (a distinct, existing-file identity — collapsing the two
    // made a formerly missing mask that appears mtime-less keep hitting the
    // cached negative forever).
    type Key = Option<(Option<std::time::SystemTime>, u64)>;
    type Cache = Mutex<std::collections::HashMap<String, (Key, Option<Arc<image::GrayImage>>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let path = geometry_raster_path(g)?;
    let cache = CACHE.get_or_init(Default::default);
    let ident: Key = std::fs::metadata(path)
        .ok()
        .map(|m| (m.modified().ok(), m.len()));
    {
        // No user code runs under the lock, so poisoning is not reachable —
        // recover anyway rather than turning a past panic into a new one.
        let map = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cached_t, img)) = map.get(path)
            && *cached_t == ident
        {
            return img.clone();
        }
    }
    // Header-only dimension precheck BEFORE the decode (L02): the snapshot
    // loader guards its own calls, but every OTHER path through here (the
    // mask list's ⚠-badge probe, a future direct call) hit image::open
    // unbounded — the decoder allocates the full raster plus its native
    // intermediate before any byte count exists. The refusal is cached under
    // the file's identity below, exactly like a failed decode.
    let over_budget = image::ImageReader::open(path)
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_some_and(|(w, h)| {
            (w as usize).saturating_mul(h as usize).saturating_mul(4) > MASK_RASTER_BUDGET_BYTES
        });
    let decoded = if over_budget {
        diag.warn(format!(
            "bitmap mask '{path}' exceeds the {MASK_RASTER_BUDGET_BYTES}-byte mask budget — mask is inert"
        ));
        None
    } else {
        match image::open(path) {
            Ok(img) => Some(Arc::new(img.to_luma8())),
            Err(e) => {
                diag.warn(format!(
                    "bitmap mask '{path}' could not be loaded ({e}) — mask is inert"
                ));
                None
            }
        }
    };
    {
        let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
        // A recipe holds a handful of masks — a rare hard reset beats
        // LRU bookkeeping on this hot path. Budgeted in BYTES as well
        // as entries: sixteen full-res 61 MP rasters would otherwise
        // pin ~1 GB for the life of the process.
        let held: usize =
            map.values().filter_map(|(_, i)| i.as_ref()).map(|i| i.as_raw().len()).sum();
        let incoming = decoded.as_ref().map_or(0, |i| i.as_raw().len());
        if incoming <= MASK_RASTER_BUDGET_BYTES {
            if map.len() > 16
                || held.saturating_add(incoming) > MASK_RASTER_BUDGET_BYTES
            {
                map.clear();
            }
            map.insert(path.to_string(), (ident, decoded.clone()));
        } else {
            map.clear();
        }
    }
    decoded
}

/// Bilinear weight lookup in an 8-bit greyscale mask at normalised (nx, ny).
pub(crate) fn sample_gray_norm(b: &image::GrayImage, nx: f32, ny: f32) -> f32 {
    let (w, h) = (b.width() as f32, b.height() as f32);
    // EXTENT scaling (`* w`), not endpoint scaling (`* (w - 1)`): a texel owns
    // a SLICE of the frame, `[i/w, (i+1)/w]`, so mapping onto 0..=size-1 here
    // was a DIFFERENT convention. A frame-sized mask then never reached its
    // last row/column — a 2-wide mask holding [0,255] rendered [0, 0.5]
    // instead of [0, 1] — and because the shortfall is one source pixel out of
    // `w`, the same mask landed differently in a 1280 px preview than in a
    // 9504 px export.
    //
    // The `− MASK_SAMPLE_CENTRE` is the other half of that slice reading, and
    // it is what makes this the TEXEL-CENTRE lookup its producers stamp for
    // (R29 C2): texel `i` owns `[i/w, (i+1)/w]`, so its centre is the
    // normalised `(i + 0.5)/w` and the texel coordinate of a normalised `nx`
    // is `nx·w − 0.5`. Bilinear then interpolates between the two texel
    // CENTRES that bracket the sample, which is the only reading under which a
    // raster and the frame it covers agree about where a given physical point
    // is. Without it the interpolation is anchored on texel top-left corners
    // and every raster mask sits half a texel out.
    //
    // The exactness the old comment claimed is KEPT, and by construction: a
    // frame-sized raster read from a frame loop that also samples at pixel
    // centres gives `nx·w − 0.5 = (x + 0.5) − 0.5 = x`, an exact texel hit
    // with no interpolation at all. `rasterise_brush_group` sizes and stamps
    // its raster to land on this same grid.
    //
    // Clamping to `0 ..= size-1` is clamp-to-edge, and it is what the outer
    // half-texel band on each side gets: a sample at `nx = 0` asks for texel
    // −0.5, which is outside the first texel's centre, and the honest answer
    // for a mask that says nothing beyond its own edge is the edge value.
    let sx = (nx.clamp(0.0, 1.0) * w - MASK_SAMPLE_CENTRE).clamp(0.0, w - 1.0);
    let sy = (ny.clamp(0.0, 1.0) * h - MASK_SAMPLE_CENTRE).clamp(0.0, h - 1.0);
    let x0 = sx.floor().min(w - 1.0);
    let y0 = sy.floor().min(h - 1.0);
    let x1 = (x0 + 1.0).min(w - 1.0);
    let y1 = (y0 + 1.0).min(h - 1.0);
    let (fx, fy) = (sx - x0, sy - y0);
    let g = |x: f32, y: f32| b.get_pixel(x as u32, y as u32)[0] as f32 / 255.0;
    let top = g(x0, y0) * (1.0 - fx) + g(x1, y0) * fx;
    let bot = g(x0, y1) * (1.0 - fx) + g(x1, y1) * fx;
    top * (1.0 - fy) + bot * fy
}
