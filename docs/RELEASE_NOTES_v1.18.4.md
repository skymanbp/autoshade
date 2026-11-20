# AutoShade v1.18.4 — masks that reach further and answer sooner

Everything merged into `main` since v1.18.3 ships here. A mask's Temp and
Tint reach about twice as far, and every photo saved before looks exactly as
it was saved. On a photo that is slow to redraw the canvas follows a slider
with a quick draft, and a change to one mask no longer runs the masks before
it again. A selected mask's controls fit their column. The reverse fit and
the denoiser are v1.18.3's; so is the AI analysis, but for what a mask Temp
or Tint number it proposes is worth (see Known changes).

The design, the rulings and this document are the maintainer's. The code of
this release was written with an AI coding assistant working under the
maintainer's direction, and every change passed the project's own tests
before it shipped.

## Masks

- **Temp shift and Tint shift reach about twice as far.** The same move of
  the slider warms, cools or tints the masked area about twice as much as it
  did. Before, the whole length of the slider was a small change.
- **A photo saved before looks exactly as it was saved.** When it is opened,
  its mask Temp and Tint are restated on the stronger scale, so the sliders
  read smaller — Temp a little over half the old number, Tint half. A
  restated number is shown as it is (48 reads 25.6) and stays so until you
  move the slider, which then lands on whole numbers as before. Opening such
  a photo, or selecting one of its masks, changes nothing and does not mark
  the photo as edited.
- **Keep skin natural allows the warmth it always allowed.** Its limit reads
  10.67 on today's slider — the same warmth that 20 was. With the switch off
  the slider runs to 100, about twice as far as it did.

## Preview

- **A slow photo follows the slider in drafts.** On a photo that is slow to
  redraw — a large canvas, many masks, a colour field — the canvas follows a
  slider with a quick, slightly soft draft while you drag, and replaces it
  with the full-quality picture a moment after you stop. The draft is only
  what you look at while your hand moves: a card's thumbnail, the saved
  develop and every export are made from the full picture. A photo that
  redraws quickly is never drawn in drafts.
- **A change to a mask redraws from that mask on.** The masks before it are
  not run again, so a mask late in the list answers much sooner — in the
  draft while you drag and in the full picture when you let go. The picture
  is the same picture, byte for byte, as one drawn from the start.

## The .xmp

- **An `.xmp` AutoShade wrote imports to the picture it was exported with** —
  v1.3.1 or later, every one that carries the whole develop, whichever
  version wrote it. The mask Temp and Tint an older version wrote are
  restated as a saved photo's are.
- **A mask Temp or Tint set in Lightroom is read at the number it states.**
  Where Lightroom changed one number of a mask and left another alone, the
  one it changed stands and the one it left keeps the picture it had.
- **One exception: a mask the reverse fit recoloured, in an `.xmp` from
  v1.18.3 or earlier.** Such a file does not always say which of that mask's
  numbers were set in Lightroom. Where it does not, the mask imports as
  v1.18.3 imported it: its Temp and Tint are restated like the older
  version's, one set in Lightroom included.

## Library

- **● edited stands to the right of a photo's name**, with its dot, and no
  longer under it.

## Fixed

- **A selected mask no longer pushes the canvas away.** With some masks
  selected, a row wider than the panel opened a black block between the
  controls and the canvas, and the controls ran into it. Every kind of
  mask's rows now fit their column, and **Redraw** has a row of its own.
- **An `.xmp` written by v1.16 or earlier keeps its masks' recolour.** Since
  v1.17.0, importing one dropped the colour a reverse fit had given a mask
  and kept that mask's Temp, Tint and Exposure alone.

## Known limits

- **A whole-photo control and the first mask redraw everything.** While you
  drag you see the draft; the full picture that follows takes as long as it
  did.
- **A target repainted over its whole frame gets no colour field.** The
  colour field needs some part of the two pictures to be the same place. A
  target that shows the same scene but was re-synthesised everywhere is
  matched by the whole-frame and zone controls alone.
- **An exported `.xmp` declares its starting tone, it does not carry it**:
  opened in Lightroom, a photo whose recipe starts on the camera base curve
  renders on Lightroom's own starting tone.

## Known changes

- **The same mask Temp or Tint number means about twice as much.** That
  holds for a number an AI analysis proposes, a number typed into the browser
  UI, and a number set in Lightroom and read from an `.xmp`. Photos you saved
  are not affected: their numbers are restated.
