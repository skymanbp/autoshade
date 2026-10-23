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
    // Per-band centroid hue disagreement — the WORST qualifying band, not a
    // weighted mean: one region with wrecked hue ruins a photo no matter how
    // small its area share (a lavender sky over perfect rocks), and an
    // area-weighted mean lets exactly that hide (measured: the violet-sky
    // curves slipped through the cast-acceptance gate on the mean variant).
    // |Δ| saturates at 60° so a fully-wrecked band reads 1.
    let (sa, ta) = band_stats_weighted(a, &evidence.source_hue_weights);
    let (sb, tb) = band_stats_weighted(b, &evidence.target_hue_weights);
    let mut hue = 0.0f32;
    let mut hue_weight = 0.0f32;
    if ta >= 1.0 && tb >= 1.0 {
        for i in 0..8 {
            let (x, y) = (&sa[i], &sb[i]);
            let range_weight = evidence.hue.get(i).map(|r| r.weight).unwrap_or(0.0);
            if range_weight <= 0.0 || x.w / ta < EVIDENCE_MIN_SHARE as f64 || y.w / tb < EVIDENCE_MIN_SHARE as f64 {
                continue;
            }
            let mut d = y.sin.atan2(y.cos).to_degrees() - x.sin.atan2(x.cos).to_degrees();
            while d > 180.0 {
                d -= 360.0;
            }
            while d < -180.0 {
                d += 360.0;
            }
            hue = hue.max((d.abs().min(60.0) / 60.0) as f32);
            hue_weight = hue_weight.max(range_weight);
        }
    }
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
