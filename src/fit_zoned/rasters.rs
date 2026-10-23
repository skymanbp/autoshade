//! Raster helpers: the centroid, the colour and tone refusal classifiers, and the mask weights.

use super::*;

/// The alpha-weighted centroid of a mask raster, normalised to its own frame.
///
/// This is the `crs:ReferencePoint` a Select Sky component carries. Weighted
/// by alpha rather than taken from a thresholded bounding box on purpose: the
/// segmenter's skies are soft-edged and frequently U-shaped around a
/// silhouette, and a box centre lands in the middle of that U — which is
/// ground, not sky. The centre of mass is inside the covered region for any
/// mask this fit will accept.
///
/// Pixel CENTRES, through the same [`render::MASK_SAMPLE_CENTRE`] the renderer
/// samples with, so the point is in the coordinate system the alpha is read
/// in. `(0.5, 0.5)` for an empty raster: the partition gate upstream already
/// refuses a sky below [`MIN_ZONE_SHARE`], so that is a floor and not a case.
pub(super) fn raster_centroid(mask: &GrayImage) -> (f32, f32) {
    let (w, h) = mask.dimensions();
    let (mut sx, mut sy, mut sa) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y, p) in mask.enumerate_pixels() {
        let a = f64::from(p.0[0]) / 255.0;
        sx += (f64::from(x) + f64::from(render::MASK_SAMPLE_CENTRE)) * a;
        sy += (f64::from(y) + f64::from(render::MASK_SAMPLE_CENTRE)) * a;
        sa += a;
    }
    if sa <= 0.0 || w == 0 || h == 0 {
        return (0.5, 0.5);
    }
    ((sx / sa / f64::from(w)) as f32, (sy / sa / f64::from(h)) as f32)
}

/// Was this note a zone's COLOUR refusal? One outcome, three carriers: since
/// R34 §D2 a region whose pixels are each other's counterparts keeps R33's
/// sentence and a region past the pairing line gets the one that also prints
/// what its cells measured; since 2026-10-21 the local-quality gate has its
/// own, for a colour class the evidence admitted and the gate refused.
/// Everything that asks "was the colour refused?" asks it here, so the
/// question cannot drift apart from the sentences that answer it.
pub(crate) fn is_colour_refusal(key: &str) -> bool {
    key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR
        || key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR_CELLS
        || key == crate::rationale::keys::ZONE_COLOUR_WITHHELD_BY_QUALITY
}

/// The tone half of [`is_colour_refusal`].
pub(crate) fn is_tone_refusal(key: &str) -> bool {
    key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE
        || key == crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE_CELLS
}

/// Per-pixel mask weights for an analysis frame of `w`×`h` — the SAME
/// normalisation and bilinear sampling the engine's mask stage uses
/// (`render::sample_gray_norm` at PIXEL CENTRES, through the shared
/// [`render::MASK_SAMPLE_CENTRE`]), so the moments are measured exactly where
/// the render will apply them. R29 C2 moved this and `apply_masks`' own
/// `weight_at` together; a zone measured on one grid and rendered on another
/// would put the gains half a pixel off the population they were solved from.
pub(crate) fn mask_weights(mask: &GrayImage, w: u32, h: u32) -> Vec<f32> {
    // usize-widen BEFORE multiplying: `w * h` is a u32 product and a frame
    // over u32::MAX pixels would overflow the reservation (panic in debug,
    // pathological reallocation in release) while the loops still push w×h.
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        for x in 0..w {
            out.push(render::sample_gray_norm(
                mask,
                (x as f32 + render::MASK_SAMPLE_CENTRE) / w as f32,
                (y as f32 + render::MASK_SAMPLE_CENTRE) / h as f32,
            ));
        }
    }
    out
}

// --------------------------------------------------------------------------
