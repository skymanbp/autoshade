# AutoShade v1.3.1 — the sidecar carries the whole develop, and reads it back after Lightroom

v1.3.0 shipped its zones as Lightroom's own Select Sky masks and left two
things unverified: what Lightroom does to the written intersections, and
whether the `ash:` intent attributes survive a Lightroom rewrite. Both were
measured on 2026-09-12 with the user's Lightroom 9.4 (cloud desktop, "local"
mode). The intersections come back intact; the intent does not, and neither
does anything Lightroom's own model has no place for. v1.3.1 answers with a
PAYLOAD: the develop exactly as the app holds it, plus the mask rasters
nothing can re-derive, carried inside the same sidecar under AutoShade's own
XMP namespace — a spelling Lightroom was measured to preserve byte for byte —
and a reader that reconciles it with whatever Lightroom changed. Two smaller
changes ride on the same measurement: `ColorNoiseReduction` is now written
even at zero, and a sky/land zone whose intent Lightroom stripped is still
recognised from the name the writer gives it.

## What Lightroom 9.4 does to a sidecar (measured)

Two real rewrites are the evidence, kept verbatim as the test material under
`AUTOSHADE_LR_PAYLOAD_FIXTURES` (`xmp::payload_tests::the_real_lightroom_rewrites_read_as_measured`).

A v1.3.0 band sidecar (two Select Sky corrections with intersecting linear
components), 17,238 → 19,818 bytes after one edit in Lightroom:

- both corrections and their gradient components came back, structurally
  intact, re-serialised from Lightroom's own model;
- all twelve `ash:` intent attributes (six names: `Role`, `Inverted`,
  `BaseInverted`, `Combine`, `OwnInverted`, `ComponentInverted`) were gone —
  0 of 12 survived — so the zone roles read back as Custom;
- `crs:Version` moved from 15.5.1 to 18.4;
- 23 root attributes this writer omits at rest were materialised at Camera
  Raw's own defaults: `ColorNoiseReduction="25"`, `ColorNoiseReductionDetail`
  and `Smoothness` 50, `ColorGradeBlending="50"`, `CurveRefineSaturation="100"`,
  the colour-grade, split-toning, grain, vignette and lens-distortion amounts
  at 0.

A PROBE sidecar written to find out which spellings survive, 281,614 →
283,566 bytes: a root attribute, a text element, an `rdf:Bag`, a
`parseType="Resource"` struct, a 15,400-character gzip+base64 recipe and five
raster structs (247,127 characters of base64 between them), all under a
foreign namespace, ALL came back byte for byte — re-serialised into XMP's
compact form (simple properties as attributes on the root `rdf:Description`,
struct fields as attributes on the struct element, every `xmlns:` hoisted to
the root). An unknown `crs:` attribute and an unknown `crs:` element were
dropped. That is the whole design constraint: Lightroom keeps what is not
its own and rewrites what is.

## The payload

Every sidecar AutoShade writes now carries, on the root `rdf:Description`,
under `xmlns:asr="https://autoshade.dev/ns/recipe/1.0/"`:

- `asr:Payload="1"` — the format version. A build that meets a version it
  does not know imports the `crs:` settings alone and says so.
- `asr:Writer="AutoShade 1.3.1"`.
- `asr:Recipe` — the recipe exactly as the app holds it, in the display
  frame, every raster path reduced to its bare file name, as compact JSON,
  zlib level 9, base64. `asr:RecipeCrc32` is the CRC-32 of the JSON bytes.
