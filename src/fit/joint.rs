//! The joint objective (v1.18.0, RULING-1180-joint-r1): ONE objective the
//! global solve minimises, with one penalty per degree of freedom and hard
//! constraints that only reject —
//!
//! `F(x) = J*(render(x)) + FIT_QUANT · dof(x)`,  `x` admissible under `G`.
//!
//! Before this the stages each fitted a different statistic (a luma map, a
//! chroma ratio, channel CDFs) and J* only ever accepted or rejected what
//! they produced: 55 decision points on the Full path, about twenty of them
//! giving something back, every one compensating for the fact that nothing
//! minimised J*. The self-consistency law's fourth form proved it: the whole
//! fit with the exposure FORBIDDEN landed J* 0.0039 against the forward
//! solve's 0.0107 on a pair whose target was that exposure (rel1180 msg-sc
//! round 11) — a solver that minimised J* over its feasible set could never
//! be beaten by one restricted to a subset of it.
//!
//! Now the stages are the INITIALISER (their estimates only; their
//! identification knowledge — knot support, withheld ranges, band admission —
//! shapes the start; a class G refuses is re-estimated from the law's own
//! asks, [`Joint::repair`]), and this module does what they could not: a
//! deterministic descent on F along the finite-difference gradient of J* over
//! the free coordinates together ([`Joint::descend`]), a coordinate polish at
//! every level ([`Joint::polish`]), then the
//! self-consistency law read whole ([`Joint::eliminate`]: a control's absence
//! is the recipe with that control at the base and pinned, re-polished; it is
//! dropped when the absence is no worse on F), then the base as a candidate
//! ([`Joint::base_wins`]: the terminal do-no-harm is the same comparison).
//! The penalty is [`FIT_QUANT`] because that is the ship law (R1): a control
//! that does not buy its own rounding is no control.

use super::*;
use super::solve::SolveCtx;
use super::validity::Class;
use crate::recipe::CurvePoint;

/// The four curves a global recipe carries, in the order [`Point::outs`]
/// stores their knots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Curve {
    Tone,
    Red,
    Green,
    Blue,
}

impl Curve {
    const ALL: [Curve; 4] = [Curve::Tone, Curve::Red, Curve::Green, Curve::Blue];

    fn of(self, r: &EditRecipe) -> &Vec<CurvePoint> {
        match self {
            Curve::Tone => &r.tone_curve,
            Curve::Red => &r.red_curve,
            Curve::Green => &r.green_curve,
            Curve::Blue => &r.blue_curve,
        }
    }

    fn of_mut(self, r: &mut EditRecipe) -> &mut Vec<CurvePoint> {
        match self {
            Curve::Tone => &mut r.tone_curve,
            Curve::Red => &mut r.red_curve,
            Curve::Green => &mut r.green_curve,
            Curve::Blue => &mut r.blue_curve,
        }
    }

    fn index(self) -> usize {
        match self {
            Curve::Tone => 0,
            Curve::Red => 1,
            Curve::Green => 2,
            Curve::Blue => 3,
        }
    }
}

/// One continuous coordinate of the global recipe: what the descent steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Coord {
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    /// The Kelvin, stepped on the inversion's log grid.
    Temperature,
    Tint,
    Vibrance,
    Saturation,
    HslHue(usize),
    HslSaturation(usize),
    HslLuminance(usize),
    /// A grade wheel's colour as the Cartesian pair of its (hue, saturation)
    /// — `a = sat·cos hue`, `b = sat·sin hue` — so a wheel at saturation 0
    /// steps out in any direction (0 shadows, 1 midtones, 2 highlights, 3
    /// global). On the polar pair (until 2026-11-07) the hue coordinate moved
    /// nothing at saturation 0 and the saturation coordinate could only step
    /// out at the stored hue, red, so a wheel the stages left at the origin
    /// was never found. The recipe stores the polar pair; `dof_of` reads it.
    GradeA(usize),
    GradeB(usize),
    GradeLum(usize),
    /// One knot of a curve — its OUTPUT code — on the point's knot grid
    /// ([`Point::grids`]: the stage's knots where it fitted a curve, the
    /// engine's grid where it did not), monotone between its neighbours. A
    /// FREE point's curves (Full mode: the pixels pair and J* reads each of
    /// them) carry one of these per knot, so the descent and the
    /// re-estimation can shape a curve (user ruling 2026-11-09 00:40).
    Knot(Curve, usize),
    /// The ONE coordinate a BOUNDED point's curve carries (Atmosphere mode:
    /// the evidence is the population's distribution): the strength `s` of
    /// the stages' proposed shape, `out = x + s · (proposal − x)` at every
    /// knot, `s` in [0, 1]; a curve the stages left on the line has no
    /// coordinate at all. Under population evidence alone the free knots
    /// shipped a 77-code cliff on the night master and a shadow crush on
    /// p37 (legs/J9, J15): each matched the histogram term and passed the
    /// law's cell medians, and a distribution cannot say which pixel goes
    /// where, so it cannot license the knots' freedom (user ruling
    /// 2026-11-09 11:30, RULING-1180-joint-r2: the curve's freedom follows
    /// the evidence; the slope axis the law read against the cliff retires).
    CurveStrength(Curve),
}

/// A grade wheel's stored (hue, saturation) as the Cartesian pair the
/// descent steps (`Coord::GradeA`, `Coord::GradeB`).
fn wheel_cartesian(g: &crate::recipe::ColorGrade, w: usize) -> (f32, f32) {
    let hue = [g.shadow_hue, g.midtone_hue, g.highlight_hue, g.global_hue][w].to_radians();
    let sat = [g.shadow_sat, g.midtone_sat, g.highlight_sat, g.global_sat][w];
    (sat * hue.cos(), sat * hue.sin())
}

/// Write wheel `w` from its Cartesian pair: the saturation is the pair's
/// length, rounded to the recipe's 0.1 and held at the strength budget's
/// wheel bound `cap` (the coordinates' box reaches past it at a corner); the
/// hue is the pair's angle, and the base's own where nothing is left to
/// point (saturation 0 renders no hue, and `Control::differs` must read the
/// wheel as unmoved).
fn set_wheel_cartesian(
    g: &mut crate::recipe::ColorGrade,
    w: usize,
    a: f32,
    b: f32,
    base: &crate::recipe::ColorGrade,
    cap: f32,
) {
    let sat = (a.hypot(b).min(cap) * 10.0).round() / 10.0;
    let hue = if sat <= 0.0 {
        [base.shadow_hue, base.midtone_hue, base.highlight_hue, base.global_hue][w]
    } else {
        b.atan2(a).to_degrees().rem_euclid(360.0)
    };
    *[&mut g.shadow_hue, &mut g.midtone_hue, &mut g.highlight_hue, &mut g.global_hue][w] = hue;
    *[&mut g.shadow_sat, &mut g.midtone_sat, &mut g.highlight_sat, &mut g.global_sat][w] = sat;
}

/// The log-Kelvin grid the white balance is inverted on (`WbLaw::invert`,
/// 401 steps over the search domain): the Temperature coordinate's quantum.
fn kelvin_quantum() -> f32 {
    (WB_SEARCH_K.1.ln() - WB_SEARCH_K.0.ln()) / 400.0
}

impl Coord {
    /// Every coordinate of `p`'s recipe (the knots on `p`'s grids).
    pub(super) fn all(p: &Point) -> Vec<Coord> {
        let mut all = vec![
            Coord::Exposure,
            Coord::Contrast,
            Coord::Highlights,
            Coord::Shadows,
            Coord::Whites,
            Coord::Blacks,
            Coord::Temperature,
            Coord::Tint,
            Coord::Saturation,
            Coord::Vibrance,
        ];
        for w in 0..4 {
            all.extend([Coord::GradeA(w), Coord::GradeB(w), Coord::GradeLum(w)]);
        }
        for b in 0..8 {
            all.extend([Coord::HslHue(b), Coord::HslSaturation(b), Coord::HslLuminance(b)]);
        }
        for c in Curve::ALL {
            all.extend(p.curve_coords(c));
        }
        all
    }

