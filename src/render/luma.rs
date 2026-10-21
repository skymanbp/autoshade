//! Luma passes and blurs: chroma-preserving and additive luma writes, clarity and texture, and the box and Gaussian blurs.

use super::*;

/// Scale a pixel's chroma so its luma moves `l_old`→`l_new` while preserving hue.
pub(super) fn scale_chroma(px: &mut [f32; 3], l_old: f32, l_new: f32) {
    if l_old > 1e-4 {
        let k = l_new / l_old;
        px[0] = (px[0] * k).clamp(0.0, 1.0);
        px[1] = (px[1] * k).clamp(0.0, 1.0);
        px[2] = (px[2] * k).clamp(0.0, 1.0);
    } else {
        *px = [l_new, l_new, l_new];
    }
}

/// The four-tap bilinear read of a SCALAR plane at a fractional position, with
/// the sample grid clamped at the border.
///
/// One definition, because the tree grew several: `render/detail.rs` reads a
/// subsampled plane back up to full resolution with it and `stack/align.rs`
/// resamples a frame through a warp with it, and those two are the same four
/// taps and the same two lerps. Callers own the COORDINATE math — which
/// fraction of which grid a position is — and hand this function a position in
/// plane units; that is the part that legitimately differs between them.
///
/// Clamping rather than refusing is deliberate and is why the bounds question
/// stays with the caller: `detail.rs` is always inside by construction, while
/// an alignment warp really can walk off the frame and has to tell its merge
/// so ([`crate::stack::align::coverage`]). A single "return 0 outside" rule
/// would be wrong for both.
pub(crate) fn bilinear_plane(p: &[f32], w: usize, h: usize, u: f32, v: f32) -> f32 {
    let u = u.clamp(0.0, (w - 1) as f32);
    let v = v.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (u.floor() as usize, v.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let top = p[y0 * w + x0] * (1.0 - fx) + p[y0 * w + x1] * fx;
    let bot = p[y1 * w + x0] * (1.0 - fx) + p[y1 * w + x1] * fx;
    top * (1.0 - fy) + bot * fy
}

/// The four axis neighbours of `(x, y)` in a scalar plane — `[left, right, up,
/// down]` — with the border row and column replicated.
///
/// Shared for `bilinear_plane`'s reason: the sharpening pass's fine band and
/// the aligner's central differences are the same clamped four-neighbourhood,
/// and the `saturating_sub` / `.min(n - 1)` pair is exactly the kind of
/// boundary idiom that is correct in one copy and off by one in the next.
pub(crate) fn neighbours4(p: &[f32], w: usize, h: usize, x: usize, y: usize) -> [f32; 4] {
    [
        p[y * w + x.saturating_sub(1)],
        p[y * w + (x + 1).min(w - 1)],
        p[y.saturating_sub(1) * w + x],
        p[(y + 1).min(h - 1) * w + x],
    ]
}

/// Clarity's radius on a `w`×`h` raster: 2 % of the SHORT edge, never under 8
/// pixels.
///
/// One definition, because there are two callers — the Basic panel's Clarity
/// (stage 3) and the SDR rendition's (`hdr::apply`, v1.5.0 F8) — and they are
/// the SAME control at two points in the chain. Two copies of "0.02 · min(w,h)"
/// is how one of them would later be tuned and the other left behind, which is
/// exactly the split `texture_pass` was built to end.
pub(crate) fn clarity_radius(w: usize, h: usize) -> usize {
    ((0.02 * w.min(h) as f32).round() as usize).max(8)
}

/// The positive Texture radius: 0.5% of the short edge, floored at 2 px —
/// the small-radius sibling of [`clarity_radius`], named for the same reason
/// (the zoned fit's detail stage reads the band this radius defines, and a
/// reading taken at a second spelling of the radius would measure a band the
/// pass does not move).
pub(crate) fn texture_radius(w: usize, h: usize) -> usize {
    ((0.005 * w.min(h) as f32).round() as usize).max(2)
}

/// Unsharp mask on luminance (chroma-preserving). `amount` scales the detail;
/// `midtone` weights the effect toward midtones (for clarity).
pub(super) fn unsharp_luma(data: &mut [[f32; 3]], w: usize, h: usize, radius: usize, amount: f32, midtone: bool) {
    unsharp_luma_weighted(data, w, h, radius, amount, midtone, |_, _, _| 1.0);
}

/// [`unsharp_luma`] with a per-pixel weight — the LOCAL-mask form (clarity /
/// texture inside a mask). `weight(x, y, px)` receives the pixel as it stands
/// when the pass reaches it, so the caller can fold a Range Mask into the
/// geometric coverage; weights ≤ 0.001 skip the pixel untouched.
///
/// **The weighting is EXACT, not an approximation.** `unsharp_luma` scales a
/// pixel's RGB by `k = new_l/l` (`scale_chroma`), so the filtered pixel is
/// `p·k`. Mixing the original and the filtered result by weight `w` gives
/// `p·(1−w) + p·k·w = p·(1 + w(k−1))`, i.e. the pixel scaled by the
/// weight-interpolated ratio `1 + w(k−1)`. Attenuating the LUMA DIFFERENCE by
/// `w` before `scale_chroma` produces `new_l' = l + w·(new_l − l)`, whose ratio
/// is `k' = new_l'/l = 1 + w(k−1)` — the same number. So "weight the detail"
/// and "filter the whole frame, then blend by weight" are the same operation,
/// and this needs only the two f32 planes the filter already builds instead of
/// a full RGB copy of the frame to blend against (~732 MB at 61 MP).
/// (Exactness holds up to the two clamps `scale_chroma` and the `new_l` clamp
/// apply; at `w = 1` the arithmetic is bit-identical to `unsharp_luma` — test
/// `unsharp_weighted_at_weight_one_is_bit_identical`.)
pub(super) fn unsharp_luma_weighted(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    radius: usize,
    amount: f32,
    midtone: bool,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
) {
    if w == 0 || h == 0 {
        return; // par_chunks_mut(0) asserts; a 0-dim frame has no pixels anyway
    }
    let luma: Vec<f32> = data.par_iter().map(luma601).collect();
    let blurred = blur_plane(&luma, w, h, radius);
    write_luma_weighted(data, w, weight, |i, l, wgt| {
        let detail = l - blurred[i];
        let m = if midtone { 1.0 - (2.0 * l - 1.0).powi(2) } else { 1.0 };
        l + amount * detail * m * wgt
    });
}

/// The WRITE half every chroma-preserving luma pass shares: per pixel, ask
/// `weight(x, y, px)`, leave the pixel alone at ≤ 0.001, and otherwise move its
/// luma to `new_luma(i, l, wgt)` (clamped to 0..=1) with [`scale_chroma`].
///
/// `l` is read off the pixel itself, before it is written — the same number a
/// luma plane built at the top of the pass holds, since every pixel is read
/// before it is written. Clarity, texture and the Detail panel's passes
/// (`detail.rs`) are the callers; they differ only in the signal they build
/// and in what `new_luma` does with it.
pub(super) fn write_luma_weighted(
    data: &mut [[f32; 3]],
    w: usize,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
    new_luma: impl Fn(usize, f32, f32) -> f32 + Sync,
) {
    if w == 0 {
        return; // par_chunks_mut(0) asserts
    }
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let wgt = weight(x, y, px);
            if wgt <= 0.001 {
                continue;
            }
            let l = luma601(px);
            let new_l = new_luma(y * w + x, l, wgt).clamp(0.0, 1.0);
            scale_chroma(px, l, new_l);
        }
    });
}

