# AutoShade v1.5.1 — a generated picture gets its own Adjust entry, a header's dot describes this photo, and every pinned download has a copy of ours

One report, five sentences (2026-09-20): the Heal / Clone Stamp buttons and the
two reverse-fit buttons "stretch without limit again"; "Lens and Export show
the edited dot and I have not touched anything"; the AI denoise "is still not
as good as Lightroom's"; "a whole-image generation needs an Adjust — once it is
generated there is no entry for changing part of it"; and an instruction about
who writes the code. The first, second and fourth ship here in full, and the
fifth shaped how all of it was made. The third — the denoiser's picture
quality — ships only its smaller half, a preference migration; the section at
the end says exactly where the rest stands.

Every implementation in this release was written by an external coding model in
isolated worktrees; the design, the line-by-line review, the merge, the gates
and this document are the maintainer's.

## A generated picture gets its own Adjust entry

A ✨ AI generated card had no way forward except starting over: Reimagine reads
the negative, Generative Fill needs a painted area, and neither answers "keep
this picture, but change that".

The AI panel gains a fold of its own, **Adjust generated image · paid API**,
right under Reimagine, available on ✨ Generated and ✎ Edited cards and
disabled while a job runs.

- **No paint: the prompt edits the whole picture.** No mask part is sent and
  the returned frame is kept whole, resampled (Lanczos3) back to the card's own
  size. A blank prompt is refused before the develop and before any billed
  call — there is nothing to ask for.
- **Paint: the same brush the Retouch panel uses limits the change.** A painted
  area makes it a fill of that area; a blank prompt then means remove, exactly
  as in the Retouch panel since v1.3.5. The fold says which of the two it is
  about to do.
- **Every answer lands as a new ✨ card** (`<stem>.adjust.png`, `.adjust-2.png`,
  … by the same atomic naming rule every output family uses). The card you
  started from keeps its pixels, recipe and origin, so an adjustment can be
  compared, adjusted again, or reverse-fitted.
- The picture sent is the card as you see it — its own pixels through its live
  recipe — through the path the fill already used
  (`developed_card_pixels`), and the divergence figure on the landing line is
  measured against the image that was SENT, never used to buy a second call.

In the library this is `generative::adjust_onto` with `AdjustJob` /
`AdjustReport`; `retouch_onto` keeps its signature and both now run on one
private core (`edit_onto`), so input scaling, the optional mask, the size
fallback, cancel and the write are one implementation. The browser and the CLI
still have the regional retouch only.

Whether any paint exists is asked every frame by two folds, and the brush
buffer is allocated at the photo's size whether or not anyone has painted
(up to 8192 px on an edge). The answer is therefore cached beside the buffer
and invalidated through ONE door (`paint_mask_changed`, which is also the only
place `mask_dirty` is set): a new canvas and Clear answer "no" outright, a
stroke answers "yes" outright, and only an erase, an import or a load leaves
the question open — for at most one scan. A counting test (scans, not
milliseconds) and a census of every write site hold it there.

## Button rows stop at the readable width

`buttons::columns` divided the WHOLE panel's width among a row's buttons, and
the panel has no maximum width, so widening the window stretched Heal, Clone
Stamp, the two reverse-fit verbs and Copy recipe across it. The prompts had
been capped at `theme::FIELD_W_MAX` (420 px) long ago; the button rows added
since had not. The rows now share that ceiling, left-aligned, with the same
whole-pixel rounding; the curve and HSL editors still grow with the panel.
The test draws the control panel at 800 and 1600 px and the gallery at 800 px,
in both languages, for three frames, and checks every button it finds.

## A header's dot describes this photo

A fold's ● says "this photo carries an edit here". Three headers lit it for
things that are not edits of this photo:

