# AutoShade v1.3.4 — a denoise dial in each fold, a luma/chroma strength, and the reverse-fit reads the negative

Three sentences from one report (2026-09-13): AI denoise needs a strength
control; after an AI denoise the Original card shows the denoised pixels but a
reverse-fit is built on the un-denoised RAW; and the denoise result itself
needs to improve. The user's own develop store showed the second one exactly:
the 「▣ Original」 card's `origin` was the full-resolution denoise master, the
active 「◭ Reverse-fit」 card had no origin, and there was no `pixels.json`.

## The reverse-fit and the reimagine read the negative

A photo is one negative and N cards. An in-place retouch — a denoise, a heal,
a clone, a fill — bakes a master and makes it the ▣ card's pixel source
(`origin`); that card develops, exports and retouches from it. The reverse-fit
and the reimagine spelled the negative differently: as the file on disk. The
fit's source frame came from `pipeline::fit_source(src_path)`, the ◭ card it
landed as had `origin: None`, the fit worker cleared the pixel link, and the
reimagine sent the RAW's own pixels to the model. After a denoise on the ▣
card, the fit was therefore solved on, rendered from and exported from the
noisy sensor frame while the card beside it showed the clean one.

One definition now: `AutoShadeApp::negative_origin` is the ▣ card's master
and `negative_path` is that or the loaded file. The reimagine develops
`negative_path`. The reverse-fit captures the master once at the click: its
source frame is the master loaded through `render::source_pixels` at the fit's
working edge (a neutral develop already; the photo's calibration still
composes on top, exactly as the ▣ card renders it), the commit's pixel link
records the master (`inplace`) instead of clearing, and the outcome carries it
to the landing, where the ◭ card hangs off the master, shares the ▣ card's
decoded pixels, and the ● pixel mirror follows the persisted link. A reopen
restores the pixels the fit was solved on. Nothing in the persisted formats
changed — the strip reader already built a card from its kind and its pixels
arm independently — and a photo whose ▣ card carries no master takes exactly
the previous path.

## Strength: a dial in each fold, and what the number means

Measured on the user's 61 MP ISO-640 frame, the shipped strength (1.0, the
model's whole output) keeps 1–7 % of the frame's high-frequency energy across
every SCUNet tier the sidecar ships (luma HF std 2.27 → 0.12/255 for
`color_real_psnr`; `color_real_gan` 6.5 %, `color_15` 3.0 %, `color_25` 1.7 %,
`color_50` 0.9 %) — the rock texture went with the noise, so a model tier is
not the fix. Feeding a brighter input does not change it (6.7 %). Tile seams
measure 0.06/255 against a single-tile run and stay as they are.

A plain blend at 0.5 keeps the colour speckle (chroma HF 2.26/255). The
strength is now a luma/chroma split, `python/denoise.py::blend_luma_chroma`:
the luminance (BT.709) is blended by the value and the model's chroma is
taken at min(1, 2·strength) — colour noise, the ugly half, goes first and is
gone in full from 0.5 up; luminance grain returns linearly and brings the
texture that lives at the same frequencies with it (at 0.5: luma HF 1.15/255,
chroma HF 0.07/255). 0 is still the identity and 1 still the model's whole
output. `denoise::DEFAULT_STRENGTH = 0.5` is the one default: the CLI's
`denoise --strength` and `auto --denoise-strength`, the web export's
`denoise_strength`, both desktop dials, and the sidecar's own `--strength`
(pinned to the constant by a test).

The desktop app follows the 2026-09-12 rule that a control sits in the fold
whose verb reads it, and a function two folds need gets two controls:

- The Detail fold's **AI denoise strength** dial sits directly above
  **🤖 AI Denoise now** and is the only strength that verb reads.
- The Export fold's **Export denoise strength** dial sits directly under
  **🤖 AI Denoise on export**, enabled with the checkbox, and is the only
  strength the export-time denoise reads; the export summary echoes it
  ("AI Denoise 50%").
- Each has its own state and its own preferences key; a preferences file
  written before the keys existed loads both at 0.5 — never at serde's 0.0,
  which would be the identity. Chinese pairs for the four new strings and the
  two changed tooltips; the embedded SC subset learns 岩 折 石
  (`subset_gui_fonts.py --check` 877/877).

