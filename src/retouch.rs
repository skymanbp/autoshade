//! Pixel-level RETOUCH (heal) — an OPTIONAL mode, distinct from BOTH other paths:
//!
//!   * the **parametric develop** path (`EditRecipe` → XMP / render): the AI only
//!     turns sliders, "never touches a pixel"; output is reproducible + a sidecar.
//!   * the **generative** path (`generative.rs`, gpt-image): SYNTHESISES new pixels.
//!
//! This mode is traditional spot-healing: it REMOVES small defects (dust, sensor
//! spots, blemishes, tiny distractions) by sampling SURROUNDING REAL pixels and
//! blending them over the defect — exactly a retoucher's heal tool. By
//! construction it only ever copies / shifts / averages pixels that ALREADY exist
//! in the photo; it never invents content. That is the architectural guarantee
//! that this is *retouching, not generation* (the user's hard constraint).
//!
//! Targeting is hybrid: a vision model can auto-detect spots ([`detect_spots`])
//! AND the user can paint regions in the UI ([`plan_from_mask`]); both feed the
//! same deterministic [`heal_image`] engine. Output is a pixel master in ./out
//! (no XMP — pixel edits aren't ACR-serialisable).

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use image::{DynamicImage, ImageBuffer, Rgb, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::Config;

/// Runtime-only shape of one painted component. The pixels are bit-packed
/// relative to the component's own bounding box, while the mask dimensions
/// preserve the normalised mapping to any heal-image resolution.
///
/// `PartialEq` compares the bitset verbatim, which is what a `HealSpot` inside
/// an `EditRecipe` needs (v1.5.0 F9): two spots are the same spot when they
/// cover the same pixels, and the bounding box is part of that answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotCoverage {
    mask_width: u32,
    mask_height: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    bits: Vec<u64>,
}

impl SpotCoverage {
    fn pixel(&self, x: i32, y: i32) -> f32 {
        let lx = x - self.x as i32;
        let ly = y - self.y as i32;
        if lx < 0 || ly < 0 || lx >= self.width as i32 || ly >= self.height as i32 {
            return 0.0;
        }
        let i = ly as usize * self.width as usize + lx as usize;
        ((self.bits[i / 64] >> (i % 64)) & 1) as f32
    }

    /// The painted weight at a NORMALISED frame coordinate — the exact shape
    /// rather than the enclosing disk.
    ///
    /// Public since v1.5.0 F9 because the GUI has to paint an imported area
    /// into the shared brush mask before handing it to the generative verb,
    /// and a disk over a long thin stroke would ask the model to regenerate a
    /// circle of content the photographer never touched.
    pub fn weight_at(&self, nx: f32, ny: f32) -> f32 {
        self.alpha_at(nx, ny)
    }

    /// Bilinear sampling makes boundary pixels fractional when the painted
    /// canvas and healed image differ in resolution.
    fn alpha_at(&self, nx: f32, ny: f32) -> f32 {
        let x = nx.clamp(0.0, 1.0) * self.mask_width as f32 - 0.5;
        let y = ny.clamp(0.0, 1.0) * self.mask_height as f32 - 0.5;
        let x0 = x.floor() as i32;
        let y0 = y.floor() as i32;
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let top = self.pixel(x0, y0) * (1.0 - fx) + self.pixel(x0 + 1, y0) * fx;
        let bottom =
            self.pixel(x0, y0 + 1) * (1.0 - fx) + self.pixel(x0 + 1, y0 + 1) * fx;
        (top * (1.0 - fy) + bottom * fy).clamp(0.0, 1.0)
    }
}

/// What made the pixels Lightroom was showing inside a retouch area (v1.5.0
/// F9) — the FILL axis of `crs:RetouchAreas`.
///
/// Two independent facts live in one of Lightroom's areas and it is worth
/// keeping them apart, because conflating them is what made this reader's
/// first census wrong. The fill is what this enum carries. The GEOMETRY is a
/// separate axis, carried where this engine already carries geometry: an
/// ellipse becomes the spot's `cx`/`cy`/`radius`, a brush stroke becomes its
/// [`SpotCoverage`]. Measured over the reference library's 121 areas: 84 are
/// an ellipse and 37 are a brush, crossed with the three fills below.
///
/// This engine repairs every one of them the same way, by copying real pixels
/// out of the same frame. The distinction is not a rendering switch — it is
/// what the panel has to be able to say, because for 116 of the 121 the pixels
/// the photographer approved are NOT in the sidecar and are not ours to
/// reproduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SpotOrigin {
    /// This engine's own: painted in the UI, or proposed by the detector.
    #[default]
    Painted,
    /// Lightroom `crs:SpotType="heal"` (5 areas) — a plain donor copy, and the
    /// only kind whose donor Lightroom actually names (`crs:SourceX`,
    /// `crs:OffsetY`). The one fill this engine reproduces exactly, because
    /// there is nothing to reproduce beyond the copy itself.
    LightroomHeal,
    /// Lightroom `crs:SpotType="heal_patchmatch"` with no `crs:fill_method`
    /// (99) — Adobe's CLASSICAL content-aware fill. PatchMatch synthesises the
    /// patch from the frame's own texture; no model is involved, and the
    /// result still lives in Adobe's own store rather than in the sidecar.
    LightroomContentAware,
    /// The same, with `crs:fill_method="firefly"` (17) — a generative model's
    /// output, stamped with `crs:pm_clio_model_version` (three distinct
    /// versions in the library). These are the ones no amount of local work
    /// reconstructs, because their pixels were never in the photograph.
    LightroomGenerative,
}

impl SpotOrigin {
    /// Did LIGHTROOM synthesise this area's pixels rather than copy them?
    /// True for both patchmatch arms — the classical one invents texture as
    /// surely as the model does, and neither result is in the sidecar. This is
    /// the question the panel asks to decide whether to offer the ✨ upgrade.
    pub fn is_synthesised(self) -> bool {
        matches!(self, Self::LightroomContentAware | Self::LightroomGenerative)
    }
}

/// One area of Lightroom's spot removal (`crs:RetouchAreas`), as the sidecar
/// states it — v1.5.0 F9.
///
/// Deliberately NOT a [`HealSpot`]. A spot is this engine's RENDER shape: one
/// disk, in short-side units, at one resolution. An area is the photographer's
/// STATEMENT, and two of its three shapes have no disk at all until something
/// rasterises them. Keeping them apart is what lets `render` build the spots
/// for the frame it is actually working on, the way every mask geometry is
/// already handled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetouchArea {
    /// What made Lightroom's own pixels here — see [`SpotOrigin`].
    pub origin: SpotOrigin,
    /// `crs:Feather`, 0..1. Stated by the 5 `heal` areas (all of them 0.5) and
    /// by none of the 116 patchmatch ones, which is why the default is this
    /// engine's own rather than zero: a hard-edged patch seam is not what
    /// Lightroom showed, and an absent attribute is not a photographer
    /// choosing 0.
    pub feather: f32,
    /// `crs:SourceX` / `crs:OffsetY` — the donor's centre, and ABSOLUTE
    /// normalised frame coordinates despite what `OffsetY` sounds like.
    ///
    /// Measured on all 5 areas that state one: read as a relative offset, one
    /// of them lands at 0.9461 + 0.8161 = 1.76, which is off the image, and
    /// three of the five fail the same way. Read as absolute, all five give a
    /// plausible donor. `None` on the 116 patchmatch areas — Adobe records a
    /// SEARCH WINDOW there and not a donor, so this engine finds its own.
    pub donor: Option<[f32; 2]>,
    /// Where the area is.
    pub shape: RetouchShape,
}

impl Default for RetouchArea {
    fn default() -> Self {
        Self {
            origin: SpotOrigin::Painted,
            feather: HealSpot::default().feather,
            donor: None,
            shape: RetouchShape::default(),
        }
    }
}

