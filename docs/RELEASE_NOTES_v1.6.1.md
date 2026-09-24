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
  and the accepted zone records those same weights.
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
  may differ where the joint veto or the rescore base now acts. Recipes,
  sidecars, the store and the library API keep their formats.
- **Downloads.** None new; the weights are the v1.6.0 release's.
- **HTTP.** The `X-Heal-Skipped` header is no longer sent; the count is in the
  `X-Heal-Rationale` text.