/// [`write_luma_weighted`]'s ADDITIVE twin: the luma move `new_luma(i, l, wgt)
/// − l` is added to the three channels alike — neutral light put in or taken
/// out — instead of scaling the pixel's chroma with it. Capture sharpening
/// (`detail::sharpen`) is the caller since v1.6.5: on the Lightroom exports it
/// was calibrated against, the sharpening moved R−L and B−L by 0.13 / 0.38 of
/// the luma move, where the chroma-scaling write reads 0.69 / 0.89 on this
/// engine's render of the same frame — Lightroom adds light, it does not scale
/// colour. A channel that runs past 0 or 1 clips on its own, so a saturated
/// colour keeps its hue and loses a little of the move.
pub(super) fn write_luma_additive(
    data: &mut [[f32; 3]],
    w: usize,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
    new_luma: impl Fn(usize, f32, f32) -> f32 + Sync,
) {
    if w == 0 {
        return; // par_chunks_mut(0) asserts
    }
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let wgt = weight(x, y, px);
            if wgt <= 0.001 {
                continue;
            }
            let l = luma601(px);
            let d = new_luma(y * w + x, l, wgt).clamp(0.0, 1.0) - l;
            if d != 0.0 {
                for c in px.iter_mut() {
                    *c = (*c + d).clamp(0.0, 1.0);
                }
            }
        }
    });
}

