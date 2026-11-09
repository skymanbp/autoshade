//! Look error, with and without evidence, and the calibration corpus and recipe the tests read.

use super::*;
use rayon::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;


/// THE look error every fit decision reads, J* (v1.18.0), in the units the
/// eye uses: 1 dE76 over the whole paired frame is 0.0075.
///
/// `J* = 0.75·[p·E_paired + (1 − p)·E_dist]`
///
/// * `E_paired` — where pixels PAIR (index i on one side is index i on the
///   other: [`EvidenceModel::paired`]), the presence-weighted mean CIE76
///   distance between the two sides' pixels, read PER PIXEL:
///   `Σ w_i·ΔE76(Lab(a_i), Lab(b_i)) / 100 / Σ w_i`, `w` the pixel's
///   PRESENCE weight (its membership, DEF-5).
///   Until 2026-11-07 the two sides were pooled at σ 2 px first (the
///   structural instrument's scale) and read as local means. On the
///   finished p39 and p36 frames that reading preferred the frame the
///   per-pixel reading and the eye both put farther from the target (2.71
///   vs 2.60 and 2.10 vs 1.97 dE76/px on the fit plate): a recipe can trade
///   fine-scale deviation the pool cannot see for a lower pooled mean
///   (RULING-1180-joint-r2, addendum 2026-11-07).
///   Who pairs is the evidence model's one reading
///   ([`EvidenceModel::paired`]: present and spatially supported), on every
///   ruler. Until 2026-11-07 the finest mode read decided it (R2): an
///   Atmosphere frame or zone emptied the pairing and was scored as two
///   distributions, a ruler the terminal re-read and the floor never take —
///   on p41 it read the sky correction the per-pixel ruler wanted as harm.
/// * `E_dist = sqrt(T_L² + T_a² + T_b²) / 100` — where pixels do not pair
///   (the pixels the structural reading left unsupported — a replaced
///   sky), the CIE76 distance between the
///   two populations' quantile-matched Lab distributions: per channel the
///   mean absolute gap of the two weighted CDFs at the same 21 quantiles
///   ([`P_CLIP`]-clamped) `T_L` reads on L* ([`tonal_err_with_evidence`]),
///   on L*, a* and b* each, in that channel's own units. A uniform offset of
///   d dE76 moves every quantile by d, so it reads d/100 here exactly as it
///   reads d/100 in `E_paired`, paired or not (v1.18.0 R15). The form it
///   replaces, `0.55·T_L + 0.20·C_ab` lifted by 1/0.75, read one dE of mean
///   colour offset as 0.001 of J* against `E_paired`'s 0.0075, so on a frame
///   where nothing pairs a white balance that moved 97 % of the frame 3-5
///   dE/px closer earned less than [`FIT_QUANT`] and was eliminated
///   (rel1180 msg7, R12's terms).
/// * `p` — the paired share of the evidence mass TIMES the pairing's
///   RELIABILITY `r` (2026-11-08, RULING-1180-joint-r2 addendum 22:30): a
///   paired pixel enters `E_paired` at `w·r` and `E_dist` at `w·(1 − r)`, an
///   unpaired pixel at `w` in `E_dist`. `r` is one scalar per population —
///   the presence-weighted Pearson ρ² of L* between the base and the target
///   over the population's paired pixels, the share of the target's tonal
///   spread the pairing explains ([`pairing_reliability`]). On a tight
///   pairing r ≈ 1 and this is the per-pixel ruler; on a loosely paired
///   population (the texture re-synthesised, the layout kept: 0 < D under
///   the pairing line) the per-pixel optimum is the squeezed conditional
///   mean (errors in variables: the toy zone's paired map held 0.58 of the
///   target's spread at ρ² 0.30), so the ruler reads that population as
///   partly unpaired and the distribution term carries the rest. The two
///   populations are read in one unit, so the 0.75 weighs their
///   mass-weighted mean.
///
/// The 0.75 is J*'s UNIT: it was the colour distance's weight while the
/// objective also scored a band colour offset (0.15·H_ab) and a structural
/// divergence (0.10·S), and every J*-denominated constant — [`FIT_QUANT`],
/// the confidence family, the mode margins — was derived in it, so it stays
/// as the scale and nothing else. HUE AND STRUCTURE ARE GATES, NOT TERMS
/// (2026-11-07): on the finished p36 frames the two terms bought 0.0036 of
/// J* while `E_paired` rose, and the eye sided with `E_paired`. Hue is
/// judged by the validity law (`fit::validity`: G of the joint objective,
/// each class of controls on the cells it moves), structure by the D thresholds (`DIVERGENCE_GLOBAL`
/// / `DIVERGENCE_ZONE`) and the seam / ramp rulers; neither is scored, and a
/// LOCAL correction's do-no-harm reads the same objective as the frame.
///
/// WEIGHTS (v1.18.0, R11): the objective JUDGES every present pixel at its
/// presence weight — its membership, 1 for a present frame pixel
/// ([`EvidenceModel::source_presence_weights`]; DEF-5: presence is
/// membership, spatial support only decides who pairs) — in every term:
/// `E_paired`, `E_dist`'s `T_L`, `T_a` and `T_b`, and the paired share `p`.
/// The evidence model's per-range estimation weights only DRIVE (which
/// ranges may move a control's estimate); a ship decision read at them is
/// blind to whatever range the evidence withheld, and a tie there goes to
/// absence however much the photo gained (rel1180 msg6: p41's cloud-edge
/// band, +4-5.7 dE/px at evidence weight 0.000-0.21).
///
/// LOCAL SHIP LAW (R19): a LOCAL correction ships when it earns FIT_QUANT on
/// its OWN population — this objective read over the region's own present
/// pixels ([`EvidenceModel::scoped`]) — and the frame reading is a guard
/// only: the correction may not raise the frame's J* by more than FIT_QUANT
/// ([`crate::fit::ships_local`]). Read on the frame, a region's gain is its
/// own gain times its share, and R1's FIT_QUANT then asked a 6 % tile for
/// 17x FIT_QUANT on its own pixels (rel1180 msg8: 57 + 12 refusals of
/// corrections both rulers call better).
///
/// Before v1.18.0 the objective was `0.55·T + 0.20·C + 0.15·H + 0.10·S` in
/// display sRGB: a frame-level distribution distance that cannot credit a
/// local correction for moving its own pixels (21 of the census's 39
/// disagreements), in which the night sky's brightness demand carried 67×
/// the energy of its colour demand.
pub(crate) fn look_err_with_evidence(
    a: &[[f32; 3]],
    b: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> f32 {
    // DEF-4: the "unidentifiable" sentinel is a statement about the
    // POPULATION, not about estimation: it is returned only when no pixel is
    // present on a side (presence mass 0), so there is nothing to judge. Low
    // estimation identifiability (every range withheld as evidence) says the
    // controls cannot be ESTIMATED from these pixels; it is not a verdict
    // that present pixels look alike, and read as one it scored a source and
    // a render whose ground IS the target both 1.0, so the tie went to
    // absence (R11: estimation weights only drive). Identifiability stays
    // with the estimation.
    let mass = |w: &[f32]| w.iter().map(|v| v.max(0.0) as f64).sum::<f64>();
    if mass(&evidence.source_presence_weights) <= 0.0 || mass(&evidence.target_presence_weights) <= 0.0 {
        return 1.0;
    }
    look_terms(a, b, evidence).total()
}

/// The terms of [`look_err_with_evidence`] (J*), each in its own unit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LookTerms {
    /// `E_paired`, dE76 / 100 over the paired population.
    pub paired: f32,
    /// `E_dist`, the Lab quantile-distribution dE76 / 100 over the unpaired
    /// population.
    pub dist: f32,
    /// `p`, the paired share of the evidence mass times the pairing's
    /// reliability.
    pub share: f32,
    /// `r`, the pairing's reliability (Pearson ρ² of L* over the paired
    /// pixels; 1 where the target has no tonal spread to explain).
    pub reliability: f32,
}

