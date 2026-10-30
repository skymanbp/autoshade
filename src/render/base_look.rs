//! The camera-matched base look: the neutral develop the camera's rendition is a tone map of, and the knots read off it.

use super::*;

/// The part of `neutral` the camera's embedded rendition shows: the whole
/// develop when the two share the sensor frame, else the centred crop at the
/// rendition's aspect. A body set to an in-camera aspect writes a centred
/// crop (a Sony 4:3 preview over the 3:2 sensor measured centred at NCC 0.987
/// against 0.83 for either side, v1.2.2); pairing the full frame against it
/// put the edge strips' histogram on one side of the CDF match only.
pub fn camera_frame_of(neutral: &DynamicImage, camera: &DynamicImage) -> DynamicImage {
    let (nw, nh) = (neutral.width(), neutral.height());
    let (cw, ch) = (camera.width(), camera.height());
    if cw == 0 || ch == 0 || crate::fit::same_frame_plausible_dims((nw, nh), (cw, ch)) {
        return neutral.clone();
    }
    let target = cw as f64 / ch as f64;
    let (w, h) = if nw as f64 / nh as f64 > target {
        ((nh as f64 * target).round() as u32, nh)
    } else {
        (nw, (nw as f64 / target).round() as u32)
    };
    let (w, h) = (w.clamp(1, nw), h.clamp(1, nh));
    neutral.crop_imm((nw - w) / 2, (nh - h) / 2, w, h)
}

