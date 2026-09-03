# AutoShade v1.2.4 — the clearing release

Every item that any earlier release had registered as a follow-up, a deferred
measurement, a compatibility layer "kept one version" or a sentence to be
re-measured later is closed in this release: shipped, measured, or written down
as a final ruling with its reason. `docs/ROADMAP.md` is now a ledger of what
each release shipped and the numbers it measured; it holds no planned work.
This document is long because the release is: each section names what was
open, what was done, and the number that decides it.

## The cast projection takes the best-paying shrink, and the finished frame is read for a fan

v1.2.3 introduced the hue-fan gate and the projection that shrinks a convicted
cast toward the shape its three curves share, and stated its cost: the gain
was evaluated at exactly ONE point, the strongest admissible shrink, so a
shrink that paid only at a milder `t` was not found. That is closed. The
search is now a bisection for the admissible edge, an eight-cell sweep of the
admissible interval and eight golden-section steps, with the gain bar applied
to the MAXIMUM over the admissible set. The pair that registered the cost
ships at `t = 0.318` with a look-error ratio of 0.885 and a finished residual
of 0.0256 → 0.0227 (under the quantisation floor, so its rationale moved to a
wider-gap arm); every other calibration pair is byte-identical.

v1.2.3 also said the gate was "a calibrated threshold, not a structural
guarantee", because the do-no-harm loop re-fits the curves after every shrink
and could in principle walk around the gate. The finished render is now
re-read with the same census after the loop: when the curves are the cause
they are withdrawn, and when they are not the reading is disclosed (two new
rationale keys, in both languages). Measured over 108 finished Full renders the
widest ADDED fan is 14.23° against the 15° line, so no shipped recipe changes;
p36 delivers a 12.9° fan with no cast at all, which is why the disclose-only
arm exists.

Two real mixed-lighting photographs from the photographer's own library join
the calibration corpus as `p40` and `p41` (P-codes, no file names), answering
v1.2.3's "no such photograph is in the corpus": the gate costs both nothing —
13.5° and 9.0° of added fan against 15°; `p40` is admitted as fitted
(0.078836 → 0.033481, confidence 0.611693) and `p41` is refused by the re-hue
veto, not by this gate.

**Four censuses re-measured, and two of them were wrong.** The aggregate-ratio
arm of the colour stage fires on 0 of 545 attributable stage runs (111 admitted
with curves, 29 by projection, 265 refused by the rotation budget alone, 67 by
the hue fan alone, 44 by both, 29 with no curves to judge), so its decision is
pinned as a pure function. ">1.75× neutral-share inflation remains
synthetic-only" was false: six of the seven corpus pairs exceed it (`p40`
reaches 25.1×). The rotation-visibility exemption can hide up to 3.41 % of a
frame turning past 75°, which is 0.68 of the rotation-share budget, and the
rustdoc now says so. The 4b do-no-harm rescue's SUCCESS arm is unreachable on
this tree, and the sentence that used to ask for a fixture now carries the
measurement instead: the loop body runs 186 times across the battery (107 on
the error arm, 140 on the hue-guard arm, 62 on both), 9 re-fits hand back a
fan-convicted cast, and all 9 are also rotation-blocked, so none earns a
projection; about forty constructed pairs reproduce the coupling. The
projection arithmetic is pinned where it lives instead.

**Two things measured and kept as they were.** `CONFIDENCE_SLOPE = 6.0` is a
measured calibration: `p36` is the only pair where it decides, and it is right
to the last digit (1 − 6 × 0.105034 = 0.369796 against the reported 0.369798).
An exposure solve on the shared-content basis — the per-pixel log2 luminance
ratio over the population the white balance already uses — was implemented,
measured and reverted: it buys nothing on the two pairs that reach it
(`neutral` 0.188686 → 0.099869 and `p37` 0.247107 → 0.161220 either way),
moves the canyon pair from −0.28 EV to −0.22 EV, and stops
`flat_sky_to_cloud_deck` fitting at all (0.1619 → 0.1619). The reason the two
statistics agree on white balance and disagree on exposure is written at the
solve. Also: the two `NA` clauses of the projection's rationale are rendered
and asserted in both languages, two dead helpers are gone, and a synthetic
Full-mode pin plus a band-centroid disclosure test close the last two
registrations of the HSL batch.

