//! attach_one_zone: the per-zone solve and its acceptance, and the zone confidence slope.

use super::*;

/// Confidence slope on the ZONE scale — the joint family's and the global
/// fit's slopes each belong to their own metric, and this is the third.
/// Anchored on this module's own measured landings: corrections that work
/// land at 0.007-0.015 and [`ZONE_MATCHED_ERR`] (0.02) is the ceiling of
/// "matched", so 0.02 must still read as high confidence (0.90 here) while
/// the floor is reached at a zone residual of 0.15 — ten times the observed
/// good landing, i.e. a zone that was not corrected at all.
pub(super) const ZONE_CONFIDENCE_SLOPE: f32 = 5.0;

/// Fit + gate ONE zone; returns its verdict when the correction was kept.
/// The zone is measured on a fresh render of the CURRENT recipe (including
/// any zone already attached), so corrections stack the way the engine
/// renders them. Judged by the ZONE-LOCAL error (see [`ZONE_ACCEPT_RATIO`]
/// for the measured reason the frame-global metric cannot be the judge);
/// `frame_err` carries the running frame-global look distance IN and OUT.
/// Semantic zones use it for bounded-drift insurance; range bands require a
/// neutral-or-better composed frame. It is deliberately not the report's own
/// field any more (R23-6).
pub(super) fn attach_one_zone(
    s_img: &DynamicImage,
    tgt_px: &[[f32; 3]],
    report: &mut FitReport,
    frame_err: &mut f32,
    attachment: &ZoneAttachment,
    divergence: Option<fit::Divergence>,
    corr: Option<&fit::PairCorrespondence>,
) -> Option<AcceptedZone> {
    // `label` drives the rationale prose; it's the zone's stable ASCII tag, so
    // the text stays English/identical regardless of the GUI's display language.
    let label = attachment.label.as_str();
    let (sw, tw) = (
        attachment.source_weights.as_slice(),
        attachment.target_weights.as_slice(),
    );
    let cur_px = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
    // Mode is derived FIRST since step 7b: the correspondence composes only
    // into a FULL zone's estimators. An Atmosphere zone is fitted as a
    // bounded distribution precisely BECAUSE its content was replaced, and
    // weighting its statistics by "does this pixel correspond" would starve
    // the very zone the divergent-zones-are-never-dropped ruling protects.
    // Atmosphere is the claim that this zone's CONTENT was replaced, and an
    // abstention makes no claim at all — so an unread zone is fitted the way a
    // measured-matched one is, and the note below says which of the two it was.
    let mode = if divergence.is_some_and(|d| d.d >= fit::DIVERGENCE_ZONE) {
        ZoneMode::Atmosphere
    } else {
        ZoneMode::Full
    };
    // R34 §D5. MODE governs the control set; SCALE governs the estimator —
    // exactly R33 §D's split, now asked of the ZONE's own reading at the
    // FRAME's own line. A Full zone whose texture was re-synthesised while its
    // layout held (0.35 <= d < DIVERGENCE_ZONE: the desert sky) still
    // regressed target luma on source luma pixel by pixel, and the
    // errors-in-variables dilution R33 §D measured on the frame is the same
    // dilution here — the zone read 0.72 of the target's L* spread. An
    // abstention makes no claim, so it keeps the pairing it had.
    let pairing = if divergence.is_some_and(|d| d.d >= fit::DIVERGENCE_GLOBAL) {
        fit::PairingScale::Cell
    } else {
        fit::PairingScale::Pixel
    };
    // R34 §D2. The REGION's own 12x8 cells, the instrument that can admit a
    // control this zone's PIXELS cannot vouch. Built here rather than threaded
    // from the orchestration: every one of the six producers hands this
    // function the frame's own analysis target and the report's own frozen
    // evidence, so building from those two IS one instrument per solve by
    // construction — the property the threading exists for — without a
    // twenty-first parameter on twenty call sites.
    let cells = crate::fit_cells::PairedCells::build(
        tgt_px,
        s_img.width(),
        s_img.height(),
        &report.evidence,
    );
    let compose = |base: &[f32], robust: &Option<fit::PairedRobustTone>| -> Vec<f32> {
        let mut out = base.to_vec();
        if let Some(r) = robust {
            for (o, w) in out.iter_mut().zip(&r.weights) {
                *o *= w;
            }
        }
        out
    };
    let min_wt =
        |i: usize| sw.get(i).copied().unwrap_or(0.0).min(tw.get(i).copied().unwrap_or(0.0));
    // THE share ruler, and the only one (v1.2.4). A mask's share is the
    // weighted fraction of the analysis frame its own population covers —
    // `spatial::scoped_mask_evidence`'s reading for tiles and free masks, the
    // raster's own alpha for a semantic zone — and NOT that population after
    // the robust pairing has thrown outliers out of it.
    //
    // Two rulers used to answer one question. This gate read the ROBUST
    // COMPOSED share while every producer admitted its candidate on the FROZEN
    // one, so on the calibration island three tiles cleared their producer's
    // gate and were then refused here, and the two disclosures printed
    // different shares for the same tile. They are not two estimates of one
    // number: robust rejection measures how clean the tone RELATION is, which
    // is the question the do-no-harm and quality gates below already ask and
    // answer with their own numbers. Size is a question about the mask, so it
    // is asked once, of the mask, in the currency the producer used.
    let share_of = |weights: &[f32]| {
        weights.iter().map(|w| w.max(0.0) as f64).sum::<f64>() as f32
            / weights.len().max(1) as f32
    };
    // The SAME robust paired estimator the global tone stage runs (one
    // mechanism, two call sites): pixels the two zones do not share — the
    // divergent cloud deck, a moved subject — lose weight by the influence
    // function, so the moments, the dials, the tone solve and the saturation
    // chase below all measure the population the zones have in common. No
    // neutral gate here: a zone is already one coherent population and its
    // chromatic body (a blue sky) is exactly the evidence.
    //
    // With a correspondence field (step 7b), a FULL zone pairs against the
    // CORRESPONDED target and weights each pair by the field's confidence —
    // shifted content becomes evidence again instead of an outlier. Two
    // invariants hold by construction: the share GATE below never reads the
    // confidence (zone size is a zone question — composing it would let a
    // heavily-shifted zone silently vanish), and a field whose composed
    // population collapses is dropped WHOLESALE, falling back to the plain
    // pairing — the field may refuse to help, never starve a zone.
    let field = match (mode, corr) {
        (ZoneMode::Full, Some(c)) => {
            let zr = fit::paired_robust_tone(
                &cur_px,
                &c.tp,
                &|i: usize| min_wt(i) * c.conf.get(i).copied().unwrap_or(0.0),
                false,
            );
            let with_conf = |base: &[f32]| -> Vec<f32> {
                compose(base, &zr)
                    .iter()
                    .enumerate()
                    .map(|(i, w)| w * c.conf.get(i).copied().unwrap_or(0.0))
                    .collect()
            };
            let zws = with_conf(sw);
            let zwt = with_conf(tw);
            let ms = zone_moments(&cur_px, &zws);
            let mt = zone_moments(&c.tp, &zwt);
            (ms.share >= attachment.min_share && mt.share >= attachment.min_share)
                .then_some((c, zr, zws, zwt, ms, mt))
        }
        _ => None,
    };
    let (tgt_eff, zone_robust, zw_source, zw_target, ms, mt, gate_s_share, gate_t_share) =
        match field {
            Some((c, zr, zws, zwt, ms, mt)) => {
                (c.tp.as_slice(), zr, zws, zwt, ms, mt, share_of(sw), share_of(tw))
            }
            None => {
                let zr = fit::paired_robust_tone(&cur_px, tgt_px, &|i: usize| min_wt(i), false);
                let zws = compose(sw, &zr);
                let zwt = compose(tw, &zr);
                let ms = zone_moments(&cur_px, &zws);
                let mt = zone_moments(tgt_px, &zwt);
                (tgt_px, zr, zws, zwt, ms, mt, share_of(sw), share_of(tw))
            }
        };
    if gate_s_share < attachment.min_share || gate_t_share < attachment.min_share {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_TOO_SMALL,
                vec![
                    ("label", label.to_string()),
                    ("s", format!("{:.0}", gate_s_share * 100.0)),
                    ("t", format!("{:.0}", gate_t_share * 100.0)),
                ],
            ),
        );
        return None;
    }
    let mode_key = match mode {
        ZoneMode::Full => crate::rationale::keys::ZONE_MODE_FULL,
        ZoneMode::Atmosphere => crate::rationale::keys::ZONE_MODE_ATMOSPHERE,
    };
    crate::rationale::push_note(
        &mut report.recipe.rationale,
        &mut report.notes,
        crate::rationale::Note::new(
            mode_key,
            vec![
                ("label", label.to_string()),
                ("d", divergence.map_or_else(
                    || "unmeasured".to_string(), |d| format!("{:.3}", d.d))),
            ],
        ),
    );
    // Only the CELL scale earns a sentence: Pixel is what every zone did
    // before R34 and what the mode note already describes, so a zone whose
    // estimator did not change says nothing new and its rationale stays
    // byte-identical.
    if mode == ZoneMode::Full && pairing == fit::PairingScale::Cell {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_PAIRING_SCALE,
                vec![
                    ("label", label.to_string()),
                    ("d", divergence.map_or_else(
                        || "unmeasured".to_string(), |d| format!("{:.3}", d.d))),
                    ("line", format!("{:.2}", fit::DIVERGENCE_GLOBAL)),
                ],
            ),
        );
    }
    let zone_before = zone_err(&ms, &mt);
    // Composition is an input fact, not an acceptance verdict. Disclose it
    // before any early evidence/quality return so a withheld correction still
    // explains why the two zone populations are not comparable.
    let (lo, hi) = (gate_s_share.min(gate_t_share), gate_s_share.max(gate_t_share));
    if hi > 2.0 * lo {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_SHARE_MISMATCH,
                vec![
                    ("label", label.to_string()),
                    ("s", format!("{:.0}", gate_s_share * 100.0)),
                    ("t", format!("{:.0}", gate_t_share * 100.0)),
                ],
            ),
        );
        return None;
    }
    // Already-matched zone: attempting a fit would be dialling noise — the
    // observed attempts regress (land 0.009 → 0.029 on the live pair), and
    // the old outcome message ("dropped: needs ≤ 50%") read as a discarded
    // improvement. Say what is true instead: there is nothing to correct.
    // The EV companion keeps the linear-light skip line honest in dark
    // zones (see [`ZONE_MATCHED_EV`]).
    let ev_gap = (mt.luma_lin.max(1e-6) / ms.luma_lin.max(1e-6)).log2().abs();
    if zone_skips(zone_before, ev_gap) {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_ALREADY_MATCHED,
                vec![("label", label.to_string()), ("before", format!("{zone_before:.3}"))],
            ),
        );
        return None;
    }
    if let Some(r) = zone_robust.as_ref()
        && r.rejected_share >= fit::ROBUST_REJECT_DISCLOSE_MIN
    {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_ROBUST_REJECTED,
                vec![
                    ("label", label.to_string()),
                    ("pct", format!("{:.0}", r.rejected_share * 100.0)),
                    (
                        "ranges",
                        if r.rejected_ranges.is_empty() {
                            "scattered".to_string()
                        } else {
                            r.rejected_ranges.clone()
                        },
                    ),
                ],
            ),
        );
    }
    let d = fit_zone_dials(&ms, &mt);
    let round1 = |v: f32| (v * 10.0).round() / 10.0;
    let round2 = |v: f32| (v * 100.0).round() / 100.0;
    report.recipe.masks.push(LocalAdjustment {
        mask: attachment.mask.clone(),
        components: attachment.components.clone(),
        range: attachment.range,
        name: attachment.name.clone(),
        role: attachment.role,
        amount: 1.0,
        inverted: attachment.inverted,
        color_gains: Some(
            match mode {
                ZoneMode::Full => d.color_gains,
                ZoneMode::Atmosphere => shrink_atmosphere_gains_in(
                    d.color_gains,
                    fit::FitBudget::for_strength(fit::carried_strength_from_notes(&report.notes))
                        .zone_gain,
                ),
            }
            .map(round2),
        ),
        ..Default::default()
    });
    // Within-zone tone: the zone's brightness/contrast is a DISTRIBUTION,
    // not a mean (see [`zone_luma_cdf`] — the real pair's land matched the
    // linear mean and still rendered far darker than the target). Map the
    // zone's weighted luma CDF onto the target zone's and solve the engine's
    // own local tone sliders from it — the same basis + magnitude prior as
    // the global stage 1. This SUPERSEDES the moment-EV from fit_zone_dials
    // (which now only normalises the gains): brightness lives here, on the
    // render of the gains-only mask so the solve sees the recoloured zone.
    //
    // IDENTIFIABILITY GUARD: a quantile map out of a near-uniform source
    // zone is degenerate — a monotone map cannot spread a luma spike into
    // the target's wide distribution, and the slider solve goes wild on the
    // violent pseudo-map instead (measured, real pair: the flat hazy sky
    // drew exposure −0.70 and its zone residual went 0.016 → 0.108). Below
    // an IQR floor, fall back to the moment-EV and leave the tone flat.
    // A NARROW band's own population can testify at fewer than two of the
    // engine's eight tone knots, and `fit_tone_sliders_supported` answers that
    // with a neutral solve rather than a one-point system. That is the right
    // answer and it used to be a silent one: the zone attached with its colour
    // move and a tone residual nothing had refused, explained, or named. The
    // count is taken inside the solve's own borrow and disclosed after it.
    let mut unsupported_knots: Option<usize> = None;
    if mode == ZoneMode::Full {
        let rp = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let s_cdf = zone_luma_cdf(&rp, &zw_source);
        let src_iqr = fit::quantile(&s_cdf, 0.75) - fit::quantile(&s_cdf, 0.25);
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        if src_iqr >= 0.05 {
            // Paired robust regression on the recoloured render (the gains
            // just changed every zone luma, so the map must be re-estimated),
            // falling back to the weighted quantile transport only when the
            // paired estimate is too thin — and either way solving only the
            // knots this zone's own population supports.
            // R34 §D5. At CELL scale index `i` on one side is not index `i`
            // on the other, so this regression is a well-formed answer to a
            // question nobody asked; the weighted quantile transport below is
            // the honest estimator, and an empty `points` is what selects it.
            // The rule is `fit::paired_correspondence`'s, asked of the zone's
            // reading instead of the frame's — one doctrine, two scopes.
            let refit = (pairing == fit::PairingScale::Pixel)
                .then(|| {
                    fit::paired_robust_tone(
                        &rp,
                        tgt_eff,
                        &|i: usize| {
                            zw_source
                                .get(i)
                                .copied()
                                .unwrap_or(0.0)
                                .min(zw_target.get(i).copied().unwrap_or(0.0))
                        },
                        false,
                    )
                })
                .flatten();
            let points = refit.as_ref().map(|r| r.points.clone()).unwrap_or_default();
            let support = fit::knot_support_for(&rp, &zw_source, &points);
            let score_set: Vec<(f32, f32, f32)> = match refit.as_ref() {
                Some(r) if r.points.len() >= 6 => r
                    .points
                    .iter()
                    .zip(&r.masses)
                    .map(|(&(x, y), &mass)| (x, y, mass))
                    .collect(),
                _ => Vec::new(),
            };
            let t_cdf = zone_luma_cdf(tgt_eff, &zw_target);
            let tone_map = |x: f32| {
                if points.len() >= 6 {
                    fit::sample_tone_points(&points, x)
                } else {
                    fit::quantile(
                        &t_cdf,
                        fit::cdf_at(&s_cdf, x).clamp(fit::P_CLIP, 1.0 - fit::P_CLIP),
                    )
                }
            };
            let supported_knots = support.iter().filter(|value| **value > 0.0).count();
            if supported_knots < 2 {
                unsupported_knots = Some(supported_knots);
            }
            let (ev, sliders) =
                fit::fit_tone_sliders_supported(&tone_map, &support, &score_set);
            m.exposure_ev = round2(ev.clamp(-ZONE_EV_LIMIT, ZONE_EV_LIMIT));
            m.contrast = round1(sliders[0] * 100.0);
            m.highlights = round1(sliders[1] * 100.0);
            m.shadows = round1(sliders[2] * 100.0);
            m.whites = round1(sliders[3] * 100.0);
            m.blacks = round1(sliders[4] * 100.0);
        } else {
            m.exposure_ev = round2(d.exposure_ev.clamp(-ZONE_EV_LIMIT, ZONE_EV_LIMIT));
        }
    } else {
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        m.exposure_ev = round2(d.exposure_ev.clamp(-ZONE_ATMOS_EV_LIMIT, ZONE_ATMOS_EV_LIMIT));
    }
    if let Some(knots) = unsupported_knots {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_TONE_UNSUPPORTED,
                vec![("label", label.to_string()), ("knots", knots.to_string())],
            ),
        );
    }
    // Closed-loop zone saturation on real renders (the gains change chroma
    // by themselves — only a render knows where the zone landed).
    for _ in 0..2 {
        let rp = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
        let zone_chroma = zone_moments(&rp, &zw_source).chroma;
        let Some(step) = zone_sat_step(zone_chroma, mt.chroma) else { break };
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        let next = clamp_zone_sat_for_mode((m.saturation + step).round(), mode);
        if next == m.saturation {
            break;
        }
        m.saturation = next;
    }
    // Evidence verdicts follow both the population a correction MOVES and its
    // mode. Atmosphere zones scope the report's one frame ruler (population
    // evidence in an Atmosphere report). Full zones retain structural survival,
    // so inside an Atmosphere frame they scope the separately carried structural
    // model. This single branch covers semantic zones, ranges and tiles.
    let (moved_source, moved_target) = match &attachment.coverage {
        Some(coverage) => (coverage.source.as_slice(), coverage.target.as_slice()),
        None => (sw, tw),
    };
    let frame_evidence = match mode {
        ZoneMode::Atmosphere => &report.evidence,
        ZoneMode::Full => report.structural_evidence.as_ref().unwrap_or(&report.evidence),
    };
    let zone_evidence = frame_evidence.scoped(tgt_px, moved_source, moved_target);
    // Probe the two control classes independently. A one-sided hue band must
    // withhold only chroma movement; supported luminance evidence still earns
    // the zone correction.
    let mut luma_probe = report.recipe.clone();
    {
        let m = luma_probe.masks.last_mut().expect("zone mask just pushed");
        m.color_gains = Some([1.0; 3]);
        m.saturation = 0.0;
    }
    let luma_probe_px = fit::pixels_of(&render::develop_preview(s_img, &luma_probe));
    let luma_ranges = fit::moved_unsupported_luma_range_names(
        &cur_px,
        &luma_probe_px,
        &zone_evidence,
    );
    let mut chroma_probe = report.recipe.clone();
    {
        let m = chroma_probe.masks.last_mut().expect("zone mask just pushed");
        m.exposure_ev = 0.0;
        m.contrast = 0.0;
        m.highlights = 0.0;
        m.shadows = 0.0;
        m.whites = 0.0;
        m.blacks = 0.0;
    }
    let chroma_probe_px = fit::pixels_of(&render::develop_preview(s_img, &chroma_probe));
    let hue_bands = fit::moved_unsupported_hue_range_names(
        &cur_px,
        &chroma_probe_px,
        &zone_evidence,
    );
    // R34 §D2. THE admission gate for a region whose PIXELS cannot pair.
    //
    // R33 §F refused to consult `fit_cells` here, on the ground that
    // `fit_zone_dials` solves `color_gains` from this zone's MASK-WEIGHTED
    // MEAN moments while a cell voucher restricted to the same mask reads
    // cell MEANS over the same pixels — one quantity, therefore no
    // independent verdict. That argument is superseded, and it was wrong in
    // one specific way: the voucher reads 96 cells, and since R34 §D1 it also
    // reads the DIRECTION of each cell's move against the direction to its
    // OWN target mean. A zone-wide gain that matches the zone mean while
    // dragging cells whose targets lie elsewhere converges the mean and fails
    // the partition. The estimator's objective is one number; the voucher's
    // measurement is 96 directed ones, so the verdict stops being a
    // tautology: R33 measured 1.000/0.000 for any probe that moved anything
    // at all, while the reference sky reads 0.865 converged / 0.135 diverged
    // / 1.000 ALIGNED — every one of its cells wants the same warm push, and
    // a seventh of its trust-weighted mass still ends further from its own
    // target than it started.
    //
    // This is also the ONE place R33's "Atmosphere passes no cells" is
    // deliberately not followed, and for the reason that doctrine exists. An
    // Atmosphere zone is the CLAIM that this region's content was replaced,
    // and the cells are exactly the instrument that separates a replaced
    // TEXTURE (layout intact — a recolour, recoverable) from a replaced
    // LAYOUT (nothing to recover). Passing no cells here would refuse the
    // question rather than answer it. What does NOT change is the budget:
    // `shrink_atmosphere_gains_in` and `zone_gain` still bound whatever is
    // admitted, because admission is evidence and budget is strength.
    //
    // AND IT IS ASKED ONLY WHERE THE PIXELS CANNOT ANSWER. That is the whole
    // lane's rule, not a second one: a region whose own structural reading is
    // under `DIVERGENCE_GLOBAL` has pixels that ARE each other's counterparts,
    // the pixel-scale refusal about it is a measurement, and a region may not
    // overrule its own pixels. Measured: on the toy zone fixtures (sky D 0.024
    // to 0.101) the cells read 1.000/0.000/1.000 and shipped a colour move
    // twelve pinned refusals exist to refuse — while the sentence printed for
    // it, "because this region's texture was re-synthesised", was false about
    // every one of them. The reference sky reads 0.617 and is the case this
    // arm was built for.
    // Each probe is asked along the axis it can move: the colour probe has its
    // tone zeroed and the tone probe its colour (`MoveAxis`).
    let region_vouch = |probe: &[[f32; 3]], axis: MoveAxis| -> Option<crate::fit_cells::CellVouch> {
        if pairing != fit::PairingScale::Cell {
            return None;
        }
        cells.as_ref().map(|c| c.vouch_along(axis, &cur_px, probe, Some(moved_source)))
    };
    let hue_cells = hue_bands.as_ref().and_then(|_| region_vouch(&chroma_probe_px, MoveAxis::Colour));
    let luma_cells = luma_ranges.as_ref().and_then(|_| region_vouch(&luma_probe_px, MoveAxis::Tone));
    // R36. A refusal on SIZE is not a refusal on DIRECTION. The reference sky
    // read 0.865 converged / 0.135 diverged / 1.000 aligned: every cell asked
    // for the warm push and a seventh of the mass was pushed PAST its own
    // target — the zone-wide gain the mean asked for was too large for those
    // cells, not wrong for them. R34 answered that by withholding the whole
    // gain, and the colour field then carried the sky at its saturated bound.
    // What the cells CAN vouch is searched for instead: the largest share k
    // of the solved move (gains and saturation scaled together, direction
    // kept) whose RENDER the same voucher admits. Admission stays evidence —
    // the move that ships is the move the cells vouched — and the search is
    // a budget on size, which is what a share of a move is. It is asked only
    // where the direction agrees: a region whose cells want opposite moves
    // (`aligned` under its line) is not asking for less of one move, and
    // stays refused exactly as before.
    let vouched_share = |probe_at: &dyn Fn(f32) -> crate::recipe::EditRecipe,
                         full: Option<crate::fit_cells::CellVouch>,
                         axis: MoveAxis|
     -> Option<(f32, crate::fit_cells::CellVouch)> {
        let full = full?;
        if full.vouched() || !full.direction_agrees() {
            return None;
        }
        let (mut lo, mut hi, mut best) = (0.0f32, 1.0f32, None);
        for _ in 0..ZONE_VOUCH_SHRINK_STEPS {
            let mid = 0.5 * (lo + hi);
            let px = fit::pixels_of(&render::develop_preview(s_img, &probe_at(mid)));
            match region_vouch(&px, axis) {
                Some(v) if v.vouched() => {
                    best = Some((mid, v));
                    lo = mid;
                }
                _ => hi = mid,
            }
        }
        best
    };
    let colour_at = |k: f32| {
        let mut probe = chroma_probe.clone();
        scale_zone_colour(probe.masks.last_mut().expect("zone mask just pushed"), k, mode);
        probe
    };
    let tone_at = |k: f32| {
        let mut probe = luma_probe.clone();
        scale_zone_tone(probe.masks.last_mut().expect("zone mask just pushed"), k);
        probe
    };
    let hue_shrunk = hue_bands.as_ref().and_then(|_| vouched_share(&colour_at, hue_cells, MoveAxis::Colour));
    let luma_shrunk = luma_ranges.as_ref().and_then(|_| vouched_share(&tone_at, luma_cells, MoveAxis::Tone));
    // An ABSTENTION is never an admission: no cell grid, or a region whose
    // cells carried no trust-weighted evidence mass, leaves the strict
    // refusal exactly where it was. That is what keeps every zero-evidence
    // pin standing by construction rather than by threshold.
    let colour_withheld = hue_bands.is_some()
        && !hue_cells.is_some_and(|v| v.vouched())
        && hue_shrunk.is_none();
    let tone_withheld = luma_ranges.is_some()
        && !luma_cells.is_some_and(|v| v.vouched())
        && luma_shrunk.is_none();
    if tone_withheld {
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        m.exposure_ev = 0.0;
        m.contrast = 0.0;
        m.highlights = 0.0;
        m.shadows = 0.0;
        m.whites = 0.0;
        m.blacks = 0.0;
    } else if let Some((k, _)) = luma_shrunk {
        scale_zone_tone(report.recipe.masks.last_mut().expect("zone mask just pushed"), k);
    }
    if colour_withheld {
        let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
        m.color_gains = Some([1.0; 3]);
        m.saturation = 0.0;
    } else if let Some((k, _)) = hue_shrunk {
        scale_zone_colour(report.recipe.masks.last_mut().expect("zone mask just pushed"), k, mode);
    }
    // ONE NOTE PER CONTROL CLASS, AND IT NAMES THE OUTCOME. The two probes
    // withhold independently, so a single "correction withheld" sentence
    // described none of the three outcomes correctly. Since R34 each class
    // has FOUR: strict-clean (nothing said, the attach note carries it),
    // shipped-on-region-evidence, refused-by-the-cells, and
    // the-cells-abstained. A refusal is now a measurement — it prints the
    // three shares it was decided on — so "withheld" can no longer be read as
    // "nobody looked".
    let cells_said = |verdict: Option<crate::fit_cells::CellVouch>| -> String {
        match verdict {
            Some(v) if v.read > 0 => {
                let (converged, diverged, aligned) = v.shares();
                format!(
                    "{converged} converged, {diverged} diverged, {aligned} aligned over {} cells",
                    v.read
                )
            }
            _ => "abstained: no cell of this region carried measurable evidence".to_string(),
        }
    };
    if let Some(hue_bands) = hue_bands.as_ref() {
        let note = match (hue_shrunk, colour_withheld, hue_cells) {
            (Some((k, v)), _, Some(full)) => {
                let (converged, diverged, aligned) = v.shares();
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_COLOUR_VOUCHED_AT_SHARE,
                    vec![
                        ("label", label.to_string()),
                        ("hue_bands", hue_bands.clone()),
                        ("share", format!("{k:.3}")),
                        ("full", cells_said(Some(full))),
                        ("converged", converged),
                        ("diverged", diverged),
                        ("aligned", aligned),
                    ],
                )
            }
            (_, false, Some(v)) => {
                let (converged, diverged, aligned) = v.shares();
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_COLOUR_VOUCHED_BY_CELLS,
                    vec![
                        ("label", label.to_string()),
                        ("hue_bands", hue_bands.clone()),
                        ("converged", converged),
                        ("diverged", diverged),
                        ("aligned", aligned),
                    ],
                )
            }
            _ if pairing == fit::PairingScale::Cell => crate::rationale::Note::new(
                crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR_CELLS,
                vec![
                    ("label", label.to_string()),
                    ("hue_bands", hue_bands.clone()),
                    ("cells", cells_said(hue_cells)),
                ],
            ),
            // A region whose pixels pair was never asked, so its sentence is
            // R33's, unchanged to the byte.
            _ => crate::rationale::Note::new(
                crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_COLOUR,
                vec![("label", label.to_string()), ("hue_bands", hue_bands.clone())],
            ),
        };
        crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes, note);
    }
    if let Some(luma_ranges) = luma_ranges.as_ref() {
        let note = match (luma_shrunk, tone_withheld, luma_cells) {
            (Some((k, v)), _, Some(full)) => {
                let (converged, diverged, aligned) = v.shares();
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_TONE_VOUCHED_AT_SHARE,
                    vec![
                        ("label", label.to_string()),
                        ("luma_ranges", luma_ranges.clone()),
                        ("share", format!("{k:.3}")),
                        ("full", cells_said(Some(full))),
                        ("converged", converged),
                        ("diverged", diverged),
                        ("aligned", aligned),
                    ],
                )
            }
            (_, false, Some(v)) => {
                let (converged, diverged, aligned) = v.shares();
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_TONE_VOUCHED_BY_CELLS,
                    vec![
                        ("label", label.to_string()),
                        ("luma_ranges", luma_ranges.clone()),
                        ("converged", converged),
                        ("diverged", diverged),
                        ("aligned", aligned),
                    ],
                )
            }
            _ if pairing == fit::PairingScale::Cell => crate::rationale::Note::new(
                crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE_CELLS,
                vec![
                    ("label", label.to_string()),
                    ("luma_ranges", luma_ranges.clone()),
                    ("cells", cells_said(luma_cells)),
                ],
            ),
            _ => crate::rationale::Note::new(
                crate::rationale::keys::ZONE_EVIDENCE_WITHHELD_TONE,
                vec![("label", label.to_string()), ("luma_ranges", luma_ranges.clone())],
            ),
        };
        crate::rationale::push_note(&mut report.recipe.rationale, &mut report.notes, note);
    }
    // The skip line, re-asked of the class that can still move: with colour
    // withheld the acceptance below judges the luma-only residual, so a zone
    // already matched THERE is left alone with the honest note instead of
    // being dialled for a hairline tone gain against a chroma gap it may not
    // touch (the calibration land: luma 0.004, chroma-dominated 0.045).
    if colour_withheld && !tone_withheld {
        let luma_before = zone_luma_err(&ms, &mt);
        if zone_skips(luma_before, ev_gap) {
            report.recipe.masks.pop();
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_ALREADY_MATCHED,
                    vec![("label", label.to_string()), ("before", format!("{luma_before:.3}"))],
                ),
            );
            return None;
        }
    }
    if colour_withheld && !tone_withheld {
        let original = {
            let m = report.recipe.masks.last().expect("zone mask just pushed");
            [m.exposure_ev, m.contrast, m.highlights, m.shadows, m.whites, m.blacks]
        };
        // The fitted tone FIRST: the ladder backs a zone off only when the
        // quality gate refuses the step above, and until 2026-09-24 it began
        // at 0.75, so every luma-only zone shipped at three quarters of the
        // tone it had fitted whether or not the full move passed.
        for factor in [1.0f32, 0.75, 0.5, 0.25, 0.0] {
            {
                let m = report.recipe.masks.last_mut().expect("zone mask just pushed");
                m.exposure_ev = original[0] * factor;
                m.contrast = original[1] * factor;
                m.highlights = original[2] * factor;
                m.shadows = original[3] * factor;
                m.whites = original[4] * factor;
                m.blacks = original[5] * factor;
            }
            let probe = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
            if local_quality(&cur_px, &probe, sw, s_img.width(), s_img.height()).passes() {
                break;
            }
        }
    }
    let neutral_zone = {
        let m = report.recipe.masks.last().expect("zone mask just pushed");
        m.exposure_ev.abs() <= 1e-4
            && m.contrast.abs() <= 1e-4
            && m.highlights.abs() <= 1e-4
            && m.shadows.abs() <= 1e-4
            && m.whites.abs() <= 1e-4
            && m.blacks.abs() <= 1e-4
            && m.saturation.abs() <= 1e-4
            && m.color_gains.map(|g| g.iter().all(|v| (*v - 1.0).abs() <= 1e-4)).unwrap_or(true)
    };
    if neutral_zone {
        report.recipe.masks.pop();
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                // NOT ZONE_ALREADY_MATCHED. That sentence is correct at the two
                // `zone_skips` exits above, where the residual really is under
                // the skip threshold. HERE the only thing established is that
                // the SOLUTION came out neutral -- see the key's own doc.
                crate::rationale::keys::ZONE_NO_MOVEMENT_SURVIVED,
                vec![("label", label.to_string()), ("before", format!("{zone_before:.3}"))],
            ),
        );
        return None;
    }
    let zoned_px = fit::pixels_of(&render::develop_preview(s_img, &report.recipe));
    let zoned_err = fit::look_err_with_evidence(&zoned_px, tgt_px, &report.evidence);
    // The SAME population `zone_before` was read on (`ms` over the robust-
    // composed `zw_source`, against `mt` over `zw_target`): until 2026-09-24
    // the after-reading used the raw attachment weights, so a zone's
    // before/after pair compared two populations and the final-stack
    // remeasurement below read a third.
    let m_after = zone_moments(&zoned_px, &zw_source);
    let zone_after = zone_err(&m_after, &mt);
    let ev_after = (mt.luma_lin.max(1e-6) / m_after.luma_lin.max(1e-6)).log2().abs();
    let quality = local_quality(&cur_px, &zoned_px, sw, s_img.width(), s_img.height());
    if quality.passes() {
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                crate::rationale::keys::ZONE_QUALITY_PASSED,
                vec![
                    ("label", label.to_string()),
                    ("texture", format!("{:.3}", quality.texture_ratio)),
                    ("clip_before", format!("{:.2}", quality.clipped_before * 100.0)),
                    ("clip_after", format!("{:.2}", quality.clipped_after * 100.0)),
                ],
            ),
        );
    } else {
        report.recipe.masks.pop();
        if !quality.texture_passes() {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_QUALITY_TEXTURE_FAILED,
                    vec![
                        ("label", label.to_string()),
                        ("ratio", format!("{:.3}", quality.texture_ratio)),
                        ("min", format!("{ZONE_TEXTURE_MIN:.2}")),
                        ("max", format!("{ZONE_TEXTURE_MAX:.2}")),
                    ],
                ),
            );
        }
        if !quality.clipping_passes() {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_QUALITY_CLIPPING_FAILED,
                    vec![
                        ("label", label.to_string()),
                        ("before", format!("{:.2}", quality.clipped_before * 100.0)),
                        ("after", format!("{:.2}", quality.clipped_after * 100.0)),
                        ("growth", format!("{:.2}", ZONE_CLIP_GROWTH * 100.0)),
                    ],
                ),
            );
        }
        return None;
    }
    // The acceptance judges the residual the zone was allowed to move: with
    // colour withheld and tone kept, that is the luma-only one. A class the
    // REGION's cells admitted is a class that shipped, so it is judged in the
    // whole-zone residual like any other shipped control.
    let accepted_before = if colour_withheld && !tone_withheld {
        zone_luma_err(&ms, &mt)
    } else {
        zone_before
    };
    let accepted_after = if colour_withheld && !tone_withheld {
        zone_luma_err(&m_after, &mt)
    } else {
        zone_after
    };
    let arm = match mode {
        ZoneMode::Full => zone_accepts(
            accepted_before,
            accepted_after,
            ev_after,
            *frame_err,
            zoned_err,
        ),
        // Atmosphere keeps its own do-no-harm arm: a bounded atmosphere move
        // is judged only on not making its zone worse, and the absolute-gain
        // arm has no calibration there.
        ZoneMode::Atmosphere => {
            (accepted_after <= accepted_before).then_some(ZoneAcceptArm::AtmosphereDoNoHarm)
        }
    };
    if arm.is_some() && zoned_err <= *frame_err + attachment.frame_regression_tol {
        // The strictly-better arm is the one that changes what ships, so it
        // says so — with the two readings it was decided on, in the zone's
        // own unit and the frame's. Without this a reader cannot tell a
        // correction that halved from one that was admitted by the R30
        // relaxation (the free-mask lesson: a disclosure that never reaches
        // the note table is a disclosure that was not made).
        if arm == Some(ZoneAcceptArm::StrictlyBetter) {
            crate::rationale::push_note(
                &mut report.recipe.rationale,
                &mut report.notes,
                crate::rationale::Note::new(
                    crate::rationale::keys::ZONE_STRICTLY_BETTER,
                    vec![
                        ("label", label.to_string()),
                        ("before", format!("{accepted_before:.3}")),
                        ("after", format!("{accepted_after:.3}")),
                        ("gain", format!("{ZONE_MIN_ABS_GAIN:.3}")),
                        ("frame_before", format!("{:.5}", *frame_err)),
                        ("frame_after", format!("{zoned_err:.5}")),
                        ("texture", format!("{:.3}", quality.texture_ratio)),
                    ],
                ),
            );
        }
        // The running frame-global value advances so the NEXT zone's drift
        // budget is measured from here — but neither `err_after` nor
        // `confidence` is written from it any more (R23-6): see the comment
        // in [`attach_zones`] and this module's own [`ZONE_ACCEPT_RATIO`]
        // proof that this number cannot judge a zone.
        *frame_err = zoned_err;
        Some(AcceptedZone {
            label: attachment.label.clone(),
            range: attachment.range,
            mask_index: report.recipe.masks.len() - 1,
            source_weights: zw_source,
            target_weights: zw_target,
            mask_weights: attachment.source_weights.clone(),
            before: zone_before,
            after: zone_after,
            rendered: zoned_px,
            judged_before: accepted_before,
            luma_only: colour_withheld && !tone_withheld,
            frame_regression_tol: attachment.frame_regression_tol,
        })
    } else {
        report.recipe.masks.pop();
        crate::rationale::push_note(
            &mut report.recipe.rationale,
            &mut report.notes,
            crate::rationale::Note::new(
                match mode {
                    ZoneMode::Full => crate::rationale::keys::ZONE_DROPPED,
                    ZoneMode::Atmosphere => crate::rationale::keys::ZONE_ATMOSPHERE_DROPPED,
                },
                vec![
                    ("label", label.to_string()),
                    // The pair the verdict was decided on: with colour
                    // withheld and tone kept that is the luma-only residual,
                    // not the whole-zone one an accepted zone records.
                    ("before", format!("{accepted_before:.3}")),
                    ("after", format!("{accepted_after:.3}")),
                    ("ratio", format!("{:.0}", ZONE_ACCEPT_RATIO * 100.0)),
                    ("floor", format!("{ZONE_MATCHED_ERR:.3}")),
                    ("gain", format!("{:.0}", (1.0 - ZONE_FLOOR_MIN_GAIN) * 100.0)),
                    ("drift", format!("{:+.5}", zoned_err - *frame_err)),
                    ("tol", format!("{:+.5}", attachment.frame_regression_tol)),
                ],
            ),
        );
        None
    }
}
