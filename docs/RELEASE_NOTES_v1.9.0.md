# AutoShade v1.9.0 — a reference library of finished photos, an AI lens blur, and a calmer desktop app

Everything merged into `main` since v1.8.1 ships here. The style index —
RAW files and their Lightroom sidecars, blended into a recipe — is replaced by
a reference library of finished photographs that the AI is shown beside the
photo it develops. An analysis now follows either that library or the
direction you type, picked by a switch. A new AI lens blur reads the depth of
the photo and softens what lies in front of and behind the point you choose.
The desktop app gains colour themes, one consistent icon set, motion, and a
side panel with far less standing text. One older bug is fixed: an AI refine
dropped a card's imported spot removals and fill layers.

The design, the measurements on the maintainer's own photographs, the code and
this document are the maintainer's; no external coding model wrote this
release.

## The reference library

- **What it is.** Pick a folder of finished photographs whose look you like —
  JPEG, TIFF or PNG exports, yours or anyone's — in **AI › Reference library**,
  or run `autoshade library-build <dir>`. Each photo is measured from its
  pixels by a local image model (SigLIP 2); with **Write look phrases** (or
  `--describe`) a second local model writes a few short phrases about its
  grade ("warm, faded, low contrast"). Nothing leaves the machine, and a
  rebuild measures only new or changed photos.
- **How a develop uses it.** The three photos whose scene is closest to the
  one being developed go to the AI as a fenced block of their phrases and, if
  you opt in, as pictures after the photo itself. Versions of one shot — an
  export and its re-edit — take one place, not three. Nothing is blended into
  the recipe: the AI proposes its own sliders and the usual review runs.
  `autoshade library-search <photo>` shows which references a develop would
  get, with no paid call.
- **Library or direction — one or the other.** An analysis is guided EITHER by
  the library OR by the direction you type, never both: with the library the
  direction text is not sent, with a direction the library is not read. The
  desktop app's **Guided by** switch picks one and greys the other; the command
  line takes `--guide library|direction` on `analyze` and `auto`, the browser
  page a Guide menu. Left unsaid, a typed direction picks Direction.
- **The photographer's own settings, when they can be trusted.** Name the
  folder your RAWs live in (**RAW folder…**, or `--raw-root DIR`). On the next
  build each finished photo's RAW is found by its file name, the RAW's `.xmp`
  is developed here, and only when that develop looks like the finished photo
  — a mean colour difference (ΔE) of 11 or less — do its settings go to the AI
  as one line under that reference. A sidecar that develops to a different
  picture — an older grade, or one reworked later in Photoshop — is ignored.
  On the maintainer's album, 63 finished photos had a RAW with a sidecar: every
  pair up to 10.7 was the same picture by eye, every pair from 11.4 up a
  different version, and 17 matched.

## AI lens blur

- In the **Lens Blur** section, **Estimate depth** runs a local depth model
  (MoGe-2, MIT licence; downloaded once from its upstream or this project's
  mirror and checked against a pinned SHA-256), then a click on the photo
  chooses what stays sharp. **Amount** and **Focus range** shape the blur.
- The blur is the develop's last step, in linear light, scaled to the frame's
  long edge so the preview and the export agree. A recipe without a lens blur
  renders exactly as before. Lightroom has no equivalent field, so the blur is
  not written to the `.xmp` (the export disclosure says so); it is not pasted
  onto another photo, and a card with a blur cannot be turned.

## The desktop app

- **Colour themes.** Four surface schemes (dark grey, light, warm ink, cool
  blue) and five accents (gold, teal, coral, violet, green) in Settings; every
  pair is tested against the contrast bars.
- **One icon set.** Every icon-like symbol in the interface is drawn from
  Lucide (ISC licence), embedded as a small font, so the toolbar and panels no
  longer mix five typefaces' hands. Icon-only buttons have no frame until the
  pointer is on them; text buttons drop their outline.
- **Motion.** Folds open and close smoothly, a message slides in from the
  right, the before/after compare cross-fades, a different photo or version
  fades in, and a fold's header rises under the pointer.
- **Less standing text.** Fourteen how-it-works paragraphs that stood open in
  the side panel are one quiet **ⓘ How this works** line each now, the
  paragraph its tooltip; status lines and warnings stay as they were.
- **And more.** The side panel keeps a fixed width; the window draws its own
  title bar; sliders sit on two lines, label over track, as in Lightroom; the
  brush inks on press, shows its size where it will land and has feather and
  flow; colour grading shows three wheels; the AI analysis sits in a bounded
  box with a full-text window.

## Fixed

- **An AI refine dropped work it could not express.** A refine replaced the
  card with the AI's new recipe, and the imported spot removals and generated
  fill layers the AI never sees were lost with it. They are carried over now,
  with the lens blur, and a test walks the whole control registry so a future
  engine-only field cannot be forgotten.