    /// The coordinates one [`Control`] of the elimination holds.
    /// The coordinates Atmosphere mode does not solve, pinned at the base:
    /// the channel curves (absent by construction), the grade wheels and
    /// vibrance (this mode fits the robust global controls, the mixer and
    /// the detail).
    pub(super) fn atmosphere_pinned(p: &Point) -> Vec<Coord> {
        let mut pinned = vec![Coord::Vibrance];
        for c in [Curve::Red, Curve::Green, Curve::Blue] {
            pinned.extend(p.curve_coords(c));
        }
        for w in 0..4 {
            pinned.extend([Coord::GradeA(w), Coord::GradeB(w), Coord::GradeLum(w)]);
        }
        pinned
    }

    pub(super) fn of_control(c: Control, p: &Point) -> Vec<Coord> {
        let knots = |curve: Curve| p.curve_coords(curve);
        match c {
            Control::Exposure => vec![Coord::Exposure],
            Control::Contrast => vec![Coord::Contrast],
            Control::Highlights => vec![Coord::Highlights],
            Control::Shadows => vec![Coord::Shadows],
            Control::Whites => vec![Coord::Whites],
            Control::Blacks => vec![Coord::Blacks],
            Control::Temperature => vec![Coord::Temperature],
            Control::Tint => vec![Coord::Tint],
            Control::Vibrance => vec![Coord::Vibrance],
            Control::Saturation => vec![Coord::Saturation],
            Control::HslHue(b) => vec![Coord::HslHue(b)],
            Control::HslSaturation(b) => vec![Coord::HslSaturation(b)],
            Control::HslLuminance(b) => vec![Coord::HslLuminance(b)],
            Control::GradeShadows => vec![Coord::GradeA(0), Coord::GradeB(0), Coord::GradeLum(0)],
            Control::GradeMidtones => vec![Coord::GradeA(1), Coord::GradeB(1), Coord::GradeLum(1)],
            Control::GradeHighlights => vec![Coord::GradeA(2), Coord::GradeB(2), Coord::GradeLum(2)],
            Control::GradeGlobal => vec![Coord::GradeA(3), Coord::GradeB(3), Coord::GradeLum(3)],
            Control::ToneCurve => knots(Curve::Tone),
            Control::RedCurve => knots(Curve::Red),
            Control::GreenCurve => knots(Curve::Green),
            Control::BlueCurve => knots(Curve::Blue),
        }
    }

    /// The coordinate's storage quantum: the step the descent ends on. A
    /// curve's strength steps by one code of the proposal's farthest knot.
    fn quantum(self, p: &Point) -> f32 {
        match self {
            Coord::Exposure => 0.01,
            Coord::Temperature => kelvin_quantum(),
            Coord::Knot(..) => 1.0,
            Coord::CurveStrength(c) => p.strength_quantum(c),
            _ => 0.1,
        }
    }

    fn get(self, p: &Point, law: render::WbLaw) -> f32 {
        let r = &p.recipe;
        let g = &r.color_grade;
        match self {
            Coord::Exposure => r.exposure_ev,
            Coord::Contrast => r.contrast,
            Coord::Highlights => r.highlights,
            Coord::Shadows => r.shadows,
            Coord::Whites => r.whites,
            Coord::Blacks => r.blacks,
            Coord::Temperature => law.point_of(r).0.ln(),
            Coord::Tint => r.tint,
            Coord::Vibrance => r.vibrance,
            Coord::Saturation => r.saturation,
            Coord::HslHue(b) => r.hsl.hue[b],
            Coord::HslSaturation(b) => r.hsl.saturation[b],
            Coord::HslLuminance(b) => r.hsl.luminance[b],
            Coord::GradeA(w) => wheel_cartesian(g, w).0,
            Coord::GradeB(w) => wheel_cartesian(g, w).1,
            Coord::GradeLum(w) => [g.shadow_lum, g.midtone_lum, g.highlight_lum, g.global_lum][w],
            Coord::Knot(c, i) => p.outs[c.index()][i],
            Coord::CurveStrength(c) => p.strength(c),
        }
    }

    /// Write `v` (already on the coordinate's grid and inside its bounds).
    fn set(self, p: &mut Point, v: f32, ctx: &SolveCtx<'_>) {
        let (base, law) = (ctx.base, ctx.law);
        let r = &mut p.recipe;
        match self {
            Coord::Exposure => r.exposure_ev = v,
            Coord::Contrast => r.contrast = v,
            Coord::Highlights => r.highlights = v,
            Coord::Shadows => r.shadows = v,
            Coord::Whites => r.whites = v,
            Coord::Blacks => r.blacks = v,
            Coord::Temperature => {
                let k = (v.exp() / 50.0).round() * 50.0;
                // At the base's own Kelvin the field is the base's (None = as
                // shot), so `Control::differs` reads it as unmoved.
                r.temperature_k = if k == law.point_of(base).0 { base.temperature_k } else { Some(k) };
            }
            Coord::Tint => r.tint = v,
            Coord::Vibrance => r.vibrance = v,
            Coord::Saturation => r.saturation = v,
            Coord::HslHue(b) => r.hsl.hue[b] = v,
            Coord::HslSaturation(b) => r.hsl.saturation[b] = v,
            Coord::HslLuminance(b) => r.hsl.luminance[b] = v,
            Coord::GradeA(w) => {
                let (_, b) = wheel_cartesian(&r.color_grade, w);
                set_wheel_cartesian(&mut r.color_grade, w, v, b, &base.color_grade, ctx.budget.grade_wheel);
            }
            Coord::GradeB(w) => {
                let (a, _) = wheel_cartesian(&r.color_grade, w);
                set_wheel_cartesian(&mut r.color_grade, w, a, v, &base.color_grade, ctx.budget.grade_wheel);
            }
            Coord::GradeLum(w) => {
                let g = &mut r.color_grade;
                *[&mut g.shadow_lum, &mut g.midtone_lum, &mut g.highlight_lum, &mut g.global_lum][w] = v;
            }
            Coord::Knot(c, i) => {
                p.outs[c.index()][i] = v;
                *c.of_mut(&mut p.recipe) = knots_curve(&p.grids[c.index()], &p.outs[c.index()]);
            }
            Coord::CurveStrength(c) => {
                p.set_strength(c, v);
                *c.of_mut(&mut p.recipe) = knots_curve(&p.grids[c.index()], &p.outs[c.index()]);
            }
        }
    }

    /// The coordinate's own bounds (the recipe's and the budget's).
    fn bounds(self, ctx: &SolveCtx<'_>) -> (f32, f32) {
        match self {
            Coord::Exposure => (-ctx.ev_limit, ctx.ev_limit),
            Coord::Temperature => (WB_SEARCH_K.0.ln(), WB_SEARCH_K.1.ln()),
            Coord::Tint => (-150.0, 150.0),
            Coord::Saturation => (-ctx.full_sat_limit, ctx.full_sat_limit),
            Coord::HslHue(_) | Coord::HslSaturation(_) | Coord::HslLuminance(_) => {
                (-ctx.budget.hsl_band, ctx.budget.hsl_band)
            }
            Coord::GradeA(_) | Coord::GradeB(_) => (-ctx.budget.grade_wheel, ctx.budget.grade_wheel),
            Coord::Knot(..) => (0.0, 255.0),
            Coord::CurveStrength(_) => (0.0, 1.0),
            _ => (-100.0, 100.0),
        }
    }

    /// `v ± steps·quantum`, on the grid and inside the bounds; a knot
    /// stays between its neighbours' outputs (a curve is monotone).
    fn step(self, p: &Point, v: f32, dir: f32, steps: f32, ctx: &SolveCtx<'_>) -> f32 {
        let q = self.quantum(p);
        let (mut lo, mut hi) = self.bounds(ctx);
        if let Coord::Knot(c, i) = self {
            let outs = &p.outs[c.index()];
            if i > 0 {
                lo = lo.max(outs[i - 1]);
            }
            if i + 1 < outs.len() {
                hi = hi.min(outs[i + 1]);
            }
        }
        let w = v + dir * steps * q;
        let w = w.clamp(lo, hi);
        // Round onto the grid so a step never drifts off the storage values.
        match self {
            Coord::Temperature => w,
            _ => (w / q).round() * q,
        }
    }
}

/// The curve a knot grid and its outputs store: the outputs rounded to the
/// codes the recipe carries; no curve at all when every knot is on the line.
fn knots_curve(grid: &[u8], outs: &[f32]) -> Vec<CurvePoint> {
    let pts: Vec<CurvePoint> =
        grid.iter().zip(outs).map(|(&input, &out)| CurvePoint { input, output: out.round().clamp(0.0, 255.0) as u8 }).collect();
    if pts.iter().all(|p| p.output == p.input) { Vec::new() } else { pts }
}

