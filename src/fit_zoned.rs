//! Zoned reverse-fit — the semantic-region extension of [`crate::fit`].
//!
//! The global fit is statistics over the WHOLE frame, and its gates refuse
//! regional regrades by design (fit.rs rotation budget: "true regional
//! regrades belong to the zoned fit"). This module supplies that fit: segment
//! the same semantic region (the sky) in BOTH the source and the target,
//! compare the two zones' colour statistics, and emit the difference as a
//! bitmap-masked [`LocalAdjustment`](crate::recipe::LocalAdjustment) driving
//! the engine's local dials (render.rs `apply_masks`).
//!
//! Identifiability stance (fit.rs's, one level down): zone MOMENTS (weighted
//! first moments) only — no per-zone CDFs or curves. A zone is a small,
//! soft-edged, non-pixel-aligned population; means are the only statistics
//! stable enough to trust there. The global fit must refuse per-channel
//! moves because it cannot tell a cast from content (WHERE is unknown); here
//! the mask answers WHERE, so exact per-channel gains on the zone are
//! identified — that is the entire expressiveness upgrade.
//!
//! Dial choice (measured, golden-sky pair): a palette transplant (pale-blue
//! sky → vivid gold) demands linear channel ratios of r/b ≈ 5.3×, while ANY
//! white-balance parametrisation caps near 1.9× (the full 2000–40000 K
//! blackbody range) and ±100 saturation only doubles chroma — Temp/Tint/Sat
//! physically cannot repaint. So the fit solves the move as **exact
//! per-channel linear gains** (`color_gains`, engine-rendered inside the
//! mask) with brightness split out into local exposure (the tone LUT's soft
//! shoulder handles it more gracefully than a raw linear gain would).
//! Saturation stays closed-loop through real renders ([`zone_sat_step`]),
//! matching the global fit's philosophy.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, GrayImage};

use crate::fit::{self, FitReport};
use crate::fit_cells::MoveAxis;
use crate::recipe::{LocalAdjustment, MaskCombine, MaskComponent, MaskGeometry, MaskRole, RangeMask};
use crate::render;
use crate::segment::{segment_file, SegmentOpts};

mod detail;
mod field;
mod freemask;
mod range;
mod spatial;
mod subzones;
pub mod semantic;
mod accept;
mod attach;
mod boundary;
mod entry;
mod joint;
mod limits;
mod one_zone;
mod rasters;
mod regions;
mod zone_fit;

