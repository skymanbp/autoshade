//! Look error, with and without evidence, and the calibration corpus and recipe the tests read.

use super::*;


/// Slice-only test helper for the production evidence-weighted objective.
/// Production callers build the evidence model with the real analysis
/// dimensions and keep it fixed for the whole fit.
#[cfg(test)]
pub(crate) fn look_err(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    let evidence = evidence_model(a, b);
    look_err_with_evidence(a, b, &evidence)
}

/// THE look error every fit decision reads, J* (v1.18.0), in the units the
/// eye uses: 1 dE76 of local-mean offset over the whole paired frame is 0.0075.
///
/// `J* = 0.75·[p·E_paired + (1 − p)·E_dist] + 0.15·H_ab·w + 0.10·S`
///
/// * `E_paired` — where pixels PAIR (index i on one side is index i on the
///   other: [`EvidenceModel::paired`]), the evidence-weighted mean CIE76
///   distance between the two sides' LOCAL MEANS,
///   `Σ w_i·ΔE76(Lab(G_2*a)_i, Lab(G_2*b)_i) / 100 / Σ w_i`, `w` the pixel's
///   PRESENCE weight (spatial support × membership) and `G_2`
///   the σ 2 px Gaussian the structural instrument pools its gradients at
///   ([`PAIRED_POOL_SIGMA`]). Local means, not raw pixels: the eye reads a
///   mean offset under texture (a 0.81 dE band is visible under 4 dE of star
///   noise), and on the disagreement census the σ 2 reading kept the
///   per-pixel sign on 39 of 41 verdicts, the two it flipped being star
///   texture a tile did not make (rel1180 msg1 §b).
///   Who pairs is decided by the finest mode that was read, both ways
///   (v1.18.0 R2, `fit_zoned`'s entry): an Atmosphere zone's pixels never
///   pair, even in a Full frame, and a Full zone's pixels pair by its own
///   structural model, even in an Atmosphere frame; pixels in no zone that
///   was read pair as the frame's mode says.
/// * `E_dist = sqrt(T_L² + T_a² + T_b²) / 100` — where pixels do not pair
///   (Atmosphere, a structure-blind model), the CIE76 distance between the
///   two populations' quantile-matched Lab distributions: per channel the
///   mean absolute gap of the two weighted CDFs at the same 21 quantiles
///   ([`P_CLIP`]-clamped) `T_L` reads on L* (`lightness_cdf_gap`),
///   on L*, a* and b* each, in that channel's own units. A uniform offset of
///   d dE76 moves every quantile by d, so it reads d/100 here exactly as it
///   reads d/100 in `E_paired`, paired or not (v1.18.0 R15). The form it
///   replaces, `0.55·T_L + 0.20·C_ab` lifted by 1/0.75, read one dE of mean
///   colour offset as 0.001 of J* against `E_paired`'s 0.0075, so on a frame
///   where nothing pairs a white balance that moved 97 % of the frame 3-5
///   dE/px closer earned less than [`FIT_QUANT`] and was eliminated
///   (rel1180 msg7, R12's terms).
/// * `p` — the paired share of the evidence mass: the two populations are
///   read in one unit, so the 0.75 weighs their mass-weighted mean.
/// * `H_ab` — the worst qualifying band's mean colour offset in units of its
///   target chroma ([`band_colour_offset`]), times that range's evidence
///   weight `w`.
/// * `S` — the structural divergence D ([`structure_divergence`]) under its
///   tie rule: two luma values within one output code are a tie; a tie is
///   charged in proportion to the fraction of a code that separates them,
///   never as a whole rank step. A PENALTY term: with no reading there is
///   nothing to charge — that is what the abstention says, not that the
///   structure survived.
///
/// WEIGHTS (v1.18.0, R11): the objective JUDGES every present pixel at its
/// presence weight — spatial support × membership, 1 for a frame pixel
/// ([`EvidenceModel::source_presence_weights`]) — in every term: `E_paired`,
/// `E_dist`'s `T_L`, `T_a` and `T_b`, `H_ab`'s band membership, offset and band
/// weight, `S`, and the paired share `p`. The evidence model's per-range
/// estimation weights only DRIVE (which ranges may move a control's
/// estimate); a ship decision read at them is blind to whatever range the
/// evidence withheld, and a tie there goes to absence however much the photo
/// gained (rel1180 msg6: p41's cloud-edge band, +4-5.7 dE/px at evidence
/// weight 0.000-0.21).
///
/// S-MODE: S is read where the question is structure — mode and evidence
/// (`DIVERGENCE_GLOBAL` / `DIVERGENCE_ZONE`, `MODE_MARGIN`) and the
/// comparisons of two different GLOBAL solves (terminal do-no-harm, the 4b
/// step-down, base against fit). The do-no-harm of a LOCAL correction (zone,
/// tile, band, field, tint) reads `E_paired + H_ab` ([`LookTerms::local`],
/// on a model whose [`EvidenceModel::local`] is set); its seams stay with the
/// boundary / ramp rulers; its structural gate is the existing D threshold
/// only.
///
/// LOCAL SHIP LAW (R19): a LOCAL correction ships when it earns FIT_QUANT on
/// its OWN population — this objective read over the region's own present
/// pixels and bands ([`EvidenceModel::scoped`]: `E_paired + H_ab`, the
/// region's pairing, its own worst band) — and the frame reading is a guard
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
    if evidence.identifiability <= 1e-5 {
        return 1.0;
    }
    let terms = look_terms(a, b, evidence);
    if evidence.local { terms.local() } else { terms.total() }
}

