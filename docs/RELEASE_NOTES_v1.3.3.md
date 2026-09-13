# AutoShade v1.3.3 — a generated image stays as generated, and the projection lives in the develop's own commit

Two changes from one report. On 2026-09-13 a RAW whose active card was
「✨ AI generated」 with a neutral develop opened cooked — +90 saturation,
−1.30 EV, a deep red frame — with no user action. Reading the develop
directory (read-only) told the story: `recipe.json`, `pixels.json` and
`variants.json` were one generation (a neutral recipe, a generated pixel
source, the generated card active), while the XMP projection beside them was
two days older and held the develop of a 「◭ Reverse-fit」 card that had since
been deleted. The open path fell through the neutral recipe into that stale
projection. The user's second sentence — 「AI 生图怎么能在上面继续编辑呢？
肯定是新开变体啊」 — is the second change: an edit made on a generated card
must not become that card.

## The projection is a member of the commit

Through v1.3.2 the XMP projection was written by a separate call after each
develop commit, outside the commit's generation. The quit-time Save-all
skipped that call for a generated card without retiring the file that stood
there; the Analyze landing wrote one even over a generated card. A projection
written for a reverse-fit card therefore outlived both the card (deleted) and
the recipe it projected (replaced by the generated card's neutral one).

`store::DevelopCommit` now has a fourth member, `xmp`, built by
`pipeline::xmp_projection_member`: `Write` for a source develop of a camera
RAW (the bytes `write_xmp` publishes, merged over the Lightroom sidecar beside
the RAW when there is one, else over the previous projection); `Clear` for a
develop that sits on AI-generated pixels or on a baked image, which no sidecar
reproduces; `Keep` when the projection could not be built. It is staged as
`projection.xmp` in the same `.commit/` generation, named in the manifest,
landed after the three JSON members and replayed by `resolve_pending_commit`.
All seven commit sites hand the member over: the desktop app's Ctrl+S, its
quit-time Save-all, the Analyze landing, the paste worker, the reverse-fit
worker, the CLI's `match`, and the web save. No writer publishes a projection
by a separate call any more.

The readers changed with it. All four — the desktop app's restore, the web
`api_recipe`, the batch export's resolver and `store::read_develop_snapshot`
— used to treat a *neutral* `recipe.json` as an absence and continue into the
projection. A neutral recipe is a saved fact (the pristine generated card),
not an absence: a present `recipe.json` ends the read, and the projection is
consulted only when there is no `recipe.json` at all (stores written before
v0.13). The backup gate no longer preserves a projection on a neutral recipe's
behalf. The GUI test that reproduces the report rebuilds the reported
directory shape and fails on the previous `persist.rs` by construction,
restoring exactly `(90.0, −1.3)`.

## A generated card is immutable

The strip's taxonomy grows its fourth kind, 「✎ Edited AI image」 (store word
`edited`), and one rule: a generated card's recipe is neutral by invariant,
and every develop over a generated raster is an edited card of its own.

The rule's action is `AutoShadeApp::fork_edited_card`. When the live recipe on
a generated card stops being neutral, the recipe moves to a new edited card
inserted right after the generated one (same base raster, same origin, a
minted id, no name), the generated card goes back to neutral keeping its
identity and its name, and the selection follows. The canvas, the undo
history, the view and the tools are not touched. It runs every frame between
the side panels and the develop dispatch, and again at every persist boundary
before the strip is read: Ctrl+S, Save as version, the quit-time Save-all,
the navigation stash, the Analyze landing's install, a version load and the
paste's live arm. The in-place retouch landing forks directly, because a heal
edits the pixels however neutral the recipe is: the retouched raster bakes
into the edited card and the generated card keeps its own.

What the surfaces do with the two kinds:

- The strip's axis is now `is_source_based` (Original, Reverse-fit) against
  `on_ai_pixels` (AI generated, Edited AI image). Calibration is stripped, the
  `pixels.json` flag is written and the projection member clears for both
  AI-pixel kinds. `fit_target` — the raster the reverse-fit reads — is still a
  policy about the generated card alone; from an edited card the AI panel's
  empty-state line says which card to select.
- **Ctrl+S no longer refuses a card on AI pixels.** It saves the develop (the
  disk form re-stamped with the RAW's calibration, the canvas kept stripped —
  the Save-all's and the Analyze saver's rule), the strip, the pixel link and a
  clearing projection member, and the status line says so: "recipe saved →
  … (no Lightroom XMP: this card's look sits on AI-generated pixels)".
- Save as version snapshots an edited card and refuses the pristine generated
  one, saying a pristine card has no develop to snapshot. Apply-to-Original
  refuses both kinds, each with its own reason.
- The generated card's label says on hover that the first edit continues on a
  new card; the reimagine button's tooltip stops promising that tweaks land on
  the generated card.

**Records saved by earlier builds.** Every build through v1.3.2 stored an
edit made on the generated card *as* that card. At the door,
`normalize_ai_cards` splits every generated card found carrying edits into
generated + edited, as unsaved work, and says so once by toast; Ctrl+S then
saves the strip in the new shape. An edited card without a generated sibling
is left alone — the user may have deleted it.

**Writers without a live strip.** The web save and the batch paste publish a
develop over a generated master through `ActiveWrite::DevelopOnAiPixels`. The
record's word follows the develop: a neutral develop is the pristine card (or
an edited card back at neutral over its own master); anything else takes the
active slot as `edited`, moves the displaced generated card into the
background with its identity, name and raster, and mints a pristine card for a
master that had none. `store::recorded_pixel_source` (the record-level half of
`read_pixel_source`, which does not ask whether the master still exists) is
what these writers read.

