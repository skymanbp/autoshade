<div align="center">
<img src="assets/icon.png" width="104" alt="Autoshop icon" />

# Autoshop

**AI-assisted automatic development of RAW photographs.**

Point it at a RAW (or an exported PNG/TIFF); an AI advisor decides the develop
adjustments and a deterministic Rust engine applies them — or writes an XMP
sidecar so the edit opens as adjustable sliders in Lightroom.
</div>

---

## The idea

The judgement-heavy part of developing a photo is *deciding what to change*
(this sky is blown, those shadows are crushed, the white balance is too cool).
The mechanical part is *applying* it. Autoshop splits exactly there:

```
 RAW ─► decode + features ─► [GPT vision advisor] ─► EditRecipe ─► [Claude verify] ─► Rust render engine
  .ARW    preview+EXIF+hist        looks at photo        JSON          QA / accept           │
                                                                                             ▼
                                                                    XMP sidecar  +  16-bit master
```

**The AI never touches a pixel** in the main path. The vision model only emits an
[`EditRecipe`](src/recipe.rs) — a small, bounded, Lightroom/ACR-style JSON of
slider values — and a deterministic engine renders from the original RAW. That
keeps results reproducible, non-destructive, auditable, and free of hallucinated
detail. (Three *opt-in*, clearly-labelled exceptions touch pixels: AI **denoise**,
**generative** retouch, and **pixel heal** — see below.)

## Features

- **One-shot develop** — `auto` decodes a RAW, asks GPT for an `EditRecipe`, has
  Claude acceptance-verify it, then renders a **16-bit TIFF** master. Since
  v0.26.2 the interactive surfaces (GUI/web/CLI `analyze`/`auto`) close the
  loop **visually**: the proposal is rendered and judged by the vision model,
  which can buy one guided revision — adopted only if it re-scores at least
  as high (batch/eval skip this; the rationale discloses every branch).
- **XMP sidecar** — the same recipe serialises to an ACR/Lightroom `.xmp`
  (global sliders + local linear/radial masks), so the AI's edit opens as
  fully-adjustable sliders in your catalog.
- **AI Denoise (SCUNet, GPU)** — ACR/LR-style denoise for high-ISO / astro
  frames. Off by default, triggered by a flag, a CLI command, or a UI button.
- **PNG/TIFF source mode** — feed an already-processed image (e.g. denoised in
  Lightroom/Photoshop) and Autoshop grades it directly. Auto-detected by file
  type; no RAW required. Embedded ICC profiles (LR "Edit in…" exports ProPhoto
  16-bit by default) are normalised into the sRGB working space — 16-bit depth
  preserved — instead of being read as if they were sRGB.
- **Desktop app (native GUI)** — `autoshop-gui`: a library grid that marks which
  photos already have a saved develop, the full develop panel (tone, presence,
  curves, 8-band HSL, colour grading), local masks (linear / radial / brush /
  AI-selected — rotatable radials, add/subtract/intersect shape composition,
  per-mask eye toggle & duplicate, brush-editable AI rasters with
  feather/expand/contract and full-resolution guided refine), crop &
  straighten, spot heal, reverse-fit, before/after, per-photo version
  snapshots and variant strips that survive a reopen (`variants.json`), and
  Ctrl+S to a Lightroom XMP. English / 中文.
- **Web UI** — `serve` opens a local gallery: pick a photo, Analyze, tweak the
  develop sliders (tone, presence, curves, 8-band HSL, colour grading) with live
  before/after, give a text direction, export.
- **Style retrieval** — learns from *similar* past edits you've made (k-NN over
  EXIF + histogram) and offers them to the advisor as soft reference. Build the
  library from all three surfaces — the GUI's **AI panel › Style reference
  library** (folder picker + background build with progress), the web info
  panel, or `autoshop style-index <dir>` — and the panel says which library is
  in use, how many of your edits it holds and how old it is. Every analysis
  names the shots it actually leaned on in the rationale; an optional switch
  also *shows* the vision model the single most similar past photo (off by
  default — it bills a second image per call).
- **Look matching (reverse-fit)** — `match` takes the same frame twice (your
  source and a finished rendition of it: a `reimagine` output, an exported JPEG,
  any reference of that shot) and *solves* for the `EditRecipe` that reproduces
  it through the engine — CDF tone matching, then saturation, then colour cast.
  No pixels are copied, so the fit applies at full sensor resolution and writes
  recipe.json + XMP. Deterministic, **no API key needed**. Opt-in **AI review**
  (GUI checkbox / `--ai-judge`): the vision model scores how faithfully the
  fitted render matches the target (0-100 + critique — LLM as a judge; one
  paid vision call, informational only).
