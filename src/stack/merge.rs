//! The four merges, over the one alignment in [`super::align`].
//!
//! Everything here takes N sRGB-encoded frames of one scene and returns one,
//! and everything here is a WEIGHTED sum — what separates the four is what a
//! weight is allowed to mean:
//!
//! * [`StackKind::Hdr`] weighs a sample by how trustworthy it is as a
//!   measurement of light, and produces a measurement of light;
//! * [`StackKind::Fuse`] weighs a pixel by how good it LOOKS, and never forms
//!   a radiance at all — the bracket's answer for someone who wants the
//!   picture rather than the data;
//! * [`StackKind::Focus`] weighs a neighbourhood by how sharp it is;
//! * [`StackKind::Noise`] weighs every frame alike, and only withdraws the
//!   ones that disagree with the rest.
//!
//! The first two are different answers to the same question and both are
//! offered deliberately: an HDR merge hands the develop pipeline a frame with
//! recovered highlights and a recorded headroom, which the SDR rendition panel
//! then shapes; a fusion hands it a finished-looking frame that no amount of
//! headroom can be read back out of. Neither is a worse version of the other.

mod composite;
mod radiance;

use anyhow::{bail, Result};

use crate::render::neighbours4;
use crate::stack::align::{self, AlignParams};

/// Which merge a stack is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackKind {
    /// An exposure bracket into one frame that holds the whole range, with the
    /// recovered stops recorded rather than thrown away.
    Hdr,
    /// The same bracket blended where each frame is best exposed, with no
    /// radiance in between.
    Fuse,
    /// A focus sweep into one frame sharp throughout.
    Focus,
    /// Repeated frames of a still scene averaged, so the signal adds and the
    /// noise does not.
    Noise,
}

impl StackKind {
    /// Every kind beside its PERSISTED spelling, which is a format and never
    /// display text: these strings are in saved variants and in sidecars, so
    /// they are never localised and never renamed.
    ///
    /// One table, read by every direction, so they cannot drift apart — which
    /// is the failure mode a set of hand-written `match` arms has, and it is a
    /// silent one, because the half that is still right keeps working.
    ///
    /// The third column is DISPLAY text, in English, and is localised at the
    /// render site the way [`crate::recipe`]'s labels are: the second column is
    /// a format and must never change, the third is a name for a person and may.
    const SPELLINGS: [(StackKind, &'static str, &'static str); 4] = [
        (StackKind::Hdr, "hdr", "HDR merge"),
        (StackKind::Fuse, "fuse", "Exposure fusion"),
        (StackKind::Focus, "focus", "Focus stack"),
        (StackKind::Noise, "noise", "Noise stack"),
    ];

    /// Every kind, for a CLI's value list and for the GUI's picker.
    pub const ALL: [StackKind; 4] =
        [StackKind::Hdr, StackKind::Fuse, StackKind::Focus, StackKind::Noise];

    fn row(self) -> &'static (StackKind, &'static str, &'static str) {
        Self::SPELLINGS
            .iter()
            .find(|(k, ..)| *k == self)
            .expect("SPELLINGS lists every kind, and a test says so")
    }

    pub fn store_str(self) -> &'static str {
        self.row().1
    }

    /// The name a person reads — English here, localised where it is drawn.
    pub fn label(self) -> &'static str {
        self.row().2
    }

    pub fn from_store_str(s: &str) -> Option<Self> {
        Self::SPELLINGS.iter().find(|(_, t, _)| *t == s).map(|(k, ..)| *k)
    }
}

/// What a merge produced, with the numbers its disclosure line needs.
#[derive(Debug, Clone)]
pub struct Merged {
    /// sRGB-encoded, the size the frames were.
    pub pixels: Vec<[f32; 3]>,
    /// Stops of highlight this merge recovered above the reference frame's own
    /// white — `0.0` for every kind but [`StackKind::Hdr`]. It is the number
    /// `EditRecipe::hdr_max_ev` is set from, which is what makes the SDR
    /// rendition panel mean something on the card a stack produces.
    pub headroom_ev: f32,
    /// Each frame's exposure relative to the FIRST, in stops, as measured on
    /// the pixels rather than read off metadata. Frame 0 is 0.0 by
    /// construction.
    pub exposures: Vec<f32>,
    /// How far the alignment had to move each frame's worst corner, in pixels.
    /// A stack whose numbers here are large is one to look at before trusting.
    pub travel: Vec<f32>,
    /// The fraction of the frame at least one source could not cover after
    /// warping — the border wedge, which every merge below weights to zero
    /// rather than averaging in.
    pub uncovered: f32,
}