/// The GEOMETRY axis of a retouch area — the second of the two independent
/// facts inside one of Lightroom's areas (the first is [`SpotOrigin`]).
///
/// Measured over the reference library's 121 areas: 84 ellipses and 37
/// brushes, every ellipse written as an attribute-only `<rdf:li/>` and every
/// brush as a nested `<rdf:Description>` with its own `<crs:Dabs>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RetouchShape {
    /// `crs:What="Mask/Ellipse"` (84). `cx`/`cy` are per-axis fractions of the
    /// frame; `size_x`/`size_y` are half-extents in WIDTH units on BOTH axes,
    /// the same convention `render::BrushDab` records for `crs:Radius` — a
    /// spot is a circle in PIXELS, so `size_x == size_y` on all 84 and the y
    /// half-extent in normalised coordinates is `size_y · W/H`.
    Ellipse { cx: f32, cy: f32, size_x: f32, size_y: f32 },
    /// `crs:What="Mask/Paint"` (37 areas, 39 components) — a brush stroke,
    /// carried as the same [`BrushStroke`] the mask reader already produces so
    /// that one dab grammar, one rasteriser and one sensor-frame rotation
    /// serve both.
    ///
    /// [`BrushStroke`]: crate::recipe::BrushStroke
    Brush(Vec<crate::recipe::BrushStroke>),
}

impl Default for RetouchShape {
    fn default() -> Self {
        Self::Ellipse { cx: 0.5, cy: 0.5, size_x: 0.02, size_y: 0.02 }
    }
}

/// One heal target: a normalised circular region to repair by sampling nearby
/// real pixels. `cx`/`cy` are 0..1 of the frame; `radius` is a fraction of the
/// SHORT side. `source` is an optional explicit donor offset (normalised frame
/// units) — `None` auto-searches the best clean donor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HealSpot {
    pub cx: f32,
    pub cy: f32,
    pub radius: f32,
    pub feather: f32,
    pub source: Option<[f32; 2]>,
    /// Exact painted component shape. AI-proposed circular spots leave this
    /// unset; a saved manual retouch persists its finished pixel master, so
    /// the raster never needs to survive serialisation.
    #[serde(skip)]
    pub coverage: Option<SpotCoverage>,
    /// CLONE semantics: copy the donor verbatim (feathered edge only), skipping
    /// the border tone-matching that makes a *heal* blend. This is Photoshop's
    /// clone stamp vs its healing brush — texture transplant vs seamless repair.
    pub clone_raw: bool,
    pub label: String,
    /// Provenance (v1.5.0 F9). `#[serde(default)]` on the struct means every
    /// plan written before this field reads back as [`SpotOrigin::Painted`],
    /// which is what those plans were.
    pub origin: SpotOrigin,
}

impl Default for HealSpot {
    fn default() -> Self {
        Self {
            cx: 0.5,
            cy: 0.5,
            radius: 0.02,
            feather: 0.4,
            source: None,
            coverage: None,
            clone_raw: false,
            label: String::new(),
            origin: SpotOrigin::Painted,
        }
    }
}

/// A retouch plan: the spots to heal plus provenance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RetouchPlan {
    pub spots: Vec<HealSpot>,
    pub rationale: String,
    pub confidence: f32,
}

/// Outcome of a heal run, for the CLI / UI to report.
pub struct HealReport {
    /// Spots actually HEALED — pixels written. A planned spot the engine had
    /// to leave untouched (see `skipped`) is not one of them, so "N spots
    /// healed" never claims a repair the picture does not show.
    pub spots: usize,
    /// Planned spots left untouched because no donor disk of the spot's own
    /// size fits inside the frame around it (`find_donor` found nothing):
    /// auto-placed blobs of roughly a sixth of the short edge or more. Always
    /// zero for a clone stamp, whose donor the user picked.
    pub skipped: usize,
    pub rationale: String,
    pub dims: (u32, u32),
    /// The rationale's DETERMINISTIC tail as typed notes (L12#2B):
    /// `rationale` == the AI detector's own prose (possibly empty) followed
    /// by `rationale::render_en(&notes)` byte-for-byte, so the GUI strips
    /// the suffix and renders it localized while the CLI printout and the
    /// `X-Heal-Rationale` header keep the English string. In-process only.
    pub notes: Vec<crate::rationale::Note>,
}

// --- the deterministic heal engine -----------------------------------------

/// The channel depth the heal engine runs at: u8 masters (baked 8-bit
/// sources) and u16 masters (RAW develops, 16-bit TIFF/PNG). The math stays
/// f32 either way; `from_chan` clamps to the depth's range. Forcing every
/// source through u8 quantised 16-bit pixels — U30's headline finding.
pub trait HealDepth: image::Primitive {
    fn chan_f32(self) -> f32;
    fn from_chan(v: f32) -> Self;
}
impl HealDepth for u8 {
    fn chan_f32(self) -> f32 {
        self as f32
    }
    fn from_chan(v: f32) -> Self {
        v.round().clamp(0.0, 255.0) as u8
    }
}
impl HealDepth for u16 {
    fn chan_f32(self) -> f32 {
        self as f32
    }
    fn from_chan(v: f32) -> Self {
        v.round().clamp(0.0, 65535.0) as u16
    }
}

/// The two halves of what the heal operator wants from a picture: neighbourhood
/// READS from a pristine source, and WRITES into the target. They are two
/// traits rather than one because the ends are not always the same buffer, and
/// they exist at all because of where v1.5.0 F9 had to put this operator.
///
/// F9 renders Lightroom's `crs:RetouchAreas` inside the develop chain, whose
/// plane is `[[f32; 3]]` and whose values run past 1.0 wherever a highlight
/// did. The cheap way to reuse the operator would have been to quantise that
/// rect to a 16-bit `ImageBuffer` and hand it to the existing path — and that
/// clamps, so a spot healed beside a specular highlight would come back with
/// the highlight crushed to white. Splitting read from write is what keeps the
/// pixels in f32 from end to end, at the cost of these two small traits.
trait HealRead {
    /// Frame size in pixels, in the signed arithmetic the operator works in.
    fn dims(&self) -> (i32, i32);

    /// One pixel as f32 RGB, coordinates CLAMPED to the image edge.
    ///
    /// The UNITS are whatever the buffer stores — 0..255 for an 8-bit image,
    /// 0..1-and-above for the develop plane — and nothing here needs to know
    /// which. Every quantity the operator forms (the border SSD, the donor
    /// interior variance, the low-frequency correction) is quadratic or linear
    /// in the pixel value, so a global scale factor ranks candidate donors in
    /// exactly the same order.
    fn get(&self, x: i32, y: i32) -> [f32; 3];
}

/// The write half — see [`HealRead`] for why the two are separate.
trait HealWrite {
    /// Store one pixel. Coordinates are in bounds by construction: every
    /// caller has already dropped the out-of-frame target and donor.
    fn put(&mut self, x: i32, y: i32, v: [f32; 3]);
}

impl<T: HealDepth> HealRead for ImageBuffer<Rgb<T>, Vec<T>>
where
    Rgb<T>: image::Pixel<Subpixel = T>,
{
    fn dims(&self) -> (i32, i32) {
        (self.width() as i32, self.height() as i32)
    }

    fn get(&self, x: i32, y: i32) -> [f32; 3] {
        let xx = x.clamp(0, self.width() as i32 - 1) as u32;
        let yy = y.clamp(0, self.height() as i32 - 1) as u32;
        let p = self.get_pixel(xx, yy).0;
        [p[0].chan_f32(), p[1].chan_f32(), p[2].chan_f32()]
    }
}

impl<T: HealDepth> HealWrite for ImageBuffer<Rgb<T>, Vec<T>>
where
    Rgb<T>: image::Pixel<Subpixel = T>,
{
    fn put(&mut self, x: i32, y: i32, v: [f32; 3]) {
        let out = [T::from_chan(v[0]), T::from_chan(v[1]), T::from_chan(v[2])];
        self.put_pixel(x as u32, y as u32, Rgb(out));
    }
}

/// A borrowed read view of the develop chain's own pixel plane.
struct PlaneRef<'a> {
    px: &'a [[f32; 3]],
    w: i32,
    h: i32,
}

/// A borrowed write view of the same — see [`heal_planar`].
struct PlaneMut<'a> {
    px: &'a mut [[f32; 3]],
    w: i32,
}

impl HealRead for PlaneRef<'_> {
    fn dims(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    fn get(&self, x: i32, y: i32) -> [f32; 3] {
        let xx = x.clamp(0, self.w - 1) as usize;
        let yy = y.clamp(0, self.h - 1) as usize;
        self.px[yy * self.w as usize + xx]
    }
}

impl HealWrite for PlaneMut<'_> {
    fn put(&mut self, x: i32, y: i32, v: [f32; 3]) {
        self.px[y as usize * self.w as usize + x as usize] = v;
    }
}

