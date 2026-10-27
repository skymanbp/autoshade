//! Per-zone local-contrast matching (2026-10-21): the one thing the zone
//! stages never asked. A zone's tone solve matches its luma DISTRIBUTION and
//! its colour MOMENTS; a quantile map is monotone and pointwise, so it cannot
//! tell "the target's bricks have twice the contrast of mine" from "the
//! target is a bit brighter in the midtones" — every zone shipped with its
//! texture and clarity at 0 whatever the target showed. This stage reads the
//! LOCAL contrast of both frames at the engine's own two scales and solves
//! the two sliders that act at exactly those scales, closed loop, on real
//! renders.
//!
//! THE READING. Both frames at [`DETAIL_EDGE`] (the analysis size's radii
//! are floors, not scales: at 384x256 the texture radius rounds to its 2-px
//! floor, 0.8% of the short edge against the 0.5% it means, and clarity to
//! its 8-px floor, 3.1% against 2%; at 768 both land on their ratio). The
//! Rec.601 luma of each side is split into TWO bands with the engine's own
//! box^3 blur ([`render::texture_radius`], [`render::clarity_radius`],
//! [`render::blur_plane`] — ONE definition): the FINE band, luma minus its
//! blur at the texture radius, which is what the positive texture pass
//! multiplies by `1 + amount` (to within a few percent: box^3 is not a
//! projection, so a little of the band survives a second blur), and the MID
//! band, the blur at the texture radius minus the blur at the clarity
//! radius. The mid band is read on purpose instead of the clarity pass's own
//! operand (luma minus the clarity blur): that operand CONTAINS the fine
//! band, and a re-synthesised texture — unpaired at the fine scale by
//! definition — then biases the coarse reading toward "softer than the
//! target" and asks for a negative clarity the layout never asked for
//! (measured on the synthetic re-synthesised fixture: clarity −31 shipped
//! against a paired layout). Inside the zone (the source membership times
//! the target membership, so a pixel counts only where the same region sits
//! on both sides), two numbers per band:
//!
//! * the CORRELATION `r` of the two bands — do the target's bumps sit where
//!   this photo's bumps sit? This is the evidence gate ([`DETAIL_PAIRED_MIN`]),
//!   and ONLY the gate.
//! * the GAIN the band asks ([`asked_gain`]): the regression slope of the
//!   target's band on the render's, in units of each zone's mean luma (`r`
//!   times the ratio of the two normalised contrasts) — the gain this
//!   photo's OWN structure needs where the two frames' structure
//!   corresponds. Deliberately not the ratio of the two contrasts alone:
//!   that counts the target's unpaired energy (its grain, a re-synthesised
//!   texture, JPEG halos), which no slider on this photo can produce.
//!   Measured with the ratio as the gain (2026-10-21, `fc_score.py final4
//!   detail3`): the busy street's ground, whose fine texture is drawn (r
//!   0.02) and whose mid band pairs at 0.65 with the target at 1.37x the
//!   render's contrast, took clarity +80 and its lanterns ended harder than
//!   the target's; the slope reads 0.89 there. The slope is asked only when
//!   the ratio agrees on its DIRECTION: a target busier overall whose
//!   corresponding structure is softer than this photo's (slope < 1 <
//!   ratio) carries its extra contrast somewhere other than on this photo's
//!   structure, and softening what corresponds would move the photo away
//!   from a busier target — that band is left alone and says so
//!   (`DETAIL_NOT_ON_STRUCTURE`). The first cut's one worse frame (the busy
//!   street's building at clarity −52 for a slope of 0.92, deltaE 13.03 →
//!   13.62) was the unbounded secant below, not the estimator.
//!
//! CALIBRATED on the eleven final-check pairs (`detail_calib.py`, lane
//! ground-9663, 2026-10-21), zones with real paired structure read r 0.83 to
//! 0.99 on both bands. A re-synthesised zone fails the FINE band by a wide
//! margin — the night pair's AI-drawn sky 0.03, its repainted ground 0.22,
//! the desert pair 0.01 and 0.04 — while its MID band tells the layout's own
//! story: the night sky 0.66 at a gain of 0.99 (the layout is kept and
//! already matched: nothing ships), the night ground 0.81 at 1.08 (the
//! repaint kept the ground's shapes and drew them a little harder), the
//! desert 0.45 and 0.24 (refused). The line sits between the paired and the
//! re-synthesised populations on the fine band with a margin on each side,
//! and each band is gated on its own. What the paired zones ASK for is
//! modest and two-sided: the busy street's land 1.13/1.10 and sky 1.21/1.18,
//! the sunset city's land 1.07/1.34, the blue hour 0.89 to 0.96 (the render
//! is SHARPER than the target there, so the sliders go negative).
//!
//! WHAT THE NIGHT PAIR TAUGHT, so nobody tries again: its target's brightest
//! fifty stars carry 154x the sky background's light in a 5x5 window, the
//! RAW decoded plainly 0.50x at the same places (0.32x after the AI
//! denoise). The light is not in the photograph; the generator drew it. A
//! slider amplifies what is there — every combination of the three detail
//! sliders that lifted those stars to 60% of the target's contrast put the
//! flat sky's noise at 2.3x the target's (27 combinations, `detail_grid.py`).
//! The correlation gate is what refuses that zone (0.03), and the sentence
//! it prints says why.
//!
//! WHY TEXTURE AND CLARITY, AND NOT SHARPNESS. Texture and clarity are
//! defined as a share of the short edge, so a reading at 768 px and the
//! export at 61 MP mean the same thing by "Texture 30". Capture sharpening's
//! radius is in FILM pixels ([`render::FilmScale`]): sigma 0.75 of the
//! negative's pixels is 0.06 px at this reading and is not a quantity this
//! stage can measure, and the target — an AI rendition or an export of one
//! to four megapixels — has no 1:1 pixels of the negative to compare it
//! against. A value nothing measured would be a guess with a sidecar.
//!
//! AND WHY TEXTURE ONLY ON ITS POSITIVE HALF. The negative half is a
//! different operator, fitted to Lightroom's ([`render::texture_negative_pass`]):
//! two low-passes at sigma 13 px and 1.17 px of a 4160-px short edge, the
//! fine arm carrying 64% of the depth. Both are FILM-scale: at this reading
//! the fine arm is 0.14 px and clamped out, the coarse arm 1.6 px, so the
//! render barely moves and a closed loop drives the slider to its stop
//! (measured on the synthetic softer-target fixture: texture −86, then
//! refused). The export is not much different in kind — on a band 0.5% of
//! the short edge wide the negative pass at −100 takes off a sixth at most —
//! and the target has no pixels at the scale it works on. A target that is
//! softer at the texture scale is therefore read, disclosed (the before/after
//! shares in `DETAIL_ATTACHED`), and left to the clarity solve, whose
//! operand contains the fine band and whose negative half is the same
//! unsharp mask with its sign turned ([`DETAIL_TEXTURE_RANGE`]).
//!
//! THE SOLVE is closed loop, as every zone control is: the open-loop
//! texture answer is exact (`1 + amount`) but clarity rides a midtone mask
//! and each slider leaks into the other's band, so the sliders are stepped
//! from a response prior, the band is re-read on the real render, and the
//! measured response replaces the prior (a secant, [`DETAIL_STEPS`] renders
//! at most). Then the gates every zone control answers to: the local-quality
//! gate on the zone's own coverage (texture ratio inside its window, no
//! clipping growth), the band energy ceiling ([`DETAIL_ENERGY_TOL`] — the
//! render's band may not end above the target's, on EVERY band both frames
//! carry structure in, paired or not: clarity leaks into the fine band as
//! much as it moves the mid band, and an unpaired fine band is exactly the
//! one whose energy must not be pushed past the target's), and the frame's
//! do-no-harm ([`ZONE_GLOBAL_REGRESSION_TOL`] on the report's own ruler). A
//! refusal is answered with a shrink ladder ([`DETAIL_SHRINK_LADDER`]) before
//! it is a refusal; what ships is disclosed with the two correlations and
//! the render's contrast before and after, as a share of the target's.
//!
//! The sliders ride on a mask of their OWN with the zone's geometry (the sky
//! and land zones' Select Sky component, a region's raster), not on the tone
//! masks: a zone's tone may have been refused, shrunk, or split into bands,
//! and its local contrast is a separate question with a separate answer.
//! Lightroom carries both sliders (`LocalTexture`, `LocalClarity2012`) on
//! every mask type this fit emits.

