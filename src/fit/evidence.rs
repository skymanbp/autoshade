//! The evidence model: fit modes, divergence, the luma and hue ranges, their aggregation and withholding, and the atmosphere white-balance pairing.

use super::*;

/// The global reverse-fit policy selected before any CDF solve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitMode {
    Full,
    Atmosphere,
}

/// The two calibrated components of structural divergence and their Euclidean
/// combination `d = sqrt((1-correlation)^2 + energy_error^2)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Divergence {
    pub correlation: f32,
    pub energy_error: f32,
    pub d: f32,
}

/// Evidence carried by one value range.  A range is identifiable only when
/// both images populate it and its own structural comparison is stable.
#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceRange {
    pub label: String,
    pub source_share: f32,
    pub target_share: f32,
    pub source_evidence_share: f32,
    pub target_evidence_share: f32,
    pub two_sided_share: f32,
    pub divergence: f32,
    pub weight: f32,
    pub source_populated: bool,
    pub target_populated: bool,
}

/// One evidence map is shared by the objective, all gates, confidence and
/// disclosures.  Pixel weights are attached to the source/target analysis
/// rasters; candidates keep the source weights, so edits cannot move pixels
/// into evidence after the fact.
#[derive(Clone, Debug)]
pub struct EvidenceModel {
    pub source_pixels: Vec<[f32; 3]>,
    /// Soft membership of each aligned source pixel in this model's
    /// population: all ones for the frame, the source coverage for a scoped
    /// view. Blind-move numerators use the same mass as [`Self::population`].
    pub source_membership: Vec<f32>,
    pub width: u32,
    pub height: u32,
    pub spatial_supported: Vec<bool>,
    /// Does pixel `i` PAIR — is index `i` on one side the same scene point as
    /// index `i` on the other? The population the objective's `E_paired`
    /// reads (`look_err_with_evidence`): the present, spatially supported
    /// pixels of a model built on an aligned pair. ONE reading for every
    /// ruler (2026-11-07): the structure-blind re-aggregation keeps it
    /// ([`Self::structure_blind`]) and the zoned entry re-pairs nothing by
    /// mode, so an Atmosphere frame or zone scores the pixels the structural
    /// reading pairs per pixel, like every other reading of the frame.
    pub paired: Vec<bool>,
    pub source_weights: Vec<f32>,
    pub target_weights: Vec<f32>,
    pub source_hue_weights: Vec<f32>,
    pub target_hue_weights: Vec<f32>,
    /// PRESENCE weights (v1.18.0, R11; DEF-5): each aligned pixel's
    /// membership in this model's population — 1 for a present frame pixel,
    /// the coverage for a scoped view, 0 for an absent one. Presence is
    /// membership: spatial support is an estimation and pairing input (it
    /// decides who pairs, R2b, and the cells' trust), never a judge weight —
    /// read as one, a structurally replaced region weighed ~0 and a
    /// correction of its pixels could not earn. The objective
    /// JUDGES every present pixel at its presence weight; the estimation
    /// weights above (`source_weights` … `target_hue_weights`, which also
    /// carry each range's evidence verdict) only DRIVE: they decide which
    /// ranges may move a control's estimate (the cells, the band estimator,
    /// `P_CLIP`, `EVIDENCE_MIN_SHARE`), never which pixels a ship decision
    /// sees. A range withheld from estimation is still part of the photo:
    /// judging at the estimation weights made a 4-5.7 dE/px gain on bright
    /// cloud edges invisible (their L* 50-90 ranges weigh 0.000-0.21 as
    /// evidence) and gave the tie to absence (rel1180 msg6, R9).
    pub source_presence_weights: Vec<f32>,
    pub target_presence_weights: Vec<f32>,
    pub luma: Vec<EvidenceRange>,
    pub hue: Vec<EvidenceRange>,
    /// Population chromaticity facts used to recognize a coherent global cast.
    pub source_chroma_vector: [f32; 2],
    pub target_chroma_vector: [f32; 2],
    pub global_cast: Option<GlobalCast>,
    pub identifiability: f32,
    /// Per-pixel ingredients of the range verdicts above — the pixel's
    /// spatial-cell confidence, that cell's divergence and the frame-wide
    /// same-content verdict — kept so [`EvidenceModel::scoped`] can
    /// re-aggregate the same support over a zone's own population.
    pub spatial_weights: Vec<f32>,
    pub spatial_divergence: Vec<f32>,
    pub globally_same_content: bool,
    /// Weighted member count of the population these verdicts are over: the
    /// frame's pixel count for the global model, a coverage's mass for a
    /// scoped view. The blind-move audit's region line is a share of THIS,
    /// so a half-withheld 6% tile is a region of its own population, not a
    /// 3% speckle of the frame.
    pub population: f32,
    /// Where the paired target HAS a pixel ([`super::presence_of`]): `None`
    /// when it has one everywhere, else 1.0 / 0.0 per aligned pixel. A target
    /// brought into the sensor frame leaves the sensor pixels outside its
    /// field with nothing to pair against, and an absent pixel is not
    /// evidence of anything — not support, not withholding — so it is a
    /// member of no population built from this model, the frame's or a
    /// zone's.
    pub presence: Option<Vec<f32>>,
}

impl EvidenceModel {
    /// Does pixel `i` have no target counterpart?
    pub fn is_absent(&self, i: usize) -> bool {
        self.presence.as_ref().is_some_and(|p| p.get(i).copied().unwrap_or(0.0) <= 0.0)
    }

    /// `weights` with every absent pixel's weight taken to zero — THE
    /// population rule for a zone's pair weights. Borrowed when every pixel
    /// is present, so a pair with nothing absent runs on its own bytes.
    pub fn present_weights<'w>(&self, weights: &'w [f32]) -> std::borrow::Cow<'w, [f32]> {
        super::with_presence(weights, self.presence.as_deref())
    }

    /// This model's range verdicts re-aggregated over ONE zone: `source_zone`
    /// / `target_zone` are the zone's soft memberships on the analysis
    /// rasters, `tp` the target's analysis pixels. Evidence verdicts follow
    /// the population a correction MOVES — the frame for the global fit, the
    /// zone for a zone — so a ground zone is no longer withheld because a
    /// replaced sky happens to share its luma bins, while a zone whose own
    /// members are divergent stays withheld unless the frame-wide
    /// same-content verdict holds. That verdict deliberately bypasses range
    /// survival and per-pixel withholding. Per-pixel support
    /// (`spatial_supported`) is unchanged; over the whole aligned frame this
    /// is the model itself. The two rasters share one geometry by
    /// construction ([`analysis_pair`]); the aligned-prefix arithmetic below
    /// is the defensive form of that contract.
    pub fn scoped(&self, tp: &[[f32; 3]], source_zone: &[f32], target_zone: &[f32]) -> EvidenceModel {
        let (source_zone, target_zone) = (self.present_weights(source_zone), self.present_weights(target_zone));
        let ranges = aggregate_ranges(
            &self.source_pixels,
            tp,
            &source_zone,
            &target_zone,
            SupportField {
                spatial_weights: &self.spatial_weights,
                spatial_divergence: &self.spatial_divergence,
                globally_same_content: self.globally_same_content,
            },
        );
        EvidenceModel {
            source_pixels: self.source_pixels.clone(),
            source_membership: ranges.source_membership,
            width: self.width,
            height: self.height,
            spatial_supported: self.spatial_supported.clone(),
            paired: self.paired.clone(),
            source_weights: ranges.source_weights,
            target_weights: ranges.target_weights,
            source_hue_weights: ranges.source_hue_weights,
            target_hue_weights: ranges.target_hue_weights,
            source_presence_weights: ranges.source_presence_weights,
            target_presence_weights: ranges.target_presence_weights,
            luma: ranges.luma,
            hue: ranges.hue,
            source_chroma_vector: self.source_chroma_vector,
            target_chroma_vector: self.target_chroma_vector,
            global_cast: self.global_cast,
            identifiability: ranges.identifiability,
            spatial_weights: self.spatial_weights.clone(),
            spatial_divergence: self.spatial_divergence.clone(),
            globally_same_content: self.globally_same_content,
            population: ranges.population,
            presence: self.presence.clone(),
        }
    }

    /// Re-aggregate this frame on Atmosphere mode's structure-blind evidence
    /// doctrine. Atmosphere mode is entered because structure diverges; its
    /// instruments are the budgets (EV +/-1, WB gain [0.80, 1.25], saturation
    /// +/-30, curve slope [0.5, 1.5]) and the population facts, not structural
    /// survival. Population vetoes remain intact because [`Self::scoped`]
    /// re-derives them from the unchanged source and target pixels.
    ///
    /// On a model that is already blind — every spatial weight 1.0, every
    /// pixel supported, `globally_same_content` true, as an identical pair
    /// produces — this is byte-equal to `scoped(tp, ones, ones)`, and the
    /// unit test pins that. Anywhere else the two differ by design: `scoped` reads
    /// the model's own structural weights and support, and replacing those is
    /// this function's whole job. The pairing ([`Self::paired`]) is kept:
    /// blindness is the estimation weights and support, not the score. Until
    /// 2026-11-07 a blind model emptied it, so every Atmosphere reading — the
    /// joint descent, the zones' acceptance, the rescore — scored the frame
    /// as two distributions while the terminal re-read and the floor read it
    /// per pixel; on p41 the zoned sky correction the per-pixel ruler wanted
    /// (horizon fifth 9.5 against 4.5 dE76/px) read as harm on the
    /// distribution and was dropped.
    pub fn structure_blind(&self, tp: &[[f32; 3]]) -> EvidenceModel {
        let n = self.source_pixels.len().min(tp.len());
        let ones = vec![1.0; n];
        let mut blind = self.clone();
        blind.spatial_weights = ones.clone();
        blind.spatial_supported = vec![true; n];
        blind.globally_same_content = true;
        blind.scoped(tp, &ones, &ones)
    }
}

