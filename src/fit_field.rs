//! The local-field ANALYZER (step 10 / B2 phase A).
//!
//! A 12x32x8 bilateral grid solved on the analysis rasters purely to MEASURE what
//! a spatially varying edit could still win on one pair.  The field never reaches
//! [`crate::render`] or an [`crate::recipe::EditRecipe`]: this module takes pixel
//! slices and returns numbers, so the recipe schema (era 1), the engine and XMP
//! stay untouched.  Its consumers are disclosure and shape verdicts.  It is a
//! port of the validated NumPy experiment `scripts/grid_experiment.py`
//! (`smooth_3tap`, `splat_table`, `GridSystem.forward/adjoint/rhs/matvec/solve`,
//! `BOUNDS_LOW/HIGH`, `OCCUPANCY_MIN`) at `fit::ANALYZE_EDGE` size: same vertex
//! order, same trilinear splat, same conjugate-gradient stopping rule, same
//! post-solve bounds and occupancy floor — at the experiment's 12x8 cells
//! refined to [`FIELD_Y`] vertex rows, with the regulariser rescaled to the
//! refinement (see [`TIKHONOV`]).  Independent structural cell readings
//! run in parallel and are scattered in cell order; the CG solve and every
//! reduction stay sequential/f64, so two solves of one input are bit-identical.

use crate::fit;
use rayon::prelude::*;

/// Grid extent on the x, y and luma axes.  A vertex is
/// `(iy * FIELD_X + ix) * FIELD_B + ib`, exactly `splat_table`'s order.
///
/// The x axis is the evidence-cell grid's 12 columns (`fit_cells`); the y
/// axis is that grid's [`CELL_ROWS`] refined [`FIELD_Y_REFINE`] times, so a
/// cell owns `FIELD_Y_REFINE` vertex rows ([`cell_vertex_spans`]). The
/// refinement is vertical only, and measured rather than aesthetic: a horizon
/// glow rises over the lowest ~15 % of a sky (e-folding ~5 % of the frame,
/// 13 rows at analysis size), and the trilinear read between vertex rows 36
/// rows apart (8 rows over 256) cannot bend that fast. On the night reference
/// pair it averaged a flat-then-steep demand into a band 1.5 L brighter and
/// 1 b* bluer than the target 25 rows above the horizon, kinked at the vertex
/// row (2026-10-20: error left after the field −1.1 L at row 178, +1.4 L at
/// row 200, +0.5 L at the vertex row 146). Twofold (17-row spacing) narrowed
/// the band but left its depth (−1.3 L: the spacing was still above the
/// glow's scale); fourfold (8.5 rows, under it) with the smoother per edge
/// (see [`SMOOTH`]) left 0.6 L, and the sky's row-profile ripple went
/// 2.40 → 0.63 L, its deltaE 3.96 → 3.26. Nothing comparable has been
/// measured along x, and the 12×8 cell grid every admission instrument reads
/// is unchanged. The cost is the recipe: 3072 vertices where there were 768.
pub(crate) const FIELD_X: usize = 12;
pub(crate) const FIELD_Y: usize = CELL_ROWS * FIELD_Y_REFINE;
pub(crate) const FIELD_B: usize = 8;
/// The evidence-cell grid's rows (`fit_cells::CELLS_Y`), the grid
/// [`local_support`] is read on.
pub(crate) const CELL_ROWS: usize = 8;
/// Vertex rows per cell row.
pub(crate) const FIELD_Y_REFINE: usize = 4;
/// The parameters carried at every vertex: `[ev, gain_r, gain_g, gain_b, slope]`.
const PARAMS: usize = 5;
const VERTICES: usize = FIELD_X * FIELD_Y * FIELD_B;
/// Post-solve bounds.  Saturation against them is a DIAGNOSTIC
/// ([`LocalField::saturated`]); the bounds are never widened to fit a field.
const BOUNDS_LOW: [f32; PARAMS] = [-1.25, -0.35, -0.35, -0.35, -0.50];
const BOUNDS_HIGH: [f32; PARAMS] = [1.25, 0.35, 0.35, 0.35, 0.50];
/// A CELL under this much weighted trilinear mass (summed over the vertex
/// rows that refine it, per column and luma bin — [`supported`]) was never
/// measured; its vertices are zeroed rather than left on whatever the
/// regulariser diffused into them. The floor belongs to the 12×8 cell it was
/// set on, not to a vertex: read per refined vertex (at 8 / `FIELD_Y_REFINE`)
/// it zeroed rows a sharp horizon or a dark luma bin leaves thin while their
/// cell is well measured, and on the calibration pair that alone cost the
/// ceiling 0.0023 (0.07242 against 0.07009 at the 8-row grid; with no floor
/// at all the refined grid reads 0.06831).
const OCCUPANCY_MIN: f32 = 8.0;
/// The production regulariser, FIXED — no per-image sweep, ever.  The experiment's
/// sweep picked s=0 on two of its five pairs (an overfitting bias) to buy a last
/// 0.003 of the objective; determinism and no selection bias are worth more.
///
/// Calibrated at 12×8×8 (λ = 1, s = 1 per axis). On the refined rows the
/// terms summed over VERTICES are rescaled so the field costs what it cost
/// there: the Tikhonov term and the x and luma differences are summed over
/// `FIELD_Y_REFINE` times as many vertices, so each weighs
/// `1 / FIELD_Y_REFINE`. The y smoother is NOT rescaled: it stays 1 per
/// edge, which over `(FIELD_Y − 1) / (CELL_ROWS − 1)` as many, shorter, edges
/// is a weaker penalty on a given ramp than the 8-row grid paid — and that is
/// the point. Rescaled to keep the ramp penalty (×31/7), the smoother spread
/// the night pair's horizon glow upward and left a 1.0 L shoulder 20 rows
/// above the horizon that the refinement existed to remove; per edge it left
/// 0.6 L (ripple 1.07 → 0.63 L). A per-edge weight is what the smoother
/// means — adjacent vertices should agree — and resolving what the grid can
/// now resolve is what refining it was for. (v1.3.0 tried a 16×12×8 grid with
/// nothing rescaled and read it worse on the calibration pair, 0.023750
/// against 0.021035; the vertex-count terms above are why.)
pub(crate) const TIKHONOV: f32 = 1.0 / FIELD_Y_REFINE as f32;
pub(crate) const SMOOTH: [f32; 3] = [1.0 / FIELD_Y_REFINE as f32, 1.0, 1.0 / FIELD_Y_REFINE as f32];
pub(crate) const ITERATIONS: usize = 90;
/// Ceiling on the active-set passes of the bound-constrained solve
/// ([`System::solve`]); each pass is one warm-started CG of at most
/// [`ITERATIONS`] steps. The loop normally stops earlier, when the set of
/// parameters pinned on a bound repeats.
const ACTIVE_SET_PASSES: usize = 40;
/// The refusal line of `fit::look_err_with_evidence`, reused verbatim: a pair the
/// objective calls unmeasurable gets no field either.
const IDENTIFIABILITY_MIN: f32 = 1e-5;
const LN2: f64 = std::f64::consts::LN_2;

