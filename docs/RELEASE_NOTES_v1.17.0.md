# AutoShade v1.17.0 — one develop law

Everything merged into `main` since v1.16.0 ships here. One idea runs
through this version: every photograph develops on one law, AutoShade's own,
whatever file it came from or whatever profile its sidecar names. A camera
profile is colour data read from the user's own Camera Raw install, like a
lens profile; it never changes how a control behaves. Around that rule this
release adds a mask Color tint, fixes the reverse fit at the causes found on
the reference pairs, and keeps dark sensor values signed.

The design, the rulings and this document are the maintainer's. The code of
this release was written with an AI coding assistant working under the
maintainer's direction, and every change passed the project's own test
battery and reference-pair gate before it shipped.

## The develop

- **Dark sensor values stay signed.** Read noise below the black level is
  kept through the demosaic and the white balance instead of being cut at
  zero first, so deep shadows and night skies no longer lift and tint.
- **A camera profile is colour data.** A named camera profile renders its
  colour matrix and colour tables and nothing else; its own tone curve and
  exposure offset are read and not rendered. A profile with a forward matrix
  owns the camera-to-working transform, built by the DNG specification's
  colour model.
- **Colour tables stack in their real order.** A creative Look's colour
  table sits on top of the camera profile's own table instead of replacing
  it. The Look's baked tone curve and sliders do not render; its
  black-and-white switch does.
- **Colour noise reduction keeps each pixel's brightness.** A rebuilt pixel
  is brought back into range along its colour difference, so a pure black
  stays black.

## The lens

- **Each chromatic-aberration channel moves by its own knots.** A red or
  blue fringe correction no longer zooms all three channels by one shared
  scale.
- **The camera's lens metadata is read by one measured convention.** The
  distortion knots are evaluated at the zoomed output radius and the CA knots
  by the same convention. The lens vignetting is the installed lens profile's
  own model, its focal, aperture and distance nodes blended in EV, and the
  camera's own knots only for a lens no installed profile covers. The two
  profile strengths run from 0 to 200 % and go to the sidecar with the
  profile they scale.
- **A linear mask's handles follow the photograph.** With lens correction on
  they move with the lens map, with it off they stay where they were drawn;
  a radial follows the photograph at its stored size.

## Masks

- **A mask's Color tint.** Masks gain **Color tint** and **Color tint hue**:
  every colour inside the mask, neutral and coloured alike, shifts toward
  one hue — most in the midtones, never brighter or darker. It is read from
  and written to the sidecar, set in the mask's Color group, and the reverse
  fit can solve it from a sky and its clouds at once.

## The reverse fit

- **The white balance takes only the colour change every brightness range
  shares.** A target graded warm in the shadows and cool in the highlights no
  longer drives the white balance to its limit. The brightness-dependent rest
  goes to the Color Grading wheels (shadows, midtones, highlights), solved
  from each range's own matched pixels and shipped only as far as the frame
  vouches for them. On the seaside street pair the sky is light blue again
  and the white balance stays as shot.
- **The colour mixer solves each band's hue.** A hue rotation the target
  applies to one colour band is read from that band's own paired pixels, by
  brightness range, and ships only where the band's ranges agree and the
  target's cells approve it; a band whose bright and dark pixels ask opposite
  rotations is left alone.
- **The cast-to-white-balance hand-off stops where it should.** The walk that
  hands a colour cast back to the white balance ends when it leaves the
  strength budget or stops moving.
- **A zone the colour field regresses is pinned** and the field solved
  again.
- **The colour field is judged on the field that ships.** Whether there is
  enough left for it is now read on the frame it actually renders, not on an
  earlier estimate it routinely beats.
- **After 「AI generate」, the app asks** whether to reverse-fit the new card,
  and from which negative.

## The Lightroom sidecar

- **A zone's colour gains travel as that mask's Temp and Tint.** The colour
  a zone was given is written into the sidecar as the mask's white-balance
  shift, with the exposure compensated, so the sidecar shows the zone's
  colour instead of dropping it.
- **The smooth colour field is named, not dropped silently.** The sidecar
  has no place for the frame-wide colour field; the export says so on the
  command line, in the desktop app's delivery note and in the sidecar's own
  description. Rendered exports from AutoShade carry it in full.
- **Channel curves always travel with a composite curve**, which Lightroom
  needs before it reads them.

## Fixed