/// The knot grid a point carries for a curve: the recipe's own knots where
/// it has a curve, the engine's grid where it does not (the tone's eight
/// knots, the channel curves' five), so a curve the stages left empty can
/// still be shaped by the descent.
fn knot_grid(c: Curve, r: &EditRecipe) -> (Vec<u8>, Vec<f32>) {
    let own = c.of(r);
    if !own.is_empty() {
        return (own.iter().map(|p| p.input).collect(), own.iter().map(|p| p.output as f32).collect());
    }
    let xs: &[f32] = match c {
        Curve::Tone => &render::TONE_KNOTS_X,
        _ => &super::curves::CHANNEL_CURVE_KNOTS_X,
    };
    let grid: Vec<u8> = xs.iter().map(|x| (x * 255.0).round() as u8).collect();
    let outs = grid.iter().map(|&g| g as f32).collect();
    (grid, outs)
}

/// The free knots of a curve: its points off the identity line. An empty
/// curve has none; a curve with every point on the line renders as none.
fn free_knots(curve: &[CurvePoint]) -> usize {
    curve.iter().filter(|p| p.output != p.input).count()
}

/// A point of the search: the recipe, with each curve's knot grid and the
/// knots' outputs — the coordinates [`Coord::Knot`] step on a FREE point,
/// or, on a BOUNDED point, the outputs [`Coord::CurveStrength`] blends
/// between the identity and the stages' proposed shape.
#[derive(Clone)]
pub(super) struct Point {
    pub(super) recipe: EditRecipe,
    grids: [Vec<u8>; 4],
    outs: [Vec<f32>; 4],
    /// The stages' proposed outputs, per curve, where the point is bounded
    /// (Atmosphere mode); `None` where the knots are free (Full mode).
    shape: Option<[Vec<f32>; 4]>,
}

impl Point {
    /// A free point: every knot of every curve is a coordinate.
    pub(super) fn new(recipe: EditRecipe) -> Self {
        let both = Curve::ALL.map(|c| knot_grid(c, &recipe));
        let grids = std::array::from_fn(|i| both[i].0.clone());
        let outs = std::array::from_fn(|i| both[i].1.clone());
        Point { recipe, grids, outs, shape: None }
    }

    /// A bounded point: each curve keeps the shape `recipe` proposes and
    /// carries one coordinate, its strength (Atmosphere mode).
    pub(super) fn bounded(recipe: EditRecipe) -> Self {
        let mut p = Point::new(recipe);
        p.shape = Some(p.outs.clone());
        p
    }

    /// `c`'s proposed shape off the identity, per knot (a bounded point).
    fn shape_rise(&self, c: Curve) -> Option<Vec<f32>> {
        let shape = self.shape.as_ref()?;
        let i = c.index();
        Some(shape[i].iter().zip(&self.grids[i]).map(|(&p, &x)| p - x as f32).collect())
    }

    /// The coordinates curve `c` carries: a knot each where the point is
    /// free; one strength where it is bounded and the proposal is off the
    /// line; none where it is bounded and the proposal is the identity.
    pub(super) fn curve_coords(&self, c: Curve) -> Vec<Coord> {
        match self.shape_rise(c) {
            None => (0..self.grids[c.index()].len()).map(|i| Coord::Knot(c, i)).collect(),
            Some(rise) if rise.iter().any(|r| *r != 0.0) => vec![Coord::CurveStrength(c)],
            Some(_) => Vec::new(),
        }
    }

    /// The strength `c`'s outputs ship the proposal at: the projection of
    /// their rise onto the proposal's (1 at the proposal, 0 at the identity).
    fn strength(&self, c: Curve) -> f32 {
        let Some(rise) = self.shape_rise(c) else { return 0.0 };
        let norm: f32 = rise.iter().map(|r| r * r).sum();
        if norm <= 0.0 {
            return 0.0;
        }
        let i = c.index();
        let dot: f32 = self.outs[i].iter().zip(&self.grids[i]).zip(&rise).map(|((&o, &x), r)| (o - x as f32) * r).sum();
        dot / norm
    }

    /// One code of the proposal's farthest knot, as a strength.
    fn strength_quantum(&self, c: Curve) -> f32 {
        let reach = self.shape_rise(c).map(|r| r.iter().fold(0.0f32, |m, r| m.max(r.abs()))).unwrap_or(0.0);
        if reach > 0.0 { 1.0 / reach } else { 1.0 }
    }

    /// `c`'s outputs at strength `s` of the proposal (a bounded point).
    fn set_strength(&mut self, c: Curve, s: f32) {
        let Some(rise) = self.shape_rise(c) else { return };
        let i = c.index();
        for ((o, &x), r) in self.outs[i].iter_mut().zip(&self.grids[i]).zip(&rise) {
            *o = x as f32 + s * r;
        }
    }

    /// The point at `base` with this point's freedom: a free point on the
    /// base's own grids; a bounded point keeps its shape, with the base
    /// curves read at its knots.
    fn at_base(&self, base: &EditRecipe) -> Point {
        if self.shape.is_none() {
            return Point::new(base.clone());
        }
        let mut p = self.clone();
        p.recipe = base.clone();
        for c in Curve::ALL {
            let i = c.index();
            let lut = render::curve_lut(c.of(base));
            p.outs[i] = p.grids[i].iter().map(|&x| lut[x as usize] * 255.0).collect();
        }
        p
    }

    /// The value the BASE recipe has on `c`: a knot's is the base curve at
    /// the knot's input (the identity where the base has no curve), every
    /// other coordinate the base recipe's own.
    fn base_value(&self, c: Coord, ctx: &SolveCtx<'_>) -> f32 {
        match c {
            Coord::Knot(curve, i) => {
                let input = self.grids[curve.index()][i];
                render::curve_lut(curve.of(ctx.base))[input as usize] * 255.0
            }
            Coord::CurveStrength(curve) => self.at_base(ctx.base).strength(curve),
            _ => c.get(&Point::new(ctx.base.clone()), ctx.law),
        }
    }
}

/// What one evaluation of a point reads: its render, J*, dof and F.
#[derive(Clone)]
pub(super) struct Eval {
    pub(super) px: Vec<[f32; 3]>,
    pub(super) j: f32,
    pub(super) dof: usize,
}

impl Eval {
    pub(super) fn f(&self) -> f32 {
        self.j + FIT_QUANT * self.dof as f32
    }
}

/// The joint objective over one solve's context, with the coordinates pinned
/// at the base (the elimination's forbidden set).
pub(super) struct Joint<'c, 'a> {
    ctx: &'c SolveCtx<'a>,
    pinned: Vec<Coord>,
    /// Renders this solve took, a reading printed with the time.
    pub(super) renders: std::cell::Cell<u32>,
    /// Seconds spent rendering, reading J*, and reading G (the class frames'
    /// extra renders counted under G), the same reading.
    pub(super) spent: std::cell::Cell<(f32, f32, f32)>,
    /// The render count at the end of each named phase, the same reading.
    phases: std::cell::RefCell<Vec<(&'static str, u32)>>,
    /// The classes [`Joint::repair`] scaled the start on and the fraction of
    /// the stages' estimate each kept — the report's seed disclosure.
    scaled: std::cell::RefCell<Vec<(Refusal, f32)>>,
    /// The solve's number in this process, for the trace.
    id: u32,
}

/// The solves started in this process, numbered for the trace.
static SOLVES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Step multipliers of the descent, coarse to the quantum.
pub(super) const STEP_MULTIPLIERS: [f32; 4] = [8.0, 4.0, 2.0, 1.0];
/// Pass cap of one polish; a pass that keeps nothing halves the step, so
/// the cap only bounds a long accepting run (as `refine_tone_sliders_on`).
const POLISH_PASS_CAP: usize = 64;
/// Re-estimations of one class in the repair before it is scaled instead:
/// a class's leave-one-out frame moves when another class is re-estimated,
/// so a class admitted whole can be refused again a round later (the coast's
/// tone after its chroma dials, gates/j106); three re-estimations let a
/// four-class cycle settle, and a class still refused after them is scaled
/// toward the base, which is admissible for it at some fraction.
const REPAIR_REFITS: usize = 3;
/// Bisections of the repair's fraction: six halvings resolve it to 1/64.
const REPAIR_BISECTIONS: usize = 6;
/// Pass cap of the gradient descent at one level: a pass that accepts nothing
/// ends the level, so the cap only bounds a long accepting run.
const DESCENT_PASS_CAP: usize = 16;
/// Pass cap of a class's re-estimation from the asks at one level, the same.
const ASK_FIT_PASS_CAP: usize = 8;

impl<'c, 'a> Joint<'c, 'a> {
    pub(super) fn new(ctx: &'c SolveCtx<'a>) -> Self {
        Self::with_pinned(ctx, Vec::new())
    }

