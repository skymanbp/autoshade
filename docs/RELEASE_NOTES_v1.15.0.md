# AutoShade v1.15.0 — a blemish remover held to the face, and our own copy of the face model

Everything merged into `main` since v1.14.0 ships here. The **Portraits**
panel's skin group gains **🤖 Remove blemishes**: the AI finds small spots
and blemishes and each is healed from the skin around it, but only the spots
on the face; a spot found anywhere else is left as it was. The face model
that MediaPipe's face finder downloads now has a byte-exact copy of ours,
tried before Google's server, like every other model the app downloads.

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

## Compatibility

- **Recipes.** The recipe format is unchanged: this version adds no field,
  so v1.14.0 opens every recipe this version saves. The v1.14.0 rule for
  older builds still holds: a recipe that uses a portrait mask or a
  protected background blur cannot be read by v1.13.0 or older.
- **Healed pixels.** A heal from **🤖 Remove blemishes** is a pixel edit,
  saved and linked like every other heal; a Lightroom sidecar does not carry
  it.
- **Command line and browser page.** Their commands, flags and routes are
  those of v1.14.0, and their heal works as before: only the desktop app's
  **🤖 Remove blemishes** holds a heal to the face. Without that limit a heal
  gives the same bytes as in v1.14.0.
- **Python.** `python/_mirror.py` gains the face model's row; every sidecar
  script is byte for byte v1.14.0's, and the denoiser weights stay pinned to
  the v1.6.0 `autoshade-raw-denoise-v2.pth`.
- **Settings.** The preferences file is unchanged.

## Gates

Before the frozen snapshot: on `a56c773`, the desktop app **274 passed / 0
failed / 4 skipped**, the library tests this release touched (healing, the
face, the model copies) **59 passed / 0 failed**, clippy 0 warnings on both
feature sets, the Python suite under `python/` 97 tests, all passing (one
new: the face model is tried from our copy first), and the i18n audit 0
findings, the font check (923 of 924 code points embedded, one left to
egui's bundle) and `check_doc_drift` 0 findings over 14 documents.

**[TO BACK-FILL from the release battery transcript]**

Final check, portraits: this release adds no new real portrait sample to the
final check. What **🤖 Remove blemishes** does is pinned by tests instead: a
synthetic frame with two defects, one inside the limit mask and one outside,
where the one inside is healed and the one outside keeps every pixel, and
without a limit the heal gives the same bytes as before; and desktop-app
tests with a stand-in for the AI heal, which check that the button is greyed
without a face, that the heal is held to the face mask, what the status line
says, that `Ctrl+Z` goes back, and that no spot on the face is reported as an
answer, not a failure.