/// The spot loop both entry points share: place each spot in pixels, settle its
/// donor, heal it. `src` is the PRISTINE frame and `dst` the one being written,
/// which is what makes each spot's output a pure function of the source.
///
/// Returns how many spots were actually healed. An auto-placed spot for which
/// [`find_donor`] finds no donor disk inside the frame is left untouched and
/// NOT counted — it used to be, and "1 spot healed" over an unchanged picture
/// is the claim this count exists to refuse.
fn heal_spots<S: HealRead, D: HealWrite>(src: &S, dst: &mut D, spots: &[HealSpot]) -> usize {
    let (w, h) = src.dims();
    let short = w.min(h) as f32;
    let mut healed = 0usize;
    for s in spots {
        let cx = s.cx.clamp(0.0, 1.0) * w as f32;
        let cy = s.cy.clamp(0.0, 1.0) * h as f32;
        let r = (s.radius.clamp(0.0, 0.5) * short).max(2.0);
        let off = match s.source {
            Some([sx, sy]) => ((sx * w as f32).round() as i32, (sy * h as f32).round() as i32),
            None => match find_donor(src, cx, cy, r) {
                Some(off) => off,
                None => continue,
            },
        };
        heal_one(
            src,
            dst,
            cx,
            cy,
            r,
            s.feather.clamp(0.0, 1.0),
            off,
            s.clone_raw,
            s.coverage.as_ref(),
        );
        healed += 1;
    }
    healed
}

/// Apply every spot to `img` in place, healing from surrounding real pixels.
/// Donors AND blend bases are read from a snapshot of the ORIGINAL image, so
/// no spot ever reads a half-written region and each spot's output is a pure
/// function of the source. Where spots OVERLAP, the later spot's result wins
/// (deliberate last-writer rule — not order-independent in the overlap).
/// Returns the number of spots healed, as [`heal_spots`] counts them.
pub fn heal_image<T: HealDepth>(
    img: &mut ImageBuffer<Rgb<T>, Vec<T>>,
    spots: &[HealSpot],
) -> usize
where
    Rgb<T>: image::Pixel<Subpixel = T>,
{
    if spots.is_empty() {
        return 0;
    }
    let src = img.clone();
    heal_spots(&src, img, spots)
}

/// [`heal_image`] on the develop chain's float plane, for the retouch areas
/// this photograph arrived carrying (v1.5.0 F9). Same operator, same snapshot
/// rule, same last-writer overlap — the only difference is that nothing is
/// quantised on the way through, so a highlight inside a healed spot keeps the
/// value the sensor recorded.
///
/// The snapshot is a full copy of the plane, which is what `heal_image` has
/// always done and what the full-resolution `heal` path already pays. It is
/// taken ONLY when there is at least one spot, so the photographs that carry
/// none — 150 of the 175 measured — pay nothing at all.
///
/// Returns the number of spots healed, as [`heal_spots`] counts them.
pub fn heal_planar(data: &mut [[f32; 3]], w: usize, h: usize, spots: &[HealSpot]) -> usize {
    if spots.is_empty() || w == 0 || h == 0 || data.len() < w * h {
        return 0;
    }
    let snapshot = data.to_vec();
    let src = PlaneRef { px: &snapshot, w: w as i32, h: h as i32 };
    let mut dst = PlaneMut { px: data, w: w as i32 };
    heal_spots(&src, &mut dst, spots)
}

/// Search candidate donor offsets on rings around the spot; pick the one whose
/// surroundings best match the spot's border (so the patch is seamless) and that
/// stays in-bounds. Returns a pixel offset (dx, dy) from the spot centre, or
/// `None` when no ring position keeps a donor disk of the spot's size inside
/// the frame — a blob of roughly a sixth of the short edge or more, on a 4000
/// × 3000 frame anything past ~440 px of radius. The caller then leaves the
/// spot untouched and does not count it: this used to answer the sentinel
/// `(0, 0)`, which `heal_one` skipped while the run still reported the spot
/// as healed.
fn find_donor<S: HealRead>(src: &S, cx: f32, cy: f32, r: f32) -> Option<(i32, i32)> {
    let (wi, hi) = src.dims();
    let (w, h) = (wi as f32, hi as f32);
    let mut best = None;
    let mut best_score = f32::INFINITY;
    for dist_mul in [2.4f32, 3.2, 4.4] {
        let dist = r * dist_mul;
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            let (vx, vy) = (dist * a.cos(), dist * a.sin());
            let (dcx, dcy) = (cx + vx, cy + vy);
            // donor disk must stay fully in-bounds
            if dcx - r < 0.0 || dcx + r >= w || dcy - r < 0.0 || dcy + r >= h {
                continue;
            }
            // score = SSD of the border ring (spot surroundings vs the donor's,
            // shifted by v) + a smoothness penalty on the donor interior. Low =
            // donor blends well and isn't itself a busy feature.
            let mut ssd = 0.0f32;
            let mut mean = [0.0f32; 3];
            let mut m2 = [0.0f32; 3];
            let rr = r * 1.2;
            for j in 0..24 {
                let b = j as f32 / 24.0 * std::f32::consts::TAU;
                let (ox, oy) = (b.cos(), b.sin());
                let tp = src.get((cx + ox * rr) as i32, (cy + oy * rr) as i32);
                let dp = src.get((cx + vx + ox * rr) as i32, (cy + vy + oy * rr) as i32);
                for c in 0..3 {
                    let d = tp[c] - dp[c];
                    ssd += d * d;
                }
                let ip = src.get((dcx + ox * r * 0.5) as i32, (dcy + oy * r * 0.5) as i32);
                for c in 0..3 {
                    mean[c] += ip[c];
                    m2[c] += ip[c] * ip[c];
                }
            }
            let n = 24.0f32;
            let mut var = 0.0;
            for c in 0..3 {
                let mu = mean[c] / n;
                var += (m2[c] / n - mu * mu).max(0.0);
            }
            let score = ssd + var * 0.5;
            if score < best_score {
                best_score = score;
                best = Some((vx.round() as i32, vy.round() as i32));
            }
        }
    }
    best
}

/// Copy the donor disk (spot centre + offset) over the spot, correcting its mean
/// to the spot's border (the "heal" vs "clone" part) and feathering the edge.
/// With `clone_raw` the correction is skipped — donor pixels land verbatim (the
/// clone stamp), which is exactly what you want when transplanting texture and
/// exactly wrong when repairing into different-toned surroundings.
#[allow(clippy::too_many_arguments)] // internal helper mirroring HealSpot's fields
fn heal_one<S: HealRead, D: HealWrite>(
    src: &S,
    dst: &mut D,
    cx: f32,
    cy: f32,
    r: f32,
    feather: f32,
    off: (i32, i32),
    clone_raw: bool,
    coverage: Option<&SpotCoverage>,
) {
    if off == (0, 0) {
        // The donor IS the spot (an explicit source picked on the spot itself):
        // copying it onto itself writes nothing, so skip the work. Auto-heal
        // never lands here — `find_donor` answers `None` instead of an offset
        // when nothing around the spot can donate.
        return;
    }
    // From the SOURCE, which is the same frame as the destination by
    // construction and is the one end of the pair that can be measured.
    let (w, h) = src.dims();
    let (ox, oy) = off;
    // Low-frequency correction: shift the donor so its border matches the spot's
    // border — this is what makes a *heal* blend where a raw *clone* would seam.
    let mut corr = [0.0f32; 3];
    if !clone_raw {
        for j in 0..24 {
            let a = j as f32 / 24.0 * std::f32::consts::TAU;
            let (dx, dy) = (a.cos() * r * 1.15, a.sin() * r * 1.15);
            let tp = src.get((cx + dx) as i32, (cy + dy) as i32);
            let dp = src.get((cx + dx) as i32 + ox, (cy + dy) as i32 + oy);
            for c in 0..3 {
                corr[c] += tp[c] - dp[c];
            }
        }
        for v in &mut corr {
            *v /= 24.0;
        }
    }

    let r_i = r.ceil() as i32;
    let inner = r * (1.0 - feather);
    for dy in -r_i..=r_i {
        for dx in -r_i..=r_i {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d > r {
                continue;
            }
            // feathered weight: 1 inside `inner`, ramping to 0 at the edge.
            let mut alpha = if d <= inner {
                1.0
            } else if r > inner {
                (r - d) / (r - inner)
            } else {
                1.0
            };
            let (tx, ty) = (cx as i32 + dx, cy as i32 + dy);
            if tx < 0 || ty < 0 || tx >= w || ty >= h {
                continue;
            }
            // A painted spot heals only its actual raster coverage — the
            // enclosing disk was rewriting up to ~50x a thin stroke's area.
            if let Some(coverage) = coverage {
                alpha *= coverage.alpha_at(
                    (tx as f32 + 0.5) / w as f32,
                    (ty as f32 + 0.5) / h as f32,
                );
                if alpha <= 0.0 {
                    continue;
                }
            }
            let (sx, sy) = (tx + ox, ty + oy);
            // An out-of-bounds donor pixel is DROPPED — for target and donor
            // alike, the consistent-rect rule — instead of edge-clamped: the
            // clamp smeared column-0/row-0 stripes across the painted area
            // whenever the picked clone source sat near the frame edge
            // (L09-7). Auto-heal never trips this: find_donor only returns
            // fully in-bounds offsets.
            if sx < 0 || sy < 0 || sx >= w || sy >= h {
                continue;
            }
            let donor = src.get(sx, sy);
            let base = src.get(tx, ty);
            let mut out = [0.0f32; 3];
            for c in 0..3 {
                let healed = donor[c] + corr[c];
                out[c] = base[c] * (1.0 - alpha) + healed * alpha;
            }
            // The buffer's own impl does the conversion, so an 8-bit image
            // still rounds and clamps exactly as it did while the develop
            // plane keeps its value.
            dst.put(tx, ty, out);
        }
    }
}

