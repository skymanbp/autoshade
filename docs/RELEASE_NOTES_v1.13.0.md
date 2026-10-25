# AutoShade v1.13.0 — a mode page, and a Normal mode for phone photos

Everything merged into `main` since v1.12.0 ships here. The desktop app now
opens on a page of three modes. **Landscape** is the workspace exactly as it
was; **Portraits** is the same workspace with the local-area switches folded
away, ahead of portrait tools in a later version; **Normal** is new — a short
panel for phone and jpg photos built around a one-click look (six looks the AI
reasons over, or "match my reference photos" from a folder you pick), four
plain sliders, a noise switch and saving as jpg, with no darkroom jargon
anywhere on screen. Underneath, the reverse fit — the step that recovers an
editable recipe from a target picture — had four root causes fixed in how it
reads tone and white balance and checks its own corrections, its local
corrections were reworked, and it now hands a colour cast over to the white
balance when per-channel curves would twist the hues. On the eleven photo
pairs of the final check, against the build before those changes: 5 closer
to their targets, 5 the same and 1 further — the neon night street, at a mean
colour difference of 7.52 against 6.75 (lower is closer).

The design, the code and this document are the maintainer's; no external
coding model wrote this release.

## The mode page

- **Every launch starts here.** Three cards — Landscape, Portraits, Normal —
  and a click enters the workspace. The card you chose last is marked
  **Last used**; there is no "skip next time", so the page is always the first
  thing you see.
- **Back to the page at any time.** Inside the workspace the mode's name is
  the first button of the toolbar. It brings the page back without closing
  anything: the open photo, its edits, its versions and the library stay as
  they are, and switching mode only changes which controls are drawn.
- **Landscape** draws everything, exactly as v1.12.0 did. **Portraits** draws
  everything too, with the reverse fit's two local-area switches inside a
  **Local areas** fold that starts closed; the face tools come later.
- **The mode belongs to the desktop app only.** Recipes do not record it,
  every photo opens in every mode, and the command line and the browser page
  have no modes.

## Normal mode

Normal is for photos from a phone or any jpg. Its panel answers four
questions, top to bottom, and shows nothing else.

- **One-click look.** Six cards — **Natural**, **Bright & clean**, **Warm**,
  **Cool night**, **Film** and **Black & white**. Each one is a short
  description of a look that the AI reasons over for your photo, not a fixed
  set of slider values; the result lands on the sliders and `Ctrl+Z` takes it
  back. Or click **Match my reference photos…** and pick a folder of photos
  whose look you like: the app reads the folder on this computer (the first
  time, it downloads a 1.5 GB model to do so), then the AI looks at your photo
  together with the three photos from the folder that are most like it and
  gives yours their look. Each of those three is billed as one more picture.
  **Look strength** (65 by default) sets how strong the look is. Without an
  image key in Settings the app uses a simple built-in look and says so.
- **Normal keeps its own reference photos.** The folder you pick here is kept
  in a library file of its own, so it never replaces the reference library
  you built in Landscape.
- **Fine-tune.** **Brightness**, **Contrast**, **Warmth** and **Vividness**
  move the photo on top of the one-click result, and **Back to the one-click
  result** puts them back. Warmth runs from −100 to +100 with 0 at the
  camera's own colour; the two ends halve or double the colour temperature,
  and no kelvin number is shown.
- **Noise.** **AI denoise** is the app's existing denoiser at its default
  strength, applied when you save (the preview does not show it). The first
  time, the noise model is downloaded (about 72 MB).
- **Save.** **Save as jpg** at three quality steps (High, Medium, Small file)
  to the same destination the full workspace's Export uses — the app's output
  folder, the folder you last saved to, or a question each time, as set in
  Settings; it never goes next to the photo, and the original is never
  changed. **Apply this look to every photo in the folder** settles the folder
  first (cancelling the dialog changes nothing), then gives every photo of the
  open folder the same look and saves each as a jpg. `Ctrl+S` and `Ctrl+E`
  save the jpg, and the status bar's unsaved marker follows the jpg. Outside
  the app's output folder, photos saved earlier are never replaced: the app
  says so in plain words, and you pick another folder or move the old ones.
- **Less on screen.** The toolbar keeps the mode button, **Open photo…**,
  undo, redo, **Compare** and Settings; the library keeps only the
  thumbnails; the canvas draws no tool marks and there is no strip of
  versions — each one-click replaces the look, and undo brings the previous
  one back. Words such as recipe, develop, reverse fit, variant, mask or API
  appear nowhere in Normal: a test reads every string the frame draws, in
  English and Chinese, and fails on any of them. Switch to Landscape from the
  mode button for every other control; the photo and its look come along.