/// What a merge is allowed to do.
#[derive(Debug, Clone, Copy)]
pub struct MergeOptions {
    pub kind: StackKind,
    /// `None` when the frames are known to be registered already — a tripod,
    /// or a bracket the camera itself stacked. Alignment is the expensive half
    /// of a stack and is not always wanted: on frames that really are
    /// registered there is nothing for it to find, and looking costs a little.
    /// Measured on the five-frame noise fixture (`stack::align`'s own table),
    /// the grain fell ÷2.03 with the alignment skipped and ÷1.97 with it
    /// running.
    pub align: Option<AlignParams>,
}

impl MergeOptions {
    /// Aligned, because the case this ships for is handheld.
    pub fn new(kind: StackKind) -> MergeOptions {
        MergeOptions { kind, align: Some(AlignParams::DEFAULT) }
    }
}

/// Merge `frames` into one, the FIRST frame being the reference for both
/// geometry and exposure.
///
/// The first frame is the reference rather than, say, the middle one of a
/// bracket, because the reference decides which frame's framing survives —
/// and that is a decision worth leaving to the caller's ordering rather than
/// making silently here.
pub fn merge(frames: &[Vec<[f32; 3]>], w: usize, h: usize, opts: &MergeOptions) -> Result<Merged> {
    if frames.len() < 2 {
        bail!("a stack needs at least two frames, got {}", frames.len());
    }
    if w < 2 || h < 2 {
        bail!("a stack needs a frame bigger than 2×2, got {w}×{h}");
    }
    for (k, f) in frames.iter().enumerate() {
        if f.len() != w * h {
            bail!("frame {k} holds {} pixels, not the {w}×{h} it was given as", f.len());
        }
    }
    let reg = register(frames, w, h, opts.align);
    let exposures: Vec<f32> =
        reg.frames.iter().map(|f| exposure_ratio(&reg.frames[0], f, &reg.cover)).collect();
    let bad = (0..w * h).filter(|k| reg.cover.iter().any(|c| !c[*k])).count();
    let (pixels, headroom_ev) = match opts.kind {
        StackKind::Hdr => radiance::merge(&reg.frames, &reg.cover, &exposures, w, h),
        StackKind::Fuse => (composite::fuse(&reg.frames, &reg.cover, w, h), 0.0),
        StackKind::Focus => (composite::focus(&reg.frames, &reg.cover, w, h), 0.0),
        StackKind::Noise => (composite::average(&reg.frames, &reg.cover, w, h), 0.0),
    };
    Ok(Merged {
        pixels,
        headroom_ev,
        exposures,
        travel: reg.travel,
        uncovered: bad as f32 / (w * h) as f32,
    })
}

/// The frames warped onto the first, with what could not be covered marked.
struct Registered {
    frames: Vec<Vec<[f32; 3]>>,
    cover: Vec<Vec<bool>>,
    travel: Vec<f32>,
}

/// Warp every frame onto the first.
///
/// The coverage masks come back with the frames and are not optional. A warped
/// frame KEEPS its original pixels where the warp read off the edge — see
/// [`align::warp_rgb`], which explains why black would be worse — and those
/// pixels are a lie about that place in the scene. Every merge below
/// multiplies its weights by this mask, so the border wedge is dropped rather
/// than averaged in.
fn register(
    frames: &[Vec<[f32; 3]>],
    w: usize,
    h: usize,
    params: Option<AlignParams>,
) -> Registered {
    let mut out = Registered {
        frames: vec![frames[0].clone()],
        cover: vec![vec![true; w * h]],
        travel: vec![0.0],
    };
    for f in frames.iter().skip(1) {
        match params {
            Some(p) => {
                let warp = align::solve(&frames[0], f, w, h, &p);
                out.travel.push(warp.global.corner_travel(w, h));
                out.cover.push(align::coverage(w, h, &warp));
                out.frames.push(align::warp_rgb(f, w, h, &warp));
            }
            None => {
                out.travel.push(0.0);
                out.cover.push(vec![true; w * h]);
                out.frames.push(f.clone());
            }
        }
    }
    out
}

