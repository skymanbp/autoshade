# AutoShade v1.5.0 — every control Lightroom lets you set now moves pixels, a burst merges four ways, and the RAW denoiser is our own network

One instruction, 2026-09-17: "while we are at it, implement every feature we
can adjust but cannot render — a slight deviation from Lightroom is allowed,
compatibility is the aim", together with "build a proper stacking / merging
feature: bracketing, focus stacking, exposure fusion".

## The policy this release revokes

Policy SF4-C (R25 B3) kept operators Adobe has never published out of the
engine. A control that reached the sidecar but had no first-party measurement
behind it was carried verbatim and shown read-only: the panel said what
Lightroom had set, and no pixel moved. That was the right line while the only
claim this engine made was byte-faithfulness to a sidecar.

It is no longer the line. Eight controls in the Detail panel, nine in Effects,
the six de-fringe sliders and the CA instruction beside them, the two lens
profile strengths, the eight Transform keys with Upright over them, the nine
HDR keys, the imported spot removal and the camera profile itself all render in
v1.5.0. Where Adobe publishes a result — the Upright matrices, the
DCP tables, the spot geometry — the render is Adobe's own numbers. Where Adobe
publishes nothing, the operator is built from what Adobe DOES document about
each slider (which way a result moves when the slider moves), every free
constant is named in the source, and the panel says whose model it is. Expect a
family resemblance on those, not a pixel match.

## Detail — sharpening and both noise reductions

Eight of the panel's eleven controls used to reach the sidecar and move no
pixel here. `render/detail.rs` renders all of them: capture sharpening with its
Radius, Detail and Masking; luminance noise reduction with its Detail and
Contrast; colour noise reduction with its Detail and Smoothness.

Lightroom states Radius, the noise-reduction neighbourhoods and the grain in
pixels of the FULL-RESOLUTION photograph. This engine develops a working raster
that is the whole frame on export and a downscaled copy everywhere a person is
looking, so every length here converts through `FilmScale` rather than being
read off the raster. A preview therefore shows what the export will look like
after downscaling — which, for a one-pixel sharpening radius on a 61 MP frame
viewed at 1280 px, is very little, and that is the honest answer rather than a
preview that flatters the slider.

A control whose Lightroom default is not zero (Radius 1.0, sharpening Detail
25, the two noise-reduction Detail and Smoothness sliders 50) shows that default
until it is moved; setting it to 0 writes a real 0.

## Effects — the post-crop vignette and film grain

The nine Effects controls were carried-only for a structural reason, not a
policy one: "post-crop" is a statement about ORDER, and this engine develops
pixels first and runs the geometric chain (lens geometry → Transform →
straighten → crop) afterwards. At the point `apply_develop` finishes, the crop
has not happened and the frame is still the whole sensor.

`render/finish.rs` is the tail that was missing. It runs after the crop, and it
is written ONCE for the four surfaces that each spelled the geometry out
separately — the RAW render, the baked render, the GUI canvas and the web
preview — two of which had already drifted into two different expressions of
the same gate. An export cuts the crop out and the finishing pass sees the
delivered frame; both previews stay full-frame on purpose and pass
`CropPolicy::Keep`, which POSITIONS the two operators on the crop rectangle
without cutting it, so the canvas shows the vignette where the export will put
it and the grain lattice is anchored to the same corner of the same rectangle.

The vignette's three Styles behave the way Lightroom describes them —
Highlight Priority spares bright pixels channel by channel and can shift their
colour, Colour Priority spares them by the pixel's brightness and cannot,
Paint Overlay mixes toward black or white — and Roundness runs from a rounded
rectangle through the crop's own ellipse to a circle. The grain is the same
grain on every export of the same photograph, never a fresh sprinkle of noise.

## Lens — de-fringing, the CA instruction, and the profile strengths

**De-fringing** (`render/lens.rs`) was carried-only because Adobe's 0..100 hue
numbers have no published mapping to a hue angle. That mapping is now stated as
OURS and named for the kit ladder that will measure it: the purple scale spans
220°–360°, the green 60°–180°, which puts Adobe's own defaults (30/70 purple,
40/60 green) over the violet-to-magenta arc a fast lens actually fringes with
and the narrow green beside it. The correction acts on high-contrast edges
only, so a purple subject that is not on an edge is never touched, and a hue
window corrects nothing until its Amount is above 0.

