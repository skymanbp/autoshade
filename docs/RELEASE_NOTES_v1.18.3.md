# AutoShade v1.18.3 — an .xmp only when you ask

Everything merged into `main` since v1.18.2 ships here. AutoShade keeps its
own record of your edits, and from this release it treats a `.xmp` as another
program's file: it reads one only when you say so and writes one only when
you export it — in the desktop app, on the command line and in the browser UI
alike. The Versions section of the Develop panel is rebuilt, and a variant
keeps its kind when the photo is opened again. The develop engine, the
reverse fit and the AI analysis are v1.18.2's.

The design, the rulings and this document are the maintainer's. The code of
this release was written with an AI coding assistant working under the
maintainer's direction, and every change passed the project's own tests
before it shipped.

## Saving

- **`Ctrl+S` saves the develop and says so.** A save writes the photo's
  develop — its adjustments, its variants and the link to any retouched
  image — into the per-user develop store, and the status line reads
  **develop saved**. Nothing is written beside the photo, and no `.xmp` is
  written anywhere.
- **A photo you have not changed has nothing to save.** `Ctrl+S` on it says
  so and writes nothing. After **Reset**, `Ctrl+S` clears what was saved and
  says that the photo is back to untouched.
- **● edited means saved in AutoShade.** The mark in the library is lit when
  the photo has adjustments AutoShade saved, and hovering it says so. A
  `.xmp` beside the photo does not light it, and it goes out when you reset
  the photo and save.

## The .xmp

- **Opening a photo never takes a `.xmp` over.** The photo opens as AutoShade
  last saved it, or untouched if it never did. When a `.xmp` holding
  adjustments sits beside a RAW — Lightroom's, usually — a question asks
  whether to read them in. **Read them in** puts the adjustments on the
  canvas as one step you can undo; nothing is saved until you press `Ctrl+S`.
  **Don't read** leaves the photo as it is. Either answer is remembered for
  that file, so you are asked again only when the file changes. A RAW that
  carries adjustments another program wrote into the file itself, as a DNG
  can, is asked about the same way when nothing sits beside it and nothing
  is saved for it. The Landscape and Portraits workspaces ask; Normal mode
  never does.
- **Two buttons in the Export section.** **Export .xmp beside the photo**
  writes the adjustments on the canvas, changes you have not saved included,
  as a `.xmp` in the photo's own folder, where Lightroom reads it. A `.xmp`
  already there is never replaced by one click: the button says so and asks
  for a second, and the file that replaces it keeps whatever in the old one
  AutoShade does not model. **Import adjustments from an .xmp…** reads any
  `.xmp` you pick onto the canvas, as one step you can undo.
- **On the command line, `export-xmp`.** `autoshade export-xmp <raw>` writes
  a RAW's saved develop as a `.xmp` — to `./out/<name>.xmp` by default, to
  the path `-o` names, or with `--beside` into the RAW's own folder (a file
  already there is replaced only with `--force`, and is merged, not
  discarded). No other command writes a `.xmp`.
- **In the browser UI, Export .xmp.** **Save** writes the develop only, and
  the new **Export .xmp** downloads the saved develop as a `.xmp`.

## Versions and variants

- **The Versions section lists versions and nothing else.** One line says
  what a version is, **Save as version** runs the full width, and every row
  carries its number, its name, **Load** and delete on one right edge. Where
  a version came from, and the filter for the current variant's versions,
  appear only when the photo has more than one variant. The list of variants
  it used to repeat is in the strip under the canvas alone.
- **A variant keeps its kind.** A denoised or stacked variant is a denoised
  or stacked variant when the photo is opened again, with its image; a
  variant whose image can no longer be found keeps its kind too, and says
  that the image is missing.
- **One word for a variant in Chinese.** The Chinese text says 「变体」
  everywhere, and a version is 「版本」.

## Fixed

- **A photo no longer comes back with two Originals.** A photo with a
  denoised or stacked variant, or one whose image did not load, used to
  reopen with a second Original that could not be deleted, and its saved
  edits could no longer be cleared. A photo saved that way is repaired the
  next time it is opened, and one line says what was repaired.
- **A photo with a stacked variant behind the canvas opens whole.** Its other
  variants used to be hidden and every save refused.
- **● edited no longer stays lit after you clear a photo's edits** when a
  `.xmp` sits beside it.
