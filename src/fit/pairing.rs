//! Pairing the two renditions: same-frame plausibility, the analysis pair, the fit report type, correspondence, and the shared population.

use super::*;

/// Aspect-ratio disagreement past which the reference's pixel population is no
/// longer this photo's (R23-6 B-7). 2% mirrors the grid-comparability rule
/// inside [`neutral_gate_misprediction`] — a few rows of a 384-edge thumbnail,
/// i.e. beyond what aspect ROUNDING explains.
///
/// A crop is NOT inside that budget, deliberately, and the doc used to imply it
/// was (R23 review LOW-3): 3:2 recomposed to 16:9 is 18% out and trips this
/// every time, on exactly the Lightroom/C1 export the tooltip asks for. The
/// reading stays correct — a crop changes which pixels the statistics are taken
/// over, so the two distributions stop being comparable — but the message it
/// drives has to say CROP rather than accuse the user of picking the wrong
/// file. A WARNING, never a refusal: the reference is a file the user chose on
/// purpose, and an unreliable fit is not an illegal one.
const SAME_FRAME_ASPECT_TOL: f32 = 0.02;

/// Do these two images plausibly show the SAME frame? `false` ⇒ warn.
///
/// ONE reading: the aspect ratios, which is all this function has ever
/// computed. (This doc used to promise a second — the grid comparability
/// [`neutral_gate_misprediction`] returns infinity for. That test is real, but
/// it belongs to `tone_cdf_pair`'s neutral-evidence gate and nothing routes its
/// answer here, so the promise was fiction — R23 round review LOW-2.)
///
/// One reading is enough for what this result is ALLOWED to do. It is a
/// necessary condition and never a sufficient one — a different photograph of
/// the same scene at the same aspect passes, and nothing short of registration
/// would catch it — which is exactly why the caller warns instead of refusing.
pub fn same_frame_plausible(src: &DynamicImage, target: &DynamicImage) -> bool {
    same_frame_plausible_dims((src.width(), src.height()), (target.width(), target.height()))
}

/// [`same_frame_plausible`] on dimensions alone — the ONE aspect rule every
/// "is this the sensor frame?" question in the crate reads: the reference
/// check above, `reimagine`'s choice of input frame, `match`'s choice of
/// source frame and the base-look estimator's pairing (v1.2.2, the in-camera
/// aspect-crop class: a body set to 4:3 writes a centred 4:3 preview over its
/// 3:2 sensor). One tolerance, so a frame cannot be "the same" to one
/// consumer and "cropped" to another.
pub fn same_frame_plausible_dims(a: (u32, u32), b: (u32, u32)) -> bool {
    let ar = |(w, h): (u32, u32)| w.max(1) as f32 / h.max(1) as f32;
    let (a, b) = (ar(a), ar(b));
    (a - b).abs() <= SAME_FRAME_ASPECT_TOL * a.max(b)
}

/// The two analysis rasters of a pair, in ONE geometry.
///
/// Every evidence statistic pairs source pixel `i` with target pixel `i`, and
/// [`structure_divergence`] abstains (returns `None`) when the two rasters
/// differ in length. Thumbnailing the two images independently let a
/// ONE-ROW difference decide that: a 1600x1067 source lands on 384x256 and a
/// 1600x1069 target on 384x257, the frame-wide divergence read as 0, the
/// same-content verdict came out true and no evidence range could ever be
/// withheld -- the calibration pair fitted 0.081 -> 0.018 with the gate silently
/// off, while the same pixels cropped to equal heights abstained at 0.057 (the
/// verdict the doctrine actually gives). So the target is thumbnailed into the
/// source's analysis geometry with the SAME operator the source went through:
/// `thumbnail(w, h)` is `resize_dimensions` + `thumbnail_exact`, image's box
/// filter where every full-resolution pixel lands in exactly one cell, so an
/// equal-shape pair is byte-for-byte the two thumbnails it always was, by
/// construction and not by a branch. The operator matters: a Lanczos3
/// `resize_exact` of the target against a box-filtered source keeps more
/// high-frequency energy on one side than the other, and on a same-scene
/// pair (a 1536x1027 preview against its 9504x6336 develop) that asymmetry
/// alone moved the fit from 0.092 -> 0.019 / conf 0.68 to 0.107 -> 0.034 /
/// conf 0.54 with no range withheld on either arm.
///
/// The target's PRESENCE rides with it: a target brought into the sensor
/// frame (`pipeline::fit_target`) carries an alpha that is opaque inside its
/// field and clear outside, and the same box filter carries it onto the
/// analysis grid, where [`presence_of`] reads it.
pub(crate) fn analysis_pair(src: &DynamicImage, target: &DynamicImage) -> (DynamicImage, DynamicImage) {
    let s_img = src.thumbnail(ANALYZE_EDGE, ANALYZE_EDGE);
    let t_img = target.thumbnail_exact(s_img.width(), s_img.height());
    (s_img, t_img)
}