use super::*;
use crate::render::{blur_plane, clarity_radius, texture_radius};

/// The reading's long edge. See the module doc: the analysis size clamps
/// both radii to their floors; this is the smallest power-of-two edge at
/// which both land on their defined share of the short edge (texture 3 px =
/// 0.59%, clarity 10 px = 1.95% at 768x512).
pub(crate) const DETAIL_EDGE: u32 = 768;
/// The pairing line on the band correlation, both scales. Calibrated:
/// paired zones read >= 0.83, re-synthesised ones <= 0.45 (module doc).
pub(crate) const DETAIL_PAIRED_MIN: f32 = 0.6;
/// A band whose asked gain is within this of unity is already matched: a
/// five-point slider on a Lightroom integer scale.
pub(crate) const DETAIL_MATCHED_GAIN: f32 = 0.05;
/// A side whose normalised band contrast (std / mean luma) is under this
/// carries no structure to pair: 0.2% of the zone's own brightness, under
/// one 8-bit code at mid-grey.
pub(crate) const DETAIL_MIN_CONTRAST: f32 = 0.002;
/// Closed-loop renders per zone at most.
pub(crate) const DETAIL_STEPS: usize = 3;
/// The render's band contrast may not end above the target's by more than
/// this share: the solve aims at the target's contrast, and the tolerance
/// covers the secant's last step and the 8-bit render.
pub(crate) const DETAIL_ENERGY_TOL: f32 = 0.05;
/// The shrink ladder a refused solve is retried on, as shares of the solved
/// sliders. The tone ladder's steps, without the zero rung: a zero slider is
/// a refusal, and is disclosed as one.
pub(crate) const DETAIL_SHRINK_LADDER: [f32; 3] = [0.75, 0.5, 0.25];
/// Response prior per slider unit, gain per point: texture's positive half is
/// 0.01 (the band is multiplied by `1 + amount`); clarity rides a midtone
/// mask that reads about 0.6 over an ordinary zone. Both are replaced by the
/// measured response after the first render.
const DETAIL_SLOPE_PRIOR: [f32; 2] = [0.010, 0.006];
/// The slider ranges the solve may use: texture on its positive half only
/// (module doc, "why texture only on its positive half"), clarity both ways.
pub(crate) const DETAIL_TEXTURE_RANGE: (f32, f32) = (0.0, 100.0);
pub(crate) const DETAIL_CLARITY_RANGE: (f32, f32) = (-100.0, 100.0);
/// The measured response may replace the prior only inside this window of
/// it (a quarter to four times), and one step may move a slider by at most
/// [`DETAIL_STEP_MAX`] points: outside that, the reading is the render's
/// quantisation, not the slider's response.
pub(crate) const DETAIL_SLOPE_BOUNDS: (f32, f32) = (0.25, 4.0);
pub(crate) const DETAIL_STEP_MAX: f32 = 40.0;