- **Chinese punctuation matches the text around it.** The full-width
  brackets and the question mark were drawn heavier than the Chinese text
  they stood in.

## Known limits

- **A target repainted over its whole frame gets no colour field.** The
  colour field needs some part of the two pictures to be the same place. A
  target that shows the same scene but was re-synthesised everywhere is
  matched by the whole-frame and zone controls alone.
- **An exported `.xmp` declares its starting tone, it does not carry it**:
  opened in Lightroom, a photo whose recipe starts on the camera base curve
  renders on Lightroom's own starting tone.

## Known changes

- **No `.xmp` appears on its own.** Earlier versions wrote one with every
  develop they saved — a save, an analysis, a batch run — and kept it with
  the develop. To give a photo's adjustments to Lightroom,
  export the `.xmp`: the button in the desktop app, **Export .xmp** in the
  browser UI, or `export-xmp` on the command line.
- **A photo that only has a `.xmp` beside it opens untouched.** Earlier
  versions showed the `.xmp`'s adjustments at once and marked the photo
  **● edited**. It now opens untouched, without the mark, and the question
  offers to read the adjustments in.
- **`batch` analyzes a photo that only has a `.xmp` beside it.** It used to
  count such a photo as developed and skip it. Each analysis is a call to the
  AI provider you configured, so a folder of Lightroom-edited photos costs a
  call apiece.
- **The `.xmp` an earlier version kept inside the develop store goes with the
  next save.** Your adjustments are in the develop itself; the `.xmp` beside
  a photo is never removed.
- **Shorter save lines.** A save names no file and no format, and a paste
  reads **Adjustments pasted to n photo(s)**.

## Compatibility

Develops saved by earlier versions open as they did, and a develop an earlier
version kept only as a `.xmp` inside the develop store still opens. No recipe
field is added or removed. The develop folder gains one small file, the
remembered answer to the `.xmp` question. `export-xmp` is the one new
command; no command and no option is removed. The browser UI's own save
request is now `POST /api/save`.

## Gates

This release changes the library's store, pipeline and web-server modules,
the command line and the desktop window. No local three-lane battery was
run; the whole suite, long tests included, ran on the project's online
runners on `bfdcff5`, the commit that carries this release's code (run
44500439286, 28 jobs, each platform's library tests cut in eight shards). On
Linux: library **1989 passed / 0 failed / 21 ignored**, CLI **29 / 0**,
contract 2 + 2, doc-tests 0, GUI **347 / 0 / 9**. On macOS: library 1987 / 0
/ 21, CLI 29 / 0, GUI 347 / 0 / 9. The debug-assertion library trip: 1989 /
0 / 21. On Windows, on the maintainer's machine and by name: the library's
store, web-server, pipeline and `.xmp` modules 513 / 0 / 3; the library
names that exist on Windows only, by name, 4 / 0; the command line 29 / 0;
every test of the desktop window 347 / 0 / 9; the two contract programs 2 +
2; and the round trip through a real Lightroom export pack, 1 / 0. By name
against v1.18.2, the names as the Windows test programs list them: library
2003 → 2013 (+26 / −16), CLI 28 → 29 (+1), GUI 323 → 356 (+39 / −6); the
names removed are those of the automatic sidecar readers and writers,
removed with them, and of the list of variants the Versions section no
longer repeats. Each new rule was broken by hand once and the test that
guards it turned red, and each such change was put back byte for byte.
`audit_i18n` 0, the font check (924 of 924 code points embedded) and
`check_doc_drift` (0 findings); clippy 0 warnings on both feature sets; the
Python suite under `scripts/` 57 tests, all passing (`python/` is unchanged
since v1.18.0 and its suite was not run); `check_docs.py` 28 / 0 / 4 PASS /
FAIL / SKIP — the skipped rows are the test-count claims, which it checks
only against a local battery's transcript. The window's new surfaces — the
Versions section, the `.xmp` question, the Export section — were read on the
offline cameras' pictures in both languages.

The commits after `bfdcff5` (the documents and the version number) change
nothing a long test can read, so the online runs on them set the long tests
aside; the run above is where the long tests ran for this release.

Not run, and why: the reference-pair check of the reverse fit and the tests
gated on the calibration photos (no statement of the fit or of the develop
engine changed: the reverse fit's and the denoiser's modules are byte for
byte v1.18.2's, and the render module differs in one test and in comments).