/// Weight of the COARSE arm in the negative texture mix, and the fraction of
/// the render raster's short edge its Gaussian σ binds to (B8-2 §6-1, five-step
/// joint fit, 890 residuals, rms 0.0048).
///
/// Landmarks rather than bare digits: σ₁ = 12.99 px on a 4160 px short edge,
/// a half-power period of 69.3 px, and 36.1 % of the total depth.
const TEXTURE_COARSE_AMPLITUDE: f32 = 0.172_443;
const TEXTURE_COARSE_SIGMA_FRAC: f32 = 0.003_123_5;

/// The FINE arm — the one B8 missed entirely and B8-2 found under the capture
/// sharpening: σ₂ = 1.174 px at a 4160 px short edge, half-power period 6.3 px,
/// and **63.9 % of the total depth**. Losing it is why the R28 band form kept
/// 0.9992 of a 4 px pattern where Lightroom keeps 0.57.
const TEXTURE_FINE_AMPLITUDE: f32 = 0.304_888;
const TEXTURE_FINE_SIGMA_FRAC: f32 = 0.000_282_2;

/// The one free parameter of the DEPTH law, `w(t) = t(1+d)/(1+d·t)` (B8-2 §1
/// ruling 3). `w(1) = 1` is exact and free, so the endpoint is the plateau
/// `1 − (A₁+A₂) = 0.52267` with no epsilon.
const TEXTURE_DEPTH_D: f32 = 0.558_583;

/// Below this σ (in RENDER-raster pixels) an arm is dropped rather than
/// approximated — the ruling of 2026-08-21 (`r29-rulings-2026-08-20.md`
/// 拍板三), and the threshold is where a sampled kernel stops representing the
/// continuous Gaussian it stands for rather than a round number.
///
/// Measured, at the frequencies that matter: at σ = 0.49 the 4σ-truncated FIR
/// transfers 0.601 at Nyquist where the continuous `G` is 0.306, and at the
/// σ₂ = 0.2407 a 1280 px preview actually asks for (`gui/model.rs:296`,
/// short edge ≈ 853) the kernel collapses to `[1.8e−4, 1, 1.8e−4]` and
/// transfers 0.9993 — an identity wearing a Gaussian's name. At σ₂ = 1.174
/// (a 4160 px raster) the discrete and continuous responses agree to 4 dp.
/// So the clamp is not a behaviour cliff — it makes explicit what a sub-pixel
/// spatial kernel was going to do anyway, and stops the pass paying for it.
pub(super) const TEXTURE_MIN_SIGMA_PX: f32 = 0.5;