/// Per-pixel presence of a paired target on its own grid: 1.0 where every
/// full-resolution pixel the cell averages lies inside the target's field,
/// 0.0 where any does not. `None` — present everywhere — for a target with no
/// alpha or a fully opaque one, which is every target that was not moved into
/// the sensor frame; that is what keeps those pairs on today's bytes.
///
/// Strict at the edge on purpose: a cell that is part field and part ring
/// averages real pixels with the edge values the ring holds, and pairing it
/// would be pairing a smear.
pub(crate) fn presence_of(target: &DynamicImage) -> Option<Vec<f32>> {
    if !target.color().has_alpha() {
        return None;
    }
    let alpha: Vec<f32> = match target {
        DynamicImage::ImageRgba8(t) => {
            t.pixels().map(|p| if p[3] == u8::MAX { 1.0 } else { 0.0 }).collect()
        }
        other => other
            .to_rgba16()
            .pixels()
            .map(|p| if p[3] >= u16::MAX - u16::MAX / 1000 { 1.0 } else { 0.0 })
            .collect(),
    };
    alpha.iter().any(|a| *a <= 0.0).then_some(alpha)
}

/// `weights` with every absent pixel's weight taken to zero — THE population
/// rule wherever a weight meets a target that carries presence. Borrowed when
/// every pixel is present, so such a pair runs on its own bytes.
pub(crate) fn with_presence<'w>(weights: &'w [f32], presence: Option<&[f32]>) -> Cow<'w, [f32]> {
    match presence {
        None => Cow::Borrowed(weights),
        Some(p) => Cow::Owned(
            weights.iter().enumerate().map(|(i, w)| w * p.get(i).copied().unwrap_or(0.0)).collect(),
        ),
    }
}

/// The most a fit may claim once [`same_frame_plausible`] has said no (R24
/// batch 2). The warning and this cap are ONE decision, for the same reason
/// [`FIT_FAR_ERR`] and [`CONFIDENCE_SLOPE`] are: printing "treat the result as
/// unreliable" beside a confidence of 0.83 states two contradictory things and
/// the user believes the number.
///
/// The 0.83 is real — measured on the cropped real pair of 2026-08-17
/// (`P43`, a portrait frame the user cropped to 1.294) — and so is the
/// blindness behind it: the joint reading handed that pair one of its BEST
/// scores (0.035), because value-range buckets correspond by value and simply
/// do not notice that the two populations came from different rectangles.
/// Neither of the fit's two readings can see this, so neither may set the
/// number.
///
/// WHY A CAP AND NOT THE FLOOR: at a 2% aspect tolerance the commonest
/// trigger is a crop of exactly the frame the user meant (see
/// [`SAME_FRAME_ASPECT_TOL`]), which is the Lightroom export the tooltip asks
/// for. Collapsing to [`CONFIDENCE_FLOOR`] would call the intended workflow
/// broken. A cap says "your numbers are not evidence", which is the true
/// claim, and leaves both ladders free to go LOWER when they have their own
/// reason to.
///
/// WHERE 0.5 COMES FROM — measured, not chosen. Forty crop-only pairs (eight
/// frames: four fixtures plus four of the real targets, each against five
/// centre-crops of ITSELF at 95/90/80/65/50% of height) put an IDENTICAL look
/// on both sides, so the truthful recipe is the identity and the truthful
/// residual is zero. The solver instead reported 0.796-0.950 confidence on
/// every one of them, and earned it by manufacturing real edits out of the
/// framing difference alone — up to +23.4 / −18.3 of saturation and ±0.45 EV.
/// The largest residual the framing alone manufactured was 0.0854, which
/// [`confidence_from_look_err`] reads as 0.488; 0.5 is that number, rounded to
/// a legible one. It is what the ladder itself says a fit is worth when its
/// whole residual could be framing.
pub(super) const NOT_SAME_FRAME_CONFIDENCE_CAP: f32 = 0.5;

