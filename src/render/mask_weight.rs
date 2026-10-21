//! Mask coverage: where inside a pixel a mask is sampled, the per-geometry and combined weights, coverage maps and Range Masks.

use super::*;

/// Where inside a pixel a mask is sampled, as a fraction of the pixel's own
/// width — `0.5`, its CENTRE.
///
/// **The whole mask family samples through this one constant** (R29 C2): the
/// frame loops that ask for a weight ([`apply_masks`]' `weight_at`,
/// [`mask_coverage`]'s overlay, `fit_zoned`'s analysis moments), the texel grid
/// [`rasterise_brush_group`] stamps onto, and the texel grid
/// [`sample_gray_norm`] reads back from. Radial, Linear, Brush, Bitmap and
/// AiMask all land on it, because all five reach the frame through those.
///
/// **MEASURED, on two different negatives.** R29 B7 fitted the hard edge
/// (`Feather = 0`) of a Lightroom-exported radial mask on a 6240 × 4160 frame
/// whose sidecar geometry is `Left = 0.333333`, `Right = 0.666667`,
/// `Top = 0.4`, `Bottom = 0.6` — a centre at normalised `(0.5, 0.5)`, i.e.
/// `(3120.0, 2080.0)` in continuous frame units. The fit, in PIXEL-INDEX
/// coordinates, put it at **(3119.46, 2079.50)** (`b7_03`, edge rms 0.24 px);
/// R29 B7-2 re-fitted a SECOND capture — a different body, lens, day and
/// `OriginalDocumentID` (`b7b_18`, D6) — and got **(3119.49, 2079.51)**. An
/// ellipse fitted over pixel INDICES returns `p − 0.5` for a true continuous
/// centre `p`, so both readings say `p = 3119.96 / 3119.99 ≈ 3120` and
/// `2080.00 / 2080.01 ≈ 2080`: Lightroom maps the stored fraction `u` to the
/// continuous position `u·W`, and pixel `i` — whose centre is at `i + 0.5` —
/// therefore carries the mask value of `u = (i + 0.5)/W`.
///
/// **It is also what the rest of this engine already assumed.**
/// [`apply_lens_geometry`] measures every pixel's radius from
/// `cx = (w − 1)/2`, and `x − (w − 1)/2` IS `x + 0.5 − w/2` — the pixel-centre
/// offset. [`MaskUnwarp::at`] takes `(nx − 0.5)·w`, which equals that same
/// offset only under this convention; with `nx = x/w` the mask un-warp sat
/// half a pixel off the resampler it is defined to invert. The two now agree
/// exactly rather than nearly.
///
/// **⚠ RENDER-BEHAVIOUR CHANGE**, the seventh on the v0.35.0 list: every mask
/// of every type now lands half a pixel up and to the left of where this
/// engine used to put it. That is the direction the measurement demands — the
/// old `x/w` gave pixel `x` the value belonging to continuous position `x`,
/// which is its own top-left CORNER — and the size of the correction is
/// exactly 0.5 px on each axis, everywhere, with no dependence on feather,
/// geometry or frame size.
///
/// Not a tunable: it is `0.5` because a pixel's centre is at its middle. It is
/// named only so the four sites that must agree can be seen to agree, and so
/// that a mutation of any one of them is a mutation of a shared constant.
pub(crate) const MASK_SAMPLE_CENTRE: f32 = 0.5;

/// [`mask_weight`] with the frame adaptation applied — the ONE place a stored
/// coordinate becomes the coordinate this engine's pre-geometry rasteriser
/// should be asked about.
///
/// Split from `mask_weight` rather than folded into it so that `mask_weight`
/// keeps meaning exactly "the weight of this geometry at this STORED point",
/// which is what every unit test in this file asserts and what the XMP
/// boundary's byte fidelity is defined against.
pub(super) fn mask_weight_in(
    g: &MaskGeometry,
    nx: f32,
    ny: f32,
    bmp: Option<&image::GrayImage>,
    unwarp: Option<&MaskUnwarp>,
    dims: (f32, f32),
) -> f32 {
    // The explicit split is the H2 boundary. RADIAL retains the landed
    // `m_lr^-1 ∘ T_engine` point sampler byte-for-byte. LINEAR uses only
    // `T_engine`; its corrections-off camera map has already moved the two
    // handles in `MaskFrame::linear_handles_to_raw`, never this sample.
    let (nx, ny) = match (g, unwarp) {
        (MaskGeometry::Radial { .. }, Some(u)) => u.at(nx, ny),
        (MaskGeometry::Linear { .. } | MaskGeometry::Shaped { .. }, Some(u)) => u.engine_at(nx, ny),
        _ => (nx, ny),
    };
    mask_weight_metric(g, nx, ny, bmp, dims)
}