/// R34 §D4. The two things the SHIPPED colour field solves differently from
/// the ANALYSIS instrument, and nothing else.
///
/// They are parameters rather than a second solver because the field the user
/// exports and the field the sequencer measures its ceiling against must be
/// the same function of the same pixels — the property
/// `the_engine_renders_the_analyzers_field_bit_for_bit` exists to hold. The
/// default IS today's analysis solve, so every existing caller is unchanged
/// byte for byte.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FieldSolveOpts {
    /// Per-channel bound on `[gain_r, gain_g, gain_b]`. The analysis solve
    /// keeps [`BOUNDS_HIGH`]'s own 0.35, so the ceiling it reports is the
    /// number it has always reported; the shipped field takes the strength
    /// budget's `field_gain` (`fit::FitBudget`).
    pub gain: f32,
    /// Weight the fit by the 96-cell structural support. FALSE is the
    /// support-free pass: `local_support` is `1 - clamp(D)` per cell, which is
    /// ~0 wherever a repaint re-synthesised the texture — exactly where the
    /// residual is — so a field weighted by it is starved of the only work it
    /// was attached to do. Dropping the term is admissible ONLY behind a
    /// per-cell admission (`fit_cells`), which is what
    /// `fit_zoned::field::attach_colour_field` puts in front of it.
    pub local_support: bool,
}

impl Default for FieldSolveOpts {
    fn default() -> Self {
        Self { gain: BOUNDS_HIGH[1], local_support: true }
    }
}

impl FieldSolveOpts {
    /// The post-solve bound pair for parameter `p`. Only the three GAIN axes
    /// move; EV and slope keep their constants, because the demand R34
    /// measured is chromatic and widening an axis nobody asked about is how a
    /// bound stops meaning anything.
    fn bounds(self, p: usize) -> (f32, f32) {
        match p {
            1..=3 => (-self.gain, self.gain),
            _ => (BOUNDS_LOW[p], BOUNDS_HIGH[p]),
        }
    }
}

/// The analysis solve's per-channel gain bound, for the budget's DEFAULT
/// point: one constant, named once, so the "default recipe is byte-identical"
/// pin reads the analyzer's own number rather than a copy of it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn default_gain_bound() -> f32 {
    BOUNDS_HIGH[1]
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SolveInfo {
    pub iterations: usize,
    /// Retained as a solver diagnostic and exercised by the phase-A probes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub relative_residual: f32,
}