/// `w(t) = t(1+d)/(1+d·t)` — the negative half's DEPTH against the slider,
/// evaluated in f64 and returned in f32 (the constants are quoted to 6 digits;
/// f32 division would spend two of them for nothing).
///
/// **The linear reading is refuted, not merely improved on.** The engine's old
/// `strength = -amount` is exactly `w(t) = t` — bit-verified at
/// D(−50)/D(−100) = 0.5000 — where the five-step Lightroom ladder gives 0.605.
/// Even with the endpoint matched, linear under-depths −50 by 18 % and −10 by
/// 32 % (`w(0.5) = 0.609`, `w(0.1) = 0.148`). A single power law is refuted
/// too: the local exponent drifts 0.85 → 0.67 across the ladder, the best fit
/// `t^0.778` misses ±0.024 at the ends, and this one-parameter hyperbolic form
/// holds them to ±0.008 (B8-2 §6-4 items 1-2).
fn texture_depth(t: f32) -> f32 {
    let t = f64::from(t.clamp(0.0, 1.0));
    let d = f64::from(TEXTURE_DEPTH_D);
    (t * (1.0 + d) / (1.0 + d * t)) as f32
}

/// The `(σ_coarse, σ_fine)` the negative half runs at on a `w × h` raster.
///
/// **`min(w, h)` is the RENDER raster's short edge, and that is adjudicated
/// rather than assumed.** A two-resolution export pair — 6240 × 4160 and
/// 3120 × 2080, sidecars byte-identical but for the delivery size — separates
/// "σ is a fixed pixel count" from "σ is a fixed fraction of the short edge" by
/// **16×** (rms 0.0886 vs 0.0054 across 4 ≤ k ≤ 96), and a leave-out check
/// carries the full-size fit onto the half-size file with σ scaled by the
/// short-edge ratio for rms 0.0048 — its own in-sample residual (B8-2 §1
/// ruling 4). This engine is the architecture that makes that reading
/// unambiguous: the develop runs at FULL resolution and `--long-edge` resamples
/// the FINISHED pixels as the last stage (the CLI's flag reaches
/// [`ExportOpts::long_edge`], and `render_to_file` resizes the finished frame as
/// its last step), so the `(w, h)` handed to this pass IS the render raster and
/// never the delivery size.
///
/// What the two-resolution pair does NOT decide is whether σ tracks the FILM's
/// resolution or a fixed pixel count — every fixture came off one ARW, so both
/// readings predict the same numbers (B8-2 §7-1). The proportional form is kept
/// because at a single film resolution it introduces no known error and it is
/// what the pass already did.
pub(super) fn texture_sigmas(w: usize, h: usize) -> (f32, f32) {
    let short = w.min(h) as f32;
    (TEXTURE_COARSE_SIGMA_FRAC * short, TEXTURE_FINE_SIGMA_FRAC * short)
}

/// The integer box³ radius whose equivalent σ = √(r(r+1)) sits nearest `sigma`.
///
/// Closed form, not a search: `σ² = r(r+1)` inverts to
/// `r = (√(1+4σ²) − 1)/2`, and only the two integers around it can win.
fn box3_radius_for_sigma(sigma: f32) -> usize {
    // `is_finite` FIRST so a NaN σ leaves by this door rather than through a
    // comparison that is false either way.
    if !sigma.is_finite() || sigma <= 0.0 {
        return 0;
    }
    let s = f64::from(sigma);
    // The clamp bounds a hand-written frame size out of an overflowing `+ 1`
    // below; no raster reaches it (σ = 1e9 needs a 3.2e11 px short edge).
    let lo = (((1.0 + 4.0 * s * s).sqrt() - 1.0) * 0.5).floor().clamp(0.0, 1e9) as usize;
    let err = |r: usize| {
        let r = r as f64;
        ((r * (r + 1.0)).sqrt() - s).abs()
    };
    if err(lo) <= err(lo + 1) { lo } else { lo + 1 }
}

