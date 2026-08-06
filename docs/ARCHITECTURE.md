# Autoshop — Architecture

> Status: **implemented** (v0.16.1). The full decode → advise → verify → render
> pipeline ships across TWO front-ends — a native desktop GUI (`autoshop-gui`,
> egui/eframe, which links this library in-process) and the local web UI
> (`serve`) — plus the CLI, AI denoise (SCUNet sidecar), the PNG/TIFF
> baked-source mode, style retrieval, XMP sidecars (global + local masks),
> experimental generative edits, and an optional pixel-**heal** retouch mode
> (§4.7). 197 library + 1 CLI + 26 GUI tests pass in both build configurations.
> This document describes the design; a few historical **[verify]** notes are
> left in place for provenance.
>
> Confirmed by the user (2026-06-25): Sony `.ARW`; output = XMP sidecar **and**
> rendered file (XMP-first); two AI roles behind one unified provider framework —
> **vision model (GPT) does image processing**, **Claude does non-image analysis
> + acceptance verification**.
>
> Since shipped, two *opt-in* pixel-level features were added alongside the
> parametric core: **AI denoise** (a Python/SCUNet GPU sidecar, run before
> tone/sharpen) and a **baked-source mode** (edit an already-exported PNG/TIFF,
> e.g. one denoised in Lightroom — auto-detected by file type).

## 1. The core idea

The expensive, judgement-heavy part of developing a RAW photo is *deciding what
to change* (this sky is blown, those shadows are crushed, the white balance is
2°°too cool, straighten the horizon). The mechanical part is *applying* those
decisions. So we split exactly there:

```
  RAW ─► decode + features ─► [vision advisor] ─► EditRecipe ─► [Claude verify] ─► render engine
   .ARW    preview+EXIF+hist     GPT (image)        JSON          QA / accept       │
                                                                                    ▼
                                                            XMP sidecar  +  rendered image
```

**The AI never touches a single pixel.** The vision advisor receives a small
preview image + metadata and returns an [`EditRecipe`](../src/recipe.rs) — a
bounded set of Lightroom/ACR-style develop controls. A deterministic Rust engine
renders from the original RAW using that recipe. Key benefits:

- **Reproducibility** — same recipe + same RAW ⇒ byte-identical output.
- **Non-destructiveness** — the recipe is a tiny JSON; originals are never modified.
- **Auditability** — every recipe carries a `rationale` + `confidence`.
- **Lightroom interop** — the recipe serialises to an XMP sidecar, so the edit
  shows up as adjustable sliders in your catalog.
- **No hallucinated pixels** — the AI can only turn the same knobs a human would.

## 2. The `EditRecipe` contract

Defined and unit-tested in [`src/recipe.rs`](../src/recipe.rs). Adobe-convention
ranges (sliders −100..=100, exposure in stops, temperature in Kelvin). Every
field is `#[serde(default)]`, so an advisor emits only the controls it moves.
`EditRecipe::clamp()` defends the render engine. `confidence` is **advisory**,
not a gate: it is carried in the XMP comment and shown to the user, while what
actually gates auto-save is the verifier's `Verdict` — a non-Accept verdict
never writes a develop (see §4.3).

Run `cargo run -- recipe-schema` to print a default `EditRecipe` **instance**
(useful for seeing the field set and defaults). It is not the advisor's output
contract: that is a JSON Schema built in `src/advisor/`, and it deliberately
excludes the engine-only fields the recipe carries for calibration
(`base_curve`, `as_shot_k`/`as_shot_tint`, the lens-profile block) — an advisor
must never emit those.

## 3. The unified AI provider framework (统一 API 框架)

A Rust trait abstracts the two RECIPE-producing calls — `propose` and `verify` —
so providers are interchangeable and transport-agnostic (an HTTP API, or
shelling out to the `claude` CLI). Two roles, each independently configurable in
the in-app **Settings (⚙)** panel. (The later pixel-side AI calls — generative
reimagine/retouch, heal spot-detection, style-prompt extraction, the denoise and
segmentation sidecars — talk to their endpoints directly rather than through
this trait.)

| Role | Provider options | Sees pixels? | Job |
|------|------------------|--------------|-----|
| **Image advisor** (图像) | OpenAI-compatible vision over HTTP (default `gpt-5.5`); Settings offers **API** (a real key) or **OAuth** (a local Codex bridge fronting a ChatGPT subscription) | **yes** (preview) | Look at the photo → emit an `EditRecipe`. The `claude` CLI has no image input in print mode, so this role never uses the claude OAuth path. |
| **Analyst / verifier** (分析) | **OAuth** (`claude` CLI, default model `opus`) **or** API (OpenAI-compatible chat) | **no** (data only) | Reason over EXIF/histogram; **acceptance-verify** the recipe (ranges sane? consistent with metadata & intent? confidence adequate?) and flag/veto bad recipes. |