- **Lens** counted the camera's own calibration stamp as a manual adjustment.
  It now lights for the manual slider families, or when the profile switches
  differ from how the photo OPENED — `LensProfile::is_as_opened`, which knows
  both stamps a photo can open with: the camera's components on, and everything
  off because the sidecar said `crs:LensProfileEnable="0"` (with that
  provenance recorded). Switching everything off by hand still lights it.
  `EditRecipe::is_noop` deliberately keeps its narrower rule, so the saved-
  develop priority and the unsaved badge mean what they meant.
- **Export** compared the delivery preferences (JPEG quality and the rest) with
  TIFF defaults. Those are preferences with no per-photo neutral state, so the
  header carries no dot at all; the toolbar's hover still lists the delivery.
- **AI** counted three strength dials that are saved in the preferences and
  restored at launch, so moving one once lit the dot on every photo for ever.
  By the user's ruling it now reads this photo only: a verdict on screen, or a
  Direction typed in.

## The two denoise dials reset once, and say what they were

v1.4.0 changed what the denoise strength MEANS — from SCUNet's luma/chroma
split, where 0.5 was the sweet spot, to a blend in the RAW domain, where 1.0 is
the whole result — and old saved values (0.65 and 0.5 on the reporting
machine) had been carried across that change ever since, quietly running the
new denoiser at two thirds. Preferences now carry an era (`prefs_era`; a file
without the key is era 0). Going from 0 to 1 resets both dials to 100 % once
and the status line states, in both languages, the two values it replaced; an
era-1 file is restored as saved, whatever it holds.

## Every pinned download has a copy of ours, and it is tried first

Each model this program runs comes from somebody else's server at a pinned
revision. The pin is what makes the download verifiable and also what makes a
vanished upstream unrecoverable — a renamed account would not be a slow
download, it would be a feature that cannot start on any cold cache, which is
every new installation. All 52 pinned files (seven upstreams) now also live in
repositories of ours. `python/_mirror.py` is the one table that says where, and
`_fetch_verified` walks ours, then the upstream.

The order is a preference and never a trust decision: a source decides WHERE
bytes come from, the pinned sha256 and byte count decide whether they are
kept, and a stale or wrong mirror is refused by the same gate a wrong upstream
would be. The cache identity is untouched, so no existing installation
re-downloads a byte. Mirroring makes this project a redistributor, so each
mirror carries its upstream's licence, and the README's licence table gains the
Stable Diffusion 2.1 row it never had. Three source invariants (a pinned tree
has a mirror; a mirror coordinate is a full 40-hex commit; an executed upstream
source is mirrored too) are held in Rust, because that is what CI runs.
AutoShade's own denoise weights are deliberately not in the table: they already
live on this project's own release page.

Also in the tree since v1.5.0: the one Lightroom sidecar with HDR edit mode on
that the HDR shoulder was measured against (`src/fixtures/`, 8,165 bytes, byte
for byte), with a test that hands the reader Lightroom's own bytes.

## Compatibility

- **Preferences.** One new key, `prefs_era`, and one for the Adjust fold's
  quality. A pre-v1.5.1 file loads; its two denoise dials reset once, with the
  note above. Nothing else moves.
- **Outputs.** A new family, `<stem>.adjust.png`, `.adjust-2.png`, ….
- **Library API.** New: `generative::{adjust_onto, AdjustJob, AdjustReport}`
  and `LensProfile::is_as_opened`. `retouch_onto` and `retouch` keep their
  signatures.
- **Store and sidecar formats.** Unchanged: an adjusted picture is an ordinary
  generated card.
- **Downloads.** Same files, same pins, same cache directories; only the order
  of sources changed. The RAW denoise weights are still the v1.5.0 asset.
- No renderer, solver or recipe-schema change: the reference pair renders to
  the same bytes (Gates).

## What this release does not fix: the denoiser against Lightroom

The report was right, and the preference reset above is only the smaller half
of it. Measured on the same capture against Lightroom's own Denoise at 50: the
residual fine grain Lightroom leaves is the same share of the input on every
frame (0.28–0.30), while ours swings with the noise level — 0.42 at ISO 2500,
0.25 at ISO 3200, 0.11 at ISO 8000 — and what we leave is as strong in colour
as in luminance (0.39 against 0.42 at ISO 2500). The cause is how the strength
is obtained, not the network: v1.5.0 gets its texture by telling the network
the noise is 0.78 of what was measured, and the effect of that under-statement
depends on the noise level. Told the truth, the same network leaves 0.07 /
0.02 / 0.01 and keeps the faint stars level with Lightroom.