- **Generative (experimental)** — `reimagine` / `retouch` via OpenAI Images.
- **Pixel retouch / Heal (experimental)** — an optional mode where the AI removes
  dust / blemishes / specks by healing from SURROUNDING REAL pixels
  (deterministic; *not* generative — it invents nothing). Hybrid targeting: AI
  auto-detect + paint-to-add. Writes a pixel master to `./out`.
- **Batch** the whole library, **eval** the AI against your own edits.
- **Your library stays read-only** — the engine refuses to write into a source
  RAW's folder. Exports (developed/heal/retouch images) go to `./out` by
  default; in the GUI the delivery folder is a setting (**Export → Destination**:
  `./out`, the last folder used, or ask every time) that both 「Export」 and
  「Render selected」 follow, so a library comes out in one folder instead of two.
  The one deliberate exception is the ▾ beside Export: it delivers a single file
  to a path you pick for that export only and leaves the setting alone. Develop
  STATE (recipes, Lightroom XMP, version snapshots, mask rasters, the baked
  pixel-master link and the GUI's variant strip) lives in a per-user develop
  store (`AUTOSHOP_DATA_DIR`, else
  `%LOCALAPPDATA%/autoshop/develops/<stem>-<path hash>/`), so two same-named
  photos never collide and edits survive launching from any directory.
  The one deliberate exception to read-only: 「Export .xmp beside the photo」
  copies a RAW's stored Lightroom/ACR sidecar into the photo's own folder,
  because that is the only place Lightroom reads one. It is a per-photo click,
  never batched, and it refuses to replace an existing `.xmp` without a second
  confirming click.

<div align="center"><img src="assets/denoise-demo.png" width="520" alt="AI denoise before/after" /></div>

## Quick start

```bash
cargo build --release                                    # CLI → target/release/autoshop(.exe)
cargo build --release --features gui --bin autoshop-gui  # desktop app → autoshop-gui(.exe)
cargo test                                               # unit + engine tests
```

The GUI sits behind the `gui` feature so a plain `cargo build` / `cargo test`
never pulls in eframe + winit + the GL stack. Both binaries link the same engine
library, and each tagged release ships both prebuilt for Windows.

Then any of:

- **Desktop app:** run `autoshop-gui` (or the released `.exe`) and point it at a
  folder — the full editor, no server and no browser involved.
- **Web UI:** double-click `Autoshop-UI.bat` (Windows) — it serves your
  library and opens the browser. Or: `autoshop serve "D:\path\to\photos" --port 8080`.
- **CLI:**
  ```bash
  autoshop auto "photo.ARW" --guidance "warm golden-hour, lift shadows"
  ```

## Commands

```
autoshop decode  <src>                       # preview + EXIF + histogram
autoshop analyze <src> [--guidance "..."]    # AI → recipe.json + .xmp (no render; incl. visual review loop)
autoshop apply   <src> <recipe.json> -o out  # render a recipe to an image
autoshop auto    <src> [--denoise] [--guidance "..."]   # analyze + render, end-to-end
autoshop denoise <src> [--strength 0..1] [--model ...]  # AI denoise → clean 16-bit master
autoshop batch   <dir> [--render] [--limit N]           # process a whole folder
autoshop eval    <dir> [--limit N]           # compare AI edits vs your own .xmp
autoshop style-index <dir>                   # build the "your taste" reference index (also in the GUI: AI panel › Style reference library)
autoshop serve   <dir> [--port 8080]         # local web UI
autoshop reimagine <raw> --prompt "..."      # experimental generative restyle
autoshop match   <raw> <target> [--render] [--zoned] [--ai-judge]  # reverse-fit a look → editable recipe + XMP (no key; --ai-judge scores the match, needs key)
autoshop retouch   <raw> --mask m.png --prompt "..."    # experimental generative object removal
autoshop heal      <src> [--mask m.png] [--no-auto]     # pixel heal: spot/blemish removal (NOT generative)
autoshop recipe-schema                       # print the EditRecipe JSON shape
```

`<src>` is a RAW (`.arw/.dng/...`) **or** a baked image (`.png/.tif/.jpg`) — the
develop pipeline runs on either. RAWs also get an `.xmp`; baked sources get
`recipe.json` only (XMP is meaningful only for RAW in Lightroom).

## AI setup

Two roles, each configurable in the in-app **Settings (⚙)** panel (or via env):

| Role | What | Default | Other option |
|------|------|---------|--------------|
| **分析 / Analysis** (verifier) | data-only acceptance-check of each recipe | **OAuth** — the `claude` CLI on PATH, signed in (reuses Claude Code OAuth, **no API key**), model `opus` | **API** — any OpenAI-compatible chat endpoint (base URL + key + model) |
| **图像 / Image** (vision advisor) | looks at the photo → `EditRecipe` | **API** — `OPENAI_API_KEY`, model `gpt-5.5` | point the base URL at any OpenAI-compatible **vision** endpoint. Without a key, a histogram heuristic is used. |

The `claude` CLI has no image input in print mode, so the image role always
speaks the OpenAI-compatible HTTP protocol. The Settings panel's "OAuth"
choice for the image role is a preset that points the endpoint at a local
subscription bridge (e.g. CLIProxyAPI) instead of a keyed API — same wire
protocol, different endpoint/credentials.

Configure keys + models from the **Settings** panel — written to
`autoshop.local.json` in your per-user Autoshop folder (never in a repo), which
overrides the environment. Keys never leave your machine: the server binds
`127.0.0.1`, refuses any request whose `Host`/`Origin` is not that exact
loopback authority *on its own port*, requires the page's per-run session
token on every state-changing request, marks all `/api/*` responses
`no-store`, never returns a key (settings answer `key_present: true/false`),
and sends `X-Frame-Options: DENY` so a page you visit cannot frame the UI.
You can also use `.env`:

```
OPENAI_API_KEY=sk-...
```

**A settings file in the current directory is not trusted with your key.**
Autoshop still *reads* a `autoshop.local.json` sitting in the folder it was
launched from — that is the pre-store layout, kept so old setups keep working —
but from v0.18.0 such a file may only choose models and providers. Its API-key
and base-URL fields are ignored, with a warning, because settings resolve per
**field**: a file supplying just `image_base_url` would have redirected the
endpoint while your real key still came from `.env`, and the next Analyze would
have posted that key to whoever wrote the file. Extracting a shared archive of
photos and running Autoshop inside it was enough. Your own settings, saved from
the panel into your user profile, are unaffected.

The same rule now covers two routes that were still open. **Saving settings no
longer launders an ambient file into your trusted one** — both settings writers
read-merge-write, so a save used to copy a planted `image_base_url` into your
user profile, where nothing strips it again and one ordinary "save" undid the
whole guard. And **a `.env` may no longer choose the endpoint**: `dotenvy`
searches the working directory *and every parent*, and its override mode beats
a variable you really set, so a `.env` dropped beside shared photos could
redirect your key exactly as the settings file could.

Since v0.23.2 that rule is stated **once**, as a property of each setting
rather than as three hand-kept lists (`config::SETTINGS`). Every setting is
one of three kinds:

| kind | examples | who may set it |
|---|---|---|
| **secret** | `OPENAI_API_KEY`, `AUTOSHOP_ANALYSIS_API_KEY` | your environment, your `.env`, the Settings panel |
| **destination** | the two base URLs, `AUTOSHOP_CLAUDE_BIN`, `AUTOSHOP_PYTHON`, the sidecar scripts and weight cache, `AUTOSHOP_DATA_DIR`, `PATH`, `PYTHONPATH`, `PYTHONHOME`, `ANTHROPIC_*` | your environment and the Settings panel **only** |
| **preference** | every model id, both provider selectors, both reasoning-effort tiers, the tuning numbers | anything, including an ambient file |

So keys and **models** from a `.env` work — that is where this project's own key
lives — while nothing ambient can name where bytes go, which account pays, or
what program runs. Only a destination coming from a `.env` is ignored, and only
that prints a warning; a `.env` naming a model no longer warns about anything,
which it used to (and which contradicted this page).

One place the table deliberately does **not** apply: what a `.env` may push
into the child processes (the `claude` verifier and the two Python sidecars).
There, "unlisted" is not a closed set — it includes every loader hook the
platform defines — so that block is an **allowlist** of compute knobs
(`CUDA_VISIBLE_DEVICES`, thread counts, the offline flags) rather than the
complement of a denylist. Anything else a `.env` names is dropped, and the app
says which. Your own environment is unaffected: the children inherit it, so an
`HF_HOME` or `HTTPS_PROXY` you set yourself still reaches them.

A saved key also remembers **which endpoint it was saved for**. The two image
providers share one key slot, and flipping to OAuth swaps the base URL to the
local bridge — which used to leave the cloud key armed, so the next call sent
`Authorization: Bearer <cloud key>` to whatever listened on the bridge port
(and a saved bridge token rode to the cloud on the flip back). Each save now
records the base the key was typed beside, and the key is simply not sent
anywhere else: flip back and it works again; move endpoints and Settings says
"no key set" until you enter one for that endpoint. Keys from your environment
are exempt — that pairing is your own. Keys saved before v0.23.2 have no
recorded home and keep working everywhere until a save records one (any save
that doesn't change the base, or a re-type, does).

Two consequences worth naming. A `.env` may switch `AUTOSHOP_ANALYSIS_PROVIDER`
from `oauth` to `api` — but not the endpoint or, therefore, where the request
goes; and it could already supply `OPENAI_API_KEY`, which is the strictly
larger capability. And on Linux/macOS the store root now resolves through
`$XDG_DATA_HOME` / `$HOME/.local/share`; if neither exists the fallback is the
shared temp folder, and a settings file found **there** is treated as ambient
too, because any account on the machine could have written it first.

**White balance renders slightly differently.** The blackbody curve behind the
Temp slider is a published piecewise fit whose branches did not meet: at
6600 K green jumped 1.31 % and blue 0.96 %, and red sat clamped flat from 6600
to 6688 K, so an 88 K band carried no temperature signal at all for the
eyedropper to solve against. The branches are now rescaled to meet exactly.
The cost is that white-balance gains change. Measured against a 5500 K shot,
as the change in the gain each channel receives:

| Temp target | red | blue |
|---|---|---|
| 2000 K (candle — the slider's floor) | 0 % | **−4.43 %** |
| 2500 K | 0 % | −0.67 % |
| 3000–5000 K | 0 % | −0.32 % … −0.03 % |
| 6500 K and below | **0 %** | +0.03 % |
| above 6600 K (any cool target) | **+3.19 %** | **+2.35 %** |

Red is untouched at and below 6500 K because that branch was already clamped
there. The two ends move most: cool targets by the seam repair, and the very
warm end because the blue branch crosses zero at 1900 K, so a small absolute
change is a large relative one right at the slider's floor. Re-exports of older
work at those settings will differ by that much, and this change has **not**
had visual acceptance.

## AI Denoise setup

The denoiser is a small Python sidecar ([`python/denoise.py`](python/denoise.py))
running **SCUNet** on the GPU. It needs Python with:

```bash
pip install torch --index-url https://download.pytorch.org/whl/cu128   # CUDA build
pip install opencv-python numpy einops requests
```

On first use it downloads the model weights (~69 MB/model) into `python/weights/`
(gitignored). The sidecar script and its weight cache are resolved against the
*program's own directory tree*, never the folder you run from — a downloaded
photo pack cannot substitute its own script or weights. Trigger it via
`autoshop auto --denoise`, `autoshop denoise <src>`,
or the **AI Denoise** checkbox in the web UI. Models: `color_real_psnr` (default,
blind, best for real high-ISO/astro), `color_real_gan`, `color_15/25/50`.

## Configuration (env vars)

The AI-provider rows below are also settable in the **Settings (⚙)** panel; the
per-user `autoshop.local.json` (in the Autoshop data folder) overrides the
environment. The two sidecar knobs (`AUTOSHOP_PYTHON`,
`AUTOSHOP_DENOISE_MODEL`) are environment-only — the panel does not carry them.

| Variable | Default | Purpose |
|----------|---------|---------|
| `OPENAI_API_KEY` | — | image (vision) advisor + generative key |
| `AUTOSHOP_OPENAI_MODEL` | `gpt-5.5` | image/vision model id |
| `AUTOSHOP_OPENAI_BASE_URL` | `https://api.openai.com/v1` | image API base (any OpenAI-compatible) |
| `AUTOSHOP_OPENAI_IMAGE_MODEL` | `gpt-image-1.5` | generative (retouch/reimagine) model |
| `AUTOSHOP_ANALYSIS_PROVIDER` | `oauth` | verifier provider: `oauth` (claude CLI) or `api` |
| `AUTOSHOP_ANALYSIS_MODEL` | `opus` | verifier model (claude alias for oauth; chat id for api) |
| `AUTOSHOP_ANALYSIS_API_KEY` | — | verifier key when provider = `api` |
| `AUTOSHOP_ANALYSIS_BASE_URL` | `https://api.openai.com/v1` | verifier API base when provider = `api` |
| `AUTOSHOP_IMAGE_EFFORT` | — | image-role reasoning effort; blank ⇒ the provider decides |
| `AUTOSHOP_ANALYSIS_EFFORT` | — | verifier reasoning effort; blank ⇒ the provider decides |
| `AUTOSHOP_PYTHON` | `python` | interpreter for the denoise sidecar |
| `AUTOSHOP_DENOISE_MODEL` | `color_real_psnr` | default SCUNet weights |

Reasoning effort is a **suggestion, not a contract**: the tiers differ per
provider (the `claude` CLI documents `low, medium, high, xhigh, max`;
OpenAI-compatible endpoints take the first three), so the Settings pickers offer
the right list beside a free-text field, and an endpoint that does not know the
tier is automatically retried without it. Blank means "send no such parameter",
which is the only correct request for a model that does not reason — and a
blank **saved in Settings** is a real choice: it silences an
`AUTOSHOP_*_EFFORT` from the environment instead of falling through to it,
because the local file wins over the environment here as everywhere. On
`/responses` the tier and the liveness summary stream ride inside one
`reasoning` object, and the retry drops whichever child the endpoint **named**:
a refused tier costs the tier, not the progress stream that keeps a long
reasoning phase from looking like a stall.

The model lists beside those pickers are suggestions too. They are whatever the
endpoint's own `/models` returned, minus the ids recognisable as something else
(audio, embeddings, rerankers, image generators, the two legacy
completion-only models). A name can only rule things OUT, so appearing in the
list is not a promise that a model can read the proposer's image — a wrong pick
fails loudly, and unbilled, on the first call, and the free-text field beside
the list is there for everything the filter has never heard of.

## Honest scope

- Render ops are tasteful **approximations** of Lightroom, not bit-exact. The
  XMP→Lightroom path renders them faithfully in the meantime.
- AI denoise runs on the demosaiced RGB (not the raw Bayer mosaic like Adobe
  Denoise), and ~3 min for a 60 MP frame on an RTX 4060 Ti. Excellent, not
  identical to Adobe.
- Kelvin white balance is **absolute** only on a RAW, where the as-shot
  temperature is read from the camera metadata and stamped into the develop. A
  baked PNG/TIFF has no as-shot reading, so the same slider anchors at 5500 K
  and acts as a relative shift from there — it moves pixels, it just cannot
  claim to be the scene's true colour temperature.
- Generative `reimagine` is a low-res, lossy re-render — an experiment, not a
  master. `retouch` (generative fill) regenerates only the masked region and
  composites it back onto the source's own develop with a feathered seam, so the
  rest of the frame keeps the original pixels. That base is the **engine's own
  neutral develop** — capped at 2048 px on the long edge by default — never the
  camera's embedded JPEG, so the composite stays on the same tone chain as the
  canvas. A baked PNG/TIFF is likewise thumbnailed to 2048 px. Pass `--full-res`
  (CLI) or tick **Full-res** (UI) to composite at full resolution instead
  (~60 MP for a RAW; slow — only the small removed patch is upscaled). Both pick
  an aspect-correct size (no square-squash) and default to `quality=high`
  (override `--quality`).
- Pixel **heal** (`autoshop heal` / the UI's 去瑕疵 panel) is the *non*-generative
  retoucher: it removes only SMALL defects (dust / blemishes / specks) by sampling
  surrounding REAL pixels — it never invents content. Best on fairly uniform
  backgrounds (sky, skin, wall, water); busy backgrounds heal approximately. AI
  auto-detection needs the vision key; the paint-a-mask path works offline. Runs
  on a ≤2048 px base by default — the engine's own neutral develop for a RAW,
  the source thumbnailed for a baked image; `--full-res` heals at full
  resolution instead (slow), on either source type.

## Tech

Rust (rustc/cargo 1.94) · `rawler` (Sony ARW decode) · `image` · `rayon` ·
`clap` · `serde` · `ureq` · `eframe`/`egui` (desktop GUI) · `tiny_http` (web UI) ·
Python + PyTorch + SCUNet (denoise) · Claude CLI (verifier) ·
OpenAI Responses + Images (advisor + generative).

See **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** for the full design.
