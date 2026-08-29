# Autoshop user manual

The operating manual for Autoshop v1.0.0 — the desktop app, the CLI, and the
embedded web UI. Installation and the first run are in the README's
[Install and quickstart](../README.md#install-and-quickstart); what the program
is, what is new in it, and how it works are the README's opening sections;
subsystem boundaries are in [ARCHITECTURE.md](ARCHITECTURE.md) and the
algorithms in [TECH_STACK.md](TECH_STACK.md).

- [1. Open and inspect a photo](#1-open-and-inspect-a-photo)
- [2. Develop the image](#2-develop-the-image)
- [3. Add local masks](#3-add-local-masks)
- [4. Use versions and variants](#4-use-versions-and-variants)
- [5. Export](#5-export)
- [CLI reference](#cli-reference)
- [Lightroom and XMP interoperability](#lightroom-and-xmp-interoperability)
- [Configure and use the AI features](#configure-and-use-the-ai-features)
- [Privacy, trust, and paid-feature boundary](#privacy-trust-and-paid-feature-boundary)

## 1. Open and inspect a photo

Use **Open photo…** (`Ctrl+O`), drag and drop, or **Open folder…**. The library
is read-only: Autoshop stores develop state separately and never rewrites the
source RAW. The viewer applies EXIF orientation before crop and mask geometry,
so every tool works in the displayed frame. The neutral view is Autoshop's own
conversion, not the camera JPEG; histogram and clipping information are
computed from the decoded image and also feed the AI verifier.

## 2. Develop the image

The Develop panel exposes white balance, exposure and tonal controls, RGB point
curves, HSL, color grading, texture, clarity, dehaze, noise reduction,
sharpening, vignette, crop, and lens-related settings, rendered through the
same engine as `autoshop apply`.

**Save develop** (`Ctrl+S`) persists the recipe and, for a RAW, its XMP
projection in the per-user develop store. A neighboring Lightroom/ACR `.xmp` is
read only as the merge base; Save does not overwrite it. A baked image keeps an
Autoshop recipe but does not receive a RAW XMP. To deliver the stored
projection where Lightroom reads it, choose **Export .xmp beside the photo**;
replacing an existing neighboring sidecar requires confirmation.

## 3. Add local masks

Open **Local Masks**, create a mask, then adjust the sliders inside that mask.
Shapes can be combined with Add, Subtract, or Intersect and can carry luminance
or color range restrictions.

- **Linear gradient:** choose **＋ Linear gradient**, then drag from the fully
  affected side toward the unaffected side; the shipped falloff eases softly at
  both handles. Hold `Shift` to lock an axis.
- **Radial gradient:** choose **＋ Radial gradient**, drag the ellipse, then
  position, rotate, and feather it.
- **Brush:** choose **🖌 Brush** and paint. Use Erase to subtract, `[` and `]`
  to change brush size, and **Apply** to bake the stroke into a bitmap alpha.
- **AI select subject:** runs local BiRefNet, with a named U²-Net fallback when
  the preferred backend cannot run.
- **AI select sky:** runs local OneFormer ADE20K sky segmentation.
- **Point-prompted object:** imported object intent and ordered positive click
  gestures are re-derived locally with SAM 2.1.

AI mask rasters are cached with backend provenance, so a better backend forces
an honest re-derivation instead of presenting an older alpha as its result.

## 4. Use versions and variants

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

Reverse-fit uses the panel's **Strength** value (or `match --strength 0..1`) as
its honesty budget. At or below the shipped 65% setting the historical path is
byte-identical, including white balance: a demand outside its budget remains
as-shot. Above 65%, WB demands outside the widened budget shrink along the
requested Kelvin/tint direction and are disclosed. The pre/post WB renders must
also pass the foreign-hue veto and a weighted rotation allowance, pinned at
0.05 through 65%, about 0.593 at 85%, and 1.0 at full strength. If no legal WB
remains, it is withheld and the recipe stays as-shot with a typed explanation.

With **Zoned fit (sky)** enabled, reverse-fit always solves the global recipe
first. Successful segmentation adds up to four disjoint semantic class bitmap
corrections; each region selects Full or Atmosphere independently. If
segmentation is disabled or unavailable, the same entry automatically tries
evidence-gated native luminance ranges instead, and if no band is accepted the
global recipe is kept. A range band is retained only when its composed
evidence-weighted frame is no worse than the running global/banded result.
The historical two-region route is the default. Enable **Up to four semantic
regions** in the GUI, or pass `--regions 4` on the CLI, to opt in to the
expanded route. It performs one OneFormer inference per frame and may take
longer. The default two-region dials and confidence are byte-identical to
`662b688`; only the rationale gains one typed `ZONE_ALREADY_MATCHED` note per
zone whose dials did not move.
Generated range masks persist as editable **Luminance range** cards with their
four ordered bounds; their sentinel-hosted range components project to
Lightroom XMP, while semantic bitmap masks remain engine-only. This release
derives luminance ranges only, not color ranges.

The mechanics — population-scoped verdicts, the local-field ceiling, quadtree
tiles, and guided refinement — are in [What is new here](../README.md#what-is-new-here)
§5–§8. What you see in use: both analysis rasters share one geometry (the
target is resampled into the source's analysis thumbnail), so a one-row
rounding difference can no longer switch the structural evidence gate off; the
global recipe and frame-wide luminance ranges are judged on the whole frame, a
semantic zone or spatial tile on its own members. The analyzer produces
numbers only and never enters the recipe, the engine, or the sidecar; after
every producer the rationale states its ceiling, whether the remainder is
band-shaped, tile-shaped, linear or free-form, which stage it skipped when a
producer already reached the ceiling, why a field-proposed band was absorbed
or refused, and which luminance bins vary too much in space for a value band
to describe them (bins 3 and 4 on the calibration pair, at 29.1/255 and
28.7/255 against a 15/255 line). Shape is read only on the pixels the field
actually measured, so an unmeasured region cannot pose as structure; a
remainder the 4x4 tile means do not explain halves the quadtree's budget from
four tiles to two, and the quadtree stops at a 4x4 grid and that cap.
Luminance ranges are never spatially refined. Colour-range semantic regions are
not derived.

The free-form field-mask pass then consumes only the remainder not already
covered by accepted tiles. It uses the field's frozen per-pixel weight, keeps
opposite signs in separate 4-connected components, and discloses every
proposal and typed refusal before or after fitting; the layer is enabled with
the field by default and is disabled whenever the field layer is disabled; there
is no separate user-facing switch.

## 5. Export

Open Export with the toolbar, `Ctrl+Shift+E`, or `Ctrl+E`. Choose JPEG, 8- or
16-bit PNG, or 8- or 16-bit TIFF; set JPEG quality, long-edge size, output
sharpening, and sRGB, Display P3, or Adobe RGB delivery color space. Resizing is
the last step, uses Lanczos3, preserves aspect ratio, and never enlarges a
smaller image.

CLI exports use q95 sRGB. `--long-edge N` is available on `apply`, `auto`, and
`batch --render`; `0` or omission means full resolution. It is deliberately an
export option rather than a recipe field, so one recipe can deliver both a
master and a web copy.

## CLI reference

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
autoshop match <src> <target> [--render] [--zoned] [--regions 2..4] [--style-prompt] [--ai-judge] [--deep] [-o|--out FILE]
autoshop correspond <source> <target> [-o|--out FILE]
autoshop retouch <src> --mask FILE --prompt TEXT [--quality low|medium|high|auto] [--full-res] [-o|--out FILE]
autoshop heal <src> [--mask FILE] [--no-auto] [--full-res] [-o|--out FILE]
autoshop serve <dir> [-p|--port N]
autoshop recipe-schema
```

`<src>` is a RAW or baked image. For commands that save develop state, baked
sources get recipe JSON but no RAW XMP. `auto` is `analyze` plus render.
`batch` analyzes RAWs by default, skips baked photos unless `--include-baked`
is set (avoiding duplicate analysis and billing for RAW+JPEG pairs), and
defaults to three photos in flight; `--long-edge` on `batch` requires
`--render`. `eval` defaults to serial work and resumes from its state file.
Denoise-strength/model overrides require `--denoise` on `auto`.

`match` itself is local inverse rendering and needs no key. Its optional
`--ai-judge` and `--deep` review paths do; `--deep` permits one guided retry.
`heal` can use a supplied mask offline, while its automatic detector uses the
vision role.

## Lightroom and XMP interoperability

Autoshop reads and writes sidecar XMP for global settings, point curves, HSL,
crop, and supported local corrections; the writer merges owned fields into the
existing document and preserves unmodeled content byte-for-byte instead of
round-tripping the whole file through a general XML serializer. Linear and
radial masks round-trip as editable geometry. Lightroom brush dab streams are
imported from the sibling `MaskBrushTable`, validated and Brotli decoded, then
rendered with Autoshop's measured brush model. Classic XMP does not contain
Lightroom's computed subject/sky/object alpha or arbitrary bitmap alpha, so
Autoshop preserves the selection intent and clearly re-derives the mask with
its own local model; generated image variants remain generated pixels until
reverse-fit produces an editable recipe.

## Configure and use the AI features

Open **Settings** to configure the image/vision role and the analysis-verifier
role. The image role uses an OpenAI-compatible API for visual proposals and
generative images. The verifier defaults to the signed-in `claude` CLI over
OAuth, receives statistics and recipe data rather than image pixels, and can
instead use an API provider.

The same roles can be configured from the environment: `OPENAI_API_KEY` serves
the image/vision and generative role; `AUTOSHOP_ANALYSIS_API_KEY` is used only
when the verifier is set to API mode. Settings are saved in the per-user
`autoshop.local.json`; do not put real credentials in the repository. A
`./autoshop.local.json` in the current working directory may select
model/provider preferences but cannot supply API credentials, endpoints,
executable/script paths, or output destinations, so an opened photo folder
cannot become a credential or path override.

- **Analyze:** choose **Analyze** in the AI panel or run `autoshop analyze`.
  The vision advisor proposes bounded sliders and masks, a data-only verifier
  checks the proposal, and normal visual review may attempt one revision;
  `--deep` permits additional bounded rounds. Accepted output remains a normal
  recipe and XMP.
- **Style match/read:** build the style reference library from Lightroom
  RAW+XMP pairs with the GUI or `style-index`. The Style control retrieves
  similar prior edits and applies their settings with `style_pull` (0.18 at
  the shipped Style 0.3, full at Style 1.0); Strength independently controls
  the fit budget and confidence cap.
- **Reimagine:** enter a prompt in the AI panel or use `reimagine` to create a
  generated, lower-resolution target. `--fidelity high` (the default, and the
  GUI's mode) tells the model to re-develop the same photograph, not repaint
  it. The structural divergence **D** against the sent input is disclosed;
  `D ≥ 0.35` warns that a reverse-fit of that result will fall back to
  Atmosphere mode, and the opt-in `--fidelity-retry` (a GUI checkbox as well —
  off by default, it buys a second image) regenerates once and keeps the
  closer result. Use **Reverse-fit** or `match` to infer a deterministic recipe
  and apply it to the original RAW at full resolution.

Local denoise and segmentation do not need an API key. Their Python sidecars
resolve relative to the installed program tree, and downloaded weights are
kept in the local cache rather than committed to the repository.

## Privacy, trust, and paid-feature boundary

| Runs locally without an API key | Uses the configured vision/generative API role |
|---|---|
| Deterministic render and manual develop, including `apply` | Full vision-backed `analyze` / `auto` proposals and visual model review |
| Local `match` inverse rendering | `match --style-prompt`, `--ai-judge`, or `--deep` |
| XMP read/write, masks, curves, and GUI sliders | Generative `reimagine` / `retouch` |
| SCUNet denoise and local BiRefNet/U²-Net, OneFormer, and SAM masks | Automatic target detection in `heal`; a supplied mask works offline |
| Style indexing and retrieval | |

Without the vision role, the advisor can fall back to its disclosed histogram
heuristic, which is not equivalent to the full vision-backed feature. The
data-only verifier defaults to the signed-in `claude` CLI over OAuth, so it does
not require an API key, although provider-backed operations may still consume a
subscription or incur charges.

Photos leave the machine only for AI operations the user requests through a
configured provider. The verifier receives recipe, EXIF, histogram, clipping,
and rationale data—not pixels—and Responses request bodies set `store:false`.
The local web UI binds to loopback only, checks Host/Origin and cross-site
requests, requires a fresh per-run session token for state changes, disables API
caching, and denies framing. By default, Autoshop keeps the source library
read-only. If the configured Delivery folder is inside or above a photo's
folder, that delivery subtree is intentionally writable; Settings warns when
this removes the folder's protection. "Export .xmp beside the photo" is the
separate, confirmed per-photo sidecar exception.
