//! Pixel statistics: pixels and luma, CDFs, the neutral-gate misprediction, quantiles, luma variance and mean chroma.

use super::*;

// --------------------------------------------------------------------------
// statistics primitives
// --------------------------------------------------------------------------

pub fn pixels_of(img: &DynamicImage) -> Vec<[f32; 3]> {
    img.to_rgb8()
        .pixels()
        .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
        .collect()
}

pub fn luma601(p: &[f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

#[cfg(test)]
fn cdf_from_values(values: impl Iterator<Item = f32>, n_hint: usize) -> Vec<f32> {
    let mut hist = vec![0.0f32; HIST_BINS];
    let mut n = 0usize;
    for v in values {
        let i = ((v.clamp(0.0, 1.0)) * (HIST_BINS - 1) as f32).round() as usize;
        hist[i] += 1.0;
        n += 1;
    }
    let total = (n.max(n_hint.min(1)) as f32).max(1.0);
    let mut acc = 0.0f32;
    for h in hist.iter_mut() {
        acc += *h;
        *h = acc / total;
    }
    hist
}

#[cfg(test)]
pub(super) fn luma_cdf(px: &[[f32; 3]]) -> Vec<f32> {
    cdf_from_values(px.iter().map(luma601), px.len())
}

/// Near-neutral gate for the TONE evidence (the cast catch-all fits on
/// ungated per-channel CDFs — see `residual_channel_curve`). Gated on HSV
/// saturation ((max−min)/max), which is INVARIANT under pure luminance
/// scaling — so the same pixels qualify in the source and in its tone-mapped
/// target (an absolute-chroma gate is not: dark colours slip under it in the
/// source and leave it once brightened, skewing the two CDFs against each
/// other). Near-black counts as neutral.
pub(super) fn is_neutralish(p: &[f32; 3]) -> bool {
    let mx = p[0].max(p[1]).max(p[2]);
    let mn = p[0].min(p[1]).min(p[2]);
    mx < 0.04 || (mx - mn) / mx < 0.15
}

/// Tone-evidence CDF pair. Near-neutral gating only carries clean evidence
/// when the SAME population is neutral on BOTH sides — the tone map is
/// quantile-to-quantile, so the gate is an identification assumption about
/// pixel correspondence, not a per-image preference. Three observed
/// breakages: a side's neutral sample is too small (< 5% or < 512 px —
/// noise); the neutral SHARES diverge, meaning the target re-hued (or
/// de-hued) part of the population — golden-sky pair, 2026-07-09: the
/// source's pale sky is neutralish ((max−min)/max ≈ 0.12), the target's
/// vivid gold one is not (≈ 0.37), and an asymmetric gate mapped the sky's
/// luma cluster across a ramp it doesn't belong to, distorting the whole
/// tone solve; or the shares stay COMPARABLE while a luma-CONCENTRATED band
/// churns out of one side's class — P20 × reimagine, 2026-08-12: the
/// target re-hued 24% of the base's neutral class (the pale sky, base-luma
/// q50 ≈ 197/255, → vivid blue), the share ratio read a passing 1.29×, and
/// the base's bright grey ranks paired against target ranks the sky no
/// longer belongs to — every upper-mid darkened and the render shipped
/// murky. Shares are a SIZE proxy, blind to composition (and the haze pair
/// proves one-sided CDF-shift proxies rank harm no better), so the gate is
/// judged by the harm itself: [`neutral_gate_misprediction`] scores the
/// gated evidence map against the shared class's own observable pairing.
/// Either way the assumption is dead: fall back to full-pixel CDFs on BOTH
/// sides (deciding per side, as the original code did, can even compare a
/// neutral-gated CDF against a full one). 1.75× keeps the
/// matched-population regressions (identity / roundtrip / violet canyon
/// ≈ 1.0×) while catching the golden-sky asymmetry (2.0×); the
/// misprediction ceiling is anchored in [`NEUTRAL_MISPREDICTION_MAX`]'s
/// doc. The two detectors are COMPLEMENTARY, not redundant (R18, measured
/// on the archive): the share ratio is alignment-free — it still works
/// when misregistration slides the misprediction metric toward 0 (its
/// fail-open direction) — while the misprediction gate catches membership
/// churn the ratio cannot see (P21 × reimagine-4: share 1.51×,
/// misprediction 0.034 — only this gate fires). A >1.75× share asymmetry
/// falls back UNCONDITIONALLY, by design: a low misprediction reading must
/// never override it, because misregistration fakes exactly that reading
/// (the fail-open direction), and the fallback itself is the safe arm —
/// on the two live pairs whose evidence failed a gate, the fallback solve
/// measured better than the gated one both times. The sentence that stood
/// here — that a benign uniform >1.75× inflation had only ever been seen on
/// a constructed frame, never on a real pair — was half right, and the
/// measurement replaces it. Neutral shares on both sides of every
/// calibration pair (2026-09-02):
///
/// | pair    | source | target | ratio |
/// |---------|--------|--------|-------|
/// | neutral | 0.7203 | 0.3073 | 2.34x |
/// | p36     | 0.3682 | 0.0733 | 5.02x |
/// | p37     | 0.3639 | 0.2884 | 1.26x |
/// | p38     | 0.3498 | 0.1668 | 2.10x |
/// | p39     | 0.7108 | 0.1925 | 3.69x |
/// | p40     | 0.4634 | 0.0184 | 25.13x |
/// | p41     | 0.2870 | 0.0638 | 4.50x |
///
/// So 6 of the 7 corpus pairs are over the line and the
/// unconditional fallback is an everyday arm rather than a contingency —
/// `p36` shows how a SAME-CONTENT pair gets there, its grade lifting chroma
/// until four fifths of the source's neutral class is no longer neutral on
/// the target side. What is still synthetic is the BENIGN case the old sentence
/// meant: two sides differing only by a uniform neutral-share scale over the
/// same composition, which only `a_uniformly_inflated_neutral_class_keeps_the_gate`
/// builds. Every real pair over the line is over it because the two sides
/// really do hold different colour populations, which is the reading the
/// fallback exists to distrust. Gate order: cheap counts and shares first,
/// the misprediction pass (a full-frame scan plus four CDFs) last, so
/// under-evidenced pairs never pay for it.
#[cfg(test)]
pub(super) fn tone_cdf_pair(sp: &[[f32; 3]], tp: &[[f32; 3]]) -> (Vec<f32>, Vec<f32>) {
    let s_n: Vec<f32> = sp.iter().filter(|p| is_neutralish(p)).map(luma601).collect();
    let t_n: Vec<f32> = tp.iter().filter(|p| is_neutralish(p)).map(luma601).collect();
    let share_s = s_n.len() as f32 / sp.len().max(1) as f32;
    let share_t = t_n.len() as f32 / tp.len().max(1) as f32;
    let gated = enough_evidence(s_n.len(), sp.len())
        && enough_evidence(t_n.len(), tp.len())
        && share_s.max(share_t) <= 1.75 * share_s.min(share_t)
        && neutral_gate_misprediction(sp, tp) <= NEUTRAL_MISPREDICTION_MAX;
    if gated {
        let (ns, nt) = (s_n.len(), t_n.len());
        (cdf_from_values(s_n.into_iter(), ns), cdf_from_values(t_n.into_iter(), nt))
    } else {
        (luma_cdf(sp), luma_cdf(tp))
    }
}

pub(super) fn tone_cdf_pair_weighted(
    sp: &[[f32; 3]],
    tp: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> (Vec<f32>, Vec<f32>) {
    let s_n: Vec<(f32, f32)> = sp
        .iter()
        .enumerate()
        .filter(|(_, p)| is_neutralish(p))
        .filter_map(|(i, p)| {
            let w = evidence.source_weights.get(i).copied().unwrap_or(0.0);
            (w > 0.0).then_some((luma601(p), w))
        })
        .collect();
    let t_n: Vec<(f32, f32)> = tp
        .iter()
        .enumerate()
        .filter(|(_, p)| is_neutralish(p))
        .filter_map(|(i, p)| {
            let w = evidence.target_weights.get(i).copied().unwrap_or(0.0);
            (w > 0.0).then_some((luma601(p), w))
        })
        .collect();
    let weighted = |v: &[(f32, f32)]| {
        let mut hist = vec![0.0f32; HIST_BINS];
        let total = v.iter().map(|(_, w)| *w).sum::<f32>();
        if total <= 1e-8 { return hist; }
        for &(x, w) in v {
            hist[(x.clamp(0.0, 1.0) * (HIST_BINS - 1) as f32).round() as usize] += w;
        }
        let mut acc = 0.0;
        for h in &mut hist { acc += *h; *h = acc / total; }
        hist
    };
    // The SAME identification gates the unweighted twin (`tone_cdf_pair`)
    // documents: the neutral gate only carries clean evidence when the same
    // population is neutral on BOTH sides. This arm shipped without them, and
    // the p36 calibration pair showed the cost live: the source's neutral
    // class is its dark rock, the target's is its bright sky, and the gated
    // quantile map sent 0.05 → 0.77 — a map the slider solve then pegged
    // itself against. Population parity is judged on the pixels that carry
    // evidence weight, floor and ratio exactly as the twin's contract states.
    let s_total = evidence.source_weights.iter().filter(|&&w| w > 0.0).count();
    let t_total = evidence.target_weights.iter().filter(|&&w| w > 0.0).count();
    let share_s = s_n.len() as f32 / s_total.max(1) as f32;
    let share_t = t_n.len() as f32 / t_total.max(1) as f32;
    let gated = enough_evidence(s_n.len(), s_total)
        && enough_evidence(t_n.len(), t_total)
        && share_s.max(share_t) <= 1.75 * share_s.min(share_t);
    let s = if gated { weighted(&s_n) } else { Vec::new() };
    let t = if gated { weighted(&t_n) } else { Vec::new() };
    if s.iter().any(|&v| v > 0.0) && t.iter().any(|&v| v > 0.0) {
        (s, t)
    } else {
        (weighted_cdf(sp, &evidence.source_weights, luma601), weighted_cdf(tp, &evidence.target_weights, luma601))
    }
}

/// The tone-evidence sample floor: at least 5% of the frame and never fewer
/// than 512 px. Shared between the per-side gate and the shared-class floor
/// inside [`neutral_gate_misprediction`], so "enough to trust" means one
/// thing.
fn enough_evidence(n: usize, total: usize) -> bool {
    n >= (total / 20).max(NEUTRAL_SHARED_MIN)
}

/// How badly the gated tone evidence MISPREDICTS the population it claims
/// to identify. The SHARED class — pixels neutral at the same position on
/// both sides — is the one population whose (source-luma, target-luma)
/// pairing is observable without any modelling: under a monotone tone map,
/// its own quantiles ARE the map. So build the production evidence map
/// exactly as the tone solve would (each side's whole neutral class,
/// quantile-paired) and score it against the shared class's empirical map:
/// mean |Δ| over the 21 look_err quantiles. Asymmetric members that merely
/// inflate a class along the shared luma ramp leave the pairing intact
/// (the synthetic uniform-inflation fixture reads < 0.0075); members that
/// churn in a luma-concentrated band bend the ranks and the misprediction
/// shows it directly — this is the murk, measured at its source.
///
/// POSITIONAL-CORRESPONDENCE ASSUMPTION, stated plainly because the rest of
/// this module deliberately avoids one (a generative target is not
/// pixel-aligned): co-membership at equal row-major index is read as "same
/// coarse region", which holds for same-frame pairs on the shared 384-edge
/// thumbnail grid and degrades with misregistration. The failure direction
/// is OPEN: under broken alignment target-membership decorrelates from
/// source-membership, the shared class becomes an unbiased thinning of both
/// sides, and the metric slides toward 0 — the gate is KEPT, not dropped,
/// and this detector goes vacuous (its sensitivity is proportional to
/// registration quality; the older share/size gates still stand in front
/// of it). Grids that disagree by more than aspect rounding (~a row) are
/// not comparable at all — that case returns infinite (fall back) as an
/// explicit decision rather than an emergent prefix artifact, and so does
/// a shared class too small to clear the same evidence floor the sides
/// must clear.
#[cfg(test)]
pub(crate) fn neutral_gate_misprediction(sp: &[[f32; 3]], tp: &[[f32; 3]]) -> f32 {
    let n = sp.len().min(tp.len());
    // 2% ≈ several rows of a 384-edge thumb: beyond aspect rounding, the
    // pairs come from different geometry and co-membership is meaningless.
    if sp.len().abs_diff(tp.len()) > n / 50 {
        return f32::INFINITY;
    }
    let (mut s_all, mut t_all) = (Vec::with_capacity(n / 2), Vec::with_capacity(n / 2));
    let (mut sh_s, mut sh_t) = (Vec::with_capacity(n / 2), Vec::with_capacity(n / 2));
    for i in 0..n {
        let (a, b) = (is_neutralish(&sp[i]), is_neutralish(&tp[i]));
        if a {
            s_all.push(luma601(&sp[i]));
        }
        if b {
            t_all.push(luma601(&tp[i]));
        }
        if a && b {
            sh_s.push(luma601(&sp[i]));
            sh_t.push(luma601(&tp[i]));
        }
    }
    if !enough_evidence(sh_s.len(), n) {
        return f32::INFINITY;
    }
    let (ns, nt, nsh) = (s_all.len(), t_all.len(), sh_s.len());
    let s_cdf = cdf_from_values(s_all.into_iter(), ns);
    let t_cdf = cdf_from_values(t_all.into_iter(), nt);
    let sh_s_cdf = cdf_from_values(sh_s.into_iter(), nsh);
    let sh_t_cdf = cdf_from_values(sh_t.into_iter(), nsh);
    let mut acc = 0.0f32;
    let mut cnt = 0.0f32;
    for i in 0..=20 {
        let p = (i as f32 / 20.0).clamp(P_CLIP, 1.0 - P_CLIP);
        let x = quantile(&sh_s_cdf, p);
        // The same formula the tone solve uses for its map (see `tone_map`).
        let predicted = quantile(&t_cdf, cdf_at(&s_cdf, x).clamp(P_CLIP, 1.0 - P_CLIP));
        let actual = quantile(&sh_t_cdf, p);
        acc += (predicted - actual).abs();
        cnt += 1.0;
    }
    acc / cnt
}

/// F(x): fraction of pixels ≤ x (linear interp between bins).
pub(crate) fn cdf_at(cdf: &[f32], x: f32) -> f32 {
    let pos = x.clamp(0.0, 1.0) * (cdf.len() - 1) as f32;
    let i = pos.floor() as usize;
    if i >= cdf.len() - 1 {
        return cdf[cdf.len() - 1];
    }
    let t = pos - i as f32;
    cdf[i] * (1.0 - t) + cdf[i + 1] * t
}

/// Q(p): the value at quantile `p` (inverse CDF, linear interp within the bin).
pub(crate) fn quantile(cdf: &[f32], p: f32) -> f32 {
    let n = cdf.len();
    let mut lo = 0usize;
    let mut hi = n - 1;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if cdf[mid] < p {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    // Interpolate within the step from the previous bin for a smooth inverse.
    if lo == 0 {
        return 0.0;
    }
    let (c0, c1) = (cdf[lo - 1], cdf[lo]);
    let t = if c1 > c0 { ((p - c0) / (c1 - c0)).clamp(0.0, 1.0) } else { 1.0 };
    ((lo - 1) as f32 + t) / (n - 1) as f32
}

/// Variance of the per-pixel channel mean — the degenerate-input probe
/// (see the refusal at the top of [`fit_recipe`]). Zero for an empty slice.
pub(super) const DEGENERATE_LUMA_VAR: f32 = 1e-6;
pub(super) fn luma_variance(px: &[[f32; 3]]) -> f32 {
    if px.is_empty() {
        return 0.0;
    }
    let n = px.len() as f32;
    let lum = |p: &[f32; 3]| (p[0] + p[1] + p[2]) / 3.0;
    let mean: f32 = px.iter().map(lum).sum::<f32>() / n;
    px.iter().map(|p| (lum(p) - mean).powi(2)).sum::<f32>() / n
}

pub(super) fn mean_chroma(px: &[[f32; 3]]) -> f32 {
    if px.is_empty() {
        return 0.0;
    }
    let sum: f32 =
        px.iter().map(|p| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])).sum();
    sum / px.len() as f32
}

/// CIE Lab, D65, from the analysis raster's display sRGB. Keep signed a/b:
/// equally strong warm/cool residuals have equal deltaE and opposite intent.
/// The one conversion the objective (J*) and the zoned rulers both read.
pub(crate) fn cie_lab(rgb: &[f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(render::srgb_to_linear);
    let f = |t: f32| if t > 216.0 / 24389.0 {
        t.cbrt()
    } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let x = f((0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047);
    let y = f(0.2126729 * r + 0.7151522 * g + 0.0721750 * b);
    let z = f((0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883);
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}