impl LookTerms {
    /// J*.
    pub(crate) fn total(&self) -> f32 {
        0.75 * (self.share * self.paired + (1.0 - self.share) * self.dist)
    }
}

/// [`look_err_with_evidence`]'s terms, read once.
pub(crate) fn look_terms(a: &[[f32; 3]], b: &[[f32; 3]], evidence: &EvidenceModel) -> LookTerms {
    let n = a.len().min(b.len());
    let target = TargetLook::prepared(b, n, evidence);
    let weight = |i: usize| evidence.source_presence_weights.get(i).copied().unwrap_or(0.0).max(0.0);
    let pairs = |i: usize| evidence.paired.get(i).copied().unwrap_or(false);
    let share = target.share;
    let reliability = target.reliability;
    let la = lab_of(&a[..n]);
    let paired = if target.paired_mass > 0.0 {
        let mut sum = 0.0f64;
        for i in (0..n).filter(|&i| pairs(i)) {
            let wi = weight(i) as f64;
            if wi <= 0.0 {
                continue;
            }
            let (x, y) = (la[i], target.lab[i]);
            let de = ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt();
            sum += wi * de as f64;
        }
        (sum / target.paired_mass / 100.0) as f32
    } else {
        0.0
    };
    let dist = if share < 1.0 {
        let sw = dist_weights(&evidence.source_presence_weights, n, evidence, reliability);
        let tonal = cdf_gap(&weighted_cdf(&la, &sw, |p| p[0] / 100.0), &target.cdf_l);
        let (ta, tb) = (target.opponent_cdf_gap(&la, &sw, 1), target.opponent_cdf_gap(&la, &sw, 2));
        (tonal * tonal + ta * ta + tb * tb).sqrt()
    } else {
        0.0
    };
    LookTerms { paired, dist, share, reliability }
}