**「Remove chromatic aberration」 is an instruction, not a number**, so
rendering it means running the solver it names. `render::lens::solve_lateral_ca`
least-squares the red-minus-green residual against the radial lever
`r·∂G/∂r` and answers in the manual Red/Cyan and Blue/Yellow sliders' own
integral units — so the preview and the export decide the same thing, and the
panel can show what it decided.

**The two profile correction strengths** (Distortion amount, Vignetting amount)
scale whichever profile the photograph has, and since this release that can be
an Adobe `.lcp` read from the Camera Raw profiles already installed on the
machine, for bodies whose RAW carries no correction data of its own. Nothing
Adobe ships is bundled or redistributed; a camera's own in-RAW measurement
always outranks a profile-database average; a body with neither renders with no
profile correction, exactly as before.

## Transform and Upright — Adobe's coordinate system, measured

The eight `crs:Perspective*` keys were the only members of `Tier::PassThrough`:
carried verbatim, never interpreted, because a keystone is a frame operation
and the engine had no frame operation to put it in. `render/perspective.rs` is
that operation — one projective map between the lens resample and the
straighten.

Adobe does not publish its Upright solver, but it publishes that solver's
RESULT: one 3×3 matrix per mode in `crs:UprightTransform_0…5`. Over the 125
matrices in this operator's library (21 sidecars, 13 with a mode actually
selected):

* they are full projective maps — 30 of the 125 carry a non-trivial bottom row,
  the strongest `h20 = −0.9458` — and not affine;
* the frame centre in **[0,1] coordinates** is a fixed point to 8.6e-4 on the
  64 near-identity ones, many of them to 5e-10, against 2.6e-2 for the
  sidecar's own `UprightCenterNorm` and 3.6e-1 for a [−1,1] centre;
* the normalisation is **per axis**, not aspect-aware: a 0.9786° rotation
  carries a scale of 1.017110, which is the unit square's cover factor
  `cos+sin = 1.016934` and not a 3:2 frame's 1.011237;
* Adobe has already folded the cover scale in — inverting each of the 13
  selected matrices and mapping the destination corners back, the worst
  excursion outside the source frame is **+0.000000**, 13 of 13.

So a photograph corrected in Lightroom renders Adobe's own answer to the last
digit and needs no fill scaling of ours. On a photograph Lightroom never
corrected, the dropdown runs this engine's own solver instead: no Hough
transform, because every edge pixel already states a line and the family's
vanishing point is the smallest eigenvector of `Σ w lᵀl`, taken by Jacobi
rotations with one robust re-weighting pass. Guided is a named refusal — it
needs the guide lines drawn in Lightroom, and no sidecar carries them in any
form this engine models.

`crs:CropConstrainToWarp` is OBEYED rather than reinterpreted (user ruling,
2026-09-17): at 0 — which is what all 52 sidecars in the library that carry it
say — the empty corners a manual slider leaves are visible and the
photographer's own crop removes them. Those corners are WHITE, measured off four
kit exports that vacate part of the frame; `PERSP-X+20`'s left column is 100.0 %
`255,255,255`, and the fill follows the warp rather than the scene, since
`PERSP-V+50` whitens the top corners of the same frame where `PERSP-V−50`
whitens the bottom.

### And the seven manual sliders, measured too

Fifteen `PERSP-*` exports were each fitted to a homography — phase-correlated
blocks, coarse to fine, the reference pre-warped by the running estimate so a
keystone's local scale change cannot blunt the correlation, and every fit taken
within ONE renderer so demosaic and profile cancel. Eleven converged to a
residual under 0.05 px. Five of the seven sliders were wrong:

* **X/Y Offset** slides 0.8121 of the frame per full slider, not 0.25 — the
  frame moved 3.25× too little — and the **y sign was inverted**;
* the **keystone** is −0.65 per full slider per short edge: the sign was
  backwards and the throw half again too strong, and the two axes differ by the
  frame's aspect because Lightroom divides a pixel offset by ONE length for
  both, which this engine did not;