- `<asr:Rasters>` — an `rdf:Seq` of `{Name, Crc32, Data}` structs in the
  compact attribute form: every raster the recipe cannot re-derive
  (`LocalAdjustment::turnable_raster_paths_mut` — the reverse-fit's bitmap
  tiles and the zone alphas, never an AI mask's cached re-derivation), each
  once, under a 6 MiB raw budget. A raster that cannot be read or would pass
  the budget is left out and named in the save line as
  `MaskLossReason::RasterNotEmbedded` (desktop: "mask rasters not embedded
  ×n"; the recipe still references it by name).

Measured on the 0.85 reference-pair fit: the recipe's 46,761 bytes of
compact JSON pack to a 14,060-character attribute; a 59,856-byte zone alpha
rides as 79,808 characters. The prefix is `asr` unless a merge base already
binds it to a foreign URI, in which case `asr1`, `asr2`… is taken; the
reader looks the prefix up by URI and never by name. The merge strips the
previous payload — attributes and `Rasters` element, by whatever prefix the
base bound — and appends the new one, the strip-then-append discipline the
`crs:` keys already follow. A payload the reader cannot trust (unknown
format, bytes that do not inflate, a CRC that does not match, a recipe field
this build does not know) is disclosed on the status line and the `crs:`
reading stands alone.

## Reading it back: the reconciliation rule

The reader decodes the `crs:` settings exactly as before, then reconciles
three recipes (`xmp::payload::restore`): **P**, the payload; **C**, the crs
reading of the document, where Lightroom's edits live; **W**, the crs reading
of P projected through this writer with no payload and no `ash:` intent —
what Lightroom would have handed back had it rewritten the file without
touching anything, rounding included and the intent gone, which is the
measured half of the rewrite. Leaf by leaf over the JSON trees:

- `W ≈ C` (2e-4, Lightroom's six decimals against this reader's own
  quantisation): Lightroom did not edit this leaf, so P's exact value is
  restored. This is automatically every leaf the projection cannot carry at
  all — the 12×8×8 colour field, a muted mask, a bitmap tile, a zone's role,
  the calibration anchor, the exact slider value behind a two-decimal
  `Exposure2012` — because W and C are then silent on it in the same way.
- otherwise C wins: the document says something different from what was
  written into it, which is what an edit looks like. The one exception is a
  leaf Lightroom MATERIALISES rather than edits — Camera Raw's default for a
  key this writer omits at rest (`SharpenRadius` 1.0, `SharpenDetail` 25,
  `LuminanceNoiseReductionDetail`, `ColorNoiseReductionDetail` and
  `ColorNoiseReductionSmoothness` 50): absent-then-default is not an edit.
- a mask's `role`, the rationale and the confidence are P's outright —
  intent and provenance with no `crs:` spelling.
- a mask's inversion is ONE bit in two homes (the correction's flag and the
  geometry's own bit) whose XOR is all the `crs:` spelling carries, and which
  home holds it is intent Lightroom drops; the pair is reconciled as a unit
  by its net. The net that was written came back → the payload's pair,
  authored home and all; a different net → the document's pair, in
  Lightroom's home. Leaf by leaf, either half would lose: a flipped net kept
  the payload's correction bit beside the document's own bit and undid the
  edit, and an untouched rewrite moved the home for nothing.

Masks are matched, not zipped: payload masks the projection never wrote
(muted, bitmap-based) come back as they were; a written mask is paired with
its read-back by order and name and then with the document's by name; a pair
Lightroom removed is dropped, a correction Lightroom added is appended, and
everything paired is reconciled leaf by leaf like the globals. The replayed
rewrite in `a_lightroom_rewrite_keeps_what_it_did_not_touch_and_yields_what_it_did`
pins all of it at once: an exposure edit, one mask's amount edit, one mask
deleted and one added in Lightroom come back as Lightroom left them, and the
colour field, the roles, the bitmap tile, the muted mask, the Intersect
spelling, the anchor, the rationale, the non-round exposure and the zero
colour noise reduction come back exactly, with Lightroom's materialised 50s
ignored.

## Rasters beside the develop

A payload's rasters are placed beside the develop only on a DISCLOSING read
(the desktop restore, the command line, the web server, the store's
snapshot); the silent probe readers — the ones the merge uses to compare —
never write. A file already there byte for byte is kept; a DIFFERENT file
under the name is left alone and the sidecar's copy takes a fresh `-2` name
that the restored recipe follows, with a line saying so; a name that is not a
plain file name is refused on both sides, never written and never honoured.
The bare names are then anchored to the develop dir exactly as a loaded
`recipe.json` is.

## Two smaller changes

**`ColorNoiseReduction` is written at zero.** This engine renders no colour
noise reduction, so the recipe's 0 is the truth of the render; the key was
omitted at rest, which let Lightroom apply its RAW default of 25 to a photo
the app showed without it. It goes out explicitly now
(`xmp::amount_carries`), and the replayed rewrite pins that the zero we wrote
comes back as the zero we wrote.

**A zone's role comes back from its name.** An unnamed zone goes out under
its role tag (`sky` / `land`) instead of the numbered placeholder; the reader
(`xmp::payload::zone_name_role`) recovers ZoneSky / ZoneLand from exactly
`sky` or `land` (the placeholder, not a name) and from the sub-zone fit's own
`sky · band 2/3` labels (a real name that stays), only on a Select Sky base
and only when no intent survives. Intent, when present, still rules — a
`custom` intent stays Custom. This is the last resort for a v1.3.0 sidecar
Lightroom rewrote before the payload existed; the real band sidecar is the
fixture that pins it.

## Compatibility

A sidecar from any earlier AutoShade, or from Lightroom alone, has no
payload and reads exactly as before. A v1.3.1 sidecar re-saved by v1.3.0
keeps the payload as an element that build does not own — its merge preserves
what it does not model — and a later v1.3.1 read treats that save the way it
treats a Lightroom edit: the `crs:` changes win, the rest is the payload's.
`crc32fast` and `flate2` are the two new direct dependencies; both were
already in the lock through `png`.

## Gates

Lane battery on the merge tree (its own target directories, BelowNormal):
library **1480 passed / 0 failed / 15 ignored** (1495 enumerated, 358.54 s,
one process), CLI **24**, contract **2 + 2**, doc-tests 0, GUI **172 passed /
0 failed / 1 ignored** (release), `clippy --all-targets -D warnings` 0 on the
default feature set and 0 with `--features gui`, `audit_i18n` 0 / 0 / 0,
`subset_gui_fonts --check` 868/868 embedded, `cargo metadata --locked` clean,
`check_docs.py --gates` on the assembled transcript **28 PASS / 0 FAIL / 2
SKIP** (the lane check has no calibration block to read, and the census
corpus is outside the repository). By-name test-set difference
against the v1.3.0 tag (`5446012`), taken statically between the tag's source
and this tree: **+10 / −1** — the nine payload pins in `xmp::payload_tests`
and one rename, `bitmap_masks_do_not_come_back_from_xmp` →
`bitmap_masks_come_back_only_through_the_payload`, re-pinned on both the
projection and the whole document; no test was deleted and no numeric limit
loosened. Eight existing xmp tests that asserted the crs projection through
the public round trip now read the payload-free document
(`xmp::bare_document`), since the ordinary round trip restores the recipe
exactly; five tests outside the module did the same (two bitmap-loss pins
now expect the payload's own verdict beside the projection's, the quarter-turn
pin reads the projection and then checks the whole document restores the
turn once, the render inversion pin runs its six documents through the
payload channel as well).

**The calibration lane did not run, and is not claimed**, for the same reason
as v1.3.0: its p36–p39 fit corpus was deleted in the 2026-09-03 clean-up and
`scripts/release_battery.sh` exits rather than let corpus-gated tests skip
and pass. This release does not touch the fit. The Python sidecars are
unchanged since v1.2.6.

## Verified after release: a v1.3.1 sidecar through Lightroom itself

At release time the end-to-end pair "v1.3.1 writes, Lightroom 9.4 rewrites,
v1.3.1 reads" was a replay built from the measured shapes; on 2026-09-12,
after the release, it was recorded. The 0.85 reference-pair develop — the
whole recipe with its colour field, two Select Sky zones and four bitmap
tiles — was written by v1.3.1 as a 285,172-byte sidecar beside a renamed copy
of the RAW; one mask toggle in Lightroom 9.4 rewrote it in place to 274,012
bytes. The recipe attribute came back byte for byte, all five rasters came
back byte for byte (and were placed beside the develop on the disclosing
read, identical to the originals), every `ash:` attribute was gone, and the
restored develop differs from the one written in exactly fifteen leaves, all
Lightroom's own: the Select Sky reference point and provenance it re-derived
on the two zone masks, and nine unmodelled keys it materialised (kept as
passthrough). The colour field, the four tiles, both zone roles, the
calibration anchor, the exact exposure and the zero colour noise reduction
are the payload's. That pair is now the third fixture under
`AUTOSHADE_LR_PAYLOAD_FIXTURES`, pinned leaf for leaf. What Lightroom paints
on screen for the written intersections is still not measured; what it wrote
back for them is. No GUI executable was launched.