Chinese pairs for the eight new strings; the door toast avoids the one hanzi
the embedded SC subset lacks (`subset_gui_fonts.py --check` 874/874, one
codepoint more than v1.3.2).

## Compatibility

- **Store format.** `variants.json` keeps its v1 shape; `edited` is a new
  value of the existing `kind` word. A v1.3.2 or older build opening a photo
  whose record names an edited card refuses the strip as one "this build does
  not understand" — background variants stay hidden and saving refuses until
  the file is fixed or deleted — while the develop itself (`recipe.json`,
  `pixels.json`) loads as before. Records with no edited card are byte-for-byte
  what earlier builds wrote.
- **Commit manifest.** The `.commit/COMMIT` manifest gains an `xmp` field. A
  manifest written before the field existed replays as `Keep`; an older build
  resolving a v1.3.3 stage ignores the field (the manifest is not
  `deny_unknown_fields`) and leaves the projection as it was, which the readers
  tolerate.
- **Projections.** For a develop on AI pixels the next commit removes the
  develop store's projection instead of leaving a stale one; for a source
  develop the bytes are the ones `write_xmp` always published. Nothing beside
  the RAW is written that was not written before.
- **CLI.** `apply`, `auto` and the batch paste's `write_recipe` + `write_xmp`
  direct paths never went through `commit_develop` and still do not; `match`
  still records the reverse-fit as `fitted`.
- No renderer, solver, recipe schema, sidecar payload or Python sidecar change:
  the reference pair renders to the same bytes (Gates).

## Gates

Measured before the tag on the release code (`d0851ab`; the version bump
touches Cargo.toml, Cargo.lock and the documents only): library **1483
passed / 0 failed / 15 ignored** (1498 enumerated, test profile, 907.61 s),
CLI **24 / 0**, GUI **183 passed / 0 failed / 1 ignored** (the release
battery's GUI lane, release profile), clippy 0 on both feature sets,
`audit_i18n` 0 / 0 / 0, `subset_gui_fonts.py --check` 874/874, `cargo metadata
--locked` clean, `check_docs.py` 26 PASS / 0 FAIL / 4 SKIP (the four skips are
the count claims, which only the battery transcript can prove), photo-name
grep 0. By name against the v1.3.2 tag (`d0f7dd4`): +15 / −1 (1696 → 1710
test functions) — the three-member commit pin rewritten for four members,
three library pins and eleven GUI pins, listed in ARCHITECTURE's counts note.
The three-lane release battery (`scripts/release_battery.sh`, the p36–p41
calibration corpus and the sidecar weights in reach) was still in its two
library lanes when the tag was cut, at the user's word; its result and the
`--gates` documentation check are recorded in the ROADMAP ledger entry after
the release, as the calibration lane's was for v1.3.2.

Recorded after the tag: the battery finished green on the `d0851ab` snapshot
(the tagged code minus the version literal and the documents — `git diff
d0851ab v1.3.3 -- src tests python` is empty): library **1483 / 0 / 15**
(717.51 s, release profile, one process per module), CLI 24, contract
2 + 2, doc-tests 0, GUI **183 / 0 / 1**, calibration lane **1483 / 0 / 15**
(1258.61 s; one skip line, the mask-brush specimen test whose
`AUTOSHADE_MB_SAMPLE_ROOT` specimen is not on this machine), `audit_i18n`
0 / 0 / 0 and the font check 874/874 inside the battery, 1498 library names
enumerated, and `check_docs.py --gates` on the transcript with the XMP census
root supplied **30 PASS / 0 FAIL / 0 SKIP**.

Final gate, reference pair: this release's CLI (`--version` 1.3.3, built in
its own target directory) rendered the 0.85 develop at full resolution;
against the v1.3.2 release CLI's render of the same develop **0 of
60,217,344 pixels differ**, and the 2048-px downscale against the R37
acceptance render reads mean |diff| 0.00044, max 0.008 — the v1.3.2 numbers.
The three shipped strengths re-measured at 2048 px from the same develops
(0.65 / 0.85 / 1.0) read 0 differing pixels each against the renders the
v1.3.0 acceptance was taken on, with the recorded numbers (0.85: sky ΔE 4.9,
|ΔL*| 0.6, L* spread 1.03 of the target's, land ΔE 6.9). The crops at all
three strengths were viewed beside the target: no seam, no rectangle, the
horizon glow and the far-land haze where the target has them. Nothing in
the solver or the renderer changed, and the pixels say so.
