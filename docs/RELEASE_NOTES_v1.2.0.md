# AutoShade v1.2.0 — a reverse fit that means what it says

v1.2.0 is a correctness-and-honesty release. The reverse fit's global white
balance stops reading three independent per-channel medians — a statistic that,
on any frame with two populations, mixes numbers taken from different halves of
the picture. The zone boundary gate stops charging a correction for a rim the
global stage had already put there. And four places that printed or persisted a
claim the code could not support now say what is actually true.

There is one deliberate render change, described first. Everything else is
either byte-identical at default settings or moves only disclosure text.

## Render change

- **Atmosphere white balance is solved jointly, on one population.** The global
  Atmosphere solve read three independent weighted medians, one per channel.
  On a bimodal frame those medians come from different sub-populations: on the
  calibration island pair `median(R) = 0.1488` sits in the dark warm land while
  `median(B) = 0.2421` sits in the bright blue sky, so their ratio (0.8293) is
  not the colour of any pixel in the frame — the linear MEAN ratio for the same
  pair is 0.9991, i.e. the target's white balance simply *was* the source's.
  v1.2.0 takes a weighted median of the per-pixel log-ratio over ONE population,
  read through the correspondence-remapped target the zoned path already used.
  A pair whose pixels changed no chromaticity now persists no cast at all:
  measured `K = 5500`, `tint = 0.0`, against `tint = +55.2` / `K = 4400` before.

## The boundary gate charges only what the correction introduced

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

## Disclosures that now match the code

None of these change a render; all of them change what the product says about
one.

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

## Upgrading

- **The installer removes the pre-rename payload.** The rename to AutoShade
  kept the same `AppId`, so setup upgrades a v1.0.x/v1.1.x install in place —
  and Inno leaves behind a file it no longer ships. Upgrading used to leave
  `autoshop.exe` and `autoshop-gui.exe` beside the new binaries, plus the old
  icon, five `*-autoshop.ttf` faces, and a Start Menu group whose shortcuts
  still launched the old version. v1.2.0's installer deletes exactly the names
  the rename changed, moves the Start Menu group, and leaves the develop store
  and downloaded model weights untouched.
- Everything the v1.1.0 notes said about `AUTOSHOP_*` environment names,
  `autoshop.local.json` and the pre-rename data directory still applies: the
  old spellings are read with a warning and will be removed in a later release.
- Style indexes built before v1.1.0's mask-habit widening still need one
  rebuild (`invalid length 10`).

## Verification

Merged-tree battery on the release commit: library 1267 passed / 0 failed /
12 ignored, CLI binary 23, GUI binary 159, contract suites 2 + 2, clippy 0
warnings across lib and all targets, `check_docs` 23 PASS / 0 FAIL / 5 SKIP.
Every behaviour change in this release carries a named test and at least one
hand-applied mutation that turns it red, each reverted to a byte-identical
`sha256`.