## The zoned fit abstains where it cannot measure, and a tile keeps only what its gate left

`structure_divergence` now returns nothing at all when the analysed core is
under 100 px or the two geometries disagree, instead of manufacturing a
"matched" verdict, and every consumer treats the abstention as what it is: a
local-field cell reads no support, a tile or a free mask is refused with a
named reason, and the structural penalty charges nothing. The attachment gate
reads one frozen, render-independent share ruler. A spatial tile reserves only
its raster times the strength `k` its boundary gate accepted, shares the
64-pixel footprint floor with the free masks, and what the accepted-tile filter
removes is disclosed with its pixel count and both shares
(`FIELD_MASK_WITHHELD`). The range gate adds a tone-order sign test — the
delivered tone order may fall back by at most 0.5/255 luma across the band; on
the real range pair it reads 0.0001 against 0.0020 allowed and changes nothing —
and its rim probe was re-cut to 64×1020 so every ramp the producer emits is
measured (n = 64/64 in all 90 cells; the table is in the rustdoc). A zone whose
population covers fewer than two of the eight tone knots says so
(`ZONE_TONE_UNSUPPORTED`) instead of solving nothing silently. A per-tile
evidence cache serves 33 of 50 reads on the calibration pair.

Measured on the six-pair corpus with the sidecars live: three pairs gain one
free mask and a lower final residual (0.089205 → 0.084185, 0.096174 → 0.092254,
0.029263 → 0.028869), three are byte-identical, and the semantic-zone gate
readings are identical in both arms. The soft-rim half of the boundary ruler was
measured at last on the three pairs that attach a semantic zone (745, 691 and
793 transitions; rims of 0.029 and 0.040 shrunk to the budget at k = 0.385 and
0.271, a rim of 0.006 kept whole): the constant stands, and no perceptual study
is owed. A source census pins the twelve GUI budget wiring sites and requires
each to be a named binding; a thirteenth fails the test.

## The reverse-fit gains a second range family, partitioned by colour (W5)

The ROADMAP had ruled the colour-range producer "not to be built"; the
clearing order reversed that, and it is built inside the existing range
family rather than beside it. With segmentation off, the zoned reverse-fit
now runs two stages: the luminance bands as before, then — on the frame the
luminance stage actually left behind — `derive_colour_bands` walks the eight
ACR hue bands the evidence model already carries. A band is proposed only
when its residual clears the observed matched domain, and it must clear four
independent guards against the circularity a hue-shifting edit creates by
emptying its own band on the target side: the frozen evidence verdict must be
two-sided, both delivered populations must clear the shared 1.5 % floor, the
attachment re-scopes the evidence over the mask's own population, and — since
ONE mask is keyed to the source band and read on BOTH frames — a move large
enough to carry the target's pixels out of that mask leaves the two shares
past the 2:1 composition gate and is refused rather than fitted. A surviving
band becomes a `RangeMask::Color` keyed to its members' weighted mean colour
at their p90 chromaticity radius, stopped before neutral, persisted as a
**Colour range NN** card and written to the sidecar as the mask Lightroom
itself writes (`Type="1"`, `crs:ColorAmount`, one `PointModels` sample —
checked element for element against seven genuine sidecars in the reference
library, which also confirmed the trailing `0` the writer had assumed).

Two defects in the family it grew from were found and fixed on the way. The
boundary gate shrinks everything it holds by one bisection `k`, so a single
gate over both families let a colour band's edge crush the luminance bands
beside it (p37: `k` fell to 0.003 and the composed frame went 0.15915 →
0.16474, invisible to the frame ceiling because that ceiling compared against
the residual the fallback was handed); each family is now a stage with its own
shrink and its own entry ceiling. And a shrink whose render is byte-identical
to the frame without it is refused (`RANGE_BOUNDARY_INERT`) instead of
shipped — p39 had kept a mask with every dial at zero. The rim gate reads
each band in its own coordinate (signed luma for a luminance band,
chromaticity distance for a colour band) and the luminance readings are
pinned unchanged across that split; the tone-order test stays a luminance
reading, because chromaticity distance from a band's own colour is a radial
coordinate that a rigid colour move inverts by construction. Measured on the
calibration corpus with the colour arm on against off: p36, p37 and p39 gain
1, 1 and 2 colour bands (0.03073848 → 0.03063063, 0.15915467 → 0.15905777,
0.02327213 → 0.02313107), `neutral` and p38 are byte-identical, and no pair
regresses at a tolerance of exactly zero.