/// **The texture operator** — the ONE calibration the global stage
/// ([`apply_develop`] 3b) and the mask arm ([`apply_masks`]) both call, so
/// "Texture −40" cannot come to mean two different things depending on whether
/// a mask is in the way. `amount` is the slider ÷ 100 (−1..=1).
///
/// **POSITIVE — unchanged, and measured.** A plain unsharp mask at the
/// resolution-normalised radius both arms have shared since R25 B2 (0.5 % of
/// the short edge, floored at 2 px), with no midtone weighting:
/// `l + amount·(l − blur)`. R27 P2 measured this half against Lightroom and not
/// one character of it is touched here.
///
/// **NEGATIVE — measured against Lightroom and rebuilt to the measurement
/// (R29 B8-2, landed 2026-08-21).** See [`texture_negative_pass`] for the
/// model, the kernels and the evidence; this function only picks the σ pair.
///
/// **RENDER-BEHAVIOUR HARD CHANGE, every negative Texture value, global and
/// per mask.** The R28 Batch-5 band form this replaces was a notch —
/// `1 − |t|·(G_f − G_c)`, returning to 1 at BOTH spectral ends — designed with
/// no Lightroom ground truth in the tree. Two controlled ladders now say the
/// shape itself was wrong: Lightroom's negative Texture is a monotone
/// HIGH-SHELF. Recipes re-render; version snapshots keep the old pixels. The
/// sidecar still carries the raw slider value so Lightroom re-renders it with
/// its own model — the same stance [`manual_vignette_lut`] takes.
pub(super) fn texture_pass(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    amount: f32,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
) {
    if amount >= 0.0 {
        let radius = texture_radius(w, h);
        unsharp_luma_weighted(data, w, h, radius, amount, false, weight);
        return;
    }
    let (sigma_coarse, sigma_fine) = texture_sigmas(w, h);
    texture_negative_pass(data, w, h, -amount, sigma_coarse, sigma_fine, weight);
}