/// Hard budgets on what ONE painted mask may plan (L02). Three terms, because
/// no single one bounds the others: region COUNT (each spot is a struct + two
/// heap allocations; a 1-px-gap tiling of an 8192-edge canvas yields ~5.6 M
/// regions ≈ 1 GB of spots and an effective hang in the serial heal loop),
/// aggregate BBOX coverage (the retained `SpotCoverage` bitsets are bbox-sized
/// and unrelated to painted-pixel count — thin diagonal staircases retain
/// ~512× their painted bytes), and aggregate HEAL-DISK area (heal work scales
/// with Σ(2r+1)², which thin full-height strokes blow up while staying tiny in
/// bbox terms). The AI path already truncates to 30 spots; 512 is ~17× that
/// and far above any hand-painted retouch.
const MAX_PAINTED_SPOTS: usize = 512;
const MAX_BBOX_COVERAGE: f64 = 4.0; // Σ bbox px ≤ 4 × mask px
const MAX_DISK_COVERAGE: f64 = 16.0; // Σ (2r+1)² px ≤ 16 × mask px

/// Pack "which pixels are being healed" into the one-bit-per-pixel form the
/// planner works in, or `None` for a raster past the `u32` index space.
///
/// u64 bitsets, not `Vec<bool>`: at an 8192-edge canvas the flat maps were
/// ~88 MB of bookkeeping for one bit of information per pixel. The `u32` guard
/// is not decoration either — the per-blob point list stores flat `u32`
/// indices, and a raster past 4.29 G px (far beyond any real canvas) must
/// refuse loudly rather than wrap indices into corrupt radii.
fn painted_bitset(w: u32, h: u32, mut is_painted: impl FnMut(usize) -> bool) -> Option<Vec<u64>> {
    let n_px = w as usize * h as usize;
    if n_px > u32::MAX as usize {
        eprintln!("⚠ mask raster {w}x{h} exceeds the u32 index space — no heal spots planned");
        return None;
    }
    let mut painted = vec![0u64; n_px.div_ceil(64)];
    for i in 0..n_px {
        if is_painted(i) {
            painted[i / 64] |= 1 << (i % 64);
        }
    }
    Some(painted)
}

/// Turn a painted RGBA mask (alpha < 128 = painted = heal here, matching the UI's
/// brush + the generative-mask convention) into heal spots via connected
/// components: each painted blob becomes one circular heal target. Coordinates
/// are normalised, so the mask can be at any resolution.
///
/// The second return is the budget note when regions were SKIPPED (planned
/// ones still heal): raster-order deterministic, and the caller must surface
/// it — the skipped regions are left untouched.
pub fn plan_from_mask(mask: &RgbaImage) -> (Vec<HealSpot>, Option<crate::rationale::Note>) {
    let (w, h) = mask.dimensions();
    let px = mask.as_raw();
    let Some(painted) = painted_bitset(w, h, |i| px[i * 4 + 3] < 128) else {
        return (Vec::new(), None);
    };
    let template = HealSpot { feather: 0.4, label: "painted".into(), ..Default::default() };
    plan_from_painted(&painted, w, h, &template)
}

/// The same planner over an ALPHA raster — what `render` hands it for one of
/// Lightroom's brush-shaped retouch areas (v1.5.0 F9), after
/// `rasterise_brush_group` has stamped that area's dabs.
///
/// Two entry points and ONE implementation, on purpose: a second copy of the
/// connected-component pass would be seventy lines that have to stay in step
/// with the budget guards, and the only thing that actually differs between
/// the two callers is how a pixel says "heal me". `template` carries whatever
/// the caller knows that the raster cannot say — the area's own feather, its
/// [`SpotOrigin`], its label — and the planner fills in the geometry.
pub fn plan_from_alpha(
    alpha: &image::GrayImage,
    threshold: u8,
    template: &HealSpot,
) -> (Vec<HealSpot>, Option<crate::rationale::Note>) {
    let (w, h) = alpha.dimensions();
    let px = alpha.as_raw();
    let Some(painted) = painted_bitset(w, h, |i| px[i] > threshold) else {
        return (Vec::new(), None);
    };
    plan_from_painted(&painted, w, h, template)
}

/// The connected-component planner both entry points share.
fn plan_from_painted(
    painted: &[u64],
    w: u32,
    h: u32,
    template: &HealSpot,
) -> (Vec<HealSpot>, Option<crate::rationale::Note>) {
    let wu = w as usize;
    let n_px = wu * h as usize;
    let mut seen = vec![0u64; painted.len()];
    let get = |bits: &[u64], i: usize| bits[i / 64] >> (i % 64) & 1 == 1;
    let short = w.min(h) as f32;
    let mut spots = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut bbox_px = 0u64;
    let mut disk_px = 0u64;
    let mut skipped = 0usize;
    for start in 0..n_px {
        if !get(painted, start) || get(&seen, start) {
            continue;
        }
        stack.clear();
        stack.push(start);
        seen[start / 64] |= 1 << (start % 64);
        let (mut sx, mut sy, mut cnt) = (0f64, 0f64, 0u32);
        // Flat u32 indices (the exact same pixels the (x,y) pair list held,
        // at half the bytes) — needed for the second, radius pass below.
        let mut pts: Vec<u32> = Vec::new();
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % wu) as i32, (i / wu) as i32);
            sx += x as f64;
            sy += y as f64;
            cnt += 1;
            pts.push(i as u32);
            for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let j = ny as usize * wu + nx as usize;
                if get(painted, j) && !get(&seen, j) {
                    seen[j / 64] |= 1 << (j % 64);
                    stack.push(j);
                }
            }
        }
        if cnt < 6 {
            continue; // ignore stray dots / brush noise
        }
        if spots.len() >= MAX_PAINTED_SPOTS {
            skipped += 1;
            continue; // labeling continues so the note can count every region
        }
        let cxp = (sx / cnt as f64) as f32;
        let cyp = (sy / cnt as f64) as f32;
        let mut rad = 0f32;
        let (mut min_x, mut min_y) = (u32::MAX, u32::MAX);
        let (mut max_x, mut max_y) = (0u32, 0u32);
        for i in &pts {
            let (x, y) = (*i as usize % wu, *i as usize / wu);
            min_x = min_x.min(x as u32);
            min_y = min_y.min(y as u32);
            max_x = max_x.max(x as u32);
            max_y = max_y.max(y as u32);
            let dd = ((x as f32 - cxp).powi(2) + (y as f32 - cyp).powi(2)).sqrt();
            if dd > rad {
                rad = dd;
            }
        }
        // bbox-local bit-packed raster (same u64-bitset rationale as `painted`
        // above) — the circle stays as the search/feather envelope, the raster
        // limits which pixels actually change.
        let coverage_width = max_x - min_x + 1;
        let coverage_height = max_y - min_y + 1;
        let coverage_len = coverage_width as usize * coverage_height as usize;
        // Budget check BEFORE the bbox-sized allocation. The disk term uses
        // the radius heal_image will actually scan: the same
        // `(rad*1.1).max(2.0)` scaling applied below, clamped to half the
        // short side exactly as heal_image clamps it.
        let this_bbox = coverage_len as u64;
        let r_eff = ((rad * 1.1).max(2.0)).min(0.5 * short);
        let d_eff = (2.0 * r_eff + 1.0) as u64;
        let this_disk = d_eff.saturating_mul(d_eff);
        if bbox_px.saturating_add(this_bbox) > (MAX_BBOX_COVERAGE * n_px as f64) as u64
            || disk_px.saturating_add(this_disk) > (MAX_DISK_COVERAGE * n_px as f64) as u64
        {
            skipped += 1;
            continue;
        }
        bbox_px += this_bbox;
        disk_px += this_disk;
        let mut coverage_bits = vec![0u64; coverage_len.div_ceil(64)];
        for i in &pts {
            let x = (*i as usize % wu) as u32;
            let y = (*i as usize / wu) as u32;
            let local =
                (y - min_y) as usize * coverage_width as usize + (x - min_x) as usize;
            coverage_bits[local / 64] |= 1u64 << (local % 64);
        }
        rad = (rad * 1.1).max(2.0);
        // Geometry from the raster, everything else from the caller — the
        // template is what carries the facts a bitset cannot state (v1.5.0 F9).
        spots.push(HealSpot {
            cx: cxp / w as f32,
            cy: cyp / h as f32,
            radius: rad / short,
            coverage: Some(SpotCoverage {
                mask_width: w,
                mask_height: h,
                x: min_x,
                y: min_y,
                width: coverage_width,
                height: coverage_height,
                bits: coverage_bits,
            }),
            ..template.clone()
        });
    }
    let note = (skipped > 0).then(|| {
        crate::rationale::Note::new(
            crate::rationale::keys::HEAL_BUDGET,
            vec![
                ("n", spots.len().to_string()),
                ("total", (spots.len() + skipped).to_string()),
                ("max_spots", MAX_PAINTED_SPOTS.to_string()),
                ("max_bbox", MAX_BBOX_COVERAGE.to_string()),
                ("max_disk", MAX_DISK_COVERAGE.to_string()),
            ],
        )
    });
    if let Some(n) = &note {
        eprintln!("⚠ {}", crate::rationale::render_one(n));
    }
    (spots, note)
}

