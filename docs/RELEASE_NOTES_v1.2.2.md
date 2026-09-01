# AutoShade v1.2.2 — the seam a scalar budget could not see

v1.2.1 changed nothing about the renderer or the reverse fit. Re-rendering the
reverse-fit showcase on that build surfaced a defect the freeze would otherwise
have hidden, and the freeze was lifted for it: the zone boundary gate budgeted
an absolute luma step for every mask edge, so a tile seam that is invisible in
foliage was plainly visible in smooth sky. This release fixes that, fixes a
second defect found while buying the new showcase targets (the generative
request was sized from the wrong frame), and ships the two library features
that were queued behind the freeze.

## Defect 1 — a tile seam in smooth sky

`ZONE_BOUNDARY_STEP_MAX = 0.012` was documented as "a visibility threshold in
luma". Visibility is contextual — the eye masks a step against the contrast
around it — but the budget was a scalar, the same in a cloud bank and in a
clear gradient. Measured on the stone-viaduct pair, at the right edge of the
top-left spatial tile in the sky band, on the mask-free ruler
(`scripts/rim_overshoot.py`, which reads the render itself, never a mask):

| Frame | Step across the tile edge | In units of the local sky's own variation |
|---|---:|---:|
| Neutral conversion (no masks) | +0.15 codes | 1.3 σ |
| v1.2.1 fit, eight masks attached | +1.59 codes | 7.8 σ |

The photograph contributes nothing at that edge; the attachment introduced
+1.44 codes, and the gate passed it because 0.0056 is under 0.012.

### Root cause, in two findings of one class

1. **The budget must be earned from the boundary's own neighbourhood.** Each
   crossing now earns `budget = max(context, 3 × slope)`, clamped to
   `[1/255, 0.012]`, where *context* is the reference render's own step across
   the contour and *slope* is the ramp the correction itself carries just
   inside and outside the edge. The introduced step is charged
   `|step| × (0.012 / budget)` and the gate ranks the charged p90. A crossing
   whose budget reaches the ceiling is charged its raw step on an explicit
   branch, so textured boundaries are bit-for-bit what v1.2.1 shipped.
2. **The slope must persist past the collar.** The guided-refine stage wears a
   soft shoulder about three analysis pixels wide — exactly the first baseline
   out from the edge — so a hard tile edge in clean sky read a spurious inner
   slope of 0.005–0.008, and three times that bought the whole ceiling back for
   a 43-crossing band. The slope is now the minimum over two consecutive
   same-side baselines; a genuine ramp shows the same slope on both.

The slope is read from the frozen k=1 candidate, never from the render under
bisection, so the budget cannot chase the dial it is meant to bound. The soft
rim ruler and the luminance-range family decline to charge (`charged == rim`).

### What it does to the viaduct

| | v1.2.1 | v1.2.2 |
|---|---:|---:|
| Tile r0c0 cross-boundary step | 0.0278 | 0.0042 |
| Its accepted dial `k` | 0.460 | 0.121 |
| Delivered sky seam, mask-free ruler | +3.15 codes | +0.92 codes |

The textured tile on the same frame and the calibration island's
ceiling-parked tiles are byte-identical between the two builds — the exchange
only changes what the neighbourhood cannot hide.

### Why nothing caught it

The boundary tests drove flat grey fixtures where the only structure at the
edge was the correction itself, so the scalar budget and a contextual one give
the same answer. The new tests carry a flat arm (the budget floors), a textured
arm pinned to the shipped bits, a three-times-dial arm, and a shouldered arm in
two lengths — the collar shape that must not fund a seam, and a persistent ramp
that must earn exactly three times its slope. Nine hand-run mutations
(adaptivity removed, a flat re-tune at 0.004, floor 0, context from the render,
slope term deleted, slope saturated, p90 → max, slope from the live render,
inner baseline only) each turn the battery red.

## Defect 2 — `reimagine` sized its request from the wrong frame