## Two AI sections, and AI adjust in place of AI Refine

- **Two sections.** The side panel now keeps the two AI paths apart under two
  headings, each with its own fold: **AI generation & reverse-fit**
  (Reimagine, Adjust generated image, Reverse-fit) and **AI analysis & auto
  develop** (Analysis, Adjust the selected variant, Reference library). The
  analysis section keeps its old fold state, so it opens as you left it.
- **AI adjust replaces AI Refine.** Type what to change ("darken the sky a
  little, warm the faces") and click **🤖 AI adjust**. The model now sees the
  card as it looks now — a card on AI-generated pixels sends its own picture,
  where AI Refine sent the RAW's preview — together with its current recipe,
  and returns the complete recipe that replaces it.
- **The AI can point at a part of the photo.** When the answer names the sky,
  the subject or an object, the app cuts that area itself, on this computer,
  after the answer; the rationale says the area is an approximation, not a
  hand-drawn selection, and a target it cannot find is dropped and named.
- **More controls for the AI.** Analysis and adjust may now also set the
  post-crop vignette, grain, the Black & White treatment and the Calibration
  panel; a control the model has no opinion on keeps your value.

## The reverse fit: four root causes

The reverse fit compares your photo with a target picture and writes the
recipe that turns one into the other. Four places where it read the wrong
thing are fixed.

- **White balance from every pixel, after the tone.** The colour the target
  asks for is now read as the middle colour of every pixel of the photo as it
  looks once the tone is fitted, not of the photo before it. A colour that
  only a minority of the frame asks for no longer cancels the white balance
  unless it covers at least a fifth as much of the photo as the majority. On
  the night sky this removes a green band along the horizon.
- **Tone from all pixels.** The fitted tone used to follow only the grey
  pixels of the frame; on a street where those were dark walls the target had
  pressed down, it darkened the whole photo. It now reads every pixel.
- **No sudden lift.** Where the tone relation has to be smoothed into a
  rising curve, a handful of edge pixels can no longer lift a whole sky; the
  smoothing weighs each part of the curve by how many pixels back it.
- **Checks along the move.** The cell-by-cell check that a correction really
  brings the photo closer now measures along the direction that control
  moves — colour for a colour move, brightness for a tone move.

The local corrections (the reverse fit's per-area pass for sky, buildings,
trees and the like) were also reworked. An area whose pixels do not pair up
with the target's falls back to matching the area's brightness distribution,
fitted on 24 points instead of eight fixed ones, so a bend inside a dark area
is seen. A correction that reshapes an area's contrast is accepted when it
matches the target's distribution and the whole frame does not pay for it.
The ruler that refuses visible seams at an area's edge no longer charges a
one-step colour difference (the smallest an 8-bit render can show) and no
longer reads the inside of a mask as a seam.

## A colour cast handed to the white balance

On the neon night street the target is red. With the white balance now read
from every pixel, the per-channel colour curves had to carry the whole red
push, and fitted channel by channel they would have turned the bright crimson
orange — a 44.9° spread of hue — so they were flattened and the red was lost.
A white balance moves the whole frame's colour without twisting hues, so when
the curves would twist them past the limit, the fit now tries white balances
in the direction the curves pull, re-fits the curves at each, checks each
candidate with the same tests the curves face, and keeps the one the fit
scores closest to the target. Where several score the same within the fit's
own precision, it keeps the one that leaves the curves the least
white-balance work. When no candidate passes, the result is the same as
before.

On the neon night street: mean colour difference 8.22 → 7.52, white balance
10100 → 40000 K, hue spread 44.9° → 2.6°, sky redness (a\*) +36.45 → +45.25
against the target's +44.80. The other ten pairs come out with the same
settings, value for value. When the search ends on the edge of the white
balance range, as it does here at 40000 K, the rationale says so.

## Local contrast per area, and a smoother colour field

- **Local contrast matched per area.** The last step of the local corrections
  now matches the target's fine and mid-scale contrast (the texture and
  clarity sliders) area by area — only where the two pictures share
  structure, never above the target's own detail, and within the frame's
  budget. Where the target's detail is not in the photo, nothing ships and
  the rationale says why: on the night sky the target's stars are about 154
  times brighter than the sky behind them, against half the sky's level in the
  RAW, so they were painted by the image model and are not matched.
