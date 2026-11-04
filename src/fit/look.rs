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

/// Evidence-weighted look error used by every fit decision.  The structural
/// term makes the objective sensitive to spatially wrong pixels; the weighted
/// distribution terms ignore ranges that are one-sided or structurally
/// divergent instead of silently treating them as a match.
pub(crate) fn look_err_with_evidence(
    a: &[[f32; 3]],
    b: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> f32 {
    if evidence.identifiability <= 1e-5 {
        return 1.0;
    }
    let tonal = tonal_err_with_evidence(a, b, evidence);
    let colour = (0..3)
        .map(|ch| {
            (weighted_mean(a, &evidence.source_weights, ch).unwrap_or(0.0)
                - weighted_mean(b, &evidence.target_weights, ch).unwrap_or(0.0))
                .abs()
        })
        .sum::<f32>()
        / 3.0;
    let base = 0.55 * tonal + 0.20 * colour;
    let (hue, hue_weight) = band_colour_offset(a, b, evidence);
    let (w, h) = (evidence.width, evidence.height);
    // A PENALTY term: with no reading there is nothing to charge. That is what
    // the abstention says here — not that the structure survived.
    let structural = if w > 0 && h > 0 && evidence.source_weights.len() == a.len() {
        structure_divergence(a, b, w, h, &evidence.source_weights).map_or(0.0, |r| r.d)
    } else {
        0.0
    };
    base + 0.15 * hue * hue_weight.min(1.0) + 0.10 * structural.min(1.0)
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
/// Membership is unchanged — the display-HSL band partition and its chroma
/// gate (`band_stats_weighted`), the [`EVIDENCE_MIN_SHARE`] floor on both
/// sides and the range's evidence weight — so the same pixels qualify the
/// same bands; only the distance moved into CIE units. Returns (the worst
/// band's offset, the largest qualifying band's evidence weight).
fn band_colour_offset(a: &[[f32; 3]], b: &[[f32; 3]], evidence: &EvidenceModel) -> (f32, f32) {
    let (sa, ta) = band_ab_weighted(a, &evidence.source_hue_weights);
    let (sb, tb) = band_ab_weighted(b, &evidence.target_hue_weights);
    let (mut worst, mut weight) = (0.0f32, 0.0f32);
    if ta < 1.0 || tb < 1.0 {
        return (worst, weight);
    }
    for i in 0..8 {
        let (x, y) = (&sa[i], &sb[i]);
        let range_weight = evidence.hue.get(i).map(|r| r.weight).unwrap_or(0.0);
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

/// The TONAL term of [`look_err_with_evidence`] on its own: the mean absolute
/// gap between the two frames' evidence-weighted luma CDFs at 21 quantiles.
/// It is the term a tone stage controls, and so the ruler a tone stage is
/// held to (2026-10-21): the sliders and the residual curve each ship only
/// where they leave this reading no worse than they found it. Judging them on
/// the whole objective charged a correct luma move for the colour-mean drift
/// that brightening a muted frame toward a vivid target necessarily brings —
/// drift the stages after it exist to correct — while the lantern street's
/// wrong +1.65 EV raised this term 0.0468 → 0.0748 on its own and needs no
/// other term to be refused.
pub(crate) fn tonal_err_with_evidence(
    a: &[[f32; 3]],
    b: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> f32 {
    let (ca, cb) = (
        weighted_cdf(a, &evidence.source_weights, luma601),
        weighted_cdf(b, &evidence.target_weights, luma601),
    );
    let mut tonal = 0.0f32;
    let mut n = 0.0f32;
    for i in 0..=20 {
        let p = (i as f32 / 20.0).clamp(P_CLIP, 1.0 - P_CLIP);
        tonal += (quantile(&ca, p) - quantile(&cb, p)).abs();
        n += 1.0;
    }
    tonal / n
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
