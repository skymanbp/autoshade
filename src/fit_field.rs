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
///
/// v1.18.0: the floor is the ANALYZER's exclusion — the band marginals, the
/// band dispersion and the saturation count read measured vertices only — and
/// it is no longer written into the grid. Since the field became a rendered
/// control the grid is what the engine applies, and to the renderer a zero
/// written into an unmeasured cell is not "no measurement": it is a measured
/// "no change" that the trilinear read interpolates against the measured
/// cells around it, a plateau edge on the cell boundary that the solve never
/// chose. On a dusk-landscape pair at 0.85 the global develop clipped 88-93 %
/// of two horizon cells' pixels (a pair clipped on one side did not testify
/// then, so those cells carried no mass), their vertices were zeroed while the cells
/// around them carried EV -0.3 and a blue gain of -0.35 in the bright bins,
/// and the horizon rendered a rectangular over-bright block (that fifth of the
/// frame 12.72 dE from its target). An unmeasured vertex now ships the value
/// the solve gave it: the smoother's continuation of its measured neighbours,
/// pulled toward zero by the Tikhonov term away from them — the surface the
/// regulariser's smoothness describes, inside the same bounds.
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
/// The estimation's refusal line: a pair whose ranges carry no evidence to
/// estimate a control from gets no field. The field solve IS an estimation, so
/// this stays here; since DEF-4 the objective's own sentinel reads presence
/// (no pixel present), not identifiability.
const IDENTIFIABILITY_MIN: f32 = 1e-5;
const LN2: f64 = std::f64::consts::LN_2;

/// R34 §D4. The things the SHIPPED colour field solves differently from the
/// ANALYSIS instrument — two since R34, the refinement since v1.18.0 — and
/// nothing else.
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
    /// Solve a second time with the regulariser centred on the first answer
    /// (v1.18.0, [`System::solve`]). FALSE is the analysis solve, byte for
    /// byte: the ceiling the sequencer stops on stays the single smooth it
    /// has always been, and the field that SHIPS — both passes of
    /// `fit_zoned::field::attach_colour_field` — is the refined one.
    pub refine: bool,
}

impl Default for FieldSolveOpts {
    fn default() -> Self {
        Self { gain: BOUNDS_HIGH[1], local_support: true, refine: false }
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
    /// bound-limited; an unmeasured vertex carries the solve's own value
    /// ([`OCCUPANCY_MIN`]: the floor excludes it from the statistics, not from
    /// the render).
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
    /// x [`testifies`]).  Shape verdicts weight the remainder by it, so a pixel whose
    /// vertices are under the occupancy floor (unmeasured: they carry the solve's
    /// smooth continuation, not a measurement) cannot masquerade as spatial
    /// structure.
    pub weight: Vec<f32>,
    /// Occupancy-supported vertices whose colour change or slope the bounds
    /// limited ([`limit_colour_change`]).
    pub saturated: usize,
    pub solve: SolveInfo,
    pub width: u32,
    pub height: u32,
    /// v1.18.0: the frame's sky / land partition the field was solved on, and
    /// the vertex owners counted on it — `None` for the plain read (every
    /// analysis solve). See [`Self::solve_zoned`].
    pub zones: Option<FieldZones>,
    /// v1.18.0: where the frame's pixels read the grid — the pixel's own
    /// place for every analysis solve, the lens-corrected plate's map for a
    /// shipped field solved on one ([`Self::solve_framed`]).
    pub frame: crate::render::FieldFrame,
}

/// The zone-owned read a SHIPPED field is solved for ([`LocalField::solve_zoned`]).
#[derive(Clone, Debug)]
pub(crate) struct FieldZones {
    /// Per pixel, the sky zone geometry's coverage (`render::preview_field_sky_coverage`).
    pub sky: Vec<f32>,
    /// Per vertex, [`crate::recipe::ColourField::owners`].
    pub owners: Vec<u8>,
}

/// The owner of every vertex: the zone holding the strict majority of its
/// population, the plain trilinear splat mass of each zone's pixels (a pixel
/// of sky coverage `c` puts `c` of its splat on the sky's mass and `1 − c` on
/// the land's; the read is this count transposed,
/// [`crate::render::field_owned_weights`]); a tie, an empty vertex included,
/// is owned by neither.
fn vertex_owners(splat: &Splat, sky: &[f32]) -> Vec<u8> {
    use crate::recipe::ColourField as F;
    let (mut sky_mass, mut land_mass) = (vec![0.0f64; VERTICES], vec![0.0f64; VERTICES]);
    for ((ids, tri), &c) in splat.ids.iter().zip(&splat.tri).zip(sky) {
        let c = c.clamp(0.0, 1.0) as f64;
        for (&id, &weight) in ids.iter().zip(tri) {
            sky_mass[id as usize] += weight as f64 * c;
            land_mass[id as usize] += weight as f64 * (1.0 - c);
        }
    }
    sky_mass.iter().zip(&land_mass).map(|(s, l)| {
        if s > l { F::OWNER_SKY } else if l > s { F::OWNER_LAND } else { F::OWNER_NONE }
    }).collect()
}

/// The plain `splat` made the zone-owned one: the owners counted on the sky
/// coverage `sky` ([`vertex_owners`]), then every pixel's weights read as
/// that count transposed, on the same coverage
/// ([`crate::render::field_owned_weights`]).
fn own_splat(splat: &mut Splat, sky: &[f32]) -> FieldZones {
    let owners = vertex_owners(splat, sky);
    for ((ids, tri), &c) in splat.ids.iter().zip(splat.tri.iter_mut()).zip(sky) {
        crate::render::field_owned_weights(tri, &ids.map(|id| owners[id as usize]), c);
    }
    FieldZones { sky: sky.to_vec(), owners }
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
        Self::solve_zoned(current, target, width, height, evidence, tikhonov, smooth, iterations, opts, None)
    }