/// One zone the stage may read: its rationale label, the mask the sliders
/// will ride, and the two membership rasters (source and target) at whatever
/// size the segmenter produced them.
pub(super) struct DetailZone {
    pub(super) label: String,
    pub(super) name: String,
    pub(super) role: MaskRole,
    pub(super) mask: MaskGeometry,
    pub(super) components: Vec<MaskComponent>,
    pub(super) inverted: bool,
    pub(super) source: GrayImage,
    pub(super) target: GrayImage,
}

/// The two-region route's zones: the sky on the Select Sky component, the
/// land on the same component inverted — exactly the carriers the tone
/// zones ride ([`sky_land_geometry`]).
pub(super) fn sky_land_zones(
    src_mask: &GrayImage,
    tgt_mask: &GrayImage,
    mask_path: &crate::store::OwnedRaster,
) -> Vec<DetailZone> {
    let geometry = sky_land_geometry(src_mask, mask_path);
    [(MaskRole::ZoneSky, false), (MaskRole::ZoneLand, true)]
        .into_iter()
        .map(|(role, inverted)| DetailZone {
            label: role.tag().to_string(),
            name: String::new(),
            role,
            mask: geometry.clone(),
            components: Vec::new(),
            inverted,
            source: src_mask.clone(),
            target: tgt_mask.clone(),
        })
        .collect()
}