pub(super) const EVIDENCE_LUMA_BINS: usize = 17;
pub(super) const EVIDENCE_HUE_BANDS: usize = 8;
pub(super) const EVIDENCE_MIN_SHARE: f32 = 0.015;
const EVIDENCE_DIVERGENCE_CUTOFF: f32 = 1.0;
/// The range form of the existing per-zone divergence policy. A range must
/// retain at least the support left at `DIVERGENCE_ZONE`; the texture
/// calibration is D=0.628 and survives, while the invented-sky value ranges
/// retain only 9-16% and do not.
pub(super) const EVIDENCE_RANGE_SURVIVAL_MIN: f32 = 1.0 - DIVERGENCE_ZONE;
pub(crate) const UNSUPPORTED_RANGE_MOVE: f32 = 2.0 / 255.0;
/// The same-content texture calibration retains 0.341 identifiability; the
/// invented-sky pair retains 0.218. Detail fitting is enabled between them.
pub(super) const DETAIL_EVIDENCE_MIN_IDENTIFIABILITY: f32 = 0.30;

/// Fold the existing 17-bin luma verdict over one inclusive contiguous run.
/// Range partitioning uses this instead of inventing a second evidence model:
/// population, structural survival and estimator weight retain the global
/// fit's exact per-bin meanings.
pub(crate) fn luma_evidence_for_bins(
    evidence: &EvidenceModel,
    first: usize,
    last: usize,
) -> EvidenceRange {
    let end = last.saturating_add(1).min(evidence.luma.len());
    let start = first.min(end);
    // The label names the bins actually FOLDED: `last` clamps to the model's
    // top bin here, and printing the request would name a bin never read.
    let last = end.saturating_sub(1).max(start);
    let bins = &evidence.luma[start..end];
    let source_share = bins.iter().map(|r| r.source_share).sum::<f32>();
    let target_share = bins.iter().map(|r| r.target_share).sum::<f32>();
    let source_evidence_share = bins.iter().map(|r| r.source_evidence_share).sum::<f32>();
    let target_evidence_share = bins.iter().map(|r| r.target_evidence_share).sum::<f32>();
    let two_sided_share = source_evidence_share.min(target_evidence_share);
    let structural_mass = bins.iter().map(|r| r.weight).sum::<f32>();
    let divergence_weight = bins
        .iter()
        .filter(|r| r.divergence.is_finite())
        .map(|r| r.two_sided_share)
        .sum::<f32>();
    let divergence = if divergence_weight > 0.0 {
        bins.iter()
            .filter(|r| r.two_sided_share > 0.0 && r.divergence.is_finite())
            .map(|r| r.divergence * r.two_sided_share)
            .sum::<f32>()
            / divergence_weight
    } else {
        f32::INFINITY
    };
    let source_populated = source_share >= EVIDENCE_MIN_SHARE;
    let target_populated = target_share >= EVIDENCE_MIN_SHARE;
    EvidenceRange {
        label: format!("luma bins {start:02}-{last:02}"),
        source_share,
        target_share,
        source_evidence_share,
        target_evidence_share,
        two_sided_share,
        divergence,
        weight: if source_populated && target_populated { structural_mass } else { 0.0 },
        source_populated,
        target_populated,
    }
}

pub fn evidence_luma_bin(v: f32) -> usize {
    ((v.clamp(0.0, 1.0) * EVIDENCE_LUMA_BINS as f32).floor() as usize)
        .min(EVIDENCE_LUMA_BINS - 1)
}

pub fn evidence_hue_band(p: &[f32; 3]) -> Option<usize> {
    let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
    if chroma < 0.06 {
        return None;
    }
    let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
    let (b0, b1, w1) = render::bracket_bands(h * 360.0, &render::HSL_CENTERS);
    Some(if w1 < 0.5 { b0 } else { b1 })
}

fn evidence_range(
    label: String,
    source_members: &[f32],
    target_members: &[f32],
    source_population: f32,
    target_population: f32,
    support: SupportField<'_>,
) -> EvidenceRange {
    let SupportField { spatial_weights, spatial_divergence, globally_same_content } = support;
    // Memberships are soft weights (1.0 everywhere for the frame); the
    // populations are the weighted member counts the shares are taken over.
    let source_n = source_population.max(1.0);
    let target_n = target_population.max(1.0);
    let source_share = source_members.iter().sum::<f32>() / source_n;
    let target_share = target_members.iter().sum::<f32>() / target_n;
    let supported_share = |members: &[f32], population: f32| {
        members
            .iter()
            .zip(spatial_weights)
            .map(|(&member, &weight)| member * weight)
            .sum::<f32>()
            / population
    };
    let source_evidence_share = supported_share(source_members, source_n);
    let target_evidence_share = supported_share(target_members, target_n);
    // Population and structural support are two different facts.  The 1.5%
    // line answers only whether the range exists on each side; applying it a
    // second time after the divergence discount made a weakly-correlated
    // population look absent.  A populated range can therefore be withheld
    // for structure (zero `two_sided_share`) without being mislabeled as
    // one-sided/empty in the disclosure.
    let source_populated = source_share >= EVIDENCE_MIN_SHARE;
    let target_populated = target_share >= EVIDENCE_MIN_SHARE;
    let two_sided_share = source_evidence_share.min(target_evidence_share);
    let (div_sum, div_weight) = source_members
        .iter()
        .zip(spatial_weights)
        .zip(spatial_divergence)
        .filter(|((member, weight), divergence)| {
            **member > 0.0 && **weight > 0.0 && divergence.is_finite()
        })
        .fold((0.0f32, 0.0f32), |(sum, weight), ((&m, &w), &d)| {
            (sum + m * w * d, weight + m * w)
        });
    let divergence = if div_weight > 0.0 { div_sum / div_weight } else { f32::INFINITY };
    let structural_survival = (source_evidence_share / source_share.max(1e-6))
        .min(target_evidence_share / target_share.max(1e-6));
    let weight = if source_populated
        && target_populated
        && (globally_same_content || structural_survival >= EVIDENCE_RANGE_SURVIVAL_MIN)
    {
        two_sided_share
    } else {
        0.0
    };
    EvidenceRange {
        label,
        source_share,
        target_share,
        source_evidence_share,
        target_evidence_share,
        two_sided_share,
        divergence,
        weight,
        source_populated,
        target_populated,
    }
}