## The Lightroom pack: 46 exports measured, two laws corrected

Forty-six Lightroom exports of one RAW — five groups, every one a single
attribute away from its reference — were rendered through the production
import and develop path (`render::lr_pack`) and scored by
`scripts/lr_mask_parity.py` against Lightroom's own COVERAGE, the tone
coordinate recovered from the pack itself, rather than against exported luma.
That change of ruler is what overturned a shipped fit. Two laws changed:

- **The LINEAR falloff.** Lightroom's half-coverage sits at t = 0.5436 of the
  handle span, not ½. The `Eased` smoothstep shipped in v1.2.0 had been fitted
  to exported luma — the tone curve composed with the mask — which keeps the
  S shape but hides where the middle is. The falloff is now the same C1
  smoothstep on the warped abscissa `t^1.124`: pooled α rms 0.0293 → 0.0074,
  the half-coverage contour +34.2/+38.2 px → +0.9/+5.0 px on an 832-px span
  (candidate laws against Lightroom's coverage: warped 0.0064, plain
  smoothstep 0.0315, raised cosine 0.0323, linear 0.0598). **Every LINEAR mask
  rerenders**: max |Δα| = 0.0624 at t = 0.46, the half-coverage line moves
  3.97 % of the span toward Full; radial, bitmap, brush and AI masks are
  byte-identical.
- **The radial decode's guard.** A radial whose stored corners fold to a
  negative semi-axis was refused at import; Lightroom draws the magnitude
  ellipse. The reader now refuses only a degenerate ellipse (a zero or
  non-finite axis). On the pack's nine tilted masks: 0 of 9 imported → 9 of 9,
  α rms 0.3931 → 0.0069. **Radials that used to be refused now import.**

And four things measured rather than changed: the radial boundary is a pure
0.99876 scale of the stored ellipse (sd 4×10⁻⁵ across three sizes, no
dilation law — the "R2 big-mask excess" is closed); interpolated feather
columns score like measured ones, so the 290×11 table keeps its columns;
Roundness moves no pixel (max |Δ| = 0 DN over 26 Mpx at three feathers on a
tilted 2:1 ellipse — the earlier "±4 DN dither" note was wrong); and the mask
eye is exact on both sides (`CorrectionActive="false"`, 0 DN). The `.lcp`
alternative for the lens arm was measured and is 2.0–4.4× worse; it stays off.

## The pre-rename names are gone (behaviour change)

The pre-rename `AUTOSHOP_*` environment variables no longer answer. Since
v1.2.0 every one of them aliased its `AUTOSHADE_*` twin and warned once; that
door is now closed, so set the `AUTOSHADE_*` name. `AUTOSHADE_DENOISE_CACHE` is
gone with them — the knob that names the weights directory is
`AUTOSHADE_WEIGHTS_DIR`, as it has been since v1.1.

Your settings file is handled differently, because it holds your API keys: a
pre-rename `autoshop.local.json` in the data folder is RENAMED once, on the
first launch that sees it, with every key and choice intact, and the app says
so. If both names are present the current one is used and the old one is left
untouched. If the rename fails, this session still reads the old file — nothing
is lost — and the next launch tries again.

One token from the old name stays, on purpose and permanently: the
`MARK_PRE_RENAME` marker that this app's earlier builds wrote into sidecars.
It lives on disk in files this build did not write and has no expiry; failing
to recognise it would leave a stale rationale on a new recipe, which is exactly
the defect the rationale refresh exists to prevent. Both spellings of
`x:xmptk` stay for the same reason, as recorded in v1.2.0.

Both rows the v1.2.0 notes promised — the `AUTOSHOP_*` variables and
`autoshop.local.json`, each "removed in a later release" — are therefore
closed here: the first by removal, the second by a one-shot rename.

## Style retrieval: tags against the library mean, and the short-direction re-test

