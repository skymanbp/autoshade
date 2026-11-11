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
    /// The zone's own present population, source and target — the pair
    /// [`attach_one_zone`]'s `ships_local` read its own J* on, so the
    /// acceptance at the strength that ships reads the same pixels by the
    /// same law.
    pub(super) own_weights: (Vec<f32>, Vec<f32>),
}

impl AcceptedZone {
    /// The reading this zone's acceptance judges, on `pixels`: the one
    /// objective on the zone's own present pixels ([`own_reading`]), as
    /// `ships_local` read it at k = 1 (RULING-1180-int-r6 (2): every
    /// do-no-harm at a zone reads the one objective; until then this read
    /// the display moments, luma-only where colour was withheld).
    pub(super) fn judged(&self, report: &FitReport, pixels: &[[f32; 3]], tgt_px: &[[f32; 3]]) -> f32 {
        own_reading(report, pixels, tgt_px, &self.own_weights.0, &self.own_weights.1)
    }

    /// ONE LAW AT THE STRENGTH THAT SHIPS (RULING-1180-joint-r2, 2026-11-06).
    /// Every boundary gate shrinks a correction AFTER [`attach_one_zone`]
    /// judged it at k = 1, and a shrunk correction is a different
    /// correction: on the reference pair, spatial tile r1c0 was admitted at
    /// full strength and shipped at k = 0.134 with its own residual WORSE
    /// than the render without it (0.19166 -> 0.20101), bought by a frame
    /// reading that moved 0.0003 the right way. So the correction that ships
    /// is judged where it ships, by the law that admitted it and no other:
    /// R19's [`fit::ships_local`] — FIT_QUANT of own-pixel J* earned against
    /// the shipped set WITHOUT it (`without`, the caller's leave-one-out
    /// baseline), the frame a guard of FIT_QUANT. Until v1.18.0 this read
    /// do-no-harm instead (own no worse than without, no margin), which kept
    /// a sky negotiated down to k = 0.088 that took its zone from 0.040 to
    /// 0.038: a correction that buys less than the quantum is not worth a
    /// control, and the k = 1 pass is the same law read before the shrink,
    /// not a second one.
    pub(super) fn still_accepted(
        &self,
        report: &FitReport,
        pixels: &[[f32; 3]],
        tgt_px: &[[f32; 3]],
        without: f32,
        frame_before: f32,
        frame_after: f32,
    ) -> bool {
        // A LOCAL correction's do-no-harm reads the one objective as the frame
        // does (2026-11-07: hue and structure are gates, not terms); its seams
        // stay with the boundary / ramp rulers and its structural gate is the
        // existing D threshold only.
        // R19: the frame is a GUARD for a local correction (`fit::ships_local`):
        // the shrunk correction may not raise it by more than FIT_QUANT; its
        // gain was decided at k = 1 on its own population.
        fit::ships_local(self.judged(report, pixels, tgt_px), without, frame_after, frame_before)
    }

    /// The refusal note a caller pushes when [`Self::still_accepted`] says
    /// no: which correction, at what shrink, its residual with and without
    /// it, and the frame pair.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn shrunk_refusal_note(
        &self,
        report: &FitReport,
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
                ("before", format!("{without:.5}")),
                ("after", format!("{:.5}", self.judged(report, pixels, tgt_px))),
                ("frame_before", format!("{frame_before:.5}")),
                ("frame_after", format!("{frame_after:.5}")),
                ("quant", format!("{:.4}", fit::FIT_QUANT)),
            ],
        )
    }
}