/// The exposure of `b` relative to `a`, in stops, measured on the pixels
/// rather than read off metadata.
///
/// Metadata would be the obvious source and is the wrong one twice over: a
/// bracket shot in aperture priority records shutter speeds that do not
/// describe the light which reached the sensor once the lens's own T-stop and
/// any auto-ISO are in it, and a stack assembled from already-developed files
/// has no shutter speed left to read at all. The pixels are what the merge is
/// going to combine, so the pixels are what it should measure.
///
/// The MEDIAN, not the mean: a subject that moved, or a sky that clipped in
/// one frame, is a large difference over a small area, and a mean would let it
/// set the whole frame's exposure. Samples are taken only where both frames
/// are inside their own range and every frame covered — a clipped highlight
/// has stopped recording the ratio, and so has a crushed shadow.
pub fn exposure_ratio(a: &[[f32; 3]], b: &[[f32; 3]], cover: &[Vec<bool>]) -> f32 {
    // The bounds are LINEAR, because that is what [`super::luma`] returns:
    // 0.0015 and 0.95 are sRGB 0.02 and 0.98, the range within which a value
    // is still recording the light rather than the sensor's two ends.
    let usable = |v: f32| v > 0.0015 && v < 0.95;
    let mut d: Vec<f32> = (0..a.len())
        .filter(|k| cover.iter().all(|c| c[*k]))
        .filter(|k| usable(super::luma(&a[*k])) && usable(super::luma(&b[*k])))
        .map(|k| (super::luma(&b[k]) / super::luma(&a[k])).log2())
        .filter(|v| v.is_finite())
        .collect();
    if d.is_empty() {
        return 0.0;
    }
    d.sort_by(f32::total_cmp);
    d[d.len() / 2]
}

/// How well exposed one channel value is: 1 in the middle, falling to nothing
/// at both ends.
///
/// A Gaussian on the ENCODED value with σ = 0.2, which is Mertens' own choice
/// and is about perception rather than about physics — it says a mid-grey
/// pixel is worth more to look at than a nearly-black or a nearly-white one,
/// which is exactly the judgement a fusion is there to make.
pub(crate) fn well_exposed(v: f32) -> f32 {
    const S: f32 = 0.2;
    (-(v - 0.5) * (v - 0.5) / (2.0 * S * S)).exp()
}

/// How trustworthy one channel value is as a MEASUREMENT: flat across the
/// middle, and quickly nothing at both ends.
///
/// Not [`well_exposed`], and that difference is the whole difference between
/// the two bracket merges. A radiance estimate has no reason to prefer a
/// mid-grey sample to a three-quarter-tone one — both are equally valid
/// readings of the light. It only has reason to distrust a sample near the
/// sensor's floor or near its clip. So this is flat wherever the twelfth power
/// is still small and collapses where it is not, which is Debevec's hat in the
/// form that does not taper the whole middle away with it.
pub(crate) fn trustworthy(v: f32) -> f32 {
    (1.0 - (2.0 * v - 1.0).powi(12)).max(0.0)
}

/// How much colour a pixel has, as the spread of its three channels.
///
/// A fusion wants it because a washed-out frame of a scene is usually the one
/// that was mis-exposed there: the channels collapsing toward each other is
/// what mis-exposure does on its way to clipping.
pub(crate) fn saturation(p: &[f32; 3]) -> f32 {
    let m = (p[0] + p[1] + p[2]) / 3.0;
    (((p[0] - m).powi(2) + (p[1] - m).powi(2) + (p[2] - m).powi(2)) / 3.0).sqrt()
}

/// The magnitude of the four-neighbour Laplacian, per pixel.
///
/// The one local measure both a fusion and a focus stack are built on: large
/// where a neighbourhood has detail, near zero where it is smooth, and silent
/// about brightness, so a dark sharp region scores like a bright one.
pub(crate) fn detail(luma: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    for (k, o) in out.iter_mut().enumerate() {
        let [l, r, u, d] = neighbours4(luma, w, h, k % w, k / w);
        *o = (l + r + u + d - 4.0 * luma[k]).abs();
    }
    out
}

