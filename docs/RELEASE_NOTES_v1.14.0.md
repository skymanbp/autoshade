# AutoShade v1.14.0 — portrait tools, and a Normal mode that numbers its saves

Everything merged into `main` since v1.13.0 ships here. **Portraits** is no
longer the Landscape workspace with two switches folded away: it keeps the
whole workspace and adds a **Portraits** panel of its own at the top of the
side panel, in four groups — people and background, skin, eyes, and the
background blur. Faces and eyes are found on this computer by Google's
MediaPipe face finder, and the panel's sliders work on four new masks: the
face, the eyes, the skin and the background. **Normal** no longer refuses a
second save of the same photo: it gives the new file a number. Its
**AI denoise** switch is now the workspace's own AI denoise, shown in the
preview before you save.

The design, the code and this document are the maintainer's; no external
coding model wrote this release.

## Portraits mode

- **The whole workspace, one panel more.** Every control of Landscape is
  still there; the Portraits panel sits above them in four groups, top to
  bottom. The reverse fit's two local-area switches stay in their **Local
  areas** fold, closed until you open it.
- **People and background.** **🤖 Find people and faces** finds the person
  the way **🤖 Select subject** does, and every face and its eyes with
  MediaPipe, both on this computer; the line under the button says how many
  faces were found. The search adds four masks to the photo — **Face**,
  **Eyes**, **Skin** (the face, narrowed to the colour of its skin) and
  **Background** (everything but the person). They appear in the mask list
  too, where every other mask control works on them, and searching again
  moves them onto the new result and keeps their sliders. **✨ AI portrait
  look** asks the AI for a portrait look — softer skin, brighter eyes, a
  calmer background — at the **Look strength** you set; it needs an image key
  in Settings, and `Ctrl+Z` takes it back. The faces, eyes and background the
  AI asks for are found on this computer the same way, never from
  coordinates the AI makes up.
- **Skin.** **Soften skin**, **Skin warmth** and **Skin colour intensity**
  each work on the skin only. **Keep skin natural** is on by default. While
  it is on, skin colour can be made only a little richer (up to +15), warmer
  or cooler (up to 20 either way) or shifted in tone (up to 8°); softening
  stops before skin looks fake (fine texture no lower than −60, local
  contrast no lower than −40); and skin is never sharpened. The limits hold
  on the face and skin masks, for your own changes and the AI's alike; turn
  the switch off to go further.
- **Opening a photo never changes its edit.** The switch is decided card by
  card: a card whose saved edit already goes past the limits opens with
  **Keep skin natural** off, and nothing moves until you turn it on — then
  the values come back within the limits and the status line says how many
  moved. When no face was found, the skin and eyes groups are greyed and say
  **No face found in this photo**.
- **Eyes.** **Brighten eyes** lifts the eyes by up to 1.5 stops; **Crisp
  eyes** brings out the detail in them.
- **Background.** **🤖 Prepare the background blur** measures how far away
  every part of the photo is, the way the Develop panel's Lens Blur does (the
  first run downloads a 1.3 GB model). Once a face and the person were found,
  it sets the focus on the face and turns on **Keep the person sharp**, so
  the whole person stays sharp however far the blur reaches. **Blur amount**
  and **Sharp range** set how strong the blur is and how deep the sharp part
  is; **Darken background** and **Fade background colour** work on
  everything but the person and need only the person, not a face. Like the
  Lens Blur, the blur is drawn in the app and in every export from it, and a
  Lightroom sidecar cannot carry it.
- **What the face finder needs.** MediaPipe is a Python package of its own:
  install it once with `pip install mediapipe` into the Python the app uses
  (Settings, **Python interpreter**). The first search downloads its face
  model (3,758,596 bytes, about 3.8 MB), checked against a pinned SHA-256
  like every other model. Unlike the other models, this one has **no copy of
  ours yet**: it is downloaded only from Google's own server, at a fixed
  versioned address, so the face search needs that server to be reachable
  the first time.
- **What it cannot do yet.** The face finder is MediaPipe's short-range
  detector, made for faces that fill a good part of the frame: small faces in
  a wide shot or a distant group can be missed, and the app then says that no
  face was found and leaves the photo alone. A blemish remover limited to the
  face is not in this version, because the healing tool cannot yet be limited
  to a mask.

## Normal mode

- **A second save gets a number.** When the folder already holds the jpg the
  app would write, the new one is saved beside it with a number —
  `name.developed-2.jpg`, then `-3` — instead of being refused, and the line
  under the button names the file written. This holds for **Save as jpg** and
  for **Apply this look to every photo in the folder** alike, and two saves
  at the same moment can never take the same name.
- **Only for names the app made.** A file you pick yourself in the save
  window is yours: if you confirmed replacing it there, it is replaced. The
  full workspace's Export keeps its own rule.
- **Never into the photo's folder.** Saving into the folder the photo itself
  is in is still refused, in plain words, and you pick another folder; the
  original is never changed.