/// One pair's local field and the readings taken off it.  Nothing here is
/// persisted or rendered at full resolution; every array is at analysis size.
#[derive(Clone, Debug)]
pub(crate) struct LocalField {
    /// `FIELD_X * FIELD_Y * FIELD_B` vertices in `splat_table` order,
    /// bound-clipped, occupancy-zeroed.
    pub grid: Vec<[f32; PARAMS]>,
    /// Weighted trilinear mass at each vertex (fit weight x splat weight).
    #[cfg_attr(not(test), allow(dead_code))]
    pub occupancy: Vec<f32>,
    /// `look_err_with_evidence(field_render, target, evidence)` — the report's own
    /// ruler on the field's render, directly comparable with `err_after`.
    pub ceiling: f32,
    /// The same objective on the unmodified `current` render.
    pub global: f32,
    /// Occupancy-weighted mean of the parameters over each luma bin's 96 vertices.
    pub band_marginal: [[f32; PARAMS]; FIELD_B],
    /// Occupancy-weighted spatial std of bin `b`'s parameters as a luma effect in
    /// [0, 1] units at the bin's centre luma `c_b = b / 7`:
    /// `sqrt(Var[ln2*c_b*ev + c_b*(0.299*g_r + 0.587*g_g + 0.114*g_b) + 0*slope])`.
    /// Slope contributes 0 because `c - guide` is 0 at the guide luma.  NOTE the
    /// metric is blind at `b = 0`, where `c_b = 0` voids every term.
    pub band_dispersion: [f32; FIELD_B],
    /// Per pixel: `luma601(field render) - luma601(band projection render)`.
    pub remainder: Vec<f32>,
    /// Per pixel: the fit weight the solve used (frozen evidence x local support
    /// x unclipped).  Shape verdicts weight the remainder by it, so a pixel whose
    /// vertices hold the occupancy-floor policy zero (a missing measurement, not
    /// a measured zero) cannot masquerade as spatial structure.
    pub weight: Vec<f32>,
    /// Occupancy-supported vertices with at least one parameter on a bound.
    pub saturated: usize,
    pub solve: SolveInfo,
    pub width: u32,
    pub height: u32,
}

impl LocalField {
    /// Solve one pair's field with the production regulariser.  `None` when
    /// `evidence.identifiability <= 1e-5`, when the fit-weight mass is 0, when the
    /// solve is non-finite, or (defensively) on a geometry mismatch.
    pub(crate) fn solve(
        current: &[[f32; 3]], target: &[[f32; 3]], width: u32, height: u32,
        evidence: &fit::EvidenceModel,
    ) -> Option<LocalField> {
        Self::solve_with(
            current, target, width, height, evidence, TIKHONOV, SMOOTH, ITERATIONS,
            FieldSolveOpts::default(),
        )
    }