/// After a SHARED boundary shrink (the semantic and range routes shrink
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
            accepted[i].judged(report, &px, tgt_px)
        })
        .collect::<Vec<_>>();
    let mut refused = false;
    let mut i = 0;
    for baseline in without {
        if accepted[i].still_accepted(report, pixels, tgt_px, baseline, frame_before, frame_after) {
            i += 1;
            continue;
        }
        let note = accepted[i].shrunk_refusal_note(report, pixels, tgt_px, baseline, k, frame_before, frame_after);
        // A refused zone ships none of its controls, so the sentences that
        // said its colour or tone SHIPPED are withdrawn with it: a note
        // describes what ships.
        let label = accepted[i].label.clone();
        crate::rationale::withdraw_notes(&mut report.recipe.rationale, &mut report.notes, |n| {
            crate::rationale::keys::ZONE_VOUCHED.contains(&n.key)
                && n.args.iter().any(|(name, value)| *name == "label" && *value == label)
        });
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
/// with its target; this no-target form is what the context-budget pins
/// measure, and it reads the settled sides' median multipliers: several of
/// those pins hand the gate a candidate frame built by hand — a planted rim
/// beside a recipe that does not render it — which the exact form
/// ([`super::boundary::Owed`]) has no recipe to render for.
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
    enforce_boundary_gates(None, s_img, report, &[sky_weights], correction_shares,
        first_zone, reference_px, initial_px)
}

/// [`enforce_boundary_gate`] toward a paired `target` (R37): the horizon the
/// target itself carries is not charged. The production sky/land path hands
/// its analysis target here; `None` is the context rule alone. `initial_px`
/// must be the render of `report`'s recipe as handed in: this entry reads the
/// soft ruler's exact owed treatment ([`semantic_full_sides`]), rendered from
/// that recipe.
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
    enforce_boundary_gates_with_shrink(target, s_img, report, &[sky_weights], correction_shares,
        first_zone, (reference_px, initial_px), shrink_zone_axes, Some(&semantic_full_sides))
}

/// The semantic pair's two FULL treatments, for the exact form of the soft
/// ruler ([`super::boundary::Owed`]): `recipe` with its gated zones
/// (`first_zone..`) of one side at full weight over the whole frame and the
/// other side's absent — `[sky, land]`, `None` for a side no gated zone
/// stands on (its treatment is the reference). `None` altogether unless every
/// gated zone is one of the pair, so a caller that gates anything else keeps
/// the multiplier transport.
fn semantic_full_sides(
    recipe: &crate::recipe::EditRecipe,
    first_zone: usize,
) -> Option<[Option<crate::recipe::EditRecipe>; 2]> {
    let gated = recipe.masks.get(first_zone..)?;
    if !gated.iter().all(|m| matches!(m.role, MaskRole::ZoneSky | MaskRole::ZoneLand)) {
        return None;
    }
    Some([MaskRole::ZoneSky, MaskRole::ZoneLand].map(|role| {
        let mut side = recipe.clone();
        side.masks.truncate(first_zone);
        side.masks.extend(gated.iter().filter(|m| m.role == role).cloned().map(over_the_whole_frame));
        (side.masks.len() > first_zone).then_some(side)
    }))
}

/// `zone` acting at its full weight on every pixel of the frame: its own
/// dials, its own amount, and a geometry that covers everything — a linear
/// gradient whose two handles coincide, which the renderer reads as full
/// coverage (`render::mask_weight`), with no component, range or inversion
/// left to cut it.
fn over_the_whole_frame(mut zone: LocalAdjustment) -> LocalAdjustment {
    zone.mask = MaskGeometry::Linear { zero_x: 0.0, zero_y: 0.0, full_x: 0.0, full_y: 0.0 };
    zone.components.clear();
    zone.range = None;
    zone.inverted = false;
    zone
}

/// What a gate's caller hands [`enforce_boundary_gates_with_shrink`] when it
/// can name the sides its corrections stand on: the two full-treatment
/// recipes of a recipe whose gated corrections start at the given index
/// ([`semantic_full_sides`]).
pub(super) type FullSides<'a> =
    &'a dyn Fn(&crate::recipe::EditRecipe, usize) -> Option<[Option<crate::recipe::EditRecipe>; 2]>;

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
        first_zone, (reference_px, initial_px), shrink_zone_axes, None)
}