    /// The objective with `pinned` held at the base throughout.
    pub(super) fn with_pinned(ctx: &'c SolveCtx<'a>, pinned: Vec<Coord>) -> Self {
        Joint {
            ctx,
            pinned,
            renders: std::cell::Cell::new(0),
            spent: std::cell::Cell::new((0.0, 0.0, 0.0)),
            phases: std::cell::RefCell::new(Vec::new()),
            scaled: std::cell::RefCell::new(Vec::new()),
            id: SOLVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// The classes the repair scaled the start on, named as the report's
    /// enumerated values, with the fraction of the estimate each kept.
    pub(super) fn scaled(&self) -> Vec<(&'static str, f32)> {
        // A class re-estimated twice is named once, with its last fraction.
        let mut out: Vec<(&'static str, f32)> = Vec::new();
        for &(class, lambda) in self.scaled.borrow().iter() {
            match out.iter_mut().find(|(c, _)| *c == class.value()) {
                Some(slot) => slot.1 = lambda,
                None => out.push((class.value(), lambda)),
            }
        }
        out
    }

    /// The phase trace under `AUTOSHADE_G_TRACE`: J*, the degrees of freedom,
    /// F and the controls off the base at the end of a phase.
    pub(super) fn trace(&self, phase: &str, p: &Point, e: &Eval) {
        if crate::config::live_env_os("AUTOSHADE_G_TRACE").is_none() {
            return;
        }
        let moved: Vec<String> =
            global_controls().into_iter().filter(|c| c.differs(&p.recipe, self.ctx.base)).map(|c| c.name()).collect();
        eprintln!("J#{} {phase}: J* {:.5} dof {} F {:.5} [{}]", self.id, e.j, e.dof, e.f(), moved.join(", "));
        if phase == "start" {
            eprintln!("J#{} start recipe: {}", self.id, serde_json::to_string(&p.recipe).unwrap_or_default());
        }
    }

    /// The trace of the stages' proposal as it enters the solve (one render,
    /// under `AUTOSHADE_G_TRACE` only).
    pub(super) fn trace_start(&self, p: &Point) {
        if crate::config::live_env_os("AUTOSHADE_G_TRACE").is_none() {
            return;
        }
        let e = self.eval(p);
        self.trace("start", p, &e);
    }

    /// A sub-solve's readings, added to this one's.
    fn absorb(&self, sub: &Joint<'_, '_>) {
        self.renders.set(self.renders.get() + sub.renders.get());
        let (r, j, g) = self.spent.get();
        let (sr, sj, sg) = sub.spent.get();
        self.spent.set((r + sr, j + sj, g + sg));
    }

    /// A phase of the solve ended: its name and the renders it took are
    /// recorded for the trace line.
    pub(super) fn mark(&self, phase: &'static str) {
        self.phases.borrow_mut().push((phase, self.renders.get()));
    }

    /// The trace line's tail: renders, where the seconds went, and the
    /// renders per phase.
    pub(super) fn spent_line(&self) -> String {
        let (r, j, g) = self.spent.get();
        let mut last = 0u32;
        let phases: Vec<String> = self
            .phases
            .borrow()
            .iter()
            .map(|&(name, at)| {
                let took = at - last;
                last = at;
                format!("{name} {took}")
            })
            .collect();
        format!(
            "{} renders ({r:.1} s rendering, {j:.1} s reading J*, {g:.1} s reading G; {})",
            self.renders.get(),
            phases.join(", ")
        )
    }

    fn render(&self, r: &EditRecipe) -> Vec<[f32; 3]> {
        self.renders.set(self.renders.get() + 1);
        let started = std::time::Instant::now();
        let px = pixels_of(&render::develop_preview(self.ctx.s_img, r));
        let (sr, sj, sg) = self.spent.get();
        self.spent.set((sr + started.elapsed().as_secs_f32(), sj, sg));
        px
    }

    /// dof(x) against this solve's base ([`dof_of`]).
    pub(super) fn dof(&self, r: &EditRecipe) -> usize {
        dof_of(r, self.ctx.base, self.ctx.law)
    }
}

/// dof(x): one per scalar control off the base `b`; a wheel's hue only where
/// its saturation moved (a hue with no saturation renders nothing); a curve
/// its free knots. `law` reads each recipe's white point.
pub(super) fn dof_of(r: &EditRecipe, b: &EditRecipe, law: render::WbLaw) -> usize {
    let mut n = 0;
    let scalar = |x: f32, y: f32| usize::from(x != y);
    n += scalar(r.exposure_ev, b.exposure_ev)
        + scalar(r.contrast, b.contrast)
        + scalar(r.highlights, b.highlights)
        + scalar(r.shadows, b.shadows)
        + scalar(r.whites, b.whites)
        + scalar(r.blacks, b.blacks)
        + scalar(r.vibrance, b.vibrance)
        + scalar(r.saturation, b.saturation);
    let (pr, pb) = (law.point_of(r), law.point_of(b));
    n += scalar(pr.0, pb.0) + scalar(pr.1, pb.1);
    for i in 0..8 {
        n += scalar(r.hsl.hue[i], b.hsl.hue[i])
            + scalar(r.hsl.saturation[i], b.hsl.saturation[i])
            + scalar(r.hsl.luminance[i], b.hsl.luminance[i]);
    }
    let (g, h) = (&r.color_grade, &b.color_grade);
    for (hue, sat, lum) in [
        ((g.shadow_hue, h.shadow_hue), (g.shadow_sat, h.shadow_sat), (g.shadow_lum, h.shadow_lum)),
        ((g.midtone_hue, h.midtone_hue), (g.midtone_sat, h.midtone_sat), (g.midtone_lum, h.midtone_lum)),
        ((g.highlight_hue, h.highlight_hue), (g.highlight_sat, h.highlight_sat), (g.highlight_lum, h.highlight_lum)),
        ((g.global_hue, h.global_hue), (g.global_sat, h.global_sat), (g.global_lum, h.global_lum)),
    ] {
        let sat_moved = scalar(sat.0, sat.1);
        n += sat_moved + scalar(lum.0, lum.1) + if sat_moved == 1 { scalar(hue.0, hue.1) } else { 0 };
    }
    for c in Curve::ALL {
        n += free_knots(c.of(r));
    }
    n
}

/// The coordinates a refused class holds ([`Joint::repair`]).
fn class_coords(class: Refusal, p: &Point) -> Vec<Coord> {
    let knots = |curve: Curve| p.curve_coords(curve);
    match class {
        Refusal::Tone => {
            let mut v = vec![Coord::Exposure, Coord::Contrast, Coord::Highlights, Coord::Shadows, Coord::Whites, Coord::Blacks];
            v.extend(knots(Curve::Tone));
            v
        }
        Refusal::WhiteBalance => vec![Coord::Temperature, Coord::Tint],
        Refusal::Chroma => {
            let mut v = vec![Coord::Vibrance, Coord::Saturation];
            for w in 0..4 {
                v.extend([Coord::GradeA(w), Coord::GradeB(w), Coord::GradeLum(w)]);
            }
            for b in 0..8 {
                v.extend([Coord::HslHue(b), Coord::HslSaturation(b), Coord::HslLuminance(b)]);
            }
            v
        }
        Refusal::Curves => [Curve::Red, Curve::Green, Curve::Blue].into_iter().flat_map(knots).collect(),
    }
}

/// `p` with each coordinate of `coords` at the fraction `lambda` of its
/// distance from the base, on the coordinate's grid.
fn scale_class(p: &mut Point, coords: &[Coord], lambda: f32, ctx: &SolveCtx<'_>) {
    for &c in coords {
        let (b, v) = (p.base_value(c, ctx), c.get(p, ctx.law));
        let w = c.step(p, b + lambda * (v - b), 0.0, 0.0, ctx);
        c.set(p, w, ctx);
    }
}

/// The proposed controls (the stages' estimate as it entered the descent)
/// that the finished recipe no longer carries, added to `unearned`: a control
/// the descent walked back to the base earned less than the quantum as
/// surely as one the elimination dropped, and one disclosure names them all.
pub(super) fn walked_back(proposed: &EditRecipe, finished: &EditRecipe, base: &EditRecipe, unearned: &mut Vec<Control>) {
    for c in global_controls() {
        if c.differs(proposed, base) && !c.differs(finished, base) && !unearned.contains(&c) {
            unearned.push(c);
        }
    }
}

/// The class G refuses a candidate on, in the order the stages compose the
/// frame: the tone, the white balance, the chroma dials (saturation,
/// vibrance, the mixer, the grade), the channel curves. The repair scales
/// that class back ([`Joint::repair`]); the probe prints it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Refusal {
    WhiteBalance,
    Tone,
    Chroma,
    Curves,
}

impl Refusal {
    /// The law's class this refusal is of.
    pub(super) fn class(self) -> Class {
        match self {
            Refusal::Tone => Class::Tone,
            Refusal::WhiteBalance => Class::WhiteBalance,
            Refusal::Chroma => Class::Chroma,
            Refusal::Curves => Class::Curves,
        }
    }

    /// The class as the report names it (`rationale::values`, translated).
    pub(super) fn value(self) -> &'static str {
        use crate::rationale::values;
        match self {
            Refusal::Tone => values::CLASS_TONE,
            Refusal::WhiteBalance => values::CLASS_WHITE_BALANCE,
            Refusal::Chroma => values::CLASS_CHROMA_DIALS,
            Refusal::Curves => values::CLASS_CHANNEL_CURVES,
        }
    }
}

/// The candidate `r` WITHOUT one class: that class's fields at the base,
/// every other class (and the detail controls) as `r` has them. G reads each
/// class between this frame and the candidate's render ([`Joint::refusal`]):
/// the class's ask is the target's distance from the frame the OTHER classes
/// leave — the residual they do not explain, the one this class must — and
/// its move is what it adds on top of them. The elimination's own
/// leave-one-out form (RULING-1180-joint-r2, 2026-11-09). Until then the
/// classes were read in composition order (the tone from the detail frame,
/// the white balance from the tone frame, …), which charged a class with
/// what the later classes would do to a cell on its axes: on the haze pair
/// the mid-aqua cell's brightness ask read +0.009 ± 0.009 because the
/// saturation, not the tone, moves it, and the whole tone class was capped
/// at 12 % of its estimate. The detail controls belong to no class and sit in
/// both frames of every class, so a texture's troughs in the cells'
/// medians cancel (they were the tone's until 2026-11-08, J74).
pub(super) fn without_class(base: &EditRecipe, r: &EditRecipe, class: Refusal) -> EditRecipe {
    let mut w = r.clone();
    match class {
        Refusal::Tone => w = super::solve::tone_part(&w, base),
        Refusal::WhiteBalance => (w.temperature_k, w.tint) = (base.temperature_k, base.tint),
        Refusal::Chroma => {
            w.saturation = base.saturation;
            w.vibrance = base.vibrance;
            w.hsl = base.hsl.clone();
            w.color_grade = base.color_grade.clone();
        }
        Refusal::Curves => {
            for curve in CAST_CURVES {
                curve.reset(&mut w, base);
            }
        }
    }
    w
}

/// The four classes in the order G reads them.
pub(super) const CLASSES: [Refusal; 4] = [Refusal::Tone, Refusal::WhiteBalance, Refusal::Chroma, Refusal::Curves];

impl<'c, 'a> Joint<'c, 'a> {
    /// Render and read `x`.
    pub(super) fn eval(&self, p: &Point) -> Eval {
        let px = self.render(&p.recipe);
        let started = std::time::Instant::now();
        let j = look_err_with_evidence(&px, self.ctx.tp, self.ctx.evidence);
        let (sr, sj, sg) = self.spent.get();
        self.spent.set((sr, sj + started.elapsed().as_secs_f32(), sg));
        Eval { px, j, dof: self.dof(&p.recipe) }
    }

