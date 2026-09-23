# AutoShade v1.6.0 — the RAW denoiser is told the true noise and keeps faint stars; the camera look and the sharpening default follow Lightroom

Everything merged into `main` since v1.5.1 ships here. The RAW denoiser was
rebuilt around one idea — tell the network the noise it actually faces, place
by place, and give back only luminance grain afterwards — then its weights were
fine-tuned so that faint stars are signal to it, not noise, and a further
fine-tune trained on a rented GPU was refused by the lines written before it
ran; the star-frame standard against Lightroom's Denoise became a release gate
read in two groups, and its two front-end lines pass for the first time;
isolated hot pixels are mapped in every develop; the camera base look is
estimated like with like and on block means, and every photo saved by an
earlier version gets the current estimate on its next open; sharpening
defaults to Lightroom's own 40 on a RAW; on bodies whose RAW declares its crop
but no active area, the develop window now sits where the camera says it does
and every recipe saved before moves with it (coordinate era 2); and the
reverse fit's hard-mask boundary ruler, its re-judgement of shrunk corrections
and its colour field at the default strength were reworked on the reference
pair (R39–R41). Each change below carries the measurement that decided it.

The design, the runs, the acceptance and this document are the maintainer's;
no external coding model wrote this release.

## The RAW denoiser: honest noise, then only luminance grain

