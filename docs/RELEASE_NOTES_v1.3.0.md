# AutoShade v1.3.0 — the reverse-fit reads one frame, ships what Lightroom can read, and charges only the seams the target did not ask for

v1.2.6 fixed one download. v1.3.0 is the reverse-fit rebuilt from its source
frame outward, in the lanes R33–R38 and the smaller batches around them, each
measured on the same reference pair — a desert-dusk RAW and a target that an
image model regenerated from it, so the sky's texture is re-synthesised and
only its layout survives — at the three strengths a user can set. It is the
case v1.2.6 handled worst: the command line and the desktop app disagreed
about which solver to run on it, and the saved desktop fit sat at a
whole-frame mean |diff| of 0.0949 against the target.

At strength 0.85 the same pair now lands at a sky ΔE of **4.9** (the first
tree that read one source frame measured 18.2 there), whole-frame mean |diff|
**0.0276** against v1.2.6's 0.0571 (command line) and 0.0949 (saved desktop
fit); the sky and ground zones ride out as Lightroom's own Select Sky mask,
a tile that earns a native carrier rides out as four intersecting gradients,
and every mask that stays engine-only is named in the save line. The numbers
behind each sentence below are in the ROADMAP's v1.3.0 ledger, lane by lane.

## One source frame, two scales (R33 §A–§B)

The command line's `match` read the camera's embedded JPEG preview when one
existed; the desktop app read a neutral develop of the sensor frame. On the
reference pair the preview measured a structure divergence of **0.361** and
chose the Atmosphere solver; the neutral develop of the same frame measured
**0.275** and chose the full solve. One function, `pipeline::fit_source`,
now serves both entries — a 2048 px neutral develop plus the RAW's own
calibration — and the preview branch is gone, with its note.

A second, coarser reading of the same divergence (an 8 px low-pass before
the statistic) was added on the hypothesis that a re-synthesised target would
read as more pairable at layout scale. It reads the opposite — whole frame
0.275 at pixel scale, **0.609** at layout scale; sky 0.620 / 0.959 — because
the luminance is rank-equalised per histogram first, so removing the shared
fine texture leaves exactly the regions the regeneration moved. The mode
line therefore stays on the pixel-scale reading; the coarse one is measured,
pinned and disclosed: the CLI prints `solver: full solve · paired at pixel
scale (D 0.275 at pixel scale, 0.609 at layout scale)`, the desktop status
line says the same.

## The region's cells vouch what its pixels cannot (R33 §C–§F, R34)

The pairing evidence is per pixel, and on a re-synthesised sky no pixel has
a partner — so every rule that asked "did this edit move the pixels toward
their partners" refused, correctly, from a premise that did not hold there.
v1.3.0 adds one admission instrument, `fit_cells`: the 12×8 evidence grid the
colour field already used, answering one question only — did this edit move
each cell's mean toward the target's cell mean. Cells only lift a refusal;
they never estimate, never supply a value, and abstaining never counts as a
vouch. A vouch needs **≥ 0.70** of the moved mass closer, **≤ 0.10** further,
and (R34) **≥ 0.70** moving *toward* the target within 60° — the direction
arm is what separates "too far" from "wrong way".

What the cells changed in what ships:

- **The full solve has a white balance for the first time.** `temperature_k`
  was only ever assigned on the Atmosphere branch, so the commonest kind of
  pair — the same frame regraded — could never be told the light changed
  colour, and expressed the cast through saturation and mixer bands that the
  hue gate mostly refused. The white balance is now solved once, shared by
  both modes, rendered, and shipped only if the target's own cells vouch the
  rendering; both outcomes are disclosed. On the reference pair: 1.000
  closer / 0.000 further, shipped.
- **The mixer's hardest band.** A neutral develop has no warmth, so a warm
  target's Orange band has no source members to vouch it and stayed neutral
  forever. It is admitted when, on the render the stage is solving against
  (light already corrected), the band is two-sided *and* the cells holding
  its members vouched the earlier stages.
- **Zones whose pixels do not pair solve their tone from their own
  population** instead of a per-pixel regression that, on re-synthesised
  texture, regresses one noise against another and reads contrast low.
- **A move the cells refuse only for its size ships at the share they
  vouch** (R36). The reference sky read 0.865 closer / 0.135 further / 1.000
  toward: every cell wanted the same warm push, one seventh were pushed past
  their target. The zone's colour and tone are bisected toward the source
  (six renders at most) and shipped at the largest share the same cells
  vouch — **0.859** on the reference sky, gains [1.04 0.98 0.95] where v1.2.6
  shipped [1.00 1.00 1.00]. Opposite-direction moves are still refused whole.