/// The σ of the local mean `E_paired` reads: the σ 2 px pool of the
/// structural instrument's gradient maps ([`structure_divergence`]), the
/// scale at which that instrument already decides two rasters show the same
/// thing. No new number.
pub(crate) const PAIRED_POOL_SIGMA: f32 = 2.0;

/// The terms of [`look_err_with_evidence`] (J*), each in its own unit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LookTerms {
    /// `E_paired`, dE76 / 100 over the paired population.
    pub paired: f32,
    /// `E_dist`, the Lab quantile-distribution dE76 / 100 over the unpaired
    /// population.
    pub dist: f32,
    /// `p`, the paired share of the evidence mass.
    pub share: f32,
    /// `H_ab·w`, already weighted by the qualifying bands' range evidence.
    pub hue: f32,
    /// `S`, the structural divergence capped at 1.
    pub structural: f32,
}

impl LookTerms {
    /// J*.
    pub(crate) fn total(&self) -> f32 {
        self.local() + 0.10 * self.structural
    }

    /// J* as a LOCAL correction's do-no-harm reads it (S-mode,
    /// [`EvidenceModel::local`]): `E_paired + H_ab` in J*'s own weights, the
    /// structural term left to the D threshold and the seam / ramp rulers.
    pub(crate) fn local(&self) -> f32 {
        0.75 * (self.share * self.paired + (1.0 - self.share) * self.dist) + 0.15 * self.hue
    }
}

