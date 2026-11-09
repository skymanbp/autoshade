//! The unsupported-range vetoes the zoned solve still reads and the per-pixel voucher (the global solve reads the one validity law, `fit::validity`).

use super::*;

pub(crate) fn moves_unsupported_range(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> bool {
    moved_unsupported_range_names(cur, with_px, evidence).is_some()
}

pub(crate) fn moved_unsupported_range_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<(String, String)> {
    let hits = moved_unsupported_range_hits(cur, with_px, evidence, None, None);
    let (luma, hue) = (hits.luma, hits.hue);
    if luma.0 == 0.0 && hue.0 == 0.0 { return None; }
    let names = |hits: &[bool], ranges: &[EvidenceRange]| {
        hits.iter().zip(ranges).filter_map(|(&hit, range)| hit.then_some(range.label.as_str())).collect::<Vec<_>>().join(", ")
    };
    Some((names(&luma.1, &evidence.luma), names(&hue.1, &evidence.hue)))
}

pub(crate) fn moved_unsupported_luma_range_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<String> {
    let luma = moved_unsupported_range_hits(cur, with_px, evidence, None, None).luma;
    if luma.0 == 0.0 { return None; }
    Some(luma.1.iter().zip(&evidence.luma).filter_map(|(&hit, range)| hit.then_some(range.label.as_str())).collect::<Vec<_>>().join(", "))
}

#[cfg(test)]
pub(crate) fn moved_unsupported_hue_range_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
) -> Option<String> {
    moved_unsupported_hue_range_names_vouched(cur, with_px, evidence, None, None)
}

/// [`moved_unsupported_hue_range_names`] with a per-pixel robust voucher for
/// the evacuation exemption (see `moved_unsupported_range_hits`). The global
/// end-state guard, the mixer and the cast outcome supply one on a paired
/// run, and since 2026-10-21 so does the zone colour probe of a region that
/// pairs at pixel scale; every other caller keeps the strict doctrine.
pub(crate) fn moved_unsupported_hue_range_names_vouched(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<CellArm<'_>>,
) -> Option<String> {
    let hue = moved_unsupported_range_hits(cur, with_px, evidence, vouch, cells).hue;
    if hue.0 == 0.0 { return None; }
    Some(hue.1.iter().zip(&evidence.hue).filter_map(|(&hit, range)| hit.then_some(range.label.as_str())).collect::<Vec<_>>().join(", "))
}