    /// The same solve with the regulariser exposed, so a test can pin the
    /// lambda-to-infinity property, and — since R34 §D4 — with the two things
    /// the SHIPPED field solves differently from the ANALYSIS instrument.
    /// Production analysis always calls [`Self::solve`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn solve_with(
        current: &[[f32; 3]], target: &[[f32; 3]], width: u32, height: u32,
        evidence: &fit::EvidenceModel, tikhonov: f32, smooth: [f32; 3], iterations: usize,
        opts: FieldSolveOpts,
    ) -> Option<LocalField> {
        let (w, h) = (width as usize, height as usize);
        let n = w.checked_mul(h)?;
        if n == 0 || current.len() != n || target.len() != n { return None; }
        if evidence.source_weights.len() != n || evidence.identifiability <= IDENTIFIABILITY_MIN {
            return None;
        }
        // Fit weight = frozen evidence x local structural support x unclipped.  The
        // evidence model is the fit's own; this module never builds a second one.
        let support = opts
            .local_support
            .then(|| local_support_present(current, target, width, height, evidence.presence.as_deref()));
        let mut fit_weight = vec![0.0f32; n];
        let mut mass = 0.0f64;
        for (i, weight) in fit_weight.iter_mut().enumerate() {
            if unclipped(&current[i]) && unclipped(&target[i]) {
                *weight = evidence.source_weights[i].max(0.0)
                    * support.as_ref().map_or(1.0, |s| s[i]);
            }
            mass += *weight as f64;
        }
        if !mass.is_finite() || mass <= 0.0 { return None; }
        let guide = smooth_3tap_luma(current, w, h);
        let system = System::new(current, target, &guide, &fit_weight, w, h);
        let (raw, solve) = system.solve(tikhonov, smooth, iterations, |p| opts.bounds(p));
        if raw.iter().any(|value| !value.is_finite()) { return None; }

        let mut flat = vec![0.0f32; VERTICES * PARAMS];
        let mut grid = vec![[0.0f32; PARAMS]; VERTICES];
        let mut saturated = 0usize;
        let measured = supported(&system.occupancy);
        for (vertex, cell) in grid.iter_mut().enumerate() {
            let supported = measured[vertex];
            let mut on_bound = false;
            for (p, value) in cell.iter_mut().enumerate() {
                let (low, high) = opts.bounds(p);
                let bounded = raw[vertex * PARAMS + p].clamp(low, high);
                on_bound |= bounded <= low || bounded >= high;
                *value = if supported { bounded } else { 0.0 };
                flat[vertex * PARAMS + p] = *value;
            }
            saturated += usize::from(supported && on_bound);
        }
        let field_render = apply_flat(current, &guide, &system.splat, &flat);
        let ceiling = fit::look_err_with_evidence(&field_render, target, evidence);
        let global = fit::look_err_with_evidence(current, target, evidence);
        let (band_marginal, band_dispersion) = band_summary(&grid, &system.occupancy, &measured);
        let projection = apply_bands(current, &guide, &band_marginal);
        let remainder = field_render.iter().zip(&projection)
            .map(|(field, band)| fit::luma601(field) - fit::luma601(band)).collect();
        let occupancy = system.occupancy.clone();
        Some(LocalField {
            grid, occupancy, ceiling, global, band_marginal, band_dispersion, remainder,
            weight: fit_weight, saturated, solve, width, height,
        })
    }

    /// Field render = `clamp(current + delta, 0, 1)` (`GridSystem.render`).
    ///
    /// Since R33 §G this is a CALL into the engine's own renderer rather than
    /// a second copy of it: the analyzer's measured field and the field the
    /// user's export carries are the same function of the same pixels, and
    /// `the_engine_renders_the_analyzers_field_bit_for_bit` holds them to it.
    pub(crate) fn render(&self, current: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let mut out = current.to_vec();
        crate::render::apply_colour_field(
            &mut out,
            self.width as usize,
            self.height as usize,
            Some(&self.as_recipe_field()),
        );
        out
    }

    /// This solved field as the recipe carries it, at full amount.
    pub(crate) fn as_recipe_field(&self) -> crate::recipe::ColourField {
        crate::recipe::ColourField {
            x: FIELD_X,
            y: FIELD_Y,
            b: FIELD_B,
            grid: self.grid.clone(),
            amount: 1.0,
            enabled: true,
        }
    }
}

/// The 96-cell structural reading, one value per pixel: per 12x8 cell,
/// `1 - clamp(D, 0, 1)` from [`fit::structure_divergence`] over that cell's mask.
///
/// A cell whose eroded core is under [`fit::STRUCTURE_MIN_CORE_PX`] is NOT
/// measured, and a cell nothing measured earns no support: it reads 0, not the
/// 1.0 of a measured match.  Before v1.2.4 the instrument answered such a cell
/// with `Divergence::matched()`, so every eroded cell claimed full structural
/// survival and this term was silently constant wherever the cells were small
/// — the support half of the fit weight was inert exactly where it was least
/// deserved.  When NO cell resolves, the frame carries no structural
/// measurement anywhere; withholding the whole frame on that ground would
/// starve a fit nothing measured, so the term is dropped wholesale (uniform
/// 1.0) instead — the pre-v1.2.4 reading for that case, and the same "may
/// refuse to help, never starve" stance the correspondence field takes.
#[cfg(test)]
pub(crate) fn local_support(
    current: &[[f32; 3]], target: &[[f32; 3]], width: u32, height: u32,
) -> Vec<f32> {
    local_support_present(current, target, width, height, None)
}

/// [`local_support`] over the pixels the target can pair: a cell's reading
/// takes no absent pixel (`fit::presence_of`) into its structure.
pub(crate) fn local_support_present(
    current: &[[f32; 3]], target: &[[f32; 3]], width: u32, height: u32,
    presence: Option<&[f32]>,
) -> Vec<f32> {
    let n = width as usize * height as usize;
    let mut support = vec![0.0f32; n];
    if n == 0 || current.len() != n || target.len() != n { return support; }
    let readings = (0..FIELD_X * CELL_ROWS)
        .into_par_iter()
        .map(|cell| {
            let (cell_x, cell_y) = ((cell % FIELD_X) as u32, (cell / FIELD_X) as u32);
            let mask = (0..n)
                .map(|i| {
                    let (x, y) = (i as u32 % width, i as u32 / width);
                    f32::from(x * FIELD_X as u32 / width == cell_x
                        && y * CELL_ROWS as u32 / height == cell_y)
                        * presence.map_or(1.0, |p| p.get(i).copied().unwrap_or(0.0))
                })
                .collect::<Vec<_>>();
            fit::structure_divergence(current, target, width, height, &mask)
                .map(|reading| 1.0 - reading.d.clamp(0.0, 1.0))
        })
        .collect::<Vec<_>>();
    if readings.iter().all(Option::is_none) {
        return vec![1.0f32; n];
    }
    for (i, slot) in support.iter_mut().enumerate() {
        let (x, y) = (i as u32 % width, i as u32 / width);
        let cell = (y * CELL_ROWS as u32 / height * FIELD_X as u32
            + x * FIELD_X as u32 / width) as usize;
        *slot = readings[cell].unwrap_or(0.0);
    }
    support
}