/// [`look_err_with_evidence`]'s terms, read once.
pub(crate) fn look_terms(a: &[[f32; 3]], b: &[[f32; 3]], evidence: &EvidenceModel) -> LookTerms {
    let n = a.len().min(b.len());
    let weight = |i: usize| evidence.source_presence_weights.get(i).copied().unwrap_or(0.0).max(0.0);
    let pairs = |i: usize| evidence.paired.get(i).copied().unwrap_or(false);
    let (mut mass, mut paired_mass) = (0.0f64, 0.0f64);
    for i in 0..n {
        mass += weight(i) as f64;
        if pairs(i) {
            paired_mass += weight(i) as f64;
        }
    }
    let share = if mass > 0.0 { (paired_mass / mass) as f32 } else { 0.0 };
    let paired = if paired_mass > 0.0 {
        let (w, h) = (evidence.width as usize, evidence.height as usize);
        let local = |px: &[[f32; 3]]| -> Vec<[f32; 3]> {
            if w * h == px.len() {
                low_pass_pixels(px, w, h, PAIRED_POOL_SIGMA)
            } else {
                px.to_vec()
            }
        };
        let (la, lb) = (local(&a[..n]), local(&b[..n]));
        let mut sum = 0.0f64;
        for i in (0..n).filter(|&i| pairs(i)) {
            let wi = weight(i) as f64;
            if wi <= 0.0 {
                continue;
            }
            let (x, y) = (cie_lab(&la[i]), cie_lab(&lb[i]));
            let de = ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt();
            sum += wi * de as f64;
        }
        (sum / paired_mass / 100.0) as f32
    } else {
        0.0
    };
    let dist = if share < 1.0 {
        let unpaired = |weights: &[f32]| -> Vec<f32> {
            (0..n).map(|i| if pairs(i) { 0.0 } else { weights.get(i).copied().unwrap_or(0.0) }).collect()
        };
        let (sw, tw) = (unpaired(&evidence.source_presence_weights), unpaired(&evidence.target_presence_weights));
        let (la, lb): (Vec<[f32; 3]>, Vec<[f32; 3]>) =
            (a[..n].iter().map(cie_lab).collect(), b[..n].iter().map(cie_lab).collect());
        let tonal = lightness_cdf_gap(&la, &sw, &lb, &tw);
        let (ta, tb) = (opponent_cdf_gap(&la, &sw, &lb, &tw, 1), opponent_cdf_gap(&la, &sw, &lb, &tw, 2));
        (tonal * tonal + ta * ta + tb * tb).sqrt()
    } else {
        0.0
    };
    let (hue, hue_weight) = band_colour_offset(a, b, evidence);
    let (w, h) = (evidence.width, evidence.height);
    let structural = if !evidence.local && w > 0 && h > 0 && evidence.source_presence_weights.len() == a.len() {
        structure_divergence(a, b, w, h, &evidence.source_presence_weights).map_or(0.0, |r| r.d)
    } else {
        0.0
    };
    LookTerms { paired, dist, share, hue: hue * hue_weight.min(1.0), structural: structural.min(1.0) }
}

/// `T_L`: the mean absolute gap between two weighted L*/100 CDFs at 21
/// quantiles (each clamped to [`P_CLIP`]), over CIE Lab pixels.
fn lightness_cdf_gap(la: &[[f32; 3]], wa: &[f32], lb: &[[f32; 3]], wb: &[f32]) -> f32 {
    let lightness = |p: &[f32; 3]| p[0] / 100.0;
    let (ca, cb) = (weighted_cdf(la, wa, lightness), weighted_cdf(lb, wb, lightness));
    let mut gap = 0.0f32;
    for i in 0..=20 {
        let q = (i as f32 / 20.0).clamp(P_CLIP, 1.0 - P_CLIP);
        gap += (quantile(&ca, q) - quantile(&cb, q)).abs();
    }
    gap / 21.0
}

/// The 21-quantile gap of the opponent channel `ch` (1 = a*, 2 = b*) over
/// 100, as [`lightness_cdf_gap`] reads L*. a* and b* are signed, so the
/// channel is mapped onto the CDF's [0, 1] by the span both populations'
/// present pixels occupy and the gap is carried back by that span: the
/// mapping is exact, it sets only the bins' width, and it adds no number.
fn opponent_cdf_gap(la: &[[f32; 3]], wa: &[f32], lb: &[[f32; 3]], wb: &[f32], ch: usize) -> f32 {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for (px, w) in [(la, wa), (lb, wb)] {
        for (p, &wi) in px.iter().zip(w) {
            if wi > 0.0 {
                lo = lo.min(p[ch]);
                hi = hi.max(p[ch]);
            }
        }
    }
    let span = hi - lo;
    if !span.is_finite() || span <= 1e-6 {
        return 0.0;
    }
    let value = |p: &[f32; 3]| (p[ch] - lo) / span;
    let (ca, cb) = (weighted_cdf(la, wa, value), weighted_cdf(lb, wb, value));
    let mut gap = 0.0f32;
    for i in 0..=20 {
        let q = (i as f32 / 20.0).clamp(P_CLIP, 1.0 - P_CLIP);
        gap += (quantile(&ca, q) - quantile(&cb, q)).abs();
    }
    gap / 21.0 * span / 100.0
}

