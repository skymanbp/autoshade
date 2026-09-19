//! Stacking and merging: several frames of one scene into one (v1.5.0 Track S).
//!
//! Four merges that photographers actually shoot for, over one shared
//! alignment:
//!
//! * **HDR merge** — an exposure bracket into one frame that holds the whole
//!   range, published through F8's SDR rendition (`render::hdr`), which is
//!   exactly the problem that panel exists to solve;
//! * **Exposure fusion** — the same bracket without an HDR intermediate,
//!   blending each frame where it is best exposed;
//! * **Focus stack** — a focus sweep into one frame sharp throughout;
//! * **Noise stack** — repeated frames of a still scene averaged, so the
//!   signal adds and the noise does not.
//!
//! The alignment is [`align`], and it is deliberately ONE implementation for
//! all four: the exposure bracket is the hard case (its frames differ by
//! stops), and an aligner that survives it is more than good enough for the
//! three same-exposure stacks. See that module for why it works in log luma.

pub mod align;
pub mod merge;
pub(crate) mod pyramid;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// One finished stack: the merge's own report beside the size it produced.
pub struct Stacked {
    pub merged: merge::Merged,
    pub width: usize,
    pub height: usize,
}

/// Read `inputs`, merge them, and write the 16-bit master at `out`.
///
/// ONE path for both surfaces. The CLI and the GUI's stack worker do exactly
/// this and differ only in how they REPORT it — the CLI prints the numbers,
/// the GUI lands them on a card — so the loading rules that a merge depends
/// on (sixteen bits through [`pixels16`], one framing for every frame, the
/// first frame as the reference) live here rather than twice.
///
/// `cap` is the long-edge limit `render::source_pixels` takes; `None` is the
/// full frame, which is what both surfaces ask for — a stack baked from a
/// working copy would cap every later export at that size, the same mistake
/// the denoise landing made before 2026-09-15.
pub fn stack_files(
    inputs: &[PathBuf],
    opts: &merge::MergeOptions,
    cap: Option<u32>,
    out: &Path,
) -> Result<Stacked> {
    let (mut w, mut h) = (0usize, 0usize);
    let mut frames: Vec<Vec<[f32; 3]>> = Vec::with_capacity(inputs.len());
    for (k, f) in inputs.iter().enumerate() {
        let img = crate::render::source_pixels(f, cap)
            .with_context(|| format!("read frame {} ({})", k + 1, f.display()))?;
        let (fw, fh) = (img.width() as usize, img.height() as usize);
        if k == 0 {
            (w, h) = (fw, fh);
        } else if (fw, fh) != (w, h) {
            // Named rather than resized: two framings mean two different
            // pictures of the scene, and silently scaling one onto the other
            // would hand the aligner a job it cannot do and call the result a
            // stack.
            bail!(
                "{} is {fw} x {fh}, but the first frame is {w} x {h} — a stack needs one framing",
                f.display()
            );
        }
        frames.push(pixels16(&img));
    }
    let merged = merge::merge(&frames, w, h, opts)?;
    let img =
        image16(&merged.pixels, w, h).context("the merged frame is too large to pack into an image")?;
    crate::pipeline::save_master(out, img)?;
    Ok(Stacked { merged, width: w, height: h })
}

/// Rec. 709 luminance of an sRGB-ENCODED pixel, in LINEAR light.
///
/// The transfer is undone first and that is the whole point: luminance is a
/// statement about how much light there is, and an encoded value is not
/// proportional to that. Everything in this module that compares two frames'
/// brightness goes through here — the aligner's log-luma domain, the merge's
/// exposure measurement, the fallback that picks one frame for a pixel no
/// frame could measure — so the stack has ONE answer to "how bright is this
/// pixel" rather than one per caller.
pub(crate) fn luma(p: &[f32; 3]) -> f32 {
    use crate::render::srgb_to_linear;
    0.2126 * srgb_to_linear(p[0]) + 0.7152 * srgb_to_linear(p[1]) + 0.0722 * srgb_to_linear(p[2])
}

/// An image's pixels as sRGB-encoded floats, read through SIXTEEN bits.
///
/// NOT `fit::pixels_of`, which goes through `to_rgb8`. Eight bits is plenty for
/// measuring a look and nowhere near enough for a merge: a bracket's darkest
/// frame is carrying the highlights every other frame lost, and quantising it
/// to 256 steps before its exposure is divided back out puts those highlights
/// back as 32 steps. The rounding would land exactly on the part of the
/// picture the stack was shot to recover.
pub fn pixels16(img: &image::DynamicImage) -> Vec<[f32; 3]> {
    const FULL: f32 = 65535.0;
    img.to_rgb16()
        .pixels()
        .map(|p| [p[0] as f32 / FULL, p[1] as f32 / FULL, p[2] as f32 / FULL])
        .collect()
}

/// The inverse: sRGB-encoded floats back to a 16-bit image.
pub fn image16(px: &[[f32; 3]], w: usize, h: usize) -> Option<image::DynamicImage> {
    const FULL: f32 = 65535.0;
    let mut buf: Vec<u16> = Vec::with_capacity(w * h * 3);
    for p in px {
        for v in p {
            buf.push((v.clamp(0.0, 1.0) * FULL).round() as u16);
        }
    }
    let w32 = u32::try_from(w).ok()?;
    let h32 = u32::try_from(h).ok()?;
    image::ImageBuffer::from_raw(w32, h32, buf).map(image::DynamicImage::ImageRgb16)
}

/// The picture every part of this module measures itself against.
///
/// One definition, because an aligner and a merge tested on different
/// pictures would be answering different questions about the same code. The
/// structure is deliberately at several scales: plain noise gives a pyramid's
/// coarse levels nothing (it averages away) and a single sine gives every
/// level the same ambiguous ridge, so neither would exercise a pyramid at all.
#[cfg(test)]
pub(crate) mod fixture {
    /// A deterministic plane, kept inside [0.04, 0.96] so an exposure change
    /// has somewhere to move it before anything clips.
    pub(crate) fn plane(w: usize, h: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(w * h);
        for row in 0..h {
            for col in 0..w {
                let (x, y) = (col as f32, row as f32);
                let v = 0.5
                    + 0.18 * (x * 0.21).sin() * (y * 0.13).cos()
                    + 0.12 * (x * 0.07 + y * 0.05).sin()
                    + 0.08 * ((x * 0.9).sin() * (y * 0.7).sin());
                out.push(v.clamp(0.04, 0.96));
            }
        }
        out
    }

    /// The same plane as a colour frame, faintly off-neutral so that a merge
    /// which mixed the channels up has somewhere to show it.
    pub(crate) fn scene(w: usize, h: usize) -> Vec<[f32; 3]> {
        plane(w, h).into_iter().map(|v| [v, v * 0.97, v * 1.02]).collect()
    }

    /// Deterministic per-pixel grain in [−0.5, 0.5], different for each seed.
    ///
    /// A hash rather than an RNG so a failure is reproducible from the seed
    /// alone, which is what makes a noise test debuggable at all. One copy for
    /// the whole module: the noise stack and the radiance merge both need
    /// repeated readings of one scene, and two hashes would eventually differ
    /// in a way that made their two tests incomparable.
    pub(crate) fn grain(seed: u32, k: usize) -> f32 {
        let mut x = (k as u32).wrapping_mul(2_654_435_761).wrapping_add(seed.wrapping_mul(40_503));
        x ^= x >> 13;
        x = x.wrapping_mul(1_274_126_177);
        x ^= x >> 16;
        x as f32 / u32::MAX as f32 - 0.5
    }
}
