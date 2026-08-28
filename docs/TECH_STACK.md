# Tech stack and algorithms

Autoshop's core is a Rust image engine with a deterministic `f32` processing
pipeline. AI is kept outside the pixel math: it proposes bounded, serializable
recipes that the same renderer, CLI, desktop GUI, and local web UI can inspect,
revise, and replay. This page records the implemented methods, where their
parameters came from, measured results, and the limits that remain.

Provenance labels used below are deliberate:

- **Measured** means fitted or checked against controlled Lightroom/Camera Raw
  exports, the project's camera corpus, or a named runtime probe.
- **Designed** means an Autoshop engineering choice or a standard numerical
  method, not a claim about Adobe's implementation.
- **Calibrated** means a designed family whose constants were selected by an
  explicit held-out measurement.
- **Approximate** marks a useful path that is not claimed to reproduce a vendor
  algorithm.

## RAW decode and CFA

### Method

`decode_any` dispatches 24 camera-RAW extensions to rawler and eight baked
raster extensions to `image`; embedded ICC profiles on baked images are
converted with qcms. Bayer data uses rawler's normal demosaic path, while a
non-2×2 three-colour CFA takes Autoshop's geometry-driven X-Trans path: for
each missing colour at a pixel it fits a plane to matching photosites in a
5×5 neighbourhood, then evaluates that plane at the target while retaining
the sensor's measured channel exactly. `orient_f32` applies EXIF orientation
immediately after demosaic, before development, masks, straighten, or crop.

If a RAW has no usable embedded preview, the decoder builds a neutral rendition
from the sensor mosaic instead of failing the open. Parser panics are contained
at the file boundary, and decoder errors distinguish unknown make, unknown
model, no matching decoder, and recognized-but-corrupt input. Monochrome and
four-colour arrays are refused before colour development because silently
coercing either to RGB would invent a colour model.

### Parameters

- rawler version: `0.7.2` (**designed dependency pin**).
- RAW extension set: 24 entries in `RAW_EXTS` (**source-derived**).
- rawler camera database: 725 models (**source-derived at release**).
- X-Trans support window: `CFA_TAP_RADIUS = 2`, therefore 5×5
  (**designed geometry**, validated by synthetic planes and steps).
- X-Trans regression variables: sensor-space `x`, `y`, and an intercept,
  fitted independently for each missing channel and CFA phase
  (**designed**).
- Baked untagged 16-bit input: assume sRGB and emit a disclosure
  (**designed fallback**, explicitly not a colour-space measurement).
- RAW memory admission: refuse an estimated decode above 4 GiB, using the
  measured 31 B/pixel envelope, or 138,547,333 pixels (**measured gate**).

### Measured results & disclosures

- On the Fujifilm X-S10 full frame used to close the X-Trans defect, the green
  to red ratio changed from `1.5503` to `0.9476`; green to blue changed from
  `2.0839` to `1.0313`. The camera preview's green/red ratio was about `0.95`.
- The earlier rawler X-Trans result left red unwritten at 8 of 36 CFA positions
  and blue at a different 8 of 36. The plane-fit path closes those holes.
- The X-Trans implementation remains **approximate**: it is a local geometric
  interpolation, not a directional Markesteijn-class demosaic, and it can lose
  fine directional detail even though flat and linear synthetic fields close.
- Reference fixtures for 12 of the 24 RAW extension families carry no embedded
  preview. They exercise the neutral no-preview fallback rather than being
  advertised as preview-capable.
- An untagged 16-bit TIFF/PNG may actually be in an editor working space; the
  sRGB assumption is disclosed because the file contains no tag that can make
  the choice authoritative.

### Source

- `src/decode.rs` — extension dispatch, camera count, fallback rendition,
  decoder-failure classification, parser-panic containment, and RAW gate.
- `src/render.rs` — CFA geometry fit, Bayer/X-Trans selection, `orient_f32`,
  and the oriented full-resolution buffer.
- `docs/ARCHITECTURE.md` — release decode matrix and refusal policy.
- `docs/ROADMAP-archive.md` — X-S10 before/after measurements.

## Develop pipeline and tone model

### Method

The shared renderer stores pixels as deterministic `f32` RGB and explicitly
decodes to linear light around operations that require radiometric arithmetic;
the ordinary working buffer itself is sRGB-gamma RGB, so this is not described
as a wholly linear pipeline. After orientation and optional SCUNet denoise,
white balance runs before the composed profile/manual vignette and dehaze;
dehaze inverts the airlight model `I = J·t + A(1−t)`. The tone LUT then combines
exposure, contrast, whites, blacks, Highlights, shadows, base curve, and the
master point curve before RGB point curves, eight-band HSL, colour grading,
clarity, Texture, saturation/vibrance, noise reduction, sharpening, and local
adjustments.

`build_tone_lut` samples an eight-knot monotone cubic Hermite curve. Its
`tone_knot_weights` derive each control's influence from adjacent exposure-curve
gaps, while Fritsch–Carlson slope limiting prevents overshoot and preserves a
monotone mapping with a pinned white point; Highlights is part of this LUT,
not a separately claimed highlight-reconstruction algorithm.

Negative Texture is not a single blur. Two low-pass arms run in parallel and
are mixed back against the original luma: a coarse arm sets the broad plateau,
a fine arm restores the measured narrow edge structure, and the slider depth
uses `w(t) = t(1+d)/(1+d·t)` for `t = −texture/100`; noise reduction remains
before sharpening so the latter does not amplify noise that the former was
supposed to remove.

### Parameters

- Tone abscissae: `[0, .10, .25, .50, .66, .82, .92, 1]`
  (**designed knot layout**).
- Tone interpolation: Fritsch–Carlson monotone cubic Hermite
  (**standard designed algorithm**); `tone_knot_weights` replaced uniform
  slider weights after the old curve could flatten or invert intervals.
- White balance: gains are generated from Kelvin/tint and normalized so green
  is `1`; cool-side coefficients and the 6688 K red plateau were
  **calibrated against measured Lightroom exports**.
- Dehaze: `A` is the P99 of the minimum RGB channel in linear light,
  `t = max(1 − 0.75·s·min(R,G,B)/A, 0.30)` for positive dehaze
  (**designed atmospheric approximation**, not an Adobe fit).
- Clarity radius: `max(round(0.02·min(width,height)), 8)`
  (**designed resolution scaling**).
- Negative Texture arm 1: amplitude `A1 = 0.172443`,
  `sigma1 = 0.0031235·short_edge` = `0.31235%` of the short edge
  (**measured Lightroom fit**).
- Negative Texture arm 2: amplitude `A2 = 0.304888`,
  `sigma2 = 0.0002822·short_edge` = `0.02822%` of the short edge
  (**measured Lightroom fit**).
- Negative Texture depth: `d = 0.558583` in the hyperbolic slider law
  (**calibrated against the −50/−100 depth ratio**).
- Sharpening sigma: `clamp(0.0008·short_edge, 0.7, 2.0)` pixels, implemented
  with three box passes (**designed**, not Lightroom-measured).
- Luma noise reduction: a separable blur plus edge range weight, with
  `sigma = 0.5 + 1.5·t` and `range = 0.05` (**designed approximation**).

### Measured results & disclosures