    /// G, read on the candidate's render: the strength budget's white-balance
    /// gains, and the one control-validity law ([`super::validity`]) on each
    /// class that moved, between the candidate WITHOUT that class and the
    /// candidate's render ([`without_class`]: the class's ask is the residual
    /// the other classes leave, its move what it adds). A class is
    /// refused where it leaves a cell worse on its own axes than it found it
    /// (the warm canyon's descent walked the tint toward magenta for the
    /// rocks' 87 % and the pale sky went violet, 213° → 251°, under the gates
    /// this replaced), and under `Withhold` where it moves a cell no evidence
    /// speaks for. Each class is read on its own work (2026-11-07; until then
    /// every veto read the whole candidate against the base, and the hue veto
    /// refused an EXPOSURE because the pixels of a one-sided hue band had
    /// moved with it — the calibration pair stuck at −0.04 EV where −0.7
    /// reads 0.112 against 0.162). Reject only.
    pub(super) fn admissible(&self, p: &Point, px: &[[f32; 3]]) -> bool {
        self.refusal(p, px).is_none()
    }

    /// The class G refuses `p` on, if any ([`Refusal`]); the gates' own
    /// renders are charged to G, not to rendering.
    pub(super) fn refusal(&self, p: &Point, px: &[[f32; 3]]) -> Option<Refusal> {
        let started = std::time::Instant::now();
        let before = self.spent.get();
        let verdict = self.refusal_inner(p, px);
        let (_, sj, _) = self.spent.get();
        self.spent.set((before.0, sj, before.2 + started.elapsed().as_secs_f32()));
        verdict
    }

    fn refusal_inner(&self, p: &Point, px: &[[f32; 3]]) -> Option<Refusal> {
        let ctx = self.ctx;
        let (r, base, law) = (&p.recipe, ctx.base, ctx.law);
        if law.point_of(r) != law.point_of(base) && !wb_gains_fit_budget(law.gains(law.point_of(r)), ctx.budget) {
            return Some(Refusal::WhiteBalance);
        }
        // Each class that moved, read between the candidate WITHOUT it and
        // the candidate's render (`without_class`); the base's own render
        // where the frame without the class is the base.
        let withhold = ctx.budget.vetoes == VetoPolicy::Withhold;
        let trace = crate::config::live_env_os("AUTOSHADE_G_TRACE").is_some();
        let cells = &ctx.validity;
        for class in CLASSES {
            let w_r = without_class(base, r, class);
            if w_r == *r {
                continue;
            }
            let own = (w_r != *base).then(|| self.render(&w_r));
            let before: &[[f32; 3]] = own.as_deref().unwrap_or(ctx.sp);
            let v = cells.judge(class.class(), before, px);
            if !v.admits(withhold) {
                if trace {
                    eprintln!("G {}", cells.describe(class.class(), &v));
                    eprintln!("G asks {}", cells.describe_asks(&cells.asks(class.class(), before)));
                }
                return Some(class);
            }
        }
        None
    }