> The verifier judges at the **data level** — recipe + histogram/clipping stats +
> the advisor's rationale — *without* re-doing vision.

Sketch (final shape):

```rust
trait Advisor {                      // one trait, many providers
    fn propose(&self, img: &Preview, meta: &Meta) -> Result<EditRecipe>;   // image role
    fn verify(&self, recipe: &EditRecipe, meta: &Meta) -> Result<Verdict>; // analyst role
}
// propose: OpenAiProvider (HTTP vision)        |  HeuristicProposer (no-key baseline)
// verify:  ClaudeProvider (claude CLI -p OAuth) |  OpenAiVerifier (HTTP chat, OpenAI-compatible)
```

Provider/model/key selection lives in `autoshop.local.json` (written by the
Settings panel) and/or `.env` — both gitignored; the local file overrides env.
That file lives in the per-user store root, not beside the checkout, so settings
do not depend on which directory the app was launched from (a cwd-relative
`autoshop.local.json` is still read as a legacy fallback).
The OAuth analysis path reuses Claude Code OAuth — **no API key needed**; the
image path and the API analysis path each need an OpenAI-compatible key.

## 4. Components & milestones

| ID | Component | Crate/tool (actual) | Status |
|----|-----------|---------------------|--------|
| M0 | Data model + CLI scaffold | `clap`, `serde`, `serde_json`, `anyhow`, `thiserror` | **done** |
| M1 | RAW decode + features (Sony ARW) | **`rawler` 0.7.2** (preview + EXIF + WB) | **done** |
| M1 | Unified provider framework + GPT advisor + Claude verifier | `ureq` (HTTP) + `claude` CLI | **done** |
| M2 | Deterministic render engine | `image`, custom tone/colour/WB/clarity/NR/sharpen ops | **done** |
| M2 | XMP sidecar writer (ACR `crs:`, global + local masks) | hand-rolled XML | **done** |
| M3 | `auto` end-to-end + batch | batch fixes its work list up front, then runs it through a bounded pool (≤3 worker threads); "pending" = no develop in the store (recipe.json or `<stem>.xmp`, central or legacy) | **done** |
| M4 | Style retrieval + eval harness (your edits as ground truth) | k-NN over EXIF+histogram; per-field MAE/bias | **done** |
| M5 | Local web UI | `tiny_http` + vanilla JS (gallery, live before/after) | **done** |
| V2 | AI denoise (high-ISO/astro) | Python sidecar → **SCUNet** on GPU, called from Rust | **done** |
| V2 | Baked-source mode (edit exported PNG/TIFF) | extension dispatch; develop runs on loaded pixels | **done** |
| V2 | Generative reimagine / retouch | OpenAI Images (`gpt-image-*`) | **done (experimental)** |
| V2 | Pixel retouch / heal (spot removal) | deterministic heal engine + vision spot-detect ([`src/retouch.rs`](../src/retouch.rs)) | **done (experimental)** |

### 4.1 RAW decode (M1)

Backed by **`rawler` 0.7.2** (chosen over the now-frozen `rawloader` for current
Sony body coverage + embedded preview + full EXIF; see [`src/decode.rs`](../src/decode.rs)).
It extracts the embedded JPEG preview (for the vision advisor + UI), a downscaled
histogram with clipping stats, and EXIF (camera/lens/ISO/shutter/aperture/
as-shot WB). Baked sources (PNG/TIFF/JPEG) skip this and load directly via the
`image` crate with neutral metadata.

### 4.2 Vision advisor — image processing (M1)

A vision-capable OpenAI model receives the preview + metadata and returns an
`EditRecipe` (JSON-schema-constrained output). Exact model id, request shape
(base64 vs URL), and structured-output mechanism are **[verify]** in M1 and
pinned in config — not hardcoded from memory.

### 4.3 Claude analyst / verifier (M1)

The verifier (analysis role) runs one of two providers (set in Settings):
**OAuth** — the `claude` CLI for non-image reasoning + acceptance verification via
`claude -p --setting-sources "" --strict-mcp-config --disable-slash-commands
--output-format json --model <opus>` (default `opus`), reusing Claude Code
OAuth — no API key. `--bare` is deliberately NOT used: since CLI ≥ 2.1.210 it
never reads the stored OAuth login ("OAuth and keychain are never read"), which
is exactly the v0.11.2 auth failure — see `src/advisor/claude.rs`; or
**API** — an OpenAI-compatible chat endpoint (`OpenAiVerifier`, `/chat/completions`)
sharing the same data-only prompt. Either returns a `Verdict` (accept / revise /
reject + reasons). A rejected recipe can trigger one revision round with the
vision advisor.

### 4.4 Render engine (M2)