/// The multi-region route's zones, one per resolved class on its own raster.
pub(super) fn region_zones(
    regions: &[semantic::SemanticRegion],
    rasters: &[crate::store::OwnedRaster],
) -> Vec<DetailZone> {
    regions
        .iter()
        .zip(rasters)
        .map(|(region, raster)| DetailZone {
            label: format!("region-{}-{}", region.class_id, region.label),
            name: format!("region-{}-{}", region.class_id, region.label),
            role: MaskRole::Custom,
            mask: MaskGeometry::Bitmap { path: raster.path().to_string_lossy().into_owned() },
            components: Vec::new(),
            inverted: false,
            source: region.source.clone(),
            target: region.target.clone(),
        })
        .collect()
}

/// One band's reading: the pairing correlation, each side's normalised
/// contrast (band std per unit of the zone's mean luma), and their ratio —
/// the gain the render needs at this scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BandReading {
    pub(crate) r: f32,
    pub(crate) target: f32,
    pub(crate) render: f32,
}

impl BandReading {
    /// The ratio of the two normalised contrasts, target over render; unity
    /// where the render carries nothing.
    pub(crate) fn ratio(self) -> f32 {
        if self.render > 0.0 { self.target / self.render } else { 1.0 }
    }

    /// The regression slope of the target's band on the render's in units
    /// of each zone's mean luma: the gain the shared structure asks.
    pub(crate) fn paired_gain(self) -> f32 {
        self.r * self.ratio()
    }
}

/// The band-energy ceiling, judged on every band with structure on both
/// sides because the sliders leak across bands. A PAIRED band may rise to
/// the target's energy. An UNPAIRED band may not rise at all: the target's
/// energy there is not this photo's structure (drawn stars, a re-synthesised
/// texture), so it is no ceiling, and nothing asks for more of this photo's
/// — what a clarity move adds there is leak, the grain and the banding of a
/// flat sky (the night pair's sky took clarity +53 for a mid band it pairs
/// with at 0.62, its unpaired fine band rose 0.38 → 0.52 of the target's,
/// and the frame's deltaE went 5.22 → 8.05 while every gate passed,
/// 2026-10-21). A band that already stood above its ceiling and was not
/// raised (the fine band under a negative clarity) is not a push.
pub(crate) fn pushed_past_evidence(paired: bool, before: BandReading, after: BandReading) -> bool {
    let ceiling = if paired { after.target } else { before.render };
    after.render > before.render && after.render > ceiling * (1.0 + DETAIL_ENERGY_TOL)
}

/// The gain a band asks of its slider: the slope on the shared structure,
/// and only when the whole band's contrast ratio agrees on the direction
/// (module doc). Unity — no ask — where they straddle it.
pub(crate) fn asked_gain(band: BandReading) -> f32 {
    let (slope, ratio) = (band.paired_gain(), band.ratio());
    if (slope - 1.0) * (ratio - 1.0) > 0.0 { slope } else { 1.0 }
}