/// Evaluate a geometry using the metric Lightroom uses for its stored frame.
///
/// Linear endpoints are stored as normalized coordinates, but their dot
/// product is a pixel-space measurement. On a non-square frame the two are
/// not equivalent: the pixel vector is `(vx * w, vy * h)`. Axis-aligned and
/// square-frame gradients deliberately retain the old normalized arithmetic,
/// keeping those render bytes stable while making the angled case exact.
fn mask_weight_metric(
    g: &MaskGeometry,
    nx: f32,
    ny: f32,
    bmp: Option<&image::GrayImage>,
    dims: (f32, f32),
) -> f32 {
    match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let (vx, vy) = (full_x - zero_x, full_y - zero_y);
            let len2 = vx * vx + vy * vy;
            if len2 < 1e-9 {
                return 1.0;
            }
            let (w, h) = dims;
            if vx == 0.0 || vy == 0.0 || w == h || !(w > 0.0 && h > 0.0) {
                return linear_coverage(
                    ((nx - zero_x) * vx + (ny - zero_y) * vy) / len2,
                    LINEAR_FALLOFF,
                );
            }
            let dx = (nx - zero_x) * w;
            let dy = (ny - zero_y) * h;
            let px = vx * w;
            let py = vy * h;
            linear_coverage((dx * px + dy * py) / (px * px + py * py), LINEAR_FALLOFF)
        }
        // The same axis and the same metric as the linear arm above; only
        // the falloff read along it differs.
        MaskGeometry::Shaped { zero_x, zero_y, full_x, full_y, falloff } => {
            let (vx, vy) = (full_x - zero_x, full_y - zero_y);
            let len2 = vx * vx + vy * vy;
            if len2 < 1e-9 {
                return 1.0;
            }
            let (w, h) = dims;
            if vx == 0.0 || vy == 0.0 || w == h || !(w > 0.0 && h > 0.0) {
                return shaped_coverage(((nx - zero_x) * vx + (ny - zero_y) * vy) / len2, falloff);
            }
            let dx = (nx - zero_x) * w;
            let dy = (ny - zero_y) * h;
            let px = vx * w;
            let py = vy * h;
            shaped_coverage((dx * px + dy * py) / (px * px + py * py), falloff)
        }
        _ => mask_weight(g, nx, ny, bmp),
    }
}