    /// A start that violates G has the class G refuses RE-ESTIMATED from
    /// the law's own asks under that law ([`Joint::ask_fit`], admissible by
    /// construction) and, where the whole is still refused on it (another
    /// class's frame moved with it), SCALED toward the base — every coordinate of the class at the
    /// fraction λ of its distance from the base ([`scale_class`]), λ the
    /// largest the class is admitted at, by bisection
    /// ([`REPAIR_BISECTIONS`]) — until the start is admissible
    /// ([`Refusal`]); each class is re-estimated at most [`REPAIR_REFITS`]
    /// times, since the other classes' re-estimations move its frame. A
    /// class at λ = 0 is the base's, which that class is
    /// admitted at by construction (nothing moved). Until 2026-11-07 the
    /// refused class was reset whole, and a tint the law refused for the sky
    /// took the rocks' Kelvin with it; until 2026-11-08 it was scaled alone,
    /// and a shape refused on DIRECTION in a cell (the haze pair's mid-aqua
    /// cell asks +0.009 ln luma, the stage's tone darkens it) stayed refused
    /// at every λ, so the class fell to the base and the pair closed 8 % of
    /// its gap (RULING-1180-joint-r2, addendum 2026-11-08 23:40).
    pub(super) fn repair(&self, p: &mut Point) -> Eval {
        let base = self.ctx.base;
        let trace = crate::config::live_env_os("AUTOSHADE_G_TRACE").is_some();
        let mut refits = [0usize; 4];
        for _ in 0..CLASSES.len() * (REPAIR_REFITS + 1) {
            let e = self.eval(p);
            let Some(refusal) = self.refusal(p, &e.px) else { return e };
            let slot = CLASSES.iter().position(|c| *c == refusal).unwrap_or(0);
            let refit = refits[slot] < REPAIR_REFITS;
            if refit {
                refits[slot] += 1;
            }
            let lambda = self.repair_class(p, refusal, refit);
            self.scaled.borrow_mut().push((refusal, lambda));
            if trace {
                if lambda >= 1.0 {
                    eprintln!("G repair: {refusal:?} re-estimated from the cells' asks, admitted whole");
                } else {
                    eprintln!("G repair: {refusal:?} scaled toward the base to {lambda:.3}");
                }
            }
        }
        // Still inadmissible after every class was re-estimated and scaled:
        // the base is admissible by construction (nothing moved, the as-shot
        // gains), so the start is the base.
        let e = self.eval(p);
        if self.admissible(p, &e.px) {
            return e;
        }
        if trace {
            eprintln!("G repair: still refused after every class was re-estimated and scaled; the start is the base");
        }
        *p = p.at_base(base);
        self.eval(p)
    }

    /// One class of `p` repaired, `p` refused on it: with `refit`, the
    /// class RE-ESTIMATED from the cells' asks under its law
    /// ([`Joint::ask_fit`]) and kept when the whole is no longer refused on
    /// it; otherwise, or failing that, the class SCALED toward the base
    /// ([`scale_class`]) to the largest fraction the whole is admitted at on
    /// it, by bisection ([`REPAIR_BISECTIONS`]; the base of the class at
    /// λ = 0 is admitted for it by construction). Returns the fraction of
    /// the estimate kept: 1.0 for the re-estimate, λ for the scaling. Used
    /// by [`Joint::repair`] on the start and by [`Joint::descend`] on a step
    /// refused on a class the other coordinates' move put past its law
    /// (gates/j109: the re-estimate alone read worse on F than the class
    /// did, and the step was lost; scaled, the class keeps most of its work
    /// and the step is admitted).
    fn repair_class(&self, p: &mut Point, class: Refusal, refit: bool) -> f32 {
        let ctx = self.ctx;
        if refit {
            let full = self.ask_fit(p, class);
            let ef = self.eval(&full);
            // A re-estimate that moved nothing — every coordinate's first
            // step refused — is the class's base, admitted trivially, not a
            // repair: the stage's whole shape is scaled below, a move a
            // per-coordinate step cannot express (the tone_stage toy: an
            // exposure and the curve that undoes it on the withheld top are
            // admitted together and refused apart, gates/j120).
            let moved = without_class(ctx.base, &full.recipe, class) != full.recipe;
            if moved && self.refusal(&full, &ef.px) != Some(class) {
                *p = full;
                return 1.0;
            }
        }
        let coords = class_coords(class, p);
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        let mut kept: Option<Point> = None;
        for _ in 0..REPAIR_BISECTIONS {
            let mid = 0.5 * (lo + hi);
            let mut trial = p.clone();
            scale_class(&mut trial, &coords, mid, ctx);
            let e = self.eval(&trial);
            if self.refusal(&trial, &e.px) == Some(class) {
                hi = mid;
            } else {
                lo = mid;
                kept = Some(trial);
            }
        }
        *p = kept.unwrap_or_else(|| {
            let mut at_base = p.clone();
            scale_class(&mut at_base, &coords, 0.0, ctx);
            at_base
        });
        lo
    }

    /// The frame a class's work is read FROM for `p` ([`without_class`]),
    /// rendered — the base's own render where that frame is the base.
    fn class_before(&self, p: &Point, class: Refusal) -> Vec<[f32; 3]> {
        let w = without_class(self.ctx.base, &p.recipe, class);
        if w == *self.ctx.base { self.ctx.sp.to_vec() } else { self.render(&w) }
    }

    /// The descent's direction at `p` over `coords` for `value`: each
    /// coordinate's slope read by a PROBE that doubles from `steps` quanta
    /// until `value` moves by at least `resolution` — the objective's own
    /// quantum — or the coordinate reaches its bound (the other way where
    /// the first step is clamped); the direction is the negative GRADIENT
    /// (change per quantum), the steepest coordinate moving its whole probe
    /// and the rest in proportion to their slopes. A probe below the
    /// objective's resolution reads noise: a slider's quantum is 0.1 of
    /// 100, and eight of them moved J* by less than it resolves, so the
    /// re-estimation returned the stages' shape unchanged and the descent
    /// crawled (gates/j100). Scaling each coordinate to its OWN probe
    /// instead (J103) moved the coordinates the objective barely feels the
    /// farthest — a weak knot by 64 codes — and tripped the curves' law on
    /// every pass (gates/j104). Empty where nothing moves the objective.
    fn direction(
        &self,
        p: &Point,
        coords: &[Coord],
        steps: f32,
        resolution: f32,
        value: &mut dyn FnMut(&Point) -> f32,
        v0: f32,
    ) -> Vec<(Coord, f32)> {
        let (ctx, law) = (self.ctx, self.ctx.law);
        // (coordinate, the side probed, the probe in quanta, the change read)
        let mut probes: Vec<(Coord, f32, f32, f32)> = Vec::new();
        for &c in coords {
            let v = c.get(p, law);
            let q = c.quantum(p);
            let mut dir = 1.0f32;
            let mut h = steps;
            let mut found: Option<(f32, f32)> = None;
            loop {
                let mut w = c.step(p, v, dir, h, ctx);
                if w == v && dir > 0.0 {
                    dir = -1.0;
                    w = c.step(p, v, dir, h, ctx);
                }
                if w == v {
                    break;
                }
                let mut trial = p.clone();
                c.set(&mut trial, w, ctx);
                if trial.recipe == p.recipe {
                    break;
                }
                let dv = value(&trial) - v0;
                if !dv.is_finite() {
                    break;
                }
                found = Some((h, dv));
                // Clamped short of the probe: the bound is reached.
                let at_bound = (w - v).abs() < 0.999 * h * q;
                if dv.abs() >= resolution || at_bound {
                    break;
                }
                h *= 2.0;
            }
            if let Some((h, dv)) = found {
                probes.push((c, dir, h, dv));
            }
        }
        // The slope per quantum; the steepest coordinate sets the step: its
        // whole probe, the rest in proportion.
        let slopes: Vec<(Coord, f32, f32, f32)> = probes.into_iter().map(|(c, dir, h, dv)| (c, dir, h, dv / h)).collect();
        let Some(&(_, _, h_max, g_max)) =
            slopes.iter().max_by(|a, b| a.3.abs().total_cmp(&b.3.abs())).filter(|s| s.3.abs() > 0.0)
        else {
            return Vec::new();
        };
        slopes
            .into_iter()
            .map(|(c, dir, _, g)| (c, -dir * g.signum() * h_max * (g.abs() / g_max.abs())))
            .filter(|(_, d)| d.abs() >= 0.5)
            .collect()
    }

    /// `d` halved, the coordinates below half a quantum dropped: the
    /// backtracking of a step the law refuses or the objective does not
    /// reward at full length. `None` once no coordinate keeps a whole
    /// quantum — the step is at the grid.
    fn halved(d: &[(Coord, f32)]) -> Option<Vec<(Coord, f32)>> {
        if !d.iter().any(|(_, x)| x.abs() >= 1.0) {
            return None;
        }
        let h: Vec<(Coord, f32)> = d.iter().map(|&(c, x)| (c, 0.5 * x)).filter(|(_, x)| x.abs() >= 0.5).collect();
        (!h.is_empty()).then_some(h)
    }

    /// `p` moved by the direction `d` (quanta per coordinate), each
    /// coordinate on its grid and inside its bounds.
    fn apply(&self, p: &Point, d: &[(Coord, f32)]) -> Point {
        let (ctx, law) = (self.ctx, self.ctx.law);
        let mut q = p.clone();
        for &(c, dq) in d {
            let v = c.get(&q, law);
            let w = c.step(&q, v, dq.signum(), dq.abs(), ctx);
            c.set(&mut q, w, ctx);
        }
        q
    }

