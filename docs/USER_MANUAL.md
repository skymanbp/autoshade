# AutoShade user manual

The operating manual for AutoShade v1.0.0 — the desktop app, the CLI, and the
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
is read-only: AutoShade stores develop state separately and never rewrites the
source RAW. The viewer applies EXIF orientation before crop and mask geometry,
so every tool works in the displayed frame. The neutral view is AutoShade's own
conversion, not the camera JPEG; histogram and clipping information are
computed from the decoded image and also feed the AI verifier.

## 2. Develop the image

The Develop panel exposes white balance, exposure and tonal controls, RGB point
curves, HSL, color grading, texture, clarity, dehaze, noise reduction,
sharpening, vignette, crop, and lens-related settings, rendered through the
same engine as `autoshade apply`.

**Save develop** (`Ctrl+S`) persists the recipe and, for a RAW, its XMP
projection in the per-user develop store (`%LOCALAPPDATA%\autoshade` on
Windows). Upgrading from Autoshop v1.1.0 or earlier moves the old
`%LOCALAPPDATA%\autoshop` folder there on first launch, in one step, and says
so; if a folder of the new name already exists nothing is moved or merged and
the app says that instead. A settings file still named `autoshop.local.json`
keeps being read until the next time settings are saved.
A neighboring Lightroom/ACR `.xmp` is
read only as the merge base; Save does not overwrite it. A baked image keeps an
AutoShade recipe but does not receive a RAW XMP. To deliver the stored
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
autoshade decode <src> [-o|--out FILE]
autoshade analyze <src> [-o|--out FILE] [--guidance TEXT] [--style 0..1] [--strength 0..1] [--adherence 0..1] [--embed|--no-embed] [--deep] [--reference-image]
autoshade apply <src> <recipe.json> (-o|--out) FILE [--long-edge N]
autoshade auto <src> [-o|--out FILE] [--guidance TEXT] [--style 0..1] [--strength 0..1] [--adherence 0..1] [--embed|--no-embed] [--deep] [--reference-image] [--denoise] [--denoise-strength 0..1] [--denoise-model NAME] [--long-edge N]
autoshade denoise <src> [-o|--out FILE] [--strength 0..1] [--model NAME]
autoshade batch <dir> [--render] [--limit N] [--include-baked] [--jobs N] [--long-edge N]
autoshade eval <dir> [--xmp-dir DIR] [--limit N] [--jobs N] [--fresh] [--state FILE]
autoshade style-index <dir> [--xmp-dir DIR] [--embed|--no-embed] [--describe]
autoshade style-index --looks <dir> [--embed|--no-embed] [--describe]
autoshade style-query <photo> [--direction TEXT] [--style 0..1] [--embed]
autoshade reimagine <src> --prompt TEXT [--fidelity high|low] [--quality low|medium|high|auto] [--fidelity-retry] [-o|--out FILE]
autoshade match <src> <target> [--render] [--zoned] [--regions 2..4] [--strength 0..1] [--style-prompt] [--ai-judge] [--deep] [-o|--out FILE]
autoshade correspond <source> <target> [-o|--out FILE]
autoshade retouch <src> --mask FILE --prompt TEXT [--quality low|medium|high|auto] [--full-res] [-o|--out FILE]
autoshade heal <src> [--mask FILE] [--no-auto] [--full-res] [-o|--out FILE]
autoshade serve <dir> [-p|--port N]
autoshade recipe-schema
```

`<src>` is a RAW or baked image. For commands that save develop state, baked
sources get recipe JSON but no RAW XMP. `auto` is `analyze` plus render.
`batch` analyzes RAWs by default, skips baked photos unless `--include-baked`
is set (avoiding duplicate analysis and billing for RAW+JPEG pairs), and
defaults to three photos in flight; `--long-edge` on `batch` requires
`--render`. `eval` defaults to serial work and resumes from its state file.
Denoise-strength/model overrides require `--denoise` on `auto`.

### Where your `.xmp` sidecars are — `--xmp-dir`

`style-index` and `eval` pair each RAW with the `.xmp` sidecar your editor
wrote. By default that is the file **beside the RAW**, which is where Lightroom
and ACR put it. If yours live somewhere else — an exported catalogue, or a
photo volume you cannot write to — point `--xmp-dir` at that folder and three
places are searched, in order:

1. `<xmp-dir>/<the RAW's folder relative to `<dir>`>/<name>.xmp` — a sidecar
   tree that **mirrors** your library;
