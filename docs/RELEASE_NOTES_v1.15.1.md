# AutoShade v1.15.1 — the leftovers of v1.15.0 closed, and small faces found

Everything merged into `main` since v1.15.0 ships here. v1.15.0's notes and
its roadmap entry listed six things the read-through had seen and that
version left as they were; this version closes every one of them, each with
a test that failed first. The one portrait limit v1.14.0 wrote down — that a
small face in a wide shot could be missed — is lifted as far as the face
model reaches: the face finder now searches the frame in tiles when the
whole-frame pass finds fewer faces than asked. Nothing new is added to the
panels; no recipe field changes.

The design, the code and this document are the maintainer's; no external
coding model wrote this release.

## Faces: small and distant faces

- **The face finder searches in tiles.** MediaPipe's face detector sees the
  whole frame squeezed to 128 × 128 pixels, so a face that is small in a wide
  shot was a few detector pixels and was missed; the app then said that no
  face was found. Now, when the whole-frame pass reports fewer faces than
  **🤖 Find people and faces** asked for, the same models search again on
  overlapping tiles — a 2 × 2 grid, then 3 × 3, then 4 × 4, neighbours
  overlapping by a quarter of a tile — so the face is larger in what the
  detector sees. A face a tile cuts at its inner edge is left to the
  neighbour that shows it whole, and a face already found is reported once.
  The whole-frame faces come first and are written exactly as before, so a
  photo in which the tiles find nothing new gets the same result as before:
  on the four public portrait samples the output is byte-identical to
  v1.15.0. The search is bounded at 29 extra detector passes and cost at
  most 0.1 s more per photo on those samples.
- **What is found now, and what still is not.** On a 16:9 test frame the
  whole-frame pass missed a face 1/15 of the width across; the tiles find
  faces down to 1/25 of the width and miss one 1/30 across. A face narrower
  than about 1/25 of the frame's long edge, a face turned more than 80°
  away, or a face less than half visible is still not found, and the app
  still says so rather than guessing.

## The six leftovers of v1.15.0

- **The reverse fit's two mixer loops have a round bound.** v1.15.0 gave
  the end-of-fit do-no-harm loop a bound of 16 rounds; the mixer stage's
  own do-no-harm loop and the later comparison of the mixer against its
  absence still halved the mixer until it was neutral. Halving reaches zero
  on saturation and luminance in at most eight rounds, but a hue rotation
  in the base recipe is never halved, so with such a recipe those two loops
  rendered the same frame forever. Both now stop at the same 16-round bound
  and withdraw the mixer to the state their comparison measured as no
  worse, recording the withdrawal as they already did. Every fixture verdict
  the fit tests pin is unchanged.
- **A data folder above the photo library no longer makes it writable.**
  The rule that no output is ever written into a photo's own folder had one
  exception: anything under AutoShade's data folder. A data folder set to a
  drive root or to the library's parent therefore made every photo folder
  writable. The exception now covers only the folders the data folder owns —
  the develops and the pixel masters — and a photo folder that merely sits
  beneath the data folder is refused like any other.
- **A sidecar read without a photo cannot carry a loose mask path.** A
  recipe read from a sidecar with no photo to anchor it kept each mask,
  depth-map, keep-sharp and pixel-layer path as the sidecar wrote it — a
  bare file name, a relative path or a network path — which a later load
  could resolve against a folder it was never meant for. Such a read now
  replaces every path it cannot anchor with the store's disabled-mask
  marker, and says how many it replaced when it has somewhere to say it.
  Reads with a photo, which is every read the app and the command line make
  when opening a photo, anchor the paths as before.
- **Redo after turning the skin guard on.** Turning **Keep skin natural**
  on clamps the skin values and records an undo step; undo put the values
  back and released the guard, but redo restored the clamped values and
  left the guard off. Each undo step now remembers the guard's state with
  the values, so undo and redo put both back together. Redo of an ordinary
  edit does not flip the guard, and a new edit after the undo drops the
  redo branch as usual.
- **The refusal hints name your delivery folder.** When the command line is
  run from inside a photo folder, its relative delivery folder lands inside
  the photos and the write is refused; the hint said `./out` even when
  Settings named another folder. The hint — and the hint on the refusal to
  overwrite an existing photo — now names the folder as configured. With
  the default folder both messages read exactly as before.
- **The colour-cast handover's white-balance gates have an end-to-end
  sample.** v1.15.0's gates on the handover's white balance were tested
  with stubs only. A synthetic pair — a grey-and-orange ramp fitted against
  itself under a +40 tint, through the public fit entry at strength 0.9 —
  now enters the handover and has six white-balance candidates refused by
  the stage's two gates, with nothing shipping. A handover whose white
  balance passes the gates and ships was not reached in 460 synthetic fits
  across white balance, tint, saturation, partial targets and strength,
  which is what v1.15.0's census of the real pairs already measured; the
  branch stays live code.

## Compatibility

- **Recipes.** The recipe format is unchanged: this version adds no field,
  so v1.15.0 and v1.14.0 open every recipe this version saves.
- **Reverse fit.** No recipe the fit tests pin changed. The only fits that
  behave differently are the ones that used to spin: a base recipe carrying
  a hue rotation now gets its mixer withdrawn after 16 rounds instead of
  never finishing.
- **Face masks.** The face-mask cache is not invalidated: a photo whose
  faces were all found before gets the same masks, and the tiles only add
  faces the whole-frame pass missed.

## Gates

The release battery (`scripts/release_battery.sh`: the library, the desktop
app and command line, and the calibration run, on a frozen copy of the release
code at `b9d8019`, with the calibration photos and the denoiser weights in
reach): library **1739 passed / 0 failed / 15 ignored** (893.02 s), CLI
**25 / 0**, contract 2 + 2, doc-tests 0, GUI **288 / 0 / 4**, calibration run
**1739 / 0 / 15** (1068.02 s; the same seven environment-gated tests as in
every battery said their sample set was absent and passed without it);
`audit_i18n` 0 findings, the font check (923 of 924 code points embedded, one
left to egui's bundle) and `check_doc_drift` (0 findings over 14 documents);
clippy 0 warnings on both feature sets; the Python suite under `python/` 109
tests, all passing (3 skipped where MediaPipe is not installed); `check_docs.py
--gates` 28 PASS / 4 FAIL on the snapshot — the four test-count claims, still
at v1.15.0's figures — and **32 PASS / 0 FAIL / 0 SKIP** once the counts were
moved. By name against the v1.15.0 release battery: library 1747 → 1754,
**+7 / −0**; GUI 289 → 292, **+3 / −0**; CLI 25. The reverse fit changed, so
the reference pairs were fitted again by the installed v1.15.0 command line
and by this build: all 15 fits (the reference pair at 0.65 / 0.85 / 1.0 and six calibration pairs at 0.65 / 0.85) produced recipes identical to v1.15.0's, the renders differ by a mean of 0.00 codes with a maximum of 1 code, and the three-way sheets were looked at.