## Compatibility

- **Command line.** `style-index` and `style-query` are gone;
  `library-build <dir> [--describe] [--raw-root DIR]` and
  `library-search <photo>` replace them. `analyze` and `auto` gain
  `--guide library|direction`.
- **Browser page.** `/api/style-info` and `/api/style-build` are replaced by
  `/api/library-info` and `/api/library-build`.
- **The old style index is not read.** Build a reference library from a folder
  of finished photos once; until then an analysis says it has no references.
- **Recipes.** A recipe gains an optional `lens_blur`; one without it is
  written and rendered as in v1.8.1. Recipes and stores from v1.8.1 and
  earlier open unchanged. The sidecar weights stay pinned to the v1.6.0
  `autoshade-raw-denoise-v2.pth`; no weights are re-shipped.

## Gates

Measured before the tag on the release code (`d7198be`). The version bump
touches Cargo.toml, Cargo.lock, the documents, the site's cache keys and the
bug template's dropdown, and the CLI, contract and doc-test suites, clippy,
the `python/` suite and `check_docs` are re-run after it. The three-lane
release battery (`scripts/release_battery.sh`, a frozen snapshot worktree, the
p36–p41 calibration corpus and the sidecar weights in reach): library
**1628 passed / 0 failed / 15 ignored** (698.44 s, release profile, one
process per module), CLI **25 / 0**, contract 2 + 2, doc-tests 0, GUI
**240 / 0 / 3**, calibration lane **1628 / 0 / 15** (1048.99 s, nothing
skipped); `audit_i18n`, the font check and `check_doc_drift` exit 0 inside the
battery; the Python suites 84 OK from `python/` and 44 OK from `scripts/`
(CPU, the real weights, `-W error::RuntimeWarning`). clippy 0 warnings on both
feature sets (`--all-targets -- -D warnings`). `check_docs.py --gates` on the
transcript with the XMP census root supplied: **32 PASS / 0 FAIL / 0 SKIP**.

By name against the v1.8.0 release battery (v1.8.1 shipped a dependency update
without re-running it): library 1777 → 1643, **+26 / −160** — 143 names
retired with the modules the reference library replaced (style 106,
mask_habit 29, style_cache 5, content_cache 3) and 17 with the style index's
callers; the 26 added are the reference library's 11, the lens blur's 7, the
pipeline's 4 (the either/or guide among them) and four elsewhere. GUI 230 →
243, **+20 / −7** (seven renamed or replaced with the reference library, the
colour themes and the panel width). CLI 28 → 25 (the four `style-query` names
retired with the command, one for `library-search`).

The first battery run, on `e7468df`, failed one library test in both library
lanes: the source guard that every temp-directory name carries the process id
read a never-created photo path in the reference library's tests. The path
carries the process id now (`d7198be`) and the battery above is the second run
on that commit.

The push of the bump failed CI's doc-drift job: two documents still named the
retired style index's source files. The same check had passed on the
maintainer's Windows checkout, because Git for Windows reads a blank line of a
CRLF `.gitignore` as a pattern that ignores every `dir/` spelling, and the
check asks about each path both ways; it now drops a match with an empty
pattern, and turned red on the same documents locally before they were fixed.
The sweep that followed rewrote what still described the style index as
current: the manual's command-line chapter (its `.xmp` folder rule now names
only `eval`, and the four retrieval-weight variables are gone from the table),
the install note, a README results row and the architecture diagram's
reference library box.

**A recipe without a lens blur renders as in v1.8.1.** One RAW and one recipe
(exposure, contrast, highlights, shadows, vibrance, temperature, clarity) were
developed at a 1600 px long edge by v1.8.1 and by the release code, both built
on this machine with the same toolchain: **0 differing pixels**. (Against the
CI-built v1.8.1 installer, 91 pixels differed by one 8-bit level — the known
difference between a CI build and a local build of the same code.)

Final gate, reference pair: not re-run. The reverse fit is unchanged, and the
develop of a recipe that carries no lens blur is identical (above); the
maintainer ruled the gate unnecessary for code the batch does not touch.

Mutations falsified the new invariants on the release code, each red and
each restored (SHA-256 equal): a RAW name followed by a digit accepted as the
finished photo's RAW turned
`a_finished_photo_names_its_raw_by_prefix_but_not_a_longer_number` red; an
explicit library guide accepted beside a typed direction turned
`a_develop_is_guided_by_the_library_or_the_direction_never_both` red; the
same-shot line raised past any cosine turned
`versions_of_one_shot_take_one_reference_slot` red; a different photo landing
without a fade turned `a_swap_fades_in_and_a_rerender_does_not` red; a fold
header that never lifts turned
`a_fold_header_rises_under_the_pointer_and_settles_back` red.