/// Which vertices were measured: a vertex is supported when the CELL it
/// belongs to — its column and luma bin, summed over the [`FIELD_Y_REFINE`]
/// vertex rows that refine the cell's row — carries at least
/// [`OCCUPANCY_MIN`] of weighted trilinear mass. One reading per cell and bin,
/// so refining the rows cannot zero a vertex its cell has measured.
pub(crate) fn supported(occupancy: &[f32]) -> Vec<bool> {
    let mut out = vec![false; occupancy.len()];
    for cell in 0..FIELD_X * CELL_ROWS {
        let spans: Vec<_> = cell_vertex_spans(cell).collect();
        for bin in 0..FIELD_B {
            let mass: f32 = spans.iter().map(|span| occupancy[span.start + bin]).sum();
            for span in &spans {
                out[span.start + bin] = mass >= OCCUPANCY_MIN;
            }
        }
    }
    out
}

/// The vertices evidence cell `cell` (row-major over the 12×8 cell grid)
/// owns: its column, its `FIELD_Y_REFINE` vertex rows, all `FIELD_B` luma
/// bins — one contiguous span per vertex row. The cell admission
/// (`fit_zoned::field::admit_cells`) carries a cell's verdict onto exactly
/// these, so a vouched cell and the vertices it switches cannot disagree.
pub(crate) fn cell_vertex_spans(cell: usize) -> impl Iterator<Item = std::ops::Range<usize>> {
    let (ix, cy) = (cell % FIELD_X, cell / FIELD_X);
    (0..FIELD_Y_REFINE).map(move |k| {
        let start = ((cy * FIELD_Y_REFINE + k) * FIELD_X + ix) * FIELD_B;
        start..start + FIELD_B
    })
}

/// Both frames strictly inside `(1/255, 254/255)` on all three channels.
/// `pub(crate)`: [`crate::fit_cells`] excludes the same pixels from its cell
/// means for the same reason, and two copies of this line would be two
/// definitions of "this pixel cannot move".
pub(crate) fn unclipped(p: &[f32; 3]) -> bool {
    p[0].min(p[1]).min(p[2]) > 1.0 / 255.0 && p[0].max(p[1]).max(p[2]) < 254.0 / 255.0
}

/// The field's guide, owned by the engine since R33 §G: see
/// [`crate::render::field_guide_luma`]. Kept under its analyzer name so the
/// solve reads the way its Python prototype did.
pub(crate) use crate::render::field_guide_luma as smooth_3tap_luma;

struct Splat { ids: Vec<[u32; 8]>, tri: Vec<[f32; 8]> }

/// `grid_experiment.splat_table`, slot for slot: x-major then y then b, high index
/// `min(low + 1, limit - 1)` so the last column/row/bin folds onto itself.
fn splat_table(guide: &[f32], width: usize, height: usize) -> Splat {
    let (xs, ys) = (
        crate::render::field_axis(width, FIELD_X),
        crate::render::field_axis(height, FIELD_Y),
    );
    let limits = [FIELD_X, FIELD_Y, FIELD_B];
    let mut ids = Vec::with_capacity(guide.len());
    let mut tri = Vec::with_capacity(guide.len());
    for (i, &value) in guide.iter().enumerate() {
        let coords = [xs[i % width], ys[i / width], value.clamp(0.0, 1.0) * (FIELD_B - 1) as f32];
        let (mut low, mut high, mut frac) = ([0usize; 3], [0usize; 3], [0.0f32; 3]);
        for (axis, &limit) in limits.iter().enumerate() {
            let floor = (coords[axis].floor() as i64).clamp(0, limit as i64 - 1) as usize;
            (low[axis], high[axis]) = (floor, (floor + 1).min(limit - 1));
            frac[axis] = coords[axis] - floor as f32;
        }
        let (mut cell, mut weights) = ([0u32; 8], [0.0f32; 8]);
        for (slot, (id, weight)) in cell.iter_mut().zip(weights.iter_mut()).enumerate() {
            let up = [slot >> 2 & 1, slot >> 1 & 1, slot & 1];
            let at = |a: usize| if up[a] == 1 { high[a] } else { low[a] };
            let mass = |a: usize| if up[a] == 1 { frac[a] } else { 1.0 - frac[a] };
            *id = ((at(1) * FIELD_X + at(0)) * FIELD_B + at(2)) as u32;
            *weight = mass(0) * mass(1) * mass(2);
        }
        ids.push(cell);
        tri.push(weights);
    }
    Splat { ids, tri }
}