`reimagine` planned the generated image's size from the embedded preview's
dimensions and then, once the flexible size outresolved that preview,
developed the full sensor and resized it *exactly* to the planned size. A
camera set to a 4:3 aspect writes a 4:3 preview over its 3:2 sensor, so the
3:2 develop went out squashed by 11 % and the target came back 4:3 against
the 3:2 develop that `match` pairs it with. Every earlier showcase body had a
3:2 preview, which is why the defect had never fired.

The size is now planned from the frame that is actually sent — for a RAW,
`DefaultCropSize` in display orientation, the frame the develop renders and
the fit measures — and a preview that is not the sensor frame is never the
input, however large it is. On the Cornwall showcase frame the same prompt and
RAW measured **D = 0.304** against the squashed input and **D = 0.139**
against the correctly framed one.

## Defect 2, continued — three consumers took the preview for the frame

The `reimagine` fix above was one instance of a class. A body set to an
in-camera aspect writes a *centred crop* of its sensor as the embedded
preview (the Cornwall frame: 1440 × 1080 over 9504 × 6336; the crop measures
centred at NCC 0.987 against 0.83 for either side), and two more production
consumers paired that preview with the sensor frame:

- **CLI `match`** fitted on the preview against a target of the full frame.
  The reference check warned "CROPPED … treat the result as unreliable" on a
  same-frame pair, and every zone and tile mask solved on the 4:3 crop
  landed on the 3:2 render. It now detects the crop and takes the route the
  desktop app has taken since v0.25.0: a neutral develop of the sensor frame
  with the calibration composed into the solve. On the Cornwall frame the
  look error went from 0.141 → 0.077 measured on the wrong frame to
  **0.137 → 0.027** on the delivered one. Bodies whose preview *is* the frame
  keep the embedded-rendition source and the post-fit calibration stamp bit
  for bit.
- **The base-look estimator** CDF-matched the whole develop against the
  cropped rendition, so the edge strips' mass sat on one side only. It now
  pairs the develop's centred crop.

One rule answers "is this the sensor frame?" for every consumer —
`fit::same_frame_plausible_dims`, the 2 % aspect tolerance the reference
check already used — so a frame cannot be "the same" to one consumer and
"cropped" to another. Display consumers (thumbnails, the web grid, `decode`)
show the preview and are untouched.

## Defect 3 — the style index refused the vocabulary it wrote

v1.2.0's symmetric distillation taught `style-index` to record the 24 HSL
cells and the 14 colour-grade fields beside the twelve reference sliders.
The loader kept its own twelve-label list. Any library with one HSL or
colour-grade edit in it therefore produced an index that the same binary
refused to read back — `style index … exemplar 0 has an unsupported setting
key` — and `style-index --looks`, which merges into the existing index,
replaced it with a looks-only file. The Style control then read no edits and
said nothing. Found on the showcase library while re-rendering Part A (169
pairs indexed, 0 loadable); the shipped index on the author's machine, built
before v1.2.0, has none of the new labels and was never affected.

One table, `setting_bands`, now owns every label the writer produces and the
band the loader clamps it to (the recipe's own: ±100 on every HSL cell; hue
0..360, saturation and blending 0..100, luminance and balance ±100 on the
colour-grade wheels). Two tests pin it: every label `read_settings` can write
has a band, and an exemplar carrying all fifty survives `save` → `load`.
Dropping either loop from the table turns both red. Rebuild your index once
on v1.2.2 if you built it on v1.2.0 or v1.2.1 and your library carries HSL or
colour-grade edits — the build itself was always correct; only the read was
refused.

## What else is in v1.2.2

- **`--xmp-dir <DIR>`** on `style-index` and `eval`, and a *Sidecar folder*
  row in the desktop app's style library. Sidecars kept in a separate tree —
  a read-only photo volume, a catalogue exported elsewhere — could not enter
  the index at all ("0 RAW+.xmp pairs", and the build refused to save). One
  pairing rule, `xmp_pair`, now owns the lookup: a mirror of the library tree
  under the sidecar folder, then a flat folder, then the sibling beside the
  RAW. The extension matches in any case on every platform by listing the
  folder once — `with_extension("xmp")` wrote a lowercase name that
  `Path::exists` folds on Windows and not on macOS, so the Mac build paired
  fewer files than Windows from the same library and said nothing. The stem
  is matched exactly, because every sidecar writer reproduces the
  photograph's name byte for byte and folding it would let one RAW claim
  another's edit on a case-sensitive volume. The build now names the RAWs it
  could not pair. The develop chain's beside-the-RAW sites are deliberately
  unchanged: they are also a write target, and reading them case-insensitively
  without moving the write would split one sidecar into two files.