    /// [`Self::solve_with`] for the read the SHIPPED field performs (v1.18.0,
    /// "a zone's correction is read on its own pixels"): with `sky`, the
    /// frame's sky zone geometry coverage per pixel, every vertex is owned by
    /// the zone holding the majority of its plain-splat population
    /// ([`vertex_owners`]) and every pixel's splat is that count transposed
    /// on the same coverage — each corner carries the pixel's share in that
    /// corner's zone, renormalised once ([`crate::render::field_owned_weights`])
    /// — in the forward map and the adjoint alike, so the field is solved for
    /// the read that ships. `None` (or a `sky` of the wrong length) is
    /// [`Self::solve_with`] byte for byte.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn solve_zoned(
        current: &[[f32; 3]], target: &[[f32; 3]], width: u32, height: u32,
        evidence: &fit::EvidenceModel, tikhonov: f32, smooth: [f32; 3], iterations: usize,
        opts: FieldSolveOpts, sky: Option<&[f32]>,
    ) -> Option<LocalField> {
        Self::solve_framed(
            current, target, width, height, evidence, tikhonov, smooth, iterations, opts, sky,
            &crate::render::FieldFrame::default(),
        )
    }

    /// [`Self::solve_zoned`] on a frame that states where its pixels read the
    /// grid (v1.18.0, J133; [`crate::render::FieldFrame`]). A stored field's
    /// grid spans the original frame and every develop of the recipe reads it
    /// there, so the field that ships is solved there: the splat puts each
    /// pixel on the vertices the engine will read it on — its own place on
    /// pixels nothing moved, the original-frame point it shows on a
    /// lens-corrected plate — and [`Self::render`] reads the same way. The
    /// default frame is [`Self::solve_zoned`] byte for byte.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn solve_framed(
        current: &[[f32; 3]], target: &[[f32; 3]], width: u32, height: u32,
        evidence: &fit::EvidenceModel, tikhonov: f32, smooth: [f32; 3], iterations: usize,
        opts: FieldSolveOpts, sky: Option<&[f32]>, frame: &crate::render::FieldFrame,
    ) -> Option<LocalField> {
        let (w, h) = (width as usize, height as usize);
        let n = w.checked_mul(h)?;
        if n == 0 || current.len() != n || target.len() != n { return None; }
        if evidence.source_weights.len() != n || evidence.identifiability <= IDENTIFIABILITY_MIN {
            return None;
        }
        // Fit weight = frozen evidence x local structural support x testifies.  The
        // evidence model is the fit's own; this module never builds a second one.
        let support = opts
            .local_support
            .then(|| local_support_present(current, target, width, height, evidence.presence.as_deref()));
        let mut fit_weight = vec![0.0f32; n];
        let mut mass = 0.0f64;
        for (i, weight) in fit_weight.iter_mut().enumerate() {
            if testifies(&current[i], &target[i]) {
                *weight = evidence.source_weights[i].max(0.0)
                    * support.as_ref().map_or(1.0, |s| s[i]);
            }
            mass += *weight as f64;
        }
        if !mass.is_finite() || mass <= 0.0 { return None; }
        let guide = smooth_3tap_luma(current, w, h);
        let mut splat = splat_table(&guide, w, h, frame);
        let zones = sky.filter(|s| s.len() == n).map(|sky| own_splat(&mut splat, sky));
        let system = System::with_splat(
            current, target, &guide, &fit_weight, splat, zones.as_ref().map(|z| z.owners.clone()),
        );
        let (raw, solve, limited) = system.solve(tikhonov, smooth, iterations, opts.refine, |p| opts.bounds(p));
        if raw.iter().any(|value| !value.is_finite()) { return None; }

        let mut flat = vec![0.0f32; VERTICES * PARAMS];
        let mut grid = vec![[0.0f32; PARAMS]; VERTICES];
        let mut saturated = 0usize;
        let measured = supported(&system.occupancy);
        for (vertex, cell) in grid.iter_mut().enumerate() {
            for (p, value) in cell.iter_mut().enumerate() {
                *value = raw[vertex * PARAMS + p];
                flat[vertex * PARAMS + p] = *value;
            }
            saturated += usize::from(measured[vertex] && limited[vertex]);
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
            weight: fit_weight, saturated, solve, width, height, zones, frame: frame.clone(),
        })
    }

    /// Field render = `clamp(current + delta, 0, 1)` (`GridSystem.render`).
    ///
    /// Since R33 §G this is a CALL into the engine's own renderer rather than
    /// a second copy of it: the analyzer's measured field and the field the
    /// user's export carries are the same function of the same pixels, and
    /// `the_engine_renders_the_analyzers_field_bit_for_bit` holds them to it —
    /// in the frame the field was solved on ([`Self::frame`]).
    pub(crate) fn render(&self, current: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let mut out = current.to_vec();
        crate::render::apply_colour_field(
            &mut out,
            self.width as usize,
            self.height as usize,
            Some(&self.as_recipe_field()),
            &self.frame,
            self.zones.as_ref().map(|z| z.sky.as_slice()),
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
            owners: self.zones.as_ref().map(|z| z.owners.clone()).unwrap_or_default(),
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

/// One frame's pixel with no channel at the white clip (`254/255`) — read
/// on the TARGET alone by [`crate::fit_cells`] for its cell means (the field
/// itself reads [`testifying`], channel by channel; a mean of a colour needs
/// all three of a pixel's channels). A channel at the black clip is a value
/// (the target shows it black) and keeps its pixel in; a channel at the
/// white clip keeps its pixel out of a MEAN OF A COLOUR — the hue the target
/// shows there is the clip's — while the field's testimony, which asks
/// channel by channel, counts it ([`testifying`], J153). Until v1.18.0 this
/// excluded the black end too, and the cells judging pass B of the field
/// (`fit_zoned::field::admit_cells`) could not see the very pixels the
/// field's data term had just been ruled to count (RULING-1180-int-r19).
/// The white end they still do not see: what the field does to pixels whose
/// target is at the clip is read by the per-band census of a finished
/// render, not by the cells (the seaside street's white clouds, J153).
pub(crate) fn unclipped(p: &[f32; 3]) -> bool {
    p[0].max(p[1]).max(p[2]) < 254.0 / 255.0
}

/// Which CHANNELS of a pair of pixels — the frame being moved and its
/// target — testify: none unless the LUMA of each frame is above the black
/// code `1/255`, and then EVERY channel, with one exception: of a frame
/// pixel at the white clip in every channel
/// ([`crate::render::blown_white`], the pixel the engine keeps neutral under
/// the field) only the channels its target holds at the clip too (254/255).
/// A pixel white in every channel on both sides testifies (its difference is
/// 0 and known): the luma gate reads the black end only, because at the
/// white end the per-channel rule already says everything there is to say,
/// and a luma gate there kept the one pixel the v1.17.1 rule had been
/// written to admit.
///
/// THE VALUE THE FIELD MEETS (v1.18.0, J153). The field is a move of the
/// RENDERED value: the engine adds it to the pixel as the stages before it
/// left it, inside the range (`render::develop`), and what it is solved
/// toward is a picture. To the field a channel at the white clip is
/// therefore a value like any other, the clip code, on either side. At the
/// clip in BOTH frames the difference is 0 and the target keeps it there
/// (v1.17.1, below). At the clip in the TARGET only, the difference is the
/// least the frame has to rise to show what its target shows, and the
/// render's own clamp stops it there. At the clip in the FRAME only, it is
/// exactly the move that takes the rendered value to its target. What is
/// unknown above the clip is the scene's light, and the field does not read
/// it. The exception is the engine's own: a pixel that reaches the field
/// blown in every channel leaves it neutral, at the brightest value the
/// field gave it (`render::wb::keep_blown_white`), so what the field would
/// do to one of its channels is never rendered, and a channel of it whose
/// target is under the clip would ask for a move that pixel is not given.
///
/// Until J153 a channel across the clip — at it on one side only — had no
/// say ("its true value on that side is unknown above the clip"). On the
/// seaside street (p39, 0.85) the white clouds' target holds its BLUE at
/// the clip: their brightest 4 % read (248.9, 250.3, 254.8) in the target and
/// (242.0, 239.6, 238.7) before the field. Red and green asked up, nothing asked the
/// blue, and the clouds rendered (250.8, 252.8, 243.6) — cream where the target is
/// blue-white, 7.10 dE76 from it against 5.46 with no
/// field at all and 2.75 in the round before, when the pixel-wide
/// rule kept those pixels out altogether (rounds V14 / V15). Under the NumPy
/// model of the solve, the two brightest luma bands, channel across the clip
/// silent -> asked: that pair's frame 1.77 / 5.58 -> 1.17 / 1.84 (the model on
/// the render without the field and the target, every pixel weighed alike);
/// the sunset street's clouds over the horizon 4.84 / 6.70 -> 3.90 / 6.13 and
/// the lantern street's frame 0.90 / 0.48 -> 0.75 / 0.49 (on those solves'
/// dumped planes; rel1180 fieldproto, 2026-11-13). No number is added: the
/// clip codes are the ones the rule already read, and the blown pixel is the
/// engine's ([`crate::render::WHITE_CLIP`]).
///
/// A CHANNEL, NOT THE PIXEL (v1.18.0, J150). The rule was always stated per
/// channel and, until this version, applied to the pixel: one channel across
/// the clip took the pixel's other two — measured in both frames — out with
/// it. On the sunset-street pair (p40, 0.85) the target's red is at the clip
/// over the brightest clouds and the frame's is a few codes under it. The
/// pixels refused for that alone are 84.7 % / 89.4 % of the two brightest
/// luma bands of the clouds over the horizon, and over the frame they read
/// (0.987, 0.636, 0.556) on average against a target at
/// (1.000, 0.620, 0.467) — so nothing asked their BLUE down, the one
/// difference that makes them pink-white where the target is
/// yellow-orange. With every pixel given its say by the evidence
/// (`fit::EvidenceModel::per_place_source_weights`) those two bands still
/// read 13.19 / 20.16 dE76 under the pixel-wide rule and
/// 4.84 / 6.70 when only the channel across the clip — red, in every
/// one of those pixels — is left out (the solve's dumped inputs under the
/// NumPy model, rel1180 fieldproto, 2026-11-13). A
/// channel that does not testify weighs nothing in the data term
/// ([`System::weigh`] and the diagonal [`System::with_splat`] builds); the
/// pixel counts as one pixel of occupancy while any of its channels does
/// ([`testifies`]). No number is added: the clip codes are the ones the
/// rule already read.
///
/// A channel at its BLACK clip (at or under 1/255) is not a missing
/// measurement (v1.18.0). The data term reads the per-channel difference
/// `target − current` ([`System::with_splat`]); a channel crushed to 0 is a
/// channel whose true value is at or under the black code, so its difference
/// is finite and points the right way (darker), and the pixel's luma is still
/// carried by its other channels: it is a DARK pixel. Dropping it removes
/// exactly the darkest pixels of a dark region, so what testifies there is
/// selected on the target's own value: a night sky whose target red is 0 on
/// its top rows had no witness at all on those vertex rows (occupancy 0, the
/// vertices shipped the smoother's continuation of the rows below), and
/// under them only the target's brighter pixels testified, biased toward
/// "target brighter" — the top of the sky was lifted where its target asks
/// for it darker. (Until J153 the white clip was held not to be symmetric
/// to it — above 254/255 the scene's value is unbounded; to a move of the
/// rendered value it is, see above.) A
/// pixel whose LUMA is at the black clip has no channel left to carry it
/// and stays out. The only numbers are the 8-bit clip codes the channel
/// test already used.
///
/// WHY (v1.17.1). Until then a pixel testified only when it was inside the
/// clip in both frames, while the colour field it was excluded from was
/// still APPLIED to it. On the sunset-rooftops pair (p36, 0.85) 32 % of the
/// upper clouds' pixels have red at the clip in both frames; their vertices
/// were solved from the unclipped pixels around them, and the field pulled
/// their red 0.999 → 0.972 against a target at 0.998 (blue 0.521 → 0.504
/// where the target asks 0.574): the clouds' a* fell 16.9 → 15.7 against
/// the target's 17.1, and the cells, which exclude the target's clip, could
/// not see it. Testifying, those pixels hold their own vertices: clouds a* 17.0 /
/// b* 28.1 against 17.1 / 28.1, the frame's 480-px distance 2.78 → 1.95
/// (the seaside street 2.28 → 1.92), every other pair of the leg unchanged.
pub(crate) fn testifying(current: &[f32; 3], target: &[f32; 3]) -> [bool; 3] {
    const LOW: f32 = 1.0 / 255.0;
    const HIGH: f32 = 254.0 / 255.0;
    let measured = |p: &[f32; 3]| fit::luma601(p) > LOW;
    if !(measured(current) && measured(target)) {
        return [false; 3];
    }
    let kept_neutral = crate::render::blown_white(current);
    std::array::from_fn(|c| !kept_neutral || target[c] >= HIGH)
}

/// Whether the pair has any say: at least one channel [`testifying`] admits.
/// The pixel's evidence weight and its occupancy read this; what each of its
/// channels weighs in the data term is [`testifying`]'s.
pub(crate) fn testifies(current: &[f32; 3], target: &[f32; 3]) -> bool {
    testifying(current, target).contains(&true)
}

/// The field's guide, owned by the engine since R33 §G: see
/// [`crate::render::field_guide_luma`]. Kept under its analyzer name so the
/// solve reads the way its Python prototype did.
pub(crate) use crate::render::field_guide_luma as smooth_3tap_luma;

struct Splat { ids: Vec<[u32; 8]>, tri: Vec<[f32; 8]> }

/// `grid_experiment.splat_table`, slot for slot: x-major then y then b, high index
/// `min(low + 1, limit - 1)` so the last column/row/bin folds onto itself. Each
/// pixel's place on the two spatial axes is the engine's own read of `frame`
/// ([`crate::render::FieldFrame::axes`]) — one implementation, so the vertices a
/// pixel is solved on are the vertices it is rendered from.
fn splat_table(guide: &[f32], width: usize, height: usize, frame: &crate::render::FieldFrame) -> Splat {
    let axes = frame.axes(width, height, FIELD_X, FIELD_Y);
    let limits = [FIELD_X, FIELD_Y, FIELD_B];
    let mut ids = Vec::with_capacity(guide.len());
    let mut tri = Vec::with_capacity(guide.len());
    for (i, &value) in guide.iter().enumerate() {
        let (gx, gy) = axes.at(i);
        let coords = [gx, gy, value.clamp(0.0, 1.0) * (FIELD_B - 1) as f32];
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
///
/// The per-pixel parameters arrive in f64, as [`slice_flat`] accumulated
/// them, and are used in f64: exactly the arithmetic
/// `render::apply_colour_field` performs on the same eight corners. Until
/// v1.17.1 they were rounded to f32 between the read and the delta, so the
/// analyzer's render and the engine's differed in the last place (1 ulp,
/// measured) while the docs here claimed bit-for-bit.
fn delta(c: &[f32; 3], guide: f32, p: &[f64; PARAMS]) -> [f32; 3] {
    std::array::from_fn(|ch| {
        (LN2 * c[ch] as f64 * p[0] + c[ch] as f64 * p[1 + ch]
            + (c[ch] - guide) as f64 * p[4]) as f32
    })
}

/// `GridSystem.slice_params` for one pixel: the trilinear read of the grid,
/// kept in the f64 it is accumulated in ([`delta`]).
fn slice_flat(v: &[f32], ids: &[u32; 8], tri: &[f32; 8]) -> [f64; PARAMS] {
    let mut acc = [0.0f64; PARAMS];
    for (&id, &weight) in ids.iter().zip(tri) {
        let base = id as usize * PARAMS;
        for (p, slot) in acc.iter_mut().enumerate() { *slot += weight as f64 * v[base + p] as f64; }
    }
    acc
}

/// One per-pixel parameter vector applied as the field's delta, display-clamped.
fn apply_params(
    current: &[[f32; 3]], guide: &[f32], mut params: impl FnMut(usize) -> [f64; PARAMS],
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
        std::array::from_fn(|q| (marginal[low][q] * (1.0 - f) + marginal[high][q] * f) as f64)
    })
}

/// Per luma bin: the occupancy-weighted mean of the parameters over its 96 (x, y)
/// vertices and the weighted std of their luma effect ([`LocalField::band_dispersion`]).
/// Only vertices ABOVE the occupancy floor take part: the others carry the solve's
/// smooth continuation of their measured neighbours, not a measurement, and averaging
/// it in reports spatial structure that is really missing evidence.
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
    /// Which channels of each pixel testify ([`testifying`]): the data term
    /// weighs a channel by its pixel's `fit_weight` when it does and by
    /// nothing when it does not.
    says: Vec<[bool; 3]>,
    target_delta: Vec<[f32; 3]>,
    splat: Splat,
    occupancy: Vec<f32>,
    /// The diagonal of the data term `AᵀWA`, per parameter: the CG's Jacobi
    /// preconditioner together with the ridge and the smoother's diagonal
    /// ([`System::preconditioner`]).
    data_diagonal: Vec<f64>,
    /// The zone owner of every vertex for a zone-owned solve, `None` plain:
    /// the smoother couples no two vertices owned by different zones
    /// ([`smoother_couples`]).
    owners: Option<Vec<u8>>,
}

/// Whether the smoother couples vertices `a` and `b`. "A zone's correction is
/// read on its own pixels" (v1.18.0): a vertex one zone owns is read only by
/// that zone's pixels, so a smoothness edge between a sky-owned and a
/// land-owned vertex would hand the land's vertex the sky's correction — read
/// on the land's pixels through the regulariser, not the data (measured on the
/// two-zone toy: the land-owned row under the horizon at +0.0133 EV with the
/// edge, +0.0000 without it, the land foot +1.54 codes against 0). Every other
/// edge — within a zone, or touching a vertex neither zone owns — is kept, so a
/// plain solve (`owners` `None`) smooths exactly as before.
fn smoother_couples(owners: Option<&[u8]>, a: usize, b: usize) -> bool {
    use crate::recipe::ColourField as F;
    owners.is_none_or(|o| o[a] == o[b] || o[a] == F::OWNER_NONE || o[b] == F::OWNER_NONE)
}

impl<'a> System<'a> {
    #[cfg(test)]
    fn new(
        current: &'a [[f32; 3]], target: &[[f32; 3]], guide: &'a [f32], fit_weight: &'a [f32],
        width: usize, height: usize,
    ) -> Self {
        let own_place = crate::render::FieldFrame::default();
        Self::with_splat(current, target, guide, fit_weight, splat_table(guide, width, height, &own_place), None)
    }

    /// [`Self::new`] on a splat the caller built: the zone-owned one
    /// ([`LocalField::solve_zoned`]) is the plain table with each pixel's
    /// weights owned and renormalised, and every term below — the forward
    /// map, its adjoint, the occupancy and the preconditioner's diagonal —
    /// reads it; with its vertex `owners` the smoother is cut between zones
    /// ([`smoother_couples`]).
    fn with_splat(
        current: &'a [[f32; 3]], target: &[[f32; 3]], guide: &'a [f32], fit_weight: &'a [f32],
        splat: Splat, owners: Option<Vec<u8>>,
    ) -> Self {
        let target_delta = current.iter().zip(target)
            .map(|(c, t)| [t[0] - c[0], t[1] - c[1], t[2] - c[2]]).collect();
        let says: Vec<[bool; 3]> = current.iter().zip(target).map(|(c, t)| testifying(c, t)).collect();
        let mut mass = vec![0.0f64; VERTICES];
        for (i, (ids, tri)) in splat.ids.iter().zip(&splat.tri).enumerate() {
            for (&id, &weight) in ids.iter().zip(tri) {
                mass[id as usize] += (weight * fit_weight[i]) as f64;
            }
        }
        let occupancy: Vec<f32> = mass.into_iter().map(|v| v as f32).collect();
        // diag(AᵀWA): Σ_i w_i·(pixel i's trilinear weight on the vertex)²·(∂δ/∂p)²,
        // with ∂δ_c/∂EV = ln2·c_c, ∂δ_c/∂gain_c = c_c, ∂δ_c/∂slope = c_c − guide,
        // summed over the channels — the operator `matvec` applies, read on its
        // diagonal. A pixel on the last column, row or bin splats twice onto one
        // vertex (`splat_table` folds the high index), so its weights on that
        // vertex are summed before they are squared.
        let mut data_diagonal = vec![0.0f64; VERTICES * PARAMS];
        for (i, (ids, tri)) in splat.ids.iter().zip(&splat.tri).enumerate() {
            let (c, g, w) = (&current[i], guide[i] as f64, fit_weight[i] as f64);
            if w <= 0.0 { continue; }
            let cc = [c[0] as f64, c[1] as f64, c[2] as f64];
            // 1 for a channel that testifies, 0 for one that does not: where
            // all three do, every product below is the one it was, bit for bit.
            let on = says[i].map(|s| f64::from(u8::from(s)));
            let per = [
                LN2 * LN2 * (on[0] * cc[0] * cc[0] + on[1] * cc[1] * cc[1] + on[2] * cc[2] * cc[2]),
                on[0] * cc[0] * cc[0],
                on[1] * cc[1] * cc[1],
                on[2] * cc[2] * cc[2],
                cc.iter().zip(on).map(|(&v, on)| on * (v - g) * (v - g)).sum::<f64>(),
            ];
            let mut merged: [(u32, f64); 8] = [(u32::MAX, 0.0); 8];
            for (&id, &weight) in ids.iter().zip(tri) {
                if let Some(slot) = merged.iter_mut().find(|(at, _)| *at == id || *at == u32::MAX) {
                    slot.0 = id;
                    slot.1 += weight as f64;
                }
            }
            for &(id, weight) in merged.iter().filter(|(id, _)| *id != u32::MAX) {
                let base = id as usize * PARAMS;
                for (p, d) in per.iter().enumerate() { data_diagonal[base + p] += w * weight * weight * d; }
            }
        }
        Self { current, guide, fit_weight, says, target_delta, splat, occupancy, data_diagonal, owners }
    }

    /// The inverse diagonal of the whole system `AᵀWA + λI + L`: the data
    /// term's own diagonal ([`Self::data_diagonal`]), the ridge, and the
    /// smoother's (one `weight` per neighbour along each axis — the diagonal
    /// of [`laplacian`]'s operator). A Jacobi preconditioner states no number
    /// of its own: it rescales each parameter by the curvature the solve
    /// already gives it, so CG reaches the SAME minimiser in fewer steps.
    /// Why the solve needs it: the data curvature grows with a vertex's
    /// occupancy and with c², so a populated vertex's EV and a thinly
    /// evidenced vertex's gain sit far apart on the diagonal, and the plain
    /// CG paid for that spread in steps: p39's three solves stopped on the
    /// 90-step budget at relative residuals 3e-4 to 8e-4; preconditioned they
    /// stop on the 1e-5 tolerance in 70 to 85 steps.
    fn preconditioner(&self, tikhonov: f32, smooth: [f32; 3]) -> Vec<f32> {
        let mut diagonal: Vec<f64> = self.data_diagonal.iter().map(|d| d + tikhonov as f64).collect();
        let strides = [FIELD_X * FIELD_B * PARAMS, FIELD_B * PARAMS, PARAMS];
        let counts = [FIELD_Y, FIELD_X, FIELD_B];
        for (axis, &weight) in [smooth[1], smooth[0], smooth[2]].iter().enumerate() {
            if weight <= 0.0 { continue; }
            let (stride, count) = (strides[axis], counts[axis]);
            for i in 0..diagonal.len() {
                if (i / stride) % count + 1 >= count { continue; }
                if !smoother_couples(self.owners.as_deref(), i / PARAMS, (i + stride) / PARAMS) { continue; }
                diagonal[i] += weight as f64;
                diagonal[i + stride] += weight as f64;
            }
        }
        diagonal.into_iter().map(|d| if d > 0.0 { (1.0 / d) as f32 } else { 1.0 }).collect()
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

    /// The weighting of a delta field: a pixel's `fit_weight` on each of its
    /// channels that testifies, nothing on one that does not (`says`) — the
    /// `w3 * x` of the NumPy, which is its old `w[:, None] * x` wherever all
    /// three channels testify.
    fn weigh(&self, values: &[[f32; 3]]) -> Vec<[f32; 3]> {
        values.iter().enumerate()
            .map(|(i, v)| {
                let (w, says) = (self.fit_weight[i], self.says[i]);
                let of = |c: usize| if says[c] { w } else { 0.0 };
                [of(0) * v[0], of(1) * v[1], of(2) * v[2]]
            })
            .collect()
    }

    fn matvec(&self, v: &[f32], tikhonov: f32, smooth: [f32; 3]) -> Vec<f32> {
        let mut out = self.adjoint(&self.weigh(&self.forward(v)));
        for (slot, &x) in out.iter_mut().zip(v) { *slot += tikhonov * x; }
        let mut lap = vec![0.0f32; v.len()];
        laplacian(v, smooth, self.owners.as_deref(), &mut lap);
        for (slot, l) in out.iter_mut().zip(lap) { *slot += l; }
        out
    }

    /// The bounded least-squares solve: Jacobi-preconditioned conjugate
    /// gradients ([`Self::preconditioner`]; f64 dot products,
    /// a per-pass step budget, a relative-residual stop at 1e-10, the
    /// `denominator <= 1e-20` break), then every vertex's colour change limited
    /// to its bounds ALONG ITS OWN DIRECTION ([`limit_colour_change`]) and
    /// held there, then an active-set loop for the slopes alone. Returns a
    /// solution inside the bounds, and per vertex whether the bounds limited
    /// it; occupancy is the caller's.
    ///
    /// Why the colour is limited as one change. A vertex's EV and three gains
    /// act on a pixel as one per-channel multiplier between them, so clipping
    /// one of them leaves the others balanced for a value it no longer has:
    /// on a night-landscape pair (2026-10-19) whose dark ground the field had
    /// to lift about 1.8 EV, the clipped field left the ground at a
    /// linear-light red/green ratio of 3.69 against the target's 3.16 —
    /// redder than the 3.46 it started from. The answer until v1.17.0 was a
    /// per-parameter box honoured inside the solve (3.59 there), which keeps
    /// each channel's own best value up to its bound — so a demand larger in
    /// one channel than the bound allows came out with that channel short and
    /// the others whole, a hue the target never asked for. On the same pair's
    /// horizon (p42, 0.85) the target lifted green 1.65× and blue 3.6×; blue
    /// stopped at 2.6× with green at 1.64×, and the horizon rendered green
    /// (Δa* −5.6). Scaled along its direction, a vertex's change keeps its
    /// hue and gives up only its size — every pixel it moves moves along the
    /// line toward its target, never past it and never off it.
    ///
    /// Why the limited changes are held and not re-solved. Re-solving the
    /// free vertices around a limited one lets them make up its shortfall,
    /// and the cheapest make-up under the regulariser is the common part
    /// (Tikhonov prices a change through EV below one through three gains):
    /// on the pure-hue fixture a neighbour re-solved that way turned 6° toward
    /// grey (cos 0.993 to its own unbounded change). Held, it keeps its own.
    /// The slope is a contrast, not a colour, and keeps the per-parameter
    /// active set: pinned on its bound while its gradient points outward,
    /// released when it turns inward, the colour held throughout.
    ///
    /// Why the shipped solve is refined (`refine`, v1.18.0). The ridge and
    /// the smoother are constants on multiplicative parameters, and the data
    /// term is in display units and evidence-weighted: a vertex's curvature
    /// is `Σ w·tri²·c²`. Wherever that is small against the constants — dark
    /// pixels, weakly evidenced ones, a vertex few pixels reach — the
    /// regulariser has a say in the answer, and one regularised solve leaves
    /// a FRACTION of whatever it is handed there, so what the field leaves
    /// follows what the stages before it left: on the night pair from its master
    /// (0.85) the zones handed the horizon rows 12-15 L* and the field left
    /// 0.5-1.9 of them, the sky's row ripple 0.84 on the solve's own frame
    /// (the solve's dumped inputs, rel1180 fieldproto, 2026-11-10). The
    /// refinement solves the SAME system once more with its regulariser
    /// centred on the first answer — `(AᵀWA + R)·v = AᵀWδ + R·v₁`, the ridge
    /// pulling toward `v₁` and the smoother penalising differences of
    /// `v − v₁` — which is the first answer plus the same smooth of its own
    /// residual (a linear smoother's "twicing"). Where the data determine a
    /// vertex the first answer already stood and nothing moves; where the
    /// regulariser took a share, the first-order part of that share comes
    /// back (the same rows 0.1-1.1 L*, ripple 0.61; the sky 3.87 -> 3.70 and
    /// the ground 6.81 -> 6.59 dE on that frame). ONE refinement, not a
    /// loop: a third solve reads the same (ripple 0.64) and each further one
    /// gives the weakly measured vertices back to the data — the regulariser
    /// removed by repetition. The bounds are applied once, to the refined
    /// answer, and the slopes' active set solves the centred system.
    fn solve(
        &self, tikhonov: f32, smooth: [f32; 3], iterations: usize, refine: bool,
        bounds: impl Fn(usize) -> (f32, f32),
    ) -> (Vec<f32>, SolveInfo, Vec<bool>) {
        let mut rhs = self.adjoint(&self.weigh(&self.target_delta));
        let limits = ChangeLimits::of(&bounds);
        let all = vec![true; rhs.len()];
        let (mut vector, mut info) =
            self.conjugate_gradients(&rhs, vec![0.0f32; rhs.len()], &all, tikhonov, smooth, iterations);
        if refine {
            // R·v₁ = λ·v₁ + L·v₁, added to the right-hand side every later
            // step solves; the CG starts from v₁, where its residual is the
            // data's own gradient `AᵀW(δ − A·v₁)`.
            let mut prior = vec![0.0f32; vector.len()];
            laplacian(&vector, smooth, self.owners.as_deref(), &mut prior);
            for ((slot, &v), &l) in rhs.iter_mut().zip(&vector).zip(&prior) {
                *slot += tikhonov * v + l;
            }
            (vector, info) = self.conjugate_gradients(&rhs, vector, &all, tikhonov, smooth, iterations);
        }
        let first = limit_all(&mut vector, &limits);
        let mut limited: Vec<bool> = first.iter().map(|&(scaled, slope)| scaled || slope).collect();
        // The colour is held from here; only the slopes move.
        let mut free: Vec<bool> = (0..rhs.len()).map(|i| i % PARAMS == 4).collect();
        let mut slopes = first;
        for _ in 0..ACTIVE_SET_PASSES {
            if !slopes.iter().any(|&(_, slope)| slope) {
                break;
            }
            let product = self.matvec(&vector, tikhonov, smooth);
            let mut pinned: Vec<bool> = (0..rhs.len()).map(|i| i % PARAMS == 4).collect();
            for (vertex, &(_, slope)) in slopes.iter().enumerate() {
                let at = vertex * PARAMS + 4;
                let (v, descent) = (vector[at], rhs[at] - product[at]);
                if slope && ((v < 0.0 && descent < 0.0) || (v > 0.0 && descent > 0.0)) {
                    pinned[at] = false;
                }
            }
            let inside = slopes.iter().enumerate().all(|(vertex, &(_, slope))| !(slope && free[vertex * PARAMS + 4]));
            if pinned == free && inside {
                break;
            }
            free = pinned;
            (vector, info) = self.conjugate_gradients(&rhs, vector, &free, tikhonov, smooth, iterations);
            slopes = limit_all(&mut vector, &limits);
            for (flag, &(_, slope)) in limited.iter_mut().zip(&slopes) {
                *flag |= slope;
            }
        }
        (vector, info, limited)
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
        let inverse = self.preconditioner(tikhonov, smooth);
        let precondition = |r: &[f32]| -> Vec<f32> { r.iter().zip(&inverse).map(|(r, m)| r * m).collect() };
        let mut vector = start;
        let mut residual: Vec<f32> = if vector.iter().all(|&v| v == 0.0) {
            rhs.to_vec()
        } else {
            let product = self.matvec(&vector, tikhonov, smooth);
            rhs.iter().zip(&product).map(|(b, a)| b - a).collect()
        };
        mask(&mut residual);
        // The stop stays on the RESIDUAL's own norm, so the tolerance means
        // what it meant before the preconditioner.
        let mut z = precondition(&residual);
        let mut direction = z.clone();
        let mut rz = dot(&residual, &z);
        let mut rr = dot(&residual, &residual);
        let initial = rr.max(1e-30);
        let mut used = 0usize;
        for step in 1..=iterations {
            used = step;
            let mut product = self.matvec(&direction, tikhonov, smooth);
            mask(&mut product);
            let denominator = dot(&direction, &product);
            if denominator <= 1e-20 { break; }
            let alpha = (rz / denominator) as f32;
            for (i, slot) in vector.iter_mut().enumerate() {
                *slot += alpha * direction[i];
                residual[i] -= alpha * product[i];
            }
            let next = dot(&residual, &residual);
            if next <= initial * 1e-10 { rr = next; break; }
            z = precondition(&residual);
            let rz_next = dot(&residual, &z);
            let beta = (rz_next / rz.max(1e-30)) as f32;
            for (slot, &zi) in direction.iter_mut().zip(&z) { *slot = zi + beta * *slot; }
            (rr, rz) = (next, rz_next);
        }
        (vector, SolveInfo { iterations: used, relative_residual: (rr / initial).sqrt() as f32 })
    }
}

/// The bounds one vertex's parameters are held to, read from the solve's
/// per-parameter pairs (all symmetric about 0): EV, the three gains, the slope.
#[derive(Clone, Copy, Debug)]
struct ChangeLimits {
    ev: f32,
    gain: f32,
    slope: f32,
}

impl ChangeLimits {
    fn of(bounds: &impl Fn(usize) -> (f32, f32)) -> Self {
        let half = |p: usize| {
            let (low, high) = bounds(p);
            debug_assert!((low + high).abs() <= 1e-6, "the field's bounds are symmetric about 0");
            high
        };
        debug_assert!((2..=3).all(|p| half(p) == half(1)), "one gain bound for the three channels");
        Self { ev: half(0), gain: half(1), slope: half(4) }
    }
}

/// A vertex's per-channel multiplier change, `ln2·EV + gain_c` — what its
/// colour parameters do to a pixel ([`delta`]; the slope is apart).
fn multiplier(p: &[f32]) -> [f32; 3] {
    std::array::from_fn(|c| (LN2 * f64::from(p[0])) as f32 + p[1 + c])
}

/// Hold one vertex's colour change to its bounds ALONG ITS OWN DIRECTION, and
/// its slope to its own; returns which of the two was limited.
///
/// EV and the gains are one change between them, the per-channel multiplier
/// `m_c = ln2·EV + gain_c` ([`multiplier`]), and the bounds allow every `m`
/// whose spread fits the gain bound and whose middle the EV bound can carry:
/// with `mid = (max m + min m)/2` and `half = (max m − min m)/2`, `half ≤ g`
/// and `|mid| ≤ ln2·E + g − half`. A change outside that set is scaled toward
/// none by the largest factor that brings it in — every channel by the same
/// factor, so the change keeps its direction and only its size gives way —
/// and split back with the middle on EV as far as EV reaches. A change the
/// set already holds but whose split sits outside a bound (EV and the gains
/// trade one for one) is only split back. Inside its bounds a vertex is left
/// exactly as the solve wrote it.
///
/// The first flag: the colour change was scaled back. The second: the slope
/// was on or past its bound.
fn limit_colour_change(p: &mut [f32], limits: &ChangeLimits) -> (bool, bool) {
    let slope = p[4].abs() >= limits.slope;
    p[4] = p[4].clamp(-limits.slope, limits.slope);
    if p[0].abs() <= limits.ev && p[1..4].iter().all(|g| g.abs() <= limits.gain) {
        return (false, slope);
    }
    let m = multiplier(p).map(f64::from);
    let (max, min) = (m[0].max(m[1]).max(m[2]), m[0].min(m[1]).min(m[2]));
    let (mid, half) = ((max + min) / 2.0, (max - min) / 2.0);
    let (ev_reach, gain) = (LN2 * f64::from(limits.ev), f64::from(limits.gain));
    let mut scale = 1.0f64;
    if half > gain {
        scale = scale.min(gain / half);
    }
    if mid.abs() + half > ev_reach + gain {
        scale = scale.min((ev_reach + gain) / (mid.abs() + half));
    }
    let ev = ((scale * mid) / LN2).clamp(-f64::from(limits.ev), f64::from(limits.ev));
    p[0] = ev as f32;
    for c in 0..3 {
        p[1 + c] = ((scale * m[c] - LN2 * ev) as f32).clamp(-limits.gain, limits.gain);
    }
    (scale < 1.0, slope)
}

/// [`limit_colour_change`] over every vertex of a flat parameter vector.
fn limit_all(vector: &mut [f32], limits: &ChangeLimits) -> Vec<(bool, bool)> {
    vector.chunks_exact_mut(PARAMS).map(|p| limit_colour_change(p, limits)).collect()
}

/// The x/y/b Laplacian of `GridSystem.matvec`: per adjacent pair along an axis,
/// `lap[second] += w*delta` and `lap[first] -= w*delta` — the gradient of
/// `sum (v[i+1] - v[i])^2`.  Axis order/weights follow the `(SY, SX, SB, PARAMS)`
/// reshape.
fn laplacian(v: &[f32], smooth: [f32; 3], owners: Option<&[u8]>, lap: &mut [f32]) {
    let strides = [FIELD_X * FIELD_B * PARAMS, FIELD_B * PARAMS, PARAMS];
    let counts = [FIELD_Y, FIELD_X, FIELD_B];
    for (axis, &weight) in [smooth[1], smooth[0], smooth[2]].iter().enumerate() {
        if weight <= 0.0 { continue; }
        let (stride, count) = (strides[axis], counts[axis]);
        for i in 0..v.len() {
            if (i / stride) % count + 1 >= count { continue; }
            if !smoother_couples(owners, i / PARAMS, (i + stride) / PARAMS) { continue; }
            let step = weight * (v[i + stride] - v[i]);
            lap[i + stride] += step;
            lap[i] -= step;
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f64 { a.iter().zip(b).map(|(&x, &y)| x as f64 * y as f64).sum() }

#[cfg(test)]
pub(crate) mod tests;