- Negative Texture is pinned by 45 Lightroom anchors: nine spatial periods at
  five slider depths. Every anchor is within `±0.02`; the recorded worst error
  is `0.0037`.
- On a 4160-pixel short edge the two fitted sigmas are about `12.99 px` and
  `1.174 px`; at −100 the combined impulse plateau is
  `1 − (A1 + A2) = 0.522669`.
- A one-arm band-limited/notch explanation was refuted. The landed two-arm
  operator is empirical, and Lightroom's amplitude-adaptive behaviour is not
  modeled beyond the measured grid.
- The recalibrated Kelvin seam reduced the measured 6590/6610 K channel jump
  from red `−0.67%` and blue `−1.11%` to at most `0.10%`.
- The cool-end calibration also records a 7000 K shift of red `+1.47%` and blue
  `+0.97%`; the 2000 K blue result is `−0.98%`. These measurements constrain
  the implemented fit, not a claim of universal camera-colour equivalence.
- Historical tone probes are preserved because they explain the knot weighting:
  Whites −50 compressed input `0.9568…0.9731` from 411 codes to 75, while
  Highlights +60 clipped above `0.8195`, affecting 740 of 4096 inputs (18%).

### Source

- `src/render.rs` — pipeline order, transfer LUTs, dehaze, tone LUT,
  `tone_knot_weights`, Texture arms, clarity, NR, and sharpening.
- `src/recipe.rs` — bounded global and local adjustment domains.
- `docs/V2_PLAN.md` — white-balance, tone, detail-model calibration ledger.
- `docs/ROADMAP-archive.md` — negative-Texture model selection and 45-anchor
  acceptance record.

## Masks

### Method

Radial feather samples a measured `alpha(rho, feather)` table rather than
pretending Lightroom's profile is a closed form. Rows are interpolated in
ellipse-normalized radius `rho`, columns are interpolated in Feather, and
Feather 0 is an analytic hard edge so the family converges correctly as
`f -> 0`. Brush dabs use the hardness-dependent kernel
`K(rho,h) = (1 − rho^m(h))^n(h)`, multiply it by a measured per-dab flow law,
and combine repeated dabs with screen accumulation `a <- 1 − (1−a)(1−dab)`.

Linear gradients project the pixel-centre sample onto the Zero→Full handle
direction using the pixel/aspect metric:
`t = clamp(dot(p−zero, dir) / dot(dir,dir), 0, 1)`. Every mask family samples
at `(x+0.5, y+0.5)` through `MASK_SAMPLE_CENTRE`, then optional luminance- or
colour-range weights refine the geometry; components compose in document order
as Add, Subtract, or Intersect rather than being flattened to one union.

Lightroom brush strokes may arrive indirectly through a sibling
`MaskBrushTable`; the importer resolves the MD5-addressed `.acr` member,
validates its envelope and bounds, Brotli-decompresses it, then parses the
`r/f/h/d` state stream without interpolating extra points.

### Parameters

- Radial table: 290 radius rows at `rho = 0.0025 + 0.005·i` and 11 measured
  Feather columns `1/5/10/15/25/35/50/65/75/90/100`
  (**measured against Lightroom exports**).
- Feather 0: exact `alpha = 1` for `rho <= 1`, else `0`
  (**designed analytic limit**, because the measured edge contains JPEG and
  capture-sharpening blur).
- Outer support: current evidence settles `d_out = sqrt(2)`
  (**measured by four shape-free instruments**), but the LUT deliberately does
  not hard-code an endpoint; the tail samples carry the measurement.
- Brush shape: cubic polynomials in hardness for `ln(m)` and `ln(n)`
  (**measured pooled fit**).
- Brush flow: `D(f) = kappa·f / (1−f+kappa·f)`, `kappa = 0.1284`, normalized so
  `D(1)=1` (**measured**).
- Brush Size: Lightroom's UI Size is the diameter at the `alpha≈0.30`
  landmark; the fitted landmark is `0.2974`, and stored `Radius` is derived
  from that mapping (**measured**).
- Brush dab spacing: Lightroom's recorded stream is already densified at
  `0.2000·radius`, so Autoshop adds no synthetic interpolation (**measured**).
- Coordinate sampling: `MASK_SAMPLE_CENTRE = 0.5` (**measured convention**).

### Measured results & disclosures

- Three successive radial closed forms were refuted. No two-parameter family
  reached the `0.003` measurement floor, so the table is the model.
- The shipped 290×11 table scores radial `RMS(alpha) = 0.0009` and maximum
  deviation `0.0031` on a held-out aspect-1.2 export; its alpha=0.5 contour is
  `+0.04 px` from the reference.
- An earlier estimate, `d_out≈1.4335`, was traced to JPEG 8×8 block spill and
  is **not current**. Four later shape-free instruments support `sqrt(2)`;
  this correction is why no 1.4335 constant appears in the renderer.
- Changing linear-gradient distance from normalized coordinates to the
  pixel/aspect metric reduced the D1 reference error from `874 px` to
  `9.8 px`. That is a large improvement, not pixel closure.
- The brush kernel fit has pooled RMS `0.0102` and held-out RMS `0.0109`.
  `kappa` measured `0.1284 ± 0.0029` across a three-fold radius/hardness span.
- The Size coefficient varied only `0.20%`; derived Radius predicted the
  286–731 px reference strokes with `1.4 px RMS`.
- `MaskBrushTable` is validated before decompression and parsing. Unsupported
  payloads are refused and disclosed rather than partially drawn.

### Source

- `src/render.rs` — `radial_falloff`, linear projection, brush kernel,
  accumulation, range weights, and `MASK_SAMPLE_CENTRE`.
- `src/recipe.rs` — mask geometry, component operations, brush state, and local
  adjustment schema.
- `src/xmp.rs` — `MaskBrushTable` discovery, MD5 lookup, `.acr` envelope,
  Brotli decode, and dab-token parser.
- `docs/ARCHITECTURE.md` and `docs/V2_PLAN.md` — mask measurement ledger.

## Reverse-fit luminance ranges

### Method

When semantic segmentation is disabled or unavailable, the zoned reverse-fit
keeps its global-first ordering and derives contiguous signed-residual runs
from the existing 17-bin luminance evidence model. Bands are attempted once in
ascending luminance order through `attach_one_zone`; robust paired weights,
evidence withholding, share and mismatch checks, step-7b correspondence,
local-quality gates, and a parameterized composed-frame gate are shared with
semantic zones. Range bands use zero regression tolerance; semantic zones keep
their independently calibrated `0.02` drift insurance.
Before each attempt, `render::range_weight` is evaluated on the current render.
Overlapping estimator weights are normalized to a total no greater than one,
while each correction keeps the raw, pre-normalization ramp of its own
`RangeMask` as the coverage it actually moves. One final value-transition gate
shrinks every retained differential by the same direction-preserving bisection
scalar.

Native masks use `MaskRole::Custom`, deterministic `Luminance range NN` names,
and an intersecting `RangeMask::Luminance` on the observed-domain full-frame
sentinel `Linear { zero_x: 0.5, zero_y: -0.8, full_x: 0.5, full_y: -0.4 }`.
That is the existing Lightroom component grammar, so no recipe era or XMP
reader/writer branch changes. Segmentation and range production are mutually
exclusive, and this batch emits no color partitions.

### Parameters and measurements

