//! Render engine v1 — apply an [`EditRecipe`] to the full-resolution RAW and
//! produce a developed image (no Lightroom needed).
//!
//! Pipeline: `rawler` demosaics + colour-calibrates the sensor data to a
//! full-res sRGB-gamma float image (`RawDevelop::develop_intermediate`), then we
//! apply the recipe. A non-2×2 RGB colour filter array (X-Trans) is the one
//! exception: rawler's demosaic is Bayer-only, so this module demosaics and
//! calibrates that class itself — see [`demosaic_over_cfa_geometry`]. The tonal
//! ops (exposure, contrast, whites/blacks, highlights/shadows, tone curve) are
//! all 1-D functions of a channel value, so they collapse into a single
//! per-channel lookup table; saturation/vibrance run per pixel; then
//! orientation + crop.
//!
//! HONEST SCOPE: these ops are tasteful **approximations**, not bit-exact
//! Lightroom — clarity is a luma unsharp mask, the Detail panel's capture
//! sharpening (v1.5.0) is the law measured against Lightroom 9.4 in v1.6.5 and
//! its luminance and colour noise reduction are built from Adobe's documented
//! slider behaviour, all in `detail.rs`, dehaze is a pointwise
//! scattering inversion (see [`apply_dehaze`]). LOCAL-mask clarity/dehaze/texture ARE engine-rendered
//! since R22 (local temperature/tint since batch #2-B) — see [`apply_masks`]
//! for the pass order and the two documented residues vs the global chain.
//! `texture` gained its GLOBAL stage in R25 B2 and the two share one radius
//! model (0.5% of the short edge, floored at 2 px) — still our own
//! calibration, Adobe's being unpublished, but now one calibration instead of
//! a local-only one with nothing to align against.

use std::borrow::Cow;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use image::{DynamicImage, GenericImageView, ImageBuffer, ImageEncoder, Rgb, RgbImage};
use rawler::decoders::RawDecodeParams;
use rawler::get_decoder;
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::rawsource::RawSource;
use rawler::Orientation;
use rayon::prelude::*;

use crate::recipe::{Crop, EditRecipe, MaskGeometry, RangeMask};

mod denoise_grain;
mod detail;
mod finish;
mod hdr;
mod lens;
mod perspective;
mod profile;
mod base_look;
mod brush;
mod camera;
mod colour;
mod develop;
mod export;
mod geometry;
mod layers;
mod luma;
mod mask_falloff;
mod mask_raster;
mod mask_warp;
mod mask_weight;
mod masks;
mod orient;
mod resample;
mod source;
mod tone;
mod wb;
pub use detail::FilmScale;
pub use finish::{frame_and_finish, CropPolicy};

const LUT_N: usize = 4096;
pub(crate) const MASK_RASTER_BUDGET_BYTES: usize = 256 * 1024 * 1024;

