# AutoShade v1.18.1 — the same fit, sooner

Everything merged into `main` since v1.18.0 ships here. This is a maintenance
release of the reverse fit. v1.18.0 made the fit one solver that renders its
candidate many thousands of times, and it took those renders one after
another. This release takes the ones that do not depend on each other side by
side and uses them in the order it always did: a fit ends on the recipe
v1.18.0 ends on, in less time. Nothing about what the fit looks for, how far
it goes or what it writes has changed.

The design, the rulings and this document are the maintainer's. The code of
this release was written with an AI coding assistant working under the
maintainer's direction, and every change passed the project's own tests
before it shipped.

## The reverse fit

- **A fit takes less time and ends on the same recipe.** The solver reads a
  candidate by rendering it, measuring its distance to the target and asking
  the fit's one rule whether it may ship. Most of those readings depend only
  on the candidate itself. They are now taken on every processor the fit is
  given — the separate starting points beside each other, the trial steps of
  a search before the search asks for them, the test of each control's
  absence beside the others — and the solver then walks them in the order it
  walked them before, so every decision is the one v1.18.0 makes. On the
  maintainer's machine a real pair that v1.18.0 fitted in 42 minutes takes
  23, each fit run alone on half of the machine's processors with other
  programs at work beside it.
- **The result is checked to be the same, not assumed to be.** New tests fit
  the same picture both ways and require the same recipe, the same controls
  set back to zero and the same final reading, at several widths of reading
  ahead. And real pairs were fitted by v1.18.0 and by this build: the
  recipes, the mask rasters, the renders and the sidecars are the same byte
  for byte (the sidecar's line naming the program version aside).
- **A failure inside a fit is still that fit's failure.** Work the fit hands
  to another processor runs under the same protection as the fit itself, so
  an internal error ends the fit with its message and leaves the application
  running, as before.

## Fixed

- **A brightness range widened by a merge is read again.** The local stage
  that corrects by brightness range merges two ranges when one overlaps
  another or when more than four would ship. The merged range kept the
  reading of the range it was before the merge, and that reading decides
  which ranges survive the limit of four and which way later merges go. It
  is now read again on what the merged range covers. No fit of the
  calibration pairs merges a range, so none of them changes.

## Known limits

- **A target repainted over its whole frame gets no colour field.** The
  colour field needs some part of the two pictures to be the same place. A
  target that shows the same scene but was re-synthesised everywhere is
  matched by the whole-frame and zone controls alone.
- **The sidecar's starting tone is declared, not carried**: opened in
  Lightroom, a photo whose recipe starts on the camera base curve renders on
  Lightroom's own starting tone.

## Known changes

- **A fit keeps more processors busy while it runs.** v1.18.0 used two to
  three; this release uses what the machine gives it. To have the fit read
  one thing after another as v1.18.0 did, set `AUTOSHADE_FIT_SERIAL` in the
  environment; the result is the same either way.
- **A fit is still much longer than v1.17.1's.** The number of renders a
  result needs is v1.18.0's; this release changes when they are taken.

## Compatibility

No new option and no new recipe or sidecar field. Recipes and sidecars
written by earlier versions open and render as they did in v1.18.0, and for
the same pair of pictures and the same settings this build writes the recipe
v1.18.0 writes.

## Gates

No local three-lane battery was run for this release; the suite ran on the
project's online runners on `d115dbf`, the last commit of this release that
changes code (those after it change documents and the version number), each
platform's library tests cut in eight shards. On Linux: library **1979 passed
/ 0 failed / 21 ignored**, CLI **28 / 0**, contract 2 + 2, doc-tests 0, GUI
**297 / 0 / 4**. On macOS: library 1977 / 0 / 21, CLI 28 / 0, GUI 297 / 0 / 4.
The debug-assertion library trip: 1979 / 0 / 21. The library names that exist
on Windows only were run on Windows by name (4 / 0). `audit_i18n` 0, the font
check (924 of 924 code points embedded) and `check_doc_drift` (0 findings);
clippy 0 warnings on both feature sets; the Python suite under `scripts/` 44
tests, all passing (`python/` is unchanged since v1.18.0 and its suite was not
run); `check_docs.py` 28 / 0 / 4 PASS / FAIL / SKIP — the skipped rows are the
test-count claims, which it checks only against a local battery's transcript.
By name against v1.18.0, the names as the Windows test program lists them:
library 1997 → 2003 (+6 / −0); GUI and CLI unchanged. Each change was broken
by hand once and the test that guards it turned red (10 such changes, each put
back byte for byte). The order of the reverse fit's work changed and its
result must not, so four real pairs were fitted again, between them every
shape the whole-frame solve takes. The lantern street (p38) and the night pair
from its denoised master (p42), each by a build of the v1.18.0 code and by
this build: the recipe, the mask rasters, the render and the sidecars are the
same byte for byte. The dusk farmhouse (p41) and the sunset rooftops (p36) by
this build against the fits of v1.18.0's last check: the recipe, the mask
rasters and the render byte for byte, the sidecar differing in the one line
that names the program's build. The tests gated on the calibration photos pass
on the online runners without the photos and were not run again with them:
they read fits, and the fits are v1.18.0's.
