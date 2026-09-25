<div align="center">
<img src="assets/icon.png" width="104" alt="AutoShade icon" />

# AutoShade

**AI-assisted automatic development of RAW photographs.**

Describe the picture you want; an image model generates it from your frame,
and AutoShade recovers from it an editable develop recipe that renders that
look on the full-resolution RAW — the recipe carries the look, the sensor
carries the detail, and the generated picture is a target, never the
delivery. An AI decides *what to change*; a deterministic Rust engine *does*
it, and **in the recipe-development path the AI never touches a pixel.**
The one network that does, the RAW denoiser, was trained here on this
project's own data and is held to a same-frame comparison with Lightroom
on a real star field whenever it changes.

[Download v1.6.1](https://github.com/skymanbp/autoshade/releases/tag/v1.6.1) ·
[Architecture](docs/ARCHITECTURE.md) ·
[Release ledger](docs/ROADMAP.md) ·
[MIT](LICENSE)

</div>

---

## What AutoShade is

- The way to get the picture you can only describe: `reimagine` asks an
  image model for it, generated from your own frame; `match` measures how
  far that picture strayed from the frame and recovers an engine recipe that
  renders its look on the full-resolution RAW — editable, replayable,
  exportable to Lightroom — while the generated picture stays a target,
  never the delivery.
- A non-destructive developer for RAW and baked images: an AI proposal becomes
  a small, inspectable `EditRecipe` — bounded controls, a rationale, a
  confidence — rendered by one local Rust engine behind the app, the CLI and
  the web UI.
- The recipe is hand-editable, replayable a year later, and can be handed to
  Lightroom; generative tools are separate, opt-in, labelled paths.
- For anyone who wants an AI first pass on a card of RAWs and still wants to
  know *what* it changed, in numbers, before trusting it.
- The one network this project ships its own weights for is a RAW denoiser:
  trained here, on this project's own data, and judged release after release
  against Lightroom's Denoise on a real star field, by pass marks written
  down before each training run.

## Contents

- [What AutoShade is](#what-autoshade-is)
- [What it does](#what-it-does)
- [What is new here](#what-is-new-here)
- [How it works](#how-it-works)
- [Measured numbers](#measured-numbers)
- [Install and quickstart](#install-and-quickstart)
- [User manual](#user-manual)
- [Supported formats](#supported-formats)
- [Tech stack, algorithms, and design philosophy](#tech-stack-algorithms-and-design-philosophy)
- [Status, roadmap, and known limitations](#status-roadmap-and-known-limitations)
- [License and acknowledgements](#license-and-acknowledgements)

## What it does

- **Whole-image AI generation, then reverse-fit** — `reimagine` asks an
  image model (gpt-image-2) for the picture you describe, generated from
  your own frame; `match` estimates an engine recipe from that picture, or
  from any finished look of the same frame, measures how far its *content*
  diverged before trusting it, then fits global, semantic, luminance-range
  and colour-range corrections behind evidence gates; from the default
  Strength up it may also carry a smooth
  12×8×8 local colour field, the one control Lightroom cannot render (the
  sidecar still carries it).
  A structured sky/land residual can earn two or three overlapping native
  bands that replace its single correction; hard spatial tiles use four
  intersecting gradients. Each candidate keeps the existing evidence and
  boundary gates, and the field solves the remainder after accepted bands.
  Where a repaint broke the pixel-to-pixel correspondence inside one region —
  and only there, since a region whose pixels still correspond may not overrule
  them — that region's own 12×8 cell means decide whether the move ships: closer
  to each cell's target, and in the direction that target asks for. A
  same-layout recolour is recoverable, a region whose layout moved is still
  refused, and the refusal is a measurement printed with the shares it was
  decided on.
- **AI develop** — `analyze`, `auto` and **Analyze** propose an editable
  recipe from preview, EXIF and histogram, check it data-only, render it, and
  may buy one bounded revision.
- **A deterministic develop engine** — tone, white balance, curves, HSL,
  colour grading, texture, clarity, dehaze, NR, sharpening, vignette, crop and
  lens correction, under linear, radial, brush, bitmap, luminance-range and
  colour-range masks composed by Add/Subtract/Intersect. Linear, radial, brush
  and AI components export that composition in Lightroom's own grammar;
  Bitmap components retain a named loss.
- **A RAW denoiser trained and judged here** — the sensor mosaic is cleaned
  before demosaic by a network whose weights were trained on this project's
  own data, told the noise measured tile by tile on the frame itself, and
  held, release after release, to a same-frame comparison with Lightroom's
  Denoise 50 on a real star field. It returns only luminance grain (71 %
  default), keeps faint stars (98.70 % of 17,817 real faint stars against
  Lightroom's 98.80 %) and maps hot pixels in every develop. Four training
  runs were made: the first two shipped, the last two failed the pass marks
  written down before they started and were refused. [§11](#11-the-raw-denoiser-is-trained-here-and-judged-by-pass-marks-written-before-the-run)
  has the picture and the numbers.
- **Local AI masks** — subject (BiRefNet, named U²-Net fallback), sky
  (OneFormer ADE20K) and point-prompted object (SAM 2.1), as local Python
  sidecars with pinned weights; no API key.
- **Lightroom/ACR interoperability** — sidecar XMP is the merge base, written
  back with unmodeled fields preserved byte for byte; beside-RAW export is a
  separate confirmed action. Since v1.3.1 the whole develop also rides inside
  the sidecar under AutoShade's own namespace — measured to survive a
  Lightroom 9.4 rewrite byte for byte — so a sidecar that went through
  Lightroom reopens exactly, with Lightroom's own edits on top.
- **Style read** — your past Lightroom edits, and a separate library of
  finished looks, retrieved as soft references through opt-in local SigLIP 2
  embeddings.
- **Generative and pixel tools, opt-in and labelled** — reimagine
  (gpt-image-2), retouch, heal and AI denoise are the only paths that can
  invent or alter scene content, and are marked so; a denoise lands as its
  own card and never rewrites the original. RAW denoise is the trained
  denoiser above: higher is cleaner, 100 % is the complete network output,
  0 % the input; the first launch after the RAW default changed reset old
  RAW dials to it once, and baked SCUNet choices are kept separately.
- **Stacking and merging** — several frames of one scene into one, over a
  single alignment: HDR merge (exposures measured from the pixels, samples
  weighted by how trustworthy they are, the recovered stops handed to the SDR
  rendition stage), exposure fusion, focus stack and noise stack. The
  alignment is a global affine — shift, rotation, scale and a zoom's breathing
  — refined per block for a subject that moved on its own, and it works on a
  bracket because it matches in log luminance, where a change of exposure is a
  constant offset that a gradient cannot see.
- **Versions, variants and three front ends** — Original, AI-generated
  (immutable: an edit on one continues on an Edited-AI card beside it),
  Reverse-fit, Denoised and Stacked cards with numbered snapshots in a
  per-user develop store shared by all three.

Out of scope in this release: bit-exact Adobe rendering (parity is measured),
an exact X-Trans demosaic (the plane fit is approximate) and a notarised macOS
build (a decision, not a gap — the bundle stays ad-hoc signed, so the first
launch needs one explicit 「Open Anyway」 per machine).

## What is new here

The techniques below are the ones you will not find in another RAW developer.
Each ends at the document that carries the rest; the last subsection lists
what is designed but not yet shipped.

### 1. Whole-image AI generation, then reverse-fit: an editable recipe from any finished look

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/pillar-reimagine-fit-dark.svg" />
  <img src="docs/images/pillar-reimagine-fit-light.svg" alt="Pillar 1: a generated or finished target is measured against the input by the structural-divergence statistic D, which selects a full solve or a bounded atmosphere mode; a robust tone regression and gated local stages produce a recipe, and only the recipe reaches the full-resolution render" />
</picture>

<sub>Zoom and pan this diagram at [autoshade.dev/#pillar-reimagine-fit](https://autoshade.dev/#pillar-reimagine-fit).</sub>

Ask for the picture in words. `reimagine` sends the frame and the prompt to
an image model and gets back a complete picture that may have invented
content; `match` then recovers, from that picture, a recipe the engine can
render on the full-resolution RAW — global tone and colour first, then
zones, bands, tiles and a smooth colour field, each admitted only on
evidence. The recipe carries the look and the sensor carries the detail;
the generated picture is a target, never the delivery. The three pairs
below are the whole path, and every number in their captions is measured.

The source frame is one function for both entry points: a neutral develop of
the RAW at a 2048 px working edge, never the camera's embedded JPEG preview.
That matters because the two read differently — on the desert pair below, the
embedded rendition measures D = 0.361 and buys the bounded atmosphere path,
while the neutral develop of the same sensor frame measures D = 0.275 and
earns the full solve. The pair is read at TWO scales and both are printed:
D = 0.275 per pixel and 0.609 per layout on that frame, so a report says
whether the solve paired source pixel with target pixel or only their
statistics. The coarse reading is a second scale, not a more forgiving one —
measured, it reads HIGHER at frame, zone and cell scope, because the luma is
rank-equalised against each image's own histogram and removing the shared
fine texture leaves exactly the regional luma a repaint moved.

<img src="docs/images/showcase-viaduct-reverse-fit.jpg" alt="Stone viaduct: straight conversion, generated target, and the recovered recipe rendered on the RAW, with a 1:1 detail row" />

<sub><b>Stone viaduct.</b> Top row: the straight conversion, a 3520×2352
<code>gpt-image-2</code> target asked for <i>a clearer afternoon, a little more
contrast, a slightly deeper blue sky, everything else unchanged</i>
(<b>D = 0.180</b>, under the 0.35 threshold, so the full solve ran),
and the recovered recipe rendered on the 9504×6336 RAW, fitted at Reverse-fit
strength 100 % (the product default is 65 %): look error
<b>0.161 → 0.023</b> at confidence 0.63 through a global solve whose cast
curves were projected to t = 0.485, a four-band colour mixer at the 45
ceiling, two semantic zones, two boundary-gated tiles and one field mask.
Bottom row: the same window of the frame at each source's native
resolution — the recipe carries the look, the RAW carries the detail, and
the generated frame carries neither at full size. At the default 65 % the
same pair fits to 0.047 at confidence 0.25 with the mixer capped at 18, and
v1.2.2's fit of it is where the seam fix was measured, on the top-left sky
tile: cross-boundary step 0.0278 → 0.0042, the delivered seam +3.15 → +0.92
codes on the mask-free ruler.</sub>

<img src="docs/images/showcase-cornwall-reverse-fit.jpg" alt="Cornwall lighthouse islet: straight conversion, generated target, and the recovered recipe rendered on the RAW, with a 1:1 detail row" />

<sub><b>Cornwall lighthouse islet.</b> The same three stages on a frame shot
with the body set to a 4:3 aspect, which is how it found the two frame defects
v1.2.2 fixes: sized from the sensor frame the same prompt bought a target at
<b>D = 0.136</b> (0.304 when the request was sized from the cropped
preview), and the fit ran on a neutral develop of the full frame with the
calibration composed into the solve — look error <b>0.137 → 0.027</b> at
confidence 0.66, two semantic zones, four boundary-gated tiles and two field masks. This is the frame
that found v1.2.3's cast defect: the v1.2.2 fit admitted three channel curves that
passed every hue veto and still fanned the sky 33.1° across luminance
(violet at the top, green-cyan in the bright cloud). A fourth veto now reads that fan,
and the curves are shrunk toward one shared shape (t = 0.363) until it clears —
the delivered sky spread is 9.6° against the target's 1.6°. Full measurements
and prompts in [docs/SHOWCASE.md](docs/SHOWCASE.md).</sub>

<img src="docs/images/showcase-canyon-reverse-fit.jpg" alt="Desert canyon at dusk: straight conversion, generated target, and the recovered recipe rendered on the RAW, with a 1:1 detail row" />

<sub><b>Desert canyon at dusk.</b> The desert pair from the paragraph
above — the reference pair v1.3.0 and v1.3.1 were measured on — and the
hardest of the three: the target is a full regeneration, not a grade, so
the sky's texture is re-synthesised and only its layout survives
(<b>D = 0.275</b> at pixel scale, 0.609 at layout scale; the sky zone alone
reads 0.617, past the 0.35 pairing line). Top row: the straight
conversion, the 3520×2336 <code>reimagine</code> target, and the recovered
recipe rendered on the 9504×6336 RAW, fitted at Reverse-fit strength 85 %: look
error <b>0.110 → 0.048</b>, whole-frame mean |diff| against the target
<b>0.0276</b> (v1.2.6 shipped 0.0571), sky ΔE <b>18.2 → 4.9</b> — a solved
white balance (5653 K as shot → 8400 K), −1.3 EV under a six-knot residual
curve, global saturation +35 with clarity and texture +20, the mixer on Red
and Orange (+33/+33), two Select Sky bands vouched by the region's own
cells where its re-synthesised pixels have no partner, four boundary-gated
bitmap tiles, and the 12×8×8 colour field that renders in-app and rides in
the sidecar's payload. Confidence 0.25, read from the accepted zone's
residual, not from the frame. Bottom row: the same window at each source's
native resolution. What the fit does not reach is disclosed rather than
measured away: the land stays at ΔE 6.9, and the warm haze on the far mesas
is the known gap. Full numbers in [docs/SHOWCASE.md](docs/SHOWCASE.md).</sub>

`match` recovers an editable recipe from any finished rendition of the same
frame ([`src/fit.rs`](src/fit.rs)). A generated target is not pixel-aligned
with its source, so the solve is **distribution-level, not per-pixel
regression**:

- Luminance CDFs are matched at the engine's own tone knots and least-squares
  solved against its own slider basis under a ridge and a model-selection
  prior; saturation closes by mean-chroma ratio.
- The per-channel CDF residual becomes RGB curves admitted only through four
  vetoes and a projection: one refuses a cast painting a hue more than 45°
  from every target family over ≥ 5 % of the frame, and the fourth (v1.2.3)
  refuses curves that fan a single-hued class by ≥ 15° across luminance.
- Residual tone-curve knots sit uniformly in the LUT's *output* domain, which
  keeps a steep camera base curve from sagging the chords by ~10/255.

Details: [docs/TECH_STACK.md#ai-advisor-and-reverse-fit](docs/TECH_STACK.md#ai-advisor-and-reverse-fit).

### 2. A structural-divergence statistic decides how much to believe a target

A structural reading `D` — gradient correlation and a five-band pyramid energy
error — measures whether the target still shows the same scene.

- Same scene → the Full solve above. Repainted scene (`D ≥ 0.35`) → bounded
  **Atmosphere** mode: EV ±1, WB gain [0.80, 1.25], saturation ±30, curve
  slope [0.5, 1.5], confidence capped at 0.50, no per-channel curves, and a
  *structure-blind* ruler that stops asking replaced content to survive.
- Strength governs that budget: the shipped 0.65 path's global controls, WB
  included, are byte-identical to the calibrated path's (the colour field is
  the one control that attaches at 0.65 and not one click below); above it an
  out-of-budget WB is shrunk along
  its fitted log-K/linear-tint manifold and must clear the foreign-hue veto
  and a rotation budget opening from 0.05 at default through 0.593 at 0.85 to
  1.0 at full strength, or it is withheld.

Details: [docs/TECH_STACK.md#reverse-fit-freedom-budget](docs/TECH_STACK.md#reverse-fit-freedom-budget).

### 3. Diffusion features find where the content moved

On divergent pairs the fit consults a **DIFT correspondence field** — Stable
Diffusion 2.1's UNet as a featurizer (`t = 261`, 768² inputs, `up_blocks[1]`
features, an 8-draw ensemble run one at a time to bound VRAM) — whose 48×48
grid weights a Full zone's pixel pairs and reads shifted content where it
moved.

- Confidence is cyclic consistency × local flow smoothness, with raw cosine
  kept out of it, so a pixel-shuffle of the same frame stays honestly
  unmatchable.
- An identity pair reads median confidence 1.000 at 100 % coverage against
  0.009 (21.5 %) for the calibration pair's generated sky; identity and
  zero-confidence fields change nothing, by test.

Details: [docs/TECH_STACK.md#ai-advisor-and-reverse-fit](docs/TECH_STACK.md#ai-advisor-and-reverse-fit).

### 4. Semantic zones, luminance bands and colour bands, judged on their own population

- Local corrections come from mutually exclusive producers: a local OneFormer
  ADE20K pass yields semantic regions (sky/land by default, up to four
  disjoint class regions opt-in), and with segmentation off or unavailable a
  pure-Rust pass derives **XMP-native luminance-range bands** from rank-paired
  residuals under an evidence gate, then **colour-range bands** from the eight
  ACR hue bands — one mask keyed to each band's own mean colour, read on both
  frames, and refused unless the band has evidence on both sides of the edit.
- Every verdict follows the population a correction moves: a land zone is not
  withheld because a replaced sky shares its luminance bins, and a zone whose
  luminance already matches says so instead of being dialled for a hairline
  gain.
- The **sky and land zones ride out to Lightroom** as its own Select Sky mask
  (`crs:What="Mask/Image"`, `crs:MaskSubType="2"`, the land zone the same
  component inverted), so the two corrections that separate a repainted sky
  from its ground reach the sidecar instead of being skipped as raster masks.
  Lightroom rebuilds its own sky alpha from that intent; the raster AutoShade
  renders from is its own, which the save line says in as many words (「AI
  masks ×N re-derived locally — not Adobe's raster」). The opt-in four-class
  regions, the spatial tiles and the free-form field masks are still raster
  masks classic XMP cannot hold, and keep the named bitmap loss.
- A semantic zone's boundary is held to the same per-crossing budget a tile's
  is: no seam larger than what the scene itself carries there — a feathered
  zone's band variation, a hard-edged tile's scene discontinuity (a smooth sky
  gradient masks nothing) — floored at one code value and capped at the
  calibrated 0.012 — and, since R37, charged only on
  the part of a step the paired target does not itself carry there, read as a
  cell average so a repainted texture cannot vote, so a horizon the target has
  is reproduced rather than shrunk away. Both rulers read luma **and** each
  colour channel, so a gain set that reproduces a target's mean colour cannot
  hide a coloured halo behind an unmoved luma.
- Because a budget can only take strength away, the source raster's feather is
  **widened first** where the guide is too smooth to hide anything: the same
  correction height is delivered over a ramp up to 6 % of the frame height, so
  its per-pixel step falls by the same factor at full strength, while alpha
  under a real edge is left byte-identical and silhouettes stay crisp. Both the
  widening and its abstention are disclosed.

Details: [docs/TECH_STACK.md#zone-scoped-evidence-view](docs/TECH_STACK.md#zone-scoped-evidence-view).

### 5. Quadtree tile splitting on frozen evidence

After the zones or bands, a frozen-evidence quadtree visits the strongest
supported nodes first and stops at a 4×4 grid.

- A tile is kept only when both frames contribute ≥ 3 % evidence, original
  structure stays comparable, its confidence interval excludes zero, its
  boundary stays within the calibrated rim ceiling (0.012, charged per
  crossing against the scene's own step since v1.2.2 — in luma and per colour
  channel, and since R37 only for what the target does not itself carry
  there), and the composed frame does not regress.
- Hard tiles are editable intersections of four gradients and export to
  Lightroom. A guided raster keeps its named bitmap loss only when the native
  trial fails the shared gates or fits the photo worse than the raster on the
  tile's own cell or on the frame.
- A **free-form remainder pass** ranks 4-connected, sign-pure components of
  the residual no tile covers, at most two, through the same gates; every
  proposal on the calibration corpus was refused, so it contributes
  disclosure, not corrections.

Details: [docs/TECH_STACK.md#layered-spatial-reverse-fit-and-mask-refinement](docs/TECH_STACK.md#layered-spatial-reverse-fit-and-mask-refinement).

### 6. A bilateral-grid local field prices every local producer first

Before any local producer runs, a read-only **12×8×8 bilateral grid** (x, y,
luma) of five develop parameters is solved by conjugate gradients in f64 — λ =
1 Tikhonov toward the global fit, a Laplacian smoother, ≤ 90 iterations,
weights = frozen evidence × structural support × unclipped.

- Its rendered residual is the **ceiling**: how much of the remaining
  difference *any* spatially varying develop could reach; the calibration
  pair's reading is under [Measured numbers](#measured-numbers), and the Rust
  solve agrees with the NumPy reference to 1.5 × 10⁻⁵ across 768 vertices.
- The field never touches a pixel: it proposes luminance bands to the range
  producer, refused when the sign disagrees, halves the tile budget when the
  remainder is not tile-shaped, and ends the fit early within 0.002 of a
  ceiling that beat the producer-free frame.

Details: [docs/TECH_STACK.md#local-field-analyzer](docs/TECH_STACK.md#local-field-analyzer).

### 7. Edge-aware mask refinement that has to earn its keep

- Semantic silhouettes and eligible tile boundaries go through guided
  refinement (radius 8) before their corrections are fitted — and the original
  mask bytes win unless coverage is conserved, every pixel outside the fixed
  collar is unchanged, guide-edge alignment does not fall, and the rim and
  frame gates still pass.
- The AI masks themselves run locally, weights pinned to the byte and every
  alpha cached under a provenance key, so a better backend forces an honest
  re-derivation rather than serving an older mask as the new model's.

Details: [docs/TECH_STACK.md#layered-spatial-reverse-fit-and-mask-refinement](docs/TECH_STACK.md#layered-spatial-reverse-fit-and-mask-refinement).

### 8. Generated pixels are quarantined and measured

- `reimagine` composes the prompt onto an unconditional faithfulness scaffold
  (because `input_fidelity` is silently dropped by gpt-image-2), measures the
  result's structural divergence with the same `D` the reverse-fit uses, warns
  at `D ≥ 0.35`, and can spend one bounded retry keeping the closer image.
- The GUI's **Adjust generated image** edits a ✨ card or its ✎ edit with its
  own prompt: the whole picture without strokes, or just the shared painted
  region (blank = remove). One paid generation lands as a new ✨ card, leaving
  its source intact; whole-image edits report divergence against the sent input.
- `heal` only ever copies, shifts and averages pixels that already exist, and
  anything that changed pixels lives on its own card as a pixel source — never
  disguised as a Lightroom adjustment.
- Spot removal imported from a Lightroom sidecar is re-solved here from the
  photograph's own pixels, and the panel says so: it names how many areas
  Lightroom removed, how many of those Adobe synthesised (content-aware or
  generative, whose pixels the sidecar does not carry), and offers to re-run a
  generative model over exactly those.
- A photo edited in Lightroom's HDR mode renders as its **SDR rendition** —
  Lightroom's own seven-control answer to publishing an HDR edit as an SDR
  file — rather than as if the mode had never been set. The seven render only
  while the mode is on, as in Lightroom; they are stored and round-tripped
  either way.

Details: [docs/TECH_STACK.md#ai-advisor-and-reverse-fit](docs/TECH_STACK.md#ai-advisor-and-reverse-fit).

### 9. Style reference is retrieval over your whole catalogue, not a preset

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/pillar-analysis-dark.svg" />
  <img src="docs/images/pillar-analysis-light.svg" alt="Pillar 2: a Lightroom RAW+XMP library becomes exemplars carrying a 14-dimension feature, a SigLIP 2 image vector, a Qwen3-VL sentence and a local-work habit; a query retrieves its four nearest past shots by the hybrid distance, and their habits reach the proposer behind an untrusted-data fence before a capped pull moves the proposal toward the photographer's own means" />
</picture>

<sub>Zoom and pan this diagram at [autoshade.dev/#pillar-analysis](https://autoshade.dev/#pillar-analysis).</sub>

<img src="docs/images/showcase-island-four-looks.jpg" alt="Lakeside island town: straight conversion and three AI develops driven by three different direction texts" />

<sub><b>One photograph, four looks.</b> The straight conversion of a hazy
lakeside frame and three AI develops of the same RAW at the same
<code>--style 1.0 --strength 0.9</code> against the photographer's full
index — 169 Lightroom RAW+XMP edits and a 94-photo finished-look library —
where only the <b>direction text</b> changes. Since v1.2.3 a written
direction leads and those edits become background: mean saturation
28 % / 11 % / 30 % for moody / golden / vivid against the
conversion's 17 %, mean brightness 43 % / 58 % / 70 % against
47 %. The vivid develop's recipe crops — its cell is 9504×5702, 7 % off
the top and 3 % off the bottom — while moody, golden and the conversion are
the full 9504×6336 frame. On v1.2.2 the same three directions on the same index came back
at 23 % / 11 % / 17 % saturation and 54 % / 58 % / 55 % brightness — inside those
edits' cool, hazy register, four points of brightness apart. Judge trails,
prompts and the finished-look-only run in [docs/SHOWCASE.md](docs/SHOWCASE.md);
model-judge scores are automated review, not human aesthetic approval.</sub>

`autoshade style-index <dir>` turns every Lightroom RAW+XMP pair you finished
into an exemplar ([`src/style.rs`](src/style.rs)); a photo retrieves its **4
most similar past shots** as a soft reference.

- An exemplar carries a 14-dimensional EXIF/histogram feature, the 12 develop
  settings you moved, your curve shape, colour families and a local-work habit
  — summary statistics only.
- Optional local models add a 768-dimensional **SigLIP 2** image vector and,
  with `--describe`, one **Qwen3-VL-2B** sentence about the *grade*; nothing
  leaves the machine.
- Retrieval is
  `d14 + W_EMB·(1−cos(q_img,e_img)) + W_TXT·(1−cos(q_txt,e_img)) + W_DESC·(1−cos(q_txt,e_desc))`,
  shipped at `W_EMB = 4`, `W_TXT = 0.5`, `W_DESC = 0.5` — the calibration
  harness's winners on the real corpus, hubness removed before the z-score.
- `W_LOOK = 1.0` is the unmeasured term: the look library carries no develop
  settings for that objective to see, so it ships inside a stable band.
- `style_pull` (0.18 at the shipped Style 0.3, full at Style 1.0) moves the
  proposal toward your historical means, unless a Direction at Adherence above
  40 % leads; a `--looks` library guides the proposer but never becomes a
  recipe target.

Details: [docs/TECH_STACK.md#ai-advisor-and-reverse-fit](docs/TECH_STACK.md#ai-advisor-and-reverse-fit).

### 10. Lightroom parity is measured, and the residuals are published

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/pillar-lightroom-math-dark.svg" />
  <img src="docs/images/pillar-lightroom-math-light.svg" alt="Pillar 3: sidecar and recipe read and write both ways into the engine over four measured laws — mask frames, lens geometry, tone and falloff, and the brush kernel — each published with its own residual" />
</picture>

<sub>Zoom and pan this diagram at [autoshade.dev/#pillar-lightroom-math](https://autoshade.dev/#pillar-lightroom-math).</sub>

The tone LUT, the two-arm Texture model (`A1 = 0.172443`, `A2 = 0.304888`;
45 of 45 Lightroom anchors within ±0.02), the 290×11 radial feather LUT,
the brush law `(1 − ρ^m)^n` with the measured flow constant `κ = 0.1284`
(D1 error 874 px → 9.8 px), and the lens mask-frame transport built from
Sony's own 16 native samples (radial 41/41 vectors within 1 px; linear
openly *not* pixel-closed, RMS 9.748/7.025/6.336 px) were each fitted to
Lightroom output. The XMP layer is hand-rolled on purpose — no XML crate —
so a catalogue sidecar is merged into byte for byte — down to the SVD fold
between Lightroom's pixel-space radial tilt and the engine's normalised
rotation, and down to its `tiff:Orientation`, rewritten only when the
photographer's own turn has moved away from it — and Lightroom's Brotli-packed
brush dab streams are imported and verified (`MD5 → .acr → Brotli`). Two of
those fits were re-measured in
v1.2.4 against Lightroom's own coverage rather than exported luma, on a
46-export pack: the LINEAR falloff moved onto the abscissa `t^1.124`
(α rms 0.0293 → 0.0074), and the radial boundary was shown to be a pure
0.99876 scale of the stored ellipse — no dilation law.

### 11. The RAW denoiser is trained here and judged by pass marks written before the run

<img src="docs/images/showcase-denoise-star-field.jpg" alt="One 1:1 window of a 61 MP star field, four ways: Lightroom's develop with Denoise off and at 50, AutoShade's neutral develop with no denoise and with its own denoiser at the 71 % default" />

<sub>The author's own ISO 2500 star field, one 796 × 462 px window at 1:1,
shown one stop brighter than the develops (the same gain on all four panels).
Top: Lightroom's own develop with Denoise off and at 50. Bottom: AutoShade's
neutral develop with no denoise and with its denoiser at the 71 % default —
clean colour, the frame's own luminance grain, the faint stars still there.
These are the four files the star-field test below measures.</sub>

Four training runs were made for this denoiser, on this project's own data.
The first two shipped; the last two were refused by pass marks written down
before they started, and one of them ran as five parallel jobs on a rented
cloud GPU. Everything below was measured on the v1.6.0 build, against
Lightroom's Denoise 50 on the same frame.

- **The mosaic is cleaned before demosaic**, by AutoShade's own weights: DPIR's
  DRUNet-colour architecture, fine-tuned for this exact transform on RawNIND
  pairs and on synthetic sensor noise over the author's own low-ISO frames,
  shipped as `autoshade-raw-denoise-v2.pth`. The network expects noise of one
  known strength everywhere, so the noise is **measured where it stands**: per
  tile, on the finest diagonal wavelet band, with the samples chosen by the
  three other bands so the choice cannot bias the number; a smooth map of the
  measured noise is divided out before the network and multiplied back after.
  On the star frame the per-tile residual's maximum fell 0.508 → 0.070 and the
  tile-to-tile spread of the fine-luminance ratio — how much fine grain the
  finished develop keeps — 0.185 → 0.014 (Lightroom's own: 0.011).
- **Only luminance grain comes back.** At any positive strength the full clean
  output is requested; the original and the clean frame go through the same
  demosaic and calibration, and in linear light `1 − strength` of the
  luminance difference returns along the grey axis. Every strength keeps the
  clean chroma, so colour noise cannot come back by construction. The default
  is 0.71.
- **Hot pixels are mapped in every develop** of a Bayer RAW, before the grain
  source and the cleaner read the mosaic: an isolated site stronger than 20
  sigma of its own neighbourhood (half the MAD of the 24 same-colour samples
  within 4 px, or the tile's sigma if larger), where a clipped sample never
  measures noise and eight neighbours must vouch. On ten 61 MP frames: 86 /
  20 / 77 sites inside the picture on three night frames, none on the fourth,
  0 / 0 / 0 / 6 / 0 / 4 on ordinary frames; a frame with no mappable site
  renders byte-identically.
- **Faint stars survive.** The first training run had never seen a star and
  kept 10 % of the light of a star four times the noise level (4σ), 42 % at
  6σ, 73 % at 10σ. The second run continued from it with stars added to the
  clean side of half of every batch and a loss that seeks the mean in the
  units light adds in; the weights that ship keep 54 % at 4σ, 84 % at 6σ,
  92 % at 10σ, and were chosen among that run's snapshots by pass marks
  written down before the run (`scripts/accept_v2.py`).
- **The star-field test is a release gate.** Whenever the denoiser changes,
  the author's ISO 2500 star field is denoised by AutoShade and by Lightroom's
  Denoise 50 and the two results are compared reading by reading
  (`scripts/denoise_star_standard.py`): eight readings on the denoiser itself
  decide, two on the finished develop are reported. On the v1.6.0 build: 6 of
  8 — 98.70 % of 17,817 real faint stars kept against Lightroom's 98.80 %;
  bright-star peaks 0.939 of the input against 0.953, and the four colour
  planes' flux spread 0.0385 against a 0.02 limit, stay behind; the finished
  develop reads 0.2922 against Lightroom's 0.2701 (limit ±0.03) and 0.0139
  (limit 0.0278). Two later training runs — one on stars drawn as a core on a
  streak, one as five runs on a rented cloud GPU with comet-shaped stars and
  extra weight on star cores — each had their pass marks written first,
  failed them, and were refused.

Details: [docs/TECH_STACK.md#raw-denoise](docs/TECH_STACK.md#raw-denoise) and
the release notes, [docs/RELEASE_NOTES_v1.6.1.md](docs/RELEASE_NOTES_v1.6.1.md).

### 12. The camera's own look is read from the picture, like with like

A neutral develop of a RAW does not look like the camera's JPEG. The **base
look** is the tone curve that closes that gap, estimated per photo from the
RAW's embedded preview, and since v1.6.0 it is estimated in three ways that
were each measured:

- **Paired like with like.** The pairing is made at the camera's own framing,
  and whether the preview carries the lens profile's corner lift is measured
  on the pair (outer ring against inner ring) rather than assumed. On ten
  ILCE-7RM4A frames with profile corner gains of 1.33–1.98 no embedded preview
  carried it, and the lift used to become tone: on night frames, a run of
  curve slopes from 0.33 to 2.25.
- **On block means, not pixels.** The two pictures are matched on 64-column
  block means: both sorted, walked in groups spanning at least 0.06 of neutral
  luminance with at least 64 blocks each, one knot per group at its median
  block, the ends pinned; a curve within 0.02 of the identity is no curve.
  The preview's in-camera sharpening, noise reduction and JPEG texture no
  longer read as tone: on the star frame the estimate is four knots instead of
  thirteen, and the develop sits 0.75 levels rms from the camera's rendition
  (median +0.19) against 4.24 (+2.56) before.
- **Every photo gets it.** A recipe's version stamp says which estimator made
  its curve (v1.6.0's is the third); one saved by an earlier version is
  re-estimated the first time it is opened — by the app, batch export, the web
  UI or `apply`, each of which says so. A recipe saved with no base look
  keeps none.

The tone stage scales colour by the luminance ratio, so every wiggle of slope
acted on grain; on the star-field test's two finished-develop readings (§11)
this moved 0.3075 → 0.2922 (Lightroom 0.2701, limit ±0.03) and 0.0387 (limit
0.0309) → 0.0139 (limit 0.0278). Details:
[docs/TECH_STACK.md#camera-base-look](docs/TECH_STACK.md#camera-base-look).

### Designed, not yet shipped

- **The two star-field readings still behind Lightroom.** On the v1.6.0 build
  the denoiser keeps a bright star's light (a 5×5 aperture reads 1.00–1.03 of
  the input) but spreads its core a little (the single brightest sample reads
  0.82 of the input for faint stars, 0.92 for bright ones), which is what the
  bright-star peak and the flux-spread readings measure (§11). The two later
  training runs were designed against exactly this, trained, and refused by
  their own pre-written pass marks; the denoiser ships with both readings
  recorded as behind.
- **The sharpening amount's scale against Lightroom's.** A RAW that carries no
  amount renders at Lightroom's own default of 40 since v1.6.0 (radius 1.0,
  detail 25, masking 0; a baked raster at 0; an absent amount is left to
  Lightroom in the sidecar), but the operator is this engine's own, so 40 here
  and 40 there are the same default, not a measured equivalence. The
  calibration needs three same-frame Lightroom exports at Sharpness 0 / 40 / 80.

Everything else that used to sit here has shipped: the style-retrieval
expansion (finished exports as a look library, the SigLIP 2 text tower, local
Qwen3-VL descriptions, the GUI embedding switch and the Direction-adherence
axis) landed across steps 14 and S1–S3; the eased linear-gradient falloff — the
C1 Hermite smoothstep, RMS 0.0045 against 0.017 for a straight ramp on its
first measurement — shipped in v1.2.0, and v1.2.4 moved its abscissa onto
`t^1.124` against Lightroom's own 46 exports (α rms 0.0064; 0.0315 for the
plain smoothstep, 0.0598 for a straight ramp); and v1.2.4 closed the last two
entries: the colour-range producer (the reverse-fit's second range family:
one mask keyed to each ACR hue band's own mean colour, written as the
colour range mask Lightroom itself writes) and a Linux x64 command-line archive built and
published from the tag beside the Windows and macOS assets.

## How it works

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-dark.svg" />
  <img src="docs/images/architecture-light.svg" alt="AutoShade architecture: three front ends over one Rust library with the style index, reverse-fit, local producers and the local-field analyzer; six local Python sidecars for embeddings, descriptions, correspondence, segmentation and two denoisers; opt-in external AI services" />
</picture>

<sub>Twenty-one components, twenty connections and three boundaries, generated from
[autoshade.architecture.json](docs/architecture/autoshade.architecture.json) by
[scripts/architecture_diagram.py](scripts/architecture_diagram.py): no position
in the picture is chosen by hand, and the shared checker in
[scripts/diagram_check.py](scripts/diagram_check.py) refuses to write the file
when any two labels, borders or arrows touch. Zoom and pan it at
[autoshade.dev/architecture.html](https://autoshade.dev/architecture.html).</sub>

- [`src/decode.rs`](src/decode.rs) decodes the RAW into a preview, EXIF and a
  histogram; the advisor in [`src/advisor/`](src/advisor/) turns those into an
  `EditRecipe` ([`src/recipe.rs`](src/recipe.rs)), and a verifier that
  receives recipe, EXIF, histogram and clipping data — never pixels — checks
  it.
- [`src/render.rs`](src/render.rs) applies it; the image, the recipe and a
  Lightroom-readable sidecar ([`src/xmp.rs`](src/xmp.rs)) go to the per-user
  develop store, and local masks, style retrieval, reverse-fit and the
  generative tools hang off that path unchanged.
- `EditRecipe` is the **only** channel between the AI and the pixels: strict
  `json_schema`, every control bounded and clamped on entry, missing fields
  defaulted so older recipes stay readable, one struct behind GUI, CLI, web UI
  and the XMP projection.
- The renderer is a deterministic f32 pipeline, so the same recipe on the same
  RAW yields the same bytes every run; and the XMP writer edits only the
  fields it owns, so a Lightroom catalogue survives a round trip.

Details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Measured numbers

Every figure is reproduced from the section that owns it; none is an estimate.
Sources are the pinned claims in [docs/TECH_STACK.md](docs/TECH_STACK.md) and
the tests [`scripts/check_docs.py`](scripts/check_docs.py) re-derives.

| What | Measured | Where |
|---|---|---|
| Automated test battery | 1763 library / 25 CLI / 223 GUI / 2+2 contract tests; `check_docs` re-derives the pinned release claims | [Tech stack](#tech-stack-algorithms-and-design-philosophy) |
| RAW coverage | 24 extensions, 725 camera bodies; nine-camera format zoo 9/9 at the last release gate | [Supported formats](#supported-formats) |
| Lightroom Texture parity | 45 of 45 period/depth anchors within ±0.02 | [Develop pipeline](#develop-pipeline-and-tone-model) |
| Radial mask closure | 41 of 41 measured vectors within ≤1 px | [Lens correction](#lens-correction-and-lightroom-mask-frame-laws) |
| Linear mask closure (openly not pixel-closed) | RMS 9.748 / 7.025 / 6.336 px with lens correction on, 12.449 / 9.943 / 4.979 px off | [Lens correction](#lens-correction-and-lightroom-mask-frame-laws) |
| Linear falloff vs Lightroom coverage (46-export pack) | smoothstep on `t^1.124`: α rms 0.0064 against 0.0315 for the plain smoothstep and 0.0598 for a straight ramp; half-coverage contour +34.2/+38.2 px → +0.9/+5.0 px | [Lens correction](#lens-correction-and-lightroom-mask-frame-laws) |
| Radial boundary (46-export pack) | a pure 0.99876 scale of the stored ellipse (sd 4×10⁻⁵ over masks 0.30/0.50/0.70 of frame): −1.12/−1.96/−2.79 px, no dilation law | [Lens correction](#lens-correction-and-lightroom-mask-frame-laws) |
| Roundness (tilted 2:1 ellipse, feather 25/50/75) | Lightroom's R−100/0/+100 exports differ by max\|Δ\| = 0 DN over 26 Mpx; the engine draws one ellipse too | [Masks](#masks) |
| Brush geometry | D1 error 874 px → 9.8 px after pixel-centre sampling and the pixel/aspect metric | [Masks](#masks) |
| X-Trans demosaic (approximate) | X-S10 G/R ratio 1.5503 → 0.9476 | [RAW decode](#raw-decode-and-cfa) |
| RAW denoise, measured noise field (star frame) | per-tile residual max 0.508 → 0.070; tile-to-tile width of the fine-luminance ratio 0.185 → 0.014 (Lightroom's own 0.011); ground-truth bench moved ≤ 0.01 dB | [What is new §11](#11-the-raw-denoiser-is-trained-here-and-judged-by-pass-marks-written-before-the-run) |
| Faint stars through the denoiser (stars of known brightness added to one green plane of the star frame) | light kept at 4σ / 6σ / 10σ: first training run 0.105 / 0.420 / 0.726 → the run that ships 0.541 / 0.835 / 0.918; pass marks written before the run | [What is new §11](#11-the-raw-denoiser-is-trained-here-and-judged-by-pass-marks-written-before-the-run) |
| Star-field test, v1.6.0 build, against Lightroom Denoise 50 on the same frame | denoiser readings 6 of 8 pass: 98.70 % of 17,817 real faint stars kept against 98.80 %; bright-star peak 0.939 against 0.953 (behind); four-plane flux spread 0.0385 against 0.02 (behind); finished develop 0.2922 against 0.2701 (±0.03) and 0.0139 against 0.0278 | [What is new §11](#11-the-raw-denoiser-is-trained-here-and-judged-by-pass-marks-written-before-the-run) |
| Hot-pixel map (ten 61 MP frames) | 86 / 20 / 77 sites inside the picture on three night frames, none on the fourth; 0 / 0 / 0 / 6 / 0 / 4 on ordinary frames; about 0.1 s a frame | [What is new §11](#11-the-raw-denoiser-is-trained-here-and-judged-by-pass-marks-written-before-the-run) |
| Camera base look, v1.6.0 estimator (star frame, 8-bit levels) | develop against the camera's rendition 0.75 rms (median +0.19), 4.24 (+2.56) before; 4 knots instead of 13; finished-develop readings 0.3075 → 0.2922 and 0.0387 → 0.0139 | [What is new §12](#12-the-cameras-own-look-is-read-from-the-picture-like-with-like) |
| Reverse-fit, stone viaduct (full solve, Reverse-fit strength 100 %) | look error 0.161 → 0.023 at confidence 0.63 (a global solve with the cast curves projected to t = 0.485, the per-band mixer on Orange/Yellow/Aqua/Blue at the 45 ceiling, two semantic zones, two boundary-gated tiles and one field mask), D = 0.180; at the default 65 % the pair fits to 0.047 at confidence 0.25 with the mixer capped at 18, four tiles and two field masks, and v1.2.2's fit of it is where the seam fix was measured: sky tile 0.0278 → 0.0042 (k 0.121), delivered +3.15 → +0.92 codes | [What is new §1](#1-whole-image-ai-generation-then-reverse-fit-an-editable-recipe-from-any-finished-look) |
| Reverse-fit, Cornwall islet (full solve, composed calibration) | look error 0.137 → 0.027 at confidence 0.66, D = 0.136 sized from the sensor frame (0.304 from the cropped preview); the global cast projected to t = 0.363, delivered sky hue spread 9.6° (v1.2.2 shipped 33.1°) | [docs/SHOWCASE.md](docs/SHOWCASE.md) |
| Reverse-fit, desert canyon at dusk (full solve, Reverse-fit strength 85 %; the v1.3.0/v1.3.1 reference pair) | look error 0.110 → 0.048 at confidence 0.25, D = 0.275 at pixel scale and 0.609 at layout scale (sky zone 0.617); on the 2048 px acceptance render, whole-frame mean \|diff\| against the target 0.0276 (v1.2.6: 0.0571), sky ΔE 18.2 → 4.9, land 7.0 → 6.9; a solved white balance, two Select Sky bands, four boundary-gated tiles and the 12×8×8 colour field; `match --zoned` 5 min 29 s | [docs/SHOWCASE.md](docs/SHOWCASE.md) |
| Local-field ceiling, calibration pair | global fit 0.0961 against a ceiling of 0.0700; the accepted sky zone realizes 0.134 of the distance | [What is new §6](#6-a-bilateral-grid-local-field-prices-every-local-producer-first) |
| AI develop, model judge | 2026-09-02 four-looks batch on the full 169 + 94 index at `--style 1.0 --strength 0.9`, the direction leading: moody 68 → 70 → 78 (both adopted) → 69 (discarded), verdict Accept; golden 87 → 84 (discarded) after the verifier twice sent the proposal back for the grain it never set, verdict Revise — unsaved, the figure renders the proposal; vivid 70 → 84 (adopted) → 82 (discarded), verdict Accept. The finished-look-only run (2026-09-01) and v1.2.2's full-index run are on the showcase page | [AI advisor](#ai-advisor-and-reverse-fit) |
| Style retrieval weights | corpus harness (169 described exemplars, 156 queries): `W_EMB=4`, `W_TXT=0.5`, `W_DESC=0.5`, standardised variant with the text-hubness correction — MAE 0.688864 vs baseline 0.713143, +0.024280, CI [+0.005837, +0.041111] under the prose proxy; the corrected point at the old `W_TXT=4` regresses with CI [−0.069654, −0.005140], which is why the weight moved; under the tag-string proxy nothing beats the text-free row; `W_LOOK=1.0` is unmeasured (the harness cannot see the look library) and its scale is a real ratio against the direction terms — it ships inside a stable band, order unchanged to 2x and first moving at 4x | [AI advisor](#ai-advisor-and-reverse-fit) |
| Memory budget | 1800 MB per photo from a 1771 MB reference probe; 4 GiB RAW admission gate | [Application](#application-and-infrastructure) |

## Install and quickstart

### Download a release

The v1.6.1 release is built by GitHub Actions from the tag: the Windows front
ends, two macOS universal (arm64 + x86_64) archives and a Linux x64
command-line archive; `checksums.txt` carries the SHA-256 of every asset.
One file the app uses is not a build product: `autoshade-raw-denoise-v2.pth`,
the trained RAW denoiser, is uploaded by hand to the release the sidecar's pin
names —
[`releases/download/v1.6.0/autoshade-raw-denoise-v2.pth`](https://github.com/skymanbp/autoshade/releases/download/v1.6.0/autoshade-raw-denoise-v2.pth)
— and a byte-exact copy of it on Hugging Face
([`Azng0/autoshade-mirror-autoshade-raw-denoise`](https://huggingface.co/Azng0/autoshade-mirror-autoshade-raw-denoise))
is tried first.
The AI denoise sidecar fetches it on demand and refuses it unless its SHA-256
and byte count match the values pinned in `python/denoise_raw.py`, so nothing is
unpickled on trust.

| File | Size | SHA-256 |
|---|---:|---|
| `autoshade.exe` (CLI) | 23,029,760 bytes | `b5cb80311a75851ed6366a6ad0842d3607ef515403348a6eed1b4dee69d3bf1b` |
| `autoshade-gui.exe` (desktop app) | 29,476,864 bytes | `94eb8fb6442468b5ff62f93fc2f2dcc3f02b59bb5745393408c9b4d8d2dde98d` |
| `AutoShade-Setup-1.6.1.exe` (installer) | 15,354,888 bytes | `27272215ea96f65c7331ced97b4debf855b916a1feea160d6088fcc4bed60dcc` |
| `autoshade-1.6.1-windows-x64.zip` (portable archive) | 20,682,532 bytes | `417b53926c289feb663dce7a9215bff179dc521efc92eb730673053e7d470a26` |
| `AutoShade-1.6.1-macos-universal.zip` (macOS app bundle) | 41,576,016 bytes | `0a70e1a89b5bd387e5b23490aa7d3fbde2d60204e4a8a00949765e75b09143d4` |
| `AutoShade-1.6.1-linux-x64.zip` (Linux command line only) | 10,004,995 bytes | `b736e91cfef2298aaf6915d259d2c1b066110fca5a6af0a126de2fa184a89cfa` |
| `AutoShade-1.6.1-macos-cli.zip` (macOS command line only) | 18,100,420 bytes | `b8de825378ccdf2400ec387c5172097eaaae8b6f1918219eec11d5c5bea17760` |
| `autoshade-raw-denoise-v2.pth` (RAW denoiser weights, fetched on demand from the v1.6.0 release) | 130,590,559 bytes | `ffafa40a53f52092149db2fcf03636117ad6855e1068142d4f6b03b634e9f9c4` |

Download from the
[v1.6.1 release page](https://github.com/skymanbp/autoshade/releases/tag/v1.6.1):

\
- **Installer (recommended):** run `AutoShade-Setup-1.6.1.exe`. It installs for
  the current user without administrator access, adds Start Menu shortcuts, and
  offers optional desktop and user `PATH` tasks.
- **Upgrading is in place.** Run a newer installer over an existing install and
  it stays the same install: same directory, one entry in Programs and
  Features, one `PATH` entry, shortcuts replaced rather than duplicated, and
  your develop store and downloaded model weights left exactly as they were.
  A running AutoShade is closed for you first. An OLDER installer is refused
  and names both versions when it refuses. Upgrading over a pre-rename install
  also deletes the executables, icon and fonts that carried the old name.
- **Uninstalling has two doors** — the Programs and Features entry, and
  「Uninstall AutoShade」 in the Start Menu group. Either one asks whether
  to delete the two things it never installed: the downloaded model
  weights and the develop store in `%LOCALAPPDATA%\autoshade`. It names the
  size it found for each, and keeping both is the default, so a later install
  starts where you left off.
- **Silently, for a scripted rollout:** `AutoShade-Setup-1.6.1.exe /VERYSILENT
  /SUPPRESSMSGBOXES /NORESTART` installs or upgrades with no window and no
  prompt, and `unins000.exe /VERYSILENT /SUPPRESSMSGBOXES` in the install
  directory uninstalls the same way. The silent uninstall keeps your weights
  and develop store unless you add `/DELETEDATA=1`.
- **Portable archive:** extract `autoshade-1.6.1-windows-x64.zip` to a directory
  you can keep intact and run either executable from there, beside the bundled
  `assets/` and `python/` sidecars.

#### macOS

Both macOS archives are universal (Apple silicon and Intel in one binary);
unpack either with Finder or `ditto -x -k <zip> <dir>`.

- `AutoShade-1.6.1-macos-universal.zip` is the app: move `AutoShade.app` to
  `/Applications`. The command line travels inside it
  (`AutoShade.app/Contents/MacOS/autoshade`), so this download alone serves a
  terminal user; `AutoShade-1.6.1-macos-cli.zip` is that binary alone.
- The bundle is **ad-hoc signed, not notarised**, so the first launch is
  refused: macOS reports that the developer cannot be verified. Clearing it is
  per machine, not per launch — **System Settings → Privacy & Security → Open
  Anyway**, or right-click in Finder and choose **Open**.
- **Python 3** is needed for the AI sidecars only, and **model weights**
  download on first use into the develop store, not the signed read-only
  bundle; the interpreter is a Settings field with **Detect**
  ([manual](docs/USER_MANUAL.md#configure-and-use-the-ai-features)).

The Linux archive, `AutoShade-1.6.1-linux-x64.zip`, is the command line for
x86-64 Linux, built on Ubuntu 22.04 with the same payload as the macOS
command-line archive: the binary, the Python sidecars without their weights,
the assets, LICENSE and README. Unpack it anywhere and run `./autoshade`;
there is no Linux desktop app.

### Build from source

AutoShade uses Rust edition 2024 and rustc/cargo 1.94.

```bash
cargo build --release
cargo build --release --features gui --bin autoshade-gui
```

The first builds the CLI, the second the desktop app, whose dependencies stay
behind the `gui` feature. The local AI tools also need Python packages
(weights download on first use and are not committed): **BiRefNet**
`pip install torchvision timm einops` against a `torchvision` matched to
`torch`; **U²-Net fallback** `pip install rembg`; **OneFormer sky and SAM
2.1** `pip install transformers torch`; **AI denoise**
([`python/denoise_raw.py`](python/denoise_raw.py) on the RAW sensor mosaic,
[`python/denoise.py`](python/denoise.py) on baked sources) a `torch` build plus
OpenCV, NumPy, einops and requests — under CUDA:

```bash
pip install torch --index-url https://download.pytorch.org/whl/cu128
pip install opencv-python numpy einops requests
```

### First run: desktop app

1. Start `autoshade-gui`.
2. Choose **Open photo…** (`Ctrl+O`), drag a photo in, or use **Open
   folder…**.
3. Move a Develop slider and compare it with the neutral conversion.
4. Press `Ctrl+Shift+E` to open Export, choose a destination and format, then
   export a copy. The original remains untouched.

### First run: CLI

Decode a preview and metadata, then make a manual recipe render:

```text
autoshade decode "photo.ARW" -o "preview.jpg"
autoshade apply "photo.ARW" "recipe.json" -o "developed.tif"
```

With the image/vision role configured, an end-to-end AI develop is:

```text
autoshade auto "photo.ARW" --guidance "natural color; protect highlights" -o "developed.tif"
```

## User manual

The full manual is [docs/USER_MANUAL.md](docs/USER_MANUAL.md) — the Develop
panel and its Save/XMP rules, local masks, versions and variants with the
Reverse-fit walkthrough, export, the CLI reference, Lightroom/XMP
interoperability, the AI roles and the privacy boundary. The essentials:

- The source library is read-only; develops, XMP projections and versions live
  in the develop store, and **Export .xmp beside the photo** is the separate,
  confirmed exception.
- Manual develop, `apply`, local `match`, XMP, masks, AI denoise, style
  indexing and the local AI masks need no API key; `analyze`/`auto`,
  `match --style-prompt`/`--ai-judge`/`--deep`, `reimagine`/`retouch` and
  automatic `heal` detection use the configured role, and the verifier gets
  data, never pixels.
- **Settings** or `OPENAI_API_KEY` / `AUTOSHADE_ANALYSIS_API_KEY` configure
  the roles; **`AUTOSHADE_PYTHON`** names the sidecar interpreter and
  **`AUTOSHADE_WEIGHTS_DIR`** moves the weight cache all five share. Those
  come only from the environment or the per-user settings file — a
  `./autoshade.local.json` beside your photos may select model and provider
  preferences and nothing else.

## Supported formats

<table>
<tr>
<td align="center"><img src="docs/images/formats/cr2.jpg" alt="Canon CR2 develop" /><br /><sub><b>.cr2</b> · Canon EOS 40D</sub></td>
<td align="center"><img src="docs/images/formats/cr3.jpg" alt="Canon CR3 develop" /><br /><sub><b>.cr3</b> · Canon EOS R6</sub></td>
<td align="center"><img src="docs/images/formats/nef.jpg" alt="Nikon NEF develop" /><br /><sub><b>.nef</b> · Nikon D700</sub></td>
</tr>
<tr>
<td align="center"><img src="docs/images/formats/arw.jpg" alt="Sony ARW develop" /><br /><sub><b>.arw</b> · Sony α7 III</sub></td>
<td align="center"><img src="docs/images/formats/orf.jpg" alt="Olympus ORF develop" /><br /><sub><b>.orf</b> · Olympus E-M5</sub></td>
<td align="center"><img src="docs/images/formats/rw2.jpg" alt="Panasonic RW2 develop" /><br /><sub><b>.rw2</b> · Panasonic DMC-GX85</sub></td>
</tr>
<tr>
<td align="center"><img src="docs/images/formats/pef.jpg" alt="Pentax PEF develop" /><br /><sub><b>.pef</b> · Pentax K-5</sub></td>
<td align="center"><img src="docs/images/formats/dng.jpg" alt="Ricoh DNG develop" /><br /><sub><b>.dng</b> · Ricoh GR II</sub></td>
<td align="center"><img src="docs/images/formats/raf.jpg" alt="Fujifilm RAF X-Trans develop" /><br /><sub><b>.raf</b> · Fujifilm X-S10 — X-Trans, approximate</sub></td>
</tr>
</table>

This grid is also the nine-camera RAW zoo: one real CC0 file per format tile,
fully decoded and neutral-rendered rather than copied from an embedded
preview. The corpus cannot ship here, so the suite is environment-gated; the
last recorded release gate was 9/9.

**Camera RAW — 24 extensions**, one predicate app-wide (`decode::is_raw`):

```text
arw, dng, raw, raf, nef, cr2, cr3, orf, rw2, pef, srw, 3fr,
fff, iiq, mef, mos, erf, kdc, dcr, dcs, crw, nrw, mrw, ari
```

Decoding is rawler 0.7.2, which carries **725 camera models**. **No embedded
preview:** 12 of the 24 formats store none. They are `orf`, `srw`, `nrw`, `mef`,
`mos`, `kdc`, `dcr`, `dcs`, `erf`, `iiq`, `crw`, and `ari`; AutoShade shows its
own neutral rendition instead and says so.

**Baked rasters — 8 extensions:** `jpg`, `jpeg`, `png`, `tif`, `tiff`, `bmp`,
`webp`, `gif`. ICC profiles on baked imports are converted through qcms when
present.

Degradation and refusal are explicit: an untagged 16-bit baked image is read
as sRGB and flagged; monochrome and four-colour arrays are refused; unknown
make, unknown model and no matching decoder are differentiated and point at
the DNG route; and a parser panic is a named per-file error, so one bad file
cannot end a batch.

## Tech stack, algorithms, and design philosophy

### Design philosophy

- **The AI decides what to change; the engine does it** — a bounded recipe
  with its rationale and confidence, one deterministic renderer behind every
  front end.
- **Measured, not assumed** — rendering laws are fitted to Lightroom and
  camera measurements and quoted with residuals; release claims are re-derived
  by a script.
- **Non-destructive, interoperable, local first** — the source library stays
  read-only, develops live in a per-user store, and sidecars are merged so a
  Lightroom catalogue survives.
- **Six local sidecars** — segmentation, two denoisers (one on the RAW sensor
  mosaic, one on baked pixels), correspondence, look descriptions and style
  embeddings run on the machine; pixels leave it only for an AI operation you
  ask for.
- **Generated pixels are labelled** — reimagine, GUI adjust, retouch, heal
  and denoise are opt-in exceptions on their own cards, and known weaknesses
  are honesty markers, not caption polish.

### Implementation

The canonical page is **[Tech stack and algorithms](docs/TECH_STACK.md)** —
equations, provenance, measured results, honesty markers and source paths
behind each summary below. Numbers already in [Measured
numbers](#measured-numbers) are not repeated.

### RAW decode and CFA

- `src/decode.rs` uses rawler for **RAW decode, 24 formats**, with 725 bodies
  in the release database; `orient_f32` applies the composed orientation — the
  RAW's EXIF state plus the photographer's own quarter turns — at the head of
  the chain, and an imported Lightroom sidecar's `tiff:Orientation` chooses
  those turns, so a rotation made in Lightroom survives the import.
- Bayer data takes rawler's demosaic path; X-Trans uses an **approximate** 5×5
  CFA-geometry plane fit, and no-preview RAWs, untagged 16-bit rasters and
  mono sensors are disclosed or refused.
- `src/dcp.rs` and `src/render/profile.rs` develop a Lightroom photo through
  the **camera profile it names** — the `.dcp` Adobe installed, plus the
  creative profile in the sidecar's `crs:Look`. Table semantics and memory
  order were measured on the installed pool rather than assumed, and the one
  half that does not decode (a Look's creative colour table) is named on screen
  instead of being silently dropped. Nothing Adobe ships is redistributed.
- `src/denoise.rs` and `python/denoise_raw.py` clean a Bayer RAW's mosaic
  before demosaic with the weights fine-tuned here, after its hot pixels are
  mapped and its noise measured tile by tile; `src/render.rs` returns
  `1 − strength` of the luminance grain in linear light (§11).

### Develop pipeline and tone model

- `src/render.rs` is a deterministic f32 pipeline: linear-light vignette and
  dehaze, a monotone Fritsch–Carlson tone LUT with `tone_knot_weights` and
  Highlights inside it, then RGB curves, HSL, colour grade, clarity/Texture,
  saturation, NR, sharpening and local edits.
- Negative Texture is two measured parallel low-pass arms (`A1=0.172443`,
  `A2=0.304888`) with a calibrated hyperbolic depth law.
- `render::camera_base_look` estimates the per-photo base curve from the
  RAW's embedded preview on 64-column block means, paired like with like
  (the v1.6.0 estimator, §12); a RAW that carries no sharpening amount renders
  at Lightroom's default of 40, a baked raster at 0.

### Masks

- `src/recipe.rs`, `src/render.rs` and `src/xmp.rs` implement radial, linear,
  brush, bitmap, luminance-range and colour-range masks with ordered
  Add/Subtract/Intersect composition in both the engine and the Lightroom
  sidecar for every spellable component. Lightroom's own feathered
  intersection rendering remains unmeasured.
- Radial feather is a measured 290×11 `alpha(rho, feather)` LUT; brush dabs
  use `(1-rho^m)^n` and the measured `kappa=0.1284` flow law over pixel-centre
  sampling and the pixel/aspect metric, and `MaskBrushTable` import validates
  MD5→`.acr`→Brotli.

### AI masks

- `src/segment.rs` and `python/segment.py` run commit-pinned BiRefNet subject
  selection with a named U²-Net fallback, OneFormer ADE20K sky selection
  through the 150-class checked-in table, and SAM 2.1 objects from ordered
  gesture points over the `gp1` IPC.
- Provenance-keyed caches include the backend generation and exact prompt
  points, so a fallback alpha is re-derived once the pinned backend arrives;
  these are local re-creations, not Adobe-computed mask pixels.

### Lens correction and Lightroom mask-frame laws

- `src/lensmeta.rs`, `src/lcp.rs` and `src/render.rs` combine Sony 0x7037's 16
  native `(i+1)/16` samples, a 2048-node/64-knot mask solve, and guarded
  Newton inversion for rectilinear `.lcp` profiles while refusing fisheye-only
  entries.
- Radials use exact-once `m_lr^-1 ∘ T_engine` transport; linear H2 keeps
  corrected-frame handles but is openly not pixel-closed, and brushes remain
  in the raw frame.

### XMP and Lightroom interoperability

- [`src/xmp.rs`](src/xmp.rs) uses scoped, typed XML traversal, including
  nested `Look`, and conservatively merges owned edits while preserving
  unmodeled fields; Save writes the develop store and beside-RAW export is
  explicit.
- `LR_MASK_FRAME_SCALE=1.0`, `LocalExposure2012=EV/4`, local Hue is
  `degrees/180`, the other measured local family is `/100`, global Sharpness
  is 1:1, and polarity comes from `MaskInverted` rather than `Flipped`.
- **One inversion, composed in one place.** A recipe spells a mask's polarity
  twice — the correction's own Invert flag and the geometry's own bit (a
  radial's `Flip`, a brush group's or an AI mask's `MaskInverted`) — and
  `LocalAdjustment::net_inverted` is the only place the two meet. The render,
  the sidecar writer, the mask-habit classifier and the GUI overlay all read
  that one helper, so an inverted brush or AI selection leaves for Lightroom
  as the half it really covers, and a Lightroom mask that arrives inverted
  renders inverted.

### AI advisor and reverse fit

- `src/advisor/` validates AI proposals into bounded recipes, keeps Responses
  at `store:false`, gives the verifier data rather than pixels, and adopts a
  guided revision only when it does not lower the score.
- `src/style.rs` retrieves z-scored RAW+XMP exemplars with four optional
  cosine terms (image, direction text, description text, and the separate
  finished-photo look library); the shipped weights are `W_EMB = 4`,
  `W_TXT = 0.5` and `W_DESC = 0.5` from the calibration harness, plus
  `W_LOOK = 1.0`, the one term that harness cannot score.
- `src/fit.rs` runs the luminance-CDF, exposure, basis, tone, saturation and
  cast inverse stages behind a >45°/≥5% foreign-hue veto, consulting the DIFT
  (SD 2.1) field of `src/correspond.rs` + `python/correspond.py` on divergent
  pairs; `src/generative.rs` negotiates gpt-image-2 sizes and `src/retouch.rs`
  is the deterministic heal.

### Application and infrastructure

- Rust (rustc/cargo **1.94**, edition 2024) · rawler (RAW decode, 24 formats /
  725 bodies) · `image`, qcms, rayon, clap, serde, ureq, `eframe`/egui and
  `tiny_http` back the shared library, CLI, desktop GUI and loopback web UI.
- The server uses a 32-byte token plus Host/Origin/no-store defenses; the GUI
  keeps variants, versions and a deleted-version registry; a denoise's success
  requires the typed `sidecar_wrote` contract; a 1771 MB reference probe sets
  the 1800 MB per-photo budget, and a 4 GiB RAW gate bounds admission.
- The [`build` workflow](.github/workflows/build.yml) covers default and GUI
  feature sets on Ubuntu and macOS; model weights are not stored here. The
  current battery is **1763 library (1748 pass + 15 `#[ignore]`d forensic probes) / 25 CLI / 223 GUI / 2+2 contract** tests, and
  [`scripts/check_docs.py`](scripts/check_docs.py) re-derives the pinned
  release claims.

## Status, roadmap, and known limitations

- Release gates for v1.6.1 cover the CLI, desktop GUI, sidecar contracts,
  format fixtures and the deterministic renderer; artifact sizes and hashes
  are above.
- macOS has shipped binaries and an app since v1.2.0 and nobody has reported
  using them interactively: CI is the whole of the evidence. Apple-silicon
  Metal/MPS is measured on every release run by `scripts/mps_probe.py`
  (device, forward time, peak memory, whether `deform_conv2d` falls back to
  the CPU — the numbers are in the release run's `macos-battery` job log); Linux ships a
  command-line archive and has no desktop app.
- Honesty markers: the approximate X-Trans path, locally re-derived rather
  than Adobe-identical AI masks, measured-but-not-bit-exact Lightroom parity,
  lossy reimagine targets, and a LINEAR mask frame that is not pixel-closed
  while RADIAL closes 41/41 vectors to ≤1 px.
- Older recipes stay readable; a v1.0.0 recipe carrying the new `LensProfile`
  frame facts is refused by older binaries rather than misread, and six
  families of existing content may rerender — both in
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), with the ledger and standing
  rulings in [docs/ROADMAP.md](docs/ROADMAP.md).

## License and acknowledgements

**AutoShade is MIT-licensed** — see [LICENSE](LICENSE).

### RAW format samples

The nine files behind the format grid come from
[raw.pixls.us](https://raw.pixls.us/) under CC0 1.0 Public Domain; their
recorded SHA-256 values were verified against that index before use.

| Format | Camera | MP | Sample |
|---|---|---:|---|
| CR2 | Canon EOS 40D | 10.08 | `RAW (3:2)` |
| CR3 | Canon EOS R6 | 19.96 | `3:2` |
| NEF | Nikon D700 | 12.2 | `14bit compressed (Lossless) (3:2)` |
| RAF | Fujifilm X-S10 | 26.7 | `14bit compressed (3:2)` |
| ORF | Olympus E-M5 | 16.11 | `16bit (4:3)` |
| RW2 | Panasonic DMC-GX85 | 15.9 | `4:3` |
| PEF | Pentax K-5 | 16.39 | `14bit (3:2)` |
| DNG | Ricoh GR II | 16.27 | `12bit (3:2)` |
| ARW | Sony ILCE-7M3 | 24.34 | `14bit compressed (3:2)` |

### Showcase photographs

The showcase photographs are the author's own Sony α7R IVA frames — © 2026
skymanbp, all rights reserved. They document AutoShade's output, are not
covered by the MIT license, omit EXIF and carry no watermark.

### Fonts and model weights

The GUI bundles subset Noto faces under the SIL Open Font License (texts under
`assets/fonts/`); model weights download separately and remain their authors'
property. The one exception is `autoshade-raw-denoise-v2.pth`, this project's
own fine-tune of DPIR's architecture, which ships as a release asset under this
project's licence with its training sources credited below (and, since v1.6.0,
has a copy of ours on Hugging Face like every other pinned download).

Every pinned model also has a **byte-exact copy of ours** on Hugging Face
(`Azng0/autoshade-mirror-*`), which the sidecars try before the upstream host.
A pinned revision is what makes a download verifiable and also what makes a
vanished upstream unrecoverable — nothing else is that revision — so the copy
exists to keep a cold cache installable years from now. Hosting it makes this
project a redistributor: each mirror carries the upstream licence declaration
unchanged, and whatever that licence permits or restricts applies to the copy.
The checksum decides in either case, so a mirror is a second host and never a
second source of truth. The table is [`python/_mirror.py`](python/_mirror.py).

| Model | Purpose | License |
|---|---|---|
| SCUNet | AI denoise (baked sources) | Apache-2.0 |
| DRUNet-colour architecture (DPIR); the weights are fine-tuned here and shipped as `autoshade-raw-denoise-v2.pth` | AI denoise (RAW sensor mosaic) | MIT (architecture and this project's weights); fine-tuning pairs from RawNIND, CC BY-SA 4.0 |
| BiRefNet | Subject segmentation | MIT |
| U²-Net | Subject fallback | Apache-2.0 |
| OneFormer ADE20K | Sky segmentation | MIT |
| SAM 2.1 | Point-prompted object masks | Apache-2.0 |
| SigLIP 2 | Optional style embeddings | Apache-2.0 |
| Qwen3-VL-2B-Instruct | Optional local look descriptions | Apache-2.0 |
| Stable Diffusion 2.1 | DIFT correspondence field; generative fill | CreativeML Open RAIL++-M (use-based restrictions travel with the weights) |

The project acknowledges the rawler, image, qcms, rayon, clap, serde, ureq,
egui/eframe, tiny_http and local-model communities whose work makes these
pipelines possible.