pub use base_look::{camera_base_knots, camera_base_look, camera_frame_of};
#[cfg(test)]
use base_look::{block_lumas, corner_residual, estimation_base};
use brush::{brush_raster, brush_token_num, rasterise_brush_group};
#[cfg(test)]
use brush::{
    BRUSH_RASTER_MAX_WORK, BRUSH_RASTER_MIN_EDGE, brush_flow_deposit, brush_kernel,
    brush_kernel_exponents, brush_raster_dims,
};
pub use camera::{as_shot_wb};
use camera::{
    calibrate_camera_buffer, camera_matrix, normalise_wb, resolve_camera_profile, space_primaries,
    validate_calibration,
};
#[cfg(test)]
pub(crate) use camera::{wb_to_kelvin_tint};
#[cfg(test)]
use camera::{camera_to_space_matrix};
pub use colour::{point_color_at, point_color_rgb, point_color_sampling_recipe};
pub(crate) use colour::{HSL_CENTERS, bracket_bands, chroma, rgb_to_hsl};
use colour::{
    CHROMA_GATE, apply_calibration, apply_color_grade, apply_gray_mix, apply_hsl,
    apply_point_colors, apply_sat_vibrance, hsl_to_rgb, to_u16, to_u8,
};
#[cfg(test)]
use colour::{POINT_HUE_SHIFT_TURNS};
pub use develop::{
    develop_preview, develop_preview_film, develop_preview_framed, develop_preview_with,
    retouch_spots,
};
pub(crate) use develop::{apply_colour_field, field_axis, field_guide_luma};
use develop::{apply_develop_with_rasters, dehaze_airlight, dehaze_px, luma601};
#[cfg(test)]
use develop::{
    apply_dehaze, apply_develop_anon, apply_radial_gain, manual_vignette_lut, profile_vignette_lut,
    vignette_gain_lut,
};
pub use export::{
    ExportColorSpace, ExportOpts, convert_export_color_space, render_to_file, stage_and_publish,
};
pub(crate) use export::{SRGB_ICC, mat3_from_slice, write_working_space};
use export::{ADOBE_PRIM, D65_XY, P3_PRIM, SRGB_PRIM, inv3, mat_mul3, mat_vec3, rgb_to_xyz};
#[cfg(test)]
use export::{ADOBE_RGB_ICC, DISPLAY_P3_ICC, srgb_to_space_matrix, transcode_srgb_trc_to_adobe};
pub use geometry::{
    MANUAL_CA_PER_UNIT, apply_lens_distortion, apply_lens_geometry, apply_lens_geometry_rgba,
    distort_norm, geometry_moves_frame, geometry_profile, lens_geom_norm, lens_ungeom_norm,
    rotate_straighten_rgba, undistort_norm,
};
pub(crate) use geometry::{profile_knot_interp};
use geometry::{profile_fill_scale};
#[cfg(test)]
use geometry::{geometry_fill_scale, lens_geom_factor};
pub(crate) use luma::{bilinear_plane, gauss_blur_plane, neighbours4};
use luma::{
    blur_plane, box_blur_h, box_blur_v, clarity_radius, scale_chroma, texture_pass, unsharp_luma,
    unsharp_luma_weighted, write_luma_additive, write_luma_weighted,
};
#[cfg(test)]
use luma::{TEXTURE_MIN_SIGMA_PX, texture_negative_pass, texture_sigmas};
use mask_falloff::{LINEAR_FALLOFF, linear_coverage, radial_falloff};
#[cfg(test)]
use mask_falloff::{LinearFalloff, RADIAL_FALLOFF, RADIAL_FALLOFF_F};
pub use mask_raster::{
    dead_bitmap_rasters, feather_mask, geometry_raster_path, geometry_raster_path_mut,
    is_raster_backed, mask_from_memory_bounded, mask_raster_write_fits_budget, morph_mask,
    open_mask_bounded, refine_mask_guided,
};
pub(crate) use mask_raster::{sample_gray_norm};
pub use layers::PixelLayer;
use mask_raster::{
    MaskRasterSnapshot, best_effort_mask_raster_snapshot, load_mask_bitmap,
    load_mask_raster_snapshot,
};
#[cfg(test)]
use mask_raster::{
    GUIDED_REFINE_TILE_EDGE, load_mask_raster_snapshot_with_budget, refine_mask_guided_tiled,
};
pub use mask_warp::{
    lr_mask_unwarp_norm, lr_mask_warp_norm, mask_warp_factor, mask_warp_from_camera_knots,
    original_to_view_norm, view_to_original_norm,
};
use mask_warp::{linear_handle_unwarp_norm, lr_mask_center_px};
pub use mask_weight::{
    chromaticity_distance, colour_range_amount, colour_range_tolerance, mask_coverage,
    preview_mask_coverage, range_weight,
};
pub(crate) use mask_weight::{MASK_SAMPLE_CENTRE};
use mask_weight::{combined_mask_weight};
#[cfg(test)]
use mask_weight::{mask_weight, mask_weight_in};
pub use masks::{MaskFrame, engine_active};
use masks::{MaskUnwarp, apply_masks};
#[cfg(test)]
use masks::{LOCAL_NR_GATE, is_lr_post_correction_geometry};
pub use orient::{
    CoordFrame, compose_orientation, orient_point, orient_recipe_coords, orient_vector,
    quarter_turn_orientation, quarter_turns_between, recipe_has_brush_strokes,
    recipe_has_frame_coords, shift_luma_raster, shift_recipe_coords, turn_image,
};
pub(crate) use orient::{oriented};
#[cfg(test)]
use orient::{orientation_mirrors};
pub use resample::{inscribed_dims, rotate_straighten};
use resample::{
    downscale_f32, flip_h_in_place, orient_f32, sample_bilinear_ch, sample_bilinear_rgb16,
};
pub use source::{render_baked_to_image, render_to_image, render_to_image_in, source_pixels};
use source::{apply_crop, rgb16_source};
#[cfg(test)]
use source::{cfa_needs_geometry_demosaic, cfa_taps, demosaic_over_cfa_geometry, wrap_table};
pub use tone::{curve_lut};
pub(crate) use tone::{
    TONE_KNOTS_X, build_tone_lut, sample_lut, sample_tone_model, tone_exposure_curve,
    tone_knot_weights, tone_model_knots, tone_slider_basis,
};
use tone::{apply_rgb_curves, hermite_eval};
#[cfg(test)]
pub(crate) use tone::{limit_tone_sliders, parametric_lut};
pub use wb::{local_temp_to_kelvin, solve_wb_from_neutral};
pub(crate) use wb::{linear_to_srgb, srgb_to_linear, wb_gains};
use wb::{apply_recipe_wb, colour_gain_luts, ramp, smoothstep, transfer_luts};
#[cfg(test)]
use wb::{kelvin_to_rgb};

