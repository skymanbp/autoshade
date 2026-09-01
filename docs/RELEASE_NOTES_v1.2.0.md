# AutoShade v1.2.0 — the rename, and a reverse fit that means what it says

Two things happen in this release. The application is now called **AutoShade**,
which touches its name on disk, in your environment and in every URL. And the
reverse fit stops publishing statistics whose premises it had itself denied:
white balance is solved on one population instead of three, the zone boundary
gate charges only for what a correction introduced, and six places that
printed, persisted or documented a claim the code could not support now say
what is true.

Everything that changes a render is described first, and every render change
here is a correction to a statistic — none of them is a taste adjustment.

---

## The app is called AutoShade

The rename is complete in this release and reaches everything a user touches:

| | v1.1.0 | v1.2.0 |
|---|---|---|
| Executables | `autoshop.exe`, `autoshop-gui.exe` | `autoshade.exe`, `autoshade-gui.exe` |
| Installer | `Autoshop-Setup-<v>.exe` | `AutoShade-Setup-<v>.exe` |
| Environment variables | `AUTOSHOP_*` | `AUTOSHADE_*` |
| Settings file | `autoshop.local.json` | `autoshade.local.json` |
| Repository | `github.com/skymanbp/autoshop` | `github.com/skymanbp/autoshade` |
| Site | `skymanbp-autoshop.dev` | `autoshade.dev` |

Nothing you have made is renamed with it without being moved: see **Upgrading**
below for the three directories that migrate themselves on first launch, and
for what still answers to the old spelling and for how long.

## Render changes

### White balance is solved jointly, on one population

The global Atmosphere solve read three independent weighted medians, one per
channel. On a bimodal frame those medians come from different sub-populations:
on the calibration island pair `median(R) = 0.1488` sits in the dark warm land
while `median(B) = 0.2421` sits in the bright blue sky, so their ratio (0.8293)
is not the colour of any pixel in the frame — the linear MEAN ratio for the
same pair is 0.9991, i.e. the target's white balance simply *was* the source's.

v1.2.0 takes a weighted median of the per-pixel log-ratio over ONE population,
read through the correspondence-remapped target the zoned path already used. A
pair whose pixels changed no chromaticity now persists no cast at all: measured
`K = 5500`, `tint = 0.0`, against `tint = +55.2` / `K = 4400` before.

### Exposure and white balance are read over the shared content

`median(target)/median(source)` over two whole frames presumes the two frames
describe the same content — which is exactly what selecting Atmosphere mode
denies. Where a cross-image correspondence field exists, both medians are now
read over the SHARED-CONTENT population: target pixels no confident source cell
maps onto (generated content, not a rendition of this frame) and source pixels
whose content the target replaced are both dropped first.

Both sides, not one. On a synthetic pair whose invented region owns every
whole-frame median (truth 1.2181 at 0.00 EV), whole-frame answers 0.911/+0.694,
target-only 1.945/−2.867, source-only 0.512/+3.593, and the two-sided cut
1.216/+0.032.

