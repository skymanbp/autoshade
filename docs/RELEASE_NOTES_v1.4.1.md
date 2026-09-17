# AutoShade v1.4.1 — the RAW denoise map is read off the noise model, so a star keeps its own brightness

One report, 2026-09-17: "the AI denoise is not good enough. I need it to reach
Lightroom's level. I just tried it on a star field and it is not the same
league."

It was not the model, and it was not under-denoising — the measurement came
back the other way round. v1.4.0 built the affine that carries the stabilised
sensor planes into DRUNet's `[0,1]` out of **the frame's own 0.05 / 99.95
percentiles**, and clipped both ends. Everything above the 99.95th percentile —
on a star field, every star — came back as the one value `igat(top)`.

## Every star came back as the same grey dot

Measured on a 15 s ISO-3200 frame from the reporter's ILCE-7RM4A, plane by
plane on the sensor mosaic so that no demosaic and no tone curve is in the way:
the ceiling the percentile put on the output sat at 2646 / 2840 / 2821 / 1720
counts against a white level of 16383 — 7.6 % of full scale on the B plane —
and the shipped output's maximum matched `igat(top)` to five decimals on all
four planes. The stars kept 38.8 % of their brightness, and 9.7 % of their
excess over the sky, against Lightroom's 100 %.

Any frame whose bright content is rarer than 0.05 % of its pixels had the same
ceiling: night point lights, speculars, fireworks. A daylight frame hid it,
because its 99.95th percentile sits near white — which is why the acceptance
bench, whose frames are landscapes, passed with room while this was there.

The reference throughout is Lightroom's own Enhance→Denoise answer, read out of
the DNG's `NewSubfileType=16` enhanced layer. (Not what libraw hands back from
such a DNG: that is sub-IFD 0, the untouched original mosaic, byte-identical to
the ARW.)

## One class, three places where the path clamped real samples