- **AI denoise you can see.** The **AI denoise** switch now runs the same AI
  denoise as the workspace's Retouch panel — the same model, strength and
  checks. It takes a moment; then the preview shows the cleaned photo, and
  **Save as jpg** saves what is shown. Off goes back to the photo without it,
  on again brings the cleaned one back without a second run, and your slider
  changes stay either way. In v1.13.0 the denoise ran only when saving and
  the preview did not show it.
- **The unsaved marker follows the picture on screen.** Turning the denoise
  on or off changes the picture, so the status bar says **unsaved changes**
  until that picture is saved.

## Compatibility

- **Recipes.** Recipes from v1.13.0 open and render as before, and a recipe
  without a portrait mask or a protected blur renders pixel for pixel as in
  v1.13.0. A recipe that uses one of the four portrait masks, or a background
  blur with **Keep the person sharp** on, cannot be read by v1.13.0 or older:
  those builds refuse the whole recipe and name what they do not know, rather
  than open it with the masks turned into ordinary ones or the person
  blurred. Open such a recipe with v1.14.0.
- **A protect mask that cannot be read.** If the mask that keeps the person
  sharp is missing, the preview draws no blur and the export is refused by
  name, rather than blurring the person.
- **Lightroom sidecars.** The portrait masks are pixel masks, which an XMP
  sidecar does not carry: the save line names them, as it does every pixel
  mask. A sidecar mask whose role this build does not know reads as an
  ordinary mask.
- **Command line and browser page.** Their commands, flags and routes are
  those of v1.13.0. When a recipe from the AI asks for the face or the eyes,
  `apply` and `auto` outline them on this computer and print one
  `face mask:` line; a mask with no face to stand on is left out and named.
- **Python.** `python/face.py` (the face finder) is new, and `mediapipe` is
  listed last in `python/requirements-common.txt`, to be installed after the
  rest. MediaPipe brings its own, fuller build of OpenCV beside the
  `opencv-python` the other sidecars use; both provide the same `cv2`
  module. The face finder's script can be named with `AUTOSHADE_FACE_SCRIPT`,
  like the other sidecars. The other sidecars are byte for byte v1.13.0's,
  and the denoiser weights stay pinned to the v1.6.0
  `autoshade-raw-denoise-v2.pth`.
- **Settings.** The preferences file is unchanged; **Keep skin natural** is
  decided per photo and card, not stored as a setting.

## Gates

Before the frozen snapshot: on `2f57492`, with the calibration photos in
reach, library **1710 passed / 0 failed / 15 skipped**, twelve Python tests
on the face finder's geometry with stub data, clippy 0 warnings on both
feature sets, and the i18n audit, the font check and `check_doc_drift` 0
findings; on `ed9a991`, the desktop app **272 passed / 0 failed / 4
skipped**, the library tests that commit touched, clippy 0 warnings on both
feature sets, and the i18n audit, the font check and `check_doc_drift` 0
findings.

The release battery (`scripts/release_battery.sh`: the library, the desktop
app and command line, and the calibration run, on a frozen copy of the release
code at `ed9a991`, with the calibration photos and the denoiser weights in
reach): library **1711 passed / 0 failed / 15 ignored** (914.76 s), CLI
**25 / 0**, contract 2 + 2, doc-tests 0, GUI **272 / 0 / 4**, calibration run
**1711 / 0 / 15** (1081.35 s; one test that needs a mask-brush sample set
said so and passed without it, as it does on every battery run without that
set); `audit_i18n` 0 findings, the font check (923 of 924 code points
embedded, one left to egui's bundle) and `check_doc_drift` (0 findings over
14 documents); clippy 0 warnings on both feature sets; the Python suite under
`python/` 96 tests, all passing, the face finder's twelve among them;
`check_docs.py --gates` 28 PASS / 4 FAIL on the snapshot — the four
test-count claims, still at v1.13.0's figures — and **32 PASS / 0 FAIL / 0
SKIP** once the counts were moved. By name against the v1.13.0 release
battery: library 1702 → 1726, **+24 / −0**; GUI 265 → 276, **+13 / −2** (two
Normal-mode names replaced by the ones that describe the new behaviour); CLI
25.

After the push, CI's macOS test job failed one test: the lens blur's
"draws the pixels it drew before protection existed" check hashed its
synthetic render bit for bit, and f32 results differ between platforms.
`78378ec` rewrites that test so every platform checks that a render with no
protect mask equals, bit for bit, the render with an all-black mask, and
keeps the historical hash only on Windows x86_64, where it was taken; the
release is tagged on that commit, with CI green on all three platforms.

Final check, portraits: three public-domain (CC0) portraits and one backlit
profile with no face the finder can see. On the three portraits, with
**Keep the person sharp** on, the person stays sharp while the background
blurs; on the backlit profile the app says that no face was found and
changes nothing. No file of the reverse fit changed in this release, so the
reference pairs of v1.13.0's final check were not part of this one.