/// The value-range verdicts and per-pixel evidence weights of ONE population.
/// The global fit moves the whole frame and is judged by the frame's bins; a
/// zone moves only its members, so [`EvidenceModel::scoped`] re-aggregates the
/// same per-pixel structural support over them. Memberships are soft (a
/// refined mask's feather), the frame is the all-ones case, and target luma
/// bins are rank-paired within the population's own target members at its own
/// source-to-target mass ratio. Every sum and share uses the one aligned
/// `0..min(source.len(), target.len())` prefix. A divergent population is
/// withheld unless the frame-wide same-content verdict deliberately bypasses
/// range survival and per-pixel withholding.
pub(super) struct RangeAggregate {
    /// Source membership over the same aligned prefix as every range sum.
    source_membership: Vec<f32>,
    source_weights: Vec<f32>,
    target_weights: Vec<f32>,
    source_hue_weights: Vec<f32>,
    target_hue_weights: Vec<f32>,
    /// Membership, the weights the objective judges at (DEF-5: presence is
    /// membership; spatial support never weighs a judged pixel).
    source_presence_weights: Vec<f32>,
    target_presence_weights: Vec<f32>,
    pub(super) luma: Vec<EvidenceRange>,
    hue: Vec<EvidenceRange>,
    identifiability: f32,
    population: f32,
}

/// The per-pixel structural support one population's verdicts are read
/// against: the frame's spatial-cell confidence, that cell's divergence and
/// the frame-wide same-content verdict.
#[derive(Clone, Copy)]
pub(super) struct SupportField<'a> {
    pub(super) spatial_weights: &'a [f32],
    pub(super) spatial_divergence: &'a [f32],
    pub(super) globally_same_content: bool,
}

pub(super) fn aggregate_ranges(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    source_zone: &[f32],
    target_zone: &[f32],
    support: SupportField<'_>,
) -> RangeAggregate {
    let spatial_weights = support.spatial_weights;
    let n = sp.len().min(tp.len());
    let member = |zone: &[f32], i: usize| zone.get(i).copied().unwrap_or(0.0).max(0.0);
    let source_membership = (0..n).map(|i| member(source_zone, i)).collect::<Vec<_>>();
    let source_mass = source_membership.iter().sum::<f32>();
    let target_mass = (0..n).map(|i| member(target_zone, i)).sum::<f32>();
    let mut luma = Vec::with_capacity(EVIDENCE_LUMA_BINS);
    let mut luma_source_weights = vec![0.0f32; n];
    let mut luma_target_weights = vec![0.0f32; n];
    let mut target_order: Vec<usize> = (0..n).filter(|&i| member(target_zone, i) > 0.0).collect();
    target_order.sort_by(|&a, &b| luma601(&tp[a]).total_cmp(&luma601(&tp[b])));
    let ratio = if source_mass > 0.0 { target_mass / source_mass } else { 0.0 };
    let mut cursor = 0usize;
    let mut taken = 0.0f32;
    let mut quota = 0.0f32;
    for bin in 0..EVIDENCE_LUMA_BINS {
        let sm: Vec<f32> = sp[..n]
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if evidence_luma_bin(luma601(p)) == bin { member(source_zone, i) } else { 0.0 }
            })
            .collect();
        let lo = bin as f32 / EVIDENCE_LUMA_BINS as f32;
        let hi = (bin + 1) as f32 / EVIDENCE_LUMA_BINS as f32;
        // Luma correspondence is monotone: a real exposure/tone edit moves a
        // source bin to a different numeric target interval. Pair the bin to
        // the same target population ranks, then let spatial evidence decide
        // whether that population actually survives on both sides.
        let mut tm = vec![0.0f32; n];
        quota += sm.iter().sum::<f32>() * ratio;
        // A rank-boundary target member is consumed whole. Consequently the
        // cumulative allocation can drift by at most one member over the
        // entire population, rather than one fresh overshoot in every bin.
        while cursor < target_order.len() && taken < quota {
            let i = target_order[cursor];
            tm[i] = member(target_zone, i);
            taken += tm[i];
            cursor += 1;
        }
        let range = evidence_range(
            format!("luma[{lo:.2}-{hi:.2}]"),
            &sm,
            &tm,
            source_mass,
            target_mass,
            support,
        );
        for (i, &m) in sm.iter().enumerate().take(n) {
            if m > 0.0 && range.source_evidence_share > 0.0 {
                luma_source_weights[i] =
                    m * spatial_weights[i] * range.weight / range.source_evidence_share;
            }
        }
        for (i, &m) in tm.iter().enumerate().take(n) {
            if m > 0.0 && range.target_evidence_share > 0.0 {
                luma_target_weights[i] =
                    m * spatial_weights[i] * range.weight / range.target_evidence_share;
            }
        }
        luma.push(range);
    }
    let mut hue = Vec::with_capacity(EVIDENCE_HUE_BANDS);
    let mut source_hue_weights = vec![0.0f32; n];
    let mut target_hue_weights = vec![0.0f32; n];
    for band in 0..EVIDENCE_HUE_BANDS {
        let sm: Vec<f32> = sp[..n]
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if evidence_hue_band(p) == Some(band) { member(source_zone, i) } else { 0.0 }
            })
            .collect();
        let tm: Vec<f32> = tp[..n]
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if evidence_hue_band(p) == Some(band) { member(target_zone, i) } else { 0.0 }
            })
            .collect();
        let range = evidence_range(
            crate::recipe::HSL_BANDS[band].to_string(),
            &sm,
            &tm,
            source_mass,
            target_mass,
            support,
        );
        for (i, &m) in sm.iter().enumerate().take(n) {
            if m > 0.0 && range.source_evidence_share > 0.0 {
                source_hue_weights[i] =
                    m * spatial_weights[i] * range.weight / range.source_evidence_share;
            }
        }
        for (i, &m) in tm.iter().enumerate().take(n) {
            if m > 0.0 && range.target_evidence_share > 0.0 {
                target_hue_weights[i] =
                    m * spatial_weights[i] * range.weight / range.target_evidence_share;
            }
        }
        hue.push(range);
    }
    let luma_mass = luma.iter().map(|r| r.weight).sum::<f32>().min(1.0);
    let hue_mass = hue.iter().map(|r| r.weight).sum::<f32>().min(1.0);
    let identifiability = (0.75 * luma_mass + 0.25 * hue_mass).clamp(0.0, 1.0);
    // DEF-5: presence is membership. `source_zone` / `target_zone` already
    // carry the target's presence (absent pixels are members of nothing,
    // `present_weights`), so nothing else enters a judged weight.
    let presence_of = |zone: &[f32]| -> Vec<f32> { (0..n).map(|i| member(zone, i)).collect() };
    RangeAggregate {
        source_membership,
        source_weights: luma_source_weights,
        target_weights: luma_target_weights,
        source_hue_weights,
        target_hue_weights,
        source_presence_weights: presence_of(source_zone),
        target_presence_weights: presence_of(target_zone),
        luma,
        hue,
        identifiability,
        population: source_mass,
    }
}

/// Per-pixel structural support from contiguous cells. The divergence
/// primitive expects coherent image geometry (erosion, gradients and Gaussian
/// bands), so value-bin masks are applied only after these cell readings.
fn spatial_evidence(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    w: u32,
    h: u32,
    presence: Option<&[f32]>,
) -> (Vec<f32>, Vec<f32>) {
    const COLS: u32 = 3;
    const ROWS: u32 = 3;
    let scale = w.max(h).div_ceil(192).max(1);
    let sw = w.div_ceil(scale);
    let sh = h.div_ceil(scale);
    let mut ss = Vec::with_capacity((sw * sh) as usize);
    let mut tt = Vec::with_capacity((sw * sh) as usize);
    let mut present = Vec::with_capacity((sw * sh) as usize);
    for y in (0..h).step_by(scale as usize) {
        for x in (0..w).step_by(scale as usize) {
            let i = (y * w + x) as usize;
            ss.push(sp[i]);
            tt.push(tp[i]);
            present.push(presence.map_or(1.0, |p| p.get(i).copied().unwrap_or(0.0)));
        }
    }
    let mut weights = vec![0.0f32; sp.len().min(tp.len())];
    let mut divergences = vec![f32::INFINITY; weights.len()];
    let mut readings = Vec::with_capacity((ROWS * COLS) as usize);
    for row in 0..ROWS {
        for col in 0..COLS {
            let mut mask = vec![0.0f32; ss.len()];
            for y in 0..sh {
                for x in 0..sw {
                    if x * COLS / sw.max(1) == col && y * ROWS / sh.max(1) == row {
                        mask[(y * sw + x) as usize] = present[(y * sw + x) as usize];
                    }
                }
            }
            readings.push(structure_divergence(&ss, &tt, sw, sh, &mask));
        }
    }
    // WHOLESALE, or per cell. A frame whose every cell falls under the
    // instrument's resolvable core carries no structural reading anywhere,
    // and withholding every pixel's evidence on that ground would starve a
    // frame nothing measured; there the modulation is dropped, which is the
    // behaviour this function had before the instrument learned to abstain.
    // Where SOME cell resolves the frame IS measured, and a cell that did not
    // resolve earns no support claim: its divergence keeps the initialised
    // infinity and its confidence stays 0.
    if readings.iter().all(Option::is_none) {
        return (vec![1.0; weights.len()], vec![0.0; divergences.len()]);
    }
    for (cell, reading) in readings.into_iter().enumerate() {
        let (row, col) = (cell as u32 / COLS, cell as u32 % COLS);
        let Some(reading) = reading else { continue };
        let d = reading.d;
        let confidence = (1.0 - d / EVIDENCE_DIVERGENCE_CUTOFF).clamp(0.0, 1.0);
        for y in 0..h {
            for x in 0..w {
                if x * COLS / w.max(1) == col && y * ROWS / h.max(1) == row {
                    let i = (y * w + x) as usize;
                    weights[i] = confidence;
                    divergences[i] = d;
                }
            }
        }
    }
    (weights, divergences)
}