/// Mask coverage [0,1] at normalized frame coordinate (nx, ny).
pub(super) fn mask_weight(g: &MaskGeometry, nx: f32, ny: f32, bmp: Option<&image::GrayImage>) -> f32 {
    // THE geometry's own inversion bit, read once from the one place that
    // knows which geometries have one (`MaskGeometry::own_inverted`). The
    // three arms below apply it themselves rather than sharing a tail,
    // because each also owns the rule that a geometry with NO coverage is not
    // inverted into covering the whole frame — `1 − 0` on an unresolved alpha
    // is the silent-whole-frame failure, not an inversion.
    //
    // The correction's OWN flag is applied by the caller's weight loop
    // (`apply_masks`, `mask_coverage`, and `lr_pack::alpha_raster`), so the
    // pair composes to `LocalAdjustment::net_inverted` exactly once — and
    // never twice, which is what
    // `an_imported_inverted_ai_mask_renders_the_complement` and
    // `a_zone_ai_mask_renders_exactly_like_the_bitmap_it_replaced` pin from
    // the two ends.
    let own_inverted = g.own_inverted();
    match g {
        MaskGeometry::Linear { zero_x, zero_y, full_x, full_y } => {
            let (vx, vy) = (full_x - zero_x, full_y - zero_y);
            let len2 = vx * vx + vy * vy;
            if len2 < 1e-9 {
                return 1.0;
            }
            linear_coverage(
                ((nx - zero_x) * vx + (ny - zero_y) * vy) / len2,
                LINEAR_FALLOFF,
            )
        }
        MaskGeometry::Shaped { zero_x, zero_y, full_x, full_y, falloff } => {
            let (vx, vy) = (full_x - zero_x, full_y - zero_y);
            let len2 = vx * vx + vy * vy;
            if len2 < 1e-9 {
                return 1.0;
            }
            shaped_coverage(((nx - zero_x) * vx + (ny - zero_y) * vy) / len2, falloff)
        }
        // `roundness` is carried but deliberately NOT rendered — pure ellipse,
        // see `MaskGeometry::Radial` in recipe.rs. Its DOMAIN is known
        // (Lightroom's ±100 integer slider — v0.31.1 widened the importer and
        // the clamp to match), and since R29 B7 (2026-08-20) the no-op is a
        // MEASURED fact, not a guess: a hand-authored Roundness="100" probe
        // renders geometry within 0.1 px of its Roundness=0 reference (edge
        // rms 0.31 px over 1440 angles — no circularisation, no superellipse)
        // and is pixel-identical to it where the two masks overlap. R29 B7-2
        // (2026-08-21) then closed the biggest hole with a Roundness=100 &
        // Feather=50 cross probe: the no-op HOLDS with feather active
        // (|Δα| ≤ 0.006 on both falloff branches, same support endpoint), so
        // Roundness does not modulate the falloff shape either. R29 me3-b
        // (2026-08-21) closed the two scope caveats this comment used to carry.
        // NEGATIVES are no longer untested: a hand-authored Roundness="-100"
        // probe renders the same geometry as its Roundness="+100" sibling on
        // the same frame at Feather=0 (ellipse parameters within 0.03 px,
        // |Δα| ≤ 0.0024 — me3-b §4), and Lightroom accepts and writes the
        // value back verbatim, so the ±100 domain gate in xmp.rs matches what
        // was measured. The "minor-axis sector only" caveat is GONE too: the
        // Roundness 0 vs +100 pair at Feather=50 is byte-identical over the
        // WHOLE exported frame — same entropy-coded segment, max|Δ| = 0 across
        // 26 M pixels (me3-b §5, re-verified first-hand at adjudication) — so
        // every sector is covered, not just the one the earlier fit sampled.
        // me6-2026-09 group D closed the rest of it on a SECOND geometry:
        // box 0.4 × 0.3 centred (0.6, 0.4) at `crs:Angle="30"`, pixel aspect
        // 2.0 — another shape, another place, and a non-zero tilt — exported at
        // Roundness −100 / 0 / +100 crossed with Feather 25 / 50 / 75. All
        // nine are the same pixels: max|Δ| = 0 DN over 26 M pixels in each of
        // the three feather triples, and the files differ only by the six or
        // seven bytes of embedded XMP. Roundness × Angle ≠ 0 is measured, the
        // second geometry is measured, and the no-op holds at every feather
        // that has one.
        // That pack also RETIRES the "zero-mean dither of ≤ ±4 DN" this
        // comment used to register from me3-b §4.3: at q100 with no output
        // sharpening the difference is not small, it is exactly zero, so the
        // earlier wash was that export's own encoder rather than a Roundness
        // effect. What no export tested is a value strictly between −100, 0
        // and +100 — Lightroom's slider is an integer and all three of its
        // ends render identically, so "stored, not rendered" is what the
        // measurements describe.
        // The sibling `feather` HAD the same guessing bug — Lightroom writes it
        // 0..100 and xmp.rs used to import the value raw, so Feather="72"
        // clamped to fully feathered; both XMP directions now convert on the
        // boundary (xmp.rs). Test radial_roundness_is_a_documented_no_op pins
        // the roundness no-op, now as the measured behaviour.
        // `midpoint: _` joins it for the same reason (R25 P5): Lightroom's
        // second falloff knob, carried through the recipe and the sidecar
        // unchanged, with no published mapping onto this engine's `feather`.
        // `mask_version: _` is Lightroom's own schema stamp and has no pixel
        // meaning at all. Both are spelled out rather than swept into `..` so
        // a field added to the geometry cannot reach the renderer unnoticed.
        MaskGeometry::Radial {
            top, left, bottom, right, feather, roundness: _, flipped: _, angle, midpoint: _,
            mask_version: _,
        } => {
            let cx = (left + right) / 2.0;
            let cy = (top + bottom) / 2.0;
            let rx = ((right - left) / 2.0).abs().max(1e-4);
            let ry = ((bottom - top) / 2.0).abs().max(1e-4);
            // Rotation (engine convention, recipe.rs `MaskGeometry::Radial`):
            // rotate the SAMPLE POINT about the bbox centre by −angle, in
            // normalised frame coords — equivalent to rotating the ELLIPSE by
            // +angle, which on screen (x right, y DOWN) is CLOCKWISE.
            // MEASURED, not derived: a synthetic one-radial recipe rendered by
            // the released binary at angle +30 puts the darkest row at
            // x = 150 → 450 at rows 147 → 249, i.e. the band descends — right
            // end DOWN — and at −30 it ascends (E1-verdict §4a, R25 P9).
            // The matrix below is `R(+θ)` applied to the point, which IS
            // counter-clockwise in a y-UP maths frame; this comment used to
            // carry that reading while explicitly claiming the y-down screen
            // sense, so it named the direction backwards. Code unchanged.
            let (mut px, mut py) = (nx - cx, ny - cy);
            if *angle != 0.0 {
                let (s, c) = (-angle.to_radians()).sin_cos();
                (px, py) = (px * c - py * s, px * s + py * c);
            }
            let d = ((px / rx).powi(2) + (py / ry).powi(2)).sqrt();
            // The falloff is Lightroom's MEASURED α(ρ), read out of the eleven
            // measured columns plus the analytic f = 0 edge — see
            // `radial_falloff` for the table, its provenance, and the three
            // successive laws it buries. NOTHING else on this arm
            // moves: the frame adaptation (`mask_weight_in`), the rotation
            // convention above, the sample point, and the `flipped` polarity
            // below all predate this batch and keep their own pins.
            //
            // The history, because it took four rounds of measurement:
            //
            // * v0.32.0 replaced `d_out = 1` — the effect reaching zero exactly
            //   ON the ellipse — with `d_out = 1 + f/2`, recovered from an
            //   11-rung exposure ladder over five frames spanning aspect
            //   1.03 … 7.46. That the OUTER boundary moves with feather at all
            //   was the load-bearing find (the sourced claim said only the inner
            //   one does), and the old edge was a 29 % under-sized mask at
            //   Feather 50 (`PROBE3-ADDENDUM.md` §3.1). The LUT keeps that half.
            // * R27 Batch-8/10 then refuted BOTH endpoints as written: `d_out`
            //   appeared to SATURATE near 1.41 rather than reach 1.5, and `d_in`
            //   read `0.79 − 0.94 f`, negative at f = 1. Left standing at the
            //   time — a two-branch replacement needed its own adjudication.
            // * R29 B7 found out why, with the nomask reference both earlier
            //   batches lacked: both readings were artefacts of forcing ONE
            //   smoothstep across a profile that is not one. `d_out` is CONSTANT
            //   in feather, and α(0) = 1 at every feather, so there is no inner
            //   knee to fit in the first place.
            // * R29 B7-2 supplied the f = 1/5/10/90 rungs and closed it: the
            //   f ∈ (0, 25) opening is continuous, no closed form reaches the
            //   measurement floor, and the shipped ramp was already RIGHT for
            //   f ≤ 5. That last one is why `radial_falloff` degenerates to an
            //   EXACT hard edge at f = 0 rather than to a table row.
            // * R29 me3 measured f = 15/35/65 and INSERTED them as columns
            //   rather than trusting the interpolation between the old ones,
            //   which was reading 14.8 px and 24.9 px wide on the α = 0.5
            //   contour at f = 15 and f = 35.
            //
            // ⚠ RENDER-BEHAVIOUR CHANGE. Every radial mask with feather ≥ 10
            // renders differently from this version on — at Feather 100 the
            // α ≥ 0.5 region was 2.08× Lightroom's and its α = 0.5 contour sat
            // 239 px out on the measured frame's major axis. f = 1 and f = 5
            // move too, by the old law's own residual there (rms 0.009–0.010 in
            // α, concentrated in the annulus within a few percent of ρ = 1), and
            // f = 0 is byte-identical: it takes the analytic branch, which
            // reproduces the old degenerate `ramp` exactly. The me3 insertion
            // moves f ∈ (10, 75) a SECOND time, relative to the eight-column
            // table itself: on a column (15/35/65) by that column's own
            // held-out error, between columns by the interpolation now running
            // through measured neighbours. f ≤ 10, f = 25, f = 50 and f ≥ 75
            // are untouched by it — those columns carried over bit for bit.
            let wgt = radial_falloff(*feather, d);
            if own_inverted {
                1.0 - wgt
            } else {
                wgt
            }
        }
        // Raster mask: bilinear lookup in the pre-decoded bitmap (normalised
        // coords, so the mask's own resolution is independent of the render's).
        // No bitmap = the load failed → inert, warned once by the loader.
        MaskGeometry::Bitmap { .. } => match bmp {
            Some(b) => sample_gray_norm(b, nx, ny),
            None => 0.0,
        },
        // Brush group: RENDERED since R29 Batch-6b, from a MEASURED model —
        // sampled exactly like a raster mask, because by the time it reaches
        // here it IS one. `brush_raster` stamps the dab stream into an 8-bit
        // grey alpha at develop time (a render-time artefact: no schema field,
        // no `schema_era` gate, nothing in `recipe.json` moved) and the caller
        // hands it in through the same `bmp` slot `Bitmap` and `AiMask` use.
        //
        // `None` is the SAME inert contract an unreadable raster gets, and it
        // has exactly two causes: the group yielded no drawable dab (an empty
        // or state-token-only stream, a zero radius, flow 0 — Lightroom draws
        // nothing there either), or the caller never asked for a raster (the
        // unit tests below, which assert `mask_weight`'s meaning at a stored
        // coordinate). Inert is also inert in both compositions — an `Add`
        // component folds in as `1−(1−w)(1−0) = w` and a `Subtract` as
        // `w·(1−0) = w`.
        //
        // THE MODEL, and every number in it is measured, not chosen (R29
        // Batch-6, `b6-analysis.md` in the R29 materials ledger; the two
        // laws re-confirmed out-of-sample against R27 Batch-8/10):
        //
        //     ρ       = |p − dab| / (Radius·W)          dab is a circle in PIXELS
        //     k(ρ;h)  = (1 − ρ^m(h))^n(h)               0 ≤ ρ < 1, else exactly 0
        //     D(f)    = κf / (1 − f + κf)               κ = 0.1284, D(1) = 1 exact
        //     α       = 1 − Π_i (1 − value·D(f_i)·k(ρ_i; h_i))        SCREEN
        //
        // with `ln m(h)` and `ln n(h)` cubics in the hardness (§4.4). See
        // `brush_kernel` / `brush_flow_deposit` for the coefficients and their
        // provenance, and `brush_kernel_matches_the_measured_nine_rungs` /
        // `brush_flow_law_matches_the_measured_deposit` for the pins.
        //
        // WHAT THIS REPLACES, and it is a HARD render-behaviour change: this
        // arm answered `0.0` for every pixel from R27 Batch-4 until now, so a
        // brush-only correction rendered NOTHING while the sidecar carried it
        // whole. Every recipe holding a brush mask therefore renders
        // DIFFERENTLY from here on — that is the point, and it is why the
        // disclosure variants moved with it (`MaskImportReason::BrushRendered`
        // / `MaskLossReason::BrushRendered`, which now say「drawn from our
        // measured model, not Adobe's rasteriser」rather than「not drawn」).
        //
        // The frame half was closed first, by R29 Batch-3. Lightroom rasterises
        // a brush in its pre-lens-correction frame, and so does this engine:
        // `apply_masks` runs before `apply_lens_geometry` and the geometry
        // stage carries the mask exactly as it carries the photograph. So the
        // dab coordinates are ALREADY in the right frame and get no warp — see
        // the mask-warp block header for the measurements and the frame table,
        // and `the_engine_evaluates_masks_before_the_geometry_stage` for the
        // pin. Applying a warp here would apply the field TWICE.
        //
        // Production routing is the explicit `mask_weight_in` match over
        // `MaskFrame`: the RADIAL arm uses `MaskUnwarp::at`, the LINEAR arm uses
        // `MaskUnwarp::engine_at`, and this brush arm stays on its stored point.
        // `is_lr_post_correction_geometry` is retained only as a historical
        // classification assertion in tests.
        //
        // The two group-level fields are spelled out rather than swept into
        // `..`, the same discipline `Radial` follows, so a field added to the
        // geometry cannot reach the renderer unnoticed:
        //
        //  * `inverted` (`crs:MaskInverted` on the Aggregate) IS rendered —
        //    `1 − α` — but only where an alpha exists. Measured `true` on 1 of
        //    39 real groups (F2 anatomy). It is deliberately NOT lifted into
        //    `LocalAdjustment::inverted` at import (xmp.rs: one bit, one home),
        //    so this arm is the only place it can be honoured at all.
        //  * `value` (`crs:MaskValue` on the Aggregate) is NOT a strength and
        //    must never scale anything here: it is the other half of the
        //    subtract pair, measured `(blend_mode, value)` as `(1, 0)` ×23 and
        //    `(0, 1)` ×16, and reading it as a density neutralises every
        //    subtract brush in the library (`recipe::MaskGeometry::Brush`).
        //    The per-STROKE `BrushStroke::value` is the genuine density, and it
        //    scales each dab BEFORE the screen, inside `brush_raster`.
        MaskGeometry::Brush { inverted: _, name: _, blend_mode: _, value: _, strokes: _ } => {
            match bmp {
                Some(b) => {
                    let w = sample_gray_norm(b, nx, ny);
                    if own_inverted { 1.0 - w } else { w }
                }
                // No alpha = no coverage, and NO inversion either. `1 − 0` here
                // would turn a group that drew nothing into a WHOLE-FRAME
                // adjustment at full strength — the identical failure
                // `is_raster_backed` exists to keep a dead bitmap out of, said
                // locally because a brush needs no file and so cannot use that
                // gate (a dab-less group is inert, not broken).
                None => 0.0,
            }
        }
        // AI mask: the RECOMPUTED alpha, sampled exactly like a raster mask —
        // because that is what it is. `segment::resolve_ai_masks` runs our own
        // segmenter at the reference point the sidecar names and caches the
        // 8-bit grey PNG beside the develop; `raster: None` (not resolved yet,
        // or the model declined) takes the same inert path a `Bitmap` with an
        // unreadable file takes, and both disclosure channels say so
        // (`MaskImportReason::AiMaskRecomputed` / `MaskLossReason::…`).
        //
        // The honest sentence, which the disclosures also carry: these pixels
        // are NOT Adobe's. The sidecar holds no raster, so the alpha here comes
        // from a different model with a different edge behaviour — an
        // approximation of the photographer's intent, never a reproduction of
        // their mask.
        // `inverted` IS rendered here now, and that is a render-behaviour
        // change with a bug on the other side of it. This arm read no
        // inversion bit at all, while `parse_one_correction` deliberately
        // homes a `Mask/Image` component's `crs:MaskInverted` INSIDE the
        // geometry (one bit, one home) — so every Lightroom sky mask the
        // photographer had inverted rendered here as a mask over the sky, the
        // exact region they excluded. `None` keeps the inert contract: no
        // alpha is no coverage, never a `1 − 0` over the whole frame.
        MaskGeometry::AiMask { .. } => match bmp {
            Some(b) => {
                let w = sample_gray_norm(b, nx, ny);
                if own_inverted { 1.0 - w } else { w }
            }
            None => 0.0,
        },
    }
}