- `RANGE_MAX_BANDS = 4`; finer value partitions fall below the established
  evidence stability floor.
- `RANGE_RESIDUAL_TRIGGER = 0.03`; corrected target-rank/source-bin means put
  supported neutral bins 01-07 and 12 at no more than `0.025`, the coherent
  08-11 run at `0.036`-`0.094`, and isolated bin 13 at `0.223`.
- `RANGE_MIN_RAMP = 1/17` and `RANGE_MAX_RAMP = 2/17`; a hard opposite-half-EV
  transition measured `5/255` versus a `1/255` smooth-gradient baseline.
- `RANGE_BOUNDARY_RIM_MAX = 0.012`, shared with semantic zones, and
  `RANGE_MIN_EVIDENCE_SHARE = 0.015`, shared with the global evidence model.
- `RANGE_FRAME_REGRESSION_TOL = 0.0`; the live `0.018 -> 0.024` composed-frame
  regression is refused, while an exactly neutral band remains acceptable.
- A naive second global residual fit improved its own tone score but regressed
  composed RGB MAE from `0.074702` to `0.076455`; current-render weighting,
  one correction per band, strict running frame acceptance, and the final
  stack gate prevent that double-application pattern.

### Source

- `src/fit.rs` — fixed 17-bin evidence verdict and contiguous-run folding.
- `src/fit_zoned.rs` — residual runs, generalized weighted attachment, range
  boundary gate, disclosures, and conservation tests.
- `src/render.rs` — sequential range evaluation on current rendered pixels.
- `src/xmp.rs` — intersected native luminance-range projection.

## Zone-scoped evidence view

### Method

Range verdicts follow the population a correction moves.
`EvidenceModel::scoped(tp, source_zone, target_zone)` re-aggregates the 17
luma bins and 8 hue bands over one zone's soft memberships from the model's
per-pixel ingredients (spatial-cell confidence, cell divergence, the frame
same-content verdict): target luma bins are rank-paired within the zone's own
target members at the source:target mass ratio, shares are taken over the
zone's weighted member counts, and the population line, the
structural-survival gate and the divergence fold are unchanged. Over all-ones
memberships the scoped view is the frame model byte for byte.

Both analysis rasters share one geometry: `fit::analysis_pair` thumbnails the
source to the `ANALYZE_EDGE = 384` long edge and thumbnails the target into
exactly that width and height with the same box operator
(`thumbnail_exact`, so the two rasters differ by no resampling kernel). Every
producer reads the pair through it. Independent thumbnails let a one-row
rounding difference (`1600x1067 -> 384x256` against `1600x1069 -> 384x257`)
make `structure_divergence` return `matched` on unequal lengths, which turned
the frame-wide same-content verdict on and disabled every range veto.

`attach_one_zone` asks the tone and colour vetoes of the view scoped over the
coverage the correction moves. An Atmosphere zone scopes the report ruler; a
Full zone scopes `structural_evidence` when an Atmosphere frame carries it, or
the report evidence in an ordinary Full frame. The composed-frame law stays on
the report ruler. This one branch serves semantic zones, luminance ranges and
tiles. A semantic mask uses its estimator weights as
coverage; a luminance-range attachment passes its own raw, pre-partition ramp;
a tile passes its raster, because its estimator weights are evidence-weighted
and would hide the withheld pixels the raster still moves.
`spatial::read_tile` derives a tile's per-pixel weights and shares from the view
scoped over its geometry. The blind-move audit's region line is a share of the
scoped population (`EvidenceModel::population`), not of the frame. Range
discovery (`derive_luminance_bands`) and composed-frame arbitration use the
frame model; each range attachment's movement vetoes use that band's raw-ramp
coverage. With colour withheld, the skip line is asked of the luma-only
residual, the same quantity the zone is then accepted on.

### Parameters and measurements

- `EVIDENCE_RANGE_SURVIVAL_MIN = 1 - DIVERGENCE_ZONE = 0.35` and
  `EVIDENCE_MIN_SHARE = 0.015` are unchanged and now apply per population.
- `ROT_SHARE = 0.05` is unchanged; a depth-2 tile is 6.25% of the frame, so
  under a frame share even a fully withheld tile barely reached the line and a
  half-withheld one never did.
- Calibration pair, semantic path: the land zone's tone controls were withheld
  through the replaced sky's luma bins (`luma[0.18-0.41]`); scoped, no veto
  fires. Its luma-only residual `0.0043` sits under `ZONE_SKIP_ERR = 0.012`
  while its full residual is `0.0456` (chroma withheld by the one-sided Blue
  band), so it is declared already matched instead of taking `+0.10 EV` for a
  hairline gain that regressed the frame `0.0179 -> 0.0193`.
