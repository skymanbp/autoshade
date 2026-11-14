//! ONE reverse fit, run the way every entry runs it: the CLI's `match`, the
//! desktop app's reverse-fit worker and the statement path of the analysis.
//!
//! The sequence after the source and the target are in one frame
//! ([`super::fit_target`]): with local areas, a fresh `mask-zone-sky` raster
//! claimed in the photo's develop folder and drawn in the source's frame, the
//! zoned fit (semantic regions, else luminance ranges, then spatial tiles);
//! without, the global fit; then the target-geometry note and the terminal
//! re-read at full size. It lived as a closure in each entry until the
//! statement path became the third; one function means the three cannot drift.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::recipe::EditRecipe;

/// The local-areas half of one fit: the photo whose develop folder holds the
/// claimed sky raster, the segmenter, and how many semantic regions it may use.
#[derive(Clone, Copy)]
pub struct ZonedRun<'a> {
    pub photo: &'a Path,
    pub seg: &'a crate::segment::SegmentOpts,
    pub regions: usize,
}

/// One fit's inputs.
#[derive(Clone, Copy)]
pub struct FitRun<'a> {
    /// The fit's source, already in the target's frame (a plate: its frame
    /// and camera registration ride the image).
    pub plate: &'a image::DynamicImage,
    /// The target and what its geometry measured.
    pub target: &'a super::FitTarget,
    /// The calibration the fit composes.
    pub base: &'a EditRecipe,
    pub options: crate::fit::FitOptions<'a>,
    /// `Some` = the zoned fit with local areas; `None` = the global fit.
    pub zoned: Option<ZonedRun<'a>>,
    /// The file the terminal full-size re-read renders (`None` = left as fitted).
    pub reread: Option<&'a Path>,
    pub sink: &'a dyn crate::diag::Sink,
}

/// Run one fit. Returns the report and, for a zoned fit, the raster it
/// CLAIMED (a fresh `mask-zone-sky[-N].png`; a caller that discards the
/// report deletes it). Fails only when the raster cannot be claimed or would
/// be written where the photo library is protected.
pub fn run_fit(run: &FitRun<'_>) -> Result<(crate::fit::FitReport, Option<PathBuf>)> {
    let (mut report, claimed) = match run.zoned {
        Some(z) => {
            // Drawn in the source's frame, and marked so (`render::fit_raster_frame`).
            let mask = crate::store::OwnedRaster::claim(z.photo, "mask-zone-sky")?
                .with_frame(crate::render::fit_raster_frame(run.plate, run.base));
            super::guard_readonly(mask.path(), z.photo)?;
            let report = crate::fit_zoned::fit_recipe_zoned_with_regions(
                run.plate,
                &run.target.image,
                z.seg,
                &mask,
                run.base,
                run.options,
                z.regions.min(crate::fit_zoned::semantic::MAX_SEMANTIC_REGIONS),
            );
            (report, Some(mask.into_path()))
        }
        None => (crate::fit::fit_recipe_from_with(run.plate, &run.target.image, run.base, run.options), None),
    };
    super::note_target_geometry(&mut report, run.target.geometry);
    super::reread_at_full_size(&mut report, run.reread, run.plate, &run.target.image, run.base, run.sink);
    Ok((report, claimed))
}