/// Estimate a photo's camera base curve: `[x, y]` knots mapping the NEUTRAL
/// develop's luma toward the camera's embedded rendition by CDF match, read
/// off BLOCK MEANS of the two pictures (2026-09-23).
///
/// Both pictures go through the SAME ≤1024px box-thumbnail, are cut into
/// 64-column block-mean lumas on their own pixel grids ([`block_lumas`], the
/// grid [`corner_residual`] reads), and the two sorted lists of blocks are
/// walked together from the darkest: a knot closes when its blocks span at
/// least `MIN_SPAN` of neutral luma AND number at least `MIN_BLOCKS`, and it
/// is the median block on each side. Knots therefore sit only where the
/// picture HAS mass (a fixed input grid once planted equal-y knots inside
/// empty luma bands — night sky vs street lamps — which the monotone LUT
/// builder flattened into ~30-level plateaus, and latched its top knots to
/// white on frames darker than the grid); the pinned (1,1) endpoint carries
/// the tail.
///
/// Until 2026-09-23 the match was made on PIXELS, with eleven knots at fixed
/// quantiles. Two things in that reading were not tone. (1) The pixel
/// histogram of the camera's rendition carries its sharpening, its noise and
/// its JPEG texture, which a ≤1024 develop of ours has averaged away, so on a
/// night sky — whose whole picture sits in a band 0.13 wide — the camera's
/// spread read as contrast: slopes of 1.3–1.5 across the sky where the same
/// two pictures paired block by block respond at 1.05. (2) Eleven quantiles
/// of a band that narrow land 0.004–0.01 apart, and the rendition is 8-bit,
/// so each knot's camera side is quantised to 1/255 = 0.0039: the slope
/// between neighbours wandered 0.85–1.72 from quantisation alone. Rendered,
/// both put grain back into the sky that the cleaner had taken out — the
/// star standard's front-end lines (`scripts/denoise_star_standard.py`, 1f
/// and 2f) read the final render's fine-noise ratio 0.3315 against the
/// curve-free 0.2992 (the line: ≤ 0.3001) and its tile-to-tile width 0.0310
/// against 0.0150 (≤ 0.0278). A block mean averages the texture out before
/// the match, and knots ≥ 0.06 apart make the 8-bit step at most 6 % of a
/// slope; through the product path the same frame then reads 0.2914 and
/// 0.0137, and the render sits nearer the camera's own rendition at the
/// median block on all seven ILCE-7RM4A frames measured (rms 2.95 vs 2.97 on
/// the star frame, 11.5–45.6 vs 11.9–47.5 on the six daylight frames, whose
/// renditions carry local tone the curve cannot follow either way).
///
/// A block-mean CDF has no notion of place, so the corner question is still
/// settled BEFORE it ([`estimation_base`]). Field-measured on A7RIV ARWs: the
/// neutral develop sits 0.6–1.4 EV under the camera JPEG with a consistent S
/// shape that is NOT a single gain (midtones move ~3× more than the toe) —
/// hence a curve. Returns `None` when it cannot JUDGE (a picture smaller than
/// the block grid on either side — an inability the pre-era repair must never
/// mistake for a verdict), `Some(empty)` for the identity verdict (= no base
/// look), and `Some(knots)` otherwise.
pub fn camera_base_knots(
    neutral: &DynamicImage,
    camera: &DynamicImage,
) -> Option<Vec<[f32; 2]>> {
    const EST_EDGE: u32 = 1024;
    const COLS: usize = 64;
    /// The least neutral luma a knot's blocks span — 15 levels of the 8-bit
    /// rendition, so the rendition's quantisation is at most 6 % of a slope.
    const MIN_SPAN: f32 = 0.06;
    /// The fewest blocks a knot is read from: a median over 64 block means.
    const MIN_BLOCKS: usize = 64;
    fn blocks(img: &DynamicImage, rows: usize) -> Option<Vec<f32>> {
        let small;
        let img = if img.width().max(img.height()) > EST_EDGE {
            small = super::thumbnail(img, EST_EDGE, EST_EDGE);
            &small
        } else {
            img
        };
        let mut v = block_lumas(img, COLS, rows)?;
        v.sort_by(f32::total_cmp);
        Some(v)
    }
    // The grid follows the neutral's aspect; each side is cut on its OWN
    // pixel grid, so nothing finer than a block has to line up — and a CDF
    // of block means does not pair blocks at all.
    let aspect = neutral.height().max(1) as f32 / neutral.width().max(1) as f32;
    let rows = ((COLS as f32 * aspect).round() as usize).max(1);
    let (Some(xs), Some(ys)) = (blocks(neutral, rows), blocks(camera, rows)) else {
        // A picture smaller than the grid cannot be compared — an INABILITY,
        // distinct from the identity verdict below. Sharing its empty return
        // meant a tiny embedded thumbnail read as "this photo needs no base
        // look", and once the repair adopted empty answers it permanently
        // cleared saved curves over an estimate that never judged anything.
        return None;
    };
    debug_assert_eq!(xs.len(), ys.len(), "one grid, two pictures");
    let n = xs.len().min(ys.len());
    if n < MIN_BLOCKS {
        return None;
    }
    // Walk the sorted neutral blocks from the darkest; the camera side is
    // read over the SAME index range, which is the CDF match at block scale.
    let mut knots: Vec<[f32; 2]> = vec![[0.0, 0.0]];
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for i in 1..=n {
        if i == n || (xs[i] - xs[start] >= MIN_SPAN && i - start >= MIN_BLOCKS) {
            groups.push((start, i));
            start = i;
        }
    }
    // A short tail (fewer blocks than a knot needs) joins the knot before it
    // rather than standing as a knot of its own.
    if let [.., prev, last] = groups[..]
        && last.1 - last.0 < MIN_BLOCKS
    {
        let len = groups.len();
        groups[len - 2] = (prev.0, last.1);
        groups.pop();
    }
    for &(a, b) in &groups {
        let mid = (a + b) / 2;
        knots.push([xs[mid], ys[mid]]);
    }
    knots.push([1.0, 1.0]);
    // Identity guard: a baked source (or an already camera-matched render)
    // maps onto itself — return empty so the recipe stays clean.
    let max_dev = knots.iter().map(|p| (p[1] - p[0]).abs()).fold(0.0f32, f32::max);
    if max_dev < 0.02 {
        return Some(Vec::new());
    }
    Some(knots)
}

