# AutoShade v1.6.1 — the audit's findings closed: the reverse fit's terminal veto and rescore base, the Upright row, the vignette's half pixel, orientation-only sidecars read in the photograph's frame, and the smaller fixes

Everything merged into `main` since v1.6.0 ships here. A read-only audit of the
whole tree on 2026-09-24 filed what it found as issues #6–#14 and a hand-over
checklist (#15); this release closes them. Every finding is either fixed and
pinned by a test of its own, or recorded below with the reason it stays as it
is. Nothing new is added — this is a patch release — and every change below
carries the measurement or the pin that decided it.

The design, the runs, the acceptance and this document are the maintainer's;
no external coding model wrote this release.

## The reverse fit

- **The end-of-pipeline veto sees its joint readings.** `fit::terminal_harm`
  has carried a joint hue/chroma veto since it was written, and both call
  sites handed it `None, None` for those readings, so the joint veto existed on
  paper only. Both sites now pass what they measured.
- **A rescored report measures against the base the solve used.** A recipe
  adjusted after the solve (the CLI's `--deep`, the GUI's judge loop) had its
  divergence and terminal harm measured against a neutral recipe; it is now
  measured against the calibration base the solve itself measured against.
- **The Atmosphere route's hue veto is recomputed on the recipe it ships**, not
  on the one it measured three stages earlier, and only where no global cast
  was attached.
- **An HSL move is not a detail-only companion.**
- **A zone's after-reading is taken on the source weights it was solved on**,
  and the accepted zone records those same weights as its populations. Its
  boundary ruler and its share keep reading the mask's own raster: handed the
  robust-composed population instead, the ruler took that population's holes
  for transitions and refused the calibration corpus's third semantic region
  — the four-region calibration test caught it before the tag. On the
  reference pair at 0.85 the one population also decides the do-no-harm
  re-judgement after a boundary shrink: v1.6.0 read the shipped correction on
  the mask's raster against a baseline read on the solver's population, and
  that mixed comparison refused two spatial tiles at k = 0.073 and 0.009 and
  a colour-field zone at k = 0.307 that the consistent reading keeps. On its
  own this moves the render by 0.15 codes mean (6 at most): the installed
  v1.6.0 re-fitted the pair beside the build as it stood before the layout
  admission below, to measure it.
- **The luma-only tone ladder probes the fitted tone first** (factor 1.0)
  before backing off through 0.75, 0.5 and 0.25. On the calibration corpus
  this is the one correction of the eight that moves a pinned number: the sky's
  luma-only band passes the local quality gate at its full step and ships
  −0.174 EV where it used to ship three quarters of its fit (−0.152 EV). The
  partition re-arbitrated around that step (the second band −0.062 → −0.007 EV
  at saturation +2.2, the land bands' saturation +2.4 → +1.6); the sky bands'
  colour distance to the target after the fit went 21.41 → 21.24, the land
  bands' 5.91 → 6.08, and the frame-wide residual is 0.095 either way. Reverting
  each correction alone in a copy of the tree attributes every moved number to
  the ladder; the after-reading correction moves one land gain by one unit in
  the last place.
- **The colour field's two do-no-harm checks read the rounded field the render
  applies**, over every attached mask with coverage — a custom region or a free
  mask included, where they used to read the sky and land roles only — and a
  stage whose solver finds nothing to fit says so instead of attaching nothing
  in silence.
- **A zone whose colour was withheld stays exactly neutral when the boundary
  gate shrinks the set.** The shrink's explicit common/differential form
  added the unity offset before the differential, so a withheld channel came
  out one unit in the last place under unity once the gate bisected; the
  differential is now summed first and cancels exactly. Found by CI on the
  close-out: with the mask's own raster back under the boundary ruler and the
  ladder's full step, a fixture that had always passed the gate at full
  strength is now bisected.
- **The colour field reads a paired region's cells on the pairing where the
  pixel evidence cannot read them at all.** The user's eyes at the final gate
  found a grey-blue block at the top centre of the reference pair's sky, where
  the target's purple deepens — identical in v1.6.0. Measured with a probe of
  the frozen evidence: the structural instrument reads that featureless ninth
  of the frame as "texture gone" (the source's noise against the target's
  smooth re-synthesis, D 1.21), so every pixel there carried zero evidence
  weight, eight of the colour field's 96 cells could be neither read nor
  solved, and the sky's single zone gain left the centre 7–12 codes too blue
  while landing the corners. A class the segmenter found in the same place on
  both frames is the same scene in the same place whatever its texture did,
  which is all a cell statistic needs: inside such a pairing the field's
  support-free solve now reads every pixel as population evidence, and the
  cells the pixel evidence could not read are read on the pairing — against
  the same target means — and take the solve on the same cell-mean verdict the
  measured cells take it on. Every pair without such cells is byte-identical;
  a new rationale sentence counts them where they exist.

What these did on the reference pair is in the gates below.

## Rendering

- **Upright Vertical and Full square a tilted photo fully.** The keystone row
  runs after the levelling turn, so it must send the vanishing points to
  infinity where the turn has put them; it was built from the unturned points,
  and a levelled solve kept a residual convergence of the turn's own angle. On
  a 3:2 chart turned 6° and keystoned 0.35, the residual after Full is now
  under 3 % of the input's, and the turn comes back level to 0.5°.
- **The post-crop vignette is sampled at the pixel centre**, like every other
  spatial read in the engine; it was sampled at the corner, half a pixel
  up-left of the rectangle it belongs to.
- **Transform Scale is Lightroom's band, 50–150.** The recipe accepted 0–200,
  and at 0 the transform map collapsed to a point, had no inverse, and the
  whole Transform step — the other sliders and the Upright matrix with it —
  silently dropped out of the render. The GUI slider follows the band.
- **An inverted mask component whose raster is missing contributes nothing.**
  It used to contribute the whole frame: an inverted Add covered everything at
  full strength, an inverted Subtract wiped the base.
- **The Transform frame's aspect is `(w−1)/(h−1)`**, the box the solver and
  the sampler already used, so the Rotate slider is rigid to the pixel rather
  than to one part in the height.
- **Only six Upright matrices are kept** by the recipe's own clamp — six is the
  number a mode can select; a hand-edited file could carry a million.

## Sidecars

- **A sidecar that declares only an orientation is read in the photograph's
  frame.** 154 of the 175 sidecars in the maintainer's library declare
  `tiff:Orientation` and no `tiff:ImageWidth`/`ImageLength`; their geometry was
  folded through the photograph's rectangle on the way out, and the reader
  decoded it with no frame at all, so a rotated radial came back unrotated and
  a "rotation dropped" loss was disclosed that had not happened. The recipe
  reader and the two photo-aware disclosure doors now decode in the same frame.
  Census through both doors on that library: the document-only door reports
  37 rotation losses and 8 unplaceable crops on the 151 orientation-only
  sidecars beside a readable RAW; the photo-aware door reports 0 and 0.
- **The CLI's Lightroom-import line names unreadable numeric settings**, as
  the GUI's open path always has; the note's unreachable "% of one edge" arm is
  gone (a placed crop with an overshoot always has a frame to measure in).
- **One raster loss per mask in the sidecar payload.** A mask naming the same
  missing raster through two geometries recorded the same loss twice.

## The advisor

- **"Too weak" is a push, not a pull-back.** The judge's direction list
  carried a bare "too ", so "the saturation is too weak / too flat / too timid"
  read as a request for less; only the over-reach phrases (too much, too
  strong, too saturated, …) now pull back. "Local contrast" and "the shadow
  areas" no longer buy a zoned re-solve — they are tonal remarks, not a part of
  the frame.
- **A chat-completions refusal is named as a refusal**, not as "no content in
  the reply".

## The style index

- **The Style pull is continuous.** It jumped from 0.294 to 0.500 between Style
  0.49 and 0.50 — a slider tick that moved the pull by 0.2. The three points
  anything documents (0.3 → 0.18, 0.5 → 0.5, 1.0 → 1.0) stay exactly where they
  were, and nothing at or above 0.5 moves.
- **A merge across two embedding models drops the other model's vectors** and
  says which model wrote what the file keeps. A looks build saved over RAW
  vectors another model had embedded kept its own stamp over them, and a query
  vector compared against them was a number, not a similarity.
- **A RAW exemplar's tags meet the same bound as a look's at the door** (four
  phrases of at most 128 characters), truncated rather than refused.
- **A link whose target is gone is stepped over** in a look folder; it used to
  fail the first decode and the whole build with it.

## Store, decode and the GUI

- **The gallery's "Import legacy" honours the explicit-clear tombstone** the
  per-photo path honours: a cleared develop stays cleared.
- **A pristine AI card with no pixel record hands its identity and name to the
  fresh master card** instead of losing them — the version snapshots taken from
  it keep something to point at.
- **A crashed-adoption resume reports only its own work**; one beaten to it by
  a concurrent resume used to claim the other's.
- **A TIFF profile the reader cannot read is a hard error, not an untagged
  file.** The re-probe went through `image`'s TIFF decoder, whose profile
  read folds every error into "no profile", so a profile tag of the wrong type
  opened the file as sRGB with its profile ignored — the exact fall-through the
  ICC rule forbids. The tag is now read through the tiff crate itself; only a
  missing tag is untagged.
- **AI mask refine refuses a turned photo**, with the same gate and the same
  sentence as the seven pixel doors: its guide is decoded in the EXIF frame
  while the raster it refines lives in the turned frame.
- **Settings saved under the shared temp folder say so.** On a machine with no
  per-user data directory the settings file is not trusted with a key or an
  endpoint, and the GUI said "saved" while the loader ignored those fields on
  stderr; the status line now says which fields will be ignored and what to
  set.
- **The variant strip's restore-time clamp losses are disclosed** in the same
  toast the active card's are.
- **Heal's "left untouched" count is one typed note in the report.** The CLI
  prints it, the GUI renders it localized, the web page reads it through the
  rationale header; the `X-Heal-Skipped` header and two hand-written sentences
  were three channels for one fact.

## Build and scripts

- **Third-party actions are pinned to commits** in every workflow:
  `dtolnay/rust-toolchain` at `02cb101` (master, 2026-09-12, with
  `toolchain: stable` as its required input — the repo's own
  `rust-toolchain.toml` still selects the compiler, and every job still asserts
  it) and `Swatinem/rust-cache` at `6323deb` (v2.9.2). Inno Setup is installed
  at 6.7.1, the Metal probe's torch at 2.14.0 / torchvision 0.29.0, and the
  release notes the workflow writes say where the denoise weights come from.
- **`installer_scenarios.ps1`** drops a dead pre-1.2.4 relaxation (whose
  `[version]` parse threw on a suffixed version) and asserts the installer's
  excludes as absences: no `test_*.py`, `.pyc`, `__pycache__` or weights in the
  installed tree.
- **`deploy_site.js`** pins wrangler at 4.138.0, quotes its arguments on
  Windows (a staging directory under a `%TEMP%` with a space broke the deploy),
  and no longer lets a failed token delete replace the deploy's own error.
- **`train_raw.py --resume`** restores the run's history and best score (the
  first validation after a resume always overwrote `best_state.pth`);
  `grid_experiment.py` reads its probe binary from `FITGRID_PROBE` (the default
  named a build that no longer exists); `lr_mask_parity.py` reports α residuals
  below its 0.005 floor as "at the floor", as its header promised; unused
  parameters and constants are gone from five scripts, and two file handles
  are closed.

## Kept, with the reason

- **A flat side of the structure-divergence reading keeps its correlation of
  1.0.** The audit read it as a manufactured match; it is the value that
  switches the correlation term off. When no translation offers gradient
  variance on both sides, the reading is the band-energy ratio alone, and that
  term is the whole structural evidence such a pair offers: a uniform patch
  that stayed uniform reads D = 0, a checkerboard that became flat reads far
  past the divergence line. The abstention was tried and withdrawn on the day:
  the tile stage's own tests attach to uniform patches through that term, and
  an abstention would have let a flattened target through the mode gate as
  "not divergent". Now written down at the function and pinned.
- **Camera-profile samples above white** stay as they are: pinned by
  `the_tone_lookup_passes_white_through_and_starts_where_the_curve_does`.
- **The error-path sanitiser still collapses URLs** ("https://host/v1/…" →
  "https:…"): a URL may embed credentials, and collapsing is the safe default.
- **A creative look's non-finite amount is inert at the recipe door** while an
  absent amount imports as 1.0 at the sidecar door: the recipe rule is
  "corrupt → inert", and JSON cannot carry NaN, so the two doors never see the
  same input.
- **No `autoshade.local.example.json`**: the code's defaults are the deleted
  file's values, so a template would document nothing.
- **The judge's "lower" keeps its disclosed residual**: "the lower half is too
  weak" still reads as a pull-back, and a phrase list cannot tell it from
  "lower the saturation".

## Compatibility

- **Pictures.** A Vertical or Full Upright solve on a tilted photo renders with
  its keystone corrected fully; a post-crop vignette moves half a pixel; a
  Style between 0.3 and 0.5 pulls more toward the retrieved target than before
  (at 0.4: 0.24 → 0.34; 0.3, 0.5 and above are unchanged); a Transform scale
  below 50 or above 150 in a saved file is clamped to the band; a reverse fit
  may differ where the joint veto, the rescore base or the tone ladder now
  acts (a zone's luma-only step can ship at its full fit where it used to
  ship three quarters of it). Recipes, sidecars, the store and the library
  API keep their formats.
- **Downloads.** None new; the weights are the v1.6.0 release's.
- **HTTP.** The `X-Heal-Skipped` header is no longer sent; the count is in the
  `X-Heal-Rationale` text.

## Gates

Measured before the tag on the release code (`dd7bb79`; the version bump
touches Cargo.toml, Cargo.lock, the documents, the site's cache keys and the
bug template's dropdown, and the CLI, contract and doc-test suites, clippy,
the Python suites and `check_docs` are re-run after it). The three-lane
release battery (`scripts/release_battery.sh`, a frozen snapshot worktree,
the p36–p41 calibration corpus and the sidecar weights in reach): library
**1745 passed / 0 failed / 15 ignored** (1989.53 s, release profile, one
process per module), CLI **25 / 0**, contract 2 + 2, doc-tests 0, GUI **222 /
0 / 1**, calibration lane **1745 / 0 / 15** (3094.73 s; one skip line, the
mask-brush specimen test whose `AUTOSHADE_MB_SAMPLE_ROOT` specimen is not on
this machine, named in every release since v1.3.2), `audit_i18n` and the font
check exit 0 inside the battery; the Python suites 81 OK from `python/` and 44
OK from `scripts/` (CPU, the real weights, `-W error::RuntimeWarning`). By name
against the v1.6.0 tag (`00d3d09`), listed by the harness on both trees:
library 1683 → 1760 (+78 / −1: the audit sessions' fifty-five and this
close-out's twenty-three, listed by theme in ARCHITECTURE; the one name removed
is the style build's carry-forward of the previous embedding pass, replaced
beside it by the description pass carrying the previous prose forward); GUI
215 → 223 (+8 / −0: the audit sessions' seven and the mask-refine refusal on a
turned photo). clippy 0 warnings on both feature sets (`--all-targets --
-D warnings`). `check_docs.py --gates` on the transcript with the XMP census
root supplied: **32 PASS / 0 FAIL / 0 SKIP**, before the bump and after it.
Photo-name, token-shape and user-path greps on every commit of the release:
0 / 0 / 0.

The calibration lane earned its place in this release: on the close-out
commit before the fix (`a91cf25`) the four-region calibration test read two
semantic regions where the corpus reads three — the boundary ruler had been
handed the robust population instead of the mask's raster, above — while the
default lane, which runs every test but the corpus ones, was green. With the
fix (`436ab2c`) the lane reads three regions again and the run's look error is
unchanged (0.050973 against 0.050972). CI on that fix then caught the one
ulp of colour the boundary shrink manufactured for a withheld channel — the
inverted-raster orchestration test's exact pin, red on ubuntu, macOS and the
debug-asserts job alike and reproduced here — closed by summing the
differential before the unity offset and pinned at the function.

Each correction that touches a pinned number was reverted alone in a copy of
the tree and the calibration tests re-run: the tone ladder moves the sky's
luma-only band (the numbers above), the after-reading correction is inert at
one unit in the last place, and the abstention the audit proposed for a flat
side turned the tile stage's own tests red and was withdrawn (kept, with the
reason, above).