/// The adjustment's COMBINED coverage at normalised (nx, ny): the base
/// geometry's weight folded with each component in list order (Lightroom's
/// Add / Subtract / Intersect grammar — the algebra is documented on
/// [`crate::recipe::MaskCombine`]). Inversion / amount / range are the
/// caller's layers, exactly as with the single-geometry `mask_weight`.
///
/// `unwarp` is the frame adaptation ([`MaskFrame`]) — applied PER COMPONENT,
/// through [`mask_weight_in`], because a correction can hold a post-correction
/// radial and a pre-correction brush side by side and each must be asked about
/// in its own frame. Folding the map in at this level instead would have moved
/// the brush too.
pub(super) fn combined_mask_weight(
    m: &crate::recipe::LocalAdjustment,
    nx: f32,
    ny: f32,
    base: Option<&image::GrayImage>,
    comp_bmps: &[Option<&image::GrayImage>],
    unwarp: Option<&MaskUnwarp>,
    dims: (f32, f32),
) -> f32 {
    let mut w = mask_weight_in(&m.mask, nx, ny, base, unwarp, dims);
    for (c, bmp) in m.components.iter().zip(comp_bmps) {
        // A raster-backed component with NO raster carries no coverage, and
        // its inversion must not turn that into the whole frame (an inverted
        // Add would cover everything at full strength, an inverted Subtract
        // would wipe the base). The develop already skips the whole
        // adjustment for a dead raster (the inert contract, recipe.rs
        // `MaskGeometry::Bitmap`); this is the same rule for every caller
        // that reaches the combination directly, and until 2026-09-24 the
        // component's own `inverted` was applied to the missing raster.
        if bmp.is_none() && is_raster_backed(&c.geometry) {
            continue;
        }
        let cw = mask_weight_in(&c.geometry, nx, ny, *bmp, unwarp, dims);
        let cw = if c.inverted { 1.0 - cw } else { cw };
        w = match c.mode {
            crate::recipe::MaskCombine::Add => 1.0 - (1.0 - w) * (1.0 - cw),
            crate::recipe::MaskCombine::Subtract => w * (1.0 - cw),
            crate::recipe::MaskCombine::Intersect => w * cw,
        };
    }
    w
}

