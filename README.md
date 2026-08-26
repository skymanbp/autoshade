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

Autoshop is a non-destructive photo developer for RAW and baked images. Its
main workflow turns an AI proposal into a small, inspectable `EditRecipe`, then
applies that recipe with the same local Rust renderer used by the desktop app,
CLI, and embedded web UI. Generative tools are separate, opt-in paths and are
labelled as such.

<p align="center">
<img src="docs/images/showcase-cat-analyze-pair.jpg" alt="Sony α7R IVA ARW: neutral cat photo beside its AI analyze develop" />
<br />
<sub><b>AI analyze develop.</b> Sony α7R IVA <code>.ARW</code>, 61 MP: neutral engine conversion at left; AI-proposed crop, global tone, a radial cat lift, and a linear water hold at right. The model judge moved from 62 to 86; that score is automated review, not human aesthetic approval.</sub>
</p>

## Contents

- [Feature overview](#feature-overview)
- [Install and quickstart](#install-and-quickstart)
- [User manual](#user-manual)
- [Showcase Part A — AI analysis and style transfer](#showcase-part-a--ai-analysis-and-style-transfer)
- [Showcase Part B — full-image generation to recipe inversion](#showcase-part-b--full-image-generation-to-recipe-inversion)
- [Supported formats](#supported-formats)
- [Tech stack and algorithms](#tech-stack-and-algorithms)
- [Status and roadmap](#status-and-roadmap)
- [License and acknowledgements](#license-and-acknowledgements)

## Feature overview

- A shared, deterministic develop engine with exposure, white balance, curves,
  HSL, color grading, texture, clarity, dehaze, detail, crop, and lens-aware
  local adjustments.
- Linear, radial, brush, luminance-range, and color-range masks, plus local AI
  subject, sky, and point-prompted object selection.
- AI `analyze` and `auto` workflows that propose editable recipes, validate
  them against image statistics, render them, and optionally run one bounded
  visual-review revision.
- Lightroom/ACR sidecars in both directions, with conservative merge behavior
  for fields Autoshop does not model.
- Versions and variants for ordinary develops, generated targets, and
  reverse-fitted looks without rewriting the source photo.
- Desktop GUI, scriptable CLI, and a small local web UI, all using the same
  library.

The core develop path never invents scene content. Local SCUNet denoise,
generative reimagine/retouch, and pixel heal are explicit opt-in exceptions;
the UI distinguishes generated pixels from engine-rendered develops.

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

## Showcase Part A — AI analysis and style transfer

### AI `analyze`: before and after

The hero cat pair is the first `analyze` example: a Sony α7R IVA 61 MP `.ARW`,
shown as straight conversion and AI develop. The AI chose the crop and a
restrained global develop plus radial and linear parametric masks; it did not
use an AI bitmap segmentation mask.

The three established pairs below remain because they show different decisions
and, importantly, two current failure modes. Each before is Autoshop's neutral
conversion of the same Sony α7R IVA `.ARW`; each after is an AI-proposed engine
render, not a generated image. The faint watermark is identical on both halves
of these three older pairs.

#### Townhouse and pond: tonal range

<table>
<tr>
<td width="50%"><img src="docs/images/showcase-1-before.jpg" alt="Sony α7R IVA ARW, townhouse and pond: neutral develop" /><br /><sub><b>Before:</b> neutral engine conversion.</sub></td>
<td width="50%"><img src="docs/images/showcase-1-after.jpg" alt="Sony α7R IVA ARW, townhouse and pond: AI develop" /><br /><sub><b>After:</b> AI tone, white balance, crop, a linear sky hold, and a radial house lift.</sub></td>
</tr>
</table>

The proposal protected white brick while opening the porch and black wall. Its
model judge moved from 84 to 86 after a bounded revision. Honest blemish: the
linear sky mask leaves a faint lighter band near the top-left corner. These are
model-judge scores recorded when the pair was produced (v0.33.0 showcase batch).

#### Balcony view: detail and texture

<table>
<tr>
<td width="50%"><img src="docs/images/showcase-2-before.jpg" alt="Sony α7R IVA ARW, balcony view: neutral develop" /><br /><sub><b>Before:</b> neutral engine conversion.</sub></td>
<td width="50%"><img src="docs/images/showcase-2-after.jpg" alt="Sony α7R IVA ARW, balcony view: AI develop" /><br /><sub><b>After:</b> AI texture, clarity, dehaze, tonal changes, and two linear masks.</sub></td>
</tr>
</table>

The siding and shaded structure gain separation; the model judge moved from 78
to 84. These are model-judge scores recorded when the pair was produced
(v0.33.0 showcase batch). This pair is deliberately kept as a counter-example:
the sky is paler than the neutral base even though the local mask asks for more
sky depth.

#### Hillside neighborhood: establishing scene

<table>
<tr>
<td width="50%"><img src="docs/images/showcase-3-before.jpg" alt="Sony α7R IVA ARW, hillside neighborhood: neutral develop" /><br /><sub><b>Before:</b> neutral engine conversion.</sub></td>
<td width="50%"><img src="docs/images/showcase-3-after.jpg" alt="Sony α7R IVA ARW, hillside neighborhood: AI develop" /><br /><sub><b>After:</b> AI global contrast, restrained color, and green/aqua HSL reductions.</sub></td>
</tr>
</table>

Automated visual model review rejected the first acidic-green proposal at 63
and retained a revision scored 87. These are model-judge scores recorded when
the pair was produced (v0.33.0 showcase batch). The landscape gains separation,
but the sky is again paler and milkier than the neutral conversion; that known
behavior is not captioned as an improvement.

### Style read: neutral, AI develop, and AI develop with references

These triptychs show three states of the same Sony α7R IVA 61 MP `.ARW`: straight
conversion, an AI develop with style influence disabled, and an AI develop that
read similar edits from the local style library. They demonstrate the style
retrieval path, not a pixel-copy or generative transfer.

<img src="docs/images/showcase-lake-style-triptych.jpg" alt="Lake scene: straight conversion, AI develop, and AI develop with style read" />

<sub><b>Lake and boat.</b> The style-read run referenced four similar edits from the indexed Lightroom library and was accepted. The style-off middle panel rendered under a Revise verdict and therefore has no saved recipe/XMP; it is retained only as a transparent comparison.</sub>

<img src="docs/images/showcase-sunset-style-triptych.jpg" alt="Sunset scene: straight conversion, AI develop, and AI develop with style read" />

<sub><b>Sunset.</b> The middle panel is an accepted style-off develop. The style-read proposal at right used retrieved references and rendered at full RAW resolution, but the model judge marked it Revise (85); its attempted revision scored 84 and was discarded, so no style-read recipe/XMP was saved.</sub>

## Showcase Part B — full-image generation to recipe inversion

Part B is a different workflow: generate a complete visual target, then fit an
ordinary engine recipe to its look. The generated target can invent content;
the fitted render cannot. The recovered recipe is editable and can be applied
deterministically to the original full-resolution RAW.

### Sunset reimagine and reverse-fit

<img src="docs/images/showcase-sunset-reimagine-fit-triptych.jpg" alt="Sunset scene: neutral conversion, AI-generated target, and reverse-fitted full-resolution engine render" />

<sub><b>Sunset, Sony α7R IVA 61 MP <code>.ARW</code>.</b> Left: neutral engine conversion. Center: a 3520×2352 full-image target generated with a configured <code>gpt-image-2</code>. Right: the recovered recipe rendered by Autoshop on the original RAW at 9504×6336. The statistical look error moved from 0.060 to 0.042 at fit confidence 0.746691; this is a deterministic tonal/color approximation, not a pixel-aligned reconstruction of generated detail.</sub>

### Viaduct reimagine and reverse-fit

<img src="docs/images/showcase-viaduct-reimagine-fit-triptych.jpg" alt="Stone viaduct scene: neutral conversion, AI-generated target, and reverse-fitted full-resolution engine render" />

<sub><b>Stone viaduct, Sony α7R IVA 61 MP <code>.ARW</code>.</b> Left: neutral engine conversion. Center: a 3520×2352 full-image target generated with the same configured <code>gpt-image-2</code>. Right: the recovered recipe rendered on the original RAW at 9504×6336. The statistical look error moved from 0.057 to 0.019 at fit confidence 0.678264; the fitted color-cast stage was rejected by the fit's own do-no-harm review, so the recovered recipe carries tone and saturation only.</sub>

Reverse-fit measures structural divergence first: same-content targets keep the
full tone, saturation, and guarded-cast solve, while structurally changed
targets use bounded Atmosphere mode for overall tone and colour. Zoned fits
retain independently bounded sky/land adjustments behind a local-quality gate;
they do not claim to reconstruct generated objects or detail.

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

## Tech stack and algorithms

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
— the diagnostic instrument (`correspond`) for the content-divergent case;
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
Ubuntu and macOS. The current battery is **937 library (928 pass + 9 `#[ignore]`d forensic probes) / 15 CLI / 145 GUI / 2+2 contract** tests; the
[`scripts/check_docs.py`](scripts/check_docs.py) gate re-derives pinned release
claims. Model weights are not stored in this repository.

## Status and roadmap

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