/// Build the single per-pixel/per-range evidence map.  Range weights are the
/// two-sided population share discounted by that range's existing structural
/// divergence; no second divergence statistic is introduced.
fn inferred_geometry(n: usize) -> (u32, u32) {
    if n > 0 {
        let mut h = (n as f64).sqrt().floor() as usize;
        while h > 1 && !n.is_multiple_of(h) {
            h -= 1;
        }
        ((n / h) as u32, h as u32)
    } else {
        (0, 0)
    }
}

/// The evidence model of a pair whose target is present everywhere — the
/// fixtures' form. Production pairs come through [`evidence_model_paired`]
/// with the presence their target carries, so no fit can build an
/// all-present model of a target that is not.
#[cfg(test)]
pub fn evidence_model_for(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    width: u32,
    height: u32,
) -> EvidenceModel {
    evidence_model_paired(sp, tp, None, width, height)
}

/// THE evidence model of one aligned pair. `presence` is the target's
/// ([`super::presence_of`]); an absent pixel is taken out of the frame
/// population, out of every structural reading and out of every support
/// verdict, so the ring a lens-corrected target leaves in the sensor frame
/// reads as nothing rather than as an edge smeared into refusers.
pub fn evidence_model_paired(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    presence: Option<&[f32]>,
    width: u32,
    height: u32,
) -> EvidenceModel {
    let n = sp.len().min(tp.len());
    let presence = presence.filter(|p| p.len() >= n && p[..n].iter().any(|v| *v <= 0.0));
    let (w, h) = if width as usize * height as usize == n {
        (width, height)
    } else {
        inferred_geometry(n)
    };
    let (mut spatial_weights, spatial_divergence) = spatial_evidence(sp, tp, w, h, presence);
    let all_spatial = presence.map_or_else(|| vec![1.0f32; n], |p| p[..n].to_vec());
    // The frame-wide reading carries the same wholesale rule: its mask is
    // all-ones, so no cell mask can resolve where this one abstains, and an
    // abstention here means the frame itself is under the instrument's
    // resolvable core. That is not a verdict about content, so it must not
    // withhold every range's evidence.
    let global_reading = structure_divergence(sp, tp, w, h, &all_spatial);
    let globally_same_content = global_reading.is_none_or(|r| r.d < DIVERGENCE_GLOBAL);
    let mut spatial_supported = spatial_divergence
        .iter()
        .map(|&divergence| globally_same_content || divergence < DIVERGENCE_ZONE)
        .collect::<Vec<_>>();
    let frame = all_spatial.clone();
    if let Some(p) = presence {
        for i in (0..n).filter(|&i| p[i] <= 0.0) {
            spatial_supported[i] = false;
            spatial_weights[i] = 0.0;
        }
    }
    let ranges = aggregate_ranges(
        sp,
        tp,
        &frame,
        &frame,
        SupportField {
            spatial_weights: &spatial_weights,
            spatial_divergence: &spatial_divergence,
            globally_same_content,
        },
    );
    let source_chroma_vector = mean_chroma_vector(sp, &frame);
    let target_chroma_vector = mean_chroma_vector(tp, &frame);
    let global_cast = detect_global_cast(sp, tp, &ranges.hue);
    EvidenceModel {
        source_pixels: sp.iter().take(n).copied().collect(),
        source_membership: ranges.source_membership,
        width: w,
        height: h,
        paired: spatial_supported.clone(),
        spatial_supported,
        source_weights: ranges.source_weights,
        target_weights: ranges.target_weights,
        source_hue_weights: ranges.source_hue_weights,
        target_hue_weights: ranges.target_hue_weights,
        source_presence_weights: ranges.source_presence_weights,
        target_presence_weights: ranges.target_presence_weights,
        luma: ranges.luma,
        hue: ranges.hue,
        source_chroma_vector,
        target_chroma_vector,
        global_cast,
        identifiability: ranges.identifiability,
        spatial_weights,
        spatial_divergence,
        globally_same_content,
        population: ranges.population,
        presence: presence.map(|p| p[..n].to_vec()),
    }
}

#[cfg(test)]
pub(crate) fn evidence_model(sp: &[[f32; 3]], tp: &[[f32; 3]]) -> EvidenceModel {
    let (width, height) = inferred_geometry(sp.len().min(tp.len()));
    evidence_model_for(sp, tp, width, height)
}

pub(super) fn movement_identifiability(after: &[[f32; 3]], evidence: &EvidenceModel) -> f32 {
    let mut unsupported = 0.0f32;
    let mut present = 0usize;
    for (i, (before, after)) in evidence.source_pixels.iter().zip(after).enumerate() {
        if evidence.is_absent(i) {
            continue;
        }
        present += 1;
        let movement = (0..3).map(|ch| (after[ch] - before[ch]).abs()).sum::<f32>() / 3.0;
        let unsupported_luma = source_luma_is_withheld(i, evidence);
        let unsupported_hue = evidence_hue_band(before).is_some()
            && source_hue_is_withheld(i, evidence);
        if unsupported_luma || unsupported_hue {
            unsupported += movement;
        }
    }
    let mean_unsupported = unsupported / present.max(1) as f32;
    (-UNSUPPORTED_MOVEMENT_CONFIDENCE_SLOPE * mean_unsupported)
        .exp()
        .clamp(0.0, 1.0)
}

pub(super) fn source_luma_is_withheld(index: usize, evidence: &EvidenceModel) -> bool {
    let Some(pixel) = evidence.source_pixels.get(index) else { return false };
    if evidence.is_absent(index) {
        return false;
    }
    let range = &evidence.luma[evidence_luma_bin(luma601(pixel))];
    range.source_populated
        && (range.weight <= 0.0
            || !evidence.spatial_supported.get(index).copied().unwrap_or(false))
}

pub(super) fn source_hue_is_withheld(index: usize, evidence: &EvidenceModel) -> bool {
    let Some(pixel) = evidence.source_pixels.get(index) else { return false };
    if evidence.is_absent(index) {
        return false;
    }
    let Some(band) = evidence_hue_band(pixel) else { return false };
    let range = &evidence.hue[band];
    range.source_populated
        && (range.weight <= 0.0
            || !evidence.spatial_supported.get(index).copied().unwrap_or(false))
}

fn range_is_withheld(range: &EvidenceRange) -> bool {
    range.weight <= 0.0 && (range.source_populated || range.target_populated)
}

fn withheld_range_hits(evidence: &EvidenceModel) -> (Vec<bool>, Vec<bool>) {
    let mut luma = evidence
        .luma
        .iter()
        .map(range_is_withheld)
        .collect::<Vec<_>>();
    let mut hue = evidence
        .hue
        .iter()
        .map(range_is_withheld)
        .collect::<Vec<_>>();
    for (index, pixel) in evidence.source_pixels.iter().enumerate() {
        if !evidence.is_absent(index) && !evidence.spatial_supported.get(index).copied().unwrap_or(false) {
            luma[evidence_luma_bin(luma601(pixel))] = true;
            if let Some(band) = evidence_hue_band(pixel) {
                hue[band] = true;
            }
        }
    }
    (luma, hue)
}

