//! Accepting zones: the accepted-zone record, the refusal of shrunk zones, the boundary gates and the attached note.

use super::*;

/// A zone whose correction was kept — what [`attach_zones`] needs to report
/// on the stage as a whole.
pub(super) struct AcceptedZone {
    /// Display/rationale identity carried by the attachment itself.
    pub(super) label: String,
    /// Optional native range refinement; `None` is a semantic bitmap zone.
    pub(super) range: Option<RangeMask>,
    /// Exact correction index, independent of role or free-text identity.
    pub(super) mask_index: usize,
    /// Estimator populations retained for the final-stack remeasurement:
    /// the robust-composed weights the zone's `before` was read on, so its
    /// `after` and every remeasurement read the SAME population
    /// (2026-09-24; until then the raw attachment weights).
    pub(super) source_weights: Vec<f32>,
    pub(super) target_weights: Vec<f32>,
    /// The attachment's own source coverage — the mask's feathered raster
    /// at analysis size — for what reads GEOMETRY, not a population: the
    /// boundary ruler's transition band and the correction's share of the
    /// frame. Handed the robust-composed population above instead, the
    /// ruler took that population's holes (the content the two zones do
    /// not share) for transitions and refused the calibration corpus's
    /// third semantic region (2026-09-24, the four-region test).
    pub(super) mask_weights: Vec<f32>,
    /// The zone-local residual before this correction.
    pub(super) before: f32,
    /// The zone-local residual it landed at ([`zone_err`]).
    pub(super) after: f32,
    /// The candidate render, retained so the boundary gate reuses the final
    /// analysis render instead of buying another full candidate render.
    pub(super) rendered: Vec<[f32; 3]>,
    /// R39. What the acceptance judged, so the correction that SHIPS can be
    /// held to do-no-harm after a boundary gate has shrunk it: the residual
    /// [`attach_one_zone`] read on the frame this zone was attached to
    /// (luma-only where colour was withheld and tone kept, else the
    /// whole-zone one — `luma_only` says which), and the frame drift its
    /// attachment was allowed. On the sequential routes (tiles, free masks)
    /// that frame is exactly the shipped set without this correction, so
    /// `judged_before` is the leave-one-out baseline there; the shared
    /// routes render their own (`refuse_shrunk_zones`).
    pub(super) judged_before: f32,
    pub(super) luma_only: bool,
    pub(super) frame_regression_tol: f32,
}

impl AcceptedZone {
    /// The residual this zone's acceptance judges, on `pixels`.
    pub(super) fn judged(&self, pixels: &[[f32; 3]], tgt_px: &[[f32; 3]]) -> f32 {
        let after = zone_moments(pixels, &self.source_weights);
        let target = zone_moments(tgt_px, &self.target_weights);
        if self.luma_only {
            zone_luma_err(&after, &target)
        } else {
            zone_err(&after, &target)
        }
    }

    /// R39. Does the correction that SHIPS still do what it was admitted
    /// for? Every boundary gate shrinks a correction AFTER
    /// [`attach_one_zone`] has judged it at k=1, and a shrunk correction is
    /// a different correction: on the reference pair, spatial tile r1c0 was
    /// admitted at full strength and shipped at k=0.134 with its own
    /// colour-inclusive residual WORSE than the render without it (0.19166
    /// -> 0.20101), bought by a frame reading that moved 0.0003 the right
    /// way — a pale block in the sky that no gate had judged. (That tile
    /// passes THIS predicate: what its acceptance read is luma only, the
    /// colour arm withheld, and that improves; the block is the boundary
    /// ruler's to refuse — R40, [`discontinuity_budget`].)
    ///
    /// The predicate is do-no-harm, not the k=1 arms. The arms
    /// ([`zone_accepts`]) ask for a real GAIN, which was established at full
    /// strength and is what the shrink trades away on purpose: a sky
    /// correction negotiated down to k=0.088 that still takes its zone from
    /// 0.040 to 0.038 (the inverted-raster fixture) is the boundary gate
    /// doing its job, not a correction to refuse — the first cut of this
    /// predicate re-ran the arms and refused it. What a shrunk correction
    /// may not do is leave its zone worse than the render WITHOUT it
    /// (`without`, the caller's leave-one-out baseline) or carry the frame
    /// past the drift its attachment was allowed.
    pub(super) fn still_accepted(
        &self,
        pixels: &[[f32; 3]],
        tgt_px: &[[f32; 3]],
        without: f32,
        frame_before: f32,
        frame_after: f32,
    ) -> bool {
        frame_after <= frame_before + self.frame_regression_tol
            && self.judged(pixels, tgt_px) <= without
    }