* **Rotate** is a rigid rotation in PIXELS — slider +5 measures 4.9884°, so the
  slider is degrees. Applied in the per-axis box, as it was, the frame both
  under-rotates by its aspect ratio and shears;
* **Aspect** is `ln(1.1)` per full slider, not `ln(1.5)` — 4.25× too strong;
* **Scale** was already right (0.79995 × 0.80007 at 80, 1.19989 × 1.20011 at 120).

The Upright path needed nothing: its four modes reproduce Lightroom to 3–4
decimal places because they render Adobe's own matrix, and against the real
library that is the path that matters — of 348 sidecars, 13 use Upright and 3
move a manual slider.

**Two deviations remain, both measured and both named in the source.** Lightroom
pairs each keystone with a stretch along that keystone's own axis, and it
follows the LENS rather than the slider: 1.27788 at slider 50 on a 51 mm frame
against 0.9487 at the same slider on a 15.5 mm one. Six readings over two
photographs do not identify a law, so no stretch is applied and the error is
stated instead — −22 % on the long-lens frame, +5 % on the wide one. And moving
BOTH keystones at once induces a roll in Lightroom (+8.027° against our
+1.347° on `PERSP-MIX`) that one observation cannot calibrate; no sidecar in the
library moves both.

## The camera profile, and creative Looks

A Lightroom sidecar names the rendering it was developed through, and since
this release that name is read rather than carried. `src/dcp.rs` parses the
`.dcp` Adobe installed — a plain TIFF/IFD, every offset bounds-checked because
the file is the user's and not ours — and `render/profile.rs` applies it in the
order RawTherapee's `rtengine/dcp.cc` documents: reference matrix → HSV →
`HueSatMap` → RGB → baseline-exposure gain → HSV → `LookTable` → RGB → tone
curve.

The table's index order was **measured, not assumed**: at saturation index 0
the saturation scale is exactly 1.0 on 60 of the 60 three-dimensional tables in
the installed pool under the shipped reading, and 0 of 60 under either
alternative. Two calibrations blend in RECIPROCAL temperature, so the midpoint
between 2856 K and 6504 K is 3969 K and not 4680 K.

**Stated deviation:** the profile's colour MATRIX is not adopted. The engine
keeps the camera→XYZ transform its calibration lane was measured against, so
only the tables and the tone curve render. **`crs:LookTable`'s own payload is
not decodable** — base85 over a measured 85-character alphabet, 6.408 bits per
character, no decompression under zlib/deflate/gzip/bzip2/xz/lzma/zstd/lz4 at
any offset ≤ 256 under four conventions — so a creative Look is named in the
panel and in `diag.warn` rather than silently dropped.

## HDR edit mode and its SDR rendition

Lightroom's HDR mode does not change the capture; it moves where DIFFUSE WHITE
sits in it and calls the stops above it headroom (`crs:HDRMaxValue`). Every file
this engine writes is SDR, so what it owes such a photograph is Lightroom's own
answer to the same problem: the SDR RENDITION, tuned by the seven `crs:SDR*`
controls that panel shows only in HDR mode.

`render/hdr.rs` renders it as the develop's LAST stage, because that is what it
is — not another edit but the mapping of the finished edit into the range this
engine can publish. The headroom gets its OWN curve: Reinhard in linear light
against a white point of `2^HDRMaxValue`, so the stops the sidecar states enter
as the thing a white point already means and the shoulder has no free parameter
to fit. It runs first and the seven SDR controls tune what it produced.
`crs:HDREditMode` gates all nine keys, so a stale SDR value cannot re-tone a
photograph whose owner has left the mode.

It was a negative Highlights push until the kit measured it. Folding the
shoulder into a slider made it inherit the engine's band-collapse guard — right
for a slider a photographer drags, wrong for a rendering transform — and it
capped the shoulder at 55 % of Lightroom's, unable to pass 0.104 below the
diagonal where Lightroom reaches 0.189 whatever headroom the sidecar stated.
Rendered end to end through the CLI, the curve moves the transfer from rms
0.06344 to **0.04193** and the top end from an error of +0.0855 to **+0.0037**.