**Tags are derived against the library, not against a fixed vocabulary.** Each
exemplar's four-tag summary now picks each group's winner by how far it stands
ABOVE the library's own mean for that phrase, so a caption every photograph
scores highly on cannot own a tag everywhere. Measured on the photographer's
own library (169 RAW exemplars + 94 finished looks), before → after: the tag
list changed for 168 of 169 exemplars and 92 of 94 looks; the most common
caption fell from 52 % to 21 % of exemplars and from 43 % to 27 % of looks;
captions ever used went from 30 of 33 to 33 of 33 (exemplars) and from 25 of 33
to 33 of 33 (looks). Tags feed the description text the ranking reads, so this
is a ranking change: the index is written at version 6, versions 4–6 are all
readable, and every index this build reads has its tags re-derived on load.
The one upgrade cost, measured: 92 looks lose a description vector that was
computed from the old tag text (0 RAW exemplars do); it is recomputed on the
next index build.

**The short-direction re-test.** The text weights had been calibrated on
proxies shaped like whole descriptions, not like the few words a user types.
The harness gained a third proxy — twelve typed-length directions, each
exemplar assigned the one its own look tags answer best — and the numbers are
on a corpus that now carries 50 settings keys per exemplar where the S2 grid
saw 12, so the pooled baseline is 0.425385 and only within-run differences
mean anything. The settings objective prefers a bigger image-text weight
monotonically (`W_TXT` 4: MAE 0.382168) — at which one exemplar answers
97.0 % of 169 different photographs for a given direction. The objective
cannot see that collapse: the proxy direction is assigned from the held-out
exemplar's own tags, so any weight that retrieves the query's own attributes
harder scores better for doing less. Both weights are therefore pinned at
0.5 and the harness's own recommendation is refused in the rustdoc that
reports it: `W_TXT` 0.5 costs nothing measurable (+0.000447, paired 95 % CI
[−0.006972, +0.007980] against no text at all) and is the largest weight that
leaves the corpus open; `W_DESC` 0.5 costs +0.013643 (CI [+0.006785,
+0.020881], 3.2 % of the baseline) and buys the direction separation that is
the Direction control's only other mechanism — opposite directions share a
top-1 46.9 % of the time with it and 60.7 % without, with 167 of 169 exemplars
still reachable. The raw-versus-standardised head-to-head that S2 could not
separate now separates: best raw 0.410916 against best standardised 0.377820,
paired CI [+0.021870, +0.043719]; the raw path's switch is gone rather than
kept as an untested second ranking.

**fp16 embeddings, measured instead of argued.** Against an fp32 twin of the
same 169-exemplar library: `1 − cos` at most 2.235e-5, the top-5 identical for
168 of 169 leave-one-out queries, no retrieved SET moves at all; the one place
the drift is visible is the tag argmax, which flips a phrase for 6 of 169. With
descriptions off it moves the top-5 set for 6, 5 and 31 of 169 on the three
showcase directions. The default stays fp16.

**Mixer hue stays ingested and never distilled**, on measured reach: the
per-band hue axes explain 1.2–7.8 % of scene variance (η²) against 1.1–5.4 %
for the axes that are distilled, the gate would fire on 8–49 % of neighbour
sets, and targets reach |55.75|; the data is still ingested, so the question
stays answerable from the index.

**Also in retrieval.** A black-and-white neighbour no longer drags a colour
band target (the real library has 0 monochrome exemplars, minimum saturation
−26, so the invariant is pinned on a synthetic index: +40 versus +30); a hint
keyword is read as a word, not a syllable (`sky` no longer fires inside
"whisky", `chroma` inside "monochrome", `satur` inside "saturation"), and the
same rule exposed that "desaturate the blues" returned nothing because the
prefixed spelling was in one list and not the other — both lists are now
constants with a test that fails if they drift; the description cache is one
cap shared between the RAW and look libraries instead of two libraries
evicting each other; a shared stem is disambiguated by its folder; an
interrupted index build's staged frames are swept by the next; and
`style-query --distil` prints the distillation a develop would apply — every
channel including the ones it refuses, the Style pull, and how many neighbours
are black-and-white.

## One sidecar door, one full-resolution slot