- **A colour field with four times the rows.** The smooth colour correction
  the fit lays over the whole frame now has 32 rows of points instead of 8.
  On the night sky the horizon glow had been drawn as a polyline across the
  rows: the ripple along the sky's profile fell 2.40 → 0.63 and the sky's
  colour difference 3.96 → 3.26. A recipe that carries a colour field is
  larger for it.
- **A measured gradient.** For a sky glow that rises like the target's own
  profile, the fit can now write a linear gradient whose falloff follows a
  measured curve. Lightroom gets it as an ordinary linear gradient on the same
  handles, which renders Lightroom's own falloff.
- Before the root-cause work above, this batch on its own left none of the
  eleven pairs further from its target, each within 0.06 of the build before it.

## Compatibility

- **Settings.** The desktop app's preferences file gains the mode, Landscape
  by default; a preferences file from v1.12.0 or earlier loads unchanged, and
  the page marks Landscape as last used.
- **Reference libraries.** Landscape's reference library is the same file as
  before. Normal's lives next to it in a file of its own
  (`reference-library-normal.json`), created the first time you pick a folder
  in Normal.
- **Recipes.** Recipes from v1.12.0 open and render as before. A recipe the
  v1.13.0 reverse fit writes can carry the measured gradient, which v1.12.0
  cannot read, so open such a recipe with v1.13.0. The XMP sidecar for
  Lightroom carries that gradient as an ordinary linear gradient.
- **Command line and browser page.** Their commands, flags and routes are
  those of v1.12.0. When a save would replace files written earlier, the
  command line still refuses with the same message; the desktop app now
  recognises that refusal to put it in plain words in Normal.
- The sidecars under `python/` are byte for byte v1.12.0's, and the weights
  stay pinned to the v1.6.0 `autoshade-raw-denoise-v2.pth`.

## Gates

Before the frozen snapshot, on `8b4f4ec`: library **1687 passed / 0 failed /
15 skipped**; the desktop and command-line binaries **286 passed / 0 failed /
4 skipped** (268 / 3 at v1.12.0); the reverse-fit battery 387 passed; the
i18n audit 0 findings, `check_doc_drift` 0 findings, `check_docs` 27 PASS /
0 FAIL / 5 SKIP without a transcript.

The release battery (`scripts/release_battery.sh`: the library, the desktop
app and command line, and the calibration run, on a frozen copy of the release
code at `968d348`, with the calibration photos and the denoiser weights in
reach): library **1687 passed / 0 failed / 15 ignored** (754.82 s), CLI
**25 / 0**, contract 2 + 2, doc-tests 0, GUI **261 / 0 / 4**, calibration run
**1687 / 0 / 15** (833.27 s, no corpus-gated test skipped); `audit_i18n` 0
findings, the font check (924 of 925 code points embedded, one left to egui's
bundle) and `check_doc_drift` (0 findings over 14 documents); clippy 0
warnings on both feature sets; `check_docs.py --gates` 28 PASS / 4 FAIL on the
snapshot — the four test-count claims, still at v1.12.0's figures — and
**32 PASS / 0 FAIL / 0 SKIP** once the counts were moved. By name against the
v1.12.0 release battery: library 1656 → 1702, **+49 / −3**; GUI 246 → 265,
**+20 / −1**; CLI 25.

The first round of that battery, on `8b4f4ec`, was not green: the development
batteries had run without the calibration photos in reach, so three
corpus-gated pins (the two-temperature pair, the default-strength white
balance, the calibration sky zone) still carried the figures from before the
root-cause work, and clippy 1.94 named three new lints. `968d348` re-pins the
three with the mechanism that moved each written beside the figure, and the
battery above is the re-run on that commit.

Final check, reference pairs: eleven pairs, each render's mean colour
difference (CIE76 ΔE, both pictures at 480 px on the long edge) against its
target, against the build before the root-cause work. Closer by more than 0.3:
golden-hour street 33.17 → 32.76, busy street 13.03 → 7.05, daylight street
10.73 → 6.34, blue hour 4.90 → 4.27, day suburb 3 5.83 → 5.34. The same:
night sky 5.22 → 5.32, desert dusk 4.47 → 4.53, sunset city 4.35 → 4.51, day
suburb 1 13.00 → 12.84, day balcony 8.66 → 8.76. Further: neon night street
6.75 → 7.52. What is left on that pair is the sky's red push at the tree
crowns, where the sky's area reaches into the trees and the boundary check
correctly refuses to push red into pixels the target darkens; it belongs to
how the sky area is cut, and is left for a later version.

The Python suites were not run: `python/` and `tests/` are byte for byte
v1.12.0's, and the two changed files under `scripts/` are measurement scripts
for the colour field, outside the battery.