- **Incremental `style-index` builds.** Every build redid the decode, the
  14-dim feature, the SigLIP image vector, the vocabulary scores and the text
  vector for every photograph; only the Qwen description was cached. A
  per-exemplar cache (`style-exemplars.json`, keyed by the staged frame's
  SHA-256 like the description cache, plus a source stamp of path, size,
  mtime and saved rotation) now serves an unchanged photograph without
  opening the RAW and a renamed or touched one without any model call. Every
  entry carries the index's feature version and this build's embedding
  provenance, and is checked at the door against the same bands the index
  itself enforces, so a stale or bit-rotted entry is recomputed rather than
  served. The normalisation (mean and σ over the merged set) is never cached.
  The build reports `reused N, recomputed M, removed K`. On the showcase
  library the second build of 169 pairs reused every one and produced a
  byte-identical index.
- **The showcase is re-rendered on this build** — one four-looks panel for
  the AI-analysis pillar and two 3 × 2 reverse-fit panels for the
  reimagine → fit pillar, with every number in [docs/SHOWCASE.md](SHOWCASE.md).
  The Cornwall panel is shown as fitted: its global stage admitted
  per-channel cast curves that pass the re-hue gate yet tint the delivered
  sky toward violet against the target's blue. That is the fit's current
  tolerance, disclosed rather than hand-corrected, and registered in the
  roadmap for v1.2.3 (a hue-preserving cast stage, and a rationale note on
  ordinary cast admission, which today is silent).

## Upgrading

Install over v1.2.1; the installer upgrades in place. No schema, sidecar or
store format changes. A reverse fit that attaches spatial tiles or field masks
in smooth regions may now accept a smaller dial than v1.2.1 did, and its
rationale discloses both the raw and the context-charged reading with the
ceiling. Everything else renders byte-identically.

## Verification

Release battery on the tagged tree, in parallel lanes with their own target
and data directories:

| Lane | Result |
|---|---|
| Library (`cargo test --release --lib`) | 1296 passed, 0 failed, 12 `#[ignore]`d forensic probes (1308 enumerated) |
| CLI binary | 23 passed |
| Contract tests (`tests/repro_*`) | 2 + 2 passed |
| Desktop GUI (`--features gui --bin autoshade-gui`) | 160 passed |
| `cargo clippy --all-targets`, default and `gui` | 0 warnings on both |
| `scripts/check_docs.py --gates <transcript>` | 27 pass, 0 fail, 1 skip (the XMP census corpus lives outside the repo) |
| `scripts/audit_i18n.py` / `scripts/subset_gui_fonts.py --check` | 0 findings / 871 of 871 glyphs embedded |

Set difference of library test names against v1.2.1's tree (`d628c80`):
**+29 / −1** — 22 from the style-index batch (`xmp_pair`, `content_cache`,
`style_cache`, six in `style`), the two contextual-budget boundary arms, the
size-plan and base-look frame tests, the two band-table tests, and the
same-content diagnosis test re-pointed from the viaduct panel to the Cornwall
one (the −1 is that rename). The GUI battery is +1 (the sidecar-folder
preference). Hand mutations run and reverted: nine on the boundary budget,
five on the style-index batch, one on the frame-class crop, one on the band
table — every one turned its named test red.

Live checks on the showcase library: two consecutive `style-index` builds of
169 RAW+XMP pairs on this binary — the second reported `reused 169,
recomputed 0` and produced a byte-identical index; the look-library build
then merged 94 finished photos into it instead of replacing it (the v1.2.1
binary's behaviour on the same files was "0 loadable, replaced").

All four release assets were downloaded from the GitHub release and verified
against `checksums.txt` and an independently computed SHA-256 before the
README and site tables were filled; the local install was upgraded from the
downloaded installer and its executables compared byte for byte with the
portable archive.