// --- AI auto-detection (vision) --------------------------------------------

/// Vision model auto-detects small removable defects (dust / blemishes / specks)
/// and returns them as heal spots. Constrained by the prompt + JSON schema to
/// SMALL spot-removals against fairly uniform surroundings — never large-area or
/// content-inventing edits, so the result stays "retouch, not generation".
pub fn detect_spots(cfg: &Config, jpeg: &[u8]) -> Result<RetouchPlan> {
    let key = cfg.openai_api_key.as_ref().ok_or_else(|| {
        anyhow!("OPENAI_API_KEY not set — AI spot-detection needs the image (vision) API; paint a mask instead")
    })?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
    let instruction = "You are a photo RETOUCHER doing blemish / dust removal. Look at the image and \
list SMALL defects that should be REMOVED by healing from surrounding pixels: sensor dust spots, \
skin blemishes / pimples, stray specks, tiny distracting objects against a fairly uniform background \
(sky, skin, wall, water). For EACH, give a circular region: cx, cy (centre, 0..1 of the frame) and \
radius (fraction of the SHORT side, keep SMALL — 0.005..0.06). DO NOT include anything that needs \
inventing new content (no large areas, no removing big / foreground subjects, no adding objects) — \
only small spot fixes a retoucher could heal from neighbours. Return up to 30 spots; fewer is fine. \
If the photo is clean, return an empty list.";

    let body = json!({
        "model": cfg.openai_model,
        // Stored responses land in the KEY OWNER's account — never persist
        // the user's photos there (see the proposer's rule in openai.rs).
        "store": false,
        "input": [{
            "role": "user",
            "content": [
                { "type": "input_text", "text": instruction },
                { "type": "input_image",
                  "image_url": format!("data:image/jpeg;base64,{b64}"),
                  "detail": "high" }
            ]
        }],
        "text": { "format": {
            "type": "json_schema",
            "name": "retouch_plan",
            "strict": true,
            "schema": plan_schema()
        }}
    });

    let url = format!("{}/responses", cfg.openai_base_url.trim_end_matches('/'));
    // Same latency class as the analyze proposer (high-detail image + strict
    // schema on /responses) — and previously a BARE `ureq::post` with no
    // deadline at all: a stalled endpoint parked the retouch worker forever,
    // and `busy` gates every GUI action. Streaming-first via the shared
    // helper: the propose budget bounds SILENCE, not healthy generation time,
    // and AUTOSHADE_HTTP_TIMEOUT_SECS still overrides.
    //
    // This is the IMAGE role (a vision call on the image key and base URL), so
    // it takes the image role's reasoning-effort tier — not the analysis one.
    let value: serde_json::Value = crate::advisor::post_ai_json(
        &url,
        key,
        body,
        crate::advisor::PROPOSE_TIMEOUT_SECS,
        crate::advisor::SseFamily::Responses,
        cfg.image_effort.as_deref(),
    )
    .map_err(|e| anyhow!("vision API: {e}"))?;

    let text = extract_text(&value)
        .ok_or_else(|| anyhow!("no structured output in vision response (shape mismatch)"))?;
    let mut plan: RetouchPlan =
        serde_json::from_str(strip_fence(&text)).context("parse retouch plan JSON")?;
    // Defend the engine + keep it "retouch not generation": clamp to small spots.
    plan.spots.retain(|s| s.radius > 0.0);
    // The prompt asks for AT MOST 30 — enforce it (strict mode cannot cap an
    // array), or an over-eager response triggers unbounded full-res healing.
    plan.spots.truncate(30);
    for s in plan.spots.iter_mut() {
        s.cx = s.cx.clamp(0.0, 1.0);
        s.cy = s.cy.clamp(0.0, 1.0);
        s.radius = s.radius.clamp(0.003, 0.08);
        s.feather = if s.feather <= 0.0 { 0.4 } else { s.feather.clamp(0.0, 1.0) };
    }
    Ok(plan)
}

/// JSON schema (OpenAI strict mode) for the spot list the model returns. Mirrors
/// the small subset of [`HealSpot`] the AI sets; `feather`/`source` default.
fn plan_schema() -> serde_json::Value {
    let num = || json!({"type": "number"});
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["spots", "rationale", "confidence"],
        "properties": {
            "spots": { "type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["cx", "cy", "radius", "label"],
                "properties": { "cx": num(), "cy": num(), "radius": num(), "label": {"type": "string"} }
            }},
            "rationale": {"type": "string"},
            "confidence": num()
        }
    })
}