- Live A/B, step-9 executable vs B1 (user-ratified 2026-08-27). GUI path
  (neutral development): old `sky -0.08 EV, land +0.08 EV (own residual
  0.041 -> 0.045), tile r2c0 -0.24 EV gains [1.30, 0.86, 0.79]` at `0.0175`;
  B1 `sky -0.08 EV` only at `0.0180` (land matched in tone at `0.002`; r2c0
  then fails the zero-drift frame law). RAW CLI path (range fallback): the
  band `[0.118, 0.294]` stays tone-withheld on its own population; r2c0
  attaches at `-0.99 EV` with colour withheld (Blue/Purple one-sided inside the
  tile), r3c0 at `-0.87 EV`: `0.0549 -> 0.0452 -> 0.0369` against the old
  `0.0549 -> 0.0427 -> 0.0345`. Wall time `73 s -> 110 s` (GUI path) and
  `149 s -> 168 s` (RAW): three more borderline tiles become eligible (shares
  are now the tile's own population) and are refused by the attachment share
  gate after refinement and robust reweighting.
- Shared analysis geometry + structure-blind Atmosphere ruler (2026-08-27).
  The GUI-path figures in the two bullets above were measured with the
  structural gate silently off (`384x256` against `384x257` thumbnails made
  `structure_divergence` answer `matched`); the RAW-path figures had it on.
  Live with `fit::analysis_pair` and the population ruler: neutral -> target
  `EV -1.00, 7100 K / tint +22.3, saturation 0, five-point curve`, ruler
  `0.189 -> 0.096`, confidence `0.25`; segmentation off attaches one tile at
  `-0.56 EV`, on the sky zone at `-0.08 EV`; RAW path `EV -1.00`, sky zone
  `-0.27 EV`, `0.194 -> 0.108`. Pixel ruler (`scripts/pixel_ruler.py`, mean
  CIE76 dE frame / sky / land): neutral `23.5 / 37.0 / 12.4`, gate-on under the
  old vetoes `22.5 / 37.0 / 10.7`, shipped gate-off `11.9 / 17.5 / 7.4`, ruled
  `10.6 / 16.4 / 5.9` (off) and `12.5 / 20.6 / 5.9` (on). p36 and viaduct (Full
  mode) byte-identical across the two executables; runs SHA-identical. Wall
  time `202 s` / `65 s` (segmentation on / off). `ANALYZE_EDGE` 512 / 768
  rejected: same tiles, `+4%` / `+25-50%` wall time, ruler collapse at 768.
- Synthetic poisoned-bin fixture (384x384): two thirds of the frame withheld,
  frame bin 6 populated at zero weight, the ground view keeps bins 5 and 6, the
  sky view withholds bin 6.

### Source

- `src/fit.rs` — `EvidenceModel::scoped`, `aggregate_ranges`, `population`.
- `src/fit_zoned.rs` — `ZoneAttachment.coverage`, scoped vetoes, luma-only
  skip line.
- `src/fit_zoned/spatial.rs` — tile readings and coverage.

## Local-field analyzer

### Method

The reverse-fit measures its own ceiling before it spends a local producer on
the pair. `LocalField::solve` is a Rust port of the validated NumPy experiment
`scripts/grid_experiment.py`: a `12 x 8 x 8` bilateral grid over
`(x, y, luma)` carrying five parameters per vertex
`[ev, gain_r, gain_g, gain_b, slope]`, trilinearly splatted over the analysis
rasters `fit::analysis_pair` already hands every producer. The modelled
display-referred delta is `dc = ln2*c*EV + c*gain_ch + (c - guide)*slope`, with
`guide` the 3-tap-smoothed `luma601` of the CURRENT render — the same pixels
the producers start from. The fit weight is the FROZEN evidence
`source_weights` times a local structural support term (`1 - D` from one
`fit::structure_divergence` reading per `12 x 8` cell) times an unclipped test
on both frames; the module never builds a second evidence model. The normal
equations are solved by conjugate gradient with f64 accumulators, Tikhonov
`lambda = 1` toward zero (= the global recipe) plus an x/y/b Laplacian `s = 1`,
at most 90 iterations or a squared relative residual of `1e-10`. The CG solve
and every reduction stay sequential; only the 96 independent cell readings run
in parallel and are scattered in cell order, so two solves of one input are
bit-identical (pinned by test). There is no per-image regulariser sweep: the
experiment's sweep picked `s = 0` on two of its five pairs to buy a last
`0.003` of the objective, and determinism plus no selection bias are worth
more. After the solve, vertices are clipped to `EV +-1.25 / gains +-0.35 /
slope +-0.5` and zeroed below an occupancy floor of `8`; saturation against
those bounds is COUNTED and disclosed, never widened away.

`ceiling` is `fit::look_err_with_evidence` of the field's rendered analysis
pixels against the target under the report's own evidence — the same
objective, the same weights and the same size as `report.err_after`, so the two
are directly comparable — and `global` is that objective on the unmodified
current render. `band_marginal[b]` is the occupancy-weighted mean of the five
parameters over luma bin `b`'s 96 `(x, y)` vertices, `band_dispersion[b]` their
occupancy-weighted spatial standard deviation converted to a luma effect in
/255 units at the bin's centre luma, and the remainder is the per-pixel luma
difference between the field's render and the band-marginal projection's,
carried together with the solve's per-pixel fit weight so every shape verdict
is taken on the pixels the field actually measured.

`fit_zoned::field` reads verdicts off that: band proposals for the luminance
range producer (spans of current-render luma that the producer maps onto its
own evidence bins through the pixels occupying them, refusing a span whose own
rank-paired residual disagrees with the field's sign), an effective attachment
cap for the quadtree, a `FieldShape` label, and the ceiling / realized / stop
disclosures the sequencer appends — the stop only when the ceiling actually
beat the producer-free frame. The
field is analysis-only — it never reaches `render::` or an `EditRecipe`, which
a test pins by grepping `src/render.rs` and `src/recipe.rs` for the module
name.

### Parameters and measurements

- Grid `12 x 8 x 8` x 5 parameters = 768 vertices. `TIKHONOV = 1.0`,
  `SMOOTH = (1, 1, 1)`, `ITERATIONS = 90`, `OCCUPANCY_MIN = 8`,
  `IDENTIFIABILITY_MIN = 1e-5`, bounds `EV +-1.25 / gains +-0.35 /
  slope +-0.5`.
- Verdict constants: `BAND_DISPERSION_MAX = 15/255`, `BAND_MERGE_STEP = 2/255`,
  `TILE_SHAPE_MIN = 0.5`, `LINEAR_SHAPE_MIN = 0.6`,
  `LOCAL_STOP_MARGIN = 0.002`. `BAND_DISPERSION_MAX` is a measured line: the
  phase-A fixture sweep (96x64 to 192x128, amplitudes 0.25 to 0.50, ramps and
  waves) reads a spatially UNIFORM two-band edit at at most `9.2/255` in sparse
  bins, spatially structured bins at `21.9-51.8/255`, and the calibration
  pair's re-rendered mid-tones at `28.7-29.1/255`; `15/255` sits in that gap.
- NumPy cross-check on the calibration pair, on the Rust module's OWN exported
  analysis pixels, guide and fit weight (NumPy 2.3.5 / Python 3.13.3,
  2026-08-27): NumPy ceiling `0.070022` against the Rust port's `0.0700223`,
  largest per-parameter disagreement over all 768 vertices `1.5e-5`. Re-run
  2026-08-28 on this tree: `max |grid_rust - grid_numpy| = 0.000015`, Rust
  `iterations 39`, `relative_residual 6.995e-6`, `ceiling 0.070022`,
  `global 0.096145`, `saturated 3`, `supported 200/768`. The two
  `#[ignore]`d probes that produce it are
  `export_calibration_field_inputs_for_numpy` and
  `compare_calibration_field_with_numpy`.
- Calibration pair live, `neutral.jpg -> target.jpg`, one release executable
  per arm (`match --zoned`, per-run store): `global 0.096145`,
  `ceiling 0.070022`, 3 saturated vertices, CG 39 iterations,
  `R2 tiles 0.394`, `R2 linear 0.050` (fit-weighted; the unweighted reading
  before the 2026-08-28 review fixes was `0.367` / `0.053`), verdict
  `free_form`, cap 2, structured bins
  `[0:blind, 3, 4]` at `29.14/255` and `28.72/255`. With segmentation on the
  sky zone takes the frame to `0.092652` = `0.134` realized; with segmentation
  off the luminance range takes it to `0.079947` = `0.620` realized. On the RAW
  arm (`source.arw -> target.jpg`, the embedded-preview path):
  `global 0.107764`, `ceiling 0.072299`, 5 saturated vertices, CG 42
  iterations, `R2 tiles 0.331`, `R2 linear 0.039` (unweighted before the
  review fixes: `0.350` / `0.027`), `free_form`, cap 2,
  structured bins `[0:blind, 3, 4]` at `28.66/255` and `30.46/255`; the sky
  zone realizes `0.280` (frame `0.097816`), and with segmentation off no
  producer is accepted, so the realized share is disclosed as `0.000` at the
  global `0.107764`.
- The analyzer moved no dial on any of those four arms: with the rationale
  dropped and the per-run store path normalised, the dials and `confidence`
  hash identically to the pre-batch executable's (`compare_recipes.py`), and
  the two seg-on runs differ only by that store path. The recipe JSON itself is
  not byte-identical — what changed inside it is the disclosure (`+544`
  rationale bytes on the semantic arm, `10331 -> 10875`, against a
  `MAX_RATIONALE` of 16 KiB) and the tile cap the quadtree prints (`4 -> 2`).
  The same four identities held on the executable rebuilt after the 2026-08-28
  adversarial-review fixes (sha256 `5cfb5e6c…`; no field proposal survives its
  gates on this pair, so the domain mapping has no live band to move here).
- The `ZONE_DROPPED` drift now prints at `{:+.5}`: the r2c0 tile both neutral
  arms drop reads `drift +0.00036 (tolerance +0.00000)` on the semantic arm and
  `+0.00035` on the range-fallback arm, where both used to read `+0.000
  (tolerance +0.000)` and looked like exactly-zero refusals.
- Pixel ruler (`scripts/pixel_ruler.py`, mean CIE76 dE frame / sky / land at
  384 px wide), `apply` renders of each match's own recipe and store: the
  untouched neutral scores `23.48 / 37.04 / 12.42`, the semantic arm
  `12.49 / 20.58 / 5.89` and the range-fallback arm `10.63 / 16.46 / 5.87` —
  the same three numbers before and after this batch, because the two renders
  are byte-identical (sha256 `7d58d379...` on and `7da778ca...` off).
- Wall time, same machine, one process at a time, pre-batch executable against
  this one: semantic neutral `168.6 s` against `263.5 / 218.7 s`,
  range-fallback neutral `104.6 s` against `90.9 / 110.7 s`, semantic RAW
  `224.0 s` against `196.9 s`, range-fallback RAW `130.6 s` against `97.8 s`.
  The new executable wins three of those four arms, so the run-to-run spread on
  this machine is wider than the analyzer's cost. Measured directly instead
  (`compare_calibration_field_with_numpy`, the second `#[ignore]`d probe):
  `LocalField::solve` is `1.343 s` on the calibration pair, of which
  `local_support` — the 96 `structure_divergence` cell readings — is `0.897 s`.
  Phase A measured the same two sequentially at `6.59 s` and `6.39 s`; running
  the independent cell readings under `rayon` and scattering them in cell order
  is bit-identical by construction and the determinism test still passes.
- `look_err_with_evidence`'s structural term is inert on this pair (its core is
  under 100 px, so `structure_divergence` answers `matched()`); the ceiling and
  every realized share inherit that. Registered elsewhere, not compensated for
  here.

### Source

- `src/fit_field.rs` — the solver, band marginals and dispersion, remainder,
  saturation count.
- `src/fit_zoned/field.rs` — shape verdicts, band proposals, `realized_share`,
  `stop_verdict`, the five disclosures.
- `src/fit_zoned.rs` — `fit_recipe_zoned_inner`, the sequencer that owns the
  realized and stop notes.
- `src/fit_zoned/range.rs` — the proposal union inside
  `derive_luminance_bands`.
- `src/fit_zoned/spatial.rs` — the parameterised attachment cap.

## Layered spatial reverse-fit and mask refinement

### Method

After the global fit and exactly one local producer (semantic zones or native
luminance ranges), `fit_zoned::spatial` traverses a quadtree over the current
render. Rectangle geometry intersects the evidence frozen from the original
pair. Nodes are visited best-first by absolute signed residual times the lesser
source/target share, with `(depth,row,col)` ties. Each accepted bitmap leaf is
re-fitted through the shared robust zone estimator, boundary gate and a
zero-regression composed-frame law; all readings are re-derived before another
leaf is considered.

`mask_refine` is a dependency-free guided-filter approximation built from
integral-image box means and deterministic row-parallel output. It can refine
semantic silhouettes and eligible tile collars before fitting. Conservation
restores the non-collar core byte for byte and gates coverage, finite values and
transition-weighted Sobel guide-edge alignment. It never receives a luminance
range ramp. Failed refinement retains the original alpha and records the
reading.

### Parameters and measurements

- `SPATIAL_MAX_DEPTH = 2` (4x4 leaves) and `SPATIAL_MAX_ATTACHMENTS = 4`, the
  latter now the DEFAULT cap: `attach_tiles` takes the effective cap as a
  parameter and the local-field analyzer passes 2 when `R2(4x4) < 0.5`.
- Both frozen evidence shares must be at least `0.03`; original `D` must be
  below `0.65`; the weighted 95% interval must exclude zero and the child must
  differ from its parent by at least `2/255`.
- `SPATIAL_FRAME_REGRESSION_TOL = 0.0` and the shared boundary rim ceiling is
  `0.012`.
- Tile rasters use normalized source coordinates with a 2048-pixel long-edge
  cap. JSON is lossless; classic XMP reports the existing named bitmap loss.
- Guided refinement uses radius `8`, epsilon `(4/255)^2`, a `2 * radius`
  restored collar boundary, and maximum coverage drift `0.002`.
- The shipped automatic path on the calibration pair (range fallback)
  attaches `r2c0` then `r3c0`: frozen shares `3.9%/3.9%` and `3.6%/3.6%`,
  original `D = 0.303` and `0.438`, signed residuals `-0.094` and `-0.060`
  (95% CI `0.0010`/`0.0006`), composed frame `0.0549 -> 0.0452 -> 0.0369`
  since the zone-scoped evidence view (r2c0's colour is withheld; the
  pre-view figures were `0.0549 -> 0.0427 -> 0.0345`); every changed-sky node
  abstains on source share (gate-on record under the old vetoes; since the
  structure-blind Atmosphere ruler the RAW path's global fit carries the frame
  at `0.194 -> 0.108` with `EV -1.00` and no tile reaches the attachment
  gate). The semantic-mask refinement
  probe measured guide-edge alignment `0.046444 -> 0.023914`, so it correctly
  abstained and retained the original bytes.
- Ineligible traversal verdicts aggregate into one typed sweep note per
  generation; the persisted-rationale abuse bound is `16 KiB` (raised from
  `4096`, which the pre-tile range path already saturated and which truncated
  the tile attachment disclosure off the persisted recipe).
- The 384x256 release probe measured 1.72 ms; scaling by pixel count estimates
  roughly 49 ms for a 2048x1365 mask, not a second benchmark.

### Source

- `src/fit_zoned/spatial.rs` — traversal, tile raster ownership, shared fitting,
  boundary/frame arbitration, rationale facts and conservation tests.
- `src/mask_refine.rs` — guided filter, collar restoration and acceptance laws.
- `src/store.rs` — atomic `OwnedRaster::claim_sibling` ownership.
- `src/xmp.rs` — existing bitmap-mask omission and named loss.

## AI masks

### Method

Subject selection runs a commit-pinned BiRefNet sidecar; if that backend is
unavailable, the named U²-Net implementation is the explicit fallback rather
than an unnamed heuristic. Sky selection uses OneFormer over ADE20K and maps
the model's 150 labels through the checked-in class table, so `sky` is selected
by semantic ID rather than by label-text guessing. Object gestures turn every
recorded brush `d` point into an ordered positive prompt for SAM 2.1 and send
the bounded point list over the `gp1` JSON IPC contract.

Computed alpha is cached by provenance, not just by photo path. The key binds
the photo identity, mask subtype, orientation, click/gesture data, backend
generation, and exact points sent; a fallback-produced subject alpha is
therefore invalidated when the pinned BiRefNet backend later becomes available.

### Parameters

- BiRefNet repository/revision:
  `ZhengPeng7/BiRefNet@e2bf8e4460fc8fa32bba5ea4d94b3233d367b0e4`
  (**designed reproducibility pin**), with SHA-256 pins for code, config,
  preprocessing README, and weights.
- BiRefNet input edge: `1024` pixels (**upstream/model configuration pin**).
- BiRefNet weights: `444,473,596 B` (**manifest-measured file size**).
- OneFormer ADE20K weights: `881,196,376 B`; checked-in class table:
  `7,085 B`, 150 classes (**manifest/source-derived**).
- SAM 2.1 weights: `897,897,416 B` (**manifest-measured file size**).
- Gesture mapping version: `gp1`; every `d x y` token becomes label `1`, with
  order and duplicates preserved (**designed IPC contract from measured XMP
  semantics**).
- AI backend generation: `2` in the cache key (**designed invalidation salt**).

### Measured results & disclosures

- BiRefNet's pinned 1024-pixel forward path measured `0.850 s` on the recorded
  local probe. That is a hardware-specific observation, not a performance SLA.
- The OneFormer table identifies ADE20K class `2` as sky; class `48` is
  skyscraper. Keeping the table in-tree prevents that name/number confusion.
- Subject, sky, and object alphas are local model outputs, not Adobe's computed
  masks. XMP preserves selection intent, but Autoshop re-derives proprietary
  alpha and says so.
- Fallback provenance remains visible: an alpha made by U²-Net is not silently
  relabeled as a BiRefNet result.

### Source

- `python/segment.py` — model manifests, digest pins, preprocessing, OneFormer,
  BiRefNet/U²-Net selection, and SAM invocation.
- `python/ade20k_class_table.json` — the 150-class semantic table.
- `src/segment.rs` — bounded sidecar process, `gp1` IPC, cache key, provenance,
  and backend generation.
- `src/recipe.rs` and `src/xmp.rs` — AI-mask intent and XMP round trip.

## Lens correction and Lightroom mask-frame laws

### Method

`lcp.rs` reads Adobe rectilinear `.lcp` perspective polynomials and numerically
inverts the radial mapping with guarded Newton iterations; a fold is rejected,
and fisheye-only entries are named refusals rather than forced through a
rectilinear approximation. Sony maker-note tag `0x7037` supplies 16 distortion
samples at native radii `(i+1)/16`. The image renderer uses its calibrated knot
convention, while the Lightroom mask solve first resamples the camera law onto
2048 canonical nodes and persists a bounded 64-knot inverse transport.

The radial mask solve is zero-parameter: its centre is
`raw_full_dims/2 − DefaultCropOrigin`, and on the A7R IVA fixture,
`(9600,6376)/2 − (32,20) = (4768,3168)`, radius is normalized by the stored
frame's half diagonal `RR = 0.5·sqrt(W^2+H^2)`, and the profile fill
`s_p = max g(r)` over the frame-edge radius band is the minimal zoom that keeps
all edge samples inside. A radial sample crosses the boundary exactly once as
`m_lr^-1 ∘ T_engine`: first map the engine's evaluation point to the relevant
stored/output frame, then invert Lightroom's stored-to-exported mask map.

Linear gradients obey the measured H2 topology rather than bending a line per
pixel: with correction ON, Zero and Full are corrected-frame handles and the
straight line is evaluated after `T_engine`; with correction OFF, both handles
are sent once through `D_fwd` and a new straight line is built in the raw pixel
metric, while brushes, bitmap masks, and AI alpha remain in the raw/pre-lens
frame.

### Parameters

- Sony `0x7037`: 16 native samples at radii `(i+1)/16`
  (**measured/adjudicated from camera metadata behaviour**).
- Mask solve: 2048-node canonical resample, persisted as 64 knots
  (**designed bounded representation**, accepted by measured closure).
- A7R IVA stored-frame centre: `(4768,3168)` from full dimensions
  `9600×6376` and `DefaultCropOrigin=(32,20)` (**measured fixture metadata**).
- Radial normalization: stored-frame half diagonal `RR`; profile fill `s_p` is
  the exact piecewise-linear maximum over the edge band (**designed numerical
  implementation of the measured camera law**).
- Radial composition: `m_lr^-1 o T_engine`, exact once and with no fitted mask
  scale (**measured zero-parameter topology**).
- Linear H2 ON: corrected-frame handles, straight after `T_engine`;
  H2 OFF: `D_fwd(Zero)` and `D_fwd(Full)`, then straight
  (**measured topology**).
- `LR_MASK_FRAME_SCALE = 1.0` (**measured decision**); the older `1.032`
  concentric-frame hypothesis was rejected.

### Measured results & disclosures

- Radial transport closes all `41/41` vectors to at most `1 px`.
  The first wall contributes `20/20` points at `0.568 px RMS`; the independent
  DSC set contributes `21/21` at `0.243 px RMS`.
- Linear H2 is explicitly **not 1 px-closed**. Correction ON gives
  `9.748 / 7.025 / 6.336 px RMS`; correction OFF gives
  `12.449 / 9.943 / 4.979 px RMS` on the three reference frames.
- The falsified H1 hypothesis predicted line sag
  `−22.8 / +24.4 / −10.8 px`; Lightroom measured
  `+0.6 / −2.9 / −0.5 px`. The sign is opposite and the magnitude is
  `8.5–41x` too large, which is why H1 is not retained as an alternative.
- The R2 large-mask dilation residual is about `1.2 percentage points` and
  remains an open item. It is not hidden inside a new fitted scale.
- `LensProfile.mask_warp_center` and `LensProfile.linear_handle_warp` are two
  intentional forward schema breaks: old binaries must refuse recipes carrying
  frame facts they cannot apply safely.
- Seventy-three fisheye-only LCP entries are refused; no rectilinear Newton
  result is claimed for them.

### Source

- `src/lcp.rs` — LCP parser, rectilinear polynomial, Newton inverse, fold and
  fisheye refusals.
- `src/lensmeta.rs` — Sony `0x7037`, native sample placement, and canonical
  resampling.
- `src/render.rs` — half-diagonal normalization, `s_p`, image geometry,
  `lr_mask_warp_norm`, inverse mask map, and H2 evaluation.
- `src/recipe.rs` and `src/xmp.rs` — stored centre, linear handle warp, schema
  gates, and Lightroom import/export.
- `docs/ROADMAP.md` — D1/D2 measurement verdicts and remaining R2 residual.

## XMP and Lightroom interoperability

### Method

The sidecar reader does not search a whole XML document for familiar local
names. `crs_own_scope` selects Camera Raw's own Description scope—including
the nested `Look` exception—and typed `Tag`/`Scope` wrappers make a whole-tree
read an explicit operation at the call site. On write, Autoshop replaces the
fields it owns and merges the edited tree back into the original document so
unknown namespaces, attributes, and unmodeled corrections survive.

Ordinary Save writes the projection into the per-user develop store and never
modifies the photographer's source library. “Export sidecar beside RAW” is a
separate explicit action; an adjacent Lightroom XMP can be read as the newer
merge base but is not overwritten by ordinary Save. Mask parsing carries
unsupported semantics with named disclosures and imports `MaskBrushTable`
through the strict binary path described above.

### Parameters

- Mask-frame scale: `LR_MASK_FRAME_SCALE = 1.0`
  (**measured Lightroom decision**, not a tuning knob).
- `LocalExposure2012`: file value `= EV/4`; importer multiplies by `4`
  (**measured scale**).
- `LocalHue`: file value `= degrees/180`; importer multiplies by `180`
  (**measured scale**).
- The `/100` local family is contrast, highlights, shadows, whites, blacks,
  clarity, dehaze, texture, saturation, temperature, tint, local sharpness,
  and luminance noise reduction (**measured file-to-UI scale; saturation and
  Texture also have real forward pairs**).
- Global `crs:Sharpness`: direct `1:1` slider amount; it is not multiplied by
  `1.5` (**measured from real sidecars**).
- Radial `Angle`: clockwise-positive on a y-down screen, rotated about the
  ellipse centre in pixel space (**measured**).
- `MaskInverted` controls inversion; `Flipped` is not treated as a second
  inversion bit (**measured polarity census**).

### Measured results & disclosures

- A controlled local Hue value of `+50` wrote `0.277778`; multiplying by 180
  gives `50.00004`.
- Ten hand-authored local file values from `±0.15…±0.75` read back at exactly
  `×100` in Lightroom, with eight zero-valued controls displayed as zero.
- Fifteen real sidecars carry global Sharpness up to `150` in the direct
  domain, refuting the earlier `×1.5` import assumption.
- Radial-angle probes measure `28.554 deg` in the pixel metric versus
  `19.692 deg` in normalized coordinates; choosing the latter can create as
  much as `11.2 deg` of rendered tilt over the observed range.
- `Flipped` and `MaskInverted` were perfectly anti-correlated in a 201/201
  census and in all 23/23 M-B cases. Import now takes polarity from
  `MaskInverted` alone; the recorded image RMS improved from `0.1099` to
  `0.0751`, and the blue-channel RMS from `0.1901` to `0.0869`.
- Conservative merge means “round trip” is structural preservation, not a
  promise that every Lightroom correction is rendered by Autoshop. Unsupported
  or carried-only fields remain disclosed.

### Source

- `src/xmp.rs` — scope selection, typed XML traversal, local/global scaling,
  polarity, mask geometry, conservative merge, and writer.
- `src/pipeline.rs` — sidecar read/merge/save/export orchestration.
- `src/store.rs` — per-user develop store and saved merge bases.
- `docs/ARCHITECTURE.md`, `docs/V2_PLAN.md`, and `docs/ROADMAP.md` — sidecar
  measurement and storage-semantics ledger.

## AI advisor and reverse fit

### Method

`analyze`/`auto` asks the configured vision role for a structured proposal,
validates it through the catalogue and schema, clamps it into a bounded
`EditRecipe`, and renders locally. The optional verifier is data-only—it sees
the structured proposal, not photo pixels—and all Responses requests set
`store:false`; a visual judge may buy one guided revision, but the replacement
is adopted only when its rescore is at least the original score.

Style retrieval indexes RAW+XMP exemplars, extracts normalized photographic
features, z-scores the varying dimensions, and optionally adds a SigLIP 2
cosine block. The distance is
`sum_i w_i(q_i−e_i)^2 + W_EMB(1−cos(q_emb,e_emb))`; missing embeddings or
`W_EMB=0` reproduce the feature-only ranking exactly.

`match` is inverse rendering rather than pixel copying: it aligns luminance
CDFs, searches exposure, solves a regularized basis of engine controls, adds a
residual monotone tone curve, closes saturation in a render/measure loop, and
fits channel-cast curves; `cast_paints_foreign_hues` vetoes a cast that creates
a visible colour family at least 45 degrees from every target family over at
least 5% of the frame; terminal do-no-harm can shrink saturation or reset to
the caller's base recipe.

Atmosphere mode is entered because structure diverges; its instruments are the
budgets (EV ±1, WB gain [0.80, 1.25], saturation ±30, curve slope [0.5, 1.5])
and the population facts, not structural survival. `EvidenceModel::structure_blind`
keeps one-sided, sparse and minimum-share population vetoes while switching off
structural survival and per-pixel withholding. One Atmosphere report uses that
one ruler for frame error, harm, joint readings, confidence and disclosure; it
carries the structural model separately only for Full zones and detail. A typed
population-evidence note names the structurally withheld luma/hue ranges and
states which downstream fits still read them. Full mode is unchanged.

`reimagine` uses the configured `gpt-image-2` Images edit path as an explicitly
lossy generated target, negotiates a flexible size and its supported fallback,
then can feed that target into `match` for an editable full-resolution
approximation; parameter downgrades occur only when structured error blame—or
the equivalent streamed-refusal wrapper—names that parameter, while `heal`
copies real neighbouring pixels, mean-corrects and feather-blends the patch,
and remains a deterministic pixel operation rather than XMP.

`correspond` (step 7a) is the measurement instrument for the content-divergent
case the reverse-fit's atmosphere mode guards: a DIFT featurizer (one SD 2.1
UNet pass per noise draw at `t=261` over 768² inputs, `up_blocks[1]` features,
an averaged 8-draw ensemble run one-at-a-time to bound VRAM) yields a 48×48
correspondence field between two renditions — per-cell target coordinates
whose confidence is cyclic consistency × local flow smoothness. Raw cosine is
exported for diagnostics but excluded from the confidence: it is
scale-dependent, and the smoothness term is what keeps a pixel-shuffle of the
same frame (the atmosphere-budget fixtures) honestly unmatchable. The sidecar
writes coordinates, never pixels; its absence degrades to today's behaviour.

