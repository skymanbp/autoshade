//! The Portraits mode's face finder (v1.14.0): the bridge to `python/face.py`,
//! which runs MediaPipe's Face Landmarker over one picture and writes a small
//! JSON file with every face it found — a box, a score, the face outline and
//! the two eye outlines, in the picture's own pixels (see that script's
//! docstring for the format and the model).
//!
//! What this side does with the outlines is the whole of the mask story: each
//! face's outline is filled and feathered into a soft FACE mask, each eye's
//! into a soft EYES mask, and several faces are UNIONED (per-pixel maximum)
//! into one mask of each kind, because the first version edits every face at
//! once. Two more roles are built on top: SKIN is the face mask narrowed by a
//! colour range seeded from the face's own median colour, and BACKGROUND is the
//! subject matte `segment.py --target subject` already makes, inverted. All
//! four become ordinary bitmap masks carrying a [`MaskRole`], so every slider
//! the masks already have works on them unchanged.
//!
//! The AI picks WHERE; every edit stays a deterministic recipe slider, as with
//! the segmentation masks ([`crate::segment`]).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use image::GrayImage;

use crate::recipe::{LocalAdjustment, MaskGeometry, MaskRole, RangeMask};

/// A real result is a few kilobytes per face; this bounds a broken or hostile
/// sidecar before anything is parsed.
pub const FACES_MAX_BYTES: u64 = 1024 * 1024;
/// The most faces one result may carry (the sidecar's own ceiling is 32).
pub const MAX_FACES: usize = 32;
/// The most points one outline may carry. MediaPipe's face outline is 36
/// points and each eye 16.
const MAX_OUTLINE_POINTS: usize = 64;

/// The face mask's feather, as a share of the face box's longer side: a soft
/// edge about as wide as the jaw's own shading, so a skin edit fades out over
/// the cheek rather than stopping on a line.
pub const FACE_FEATHER: f32 = 0.025;
/// The eye outline traces the lid opening; the mask grows it about its own
/// centre by this factor so the lashes and the lid crease are inside.
pub const EYE_GROW: f32 = 1.3;
/// The eyes mask's feather, as a share of the (grown) eye's width.
pub const EYE_FEATHER: f32 = 0.1;
/// The skin colour range's tolerance (`RangeMask::Color::amount`, 0..=1):
/// wider than Lightroom's 0.5 default, because skin under one light still
/// spans a broad band of chromaticity from the lit cheek to the shaded jaw.
pub const SKIN_RANGE_AMOUNT: f32 = 0.6;

/// One face the sidecar found, in the pixels of [`FaceReport::width`] ×
/// [`FaceReport::height`].
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    /// The detector's score, 0..=1, or `None` when no detection matched the
    /// outline (the landmarker itself returns no score).
    pub confidence: Option<f32>,
    /// `[x0, y0, x1, y1]` around the face outline.
    pub bbox: [f32; 4],
    /// The face outline, in order.
    pub oval: Vec<[f32; 2]>,
    /// The person's left eye (on the image's right), in order.
    pub left_eye: Vec<[f32; 2]>,
    /// The person's right eye (on the image's left), in order.
    pub right_eye: Vec<[f32; 2]>,
}

/// One validated sidecar result.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceReport {
    /// Which model answered, as the sidecar names it.
    pub backend: String,
    /// The frame the coordinates are in: the INPUT image's size.
    pub width: u32,
    pub height: u32,
    /// Every face found; empty when there is none, which is not an error.
    pub faces: Vec<Face>,
}

/// Parse and validate the JSON `face.py` writes. Anything malformed rejects
/// the whole result: a mask is never built from a guess.
pub fn parse_faces(text: &str) -> Result<FaceReport> {
    #[derive(serde::Deserialize)]
    struct Raw {
        version: u32,
        backend: String,
        width: u32,
        height: u32,
        faces: Vec<RawFace>,
    }
    #[derive(serde::Deserialize)]
    struct RawFace {
        confidence: Option<f32>,
        #[serde(rename = "box")]
        bbox: [f32; 4],
        oval: Vec<[f32; 2]>,
        left_eye: Vec<[f32; 2]>,
        right_eye: Vec<[f32; 2]>,
    }
    let raw: Raw = serde_json::from_str(text.trim()).context("face result is not the expected JSON")?;
    if raw.version != 1 {
        bail!("face result version {} is unsupported", raw.version);
    }
    if raw.width == 0 || raw.height == 0 || raw.width > 65_535 || raw.height > 65_535 {
        bail!("face result has an invalid frame {}x{}", raw.width, raw.height);
    }
    if raw.faces.len() > MAX_FACES {
        bail!("face result lists {} faces (cap {MAX_FACES})", raw.faces.len());
    }
    if raw.backend.len() > 256 {
        bail!("face result backend name is too long");
    }
    // Outlines may run a little past the frame (a face cut by the edge); a
    // point further out than one frame is not a face of this picture.
    let (w, h) = (raw.width as f32, raw.height as f32);
    let in_reach = |p: &[f32; 2]| p[0].is_finite() && p[1].is_finite() && p[0] >= -w && p[0] <= 2.0 * w && p[1] >= -h && p[1] <= 2.0 * h;
    let mut faces = Vec::with_capacity(raw.faces.len());
    for (i, f) in raw.faces.into_iter().enumerate() {
        if let Some(c) = f.confidence
            && !(c.is_finite() && (0.0..=1.0).contains(&c))
        {
            bail!("face {i}: confidence {c} is not in 0..=1");
        }
        let b = f.bbox;
        if !(in_reach(&[b[0], b[1]]) && in_reach(&[b[2], b[3]]) && b[0] <= b[2] && b[1] <= b[3]) {
            bail!("face {i}: the box is not a box in this frame");
        }
        for (what, poly) in [("face outline", &f.oval), ("left eye", &f.left_eye), ("right eye", &f.right_eye)] {
            if !(3..=MAX_OUTLINE_POINTS).contains(&poly.len()) {
                bail!("face {i}: the {what} has {} points (want 3..={MAX_OUTLINE_POINTS})", poly.len());
            }
            if !poly.iter().all(in_reach) {
                bail!("face {i}: the {what} has a point outside this frame");
            }
        }
        faces.push(Face { confidence: f.confidence, bbox: b, oval: f.oval, left_eye: f.left_eye, right_eye: f.right_eye });
    }
    Ok(FaceReport { backend: raw.backend, width: raw.width, height: raw.height, faces })
}

