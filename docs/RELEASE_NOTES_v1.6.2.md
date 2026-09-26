# AutoShade v1.6.2 — a denoised or stacked master is reverse-fitted under its photo's calibration on the command line, as the desktop app already did

Everything merged into `main` since v1.6.1 ships here: one fix to the reverse
fit, found while the desert-canyon showcase gained its fourth column, and the
document and site work of the same two days. Nothing new is added — this is a
patch release — and every change below carries the measurement that decided
it.

The design, the runs, the acceptance and this document are the maintainer's;
no external coding model wrote this release.

## The reverse fit

- **`match --negative <RAW>`: a baked master is fitted under the photo's
  calibration.** `match` on a RAW fits a neutral develop of the sensor frame
  with the photo's calibration — camera base look, lens profile, as-shot white
  balance — composed into the solve, and the desktop app fits a ◈ Denoised or
  ▦ Stacked card the same way, on the master's pixels under that calibration.
  `match` on a master `denoise` or `stack` had written to a file composed
  nothing: the file was a baked image, so it was fitted as it stood. On the
  reference pair (the desert canyon every release is gated on) the denoised
  master then read its sky at a structural divergence of **0.716** against the
  RAW's **0.649** — across the 0.65 zone line into the bounded atmosphere
  solver — and its render, without the lens profile, did not share the RAW's
  frame. With the RAW named, the same master reads the same room the RAW's fit
  reads: global 0.279 / 0.655 against the RAW's 0.278 / 0.658 at pixel and
  layout scale, sky **0.645** and the full solve, look error 0.127 → 0.050
  against 0.129 → 0.048 at confidence 0.25 on both (the fit's own residual,
  the reading the README quotes), the render in the RAW's frame (offset 0, 0 against
  the RAW's own render, template-matched), and on the showcase's convention
  sky ΔE 3.4, land 5.8, whole-frame mean |diff| 0.0239 against the target —
  0.0031 in mean |diff| and under one 8-bit code of mean shift per channel
  from "denoise, then apply the RAW's recipe", the route the showcase's fourth
  column takes. The flag is refused on a RAW source and on a negative that
  is not a RAW; both refusals and the parser are pinned in the CLI's own
  tests.
- **A RAW source keeps fitting its own sensor frame**, even when the app has
  recorded a retouch master for the photo: `match` on a RAW publishes into
  that RAW's saved develop and its Lightroom sidecar and clears the pixel
  link, as it always has, so what renders afterwards is the RAW and the fit
  must describe it. (A first draft of this release read the recorded master
  there; it was withdrawn before the tag for exactly that reason.)
- **The RAW path is unchanged.** The release gate re-fitted the reference
  pair on this build at Reverse-fit strength 65 %, 85 % and 100 %: at every
  strength the recipe (store paths normalised), every mask raster and both
  1000-px renders, with and without the masks, are byte for byte the ones
  the v1.6.1 gate was approved on.

## Documents and the site

- **The canyon showcase has a fourth column** (`c5402fa`, on `main` since the
  v1.6.1 release): the RAW denoised on its sensor mosaic by AutoShade's own
  weights, then the gate's recipe applied to that master, so the 1:1 crops
  show the denoiser beside the reverse fit. The literal denoise-then-refit
  route was measured on v1.6.1, set aside and disclosed in
  [SHOWCASE.md](SHOWCASE.md); that measurement is what found this release's
  fix, and the showcase now records what v1.6.2 does with the route.
- **The README's cut** (`b7ae2aa`, `c7c7a2f`, `3a25c1e`, `467ccf8`):
  1103 → 907 lines, and 896 once the Cornwall figure left; the sections on
  how the reverse fit decides merged into one, the tech-stack subsections to a paragraph
  each, every number and code span the README loses verified present in a
  linked document, the two sentence shapes `check_docs` pins restored.
- **The site's twelve overview cards** rewritten to one length, 32–46 words
  each, so the grid reads even; the Cornwall figure leaves README §1 and the
  site's Part A (its section stays in [SHOWCASE.md](SHOWCASE.md), the table
  row keeps its numbers), and both ledes count two pairs and point to the
  third (`4fa9dc9`).
