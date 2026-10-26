# AutoShade v1.15.0 — a blemish remover held to the face, fixes from a read-through, and a steadier colour-cast handover

Everything merged into `main` since v1.14.0 ships here, in three parts. The
**Portraits** panel's skin group gains **🤖 Remove blemishes**: the AI finds
small spots and blemishes and each is healed from the skin around it, but
only the spots on the face; a spot found anywhere else is left as it was. The
face model that MediaPipe's face finder downloads now has a byte-exact copy
of ours, tried before Google's server, like every other model the app
downloads. Second, a read-through of the whole program — the desktop app;
the rendering, recipes and the develop store; the reverse fit; the Python
helpers; the command line, the browser page and the documents — found a set
of bugs, each fixed at its cause with a test that failed first. Third, the
reverse fit's handover of a colour cast to the white balance keeps to the
limits the rest of the fit keeps to.

The design, the code and this document are the maintainer's; no external
coding model wrote this release.

## Portraits mode

- **🤖 Remove blemishes.** In the skin group, under **Keep skin natural**.
  It runs the same AI spot finder and heal as **🤖 AI heal (auto)** under
  **Heal (pixel)**, held to the face mask that **🤖 Find people and faces**
  made: a spot is healed only when its centre lies on the face (where the
  face mask is at least half white), and every spot outside the face is
  dropped before any pixel is changed. A spot on the face's edge is healed
  whole, so its heal reaches a little past the face. The status line says how many spots
  were healed on the face, and `Ctrl+Z` takes the step back like any heal.
- **When it can run.** The button is greyed until faces have been found,
  while another job is running, and when no photo is open. Finding the spots
  is an AI call: it needs an image key in Settings (⚙), as **🤖 AI heal
  (auto)** does.
- **Nothing on the face is not a failure.** When the AI finds spots but none
  of them is on the face, the status line says **No spots on the face to
  heal — nothing was changed**, with how many were found elsewhere and left
  alone, and the photo is not touched.
- **Chinese label.** In Chinese the button reads **🤖 去掉斑点**.
- **The face model has a copy of ours.** The Face Landmarker bundle the face
  finder downloads on its first search (3,758,596 bytes, about 3.8 MB) now
  also lives in a repository of ours on Hugging Face,
  `Azng0/autoshade-mirror-mediapipe-face-landmarker`, at a fixed commit. The
  copy was checked against the SHA-256 the app pins before it was uploaded,
  and read back without signing in and matched again, byte for byte, after;
  the repository carries MediaPipe's Apache-2.0 licence text. The download
  tries our copy first and Google's versioned address after it, and the
  pinned SHA-256 decides whether the bytes are used, wherever they came from.
  In v1.14.0 this model came only from Google's server.

## Fixes from a read-through of the whole program

### Desktop app

- **Pasting a look over a photo with unsaved changes.** **Apply this look to
  every photo in the folder** (Normal) and the workspace's paste to several
  photos wrote the look to every photo, but set aside the unsaved changes of
  the open photo only. Another photo you had changed and left without saving
  kept those changes in memory, and they were drawn and saved back over the
  pasted look. The paste now replaces the unsaved changes of every photo it
  writes.
- **The AI portrait look is saved as you see it.** **✨ AI portrait look**
  saved its result before the face, eyes, skin and background masks and the
  background blur were put back on it, and before **Keep skin natural** had
  held it to its limits; the panel showed the right look, but the file on
  disk was not that look. The saved file is now the one on screen.
- **Keep skin natural in more places.** Switching into Portraits from
  another mode changes no value: a card whose edit went past the skin limits
  in that mode has the switch off, as opening such a photo does. An undo or a
  redo that lands on an edit past the limits puts that edit back as it was
  and turns the switch off for the card, instead of pulling the values back
  and wiping the redo steps. A paste in Portraits holds the skin of each
  photo it writes, for every photo whose switch is on, not only the open one.
- **Match my reference photos.** Reading the reference folder (minutes the
  first time) did not count as work in progress, so another job could start
  over it, and the look that follows could be dropped without a word or made
  for a photo opened in the meantime. The buttons that start work now wait
  for the reading, the look is made for the photo the folder was picked for,
  and when it cannot start the line says so: **Your reference photos are
  read, but the look could not start.**
- **Normal's AI denoise keeps your undo steps.** Turning **AI denoise** on
  or off emptied the undo history of your slider changes; the steps now
  stay.
- **A new face search that finds no face.** The old Face, Eyes and Skin
  masks stayed in effect after a search that found no face; they are now
  taken away.