/// The fit outcome: the recipe plus the evidence-weighted tonal, colour, hue
/// and spatial error (0 = identical supported look) before and after.
pub struct FitReport {
    pub recipe: EditRecipe,
    pub err_before: f32,
    pub err_after: f32,
    /// Global solve policy selected before any CDF fitting.
    pub mode: FitMode,
    /// Structural reading that selected `mode` (promotion may select
    /// Atmosphere even when this frame-global value is below its threshold).
    /// `None` is [`structure_divergence`]'s abstention — not a matched
    /// reading. In production this reading always resolves, because
    /// [`divergence_raster`] builds a fixed 384x256 grid whose all-ones core
    /// is 378x250 px; the option is here so no consumer can read a matched
    /// verdict off a frame nothing measured.
    pub divergence: Option<Divergence>,
    /// The LAYOUT-scale twin of [`Self::divergence`], measured on the same
    /// raster in the same call ([`divergence_pair_for`]). Nothing is gated on
    /// it — the mode line and every evidence gate read the fine number — but
    /// it is disclosed beside it, because the two scales disagreeing is a fact
    /// about the pair that used to be invisible. See [`COARSE_SIGMA_DIVISOR`].
    pub divergence_coarse: Option<Divergence>,
    /// Whether this solve was allowed to pair a source PIXEL with a target
    /// pixel, or only a source CELL with a target cell.
    pub pairing: PairingScale,
    /// The rationale as typed notes (L12#2B): `render_en(&notes)` is the
    /// recipe's `rationale` byte-for-byte (empty prose prefix — the fit
    /// rationale is fully deterministic), so the GUI renders it localized
    /// while every persisted surface keeps the English string. In-process
    /// only, never serialized.
    pub notes: Vec<crate::rationale::Note>,
    /// Fixed source/target evidence for every downstream zoned gate and the
    /// finished-render disclosure. In-process only, never serialized.
    pub(crate) evidence: EvidenceModel,
    /// The structural frame model retained by an Atmosphere report for the two
    /// consumers for which structure remains a fact: Full zones and detail.
    /// `None` in Full mode; in Atmosphere mode [`Self::evidence`] is the
    /// structure-blind population ruler and this field is `Some(structural)`.
    pub structural_evidence: Option<EvidenceModel>,
    /// Cross-image correspondence for a content-divergent pair, when the
    /// caller supplied a provider and the D gate consulted it (step 7b).
    /// In-process only, never serialized — the zoned passes read it.
    pub(crate) correspondence: Option<PairCorrespondence>,
    /// R30 R2: which population this report's Atmosphere white balance and
    /// exposure were read over. In-process only, never serialized; the
    /// consultation site reads it to tell R2-lite's unpaired share from an
    /// EXCLUDED one. Always [`AtmosphereReference::WholeFrame`] in Full mode,
    /// where the two controls do not exist.
    pub(crate) atmosphere_reference: AtmosphereReference,
}