/// The same measured gate also handles replacement deltas. Its zero render
/// is supplied by the caller; ordinary new zones use zero controls, while a
/// native band replacement retains its accepted parent's controls at zero.
///
/// The shrink is found shared and then RELEASED PER AXIS (2026-11-10). The
/// gate reads two rims — the luma step and the colour step, each charged for
/// the part the target's own boundary does not ask ([`BoundaryReading`]) —
/// and the corrections move on two axes, the tone dials and the colour
/// dials. One scalar over both let a seam on one axis take the other with
/// it: on the night pair the sky's foot band solved +1.14 EV, the target's
/// own boundary asked a luma step of 0.290 and none of it was charged, the
/// COLOUR rim read 0.025 against the ceiling 0.012, and the one scalar took
/// the whole band to k 0.205 — the lift to +0.20 EV — leaving the colour
/// field to deliver the rest past its own bound (legs/T2: three vertices
/// saturated at the horizon, the sky's foot 1.1 L* short). So after the
/// shared bisection each axis is tried back at the full solved move, the
/// axis the shared shrink did not bind on first, and bisected between the
/// shared k and 1 where the full move does not pass; the gate's verdict is
/// the same `gated()` on every candidate, so nothing ships that the shared
/// form would have refused on a rim.
///
/// `full` (2026-11-10) is the caller's naming of the sides its corrections
/// stand on. With it every reading takes the soft ruler's EXACT owed
/// treatment — two more renders per candidate, each side's corrections at
/// full weight over the whole frame ([`super::boundary::Owed`]) — and
/// without it the settled sides' median multipliers, as before.
#[allow(clippy::too_many_arguments)]
pub(super) fn enforce_boundary_gates_with_shrink(
    target: Option<&[[f32; 3]]>,
    s_img: &DynamicImage,
    report: &mut FitReport,
    boundaries: &[&[f32]],
    correction_shares: &[f32],
    first_zone: usize,
    (reference_px, initial_px): (&[[f32; 3]], Vec<[f32; 3]>),
    shrink: impl Fn(&mut [LocalAdjustment], &[LocalAdjustment], &[f32], (f32, f32)),
    full: Option<FullSides<'_>>,
) -> BoundaryGateResult {
    let presence = report.evidence.presence.clone();
    // `[inside, outside]` at full weight for one recipe; a side no gated
    // correction stands on owes the reference itself.
    let owed_frames = |recipe: &crate::recipe::EditRecipe| -> Option<[Vec<[f32; 3]>; 2]> {
        let sides = full?(recipe, first_zone)?;
        Some(sides.map(|side| match side {
            Some(side) => fit::pixels_of(&render::develop_preview(s_img, &side)),
            None => reference_px.to_vec(),
        }))
    };
    // The frozen k=1 candidate's, rendered once.
    let frozen_owed = owed_frames(&report.recipe);
    let read = |pixels: &[[f32; 3]], owed: Option<&[Vec<[f32; 3]>; 2]>, frozen: &[[f32; 3]]| {
        let owed = owed.zip(frozen_owed.as_ref()).map(|(of_rendered, of_frozen)| super::boundary::Owed {
            rendered: [&of_rendered[0][..], &of_rendered[1][..]],
            frozen: [&of_frozen[0][..], &of_frozen[1][..]],
        });
        boundaries.iter().fold(BoundaryReading::nothing_measured(), |mut all, weights| {
            let next = super::boundary::boundary_rim_owed(target, reference_px, pixels, frozen, weights, s_img.width(), s_img.height(), presence.as_deref(), owed);
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
    let initial = read(&initial_px, frozen_owed.as_ref(), &initial_px);
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
    let render_at = |report: &mut FitReport, k: (f32, f32)| -> (BoundaryReading, Vec<[f32; 3]>) {
        shrink(
            &mut report.recipe.masks[first_zone..],
            &originals,
            &shares,
            k,
        );
        let pixels = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let owed = owed_frames(&report.recipe);
        let reading = read(&pixels, owed.as_ref(), &initial_px);
        (reading, pixels)
    };

    // INVARIANT, not a policy branch. The reading is now the rim the
    // correction INTRODUCED against `reference_px`, and `k=0` restores the
    // caller's baseline (zero controls for an addition, accepted parent
    // controls for replacement bands). It MUST render back to that baseline
    // and read exactly 0.0. A non-zero reading is an invariant failure,
    // not permission to spend a different boundary budget.
    let (zero, zero_px) = render_at(report, (0.0, 0.0));
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
        let measured = render_at(report, (mid, mid));
        if measured.0.gated() <= ZONE_BOUNDARY_RIM_MAX {
            lo = mid;
            best = measured;
        } else {
            hi = mid;
        }
    }
    // The release, per axis (see the doc): `k` is (tone, colour), `best` the
    // reading and render of `k` throughout.
    let mut k = (lo, lo);
    let colour_binds = best.0.colour_charged >= best.0.charged;
    for tone_axis in if colour_binds { [true, false] } else { [false, true] } {
        let held = k;
        let at = move |v: f32| if tone_axis { (v, held.1) } else { (held.0, v) };
        let whole = render_at(report, at(1.0));
        if whole.0.gated() <= ZONE_BOUNDARY_RIM_MAX {
            k = at(1.0);
            best = whole;
            continue;
        }
        let (mut passes, mut fails) = (lo, 1.0f32);
        for _ in 0..AXIS_RELEASE_BISECTIONS {
            let mid = (passes + fails) * 0.5;
            let measured = render_at(report, at(mid));
            if measured.0.gated() <= ZONE_BOUNDARY_RIM_MAX {
                passes = mid;
                best = measured;
            } else {
                fails = mid;
            }
        }
        k = at(passes);
    }
    if best.1.as_slice() == reference_px {
        refuse_inert(report, best.0, lo);
        return BoundaryGateResult::Dropped;
    }
    shrink(
        &mut report.recipe.masks[first_zone..],
        &originals,
        &shares,
        k,
    );
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            crate::rationale::keys::ZONE_BOUNDARY_PASSED,
            boundary_note_args(zone_count, lo, initial, best.0),
        ),
    );
    if k != (lo, lo) {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_BOUNDARY_AXES,
                vec![("tone", format!("{:.3}", k.0)), ("colour", format!("{:.3}", k.1))],
            ),
        );
    }
    BoundaryGateResult::Kept { k: lo, before: initial, after: best.0, pixels: best.1 }
}

