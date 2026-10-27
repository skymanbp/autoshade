# AutoShade v1.16.0 — the reverse fit pairs the photo in the frame the target was exported in

Everything merged into `main` since v1.15.1 ships here. One cause runs
through this version: the reverse fit compared its own rendering of the
photo with the target pixel for pixel, but the two were not in the same
frame. A target exported from Lightroom with the lens profile on has its
distortion corrected; the fit's internal rendering did not, so every pair
near a cloud edge, a roof line or a frame corner was a few pixels off, and
the fit read that offset as content that disagreed with the move it was
about to make. This version measures the target's geometry, pairs in one
frame, and leaves the rest of the fit — its gates, its vocabulary, its
masks — exactly where it was.

The design, the code and this document are the maintainer's; no external
coding model wrote this release.

## The reverse fit: one frame for the photo and the target

- **The target's geometry is measured before anything is paired.** The
  fit now aligns the target against two renderings of the photo — as shot,
  and with the camera's own lens profile applied — on six patches, and
  reads which one it matches. A target that matches the corrected rendering
  by a clear margin was exported with the lens profile on; one that matches
  the as-shot rendering was exported without it; when neither wins clearly
  the fit says so and behaves exactly as before.
- **A lens-corrected target is brought into the photo's own frame.** Rather
  than warping every candidate rendering during the solve, the target is
  resampled once, through the same lens map the engine uses to correct the
  photo, into the sensor frame where the fit's masks and recipes already
  live. The narrow ring of sensor pixels that lies outside the corrected
  field — about 3 % of the frame on the calibration photos — has no target
  to be compared with, and it carries no evidence anywhere: not in the
  frame-wide statistics, not in any zone's solve or voucher, not in the seam
  and structure readings. The masks the fit writes, the recipe, and the
  final rendering are unchanged in form; only the comparison moved.
- **An uncorrected target keeps the geometry it was exported with.** When
  the target matches the as-shot rendering, the fit pairs as before and
  switches the profile's distortion and chromatic-aberration corrections
  off in the recipe it writes, so the rendering lands on the target's
  geometry rather than on a corrected frame the target never had.
- **What it changed on the calibration pairs.** Five of the six reference
  photos were exported with the lens profile on. On those the fit's own
  look error after the fit fell to between a quarter and a half of what it
  was (0.023 → 0.009, 0.025 → 0.006, 0.034 → 0.011, 0.035 → 0.013), and the
  sky of the seaside street — the photo that started this — moved from a
  pale, warm-clouded sky toward the target's deeper blue with cooler clouds.
  The pair exported without the profile and the two generated references
  read as undecided and render the same pixels as v1.15.1.

## The command line renders a sidecar the way the desktop app does

- **`apply` with a Lightroom sidecar stamps the lens profile.** The desktop
  app, when it opens a photo beside a sidecar, reads the sidecar's lens
  switch and stamps the camera's profile accordingly; the command line's
  `apply` did not, so the same sidecar rendered a corrected frame in the
  app and an uncorrected one from the command line. Both now go through the
  one function that reads the switch. A JSON recipe that already carries a
  profile renders exactly as before.

## Compatibility

No recipe field changes; sidecars written by earlier versions open as
before. A fit of a lens-corrected target now writes a different recipe than
v1.15.1 did for the same pair, because its evidence is aligned; a fit of an
uncorrected target writes the profile's distortion and CA switches off.

## Gates

待填_GATES