pub(crate) fn withheld_range_names(evidence: &EvidenceModel) -> (String, String) {
    let (luma, hue) = withheld_range_hits(evidence);
    let names = |hits: &[bool], ranges: &[EvidenceRange]| {
        hits
            .iter()
            .zip(ranges)
            .filter_map(|(&hit, range)| hit.then_some(range.label.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    (names(&luma, &evidence.luma), names(&hue, &evidence.hue))
}

/// Whether the shared evidence model withheld a populated range because only
/// one image carried it. This is the cause input for the single FAR classifier.
pub(crate) fn evidence_has_one_sided(evidence: &EvidenceModel) -> bool {
    evidence
        .luma
        .iter()
        .chain(&evidence.hue)
        .any(|range| {
            range.weight <= 0.0 && range.source_populated != range.target_populated
        })
}

pub(super) fn divergent_range_names(evidence: &EvidenceModel) -> String {
    let (luma, hue) = withheld_range_hits(evidence);
    luma.iter()
        .zip(&evidence.luma)
        .chain(hue.iter().zip(&evidence.hue))
        .filter_map(|(&withheld, range)| {
            (withheld && range.source_populated && range.target_populated)
                .then_some(range.label.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn weighted_cdf(px: &[[f32; 3]], weights: &[f32], value: impl Fn(&[f32; 3]) -> f32) -> Vec<f32> {
    let mut hist = vec![0.0f32; HIST_BINS];
    let mut total = 0.0f32;
    for (i, p) in px.iter().enumerate() {
        let w = weights.get(i).copied().unwrap_or(0.0).max(0.0);
        if w <= 0.0 { continue; }
        let bin = (value(p).clamp(0.0, 1.0) * (HIST_BINS - 1) as f32).round() as usize;
        hist[bin] += w;
        total += w;
    }
    if total <= 1e-8 { return vec![0.0; HIST_BINS]; }
    let mut acc = 0.0;
    for v in &mut hist { acc += *v; *v = acc / total; }
    hist
}

/// Weighted median of a scattered sample, lower-median convention: the first
/// value at which the cumulative weight reaches half of the total.
///
/// Deliberately NOT [`weighted_cdf`] + [`quantile`]. That pair bins its key
/// into `HIST_BINS` buckets over `[0, 1]`, which is right for a channel value
/// and wrong for the quantity this batch reads: a per-pixel LOG RATIO is
/// signed, unbounded, and concentrated near zero, so a `[0, 1]` histogram
/// would clamp half of it into one bin. Sorting is exact and the cost is one
/// sort of the analysis frame per channel, against thirteen full renders on
/// the same path.
pub(super) fn weighted_median(samples: &mut [(f32, f32)]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total = samples.iter().map(|&(_, w)| w as f64).sum::<f64>();
    if total <= 0.0 {
        return 0.0;
    }
    let half = total * 0.5;
    let mut carried = 0.0f64;
    for &(value, weight) in samples.iter() {
        carried += weight as f64;
        if carried >= half {
            return value;
        }
    }
    samples[samples.len() - 1].0
}

/// The population AND the pairing the Atmosphere white balance is solved on.
/// ONE authority chooses both, and that is the point: a readable
/// shared-content population EXISTS only because a correspondence field was
/// readable, and the field that is trusted to say WHICH pixels are shared is
/// the same field that says WHICH target pixel each source pixel corresponds
/// to. Taking the population without the pairing is how a RECOMPOSED pair —
/// the same content, moved in frame, which is squarely inside Atmosphere's
/// remit — gets read as a colour cast: same-index pairing against the raw
/// target array is a random association once content has moved.
///
/// With no readable field the two sides are index-paired exactly as they
/// always were, and the per-pixel weight is `min(source, target)`. That is
/// not an invention: it is this module's own two-sidedness rule, the
/// `source_evidence_share.min(target_evidence_share)` that [`EvidenceModel`]
/// already applies to every range.
///
/// With a readable field `p.source` ALONE is the coherent weight, and mixing
/// in `p.target` would not be. `shared_content_population` sets `source[i]`
/// exactly where `conf[i] >= CONFIDENT_MATCH`, i.e. exactly where `pair_tp[i]`
/// IS that source pixel's counterpart; the target weights are RANK-paired
/// (built by sorting `luma601(&tp[..])`), so `target_weights[i]` is a
/// statement about `tp[i]`'s own luma rank, not about a pairing with `sp[i]`.
pub(super) fn atmosphere_wb_pairing<'a>(
    tp: &'a [[f32; 3]],
    evidence: &'a EvidenceModel,
    correspondence: Option<&'a PairCorrespondence>,
    readable: Option<&'a SharedPopulation>,
) -> (&'a [[f32; 3]], Cow<'a, [f32]>) {
    match readable {
        Some(p) => (
            correspondence.map_or(tp, |c| c.tp.as_slice()),
            Cow::Borrowed(p.source.as_slice()),
        ),
        None => {
            let n = evidence.source_weights.len().min(evidence.target_weights.len());
            let paired = (0..n)
                .map(|i| evidence.source_weights[i].min(evidence.target_weights[i]))
                .collect::<Vec<_>>();
            (tp, Cow::Owned(paired))
        }
    }
}

/// The Atmosphere global white balance: a weighted median of the PER-PIXEL
/// log ratio, taken jointly over ONE population, normalised by its own
/// geometric mean and inverted through the engine's WB model. Returns the
/// rounded Kelvin, the rounded tint, and the normalised `wanted` gains the
/// search was fitted to.
///
/// Three INDEPENDENT per-channel medians — what this replaces — are a ratio
/// of two marginals. On a bimodal frame the two marginals' halfway points
/// fall in different sub-populations, so their ratio is not the colour change
/// of any pixel in the frame; on the crate's own `flat_sky_to_cloud_deck`,
/// where no pixel changed its chromaticity at all, they read K 4400 /
/// tint +55.2. This reads one cloud of per-pixel colour CHANGES and takes its
/// centre.
///
/// The LOG form is written rather than the raw ratio because the median
/// commutes with a monotone map — the two are numerically identical — and the
/// log is what makes the estimator a LOCATION statistic on one cloud instead
/// of a ratio of two. The `1e-5` floor is this call site's own existing
/// convention, not a new knob.
///
/// The MEDIAN, not the mean, for exactly the reason it always was: a newly
/// generated cloud highlight must not own a frame-wide average. That comment
/// was not overruled by this batch — what the per-pixel form adds is that the
/// robustness now applies to the CHANGE each pixel underwent rather than to
/// two brightness distributions read apart from each other.
///
/// THE STATISTIC IS A CHROMATICITY (2026-10-21). Brightness is not this
/// stage's to set (exposure owns it), so each pixel's log colour change is
/// read as its red and its blue against its green BEFORE any median is
/// taken: two medians of one cloud of per-pixel chromaticity changes. A
/// median per channel, even of per-pixel ratios, still draws its red and its
/// blue from different pixels, and a photo whose dark pixels went one way
/// and bright pixels another reads a colour no pixel asked for — the lantern
/// street read tint −29 that way and −5 as a chromaticity (measured on its
/// 92272 paired pixels, 2026-10-21).
///
/// THE DEMAND IS THE CONSENSUS OF THE BRIGHTNESS RANGES (2026-10-21, the
/// same day; the consensus form later that day). A white balance is an
/// illuminant, and an illuminant moves every brightness the same way; a
/// grade that warms the shadows and cools the highlights is not one, and the
/// robust centre of that cloud is the majority's half of it applied to the
/// minority — the seaside street's dark 85% asked 40000 K and the sky, sea,
/// far houses and windows went yellow. So the cloud is read per evidence
/// luma range (`EVIDENCE_LUMA_BINS` on the source luma, ranges under
/// `EVIDENCE_MIN_SHARE` of the mass not testifying — the floors the evidence
/// model already uses), each range's medians are put on the engine's two
/// axes — red against blue (Kelvin) and green against the red/blue mean
/// (tint) — and on each axis: where the ranges asking the OPPOSITE sign of
/// the mass-weighted median, by at least [`WB_OPPOSING_RATIO`] of its size,
/// hold [`WB_OPPOSING_SHARE`] or more of the testifying mass there is no
/// single illuminant and the demand is nothing; otherwise it is that median.
/// A range asking a sliver the other way is idle, not a second light
/// (2026-10-22: by sign alone a dark quarter at −0.032 against +0.37
/// vetoed the toy's Kelvin). Pixels too dark to carry a chromaticity
/// (a channel maximum under [`WB_CHROMA_FLOOR`] on either side — the dark
/// clause of `is_neutralish`, two or three 8-bit levels where one level is
/// the whole reading) do not testify.
///
/// The first form of this rule took the signed value nearest zero across
/// the ranges — unanimity — and one small range could zero the frame: the
/// night sky's darkest fifth (source maximum 0.035, near black) read −0.22
/// against the sky's −2.6..−2.8 and the stage shipped tint 100 at 5000 K
/// (ΔE 5.22 → 12.62); the sunset city's two brightest ranges (16%) read
/// −0.03/−0.14 against +0.3..+0.68 everywhere below and the stage shipped
/// nothing (4.35 → 5.08); the neon street's darkest 7.5% read −0.13 against
/// +0.15..+0.60 (6.75 → 8.91). Measured under the consensus rule (opposition
/// by sign alone, 2026-10-21): night
/// −2.77 / +0.32 (opposing 0.000), sunset city +0.31 (0.078), neon +0.15 /
/// +0.25 (0.064), seaside street 0 (0.204), lantern street +1.17 (0.000),
/// both suburbs 0 (0.36). The size that ships is still the cells' to vouch
/// (`vouch_along`, then the share search).
///
/// INVERSION is the render's own law's (`render::WbLaw::invert`, v1.17.0):
/// the demand goes onto the (Kelvin, tint) pair whose rendered gains match
/// it, and "no demand" is As Shot. The estimator before that inversion
/// normalised the ratios by their geometric mean
/// and read the tint off the resulting green, which is 1 only when the
/// Kelvin gains' own geometric mean is 1 — it never is — so every warm move
/// came back green: a pure 40000 K move from a 5768 K anchor read tint
/// −19.7, and the lantern street shipped at 40000 K / −30.5, 3.1 Lab-a units
/// greener than its target.
/// Ranges asking the opposite sign on an axis that hold this share of the
/// testifying mass or more: no single illuminant, no demand on that axis.
/// Below it the opposition is a minority the median absorbs (the neon
/// street's darkest 7.5%, the sunset city's blown highlights at 7.8%); at
/// it and above it is a second light (the seaside street's bright fifth,
/// 0.204; the suburbs' 0.36).
pub(super) const WB_OPPOSING_SHARE: f32 = 0.10;
/// A range opposes the consensus only when it asks the opposite sign by at
/// least this fraction of the consensus's own size. By sign alone a range
/// that asks for almost nothing vetoed a demand every other range agreed
/// on: an idle dark quarter read −0.032 on the Kelvin axis after the tone
/// sliders (0.102 of the mass, 0.09 of the +0.37 the rest asked) and the
/// frame shipped no Kelvin; the canyon's unchanged sky read ~0 against the
/// rocks' warmth and the whole warm demand fell on the cast curves
/// (2026-10-22). Why a fifth (2026-10-22): on the seven real-photo
/// per-pixel range dumps (read before the tone stage, chroma floor applied,
/// ceiling-capped pixels kept as the engine keeps them) every ratio up to
/// 0.22 leaves all seven demands where sign alone put them, and above it
/// the seaside street's bright ranges (0.22 / 0.17 / 0.10 of its +0.86)
/// stop opposing and its veto lifts; the toy's sliver sits at 0.09. A
/// quarter, which a toned-render reading that dropped capped pixels had
/// admitted, sits outside that window; a fifth sits inside it on both.
pub(super) const WB_OPPOSING_RATIO: f32 = 0.20;
/// A channel maximum under this carries no chromaticity worth a median —
/// the dark clause of `is_neutralish`, named here because the white
/// balance reads it for the same reason.
pub(super) const WB_CHROMA_FLOOR: f32 = 0.04;

/// The per-range readings behind the white-balance demand: for every
/// evidence luma range holding `EVIDENCE_MIN_SHARE` of the testifying mass,
/// the range's chromaticity medians on the engine's two axes — red against
/// blue (Kelvin) and green against the red/blue mean (tint) — each paired
/// with the range's mass. Pixels under [`WB_CHROMA_FLOOR`] on either side do
/// not testify.
#[derive(Default)]
pub(super) struct WbAxes {
    pub(super) kelvin: Vec<(f32, f32)>,
    pub(super) tint: Vec<(f32, f32)>,
    /// Per testifying entry, the standard error of its `[kelvin, tint]`
    /// ask ([`median_se`]); filled by [`wb_axes_by_region`], empty from
    /// [`wb_axes_by_range`] (whose consensus reads no abstention).
    pub(super) se: Vec<[f32; 2]>,
    /// Per entry, the tonal region it reads (0 shadows, 1 midtones, 2
    /// highlights) and one output code's log step at its level
    /// ([`code_step_variance`]'s `q`); filled by [`wb_axes_by_region`].
    pub(super) region: Vec<usize>,
    pub(super) code: Vec<f32>,
}

/// `1 / Φ⁻¹(3/4)`: the median absolute deviation of a normal sample times
/// this is its standard deviation.
const MAD_TO_SIGMA: f64 = 1.482_602_2;

/// The standard error of a weighted sample's median `ask`: the median's
/// asymptotic spread `√(π/2) · σ / √n` (its efficiency against the mean
/// under a normal), with σ read robustly as [`MAD_TO_SIGMA`] times the
/// weighted median absolute deviation about `ask`, and `n` the sample's
/// effective size `(Σw)² / Σw²`. Zero for an empty sample.
pub(super) fn median_se(samples: &[(f32, f32)], ask: f32) -> f32 {
    let (sw, sw2) = samples.iter().fold((0.0f64, 0.0f64), |(a, b), &(_, w)| (a + w as f64, b + (w as f64).powi(2)));
    if sw <= 0.0 || sw2 <= 0.0 {
        return 0.0;
    }
    let mut dev: Vec<(f32, f32)> = samples.iter().map(|&(v, w)| ((v - ask).abs(), w)).collect();
    let mad = weighted_median(&mut dev) as f64;
    ((std::f64::consts::PI / 2.0).sqrt() * MAD_TO_SIGMA * mad / (sw * sw / sw2).sqrt()) as f32
}

pub(super) fn wb_axes_by_range(sp: &[[f32; 3]], pair_tp: &[[f32; 3]], pair_w: &[f32]) -> WbAxes {
    let n = sp.len().min(pair_tp.len());
    // Each pixel's own log colour change with its brightness change removed
    // PIXEL BY PIXEL — red and blue against green — binned by source luma.
    let mut red: Vec<Vec<(f32, f32)>> = vec![Vec::new(); EVIDENCE_LUMA_BINS];
    let mut blue: Vec<Vec<(f32, f32)>> = vec![Vec::new(); EVIDENCE_LUMA_BINS];
    let mut mass = [0.0f64; EVIDENCE_LUMA_BINS];
    for i in 0..n {
        let w = pair_w.get(i).copied().unwrap_or(0.0).max(0.0);
        if w <= 0.0 {
            continue;
        }
        let too_dark = |p: &[f32; 3]| p[0].max(p[1]).max(p[2]) < WB_CHROMA_FLOOR;
        if too_dark(&sp[i]) || too_dark(&pair_tp[i]) {
            continue;
        }
        let d: [f32; 3] = std::array::from_fn(|ch| {
            let source = render::srgb_to_linear(sp[i][ch]).max(1e-5);
            let target = render::srgb_to_linear(pair_tp[i][ch]).max(1e-5);
            target.ln() - source.ln()
        });
        let bin = evidence_luma_bin(luma601(&sp[i]));
        red[bin].push((d[0] - d[1], w));
        blue[bin].push((d[2] - d[1], w));
        mass[bin] += w as f64;
    }
    let total: f64 = mass.iter().sum();
    let mut axes = WbAxes::default();
    for bin in 0..EVIDENCE_LUMA_BINS {
        if total <= 0.0 || mass[bin] / total < EVIDENCE_MIN_SHARE as f64 {
            continue;
        }
        let (u, v) = (weighted_median(&mut red[bin]), weighted_median(&mut blue[bin]));
        axes.kelvin.push((u - v, mass[bin] as f32));
        axes.tint.push((0.5 * (u + v), mass[bin] as f32));
    }
    axes
}

/// The consensus of one axis across the ranges that testify: `(value,
/// opposing share)`. The opposing share is the mass of the ranges asking
/// the sign opposite the mass-weighted median, by at least
/// [`WB_OPPOSING_RATIO`] of the median's size, over the whole testifying
/// mass; at [`WB_OPPOSING_SHARE`] or more there is no single illuminant and
/// the value is zero, below it the value is the median. A median of exactly
/// zero has nothing to oppose.
pub(super) fn wb_consensus(axis: &[(f32, f32)]) -> (f32, f32) {
    let testifying: f32 = axis.iter().map(|&(_, m)| m).sum();
    if testifying <= 0.0 {
        return (0.0, 0.0);
    }
    let median = weighted_median(&mut axis.to_vec());
    let opposing = axis
        .iter()
        .filter(|&&(v, _)| v * median < 0.0 && v.abs() >= WB_OPPOSING_RATIO * median.abs())
        .map(|&(_, m)| m)
        .sum::<f32>()
        / testifying;
    if opposing >= WB_OPPOSING_SHARE {
        return (0.0, opposing);
    }
    (median, opposing)
}

/// The same per-pixel chromaticity change as [`wb_axes_by_range`], read per
/// TONAL REGION of the colour grade instead of per evidence luma range: each
/// paired pixel testifies to the shadow, midtone and highlight regions with
/// the weights the grade itself gives its source luma
/// ([`render::color_grade_regions`], at the grade's default split), and a
/// region holding under [`EVIDENCE_MIN_SHARE`] of the testifying mass does
/// not testify. The regions are the partition the colour-grade stage
/// (`grade::fit_grade_stage`) can carry a remainder on, so the white
/// balance is read on the same one. Each region's ask carries its own
/// standard error on each axis ([`median_se`] of the per-pixel axis value,
/// `ln r − ln b` for Kelvin and `(ln r + ln b)/2 − ln g` for tint, about
/// the region's ask).
pub(super) fn wb_axes_by_region(sp: &[[f32; 3]], pair_tp: &[[f32; 3]], pair_w: &[f32]) -> WbAxes {
    let n = sp.len().min(pair_tp.len());
    let split = crate::recipe::ColorGrade::default();
    let mut red: [Vec<(f32, f32)>; 3] = Default::default();
    let mut blue: [Vec<(f32, f32)>; 3] = Default::default();
    let mut along: [[Vec<(f32, f32)>; 2]; 3] = Default::default();
    let mut mass = [0.0f64; 3];
    let mut level = [0.0f64; 3];
    for i in 0..n {
        let w = pair_w.get(i).copied().unwrap_or(0.0).max(0.0);
        if w <= 0.0 {
            continue;
        }
        let too_dark = |p: &[f32; 3]| p[0].max(p[1]).max(p[2]) < WB_CHROMA_FLOOR;
        if too_dark(&sp[i]) || too_dark(&pair_tp[i]) {
            continue;
        }
        let d: [f32; 3] = std::array::from_fn(|ch| {
            let source = render::srgb_to_linear(sp[i][ch]).max(1e-5);
            let target = render::srgb_to_linear(pair_tp[i][ch]).max(1e-5);
            target.ln() - source.ln()
        });
        let regions = render::color_grade_regions(&split, luma601(&sp[i]));
        for r in 0..3 {
            let m = w * regions[r];
            if m > 0.0 {
                red[r].push((d[0] - d[1], m));
                blue[r].push((d[2] - d[1], m));
                along[r][0].push((d[0] - d[2], m));
                along[r][1].push((0.5 * (d[0] + d[2]) - d[1], m));
                mass[r] += m as f64;
                level[r] += (m * luma601(&sp[i])) as f64;
            }
        }
    }
    let total: f64 = mass.iter().sum();
    let mut axes = WbAxes::default();
    for r in 0..3 {
        if total <= 0.0 || mass[r] / total < EVIDENCE_MIN_SHARE as f64 {
            continue;
        }
        let (u, v) = (weighted_median(&mut red[r]), weighted_median(&mut blue[r]));
        let (k, t) = (u - v, 0.5 * (u + v));
        axes.kelvin.push((k, mass[r] as f32));
        axes.tint.push((t, mass[r] as f32));
        let level = (level[r] / mass[r]) as f32;
        axes.se.push([region_se(median_se(&along[r][0], k), level), region_se(median_se(&along[r][1], t), level)]);
        axes.region.push(r);
        axes.code.push((12.0 * code_step_variance(level)).sqrt());
    }
    axes
}

/// The resolution of one region's reading: `q² / 12`, the variance of a
/// uniform quantisation of one output code, with `q` that code's step in the
/// axes' own unit — the log step `ln lin(v + 1/255) − ln lin(v)` of one 8-bit
/// display code at the region's mean display level `v` (its pixels' luma,
/// mass-weighted). Both axes are differences of such log steps per channel,
/// so the Kelvin axis (`ln r − ln b`) and the tint axis read the same `q`.
/// Added to the median's own variance ([`median_se`]): a region whose
/// residual is under one code reads a median of exactly 0.0 with a tiny
/// spread, and without the floor that 0.0 would read as a confident "stay"
/// (rel1180 RULING-1180-sc-r2).
pub(super) fn code_step_variance(level: f32) -> f32 {
    let v = level.clamp(0.0, 1.0 - 1.0 / 255.0);
    let lin = |x: f32| render::srgb_to_linear(x).max(1e-5);
    let q = (lin(v + 1.0 / 255.0) / lin(v)).ln();
    q * q / 12.0
}

/// One region's error on an axis: its median's own ([`median_se`]) and one
/// output code's quantisation at its mean display level
/// ([`code_step_variance`]), in quadrature.
pub(super) fn region_se(median_se: f32, level: f32) -> f32 {
    (median_se * median_se + code_step_variance(level)).sqrt()
}


/// The part of one axis's demand every tonal region SHARES (`axis` 0 is
/// Kelvin, 1 tint), each region read with its own error (`se`, the median's
/// plus one output code's quantisation, [`code_step_variance`]):
/// * a region whose interval `ask ± se` excludes zero RESOLVES a move: the
///   common ask is the smallest of the resolving asks when they share a
///   sign ([`wb_common`]), nothing when they split;
/// * a region whose interval contains zero cannot propose a move but BOUNDS
///   it: the common ask is clipped into that interval, toward zero — a move
///   the region's own reading rules out is not shared with it;
/// * a region ABSTAINS only with no information (no mass, an undefined
///   error).
///
/// The white balance takes only what every band shares (2026-11-01; rel1180
/// RULING-1180-sc-r2): measured at 8250 K on a real pair, the highlights'
/// −0.00086 ± 0.00088 is a measured "stay" and bounds the shadows' +0.954,
/// where reading it as silence walked the frame to the 40000 K bound; on the
/// self-consistency gate's 4400 K render the shadows' quantised 0.0 reads an
/// interval of at least one code and leaves the highlights' −0.0109 whole.
pub(super) fn wb_common_shared(axes: &WbAxes, axis: usize) -> f32 {
    wb_move_shared(axes, axis, &[])
}

/// [`wb_common_shared`] read where the CONTROL acts (rel1180
/// RULING-1180-sc-r3): `response[i]` is entry `i`'s own measured response on
/// the axis — its reading's fall per unit of log gain over the chase's last
/// step (the secant's device), `None` where none was measured (the first
/// step, a step that did not move the axis, a reading that did not fall).
/// A resolving region proposes its ask divided by its own response (its ask
/// itself without one); the common move is the smallest proposal when they
/// share a sign. A bounding region with a response bounds in the control's
/// unit — its interval divided by its response — and has no opinion on a
/// move smaller than one output code of its own reading carried the same
/// way (a region on the tone curve's shoulder cannot see 50 K); without a
/// response it bounds in the ask's unit. Measured on the self-consistency
/// gate's 4300 K render: read in the ask's unit, the highlights' quantised
/// 0.0 at 4350 K (± 0.0029) clipped the midtones' −0.0147 to a 28 K step
/// that earned nothing, and the chase stalled 50 K short.
pub(super) fn wb_move_shared(axes: &WbAxes, axis: usize, response: &[Option<f32>]) -> f32 {
    let values = if axis == 0 { &axes.kelvin } else { &axes.tint };
    let read: Vec<(f32, f32, f32, f32, Option<f32>)> = values
        .iter()
        .enumerate()
        .filter_map(|(i, &(v, m))| {
            let se = axes.se.get(i)?[axis];
            let code = axes.code.get(i).copied().unwrap_or(0.0);
            (m > 0.0 && se.is_finite()).then(|| (v, m, se, code, response.get(i).copied().flatten()))
        })
        .collect();
    let resolving: Vec<(f32, f32)> = read
        .iter()
        .filter(|&&(v, _, se, _, _)| v.abs() > se)
        .map(|&(v, m, _, _, r)| (r.map_or(v, |r| v / r), m))
        .collect();
    let mut common = wb_common(&resolving);
    for &(v, _, se, code, r) in read.iter().filter(|&&(v, _, se, _, _)| v.abs() <= se) {
        common = match r {
            Some(r) if code / r >= common.abs() => common,
            Some(r) => common.clamp((v - se) / r, (v + se) / r),
            None => common.clamp(v - se, v + se),
        };
    }
    common
}

/// The part of one axis's demand EVERY testifying tonal region asks: the
/// smallest in size when they all ask the same sign, nothing when any two
/// disagree or one asks nothing. That is the brightness-invariant component
/// — what an illuminant can be — and everything past it in any region is
/// that region's own remainder, the colour grade's to carry.
pub(super) fn wb_common(axis: &[(f32, f32)]) -> f32 {
    let mut common: Option<f32> = None;
    for &(v, _) in axis {
        common = match common {
            None => Some(v),
            Some(c) if c * v <= 0.0 => return 0.0,
            Some(c) => Some(if v.abs() < c.abs() { v } else { c }),
        };
    }
    common.unwrap_or(0.0)
}

pub(super) fn atmosphere_wb_from_populations(
    sp: &[[f32; 3]],
    pair_tp: &[[f32; 3]],
    pair_w: &[f32],
    law: render::WbLaw,
) -> (f32, f32, [f32; 3]) {
    let axes = wb_axes_by_range(sp, pair_tp, pair_w);
    let (a, _) = wb_consensus(&axes.kelvin);
    let (m, _) = wb_consensus(&axes.tint);
    wb_demand(a, m, law)
}

/// The FULL solve's white-balance demand: the brightness-invariant
/// component ([`wb_common`] over [`wb_axes_by_region`]), because the Full
/// solve's colour-grade stage (`grade::fit_grade_stage`) carries the
/// region-dependent rest. The Atmosphere solve has no such stage and keeps
/// the consensus of [`atmosphere_wb_from_populations`].
pub(super) fn paired_wb_from_populations(
    sp: &[[f32; 3]],
    pair_tp: &[[f32; 3]],
    pair_w: &[f32],
    law: render::WbLaw,
) -> (f32, f32, [f32; 3]) {
    let axes = wb_axes_by_region(sp, pair_tp, pair_w);
    wb_demand(wb_common_shared(&axes, 0), wb_common_shared(&axes, 1), law)
}

/// One tonal region's `[kelvin, tint]` asks, by region index.
type RegionAsk = (usize, [f32; 2]);

/// [`paired_wb_from_populations`] read CLOSED-LOOP through the engine: the
/// demand is re-read on the render at the white balance it asked for, and
/// what is still asked in common is composed onto it in the law's own log
/// gains, until nothing is shared (no region resolves a move, the resolving
/// regions split in sign, or a bounding region clips it to zero —
/// [`wb_common_shared`]), the re-read lands where it
/// stands (the solve's 50 K / 0.1 quantisation), or the step does not earn
/// [`FIT_QUANT`] of `objective` over the white balance it would replace
/// ([`ships`]). One reading is open-loop on the DISPLAY pixels, where the
/// tone curve compresses a white balance's log-ratio change most at the
/// ends of the range, and [`wb_common`] keeps the smallest region's: the
/// read under-asks by construction. So from the second step on, each
/// region's ask is read in the control's unit through its OWN response —
/// the fall of its reading per unit of log gain over the chase's last step,
/// a secant through the real engine as the saturation chase and the grade's
/// strength are refined — and the regions' asks and bounds are combined
/// there ([`wb_move_shared`]); a region with no measured response is read in
/// the ask's unit. Measured on the self-consistency gate's 4300 K render (as
/// shot 5400 K): one reading asked 4800 K and the re-reads stopped at 4400 K;
/// with the common ask divided by its secant the chase reached 4350 K, where
/// the highlights' quantised 0.0 read in the ask's unit clipped the
/// midtones' −0.0147 to a 28 K step that earned nothing (rel1180 msg-sc
/// rounds 2-3).
/// `render_at` renders the stage's recipe at a white balance; `first` is
/// the open-loop reading.
pub(super) fn paired_wb_closed_loop(
    first: (f32, f32),
    pair_tp: &[[f32; 3]],
    pair_w: &[f32],
    law: render::WbLaw,
    render_at: impl Fn((f32, f32)) -> Vec<[f32; 3]>,
    objective: impl Fn(&[[f32; 3]]) -> f32,
) -> (f32, f32) {
    let mut at = first;
    let mut px = render_at(at);
    // The last step, in the two axes' log gains, and each region's asks it
    // was read on: (moved, [(region, [kelvin, tint])]).
    let mut last: Option<([f32; 2], Vec<RegionAsk>)> = None;
    let axes_of = |g: [f32; 3]| [g[0] - g[2], 0.5 * (g[0] + g[2]) - g[1]];
    loop {
        let axes = wb_axes_by_region(&px, pair_tp, pair_w);
        // Each region's own response per axis over the last step.
        let response = |axis: usize| -> Vec<Option<f32>> {
            (0..axes.region.len())
                .map(|i| {
                    let (moved, asked) = last.as_ref()?;
                    let &(_, before) = asked.iter().find(|(r, _)| *r == axes.region[i])?;
                    let now = if axis == 0 { axes.kelvin[i].0 } else { axes.tint[i].0 };
                    let r = (before[axis] - now) / moved[axis];
                    (moved[axis] != 0.0 && r > 0.0).then_some(r)
                })
                .collect()
        };
        let (a, m) = (wb_move_shared(&axes, 0, &response(0)), wb_move_shared(&axes, 1, &response(1)));
        if a == 0.0 && m == 0.0 {
            return at;
        }
        let from = law.log_gains(at);
        let next = law.invert([from[0] + m + 0.5 * a, from[1], from[2] + m - 0.5 * a], WB_SEARCH_K);
        if next == at {
            return at;
        }
        let next_px = render_at(next);
        if !ships(objective(&next_px), objective(&px)) {
            return at;
        }
        let (g0, g1) = (axes_of(from), axes_of(law.log_gains(next)));
        let asked = (0..axes.region.len()).map(|i| (axes.region[i], [axes.kelvin[i].0, axes.tint[i].0])).collect();
        last = Some(([g1[0] - g0[0], g1[1] - g0[1]], asked));
        (at, px) = (next, next_px);
    }
}

/// One axis pair's demand onto the render's own law: the Kelvin axis `a`
/// (red against blue) and the tint axis `m` (green against their mean).
fn wb_demand(a: f32, m: f32, law: render::WbLaw) -> (f32, f32, [f32; 3]) {
    // Nothing in common is no demand: the anchor itself, at the solve's own
    // quantisation, so the stage can tell "no move" from a small one.
    if a == 0.0 && m == 0.0 {
        let (k, tint) = law.as_shot();
        return ((k / 50.0).round() * 50.0, round1(tint), [1.0; 3]);
    }
    let want_log = [m + 0.5 * a, 0.0, m - 0.5 * a];
    // The normalised demand the callers and tests read: the same chromaticity
    // as gains with unit geometric mean.
    let common = (want_log[0] + want_log[1] + want_log[2]) / 3.0;
    let wanted = want_log.map(|v| (v - common).exp());
    let (k, tint) = law.invert(want_log, WB_SEARCH_K);
    (k, tint, wanted)
}

pub(super) fn weighted_mean_chroma(px: &[[f32; 3]], weights: &[f32]) -> Option<f32> {
    let mut sum = 0.0f32;
    let mut total = 0.0f32;
    for (i, p) in px.iter().enumerate() {
        let w = weights.get(i).copied().unwrap_or(0.0).max(0.0);
        sum += (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) * w;
        total += w;
    }
    (total > 1e-8).then_some(sum / total)
}

impl Divergence {
    /// A hand-built matched reading, for fixtures that need one.
    /// PRODUCTION NEVER MANUFACTURES ONE: `structure_divergence` either
    /// measures a divergence or abstains, and a manufactured 0.0 on a
    /// production path is the defect v1.2.4 removed.
    #[cfg(test)]
    pub(super) fn matched() -> Self {
        Self { correlation: 1.0, energy_error: 0.0, d: 0.0 }
    }
}