- **Normal remembers the folder you saved to.** A folder picked in Normal's
  save window was never remembered; it now is, as the workspace's Export
  remembers its folder.
- **AI masks from a Lightroom sidecar are drawn on opening.** An AI mask
  read from a Lightroom sidecar carries no pixels of its own, and the app did
  not draw it, so it had no effect. It is now drawn on this computer when the
  photo opens and before an export, and so is a face or eyes mask the AI
  asked for; the command line already did this. A mask that cannot be drawn
  is named on the status line and left out.

### Library and store

- **The lens blur's maps and the pixel layers are kept like masks.** The
  depth map and the keep-sharp mask of the background blur, and the image of
  each pixel layer, were left out of the rules that already held for mask
  files. Now, when a develop is opened, a network path or a path that climbs
  out of the develop folder is not followed, as for a mask; a file inside the
  develop folder is saved by its bare name, so the folder can be moved; a
  saved version keeps its own copy, so a later edit cannot change it; a
  version made the current edit gets fresh copies; and a Lightroom sidecar
  carries bare names for them.
- **Masks are drawn on the full frame of a RAW.** The AI masks and the face
  masks of a RAW were drawn on the preview the camera embeds in the file,
  which on some cameras is a crop of the full frame, so on those photos the
  masks sat in the wrong place. They are now drawn on the RAW developed
  plainly from the sensor. Masks cached from the old frame are drawn again
  once.
- **The skin colour is read from the photo as it is developed.** The Skin
  mask takes its colour from the face; it now reads that colour from the
  photo as the edit develops it before the mask, as the eyedropper does,
  rather than from the photo before any edit.
- **No outputs inside the photo folder through `./out`.** The default
  output folder `./out` is relative to where AutoShade is run, so when run
  from inside a photo folder it pointed into that folder, and outputs were
  written there. That write is now refused like any other write into the
  photo's own folder, with a hint to run from another folder or pass `-o`
  with a folder outside the photos. A Delivery folder set in Settings as a
  full path is unchanged.
- **No second denoise.** The browser page's export and `auto --denoise`
  denoised a second time a card the desktop app had already denoised. They
  now leave it as it is; `auto` says so on one line.

### Command line and browser page

- **What a Lightroom sidecar leaves out.** When a recipe saved from the
  command line or the browser page uses controls a Lightroom sidecar cannot
  carry (a lens blur, fill layers, healed spots, the colour field, the camera
  calibration), they are now named, as the desktop app already named them.
- **Analyze saves the way the app saves.** `analyze` on the command line and
  on the browser page saves the recipe, the card record and the Lightroom
  sidecar together, as the desktop app does. Over pixels an AI generated, it
  no longer writes a sidecar that cannot reproduce them, and the command line
  prints `xmp    -> none` with the reason.
- **AI masks from a Lightroom sidecar on the browser page.** The browser
  page now draws them before it renders, as the command line does.

### Python

- **Folders with non-English names.** The two denoise helpers could not read
  or write an image whose path has characters outside English (a Chinese
  folder or user name, for example); they now can, as the face and depth
  helpers already could.
- **One AI model at a time.** The subject finder and the two denoisers now
  wait their turn like the other AI helpers, so at most one AI model is in
  memory at a time.

## The reverse fit: the colour-cast handover

- **A steadier last check.** At its end the fit checks that its result is
  no farther from the target than doing nothing, and while it is farther, it
  halves saturation and the colour mixer and fits the colour cast again.
  Each of those fits could hand the cast over to the white balance, and the
  handover set saturation afresh, undoing the halving, so a round could take
  back its own step and nothing bounded the rounds. The handover in this
  step now keeps the lowered saturation, and the step stops after sixteen
  rounds at most, with a warning line when it does. Above the default
  **Strength**, the white balance a handover sets now has to pass the two
  checks the white-balance step itself applies there: it may not bring in
  colours the target does not hold, and it may change the colour of no more
  of the photo than that step allows. Both problems were found by reading
  the code, not seen on a photo. At the default strength the new
  white-balance check does not run, and a fit that never hands a cast over
  gives the same recipe as in v1.14.0. The neon night street is the one
  reference pair whose cast goes through the handover, so it is the only
  pair expected to move.