/// What one correspondence field means FOR THIS PAIR's rasters: for every
/// source-thumbnail index, the target pixel its content corresponds to and
/// how much that match can be trusted. Derived once from the sidecar's 48x48
/// field ([`correspondence_for_pair`]); consumed by the zone estimators as a
/// pair-weight factor and a remapped target.
pub(crate) struct PairCorrespondence {
    /// Per-source-index confidence in [0, 1] (cyclic x smoothness).
    pub(crate) conf: Vec<f32>,
    /// The target rendition READ AT the corresponded position, one sample per
    /// source index — same-index pairing against this array is
    /// correspondence-aware pairing against the original.
    pub(crate) tp: Vec<[f32; 3]>,
    /// Share of the frame with a confident counterpart (conf >= [`CONFIDENT_MATCH`]).
    pub(crate) coverage: f32,
    /// Median per-cell confidence, for the disclosure.
    pub(crate) median: f32,
    /// R2-lite: the share of the TARGET grid no confident source cell maps
    /// onto. [`Self::coverage`] is the mirror-image, SOURCE-side reading;
    /// this one is what the Atmosphere global solve's whole-frame target
    /// median is exposed to. Grid resolution, never pixel resolution.
    pub(crate) target_unpaired: f32,
    /// R30 R2: the same fact as [`Self::target_unpaired`], kept as the MASK
    /// it was counted from and not only as its share — one derivation, two
    /// consumers (the disclosure's number and the Atmosphere solve's
    /// reference population, which must never be able to disagree). `1.0`
    /// where some confident source cell maps ONTO the target grid cell
    /// holding this analysis pixel, `0.0` where none does. Indexed like
    /// [`Self::conf`]: the two analysis rasters share one geometry
    /// ([`analysis_pair`]), so one index names the same rectangle on both.
    pub(crate) target_answered: Vec<f32>,
    /// The sidecar grid the two shares above were counted on, so the
    /// disclosure can state its own resolution.
    pub(crate) grid: (usize, usize),
}

/// The confidence at which a correspondence cell counts as a real match.
/// Named because two shares are now read off it and they must agree on where
/// the line is.
pub(crate) const CONFIDENT_MATCH: f32 = 0.5;

/// A caller-supplied way to obtain a correspondence field for one pair —
/// the CLI and GUI hand in a closure that runs the local DIFT sidecar
/// (`correspond::fit_provider`); tests hand in stubs; `None` (or an `Err`)
/// degrades to the pre-7b behaviour. The fit consults it ONLY on a
/// content-divergent pair (`divergence.d >= DIVERGENCE_GLOBAL`) — the gate
/// lives here, single-sourced, not at the callers.
pub type CorrespondenceProvider<'a> =
    &'a dyn Fn(&DynamicImage, &DynamicImage) -> anyhow::Result<crate::correspond::CorrespondenceField>;