/// The negative half, at an explicit σ pair — **two low-passes mixed in
/// PARALLEL, scaled by a hyperbolic depth law**:
///
/// ```text
///   l' = l − w(t)·[ A₁·(l − G_σ₁∗l) + A₂·(l − G_σ₂∗l) ]
/// ```
///
/// `t = |slider|/100`. Parallel, NOT cascaded: the two high-passes are summed,
/// not composed, and the arms carry 36 % / 64 % of the depth. Free-refitting
/// the old cascade band form against the same ground truth lands 8.2× worse
/// (rms 0.0392 vs 0.0048) — a wrong function family, not mistuned constants
/// (B8-2 §1 ruling 5).
///
/// **Why σ is a parameter here and not read off `(w, h)`.** The acceptance
/// grid is defined on a 4160 px short edge; a test that had to build a
/// 4160 × 4160 frame to reach it would cost 200 MB to assert nine numbers.
/// Splitting the σ choice ([`texture_sigmas`]) from the filter lets the anchor
/// test drive the real filter at the real σ on a 2048 × 64 strip.
///
/// **The kernels, and why they are not the same kernel.** The anchor grid is
/// the arbiter, and it rejected the cheap answer:
///
/// | scheme | max dev vs the closed form, 45 anchors |
/// |---|---|
/// | box³ both arms, integer radius | **0.0443 — fails** |
/// | box³ both arms, fractional (extended-box) radius | **0.0373 — fails** |
/// | box³ coarse + true Gaussian FIR fine | 0.0088 |
/// | **as shipped** (below) | **0.0037** |
///
/// σ₂ = 1.174 px is simply not on the box³ grid — the nearest integer radius
/// (r = 1) is σ = 1.414, and no fractional-radius box³ has the right SHAPE at
/// that support either (its sinc³ transfer reads 0.037 at a 4 px period where
/// the Gaussian reads 0.183). So the fine arm is a real separable Gaussian FIR
/// ([`gauss_blur_plane`]), and the coarse arm stays on the O(N) box³ the whole
/// file already uses, where at σ₁ ≈ 13 px the shape error is a rounding
/// difference.
///
/// **`coarse` is still grown FROM `fine`, and that is the parallel model, not a
/// cascade.** Gaussians compose — `G_σ₁ = G_σ₂ ∗ G_√(σ₁²−σ₂²)` — so blurring
/// the fine plane by the residual σ′ = 12.941 px produces exactly the coarse
/// arm the formula asks for, while the luma plane dies before the coarse blur
/// starts. The pass therefore holds the same TWO f32 planes the old band form
/// did (`jobs::PER_PHOTO_PEAK_COMMIT_MB` unmoved), and `l` is recomputed from
/// the pixel — the same number the dropped luma plane held, since every pixel
/// is read before it is written.
///
/// **Cost of the FIR arm**, stated rather than hoped: the kernel is 2⌈4σ₂⌉+1
/// taps, so 11 at a 4160 px short edge and 17 at 61 MP — 0.57 and 2.05 G
/// multiply-adds across both separable passes, against ~0.7 G for the entire
/// box³ chain. It is row-parallel like every other plane pass here.
///
/// **The domain is load-bearing.** The fit holds in the sRGB-gamma domain and
/// diverges by 0.041 at a 4 px period in linear light (B8-2 §6-3), so this pass
/// must run on gamma-encoded pixels. It does: the develop's buffer is
/// sRGB-encoded before `apply_develop` is ever called — every buffer
/// `develop_raw_buffer` returns carries the sRGB transfer, and
/// `calibrate_camera_buffer`'s last line is that encode.
///
/// **Where the model is honest about not applying.** Lightroom's operator is
/// amplitude-adaptive — not LTI: H spans 0.33 → 0.85 with detail amplitude
/// inside one octave on the clean base, against ≤ 0.009 for an LTI control. A
/// fixed kernel can only match the ENSEMBLE, which is what the anchor grid is
/// (512-block cross-spectrum over one 6240 × 4160 frame). Edge-preserving
/// behaviour is not modelled here and is registered, not silently claimed.
///
/// **Preview fidelity, and the promise that does not hold on this branch.**
/// R25 B2 promised one slider value means one structure at a 1280 px preview
/// and at 61 MP. On the negative half it still does not — the reason has
/// changed from "`fine_radius = radius/4` degenerates to 1" to "σ₂ is
/// sub-pixel". At the GUI preview raster (`gui/model.rs:296`, short edge ≈ 853)
/// σ₂ = 0.241 px, so [`TEXTURE_MIN_SIGMA_PX`] drops the fine arm and the
/// preview's negative Texture is WEAKER than the export's by up to 0.021 in
/// transfer at a 4 px preview period, 0.036 at 3 px and 0.076 at its Nyquist.
/// User ruling of 2026-08-21: clamp and disclose, no approximation and no 1:1
/// patch render. The export is exact; the preview is honestly weaker.
///
/// Below a 228 px short edge the coarse arm's own box³ radius rounds to 0 and
/// the whole negative half becomes a no-op — a thumbnail has no mid band left
/// to take out, and a radius-0 box³ would have silently applied the FINE arm's
/// high-pass at the COARSE arm's amplitude.
pub(super) fn texture_negative_pass(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    t: f32,
    sigma_coarse: f32,
    sigma_fine: f32,
    weight: impl Fn(usize, usize, &[f32; 3]) -> f32 + Sync,
) {
    if w == 0 || h == 0 {
        return; // par_chunks_mut(0) asserts; a 0-dim frame has no pixels anyway
    }
    let fine_on = sigma_fine >= TEXTURE_MIN_SIGMA_PX;
    // The coarse arm is grown from the fine plane, so what it must supply is the
    // RESIDUAL σ′ = √(σ₁²−σ₂²) — and the whole σ₁ when the fine arm was clamped
    // out and the plane is still the raw luma.
    let residual = if fine_on {
        (sigma_coarse * sigma_coarse - sigma_fine * sigma_fine).max(0.0).sqrt()
    } else {
        sigma_coarse
    };
    let coarse_r =
        if sigma_coarse >= TEXTURE_MIN_SIGMA_PX { box3_radius_for_sigma(residual) } else { 0 };
    let coarse_on = coarse_r >= 1;
    let depth = texture_depth(t);
    if (!fine_on && !coarse_on) || depth <= 0.0 {
        return;
    }
    // Either the fine arm's plane or — when it is clamped out — the luma itself.
    // Either way it is what the coarse arm is grown from, and either way the
    // luma plane is gone by the time the coarse blur allocates.
    let fine = {
        let luma: Vec<f32> = data.par_iter().map(luma601).collect();
        if fine_on { gauss_blur_plane(&luma, w, h, sigma_fine) } else { luma }
    };
    let coarse = if coarse_on { Some(blur_plane(&fine, w, h, coarse_r)) } else { None };
    write_luma_weighted(data, w, weight, |i, l, wgt| {
        // `fine` holding the unblurred luma would make this term exactly
        // zero on its own (same function, same pixel, read before written);
        // the guard states the arm is OFF rather than leaving a reader to
        // rediscover that.
        let hp_fine = if fine_on { l - fine[i] } else { 0.0 };
        let hp_coarse = coarse.as_ref().map_or(0.0, |c| l - c[i]);
        let mix = TEXTURE_COARSE_AMPLITUDE * hp_coarse + TEXTURE_FINE_AMPLITUDE * hp_fine;
        l - depth * mix * wgt
    });
}