The cut is binary at the same `CONFIDENT_MATCH = 0.5` the disclosure already
publishes, from the very bitmap that share is counted from — one derivation,
two consumers, so the sentence and the population cannot disagree. When either
side keeps less of its own evidence mass than `SHARED_POPULATION_MIN_RETENTION`
(0.35, the evidence model's own range-survival floor) the restriction is
refused and the whole-frame reading stands: solving a global control on a
corner of the frame is the same failure in a different costume.

Across the seven-pair corpus: the island pair restricts to 84 % / 76 % and its
render changes; the calibration pair restricts to 58 % / 50 % with identical
dials; one pair retains 11 % / 7 % and refuses; the four Full pairs never
consult a field. Six of seven recipes are field-for-field identical and three
full-resolution renders were verified byte-identical. **A pair with no
correspondence field is byte-identical by construction.**

### A zone correction can now be bought on an absolute gain

The two existing acceptance arms for Full zones are both RATIO yardsticks with
nothing absolute in them. The calibration land zone improved 0.078 → 0.054 with
the frame moving −0.00004 and every quality gate clear, and was dropped only
for landing at 69 % of its start instead of 50 %.

A third arm buys such a correction when the ABSOLUTE zone gain clears
`ZONE_MIN_ABS_GAIN = 0.012` **and the frame-global reading does not regress at
all** — zero regression, stricter than the semantic route's own 0.02 drift
insurance and equal to what the spatial, range and free-mask routes already
demand. The floor is derived twice over: half of the one measured instance
(0.024, n = 1, so it sits a factor of two under rather than on it), and the
ceiling of the observed already-matched domain. It is a separate constant
because those two calibrations only happen to agree today. The arm never
reaches an Atmosphere zone, and an admitted correction names the arm that
bought it — including naming the one quality gate known NOT to discriminate.

### The boundary gate charges only what the correction introduced

- `boundary_rim` measured an absolute quantity on a single render with no
  reference, while its sibling `boundary_step` was already differential and the
  gate's own comment claimed it was "monotone in the differential". It
  therefore billed a zone correction for a rim the global stage had already
  produced — the disclosure could read "still 0.058 against a 0.012 budget with
  k = 0", which by the gate's own construction cannot be caused by the
  correction it had just refused. Four plane-synthetic fixtures hid this,
  because on a flat frame an absolute reading equals a differential one.
- The rim is now transported: `M = linear(rendered) / linear(reference)` is
  applied to the reference luma before differencing, and the percentile ranks
  by MAGNITUDE so a dark seam cannot hide behind a bright one.
- A shrink factor of zero used to attach anyway: `k` reached only the
  disclosure note, so a k = 0 tile still occupied the exclusion quota while
  printing a false before/after. Differencing makes k = 0 read exactly 0.0,
  which turns that from a rare path into the guaranteed one — so an inert
  correction is now refused through the two gates' existing rejection paths.
  The test is byte identity of the render, deliberately not a threshold on `k`.
- `ZONE_BOUNDARY_RIM_MAX` is split from `ZONE_BOUNDARY_STEP_MAX` (both 0.012).
  One constant feeding two rulers would silently retune three spatial tiles
  sitting on 1.7–3.3 % margin the moment either is recalibrated.

## Style and generation

### Distillation carries colour, not only tone

At Style 1.0 the pull reached twelve flat sliders and nothing else, so
`vibrance` and `saturation` were replaced by the library's means while the
mixer, the grade wheels, the curve and the masks carried no target at all —
colour could only be *subtracted*. The fix is not a cap on the pull; it is the
rest of the vocabulary. Five channels, one mechanism (lerp toward a retrieved
mean), and the pull curve itself is untouched:

- the twelve flat globals — unchanged and ungated;
- the 8-band mixer's saturation and luminance, per band;
- the four grade wheels' sat/lum, and their hue as a saturation-weighted
  CIRCULAR mean, learned only for a wheel whose own intensity is a habit;
- the master tone curve's shape, written back through points that pin inputs
  0/64/191/255 so the pull lands exactly and the curve stays monotone;
- each mask's slider amounts — AMPLITUDES ONLY, no coordinate read or written,
  addressed by name so a widened slider list cannot pull the wrong one.

Also honest now: `style_targets` and `blend_toward` both documented a cap that
no caller applies. Style 100 % reaches the target; three doc comments and the
call site say so. And the distillation disclosure, which used to carry one
percentage and no answer to "toward what?", names every field that moved,
measured from the two recipes. Masks are named by position, never by their
`name` field — that is user text and can carry a photo's file name.

### Colour has dimensions in the prompt, not adjectives

Tone had numbers that opened and closed with the intensity axis; colour and
curves had only adjectives, so the model treated "don't touch colour" as the
safe default. Every one of the 17 corpus curves has exactly five points and a
largest departure from the straight line of median 13/255 — a 2 % use of a
control whose engine limit is 256 points, and whose truncation counter has
never once fired. The prompt was the whole cause: it asked for "a 3-5 point
tone curve forming a gentle S", so the model read the top of the range as the
target shape and "gentle" as the amplitude.

- HSL single-band guardrails ±10–20 → ±20–40; the four grade wheels' combined
  budget 20–40 → 40–80. Both ride a ONE-SIDED ramp above the calibration point,
  so at intensity ≤ 0.5 the factory wording is byte-identical and the axis only
  ever loosens.
- Master curve 3–5 → 7–9 points and 8–15 → 15–30 of 255 in amplitude; channel
  curves 4–8 → 8–15 of 255; plus an unconditional monotonicity constraint, so
  the extra points buy a more precise shape rather than ripples.
- The neutral escape hatch is now templated: at the restrained setting it is
  byte-identical to the factory sentence; above it, a neutral answer must
  explain in the rationale why this frame really has no colour to shape.
  Neutral is an answer to defend, not a default to slide into.
- Two things deliberately do NOT ride the axis: the judge's rubric gains a
  positive colour criterion, and the verifier's checklist gains colour
  completeness and curve monotonicity — both written both ways. Judging
  standards should not swing with the user's dial.
- Mask and curve degrees of freedom are now addressable: the habit slider list
  grows to eleven with `hue` appended, and the colour shape reader covers all
  four colour axes (it had been missing `saturation`).

## Disclosures that now match the code

None of these change a render; all of them change what the product says.

- **A zone that solved to neutral no longer claims it already matches.** The
  neutral-solution exit borrowed a sentence asserting "the zone already matches
  the target" and printed the contradicting residual inside its own claim. In
  the flagship pair's live logs that sentence went out 24 times in 40, for
  zones whose residual reached 0.143 against a 0.012 ceiling. It now states its
  own reason. The two exits where the residual really is below the skip
  threshold keep the original sentence, which is true there.
- **A truncated rationale says how much it lost.** Past the 16 KiB ceiling the
  record simply stopped mid-sentence and read as complete; the loss was
  reported only out of band. The cut now spends part of its own budget naming
  the ceiling and the original length, and the ceiling still holds.
- **A residual now names the frame it was measured on.** The AI-proposal path
  deliberately solves and scores over the camera's embedded rendition — the
  base look and lens corrections are already in those pixels, and the
  confidence ladder is calibrated on exactly those numbers — then stamps the
  photo's calibration onto the finished recipe. The delivered render is
  therefore a different frame, differing in chroma by construction because the
  camera curve is matched on luma alone. Both production stampers (CLI `match`
  and the analyze / batch / web path) now say so, in the console line and in
  the persisted rationale, and only when the stamp actually changes the frame.
- **Every Atmosphere report states the population it read.** Where a
  correspondence field exists it also states how much of the target has no
  confident counterpart in the source; with no field that share reads as NOT
  MEASURED, never as zero.
- **`W_LOOK` is documented as unmeasured, not inert.** Its scale was described
  in code and in three shipped documents as unable to reorder the look library.
  It can: the direction text is scored against each look's own image vector and
  its description, so the weight is a real ratio against them. Measured on a
  library where direction and image disagree, the order holds from 0 through
  2× the shipped value and first moves at 4×. The guard test that "pinned" the
  old claim drove both text weights to zero over fixtures carrying neither tags
  nor a description, so it could not have failed; it is renamed to the regime
  it actually covers and a second test states the general case.
- **No toolchain is pinned, and the docs no longer say one is.** TECH_STACK
  described rustc/cargo 1.94 as a "release toolchain pin"; the repo has no
  `rust-toolchain*` file, no `.cargo` config, and all six CI steps use
  `dtolnay/rust-toolchain@stable` with no version. The reproducibility claim in
  ARCHITECTURE is scoped to match: same recipe + same RAW + the same build.

## macOS

- **This is the first release with a macOS desktop app.** `AutoShade.app` is
  universal — Apple silicon and Intel in one binary — and ad-hoc signed, which
  is what lets it run at all on Apple silicon; it is not notarisation and is
  not claimed to be, so the first launch needs one explicit **Open Anyway** per
  machine. The CLI sits beside the GUI in `Contents/MacOS`, because the sidecar
  search stops at the bundle and a command-line binary anywhere else would not
  find `python/`.
- **Two macOS assets ship, not one.** The `.app` archive, and a standalone CLI
  archive carrying the same universal binary with the same sidecars beside it —
  for anyone who wants the command line without downloading a GUI bundle and
  reaching inside it. Both are packed with `ditto`, the archiver macOS itself
  unpacks with: a plain `zip` mangles the symlink layout and the extended
  attributes a bundle needs, and the copy that comes back out is a directory
  that no longer launches. (The binaries' own ad-hoc signatures ride inside the
  Mach-O, not in an attribute — the archiver matters for the tree, not for
  them.)
- **The Apple-GPU path is implemented and unmeasured.** The sidecars share one
  `cuda → mps → cpu` ladder, and the CUDA argument vector is byte-for-byte what
  it was. No latency, memory or `deform_conv2d`-fallback numbers have been
  taken on real Apple hardware, and none are claimed.

## A platform decision no test could reach

- **The macOS test lane is green again.** The Mac port turned the pre-rename
  store adoption off on macOS on purpose: no Mac ever ran the Autoshop
  spelling, `Library/Application Support` is case-insensitive by default, and
  the adoption does not merely read what it finds — it renames it. A folder of
  that name on a Mac is a stranger's. What the port did not do was update the
  three tests describing the adoption's branches, which went on asserting them
  unconditionally, so every push after it failed `test (macos-latest)` on
  exactly those three and nothing else.
- The decision is now a parameter, the shape the GUI's preference adoption
  already used, so all four branches — the refusal included — run on every
  platform. The refusal had until now been executed on none: macOS was the only
  build that took it and macOS was where the suite died. It is pinned by a test
  that a stranger's directory keeps every byte, plus a source-level check that
  both production call sites pass the platform constant rather than `true`.
- No behaviour changes on any platform: production passes the same constant it
  did before.

## Upgrading

- **The installer removes the pre-rename payload.** The rename kept the same
  `AppId`, so setup upgrades a v1.0.x/v1.1.x install in place — and Inno leaves
  behind a file it no longer ships. Upgrading used to leave `autoshop.exe` and
  `autoshop-gui.exe` beside the new binaries, plus the old icon, five
  `*-autoshop.ttf` faces, and a Start Menu group whose shortcuts still launched
  the old version. v1.2.0's installer deletes exactly the names the rename
  changed, moves the Start Menu group, and leaves the develop store and
  downloaded model weights untouched.

### Three directories move themselves on first launch

Each is a whole-directory `rename` on one volume, so every file inside comes
across together or none does; each says so once; and each falls back to the old
name and retries next time rather than starting an empty second copy.

| What | From | To | If it cannot move |
|---|---|---|---|
| Develop store — every recipe, version, snapshot and mask raster | `%LOCALAPPDATA%\autoshop` | `%LOCALAPPDATA%\autoshade` | keeps using the old folder |
| Window geometry, recent library, theme (eframe prefs) | `%APPDATA%\Autoshop` | `%APPDATA%\AutoShade` | keeps the old storage key for the session |
| Export registry — which deliverable filename belongs to which photo | `.autoshop-export-registry` beside the output | `.autoshade-export-registry` | keeps claiming under the old name |

The export registry is the one to understand: it is an on-disk identity
namespace, not a label. Re-keying it instead of moving it would abandon every
claim and start reassigning `-2`, `-3` suffixes from scratch, which is exactly
the collision it exists to prevent. Every claim file travels byte-for-byte
under the same group key.

If a directory of the NEW name already exists beside one of the old, nothing is
moved and nothing is merged: which of two develops of the same photo you meant
to keep is not a decision this app makes silently. It says so and uses the new
one.

On macOS none of this runs. This is the first Mac release, so no Mac ever wrote
the old spellings — and `Library/Application Support` is case-insensitive by
default, so a probe for the old name could match another program's folder,
which these code paths would then rename.

### What still answers to the old name, and for how long

| Compatibility shim | Status |
|---|---|
| `AUTOSHOP_*` environment variables | read, with a one-per-name warning; removed in a later release |
| `autoshop.local.json` settings file | read until the next time settings are saved; removed in a later release |
| Pre-rename rationale mark in an XMP | read; written only in the new spelling |
| `x:xmptk` era marker, both spellings | **permanent.** An existing sidecar does not upgrade itself, and reading an era-1 file as era-2 reinterprets a relative colour temperature as an absolute one — a white-balance shift across the whole library |
| eframe storage key `Autoshop` | read once, to perform the move above |

### Style indexes

An index built by an older version still loads: the habit array is short and
zero-fills, and zero is below the floor that decides a habit exists, so the new
`hue` column claims nothing rather than claiming zero. **Rebuild to gain it**,
not to keep working.

The reverse does not hold. An index written by v1.2.0 cannot be read by an
older build, which reports `invalid length 11`.

## Verification

Merged-tree battery on the release commit: library 1268 passed / 0 failed /
12 ignored, CLI binary 23, GUI binary 159, contract suites 2 + 2, clippy 0
warnings across lib and all targets, `check_docs` 23 PASS / 0 FAIL / 5 SKIP.
Every behaviour change in this release carries a named test and at least one
hand-applied mutation that turns it red, each reverted to a byte-identical
`sha256`.