/// CIE Lab of every pixel, converted in parallel (each pixel on its own, so
/// the same values as one after the other).
pub(super) fn lab_of(px: &[[f32; 3]]) -> Vec<[f32; 3]> {
    px.par_iter().map(cie_lab).collect()
}

/// A side's presence weights as the population `E_dist` reads: a paired
/// pixel at `w·(1 − r)`, an unpaired one at `w`.
fn dist_weights(weights: &[f32], n: usize, evidence: &EvidenceModel, reliability: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let w = weights.get(i).copied().unwrap_or(0.0);
            if evidence.paired.get(i).copied().unwrap_or(false) { w * (1.0 - reliability) } else { w }
        })
        .collect()
}

/// The pairing's RELIABILITY `r`: the presence-weighted Pearson ρ² of L*
/// between the base (`sp`, the evidence model's own source pixels) and the
/// target over the population's paired pixels — the share of the target's
/// tonal spread the pairing explains. A target with no spread over those
/// pixels leaves nothing to explain (r = 1); a flat base explains none of a
/// spread target (r = 0); no paired mass reads 0 (p is 0 there anyway).
pub(super) fn pairing_reliability(
    sp: &[[f32; 3]],
    target_lab: &[[f32; 3]],
    n: usize,
    weight: &dyn Fn(usize) -> f32,
    pairs: &dyn Fn(usize) -> bool,
) -> f32 {
    let mut samples: Vec<(f64, f64, f64)> = Vec::new();
    let (mut m, mut sx, mut sy) = (0.0f64, 0.0f64, 0.0f64);
    for (i, (p, t)) in sp.iter().zip(target_lab).enumerate().take(n) {
        if !pairs(i) {
            continue;
        }
        let w = weight(i) as f64;
        if w <= 0.0 {
            continue;
        }
        let (x, y) = (cie_lab(p)[0] as f64, t[0] as f64);
        samples.push((x, y, w));
        m += w;
        sx += w * x;
        sy += w * y;
    }
    if m <= 0.0 {
        return 0.0;
    }
    let (mx, my) = (sx / m, sy / m);
    let (mut vx, mut vy, mut cov) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y, w) in samples {
        vx += w * (x - mx) * (x - mx);
        vy += w * (y - my) * (y - my);
        cov += w * (x - mx) * (y - my);
    }
    if vy <= 0.0 {
        return 1.0;
    }
    if vx <= 0.0 {
        return 0.0;
    }
    ((cov * cov) / (vx * vy)).clamp(0.0, 1.0) as f32
}

/// The mean absolute gap between two CDFs at 21 quantiles (each clamped to
/// [`P_CLIP`]).
fn cdf_gap(ca: &[f32], cb: &[f32]) -> f32 {
    let mut gap = 0.0f32;
    for i in 0..=20 {
        let q = (i as f32 / 20.0).clamp(P_CLIP, 1.0 - P_CLIP);
        gap += (quantile(ca, q) - quantile(cb, q)).abs();
    }
    gap / 21.0
}

