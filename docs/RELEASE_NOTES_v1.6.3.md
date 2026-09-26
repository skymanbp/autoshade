# AutoShade v1.6.3 — files the app wrote read back as it wrote them, and a baked master is fitted under the calibration it carries

Everything merged into `main` since v1.6.2 ships here: three fixes, all found
by the one measurement v1.6.2's notes listed as not made — `match --negative`
on a real stack — and one to the release battery's own count, found by this
release's battery. Nothing new is added — this is a patch release — and every
change below carries the measurement that decided it.

The design, the runs, the acceptance and this document are the maintainer's;
no external coding model wrote this release.

## The reverse fit

- **A baked master is fitted under the calibration its own saved develop
  carries.** Without `--negative`, `match` fitted a baked file with no
  calibration and stamped its saved one on afterwards. A master fitted once
  with `--negative` keeps its RAW's calibration in its saved develop, so
  fitting it again without the flag shipped a camera curve the solve had never
  seen: on a real three-frame stack (the burst's first frame as the negative)
  that second fit rendered **+50.04 / +56.93 / +60.01** codes per channel
  brighter than the target, mean |diff| **0.2212**. The calibration now
  composes into the solve — the desktop app's rule for a baked file — and the
  same sequence lands at **0.0348**, 0.0009 from the RAW's own fit (the
  `--negative` fit: 0.0346, and 0.0010 from it). A file nothing has calibrated
  fits as before: on the same stack its recipe is byte for byte v1.6.2's. One
  CLI test pins the rule.

## Colour profiles

- **AutoShade's own sRGB profile reads as the working space.** AutoShade tags
  its sRGB exports and its masters (as JPEG, TIFF or PNG, the formats that
  hold a profile) with one compact sRGB profile, and reading such a file back
  ran it through qcms, whose rendering of that profile's sampled curve against
  its own parametric sRGB moved 575,212 of an 8-bit cube's 50,331,648 samples,
  by up to 2 codes: a file the app wrote did not read back as the pixels it
  wrote. That profile now reads as the working space itself, with no
  transform; a library test pins a tagged file reading back bit for bit at 8
  and at 16 bits.
- **The 16-bit profile lattice sits on exact codes.** A 16-bit image with an
  embedded profile — Lightroom's "Edit in…" ProPhoto TIFF is the common one —
  is mapped through a lattice of qcms's 8-bit transform. Its 33 nodes sat
  255/32 = 7.97 codes apart, so their 8-bit inputs truncated below the
  positions the lookup assumed and every 16-bit profiled read came back
  darker: through an identity transform 65,403 of the 65,536 grey levels
  moved, by 0.484 codes on average and 0.969 at worst. The lattice has 52
  nodes now, 5 codes apart, every input an exact code, and the identity moves
  nothing. On the real stack the two effects together read one master 0.673
  codes darker tagged than untagged under v1.6.2 — 99.7 % of the render's
  samples, 1.957 codes at most.
- **The stack, heal and clone masters, and a baked photo's 16-bit denoise
  master, carry the profile.** v1.6.2 wrote them untagged, so reading a 16-bit
  one back printed "16-bit but carries no ICC profile" — the warning meant for
  an editor's ProPhoto export that lost its tag — and any other editor had to
  guess its space. On the real stack the v1.6.3 master carries the profile; a
  render and both fits read it without the warning (v1.6.2 printed it five
  times over the same three steps); its default render is within one 16-bit
  step of the untagged master's; and the two fits' recipes are the fix
  above's, key for key. An 8-bit denoise master of a baked photo stays
  untagged: it reads as sRGB everywhere, and a tag carried onto a JPEG product
  would re-encode it.