Since step 7b the reverse-fit consults the sidecar itself — automatically,
on content-divergent pairs only (the D gate is single-sourced inside the
fit; a Full-mode pair never pays for a run). The projected field weights a
FULL zone's pixel pairs by per-cell confidence and reads shifted content at
its corresponded position; Atmosphere zones and every share gate keep their
pre-field semantics, an abstaining field falls back wholesale, and identity
and zero-confidence fields are conservation-tested to change nothing.

### Parameters

- Advisor output: catalogue-registered fields only, schema validation and
  per-field clamp before rendering (**designed safety boundary**).
- Responses persistence: `store:false` for proposer, verifier, and judge
  (**designed privacy control**).
- Revision gate: accept revised recipe only when `new_score >= old_score`
  (**designed do-no-harm rule**).
- Style features: 14 dimensions in the persisted normalization block, with
  only dimensions having useful variance z-scored (**designed representation**).
- SigLIP 2: 768-dimensional optional vectors; `W_EMB = 2.0`
  (**designed default retained after calibration**).
- Foreign-hue veto: distance `>=45 deg` and newly painted share `>=5%`
  (**calibrated against known cast failures and haze controls**).
- Reimagine flexible-size budget: dimensions are multiples of 16, aspect ratio
  at most `3:1`, and at most `8,294,400` pixels (**current gpt-image-2 contract
  encoded in the client**).