- **The strength dial reaches the one window it could not.** The Atmosphere
  zone's per-channel gains were clamped by two file constants that did not
  follow strength; they are now `FitBudget::zone_gain`, byte-identical at the
  default point.

The cast arm's cell vouch is read against the pixel's own spatial weights,
which is why it still carries the reference pair's [Aqua, Blue] and still
refuses the canyon-gold pair, where an earlier draft turned a pale sky 158°
into the target's own gold. Cells answer where pixels *cannot be asked*, never
where pixels were asked and said no.

## The colour field is a control (R33 §G, R34 §D4)

The 12×8×8 bilateral grid that has measured the residual since v1.2.4 now
ships, as `EditRecipe.colour_field`, **only past the default strength**: the
dial means "how far past Lightroom may this fit go", and this is the first
control that leaves Lightroom entirely. At or below the default the stage is
inert and the recipe writes no key, so archived recipes keep their hashes.
It renders last, after the masks, because it is the residual the masks could
not reach. It is solved twice — the analyser's pass unchanged, then a pass
without the local-support weight that starved exactly the repainted cells —
and the second pass is admitted cell by cell by the same instrument, with a
whole-frame and a per-zone no-harm check. The grid-space axes are
normalised, so a field solved at 384 px is right at any size; the guide
kernel is not, and that gap was measured rather than argued: mean 0.173
codes, p99 1.0, worst 2.0, against the field's own effect of mean 8.9 and
p99 60. A 16×12×8 grid was tried and was worse (0.023750 against 0.021035 on
the analysis raster), because the regularisers were calibrated at 12×8×8.

## Zones ship as Lightroom's own masks (Select Sky, R35 §D1–§D2, R36 §4)

The two zones were bitmap masks the sidecar could not carry ("Lightroom XMP
will not carry: bitmap masks ×5"). The sky and ground zones now ride out as
Lightroom's own **Select Sky** component (`crs:MaskSubType="2"`, reference
point at the alpha-weighted centroid, the ground as the same component
inverted); Lightroom rebuilds the sky alpha itself, and the local render still
uses the same claimed PNG byte for byte. Two old defects closed with it: the
AI branch of the mask weight never read the inversion bit, and the writer
wrote geometric rather than net inversion, so an Invert ticked on a brush or
AI mask had never reached the sidecar. Inversion now has one net value
composed in one place.

The writer then learned the whole composition vocabulary. Linear, radial,
brush and AI components each write their own `rdf:li`; Add, Subtract and
Intersect share one spelling per family (`0/own/1`, `1/own/0`, `1/!own/0` for
blend / inverted / value), read back from a census of 174 sidecars, 399
corrections and 102 multi-member groups — 433 / 95 / 31 members per row,
with no user sidecar or photo name copied. Only bitmap components stay in
`ComponentsFlattened`. A recipe's inversion is projected through De Morgan
step by step; optional `ash` metadata keeps the enum, the inversion's home
and the zone role, and (R36) its prefix is resolved by XML namespace scope,
so a sidecar whose declarations an XMP toolkit hoisted to the document root
keeps its editor intent. What Lightroom itself renders for a written
intersection, and whether the `ash` attributes survive a Lightroom rewrite,
is not verified on this machine.

The spatial tiles the fit attaches are now four intersecting half-planes —
four Lightroom linear gradients, ramp `0.5/2048` — instead of bitmaps: on the
analysis raster the gradient matches the hard cell within one code per
pixel and its frozen evidence share within 1e-4. After guided refinement the
raster is kept, and a native candidate is solved from the same evidence; it
ships (R36) only when its own hard-cell residual and the whole-frame error
are no worse than the raster's by more than 1e-6 — fidelity to the target
decides the carrier, not similarity to an intermediate. On the reference
pair v1.3.0 ships tile r3c3 native at 0.65 and refined rasters elsewhere,
each with its two residuals printed.

## Bands earned by the residual (R35 §D3, R36 §3, R37 §3)

Within a zone, eight horizontal quantile bands of the mask's own alpha mass
read the *signed* Lab residual (an unsigned ΔE cannot tell a warm demand from
a cold one of the same size). A two- or three-segment model earns a trial
only if it explains more than half the variance with adjacent means more
than 2 ΔLab apart; every qualifying model then walks the render gates — the
same pairing scale, cell admission and boundary budget as a zone — and the
set must also improve ΔE by more than 0.02 without harming the region or the
frame. The overlap ladder is geometric (base × {1, 2, 4, 8, 16}, capped and
deduplicated) with three models per region at most, which took the 0.85
run from 17 min 05 s to 3 min 41 s while reaching the same verdicts.

