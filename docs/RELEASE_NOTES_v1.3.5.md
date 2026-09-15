# AutoShade v1.3.5 — the fill sees the card's picture and lands as a new card; a blank prompt removes

Two sentences from one report (2026-09-15): the Retouch panel's Generative
Fill "does not seem to generate from the current variant but from the
original"; and "let an empty prompt mean remove".

## The fill sees the card's picture and lands as a new card

Through v1.3.4 the fill was an in-place retouch like a heal: `start_fill`
handed the active card's pixel source to `generative::retouch`, which developed
it through `render::source_pixels` with a default recipe — no camera curve
(0.6–1.4 EV under the card's Before), none of the card's sliders or masks —
sent that to the model, and composited the answer back into the neutral
master, where the card's recipe kept rendering on top. That is the right shape
for pixel arithmetic (a heal, a clone, a denoise are the same at any tone), and
the wrong one for a model that has to see the picture: the prompt refers to
what is on screen, the fill it returns matches a develop nobody looks at, and
the card's content-keyed corrections — the sky raster masks, the tiles, the
vignette, the dehaze — then land on content they were never solved for.

The user chose, from four options, "new card: what you see is what is filled"
(the in-place un-develop, which double-applies every position-keyed
correction inside the region and needs a specimen study, and the minimal
"send the Before look" were declined).

- The library gains `generative::retouch_onto`: the fill on a base its caller
  produces inside the full-resolution section (the model call runs outside it,
  the composite re-enters). `retouch` is now its thin wrapper on the source's
  neutral develop — the CLI's and the browser's fill, unchanged.
- The desktop fill develops the card's own pixel source (a ✨ / ✎ card's
  raster, the ▣ / ◭ cards' negative) through the card's live recipe with
  `render::develop_preview_framed` under `MaskFrame::without_downstream`: the
  tone chain the canvas runs, without the geometry stage, because the mask is
  painted in the original frame — the same declaration the range references
  make. At ≤2048 px, or the whole frame with **Full-res fill** on a RAW.
- The answer lands as a new **✨ AI generated** card (`RetouchKind::
  NewGenerated`, artifact `<stem>.fill-N.png`, named like a reimagine's): its
  look lives in its pixels, further edits fork a ✎ card, the reverse-fit can
  target it, no XMP projects it. The card you filled from keeps its recipe, base
  and origin, the ▣ negative is not changed by a fill, and crop / straighten are
  not carried over (set them on the new card).
- Heal, clone and denoise stay in-place touch-ups of the neutral master.

## A blank prompt removes

An empty prompt used to be refused on both front ends ("write what should fill
the painted area") while the mask alone already said what the user meant.
`generative::fill_prompt` now swaps a blank or whitespace prompt for
`REMOVE_PROMPT` — continue the surroundings into the area, add nothing new,
leave everything outside the mask as it is — inside the library, so the
desktop app, the browser and `autoshade retouch` (whose `--prompt` is now
optional) send one and the same instruction. Typed words ride verbatim. The
placeholder, the hint, the tooltip and the landing line say so.

## Compatibility

- **Desktop fill.** A fill no longer edits the card in place: it makes a new ✨
  card. A workflow that filled the ▣ card to clean the negative before a
  reimagine or a reverse-fit no longer does that through the fill (the
  negative's master still follows a denoise, a heal and a clone).
- **CLI.** `retouch --prompt` may be omitted (removal); a given prompt behaves
  as before. The browser's Fill and the CLI's `retouch` still composite onto
  the source's neutral develop (documented in the manual).
- **Library API.** `generative::retouch` keeps its signature; `retouch_onto`,
  `FillJob`, `fill_prompt` and `REMOVE_PROMPT` are new.
- **Store format.** Unchanged: the fill card is an ordinary generated card.
- No renderer, solver or recipe schema change: the reference pair renders to
  the same bytes (Gates).

## Gates

Measured before the tag on the release code (`24960d9`; the version bump
touches Cargo.toml, Cargo.lock and the documents only): library **1487
passed / 0 failed / 15 ignored** (1502 enumerated, release profile, 260.01
s), CLI **24 / 0**, contract 2 + 2, doc-tests 0, GUI **187 passed / 0 failed
/ 1 ignored** (the gui feature), clippy 0 on both feature sets, `audit_i18n`
0 / 0 / 0, `subset_gui_fonts.py --check` 875/875 (the new Chinese strings use
hanzi the embedded subset already carries; the coverage test had named 看 东
西 矫 ＝ and the strings were reworded rather than the fonts regenerated),
`cargo metadata --locked` clean, `check_docs.py` 25 PASS / 0 FAIL / 5 SKIP
(the skips are the count claims and the census, which only the battery
transcript and the census root can prove), photo-name / token / user-path
grep 0. By name against the v1.3.4 tag (`14a2a4b`): library 1499 → 1502
(+3 / −0), GUI 187 → 188 (+1 / −0), 1714 → 1718 test functions — listed in
ARCHITECTURE's counts note. Mutations, each restored byte-for-byte: a blank
prompt passed through verbatim turns the two blank-prompt tests red;
`retouch_onto` ignoring the caller's base turns the composite test red;
`start_fill` landing in place, and `start_fill` sending the neutral base
instead of the card's picture, each turn the GUI card test red on its source
pin.

Not measured: no paid gpt-image call was made for this release; the wire
prompt and the composite are proven on a loopback endpoint.

Final gate, reference pair, before the tag: the 1.3.5 CLI (built in its own
target directory, `--version` 1.3.5) re-rendered the 0.85 reference develop
at full resolution — **0 of 60,217,344 pixels** differ from the v1.3.4 CLI's
render of the same develop; downscaled to 2048 px it sits at mean |diff|
0.00044 / max 0.008 against the R37 acceptance render, the same numbers as
v1.3.4, v1.3.3 and v1.3.2; 0.65 / 0.85 / 1.0 at 2048 px are pixel-identical to
the v1.3.4 release's renders (0 of 2,795,520 each; 0.85: sky ΔE 4.9, |ΔL*|
0.6, L* spread 1.03, land ΔE 6.9), and the three crop sheets were viewed
beside the target — v1.3.4's sheets, since those renders and these are the
same bytes and the target file has since left the install directory's `out`
folder: no seam, no rectangle. The solver and the renderer did not change;
the pixels prove it.

Recorded after the tag: the three-lane release battery
(`scripts/release_battery.sh`, the p36–p41 calibration corpus and the sidecar
weights in reach) ran on the `24960d9` snapshot before the tag (the tagged
code minus the version literal and the documents — `git diff 24960d9 v1.3.5
-- src tests python assets scripts` is empty) and finished green: library
**1487 / 0 / 15** (445.96 s, release profile, one process per module),
CLI 24, contract 2 + 2, doc-tests 0, GUI **187 / 0 / 1**, calibration lane
**1487 / 0 / 15** (680.99 s; one skip line, the mask-brush specimen
test whose `AUTOSHADE_MB_SAMPLE_ROOT` specimen is not on this machine),
`audit_i18n` 0 / 0 / 0 and the font check 875/875 inside the battery, 1502
library names enumerated (+3 / −0 against the v1.3.4 transcript; GUI 187 →
188 by name, +1 / −0), and `check_docs.py --gates` on the transcript with the
XMP census root supplied **30 PASS / 0 FAIL / 0 SKIP**. The ship facts (the
release run, the downloaded assets, the site, the local upgrade) are in the
ROADMAP ledger entry.