- **The manual** gains `--negative` in the `match` synopsis and a paragraph
  in the CLI reference, after the `denoise` and `stack` text, on what
  `match` fits; **TECH_STACK**'s sentence on `match`'s source, which
  still described the embedded-rendition rule retired in v1.3.0, is
  corrected.

## Compatibility

No recipe, sidecar, store or weight format changes. The sidecar weights stay
pinned to the v1.6.0 `autoshade-raw-denoise-v2.pth`; no weights are re-shipped.
`match` on a RAW, or on a baked image without the flag, runs the code v1.6.1
ran; on a RAW the release gate below shows it byte for byte.

## Gates

Measured before the tag on the release code (`6bf357c`; the version bump
touches Cargo.toml, Cargo.lock, the documents, the site's cache keys and the
bug template's dropdown, and the CLI, contract and doc-test suites, clippy,
the `python/` suite and `check_docs` are re-run after it). The three-lane
release battery (`scripts/release_battery.sh`, a frozen snapshot worktree,
the p36–p41 calibration corpus and the sidecar weights in reach): library
**1748 passed / 0 failed / 15 ignored** (1282.41 s, release profile, one process per module), CLI
**26 / 0**, contract 2 + 2, doc-tests 0, GUI **222 / 0 / 1**, calibration
lane **1748 / 0 / 15** (1900.93 s; one skip line, the mask-brush specimen
test whose `AUTOSHADE_MB_SAMPLE_ROOT` specimen is not on this machine, named
in every release since v1.3.2), `audit_i18n` and the font check exit 0 inside
the battery; the Python suites 81 OK from `python/` and 44 OK
from `scripts/` (CPU, the real weights, `-W error::RuntimeWarning`). By name
against the v1.6.1 release battery: library 1763 → 1763 and GUI 223 → 223,
nothing added or removed; CLI 25 → 26, the one addition being
`match_negative_refuses_a_raw_source_and_a_baked_negative`. clippy 0 warnings
on both feature sets (`--all-targets -- -D warnings`). `check_docs.py --gates`
on the transcript with the XMP census root supplied: on the frozen snapshot
**28 PASS / 4 FAIL / 0 SKIP** — the four rows that count the CLI suite still read 25, the
number the bump moves to 26 — and after the bump **32 PASS / 0 FAIL / 0 SKIP**. Photo-name,
token-shape and user-path greps on every commit of the release: 0 / 0 / 0.

Final gate, reference pair, before the tag: the release code's CLI re-fitted
the reference pair at 0.65 / 0.85 / 1.0 and rendered each at the target's
size. At every strength the recipe (store paths normalised), every mask
raster and both renders, with and without the masks, are byte for byte the
ones the v1.6.1 gate was approved on, so its readings stand: sky ΔE /
whole-frame mean |diff| 6.3 / 0.0283, 3.8 / 0.0248 and 3.8 / 0.0249 against
the target. The sky's L\*std reads 1.10 of the target's at 0.85 and 1.0 on
this 1000-px convention, on the same pixels the v1.6.1 gate was approved on;
the 0.95–1.05 line for it dates from the 2048-px convention the gate used
while the full-size target still existed. One binary renders the gate's
recipe byte for byte twice; the CI-built v1.6.1 and a local v1.6.2 build
differ in 8,211 of the full-size render's 180,652,032 samples, by one code at
most — the build environment, not the code. The `--negative` fit was run on
the first draft and again on the release code: the two recipes and their five
rasters are identical.

Not measured: no paid image call was made for this release (the target is the
one every release since v1.3.2 has been measured against); the GUI executable
was not launched; `--negative` was measured on the reference pair's denoised
master only — a stacked master was not measured on a real stack, where the
negative is the first frame because `stack` takes its reference and framing
from it; the desktop app's ◈ and ▦ fits are untouched by this release. The
ship facts (the release run, the downloaded assets, the site, the local
upgrade) are in the ROADMAP ledger entry.