/// Coverage map of ONE local adjustment for on-screen display: geometry ×
/// inversion × amount × range, evaluated with the SAME primitives
/// `apply_masks` uses (`mask_weight` / `range_weight`), so the overlay the
/// GUI paints is the weight the render actually applies. `reference`
/// supplies the pixels the range mask is judged on — pass the develop as it
/// stands when THIS mask runs (its PREFIX: earlier masks applied, matching
/// apply_masks' sequential stacking; the GUI's overlay and range sampler
/// both do). Output is an 8-bit map at the reference's size
/// (255 = full effect), in the ORIGINAL frame like every mask.
///
/// `frame` must be the SAME [`MaskFrame`] the caller will use on the pixels
/// this overlay is painted over. The GUI's overlay applies
/// [`apply_lens_geometry`] to the coverage raster so the red wash follows the
/// rendered pixels; with the R29 Batch-3 wiring the render now puts a
/// parametric mask back on its STORED coordinates, so an overlay built without
/// the same adaptation would advertise coverage the render does not apply —
/// by the full field, up to 186 px at 24 mm.
pub fn mask_coverage(
    m: &crate::recipe::LocalAdjustment,
    reference: &DynamicImage,
    frame: MaskFrame<'_>,
) -> image::GrayImage {
    let rgb = reference.to_rgb8();
    let (w, h) = rgb.dimensions();
    // A muted (eye-toggled) mask applies nothing — advertise nothing.
    if !m.enabled {
        return image::GrayImage::new(w, h);
    }
    // Same one-time H2 preparation as `apply_masks`; keeping it above the
    // pixel loop makes the overlay and render share both topology and metric.
    let framed_mask = frame.linear_handles_to_raw(m, (w as f32, h as f32));
    let m = framed_mask.as_ref();
    let unwarp = frame.unwarp((w as f32, h as f32));
    let unwarp = unwarp.as_ref();
    // DROPPED channel, for the same reason as `dead_bitmap_rasters` (R29-1):
    // this is the overlay probe, it runs per frame, and a dead raster's
    // disclosure is the empty coverage it returns plus the ⚠ on the mask row —
    // not a console line the windowed surface cannot show anyway. Under 5c
    // this was a `None` that suppressed only the STEM.
    let probe = crate::diag::dropped();
    // `or_else`, not a second branch: a brush group has no file for
    // `load_mask_bitmap` to find and a bitmap has no dab stream to stamp, so
    // exactly one of the two answers for any geometry. Same frame size the
    // caller will paint this overlay over, so the wash is the weight the
    // render applies — which is what
    // `the_gui_coverage_overlay_matches_what_the_render_applies` asserts.
    let bmp = load_mask_bitmap(&m.mask, &probe).or_else(|| brush_raster(&m.mask, w, h));
    let comp_bmps: Vec<Option<std::sync::Arc<image::GrayImage>>> = m
        .components
        .iter()
        .map(|c| load_mask_bitmap(&c.geometry, &probe).or_else(|| brush_raster(&c.geometry, w, h)))
        .collect();
    // Same load-failure contract as `apply_masks` (inert, inversion included,
    // components included), so the overlay never advertises coverage the
    // render will not apply.
    if (bmp.is_none() && is_raster_backed(&m.mask))
        || m.components
            .iter()
            .zip(&comp_bmps)
            .any(|(c, b)| b.is_none() && is_raster_backed(&c.geometry))
    {
        return image::GrayImage::new(w, h);
    }
    let comp_refs: Vec<Option<&image::GrayImage>> =
        comp_bmps.iter().map(|bmp| bmp.as_deref()).collect();
    let amount = m.amount.clamp(0.0, 1.0);
    let mut out = image::GrayImage::new(w, h);
    for (x, y, px) in out.enumerate_pixels_mut() {
        // Same normalisation as apply_masks' weight_at — pixel CENTRES,
        // through the shared [`MASK_SAMPLE_CENTRE`], so the wash cannot drift
        // half a pixel from the weight the render applies.
        let mut wgt = combined_mask_weight(
            m,
            (x as f32 + MASK_SAMPLE_CENTRE) / w as f32,
            (y as f32 + MASK_SAMPLE_CENTRE) / h as f32,
            bmp.as_deref(),
            &comp_refs,
            unwarp,
            (w as f32, h as f32),
        );
        if m.inverted {
            wgt = 1.0 - wgt;
        }
        wgt *= amount;
        if wgt > 0.001
            && let Some(rm) = &m.range
        {
            let p = rgb.get_pixel(x, y);
            wgt *= range_weight(
                rm,
                &[p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0],
            );
        }
        *px = image::Luma([(wgt.clamp(0.0, 1.0) * 255.0).round() as u8]);
    }
    out
}