    /// The refusal note a caller pushes when [`Self::still_accepted`] says
    /// no: which correction, at what shrink, its residual with and without
    /// it, and the frame pair.
    pub(super) fn shrunk_refusal_note(
        &self,
        pixels: &[[f32; 3]],
        tgt_px: &[[f32; 3]],
        without: f32,
        k: f32,
        frame_before: f32,
        frame_after: f32,
    ) -> crate::rationale::Note {
        crate::rationale::Note::new(
            crate::rationale::keys::ZONE_SHRUNK_REFUSED,
            vec![
                ("label", self.label.clone()),
                ("k", format!("{k:.3}")),
                ("before", format!("{without:.3}")),
                ("after", format!("{:.3}", self.judged(pixels, tgt_px))),
                ("frame_before", format!("{frame_before:.5}")),
                ("frame_after", format!("{frame_after:.5}")),
            ],
        )
    }
}

/// R39. After a SHARED boundary shrink (the semantic and range routes shrink
/// a whole set of zones by one k), every zone in `accepted` is held to
/// [`AcceptedZone::still_accepted`] on `pixels`, the render that would ship,
/// against the same shipped set rendered WITHOUT that zone: `report.recipe`
/// carries the shrunk masks at `first_zone..` at this point, so a copy with
/// the zone's own mask left out, rendered at analysis size, is the
/// leave-one-out baseline. The stale alternative — the residual each zone
/// was judged against at k=1, read with the EARLIER zones at full strength
/// — refused the calibration land zone at k=0.244 for reading 0.030 against
/// a 0.017 that no longer described the frame it shipped in. A zone that no
/// longer passes is refused — its note pushed, and it and its k=1 controls
/// dropped from `accepted` and `originals` in step — so the caller can
/// restore the survivors' k=1 controls and run the gate again on them
/// alone. Returns whether anything was refused.
#[allow(clippy::too_many_arguments)]
pub(super) fn refuse_shrunk_zones(
    s_img: &DynamicImage,
    report: &mut FitReport,
    accepted: &mut Vec<AcceptedZone>,
    originals: &mut Vec<LocalAdjustment>,
    first_zone: usize,
    pixels: &[[f32; 3]],
    tgt_px: &[[f32; 3]],
    k: f32,
    frame_before: f32,
    frame_after: f32,
) -> bool {
    debug_assert_eq!(accepted.len(), originals.len(), "one k=1 control set per accepted zone");
    debug_assert_eq!(
        report.recipe.masks.len(),
        first_zone + accepted.len(),
        "the shrunk set is what the recipe carries when it is judged"
    );
    let without = (0..accepted.len())
        .map(|i| {
            let mut recipe = report.recipe.clone();
            recipe.masks.remove(first_zone + i);
            let px = fit::pixels_of(&render::develop_preview(s_img, &recipe));
            accepted[i].judged(&px, tgt_px)
        })
        .collect::<Vec<_>>();
    let mut refused = false;
    let mut i = 0;
    for baseline in without {
        if accepted[i].still_accepted(pixels, tgt_px, baseline, frame_before, frame_after) {
            i += 1;
            continue;
        }
        let note = accepted[i].shrunk_refusal_note(pixels, tgt_px, baseline, k, frame_before, frame_after);
        crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes, note);
        accepted.remove(i);
        originals.remove(i);
        refused = true;
    }
    refused
}

