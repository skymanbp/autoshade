# AutoShade v1.7.0 — a generative fill that lands as a layer, and a side panel that reads in levels

Everything merged into `main` since v1.6.7 ships here: two reports the
maintainer made on 2026-10-01 while using v1.6.7. A generative fill painted on
a normal card came back as a separate ✨ AI-generated card instead of on the
card it was made on, and the desktop app's side panel had a dropdown that ran
the whole width of a widened panel, an AI section whose title looked like its
own sub-sections, and grey separator lines between groups. The develop engine
gains one final step — drawing a card's fill layers — that draws nothing on a
recipe without layers, so every existing recipe renders as in v1.6.7.

The diagnosis, the design decision (asked and answered before the code was
written), the fixes and this document are the maintainer's; no external coding
model wrote this release.

## Generative fill: a layer on the card it was made on

- **What happened.** The question: 「为什么我选中正常图片变体，涂抹并AI生成填充后，
  出来的结果是一个新"AI生成"变体？不应该是直接在原先的变体上改吗？」 — and: how
  can the generated area sit over the original as a non-destructive layer that
  is saved? Through v1.6.7 a fill composited the generated pixels into a copy of
  the whole developed frame and landed that copy as a new ✨ card, so the fill
  and the photograph it was made on were two separate pictures.
- **The choice.** Asked whether a layer should sit under the develop (the
  sliders would then change the generated pixels too) or over it (the model
  sees the developed picture; the layer keeps that look), the maintainer chose
  **over the develop**.
- **What changed.** A fill now writes only the generated patch: an RGBA file
  whose colour is the generated pixels and whose alpha is the feathered painted
  area. It lands on the card the fill was made on as one undo step (a pristine
  ✨ card is first forked to an ✎ card, as for any edit). The recipe gains a
  `pixel_layers` list — each entry a path, an on / off switch and an opacity —
  and the develop draws every layer that is on as its **last** step, after the
  tone and colour chain, before geometry, so a crop or a straighten carries the
  layer with the rest of the picture. The card's own pixels never change:
  switching a layer off or removing it gives the photograph back. The
  Generative Fill fold lists the card's layers with a show / hide eye, a remove
  button and an opacity slider. A layer file is read through the same bounded
  loader and the same memory budget as the mask rasters; an export refuses an
  unreadable layer rather than writing a file without it, and the preview skips
  it with one line of warning.
- **The trade, stated.** A layer keeps the look of the moment it was made.
  Sliders moved afterwards change the photograph under it, not the layer, so a
  large later edit can show a seam around the patch — fill again then. The
  manual says so.