Until R37 every sky band trial was refused as a "boundary regression",
because the two target-referenced rulers ranked per-crossing p90s on a
target whose texture is re-synthesised — one crossing's target step *is*
noise. They now read each 12×8 cell's **mean** through one scan, a cell may
regress up to its family's own seam ceiling (0.012; chroma up to one
just-noticeable difference, 2.3 ΔE\*ab), and the three components' mean over
the occupied cells may drift at most one code, so a drift every cell can
hide still cannot add up. The reference sky then earns **two bands at every
strength** (analysis-scale ΔE 30.37 → 27.43 at 0.65, 30.12 → 27.09 at 0.85,
29.99 → 26.99 at 1.0, before the colour field); the land's trials are still
refused by the band estimator itself.

## Seams (the ruler lane, R37, R38)

Three rounds on the same horizon, each a root cause rather than a constant:

1. **The two boundary families now share one budget** (the ruler lane before
   R33). The soft transition-band ruler "declined to charge" and let a visible
   seam through the featureless haze between two mesas at exactly its
   ceiling; the hard cross-boundary ruler gave the same haze one code. Both
   now use one budget — the scene's own change across the transition, or
   three times the frozen candidate's slope, clamped to [1/255, 0.012] — and
   both read a second coordinate, colour: per-channel steps with the channel's
   own context, gated on the larger of luma and colour. A feather that sits
   over smooth haze is widened before any ruler reads it, from the guide's
   own smoothness; hard rasters are never widened. R34 made the smoothness
   verdict scale-free (it had been calibrated on the in-camera preview and
   read 0.0 % smooth on the neutral develop), so the widening triggers again.
2. **A step the target itself carries is not a seam** (R37). Both families
   take the paired target under the analysis geometry and charge each
   crossing only for the part of its step the target does not carry: same
   sign and within the target's step is free, the excess is charged, the
   opposite sign is charged in full. The target's own step is read per
   evidence cell as a mean share of the frozen candidate — quorum 8
   crossings, one-code floor, two standard errors — so texture cannot rank
   as a seam. The reference sky's gate share rose from 0.105 to **0.146** at
   0.85, and the pass sentences now print what the target asked and what the
   unasked part was charged.
3. **The tile gate reads the frame the preview paints** (R38). The native
   tile trial, the zone regression check and the band rulers built their
   contours in stored coordinates and then judged the preview's pixels, which
   evaluate parametric geometry through the lens profile. Under the reference
   pair's profile the preview painted tile r1c0's right edge 3–4 analysis
   pixels off its contour; the ruler's feet straddled two lifted pixels, a
   27-code seam read 5 codes, and the tile shipped as a +10-code rectangle in
   the sky. `render::preview_mask_coverage` builds the contour in the frame
   the preview develops in; all three sites read it. The rectangle is gone:
   the step across that edge in the 2048 px output went from **+9.99 to
   +0.38 codes** at 0.85 (+0.49 at 0.65, +0.35 at 1.0).

## What the fit tells you

- Typed disclosure notes are capped at **512** (was 64) and the rationale
  string at 512 KiB, because a zoned fit on the reference pair writes more
  than a hundred notes and the old cap truncated them, which made the Chinese
  panel fall back to English for exactly the richest fits. A pin derives the
  cap from the longest template in the source.
- A `--deep` re-score of a zoned fit keeps every local producer's notes
  (zones, native ranges, tiles, free masks, boundary gates, guided
  refinement, the local field). The carry rule became one deny-list of
  global keys pinned against the rationale's own text, instead of an
  allow-list five producer families had already forgotten to join.
- The summary is followed by the pairing scale and by "mode decided by a
  margin of {margin}" when the two solvers were within 0.05.

## Desktop

- **The AI panel is four folds that say what they cost**: Analysis (paid
  API), Reference libraries (local), Reimagine (paid API), Reverse-fit
  (local; the AI review is paid). The reference libraries are a three-rung
  ladder — your Lightroom edits, the retrieval engine, the finished-photo
  look library — and the panel now shows the dependencies it always had: at
  Style 0 % the read switches are greyed while building stays live, and a
  ticked look library with SigLIP 2 off is labelled unreachable.
- **Deep thinking draws in its own box** under the rationale — bordered,
  132 px, scrolling — instead of a wall of italic grey inside the summary.
- **One button vocabulary** (`bin/gui/buttons.rs`): primary / action / glyph
  square / toggle. Every button in a row is one row tall, glyph buttons are
  square, rows of equal verbs lay out on equal columns, the toolbar wraps only
  between measured groups, glyph prefixes stay only where the same glyph
  means the same thing twice, long labels are shorter ("Extract style",
  "Heal area", "Soften"), the mask row is split into verbs and switches, and
  the slider rail takes what its row leaves. One pin renders every panel in
  both languages at its widest state and holds every button one row tall
  with both side panels at their default width.