pub(super) enum BoundaryGateResult {
    Kept {
        k: f32,
        before: BoundaryReading,
        after: BoundaryReading,
        pixels: Vec<[f32; 3]>,
    },
    Dropped,
}

fn boundary_note_args(
    n: usize,
    k: f32,
    before: BoundaryReading,
    after: BoundaryReading,
) -> Vec<(&'static str, String)> {
    vec![
        ("n", n.to_string()),
        ("k", format!("{k:.3}")),
        ("before", format!("{:.3}", before.rim)),
        ("after", format!("{:.3}", after.rim)),
        ("max", format!("{ZONE_BOUNDARY_RIM_MAX:.3}")),
        ("transitions", after.transitions.to_string()),
        ("asked", format!("{:.3}", after.asked)),
        ("charged", format!("{:.3}", after.charged)),
        ("colour", format!("{:.3}", after.colour)),
        ("colour_charged", format!("{:.3}", after.colour_charged)),
    ]
}

/// Enforce the pair-level boundary budget after the independent zone-local
/// gates. `initial_px` is the analysis render the last accepted zone already
/// made; `reference_px` is the SAME frame rendered before any zone was
/// attached, i.e. this gate own `k=0` baseline, which the caller already had
/// in hand. Only re-measurements during an actual shrink render again, always
/// at analysis size; no full-resolution render is introduced.
///
/// Since R37 the production sky/land path calls [`enforce_boundary_gate_toward`]
/// with its target; this no-target form is what the context-budget pins measure.
#[cfg(test)]
pub(super) fn enforce_boundary_gate(
    s_img: &DynamicImage,
    report: &mut FitReport,
    sky_weights: &[f32],
    correction_shares: &[f32],
    first_zone: usize,
    reference_px: &[[f32; 3]],
    initial_px: Vec<[f32; 3]>,
) -> BoundaryGateResult {
    enforce_boundary_gate_toward(None, s_img, report, sky_weights, correction_shares,
        first_zone, reference_px, initial_px)
}

/// [`enforce_boundary_gate`] toward a paired `target` (R37): the horizon the
/// target itself carries is not charged. The production sky/land path hands
/// its analysis target here; `None` is the context rule alone.
#[allow(clippy::too_many_arguments)]
pub(super) fn enforce_boundary_gate_toward(
    target: Option<&[[f32; 3]]>,
    s_img: &DynamicImage,
    report: &mut FitReport,
    sky_weights: &[f32],
    correction_shares: &[f32],
    first_zone: usize,
    reference_px: &[[f32; 3]],
    initial_px: Vec<[f32; 3]>,
) -> BoundaryGateResult {
    enforce_boundary_gates(target, s_img, report, &[sky_weights], correction_shares,
        first_zone, reference_px, initial_px)
}

/// The same budget and shrink for one semantic horizon or all the horizons
/// and breaks of a sub-zone set. One bisection owns the complete correction
/// set; no band can borrow a different ruler or hide a worse boundary.
#[allow(clippy::too_many_arguments)]
pub(super) fn enforce_boundary_gates(
    target: Option<&[[f32; 3]]>,
    s_img: &DynamicImage,
    report: &mut FitReport,
    boundaries: &[&[f32]],
    correction_shares: &[f32],
    first_zone: usize,
    reference_px: &[[f32; 3]],
    initial_px: Vec<[f32; 3]>,
) -> BoundaryGateResult {
    enforce_boundary_gates_with_shrink(target, s_img, report, boundaries, correction_shares,
        first_zone, (reference_px, initial_px), shrink_zone_corrections)
}

