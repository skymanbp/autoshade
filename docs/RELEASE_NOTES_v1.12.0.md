# AutoShade v1.12.0 — pixel masters out of the delivery folder

Everything merged into `main` since v1.11.0 ships here. A user cleaned out the
delivery folder and then pressed **AI denoise** on the **▣ Original** card,
which failed with *"open image …denoise.png: The system cannot find the file
specified"*. That card still pointed at an in-place denoise master an older
version had written into the delivery folder, and the cleaning had deleted
it. Pixel masters are what cards are built on, so they no longer share a
folder with exports, and a card whose master is gone is handled where it is
found.

The design, the code and this document are the maintainer's; no external
coding model wrote this release.

## Masters have their own folder

- **What moved.** Every pixel master — the result of AI denoise, stacking,
  heal, clone, fill, the ✨ AI edit and reimagine, from the desktop app and
  the browser page alike — is now written to a `masters` folder inside the
  develop store (`%LOCALAPPDATA%\autoshade\masters` on Windows; it moves with
  `AUTOSHADE_DATA_DIR`), with the names it had before:
  `<photo>.denoise.png`, then `<photo>.denoise-2.png`, and so on. The delivery
  folder now holds exports and the extracted style prompt only, so cleaning it
  never breaks a card.
- **Where to find them.** **Settings → Develop store** shows the masters
  folder and a **🗂 Show masters folder** button. Nothing in the app deletes
  masters — deleting a card leaves its master on disk — so this is where to
  clean up; delete the card before its master.
- **Older masters stay put.** A develop links its master by absolute path, so
  masters written by v1.11.0 and earlier are read from wherever they were
  written; nothing is moved.

## A deleted master on the ▣ Original card

Older versions wrote an AI denoise, heal or clone straight onto the **▣
Original** card. When that file has since been deleted, the card now lets go
of it and works from the photo file itself — which is what the canvas was
already showing — and says so in a notice; **Ctrl+S** saves the change. Before,
the card kept the dead path and every pixel action on it (AI denoise, heal,
export) failed opening the missing file. Only a file that is gone is let go:
a master that is there but will not decode keeps its link. Any other card
whose master is gone has no pixels left to show and can be deleted.

## Buttons stay inside their row

In a side panel dragged to its narrowest (280 px), the reference library's
**🗂 RAW folder…** label was two pixels wider than its half of the row in
Chinese, so the button grew and pushed **✕ Clear** past the edge of its card.
A button in a grid row now shortens its label with "…" when the label does not
fit, at any panel width. The layout tests had measured labels in egui's
bundled font, which draws Chinese narrower than the fonts the app installs;
the new test lays the panel out in the app's own fonts.

## Compatibility

- **Nothing to migrate.** Recipes, stores, settings, the reference library,
  the command line's commands and the browser page's routes are those of
  v1.11.0. The delivery folder setting keeps its name and meaning for exports;
  only masters stop landing there. The sidecars under `python/` are byte for
  byte v1.11.0's, and the weights stay pinned to the v1.6.0
  `autoshade-raw-denoise-v2.pth`.

## Gates

Measured before the tag on the release code (`72b194b`); the test counts in
the documents moved after it to the figures below, and the version bump
touches Cargo.toml, Cargo.lock, the documents, the site's cache keys and the
bug template's dropdown. The CLI, contract and doc-test suites, clippy,
`check_docs`, `check_doc_drift`, the i18n audit and the font check are re-run
after the bump. The three-lane release battery (`scripts/release_battery.sh`,
a frozen snapshot worktree, the p36–p41 calibration corpus and the sidecar
weights in reach): library **1641 passed / 0 failed / 15 ignored** (716.58 s,
release profile, one process per module), CLI **25 / 0**, contract 2 + 2,
doc-tests 0, GUI **243 / 0 / 3**, calibration lane **1641 / 0 / 15**
(1647.37 s); `audit_i18n`, the font check (898 of 899 code points embedded,
one left to egui's bundle) and `check_doc_drift` (0 findings over 14
documents) exit 0 inside the battery. clippy 0 warnings on both feature sets
(`--all-targets -- -D warnings`). `check_docs.py --gates` on the transcript
with the XMP census root supplied: 28 PASS / 4 FAIL on the snapshot — the
four test-count claims, still at v1.11.0's figures — and **32 PASS / 0 FAIL /
0 SKIP** once the counts were moved.

By name against the v1.11.0 release battery: library 1655 → 1656, **+1 / −0**
(`pixel_masters_are_claimed_outside_the_delivery_folder`: every master claim
lands in the masters folder with the old naming, never in the delivery
folder); GUI 244 → 246, **+2 / −0**
(`a_vanished_original_master_hands_the_card_back_to_the_photo`,
`no_cell_button_reaches_past_the_narrowest_panel`); CLI 25, unchanged.

The Python suites were not run: `python/`, `scripts/` and `tests/` are byte
for byte v1.11.0's (the ruling of 2026-10-13: run what changed). Final gate,
reference pair: not re-run. The reverse fit and the develop are untouched —
the batch changes where masters are written, one GUI landing, the button
helper, Settings and the documents — and a recipe renders as in v1.11.0.

Mutations falsified the three new invariants on the code that became
`72b194b`, each red and each restored: writing masters through the delivery
folder again turned `pixel_masters_are_claimed_outside_the_delivery_folder`
red; keeping the ▣ card's dead link turned
`a_vanished_original_master_hands_the_card_back_to_the_photo` red; and taking
out the label truncation grew the 280 px panel to 281 px (English) and turned
`no_cell_button_reaches_past_the_narrowest_panel` red.