- **A RAW's denoised master reads as it was written.** On the reference pair
  the RAW, denoised on its sensor mosaic into a 16-bit master (which carries
  AutoShade's profile), was fitted with `match --negative <RAW>` at 85 % by
  v1.6.2 and by this release, the same master file for both. v1.6.2 read it
  through qcms and the 33-node lattice and fitted a residual look error of
  0.127 → 0.050; this release reads it as written and fits 0.129 → 0.048, the
  pair the RAW's own fit gives at the same strength. The two renders differ by
  0.232 codes on average and 8.8 at most, and sit equally close to the target
  (mean |diff| 0.0239 each).

## Documents and the release battery

- **The manual** says which masters carry the profile, and its `match`
  paragraph says what a master fitted with, then without, `--negative` is
  fitted under. **TECH_STACK** gains the profile reading's method, its two
  parameters and what v1.6.2 did, and its `match` sentence follows the fit.
  **ARCHITECTURE**'s CLI `match` contract follows the fit; the battery counts
  and the five added test names are in its count paragraph.
- **The release battery counts a skip line wherever it lands.** In the
  calibration lane a test's skip line can arrive between the test harness's
  `test <name> ... ` and its `ok`; the first run of this release's battery had
  its one skip there, and its summary read 0. The summary and `check_docs` now
  find the line anywhere: on v1.6.2's transcript they read 1 as before, on the
  first v1.6.3 run 1 instead of 0.

## Compatibility

No recipe, sidecar, store or weight format changes. The sidecar weights stay
pinned to the v1.6.0 `autoshade-raw-denoise-v2.pth`; no weights are
re-shipped. What reads differently: a file carrying AutoShade's own sRGB
profile — an sRGB export as JPEG, TIFF or PNG, a RAW's v1.6.2 16-bit denoise
master — reads as the pixels it holds (v1.6.2: up to 2 codes off at 8 bits and
up to 2.8 at 16, darker on average); any other 16-bit file with an embedded
profile reads without the lattice's darkening, which through an identity
transform was 0.48 codes on average and 0.97 at most, and through another
profile scales with that profile's own slope; a master v1.6.2 wrote untagged
still reads as sRGB, with the warning. A fit or render of such a file differs
from v1.6.2's accordingly: on the reference pair's denoised master the two
renders sat 0.232 codes apart on average. `match` on a RAW is unchanged; on
the reference pair the release gate below shows it byte for byte.

## Gates

Measured before the tag on the release code (`0e6c851`, whose Rust is
`0e00a8e`'s: the commit between touches the battery's skip count only; the
first run, on `0e00a8e`, read the same counts. The version bump touches
Cargo.toml, Cargo.lock, the documents, the site's cache keys and the bug
template's dropdown, and the CLI, contract and doc-test suites, the denoise
module, clippy, the `python/` suite and `check_docs` are re-run after it). The
three-lane release battery (`scripts/release_battery.sh`, a frozen snapshot
worktree, the p36–p41 calibration corpus and the sidecar weights in reach):
library **1752 passed / 0 failed / 15 ignored** (1026.43 s, release profile,
one process per module), CLI **27 / 0**, contract 2 + 2, doc-tests 0, GUI
**222 / 0 / 1**, calibration lane **1752 / 0 / 15** (1537.83 s; one skip line,
the mask-brush specimen test whose `AUTOSHADE_MB_SAMPLE_ROOT` specimen is not
on this machine, named in every release since v1.3.2), `audit_i18n` and the
font check exit 0 inside the battery; the Python suites 81 OK from `python/`
and 44 OK from `scripts/` (CPU, the real weights, `-W error::RuntimeWarning`).
By name against the v1.6.2 release battery: library 1763 → 1767, the four
additions being `the_16bit_lattice_is_exact_on_an_identity_transform`,
`the_engines_own_srgb_tag_reads_back_bit_for_bit`,
`a_deep_baked_denoise_master_carries_the_working_space_tag`,
`a_pixel_master_carries_the_working_space_tag_and_reads_back_exactly`; GUI 223
→ 223, nothing added or removed; CLI 26 → 27, the one addition being
`match_fits_a_baked_source_under_its_saved_calibration`. clippy 0 warnings on
both feature sets (`--all-targets -- -D warnings`). `check_docs.py --gates` on
the transcript with the XMP census root supplied: on the frozen snapshot **32
PASS / 0 FAIL / 0 SKIP** — the counts moved with the code before the snapshot
— and after the bump **32 PASS / 0 FAIL / 0 SKIP**; after the bump the CLI,
contract and doc-test suites read 27 / 0, 2 + 2 and 0, the denoise module 47 /
0, clippy 0 warnings on both feature sets, the `python/` suite 81 OK, and
`audit_i18n` and the font check exit 0. Photo-name, token-shape and user-path
greps on every commit of the release: 0 / 0 / 0.

Final gate, reference pair, before the tag: the release code's CLI re-fitted
the reference pair at 0.65 / 0.85 / 1.0 and rendered each at the target's
size. At every strength the recipe (store paths normalised), every mask raster
and both renders, with and without the masks, are byte for byte the v1.6.2
gate's — themselves the v1.6.1 gate's approved ones — so the readings stand:
sky ΔE / whole-frame mean |diff| 6.3 / 0.0283, 3.8 / 0.0248 and 3.8 / 0.0249
against the target. The profile fixes do not reach this path: a RAW is decoded
by the RAW reader, and the target JPEG carries no profile.

Not measured: no paid image call was made for this release (the target is the
one every release since v1.3.2 has been measured against); the GUI executable
was not launched — its heal, clone, stack and denoise buttons call the library
functions the command line calls (`retouch::heal`, `retouch::clone_stamp`,
`stack::stack_files`, `denoise::denoise_active`), which write through the
changed writers, and it reads files through the same `decode::load_image`; the
heal and clone masters and a baked photo's 16-bit denoise master were measured
by library tests, not on a real photo; no 16-bit file carrying another
editor's profile (Lightroom's ProPhoto TIFF) was read, so the lattice's
correction was measured through an identity transform and through AutoShade's
own profile only. The ship facts (the release run, the downloaded assets, the
site, the local upgrade) are in the ROADMAP ledger entry.