/// The same measured gate also handles replacement deltas. Its zero render
/// is supplied by the caller; ordinary new zones use zero controls, while a
/// native band replacement retains its accepted parent's controls at zero.
#[allow(clippy::too_many_arguments)]
pub(super) fn enforce_boundary_gates_with_shrink(
    target: Option<&[[f32; 3]]>,
    s_img: &DynamicImage,
    report: &mut FitReport,
    boundaries: &[&[f32]],
    correction_shares: &[f32],
    first_zone: usize,
    (reference_px, initial_px): (&[[f32; 3]], Vec<[f32; 3]>),
    shrink: impl Fn(&mut [LocalAdjustment], &[LocalAdjustment], &[f32], f32),
) -> BoundaryGateResult {
    let presence = report.evidence.presence.clone();
    let read = |pixels: &[[f32; 3]], frozen: &[[f32; 3]]| {
        boundaries.iter().fold(BoundaryReading::nothing_measured(), |mut all, weights| {
            let next = boundary_rim_toward(target, reference_px, pixels, frozen, weights, s_img.width(), s_img.height(), presence.as_deref());
            all.rim = all.rim.max(next.rim);
            all.charged = all.charged.max(next.charged);
            all.colour = all.colour.max(next.colour);
            all.colour_charged = all.colour_charged.max(next.colour_charged);
            all.asked = all.asked.max(next.asked);
            all.transitions += next.transitions;
            all
        })
    };
    // `initial_px` is the k=1 candidate, so it is also this gate's FROZEN
    // frame: the correction's own slope is a property of the correction and
    // must never chase the bisection (see [`crossing_slope`]).
    let initial = read(&initial_px, &initial_px);
    let zone_count = report.recipe.masks.len().saturating_sub(first_zone);
    // A correction that survives this gate must MOVE something. Step 9 made
    // that worth checking: under the transported differential a `k=0` render
    // reads exactly 0.0, so the budget can no longer refuse it and the
    // bisection returns the largest passing `k` — which, for a correction
    // whose every visible strength introduces a seam, renders to nothing. An
    // inert attachment is strictly worse than a refusal: it keeps a raster on
    // disk and discloses a before/after pair it did not produce. The test is
    // BYTE IDENTITY of the render against `reference_px` rather than a
    // threshold on `k`; see `spatial::BitmapBoundaryWhy::Inert` for why a
    // threshold on `k` would be the wrong instrument.
    let refuse_inert = |report: &mut FitReport, reading: BoundaryReading, k: f32| {
        report.recipe.masks.truncate(first_zone);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_BOUNDARY_INERT,
                boundary_note_args(zone_count, k, initial, reading),
            ),
        );
    };
    if initial_px.as_slice() == reference_px {
        refuse_inert(report, initial, 1.0);
        return BoundaryGateResult::Dropped;
    }
    if initial.gated() <= ZONE_BOUNDARY_RIM_MAX {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_BOUNDARY_PASSED,
                boundary_note_args(zone_count, 1.0, initial, initial),
            ),
        );
        return BoundaryGateResult::Kept {
            k: 1.0,
            before: initial,
            after: initial,
            pixels: initial_px,
        };
    }

    let originals = report.recipe.masks[first_zone..].to_vec();
    let shares = correction_shares.to_vec();
    debug_assert_eq!(shares.len(), originals.len());
    let render_at = |report: &mut FitReport, k: f32| -> (BoundaryReading, Vec<[f32; 3]>) {
        shrink(
            &mut report.recipe.masks[first_zone..],
            &originals,
            &shares,
            k,
        );
        let pixels = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let reading = read(&pixels, &initial_px);
        (reading, pixels)
    };

    // INVARIANT, not a policy branch. The reading is now the rim the
    // correction INTRODUCED against `reference_px`, and `k=0` restores the
    // caller's baseline (zero controls for an addition, accepted parent
    // controls for replacement bands). It MUST render back to that baseline
    // and read exactly 0.0. A non-zero reading is an invariant failure,
    // not permission to spend a different boundary budget.
    let (zero, zero_px) = render_at(report, 0.0);
    if zero.gated() > ZONE_BOUNDARY_RIM_MAX {
        report.recipe.masks.truncate(first_zone);
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_BOUNDARY_DROPPED,
                boundary_note_args(zone_count, 0.0, initial, zero),
            ),
        );
        return BoundaryGateResult::Dropped;
    }

    // Monotone in the differential for the bounded pointwise zone dials.
    // Twelve bisections resolve k to <0.00025, much finer than the displayed
    // three decimals or the 8-bit analysis render can distinguish.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut best = (zero, zero_px);
    for _ in 0..12 {
        let mid = (lo + hi) * 0.5;
        let measured = render_at(report, mid);
        if measured.0.gated() <= ZONE_BOUNDARY_RIM_MAX {
            lo = mid;
            best = measured;
        } else {
            hi = mid;
        }
    }
    if best.1.as_slice() == reference_px {
        refuse_inert(report, best.0, lo);
        return BoundaryGateResult::Dropped;
    }
    shrink(
        &mut report.recipe.masks[first_zone..],
        &originals,
        &shares,
        lo,
    );
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::ZONE_BOUNDARY_PASSED,
            boundary_note_args(zone_count, lo, initial, best.0),
        ),
    );
    BoundaryGateResult::Kept { k: lo, before: initial, after: best.0, pixels: best.1 }
}