- **Where it does not apply.** Lightroom has no element for a pixel layer, so
  the written sidecar leaves layers out and the save line names 「fill layers」
  among what is not exported. Pasting a recipe from another photo drops its
  layers (they are that photo's pixels) with a note saying how many. Rotation
  by 90° is off while any card holds a layer. The Adjust region tool still lands
  as a new card, and the photo browser's Fill and the command line's `retouch`
  are unchanged.

## The side panel

- **The From menu.** The reverse-fit fold's **From** dropdown took the whole
  available width, so on a dragged-wide panel it ran edge to edge. It is now
  sized at the start of its row and ends on the right edge of the verb buttons
  under it (capped at the same 420 px as every other row).
- **Three heading levels.** Panel titles (AI, Develop, Retouch) are drawn in
  the heading size under a short gold rule — the AI title used to be drawn at
  the size of its own folds, so its five folds read as its siblings. Group
  heads are in the accent gold with a small tick: Develop's four groups, and two
  new groups in the AI section, **Analysis & References** and **Generate &
  Reverse-fit**. Folds sit below.
- **No separator lines.** Every grey separator line in the three side panels is
  gone; groups are set apart by space and their heads, captions inside a fold
  are small muted labels. The remaining hairlines are one step quieter, and the
  heading size moves from 16.5 to 17.5 pt.

## Documents and the site

- **The manual** describes fill layers (the fold's list, the switch, opacity,
  the seam trade, what is not exported) and the imported-removal button's new
  wording. README's line on labelled generated pixels and ARCHITECTURE's
  landing paragraph follow. README, ARCHITECTURE, TECH_STACK and the site carry
  the new battery counts (1771 library, 228 GUI tests) and their version words;
  the site changes nothing else but its cache keys.

## Compatibility

The recipe gains one field, `pixel_layers`, written on every save (empty when a
card has no layer). Recipes and stores from v1.6.7 and earlier open unchanged
and render as before. A recipe saved by v1.7.0 is refused by v1.6.7 and earlier,
which reject fields they do not know; downgrading is not supported (the
installer refuses one). The sidecar's XMP is unchanged. The sidecar weights
stay pinned to the v1.6.0 `autoshade-raw-denoise-v2.pth`; no weights are
re-shipped.

## Gates

Measured before the tag on the release code (`0499b71`: the side panel and
the fill layer `ae4dea5`, the battery counts `0499b71`). The version bump
touches Cargo.toml, Cargo.lock, the documents, the site's cache keys and the
bug template's dropdown, and the CLI, contract and doc-test suites, the
denoise module, clippy, the `python/` suite and `check_docs` are re-run after
it. The three-lane release battery (`scripts/release_battery.sh`, a frozen
snapshot worktree, the p36–p41 calibration corpus and the sidecar weights in
reach): library **1756 passed / 0 failed / 15 ignored** (1072.26 s, release
profile, one process per module), CLI **27 / 0**, contract 2 + 2, doc-tests
0, GUI **227 / 0 / 1**, calibration lane **1756 / 0 / 15** (1626.50 s),
`audit_i18n` and the font check exit 0 inside the battery; the Python suites
82 OK from `python/` and 44 OK from `scripts/` (CPU, the real weights,
`-W error::RuntimeWarning`). By name against the v1.6.7 release battery:
library 1768 → 1771, three additions
(`a_layer_replaces_where_it_is_opaque_and_leaves_the_rest`,
`the_develop_draws_its_layers_and_an_export_refuses_a_missing_one`,
`layers_round_trip_through_the_recipe_and_refuse_unknown_keys`), nothing
removed; CLI 27 → 27; GUI 226 → 228, three additions
(`the_reverse_fit_from_menu_ends_where_the_verb_row_ends`,
`the_ai_title_outranks_its_folds_and_its_folds_sit_under_group_heads`,
`a_fill_lands_as_a_layer_on_the_card_it_was_made_on`) and one removal
(`a_fill_lands_as_a_new_generated_card_and_leaves_the_filled_card_alone`,
the behaviour this release replaces). clippy 0 warnings on both feature sets
(`--all-targets -- -D warnings`). `check_docs.py --gates` on the transcript
with the XMP census root supplied: on the frozen snapshot **32 PASS / 0 FAIL
/ 0 SKIP** — the counts moved with the code before the snapshot. The re-run after the bump is recorded at the bump.

Final gate, reference pair, before the tag: the release code's CLI re-fitted
the reference pair at 0.65 / 0.85 / 1.0 and rendered each at the target's
size. Of 23 comparisons with the v1.6.7 gate, 20 are **byte-identical**:
every recipe once its new empty `pixel_layers` line is set aside, every mask
raster and every 1000 px render with and without masks. The three written
sidecars differ only inside the compressed copy of the recipe they carry:
decoded, that copy gains `"pixel_layers": []` and is otherwise equal, and
every byte of the XMP outside it is equal. The readings are therefore
v1.6.7's (sky ΔE / whole-frame mean |diff| against the target: 0.65: 6.3 /
0.0284, 0.85: 3.9 / 0.0250, 1.0: 3.9 / 0.0252). The maintainer's own look at
the three-way sheet at 0.85 (the target, the v1.6.1 approved gate, this
build): the sky is one smooth gradient with no block, seam or tile, and the
ground reads as the approved gate's.

Four mutations falsified the new invariants on the snapshot, each red and
each restored (SHA-256 of all four files matched, worktree clean): the From
menu taking the whole available width turned
`the_reverse_fit_from_menu_ends_where_the_verb_row_ends` red; the panel title
drawn at button size turned
`the_ai_title_outranks_its_folds_and_its_folds_sit_under_group_heads` red;
the develop skipping the layer composite turned
`the_develop_draws_its_layers_and_an_export_refuses_a_missing_one` red; the
fill landing as a new generated card again turned
`a_fill_lands_as_a_layer_on_the_card_it_was_made_on` red.