/// Pull the model's text out of a Responses-API reply (convenience field first,
/// then walk `output[].content[]`). Mirrors `advisor/openai.rs`.
fn extract_text(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.get("output_text").and_then(|x| x.as_str()) {
        return Some(s.to_string());
    }
    for item in v.get("output")?.as_array()? {
        if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
            for c in content {
                if c.get("type").and_then(|t| t.as_str()) == Some("output_text")
                    && let Some(s) = c.get("text").and_then(|t| t.as_str())
                {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

fn strip_fence(s: &str) -> &str {
    let t = s.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

// --- orchestration ---------------------------------------------------------

/// Run the heal mode for one source: gather spots (AI auto-detect and/or a
/// painted mask), heal them on the developed pixels, and save a pixel master to
/// `out`. Non-XMP by nature (pixel edits don't serialise to ACR).
///
/// `full_res` heals the full-sensor develop (e.g. 61 MP) for a RAW; otherwise a
/// ≤2048px thumbnail of the SAME develop. Both are the engine's own neutral
/// develop — never the camera's baked 8-bit JPEG preview, which this used to
/// heal: that swapped the canvas onto camera-curve pixels mid-session (visible
/// brightness jump) and put every later slider/export on a different tone
/// chain than before the heal. Detection runs on a downscaled JPEG —
/// coordinates are normalised, so placement is resolution-independent.
pub fn heal(
    cfg: &Config,
    src_path: &Path,
    manual_mask: Option<&Path>,
    auto_detect: bool,
    full_res: bool,
    out: &Path,
) -> Result<HealReport> {
    // FIRST, before decode and before the BILLED auto-detect call (Codex
    // AL F2: the L09#1 preflight covered analyze/auto/reimagine/retouch,
    // but heal paid for detect_spots and then failed on a directory -o at
    // save time). No-op for GUI/web (unique_out prepared the path).
    crate::pipeline::preflight_out(out, src_path)?;
    // ONE dispatch for both source kinds (`render::source_pixels`): preview
    // mode develops a RAW AT ≤2048 (cap before tone/geometry) instead of
    // developing 61 MP and thumbnailing the result, and bounds a baked source
    // by the same contract — the flag used to be ignored for baked sources, so
    // a "preview mode" heal still processed (and saved) an entire
    // high-resolution TIFF/JPEG.
    let base = crate::render::source_pixels(src_path, (!full_res).then_some(2048))?;
    let (w, h) = (base.width(), base.height());

    let mut spots: Vec<HealSpot> = Vec::new();
    let mut rationale = String::new();
    let mut notes: Vec<crate::rationale::Note> = Vec::new();
    let mut detect_failed = false;
    if auto_detect {
        // ≤1568px detection JPEG. `resize` allocates only the TARGET buffer
        // (the old path cloned the full frame first); a source already ≤1568
        // clones, which is ≤ ~20 MB.
        let small = if w.max(h) > 1568 {
            base.resize(1568, 1568, image::imageops::FilterType::Triangle)
        } else {
            base.clone()
        };
        let mut jpeg = Vec::new();
        DynamicImage::ImageRgb8(small.into_rgb8())
            .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .context("encode jpeg for detection")?;
        match detect_spots(cfg, &jpeg) {
            Ok(p) => {
                rationale = p.rationale.clone();
                spots.extend(p.spots);
            }
            Err(e) => {
                // If the user also painted, heal that and disclose the AI failure;
                // otherwise surface the error (don't silently do nothing).
                if manual_mask.is_none() {
                    return Err(e);
                }
                detect_failed = true;
                eprintln!("⚠ AI spot-detection failed ({e}); healing the painted mask only.");
                // stderr is invisible from the GUI — carry the disclosure in
                // the report too (typed, so the GUI renders it localized).
                crate::rationale::push_note(
                    &mut rationale,
                    &mut notes,
                    crate::rationale::Note::new(
                        crate::rationale::keys::HEAL_DETECT_FAILED,
                        vec![("e", crate::rationale::error_line(&e))],
                    ),
                );
            }
        }
    }
    if let Some(mp) = manual_mask {
        let m = crate::render::open_mask_bounded(mp)
            .with_context(|| format!("open mask {}", mp.display()))?
            .to_rgba8();
        let (planned, note) = plan_from_mask(&m);
        spots.extend(planned);
        if let Some(n) = note {
            if !rationale.is_empty() {
                crate::rationale::push_note(
                    &mut rationale,
                    &mut notes,
                    crate::rationale::Note::plain(crate::rationale::keys::HEAL_NOTE_SEP),
                );
            }
            crate::rationale::push_note(&mut rationale, &mut notes, n);
        }
    }
    if spots.is_empty() {
        // Writing (and linking) a byte-identical zero-spot master helps
        // nobody — and after an AI failure it silently masked the failure as
        // success. Name what ACTUALLY happened per input: the old text
        // claimed "AI detection added nothing" even when detection never ran
        // (--no-auto) or when it FAILED outright.
        let mut parts: Vec<&str> = Vec::new();
        if manual_mask.is_some() {
            parts.push("the painted mask selected no area");
        }
        if auto_detect {
            parts.push(if detect_failed {
                "AI spot-detection failed (see the warning above)"
            } else {
                "AI detection found no spots"
            });
        }
        if parts.is_empty() {
            parts.push("no mask was given and AI detection was off");
        }
        anyhow::bail!("nothing to heal: {}", parts.join("; "));
    }

    let planned = spots.len();
    let healed = heal_and_save(base, &spots, out)?;
    let skipped = planned - healed;
    if skipped > 0 {
        // stderr for the CLI; the GUI and the server cannot see this line and
        // read `skipped` off the report instead.
        eprintln!(
            "⚠ {skipped} of {planned} spot(s) left untouched: no donor area of the spot's own \
             size fits inside the frame around it"
        );
    }
    Ok(HealReport { spots: healed, skipped, rationale, dims: (w, h), notes })
}

/// Heal at the source's OWN depth, carry alpha through, and stage the result
/// as the pixel master (U30).
///
/// A 16-bit source must not quantise to 8 bits just to pass the healer, and
/// transparency is not a defect — the alpha plane rides through untouched.
/// Shared by `heal` and `clone_stamp`, which differ only in how they build
/// the spot list.
fn heal_and_save(base: DynamicImage, spots: &[HealSpot], out: &Path) -> Result<usize> {
    // A17: the LOCAL full-resolution phase, one at a time process-wide
    // (`crate::full_res_slot`). Scoped HERE rather than at `heal`'s entry, and
    // that is the honest boundary: `base` is live from the decode above and
    // across `detect_spots`' network call by construction, so taking the slot
    // at the decode would queue every export behind a model call — exactly the
    // stall this scoping exists to prevent. What IS bounded is the phase that
    // allocates ON TOP of `base`: the depth conversion, the healed frame, the
    // re-attached alpha and the encode. `clone_stamp` reaches the same phase
    // through the same door, so neither caller can forget it.
    let _heavy = crate::full_res_slot();
    let alpha = split_alpha(&base);
    let (healed, n) = if deep_color(&base) {
        let mut rgb = base.into_rgb16();
        let n = heal_image(&mut rgb, spots);
        (DynamicImage::ImageRgb16(rgb), n)
    } else {
        let mut rgb = base.into_rgb8();
        let n = heal_image(&mut rgb, spots);
        (DynamicImage::ImageRgb8(rgb), n)
    };
    crate::pipeline::save_master(out, reattach_alpha(healed, alpha))?;
    Ok(n)
}

/// 8-bit or deeper? Everything that is not 8-bit heals at 16 bits.
fn deep_color(img: &DynamicImage) -> bool {
    !matches!(
        img,
        DynamicImage::ImageRgb8(_)
            | DynamicImage::ImageRgba8(_)
            | DynamicImage::ImageLuma8(_)
            | DynamicImage::ImageLumaA8(_)
    )
}

/// Detach the alpha plane (16-bit precision; 8-bit sources scale up
/// losslessly ×257) so the RGB heal cannot touch it.
fn split_alpha(img: &DynamicImage) -> Option<Vec<u16>> {
    if !img.color().has_alpha() {
        return None;
    }
    Some(match img {
        DynamicImage::ImageRgba8(b) => b.pixels().map(|p| p.0[3] as u16 * 257).collect(),
        DynamicImage::ImageLumaA8(b) => b.pixels().map(|p| p.0[1] as u16 * 257).collect(),
        DynamicImage::ImageRgba16(b) => b.pixels().map(|p| p.0[3]).collect(),
        DynamicImage::ImageLumaA16(b) => b.pixels().map(|p| p.0[1]).collect(),
        other => other.to_rgba16().pixels().map(|p| p.0[3]).collect(),
    })
}

/// Put a detached alpha plane back onto the healed RGB frame, at its depth.
fn reattach_alpha(healed: DynamicImage, alpha: Option<Vec<u16>>) -> DynamicImage {
    let Some(a) = alpha else { return healed };
    match healed {
        DynamicImage::ImageRgb16(rgb) => {
            let (w, h) = rgb.dimensions();
            let mut out: ImageBuffer<image::Rgba<u16>, Vec<u16>> = ImageBuffer::new(w, h);
            for (i, (p, o)) in rgb.pixels().zip(out.pixels_mut()).enumerate() {
                *o = image::Rgba([p.0[0], p.0[1], p.0[2], a[i]]);
            }
            DynamicImage::ImageRgba16(out)
        }
        DynamicImage::ImageRgb8(rgb) => {
            let (w, h) = rgb.dimensions();
            let mut out: RgbaImage = ImageBuffer::new(w, h);
            for (i, (p, o)) in rgb.pixels().zip(out.pixels_mut()).enumerate() {
                *o = image::Rgba([p.0[0], p.0[1], p.0[2], (a[i] / 257) as u8]);
            }
            DynamicImage::ImageRgba8(out)
        }
        other => other,
    }
}


/// Clone-stamp mode: copy pixels from a user-picked SOURCE point over every
/// painted target blob — verbatim texture transplant (feathered edge, no tone
/// matching), Photoshop's clone stamp beside heal's seamless repair. Each blob
/// clones FROM the same picked point (PS "non-aligned" sampling), so the user
/// picks clean texture once and paints any number of targets. Deterministic,
/// no AI. Output is the same ./out pixel master as heal (non-XMP by nature).
pub fn clone_stamp(
    src_path: &Path,
    mask_path: &Path,
    source_norm: (f32, f32),
    full_res: bool,
    out: &Path,
) -> Result<HealReport> {
    // FIRST, before the decode — the exact preflight `heal` runs (L09-9
    // made it symmetric): a bad `out` must refuse before minutes of
    // full-res base work, and an `out` that would overwrite the library —
    // clone_stamp(photo.arw, …, out = photo.arw) replaces a RAW with PNG
    // bytes — is refused by the same guard. No-op for the GUI (unique_out).
    crate::pipeline::preflight_out(out, src_path)?;
    // Same base contract as `heal`, through the same one dispatch: the engine's
    // OWN neutral develop (full or ≤2048px), never the camera's baked preview —
    // see the heal doc comment.
    let base = crate::render::source_pixels(src_path, (!full_res).then_some(2048))?;
    let (w, h) = (base.width(), base.height());

    let m = crate::render::open_mask_bounded(mask_path)
        .with_context(|| format!("open mask {}", mask_path.display()))?
        .to_rgba8();
    let (mut spots, budget_note) = plan_from_mask(&m);
    if spots.is_empty() {
        return Err(anyhow!("nothing painted — brush over the target area first"));
    }
    for s in spots.iter_mut() {
        s.source = Some([source_norm.0 - s.cx, source_norm.1 - s.cy]);
        s.clone_raw = true;
        s.feather = 0.3;
        s.label = "clone".into();
    }
    let planned = spots.len();
    let healed = heal_and_save(base, &spots, out)?;
    let (rationale, notes) = match budget_note {
        Some(note) => (crate::rationale::render_one(&note), vec![note]),
        None => (String::new(), Vec::new()),
    };
    Ok(HealReport { spots: healed, skipped: planned - healed, rationale, dims: (w, h), notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba};

    /// L09-7: an explicit clone source whose donor rect leaves the frame
    /// drops those pixels instead of edge-replicating them across the target.
    #[test]
    fn an_out_of_bounds_clone_donor_is_dropped_not_edge_replicated() {
        let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(32, 32, Rgb([200u8, 200, 200]));
        for y in 0..32 {
            img.put_pixel(0, y, Rgb([255, 0, 0])); // a distinctive edge column
        }
        let spots = vec![HealSpot {
            cx: 0.5,
            cy: 0.5,
            radius: 0.1,
            feather: 0.0,
            source: Some([-2.0, 0.0]), // donor fully out of frame
            coverage: None,
            clone_raw: true,
            label: "clone".into(),
            origin: SpotOrigin::Painted,
        }];
        heal_image(&mut img, &spots);
        assert_eq!(
            *img.get_pixel(16, 16),
            Rgb([200, 200, 200]),
            "no donor pixel exists — the target stays untouched instead of smearing the red edge"
        );
    }

    /// A spot so large that no donor disk of its own size fits anywhere inside
    /// the frame is left untouched — and, since this count is what the CLI and
    /// the GUI report as "spots healed", it is not counted as a repair.
    /// `find_donor` used to answer the sentinel `(0, 0)` here, which `heal_one`
    /// skipped while the report still said "1 spot healed".
    ///
    /// MUTATION: have `find_donor` answer `Some((0, 0))` instead of `None`, or
    /// count the spot before its donor is settled.
    #[test]
    fn a_spot_with_no_room_for_a_donor_is_left_untouched_and_not_counted() {
        let spot = |radius: f32| HealSpot {
            cx: 0.5,
            cy: 0.5,
            radius,
            feather: 0.0,
            source: None,
            coverage: None,
            clone_raw: false,
            label: "auto".into(),
            origin: SpotOrigin::Painted,
        };
        let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(64, 64, Rgb([200u8, 200, 200]));
        for y in 28..36 {
            for x in 28..36 {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
        let before = img.clone();
        // Half the short edge: r = 32 px on a 64 px frame, so the nearest donor
        // ring (2.4 r) sits 77 px out and no donor disk fits.
        assert_eq!(heal_image(&mut img, &[spot(0.5)]), 0, "no donor — nothing to count");
        assert_eq!(img, before, "an unhealed spot leaves every pixel as it was");
        // Beside it, a spot that fits heals and is the only one counted.
        assert_eq!(
            heal_image(&mut img, &[spot(0.5), spot(0.06)]),
            1,
            "only the spot that found a donor counts"
        );
        assert_ne!(*img.get_pixel(32, 32), Rgb([0, 0, 0]), "the small spot really was healed");
    }

    /// L09-9: clone_stamp carries heal's pre-decode preflight — an output
    /// that lands in the photo's own library folder refuses before any base
    /// work (the fixture photo does not even exist, so reaching the decode
    /// would error differently).
    #[test]
    fn clone_stamp_refuses_a_library_output_before_any_work() {
        let dir = std::env::temp_dir()
            .join(format!("autoshade-clonestamp-preflight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lib = dir.join("library");
        std::fs::create_dir_all(&lib).unwrap();
        let src = lib.join("photo.arw");
        let e = match clone_stamp(&src, &lib.join("mask.png"), (0.5, 0.5), false, &lib.join("photo.arw")) {
            Ok(_) => panic!("a library output must refuse"),
            Err(e) => e.to_string(),
        };
        assert!(e.contains("read-only"), "the library guard fires first: {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }


    #[test]
    fn heal_engine_runs_at_sixteen_bits() {
        // The generic engine must heal in the u16 domain — a healed value
        // ABOVE 255 proves no 8-bit quantisation happened anywhere.
        let mut img: ImageBuffer<Rgb<u16>, Vec<u16>> =
            ImageBuffer::from_pixel(64, 64, Rgb([30000u16, 30000, 30000]));
        for y in 28..36 {
            for x in 28..36 {
                img.put_pixel(x, y, Rgb([500u16, 500, 500]));
            }
        }
        let spots = vec![HealSpot { cx: 0.5, cy: 0.5, radius: 7.0 / 64.0, ..Default::default() }];
        heal_image(&mut img, &spots);
        let c = img.get_pixel(32, 32).0;
        assert!(c[0] > 20000, "healed centre must approach the 16-bit grey, got {c:?}");
    }

    #[test]
    fn heal_keeps_bit_depth_and_alpha_and_stages_the_master() {
        // src lives in its own subdir: heal's pre-pay preflight (Codex AL
        // F2) runs guard_readonly, which refuses an out beside the source.
        let root = std::env::temp_dir().join(format!("autoshade-retouch-depth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("library");
        let out_dir = root.join("exports");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&out_dir).unwrap();
        // 16-bit RGBA source: grey field, dark defect at centre, alpha ramp.
        let (w, h) = (64u32, 64u32);
        let mut img: ImageBuffer<Rgba<u16>, Vec<u16>> = ImageBuffer::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v: u16 = if (28..36).contains(&x) && (28..36).contains(&y) { 500 } else { 30000 };
            *p = Rgba([v, v, v, (x * 1000).min(65535) as u16]);
        }
        let srcp = dir.join("deep_src.png");
        DynamicImage::ImageRgba16(img).save(&srcp).unwrap();
        // Painted mask, client contract: TRANSPARENT = heal here.
        let mut mask: RgbaImage = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
        for y in 26..38 {
            for x in 26..38 {
                mask.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            }
        }
        let maskp = dir.join("mask.png");
        mask.save(&maskp).unwrap();
        let outp = out_dir.join("healed.png");
        let rep =
            heal(&Config::load(), &srcp, Some(&maskp), false, true, &outp).unwrap();
        assert!(rep.spots >= 1);
        let healed = image::open(&outp).unwrap();
        assert!(
            matches!(healed, DynamicImage::ImageRgba16(_)),
            "16-bit + alpha must SURVIVE the heal, got {:?}",
            healed.color()
        );
        let hb = healed.to_rgba16();
        let c = hb.get_pixel(32, 32).0;
        assert!(c[0] > 20000, "defect healed in the 16-bit domain, got {c:?}");
        assert_eq!(c[3], 32 * 1000, "alpha rides through untouched");
        // The staged write leaves no residue beside the master — scanned in
        // the EXPORT dir, where the staged temp name would linger now that
        // the F2 preflight split src and out into separate folders.
        let residue: Vec<String> = std::fs::read_dir(&out_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(residue.is_empty(), "no staging residue: {residue:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn heal_replaces_a_spot_with_surroundings() {
        // 64×64 mid-gray with a black blotch at the centre; healing from the
        // uniform gray surroundings should pull the centre back toward gray.
        let mut img = RgbImage::from_pixel(64, 64, Rgb([128, 128, 128]));
        for y in 28..36 {
            for x in 28..36 {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
        let spots = vec![HealSpot {
            cx: 0.5, cy: 0.5, radius: 7.0 / 64.0, feather: 0.4, source: None,
            coverage: None, clone_raw: false, label: "x".into(),
            origin: SpotOrigin::Painted,
        }];
        heal_image(&mut img, &spots);
        let c = img.get_pixel(32, 32).0;
        assert!(c[0] > 100, "healed centre should approach gray, got {c:?}");
    }

    #[test]
    fn heal_with_explicit_source_removes_a_defect() {
        // White field with a small black defect; heal it using an EXPLICIT clean
        // donor offset → the defect is replaced and tone-matched to the white
        // surroundings (heal semantics: match surroundings, not transplant tone).
        let mut img = RgbImage::from_pixel(40, 20, Rgb([255, 255, 255]));
        for y in 8..12 {
            for x in 28..32 {
                img.put_pixel(x, y, Rgb([0, 0, 0])); // defect
            }
        }
        let spots = vec![HealSpot {
            cx: 30.0 / 40.0, cy: 0.5, radius: 4.0 / 20.0, feather: 0.2,
            source: Some([-0.3, 0.0]), coverage: None,
            clone_raw: false, label: "spot".into(),
            origin: SpotOrigin::Painted,
        }];
        heal_image(&mut img, &spots);
        let c = img.get_pixel(30, 10).0;
        assert!(c[0] > 200, "defect should heal to the white surroundings, got {c:?}");
    }

    #[test]
    fn clone_raw_transplants_verbatim_where_heal_tone_matches() {
        // A white field with a gray donor patch on the left. Cloning the gray
        // ONTO the white must land the donor's tone verbatim (clone_raw), while
        // the same spot healed (clone_raw=false) tone-matches to the white
        // surroundings — the defining difference between the two tools.
        let mut base = RgbImage::from_pixel(60, 20, Rgb([255, 255, 255]));
        for y in 6..14 {
            for x in 6..14 {
                base.put_pixel(x, y, Rgb([100, 100, 100])); // donor texture
            }
        }
        let spot = |clone_raw: bool| HealSpot {
            cx: 45.0 / 60.0,
            cy: 0.5,
            radius: 3.0 / 20.0,
            feather: 0.2,
            source: Some([(10.0 - 45.0) / 60.0, 0.0]), // donor centre at (10, 10)
            coverage: None,
            clone_raw,
            label: "clone".into(),
            origin: SpotOrigin::Painted,
        };
        let mut cloned = base.clone();
        heal_image(&mut cloned, &[spot(true)]);
        let c = cloned.get_pixel(45, 10).0;
        assert!(c[0] < 130, "clone must transplant the gray verbatim, got {c:?}");

        let mut healed = base.clone();
        heal_image(&mut healed, &[spot(false)]);
        let hh = healed.get_pixel(45, 10).0;
        assert!(hh[0] > 200, "heal must tone-match toward the white border, got {hh:?}");
    }

    #[test]
    fn mask_blob_becomes_one_spot() {
        // One painted 20×20 blob (alpha=0) centred → exactly one heal spot there.
        let mut m = RgbaImage::from_pixel(100, 100, Rgba([0, 0, 0, 255])); // opaque = keep
        for y in 40..60 {
            for x in 40..60 {
                m.put_pixel(x, y, Rgba([255, 0, 0, 0])); // painted
            }
        }
        let (spots, _) = plan_from_mask(&m);
        assert_eq!(spots.len(), 1, "one blob → one spot");
        assert!((spots[0].cx - 0.495).abs() < 0.05 && (spots[0].cy - 0.495).abs() < 0.05);
    }

    #[test]
    fn empty_inputs_are_noops() {
        let mut img = RgbImage::from_pixel(8, 8, Rgb([10, 20, 30]));
        let before = img.clone();
        heal_image(&mut img, &[]);
        assert_eq!(img, before, "no spots → image unchanged");
        let clean = RgbaImage::from_pixel(16, 16, Rgba([0, 0, 0, 255])); // nothing painted
        assert!(plan_from_mask(&clean).0.is_empty());
    }

    /// L02: the painted-mask budget — a mask tiled with more regions than
    /// the cap plans exactly the cap, and the note names the split.
    #[test]
    fn plan_from_mask_budget_caps_regions_and_discloses() {
        // 24×24 painted blocks of 3×2 px on a 4-px grid (each ≥ the cnt<6
        // floor, 1-px gaps keep them 4-disconnected): 576 regions > 512.
        let mut mask = RgbaImage::from_pixel(96, 96, Rgba([0, 0, 0, 255]));
        for by in 0..24u32 {
            for bx in 0..24u32 {
                for dy in 0..2 {
                    for dx in 0..3 {
                        mask.put_pixel(bx * 4 + dx, by * 4 + dy, Rgba([255, 0, 0, 0]));
                    }
                }
            }
        }
        let (spots, note) = plan_from_mask(&mask);
        assert_eq!(spots.len(), 512, "the cap plans exactly the budget");
        let note = crate::rationale::render_one(&note.expect("over-budget must disclose"));
        assert!(note.contains("512 of 576"), "{note}");
    }

    #[test]
    fn a_thin_painted_stroke_does_not_heal_its_bounding_disk() {
        let mut mask = RgbaImage::from_pixel(100, 100, Rgba([0, 0, 0, 255]));
        for y in 49..=50 {
            for x in 20..80 {
                mask.put_pixel(x, y, Rgba([255, 0, 0, 0]));
            }
        }
        let (mut spots, _) = plan_from_mask(&mask);
        assert_eq!(spots.len(), 1);
        spots[0].source = Some([0.0, 0.3]);
        spots[0].clone_raw = true;
        spots[0].feather = 0.0;

        let encoded = serde_json::to_value(&spots[0]).unwrap();
        assert!(
            encoded.get("coverage").is_none(),
            "runtime raster coverage must not change the saved HealSpot shape"
        );

        let mut img = RgbImage::from_fn(100, 100, |_x, y| {
            let v = (y * 2) as u8;
            Rgb([v, v, v])
        });
        let unpainted_before = *img.get_pixel(50, 25);
        let painted_before = *img.get_pixel(50, 50);

        heal_image(&mut img, &spots);

        assert_eq!(
            *img.get_pixel(50, 25),
            unpainted_before,
            "an unpainted pixel inside the stroke's bounding disk must remain untouched"
        );
        assert_ne!(
            *img.get_pixel(50, 50),
            painted_before,
            "the painted stroke itself must still receive donor pixels"
        );
    }
    /// L09#4 behaviour half: heal honours --full-res on a BAKED source —
    /// without it the image is thumbnailed to 2048px and THAT is the saved
    /// master; with it the source resolution survives. No API is reached
    /// (auto_detect off, painted mask only).
    #[test]
    fn heal_downsamples_a_baked_source_without_full_res() {
        // src in its own subdir — heal's pre-pay preflight (Codex AL F2)
        // refuses an out beside the source.
        let root = std::env::temp_dir()
            .join(format!("autoshade-heal-fullres-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("library");
        let out_dir = root.join("exports");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&out_dir).unwrap();
        let src = dir.join("big.png");
        image::RgbImage::from_fn(3000, 2000, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 128])
        })
        .save(&src)
        .unwrap();
        // Painted mask: alpha-0 blob on an opaque field (plan_from_mask's
        // convention), small raster — spot coords are normalised.
        let mut mask = RgbaImage::from_pixel(100, 66, Rgba([0, 0, 0, 255]));
        for y in 20..30 {
            for x in 40..55 {
                mask.put_pixel(x, y, Rgba([255, 0, 0, 0]));
            }
        }
        let maskp = dir.join("mask.png");
        mask.save(&maskp).unwrap();
        let cfg = crate::config::Config::load();
        let out_small = out_dir.join("small-out.png");
        heal(&cfg, &src, Some(&maskp), false, false, &out_small).unwrap();
        let d = image::image_dimensions(&out_small).unwrap();
        assert_eq!(d.0.max(d.1), 2048, "without --full-res the saved master IS the 2048px thumbnail");
        let out_full = out_dir.join("full-out.png");
        heal(&cfg, &src, Some(&maskp), false, true, &out_full).unwrap();
        let d = image::image_dimensions(&out_full).unwrap();
        assert_eq!((d.0, d.1), (3000, 2000), "with --full-res the source resolution survives");
        let _ = std::fs::remove_dir_all(&root);
    }
}