/// Run the sidecar `cfg.face_script` names: `input` (any image file) →
/// `output` (the faces JSON), and return the validated result. Through the
/// shared model-sidecar run, so the model slot, the bounded wait, "exit 0 is
/// not success" and the size cap are the family's.
pub fn detect_faces(cfg: &crate::config::Config, input: &Path, output: &Path) -> Result<FaceReport> {
    let script = Path::new(&cfg.face_script);
    if !script.exists() {
        bail!(
            "face sidecar not found at {} — run from the project dir or set AUTOSHADE_FACE_SCRIPT.",
            script.display()
        );
    }
    let args: Vec<std::ffi::OsString> =
        vec!["-E".into(), script.into(), "--input".into(), input.into(), "--output".into(), output.into()];
    let text = crate::run_model_sidecar_bounded("face sidecar", &cfg.python_bin, args, output, Some(FACES_MAX_BYTES))?;
    parse_faces(&text).with_context(|| format!("face sidecar produced an unusable result {}", output.display()))
}

/// Fill `poly` (pixel coordinates of a `w`×`h` frame) into `mask` at 255,
/// even-odd, sampling pixel CENTRES — the convention every mask here reads.
fn fill_polygon(mask: &mut GrayImage, poly: &[[f32; 2]]) {
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    if poly.len() < 3 || w == 0 || h == 0 {
        return;
    }
    let (lo, hi) = poly.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
    let y0 = (lo - 0.5).ceil().max(0.0) as usize;
    let y1 = ((hi - 0.5).floor().min(h as f32 - 1.0)).max(-1.0);
    if y1 < 0.0 {
        return;
    }
    let mut xs: Vec<f32> = Vec::with_capacity(poly.len());
    for y in y0..=y1 as usize {
        let cy = y as f32 + 0.5;
        xs.clear();
        for (i, a) in poly.iter().enumerate() {
            let b = poly[(i + 1) % poly.len()];
            // Half-open in y, so a vertex on the scanline counts once.
            if (a[1] <= cy) != (b[1] <= cy) {
                xs.push(a[0] + (cy - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
            }
        }
        xs.sort_by(f32::total_cmp);
        for pair in xs.chunks_exact(2) {
            let xa = (pair[0] - 0.5).ceil().max(0.0) as usize;
            let xb = (pair[1] - 0.5).floor().min(w as f32 - 1.0);
            if xb < xa as f32 {
                continue;
            }
            for x in xa..=xb as usize {
                mask.put_pixel(x as u32, y as u32, image::Luma([255]));
            }
        }
    }
}

/// Fill one outline into its own hard mask, feather it by `sigma` pixels and
/// fold it into `out` by per-pixel maximum (the union). Only the outline's
/// padded box is blurred, so ten faces on a large frame cost ten small blurs.
fn union_feathered(out: &mut GrayImage, poly: &[[f32; 2]], sigma: f32) {
    let (w, h) = (out.width() as f32, out.height() as f32);
    let pad = (3.0 * sigma).ceil() + 2.0;
    let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in poly {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let bx0 = (x0 - pad).floor().clamp(0.0, w) as u32;
    let by0 = (y0 - pad).floor().clamp(0.0, h) as u32;
    let bx1 = (x1 + pad).ceil().clamp(0.0, w) as u32;
    let by1 = (y1 + pad).ceil().clamp(0.0, h) as u32;
    if bx1 <= bx0 || by1 <= by0 {
        return;
    }
    let local: Vec<[f32; 2]> = poly.iter().map(|p| [p[0] - bx0 as f32, p[1] - by0 as f32]).collect();
    let mut hard = GrayImage::new(bx1 - bx0, by1 - by0);
    fill_polygon(&mut hard, &local);
    let soft = if sigma > 0.0 { crate::render::feather_mask(&hard, sigma) } else { hard };
    for (x, y, p) in soft.enumerate_pixels() {
        let o = out.get_pixel_mut(bx0 + x, by0 + y);
        o[0] = o[0].max(p[0]);
    }
}

/// `poly` from the report's frame into a `w`×`h` frame.
fn scaled(report: &FaceReport, poly: &[[f32; 2]], w: u32, h: u32) -> Vec<[f32; 2]> {
    let (sx, sy) = (w as f32 / report.width as f32, h as f32 / report.height as f32);
    poly.iter().map(|p| [p[0] * sx, p[1] * sy]).collect()
}

/// Every face's outline, filled, feathered by [`FACE_FEATHER`] of its box and
/// unioned, as a `w`×`h` soft mask (white = face). Any size: the outlines are
/// scaled from the report's frame, so the mask can be drawn at the preview's
/// resolution and still lands where the faces are.
pub fn face_mask(report: &FaceReport, w: u32, h: u32) -> GrayImage {
    let mut out = GrayImage::new(w, h);
    for f in &report.faces {
        let poly = scaled(report, &f.oval, w, h);
        let (x0, y0, x1, y1) = poly.iter().fold(
            (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
            |(a, b, c, d), p| (a.min(p[0]), b.min(p[1]), c.max(p[0]), d.max(p[1])),
        );
        union_feathered(&mut out, &poly, FACE_FEATHER * (x1 - x0).max(y1 - y0));
    }
    out
}

/// Both eyes of every face, each grown by [`EYE_GROW`] about its centre,
/// feathered by [`EYE_FEATHER`] of its width and unioned, as a `w`×`h` soft
/// mask (white = eyes).
pub fn eyes_mask(report: &FaceReport, w: u32, h: u32) -> GrayImage {
    let mut out = GrayImage::new(w, h);
    for f in &report.faces {
        for eye in [&f.left_eye, &f.right_eye] {
            let poly = scaled(report, eye, w, h);
            let n = poly.len() as f32;
            let c = poly.iter().fold([0.0f32; 2], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
            let grown: Vec<[f32; 2]> =
                poly.iter().map(|p| [c[0] + (p[0] - c[0]) * EYE_GROW, c[1] + (p[1] - c[1]) * EYE_GROW]).collect();
            let (lo, hi) = grown.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
            union_feathered(&mut out, &grown, EYE_FEATHER * (hi - lo));
        }
    }
    out
}

/// The skin colour range's seed: the per-channel median of `reference` (sRGB)
/// over the pixels the face mask holds at ≥ 90 %, and where they are (their
/// centroid, normalised — Lightroom's sample marker). `None` when the mask
/// holds no such pixel. The mask is read at `reference`'s pixel centres, so
/// the two need not be the same size.
pub fn skin_seed(reference: &image::RgbImage, face: &GrayImage) -> Option<([f32; 3], [f32; 2])> {
    let (w, h) = reference.dimensions();
    let mut ch: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    for (x, y, p) in reference.enumerate_pixels() {
        let nx = (x as f32 + 0.5) / w as f32;
        let ny = (y as f32 + 0.5) / h as f32;
        if crate::render::sample_gray_norm(face, nx, ny) >= 0.9 {
            for c in 0..3 {
                ch[c].push(p[c]);
            }
            sx += nx as f64;
            sy += ny as f64;
        }
    }
    let n = ch[0].len();
    if n == 0 {
        return None;
    }
    let median = |v: &mut Vec<u8>| {
        v.sort_unstable();
        v[v.len() / 2] as f32 / 255.0
    };
    let [mut r, mut g, mut b] = ch;
    Some(([median(&mut r), median(&mut g), median(&mut b)], [(sx / n as f64) as f32, (sy / n as f64) as f32]))
}

/// A bitmap mask over `raster` carrying `role` — the FACE and EYES masks. The
/// row's label comes from [`MaskRole::en_name`], so `name` stays empty.
pub fn portrait_adjustment(role: MaskRole, raster: String) -> LocalAdjustment {
    LocalAdjustment { mask: MaskGeometry::Bitmap { path: raster }, role, ..Default::default() }
}

/// The SKIN mask: the face raster intersected with a colour range around
/// `seed` (from [`skin_seed`]), sampled at `at`.
pub fn skin_adjustment(face_raster: String, seed: [f32; 3], at: [f32; 2]) -> LocalAdjustment {
    LocalAdjustment {
        range: Some(RangeMask::Color { r: seed[0], g: seed[1], b: seed[2], amount: SKIN_RANGE_AMOUNT, px: at[0], py: at[1] }),
        ..portrait_adjustment(MaskRole::Skin, face_raster)
    }
}

/// The BACKGROUND mask: the subject matte (`segment.py --target subject`),
/// inverted.
pub fn background_adjustment(subject_raster: String) -> LocalAdjustment {
    LocalAdjustment { inverted: true, ..portrait_adjustment(MaskRole::Background, subject_raster) }
}

// ── "Keep skin natural" (design-portraits-mode.md §4) ───────────────────────
//
// One switch in the Portraits panel, on by default. Inside the masks that
// cover skin — the SKIN mask and the FACE mask, since a proposal's `face`
// target is the face mask and its edits land on skin too — three hard limits:
// colour (saturation, hue rotation, warmth), softening (texture, clarity) and
// no sharpening, the "smoothed then sharpened" plastic look. The values are
// the first version's, set to Lightroom's usual portrait ranges; the final
// check on real portraits tunes them. The GUI applies them to every write of
// the recipe while the switch is on — a slider, a paste, an AI landing — so
// the user's hand and the model's answer meet the same limits at one place.

/// The local hue slider's full travel (±100) in degrees: `render::masks`
/// rotates every hue inside the mask by `hue / 100 × 30°`.
pub const LOCAL_HUE_FULL_DEG: f32 = 30.0;
/// Skin saturation may be lifted by at most this much (fading it is free).
pub const SKIN_SATURATION_MAX: f32 = 15.0;
/// Skin hues rotate by at most this many degrees either way.
pub const SKIN_HUE_MAX_DEG: f32 = 8.0;
/// Skin warmth moves by at most this much either way (the relative ±100 scale).
pub const SKIN_TEMPERATURE_MAX: f32 = 20.0;
/// Softening stops here: texture no lower than this…
pub const SKIN_TEXTURE_MIN: f32 = -60.0;
/// …and clarity no lower than this.
pub const SKIN_CLARITY_MIN: f32 = -40.0;
/// Skin is never sharpened: local sharpness at most 0 (negative softens).
pub const SKIN_SHARPNESS_MAX: f32 = 0.0;

/// The local hue limit in the field's own units (±100 = ±30°).
pub fn skin_hue_max() -> f32 {
    SKIN_HUE_MAX_DEG / LOCAL_HUE_FULL_DEG * 100.0
}

/// Does "Keep skin natural" hold a mask with this role?
pub fn guards_skin(role: MaskRole) -> bool {
    matches!(role, MaskRole::Skin | MaskRole::Face)
}

/// Pull one mask's skin controls back inside the limits; `true` when anything
/// moved. The role is the caller's question ([`guards_skin`]); this only
/// clamps.
pub fn keep_skin_natural(m: &mut LocalAdjustment) -> bool {
    let before = [m.saturation, m.hue, m.temperature, m.texture, m.clarity, m.sharpness];
    let hue = skin_hue_max();
    m.saturation = m.saturation.min(SKIN_SATURATION_MAX);
    m.hue = m.hue.clamp(-hue, hue);
    m.temperature = m.temperature.clamp(-SKIN_TEMPERATURE_MAX, SKIN_TEMPERATURE_MAX);
    m.texture = m.texture.max(SKIN_TEXTURE_MIN);
    m.clarity = m.clarity.max(SKIN_CLARITY_MIN);
    m.sharpness = m.sharpness.min(SKIN_SHARPNESS_MAX);
    before != [m.saturation, m.hue, m.temperature, m.texture, m.clarity, m.sharpness]
}

/// How many skin values of `recipe` lie past the limits — the count
/// [`keep_skin_natural_in`] would move, counted without moving anything. The
/// GUI reads it when a photo opens: a saved edit that already goes further
/// opens with the switch off, because opening never changes a value.
pub fn skin_values_beyond(recipe: &crate::recipe::EditRecipe) -> usize {
    let fields = |m: &LocalAdjustment| [m.saturation, m.hue, m.temperature, m.texture, m.clarity, m.sharpness];
    recipe
        .masks
        .iter()
        .filter(|m| guards_skin(m.role))
        .map(|m| {
            let mut held = m.clone();
            keep_skin_natural(&mut held);
            fields(m).iter().zip(fields(&held)).filter(|(a, b)| *a != b).count()
        })
        .sum()
}

/// [`keep_skin_natural`] over every skin-covering mask of `recipe`; `true`
/// when anything moved.
pub fn keep_skin_natural_in(recipe: &mut crate::recipe::EditRecipe) -> bool {
    recipe.masks.iter_mut().filter(|m| guards_skin(m.role)).fold(false, |moved, m| keep_skin_natural(m) | moved)
}

// ── The face masks a proposal asks for ──────────────────────────────────────
//
// A proposal may name a part of the picture by what it is (`kind=ai`); its
// `face` and `eyes` targets arrive here as a FACE or EYES mask that names no
// raster yet (`advisor::openai::expand_ai_mask_intents`) and are outlined on
// this computer — the model's coordinates are never read. `background` needs
// no help from here: it is the subject matte, inverted, which the segmenter
// already cuts (`segment::resolve_ai_masks`).

/// What [`resolve_face_masks`] did, for the caller's disclosure.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FaceMaskResolution {
    /// Masks that now name a raster — cache hits included.
    pub resolved: usize,
    /// Of those, the ones whose rasters were already on disk.
    pub cached: usize,
    /// Masks taken out of the recipe, each with the reason, in recipe order:
    /// a face mask with nothing to stand on would edit nothing, or — worse —
    /// fail an export that cannot read its raster.
    pub dropped: Vec<(String, String)>,
}

impl FaceMaskResolution {
    /// One English line for the CLI, or `None` when there was nothing to do.
    pub fn describe(&self) -> Option<String> {
        if self.resolved == 0 && self.dropped.is_empty() {
            return None;
        }
        let mut line = format!("{} face/eye mask(s) outlined on this computer by the face finder", self.resolved);
        if self.cached > 0 {
            line.push_str(&format!(" ({} from the photo's cache)", self.cached));
        }
        for (name, why) in &self.dropped {
            line.push_str(&format!("; \"{name}\" left out ({why})"));
        }
        Some(line)
    }
}

/// A face, eyes or skin mask that names no raster yet — the shape a
/// proposal's `face` / `eyes` target takes until [`resolve_face_masks`] runs.
pub fn is_face_intent(m: &LocalAdjustment) -> bool {
    matches!(m.role, MaskRole::Face | MaskRole::Eyes | MaskRole::Skin)
        && matches!(&m.mask, MaskGeometry::Bitmap { path } if path.is_empty())
}

/// Bump when the masks drawn from one face result change shape (a feather, a
/// grow factor), so cached rasters from an older build are drawn again.
const FACE_MASK_GENERATION: u32 = 1;
/// The longest edge the photo is shown to the face finder at, and the masks
/// drawn at: the masks are read in normalised coordinates at any render size.
const RESOLVE_EDGE: u32 = 2048;

/// The two cached rasters of one photo in one frame, keyed and homed on the
/// PHOTO like the segmenter's (`segment::resolve_ai_masks`): the photo's key,
/// the quarter turns the frame is in, and [`FACE_MASK_GENERATION`].
fn cached_rasters(raw: &Path, turns: u8) -> (PathBuf, PathBuf) {
    let ident = format!("{}|t{}|g{FACE_MASK_GENERATION}", crate::store::photo_key(raw), turns % 4);
    // FNV-1a: stable across runs, platforms and releases.
    let h = ident.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    (
        crate::store::raster_target(raw, &format!("mask-face-ai-{h:016x}")),
        crate::store::raster_target(raw, &format!("mask-eyes-ai-{h:016x}")),
    )
}

/// The photo's frame as the masks see it: `src`'s pixels in the recipe's
/// quarter turns, no longer than [`RESOLVE_EDGE`].
fn source_frame(src: &Path, turns: u8) -> Result<image::RgbImage> {
    let decoded = crate::decode::decode_any_turned(src, turns).with_context(|| format!("decode {}", src.display()))?;
    // `thumbnail` also ENLARGES a smaller frame to the box; only shrink.
    let p = decoded.preview;
    let p = if p.width().max(p.height()) > RESOLVE_EDGE { p.thumbnail(RESOLVE_EDGE, RESOLVE_EDGE) } else { p };
    Ok(p.to_rgb8())
}

/// Write a mask to `target` whole or not at all (a sibling, then a rename).
fn write_mask_atomically(mask: &GrayImage, target: &Path) -> Result<()> {
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let part = target.with_extension("part.png");
    mask.save_with_format(&part, image::ImageFormat::Png).with_context(|| format!("write {}", part.display()))?;
    std::fs::rename(&part, target).with_context(|| format!("publish {}", target.display()))?;
    crate::store::durable_adopt(target).with_context(|| format!("sync {}", target.display()))
}

/// Run the face finder on the photo's frame and draw the two rasters; the
/// frame comes back for the skin seed.
fn outline_faces(
    src: &Path,
    turns: u8,
    face_path: &Path,
    eyes_path: &Path,
    detect: impl FnOnce(&Path, &Path) -> Result<FaceReport>,
) -> Result<image::RgbImage> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let frame = source_frame(src, turns)?;
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("autoshade-face-src-{}-{n}", std::process::id()));
    let (input, json) = (tmp.with_extension("png"), tmp.with_extension("json"));
    frame.save(&input).with_context(|| format!("stage {}", input.display()))?;
    let report = detect(&input, &json);
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&json);
    let report = report?;
    if report.faces.is_empty() {
        bail!("no face was found in this photo");
    }
    let (w, h) = frame.dimensions();
    write_mask_atomically(&face_mask(&report, w, h), face_path)?;
    write_mask_atomically(&eyes_mask(&report, w, h), eyes_path)?;
    Ok(frame)
}

/// Give every face, eyes and skin mask that names no raster yet
/// ([`is_face_intent`]) its raster: the face finder runs once on the photo's
/// own frame (`src`'s pixels, in the recipe's quarter turns), and its masks
/// are cached beside the develop, so a re-render costs nothing. A skin mask
/// without a colour range gets one, seeded from the face. When no face is
/// found, or the face finder cannot run, those masks are taken out and the
/// reason is returned.
///
/// `raw` is the photo the cache is keyed and homed on; `src` the pixels the
/// render uses (a saved retouch master, when there is one) — the same split
/// as `segment::resolve_ai_masks`.
pub fn resolve_face_masks(
    cfg: &crate::config::Config,
    raw: &Path,
    src: &Path,
    recipe: &mut crate::recipe::EditRecipe,
) -> FaceMaskResolution {
    resolve_face_masks_with(raw, src, recipe, |input, output| detect_faces(cfg, input, output))
}

fn resolve_face_masks_with(
    raw: &Path,
    src: &Path,
    recipe: &mut crate::recipe::EditRecipe,
    detect: impl FnOnce(&Path, &Path) -> Result<FaceReport>,
) -> FaceMaskResolution {
    let mut out = FaceMaskResolution::default();
    if !recipe.masks.iter().any(is_face_intent) {
        return out;
    }
    let turns = recipe.quarter_turns % 4;
    let (face_path, eyes_path) = cached_rasters(raw, turns);
    let readable = |p: &Path| p.exists() && crate::render::open_mask_bounded(p).is_ok();
    let from_cache = readable(&face_path) && readable(&eyes_path);
    let (mut frame, failure) = if from_cache {
        (None, None)
    } else {
        match outline_faces(src, turns, &face_path, &eyes_path, detect) {
            Ok(frame) => (Some(frame), None),
            Err(e) => (None, Some(format!("{e:#}"))),
        }
    };
    let (face_s, eyes_s) = (face_path.to_string_lossy().into_owned(), eyes_path.to_string_lossy().into_owned());
    let mut seed: Option<Option<([f32; 3], [f32; 2])>> = None;
    recipe.masks.retain_mut(|m| {
        if !is_face_intent(m) {
            return true;
        }
        let name = if m.name.is_empty() { m.role.en_name().unwrap_or("unnamed").to_string() } else { m.name.clone() };
        if let Some(why) = &failure {
            out.dropped.push((name, why.clone()));
            return false;
        }
        if m.role == MaskRole::Skin && m.range.is_none() {
            let s = seed.get_or_insert_with(|| {
                let frame = match frame.take() {
                    Some(f) => f,
                    None => source_frame(src, turns).ok()?,
                };
                let face = crate::render::open_mask_bounded(&face_path).ok()?.to_luma8();
                skin_seed(&frame, &face)
            });
            let Some((c, at)) = *s else {
                out.dropped.push((name, "the face holds no pixel to take the skin colour from".into()));
                return false;
            };
            m.range = Some(RangeMask::Color { r: c[0], g: c[1], b: c[2], amount: SKIN_RANGE_AMOUNT, px: at[0], py: at[1] });
        }
        m.mask = MaskGeometry::Bitmap { path: if m.role == MaskRole::Eyes { eyes_s.clone() } else { face_s.clone() } };
        out.resolved += 1;
        if from_cache {
            out.cached += 1;
        }
        true
    });
    out
}

/// Write a mask under a freshly claimed raster name beside `src`'s develop
/// (`mask-face`, `mask-eyes`, …), durable before anything references it —
/// the same three steps every raster the app makes goes through.
pub fn save_mask(mask: &GrayImage, src: &Path, prefix: &str) -> Result<PathBuf> {
    let path = crate::store::claim_raster(src, prefix).with_context(|| format!("claim a {prefix} raster"))?;
    mask.save(&path).with_context(|| format!("write {}", path.display()))?;
    crate::store::durable_adopt(&path).with_context(|| format!("sync {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub of the sidecar's result: two square "faces" on a 400×200 frame,
    /// each with two small triangle eyes.
    fn stub() -> String {
        let face = |x0: f32, conf: &str| {
            format!(
                r#"{{"confidence":{conf},"box":[{x0},40,{x1},160],
                    "oval":[[{x0},40],[{x1},40],[{x1},160],[{x0},160]],
                    "left_eye":[[{lx0},80],[{lx1},80],[{lxm},92]],
                    "right_eye":[[{rx0},80],[{rx1},80],[{rxm},92]]}}"#,
                x1 = x0 + 120.0,
                lx0 = x0 + 70.0,
                lx1 = x0 + 100.0,
                lxm = x0 + 85.0,
                rx0 = x0 + 20.0,
                rx1 = x0 + 50.0,
                rxm = x0 + 35.0,
            )
        };
        format!(
            r#"{{"version":1,"backend":"stub","width":400,"height":200,"faces":[{},{}]}}"#,
            face(20.0, "0.93"),
            face(240.0, "null")
        )
    }

    fn at(m: &GrayImage, x: u32, y: u32) -> u8 {
        m.get_pixel(x, y)[0]
    }

    #[test]
    fn a_sidecar_result_parses_into_faces() {
        let r = parse_faces(&stub()).unwrap();
        assert_eq!((r.backend.as_str(), r.width, r.height, r.faces.len()), ("stub", 400, 200, 2));
        assert_eq!(r.faces[0].confidence, Some(0.93));
        assert_eq!(r.faces[1].confidence, None, "no matching detection is a face without a score");
        assert_eq!(r.faces[0].bbox, [20.0, 40.0, 140.0, 160.0]);
        assert_eq!(r.faces[0].oval.len(), 4);
        assert_eq!(r.faces[1].left_eye[0], [310.0, 80.0]);
        let none = parse_faces(r#"{"version":1,"backend":"stub","width":10,"height":10,"faces":[]}"#).unwrap();
        assert!(none.faces.is_empty(), "no face is a result, not an error");
    }

    /// Every malformed shape rejects the whole result, by a sentence.
    #[test]
    fn a_malformed_result_is_refused_whole() {
        let good = stub();
        let cases = [
            ("version", good.replace(r#""version":1"#, r#""version":2"#)),
            ("frame", good.replace(r#""width":400"#, r#""width":0"#)),
            ("confidence", good.replace("0.93", "1.5")),
            ("too far", good.replace("[20,40],", "[2000,40],")),
            ("too few", good.replace(r#"[[20,40],[140,40],[140,160],[20,160]]"#, "[[20,40],[140,40]]")),
            ("not json", "faces: none".to_string()),
        ];
        for (what, text) in cases {
            assert_ne!(text, good, "{what}: the case must change the stub");
            assert!(parse_faces(&text).is_err(), "{what} must be refused");
        }
        let many = format!(
            r#"{{"version":1,"backend":"s","width":10,"height":10,"faces":[{}]}}"#,
            vec![r#"{"confidence":null,"box":[0,0,1,1],"oval":[[0,0],[1,0],[1,1]],"left_eye":[[0,0],[1,0],[1,1]],"right_eye":[[0,0],[1,0],[1,1]]}"#; MAX_FACES + 1].join(",")
        );
        assert!(parse_faces(&many).is_err(), "more faces than the cap is refused");
    }

    /// The face mask is white inside each outline, black well outside, soft
    /// across the edge, and the two faces are one mask.
    #[test]
    fn the_face_mask_fills_feathers_and_unions_every_face() {
        let r = parse_faces(&stub()).unwrap();
        let m = face_mask(&r, 400, 200);
        assert_eq!(at(&m, 80, 100), 255, "inside the first face");
        assert_eq!(at(&m, 300, 100), 255, "inside the second face: the union");
        assert_eq!(at(&m, 190, 100), 0, "between the faces");
        assert_eq!(at(&m, 5, 5), 0);
        let edge = at(&m, 20, 100);
        assert!((60..=200).contains(&edge), "the outline's edge is feathered, not cut: {edge}");
        assert!(at(&m, 17, 100) > 0 && at(&m, 17, 100) < edge, "the feather fades outward");
    }

    /// Outlines scale with the frame, so a mask drawn at half size covers the
    /// same part of the picture.
    #[test]
    fn the_masks_land_in_the_same_place_at_any_size() {
        let r = parse_faces(&stub()).unwrap();
        let (full, half) = (face_mask(&r, 400, 200), face_mask(&r, 200, 100));
        for (x, y) in [(80u32, 100u32), (300, 100), (190, 100), (60, 60)] {
            let d = at(&full, x, y) as i32 - at(&half, x / 2, y / 2) as i32;
            assert!(d.abs() <= 40, "({x}, {y}): {} at full size, {} at half", at(&full, x, y), at(&half, x / 2, y / 2));
        }
    }

    /// The eyes mask holds the four eyes and nothing between them; the cheek
    /// below is not an eye.
    #[test]
    fn the_eyes_mask_holds_every_eye_and_nothing_else() {
        let r = parse_faces(&stub()).unwrap();
        let m = eyes_mask(&r, 400, 200);
        for x in [55u32, 105, 275, 325] {
            assert!(at(&m, x, 84) > 180, "eye centre at x={x}: {}", at(&m, x, 84));
        }
        // The stub's eyes are 20 px apart, so their two feathers just meet here.
        assert!(at(&m, 80, 84) < 8, "between one face's eyes: {}", at(&m, 80, 84));
        assert_eq!(at(&m, 80, 140), 0, "the cheek");
    }

    /// The fill is even-odd at pixel centres, so a concave outline keeps its
    /// notch.
    #[test]
    fn a_concave_outline_keeps_its_notch() {
        let mut m = GrayImage::new(20, 20);
        // An L: the top-right quarter is outside.
        fill_polygon(&mut m, &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [0.0, 20.0]]);
        assert_eq!(at(&m, 5, 5), 255);
        assert_eq!(at(&m, 15, 5), 0, "the notch");
        assert_eq!(at(&m, 15, 15), 255);
        assert_eq!(m.pixels().filter(|p| p[0] == 255).count(), 300, "exactly the L's 300 pixel centres");
    }

    /// The skin seed is the face's median colour, read through the mask at
    /// the reference's own size; the four builders carry their roles.
    #[test]
    fn the_skin_seed_and_the_four_portrait_masks() {
        let r = parse_faces(&stub()).unwrap();
        let mask = face_mask(&r, 400, 200);
        let skin = image::Rgb([200u8, 150, 120]);
        let reference = image::RgbImage::from_fn(200, 100, |x, _| {
            if (15..65).contains(&x) || (125..175).contains(&x) { skin } else { image::Rgb([20, 90, 30]) }
        });
        let (seed, centre) = skin_seed(&reference, &mask).expect("the face holds pixels");
        assert_eq!(seed, [200.0 / 255.0, 150.0 / 255.0, 120.0 / 255.0]);
        // The two faces' centres, x = 0.2 and 0.75 of the frame, y = 0.5.
        assert!((centre[0] - 0.475).abs() < 0.02 && (centre[1] - 0.5).abs() < 0.02, "{centre:?}");
        assert!(skin_seed(&reference, &GrayImage::new(4, 4)).is_none(), "an empty mask seeds nothing");

        let face = portrait_adjustment(MaskRole::Face, "mask-face.png".into());
        assert_eq!((face.role, face.inverted, face.range), (MaskRole::Face, false, None));
        assert_eq!(face.mask, MaskGeometry::Bitmap { path: "mask-face.png".into() });
        assert_eq!(portrait_adjustment(MaskRole::Eyes, "e.png".into()).role, MaskRole::Eyes);
        let s = skin_adjustment("mask-face.png".into(), seed, centre);
        assert_eq!(s.role, MaskRole::Skin);
        assert!(matches!(s.range, Some(RangeMask::Color { amount, .. }) if amount == SKIN_RANGE_AMOUNT));
        let b = background_adjustment("mask-subject.png".into());
        assert_eq!((b.role, b.inverted), (MaskRole::Background, true));
    }

    /// A missing script is refused with the variable that fixes it, before
    /// anything is spawned or any file is touched.
    #[test]
    fn a_missing_face_sidecar_is_named_with_its_override() {
        let mut cfg = crate::config::Config::load();
        cfg.face_script = "no-such-face-script.py".into();
        let out = crate::test_dir("face-missing").join("faces.json");
        let why = format!("{:#}", detect_faces(&cfg, Path::new("in.png"), &out).unwrap_err());
        assert!(why.contains("AUTOSHADE_FACE_SCRIPT"), "the refusal must name its override: {why}");
        assert!(!out.exists(), "nothing may be written when the sidecar is absent");
    }

    /// The bridge spawns nothing itself: the shared run holds the model slot
    /// and caps the result's size.
    #[test]
    fn the_face_run_goes_through_the_shared_model_sidecar() {
        let src = include_str!("face.rs");
        let body = src.split("pub fn detect_faces(").nth(1).unwrap().split("fn fill_polygon(").next().unwrap();
        assert!(body.contains("crate::run_model_sidecar_bounded(") && body.contains("Some(FACES_MAX_BYTES)"));
        for direct in ["Command::new", "sidecar_command("] {
            assert!(!body.contains(direct), "the bridge must not spawn through {direct}");
        }
    }

    /// The guardrail's table, one row per limit: a value past the limit comes
    /// back AT the limit, a value inside it is untouched, and no row moves
    /// any other field.
    ///
    /// MUTATION THIS CATCHES: delete (or loosen) any one of the six clamps in
    /// `keep_skin_natural` and its row fails; clamp a free direction (fading
    /// saturation, adding texture) and the "inside" column fails.
    #[test]
    fn the_skin_guardrail_holds_each_limit_and_nothing_else() {
        type Field = fn(&mut LocalAdjustment) -> &mut f32;
        let hue = skin_hue_max();
        assert!((hue - 26.666_666).abs() < 1e-3, "8° of the ±30° hue travel: {hue}");
        // (field, past the limit → the limit, a value inside that stays)
        let rows: [(&str, Field, f32, f32, f32); 9] = [
            ("saturation up", |m| &mut m.saturation, 60.0, SKIN_SATURATION_MAX, -80.0),
            ("hue +", |m| &mut m.hue, 100.0, hue, 20.0),
            ("hue −", |m| &mut m.hue, -100.0, -hue, -20.0),
            ("warmer", |m| &mut m.temperature, 70.0, SKIN_TEMPERATURE_MAX, 15.0),
            ("cooler", |m| &mut m.temperature, -70.0, -SKIN_TEMPERATURE_MAX, -15.0),
            ("texture", |m| &mut m.texture, -100.0, SKIN_TEXTURE_MIN, 40.0),
            ("clarity", |m| &mut m.clarity, -100.0, SKIN_CLARITY_MIN, 30.0),
            ("sharpness", |m| &mut m.sharpness, 40.0, SKIN_SHARPNESS_MAX, -50.0),
            ("saturation at the limit", |m| &mut m.saturation, 15.0, 15.0, 15.0),
        ];
        for (what, field, past, limit, inside) in rows {
            let mut m = LocalAdjustment { role: MaskRole::Skin, exposure_ev: 0.3, ..Default::default() };
            *field(&mut m) = past;
            let moved = keep_skin_natural(&mut m);
            assert_eq!(*field(&mut m), limit, "{what}: past the limit comes back to it");
            assert_eq!(moved, past != limit, "{what}: reports whether it moved");
            let mut expect = LocalAdjustment { role: MaskRole::Skin, exposure_ev: 0.3, ..Default::default() };
            *field(&mut expect) = limit;
            assert_eq!(m, expect, "{what}: no other field moves");
            *field(&mut m) = inside;
            assert!(!keep_skin_natural(&mut m), "{what}: {inside} is inside");
            assert_eq!(*field(&mut m), inside);
        }
    }

    /// The guardrail holds the skin and face masks and no other: the eyes
    /// may be sharpened and the background pushed as far as the user likes.
    #[test]
    fn the_skin_guardrail_holds_only_the_masks_that_cover_skin() {
        let pushed = |role| LocalAdjustment { role, saturation: 50.0, sharpness: 60.0, texture: -90.0, ..Default::default() };
        let mut r = crate::recipe::EditRecipe {
            masks: [MaskRole::Skin, MaskRole::Face, MaskRole::Eyes, MaskRole::Background, MaskRole::Custom, MaskRole::ZoneSky]
                .into_iter()
                .map(pushed)
                .collect(),
            ..Default::default()
        };
        let before = r.clone();
        assert_eq!(skin_values_beyond(&r), 6, "three past the limits on each of the two skin-covering masks");
        assert_eq!(r, before, "counting moves nothing");
        assert!(keep_skin_natural_in(&mut r));
        assert_eq!(skin_values_beyond(&r), 0);
        for m in &r.masks {
            let held = (m.saturation, m.sharpness, m.texture) == (SKIN_SATURATION_MAX, 0.0, SKIN_TEXTURE_MIN);
            assert_eq!(held, guards_skin(m.role), "{:?}", m.role);
            assert_eq!(held, matches!(m.role, MaskRole::Skin | MaskRole::Face), "{:?}", m.role);
        }
        assert!(!keep_skin_natural_in(&mut r), "a second pass moves nothing");
    }

    /// A photo the store keys by path: a flat skin-coloured square on green,
    /// as a PNG on disk, and its develop folder scrubbed afterwards.
    fn photo(tag: &str) -> (PathBuf, impl Drop) {
        struct Scrub(Vec<PathBuf>);
        impl Drop for Scrub {
            fn drop(&mut self) {
                for d in &self.0 {
                    let _ = std::fs::remove_dir_all(d);
                }
            }
        }
        let dir = crate::test_dir(&format!("face-resolve-{tag}"));
        let src = dir.join("portrait.png");
        image::RgbImage::from_fn(400, 200, |x, y| {
            if (20..140).contains(&x) && (40..160).contains(&y) { image::Rgb([200, 150, 120]) } else { image::Rgb([20, 90, 30]) }
        })
        .save(&src)
        .unwrap();
        let dev = crate::store::develop_dir(&src);
        let _ = std::fs::remove_dir_all(&dev);
        (src, Scrub(vec![dir, dev]))
    }

    fn intents() -> crate::recipe::EditRecipe {
        let intent = |role, name: &str| LocalAdjustment {
            role,
            name: name.into(),
            texture: -30.0,
            mask: MaskGeometry::Bitmap { path: String::new() },
            ..Default::default()
        };
        crate::recipe::EditRecipe {
            masks: vec![
                intent(MaskRole::Face, "Soften the face"),
                LocalAdjustment { name: "Mine".into(), mask: MaskGeometry::Bitmap { path: "mine.png".into() }, ..Default::default() },
                intent(MaskRole::Eyes, ""),
                intent(MaskRole::Skin, "Skin"),
            ],
            ..Default::default()
        }
    }

    /// The proposal's face, eyes and skin intents get their rasters from ONE
    /// face-finder run on the photo's own frame; the skin mask gets its colour
    /// range from the face; a mask that was already drawn is not touched; a
    /// second develop reads the cache and runs nothing.
    #[test]
    fn face_intents_are_outlined_once_and_cached() {
        let (src, _scrub) = photo("once");
        let mut r = intents();
        let mut runs = 0;
        let res = resolve_face_masks_with(&src, &src, &mut r, |input, _| {
            runs += 1;
            let frame = image::open(input).unwrap();
            assert_eq!((frame.width(), frame.height()), (400, 200), "the photo's own frame is shown");
            Ok(FaceReport { faces: parse_faces(&stub()).unwrap().faces[..1].to_vec(), ..parse_faces(&stub()).unwrap() })
        });
        assert_eq!(runs, 1);
        assert_eq!((res.resolved, res.cached, res.dropped.len()), (3, 0, 0), "{res:?}");
        assert!(res.describe().unwrap().starts_with("3 face/eye mask(s)"));
        let path = |m: &LocalAdjustment| match &m.mask {
            MaskGeometry::Bitmap { path } => path.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(path(&r.masks[1]), "mine.png", "a drawn mask is left alone");
        let (face, eyes) = (path(&r.masks[0]), path(&r.masks[2]));
        assert_eq!(path(&r.masks[3]), face, "skin rides the face raster");
        assert_ne!(face, eyes);
        let face_img = image::open(&face).unwrap().to_luma8();
        assert_eq!(face_img.get_pixel(80, 100)[0], 255, "the face is where the finder put it");
        assert_eq!(face_img.get_pixel(300, 100)[0], 0, "only the one face the finder reported");
        assert!(image::open(&eyes).unwrap().to_luma8().get_pixel(55, 84)[0] > 180);
        match r.masks[3].range {
            Some(RangeMask::Color { r: cr, g, b, amount, .. }) => {
                assert_eq!([cr, g, b], [200.0 / 255.0, 150.0 / 255.0, 120.0 / 255.0], "seeded from the face's own colour");
                assert_eq!(amount, SKIN_RANGE_AMOUNT);
            }
            ref other => panic!("the skin mask's range: {other:?}"),
        }
        assert!(r.masks.iter().all(|m| !is_face_intent(m)));

        let mut again = intents();
        let res = resolve_face_masks_with(&src, &src, &mut again, |_, _| panic!("the cache answers"));
        assert_eq!((res.resolved, res.cached), (3, 3));
        assert_eq!(again.masks, r.masks, "the same masks from the cache, the skin seed included");
        let mut none = crate::recipe::EditRecipe::default();
        assert_eq!(resolve_face_masks_with(&src, &src, &mut none, |_, _| panic!("nothing asks")), Default::default());
    }

    /// A recipe FILE carrying an intent (a hand-written one, `apply`'s input)
    /// keeps it through the load: anchoring raster names to the recipe's
    /// folder skips an empty path, which names no file, while a real one is
    /// still anchored.
    ///
    /// MUTATION THIS CATCHES: drop the empty-path guard in
    /// `LocalAdjustment::bitmap_paths_mut` and the intent becomes the folder.
    #[test]
    fn a_face_intent_survives_the_recipe_files_path_anchoring() {
        let mut r = intents();
        let base = crate::test_dir("face-intent-anchor");
        crate::store::resolve_mask_paths(&mut r, &base);
        assert!(is_face_intent(&r.masks[0]) && is_face_intent(&r.masks[2]) && is_face_intent(&r.masks[3]));
        match &r.masks[1].mask {
            MaskGeometry::Bitmap { path } => assert_eq!(Path::new(path), base.join("mine.png"), "a real name is anchored"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// No face in the photo, or no face finder on this computer: the intents
    /// are taken out, each named with the reason, and the rest of the recipe
    /// stands. Nothing is cached, so the next develop asks again.
    #[test]
    fn face_intents_with_no_face_are_taken_out_and_named() {
        let (src, _scrub) = photo("none");
        let mut r = intents();
        let res = resolve_face_masks_with(&src, &src, &mut r, |_, _| {
            Ok(FaceReport { faces: Vec::new(), ..parse_faces(&stub()).unwrap() })
        });
        assert_eq!(res.resolved, 0);
        let names: Vec<&str> = res.dropped.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Soften the face", "Eyes", "Skin"], "named by the proposal's name, else the role's");
        assert!(res.dropped.iter().all(|(_, why)| why.contains("no face was found")), "{res:?}");
        assert_eq!(r.masks.len(), 1, "only the drawn mask stands");
        assert!(res.describe().unwrap().contains("left out (no face was found in this photo)"));
        let (face, eyes) = cached_rasters(&src, 0);
        assert!(!face.exists() && !eyes.exists(), "a miss caches nothing");

        let mut r = intents();
        let res = resolve_face_masks_with(&src, &src, &mut r, |_, _| bail!("set AUTOSHADE_FACE_SCRIPT"));
        assert_eq!(res.dropped.len(), 3);
        assert!(res.dropped[0].1.contains("AUTOSHADE_FACE_SCRIPT"), "the sidecar's own reason: {res:?}");
    }
}
