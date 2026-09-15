# AutoShade v1.4.0 — the AI denoise runs on the sensor mosaic and lands as a new ◈ card

Two reports from the same day (2026-09-15): "after AI denoise the original
can no longer be brought back — can the result be a new variant instead of
replacing the original?" and "after AI denoise the picture loses a great deal
of detail, even with full-resolution denoise ticked; Lightroom's AI denoise
barely loses any."

## The denoise runs on the sensor mosaic

Through v1.3.5 the denoiser was SCUNet, a *blind* model, run on demosaiced
sRGB pixels. That is the wrong place for it. Measured on the user's
ILCE-7RM4A frames, the noise on the sensor mosaic is white within each colour
plane (a box-5 / Haar variance ratio of 0.975–0.995) and follows
`var = a·x + b` (ISO 640: a ≈ 2.0e-4, b ≈ 1.5e-7 in [0,1] units); after
demosaic it is spatially correlated, and no sRGB-domain model — the blind
SCUNet or a non-blind one handed the right sigma — separated it from texture.
Every strength was noise traded for texture: 1.0 kept 1–7 % of the frame's
high-frequency energy, 0.5 was a compromise.

A RAW is now denoised on its mosaic, before demosaic, by a new sidecar,
`python/denoise_raw.py`:

- The noise model is measured on the frame itself: 32-px blocks, the Haar
  diagonal variance against a box-5 high-pass variance to keep only
  textureless blocks, a least-squares fit of `var = a·x + b` per plane with
  three rounds of outlier rejection (a plane with too few blocks borrows the
  pooled fit; a frame with too few is refused, not guessed).
- Each plane goes through the generalized Anscombe transform to unit noise,
  the four planes are packed as two half-resolution colour images —
  (R, G1, B) and (R, G2, B) — and given to DRUNet-colour, a non-blind model
  that reads the noise level as its fourth channel; the exact unbiased inverse
  (Mäkitalo & Foi) brings the planes back, R and B as the mean of their two
  estimates.
- Strength is a mosaic-domain blend, default 1.0: the model's whole output is
  the right amount, and anything less only puts noise back.

Against ground truth — an ISO-100 frame of the same camera with synthetic
noise injected into the whole 61 MP frame at the ISO-640 model the shipped
estimator measured on a real frame, two 2048² windows cut afterwards
(`scripts/denoise_bench.py`, new) — the mosaic path scored +3.2 / +4.9 dB
PSNR over SCUNet 1.0 on the two windows and +3.1 / +5.9 dB on their 20 %
most-detailed blocks, where SCUNet 1.0 fell below the noisy input; it keeps
94–98 % of the fine texture there against SCUNet's 83–89 %, with less colour
error (1.0 vs 1.4–1.6 /255). At an ISO-3200 equivalent (the darktable
profile's 640→3200 ratios) the lead narrows to +1.1 / +2.0 dB whole-frame and
+1.1 / +2.6 dB on the detail blocks, still ahead on every metric; the bench's
1.5 dB acceptance line is defined at the measured level. The engine hooks
the sidecar into `render::render_to_image_in`
ahead of `develop_intermediate`, so everything downstream — white balance,
the camera curve, sharpening, the masks — is untouched, and a sensor without
a 2×2 Bayer mosaic (X-Trans, a linear DNG, a monochrome or four-colour array,
floating-point samples) is disclosed and falls back to the developed-frame
path. A baked PNG/TIFF/JPEG source keeps SCUNet with the v1.3.4 luma/chroma
law and its 0.5 default; `denoise::default_strength_for` picks the default per
source on the CLI, the web export and both GUI dials.

## It lands as a new ◈ Denoised negative card

The in-place landing redefined the ▣ Original card: its pixel source became
the denoised master, the undo lived only in that session, and nothing could
detach the negative again. 「🤖 AI Denoise now」 now lands its result as the
strip's fifth kind of card, **◈ Denoised negative** (store word `"denoised"`):
source-based over its own 16-bit master, carrying the develop of the card it
was run from, auto-switched to. The card you started from keeps its pixels,
recipe and origin; the ▣ card is never changed. While a ◈ card exists its
master is the photo's negative (`negative_origin`): a Reimagine sends it and a
Reverse-fit is solved on it and lands on it. Heal and clone stay in place on
it; a `.xmp` from it carries the sliders only; 「🤖 AI Denoise on export」 sits
out on it (its master is already denoised) and the export summary carries no
amount there.

## Always the full frame

The ≤2048 px working-copy tier and the **Full-res denoise** checkbox are
gone: a card baked from a working copy capped every later export of that card
at 2048 px, which is what "even with full-resolution ticked" was reporting
when the box had been left unticked. The denoise always runs on the full
sensor (a RAW) or the image itself (a baked source).

## Compatibility

- **Desktop.** The result of an AI denoise is a new ◈ card; older strips with
  a ▣ card that carries an in-place denoise master keep working (that master
  is still the negative when no ◈ card exists). The two denoise dials now
  start at 100%; a prefs file carrying 50% keeps it, and on a baked source the
  SCUNet law reads it as before.
- **CLI.** `autoshade denoise` on a RAW writes a neutral 16-bit develop of the
  denoised mosaic; `--model` (a SCUNet tier) applies to baked sources only.
  `auto --denoise` and the web export take the same RAW path. The default
  strength is 1.0 on a RAW and 0.5 on a baked source.
- **Configuration.** `AUTOSHADE_DENOISE_RAW_SCRIPT` points at
  `python/denoise_raw.py` (bundled beside the other sidecars; a release
  package carries neither, as before). The DRUNet weights (130,579,305 B,
  MIT, from KAIR/DPIR) and the two KAIR network files download on first use,
  sha256- and byte-pinned, through the same verified fetch the other sidecars
  use.
- **Store format.** The strip record admits the `"denoised"` spelling; no
  other field changed.
- No solver or recipe schema change, and the renderer's only change is the
  mosaic hook, which is inert without a denoise request; the reference-pair
  final gate (the same bytes as the previous release) runs at release time,
  as it does for every version.

## Gates

Measured on the lane before the merge (dev test profile, per-lane target
directories): library **1494 passed / 0 failed / 15 ignored** (313.28 s; by
name 1526 → 1533, all seven in `denoise::`), CLI **24 / 0**, contract 2 + 2,
doc-tests 0, GUI **190 passed / 0 failed / 1 ignored** (by name 188 → 191:
four ◈-card tests in, the in-place denoise test out), clippy 0 on both
feature sets, `audit_i18n` 0 / 0 / 0, `subset_gui_fonts.py --check` 874/874
(one new Chinese string reworded rather than the fonts regenerated),
`check_docs.py` 25 PASS / 0 FAIL / 5 SKIP, the four python suites 14 + 11 +
15 + 4, photo-name grep 0. `scripts/denoise_bench.py` at the measured ISO-640
model: the RAW path leads SCUNet 1.0 on the detail blocks by +3.13 / +5.92 dB
(whole-frame +3.21 / +4.90) — PASS against the 1.5 dB line; at the
extrapolated ISO-3200 level +1.10 / +2.60 dB (whole-frame +1.10 / +2.00),
reported, not gated. Five hand mutations, each restored byte-identically
(sha256): the mosaic hook deleted from the render, the denoise landing
flipped to in-place, the landing's card kind flipped, `negative_origin`
ignoring the ◈ card, and the sidecar built with bias tensors — each named by
its test (the last one's first pin matched a comment and was tightened to
the call text before it did).