/// H_ab, the colour term's WORST band (v1.18.0): per qualifying HSL band,
/// the distance between the two sides' mean CIE a*, b* over the band's
/// members, in units of the TARGET band's own chroma — `min(|Δ(mean a*,
/// mean b*)| / C*_target, 1)` with `C*_target = |(mean a*, mean b*)|` of the
/// target's members — and the worst such band, not a weighted mean: one
/// region with wrecked colour ruins a photo no matter how small its area
/// share (a lavender sky over perfect rocks), and an area-weighted mean lets
/// exactly that hide (measured: the violet-sky curves slipped through the
/// cast-acceptance gate on the mean variant).
///
/// The reading it replaces took the band's centroid HUE ANGLE alone,
/// saturating at 60°, so a band whose chroma overshot the target at the
/// right angle read as matched (rel1180 msg1: a land tile's a* went -3.8 ->
/// +4.4 and was admitted because its centroid angle moved closer). The 60°
/// saturation transfers exactly: a rotation of 60° at chroma C is a chord of
/// length C, so a band rotated by the old saturation reads 1 here as it did
/// there, and an offset in chroma now counts as an offset in angle does.
///
/// Membership is the display-HSL band partition and its chroma gate
/// (`band_stats_weighted`) and the [`EVIDENCE_MIN_SHARE`] floor on both
/// sides, every pixel at its PRESENCE weight (R11); a band's weight is its
/// two-sided presence share (`two_sided_share`: the range's evidence weight
/// before the estimation verdict withholds it). A band the evidence withholds
/// from estimation is still judged. Returns (the worst band's offset, the
/// largest qualifying band's two-sided presence share).
fn band_colour_offset(a: &[[f32; 3]], b: &[[f32; 3]], evidence: &EvidenceModel) -> (f32, f32) {
    let (sa, ta) = band_ab_weighted(a, &evidence.source_presence_weights);
    let (sb, tb) = band_ab_weighted(b, &evidence.target_presence_weights);
    let (mut worst, mut weight) = (0.0f32, 0.0f32);
    if ta < 1.0 || tb < 1.0 {
        return (worst, weight);
    }
    for i in 0..8 {
        let (x, y) = (&sa[i], &sb[i]);
        let range_weight = evidence.hue.get(i).map(|r| r.two_sided_share).unwrap_or(0.0);
        if range_weight <= 0.0 || x[0] / ta < EVIDENCE_MIN_SHARE as f64 || y[0] / tb < EVIDENCE_MIN_SHARE as f64 {
            continue;
        }
        let mean = |s: &[f64; 3]| (s[1] / s[0], s[2] / s[0]);
        let ((xa, xb), (ya, yb)) = (mean(x), mean(y));
        let chroma = ya.hypot(yb);
        let offset = (ya - xa).hypot(yb - xb);
        // A band whose target mean sits at the neutral axis has no chroma to
        // measure in; any offset there is a whole band's worth of wrong colour.
        let read = if chroma > 1e-6 { (offset / chroma).min(1.0) } else if offset > 1e-6 { 1.0 } else { 0.0 };
        worst = worst.max(read as f32);
        weight = weight.max(range_weight);
    }
    (worst, weight)
}

/// Per-band [weight, Σ weight·a*, Σ weight·b*] over the display-HSL band
/// partition `band_stats_weighted` uses (the same chroma gate, the same
/// two-band bracketing), and the total weight of the pixels that entered.
fn band_ab_weighted(px: &[[f32; 3]], weights: &[f32]) -> ([[f64; 3]; 8], f64) {
    let mut bands = [[0.0f64; 3]; 8];
    let mut total = 0.0f64;
    for (i, p) in px.iter().enumerate() {
        let weight = weights.get(i).copied().unwrap_or(0.0).max(0.0) as f64;
        if weight <= 0.0 {
            continue;
        }
        let chroma = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
        if chroma < 0.06 {
            continue;
        }
        let (h, _, _) = render::rgb_to_hsl(p[0], p[1], p[2]);
        let (b0, b1, w1) = render::bracket_bands(h * 360.0, &render::HSL_CENTERS);
        let lab = cie_lab(p);
        for (bi, w) in [(b0, 1.0 - w1 as f64), (b1, w1 as f64)] {
            let w = w * weight;
            let band = &mut bands[bi];
            band[0] += w;
            band[1] += w * lab[1] as f64;
            band[2] += w * lab[2] as f64;
        }
        total += weight;
    }
    (bands, total)
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