/// [`mask_coverage`] in the frame [`develop_preview`] evaluates the mask in,
/// for the fit's rulers: the recipe answers "will geometry follow?" exactly
/// as `develop_preview_inner` answers it (the clamped recipe, its composed
/// profile, the manual amount), so a contour built here sits where the
/// preview paints the mask's edge.
///
/// R38 (2026-09-12): the native four-gradient tile trial, the zone regression
/// check and the band rulers built their contours with
/// [`MaskFrame::AsRendered`] and then judged `develop_preview`'s pixels.
/// Under the reference pair's lens profile (distortion to 1.050) the preview
/// paints a parametric tile's edge three to four analysis pixels (of 384)
/// off its stored contour; a step ruler's feet 1.5 px each side of the STORED
/// contour straddled two lifted pixels on the sky sides, and the one
/// undisplaced edge ran over dark land — so a 27-code seam read 5 codes and
/// shipped as a rectangle in the sky. Raster geometry is unmoved by every
/// frame, which is why only the parametric carrier misread.
pub fn preview_mask_coverage(
    m: &crate::recipe::LocalAdjustment,
    reference: &DynamicImage,
    recipe: &EditRecipe,
) -> image::GrayImage {
    let validated = crate::recipe::ValidatedRecipe::new(recipe);
    let geom = geometry_profile(&validated);
    mask_coverage(m, reference, MaskFrame::downstream(&geom, validated.lens_distortion))
}