Four bridges spelled the Python-child preamble out by hand — denoise,
segmentation, the segmentation backend probe, and the single-artifact model
executor — and a hand-copied preamble is one that drifts silently. They now
share one door: the environment allowlist, captured stdio (the windowed GUI
has no console to inherit), no console flash, and a kill group so a timeout
takes the torch workers with the launcher. Denoise lost 54 lines and
segmentation 85 to it.

The full-resolution admission slot moved from the server into the library.
Heal/clone and the generative fill had never taken a permit at all; they do
now, and the permit is scoped to the local compositing phase rather than held
across a generative API call that can run for minutes.

## Diagnostics finished, disclosures typed, the ceiling measured

The diagnostics sweep is complete: every disclosure a stage can make reaches
the sink, none is a bare `eprintln!`, and the provider's own repairs — the
8-band colour-mixer axis that OpenAI's strict mode cannot bound — are typed
notes the GUI translates instead of English prose in the rationale prefix. The
rationale ceiling rose from 16 KiB to 64 KiB on a measurement (the 64 longest
templates are 16,183 bytes of template alone) and truncation now cuts from the
front, keeping the newest sentences. The default recipe is pinned
byte-for-byte to a fixture (with a `.gitattributes` LF rule so an autocrlf
clone cannot make the test lie). An unmarked cached alpha is kept when this
machine cannot replace it. The disclosed style pull is the one the blend used.
The strict-output field-order probe was run live: the provider emits fields
ALPHABETICALLY, not in declared order (`recipe` first at bytes 194/230,
`scene` at 1604/2527, matching the earlier 170/1269), and the assertion now
encodes the measurement.

## The case rule is the volume's, not the platform's

Whether two spellings of a path are one file is read off the volume that
holds it (`store::volume_folds_case`), not off the operating system: a
case-sensitive volume on Windows and a case-insensitive one on macOS are both
real. `same_path` uses it; the photo key deliberately does not.

## CI: a pinned compiler, a Linux archive, Metal measured

The compiler is pinned in the repository (`rust-toolchain.toml`, channel
1.94.1). Every job that builds a published binary asserts that the runner
honoured the pin before it compiles anything, and `scripts/check_docs.py`
compares ARCHITECTURE section 5's sentence against the pin with the patch
digit included — so the version in the docs and the version that built the
download cannot drift apart again.

A Linux x64 command-line archive is published for the first time:
`AutoShade-<version>-linux-x64.zip`. It carries the same payload as the macOS
CLI archive — the binary, the Python sidecars without their weights, the
assets, LICENSE and README — and is built on Ubuntu 22.04 so it runs on the
widest range of distributions. There is no Linux desktop app.

The Metal path is measured rather than described. The macOS battery runner now
reports which device `python/_device.py` chose, the wall time of a fixed
forward pass, the peak device memory, and whether
`torchvision.ops.deform_conv2d` runs natively on Metal or falls back to the CPU
— asked in a separate process with `PYTORCH_ENABLE_MPS_FALLBACK` removed,
because with the app's own setting in place the fallback is invisible. The
numbers appear in the run's job summary. The v1.2.4 release run measures it on the macOS runner (the `macos-battery` job runs `scripts/mps_probe.py`), and its numbers are copied into this paragraph by the post-release refill commit, the same commit that fills the asset table from the published bytes.

The workflows moved to the current major versions of the GitHub-provided
actions (checkout v7, setup-python v7, upload-artifact v7, download-artifact
v8), which are the Node 24 builds.

## The installer upgrades in place, and uninstalling is a question

Running a newer `AutoShade-Setup-<version>.exe` over an existing install keeps
it one install: the same directory, one entry in Programs and Features, one
user `PATH` entry, shortcuts replaced rather than duplicated, and the develop
store and the downloaded weights untouched. A running AutoShade is closed
first, and an older installer is refused with both versions named.
Uninstalling — from Programs and Features or from the Start Menu group — asks
whether to delete the two things the installer never installed, the weights
and the develop store, names the size of each, and keeps both by default;
`/DELETEDATA=1` answers yes for a silent uninstall. Every dialog in the script
is a `SuppressibleMsgBox`, which is what makes `/SUPPRESSMSGBOXES` honoured
throughout — a plain `MsgBox` would have hung a silent run. The chain is
measured rather than asserted: `scripts/installer_scenarios.ps1` installs,
upgrades, refuses a downgrade, uninstalls and re-installs under a throwaway
AppId (107 assertions, the real install untouched), and the new
`installer-upgrade` workflow runs the previous published release against each
build on a clean Windows runner. Its first run paid for itself: the runner's
user PATH ends in `;`, and the uninstaller's cleanup — a split-and-rejoin —
handed it back one byte shorter. The uninstaller now deletes exactly the span
setup wrote (setup records whether it wrote the separator), and the chain runs
under three PATH shapes. Over an install made by an earlier installer, which
recorded nothing about the separator, the uninstaller takes the entry alone:
exact for the Windows default user PATH, at worst one trailing `;` otherwise.

