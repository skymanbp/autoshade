# AutoShade v1.18.5 — the full picture sooner, and a check for updates

Everything merged into `main` since v1.18.4 ships here. On a photo with
raster masks or a colour field the full-quality picture comes sooner after a
change, and it is the same picture, byte for byte. The desktop app can tell
you when a newer version is out: it asks GitHub for the latest version
number, at most once a day, and you can turn that off. The masks, the reverse
fit, the denoiser, the AI analysis and the `.xmp` are v1.18.4's.

The design, the rulings and this document are the maintainer's. The code of
this release was written with an AI coding assistant working under the
maintainer's direction, and every change passed the project's own tests
before it shipped.

## Preview

- **The full picture comes sooner on a photo with raster masks.** A raster
  mask — one that follows a subject, a sky or an object, a brushed one, or
  one a reverse fit made — is read along its edge before it is applied. Part
  of that reading depends on the mask's shape alone, and is now kept while
  the photo is open instead of being worked out again at every redraw. It is
  kept from the second redraw of a photo on, so the third and later redraws
  are the quicker ones — the draft while you drag as well as the full picture
  when you let go.
- **The colour field is read on every processor core.** A photo that carries
  a colour field — a reverse fit's — was held up by one core at that step.
- **The picture is the same picture, byte for byte**, on the canvas, in a
  saved develop and in an export.

## Updates

- **Settings → About says whether a newer version is out.** The **Updates**
  row shows what the last check found. **Check now** asks at once. When a
  newer version is out, **Open the download page** opens its release page in
  your browser, and **Skip this version** stops the automatic check
  mentioning that version again.
- **The automatic check runs at most once a day**, in the background, after
  the window is up. When it finds a newer version it says so once, in a
  notice; when it cannot reach GitHub it says nothing.
- **What is sent.** One request to GitHub for the latest release of this
  project. It carries the program's name and version and nothing else — no
  photo, no file name, no setting, no identifier.
- **Nothing is downloaded or installed.** Upgrading is still the installer
  from the release page.
- **Turning it off.** Untick **Check for updates automatically** in the same
  row, or set the environment variable `AUTOSHADE_NO_UPDATE_CHECK` to any
  non-empty value. **Check now** still asks when you press it. The command
  line and the web UI never check.

## Known limits

- **A whole-photo control and the first mask still redraw everything.** While
  you drag you see the draft; the full picture follows sooner than it did,
  and is not instant on a large photo with many masks.
- **A target repainted over its whole frame gets no colour field.** The
  colour field needs some part of the two pictures to be the same place. A
  target that shows the same scene but was re-synthesised everywhere is
  matched by the whole-frame and zone controls alone.
- **An exported `.xmp` declares its starting tone, it does not carry it**:
  opened in Lightroom, a photo whose recipe starts on the camera base curve
  renders on Lightroom's own starting tone.

## Known changes

- **The desktop app contacts GitHub once a day at most**, unless the
  automatic check is turned off (Updates, above).
- **The app holds a little more memory while a photo with raster masks is
  open** — up to about 256 MB, for the kept part of the masks' edge reading —
  and gives it back when you open another photo.

## Compatibility

Develops, recipes and `.xmp` files are read and written exactly as v1.18.4
reads and writes them: this release adds no recipe field, so a develop it
saves opens in v1.18.4. The desktop app's settings file gains the update
check's entries. No command and no option is added or removed.

## Gates

This release changes two steps of the library's renderer (the read of a
raster mask's band and the colour field's read), adds the library's update
module and adds the Updates row to the desktop window. No local three-lane
battery was run; the whole suite, long tests included, ran on the project's
online runners on `e235b6a`, the commit that carries this release's code
(run 44840410765, 28 jobs, each platform's library tests cut in eight
shards). On Linux: library **2052 passed / 0 failed / 22 ignored**, CLI **30
/ 0**, contract 2 + 2, doc-tests 0, GUI **371 / 0 / 12**. On macOS: library
2050 / 0 / 22, CLI 30 / 0, GUI 371 / 0 / 12. The debug-assertion library
trip: 2052 / 0 / 22. On Windows, on the maintainer's machine and by name:
the library's renderer and update modules 427 / 0 / 5, with the folder of a
real Lightroom export pack named, so the renderer's tests that read it ran;
the four library names that exist on Windows only are among them; the
command line 30 / 0; every test of the desktop window 371 / 0 / 12; the two
contract programs 2 + 2. By name against v1.18.4, the names as the Windows
test programs list them: library 2050 → 2077 (+27 / −0), CLI 30 → 30 (the
same names), GUI 374 → 383 (+9 / −0). The rules this release adds — what a
kept plane is kept under, when it is kept and when it is given back, and
each rule of the update check — were broken by hand, 11 and 10 of them; the
test that guards each turned red, and each such change was put back byte for
byte. `audit_i18n` 0, the font check (924 of 924 code points embedded) and
`check_doc_drift` (0 findings); clippy 0 warnings on both feature sets, test
code included; the Python suite under `scripts/` 57 tests, all passing
(`python/` is unchanged since v1.18.0 and its suite was not run);
`check_docs.py` 28 / 0 / 4 PASS / FAIL / SKIP — the skipped rows are the
test-count claims, which it checks only against a local battery's
transcript, and the census of the maintainer's own sidecars. The Updates row
was read on the offline camera's pictures in both languages. No test and no
build step contacts GitHub: the update check's tests answer from a stub on
the machine itself.

Saved photos, drawn again. The three develops v1.18.4 fitted for the
reference pair — zones on raster masks and a colour field — were drawn at
the editor's canvas size and at 1000 px by this build and by v1.18.4's code
built with the same compiler: the same bytes, each of the six pictures.

The reverse fit, checked on the reference pair. This build fitted the pair
again at 0.65, 0.85 and 1.0. Against the same three fits kept from v1.18.4,
the recipe, every mask and the render are the same bytes. The tests that
need the calibration photos were run for what this release touches, the
renderer's and the colour field's: 462 / 0 / 7.

The commits after `e235b6a` (the documents and the version number) change
nothing a long test can read, so the online runs on them set the long tests
aside; the run above is where the long tests ran for this release.

Not run, and why: the other tests gated on the calibration photos (the
reverse fit's and the denoiser's modules are byte for byte v1.18.4's). The
real window was not opened; its pictures come from the offline camera. The
update check's own code asked GitHub once, by hand, before the release: it
read v1.18.4 as the latest release and the build as up to date. The line
that says a newer version is out was not seen against GitHub — before this
release existed there was no newer release to find; the stub tests cover it.