**`model_affine(ab)` replaces the percentiles.** It takes no pixels — the
signature is the guarantee, and a test asserts the signature — and spans `0`
(the transform's own floor) to `max gat(1)`, which brackets every sample any
plane can hold. Nothing can clip, by construction rather than by tuning.

**The input normalisation no longer rectifies at the black level.** A sample
below it is real data carrying the lower half of the noise distribution:
14–22 % of a 15 s ISO-8000 frame. Clamping it inflated the fitted `a` by 3–19 %
and left the darkest band carrying 0.39–0.65 of the unit variance the model is
told to expect, so the shadows were smoothed as though 1.5–2.5× noisier than
they look. The transform floors it at its own domain instead.

**`physical_model` refuses a noise model no sensor could have.** Least squares
returns a *negative* slope when the admitted blocks share one signal level —
`a = -3.9e-04` on a synthetic flat sky — and `a <= 0` sends the transform's
radicand negative for the *bright* samples, inverting every highlight. This was
latent in v1.4.0 too, and equally fatal there. The variance at the blocks' own
level survives an unidentifiable slope, so that is what is kept.

## The operating point

The stabilised frame has one z-unit of noise per sigma, so the affine's slope
*is* the noise, and what remains is how much of it to tell the model about.
`SIGMA_SCALE = 0.85`, measured against Lightroom's answer on two astro frames
of this camera (ISO 3200 / 15 s and ISO 8000 / 15 s, three 2048² windows each)
and against `scripts/denoise_bench.py` on its ground-truth levels:

| scale | grain vs Lightroom | faint sources kept | bench, measured level | bench, ×5 level |
|---|---|---|---|---|
| 1.00 | 0.12 / 0.17× | 83 % / 60 % | +4.36 / +5.92 dB | +1.58 / +2.13 dB |
| **0.85** | **0.47 / 0.60×** | **87 % / 70 %** | **+4.40 / +5.88 dB** | **+0.53 / +1.60 dB** |
| 0.70 | 1.05 / 1.25× | 90 % / 79 % | +3.63 / +5.45 dB | −1.35 / +0.27 dB |
| *Lightroom* | *1.00* | *91 % / 71 %* | — | — |

0.85 keeps as much faint detail as Lightroom with about half its grain, and
costs the bench's measured level nothing. Below 1.0 at all because a Gaussian
denoiser told the exactly-measured sigma keeps nothing the frame cannot prove,
and a photograph is not a maximum-likelihood estimate of itself; not below 0.85
because the bench then falls behind on the ×5 level and nothing in the
Lightroom comparison pays for it.

## What it does now

End to end on the two reference frames, through the shipped sidecar at the full
61 MP, measuring the brightest 0.01 % of the R plane as a fraction of its own
input brightness:

| frame | v1.4.0 | v1.4.1 | Lightroom |
|---|---|---|---|
| ISO 3200 / 15 s | 38.8 % | **99.7 %** | 101.0 % |
| ISO 8000 / 15 s | — | **99.2 %** | 98.1 % |

with 0.28–0.82× Lightroom's residual high-frequency noise across the six
windows.

## Compatibility

- **Sidecar only.** `python/denoise_raw.py` is the whole of the behaviour
  change. No Rust code moved: the one `src/` line this release touches is a
  doc comment re-pinning the XMP census total the doc gate re-derives, from
  174 sidecars to 175 — the operator edited one more photograph in Lightroom
  while this was being prepared. No recipe or store schema changed, no CLI
  flag or GUI control moved,
  and the DRUNet weights and their pins are the same bytes — an installed
  machine downloads nothing new.
- **Existing ◈ cards keep their pixels.** A ◈ Denoised negative card carries a
  baked 16-bit master; nothing re-develops a master. To get the new result, run
  「🤖 AI Denoise now」 from the ▣ Original card again — it lands as another ◈
  card, and the old one can be deleted.
- **Frames a fit could not measure are refused, not guessed**, as before; what
  changed is that a fit which *succeeded* can no longer hand the model a slope
  a sensor could not have.

## Gates

The release battery on the bumped tree, two lanes in parallel, release profile,
each in its own target and data directory: library **1494 passed / 0 failed /
15 ignored** (1509 enumerated, 489.41 s, one process per top-level module),
CLI **24 / 0**, contract 2 + 2, doc-tests 0, GUI **190 passed / 0 failed /
1 ignored**. By name the library is 1509 — the same set v1.4.0 shipped, because
this release moves no Rust test and the only `src/` line it touches is a doc
comment. Inside the battery, `audit_i18n` reports 0 on each of its eleven checks
and `subset_gui_fonts.py --check` 874/874. Alongside it: clippy **0 on both
feature sets**, `check_docs.py --gates` **29 PASS / 0 FAIL / 1 SKIP**, the four
python suites **11 + 21 + 4 + 15**, photo-name grep 0.

The RAW denoise suite went 14 → 21. Each of the seven new cases was driven red
once by breaking the thing it claims to pin, with the source restored
byte-identically (sha256) after every one: the affine's ceiling narrowed below
full scale; the shared affine sized from the first plane instead of the widest;
the affine given a data argument; the operating point pushed outside `(0, 1]`;
the estimator made to rectify its samples before fitting; and the v1.4.0
data-percentile affine put back.

The seventh case exists because that sweep found a hole. Reverting the fix's
third site — `main()`'s lower bound, back to the rectifying clamp — left all
twenty of the others green: the estimator's duty to read samples below the black
level was pinned, but `main()`'s duty to hand them over was not, so that site
could have been reverted without a single test noticing.
`test_the_samples_below_the_black_level_reach_the_estimator` now runs the
sidecar on a night sky a quarter of whose samples fall below the black level and
reads the fitted noise model back off the sidecar's own log; with the clamp
restored it is red.

**The calibration lane did not run before the tag; it ran on the tag the same
day.** The release battery ran two lanes, not three, because the calibration
corpus was believed deleted from this machine on 2026-09-03. That belief was
wrong: the p36–p41 corpus has been at `~/autoshop-fixtures/fit-calibration/`
throughout, as the roadmap's v1.3.2 entry already records, so the doc gate's
lane claim reported SKIP at release for a reason that did not hold. After the
release, `scripts/release_battery.sh` ran unmodified on the `v1.4.1` tag with
the corpus and the weights in reach, all three lanes: library **1494 / 0 / 15**
in the default lane and **1494 / 0 / 15** in the calibration lane (884.96 s;
one SKIPPED, the mask-brush sample test, whose `AUTOSHADE_MB_SAMPLE_ROOT` sample
is not on this machine), CLI 24, contract 2 + 2, doc-tests 0, GUI
**190 / 0 / 1**, 1509 test names (+0 / −0) — and `check_docs.py --gates` on
that transcript **30 PASS / 0 FAIL / 0 SKIP**.

**The final gate.** v1.4.1 moves no renderer code, so the reference pair must
render exactly as v1.4.0 rendered it. The 1.4.1 CLI's full-resolution 0.85
render differs from the v1.4.0 release CLI's in **0 of 60,217,344 pixels**;
downscaled it sits 0.00044 mean absolute difference from the R37 acceptance
render, the same number every release since v1.3.2 has measured. The three
strengths at 2048 px are pixel-identical as well. (The full-resolution AI target
left the install's `out/` directory before v1.3.5, so the per-region Lab report
against it is again not available; the comparison is against the previous
release's renders, which were measured against that target.)

The half of that gate this release actually moves was looked at rather than only
counted: the same 15 s ISO-3200 window at 1:1 through the noisy input, v1.4.0,
v1.4.1 and Lightroom's Enhance→Denoise. v1.4.0's sky is smooth but its stars are
dimmed and several of the faint ones are gone; v1.4.1 has their brightness back
and keeps more of the faint ones, next to Lightroom's rendition and with less
grain than Lightroom leaves.