/// Bisections of one axis's release between the shared shrink and the full
/// solved move: six halvings resolve it to 1/64 of that span, the
/// resolution the zone's own vouched-share search works to
/// ([`ZONE_VOUCH_SHRINK_STEPS`]).
const AXIS_RELEASE_BISECTIONS: usize = 6;

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

/// R19 (v1.18.0): a LOCAL correction's reading on its OWN population — the
/// frame's ruler (the report's local J*, its evidence) scoped to the
/// region's own weights, so its pairing, its bands and its worst band are
/// the region's (`fit::ships_local`).
pub(super) fn own_reading(report: &FitReport, px: &[[f32; 3]], tgt: &[[f32; 3]], sw: &[f32], tw: &[f32]) -> f32 {
    fit::look_err_with_evidence(px, tgt, &report.evidence.scoped(tgt, sw, tw))
}

/// R19: the analysis-size coverage of `masks` (the engine's own weight per
/// mask, [`render::preview_mask_coverage`], the union as the per-pixel max) —
/// the population a mask-carried local correction is judged on.
pub(super) fn masks_coverage(
    s_img: &DynamicImage, report: &FitReport, masks: &[LocalAdjustment], n: usize,
) -> Vec<f32> {
    let mut weights = vec![0.0f32; n];
    for mask in masks {
        let coverage = render::preview_mask_coverage(mask, s_img, &report.recipe);
        for (slot, value) in weights.iter_mut().zip(coverage.as_raw()) {
            *slot = slot.max(*value as f32 / 255.0);
        }
    }
    weights
}