/// The TARGET's side of [`look_terms`], prepared once per (target, evidence)
/// and re-read from a thread-local slot. A solve reads one target against
/// thousands of renders; every quantity here depends on the target raster and
/// the evidence alone and is computed exactly as the one-pass form computed
/// it, so a prepared read is the unprepared read bit for bit.
struct TargetLook {
    key: (usize, u64),
    /// `p`, the paired share of the evidence mass times the pairing's
    /// reliability `r`; the paired mass; and `r` itself.
    share: f32,
    paired_mass: f64,
    reliability: f32,
    /// Lab of the target as it is; its distribution weights (a paired pixel
    /// at `w·(1 − r)`, an unpaired one at `w`) and its L*/100 CDF over them
    /// (empty while everything pairs at r = 1).
    lab: Vec<[f32; 3]>,
    unpaired: Vec<f32>,
    cdf_l: Vec<f32>,
    /// Per Lab channel, the target's extremes over its distribution-weighted
    /// present pixels (`(INFINITY, NEG_INFINITY)` where none is present).
    extremes: [(f32, f32); 3],
    /// Per channel, the target's opponent CDF at the span it was last read
    /// on: the span is set by both sides, so the CDF is re-cut when it moves.
    opponent: RefCell<[Option<OpponentCdf>; 3]>,
}

struct OpponentCdf {
    span: (f32, f32),
    cdf: Vec<f32>,
}

thread_local! {
    static TARGET_LOOKS: RefCell<Vec<Rc<TargetLook>>> = const { RefCell::new(Vec::new()) };
}

/// Prepared target sides kept per thread: the frame's read and a band
/// reading of the same frame alternate, and nothing else does.
const TARGET_LOOK_SLOTS: usize = 2;

impl TargetLook {
    fn prepared(b: &[[f32; 3]], n: usize, evidence: &EvidenceModel) -> Rc<Self> {
        let words = b[..n]
            .iter()
            .flatten()
            .map(|v| v.to_bits())
            .chain(evidence.source_pixels.iter().flatten().map(|v| v.to_bits()))
            .chain(evidence.source_presence_weights.iter().map(|v| v.to_bits()))
            .chain(evidence.target_presence_weights.iter().map(|v| v.to_bits()))
            .chain(evidence.paired.iter().map(|&p| p as u32))
            .chain([evidence.width, evidence.height]);
        let key = (n, structure::fingerprint(words));
        if let Some(hit) = TARGET_LOOKS.with(|slots| slots.borrow().iter().find(|t| t.key == key).cloned()) {
            return hit;
        }
        let built = Rc::new(Self::build(b, n, evidence, key));
        TARGET_LOOKS.with(|slots| {
            let mut slots = slots.borrow_mut();
            slots.insert(0, built.clone());
            slots.truncate(TARGET_LOOK_SLOTS);
        });
        built
    }

    fn build(b: &[[f32; 3]], n: usize, evidence: &EvidenceModel, key: (usize, u64)) -> Self {
        let weight = |i: usize| evidence.source_presence_weights.get(i).copied().unwrap_or(0.0).max(0.0);
        let pairs = |i: usize| evidence.paired.get(i).copied().unwrap_or(false);
        let (mut mass, mut paired_mass) = (0.0f64, 0.0f64);
        for i in 0..n {
            mass += weight(i) as f64;
            if pairs(i) {
                paired_mass += weight(i) as f64;
            }
        }
        let lab = lab_of(&b[..n]);
        let reliability = pairing_reliability(&evidence.source_pixels, &lab, n, &weight, &pairs);
        let share = if mass > 0.0 { (paired_mass / mass) as f32 * reliability } else { 0.0 };
        let (unpaired, cdf_l, extremes) = if share < 1.0 {
            let unpaired = dist_weights(&evidence.target_presence_weights, n, evidence, reliability);
            let cdf_l = weighted_cdf(&lab, &unpaired, |p| p[0] / 100.0);
            let mut extremes = [(f32::INFINITY, f32::NEG_INFINITY); 3];
            for (p, &wi) in lab.iter().zip(&unpaired) {
                if wi > 0.0 {
                    for (ch, (lo, hi)) in extremes.iter_mut().enumerate() {
                        *lo = lo.min(p[ch]);
                        *hi = hi.max(p[ch]);
                    }
                }
            }
            (unpaired, cdf_l, extremes)
        } else {
            (Vec::new(), Vec::new(), [(f32::INFINITY, f32::NEG_INFINITY); 3])
        };
        TargetLook {
            key,
            share,
            paired_mass,
            reliability,
            lab,
            unpaired,
            cdf_l,
            extremes,
            opponent: RefCell::new([None, None, None]),
        }
    }