/// Approximate a Gaussian blur with 3 separable box-blur passes. Box blur uses a
/// running sum, so cost is O(N) regardless of `radius` — essential for clarity's
/// large radius on a 60 MP frame.
pub(crate) fn blur_plane(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    // Zero-dim guard: the box-blur seeds use `Ord::clamp(0, w-1)`, which PANICS
    // when w or h is 0 (min > max). No caller produces a 0-dim buffer today —
    // this turns a future one into a no-op instead of a crash.
    if radius == 0 || w == 0 || h == 0 {
        return src.to_vec();
    }
    // The first pass reads the caller's plane directly — the old
    // `src.to_vec()` seed was a full extra plane (~240 MB at 61 MP) copied
    // only to be replaced by the first horizontal pass (A7). Bit-identical:
    // that pass reads exactly the same values either way.
    let mut buf = box_blur_h(src, w, h, radius);
    buf = box_blur_v(&buf, w, h, radius);
    for _ in 0..2 {
        buf = box_blur_h(&buf, w, h, radius);
        buf = box_blur_v(&buf, w, h, radius);
    }
    buf
}

/// A TRUE separable Gaussian blur — the one place in this file that cannot use
/// [`blur_plane`], because the σ it is asked for is small enough that box³'s
/// shape stops being an approximation of a Gaussian and starts being a
/// different filter.
///
/// [`texture_negative_pass`] is the caller and its doc carries the arbitration:
/// at σ = 1.174 px a fractional-radius box³ transfers 0.037 at a 4 px period
/// where the Gaussian transfers 0.183, which alone misses the acceptance grid
/// by 0.037 against a ±0.02 budget. Above ~5 px the two agree to a rounding
/// difference and `blur_plane`'s O(N) running sums are the right tool; this is
/// for the other end.
///
/// O(taps) per pixel per axis rather than O(1), so the kernel truncation is
/// also the cost: 2⌈4σ⌉+1 taps, the tail beyond 4σ being `exp(−8) = 3.4e−4` of
/// the peak and renormalised away. Both passes are row-parallel and the
/// vertical one accumulates row-major, for the same cache reason
/// [`box_blur_v`] gives.
pub(crate) fn gauss_blur_plane(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if w == 0 || h == 0 {
        return src.to_vec();
    }
    // A kernel wider than the plane buys nothing: past the edge every tap reads
    // the same clamped sample.
    let Some(kernel) = gauss_kernel(sigma, w.max(h)) else {
        return src.to_vec();
    };
    let r = kernel.len() / 2;
    let mut mid = vec![0.0f32; src.len()];
    mid.par_chunks_mut(w).enumerate().for_each(|(y, orow)| {
        let row = &src[y * w..][..w];
        for (x, o) in orow.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            if x >= r && x + r < w {
                // Interior — no clamp in the hot loop. Same tap ORDER as the
                // border arm, so the two are bit-identical where they meet.
                for (k, wk) in kernel.iter().enumerate() {
                    acc += row[x + k - r] * wk;
                }
            } else {
                for (k, wk) in kernel.iter().enumerate() {
                    acc += row[(x + k).saturating_sub(r).min(w - 1)] * wk;
                }
            }
            *o = acc;
        }
    });
    let mut out = vec![0.0f32; src.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, orow)| {
        for (k, wk) in kernel.iter().enumerate() {
            let row = &mid[(y + k).saturating_sub(r).min(h - 1) * w..][..w];
            for (o, v) in orow.iter_mut().zip(row) {
                *o += v * wk;
            }
        }
    });
    out
}