## The diagrams are generated, checked, and operable

The architecture diagram and the three pillar diagrams are SVG emitted by
`scripts/architecture_diagram.py` and `scripts/pillar_diagrams.py` from
`docs/architecture/autoshade.architecture.json`, which now carries all five
sidecars (20 components, 19 connections, 3 boundaries); no position is chosen
by hand. Both generators draw on one canvas, `scripts/diagram_check.py`, that
records every text run, box and connector as it draws and refuses to write a
file in which any two touch. Measured that way, the v1.2.3 pillar SVGs had a
label on a box, a line through a caption and an arrow that ended in empty
canvas. Text widths are Chrome's own measurements of the page's font with its
kerning pairs, plus a margin for the other platform's face. On autoshade.dev
the four figures zoom and pan — wheel, drag, pinch, keyboard, full screen, and
`architecture.html` is the whole-page version — through one same-origin
script, for which the site's content-security policy moves from
`script-src 'none'` to `'self'`. The archify HTML and the PNGs are gone.

## The release battery is one command

`scripts/release_battery.sh` runs three lanes in parallel — the default
battery, the GUI binary, and the library a second time with the calibration
corpus — each in its own target and data directory. It refuses to start when
the corpus is absent, because a battery whose corpus-gated tests skip is green
for the wrong reason; every skipped test prints one line with one spelling,
and the script counts them; it prints the by-name test-set difference against
a saved baseline. Its first run found a test that asserted the argv list only
an unconfigured machine produces, which is fixed. Its library lanes run one
process per top-level module: an endpoint-security sensor on the release
machine (CrowdStrike Falcon) terminates a single process that runs the whole
suite — its own event log says so at the second of every one of eleven
deaths, exit status `0xE0000027`, and any module alone passes (37 of 37) —
so the script partitions by the harness's own module list, runs every name
exactly once, and sums the counts by suite. Nothing is skipped and nothing
is retried.

## A rotation made in Lightroom survives import (W14)

Lightroom keeps a photograph's current orientation in the sidecar's
`tiff:Orientation`, and that value — not the RAW's EXIF tag — is the frame it
delivers: the me6-2026-09 pack's capture carries IFD0 `Orientation = 8` while
all 46 sidecars declare `1`, and Lightroom exported 6240 × 4160 landscape from
every one of them. AutoShade rendered all 46 as 4160 × 6240 portrait, and a
rotation made in Lightroom did not survive the import at all. The reader now
solves `compose_orientation(EXIF, quarter_turns) == tiff:Orientation` for the
turn (`render::quarter_turns_between`) and the writer runs the same equation
forwards, so the delivered frame is Lightroom's and a turn made in AutoShade
reaches Lightroom — through the one door the CLI, the GUI and the XMP round
trip all save by. Every α measurement of the pack is reproduced byte for byte
(92 of 92 rasters); the 174-sidecar reference library has 0 sidecars whose
declaration disagrees with their capture, so nothing in an existing library
moves — except the 17 portrait sidecars that declare an orientation without a
rectangle, whose masks were a quarter turn off and now are not (154 of the
174 have that shape; the declaration alone places the geometry). A mirrored
declaration (2/4/5/7) over an un-mirrored capture is refused by name instead
of being mapped to the nearest rotation. The merge corrects a sidecar's
`tiff:Orientation` only when the photographer's turn has moved away from it,
only where this engine's own reader will read it back (the Description that
declares the frame, or the whole document), and with its namespace binding
when the tag lacks one; anywhere else the base's declaration is kept and the
loss is named.