**Honest sigma.** The cleaner (DRUNet on the stabilised sensor mosaic) used to
be told a lower noise level than the frame carried, because that kept texture;
it now receives the measured level, and samples below black are no longer
clipped before it. At any positive strength the Rust side requests the full
clean output; the original and the clean frame go through the same demosaic and
calibration; one float luminance plane of the original is kept, and in linear
light, just before encoding, `1 − strength` of the luminance difference returns
along the grey axis (weights: the Y row of the working space's own matrix).
Strength 1 never develops the original; 0 never starts the sidecar; every other
value keeps the clean chroma — colour noise cannot come back by construction.
The default is 0.71 (fine-luminance residual on two ordinary night frames
0.3145 / 0.2938 of the input's, beside Lightroom's Denoise 50's 0.28–0.30 on
the same frames; chroma residual within 0.0002 of the fully clean output). Cost at 61 MP
on the CPU stand-in, model process excluded: peak 1765 → 1996 MiB, 5.1 → 10.2 s.
Not matched on the star frame: chroma-to-luma of the fine grain 0.268 against
Lightroom's 0.152, and star widths on two frames +0.285 / +0.325 px against
Lightroom's +0.147 / +0.291 px.

**The noise is measured where it stands.** The noise model was a function of
level only, while real noise on the operator's frames varies by position: after
stabilisation the network is promised unit variance everywhere, and the worst
plane of the star frame read σ 0.83–1.19 per tile (edge tiles up to 1.34), so
the centre was over-cleaned and the edges half-cleaned (per-tile residual
p10/p50/p90 0.000 / 0.056 / 0.315, max 0.508). `python/denoise_raw.py` now
measures σ per tile on the finest diagonal wavelet band — choosing its samples
by the three orthogonal bands, so the choice cannot bias the number — fits a
local linear field, clamps it to 0.5–2.0, divides it out before the network and
multiplies it back before the inverse transform. After: per-tile σ p5–p95
0.963–1.031 on that frame, residual max 0.070; the tile-to-tile width of the
fine-luminance ratio on the same develop fell from 0.185 to 0.014 (Lightroom's
own 0.011); the ground-truth bench moved by at most 0.01 dB. The level fit also
takes its darkest blocks again — a stale lower bound had kept the only blocks
that fix the floor term out, and a dark foreground was being told 2.3× its
noise.

**Hot pixels are mapped in every develop.** Every develop of a Bayer RAW —
`apply`, the app, batch, the web UI — first maps isolated sites stronger than
20 local sigma, before the grain source and the cleaner read the mosaic (the
CLI help says so). The rule was rebuilt on what ordinary frames showed: the
first version's whole-frame noise law flagged 1–4,003 sites on daylight frames
and ate star cores on night frames, because evidence that cannot testify was
being counted as testimony. Now the site is judged on its own neighbourhood's
spread (the MAD of the 24 same-colour samples within 4 px, halved, or the
tile's sigma if that is larger), clipped samples never measure noise, and
eight neighbours must vouch. On ten 61 MP
frames the current rule is a subset of the old one everywhere: 86 / 20 / 77
sites inside the picture on three night frames (none on the fourth), 0 / 0 / 0
/ 6 / 0 / 4 on ordinary frames; the scan costs about 0.1 s. Frames with no
mappable site render byte-identically.

**Preferences.** Preference era 2 resets the two RAW-denoise dials once (the
status bar lists the old values on start-up); baked images get their own
strength slot, SCUNet and its 0.5 default unchanged. Nothing else migrates.

## The weights are v2 of the fine-tune

One measurement (2026-09-21), made with a synthetic-truth probe on the
operator's own star frame: the v1 network, which had never seen a star, removed
faint ones as noise — 10 % of a 4σ star's flux survived the cleaning, 42 % of a
6σ one, 73 % of a 10σ one. Lightroom's Denoise keeps them. This release ships
v2 of the fine-tune: the same network continued from the v1 weights with point
sources injected on the clean side, chosen among its checkpoints by acceptance
lines written down before the run, and held to the star-frame standard against
Lightroom's Denoise 50 exactly as v1 was.

**What changed in training** (`scripts/train_raw.py`, its new defaults):

- `--stars 0.5`: half of every batch's crops receive point sources — stars with
  their own photon counts on the noisy side, the same stars on the clean side —
  so the network learns that an isolated positive spike on a flat sky can be
  signal. v1's clean targets held none (`scripts/point_sources.py`).
- `--loss l2x`: the loss seeks the mean in the units light adds in, after the
  inverse transform, weighted by the variance the stabiliser itself assumes.
  v1's L1 in the stabilised domain has the conditional median as its
  minimiser, and under a sky of stars too faint to stand alone the median is
  the empty sky: 0.4–1.9 DN per plane of star flux went missing on the
  operator's frame.
- Started from the v1 weights, 60000 steps at 0.289 s each, on the same data
  (RawNIND pairs, synthetic sensor noise over the operator's low-ISO frames),
  under a supervisor that only used the GPU while nothing else did.

**How the checkpoint was chosen.** The lines live in `scripts/accept_v2.py` as
constants and were committed before the run started. On the G1 plane of the
synthetic-truth instrument (`scripts/denoise_flux_truth.py`): flux returned at
1.5σ and 2.5σ at most 0.35 (noise peaks must not be kept), at 4σ at least 0.50,
6σ 0.75, 10σ 0.90, 20σ and 40σ 0.95; the star-free sky's mean shift at most
0.40 DN per plane (the way a network could fake the first line by lifting the
background). Then two lines that need a photograph: the ground-truth bench's
two detail windows within 0.150 dB of v1, and the star-frame standard's cleaner
group must not lose a line v1 passes.

Every 10000 steps the checkpoint was measured (a watcher in the training lane
ran `scripts/accept_v2.py` on each new one; the passing ones were kept):

| checkpoint | 1.5σ | 2.5σ | 4σ | 6σ | 10σ | 20σ | 40σ | sky R / G1 / G2 / B (DN) | verdict |
|---|---|---|---|---|---|---|---|---|---|
| v1 (shipped) | 0.062 | 0.061 | 0.105 | 0.420 | 0.726 | 0.917 | 0.976 | −0.016 / +0.216 / +0.317 / +0.097 | 4σ–20σ FAIL |
| 20000 | 0.103 | 0.286 | 0.575 | 0.879 | 0.950 | 0.973 | 0.994 | −0.249 / +0.022 / +0.038 / +0.377 | PASS |
| 30000 | 0.112 | 0.299 | 0.566 | 0.855 | 0.927 | 0.955 | 0.986 | −0.149 / −0.020 / −0.012 / +0.101 | PASS |
| 40000 | 0.126 | 0.306 | 0.566 | 0.844 | 0.918 | 0.950 | 0.981 | +0.208 / +0.211 / +0.247 / +0.065 | PASS |
| **50000** | 0.045 | 0.252 | 0.541 | 0.835 | 0.918 | 0.950 | 0.982 | −0.092 / −0.174 / −0.129 / +0.013 | PASS |
| 60000 | 0.068 | 0.266 | 0.548 | 0.833 | 0.913 | 0.945 | 0.979 | −0.028 / −0.104 / −0.110 / +0.099 | 20σ FAIL by 0.005 |

(12000 failed on the sky, B +0.800 DN, a transient the next checkpoints undid.)

The bench (`scripts/denoise_bench.py`, ISO-100 frame + the ISO-640 noise model,
read to three decimals with the bench's own `report()`): 20000 read −0.155 dB
on the first window against the −0.150 line and was set aside — the line was
not moved; 30000 −0.032 / −0.039, 40000 −0.038 / −0.063, 50000 −0.050 / −0.059
(dPSNR 48.524 / 48.539 against v1's 48.574 / 48.597).

The star-frame standard (`scripts/denoise_star_standard.py`, the operator's
ISO-2500 frame against Lightroom's Denoise 50, one real GPU run per weight set
on this build, every render through the product path): the shipped v1 weights
on this build reproduce the 2026-09-21 verdicts line for line (1c, 2c, 3, 4, 8
PASS; 5c, 6, 7c FAIL). 30000 lost line 1c (fine-Y 0.303 against Lightroom's
0.270, limit ±0.03). **40000 and 50000 lost no line v1 passes and gained 7c**,
the four colour planes' flux balance (spread 0.016 against v1's 0.046, limit
0.02). 50000 ships: it is the cleaner of the two on noise (1.5σ 0.045 against
0.126) and on the sky (largest shift 0.17 DN against 0.25), its stars spread
least, and its bench margin is the same.

**What was verified about the instrument.** This build's renders sit (32, 20) px
off the 2026-09-21 renders (the origin fix below), so the star mask that day's
run saved could not be reused; the standard's sites were derived again from the
input render and then held fixed for every candidate, which is why v1 on this
build had to — and did — reproduce that day's verdicts before any candidate
was read. Every candidate ran the sidecar unchanged but for its tensors
(`weights_only=True`, `strict=True`), at the same `SIGMA_SCALE` of 1.0, and its
input mosaic differed from v1's run in 0 photosites.

**The weights have a copy of ours, and it is tried first.**
`autoshade-raw-denoise-v2.pth` — 130,590,559 bytes, sha256
`ffafa40a53f52092149db2fcf03636117ad6855e1068142d4f6b03b634e9f9c4` — is the
one file this project publishes itself, and until now the one pinned download
without a second host. It now has one:
`Azng0/autoshade-mirror-autoshade-raw-denoise` at commit
`0361b7af8853a57aa5c4a5c18ee589233caf14a0`, uploaded in a single commit with
the project licence and a model card (so the pinned tree carries its terms),
then read back anonymously at that commit and hashed: the same bytes. The
sidecar (`python/_mirror.py`) tries that copy first and the release asset
after it; as with every mirror, the source decides only where bytes come from
and the digest decides whether they are kept. The pin moves from the v1.5.0
asset to this release's; a cold cache downloads 130 MB once, v1 is not fetched.
`scripts/check_docs.py` now re-derives the README's weights row (release, size,
SHA-256) and the release the prose sends you to from the pin the sidecar
enforces — the v1.5.0 release shipped with that row unguarded, which is how a
hand-uploaded asset could have gone missing with every gate green.

## The star-frame standard is a release gate, read in two groups

Since the luminance-grain lane (2026-09-20) the same-frame comparison against
Lightroom's Denoise 50 on the operator's star frame is a release obligation
whenever the denoiser moves
(`scripts/denoise_star_standard.py`). It is read in two groups: the **cleaner
group** (1c, 2c, 3, 4, 5c, 6, 7c, 8) decides the exit status and reads the
cleaner on a develop whose base curve is emptied where the curve would
otherwise be what is measured; the **front-end group** (1f, 2f) reads the same
lines on the finished develop and is reported only, because what it reads is
the camera-matched base curve, not the cleaner. Line 5c's faint sites first
pass a true-star test on the mosaic the cleaner received (36 photosites behind
every sample), so a noise peak the input happened to hold cannot count as a
star kept or lost; line 7c reads the cleaner's own output plane by plane.

**On the release build, with the v2 weights: cleaner group 6 of 8, and the
front-end lines inside for the first time.** The verdicts were read twice on
the same weights, the same GPU capture of the cleaner's output and the same
mosaics, because the standard's star sites are derived from the input render
and the input render carries the base look:

- On the 2026-09-22 curve (the pixel-level estimate): 1c, 2c, 3, 4, 7c, 8
  pass; 5c and 6 read outside, as they did for v1 (which passed 5 of 8); 1f
  and 2f outside. **5c** read 94.2 % against Lightroom's 96.3 % — 482 of
  22,587 stars; 826 stars we lose Lightroom keeps, 344 the other way; the
  lost ones sit just under the line (peak ratio p25/p50/p75 0.38 / 0.43 /
  0.47), are the faintest (detection SNR 5–6: 89.4 % against 93.4 %; SNR ≥ 8:
  within 0.3 points) and sit toward the corners (outer quarter of the frame
  80.3 % against 87.2 %; inner half within 0.7). **6** read 0.871 of the
  input's peak against Lightroom's 0.941, with the same widening (+0.36 px
  against +0.37); the flux is kept — inside 2.5 px ours reads 0.98 of the
  input, Lightroom's 1.09 — and the loss is largest on the least bright of
  the bright stars (first quartile 0.78 against 0.91) and in the corners
  (0.80 against 0.87).
- On this release's curve (block means, below): **1c 0.2924, 2c 0.0116, 3, 4,
  8 (glow plates clear) and 5c pass — 98.70 % against Lightroom's 98.80 %,
  17,586 against 17,603 of 17,817 true faint stars**; 6 reads 0.939 against
  0.953 (limit Lightroom − 0.01) and 7c's four-plane flux spread 0.0385
  (1.033 / 1.012 / 1.011 / 0.995) against the 0.02 limit. **1f reads 0.2922
  against Lightroom's 0.2701 (limit ±0.03) and 2f 0.0139 (limit 0.0278)** —
  the standard was run on the product path, on the recipe this build
  estimates for the frame when it is opened fresh.
- **The two red lines are the same effect, and it is in the network, not the
  pipeline.** On the cleaner's own output mosaic at strength 1, before
  demosaic and before the grain return, the single-sample peak on the plane
  each star lands on reads 0.82 of the input's for faint true stars (v1: 0.89)
  and 0.92 for bright ones (v1: 0.97), while the 5×5-sample aperture reads
  1.00–1.03 for both: v2 keeps the flux and spreads the core a little more
  than v1 did. That is the price of a loss that seeks the mean, and it is what
  the flux-truth lines bought (54 % of a 4σ star's flux kept against 10 %).

**Two more fine-tunes were trained and refused.** Each had its acceptance
written before the result. v3, the same network continued with composite
stars (a core on a streak, an end, a halo), moved line 6 only 0.871 → 0.88,
lost 0.7–0.9 pt of 5c and failed the flux-truth lines at 10,000 steps. v4,
five runs at once on a rented H200 from the v2 weights — a comet prior (a
core narrower than a photosite pair at the head of a 4–14 px flare) and a
loss that counts a star's own pixels K times where its light stands 2–5
sigma over the noise, alone and together (K 3, 6, 12) — produced ten
candidates that all failed the synthetic-core line and the ordinary-frame
line, and on the star frame kept fewer faint stars (93.7–94.9 % against
96.3 %) and lower bright-star peaks (0.867–0.894 against 0.94) than v2,
three of them losing 1c as well. A return of the input's own 3×3 star cores
after the network was measured and withdrawn: the one variant that reached
the lines returned the input's noise with the star (+40 % spikes in the flat
blocks of a daylight frame, colour noise beside faint stars). v2 ships; lines
6 and 7c stay red and are recorded here. The rented machine cost 8.07 USD and
was torn down.

## The camera base look is read from the picture, block by block

The base look — the tone curve that makes a neutral develop look like the
camera's own JPEG — was wrong in two ways, and both were measured.

**Like with like.** It used to be estimated by matching the neutral develop
*with the lens profile's vignetting lift applied* against the camera's
embedded preview, on the strength of a review remark that camera JPEGs carry
that correction. On ten ILCE-7RM4A frames (profile corner gain 1.33–1.98) no
embedded preview carried it, so a lift the camera never made had become tone:
on night frames whose whole picture sits in a 0.13-wide luminance band it was
a run of curve slopes from 0.33 to 2.25. Now one entry, `render::camera_base_look`,
serves the app's open path, the pipeline and the web UI (the three had drifted
apart); the pairing is made at the camera's own framing, and whether the
preview carries the lift is *measured on the pair* (outer-ring against
inner-ring residual) — the lift is applied only when the pairing says the
camera made it, and a pair that cannot say keeps the picture the sensor saw.
The estimates made that way are brighter by +1.3 to +8.5 levels (8-bit) on
average over the ten frames, at most 2.7–16.4, and the night frames' slope
humps are smaller (F1 2.25 → 1.73, F2 3.06 → 2.28, F3 1.78 → 1.50). Eight
mutations of that estimator each turned its own test red.

**On block means, not pixels.** Paired like with like, the estimate was still
a pixel-level match of the two pictures' distributions: eleven quantile knots
that on a night frame sit 0.004–0.01 apart, at the scale of the 8-bit
preview's own quantisation, with the preview's in-camera sharpening, noise
reduction and JPEG texture read as tone — F1's curve still ran slopes of
0.85–1.72 across the sky band. The tone stage scales colour by the luminance
ratio, so every wiggle of slope acted on the grain of the input and on the
cleaner's output differently, and that is what the star standard's two
front-end lines read (fine-Y ratio 0.3075 against Lightroom's 0.2701, limit
±0.03; tile-to-tile width 0.0387, limit 0.0309). The estimator now matches
the two pictures on 64-column block means (the grid the corner measure
already uses): both sorted, walked in groups spanning at least 0.06 of
neutral luminance with at least 64 blocks each, one knot per group at its
median block, the ends pinned; a curve within 0.02 of the identity is no
curve. The design was chosen on the real engine, not on a simulator (which
sat 0.006 above it): pixel-level matching with merged knots failed at 0.3234,
in-place pairing produced plateaus on three other bodies, every block-mean
variant passed. On F1 the estimate is four knots instead of thirteen, and the
develop sits 0.75 levels rms from the camera's rendition (median +0.19)
against 4.24 (+2.56) before. On the product path — this build's own
fresh-open recipe, one GPU capture of the cleaner replayed — **1f reads
0.2922 and 2f 0.0139, both inside.**

**Every photo gets it.** A recipe saved by an earlier version carries the
curve that version estimated. The estimate is a property of the version, not
of the edit, so the recipe's `version` stamp now says which estimator made it
(calibration era 3), and a recipe stamped below it is re-estimated the first
time it is opened — by the app, batch export, the web UI or `apply`, each of
which says so; a recipe saved with no base look keeps none, because a develop
tuned without one would change under one. Until now only era 1's washed
curves had been replaced, and only by their fingerprint.

## Sharpening defaults to Lightroom's 40 on a RAW

Lightroom sharpens every RAW at Amount 40 (radius 1.0, detail 25, masking 0)
unless told otherwise, and a JPEG or TIFF at 0. This app's default was 0 for
both, and its sidecar always wrote `Sharpness="0"`: a RAW opened here rendered
and exported unsharpened, 40 short of the Lightroom develop it was measured
against, and once saved it switched Lightroom's own default off. The amount is
now a companion control like the radius and the details, with the default
Lightroom itself uses for the kind of file: a recipe that holds no amount
renders at 40 on a RAW negative and at nothing on a baked raster; the Detail
panel's slider shows the value the engine renders and resets to it; a drag to
0 on a RAW is a real 0 and reaches the sidecar as one; an absent amount leaves
the key out, so Lightroom applies its own default, and the 40 it writes back
is read as a materialisation, not an edit. Every surface that develops a
picture passes the kind of its source — the canvas, its Range and Point Color
references, the fill and adjust models' picture of a card, the web preview,
the RAW and the baked export — and the analysis surfaces (the reverse fit, the
judge) develop as they did. What is not claimed is the amount's *scale*: the
sharpening operator is this engine's own, built from what Adobe documents, so
40 here and 40 in Lightroom are the same default, not a measured equivalence.

## The develop window sits on the declared crop

On bodies whose RAW declares a crop origin but no active area — the operator's
ILCE-7RM4A files say sensor 9600×6376, crop 9504×6336 at (32, 20), and nothing
else — the decoder adapted the crop against itself, the origin collapsed to
(0, 0), and every develop was cut from the sensor's top-left corner: v0.32.0's
fix had addressed a shape read from the decoder's source that these files do
not have, so it had never fired for them. The develop window now moves onto
the declared crop whenever the decoder would otherwise cut from (0, 0). Measured:
an unconstrained ±48 px registration of the full-resolution develop against
Lightroom's export of the same frame reads (32, 20) before and (0, 0) after;
the two develops differ in 0.10 % of their overlap, all of it within 3 px of
the border — the new develop is the old one shifted, nothing else. On this body
**every develop and every pixel-level pin moves by (32, 20) px**. The
calibration corpus moves with the frame (the largest change: one pair's colour
cast, previously vetoed, is now adopted — residual 0.058 → 0.019 at confidence
0.37 → 0.50).

**Saved recipes move with it: coordinate era 2.** A recipe's geometry — crop,
every mask geometry, brush and gesture points, colour-range sample points,
retouch centres and donors, the colour field, and the raster mask files it
names — is stored normalised to the frame, and a recipe saved under the old
origin would have sat 0.34 % of the width and 0.32 % of the height off the
new picture (era 1 rotated a recipe into the display frame but never
translated it, and no field said which origin a recipe was drawn on). The
decoder now answers, in the same metadata read that places the window, how
far the old develop of this file stood from the new one (`legacy_shift`:
non-zero only for the shape "no active area, crop origin not at (0, 0)"; the
v0.32.0 shape has developed at its declared origin since that version, so its
era-1 recipes already sit in the new frame and read 0; a window the decoder
refuses never developed aligned before or after, 0 as well). On the first
open of an era ≤ 1 recipe — the six entries era 1 had: the app's open, the
variant strip, version snapshots, batch export, `api_recipe`, `apply` —
the recipe is first oriented by era 1's rules, then the shift is turned into
the display frame (`orient_vector`, the linear part of `orient_point`) and
every carrier is translated (`shift_recipe_coords`: the crop clamped to the
frame, lengths and angles unchanged, the colour field resampled bilinearly at
its cell centres). A raster mask file is rewritten as a shifted copy — edge
pixels extrapolated, a whole-pixel shift an exact copy — under a name the
operation decides (`<stem>-e2-o<orientation>[t]-<dx>x<dy>.png`), so several
recipes naming one raster, or a CLI develop run again and again, produce one
file, and the original stays for the snapshots that still name it; an AI
mask's cached alpha is shifted with it (the shifted alpha is the shifted
picture's alpha, so masks keep working on a machine without the segmentation
sidecar), while an orientation change discards the cache and re-segments as
era 1 did. If any raster cannot be read or written the whole recipe is left
untouched and unstamped, tried again next time, and the failure is said at
each entry. Era 1's one disclosed-only gap — era-0 raster masks could not be
rotated — closes with it: they are. Recipes from the browser, the model and
the Lightroom sidecar are stamped era 2 at the boundary (Lightroom's
coordinates are already the declared crop's). Measured: the six window shapes'
shifts; `orient_vector` against `orient_point` in all nine orientations; every
carrier's translation, clamp and round trip; whole-pixel, half-pixel and
edge-extrapolated raster shifts pixel by pixel; the pipeline end to end
(geometry translated, raster renamed and rewritten, a second open stops and
rewrites nothing, several recipes one file, a zero shift only stamps, a missing
file leaves the recipe whole); an era-0 recipe under Rotate90 oriented and
translated in one pass.

## The reverse fit on the reference pair: three rulings

The reference pair this project measures every release against (an ILCE-7RM4A
frame reverse-fitted to its Lightroom edit) still showed a pale block in one
sky tile at the final gate, with every numeric gate green. Three things were
wrong, and each is a ruling with its own measurement (`docs/ROADMAP.md`,
R39–R41):

- **R39 — a shrunk correction is judged again.** The boundary gate negotiated
  an over-budget correction down to a strength `k` instead of refusing it, but
  every route had judged its candidate at `k` = 1 only; that sky tile shipped
  at `k` = 0.134 with its own cell's coloured residual *worse* than no
  correction (0.19166 → 0.20101), bought by a whole-frame reading 0.0003
  better. Every accepted zone now keeps its full-strength control, and a
  correction shipping below it must not read worse, on the residual it was
  accepted on, than the shipping set without it, nor push the frame past the
  drift it was allowed when attached; a loser is refused with its numbers and
  the survivors pass the gate again from full strength.
- **R40 — the hard-mask boundary ruler reads discontinuity, not slope.** The
  ruler read each crossing as the difference 3 px in against 3 px out, its
  budget as the larger of the scene's own difference and three times the
  correction's own slope, and R37's allowance as the target's own difference
  at the same point: three places where a smooth sky gradient counted as an
  edge. On the pair's 0.85 re-fit the block's crossings introduced +0.009 of
  luminance (2.3 levels), the "context" of 0.005–0.007 was the sky's own
  3 px slope, and the block passed at `k` = 0.134. Each side's reading is now
  (inside − outside) minus that side's own trend, the trend taken on two
  flanking baselines 6 px further out, so a gradient reads about zero on any
  frame and a step reads its height; the budget is the scene's own
  discontinuity clamped between 1/255 and 0.012, with no credit for anyone's
  slope; luminance and each channel are read. Re-fitted: at 0.85 the tile is
  no longer attached (its correction is dropped by the zone's residual test),
  at 1.0 it attaches at `k` = 0.119 with its steps 0.0090 of luminance,
  charged p90 0.0078; on the 1000 px render the block's two edges introduce
  +0.0 / +0.0 levels at 0.65, −0.8 / +0.4 at 0.85, −1.9 / +1.6 at 1.0
  (before: +0.0 / +0.0, −2.0 / +1.6, −2.3 / +1.9).
- **R41 — the colour field attaches from the default Strength up.** The field
  (R33 §G) ran only *above* the default, so the pair's default fit carried no
  field at all — most of the visible distance between 0.65 and 0.85 (sky mean
  ΔE against the target 26.7 at 0.65, 4.6 at 0.85). The gate moved one step
  down: the default runs it at the ladder's own most conservative per-channel
  gain bound (0.35, against 0.61 at 0.85 and 0.80 at 1.0); the calibration
  point one click below still ships nothing a sidecar cannot carry. The 0.65
  re-fit attaches 87 of 88 cells (frame 0.042208 → 0.025230) and its render
  reads sky ΔE 7.2, whole-frame mean |diff| 0.0687 → 0.0296 (0.85: 4.6 /
  0.0256); the calibration recipe is byte-identical.

Two calibration-corpus tests that read the free-mask stage through the final
frame now read the stage's own realized reading, because since R41 the colour
field runs after the free masks at the default strength and its bounded
per-cell solve lands 1.5e-5 apart from two starting frames 0.003 apart — the
field's arithmetic, not a regression by the masks (0.083255 → 0.080259); and
the 16 KiB guard on the prose rationale is 24 KiB (p36 reads 17,091 bytes
after the three rulings' sentences; the consumer's cap is 512 KiB).

## Compatibility

- **Preferences.** Era 2 resets the two RAW-denoise dials once and says so;
  baked-image strength gets its own slot. Nothing else migrates.
- **Pictures.** Every develop of a Bayer RAW may differ from v1.5.1's: the
  hot-site mapping touches at most a few thousand pixels on a frame that has
  mappable sites and none on one that has not; every RAW denoise at a positive
  strength renders through the new pipeline and the new network; every RAW
  gets this version's base look the first time it is opened, a recipe saved
  by an earlier version included (its `version` stamp becomes 3 and the app
  says so; a recipe with no curve keeps none); every RAW whose recipe states
  no sharpening amount renders and exports at 40; and on an ILCE-7RM4A (or
  any body with a declared crop and no active area) the whole develop shifts
  by the declared origin, and every recipe saved before this release moves
  with it the first time it is opened (its `coord_era` becomes 2; a raster
  mask file it names is rewritten as a shifted copy beside the original).
  Recipes, sidecars, the store and the library API keep their formats;
  `coord_era` and `version` are the two fields whose values change, and a
  sidecar written for a RAW with no stated sharpening amount omits
  `crs:Sharpness` instead of writing 0.
- **Sharpening.** A recipe or sidecar that states an amount, 0 included,
  renders as before; a JPEG or TIFF renders as before; a RAW that states none
  renders at 40 (Lightroom's default) instead of 0.
- **Downloads.** One new file, `autoshade-raw-denoise-v2.pth` (the v1.6.0
  release asset, and the copy above). `autoshade-raw-denoise-v1.pth` is no
  longer fetched; a cache directory that holds it keeps it, unused.
- **The reference pair.** The reverse-fit reference this project measures every
  release against was fitted on an ILCE-7RM4A frame under the old origin and
  the old base look, so its saved recipe no longer sits on the picture; the
  gate below re-ran the fit on this release instead of re-rendering the old
  recipe.

## Gates

Measured before the tag on the release code (`fb1f1a9`; the version bump touches
Cargo.toml, Cargo.lock, the documents, the sidecar's download pin and the
release name in source comments and two source-text tests, re-run after it). The
three-lane release battery
(`scripts/release_battery.sh`, a frozen snapshot worktree, the p36–p41
calibration corpus and the sidecar weights in reach): library **1668 passed /
0 failed / 15 ignored** (1006.46 s, release profile, one process per module),
CLI **25 / 0**, contract 2 + 2, doc-tests 0, GUI **214 / 0 / 1**, calibration
lane **1668 / 0 / 15** (1308.41 s; one skip line, the mask-brush specimen test whose
`AUTOSHADE_MB_SAMPLE_ROOT` specimen is not on this machine, named in every
release since v1.3.2), `audit_i18n` and the font check exit 0 inside the
battery; the Python suites 73 OK from `python/` and 44 OK from `scripts/` (CPU,
the real weights, `-W error::RuntimeWarning`). By name against the v1.5.1 tag,
listed by the harness on both trees: library 1649 → 1683 (+39 / −5: the
sixteen hot-site, grain-return and cleaner-contract tests of the merged
denoise lanes, the five of the base look's pairing, the era-3 re-estimate, the
two sharpening defaults, the seven of coordinate era 2, R39's two, R40's two
and R41's one; the five removed names were renamed or replaced under R40, R41,
calibration era 3 and coordinate era 2, each beside its replacement in
ARCHITECTURE); GUI 214 → 215 (+1 / −0, the preference-era reset). clippy 0
warnings on both feature sets (`--all-targets -- -D warnings`).
`check_docs.py --gates` on the transcript with the XMP census root supplied:
**32 PASS / 0 FAIL / 0 SKIP** (after the bump; before it the same run fails
exactly the count claims, which is what it is for). Photo-name, token-shape
and user-path greps on the release diff: 0 / 0 / 0.

Mutations recorded in the ledger for this release, each run by hand with the
source restored byte for byte afterwards: the fourteen of the hot-pixel rule
(each turned its module's tests red) and the eight of the base look's pairing
(each turned its own test red).

Final gate, reference pair, before the tag: the pre-bump release CLI re-fitted
the reference pair at 0.65 / 0.85 / 1.0 (`match --zoned`) and rendered each at
the target's size. At 0.85: sky ΔE 4.6 and whole-frame mean |diff| 0.0260
against the target (the R41 lane read 4.6 / 0.0256); at 0.65: 7.0 / 0.0296
(7.2 / 0.0296); at 1.0: 4.7 / 0.0263. This release moves the solver (R39–R41)
and the front end (calibration era 3, the sharpening default), so a pixel
comparison against the previous release's render is not the measure; the three
side-by-sides went to the user, whose eyes are the gate.

Not measured: no paid image call was made for this release; the GUI executable
was not launched; the sharpening amount's scale against Lightroom's (above);
the star standard's lines 6 and 7c stay red and are recorded above. The ship
facts (the release run, the downloaded assets, the site, the local upgrade)
are in the ROADMAP ledger entry.
