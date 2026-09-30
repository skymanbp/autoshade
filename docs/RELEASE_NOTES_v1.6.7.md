# AutoShade v1.6.7 — an AI denoise that no longer stalls the machine, and a reverse-fit that says where it starts

Everything merged into `main` since v1.6.6 ships here: two fixes the
maintainer reported on 2026-09-30 while using v1.6.6. An AI denoise of a large
baked master made the whole computer unusable for minutes, and the reverse-fit
never said which negative it solved from. The develop engine, the reverse fit
and every render are v1.6.6's; the denoise sidecar's output is v1.6.6's byte
for byte.

The diagnosis, the measurements, the fixes and this document are the
maintainer's; no external coding model wrote this release.

## AI denoise on baked sources

- **What happened.** The report: a few minutes into a session the computer
  became so slow it could not be used. The session had run an AI denoise on a
  9504 × 6336 16-bit master (a 313 MB PNG), which goes through the SCUNet
  sidecar. That sidecar read the frame, converted it to floating point and
  kept the input, the tile accumulator, the weight plane, the quotient and the
  luminance / chroma blend's planes alive at the same time. Measured on 16-bit
  inputs of 3, 12 and 27 MP: **83–86 bytes per pixel** on top of about 3.3 GB
  for the model — at 60 MP about 8 GB of commit and 6 GB resident. The machine
  had 3.6 GB of physical memory free, so Windows paged the whole desktop. The
  desktop app's memory budget never counted the sidecar: it is a separate
  process.
- **What changed.** The sidecar now works one row of tiles at a time. A band
  of rows is finished, blended, converted back to the file's integers and
  written into the output as soon as no later tile can touch it; the only
  whole-frame arrays left are the file's own integers, in and out. The tiles
  run in the same order and every pixel's weighted sum is accumulated in the
  same order, so **the output is the old output byte for byte** — checked on
  four inputs (16-bit, 8-bit with alpha, a frame smaller than one tile, odd
  sizes) and pinned by a new test with a neighbour-mixing stand-in model.
  Measured on the same three inputs: **19 bytes per pixel**; on a real 60 MP
  16-bit master the peak is 4317 MB of commit and 2233 MB resident (was about
  8 GB of commit and 6 GB resident by the slope above).
- **The Rust side** released the decoded frame only after the sidecar had
  finished; it now releases it before the sidecar starts.
- **Not changed.** The RAW-domain denoiser works on quarter-resolution sensor
  planes and measured 22 bytes per pixel; it is untouched. The model's own
  ~3.3 GB of commit is the same as before.

## Reverse-fit: where it starts, where it goes

- **What happened.** The question: after an AI denoise, with the AI-generated
  card selected, does the reverse-fit start from the denoised negative or the
  original? It started from a rule nothing on screen showed — the ◈ / ▦ card
  you stand on, else the first one in the strip, else the ▣ card. Standing on
  a ✨ card with a ◈ card in the strip, it always started from the first
  denoised master.
- **What changed.** The Reverse-fit fold has a **From** menu: **Automatic**
  (the rule above, with the card it picks named), the ▣ Original, and every ◈
  Denoised / ▦ Stacked card, each by its strip label and position. A **To**
  line names the ✨ card when no reference file is picked. The solve's source
  frame, the ◭ card's pixels and the link saved in `pixels.json` all follow
  the pick. A pick is forgotten when another photo opens, and a pick whose
  card is deleted falls back to Automatic. On the command line the same choice
  was already `match --negative`.

## Documents and the site

- **The manual** describes the two ends of the reverse-fit and Automatic.
  README, ARCHITECTURE, TECH_STACK and the site carry the new battery count
  (226 GUI tests) and their version words; the site changes nothing else but
  its cache keys.

## Compatibility

No recipe, sidecar, store or weight format changes. The sidecar weights stay
pinned to the v1.6.0 `autoshade-raw-denoise-v2.pth`; no weights are
re-shipped. No pixel of any render or denoise changes. The reverse-fit's
default is the rule it always used.

## Gates

Measured before the tag on the release code (`5039d38`: the sidecar change
`27befb9`, the reverse-fit source `ca29fbf`, the battery counts `5039d38`).
The version bump touches Cargo.toml, Cargo.lock, the documents, the site's
cache keys and the bug template's dropdown, and the CLI, contract and doc-test
suites, the denoise module, clippy, the `python/` suite and `check_docs` are
re-run after it. The three-lane release battery (`scripts/release_battery.sh`,
a frozen snapshot worktree, the p36–p41 calibration corpus and the sidecar
weights in reach): library **1753 passed / 0 failed / 15 ignored** (904.91 s,
release profile, one process per module), CLI **27 / 0**, contract 2 + 2,
doc-tests 0, GUI **225 / 0 / 1**, calibration lane **1753 / 0 / 15** (1317.16
s; no skip line), `audit_i18n` and the font check exit 0 inside the battery;
the Python suites 82 OK from `python/` (81 + the new streaming test) and 44 OK
from `scripts/` (CPU, the real weights, `-W error::RuntimeWarning`). By name
against the v1.6.6 release battery: library 1768 → 1768, nothing added or
removed; CLI 27 → 27; GUI 225 → 226, one addition,
`the_reverse_fit_solves_from_the_picked_source_card`, nothing removed. clippy
0 warnings on both feature sets (`--all-targets -- -D warnings`).
`check_docs.py --gates` on the transcript with the XMP census root supplied:
on the frozen snapshot **32 PASS / 0 FAIL / 0 SKIP** — the counts moved with
the code before the snapshot — and after the bump **32 PASS / 0 FAIL / 0
SKIP**; after the bump the CLI, contract and doc-test suites read 27 / 0, 2 +
2 and 0, the denoise module 47 / 0, clippy 0 warnings on both feature sets,
the `python/` suite 82 OK, and `audit_i18n` and the font check exit 0.

Memory, measured with the process's own peak commit and peak working set
(`PeakPagefileUsage` / `PeakWorkingSetSize`), 16-bit inputs, strength 0.5, the
real SCUNet weights on the GPU: at 3 / 12 / 27 MP v1.6.6's sidecar peaked at
3374 / 4118 / 5281 MB commit and 1371 / 2113 / 3294 MB working set, v1.6.7's
at 3291 / 3470 / 3735 MB and 1259 / 1457 / 1680 MB; on a real 9504 × 6336
16-bit master v1.6.7's peaked at **4317 MB commit and 2233 MB working set**.
The output of both versions was array-equal on the four inputs named above.

Final gate, reference pair, before the tag: the release code's CLI re-fitted
the reference pair at 0.65 / 0.85 / 1.0 and rendered each at the target's
size. Every recipe, mask raster, written sidecar and 1000 px render (with and
without masks) is **byte-identical to the v1.6.6 gate's** — 23 comparisons,
none differing; the readings are therefore v1.6.6's (sky ΔE / whole-frame mean
|diff| against the target: 0.65: 6.3 / 0.0284, 0.85: 3.9 / 0.0250, 1.0: 3.9 /
0.0252). The maintainer's own look at the three-way sheet at 0.85 (the target,
the v1.6.1 approved gate, this build): the sky is one smooth gradient with no
block, seam or tile, and the ground reads as the approved gate's.

Three mutations falsified the new invariants on the snapshot, each red and
each restored (SHA-256 of both files matched, worktree clean): `start_fit`
reading `negative_origin` again turned the source-text pin red; a
`fit_source_index` that accepts any card id turned
`the_reverse_fit_solves_from_the_picked_source_card` red; a band emitted at
the next tile row's bottom turned `StreamedBandsTest` red.