- Heal auto-detection: bounded to at most 30 spots; painted-mask heal uses the
  same deterministic patch compositor (**designed resource bound**).
- DIFT recipe: timestep 261, 768² bilinear input, `up_blocks[1]` (1280 ch at
  48×48), ensemble 8, seed 0 (**the paper's featurizer settings**).
- Correspondence confidence: `exp(−cyc²/2·1.5²) · exp(−dev²/2·2.0²)` in grid
  cells — cyclic round-trip distance × deviation from the 3×3 median flow;
  raw cosine excluded (**designed scale-free gate**).
- Correspondence pinning: 11 files sha256 + byte-capped at one 40-hex commit;
  `local_files_only` loads; fp16 on CUDA, fp32 on CPU (**provenance gate**).
- Estimator wiring (7b): consulted iff `D >= 0.35`; confidence composes only
  into FULL zone estimators; share gates never read it; abstain = wholesale
  fallback (**designed conservation rules, mutation-tested**).

### Measured results & disclosures

- SigLIP calibration covered `147/147` local 768-D embeddings and scanned
  `W_EMB = 0/0.5/1/2/4/8`. Overall settings MAE was nearly flat
  (`0.7368` at 0, `0.7351` best at 8; bootstrap interval
  `[−0.018,+0.019]`), while curve-habit rank improved `3.44 -> 2.98` at 8.
  The evidence did not justify retuning the designed default away from `2.0`.
- The foreign-hue gate is intentionally categorical: a cast that improves an
  aggregate RGB error can still be rejected if it paints a target-absent hue.
- A streamed `2048x1360` size refusal is attributed to `size` and can negotiate
  the supported `1536x1024` fallback; a generic upstream failure is not blamed
  on `stream` or `size` by substring.
- Generated images are regenerated pixels and are lossy targets, not RAW
  masters. Reverse fit approximates their look with the deterministic engine;
  it does not recover the generated pixels or guarantee exact equality.
- Heal changes pixels and is therefore stored in the pixel-source/version
  model, not misrepresented as a parametric Lightroom adjustment.
- Correspondence zero point: on an identity pair the field reads median
  confidence `1.000`, coverage `100.0%`, mean |flow| `0.00` cells. On the
  content-divergent calibration pair (generated sky), sky cells read median
  confidence `0.009` (coverage `21.5%`) against ground `1.000` (`90.5%`) —
  the field separates replaced content from preserved content.
- SD 2.1 provenance: the official `stabilityai` repo is delisted upstream
  (2026-08-26: anonymous 401, authenticated 404); the pinned community
  mirror's fp32 tower digests are byte-identical to an independent
  uploader's, and the sha256 gate is the only door at run time.
- Live zoned A/B on the calibration pair (field on vs unavailable): the recipes' dials are byte-identical (this pair's land-zone corrections are evidence-withheld either way, upstream of where the field composes; only the disclosure differs — field on carries the measured 59% / 0.80 line, field off the unavailable note). Zero regression on the flagship pair; the mechanism's gain is pinned at estimator level by the shift-recovery test (24 px shift, map error < 0.03).
  The correspondence disclosure line carries coverage and median confidence
  either way the run went.