/// `dc = ln2*c*EV + c*gain_c + (c - guide)*slope` (`GridSystem.forward`).
fn delta(c: &[f32; 3], guide: f32, p: &[f32; PARAMS]) -> [f32; 3] {
    std::array::from_fn(|ch| {
        (LN2 * c[ch] as f64 * p[0] as f64 + c[ch] as f64 * p[1 + ch] as f64
            + (c[ch] - guide) as f64 * p[4] as f64) as f32
    })
}

/// `GridSystem.slice_params` for one pixel: the trilinear read of the grid.
fn slice_flat(v: &[f32], ids: &[u32; 8], tri: &[f32; 8]) -> [f32; PARAMS] {
    let mut acc = [0.0f64; PARAMS];
    for (&id, &weight) in ids.iter().zip(tri) {
        let base = id as usize * PARAMS;
        for (p, slot) in acc.iter_mut().enumerate() { *slot += weight as f64 * v[base + p] as f64; }
    }
    acc.map(|value| value as f32)
}

/// One per-pixel parameter vector applied as the field's delta, display-clamped.
fn apply_params(
    current: &[[f32; 3]], guide: &[f32], mut params: impl FnMut(usize) -> [f32; PARAMS],
) -> Vec<[f32; 3]> {
    current.iter().enumerate().map(|(i, c)| {
        let d = delta(c, guide[i], &params(i));
        [(c[0] + d[0]).clamp(0.0, 1.0), (c[1] + d[1]).clamp(0.0, 1.0),
            (c[2] + d[2]).clamp(0.0, 1.0)]
    }).collect()
}

fn apply_flat(current: &[[f32; 3]], guide: &[f32], splat: &Splat, flat: &[f32]) -> Vec<[f32; 3]> {
    apply_params(current, guide, |i| slice_flat(flat, &splat.ids[i], &splat.tri[i]))
}

/// The band-marginal projection render: the same delta formula with the parameters
/// splatted in luma ONLY — each pixel interpolates `band_marginal` between its two
/// luma bins by the same `b` fraction, no spatial variation.
fn apply_bands(
    current: &[[f32; 3]], guide: &[f32], marginal: &[[f32; PARAMS]; FIELD_B],
) -> Vec<[f32; 3]> {
    apply_params(current, guide, |i| {
        let bin = guide[i].clamp(0.0, 1.0) * (FIELD_B - 1) as f32;
        let low = (bin.floor() as usize).min(FIELD_B - 1);
        let (high, f) = ((low + 1).min(FIELD_B - 1), bin - low as f32);
        std::array::from_fn(|q| marginal[low][q] * (1.0 - f) + marginal[high][q] * f)
    })
}

/// Per luma bin: the occupancy-weighted mean of the parameters over its 96 (x, y)
/// vertices and the weighted std of their luma effect ([`LocalField::band_dispersion`]).
/// Only vertices ABOVE the occupancy floor take part: the others hold a policy zero,
/// not a measurement, and averaging it in reports spatial structure that is really
/// missing evidence.
fn band_summary(
    grid: &[[f32; PARAMS]], occupancy: &[f32], supported: &[bool],
) -> ([[f32; PARAMS]; FIELD_B], [f32; FIELD_B]) {
    let mut marginal = [[0.0f32; PARAMS]; FIELD_B];
    let mut dispersion = [0.0f32; FIELD_B];
    for (bin, means) in marginal.iter_mut().enumerate() {
        let centre = bin as f64 / (FIELD_B - 1) as f64;
        let (mut mass, mut first, mut second) = (0.0f64, 0.0f64, 0.0f64);
        let mut sums = [0.0f64; PARAMS];
        for spatial in 0..FIELD_X * FIELD_Y {
            let vertex = spatial * FIELD_B + bin;
            let (cell, weight) = (&grid[vertex], occupancy[vertex] as f64);
            if !supported[vertex] { continue; }
            mass += weight;
            for (p, slot) in sums.iter_mut().enumerate() { *slot += weight * cell[p] as f64; }
            let effect = LN2 * centre * cell[0] as f64 + centre
                * (0.299 * cell[1] as f64 + 0.587 * cell[2] as f64 + 0.114 * cell[3] as f64);
            first += weight * effect;
            second += weight * effect * effect;
        }
        if mass <= 0.0 { continue; }
        for (slot, sum) in means.iter_mut().zip(sums) { *slot = (sum / mass) as f32; }
        let mean = first / mass;
        dispersion[bin] = (second / mass - mean * mean).max(0.0).sqrt() as f32;
    }
    (marginal, dispersion)
}