Reference-pair gate (the reverse fit changed, so the calibration pairs were
re-fitted): the seven calibration pairs with a target — the reference pair at
strengths 0.65, 0.85 and 1.0, the six others at 0.65 and 0.85 — were fitted
with `match --zoned` by the installed v1.14.0 command line and by this
release's build, from the same photos in fresh develop stores. Every one of
the fifteen recipes is identical to v1.14.0's, and every pair of 1000-px
renders differs by a mean of 0.00 codes with a largest single-pixel
difference of 1 code (the PNG bytes differ only in what the encoder writes
around the pixels). The three-way sheets (target, v1.14.0, this build) were
viewed for each pair; no pair moved, the one that reaches the handover
included.

## Compatibility

- **Recipes.** The recipe format is unchanged: this version adds no field,
  so v1.14.0 opens every recipe this version saves. The v1.14.0 rule for
  older builds still holds: a recipe that uses a portrait mask or a
  protected background blur cannot be read by v1.13.0 or older.
- **Paths in recipes and sidecars.** A recipe or a Lightroom sidecar that
  names a network path, or a path that climbs out of the develop folder, for
  a depth map, a keep-sharp mask or a pixel layer no longer has it followed
  when opened, the way a mask path with the same problem already was.
  Sidecars written by this version carry bare file names for those three.
- **Cached masks.** The cached face masks are drawn again once, on the
  first face search after the update, because a RAW's masks are now drawn on
  its full frame; a RAW's cached AI masks are drawn again once for the same
  reason. A jpg's cached AI masks are kept.
- **Healed pixels.** A heal from **🤖 Remove blemishes** is a pixel edit,
  saved and linked like every other heal; a Lightroom sidecar does not carry
  it.
- **Command line and browser page.** Their commands, flags and routes are
  those of v1.14.0, and their heal works as before: only the desktop app's
  **🤖 Remove blemishes** holds a heal to the face. Without that limit a heal
  gives the same bytes as in v1.14.0. Writing to the default `./out` while
  running from inside a photo folder is now refused, as above.
- **Python.** `python/_mirror.py` gains the face model's row, and
  `python/denoise.py` and `python/denoise_raw.py` read and write images in a
  way that works for any path; every other helper script is byte for byte
  v1.14.0's, and the denoiser weights stay pinned to the v1.6.0
  `autoshade-raw-denoise-v2.pth`.
- **Settings.** The preferences file is unchanged.

## Gates

Before the frozen snapshot: on `0194a95`, the desktop app **285 passed / 0
failed / 4 skipped**; the library tests of each area this release changed,
run area by area, 0 failed; clippy 0 warnings on both feature sets; the
Python suite under `python/` 100 tests, all passing; and the i18n audit 0
findings, the font check (923 of 924 code points embedded, one left to
egui's bundle) and `check_doc_drift` 0 findings over 14 documents.

The release battery (`scripts/release_battery.sh`: the library, the desktop
app and command line, and the calibration run, on a frozen copy of the release
code at `a89a9d6`, with the calibration photos and the denoiser weights in
reach): library **1732 passed / 0 failed / 15 ignored** (850.53 s), CLI
**25 / 0**, contract 2 + 2, doc-tests 0, GUI **285 / 0 / 4**, calibration run
**1732 / 0 / 15** (1007.70 s; one test that needs a mask-brush sample set said
so and passed without it, as it does on every battery run without that set);
`audit_i18n` 0 findings, the font check (923 of 924 code points embedded, one
left to egui's bundle) and `check_doc_drift` (0 findings over 14 documents);
clippy 0 warnings on both feature sets; the Python suite under `python/` 100
tests, all passing; `check_docs.py --gates` 28 PASS / 4 FAIL on the snapshot —
the four test-count claims, still at v1.14.0's figures — and **32 PASS / 0
FAIL / 0 SKIP** once the counts were moved. By name against the v1.14.0
release battery: library 1726 → 1747, **+21 / −0**; GUI 276 → 289, **+13 /
−0**; CLI 25. The first battery run of this release, on the commit before the
last, failed one library test in both lanes: the new model-slot test left a
temporary file in the shared folder for the 300 ms it held the slot, and an
older test counting those files ran beside it; the two tests now take turns
(a test-module change only), and the battery above is the re-run.

Final check, portraits: this release adds no new real portrait sample to the
final check. What **🤖 Remove blemishes** does is pinned by tests instead: a
synthetic frame with two defects, one inside the limit mask and one outside,
where the one inside is healed and the one outside keeps every pixel, and
without a limit the heal gives the same bytes as before; and desktop-app
tests with a stand-in for the AI heal, which check that the button is greyed
without a face, that the heal is held to the face mask, what the status line
says, that `Ctrl+Z` goes back, and that no spot on the face is reported as an
answer, not a failure.