Final gate, reference pair, before the tag: the pre-bump release CLI re-fitted
the reference pair at 0.65 / 0.85 / 1.0 (`match --zoned`) and rendered each at
the target's size. The first round, on the build before the colour field's
layout admission, read sky ΔE / whole-frame mean |diff| of 7.0 / 0.0295,
4.6 / 0.0260 and 4.7 / 0.0263 against the target (v1.6.0: 7.0 / 0.0296,
4.6 / 0.0260, 4.7 / 0.0263) and went to the user, whose eyes are the gate;
they found the grey-blue block at the top centre of the sky — identical in
v1.6.0, re-fitted the same night — and the release waited for the correction
above. With it the same three fits read 6.3 / 0.0283, 3.8 / 0.0248 and
3.8 / 0.0249; in the 8×6 grid of the frame the top row's B−R excess against
the target, −3.6 −7.1 −5.0 +7.1 +12.3 +4.5 −5.3 −4.5 codes left to right on
v1.6.0, reads −3.6 −7.2 −8.7 −8.5 −7.4 −5.9 −5.2 −4.1: a uniform warmth and
no block. This release moves the reverse fit (the joint veto, the rescore
base, the tone ladder, the zone populations, the field's layout admission),
so the numbers are read against v1.6.0's, and the three side-by-sides and
the sky strip (target, v1.6.0, the withdrawn guard, this build) went to the
user.

Not measured: no paid image call was made for this release (the target is the
one every release since v1.3.2 has been measured against); the GUI executable
was not launched; the Upright correction and the vignette's half pixel were
measured on synthetic charts and by the engine's own tests, not on a
photograph against Lightroom's render; the star standard's two red readings
recorded for v1.6.0 stay as recorded there — nothing in this release touches
the denoiser. The ship facts (the release run, the downloaded assets, the
site, the local upgrade) are in the ROADMAP ledger entry.