/// The brightness-independent colour distance a [`RangeMask::Color`]
/// selects on: each channel divided by its own pixel's Rec.601 luma, then
/// Euclidean. The two arguments have the same FORM, so one function answers
/// both questions the colour range asks -- "how far is this pixel from the
/// band's reference colour" (the mask's own selector) and "how far apart are
/// these two pixels" (the step the range family measures across a colour
/// band's ramp).
///
/// `INFINITY` where either colour is too dark to carry a reliable
/// chromaticity, which is the documented rule this used to spell inline
/// inside [`range_weight`]: clamping instead let a black pixel match a black
/// reference at full weight through arbitrary 1e-4/1e-4 ratios, and flooring
/// a near-black REFERENCE made it read as ordinary grey and select bright
/// neutral regions at full weight. Each caller decides what the unreadable
/// pixel means for its own question: zero weight in the mask, a skipped
/// sample in the range family's boundary gate.
pub fn chromaticity_distance(reference: &[f32; 3], px: &[f32; 3]) -> f32 {
    if luma601(px) < 1e-4 || luma601(reference) < 1e-4 {
        return f32::INFINITY;
    }
    let rl = luma601(reference).max(1e-4);
    let pl = luma601(px).max(1e-4);
    let mut d2 = 0.0;
    for (rc, pc) in [(reference[0], px[0]), (reference[1], px[1]), (reference[2], px[2])] {
        let diff = rc / rl - pc / pl;
        d2 += diff * diff;
    }
    d2.sqrt()
}