use accept::{
    AcceptedZone, BoundaryGateResult, enforce_boundary_gate_toward, enforce_boundary_gates,
    enforce_boundary_gates_with_shrink, push_zone_attached_note, refuse_shrunk_zones,
};
#[cfg(test)]
use accept::{enforce_boundary_gate};
use attach::{
    ZoneAttachment, ZoneCoverage, ZoneDivergence, attach_zones_with_divergence,
    measure_zone_divergence, segment_both, segmentation_input, sky_land_geometry,
};
#[cfg(test)]
use attach::{SEGMENT_BOTH_OVERRIDE, ZoneDivergences, attach_zones};
pub(super) use boundary::{BoundaryReading, rim_cell_gaps, step_cell_gaps};
use boundary::{ZONE_BOUNDARY_HIGH, ZONE_BOUNDARY_PERCENTILE, boundary_rim_toward, boundary_step_toward, lab};
#[cfg(test)]
use boundary::{
    CellSums, TARGET_STEP_MIN_CROSSINGS, ZONE_BOUNDARY_LOW, boundary_rim,
    boundary_step, cell_shares, unasked,
};
pub use entry::{fit_recipe_zoned, fit_recipe_zoned_from, fit_recipe_zoned_with_regions};
use entry::{EdgeMatte, FeatherWidening, PlaneRefinement, matte_source_edge};
#[cfg(test)]
use entry::{
    fit_recipe_zoned_inner, fit_recipe_zoned_inner_with_options, frame_err_under,
    release_unselected_rasters,
};
pub use joint::{JointBucket, JointReading, joint_buckets, joint_reading};
pub(crate) use joint::{
    JOINT_CONFIDENCE_SLOPE, JOINT_DRIFT_TOL, classify_joint_far, joint_buckets_with_evidence,
    joint_reading_with_evidence,
};
use joint::{zone_luma_err};
#[cfg(test)]
pub(crate) use joint::{JOINT_BUCKETS, JOINT_FAR_ERR, JointFarCause};
#[cfg(test)]
use joint::{joint_weights};
pub(crate) use limits::{MIN_MASK_PIXELS, MIN_ZONE_SHARE, ZONE_ATMOS_GAIN_MAX, ZONE_ATMOS_GAIN_MIN};
pub(super) use limits::{
    BOUNDARY_STEP_FLOOR, BOUNDARY_STEP_SHAPE, ZONE_BOUNDARY_RIM_MAX, ZONE_BOUNDARY_STEP_MAX,
};
use limits::{
    MASK_REFINE_EPSILON, MASK_REFINE_RADIUS, SHIPPED_LAYERS, ZONE_ACCEPT_RATIO, ZONE_ATMOS_EV_LIMIT,
    ZONE_ATMOS_SAT_LIMIT, ZONE_CLIP_GROWTH, ZONE_EV_LIMIT, ZONE_FLOOR_MIN_GAIN,
    ZONE_GLOBAL_REGRESSION_TOL, ZONE_MATCHED_ERR, ZONE_MATCHED_EV, ZONE_MIN_ABS_GAIN,
    ZONE_SAT_LIMIT, ZONE_SKIP_ERR, ZONE_TEXTURE_MAX, ZONE_TEXTURE_MIN, ZonedLayerOpts,
};
use one_zone::{ZONE_CONFIDENCE_SLOPE, attach_one_zone};
pub(crate) use rasters::{is_colour_refusal, is_tone_refusal};
pub(super) use rasters::{mask_weights};
use rasters::{raster_centroid};
use regions::{attach_semantic_regions, segment_multiclass_both};
pub use zone_fit::{ZoneMode};
pub(crate) use zone_fit::{
    ZoneMoments, fit_zone_dials, zone_err, zone_luma_cdf, zone_moments, zone_sat_step,
};
use zone_fit::{
    ZONE_VOUCH_SHRINK_STEPS, ZoneAcceptArm, clamp_zone_sat, clamp_zone_sat_for_mode, local_quality,
    scale_zone_colour, scale_zone_tone, shrink_atmosphere_gains_in, shrink_zone_corrections,
    zone_accepts, zone_skips,
};

/// The zoned fit's source as ONE text, for the source-text gates that read what
/// `fit_zoned.rs` alone held before it was split into files: the modules in their
/// declaration order, the root last (so `source_before_tests` cuts at the root's
/// own test modules and nowhere else). Test-only: nothing here is compiled into a
/// shipped binary.
#[cfg(test)]
pub(crate) const SOURCE_FILES: [(&str, &str); 11] = [
    ("src/fit_zoned/accept.rs", include_str!("fit_zoned/accept.rs")),
    ("src/fit_zoned/attach.rs", include_str!("fit_zoned/attach.rs")),
    ("src/fit_zoned/boundary.rs", include_str!("fit_zoned/boundary.rs")),
    ("src/fit_zoned/entry.rs", include_str!("fit_zoned/entry.rs")),
    ("src/fit_zoned/joint.rs", include_str!("fit_zoned/joint.rs")),
    ("src/fit_zoned/limits.rs", include_str!("fit_zoned/limits.rs")),
    ("src/fit_zoned/one_zone.rs", include_str!("fit_zoned/one_zone.rs")),
    ("src/fit_zoned/rasters.rs", include_str!("fit_zoned/rasters.rs")),
    ("src/fit_zoned/regions.rs", include_str!("fit_zoned/regions.rs")),
    ("src/fit_zoned/zone_fit.rs", include_str!("fit_zoned/zone_fit.rs")),
    ("src/fit_zoned.rs", include_str!("fit_zoned.rs")),
];

#[cfg(test)]
pub(crate) fn source_text() -> String {
    SOURCE_FILES.iter().map(|(_, text)| *text).collect()
}

#[cfg(test)]
mod tests;