### Source

- `src/advisor/mod.rs`, `src/advisor/openai.rs`, `src/advisor/claude.rs`, and
  `src/advisor/judge.rs` — proposal, negotiation, data-only verification, and
  revision gate.
- `src/style.rs` and `src/embed.rs` — feature index, z-scoring, SigLIP vectors,
  and hybrid distance.
- `src/fit.rs` and `src/fit_zoned.rs` — structural-divergence modes, bounded
  atmosphere fitting, CDF/basis tone solves, per-zone quality gates, cast
  vetoes, and do-no-harm.
- `src/generative.rs` — gpt-image-2 sizing, streamed refusal attribution,
  staged publication, reimagine, and generative fill.
- `src/retouch.rs` — deterministic heal.
- `src/correspond.rs` and `python/correspond.py` — DIFT correspondence
  field, digest pins, parse gates, and the `correspond` CLI diagnostic.
- `docs/V2_PLAN.md`, `docs/ARCHITECTURE.md`, and `docs/ROADMAP-archive.md` —
  advisor, style-calibration, reverse-fit, and generation ledger.

## Application and infrastructure

### Method

The library is the product boundary. `clap` exposes it as the CLI,
`eframe`/`egui` supplies the desktop application, and `tiny_http` serves an
embedded `include_str!` web UI with no runtime CDN or frontend build step.
The GUI models one source photo as variant cards (base pixels + one develop,
all saved by one `Ctrl+S`) and numbered versions (a snapshot of one card's
develop at one moment). Loading a version replaces the active card's canvas as
one undo step; `auto` versions are the backup gate's snapshots. Discarded
version identities are written to a deleted-versions registry so a later save
cannot silently reuse an identity that may still exist in an export or log.