/// `GridSystem` with the anchor terms dropped (`total_weight` is `fit_weight`).
struct System<'a> {
    current: &'a [[f32; 3]],
    guide: &'a [f32],
    fit_weight: &'a [f32],
    target_delta: Vec<[f32; 3]>,
    splat: Splat,
    occupancy: Vec<f32>,
}

impl<'a> System<'a> {
    fn new(
        current: &'a [[f32; 3]], target: &[[f32; 3]], guide: &'a [f32], fit_weight: &'a [f32],
        width: usize, height: usize,
    ) -> Self {
        let splat = splat_table(guide, width, height);
        let target_delta = current.iter().zip(target)
            .map(|(c, t)| [t[0] - c[0], t[1] - c[1], t[2] - c[2]]).collect();
        let mut mass = vec![0.0f64; VERTICES];
        for (i, (ids, tri)) in splat.ids.iter().zip(&splat.tri).enumerate() {
            for (&id, &weight) in ids.iter().zip(tri) {
                mass[id as usize] += (weight * fit_weight[i]) as f64;
            }
        }
        let occupancy: Vec<f32> = mass.into_iter().map(|v| v as f32).collect();
        Self { current, guide, fit_weight, target_delta, splat, occupancy }
    }

    fn forward(&self, v: &[f32]) -> Vec<[f32; 3]> {
        self.current.iter().enumerate().map(|(i, c)| {
            delta(c, self.guide[i], &slice_flat(v, &self.splat.ids[i], &self.splat.tri[i]))
        }).collect()
    }

    fn adjoint(&self, residual: &[[f32; 3]]) -> Vec<f32> {
        let mut acc = vec![0.0f64; VERTICES * PARAMS];
        for (i, r) in residual.iter().enumerate() {
            let (c, g) = (&self.current[i], self.guide[i]);
            let ev = LN2 * (c[0] as f64 * r[0] as f64 + c[1] as f64 * r[1] as f64
                + c[2] as f64 * r[2] as f64);
            let slope = (c[0] - g) as f64 * r[0] as f64 + (c[1] - g) as f64 * r[1] as f64
                + (c[2] - g) as f64 * r[2] as f64;
            let per = [ev as f32, c[0] * r[0], c[1] * r[1], c[2] * r[2], slope as f32];
            for (&id, &weight) in self.splat.ids[i].iter().zip(&self.splat.tri[i]) {
                let base = id as usize * PARAMS;
                for (p, v) in per.iter().enumerate() { acc[base + p] += (weight * v) as f64; }
            }
        }
        acc.into_iter().map(|v| v as f32).collect()
    }

    /// Per-pixel weighting of a delta field, the `w[:, None] * x` of the NumPy.
    fn weigh(&self, values: &[[f32; 3]]) -> Vec<[f32; 3]> {
        values.iter().enumerate()
            .map(|(i, v)| { let w = self.fit_weight[i]; [w * v[0], w * v[1], w * v[2]] })
            .collect()
    }

    fn matvec(&self, v: &[f32], tikhonov: f32, smooth: [f32; 3]) -> Vec<f32> {
        let mut out = self.adjoint(&self.weigh(&self.forward(v)));
        for (slot, &x) in out.iter_mut().zip(v) { *slot += tikhonov * x; }
        let mut lap = vec![0.0f32; v.len()];
        laplacian(v, smooth, &mut lap);
        for (slot, l) in out.iter_mut().zip(lap) { *slot += l; }
        out
    }