/// The camera-matched base look of one photo — the ONE entry of the three open
/// paths (the GUI open worker, `pipeline::photo_base_knots*`, `serve`'s fresh
/// open): the neutral develop paired with the camera's embedded rendition LIKE
/// WITH LIKE ([`estimation_base`]), then CDF-matched on block means
/// ([`camera_base_knots`]).
/// Until 2026-09-21 each path assembled the pair by hand, and they had drifted:
/// only the pipeline's paired the frame the rendition shows (v1.2.2), so a GUI
/// or web open of a body set to an in-camera aspect matched the whole sensor
/// against a centred crop.
pub fn camera_base_look(
    neutral: &DynamicImage,
    lens: &crate::recipe::LensProfile,
    camera: &DynamicImage,
) -> Option<Vec<[f32; 2]>> {
    camera_base_knots(&estimation_base(neutral, lens, camera), camera)
}

/// The neutral develop the camera's rendition is a tone map OF — the picture
/// the CDF match may be run against. Two things can stand between the two
/// pictures besides tone, and both are settled here:
///
/// * the FRAME — a body set to an in-camera aspect writes a centred crop
///   ([`camera_frame_of`]);
/// * the CORNERS — a stamped lens profile lifts them on our canvas. Whether
///   the camera's rendition shows that lift is MEASURED on the pair
///   ([`corner_residual`]), and the lift is applied to the estimate's neutral
///   only when the rendition shows it.
///
/// From 2026-08-03 the lift was applied unconditionally, on a review's
/// statement that the camera JPEG already contains the correction. Measured
/// 2026-09-21 on ten ILCE-7RM4A frames (five lenses' worth of corner gains,
/// 1.33–1.98; the body's own switch, tag 0x7031, reads 257 on every one):
/// NONE of the ten embedded previews shows it. A CDF match has no notion of
/// place, so the lift the camera never made came out as TONE: on a star frame,
/// whose whole sky sits in a band 0.13 wide, the estimate's slope ran 0.33–2.25
/// inside that band where the camera's own response — the same two pictures
/// paired in place, block by block — runs 0.99–1.32. Rendered with that
/// estimate the ten frames sat 0.4–6.0 levels DARKER than the camera's
/// rendition at the median block; with the estimate made here they sit within
/// 0.05–1.7, nearer in rms on all ten (`probe_real_raw_base_look` prints both).
///
/// Pre-thumbnailed to the estimator's own working size, so the extra develop
/// pass is a LUT walk over ≤1 MP, not the full frame. The lift runs on the
/// WHOLE frame and the camera's crop is cut afterwards: the gain is a function
/// of the sensor's radius, not the crop's. Geometry is skipped on purpose: it
/// moves pixels, not their luma histogram.
pub(super) fn estimation_base(
    neutral: &DynamicImage,
    lens: &crate::recipe::LensProfile,
    camera: &DynamicImage,
) -> DynamicImage {
    let small = if neutral.width().max(neutral.height()) > 1024 {
        super::thumbnail(neutral, 1024, 1024)
    } else {
        neutral.clone()
    };
    let plain = camera_frame_of(&small, camera);
    if !lens.vignette_active() {
        return plain;
    }
    let vig_only = EditRecipe {
        lens_profile: crate::recipe::LensProfile {
            vignette: lens.vignette.clone(),
            vignette_on: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let lifted = camera_frame_of(&develop_preview(&small, &vig_only), camera);
    // The sensor's own picture is the null: the lift is a claim about what the
    // camera did, and a pair that cannot say (a blown or tiny rendition) has
    // not made it.
    match (corner_residual(&plain, camera), corner_residual(&lifted, camera)) {
        (Some(as_seen), Some(with_lift)) if with_lift.abs() < as_seen.abs() => lifted,
        _ => plain,
    }
}

/// Block-mean luma on a `cols × rows` grid cut on the image's OWN pixel grid:
/// the two pictures of a pair are never resampled onto each other, so nothing
/// finer than a block has to line up (an embedded preview may carry the
/// camera's distortion correction and sits a few pixels off the sensor crop).
/// `None` when the image is smaller than the grid.
pub(super) fn block_lumas(img: &DynamicImage, cols: usize, rows: usize) -> Option<Vec<f32>> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    if w < cols || h < rows {
        return None;
    }
    let px = rgb.as_raw();
    let mut out = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        let (y0, y1) = (r * h / rows, (r + 1) * h / rows);
        for c in 0..cols {
            let (x0, x1) = (c * w / cols, (c + 1) * w / cols);
            let mut sum = 0.0f64;
            for y in y0..y1 {
                for p in px[(y * w + x0) * 3..(y * w + x1) * 3].chunks_exact(3) {
                    sum += 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
                }
            }
            out.push((sum / (((y1 - y0) * (x1 - x0)) as f64 * 255.0)) as f32);
        }
    }
    Some(out)
}