/// Project one sidecar field onto THIS pair's rasters. Pure geometry:
/// every source pixel centre lands in one grid cell; its confidence is that
/// cell's, and its corresponded target sample is read at the cell's mapped
/// position PLUS the pixel's own within-cell offset (locally the flow is
/// rigid — the 16-px cells are far below the scale content moves at).
///
/// Under an IDENTITY field (every cell maps to itself, full confidence) and
/// equal raster dims, the output target array is BYTE-IDENTICAL to the input
/// — the conservation law the wiring's tests pin: a field that says
/// "nothing moved, everything corresponds" must change nothing. When the two
/// rasters' dims DIFFER (the calibration target is two rows taller than its
/// source), same-index pairing carries a small row shear and the normalised
/// remap quietly corrects it — a real improvement measured at ~0.015 EV on
/// the calibration land zone, and the reason the conservation tests pin the
/// law on geometry-normalised fixtures.
pub(crate) fn correspondence_for_pair(
    field: &crate::correspond::CorrespondenceField,
    tp: &[[f32; 3]],
    (sw, sh): (u32, u32),
    (tw, th): (u32, u32),
) -> PairCorrespondence {
    let (gw, gh) = (field.grid_w, field.grid_h);
    let n = (sw * sh) as usize;
    let mut conf = vec![0.0f32; n];
    let mut out = vec![[0.0f32; 3]; n];
    for y in 0..sh {
        for x in 0..sw {
            let i = (y * sw + x) as usize;
            let u = (x as f32 + 0.5) / sw as f32;
            let v = (y as f32 + 0.5) / sh as f32;
            let cx = ((u * gw as f32) as usize).min(gw - 1);
            let cy = ((v * gh as f32) as usize).min(gh - 1);
            let c = cy * gw + cx;
            conf[i] = field.confidence[c];
            let fu = u * gw as f32 - cx as f32;
            let fv = v * gh as f32 - cy as f32;
            let un = ((field.map_x[c] + fu) / gw as f32).clamp(0.0, 1.0);
            let vn = ((field.map_y[c] + fv) / gh as f32).clamp(0.0, 1.0);
            let tx = ((un * tw as f32) as u32).min(tw - 1);
            let ty = ((vn * th as f32) as u32).min(th - 1);
            out[i] = tp.get((ty * tw + tx) as usize).copied().unwrap_or([0.0; 3]);
        }
    }
    let coverage = if conf.is_empty() {
        0.0
    } else {
        conf.iter().filter(|&&c| c >= CONFIDENT_MATCH).count() as f32 / conf.len() as f32
    };
    let mut sorted = conf.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    // R2-lite: the same field read from the OTHER side. `coverage` says how
    // much of the SOURCE has a counterpart; the Atmosphere solve's unstated
    // assumption runs the other way, because both its medians are read over
    // the whole TARGET. So count the target cells that some confident source
    // cell actually maps onto — a target cell no source cell answers for is a
    // population the ratio `median(target)/median(source)` has no partner
    // for. Grid resolution by construction; the disclosure states the grid.
    let cells = gw * gh;
    let mut answered = vec![false; cells];
    for c in 0..cells.min(field.confidence.len()) {
        if field.confidence[c] >= CONFIDENT_MATCH {
            let tx = (field.map_x[c].max(0.0) as usize).min(gw - 1);
            let ty = (field.map_y[c].max(0.0) as usize).min(gh - 1);
            answered[ty * gw + tx] = true;
        }
    }
    let target_unpaired = if cells == 0 {
        0.0
    } else {
        1.0 - answered.iter().filter(|a| **a).count() as f32 / cells as f32
    };
    // R30 R2: project that same bitmap onto the analysis raster by the
    // identical nearest-cell rule the loop above uses, so the population the
    // solve drops and the share the rationale prints are ONE fact.
    let mut target_answered = vec![0.0f32; n];
    for y in 0..sh {
        for x in 0..sw {
            let u = (x as f32 + 0.5) / sw as f32;
            let v = (y as f32 + 0.5) / sh as f32;
            let cx = ((u * gw as f32) as usize).min(gw - 1);
            let cy = ((v * gh as f32) as usize).min(gh - 1);
            if answered[cy * gw + cx] {
                target_answered[(y * sw + x) as usize] = 1.0;
            }
        }
    }
    PairCorrespondence {
        conf,
        tp: out,
        coverage,
        median,
        target_unpaired,
        target_answered,
        grid: (gw, gh),
    }
}

/// R30 R2: the least of its OWN evidence mass either side's shared-content
/// population may retain before the Atmosphere global solve refuses to read
/// its two robust controls there and keeps the whole-frame reading — with a
/// sentence saying it had to.
///
/// Not a new number, and deliberately not a copy of one: this IS
/// [`EVIDENCE_RANGE_SURVIVAL_MIN`], the retention floor the evidence model
/// already applies to every luma range and hue band ("a population must keep
/// at least the support left at `DIVERGENCE_ZONE`"), pointed at the one
/// population that had never been asked the question. If the evidence
/// doctrine ever moves that line, this moves with it.
pub(crate) const SHARED_POPULATION_MIN_RETENTION: f32 = EVIDENCE_RANGE_SURVIVAL_MIN;

/// R30 R2: WHICH population an Atmosphere report's white balance and exposure
/// were actually read over. A solve fact — no later re-measurement of the
/// finished recipe can recover it — and the thing the rationale states.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum AtmosphereReference {
    /// No usable correspondence field for this pair: the whole frame, with
    /// the distribution-pairing assumption R2-lite disclosed and nothing
    /// available to qualify it.
    WholeFrame,
    /// A field existed and the shared-content sub-population kept enough of
    /// BOTH sides' evidence mass to be read: the two medians came from it.
    /// The numbers are the retained shares of source and target evidence mass.
    SharedContent { source: f32, target: f32 },
    /// A field existed but one of the two sides retains less than
    /// [`SHARED_POPULATION_MIN_RETENTION`] of its own evidence mass — too
    /// little to be read as a population. The whole-frame medians stand and
    /// the report says why they had to.
    Thin { source: f32, target: f32 },
}

