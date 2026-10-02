use super::*;
use crate::recipe::{EditRecipe, LocalAdjustment};

// The tests themselves, in parts under tests/: each file is spliced in here
// by include!, so its items are items of this module and every test keeps
// its name and its path. The order is the order the old file had.
include!("tests/sources_and_base_look.rs");
include!("tests/tone_and_colour.rs");
include!("tests/local_wb_and_colour_management.rs");
include!("tests/geometry_and_lens.rs");
include!("tests/radial_and_bitmap_masks.rs");
include!("tests/orientation_and_shift.rs");
include!("tests/brush.rs");
include!("tests/coverage_and_ramps.rs");
include!("tests/dehaze_texture_clarity.rs");
include!("tests/local_effects_and_hsl.rs");
include!("tests/budgets_and_refine.rs");
include!("tests/raw_matrix_and_ca.rs");
include!("tests/ai_masks_and_cfa.rs");
include!("tests/pixel_layers.rs");
