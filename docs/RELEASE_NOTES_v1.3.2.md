# AutoShade v1.3.2 — the reverse-fit's Strength lives in the reverse-fit fold

One change, made because a user could not find a dial. The showcase captions
quote fits "at Strength 85 %" and "at Strength 100 %", and through v1.3.1 the
only Strength dial on the AI panel sat in the *Analysis* fold, two folds above
the Reverse-fit button that silently read it (F1's single `panel_strength()`:
"one reading, so the two cannot drift apart"). Asked where the 85 % is set, the
answer was "the slider under a different heading", which is the answer this
release removes. The ruling behind it, verbatim: a control belongs to the
section whose function it serves; a function two sections need gets two
controls, never one shared reading.

## The dial

The Reverse-fit fold now carries its own **Reverse-fit strength** slider,
directly above the 「Reverse-fit recipe」 button, on the same helper as every
other dial in the app (a 0..100 track, double-click to reset, hover nudge, the
explanation as the tooltip). Its state is `AutoShadeApp::fit_strength`, its own
prefs key `fit_strength`, its default the library's `GradeStrength::DEFAULT`
(0.65) — a prefs file written before the key existed decodes to that
byte-identical budget, never to serde's 0.0. Moving it lights the AI area's ●
like every other AI input. The tooltip names the three bands the budget has
always had: at or below 65 % the fit is byte-identical to the calibrated path;
above it the Atmosphere budget widens and a white balance outside the budget
shrinks along its fitted move instead of staying as-shot; from 85 % unsupported
movement is disclosed with the confidence capped instead of withheld.

`panel_strength()` is gone. `analysis_strength()` feeds the analyze request and
nothing else; `fit_strength()` feeds both reverse-fit entry points (zoned and
global) and nothing else. The GUI test that pinned the shared reading is
rewritten as `gui_reverse_fit_has_its_own_strength_dial`: one default in three
places, the two dials provably apart (a moved Analysis Strength leaves the
fit's veto policy at Withhold; a moved fit dial leaves the analyze request at
its default), the label drawn in the fold at the default 320 px panel width,
the prefs round trip with the older-file case, and the worker's source pinned
on both entry points.

## Words that moved with it

The button tooltip, the CLI's `match --strength` help, the manual's two
passages, the README and showcase captions, the site's two figcaptions and the
architecture note on the strength axis now say "Reverse-fit strength" where
they said "panel Strength". The library's own rationale sentence — "Reverse-fit
used panel Strength N % to derive its honesty budget" — is unchanged on
purpose: `fit.rs` recovers the strength a report was solved at from that
sentence, and a respelling would make every saved rationale unreadable to it.

The Chinese tooltip adds five hanzi to the embedded font subset;
`NotoSansSC-autoshade.ttf` was regenerated (217,760 → 219,476 bytes) and the
four other subsets kept as committed, their glyph sets being unchanged.

## Recorded since the v1.3.1 notes closed

Two things landed on `main` between the two tags and are part of this
release's tree: the third real Lightroom 9.4 fixture pair — a v1.3.1 sidecar
itself, rewritten in place by Lightroom, whose recipe and five rasters came
back byte for byte and whose restored develop differs from the written one in
exactly the fifteen leaves Lightroom wrote (pinned leaf for leaf in
`the_real_lightroom_rewrites_read_as_measured`) — and the desert-canyon
reimagine → reverse-fit pair as the showcase's third panel, rendered by the
shipped v1.3.1 CLI from the 0.85 develop. Both are described in
[RELEASE_NOTES_v1.3.1.md](RELEASE_NOTES_v1.3.1.md)'s closing section and in
[SHOWCASE.md](SHOWCASE.md).

## Compatibility

No library change: sidecars, recipes, the payload and every solver are the
v1.3.1 bytes. The only new persisted state is the GUI prefs key
`fit_strength`; an older prefs file loads and answers 0.65 for it, and a
v1.3.1 build reading a v1.3.2 prefs file ignores the key. A user who had the
Analysis Strength above 65 % and relied on the fit following it will find the
fit back at 65 % until the new dial is moved — that is the point of the change,
and the tooltip says so.

## Gates

Release battery on the lane (release profile, own target directories,
BelowNormal, isolated data directories): library **1480 passed / 0 failed /
15 ignored** (1495 enumerated, one process, 514.85 s), CLI **24**, contract
**2 + 2**, doc-tests 0, GUI **172 passed / 0 failed / 1 ignored**, clippy 0 on
both feature sets, `audit_i18n` 0 / 0, `subset_gui_fonts.py --check` 873/873
(868 for v1.3.1), `cargo metadata --locked` clean, `check_docs.py --gates` on
the assembled transcript **28 PASS / 0 FAIL / 2 SKIP** (the skips: the battery
lanes' own run, the active XMP census), photo-name grep 0. By name against the
v1.3.1 tag (`91df8f0`): +1 / −1, the rewritten pin, 1696 test functions either
way.

The calibration lane ran after the release — `scripts/release_battery.sh`, all
three lanes at once, the p36–p41 corpus and the sidecar weights in reach,
`--nocapture` — for the first time since v1.2.6: the corpus was never deleted,
it lives in the fixtures directory the other corpora live in. It found two
stale pins, neither on the fit: the saved develop (`fitted.recipe.json`)
still named its sky raster by the absolute path the raster had on the machine
that wrote it, and the calibration sky test pinned a single sky zone at
−0.20..−0.17 EV, a number from before v1.3.0's banding. `6ce45a3` (test code
only; the shipped binaries are this release's bytes) resolves the corpus's
raster references by file name in one loader and re-pins the sky on its two
bands (−0.152 EV with colour withheld, −0.062 EV with the colour its cells
vouched); the three lanes re-ran on that tree: library **1480 / 0 / 15**, GUI
**172 / 0 / 1**, calibration **1480 / 0 / 15** (901 s; one skip line, the
mask-brush specimen test, whose `AUTOSHADE_MB_SAMPLE_ROOT` specimen is not on
this machine), test names 1495 (+0 / −0).

Final gate, reference pair: this release's CLI rendered the 0.85 develop at
full resolution; against the v1.3.1 shipped CLI's render 5,326 of 60,217,344
pixels differ by one code (the local-versus-CI codegen noise measured for
v1.3.1's own showcase), and against the v1.3.1 acceptance render the
2048-px downscale reads mean |diff| 0.00044, max 0.008 — the same numbers as
before the change, as a render-only check of an untouched solver should.