pub(super) fn push_zone_attached_note(report: &mut FitReport, zone: &AcceptedZone) {
    let label = zone.label.as_str();
    let (ev, gains, saturation, rides_out) = {
        let mask = report
            .recipe
            .masks
            .get(zone.mask_index)
            .expect("accepted zone mask remains attached");
        (
            mask.exposure_ev,
            mask.color_gains.unwrap_or([1.0; 3]),
            mask.saturation,
            // Which sentence this note ends with is a fact about the CARRIER,
            // so it is read off the carrier and nowhere else. The sky/land
            // pair rides Lightroom's own Select Sky; a semantic region is
            // still a raster classic XMP cannot hold.
            matches!(mask.mask, MaskGeometry::AiMask { .. }),
        )
    };
    if let Some(RangeMask::Luminance { lo, hi, .. }) = zone.range {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::RANGE_ATTACHED,
                vec![
                    ("label", label.to_string()),
                    ("lo", format!("{lo:.3}")),
                    ("hi", format!("{hi:.3}")),
                    ("ev", format!("{ev:+.2}")),
                    ("g0", format!("{:.2}", gains[0])),
                    ("g1", format!("{:.2}", gains[1])),
                    ("g2", format!("{:.2}", gains[2])),
                    ("sat", format!("{saturation:+.0}")),
                    ("before", format!("{:.3}", zone.before)),
                    ("after", format!("{:.3}", zone.after)),
                ],
            ),
        );
        return;
    }
    // A colour band's own sentence. Its interval would be a chromaticity
    // radius around a colour, which no reader can picture from two numbers,
    // so the band it was measured on rides in the LABEL and the radius is
    // printed once, by the proposal note, in the units it was measured in.
    if let Some(RangeMask::Color { .. }) = zone.range {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::COLOUR_RANGE_ATTACHED,
                vec![
                    ("label", label.to_string()),
                    ("ev", format!("{ev:+.2}")),
                    ("g0", format!("{:.2}", gains[0])),
                    ("g1", format!("{:.2}", gains[1])),
                    ("g2", format!("{:.2}", gains[2])),
                    ("sat", format!("{saturation:+.0}")),
                    ("before", format!("{:.3}", zone.before)),
                    ("after", format!("{:.3}", zone.after)),
                ],
            ),
        );
        return;
    }
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            if rides_out {
                crate::rationale::keys::ZONE_ATTACHED_AI
            } else {
                crate::rationale::keys::ZONE_ATTACHED
            },
            vec![
                ("label", label.to_string()),
                ("ev", format!("{ev:+.2}")),
                ("g0", format!("{:.2}", gains[0])),
                ("g1", format!("{:.2}", gains[1])),
                ("g2", format!("{:.2}", gains[2])),
                ("sat", format!("{saturation:+.0}")),
                ("before", format!("{:.3}", zone.before)),
                ("after", format!("{:.3}", zone.after)),
            ],
        ),
    );
}
