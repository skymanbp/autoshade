# AutoShade v1.8.0 — a fill that matches its surroundings, an AI edit with nothing painted, and a side panel that keeps its edge

Everything merged into `main` since v1.7.0 ships here: four reports the
maintainer made on 2026-10-12 while using v1.7.0. A generative fill on an
already edited card came back visibly brighter than the picture around it
(「涂抹生成后的区域跟周围还是有很明显的差距，感觉是没用变体用了原片？」); only a
✨ AI-generated card could be changed by words, an ordinary card only by its
sliders; the Generative Fill fold's hint text ran under the side panel's right
edge; and the panel still looked unfinished. The develop engine is unchanged:
every recipe renders as in v1.7.0.

The diagnosis, the measurements on the maintainer's own photographs, the fixes
and this document are the maintainer's; no external coding model wrote this
release.

## Generative fill: levelled onto its surroundings

- **What happened.** The question was whether the fill had been shown the
  original photograph instead of the edited card. It had not: measured on the
  maintainer's own fill, the picture sent was the card's develop (a neutral
  develop of the same photograph sits about 104 levels away from what was
  sent). The model itself redraws the whole frame a little brighter or darker
  than it was sent — in the band where that fill was feathered in, the
  generated pixels sat **+42 / +40 / +24** levels (8-bit, R / G / B) above
  the card around them.
- **What changed.** Before the generated area is pasted, it is levelled by
  how far the model moved the parts it was meant to leave alone: the shift is
  measured cell by cell over everything outside the painted area, spread
  smoothly into the area from its rim, and taken off the generated pixels.
  The same fill re-run after the change (one paid request at medium quality)
  met its surroundings at **−0.6 / −0.1 / +0.1** levels in that band. This
  applies to every painted fill: the desktop app's, the command line's
  `retouch` and the browser page's.

## AI edit: change a card by words, with nothing painted

- **The request.** 「对于非AI整图生成的一般变体，也要有个"AI生成式修改"」 — an
  ordinary card should take instructions like "remove the tree on the left"
  or "make the sky bluer", the way a ✨ AI-generated card already could. Asked
  how it should work, the maintainer chose: like the fill, without the
  painting step.
- **What changed.** In the Generative Fill fold, with no area painted, write
  what to change and press **✨ AI edit**. The model is sent the card's
  picture and the instruction with no mask; AutoShade then finds the area the
  answer actually changed — against the model's whole-frame drift, which it
  measures from the half of the picture that moved least — levels it like a
  fill and lands only that area as a layer on the card, as one undo step. The
  status line says what share of the picture changed. With an area painted
  the button is **Remove / Fill** as before.
- **Measured on real answers** (two paid requests on the maintainer's
  photograph): removing a tree changed **2.6 %** of the frame and that is
  what was layered; making the sky bluer changed **51 %** — the sky and its
  rim — and the ground stayed the card's own.
- **Where it does not apply.** The ✨ AI-generated card's own whole-picture
  edit (**Adjust generated image**) is unchanged; the command line and the
  browser page have no maskless edit.

## The side panel

- **An edge on the right.** The panel's scroll bar floats over its content
  and reserved no width, so text and buttons could run under it. The column
  now keeps a fixed gutter of the bar's width plus a small space on its right,
  and a test renders the panel offline and fails if any text reaches into it.
- **Cards.** Every open fold in the AI, Develop and Retouch panels sits on a
  rounded card with its own fill and a hairline edge, so where a fold starts and ends
  is visible without separator lines. Idle buttons and fields gain a slightly
  stronger edge, and the fold indent loses its vertical line.

## Documents and the site

- **The manual** describes the levelling and the AI edit; ARCHITECTURE
  describes how the changed area is found and the levelling. README's two
  lines on where generated pixels land now name the AI edit beside the fill.
  README, ARCHITECTURE, TECH_STACK and the site carry the new battery counts
  (1777 library, 28 CLI, 230 GUI tests) and their version words; the site
  changes nothing else but its cache keys.

## Compatibility

The recipe format is unchanged from v1.7.0: an AI edit is stored as the same
`pixel_layers` entry a fill is. Recipes and stores from v1.7.0 and earlier
open unchanged and render as before. The sidecar's XMP is unchanged. The
sidecar weights stay pinned to the v1.6.0 `autoshade-raw-denoise-v2.pth`; no
weights are re-shipped.

## Gates

Measured before the tag on the release code (`87a4031`: the fill levelling,
the AI edit and the side panel `2257942`, the battery counts `87a4031`). The
version bump touches Cargo.toml, Cargo.lock, the documents, the site's cache
keys and the bug template's dropdown, and the CLI, contract and doc-test
suites, the denoise module, clippy, the `python/` suite and `check_docs` are
re-run after it. The three-lane release battery (`scripts/release_battery.sh`,
a frozen snapshot worktree, the p36–p41 calibration corpus and the sidecar
weights in reach): library **1761 passed / 0 failed / 16 ignored** (988.36 s,
release profile, one process per module), CLI **28 / 0**, contract 2 + 2,
doc-tests 0, GUI **228 / 0 / 2**, calibration lane **1761 / 0 / 16**
(1404.39 s); `audit_i18n`, the font check and `check_doc_drift` exit 0 inside
the battery; the Python suites 82 OK from `python/` and 44 OK from `scripts/`
(CPU, the real weights, `-W error::RuntimeWarning`). By name against the
v1.7.0 release battery: library 1771 → 1777, six additions
(`a_fill_is_levelled_onto_its_surroundings`,
`pull_push_keeps_known_cells_and_fills_holes_from_their_rim`,
`the_found_area_is_the_changed_block_not_the_drift`,
`a_sky_sized_edit_is_the_edit_not_the_drift`,
`an_auto_fill_sends_no_mask_and_layers_only_what_changed`, and the ignored
tuning bench `found_region_bench`), nothing removed; CLI 28 (the synopsis
test added after v1.7.0); GUI 228 → 230, two additions
(`no_panel_text_runs_under_the_scrollbar` and the ignored offline
`panel_screenshots`), nothing removed. clippy 0 warnings on both feature sets
(`--all-targets -- -D warnings`). `check_docs.py --gates` on the transcript
with the XMP census root supplied: on the frozen snapshot **32 PASS / 0 FAIL
/ 0 SKIP**. After the bump ⟨POST⟩.

Final gate, reference pair: not re-run for this release. This batch changes
neither the reverse fit nor the develop (the diff touches the generative
fill and the desktop app's side panel only), and the pair's inputs were
cleared on 2026-10-02; the maintainer ruled the gate unnecessary for code the
batch does not touch.

Four mutations falsified the new invariants on the snapshot, each red and
each restored (SHA-256 of both files matched, worktree clean): the generated
area pasted without levelling turned
`an_auto_fill_sends_no_mask_and_layers_only_what_changed` red; the drift's
median taken over every cell instead of the half that moved least turned
`a_sky_sized_edit_is_the_edit_not_the_drift` red; the drift left out of the
final score turned `the_found_area_is_the_changed_block_not_the_drift` red;
the side panel's right gutter set to 0 turned
`no_panel_text_runs_under_the_scrollbar` red.
