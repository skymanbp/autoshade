# AutoShade v1.18.2 — a tidier window

Everything merged into `main` since v1.18.1 ships here. This release changes
the desktop window and nothing else: Settings is laid out in pages, the
language can be changed on the opening page, the toolbar and both side
columns stand on one grid, and AutoShade speaks about Lightroom only when you
ask it for a Lightroom file. The develop engine, the reverse fit, the AI
analysis and the command line are v1.18.1's.

The design, the rulings and this document are the maintainer's. The code of
this release was written with an AI coding assistant working under the
maintainer's direction, and every change passed the project's own tests
before it shipped.

## The window

- **Settings has five pages.** The ⚙ button opens a window with its pages
  named on the left: **General** (language, theme, accent colour — these
  apply at once), **Folders** (the delivery folder and the develop store),
  **AI models** (the analysis model and the image model), **Advanced** (the
  Python interpreter and the segmentation helper) and **About**. Every
  setting of the old single column is on one of them. **Save settings**
  stands under the three pages whose fields it saves, and saves them
  together.
- **The language can be changed on the opening page.** **English · 中文** at
  the top right of the mode page switches the app at once. Both names are
  always shown in their own script, so the switch can be found from either
  language.
- **One toolbar in two groups.** On the left: the mode's name, Open photo…
  and Reset, Undo and Redo, and the two turns. On the right, against the
  window buttons: the Compare / Single switch, **Save develop**, **Export**
  with its ▾, and the ⚙ and ⌨ buttons. On a narrow window a group moves to
  a second line whole.
- **The controls column stays put while you resize it.** Dragging its edge
  changes how wide the rows are and never where they stand: no row moves
  onto another line, nothing changes height and nothing below it jumps.
  Dropdowns run to the row's right edge, text fields are as tall as the
  buttons beside them, a checkbox is a square box and a radio button a
  round one, a greyed button still looks like a button, and a hint longer
  than its field ends in “…” (hover the empty field to read all of it).
- **The library column has one layout in every state.** Its title row has
  **Open folder…** at the right, and under it the folder's name and photo
  count (hover the line for the full path). The batch buttons are two rows
  of two — **Copy recipe** and **Paste (n)**, **Render (n)** and
  **Deselect** — with **Paste crop / straighten too** always shown under
  them. A button that cannot run yet is greyed in its place and says under
  the pointer what it is waiting for, so the list of photos no longer moves
  when you copy a recipe or pick photos. Each row shows the photo's name,
  its format when it is not a RAW, and **● edited** when it has a saved
  develop or a Lightroom sidecar.
- **Both side columns keep the width you drag them to.** What is inside a
  column no longer widens it.

## Lightroom

- **AutoShade mentions Lightroom only when you ask for a Lightroom file.**
  What a sidecar cannot carry — bitmap masks, a lens blur, the camera base
  curve and the like — used to be named after every save and when an AI
  analysis landed. It is now said at one moment: **Export .xmp beside the
  photo**, the click that hands the file to Lightroom. Saving and an
  analysis write the sidecar as they did; they say nothing about Lightroom.
  Your `recipe.json` keeps everything either way.

## Fixed

- **A JPEG in the library is labelled as a JPEG.** Every photo that was not
  a RAW read “PNG/TIFF”; a row now names the file's own format.

## Known limits

- **A target repainted over its whole frame gets no colour field.** The
  colour field needs some part of the two pictures to be the same place. A
  target that shows the same scene but was re-synthesised everywhere is
  matched by the whole-frame and zone controls alone.
- **The sidecar's starting tone is declared, not carried**: opened in
  Lightroom, a photo whose recipe starts on the camera base curve renders on
  Lightroom's own starting tone.

## Known changes

- **Saving no longer says what Lightroom will not get.** To read it, use
  **Export .xmp beside the photo**. The command line and the browser UI are
  unchanged: they name it whenever they write a sidecar.
- **Shorter names in the library.** “Paste to selected (n)” is **Paste
  (n)**, “Render selected (n)” is **Render (n)**, the ✕ button is
  **Deselect**, and “Include crop / straighten when pasting” is **Paste
  crop / straighten too**. Each does what it did.
- **The side columns have new narrowest widths.** The controls column is at
  least 320 px wide (280 px before) and the library column at least 240 px.

## Compatibility

No new option, no new saved preference and no new recipe or sidecar field.
Recipes, sidecars and settings written by earlier versions open as they did
in v1.18.1. The code that develops a photo, fits a look and writes a sidecar
is v1.18.1's — outside the desktop window this release changes five source
comments and no statement.

## Gates

This release changes the desktop window's code and the documents; outside
the window's sources five comment lines differ from v1.18.1. No local
three-lane battery was run; the whole suite ran on the project's online
runners on `efde43c`, the commit that carries this release's code (run
44246002878, 27 jobs, each platform's library tests cut in eight shards). On
Linux: library **1979 passed / 0 failed / 21 ignored**, CLI **28 / 0**,
contract 2 + 2, doc-tests 0, GUI **316 / 0 / 7**. On macOS:
library 1977 / 0 / 21, CLI 28 / 0, GUI 316 / 0 / 7. The
debug-assertion library trip: 1979 / 0 / 21. On Windows the GUI's tests
were run on the maintainer's machine: 316 / 0 / 7. By name against
v1.18.1, the names as the Windows test program lists them: GUI 301 → 323
(+24 / −2, the two removed being two tests renamed); library and CLI
unchanged. Each new rule of the window was broken by hand once and the test
that guards it turned red (29 such changes, each put back byte for byte).
`audit_i18n` 0, the font check (924 of 924 code points embedded) and
`check_doc_drift` (0 findings); clippy 0 warnings on the GUI feature set; the
Python suite under `scripts/` 57 tests, all passing (`python/` is unchanged
since v1.18.0 and its suite was not run); `check_docs.py` 28 / 0 / 4 PASS /
FAIL / SKIP — the skipped rows are the test-count claims, which it checks
only against a local battery's transcript. The maintainer used a trial build
of the new window before the release.

Since this release the online runs choose their tests by what a commit
changed. The long tests — the library tests of the reverse fit's and the
colour-grain denoiser's own modules, 446 of the library's 2003 — are set aside
when nothing they can read differs from a commit on which they passed, and
every other test runs on every push as before; on any doubt a run takes
everything. The commits after `efde43c` (this change to the runs, the
documents and the version number) change nothing a long test can read; the
run above is where the long tests ran for this release. The rule was read
before it was used: 12 of its conditions broken by hand, each caught by its
own test, and the test-splitting script walked through its eight shards in
both modes on the 2003 real names — every test once with the rule off, every
test that is not long once with it on.

Not run, and why: the reference-pair check of the reverse fit (no statement
of the fit or of the develop engine changed); clippy on the default feature
set (outside the window it would read comments only); the library names that
exist on Windows only (the library is v1.18.1's); the tests gated on the
calibration photos (they read fits, and the fits are v1.18.1's).