/// The normalised 1-D Gaussian taps for `sigma`, or `None` when there is no
/// kernel to build. Summed and normalised in f64: the taps are quoted to f32 in
/// the end, but a kernel whose weights do not sum to 1 is a DC gain error, and
/// that is the one error a blur must not have.
fn gauss_kernel(sigma: f32, max_radius: usize) -> Option<Vec<f32>> {
    if !sigma.is_finite() || sigma <= 0.0 {
        return None;
    }
    let s = f64::from(sigma);
    let r = ((4.0 * s).ceil() as usize).clamp(1, max_radius.max(1));
    let mut k: Vec<f64> = (0..=2 * r)
        .map(|i| {
            let d = i as f64 - r as f64;
            (-0.5 * (d / s) * (d / s)).exp()
        })
        .collect();
    let sum: f64 = k.iter().sum();
    if !sum.is_finite() || sum <= 0.0 {
        return None;
    }
    for v in k.iter_mut() {
        *v /= sum;
    }
    Some(k.into_iter().map(|v| v as f32).collect())
}

pub(super) fn box_blur_h(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    debug_assert_eq!(src.len(), w * h);
    let mut out = vec![0.0f32; src.len()];
    let r = radius as isize;
    let win = (2 * radius + 1) as f32;
    // Rows are independent → parallel (row count now comes from the chunking,
    // not `h`); the per-row arithmetic order is exactly the serial version's,
    // so the result is bit-identical.
    out.par_chunks_mut(w).enumerate().for_each(|(y, orow)| {
        let base = y * w;
        let mut sum = 0.0f32;
        for k in -r..=r {
            sum += src[base + k.clamp(0, w as isize - 1) as usize];
        }
        orow[0] = sum / win;
        for (x, o) in orow.iter_mut().enumerate().skip(1) {
            let add = (x as isize + r).min(w as isize - 1) as usize;
            let sub = (x as isize - 1 - r).max(0) as usize;
            sum += src[base + add] - src[base + sub];
            *o = sum / win;
        }
    });
    out
}

pub(super) fn box_blur_v(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; src.len()];
    let r = radius as isize;
    let win = (2 * radius + 1) as f32;
    // Row-major with one running sum PER COLUMN: the old column-major walk
    // strode 4·w bytes on all three access streams, so export-sized planes
    // fell out of cache into DRAM latency for most of the pass. Every access
    // below is sequential, and each column's adds/subs happen in the exact
    // order of the old per-column walk — the result is bit-identical.
    let mut sums = vec![0.0f32; w];
    for k in -r..=r {
        let row = &src[k.clamp(0, h as isize - 1) as usize * w..][..w];
        for (s, v) in sums.iter_mut().zip(row) {
            *s += v;
        }
    }
    for (o, s) in out[..w].iter_mut().zip(&sums) {
        *o = s / win;
    }
    for y in 1..h {
        let add = &src[(y as isize + r).min(h as isize - 1) as usize * w..][..w];
        let sub = &src[(y as isize - 1 - r).max(0) as usize * w..][..w];
        let orow = &mut out[y * w..][..w];
        for x in 0..w {
            sums[x] += add[x] - sub[x];
            orow[x] = sums[x] / win;
        }
    }
    out
}

// ---------------------------------------------------------------------------