    /// The bound-constrained least-squares solve: an active-set loop around
    /// conjugate gradients (f64 dot products, a per-pass step budget, a
    /// relative-residual stop at 1e-10, the `denominator <= 1e-20` break).
    ///
    /// The bounds are honoured INSIDE the solve, not clipped onto an
    /// unconstrained answer afterwards — and that difference is the whole
    /// reason this is a loop. A vertex's five parameters act on a pixel
    /// jointly (EV and the three gains are one per-channel multiplier between
    /// them; the slope trades against both), so clipping one of them after the
    /// solve leaves the others balanced for a value it no longer has. Measured
    /// on a night-landscape pair (2026-10-19) whose dark ground the field had
    /// to lift about 1.8 EV: the clipped field left the ground at a linear-light
    /// red/green ratio of 3.69 against the target's 3.16 — redder than the 3.46
    /// it started from — while the constrained solve of the same system reads
    /// 3.59 end to end. Each pass pins the parameters sitting on a bound whose gradient
    /// points outward, re-solves the free ones with the pinned ones held, and
    /// releases any pinned one whose gradient turns inward; it stops when the
    /// pinned set repeats. Returns a solution inside the bounds; occupancy is
    /// the caller's.
    fn solve(
        &self, tikhonov: f32, smooth: [f32; 3], iterations: usize, bounds: impl Fn(usize) -> (f32, f32),
    ) -> (Vec<f32>, SolveInfo) {
        let rhs = self.adjoint(&self.weigh(&self.target_delta));
        let (low, high): (Vec<f32>, Vec<f32>) = (0..rhs.len()).map(|i| bounds(i % PARAMS)).unzip();
        let mut free = vec![true; rhs.len()];
        let (mut vector, mut info) =
            self.conjugate_gradients(&rhs, vec![0.0f32; rhs.len()], &free, tikhonov, smooth, iterations);
        for _ in 0..ACTIVE_SET_PASSES {
            for (i, slot) in vector.iter_mut().enumerate() { *slot = slot.clamp(low[i], high[i]); }
            let product = self.matvec(&vector, tikhonov, smooth);
            let pinned: Vec<bool> = (0..rhs.len())
                .map(|i| {
                    let descent = rhs[i] - product[i];
                    !((vector[i] <= low[i] && descent < 0.0) || (vector[i] >= high[i] && descent > 0.0))
                })
                .collect();
            if pinned == free && (0..rhs.len()).all(|i| !free[i] || (low[i] < vector[i] && vector[i] < high[i])) {
                break;
            }
            free = pinned;
            (vector, info) = self.conjugate_gradients(&rhs, vector, &free, tikhonov, smooth, iterations);
        }
        for (i, slot) in vector.iter_mut().enumerate() { *slot = slot.clamp(low[i], high[i]); }
        (vector, info)
    }

    /// `GridSystem.solve` restricted to the `free` parameters, warm-started
    /// from `start`: the held ones keep their value and take no step. With
    /// every parameter free and a zero start this is the original solve step
    /// for step.
    fn conjugate_gradients(
        &self, rhs: &[f32], start: Vec<f32>, free: &[bool], tikhonov: f32, smooth: [f32; 3],
        iterations: usize,
    ) -> (Vec<f32>, SolveInfo) {
        let mask = |values: &mut [f32]| {
            for (slot, &open) in values.iter_mut().zip(free) { if !open { *slot = 0.0; } }
        };
        let mut vector = start;
        let mut residual: Vec<f32> = if vector.iter().all(|&v| v == 0.0) {
            rhs.to_vec()
        } else {
            let product = self.matvec(&vector, tikhonov, smooth);
            rhs.iter().zip(&product).map(|(b, a)| b - a).collect()
        };
        mask(&mut residual);
        let mut direction = residual.clone();
        let mut rr = dot(&residual, &residual);
        let initial = rr.max(1e-30);
        let mut used = 0usize;
        for step in 1..=iterations {
            used = step;
            let mut product = self.matvec(&direction, tikhonov, smooth);
            mask(&mut product);
            let denominator = dot(&direction, &product);
            if denominator <= 1e-20 { break; }
            let alpha = (rr / denominator) as f32;
            for (i, slot) in vector.iter_mut().enumerate() {
                *slot += alpha * direction[i];
                residual[i] -= alpha * product[i];
            }
            let next = dot(&residual, &residual);
            if next <= initial * 1e-10 { rr = next; break; }
            let beta = (next / rr.max(1e-30)) as f32;
            for (slot, &r) in direction.iter_mut().zip(&residual) { *slot = r + beta * *slot; }
            rr = next;
        }
        (vector, SolveInfo { iterations: used, relative_residual: (rr / initial).sqrt() as f32 })
    }
}

/// The x/y/b Laplacian of `GridSystem.matvec`: per adjacent pair along an axis,
/// `lap[second] += w*delta` and `lap[first] -= w*delta` — the gradient of
/// `sum (v[i+1] - v[i])^2`.  Axis order/weights follow the `(SY, SX, SB, PARAMS)`
/// reshape.
fn laplacian(v: &[f32], smooth: [f32; 3], lap: &mut [f32]) {
    let strides = [FIELD_X * FIELD_B * PARAMS, FIELD_B * PARAMS, PARAMS];
    let counts = [FIELD_Y, FIELD_X, FIELD_B];
    for (axis, &weight) in [smooth[1], smooth[0], smooth[2]].iter().enumerate() {
        if weight <= 0.0 { continue; }
        let (stride, count) = (strides[axis], counts[axis]);
        for i in 0..v.len() {
            if (i / stride) % count + 1 >= count { continue; }
            let step = weight * (v[i + stride] - v[i]);
            lap[i + stride] += step;
            lap[i] -= step;
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f64 { a.iter().zip(b).map(|(&x, &y)| x as f64 * y as f64).sum() }

#[cfg(test)]
pub(crate) mod tests;