## Documents

`docs/ROADMAP.md` is a ledger: every entry is a shipped release with its
measured numbers or a final ruling with its reason; the 1,156-line plan it
replaced is in `docs/ROADMAP-archive.md`. Every place that linked it as
"planned work" now says what it is. `CONTRIBUTING.md` states what a change has
to carry before it merges. The site's edge cache is purged after every deploy
(the image cache keys stay). The README says its facts as key points — every
pillar is a lead sentence, a few bullets and a `Details:` link into the
document that holds the derivation — and is 942 → 732 lines lighter for it;
every number that left it is stated where the section links, and the sentence
about the retrieval weights now says what `src/style.rs` ships (`W_EMB = 4`,
`W_TXT = 0.5`, `W_DESC = 0.5`, and `W_LOOK = 1.0` as the one unmeasured term).
The "designed, not yet shipped" list is empty for the first time, and says so;
the status table and every pinned test count are re-derived from the release
battery's transcript by `scripts/check_docs.py --gates`.

## Upgrading

Install over v1.2.3; the installer upgrades in place, and uninstalling now
asks whether to keep the weights and the develop store (it keeps them unless
you say otherwise).
Three things change on purpose. The `AUTOSHOP_*` environment variables no
longer answer and `autoshop.local.json` is renamed once (see above). The style
index is rewritten at version 6 the next time it is built, and an existing
v4/v5 index is read with its tags re-derived on load — retrieval order changes
for the photographs whose tags moved (168 of 169 in the photographer's own
library). Four more changes are in what a develop produces, each measured in
its own section above: every LINEAR mask re-renders on the `t^1.124` falloff
(max |Δα| 0.0624 at t = 0.46; radial, brush, bitmap and AI masks are
byte-identical) and a radial whose stored corners fold to a negative semi-axis
now imports instead of being refused; a sidecar's `tiff:Orientation` chooses
the delivered frame, which turns the masks of the 17 portrait sidecars in the
reference library that declare an orientation without a rectangle (0 of 174
declarations disagree with their capture, so nothing else moves); the zoned
reverse-fit may attach colour-range bands where it attached none before
(p36, p37 and p39 of the calibration corpus gain 1, 1 and 2; `neutral` and p38
are byte-identical) and may add a free mask (three corpus pairs do); and the
cast projection re-ships one calibration pair at `t = 0.318` while every
other pair is byte-identical. Everything else a v1.2.3 develop produced, this
build produces byte for byte.

## Verification

The release battery ran once, on 2026-09-02, on `7a4d42e` plus the documents
and the battery script that became `df9a91d`; the bump commit that follows
changes version literals and documents only and was not re-run. Three lanes
in parallel, each in its own target and data directory, `--offline
--release`:

- **default**: library 1381 pass + 14 `#[ignore]`d = 1395, run as 37
  processes (one per top-level module, 348 s summed); CLI 24; contract
  suites 2 + 2; doc-tests 0. Exit 0.
- **gui** (`--features gui --bin autoshade-gui`): 164. Exit 0.
- **calib** (the library again with the calibration corpus and
  `--nocapture`): 1381 pass + 14 ignored, 0 skip lines. Exit 0.

By name against v1.2.3 (`f3885b2`, 1332 names): +70 / −7. The seven that
are gone are the six `config::` tests of the retired `AUTOSHOP_*` alias door
and `segment::tests::an_unmarked_cached_alpha_is_left_alone`, whose
replacement `…_is_kept_when_this_machine_cannot_replace_it` is among the
seventy.

`cargo clippy --all-targets` in the default and `gui` feature sets: 0
warnings on `7a4d42e`, the last commit that touched Rust. `check_docs.py
--gates` over the transcript with the census corpus: 30 claims, 30 pass.
`audit_i18n.py`: 795 literal + 213 dynamic keys, 997 zh entries, every check
at 0. `subset_gui_fonts.py --check`: 875 of 875 glyphs embedded. The staged
diff of every commit was grepped for photograph file names: 0. CI on `main`:
the `build` workflow's test jobs pass on every commit from `0d3cd0d` to
`a113f66`; its debug-profile library job (about four hours a run) passed on
`8ff3260`, and the later runs were still in progress when this was written.