fn moved_unsupported_range_hits(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<CellArm<'_>>,
) -> MovedRangeHits {
    let mut moved_luma = 0.0f32;
    let mut moved_hue = 0.0f32;
    let mut luma = [false; EVIDENCE_LUMA_BINS];
    let mut hue = [false; EVIDENCE_HUE_BANDS];
    let mut vouched_hue = [false; EVIDENCE_HUE_BANDS];
    for (i, (before, after)) in cur.iter().zip(with_px).enumerate() {
        let unsupported_luma = source_luma_is_withheld(i, evidence);
        let unsupported_hue = source_hue_is_withheld(i, evidence);
        let delta = (0..3).map(|channel| (after[channel] - before[channel]).abs()).sum::<f32>()
            / 3.0;
        if delta < UNSUPPORTED_RANGE_MOVE { continue; }
        let membership = evidence.source_membership.get(i).copied().unwrap_or(0.0).max(0.0);
        if membership <= 0.0 { continue; }
        if unsupported_luma && let Some(pixel) = evidence.source_pixels.get(i) {
            moved_luma += membership;
            luma[evidence_luma_bin(luma601(pixel))] = true;
        }
        if unsupported_hue && let Some(pixel) = evidence.source_pixels.get(i) && let Some(band) = evidence_hue_band(pixel) {
            // VOUCHED convergence (the hue form of the luma rank-pairing
            // doctrine, gated by the robust fit): on a paired run, a pixel
            // whose transport residual the robust fit vouches for has its OWN
            // paired target pixel — moving it TOWARD that target is
            // convergence, not a blind move through an unmeasurable band
            // (measured: the haze pair's blue-cast pixels sat in source-only
            // Red/Blue, and un-casting them was vetoed by the very bands the
            // cast invented; a band-topology exemption then failed the same
            // pair again on pixels the cast's step caps left mid-way). A
            // content-divergent pixel (the canyon pair's vanished reds are
            // the reconstruction's doing, not an edit's) carries a large
            // transport residual, earns no voucher, and keeps its veto.
            // Callers with no paired verdict pass None and get the strict
            // doctrine unchanged. The zoned colour probe did so until
            // 2026-10-21 (zone colour stayed class-split withheld); the
            // user's ruling of that date hands it the zone's own robust
            // pairing where the region pairs at pixel scale, because a
            // one-sided band on such a region is band membership and not a
            // lack of evidence — the seaside street's source sky sits in
            // Aqua, the target's in Blue, and the zone shipped unity gains
            // over a sky whose every pixel had its counterpart. A Cell-scale
            // region still passes None and asks its cells. The voucher names
            // the AXIS its move is judged along (`PixelVouch`): a zone's
            // colour probe is read in chromaticity, a global move in full.
            let spatially = evidence.spatial_supported.get(i).copied().unwrap_or(false);
            let converges = spatially
                && vouch.is_some_and(|(weights, targets, axis)| {
                    weights.get(i).copied().unwrap_or(0.0) >= 0.5
                        && targets
                            .get(i)
                            .is_some_and(|target| converges_toward(axis, target, before, after))
                });
            // R34 §D3. THE cell arm, for the pixels the paired one cannot
            // be ASKED about — a strictly smaller set than the pixels it did
            // not vouch, and the difference is the whole policy. A pixel
            // whose texture SURVIVED has a counterpart; moving it away from
            // that counterpart is evidence against the move, never a reason
            // to ask its neighbours for a second opinion. Guarding on
            // `!converges` alone let a surviving region overrule its own
            // pixels: measured on the canyon-gold fixture, whose synthetic
            // pair is aligned to the pixel (D 0.028), the cells read
            // 0.921 / 0.079 / 0.981 and rotated a pale-blue sky 158° into the
            // target's native gold — the exact policy
            // `cast_must_not_rotate_the_sky_into_a_target_native_hue` exists
            // to refuse, and the sentence this arm prints ("those pixels are
            // re-synthesised and have no paired counterpart") would have been
            // false about every one of them.
            //
            // The reading is the PIXEL's OWN structural confidence against
            // `DIVERGENCE_GLOBAL` — the same line the mode split and the zone
            // tone estimator use — and deliberately NOT `spatially`.
            // `spatial_supported` is `globally_same_content || d <
            // DIVERGENCE_ZONE`: a frame that pairs overall marks every pixel
            // of a repainted sky supported, which is R33's "region is not
            // frame" inside the flag's own definition. The reference frame
            // reads 0.275, so `spatially` is true right across a sky whose own
            // reading is 0.617 and whose pixels are therefore not each other's
            // counterparts. Measured through this predicate rather than
            // through that flag, the arm still carries [Aqua, Blue] on the
            // reference pair (0.966 / 0.017 / 0.937) and is refused on the
            // canyon, which is the whole difference.
            let unpaired =
                evidence.spatial_weights.get(i).copied().unwrap_or(1.0) < 1.0 - DIVERGENCE_GLOBAL;
            let by_cells = unpaired
                && !converges
                && cells.is_some_and(|(grid, verdicts)| {
                    grid.cell_of(i)
                        .and_then(|cell| verdicts.get(cell).copied().flatten())
                        == Some(true)
                });
            if converges {
                vouched_hue[band] = true;
            } else if !by_cells {
                moved_hue += membership;
                hue[band] = true;
            }
        }
    }
    // The region line is a share of the population the correction moves --
    // the frame for the global fit, a zone's coverage for a zone (a frame
    // share let every tile move its blind pixels: a depth-2 tile is 6% of
    // the frame, so its blind half never reached the 5% line).
    let total = evidence.population.max(1.0);
    if moved_luma / total < ROT_SHARE { moved_luma = 0.0; luma = [false; EVIDENCE_LUMA_BINS]; }
    if moved_hue / total < ROT_SHARE { moved_hue = 0.0; hue = [false; EVIDENCE_HUE_BANDS]; }
    MovedRangeHits { luma: (moved_luma, luma), hue: (moved_hue, hue), vouched_hue }
}

/// The per-range verdicts of one blind-move audit: which withheld ranges were
/// MOVED (the veto's subject) and which one-sided hue bands vouched
/// convergence carried movement THROUGH — the second list exists so the
/// disclosure can stop claiming "vetoed movement" about bands whose veto the
/// voucher lifted (E-15: two different outcomes must not read the same).
struct MovedRangeHits {
    luma: (f32, [bool; EVIDENCE_LUMA_BINS]),
    hue: (f32, [bool; EVIDENCE_HUE_BANDS]),
    vouched_hue: [bool; EVIDENCE_HUE_BANDS],
}

/// The per-pixel voucher: each pixel's robust pairing weight, its own paired
/// target pixel, and the axis the move it is asked about can move along
/// (`fit_cells::MoveAxis` — full RGB for a global cast or mixer, chromaticity
/// for a zone's colour probe solved with its tone zeroed).
pub(crate) type PixelVouch<'a> = (&'a [f32], &'a [[f32; 3]], crate::fit_cells::MoveAxis);

/// One candidate render's REGION verdicts, and the grid that indexes them:
/// `fit_cells`'s answer for the pixels the per-pixel voucher cannot reach.
/// Recomputed per candidate — it is a verdict about what an edit DID, so it
/// cannot outlive the edit it judged.
pub(crate) type CellArm<'a> = (&'a crate::fit_cells::PairedCells, &'a [Option<bool>]);

/// Names of the one-sided hue bands that vouched convergence moved pixels
/// through on the finished render — the disclosure's raw material.
pub(crate) fn vouched_hue_band_names(
    cur: &[[f32; 3]],
    with_px: &[[f32; 3]],
    evidence: &EvidenceModel,
    vouch: Option<PixelVouch<'_>>,
    cells: Option<CellArm<'_>>,
) -> Option<String> {
    let hits = moved_unsupported_range_hits(cur, with_px, evidence, vouch, cells);
    band_names(&hits.vouched_hue, evidence)
}

fn band_names(hits: &[bool; EVIDENCE_HUE_BANDS], evidence: &EvidenceModel) -> Option<String> {
    let names = hits
        .iter()
        .zip(&evidence.hue)
        .filter_map(|(&hit, range)| hit.then_some(range.label.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    (!names.is_empty()).then_some(names)
}