/// The chromaticity tolerance a `crs:ColorAmount` of zero still selects.
const COLOUR_RANGE_MIN_TOLERANCE: f32 = 0.15;
/// How much further a full `crs:ColorAmount` of one widens it.
const COLOUR_RANGE_TOLERANCE_SPAN: f32 = 0.9;

/// The chromaticity distance at which a colour Range Mask's weight reaches
/// zero. The mask holds full weight out to half of it and ramps to nothing
/// over the second half; Lightroom's `crs:ColorAmount` (0..=1, LR default
/// 0.5) is the widening knob and [`COLOUR_RANGE_MIN_TOLERANCE`] is what a
/// zero amount still selects.
pub fn colour_range_tolerance(amount: f32) -> f32 {
    COLOUR_RANGE_MIN_TOLERANCE + COLOUR_RANGE_TOLERANCE_SPAN * amount.clamp(0.0, 1.0)
}

/// The inverse of [`colour_range_tolerance`]: the `crs:ColorAmount` whose
/// tolerance is `tolerance`, clamped into the 0..=1 the schema allows. A
/// measured radius under the floor returns 0 and the emitted mask is WIDER
/// than the measurement asked for; one over the ceiling returns 1 and it is
/// narrower. Both are honest limits of the sidecar's own grammar, and a
/// caller that cares prints the tolerance it actually got.
pub fn colour_range_amount(tolerance: f32) -> f32 {
    ((tolerance - COLOUR_RANGE_MIN_TOLERANCE) / COLOUR_RANGE_TOLERANCE_SPAN).clamp(0.0, 1.0)
}

/// Per-pixel Range Mask weight [0,1] — Lightroom's 范围蒙版, multiplied into the
/// geometric mask weight (intersection).
///
/// * `Luminance`: trapezoid over `LumRange` — smooth ramp lo_outer→lo, hold 1
///   across lo..hi, ramp down hi→hi_outer. Degenerate edges (outer == inner,
///   e.g. ACR's real `"… 1.000000 1.000000"`) become hard steps.
/// * `Color`: falloff on the luminance-invariant chromaticity distance to the
///   reference colour (each colour divided by its own luma), so a darker patch
///   of the same hue still matches. `amount` widens tolerance: at the LR-default
///   0.5 a saturated reference keeps same-hue pixels (d=0), rejects neutral grey
///   (d≈0.8) and opposite hues (d≳2); at 1.0 grey gains partial weight. Very
///   dark pixels (luma < 1e-4) have no reliable chroma and get weight 0.
pub fn range_weight(rm: &RangeMask, px: &[f32; 3]) -> f32 {
    match rm {
        RangeMask::Luminance { lo_outer, lo, hi, hi_outer } => {
            let l = luma601(px);
            // The upper edge must stay INCLUSIVE at l == hi (the trapezoid
            // holds 1 across lo..=hi): ramp's degenerate step counts x == e0
            // as already past, so a full-range {0,0,1,1} mask silently
            // rejected pure white. The lower edge needs no twin — ramp's
            // step already includes l == lo on the hold side.
            let up = if *hi_outer - *hi < 1e-6 {
                if l > *hi { 1.0 } else { 0.0 }
            } else {
                ramp(*hi, *hi_outer, l)
            };
            ramp(*lo_outer, *lo, l) * (1.0 - up)
        }
        RangeMask::Color { r, g, b, amount, .. } => {
            // Documented: very dark pixels have no reliable chroma and get
            // weight 0 — clamping instead let a black pixel match a black
            // reference at full weight through arbitrary 1e-4/1e-4 ratios.
            // The REFERENCE is held to the same rule: flooring a near-black
            // reference and normalising it made it read as ordinary grey and
            // select bright neutral regions at full weight.
            let d = chromaticity_distance(&[*r, *g, *b], px);
            if !d.is_finite() {
                return 0.0;
            }
            let d_max = colour_range_tolerance(*amount);
            1.0 - ramp(0.5 * d_max, d_max, d)
        }
    }
}