## Compatibility

- **Defaults.** A scripted `autoshade denoise` or `auto --denoise` without a
  strength, and a web export with `denoise` and no `denoise_strength`, now
  denoise at 0.5 where they denoised at 1.0; pass `--strength 1.0`
  (`--denoise-strength 1.0`, `"denoise_strength": 1.0`) for the previous
  output. Explicit strengths between 0 and 1 render differently from v1.3.3
  (the split law); 0 and 1 render the same bytes.
- **Store format.** Unchanged. A ◭ card may now carry an `inplace` pixel link
  and an `origin` in `variants.json`; earlier builds read both fields already.
- **Sidecar contract.** `--strength` keeps its name, range and default
  position; only its law changed. The download, verification and acceptance
  paths are untouched.
- No renderer, solver or recipe schema change: the reference pair renders to
  the same bytes (Gates).

## Gates

Measured before the tag on the release code (`f6d7af3`; the version bump
touches Cargo.toml, Cargo.lock and the documents only): library **1484
passed / 0 failed / 15 ignored** (1499 enumerated, test profile, 436.93 s),
CLI **24 / 0**, GUI **186 passed / 0 failed / 1 ignored** (the gui feature),
python `test_denoise` **11 / 11** (five new blend-law tests), clippy 0 on both
feature sets, `audit_i18n` 0 / 0 / 0, `subset_gui_fonts.py --check` 877/877,
`cargo metadata --locked` clean, `check_docs.py` 25 PASS / 0 FAIL / 5 SKIP
(the skips are the count claims and the census, which only the battery
transcript and the census root can prove), photo-name grep 0. By name against
the v1.3.3 tag (`f5046c9`): +4 / −0 (1710 → 1714 test functions) — one library
pin and three GUI pins, listed in ARCHITECTURE's counts note. Mutation: the ◭
card ignoring the negative's master turns the landing test red. The sidecar
end to end on the user's crop: 1.0 → luma HF 0.12 / chroma 0.07, 0.5 → 1.15 /
0.07, 0.25 → 1.71 / 2.26 (per 255), and a run with no `--strength` is
byte-identical to 0.5.

Final gate, reference pair, before the tag: the 1.3.4 CLI (built in its own
target directory, `--version` 1.3.4) re-rendered the 0.85 reference develop
at full resolution — **0 of 60,217,344 pixels** differ from the v1.3.3 CLI's
render of the same develop; downscaled to 2048 px it sits at mean |diff|
0.00044 / max 0.008 against the R37 acceptance render, the same numbers as
v1.3.3 and v1.3.2; 0.65 / 0.85 / 1.0 at 2048 px are pixel-identical to the
acceptance renders (0 of 2,795,520 each; 0.85: sky ΔE 4.9, |ΔL*| 0.6, L*
spread 1.03, land ΔE 6.9), and the three crops were viewed beside the
target: no seam, no rectangle. The solver and the renderer did not change;
the pixels prove it.

Recorded after the tag: the three-lane release battery
(`scripts/release_battery.sh`, the p36–p41 calibration corpus and the sidecar
weights in reach) ran on the `f6d7af3` snapshot before the tag (the tagged
code minus the version literal and the documents — `git diff f6d7af3 v1.3.4
-- src tests python assets scripts` is empty) and finished green: library
**1484 / 0 / 15** (742.83 s, release profile, one process per module),
CLI 24, contract 2 + 2, doc-tests 0, GUI **186 / 0 / 1**, calibration lane
**1484 / 0 / 15** (1162.86 s; one skip line, the mask-brush specimen
test whose `AUTOSHADE_MB_SAMPLE_ROOT` specimen is not on this machine),
`audit_i18n` 0 / 0 / 0 and the font check 877/877 inside the battery, 1499
library names enumerated (+1 / −0 against the v1.3.3 transcript; GUI 184 →
187 by name, +3 / −0), and `check_docs.py --gates` on the transcript with the
XMP census root supplied **30 PASS / 0 FAIL / 0 SKIP**. The ship facts (the
release run, the downloaded assets, the site, the local upgrade) are in the
ROADMAP ledger entry.