- The four-region calibration test names a run whose segmentation could not
  start instead of reporting a bitwise mismatch.

## Known limits

- **The night reference pair's horizon glow.** On the night pair the horizon
  band renders even by eye. The frame-wide colour field cannot follow a
  demand that changes sign within a few rows, so a faint residual stays in
  the band's numbers.
- **Strongly saturated sky colour in the sidecar.** A zone's colour carried
  as Temp and Tint is exact on greys; on a deeply saturated blue band of sky
  the sidecar's version is less saturated than AutoShade's own export.
- **Colour noise reduction** leaves a faint colour grain that comes from the
  demosaic itself.
- **One reference pair's clouds.** On the sunset rooftops pair the upper
  clouds come out a little less pink than the target. The frame-wide colour
  field solves pixel by pixel and trades the clouds' mean colour for a
  smaller per-pixel error; a mean-colour term in that solve is the next step.

## Known changes

Every recipe, imported or new, develops on the same AutoShade law; a camera
profile is colour data only.

- **Creative profiles render their colour, not their tone.** A creative
  profile's colour table and its black-and-white switch still render; its
  baked tone curve and baked sliders no longer do.
- **Imported sidecars start where a new develop starts**, on the photo's
  camera base look.
- **A master that says it carries lens corrections is refused.** A heal,
  clone, fill, denoise or stack file whose own marker says its pixels are
  already lens-corrected is not opened; the message asks for the master to
  be made again from the RAW.
- **Temperature and Tint ranges stay wide**: Tint ±150, Temperature up to
  50000 K.
- **Style library matches may shift.** When the library pairs a RAW's own
  sidecar with its finished photo, it develops that sidecar on the one law,
  so a sidecar that matched before may no longer match, or the other way
  round.

## Compatibility

Recipes and sidecars written by earlier versions open. New fields: a mask's
Color tint and Color tint hue (`crs:LocalToningHue`,
`crs:LocalToningSaturation`).

## Gates

The release battery (`scripts/release_battery.sh`: the library, the desktop
app and command line, and the calibration run, on a frozen copy of the release
code at `7918fa4`, with the calibration photos and the denoiser weights in
reach): library **1814 passed / 0 failed / 15 ignored** (1335.32 s), CLI
**26 / 0**, contract 2 + 2, doc-tests 0, GUI **293 / 0 / 4**, calibration run
**1814 / 0 / 15** (1513.50 s; the tests gated on the calibration corpus ran on it here, where the default lane passes them without it);
`audit_i18n` 0 findings, the font check (923 of 924 code points embedded, one
left to egui's bundle) and `check_doc_drift` (0 findings);
clippy 0 warnings on both feature sets; the Python suite under `scripts/` 44
tests, all passing (the suite under `python/` was not run: `python/` is unchanged
since v1.16.0); `check_docs.py
--gates` 28 / 4 PASS / FAIL on the snapshot — the test-count claims, still
at the earlier snapshot's figures — and **32 / 0 / 0 PASS / FAIL / SKIP** once the counts were
moved. By name against the v1.16.0 release battery: library 1759 → 1829 (+92 / −22); GUI
292 → 297 (+5 / −0); CLI 26 → 26. The reverse fit changed, so the reference
pairs were fitted again by the installed v1.16.0 command line and by this build:
on the seaside street pair (p39) the mean colour distance to the target at 1000 px fell from 7.89 to 3.59 over the frame and from 9.20 to 4.09 over the sky (top-centre blue-minus-red 42.1 → 54.1 against the target's 56.3): the sky reads light blue under white clouds, as the target does; the lantern street (p38) 5.86 → 4.02, the sunset street (p40) 8.72 → 6.16, the reference pair 8.01 → 7.91 at 0.85 (8.09 → 8.15 at 0.65, 8.00 → 7.90 at 1.0); the dusk farmhouse (p41) 5.53 → 5.91 at 1000 px while its 480 px distance fell 4.27 → 4.19 and its upper sky reads closer by eye; the sunset rooftops (p36) 3.57 → 3.90, its upper clouds a touch less pink (the known limit above); the pair whose target is another photograph (p37) 62.4, unchanged. The night pair (p42) fitted from its denoised master: 5.14 with a horizon ripple of 0.73, against v1.16.0's 5.31 / 0.88. Every recipe differs from v1.16.0's throughout, as one develop law must. The three-way sheets (target | v1.16.0 | this build) were viewed for every pair, the night pair from both starts.