/// The two bands — `[fine, mid]`: luma minus its blur at the texture radius,
/// and that blur minus the blur at the clarity radius — of a render against
/// the target inside `weights`, at the engine's radii for a `w`x`h` frame.
pub(crate) fn band_readings(
    render: &[[f32; 3]],
    target: &[[f32; 3]],
    weights: &[f32],
    w: u32,
    h: u32,
) -> [BandReading; 2] {
    let (wu, hu) = (w as usize, h as usize);
    let n = (wu * hu).min(render.len()).min(target.len()).min(weights.len());
    let luma = |px: &[[f32; 3]]| -> Vec<f32> {
        px[..n].iter().map(|p| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]).collect()
    };
    let (lr, lt) = (luma(render), luma(target));
    let weight = |i: usize| f64::from(weights[i].max(0.0));
    let total: f64 = (0..n).map(weight).sum();
    let mean = |l: &[f32]| -> f64 {
        if total <= 0.0 { 0.0 } else { (0..n).map(|i| weight(i) * f64::from(l[i])).sum::<f64>() / total }
    };
    let (mr, mt) = (mean(&lr), mean(&lt));
    if total <= 0.0 || n < wu * hu {
        return [BandReading { r: 0.0, target: 0.0, render: 0.0 }; 2];
    }
    // The two blurs per side, once; the bands are their differences.
    let (r_tex, r_cla) = (texture_radius(wu, hu), clarity_radius(wu, hu));
    let (br_tex, bt_tex) = (blur_plane(&lr, wu, hu, r_tex), blur_plane(&lt, wu, hu, r_tex));
    let (br_cla, bt_cla) = (blur_plane(&lr, wu, hu, r_cla), blur_plane(&lt, wu, hu, r_cla));
    [0usize, 1].map(|band| {
        let (mut sr, mut st, mut srr, mut stt, mut srt) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for i in 0..n {
            let wt = weight(i);
            if wt == 0.0 {
                continue;
            }
            let (hr, ht) = if band == 0 {
                (f64::from(lr[i] - br_tex[i]), f64::from(lt[i] - bt_tex[i]))
            } else {
                (f64::from(br_tex[i] - br_cla[i]), f64::from(bt_tex[i] - bt_cla[i]))
            };
            sr += wt * hr;
            st += wt * ht;
            srr += wt * hr * hr;
            stt += wt * ht * ht;
            srt += wt * hr * ht;
        }
        let (ar, at) = (sr / total, st / total);
        let var_r = (srr / total - ar * ar).max(0.0);
        let var_t = (stt / total - at * at).max(0.0);
        let cov = srt / total - ar * at;
        let r = if var_r > 0.0 && var_t > 0.0 { cov / (var_r * var_t).sqrt() } else { 0.0 };
        BandReading {
            r: r as f32,
            target: (var_t.sqrt() / mt.max(1e-6)) as f32,
            render: (var_r.sqrt() / mr.max(1e-6)) as f32,
        }
    })
}

/// The zone's memberships at the reading size: the source raster's weights
/// (the coverage the sliders act on) and the paired weights (source times
/// target), inverted as the zone is.
fn zone_weights(zone: &DetailZone, w: u32, h: u32) -> (Vec<f32>, Vec<f32>) {
    let flip = |v: f32| if zone.inverted { 1.0 - v } else { v };
    let source: Vec<f32> = mask_weights(&zone.source, w, h).into_iter().map(flip).collect();
    let target = mask_weights(&zone.target, w, h);
    let paired = source.iter().zip(&target).map(|(s, t)| s * flip(*t)).collect();
    (source, paired)
}

fn set_sliders(mask: &mut LocalAdjustment, sliders: [f32; 2]) {
    mask.texture = sliders[0];
    mask.clarity = sliders[1];
}

/// The slider pair at a share `k` of the solve, on Lightroom's integer scale.
fn shrunk(sliders: [f32; 2], k: f32) -> [f32; 2] {
    sliders.map(|s| (s * k).round())
}

/// Which gate refused a candidate, for the ladder and the note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetailRefusal {
    Quality,
    Energy,
    Frame,
}