The local server binds loopback and issues a fresh 32-byte capability token.
State-changing requests require `X-Autoshop-Token`; Host and Origin must name a
literal loopback authority on the actual bound port, API responses are
`no-store`, and framing is denied; request concurrency and per-photo work are
bounded instead of allowing browsers or batch jobs to multiply full-resolution
RAW memory without limit.

SCUNet denoise is an optional local sidecar with an output contract stronger
than process exit status: the caller accepts success only when the typed result
sets `sidecar_wrote` and the expected artifact is present, non-empty, and newer
than the pre-call state; model weights remain outside the repository.

### Parameters

- Core toolchain: Rust edition 2024, rustc/cargo `1.94`
  (**release toolchain pin**).
- Desktop UI: `eframe`/`egui 0.29` (**dependency pin**).
- HTTP implementation: `tiny_http`; embedded HTML/CSS/JS via `include_str!`
  (**designed self-contained UI**).
- Web capability: 32 random bytes, URL-safe base64 without padding
  (**designed per-run secret**).
- Server request concurrency: 8 (**designed single-user bound**).
- RAW admission: 4 GiB at 31 B/pixel, or 138,547,333 pixels
  (**measured memory bound**).
- Batch per-photo budget: 1,800 MB (**rounded designed budget from a measured
  1,771 MB high-water mark**).
- Style index: 5,000 exemplars, 16 KiB each, 96 MiB file cap
  (**designed mutually checked bounds**).

### Measured results & disclosures

- The 61 MP RAW probe measured `151 MB` peak commit for decode,
  `1771 MB` for calibration/render preparation, and `1766 MB` for the
  full-resolution render tail; the combined process peak remained `1771 MB`.
- The release battery is **974 library (965 pass + 9 `#[ignore]`d forensic
  probes) / 15 CLI / 145 GUI / 2+2 contract** tests. Environment-gated real
  Lightroom, brush-table, and RAW-zoo suites are additional and are not
  smuggled into the ordinary count.
- The build workflow checks default and GUI feature sets on Ubuntu and macOS;
  the published v1.0.0 binary artifacts are Windows builds.
- `scripts/check_docs.py` re-derives version, extension, camera, dependency,
  toolchain, and test-battery claims from the tree. A moved claim is a failure,
  not a silent skip.
- Loopback is a network boundary, not an excuse to trust every browser tab.
  Capability, Host/Origin, cache, and frame protections remain independently
  tested.

### Source

- `src/lib.rs` and `src/main.rs` — shared library and CLI boundary.
- `src/gui.rs`, `src/gui/`, `src/store.rs`, and `src/pixel_source.rs` — egui,
  variants, versions, and deleted-version identity registry.
- `src/serve.rs` and `src/web/` — embedded web UI and loopback defenses.
- `src/denoise.rs` and `python/denoise.py` — SCUNet sidecar and
  `sidecar_wrote` contract.
- `src/jobs.rs` and `src/decode.rs` — memory probes, concurrency budget, and
  RAW admission.
- `.github/workflows/build.yml` and `scripts/check_docs.py` — CI and document
  drift gates.
- `docs/ARCHITECTURE.md` and `docs/ROADMAP.md` — release battery and operational
  boundaries.