The replacement is designed and under way, and it is not in this release:
clean once at the honest noise level, then hand back LUMINANCE grain only,
after demosaic, in linear light, as one number added to all three channels —
so no colour noise can return, by construction. A first attempt to do that in
the mosaic domain was withdrawn on a measurement (grey grain put back per CFA
quad came out of the demosaic as colour: Cb 0.27, Cr 0.24 beside Y 0.24). It
ships when the starry-sky frame and its Lightroom pair pass as a standard, not
before.

## Gates

Measured before the tag on the release code (`29ab1b6`; the version bump touches
Cargo.toml, Cargo.lock and the documents only). The three-lane release battery
(`scripts/release_battery.sh`, a frozen snapshot worktree, the p36–p41
calibration corpus and the sidecar weights in reach): library **1634 passed / 0
failed / 15 ignored** (689.61 s, release profile, one process per module), CLI
**25 / 0**, contract 2 + 2, doc-tests 0, GUI **213 / 0 / 1**, calibration lane
**1634 / 0 / 15** (1151.14 s; one skip line, the mask-brush specimen test whose
`AUTOSHADE_MB_SAMPLE_ROOT` specimen is not on this machine, named in every
release since v1.3.2), `audit_i18n` and the font check exit 0 inside the
battery. By name against the v1.5.0 tag, listed by the harness on both trees:
library 1640 → 1649 (+9 / −0: four mirror invariants, three adjust tests, the
lens-profile truth table, the Lightroom HDR sidecar); GUI 201 → 214 (+14 / −1:
the row-width ceiling, the three header rules, five preference-era tests, the
adjust fold's landing, enablement and region hint, the paint-presence scan
count and its write-site census; the one removed name pinned a saved dial
lighting the AI dot, replaced under the ruling above). clippy 0 warnings on
both feature sets (`--all-targets -- -D warnings`, on `29ab1b6`).
`check_docs.py --gates` on the transcript with the XMP census root supplied:
**30 PASS / 0 FAIL / 0 SKIP** (after the bump; before it the same run failed
exactly the four count claims, which is what it is for). Photo-name,
token-shape and user-path greps on the release diff: 0 / 0 / 0.

Mutations, each run by hand and restored byte for byte: removing the width
ceiling turns the row test red ("Copy recipe" is 784.0 px wide); treating any
all-off lens profile as the opened state turns the hand-switched arm red;
putting one saved dial back into the AI predicate turns the preference-only arm
red; dropping the two dial resets turns the era test red with (0.65, 0.5);
sending a mask on a whole-image adjust, and dropping the AI-pixel gate, each
turn their own test red; dropping the erase branch's invalidation turns the
scan-count test red.

Final gate, reference pair, before the tag: the pre-bump release CLI re-rendered
the 0.85 reference develop at full resolution — **0 of 60,217,344 pixels**
differ from the v1.5.0 CLI's render of the same develop; downscaled to 2048 px
it sits at mean |diff| 0.00044 / max 0.008 against the R37 acceptance render,
the same numbers as every release since v1.3.2; 0.65 / 0.85 / 1.0 at 2048 px
are pixel-identical to the v1.5.0 release's renders (0 of 2,795,520 each). The
whole frame and three crops (sky gradient, horizon seam, foreground) were
viewed beside the acceptance plate: no seam, no rectangle.

Not measured: no paid image call was made for this release. The Adjust entry's
wire request and landing are proven on a loopback endpoint and by driving the
GUI's landing path; the first real call is the user's. The GUI executable was
not launched. The ship facts (the release run, the downloaded assets, the site,
the local upgrade) are in the ROADMAP ledger entry.