    /// A refused class RE-ESTIMATED from the law's own asks (RULING-1180-joint-r2,
    /// addendum 2026-11-08 23:40): the asks are read once on the class's
    /// before frame ([`validity::Cells::asks`]) — fixed while the class's own
    /// coordinates move — and the class's coordinates descend on the ask
    /// misfit `M = Σ mass · |move − ask|²` ([`validity::Cells::misfit`]) FROM
    /// THE BASE of that class, by the same finite-difference direction as
    /// [`Joint::descend`], coarse to the quantum, every step admitted by the
    /// class's own law on that frame ([`validity::Cells::verdict`]; the white
    /// balance also inside the gain budget) and backtracked to the quantum
    /// before the pass ends: the estimate is admissible by construction. The
    /// stages remain the proposal; where it is refused, the law's own asks
    /// are the estimate. One mechanism for every class, no per-class
    /// estimator. Until 2026-11-09 the misfit was minimised unconstrained
    /// from the stages' shape and the result scaled toward the base: a
    /// least-squares fit lets the massive cells outvote one cell's ask, the
    /// refit was refused on the very cell the stages were (the coast's
    /// luma[0.12-0.18] asks +0.23 ln, the refit still darkened it) and the
    /// scaled class earned less than its dof price — nothing shipped
    /// (gates/j104).
    fn ask_fit(&self, p: &Point, class: Refusal) -> Point {
        let ctx = self.ctx;
        let coords: Vec<Coord> = class_coords(class, p).into_iter().filter(|c| self.free(*c)).collect();
        if coords.is_empty() {
            return p.clone();
        }
        let before = self.class_before(p, class);
        let asks = ctx.validity.asks(class.class(), &before);
        // The misfit's own quantum: one code on every asked cell.
        let unit = ctx.validity.misfit_unit(&asks).max(f32::MIN_POSITIVE);
        let withhold = ctx.budget.vetoes == VetoPolicy::Withhold;
        let law = ctx.law;
        // Admitted by the class's law on the asks' frame (and the budget
        // for the white balance's gains); read on the render the misfit
        // needs anyway.
        let admits = |q: &Point, px: &[[f32; 3]]| -> bool {
            let within_budget = class != Refusal::WhiteBalance
                || law.point_of(&q.recipe) == law.point_of(ctx.base)
                || wb_gains_fit_budget(law.gains(law.point_of(&q.recipe)), ctx.budget);
            within_budget && ctx.validity.verdict(&asks, px).admits(withhold)
        };
        let misfit = |q: &Point| -> (f32, bool) {
            let px = self.render(&q.recipe);
            (ctx.validity.misfit(&asks, &px), admits(q, &px))
        };
        let mut m = |q: &Point| -> f32 { misfit(q).0 };
        // From the base of the class: admissible by construction (nothing
        // of the class moved).
        let mut q = p.clone();
        scale_class(&mut q, &coords, 0.0, ctx);
        let mut m0 = m(&q);
        for &steps in &STEP_MULTIPLIERS {
            for _ in 0..ASK_FIT_PASS_CAP {
                let mut d = self.direction(&q, &coords, steps, unit, &mut m, m0);
                let mut accepted = false;
                while !d.is_empty() {
                    let trial = self.apply(&q, &d);
                    if trial.recipe == q.recipe {
                        break;
                    }
                    let (mt, ok) = misfit(&trial);
                    if ok && mt < m0 {
                        q = trial;
                        m0 = mt;
                        accepted = true;
                        break;
                    }
                    match Self::halved(&d) {
                        Some(h) => d = h,
                        None => break,
                    }
                }
                if !accepted {
                    break;
                }
                loop {
                    let again = self.apply(&q, &d);
                    if again.recipe == q.recipe {
                        break;
                    }
                    let (ma, ok) = misfit(&again);
                    if ok && ma < m0 {
                        q = again;
                        m0 = ma;
                    } else {
                        break;
                    }
                }
            }
        }
        q
    }

    /// The descent that moves the coordinates TOGETHER (RULING-1180-joint-r2,
    /// addendum 2026-11-08 23:40): from an admissible `p`, steps along the
    /// finite-difference gradient of J* over the free coordinates
    /// ([`Joint::direction`], one render per coordinate at the level's step),
    /// each accepted when F is lower and G admits, and walked on while it
    /// keeps paying. A step G refuses, or F does not reward, is BACKTRACKED
    /// — halved to the quantum ([`Joint::halved`]) — before anything else:
    /// at the law's edge the gradient step is refused at full length and
    /// admitted shorter (p36's exposure stood at 0.45 where the darkest cell
    /// asks +0.06 ln and exposure alone gives it +0.31; the admissible move
    /// is the exposure up WITH the blacks down, gates/j104). A step still
    /// refused at the quantum on a class moves the OTHER classes by the
    /// gradient and re-estimates that class from its asks in the frame they
    /// leave ([`Joint::ask_fit`]): a class's move in a cell is read between
    /// leave-one-out frames, so another class's step changes what it does
    /// there, and a class that did not move can be refused (canyon gold's
    /// curves, gates/j108); failing that, the class's coordinates leave the
    /// direction and the step goes on with what remains, each refused class
    /// in turn (gates/j110: the white balance pinned by its neutral cell
    /// ended the pass before the tone was ever tried alone). A step F
    /// refuses while J* fell — the dof price of
    /// coordinates leaving the base — is retried on the coordinates already
    /// off the base; a level that accepts nothing ends
    /// ([`STEP_MULTIPLIERS`], 8 → 1). The
    /// coordinate polish ([`Joint::polish`]) follows at every level: a
    /// single coordinate's step was never admissible where two had to move
    /// together (exposure with contrast on the haze pair), which is what this
    /// descent exists for.
    pub(super) fn descend(&self, p: &mut Point, mut best: Eval, multipliers: &[f32]) -> Eval {
        let ctx = self.ctx;
        let trace = crate::config::live_env_os("AUTOSHADE_G_TRACE").is_some();
        let coords: Vec<Coord> = Coord::all(p).into_iter().filter(|c| self.free(*c)).collect();
        for &steps in multipliers {
            for _ in 0..DESCENT_PASS_CAP {
                let j0 = best.j;
                let mut j = |q: &Point| -> f32 { self.eval(q).j };
                let full = self.direction(p, &coords, steps, FIT_QUANT, &mut j, j0);
                let mut d = full.clone();
                let mut accepted = false;
                let (mut dropped, mut restricted): (Vec<Refusal>, bool) = (Vec::new(), false);
                let (mut last, mut last_j): (Option<Refusal>, Option<f32>) = (None, None);
                let mut reestimated = false;
                // The direction with the dropped classes' coordinates out
                // and, once the dof price refused, the coordinates on the
                // base out.
                let remaining = |dropped: &[Refusal], restricted: bool, p: &Point| -> Vec<(Coord, f32)> {
                    let out: Vec<Coord> = dropped.iter().flat_map(|&c| class_coords(c, p)).collect();
                    full.iter()
                        .copied()
                        .filter(|(c, _)| !out.contains(c))
                        .filter(|(c, _)| !restricted || c.get(p, ctx.law) != p.base_value(*c, ctx))
                        .collect()
                };
                while !d.is_empty() {
                    let trial = self.apply(p, &d);
                    if trial.recipe != p.recipe {
                        let e = self.eval(&trial);
                        last = self.refusal(&trial, &e.px);
                        if last.is_none() && e.f() < best.f() {
                            *p = trial;
                            best = e;
                            accepted = true;
                            break;
                        }
                        last_j = Some(e.j);
                        // Shorter first: the step at full length may cross
                        // the law's edge or overshoot the objective.
                        if let Some(h) = Self::halved(&d) {
                            d = h;
                            continue;
                        }
                    }
                    // At the grid, or a step that rounds back onto it: what
                    // the last trial read decides.
                    match last {
                        Some(class) if !dropped.contains(&class) => {
                            dropped.push(class);
                            let others = remaining(&dropped, restricted, p);
                            // The other classes move by the gradient and the
                            // refused class is re-estimated from its asks in
                            // the frame they leave: the curves' move in a
                            // cell is read between leave-one-out frames, so
                            // a tone step changes what the curves do there
                            // (canyon gold: -0.0155 ln against an ask of
                            // +0.30, one code past se, the whole step refused
                            // for a class that had not moved, gates/j108).
                            if !others.is_empty() {
                                let mut moved = self.apply(p, &others);
                                if moved.recipe != p.recipe {
                                    // Re-estimated, or scaled toward the base
                                    // where the re-estimate is worse on F
                                    // (gates/j109).
                                    for refit in [true, false] {
                                        let mut repaired = moved.clone();
                                        self.repair_class(&mut repaired, class, refit);
                                        let e = self.eval(&repaired);
                                        if self.refusal(&repaired, &e.px).is_none() && e.f() < best.f() {
                                            moved = repaired;
                                            reestimated = true;
                                            break;
                                        }
                                    }
                                    if reestimated {
                                        *p = moved;
                                        best = self.eval(p);
                                        accepted = true;
                                        break;
                                    }
                                }
                            }
                            d = others;
                        }
                        Some(_) => break,
                        None if last_j.is_some_and(|lj| lj < j0) && !restricted => {
                            restricted = true;
                            d = remaining(&dropped, restricted, p);
                        }
                        None => break,
                    }
                }
                if trace {
                    eprintln!(
                        "D#{} h={steps} n={}: {}{}{}",
                        self.id,
                        full.len(),
                        if accepted { format!("accepted F {:.5}", best.f()) } else { "nothing".to_string() },
                        if dropped.is_empty() {
                            String::new()
                        } else {
                            format!(
                                ", {} refused{}",
                                dropped.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join("+"),
                                if reestimated { ", the last repaired in the new frame" } else { "" }
                            )
                        },
                        if restricted { ", dof restricted" } else { "" }
                    );
                }
                if !accepted {
                    break;
                }
                if reestimated {
                    continue;
                }
                loop {
                    let again = self.apply(p, &d);
                    if again.recipe == p.recipe {
                        break;
                    }
                    let e = self.eval(&again);
                    if e.f() < best.f() && self.admissible(&again, &e.px) {
                        *p = again;
                        best = e;
                    } else {
                        break;
                    }
                }
            }
        }
        best
    }