/// Where a corner lift the pair disagrees about would show: the camera's tone
/// map is read off the pair itself (medians of 32 equal-population groups of
/// blocks), and what is left over is compared between the OUTER
/// blocks (from 0.75 of the half diagonal) and the INNER ones (within 0.5) by
/// their medians. A neutral the rendition is a tone map of reads near zero; one
/// that carries a lift the camera never made reads negative (its corners
/// predict more light than the camera drew), and one that lacks a lift the
/// camera did make reads positive. A free tone map cannot absorb either:
/// blocks of equal luma stand at different radii.
///
/// The ten frames above read −0.004…+0.004 as the sensor saw them and
/// −0.009…−0.030 with the lift, the nearest pair 2.8 times apart. LibRaw's
/// develop of the same ten — nothing of this engine in the reading — says the
/// same (−0.005…+0.006 against −0.008…−0.025), and a synthetic rendition that
/// DID carry the lift reads the other way round on all ten. `None` when the
/// pair cannot say: an image smaller than the grid, fewer than 256 unclipped
/// blocks, or fewer than 32 of them in either zone.
pub(super) fn corner_residual(neutral: &DynamicImage, camera: &DynamicImage) -> Option<f32> {
    const COLS: usize = 64;
    const GROUPS: usize = 32;
    fn median(values: &mut [f32]) -> f32 {
        values.sort_by(|a, b| a.total_cmp(b));
        (values[(values.len() - 1) / 2] + values[values.len() / 2]) * 0.5
    }
    let aspect = neutral.height().max(1) as f32 / neutral.width().max(1) as f32;
    let rows = ((COLS as f32 * aspect).round() as usize).max(1);
    let n = block_lumas(neutral, COLS, rows)?;
    let c = block_lumas(camera, COLS, rows)?;
    // A clipped block says nothing about the map between the two pictures.
    let open = |v: f32| v > 2.0 / 255.0 && v < 250.0 / 255.0;
    let mut order: Vec<usize> = (0..n.len()).filter(|&i| open(n[i]) && open(c[i])).collect();
    if order.len() < GROUPS * 8 {
        return None;
    }
    order.sort_by(|&a, &b| n[a].total_cmp(&n[b]));
    let (mut xs, mut ys) = (Vec::with_capacity(GROUPS), Vec::with_capacity(GROUPS));
    for g in 0..GROUPS {
        let group = &order[g * order.len() / GROUPS..(g + 1) * order.len() / GROUPS];
        xs.push(median(&mut group.iter().map(|&i| n[i]).collect::<Vec<f32>>()));
        ys.push(median(&mut group.iter().map(|&i| c[i]).collect::<Vec<f32>>()));
    }
    let tone = |x: f32| -> f32 {
        let j = xs.partition_point(|&v| v < x);
        if j == 0 {
            return ys[0];
        }
        if j == xs.len() {
            return ys[j - 1];
        }
        let (x0, x1) = (xs[j - 1], xs[j]);
        if x1 <= x0 { ys[j] } else { ys[j - 1] + (ys[j] - ys[j - 1]) * (x - x0) / (x1 - x0) }
    };
    let corner = (0.25 + 0.25 * aspect * aspect).sqrt();
    let (mut outer, mut inner) = (Vec::new(), Vec::new());
    for &i in &order {
        let dx = ((i % COLS) as f32 + 0.5) / COLS as f32 - 0.5;
        let dy = (((i / COLS) as f32 + 0.5) / rows as f32 - 0.5) * aspect;
        let r = (dx * dx + dy * dy).sqrt() / corner;
        let residual = c[i] - tone(n[i]);
        if r >= 0.75 {
            outer.push(residual);
        } else if r < 0.5 {
            inner.push(residual);
        }
    }
    if outer.len() < 32 || inner.len() < 32 {
        return None;
    }
    Some(median(&mut outer) - median(&mut inner))
}