    /// The 21-quantile gap of the opponent channel `ch` (1 = a*, 2 = b*) over
    /// 100, as `cdf_gap` reads L*. a* and b* are signed, so the channel is
    /// mapped onto the CDF's [0, 1] by the span both populations' present
    /// pixels occupy and the gap is carried back by that span: the mapping is
    /// exact, it sets only the bins' width, and it adds no number.
    fn opponent_cdf_gap(&self, la: &[[f32; 3]], wa: &[f32], ch: usize) -> f32 {
        let (mut lo, mut hi) = self.extremes[ch];
        for (p, &wi) in la.iter().zip(wa) {
            if wi > 0.0 {
                lo = lo.min(p[ch]);
                hi = hi.max(p[ch]);
            }
        }
        let span = hi - lo;
        if !span.is_finite() || span <= 1e-6 {
            return 0.0;
        }
        let value = |p: &[f32; 3]| (p[ch] - lo) / span;
        let ca = weighted_cdf(la, wa, value);
        let mut slot = self.opponent.borrow_mut();
        let cut = match &slot[ch] {
            Some(c) if c.span == (lo, hi) => c,
            _ => {
                slot[ch] = Some(OpponentCdf { span: (lo, hi), cdf: weighted_cdf(&self.lab, &self.unpaired, value) });
                slot[ch].as_ref().expect("just cut")
            }
        };
        cdf_gap(&ca, &cut.cdf) * span / 100.0
    }
}

/// The number of luminance bands a tone slider's own population is read on
/// (R22): the intervals between `E_dist`'s 21 quantiles, so each band holds
/// one twentieth of the paired presence mass. No new number.
pub(crate) const LUMA_BANDS: usize = 20;

/// R22 (a): the [`LUMA_BANDS`] luminance bands of the TARGET's paired present
/// pixels — `E_dist`'s quantile bins, cut on the target's L* so the population
/// is the same for every recipe compared — and each member's presence weight.
pub(crate) struct LumaBands {
    band: Vec<Option<usize>>,
    weight: Vec<f64>,
}

pub(crate) fn luma_bands(target: &[[f32; 3]], evidence: &EvidenceModel) -> LumaBands {
    let n = target.len();
    let weight = (0..n)
        .map(|i| {
            let paired = evidence.paired.get(i).copied().unwrap_or(false);
            let w = evidence.source_presence_weights.get(i).copied().unwrap_or(0.0).max(0.0) as f64;
            if paired { w } else { 0.0 }
        })
        .collect::<Vec<_>>();
    let mut members: Vec<(f32, usize)> =
        (0..n).filter(|&i| weight[i] > 0.0).map(|i| (cie_lab(&target[i])[0], i)).collect();
    members.sort_by(|x, y| x.0.total_cmp(&y.0));
    let total: f64 = members.iter().map(|&(_, i)| weight[i]).sum();
    let mut band = vec![None; n];
    let mut below = 0.0f64;
    for &(_, i) in &members {
        band[i] = Some((((below + 0.5 * weight[i]) / total * LUMA_BANDS as f64) as usize).min(LUMA_BANDS - 1));
        below += weight[i];
    }
    LumaBands { band, weight }
}

