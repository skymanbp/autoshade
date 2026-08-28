<div align="center">
<img src="assets/icon.png" width="104" alt="Autoshop icon" />

# Autoshop

**AI-assisted automatic development of RAW photographs.**

An AI decides *what to change*. A deterministic Rust engine *does* it.
**In the recipe-development path, the AI never touches a pixel.**

[Download v1.0.0](https://github.com/skymanbp/autoshop/releases/tag/v1.0.0) ·
[Architecture](docs/ARCHITECTURE.md) ·
[Roadmap](docs/ROADMAP.md) ·
[MIT](LICENSE)

</div>

---

## What Autoshop is

Autoshop is a non-destructive photo developer for RAW and baked images. Its
main workflow turns an AI proposal into a small, inspectable `EditRecipe`, then
applies that recipe with the same local Rust renderer used by the desktop app,
CLI, and embedded web UI. Generative tools are separate, opt-in paths and are
labelled as such.

Who it is for: photographers who want an AI first pass on a card of RAWs
without giving up an editable, Lightroom-compatible develop; and anyone who
wants to know *what* an AI changed, in numbers, before trusting it. The rule
that shapes every part of the program: **the AI decides what to change, the
deterministic engine does it**. Every AI proposal is a small JSON recipe with
bounded controls, a written rationale, and a confidence — the same recipe a
person can edit by hand, replay a year later, or hand to Lightroom.

## Contents

- [What Autoshop is](#what-autoshop-is)
- [What it does](#what-it-does)
- [What is new here](#what-is-new-here)
- [How it works](#how-it-works)
- [Results: two batches, six frames](#results-two-batches-six-frames)
- [Measured numbers](#measured-numbers)
- [Install and quickstart](#install-and-quickstart)
- [User manual](#user-manual)
- [Supported formats](#supported-formats)
- [Tech stack, algorithms, and design philosophy](#tech-stack-algorithms-and-design-philosophy)
- [Status, roadmap, and known limitations](#status-roadmap-and-known-limitations)
- [License and acknowledgements](#license-and-acknowledgements)

## What it does

- **Feature 1 — AI develop.** `analyze`, `auto`, or the GUI's **Analyze**: a
  vision advisor turns the preview, EXIF, and histogram into an editable
  recipe (crop, tone, white balance, curves, HSL, colour grading, texture,
  clarity, dehaze, detail, and parametric or bitmap local masks); a data-only
  verifier checks the proposal against the image statistics; the engine renders
  it; one bounded visual-review revision may follow. Guidance text steers the
  proposal and a Strength control bounds how far it may move.
- **Feature 2 — A deterministic develop engine.** Exposure, white balance,
  tonal controls, RGB point curves, HSL, colour grading, texture, clarity,
  dehaze, noise reduction, sharpening, vignette, crop, and lens correction, with
  linear, radial, brush, bitmap, luminance-range, and colour-range masks
  combined by Add, Subtract, or Intersect. The GUI, the CLI's `apply`, and the
  web UI render through the same code; there is no hidden GUI-only look.
- **Feature 3 — Local AI masks.** Subject (BiRefNet, with a named U²-Net
  fallback), sky (OneFormer ADE20K), and point-prompted object (SAM 2.1)
  selection run as local Python sidecars with pinned weights and need no API
  key.
- **Feature 4 — Lightroom/ACR interoperability.** Sidecar XMP is read as the
  merge base and written back with unmodeled fields preserved byte for byte;
  Lightroom brush dab streams are imported; the beside-RAW export is a
  separate, confirmed action.
- **Feature 5 — Style read.** Index your own Lightroom RAW+XMP pairs and let
  the advisor retrieve similar prior edits as soft references. Retrieval steers
  the proposal; nothing is copied pixel for pixel.
- **Feature 6 — Reverse-fit.** `match` or the GUI's **Reverse-fit** estimates an
  engine recipe from any target look — a generated image, someone else's
  render, a reference frame — measures how far the target's *content* has
  diverged before deciding how much to trust it, then fits global, zoned, and
  luminance-range corrections behind evidence gates. The recovered recipe
  applies deterministically to the original full-resolution RAW.
- **Feature 7 — Generative and pixel tools, opt-in and labelled.** Reimagine
  (gpt-image-2) creates a lower-resolution target from a prompt; retouch, heal,
  and SCUNet denoise change pixels directly. The UI marks generated pixels as
  generated.
- **Feature 8 — Versions, variants, and three front ends.** Every photo keeps
  Original, AI-generated, and Reverse-fit cards with numbered snapshots in a
  per-user develop store; the desktop GUI, the scriptable CLI, and a small
  loopback web UI all link the same library.

The core develop path never invents scene content. Local SCUNet denoise,
generative reimagine/retouch, and pixel heal are explicit opt-in exceptions;
the UI distinguishes generated pixels from engine-rendered develops.

Out of scope in this release: bit-exact Adobe rendering (parity is measured,
not identical), an exact X-Trans demosaic (the plane fit is approximate),
prebuilt Linux and macOS binaries (CI builds and tests them from source), and
multi-class semantic segmentation.

## What is new here

The ideas below are the ones you will not find in another RAW developer. Every
number is copied from the source or from
[docs/TECH_STACK.md](docs/TECH_STACK.md) / [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md);
the last subsection lists what is designed but not yet shipped, so nothing
here is a promise dressed as a feature.

### 1. Style reference is retrieval over your whole catalogue, not a global average

`autoshop style-index <dir>` (or the GUI's **Style reference library**) walks
your Lightroom RAW+XMP pairs and turns *every finished edit you ever made*
into an exemplar ([`src/style.rs`](src/style.rs)):

- a 14-dimensional photographic feature vector from EXIF and the histogram
  (log/ratio dimensions z-scored, scene-type discriminators weighted 1.5×);
- the 12 develop settings you actually moved (exposure, contrast, highlights,
  shadows, whites, blacks, vibrance, clarity, temperature, tint, saturation,
  dehaze), your master tone-curve shape (black-lift and S-strength) and a
  colour-family summary;
- optionally a 768-dimensional **SigLIP 2** image embedding of the frame
  (`base/16 @384`, Apache-2.0 — CLIP and OpenCLIP were passed over on licence
  grounds), computed by a local sidecar through the same 512-px frame the
  query goes through, so index and query can never disagree.

At develop time the photo's own vector retrieves the **4 most similar past
shots** with the hybrid distance `Σ wᵢ(qᵢ−eᵢ)² + W_EMB·(1−cos(q,e))`
(`W_EMB = 2.0`, retained after a 147-exemplar calibration that scanned
0…8). Their measured settings, curve habit and colour families are rendered
into the advisor's prompt as a *soft reference* — a ceiling below the
committed strength band, a floor at it — and a capped `blend_toward` pull
(≤ 0.6) moves the proposal toward your historical means without ever copying
one. The rationale names the shots it leaned on. It is a retrieval system
sized like one: 5,000 exemplars / 96 MiB, both caps derived from the
measured 12.41 bytes per serialised embedding element rather than guessed.

`match --style-prompt` closes the loop from the other side: given a source
and a finished target of the same frame, the vision role writes a reusable
text style brief that `reimagine` accepts as its Direction.

### 2. One control registry generates the entire AI contract

[`src/advisor/catalogue.rs`](src/advisor/catalogue.rs) lists every develop
control once — range, neutral value, engine-only flag, `crs:` key, purpose —
and everything downstream is *derived* from it: the strict Responses
`json_schema` (both mirrors), the proposer's control catalogue, the eval
ruler, and the style index's reference keys. A field added to `EditRecipe`
without a registry row does not compile, so the AI side and the measuring
side cannot silently drift apart.

### 3. "How hard should the AI push" is one dial wired into six gates

`GradeStrength` (GUI **Strength**, CLI `--strength`) reaches the proposer's
banded restraint prose and its ±Highlights/Shadows guardrail pair, the
recipe's `temper` soft caps (knees scale by `1 + (s − 0.5)·0.7`), the
verifier's too-flat/over-cooked bands, the visual judge's rubric, the style
reference wording, and even the no-key heuristic fallback. `0.50` is the
calibration point every restraint number was tuned at (147-photo eval);
`0.65` ships as default. Six constants that used to disagree now read one
number — which is why turning the dial up cannot be quietly undone by a
verifier that revises it back.

### 4. Three AIs, and none of them can see what the others see

- The **vision advisor** sees the preview and can only answer with bounded
  controls under a strict schema (`store:false`).
- The **verifier** (the signed-in `claude` CLI over OAuth, or any
  OpenAI-compatible chat model) sees the recipe, EXIF, histogram, clipping
  statistics and the advisor's rationale — never a pixel — and a non-Accept
  verdict never writes a develop.
- The **visual judge** sees only two JPEG renders, may buy one guided
  revision, and the revision is adopted only if it re-scores at least as
  high.

The doctrine is structural, not a prompt instruction: in the develop path
there is no code path by which a model output becomes a pixel.

### 5. Reverse-fit is inverse rendering with an honesty budget

`match` recovers an editable recipe from any finished look of the same frame
— a generated image, someone else's render — without copying a pixel
([`src/fit.rs`](src/fit.rs), [`src/fit_zoned.rs`](src/fit_zoned.rs),
[`src/fit_field.rs`](src/fit_field.rs)):

- **Distribution-level, never per-pixel regression.** Luminance CDFs are
  matched at the engine's own tone knots and least-squares solved against
  the engine's own slider basis with a ridge and a model-selection prior;
  saturation closes through real renders; per-channel curves are admitted
  only through three vetoes, one of which refuses any cast that paints a
  hue ≥ 45° from every target family over ≥ 5 % of the frame.
- **A structural-divergence statistic decides how much to believe.** Same
  scene → Full solve. Repainted scene (`D ≥ 0.35`) → bounded Atmosphere mode
  (EV ±1, WB gain [0.80, 1.25], saturation ±30, curve slope [0.5, 1.5],
  confidence capped at 0.50) on a structure-blind ruler that keeps the
  population vetoes.
- **Local corrections are produced by mutually exclusive, evidence-gated
  producers** — semantic sky/land zones from a local OneFormer pass, or
  XMP-native luminance-range bands derived from rank-paired residuals — then
  a frozen-evidence quadtree adds bitmap tiles only when both frames hold
  ≥ 3 % evidence, structure survives, the confidence interval excludes
  zero, the boundary rim stays within 0.012, and the composed frame does not
  regress.
- **A read-only local-field analyzer measures the ceiling first.** A
  12×8×8 bilateral grid of five develop parameters is solved by conjugate
  gradients on the same frozen evidence and reports how much of the
  remaining difference *any* spatially varying develop could reach — on the
  calibration pair the global fit reads 0.0961 against a ceiling of 0.0700,
  and the accepted sky zone realises 0.134 of that distance. The field never
  touches a pixel; it prices the producers, halves the tile budget when the
  remainder is not tile-shaped, and says so in the rationale.
- **Content that moved is matched where it moved to.** On divergent pairs
  the fit consults a DIFT correspondence field (Stable Diffusion 2.1 as a
  featurizer, 48×48 cells, confidence = cyclic consistency × flow
  smoothness) to weight a Full zone's pixel pairs — identity fields are
  conservation-tested to change nothing.

### 6. Lightroom parity is measured, and the residuals are published

The tone LUT, the two-arm Texture model (`A1 = 0.172443`, `A2 = 0.304888`;
45 of 45 Lightroom anchors within ±0.02), the 290×11 radial feather LUT,
the brush law `(1 − ρ^m)^n` with the measured flow constant `κ = 0.1284`
(D1 error 874 px → 9.8 px), and the lens mask-frame transport built from
Sony's own 16 native samples (radial 41/41 vectors within 1 px; linear
openly *not* pixel-closed, RMS 9.748/7.025/6.336 px) were each fitted to
Lightroom output. The XMP layer is hand-rolled on purpose — no XML crate —
so a catalogue sidecar is merged into byte for byte, down to the SVD fold
between Lightroom's pixel-space radial tilt and the engine's normalised
rotation, and Lightroom's Brotli-packed brush dab streams are imported and
verified (`MD5 → .acr → Brotli`).

### 7. Local models are licence-screened, pinned to the byte, and never in the repo

BiRefNet (444,473,596 B), OneFormer ADE20K Swin-L (881,196,376 B), SAM 2.1
Hiera-Large (897,897,416 B), SigLIP 2 (1,501,968,264 B), SD 2.1 as a DIFT
featurizer (2,580,061,174 B) and five SCUNet checkpoints are fetched on
first use through one shared download-and-refuse implementation
(sha256 + byte cap), run as `-E` subprocesses inside job-object kill groups
with a single-flight model slot, and every AI mask is cached under a
provenance key so a better backend forces an honest re-derivation. SegFormer
was removed for its research-only licence; the licence is a selection
criterion, not a footnote.

### 8. Generated pixels are quarantined and measured

`reimagine` composes the prompt onto an unconditional faithfulness scaffold
(because `input_fidelity` is silently dropped by gpt-image-2), measures the
result's structural divergence with the same statistic the reverse-fit uses,
warns at `D ≥ 0.35`, and can spend one bounded retry keeping the closer
image. `heal` only ever copies, shifts and averages pixels that already
exist. Anything that changed pixels lives on its own card as a pixel source
— never disguised as a Lightroom adjustment.

### 9. Guards the compiler and the release script enforce

Deletable rasters are a type (`OwnedRaster`): a call site that deletes cannot
be handed a user path, so the mistake that once reached a calibration mask
no longer compiles. `scripts/check_docs.py` re-derives 26 pinned release
claims (formats, camera bodies, test counts, toolchain) from the source and
the gate transcripts, and every batch ships with named falsifier tests and a
mutation table.

### Designed, not yet shipped

Written down in the plan and the design memos, in delivery order:

- **Free-form remainder masks** — the analyzer's fourth producer draws
  bitmap masks where its remainder says a spatially varying develop is still
  owed, through the same gates as the tiles (in implementation).
- **Multi-region semantic calibration** — one OneFormer pass, up to four
  disjoint class regions each choosing Full or Atmosphere on its own,
  confidence taken from the worst accepted region; colour-range regions
  alongside luminance ranges (design memo complete).
- **Style retrieval expansion** — ingest finished exports as exemplars (not
  only RAW+XMP pairs), text embeddings so a written style brief retrieves by
  meaning, the embedding switch in the GUI (today the
  `AUTOSHOP_STYLE_EMBED` environment variable), and a prompt-adherence axis
  next to Strength.
- **Linear-gradient falloff continuity** (C¹ clamp ramp; a rendering change
  reserved for v1.1) and a **macOS build**.

## How it works

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-dark.png" />
  <img src="docs/images/architecture-light.png" alt="Autoshop runtime architecture: three front ends over one Rust library, local Python sidecars, and opt-in external AI services" />
</picture>

<sub>Runtime architecture. The interactive version is
[docs/architecture/autoshop.architecture.html](docs/architecture/autoshop.architecture.html),
generated from [autoshop.architecture.json](docs/architecture/autoshop.architecture.json)
with [archify](https://github.com/tt-a1i/archify).</sub>

The primary path is short. [`src/decode.rs`](src/decode.rs) decodes the RAW
and yields a preview, EXIF, and a histogram; the vision advisor in
[`src/advisor/`](src/advisor/) turns those into an `EditRecipe`
([`src/recipe.rs`](src/recipe.rs)); a verifier that receives recipe, EXIF,
histogram, and clipping data — never pixels — checks it; the engine in
[`src/render.rs`](src/render.rs) applies it; the developed image, the recipe,
and a Lightroom-readable sidecar ([`src/xmp.rs`](src/xmp.rs)) are written to
the per-user develop store. Local masks, style retrieval, reverse-fit, and the
generative tools hang off that path without changing it.

What is deliberately hard about it:

- **One contract between the AI and the pixels.** `EditRecipe` is the only
  channel. The advisor answers under a strict `json_schema`; every control is
  bounded and clamped on entry; missing fields take defaults, so older recipes
  stay readable; and
  the rationale and confidence the model wrote are shown to the user and
  stored with the develop. The same struct drives the GUI sliders, the CLI,
  the web UI, and the XMP projection.
- **Reproducible by construction.** The renderer is a deterministic f32
  pipeline: the same recipe on the same RAW yields the same bytes on every
  run, which is what makes an AI proposal auditable, replayable, and safe to
  hand to a batch.
- **Measured against Lightroom, not assumed.** The tone LUT, the two-arm
  Texture model, the radial feather LUT, the brush flow law, and the lens
  mask-frame transport were each fitted to measurements and are quoted with
  their residuals in the next section and in
  [docs/TECH_STACK.md](docs/TECH_STACK.md); where a law is not closed, the
  documentation says so instead of rounding it off.
- **Reverse-fit is an estimator with an honesty budget.** A structural
  divergence statistic decides whether a target still shows the same scene
  (full solve) or a repainted one (bounded Atmosphere mode). Semantic zones,
  luminance-range bands, and quadtree tiles are each admitted only through
  evidence gates and a do-no-harm frame check, and a read-only local-field
  analyzer states how much of the remaining difference *any* spatially
  varying develop could reach before a producer runs — a ceiling reported in
  numbers, never applied to pixels.
- **Sidecars are merged, not regenerated.** The XMP writer edits the fields it
  owns inside the existing document and leaves everything else untouched, so a
  Lightroom catalogue survives a round trip.
- **AI masks carry provenance.** Each cached alpha records the backend that
  produced it; a better backend forces an honest re-derivation instead of
  presenting an older mask as the new model's result.
- **Generated pixels are labelled.** Denoise, reimagine, retouch, and heal are
  explicit exceptions to the develop path, kept on their own cards, and a
  reimagined target is scored for structural divergence before anything is
  fitted to it.

## Results: two batches, six frames

Each row is one Sony α7R IVA 61 MP `.ARW`. Every frame not marked *generated*
is rendered by Autoshop's engine from a recipe; the neutral frame is
Autoshop's own conversion, not the camera JPEG. Model-judge scores are
automated review, not human aesthetic approval.

### Batch 1 — original · AI analysis · AI analysis with a style reference

<table>
<tr>
<td width="33%"><img src="docs/images/showcase-lake-neutral.jpg" alt="Lake and boat: neutral engine conversion" /><br /><sub><b>Original.</b> Neutral engine conversion of the RAW.</sub></td>
<td width="33%"><img src="docs/images/showcase-lake-ai.jpg" alt="Lake and boat: AI develop with style read disabled" /><br /><sub><b>AI analysis.</b> The vision advisor's develop with style influence disabled. This run rendered under a Revise verdict and therefore has no saved recipe/XMP; it is kept as a transparent comparison.</sub></td>
<td width="33%"><img src="docs/images/showcase-lake-ai-style.jpg" alt="Lake and boat: AI develop with four retrieved style references" /><br /><sub><b>AI analysis + style reference.</b> The same advisor, now handed four similar edits retrieved from the indexed Lightroom library as soft references; accepted and saved as a normal recipe and XMP. Nothing is copied pixel for pixel.</sub></td>
</tr>
</table>

### Batch 2 — original · AI full-image generation · AI reverse-fit

<table>
<tr>
<td width="33%"><img src="docs/images/showcase-viaduct-neutral.jpg" alt="Stone viaduct: neutral engine conversion" /><br /><sub><b>Original.</b> Neutral engine conversion of the RAW.</sub></td>
<td width="33%"><img src="docs/images/showcase-viaduct-reimagine.jpg" alt="Stone viaduct: AI-generated 3520×2352 target" /><br /><sub><b>AI full-image generation</b> (<i>generated</i>). A 3520×2352 target from a configured <code>gpt-image-2</code>; it may invent content, and its structural divergence from the input is measured and disclosed before anything is fitted to it.</sub></td>
<td width="33%"><img src="docs/images/showcase-viaduct-fit.jpg" alt="Stone viaduct: reverse-fitted recipe rendered on the original RAW at 9504×6336" /><br /><sub><b>AI reverse-fit.</b> The recipe recovered from that look, rendered on the original RAW at 9504×6336 — editable, deterministic, and unable to invent detail. Look error 0.057 → 0.019 at fit confidence 0.678264; the colour-cast stage was rejected by the fit's own do-no-harm review, so the recipe carries tone and saturation only.</sub></td>
</tr>
</table>

More examples — the cat `analyze` pair, three further pairs including two
documented failure modes, the style-read triptychs, and the sunset
reimagine — are in [docs/SHOWCASE.md](docs/SHOWCASE.md).

## Measured numbers

Every figure below is reproduced from the sections that own it; none is an
estimate. Sources are the pinned claims in
[docs/TECH_STACK.md](docs/TECH_STACK.md) and the tests that
[`scripts/check_docs.py`](scripts/check_docs.py) re-derives.

| What | Measured | Where |
|---|---|---|
| Automated test battery | 1017 library / 15 CLI / 145 GUI / 2+2 contract tests; `check_docs` re-derives the pinned release claims | [Tech stack](#tech-stack-algorithms-and-design-philosophy) |
| RAW coverage | 24 extensions, 725 camera bodies; nine-camera format zoo 9/9 at the last release gate | [Supported formats](#supported-formats) |
| Lightroom Texture parity | 45 of 45 period/depth anchors within ±0.02 | [Develop pipeline](#develop-pipeline-and-tone-model) |
| Radial mask closure | 41 of 41 measured vectors within ≤1 px | [Lens correction](#lens-correction-and-lightroom-mask-frame-laws) |
| Linear mask closure (openly not pixel-closed) | RMS 9.748 / 7.025 / 6.336 px with lens correction on, 12.449 / 9.943 / 4.979 px off | [Lens correction](#lens-correction-and-lightroom-mask-frame-laws) |
| Brush geometry | D1 error 874 px → 9.8 px after pixel-centre sampling and the pixel/aspect metric | [Masks](#masks) |
| X-Trans demosaic (approximate) | X-S10 G/R ratio 1.5503 → 0.9476 | [RAW decode](#raw-decode-and-cfa) |
| Reverse-fit, stone viaduct | look error 0.057 → 0.019, confidence 0.678264 | [Results](#results-two-batches-six-frames) |
| Reverse-fit, sunset | look error 0.060 → 0.042, confidence 0.746691 | [docs/SHOWCASE.md](docs/SHOWCASE.md) |
| Local-field ceiling, calibration pair | global fit 0.0961 against a ceiling of 0.0700; the accepted sky zone realizes 0.134 of the distance | [User manual §4](#4-use-versions-and-variants) |
| AI develop, model judge | cat pair 62 → 86; townhouse 84 → 86; balcony 78 → 84; hillside 63 → 87 (automated scores) | [docs/SHOWCASE.md](docs/SHOWCASE.md) |
| Style retrieval weight | `W_EMB=2.0` retained after a 147-exemplar calibration | [AI advisor](#ai-advisor-and-reverse-fit) |
| Memory budget | 1800 MB per photo from a 1771 MB reference probe; 4 GiB RAW admission gate | [Application](#application-and-infrastructure) |

## Install and quickstart

### Download a release

The v1.0.0 release provides both Windows front ends. Linux and macOS are built
and tested in CI, but no prebuilt binaries are published for them yet.

| File | Size | SHA-256 |
|---|---:|---|
| `autoshop.exe` (CLI) | 31,180,152 bytes | `116a38410a810b1b27602c97daa4db614241b89fffbb80c6691a275fc7f168c0` |
| `autoshop-gui.exe` (desktop app) | 40,810,704 bytes | `847f42c4b35c09ab5dd040fdf8e90f99d597c66624ef131ac02d93071bcb58ce` |
| `Autoshop-Setup-1.0.0.exe` (installer) | 19,768,387 bytes | `28c4acd37089e78bf02182cd8b20a214a63cababb1b02971209be3fdf33d4750` |
| `autoshop-1.0.0-windows-x64.zip` (portable archive) | 27,131,443 bytes | `47389ed42f80798ead96980d69ce10f5063ece606e0f0d548482c58aef9f717e` |

Download from the
[v1.0.0 release page](https://github.com/skymanbp/autoshop/releases/tag/v1.0.0):

- **Installer (recommended):** run `Autoshop-Setup-1.0.0.exe`. It installs for
  the current user without administrator access, adds Start Menu shortcuts,
  offers optional desktop and user `PATH` tasks, and removes its own files on
  uninstall while keeping the develop store in `%LOCALAPPDATA%\autoshop`.
- **Portable archive:** extract `autoshop-1.0.0-windows-x64.zip` to a directory
  you can keep intact. Run either executable from that directory so it remains
  beside the bundled `assets/` and `python/` sidecars.

### Build from source

Autoshop uses Rust edition 2024 and rustc/cargo 1.94.

```bash
cargo build --release
cargo build --release --features gui --bin autoshop-gui
```

The first command builds the CLI. The second builds the desktop app; GUI
dependencies stay behind the `gui` feature.

The Rust build covers the core application. Source builds that use the local AI
tools also need Python packages:

- **SCUNet denoise** ([`python/denoise.py`](python/denoise.py)): install a
  suitable `torch` build, then OpenCV, NumPy, einops, and requests. The CUDA
  setup used by the sidecar is:

  ```bash
  pip install torch --index-url https://download.pytorch.org/whl/cu128
  pip install opencv-python numpy einops requests
  ```

- **BiRefNet subject masks:** `pip install torchvision timm einops` using a
  `torchvision` build matched to `torch`.
- **U²-Net subject fallback:** `pip install rembg`.
- **OneFormer sky and SAM 2.1 object masks:** `pip install transformers torch`.

Weights download on first use and are not committed to the repository.

### First run: desktop app

1. Start `autoshop-gui`.
2. Choose **Open photo…** or press `Ctrl+O`, then select a supported photo. You
   can also drag a photo into the window or use **Open folder…** for the library
   view.
3. Move a Develop slider and compare it with the neutral conversion.
4. Press `Ctrl+Shift+E` to open Export, choose a destination and format, then
   export a copy. The original remains untouched.

### First run: CLI

Decode a preview and metadata, then make a manual recipe render:

```text
autoshop decode "photo.ARW" -o "preview.jpg"
autoshop apply "photo.ARW" "recipe.json" -o "developed.tif"
```

With the image/vision role configured, an end-to-end AI develop is:

```text
autoshop auto "photo.ARW" --guidance "natural color; protect highlights" -o "developed.tif"
```

## User manual

### 1. Open and inspect a photo

Use **Open photo…** (`Ctrl+O`), drag and drop, or **Open folder…**. The library
is read-only: Autoshop stores develop state separately and never rewrites the
source RAW. The viewer applies EXIF orientation before crop and mask geometry,
so every tool works in the displayed frame.

The neutral view is Autoshop's own conversion, not the camera JPEG. Use the
before/after control while editing; histogram and clipping information are
computed from the decoded image and also feed the AI verifier.

### 2. Develop the image

The Develop panel exposes white balance, exposure and tonal controls, RGB point
curves, HSL, color grading, texture, clarity, dehaze, noise reduction,
sharpening, vignette, crop, and lens-related settings. Changes render through
the same engine as `autoshop apply`; there is no hidden GUI-only look.

Press **Save develop** or `Ctrl+S` to persist the recipe and, for a RAW, its XMP
projection in the per-user develop store. A neighboring Lightroom/ACR `.xmp`,
when present, is read only as the merge base; Save does not overwrite it. A
baked image keeps an Autoshop recipe but does not receive a RAW XMP. To deliver
the stored projection where Lightroom reads it, choose **Export .xmp beside the
photo**; replacing an existing neighboring sidecar requires confirmation.

### 3. Add local masks

Open **Local Masks**, create a mask, then adjust the sliders inside that mask.
Shapes can be combined with Add, Subtract, or Intersect and can carry luminance
or color range restrictions.

- **Linear gradient:** choose **＋ Linear gradient**, then drag from the fully
  affected side toward the unaffected side. Hold `Shift` to lock an axis.
- **Radial gradient:** choose **＋ Radial gradient**, drag the ellipse, then
  position, rotate, and feather it.
- **Brush:** choose **🖌 Brush** and paint. Use Erase to subtract, `[` and `]`
  to change brush size, and **Apply** to bake the stroke into a bitmap alpha.
- **AI select subject:** runs local BiRefNet, with a named U²-Net fallback when
  the preferred backend cannot run.
- **AI select sky:** runs local OneFormer ADE20K sky segmentation.
- **Point-prompted object:** imported object intent and ordered positive click
  gestures are re-derived locally with SAM 2.1.

AI mask rasters are cached with backend provenance. If a better backend becomes
available, the cache key forces an honest re-derivation instead of presenting
an older alpha as the new model's result.

### 4. Use versions and variants

A variant is one card for the same photo: **▣ Original**, **✨ AI generated**,
or **◭ Reverse-fit**. Each card combines its own base pixels with one develop.
`Ctrl+S` saves every card in the strip together. Switching cards is navigation,
not an edit; reopening returns to the card that was active at the last save,
not the last card viewed.

A version is a numbered snapshot of one card's develop at one moment. **＋ Save
as version** writes `v<N>.recipe.json`, frozen `v<N>.mask-*.png` rasters, and
`.version-meta.json` provenance (`from_kind`/`from_id`, name, and `user` or
`auto` origin). Loading a version replaces the active card's canvas as one undo
step. `auto` versions are snapshots made by the backup gate before it replaces
a saved develop.

An AI-generated variant carries its look in pixels and has no editable XMP
develop. Reverse-fit estimates an engine recipe from that look; copy the fitted
develop to Original when you want an editable recipe and sidecar for the
full-resolution source.

With **Zoned fit (sky)** enabled, reverse-fit always solves the global recipe
first. Successful segmentation adds semantic sky/land bitmap corrections. If
segmentation is disabled or unavailable, the same entry automatically tries
evidence-gated native luminance ranges instead; if no band is accepted, the
global recipe is kept. A range band is retained only when its composed
evidence-weighted frame is no worse than the running global/banded result.
Generated range masks persist as editable **Luminance
range** cards with their four ordered bounds. Their sentinel-hosted range
components project to Lightroom XMP, while semantic bitmap masks remain
engine-only. This release derives luminance ranges only, not color ranges.

Both analysis rasters share one geometry: the target is resampled into the
source's analysis thumbnail, so a one-row rounding difference between the two
images can no longer switch the structural evidence gate off.
Evidence verdicts follow the population a correction moves. The global recipe
and the frame-wide luminance ranges are judged on the whole frame; a semantic
zone or a spatial tile is judged on its own members, so a land zone is no
longer withheld because a replaced sky happens to share its luminance bins.
With its colour controls withheld, a zone whose luminance already matches is
left alone and says so instead of being dialled for a hairline tone gain.

Before either local producer runs, reverse-fit also measures how much of the
remaining difference a spatially varying develop could reach at all. A
read-only 12x8x8 bilateral field is solved on the same analysis thumbnails,
under the same frozen evidence and the same frame ruler the fit is judged by;
it produces numbers only and never enters the recipe, the engine, or the
sidecar. Its rendered residual is the *ceiling*. On the calibration pair the
global fit reads 0.0961 against a ceiling of 0.0700, and the accepted sky zone
realizes 0.134 of that distance; the rationale says so after every producer.
The analyzer also reports whether the remainder is band-shaped, tile-shaped,
linear or free-form, and names the luminance bins that vary too much in space
for a value band to describe them (bins 3 and 4 on that pair, at 29.1/255 and
28.7/255 against a 15/255 line). Shape is read only on the pixels the field
actually measured, so an unmeasured region cannot pose as structure; a
remainder that the 4x4 tile means do not explain halves the quadtree's budget
from four tiles to two, and a producer that already lands within 0.002 of a
ceiling that genuinely beat the producer-free frame ends the fit with a note
naming the stage it skipped. A band the field proposes reaches the
luminance-range producer as a span of current-render luma; the producer maps
it onto its own evidence bins through the pixels occupying that span, refuses
it when its own rank-paired residual disagrees with the field's sign, and says
why whenever it absorbs it.

After either local producer, reverse-fit automatically examines spatial
residuals with a frozen-evidence quadtree. It visits the strongest supported
nodes first, stops at a 4x4 grid and the analyzer's cap, and keeps a tile only
when both frames contribute at least 3% evidence, original structure remains
comparable, its confidence interval excludes zero, its boundary stays within
the calibrated rim budget, and the composed frame does not regress. Tiles are
editable engine bitmap masks; recipe JSON preserves them losslessly, while
classic XMP omits each one with a named bitmap-mask loss rather than inventing
an approximate rectangle.

Semantic silhouettes and eligible tile boundaries may be proposed for
edge-aware guided refinement before their corrections are fitted. The original
mask bytes win unless coverage is conserved, pixels outside the fixed collar
are unchanged, guide-edge alignment does not decrease, and the normal rim and
frame gates still pass. Luminance ranges are never spatially refined. There is
no additional switch and no multi-class semantic segmentation in this release.

### 5. Export

Open Export with the toolbar, `Ctrl+Shift+E`, or `Ctrl+E`. Choose JPEG, 8- or
16-bit PNG, or 8- or 16-bit TIFF; set JPEG quality, long-edge size, output
sharpening, and sRGB, Display P3, or Adobe RGB delivery color space. Resizing is
the last step, uses Lanczos3, preserves aspect ratio, and never enlarges a
smaller image.

CLI exports use q95 sRGB. `--long-edge N` is available on `apply`, `auto`, and
`batch --render`; `0` or omission means full resolution. It is deliberately an
export option rather than a recipe field, so one recipe can deliver both a
master and a web copy.

### CLI reference

The following commands and flags match the v1.0.0 command definitions in
`src/main.rs`:

```text
autoshop decode <src> [-o|--out FILE]
autoshop analyze <src> [-o|--out FILE] [--guidance TEXT] [--style 0..1] [--strength 0..1] [--deep]
autoshop apply <src> <recipe.json> (-o|--out) FILE [--long-edge N]
autoshop auto <src> [-o|--out FILE] [--guidance TEXT] [--style 0..1] [--strength 0..1] [--deep] [--denoise] [--denoise-strength 0..1] [--denoise-model NAME] [--long-edge N]
autoshop denoise <src> [-o|--out FILE] [--strength 0..1] [--model NAME]
autoshop batch <dir> [--render] [--limit N] [--include-baked] [--jobs N] [--long-edge N]
autoshop eval <dir> [--limit N] [--jobs N] [--fresh] [--state FILE]
autoshop style-index <dir>
autoshop reimagine <src> --prompt TEXT [--fidelity high|low] [--quality low|medium|high|auto] [--fidelity-retry] [-o|--out FILE]
autoshop match <src> <target> [--render] [--zoned] [--style-prompt] [--ai-judge] [--deep] [-o|--out FILE]
autoshop correspond <source> <target> [-o|--out FILE]
autoshop retouch <src> --mask FILE --prompt TEXT [--quality low|medium|high|auto] [--full-res] [-o|--out FILE]
autoshop heal <src> [--mask FILE] [--no-auto] [--full-res] [-o|--out FILE]
autoshop serve <dir> [-p|--port N]
autoshop recipe-schema
```

`<src>` is a RAW or baked image. For commands that save develop state, baked
sources get recipe JSON but no RAW XMP. `batch` skips baked photos unless
`--include-baked` is set, avoiding duplicate analysis and billing for RAW+JPEG
pairs.

`auto` is `analyze` plus render. `batch` analyzes RAWs by default, accepts
`--include-baked`, and defaults to three photos in flight; `eval` defaults to
serial work and resumes from its state file. `--long-edge` on `batch` requires
`--render`, and denoise-strength/model overrides require `--denoise` on
`auto`.

`match` itself is local inverse rendering and needs no key. Its optional
`--ai-judge` and `--deep` review paths do; `--deep` permits one guided retry.
`heal` can use a supplied mask offline, while its automatic detector uses the
vision role.

### Lightroom and XMP interoperability

Autoshop reads and writes sidecar XMP for global settings, point curves, HSL,
crop, and supported local corrections. Its writer merges owned fields into the
existing document and preserves unmodeled content byte-for-byte instead of
round-tripping the whole file through a general XML serializer.

In the desktop Save workflow, that merged XMP projection is written to the
per-user develop store. A Lightroom sidecar beside the RAW is only a merge base
and remains untouched. **Export .xmp beside the photo** is the separate,
explicit action that copies the stored projection into the photo folder for
Lightroom, with a second confirmation before replacement.

Linear and radial masks round-trip as editable geometry. Lightroom brush dab
streams are imported from the sibling `MaskBrushTable`, validated and Brotli
decoded, then rendered with Autoshop's measured brush model. Classic XMP does
not contain Lightroom's computed subject/sky/object alpha or arbitrary bitmap
alpha, so Autoshop preserves the selection intent and clearly re-derives the
mask with its own local model; generated image variants remain generated pixels
until reverse-fit produces an editable recipe.

### Configure and use the AI features

Open **Settings** to configure the image/vision role and the analysis-verifier
role. The image role uses an OpenAI-compatible API for visual proposals and
generative images. The verifier defaults to the signed-in `claude` CLI over
OAuth, receives statistics and recipe data rather than image pixels, and can
instead use an API provider.

The same roles can be configured from the environment. `OPENAI_API_KEY` serves
the image/vision and generative role; `AUTOSHOP_ANALYSIS_API_KEY` is used only
when the verifier is set to API mode. Settings are saved in the per-user
`autoshop.local.json`; do not put real credentials in the repository.

There is an additional trust guard for `./autoshop.local.json` in the current
working directory: it may select model/provider preferences, but it cannot
supply API credentials, endpoints, executable/script paths, or output
destinations. This allows a project to express harmless preferences without
turning an opened photo folder into a credential or path override.

- **Analyze:** choose **Analyze** in the AI panel or run `autoshop analyze`.
  The vision advisor proposes bounded sliders and masks, a data-only verifier
  checks the proposal, and normal visual review may attempt one revision;
  `--deep` permits additional bounded rounds. Accepted output remains a normal
  recipe and XMP.
- **Style match/read:** build the style reference library from Lightroom
  RAW+XMP pairs with the GUI or `style-index`. The Style control retrieves
  similar prior edits as soft references; Strength independently controls how
  strongly the proposal is allowed to move.
- **Reimagine:** enter a prompt in the AI panel or use `reimagine`. This creates
  a generated, lower-resolution target. Under `--fidelity high` (the default,
  and the GUI's mode) the prompt is composed onto an unconditional
  faithfulness scaffold — the model is told to re-develop the same photograph,
  not repaint it — because the `input_fidelity` request parameter is silently
  rejected by newer models (gpt-image-2). After generating, the structural
  divergence **D** against the sent input is measured (the same statistic the
  reverse-fit's mode selector uses) and disclosed; `D ≥ 0.35` warns that a
  reverse-fit of that result will fall back to atmosphere mode, and the
  opt-in `--fidelity-retry` (a GUI checkbox as well — off by default, it buys
  a second image) regenerates once and keeps the closer result. Use
  **Reverse-fit** or `match` to infer a deterministic recipe, then apply it
  to the original RAW at full resolution.

Local denoise and segmentation do not need an API key. Their Python sidecars
resolve relative to the installed program tree, and downloaded weights are
kept in the local cache rather than committed to the repository.

### Privacy, trust, and paid-feature boundary

| Runs locally without an API key | Uses the configured vision/generative API role |
|---|---|
| Deterministic render and manual develop, including `apply` | Full vision-backed `analyze` / `auto` proposals and visual model review |
| Local `match` inverse rendering | `match --style-prompt`, `--ai-judge`, or `--deep` |
| XMP read/write, masks, curves, and GUI sliders | Generative `reimagine` / `retouch` |
| SCUNet denoise and local BiRefNet/U²-Net, OneFormer, and SAM masks | Automatic target detection in `heal`; a supplied mask works offline |
| Style indexing and retrieval | |

Without the vision role, the advisor can fall back to its disclosed histogram
heuristic; that is not equivalent to the full vision-backed feature. The
data-only verifier defaults to the signed-in `claude` CLI over OAuth, so it does
not require an API key, although provider-backed operations may still consume a
subscription or incur charges.

Photos leave the machine only for AI operations the user requests through a
configured provider. The verifier receives recipe, EXIF, histogram, clipping,
and rationale data—not pixels—and Responses request bodies set `store:false`.
The local web UI binds to loopback only, checks Host/Origin and cross-site
requests, requires a fresh per-run session token for state changes, disables API
caching, and denies framing. By default, Autoshop keeps the source library read-only. If the configured Delivery folder is inside or above a photo’s folder, that delivery subtree is intentionally writable; Settings warns when this removes the folder’s protection. “Export .xmp beside the photo” is the separate, confirmed per-photo sidecar exception.

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
each fully decoded and neutral-rendered rather than copied from an embedded
preview. The corpus cannot ship in the repository, so the suite is
environment-gated and a bare test run skips it; the release process reruns and
records it explicitly. The last recorded release gate was 9/9.

**Camera RAW — 24 extensions**, one predicate app-wide (`decode::is_raw`):

```text
arw, dng, raw, raf, nef, cr2, cr3, orf, rw2, pef, srw, 3fr,
fff, iiq, mef, mos, erf, kdc, dcr, dcs, crw, nrw, mrw, ari
```

Decoding is rawler 0.7.2, which carries **725 camera models**. **No embedded
preview:** 12 of the 24 formats store none. They are `orf`, `srw`, `nrw`, `mef`,
`mos`, `kdc`, `dcr`, `dcs`, `erf`, `iiq`, `crw`, and `ari`; Autoshop shows its
own neutral rendition instead and says so.

**Baked rasters — 8 extensions:** `jpg`, `jpeg`, `png`, `tif`, `tiff`, `bmp`,
`webp`, `gif`. ICC profiles on baked imports are converted through qcms when
present.

Decode degradation and refusal behavior is explicit:

- An untagged 16-bit baked image is read as sRGB and flagged; that assumption
  is often wrong for an editor export even though it is usually right for an
  8-bit JPEG.
- Monochrome and four-colour sensor arrays are refused before development;
  Autoshop does not reinterpret them as three-channel colour.
- Unknown make, unknown model, and no matching decoder are differentiated and
  point to the DNG conversion route; a recognized but corrupt file keeps its
  separate integrity error.
- A third-party RAW parser panic is contained as a named per-file error, so one
  malformed file does not terminate a batch run.

## Tech stack, algorithms, and design philosophy

### Design philosophy

- **The AI decides what to change; the engine does it.** In the develop path
  the model never touches a pixel — it writes a bounded recipe, and the same
  deterministic renderer serves every front end.
- **Nothing hidden.** Every proposal carries its rationale and confidence;
  known weaknesses are written down as honesty markers rather than smoothed
  over in a caption.
- **Measured, not assumed.** Rendering laws are fitted to Lightroom and camera
  measurements and quoted with residuals; release claims in the documentation
  are re-derived by a script, not copied forward.
- **Non-destructive and interoperable.** The source library stays read-only,
  develops live in a per-user store, and sidecars are merged so a Lightroom
  catalogue survives the round trip.
- **Local first.** Segmentation, denoise, correspondence, and style embeddings
  run as local sidecars; pixels leave the machine only for an AI operation
  the user asks for, and the verifier never receives them.
- **Generated pixels are labelled.** Reimagine, retouch, heal, and denoise are
  opt-in exceptions kept on their own cards.

### Implementation

The canonical implementation page is **[Tech stack and algorithms](docs/TECH_STACK.md)**.
It gives the equations, parameter provenance, measured Lightroom/camera results,
honesty markers, and source paths behind each summary below.

### RAW decode and CFA

`src/decode.rs` uses rawler for **RAW decode, 24 formats**, with 725 bodies in
the release database. Bayer data takes rawler's demosaic path; X-Trans uses an
**approximate** 5×5 CFA-geometry plane fit that moved the measured X-S10 G/R
ratio from 1.5503 to 0.9476. `orient_f32` applies EXIF orientation at the head
of the chain; no-preview RAWs receive a neutral develop, untagged 16-bit rasters
are disclosed as assumed sRGB, and mono/four-colour sensors are refused.

### Develop pipeline and tone model

`src/render.rs` is a deterministic f32 pipeline with explicit linear-light
vignette/dehaze stages, a monotone Fritsch–Carlson tone LUT with
`tone_knot_weights` and Highlights inside the LUT, then RGB curves, HSL, colour
grade, clarity/Texture, saturation, NR, sharpening, and local edits. Negative
Texture is two measured parallel low-pass arms (`A1=0.172443`, `A2=0.304888`)
with a calibrated hyperbolic depth law; all 45 Lightroom period/depth anchors
land inside ±0.02.

### Masks

`src/recipe.rs`, `src/render.rs`, and `src/xmp.rs` implement radial, linear,
brush, bitmap, luminance-range, and colour-range masks with ordered
Add/Subtract/Intersect composition. Radial feather is a measured 290×11
`alpha(rho, feather)` LUT with an analytic hard edge at zero; brush dabs use
`(1-rho^m)^n`, the measured `kappa=0.1284` flow law, and screen accumulation.
Pixel-centre sampling and the pixel/aspect linear metric reduced the D1 error
from 874 px to 9.8 px; `MaskBrushTable` import validates MD5→`.acr`→Brotli.

### AI masks

`src/segment.rs` and `python/segment.py` run commit-pinned BiRefNet subject
selection with a named U²-Net fallback, OneFormer ADE20K sky selection through
the 150-class checked-in table, and SAM 2.1 object selection from ordered
positive gesture points over the `gp1` IPC. Provenance-keyed caches include the
backend generation and exact prompt points, so a fallback alpha is re-derived
when the pinned backend becomes available; these are local re-creations, not
Adobe-computed mask pixels.

### Lens correction and Lightroom mask-frame laws

`src/lensmeta.rs`, `src/lcp.rs`, and `src/render.rs` combine Sony 0x7037's 16
native `(i+1)/16` samples, a 2048-node/64-knot mask solve, and guarded Newton
inversion for rectilinear `.lcp` profiles while refusing fisheye-only entries.
Radials use exact-once `m_lr^-1 ∘ T_engine` transport and close 41/41 vectors to
≤1 px. Linear H2 keeps corrected-frame handles but is openly not pixel-closed:
ON RMS is 9.748/7.025/6.336 px and OFF is 12.449/9.943/4.979 px; brushes remain
in the raw frame.

### XMP and Lightroom interoperability

[`src/xmp.rs`](src/xmp.rs) uses scoped, typed XML traversal, including nested
`Look`, and conservatively merges owned edits while preserving unmodeled
fields. Ordinary Save writes the per-user develop store; beside-RAW export is
explicit. `LR_MASK_FRAME_SCALE=1.0`, `LocalExposure2012=EV/4`, local Hue is
`degrees/180`, the other measured local family is `/100`, global Sharpness is
1:1, and polarity comes from `MaskInverted` rather than `Flipped`.

### AI advisor and reverse fit

`src/advisor/` validates AI proposals into bounded recipes, keeps Responses at
`store:false`, gives the verifier data rather than pixels, and adopts a guided
revision only when it does not lower the score. `src/style.rs` retrieves
z-scored RAW+XMP exemplars with optional SigLIP 2 (`W_EMB=2.0` retained after a
147-exemplar calibration). `src/fit.rs` performs luminance-CDF, exposure,
basis, tone, saturation, and cast inverse stages with a ≥45°/≥5% foreign-hue
veto; `src/correspond.rs` + `python/correspond.py` measure a DIFT (SD 2.1)
correspondence field between two renditions of one frame — 48×48 cells of
target coordinates whose confidence is cyclic consistency × flow smoothness
— on content-divergent pairs the reverse-fit consults it automatically
(local sidecar; its D gate decides) and full zone fits weight their pixel
pairs by the field's confidence and read shifted content at its
corresponded position, disclosed in the recipe rationale (`correspond` is
the standalone diagnostic door);
`src/generative.rs` negotiates gpt-image-2 reimagine sizes, and
`src/retouch.rs` supplies deterministic pixel heal.

### Application and infrastructure

Rust (rustc/cargo **1.94**, edition 2024) · rawler (RAW decode, 24 formats / 725 bodies) ·
`image`, qcms, rayon, clap, serde, ureq, `eframe`/egui, and `tiny_http` back the
shared library, CLI, desktop GUI, and embedded loopback web UI. The server uses
a 32-byte token plus Host/Origin/no-store defenses; the GUI keeps variants,
versions, and a deleted-version registry; SCUNet success requires the typed
`sidecar_wrote` contract. A 1771 MB reference probe sets the 1800 MB per-photo
budget, while the 4 GiB RAW gate bounds admission. The [`build`
workflow](.github/workflows/build.yml) covers default and GUI feature sets on
Ubuntu and macOS. The current battery is **1017 library (1006 pass + 11 `#[ignore]`d forensic probes) / 15 CLI / 145 GUI / 2+2 contract** tests; the
[`scripts/check_docs.py`](scripts/check_docs.py) gate re-derives pinned release
claims. Model weights are not stored in this repository.

## Status, roadmap, and known limitations

Release gates for v1.0.0 cover the CLI, desktop GUI, sidecar contracts, format
fixtures, and deterministic renderer; the built artifacts' sizes and hashes
are listed above. Prebuilt artifacts are Windows-only; CI checks source builds
on Ubuntu and macOS, while interactive use there remains less exercised.

Current honesty markers include the approximate X-Trans path, locally
re-derived rather than Adobe-identical AI masks, measured-but-not-bit-exact
Lightroom rendering parity, and lossy generated reimagine targets. Older
recipes remain readable. v1.0.0 recipes can carry the new
`LensProfile.mask_warp_center` and `LensProfile.linear_handle_warp` frame facts;
older binaries cannot safely ignore those fields and therefore refuse recipes
that contain them.

Existing content that may rerender includes angled LINEAR masks on non-square frames, RADIAL/LINEAR masks with camera-metadata lens profiles, modern table-backed Lightroom brushes, and subtype-0 object masks with gesture points. RADIAL closes 41/41 measured vectors to ≤1 px; clean dilation is within 0.35 pp, R1 about 0.5 pp, with an open R2 excess of about 1.2 pp. LINEAR remains not pixel-closed: ON RMS 9.748/7.025/6.336 px and OFF RMS 12.449/9.943/4.979 px.

See [docs/ROADMAP.md](docs/ROADMAP.md) for planned work and
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for subsystem boundaries and
dependency rationale.

## License and acknowledgements

**Autoshop is MIT-licensed** — see [LICENSE](LICENSE).

### RAW format samples

The nine files behind the format grid come from the
[raw.pixls.us](https://raw.pixls.us/) community sample repository under CC0
1.0 Public Domain. The recorded sample SHA-256 values were verified against
that index before use.

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
skymanbp, all rights reserved. They are included only to document Autoshop's
output and are not covered by the software's MIT license. The three established
before/after pairs retain their matching visible watermarks and embedded
copyright metadata; the newer composed cat/style/reimagine JPEGs omit EXIF and
do not add a watermark.

### Fonts and model weights

The GUI bundles subset Noto faces under the SIL Open Font License; license texts
are under `assets/fonts/`. Model weights are downloaded separately and remain
the property of their authors; none are redistributed in this repository.

| Model | Purpose | License |
|---|---|---|
| SCUNet | AI denoise | Apache-2.0 |
| BiRefNet | Subject segmentation | MIT |
| U²-Net | Subject fallback | Apache-2.0 |
| OneFormer ADE20K | Sky segmentation | MIT |
| SAM 2.1 | Point-prompted object masks | Apache-2.0 |
| SigLIP 2 | Optional style embeddings | Apache-2.0 |

The project acknowledges the rawler, image, qcms, rayon, clap, serde, ureq,
egui/eframe, tiny_http, and local-model communities whose work makes these
pipelines possible.