/// Run the stage over every zone of the pair. Sequential, each zone read on
/// the render the zones before it left behind, the same rule the tile sweep
/// and the regions follow.
pub(super) fn attach_zone_detail(
    src: &DynamicImage,
    target: &DynamicImage,
    report: &mut FitReport,
    zones: &[DetailZone],
) {
    if zones.is_empty() {
        return;
    }
    let big = src.thumbnail(DETAIL_EDGE, DETAIL_EDGE);
    let big_t = target.thumbnail_exact(big.width(), big.height());
    let tgt_px = fit::pixels_of(&big_t);
    let presence = fit::presence_of(&big_t);
    let (w, h) = (big.width(), big.height());
    // The frame ruler lives at analysis size: the evidence model was built
    // there and is read there.
    let (s_img, t_img) = fit::analysis_pair(src, target);
    let frame_tgt = fit::pixels_of(&t_img);
    let frame_err = |report: &FitReport| {
        let px = fit::pixels_of(&render::develop_preview(&s_img, &report.recipe));
        fit::look_err_with_evidence(&px, &frame_tgt, &report.evidence)
    };
    let note = |report: &mut FitReport, key: &'static str, args: Vec<(&'static str, String)>| {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(key, args),
        );
    };
    let two = |v: f32| format!("{v:.2}");
    for zone in zones {
        let (coverage, paired_weights) = zone_weights(zone, w, h);
        // Paired only where the target has a pixel to pair.
        let paired_weights = fit::with_presence(&paired_weights, presence.as_deref()).into_owned();
        let share = |weights: &[f32]| weights.iter().sum::<f32>() / weights.len().max(1) as f32;
        if share(&coverage) < MIN_ZONE_SHARE || share(&paired_weights) < MIN_ZONE_SHARE {
            // The tone stage already said this zone is too small; nothing
            // new to disclose.
            continue;
        }
        let label = zone.label.clone();
        let base_px = fit::pixels_of(&render::develop_preview(&big, &report.recipe));
        let before = band_readings(&base_px, &tgt_px, &paired_weights, w, h);
        let structured = before
            .map(|b| b.target >= DETAIL_MIN_CONTRAST && b.render >= DETAIL_MIN_CONTRAST);
        if !structured[0] && !structured[1] {
            note(report, crate::rationale::keys::DETAIL_NO_STRUCTURE, vec![("label", label)]);
            continue;
        }
        let paired = [0, 1].map(|b| structured[b] && before[b].r >= DETAIL_PAIRED_MIN);
        let correlations = vec![
            ("r_tex", two(before[0].r)),
            ("r_cla", two(before[1].r)),
        ];
        let want = [0, 1].map(|b| if paired[b] { asked_gain(before[b]) } else { 1.0 });
        // A fine band asking for LESS is not a slider this solve has (the
        // texture range starts at 0), so it does not count as an ask.
        let asks = [0, 1].map(|b| {
            let g = want[b];
            (g - 1.0).abs() >= DETAIL_MATCHED_GAIN && (b == 1 || g > 1.0)
        });
        if !asks[0] && !asks[1] {
            // Nothing to ship. A band that carries structure on both sides
            // and does not pair is the sentence worth printing — the drawn
            // texture — over "already matched" on the band that does.
            let mut args = vec![("label", label)];
            args.extend(correlations);
            if (0..2).any(|b| structured[b] && !paired[b]) {
                args.push(("min", two(DETAIL_PAIRED_MIN)));
                note(report, crate::rationale::keys::DETAIL_UNPAIRED, args);
            } else if (0..2).any(|b| paired[b] && (before[b].ratio() - 1.0).abs() >= DETAIL_MATCHED_GAIN) {
                // Both bands pair, and one shows a contrast difference the
                // shared structure does not ask for: it sits elsewhere.
                args.extend([
                    ("beta_tex", two(before[0].paired_gain())),
                    ("beta_cla", two(before[1].paired_gain())),
                    ("ratio_tex", two(before[0].ratio())),
                    ("ratio_cla", two(before[1].ratio())),
                ]);
                note(report, crate::rationale::keys::DETAIL_NOT_ON_STRUCTURE, args);
            } else {
                args.push(("beta_tex", two(want[0])));
                args.push(("beta_cla", two(want[1])));
                note(report, crate::rationale::keys::DETAIL_ALREADY_MATCHED, args);
            }
            continue;
        }
        // A zone whose tone was refused may have released its raster; the
        // sliders need the alpha back on disk. Restored here and removed
        // again below if nothing ends up referencing it.
        let restored = match render::geometry_raster_path(&zone.mask) {
            Some(path) if !std::path::Path::new(path).exists() => {
                if zone.source.save(path).is_err() {
                    continue;
                }
                Some(path.to_string())
            }
            _ => None,
        };
        let release = |report: &FitReport| {
            if let Some(path) = restored.as_deref()
                && !report.recipe.masks.iter().any(|m| render::geometry_raster_path(&m.mask) == Some(path))
            {
                let _ = std::fs::remove_file(path);
            }
        };
        let frame_before = frame_err(report);
        report.recipe.masks.push(LocalAdjustment {
            mask: zone.mask.clone(),
            components: zone.components.clone(),
            name: zone.name.clone(),
            role: zone.role,
            amount: 1.0,
            inverted: zone.inverted,
            ..Default::default()
        });
        let mask_index = report.recipe.masks.len() - 1;
        // The closed loop: each paired band's slider steps by the gain it
        // still needs over the measured (or prior) response, the render is
        // read again, and the response is re-estimated from what it did.
        // The measured response is held inside [`DETAIL_SLOPE_BOUNDS`] of the
        // prior and a step inside [`DETAIL_STEP_MAX`]: a dark zone's 8-bit
        // render answers a small step with a reading that is mostly
        // quantisation, and a secant through that reading ran the night
        // pair's ground to the clarity stop (2026-10-21).
        let mut sliders = [0.0f32; 2];
        let mut slope = DETAIL_SLOPE_PRIOR;
        let mut now = before;
        let mut final_px = base_px.clone();
        for _ in 0..DETAIL_STEPS {
            let mut moved = false;
            for b in 0..2 {
                let need = asked_gain(now[b]);
                if !paired[b] || (need - 1.0).abs() < DETAIL_MATCHED_GAIN {
                    continue;
                }
                let (lo, hi) = if b == 0 { DETAIL_TEXTURE_RANGE } else { DETAIL_CLARITY_RANGE };
                // `need` is the gain still missing on the CURRENT render;
                // the response is per point of slider on the band as it
                // stands, so the step is the missing gain over the slope.
                let step = ((need - 1.0) / slope[b]).clamp(-DETAIL_STEP_MAX, DETAIL_STEP_MAX);
                let next = (sliders[b] + step).clamp(lo, hi).round();
                if next != sliders[b] {
                    sliders[b] = next;
                    moved = true;
                }
            }
            if !moved {
                break;
            }
            set_sliders(&mut report.recipe.masks[mask_index], sliders);
            final_px = fit::pixels_of(&render::develop_preview(&big, &report.recipe));
            now = band_readings(&final_px, &tgt_px, &paired_weights, w, h);
            for b in 0..2 {
                if !paired[b] || sliders[b] == 0.0 || before[b].render <= 0.0 {
                    continue;
                }
                let achieved = now[b].render / before[b].render;
                let measured = (achieved - 1.0) / sliders[b];
                if measured.is_finite() {
                    slope[b] = measured.clamp(
                        DETAIL_SLOPE_PRIOR[b] * DETAIL_SLOPE_BOUNDS.0,
                        DETAIL_SLOPE_PRIOR[b] * DETAIL_SLOPE_BOUNDS.1,
                    );
                }
            }
        }
        if sliders == [0.0, 0.0] {
            report.recipe.masks.pop();
            release(report);
            let mut args = vec![("label", label)];
            args.extend(correlations);
            args.push(("beta_tex", two(want[0])));
            args.push(("beta_cla", two(want[1])));
            note(report, crate::rationale::keys::DETAIL_ALREADY_MATCHED, args);
            continue;
        }
        // A slider on its stop in the direction it was still asked to go,
        // with the band not landed, has run out: what the target shows at
        // that scale is not what the slider makes (the night pair's sky
        // asked 1.5x at the clarity scale, clarity +100 answered 1.2x and the
        // frame got worse, 2026-10-21). The ladder cannot help — a share of
        // a slider that did not reach reaches less — so it is refused here.
        let out_of_reach = (0..2).find(|&b| {
            let (lo, hi) = if b == 0 { DETAIL_TEXTURE_RANGE } else { DETAIL_CLARITY_RANGE };
            let need = asked_gain(now[b]);
            paired[b]
                && (need - 1.0).abs() >= DETAIL_MATCHED_GAIN
                && ((sliders[b] >= hi && need > 1.0) || (sliders[b] <= lo && lo < 0.0 && need < 1.0))
        });
        if let Some(b) = out_of_reach {
            report.recipe.masks.pop();
            release(report);
            let mut args = vec![
                ("label", label),
                ("texture", format!("{:+.0}", sliders[0])),
                ("clarity", format!("{:+.0}", sliders[1])),
                ("share", two(now[b].render / now[b].target.max(1e-6))),
            ];
            args.extend(correlations);
            note(report, crate::rationale::keys::DETAIL_REFUSED_REACH, args);
            continue;
        }
        // The gates, then the ladder. Every rung re-renders and re-asks all
        // three; the first passing rung ships.
        let judge = |report: &mut FitReport, px: &[[f32; 3]]| -> Result<(f32, [BandReading; 2]), DetailRefusal> {
            let quality = local_quality(&base_px, px, &coverage, w, h);
            if !quality.passes() {
                return Err(DetailRefusal::Quality);
            }
            let after = band_readings(px, &tgt_px, &paired_weights, w, h);
            if (0..2).any(|b| structured[b] && pushed_past_evidence(paired[b], before[b], after[b])) {
                return Err(DetailRefusal::Energy);
            }
            let frame_after = frame_err(report);
            if frame_after > frame_before + ZONE_GLOBAL_REGRESSION_TOL {
                return Err(DetailRefusal::Frame);
            }
            Ok((frame_after, after))
        };
        let solved = sliders;
        let mut verdict = judge(report, &final_px);
        let mut k = 1.0f32;
        if verdict.is_err() {
            for rung in DETAIL_SHRINK_LADDER {
                let shrunk = shrunk(solved, rung);
                if shrunk == [0.0, 0.0] {
                    break;
                }
                set_sliders(&mut report.recipe.masks[mask_index], shrunk);
                let px = fit::pixels_of(&render::develop_preview(&big, &report.recipe));
                verdict = judge(report, &px);
                if verdict.is_ok() {
                    sliders = shrunk;
                    k = rung;
                    break;
                }
            }
        }
        match verdict {
            Ok((frame_after, after)) => {
                report.err_after = frame_after;
                let mut args = vec![
                    ("label", label),
                    ("texture", format!("{:+.0}", sliders[0])),
                    ("clarity", format!("{:+.0}", sliders[1])),
                    ("k", two(k)),
                ];
                args.extend(correlations);
                args.extend([
                    ("tex_before", two(before[0].render / before[0].target.max(1e-6))),
                    ("tex_after", two(after[0].render / after[0].target.max(1e-6))),
                    ("cla_before", two(before[1].render / before[1].target.max(1e-6))),
                    ("cla_after", two(after[1].render / after[1].target.max(1e-6))),
                ]);
                note(report, crate::rationale::keys::DETAIL_ATTACHED, args);
            }
            Err(why) => {
                report.recipe.masks.pop();
                release(report);
                let key = match why {
                    DetailRefusal::Quality => crate::rationale::keys::DETAIL_REFUSED_QUALITY,
                    DetailRefusal::Energy => crate::rationale::keys::DETAIL_REFUSED_ENERGY,
                    DetailRefusal::Frame => crate::rationale::keys::DETAIL_REFUSED_FRAME,
                };
                let mut args = vec![
                    ("label", label),
                    ("texture", format!("{:+.0}", solved[0])),
                    ("clarity", format!("{:+.0}", solved[1])),
                ];
                args.extend(correlations);
                note(report, key, args);
            }
        }
    }
}

#[cfg(test)]
mod tests;