**What is still NOT measured, and says so in the source**: all three HDR cases
carry `crs:HDRMaxValue="+2.30"`, so one white point is pinned and the curve's
behaviour at other headrooms rests on Reinhard's own form; `SDRBlend` is 0 in
all three; and Brightness's stops-per-100 remains a first-principles value,
because the one case that moves the SDR controls moves four of them at once.

## Imported spot removal

`crs:RetouchAreas` — the dust, the power line, the stranger on the beach — is
read into `EditRecipe::retouch` and re-solved from the frame's own pixels,
first in the develop chain so that no later stage sharpens a patch seam.
Measured on the reference library: 25 of 175 sidecars, 121 areas, every one of
which used to come back onto the canvas with nothing on screen to say so.

Two axes are kept apart: the FILL (5 plain heals, 99 classical PatchMatch, 17
Firefly) and the GEOMETRY (84 ellipses, 37 brushes, the brushes reusing the
mask side's own stroke rasteriser). For 116 of the 121, Adobe keeps the
synthesised pixels in its own store, so the panel names how many it synthesised
and offers to re-run the generative model over exactly those shapes — this
engine's own answer, from this engine's own pixels, labelled as ours. The
coordinates are pre-lens-correction and need no unwarp, but they do turn: 7 of
the 25 sidecars are `tiff:Orientation="8"`.

## Stacking and merging

`src/stack/` merges several frames of one scene into one, over a single
alignment, four ways:

| | what a weight means | what comes out |
|---|---|---|
| **HDR merge** | how trustworthy a sample is as a MEASUREMENT of light | a radiance, plus the stops of headroom it recovered |
| **Exposure fusion** | how good a pixel LOOKS | a finished frame, with no radiance anywhere in it |
| **Focus stack** | how sharp a NEIGHBOURHOOD is | one frame sharp throughout |
| **Noise stack** | every frame alike, minus the readings that disagree | one frame with the grain averaged away |

The first two are different answers to the same question and both are offered
deliberately: the HDR merge hands the develop pipeline a frame with recovered
highlights and a recorded headroom, which the SDR rendition section above then
shapes; the fusion hands it a finished-looking picture that no amount of
headroom can be read back out of.

The result lands as a new **▦ Stacked** card carrying the develop it was run
from, and the frame it started from keeps its own pixels. One entry point loads,
merges and writes for all three front ends, so the CLI's `stack` command, the
desktop app's ▦ card worker and the web UI's `/api/stack` cannot drift on the
rules a merge depends on.

**The alignment is the part that had to be got right.** It is
inverse-compositional Lucas–Kanade on a Gaussian pyramid solved in LOG
LUMINANCE — a change of exposure is a constant offset there and a gradient
cannot see a constant, which is the whole reason a two-stop bracket registers
at all. Three defects were found by measuring the merged picture rather than
the warp, and each is fixed and pinned:

* a plain least-squares fit read a subject that walked across the frame as
  evidence about the CAMERA, and aligned that frame by **539.7 px** with 69.42 %
  of the picture pushed out of view. Samples are now weighted Geman–McClure
  against three times the median absolute residual;
* a Gauss–Newton step is a GUESS, and both passes now keep the parameters whose
  cost they actually MEASURED on the following pass rather than the last
  untested step;
* a near-periodic texture matches itself again one period over, so blocks
  invented 7.5–23.3 px of local motion on frames that had not moved at all. A
  block's answer must now beat DOING NOTHING by 10 %, and then survive the
  FIELD: a subject that moved is several blocks wide and its blocks agree with
  each other, while a period re-lock is one block disagreeing with everything
  around it.

Measured end to end on real files through the CLI. The bracket's exposures come
back at **−2.49 / −4.98 EV** against a −2.5 / −5.0 truth and the headroom at
**3.79 EV** against log₂(13.45) = 3.7495; the focus stack carries **42.03** mean
|Laplacian| where each of its two sources carries 24.05, because each source is
sharp in only half the frame. The noise stack is measured against the fixture's
own truth both ways, because alignment is the one thing that can only cost it:

| five frames, one carrying a ghost | outside a source | rms vs truth | against one frame |
|---|---|---|---|
| one frame | — | 0.02982 | — |
| stacked, no alignment | 0 % | 0.01472 | ÷2.03 |
| stacked, aligned | 0.87 % | 0.01514 | ÷1.97 |

The ideal for five frames is ÷2.24. Aligning frames that were already
registered therefore costs 3 % of the noise reduction — and the ghost comes out
slightly better for it (worst deviation 0.0663 against 0.0702 unaligned), which
is the other half of what a noise stack is for.

## The RAW denoiser is our own network now

v1.4.1 ran DPIR's released `drunet_color` weights on the sensor mosaic. This
release ships `autoshade-raw-denoise-v1.pth`: those weights fine-tuned for
THIS pipeline — the generalised Anscombe transform this sidecar actually
applies, with the loss taken in the stabilised domain the model sees — on real
RawNIND pairs (Brummer & De Vleeschouwer, CC BY-SA 4.0) and on synthetic sensor
noise drawn from a wide sensor family over the operator's own low-ISO frames.
The whole training pipeline ships with it (`scripts/fetch_rawnind.py` →
`prep_pairs.py` / `prep_clean.py` → `train_raw.py`, with `noise_synth.py`,
`val_scales.py`, `lr_realset.py` and `denoise_bench.py` for the measurements).

It is a much stronger denoiser at the same noise estimate, so the operating
point is not inherited — it is a property of the network, and it was chosen
again from the same four measurements:

| `SIGMA_SCALE` | grain vs Lightroom, 15 pairs | stars: sky / faint / v.faint | held-out PSNR | bench ×5 |
|---|---|---|---|---|
| 1.00 | 0.03 / 0.11 / 0.06 | 0.03× · 58 % · 24 % | 46.95 | +2.02 +2.77 |
| 0.85 | 0.68 / 0.55 / 0.65 | 0.21× · 64 % · 32 % | 45.95 | +1.55 +2.31 |
| **0.78** | **1.06 / 1.01 / 1.01** | **0.53× · 69 % · 40 %** | **44.68** | **+0.81 +1.72** |
| 0.72 | 1.42 / 1.29 / 1.32 | 0.97× · 73 % · 48 % | 43.10 | +0.08 +1.17 |
| *Lightroom itself* | *1.00 everywhere* | *— · 71 % · 43 %* | — | — |

0.78 is where this network has **Lightroom's own texture** on real photographs
— within 6 % of it in all three windows — and keeps the faint stars within
three points of Lightroom's own count, while reading **1.98 dB** better than
the generic weights on held-out pairs. Higher smooths past Lightroom; lower
gives the synthetic bench's ×5 level away. Compatibility is the aim, so the
point that matches Lightroom wins the ties (user's decision, 2026-09-17).

The weights are a release asset, fetched once against a pinned SHA-256 and byte
count before anything is unpickled, exactly as the generic weights were. The
generic `drunet_color.pth` is no longer fetched at all: every one of its
32,640,960 parameters moved, so an installed machine downloads 130 MB once
instead of twice.

## What is not in this release

**Lens Blur** (Lightroom's depth-aware blur) is not implemented. It appears in
none of the 175 sidecars in the reference library, and calibrating a depth
model against Lightroom needs exported pixels this project does not yet have;
its keys go on being named by `unmodelled_global_crs` rather than silently
dropped.

The Lightroom experiment kit specified for this release — 140 cases over six
base frames — **was exported and measured**, and it is what turned
`perspective::VOID`, the HDR shoulder and five of the seven Transform sliders
from choices into measurements. That is also the honest limit of what it
settled. Its `SH-*`, `NR-*`, `CNR-*`, `PCV-*`, `GRAIN-*` and `DF-*` ladders were
rendered and scored, and each still shows a residue against Lightroom that this
release does not close; those numbers are recorded rather than acted on, because
a ladder that has been measured once is a starting point for a calibration and
not a calibration.

**What remains first-principles, and says so where it lives**: the de-fringe hue
scales, `hdr::BRIGHTNESS_EV_PER_100`, and the keystone's same-axis stretch,
which the kit measured but could not model — it follows the lens rather than the
slider and two photographs do not identify it. Each names the experiment that
would replace it, and the Transform and HDR sections above give the numbers as
they stand. The panels say whose model they are.