## What was measured

Reference pair, `match --zoned`, isolated store, 2048 px acceptance render.
The baseline column is the first tree that read one source frame (the R33
merge); v1.2.6 itself is the anchor in the opening paragraph. The seam
readings are v1.3.0's; the pre-R37 rulers are retired in the ledger with
their last readings kept.

| quantity | 0.65 (default) | 0.85 | 1.0 |
|---|---|---|---|
| whole-frame mean \|diff\| | 0.0818 → **0.0714** | 0.0504 → **0.0276** | 0.0494 → **0.0280** |
| sky ΔE | 34.0 → **27.4** | 18.2 → **4.9** | 17.6 → **5.0** |
| sky dL | −9.1 → −9.3 | −4.9 → **−0.6** | −4.1 → **−0.6** |
| sky L\*std / target | 0.79 → 0.78 | 0.73 → **1.03** | 0.74 → **1.04** |
| land ΔE | 8.7 → 8.6 | 7.0 → 6.9 | 7.1 → 7.0 |
| land dL | +0.5 → +0.5 | +0.6 → +0.8 | +0.6 → +0.8 |
| gap to the target's horizon step, p50 / p90 (codes) | 3.7 / 7.4 | 2.8 / 6.0 | 2.7 / 5.7 |
| unasked introduced step, p90 (codes) | 1.5 | 2.5 | 2.5 |
| horizon chroma-contrast difference, p90 (ΔE\*ab) | 5.4 | 7.4 | 7.6 |
| `match --zoned` wall time | 4 min 59 s | 5 min 29 s | 4 min 32 s |

At 0.85 the sky's three acceptance lines hold (ΔE ≤ 5, |dL| ≤ 2, L\*std
0.95–1.05) and so do the land's (ΔE ≤ 7, |dL| ≤ 1). At the default strength
the colour field is inert by design and the sky stays 27 ΔE from a target
whose sky contrast no develop of the neutral RAW reaches (L\*std 0.78): the
default is the honest budget, not the closest fit. The wall time is longer
than the 1 min 35 s the R34 tree took, because of the fifteen band trials
per region and the share search; it is a third of R35's 17 min.

The seam lines were re-read column by column before release: the worst
colour-contrast columns all sit on the far mesa tops against the sky, where
the target keeps the warm haze on the land side (a\* 24.5) and the render
reads 18. That is a haze-colour gap along a real edge, inside the land's
ΔE 6.9, not a line the frame does not have; closing it needs a haze band
straddling the horizon, and the land-side band trials are refused at every
strength. It is disclosed as the known gap, not measured away.

## Gates

Release battery on the merge tree (its own target directory, BelowNormal):
library **1471 passed / 0 failed / 15 ignored** (1486 enumerated, 341.08 s),
CLI **24**, contract **2 + 2**, doc-tests 0, GUI **172 passed / 0 failed /
1 ignored**; `clippy --release --all-targets` 0 warnings on the default
feature set and 0 with `--features gui`; `audit_i18n` 0 findings;
`subset_gui_fonts --check` 868/868 embedded; `check_docs.py --gates`
**28 PASS / 0 FAIL / 2 SKIP**. By-name test-set difference against the
v1.2.6 tag: **+103 / −8**, the eight being renames re-pinned under names that
state the new rule (three in R34, four in R35, one in R36); no test was
deleted and no numeric limit loosened. The library battery now compiles its
tests at `opt-level = 2` with assertions and overflow checks kept on (a probe
crate proves both), which took the unoptimised 4014 s run to 325 s; the
release lane was already `--release`.

**The calibration lane did not run, and is not claimed**, for the same reason
as v1.2.5 and v1.2.6: its p36–p39 fit corpus was deleted in the 2026-09-03
clean-up and `scripts/release_battery.sh` exits rather than let corpus-gated
tests skip and pass. Unlike those two releases, this one changes the fit
those tests cover, so the gap is real: the corpus-gated pins (R33's
`fit_source` geometry pin on real material among them) are unverified, and
the reference pair above is the one real pair that was measured, at three
strengths. The Python sidecars are unchanged since v1.2.6 (`git diff --stat
v1.2.6 -- python/` is empty). Not verified on this machine:
Lightroom's own rendering of the written intersections and whether the `ash`
attributes survive a Lightroom rewrite; the sidecars for the reference pair
at all three strengths are kept for that check. No GUI executable was
launched.