2. `<xmp-dir>/<name>.xmp` — one **flat** folder of sidecars for a nested
   library;
3. `<the RAW's own folder>/<name>.xmp` — beside the RAW, as before.

The extension matches in any case (`.xmp` or `.XMP`) on every platform,
Windows and macOS alike. The stem does not: a sidecar belongs to the photograph
whose name it carries exactly.

`style-index` now also says which RAWs it **skipped** for want of a sidecar —
the count, and the first ten by name. A library that indexed 40 of 2,000
photographs used to look exactly like a library of 40.

### Rebuilds only measure what changed

A `style-index` build caches what it measured per photograph — the 14 camera
features, the SigLIP image vector, the vocabulary scores, the description and
its text vector — in `style-exemplars.json` beside the index, keyed by the
content of the frame it measured (the same key the description cache has always
used). A rebuild:

* **reuses** a photograph whose file is unchanged and whose cached answers cover
  the passes you asked for — no decode, no model call at all;
* **recomputes** anything new, edited, moved or rotated, and anything whose
  cached answers came from a different checkpoint, phrase list or prompt;
* **retires** entries for photographs that have left the library;
* still re-reads every `.xmp`, because your sliders, curve, colour families and
  mask habit can change without the pixels changing;
* still recomputes the index's normalisation over the **whole merged set** —
  adding one photograph legitimately moves the mean and deviation of every
  normalised dimension, so those are never cached.

Each build prints one line saying so:
`style index cache: reused N, recomputed M, removed K, skipped-for-sidecar S`.
A rebuild where every photograph is reused starts no model sidecar at all —
neither the 1.5 GB SigLIP checkpoint nor the 4.3 GB Qwen one is loaded. The
cache is only ever a saving: deleting `style-exemplars.json` costs time, never
correctness, and a corrupt or foreign one is rebuilt with a printed reason.

**Built your index on v1.2.0 or v1.2.1?** If your library carries any HSL or
colour-grade edit, that index was written correctly but refused on load
(`exemplar 0 has an unsupported setting key`), and the Style control read
nothing. Run `style-index` once on v1.2.2; the build is the same, only the read
was wrong.