/// The engine's source as ONE text, for the source-text gates that read what
/// `render.rs` alone held before it was split into files: the modules in their
/// declaration order, the root last (so `source_before_tests` cuts at the root's
/// own test modules and nowhere else). Test-only: nothing here is compiled into a
/// shipped binary.
#[cfg(test)]
pub(crate) const SOURCE_FILES: [(&str, &str); 19] = [
    ("src/render/base_look.rs", include_str!("render/base_look.rs")),
    ("src/render/brush.rs", include_str!("render/brush.rs")),
    ("src/render/camera.rs", include_str!("render/camera.rs")),
    ("src/render/colour.rs", include_str!("render/colour.rs")),
    ("src/render/develop.rs", include_str!("render/develop.rs")),
    ("src/render/export.rs", include_str!("render/export.rs")),
    ("src/render/geometry.rs", include_str!("render/geometry.rs")),
    ("src/render/luma.rs", include_str!("render/luma.rs")),
    ("src/render/mask_falloff.rs", include_str!("render/mask_falloff.rs")),
    ("src/render/mask_raster.rs", include_str!("render/mask_raster.rs")),
    ("src/render/mask_warp.rs", include_str!("render/mask_warp.rs")),
    ("src/render/mask_weight.rs", include_str!("render/mask_weight.rs")),
    ("src/render/masks.rs", include_str!("render/masks.rs")),
    ("src/render/orient.rs", include_str!("render/orient.rs")),
    ("src/render/resample.rs", include_str!("render/resample.rs")),
    ("src/render/source.rs", include_str!("render/source.rs")),
    ("src/render/tone.rs", include_str!("render/tone.rs")),
    ("src/render/wb.rs", include_str!("render/wb.rs")),
    ("src/render.rs", include_str!("render.rs")),
];

/// The engine's own tests, the module `render.rs` carried inline before the split.
#[cfg(test)]
pub(crate) const TESTS_SOURCE: &str = concat!(
    include_str!("render/tests/sources_and_base_look.rs"),
    include_str!("render/tests/tone_and_colour.rs"),
    include_str!("render/tests/local_wb_and_colour_management.rs"),
    include_str!("render/tests/geometry_and_lens.rs"),
    include_str!("render/tests/radial_and_bitmap_masks.rs"),
    include_str!("render/tests/orientation_and_shift.rs"),
    include_str!("render/tests/brush.rs"),
    include_str!("render/tests/coverage_and_ramps.rs"),
    include_str!("render/tests/dehaze_texture_clarity.rs"),
    include_str!("render/tests/local_effects_and_hsl.rs"),
    include_str!("render/tests/budgets_and_refine.rs"),
    include_str!("render/tests/raw_matrix_and_ca.rs"),
    include_str!("render/tests/ai_masks_and_cfa.rs"),
    include_str!("render/tests.rs"),
);

#[cfg(test)]
pub(crate) fn engine_source() -> String {
    SOURCE_FILES.iter().map(|(_, text)| *text).collect()
}

/// The Lightroom export pack's producer and its pinned measurements — a test
/// module, so nothing here is compiled into a shipped binary.
#[cfg(test)]
mod lr_pack;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod native_composition_round_trip {
    use super::*;
    use crate::recipe::{EditRecipe, LocalAdjustment, MaskCombine, MaskComponent};

    #[test]
    fn xmp_component_round_trip_preserves_unquantized_composed_weights() {
        for mode in [MaskCombine::Add, MaskCombine::Subtract, MaskCombine::Intersect] {
            for inverted in [false, true] {
                let mask = LocalAdjustment {
                    mask: MaskGeometry::Linear { zero_x: 0.25, zero_y: 0.0, full_x: 0.75, full_y: 1.0 },
                    components: vec![MaskComponent {
                        geometry: MaskGeometry::Linear { zero_x: 0.5, zero_y: 0.25, full_x: 0.5, full_y: 0.75 },
                        mode, inverted,
                    }], ..Default::default()
                };
                let recipe = EditRecipe { masks: vec![mask.clone()], ..Default::default() };
                let back = crate::xmp::xmp_to_recipe(&crate::xmp::recipe_to_xmp(&recipe));
                for y in 0..73 {
                    for x in 0..127 {
                        let (nx, ny) = ((x as f32 + 0.5) / 127.0, (y as f32 + 0.5) / 73.0);
                        let sample = |m| combined_mask_weight(m, nx, ny, None, &[None], None, (127.0, 73.0));
                        assert_eq!(sample(&mask), sample(&back.masks[0]));
                    }
                }
            }
        }
    }
}