impl LumaBands {
    /// `E_paired` read band by band, in J*'s own unit (`0.75 · E_paired` of a
    /// population that pairs whole). Empty when nothing pairs.
    pub(crate) fn paired_readings(&self, a: &[[f32; 3]], b: &[[f32; 3]], evidence: &EvidenceModel) -> Vec<f32> {
        let n = a.len().min(b.len()).min(self.band.len());
        if self.band[..n].iter().all(Option::is_none) {
            return Vec::new();
        }
        let target = TargetLook::prepared(b, n, evidence);
        let la = lab_of(&a[..n]);
        let lb = &target.lab;
        let (mut sum, mut mass) = (vec![0.0f64; LUMA_BANDS], vec![0.0f64; LUMA_BANDS]);
        for i in 0..n {
            let Some(k) = self.band[i] else { continue };
            let (x, y) = (la[i], lb[i]);
            let de = ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt();
            sum[k] += self.weight[i] * de as f64;
            mass[k] += self.weight[i];
        }
        (0..LUMA_BANDS).map(|k| if mass[k] > 0.0 { (0.75 * sum[k] / mass[k] / 100.0) as f32 } else { 0.0 }).collect()
    }
    /// R22 (a): the band a tone slider ACTS on, set by its own response before
    /// any error is read — the band whose members its move shifts most in L*
    /// (presence-weighted mean |ΔL*| between the render with it and the render
    /// without it). Not the band where its removal costs most: a control that
    /// may earn on any of twenty bands has twenty chances where the frame has
    /// one. `None` when the move shifts no paired pixel.
    pub(crate) fn acting_band(&self, with_l: &[f32], without_l: &[f32]) -> Option<usize> {
        let n = with_l.len().min(without_l.len()).min(self.band.len());
        let (mut sum, mut mass) = (vec![0.0f64; LUMA_BANDS], vec![0.0f64; LUMA_BANDS]);
        for i in 0..n {
            let Some(k) = self.band[i] else { continue };
            sum[k] += self.weight[i] * (with_l[i] - without_l[i]).abs() as f64;
            mass[k] += self.weight[i];
        }
        let response = (0..LUMA_BANDS).map(|k| if mass[k] > 0.0 { sum[k] / mass[k] } else { 0.0 }).collect::<Vec<_>>();
        let best = (0..LUMA_BANDS).max_by(|a, b| response[*a].total_cmp(&response[*b]))?;
        (response[best] > 0.0).then_some(best)
    }
}

pub(super) fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}
pub(super) fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// The OPTIONAL structural-divergence calibration corpus, located exactly the
/// way `scripts/check_docs.py` locates the XMP census (`AUTOSHADE_CENSUS_ROOT`):
/// through an environment variable, never a source literal. The corpus is a
/// photographer's own RAW and its generative rendition, so it cannot live in
/// this public repository — and a machine-specific path baked into a test would
/// publish a home directory, a develop-store id and a photo's filename along
/// with it. With the variable unset the fixtures still assert the SYNTHETIC
/// pairs; only the measured real-pair numbers go unpinned.
///
/// Expected contents, under canonical names so no corpus filename reaches the
/// source either:
/// * `neutral.jpg` — calibration-only render of the source frame,
/// * `target.jpg` — the generated rendition being fitted,
/// * `fitted.recipe.json` — the saved zoned develop of that pair,
/// * `sky-mask.png` — the sky raster that develop references,
/// * `source.arw` — optional; the RAW behind `neutral.jpg`.
#[cfg(test)]
pub(crate) fn calibration_corpus() -> Option<std::path::PathBuf> {
    let dir =
        std::path::PathBuf::from(crate::config::live_env_os("AUTOSHADE_FIT_CALIBRATION_DIR")?);
    let required = ["neutral.jpg", "target.jpg", "fitted.recipe.json", "sky-mask.png"];
    if !dir.is_dir() || required.iter().any(|name| !dir.join(name).is_file()) {
        crate::test_skipped(
            "calibration test",
            &format!(
                "incomplete corpus at {} (need neutral.jpg, target.jpg, fitted.recipe.json, sky-mask.png)",
                dir.display()
            ),
        );
        return None;
    }
    Some(dir)
}

/// The corpus's saved zoned develop, `fitted.recipe.json`, with its raster
/// references resolved INTO the corpus. The app wrote that file with the
/// absolute path its sky raster had on the machine that produced it, and the
/// corpus has moved since (from the repository's own target directory, under
/// the repository's old name, to a fixtures directory) — a test that parsed
/// the JSON itself rendered a develop whose raster no longer existed and died
/// on it. The contract above is that the raster lives BESIDE the recipe under
/// its canonical name, so a reference whose file name is present in the
/// corpus resolves there; one that is not stays as written and fails where it
/// always did. Every corpus test that reads the saved develop comes through
/// here, which is what makes the corpus relocatable as a directory.
#[cfg(test)]
pub(crate) fn calibration_recipe(root: &std::path::Path) -> crate::recipe::EditRecipe {
    let text = std::fs::read_to_string(root.join("fitted.recipe.json"))
        .expect("calibration fitted.recipe.json");
    let mut recipe: crate::recipe::EditRecipe =
        serde_json::from_str(&text).expect("saved calibration recipe");
    for mask in &mut recipe.masks {
        for path in mask.bitmap_paths_mut() {
            let Some(name) = std::path::Path::new(path.as_str()).file_name() else { continue };
            let local = root.join(name);
            if local.is_file() {
                *path = local.to_string_lossy().into_owned();
            }
        }
    }
    recipe
}

// --------------------------------------------------------------------------