/// The two restricted weight vectors plus what each side kept.
pub(crate) struct SharedPopulation {
    pub(super) source: Vec<f32>,
    pub(super) target: Vec<f32>,
    pub(super) source_retained: f32,
    pub(super) target_retained: f32,
}

impl SharedPopulation {
    /// Both sides kept enough of their own evidence mass to be read as a
    /// population rather than as a corner of one.
    pub(super) fn readable(&self) -> bool {
        self.source_retained >= SHARED_POPULATION_MIN_RETENTION
            && self.target_retained >= SHARED_POPULATION_MIN_RETENTION
    }
}

/// R30 R2: restrict the Atmosphere global solve's two reference populations
/// to the content the two frames actually SHARE.
///
/// `median(target) / median(source)` is a distribution-level pairing, and a
/// distribution-level pairing is only meaningful when the two distributions
/// describe the same content — precisely what selecting Atmosphere denies.
/// The correspondence field is the instrument that says which pixels do:
///
///   * TARGET side — a pixel some confident source cell maps ONTO is a
///     rendition of this frame. A pixel no cell answers for is not a rendition
///     of anything in it: it is generated content, and it cannot say what THIS
///     frame would look like developed differently. That is the population
///     R2-lite measured at 24% of the island pair's target and 93% of `p37`'s,
///     and the one its sentence says "defined those two controls all the same".
///   * SOURCE side — the mirror image, and NOT optional. Restricting only the
///     target moves one marginal onto a sub-population while the other stays
///     on the whole frame, and a ratio of medians read over two different
///     compositions is not a repair of the mismatched pairing, only a
///     different mismatch. Measured on a synthetic pair whose invented region
///     is the brighter 60% of the frame and therefore owns every whole-frame
///     median: the true cast is EV 0.00 / `gr/gb` 1.256, the whole-frame solve
///     answers +0.69 / 0.911, the target-only cut answers **−2.87 / 1.945**,
///     and the symmetric cut answers +0.03 / 1.216. The target-only failure is
///     larger than the defect it was meant to repair.
///
/// The cut is BINARY at [`CONFIDENT_MATCH`] rather than a confidence-
/// proportional down-weight, for a reason about what the number means: the
/// sidecar's confidence measures TRUST, not mass. Multiplying mass by trust
/// lets a large barely-trusted population outvote a small certain one (0.49
/// over 60% of a frame beats 1.00 over 20% of it) — the failure being
/// repaired, in a quieter form. Cutting at the line R2-lite already publishes
/// also keeps the disclosed share and the excluded population ONE fact.
///
/// Retention is measured on EVIDENCE MASS, not on pixel count: a pixel the
/// evidence model already weighted 0 was never in the reference population,
/// so it can neither be kept nor dropped from it.
///
/// `None` only when there is no evidence mass at all to restrict.
pub(super) fn shared_content_population(
    evidence: &EvidenceModel,
    c: &PairCorrespondence,
) -> Option<SharedPopulation> {
    let n = evidence
        .source_weights
        .len()
        .min(evidence.target_weights.len())
        .min(c.conf.len())
        .min(c.target_answered.len());
    let mut source = vec![0.0f32; n];
    let mut target = vec![0.0f32; n];
    let (mut kept_s, mut kept_t) = (0.0f64, 0.0f64);
    let (mut all_s, mut all_t) = (0.0f64, 0.0f64);
    for i in 0..n {
        let (ws, wt) =
            (evidence.source_weights[i].max(0.0), evidence.target_weights[i].max(0.0));
        all_s += ws as f64;
        all_t += wt as f64;
        if c.conf[i] >= CONFIDENT_MATCH {
            source[i] = evidence.source_weights[i];
            kept_s += ws as f64;
        }
        if c.target_answered[i] > 0.0 {
            target[i] = evidence.target_weights[i];
            kept_t += wt as f64;
        }
    }
    (all_s > 0.0 && all_t > 0.0).then(|| SharedPopulation {
        source,
        target,
        source_retained: (kept_s / all_s) as f32,
        target_retained: (kept_t / all_t) as f32,
    })
}