Applies the recipe deterministically. Frame stage first: decode → EXIF
orientation → working-resolution cap → lens geometry (distortion/CA) →
straighten → crop. Then the pixel stage, in this order: anchored white balance →
lens-profile vignette → manual vignette → **dehaze in linear light** (before any
tonal work, so the airlight estimate cannot move when Exposure is dragged) →
tone LUT (exposure/contrast/whites/blacks/highlights/shadows, the tone curve and
the per-photo camera base curve composed into one table) → per-channel RGB
curves → 8-band HSL → colour grading → saturation/vibrance → local adjustments
(linear/radial/bitmap masks, each with its own tone, colour and noise
reduction).

Export adds the colour-space encode (sRGB / Adobe RGB / Display P3 / ProPhoto,
with a wide-gamut develop path), an optional long-edge resize (Lanczos3, never
upscales) and resolution-aware sharpening, then writes JPEG, TIFF or **PNG** —
PNG is a first-class 16-bit master here, since every heal / clone / denoise /
reimagine master is one.

### 4.5 XMP sidecar (M2) — primary deliverable

The recipe written as an ACR/Lightroom `.xmp` sidecar (`crs:` keys like
`Exposure2012`, `Contrast2012`, `Temperature`, `ToneCurvePV2012`), so the AI's
edit appears as fully-adjustable sliders in Lightroom — the "AI does 90%, I
nudge the last 10%" workflow.

Since v0.13.0 Autoshop does **not** write it next to the `.ARW`: the source
library is read-only, so the projection lands in the per-user develop store
(`<AUTOSHOP_DATA_DIR | %LOCALAPPDATA%/autoshop>/develops/<stem>-<hash of the
absolute path>/<stem>.xmp`), alongside `recipe.json` (the authoritative develop
state), version snapshots and mask rasters. Copy it beside the RAW when you want
Lightroom to pick it up. A Lightroom sidecar that already sits beside the RAW is
READ on open — the newer intent wins — and never overwritten.

### 4.6 Style / eval harness (M4)

The user's **finished edits** are ground truth. If they're Lightroom XMP/develop
settings, diff the AI recipe against them; if they're exported JPEGs, compare the
AI render perceptually. Lets us measure "does the AI match *how the user*
develops a shot?" and tune the advisor prompt accordingly.

### 4.7 Pixel retouch / heal (optional) — V2

A third, opt-in editing mode (`autoshop heal`, or the UI's **修图 · 去瑕疵** panel),
distinct from BOTH the parametric path (which never touches pixels) and the
generative path (which *synthesises* them). It does traditional **spot-healing**:
small defects (dust, sensor spots, blemishes, specks) are removed by sampling
SURROUNDING REAL pixels and blending them over the defect with a mean-corrected,
feathered patch (the "heal" vs "clone" distinction). By construction the engine
only ever copies / shifts / averages pixels that ALREADY exist — it never invents
content, so this stays *retouching, not generation* (the hard design constraint).

Targeting is hybrid: a vision model auto-detects small spots
([`detect_spots`](../src/retouch.rs), constrained by prompt + schema to small
spot-removals) and/or the user paints regions in the UI
([`plan_from_mask`](../src/retouch.rs) → connected components → circular targets);
both feed the deterministic [`heal_image`](../src/retouch.rs) engine. Donors are
auto-searched (the in-bounds neighbour whose surroundings best match the spot's
border) unless an explicit source offset is given. Output is a pixel master in
./out — **non-XMP** (pixel edits don't serialise to ACR) — and the develop
records it as its pixel source in `<store>/develops/<key>/pixels.json`, so every
later render, export and reopen applies the parametric recipe ON TOP of the
healed pixels instead of silently reverting to the untouched source. It runs on
the engine's own neutral develop for a RAW (a ≤2048px cap by default) or the
source thumbnailed to 2048px for a baked image — never the camera's baked
preview, so the healed master stays on the same tone chain as the canvas
develop; `--full-res` works at full resolution on either source type (slow).

## 5. Why Rust

Cross-platform, no GC pauses on large-image pipelines, first-class image crates,
single-binary distribution, trivial `std::process` shell-out to `claude`.
Toolchain in use: rustc/cargo **1.94.1** (verified locally).

## 6. Open questions

| # | Question | Status |
|---|----------|--------|
| 1 | **Image library path** (originals + finished edits) | resolved: passed per invocation (`batch <dir>`, `serve --dir`, `style-index <dir>`, the GUI folder picker) — no configured library root; develop state is keyed by each photo's absolute path in the per-user store |
| 2 | Camera / RAW format | resolved: Sony `.ARW` |
| 3 | Output target | resolved: XMP sidecar **+** rendered, XMP-first |
| 4 | AI roles | resolved: GPT=image, Claude=non-image+verify, unified framework |
| 5 | Exact meaning of Claude's "收货验证" (data-level vs pixel-level) | assumed data-level (§3) — confirm |
| 6 | How to feed the preview to the GPT vision API; `crs:` key set for ARW | **[verify]** in M1 (research underway) |