- **An `.xmp` exported for a photo saved before carries the smaller
  numbers.**
- **The app holds more memory while a photo with masks is open** — a few
  hundred megabytes on a large canvas, for the frames a mask resumes from —
  and gives it back when you open another photo.

## Compatibility

Develops saved by earlier versions open as they did, with their mask Temp and
Tint restated; so does a recipe file named on the command line or sent to the
browser UI. A develop this version saves carries one new recipe field, which
says that its mask Temp and Tint are on the stronger scale. **v1.18.3 and
earlier refuse a develop that carries it**: to go back to an earlier version,
keep a copy of the develop store from before the upgrade. No command and no
option is added or removed.

## Gates

This release changes the library's renderer, recipe, store, pipeline,
web-server, `.xmp`, AI-reply and face modules, the command line and the
desktop window. No local three-lane battery was run; the whole suite, long
tests included, ran on the project's online runners on `dc396a1`, the commit
that carries this release's code (run 44763767884, 28 jobs, each platform's
library tests cut in eight shards). On Linux: library **2025 passed / 0
failed / 22 ignored**, CLI **30 / 0**, contract 2 + 2, doc-tests 0, GUI
**363 / 0 / 11**. On macOS: library 2023 / 0 / 22, CLI 30 / 0, GUI 363 / 0 /
11. The debug-assertion library trip: 2025 / 0 / 22. On Windows, on the
maintainer's machine and by name: the library's renderer, recipe, store,
pipeline, `.xmp`, AI-reply, web-server and face modules 1120 / 0 / 9, with
the folder of a real Lightroom export pack and the folder of sidecars a real
Lightroom rewrote both named, so the tests that read them ran; the four
library names that exist on Windows only are among them; the command line 30
/ 0; every test of the desktop window 363 / 0 / 11; the two contract
programs 2 + 2. By name against v1.18.3, the names as the Windows test
programs list them: library 2013 → 2050 (+37 / −0), CLI 29 → 30 (+1), GUI
356 → 374 (+18 / −0). The rules this release adds on the frames a mask
resumes from, on the mask scale and in the sidecar reader were broken by
hand, the test that guards each turned red, and each such change was put
back byte for byte. `audit_i18n` 0, the font check (924 of 924 code points
embedded) and `check_doc_drift` (0 findings); clippy 0 warnings on both
feature sets, test code included; the Python suite under `scripts/` 57
tests, all passing (`python/` is unchanged since v1.18.0 and its suite was
not run); `check_docs.py` 27 / 0 / 5 PASS / FAIL / SKIP — the skipped rows
are the test-count claims, which it checks only against a local battery's
transcript, and the census of the maintainer's own sidecars. A selected
mask's rows were read on the offline camera's pictures in both languages.

Saved photos, drawn again. The develops v1.18.3 saved for three
reverse-fitted photos, and for one photo with four masks that carry Temp and
Tint, were drawn by this build and by v1.18.3's code built with the same
compiler: the same bytes, each. The `.xmp` the released v1.18.3 exported for
the four-mask photo imports here to the picture that program drew, every
pixel. The sidecar reader was run over the 565 `.xmp` files that eight
versions from v1.3.0 to v1.18.2 wrote for the maintainer's photos: every
recoloured mask of a file v1.16 or earlier wrote keeps its recolour (188 of
188), so does every one of a file v1.17.1 or later wrote (1,005 of 1,005),
and 746 of v1.17.0's 954 — the others lose it as they did in v1.18.3.

The reverse fit, checked on the reference pair. This build fitted the pair
again at 0.65, 0.85 and 1.0. Against the same three fits kept from v1.18.0,
every mask and the render are the same bytes, and the recipe differs in one
value, the field this release adds; the three renders were read beside the
target. The tests that need the calibration photos were run for what this
release touches, the renderer's and those that read the photos' saved
recipe: 402 / 0 / 5.

The commits after `dc396a1` (the documents and the version number) change
nothing a long test can read, so the online runs on them set the long tests
aside; the run above is where the long tests ran for this release.

Not run, and why: the other tests gated on the calibration photos (the
reverse fit's modules are v1.18.3's but for three statements that read a
saved recipe, and the denoiser's are byte for byte v1.18.3's).