    fn free(&self, c: Coord) -> bool {
        !self.pinned.contains(&c)
    }

    /// Coordinate descent on F from `p` (which must be admissible, as
    /// [`Joint::repair`] leaves it): each free coordinate tries `±step`,
    /// keeps a trial that is admissible and strictly lowers F, and then walks
    /// on in that direction one step at a time while each step keeps paying;
    /// a pass that keeps nothing moves to the next multiplier; done at the
    /// quantum. `multipliers` are the steps tried, coarse to fine. The walk
    /// is the same descent with fewer passes: one accepted step per pass had
    /// crawled the quantum level a pass per step, every pass a full round of
    /// every coordinate (14529 renders on an exposure-only pair).
    pub(super) fn polish(&self, p: &mut Point, mut best: Eval, multipliers: &[f32]) -> Eval {
        let ctx = self.ctx;
        let law = ctx.law;
        let coords: Vec<Coord> = Coord::all(p).into_iter().filter(|c| self.free(*c)).collect();
        let mut level = 0usize;
        for _ in 0..POLISH_PASS_CAP {
            let Some(&steps) = multipliers.get(level) else { break };
            let mut accepted = false;
            for &c in &coords {
                let v = c.get(p, law);
                for dir in [1.0f32, -1.0] {
                    let w = c.step(p, v, dir, steps, ctx);
                    if w == v {
                        continue;
                    }
                    let mut trial = p.clone();
                    c.set(&mut trial, w, ctx);
                    if trial.recipe == p.recipe {
                        continue;
                    }
                    let e = self.eval(&trial);
                    if e.f() < best.f() && self.admissible(&trial, &e.px) {
                        *p = trial;
                        best = e;
                        accepted = true;
                        loop {
                            let v = c.get(p, law);
                            let w = c.step(p, v, dir, steps, ctx);
                            if w == v {
                                break;
                            }
                            let mut trial = p.clone();
                            c.set(&mut trial, w, ctx);
                            if trial.recipe == p.recipe {
                                break;
                            }
                            let e = self.eval(&trial);
                            if e.f() < best.f() && self.admissible(&trial, &e.px) {
                                *p = trial;
                                best = e;
                            } else {
                                break;
                            }
                        }
                        break;
                    }
                }
            }
            if !accepted {
                level += 1;
            }
        }
        best
    }

    /// The self-consistency law read whole (RULING-1180-joint-r1 (4)): for
    /// each control off the base, its ABSENCE is the point with the control
    /// at the base and pinned, re-polished at the quantum; the control is
    /// dropped when the absence is no worse on F — a tie goes to the simpler
    /// recipe — unless it is one of the four band sliders and earns on the
    /// luminance band it acts on (R22, the frame a guard). Smallest absence
    /// first, repeated until a pass drops nothing. Returns what was dropped.
    pub(super) fn eliminate(&mut self, p: &mut Point, best: &mut Eval) -> Vec<Control> {
        let ctx = self.ctx;
        let bands = look::luma_bands(ctx.tp, ctx.evidence);
        let reading = |e: &Eval| -> BandReading {
            use rayon::prelude::*;
            let lightness = e.px.par_iter().map(|q| cie_lab(q)[0]).collect::<Vec<_>>();
            (e.j, bands.paired_readings(&e.px, ctx.tp, ctx.evidence), lightness)
        };
        let mut dropped = Vec::new();
        // The solved point's reading, taken when the point moves, not per
        // candidate.
        let mut solved = reading(best);
        loop {
            let candidates: Vec<Control> =
                global_controls().into_iter().filter(|c| c.differs(&p.recipe, ctx.base)).collect();
            let mut absences: Vec<(f32, Control, Point, Eval)> = Vec::new();
            for c in candidates {
                let mut without = p.clone();
                for coord in Coord::of_control(c, p) {
                    let v = without.base_value(coord, ctx);
                    coord.set(&mut without, v, ctx);
                }
                c.reset(&mut without.recipe, ctx.base);
                let mut sub = Joint::with_pinned(ctx, self.pinned.clone());
                sub.pinned.extend(Coord::of_control(c, p));
                let start = sub.eval(&without);
                let e = if sub.admissible(&without, &start.px) {
                    sub.polish(&mut without, start, &STEP_MULTIPLIERS[3..])
                } else {
                    start
                };
                self.absorb(&sub);
                absences.push((e.f(), c, without, e));
            }
            absences.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut changed = false;
            for (_, c, without, e) in absences {
                if changed {
                    // The pass's absences were read on the state they judge;
                    // after a drop the state moved, so this one is read again.
                    if !c.differs(&p.recipe, ctx.base) {
                        continue;
                    }
                    let mut again = p.clone();
                    c.reset(&mut again.recipe, ctx.base);
                    for coord in Coord::of_control(c, p) {
                        let v = again.base_value(coord, ctx);
                        coord.set(&mut again, v, ctx);
                    }
                    let mut sub = Joint::with_pinned(ctx, self.pinned.clone());
                    sub.pinned.extend(Coord::of_control(c, p));
                    let start = sub.eval(&again);
                    let e = if sub.admissible(&again, &start.px) {
                        sub.polish(&mut again, start, &STEP_MULTIPLIERS[3..])
                    } else {
                        start
                    };
                    self.absorb(&sub);
                    if self.keeps(c, &bands, &solved, &reading(&e), best, &e) {
                        continue;
                    }
                    *p = again;
                    *best = e;
                } else {
                    if self.keeps(c, &bands, &solved, &reading(&e), best, &e) {
                        continue;
                    }
                    *p = without;
                    *best = e;
                }
                solved = reading(best);
                self.pinned.extend(Coord::of_control(c, p));
                dropped.push(c);
                changed = true;
            }
            if !changed {
                return dropped;
            }
        }
    }

    /// Is `c` kept against its absence? On F, a tie to the absence; the four
    /// band sliders may also keep by R22's band clause.
    fn keeps(
        &self,
        c: Control,
        bands: &look::LumaBands,
        solved: &BandReading,
        without: &BandReading,
        best: &Eval,
        absence: &Eval,
    ) -> bool {
        if best.f() < absence.f() {
            return true;
        }
        matches!(c, Control::Whites | Control::Highlights | Control::Shadows | Control::Blacks)
            && earns_on_its_band(c, bands, solved, without)
    }

}