`style-index --looks` builds the separate finished-photo look library; it never
adds camera features or develop settings to those records, and it is capped at
**500** finished photos (a curated set of reference grades, not an archive —
the RAW half's 5,000-exemplar cap and this one share a 228 MiB index envelope).
`style-query` is an offline diagnostic that prints the weights in force, the
exact retrieval terms behind every ranked neighbour and look — each weighted
term beside the raw cosine it came from — each neighbour's local-work counts
(`masks=… sky=… subject=…`), and the proposer reference
blocks, including the explicit reason a look library is unreachable when no
embedding vector is available.

A RAW index build also reads the **masks** in each sidecar, and summarises them
as a habit: how many you enabled, how many carry a Range Mask, and per use —
sky, subject, foreground, range, other — a count and the average strength of
ten local sliders (in-mask temperature and tint included), plus whether the
mask carries its own local curve. It reaches the proposer as one sentence ("3 of 4 mask the
sky (linear from the top: exposure -0.6 EV, highlights -25) …"), so the AI
places its own masks the way you place yours. **No mask shape is copied or
averaged**: geometry belongs to one frame, and only counts and slider averages
cross between photographs. The Range Mask count is taken from both the imported
recipe and the import's own refusal notes, because a Range Mask in an encoding
this engine does not model is dropped on the way in — counting only what
survived would report "none use range masks" about a library that plainly does. The build prints what it learned and, separately,
any mask content it could not read whole (an unresolvable AI mask, a brush
table it refuses) so the summary is never mistaken for a complete one. An index
built before this feature keeps working; its reference block simply says
nothing about local work, and nothing needs rebuilding to keep using it.

`--embed` opts into the local SigLIP 2 sidecar for that run and `--no-embed`
refuses it; either flag wins over the environment, and neither writes to it.

`--describe` adds the local **look-description** pass to an index build. A
second local model (Qwen3-VL-2B-Instruct, Apache-2.0) writes ONE short sentence
per photo about its *grade* — white balance lean, tonality, contrast,
saturation and colour treatment, finishing, mood — and never about the subject.
That sentence is what the SigLIP text tower embeds for that record, in place of
the fixed attribute tags, and it is what `style-query` prints beside the
`desc=` term and what the proposer's reference blocks carry after the tags.

The pass needs `--embed` (the prose only reaches the ranking through the text
tower), and the **first run downloads about 4.3 GB** of weights into
`python/weights/` — every file pinned to a 40-hex Hugging Face commit and gated
on its own sha256 and exact byte count. It is off by default on both front
ends. Nothing leaves this machine and nothing is billed. Descriptions are
cached by frame CONTENT in `style-descriptions.json` beside the index, so a
rebuild only describes the photographs that actually changed; editing the
prompt bumps a version that invalidates the cache rather than serving the old
prompt's answers for ever. In the desktop app the same switch is the *Describe
looks with the local vision model* checkbox, which stays greyed out until the
embedding checkbox above it is on.
`--adherence 0..1` picks the prompt tier the proposer and verifier are told:
`<=0.40` Hint, `0.40..0.70` Direct, above `0.70` Brief, default `0.65`
(Direct). It is prompt intent only — it never moves a render bound — and it
does nothing without a `--guidance` direction, which is why the desktop app
greys the slider out until Direction has text.

Every setting below is named `AUTOSHADE_*`. Up to v1.1.0 the app was called
Autoshop and these variables were named `AUTOSHOP_*`; the old spelling still
works everywhere, warns once naming its replacement, and is removed in the
release after this one. Where both are set, the `AUTOSHADE_*` one wins.

Six environment overrides steer retrieval, each read in exactly one place:

| Variable | Effect |
|---|---|
| `AUTOSHADE_STYLE_EMBED` | `1`/`0` — use the SigLIP sidecar. Set (any value) beats the GUI preference; `--embed`/`--no-embed` beats both. |
| `AUTOSHADE_STYLE_DESCRIBE` | `1`/`0` — run the local look-description pass during an index build. Set (any value) beats the GUI preference; `--describe` beats both. It never turns the embedding on by itself. |
| `AUTOSHADE_STYLE_EMBED_WEIGHT` | `W_EMB`, the query-image ↔ exemplar-image cosine block. `0` reproduces the 14-dimension ranking exactly. |
| `AUTOSHADE_STYLE_TEXT_WEIGHT` | `W_TXT`, the Direction-text ↔ exemplar-image term, scored after each exemplar's text hubness is subtracted. Ships at `0.5`: it spent one batch at `4`, where the corrected re-measurement showed the ranking collapsing onto a few hub exemplars; it shipped at `0` while the only query text available to the harness was a tag string. |
| `AUTOSHADE_STYLE_DESC_WEIGHT` | `W_DESC`, the Direction-text ↔ exemplar-description term. Ships at `0.5`. It shipped at `4` when both sides of the term were tag strings; with real prose that point measures *worse* than switching the term off, so it was re-fitted. |
| `AUTOSHADE_STYLE_LOOK_WEIGHT` | `W_LOOK`, the look-library image term. |
| `AUTOSHADE_SEND_REFERENCE_IMAGE` | `1`/`0` — also send the retrieved reference photo itself (not just its text) with `analyze`/`auto` proposals; `--reference-image` turns it on per run. Destination-trust: only your own environment or user-level settings can set it — a downloaded photo pack's `.env` cannot, because it decides whether your photograph goes on the wire. `batch` never sends one. |

All four weights parse the same way: trimmed, and taken only if finite and
non-negative — anything else falls back to the shipped default, because a
negative weight would rank the *least* similar photo first.

`match` itself is local inverse rendering and needs no key. Its optional
`--ai-judge` and `--deep` review paths do; `--deep` permits one guided retry.
`heal` can use a supplied mask offline, while its automatic detector uses the
vision role.

## Lightroom and XMP interoperability

AutoShade reads and writes sidecar XMP for global settings, point curves, HSL,
crop, and supported local corrections; the writer merges owned fields into the
existing document and preserves unmodeled content byte-for-byte instead of
round-tripping the whole file through a general XML serializer. Linear and
radial masks round-trip as editable geometry. Lightroom brush dab streams are
imported from the sibling `MaskBrushTable`, validated and Brotli decoded, then
rendered with AutoShade's measured brush model. Classic XMP does not contain
Lightroom's computed subject/sky/object alpha or arbitrary bitmap alpha, so
AutoShade preserves the selection intent and clearly re-derives the mask with
its own local model; generated image variants remain generated pixels until
reverse-fit produces an editable recipe.

## Configure and use the AI features

Open **Settings** to configure the image/vision role and the analysis-verifier
role. The image role uses an OpenAI-compatible API for visual proposals and
generative images. The verifier defaults to the signed-in `claude` CLI over
OAuth, receives statistics and recipe data rather than image pixels, and can
instead use an API provider.

The same roles can be configured from the environment: `OPENAI_API_KEY` serves
the image/vision and generative role; `AUTOSHADE_ANALYSIS_API_KEY` is used only
when the verifier is set to API mode. Settings are saved in the per-user
`autoshade.local.json`; do not put real credentials in the repository. A
`./autoshade.local.json` in the current working directory may select
model/provider preferences but cannot supply API credentials, endpoints,
executable/script paths, or output destinations, so an opened photo folder
cannot become a credential or path override.

**Python interpreter.** The AI sidecars run under a Python 3 that AutoShade
does not bundle. Settings carries a **Python interpreter** field with a
**Detect** button beside it; Detect looks in the standard install locations and
fills the field with the first one that actually RUNS — it executes
`--version` rather than trusting that a file exists, because a Mac without
developer tools has a `/usr/bin/python3` whose only behaviour is to offer to
install them. Finding none is reported as such rather than leaving the field
silently unchanged. Blank means the platform default: `python` on Windows,
`python3` elsewhere.

The field matters most on macOS, where an app launched from Finder inherits no
shell environment and the variable below therefore cannot be set for it at all.

| Variable | Effect |
|---|---|
| `AUTOSHADE_PYTHON` | The interpreter the sidecars are launched with. The same setting as the Settings field; the environment wins where both are set. |
| `AUTOSHADE_WEIGHTS_DIR` | Where all five sidecars keep downloaded model weights. Defaults to `weights/` beside the scripts — except inside a macOS `.app`, where it defaults into the develop store, because the bundle is signed and read-only. |

Both are *destination* settings — they name a program to execute and a
directory to write into — so neither may come from a `./autoshade.local.json`
sitting in the working directory. Only the environment or the per-user
settings file can supply them.

- **Analyze:** choose **Analyze** in the AI panel or run `autoshade analyze`.
  The vision advisor proposes bounded sliders and masks, a data-only verifier
  checks the proposal, and normal visual review may attempt one revision;
  `--deep` permits additional bounded rounds. Accepted output remains a normal
  recipe and XMP.
- **Style match/read:** build the RAW+XMP style reference library with the GUI
  or `style-index`, and optionally build a separate finished-photo look library
  with `style-index --looks`. The Style control retrieves similar prior edits
  and pulls the proposal toward them with `style_pull` (0.18 at the shipped
  Style 0.3, full at Style 1.0 — at 1.0 a control that has a target ends ON it).
  It pulls the twelve global sliders, the 8-band mixer's saturation and
  luminance, the colour-grade wheels, the master tone curve's shape and each
  mask's slider amounts — never a mask's position or size. A control is pulled
  only where your past edits AGREE on a direction; where they cancel out, or
  where you never touched that control, the AI's own choice for this photograph
  is kept. The rationale names every field that moved. Look records guide the
  proposer only. The embedding switch
  is opt-in and reports how many indexed records carry vectors. Strength
  independently controls the fit budget and confidence cap; Direction adherence
  chooses Hint, Direct, or Brief wording when a Direction is present.
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
caching, and denies framing. By default, AutoShade keeps the source library
read-only. If the configured Delivery folder is inside or above a photo's
folder, that delivery subtree is intentionally writable; Settings warns when
this removes the folder's protection. "Export .xmp beside the photo" is the
separate, confirmed per-photo sidecar exception.
