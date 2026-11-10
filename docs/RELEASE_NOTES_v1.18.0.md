# AutoShade v1.18.0 — one objective, one law

Everything merged into `main` since v1.17.1 ships here. This release replaces
the reverse fit's chain of per-stage decisions with one objective and one
rule. Until now each stage of the fit (tone, white balance, colour mixer,
grade wheels, cast curves) estimated its own correction and defended it with
its own gates — a look-error ratio here, a hue census there, a do-no-harm
check at the end — and the only thing that minimised the distance to the
target was the order they happened to run in. Now the stages only propose a
starting point; a single solver moves every control together to make the
finished photo as close to its target as the controls allow, charging each
control it keeps the price of the fit's own rounding, and one rule decides
what a control may not do: leave any part of the picture further from what
that part's own evidence asks than it found it.

The design, the rulings and this document are the maintainer's. The code of
this release was written with an AI coding assistant working under the
maintainer's direction, and every change passed the project's own test
battery and reference-pair gate before it shipped.

## The reverse fit: one objective

- **One number is minimised, and every control pays for its place.** The fit
  minimises the perceptual colour distance between the finished photo and
  its target plus a fixed price per control that is off its neutral value.
  That price is the smallest difference the fit can produce by rounding its
  own controls — measured on the corpus, not chosen — so a control that does
  not buy its own rounding is dropped. The report names the controls set
  back to zero.
- **The stages propose, the solver decides.** The tone sliders, the master
  curve, the white balance, the colour mixer, the grade wheels and the cast
  curves are estimated as before and enter the solve as a starting point.
  From there the solver moves all of them together down the objective's
  slope, reading each control's effect at the resolution the objective can
  resolve, then polishes one control at a time, then holds every control
  against the photo without it and keeps only what earns its price.
- **The distance is read where it means something.** Where the photo and its
  target show the same scene, the fit compares them pixel by pixel as a
  perceptual colour difference on lightly smoothed neighbourhoods. Where
  they do not correspond — a sky the target repainted, a texture a generator
  invented — it compares the distributions of lightness and colour instead,
  in the same units. How much the pixel pairing is trusted is measured on
  the picture itself (how well the source's tones predict the target's over
  the paired pixels), and that trust weights the two readings. The old
  measure weighed a night sky's brightness far above its colour, which is
  why a correction that brought the night horizon's colour closer was
  refused in v1.17.1.
- **The reported confidence is re-derived.** The confidence a fit reports is
  set so that the one calibration pair whose target is a different photograph
  — which no recipe can match — sits on the confidence floor.

## The reverse fit: one law

- **A control may not move any part of the picture against that part's own
  evidence.** The source is partitioned into cells by brightness and hue,
  merged along the axis a control cannot tell apart (the tone sliders see
  brightness only, the white balance hue only, the colour mixer and the grade
  wheels both). For each cell the target's evidence asks for a move — read
  from the paired pixels where the pairing is trusted, from the matched
  distributions where it is not — and a control class is admitted only if
  its move in every cell stays within that cell's ask, to the cell's own
  measurement error. Each class is judged on its own axes (brightness, the
  colour vector, the white-balance axes) against the photo with every other
  class in place and itself at its neutral value, so a class answers only for
  what it adds.
- **This one rule replaces the gates it used to take several to state.** The
  four cast-curve vetoes, the unsupported-range vetoes for tone and hue, the
  white balance's foreign-hue, rotation and shared-component checks, the
  stage and terminal do-no-harm checks and the cast-to-white-balance hand-off
  walk are gone; each had been one consequence of the rule, read on one
  class with its own numbers.
- **A proposal the law refuses is repaired, not thrown away.** A class the
  rule refuses is re-estimated from the cells' own asks under the rule, and
  where no single control can be moved on its own it is scaled toward
  neutral as a whole until the rule admits it. The report says which class
  was re-estimated or scaled and by how much.
- **The curve's freedom follows the evidence.** Where the pixels pair, every
  knot of a tone curve is a free coordinate of the solve, so the solver can
  shape a curve the stages did not propose. Where only the distributions can
  be read, the curve keeps the shape the stage proposed and the solver sets
  its strength alone — a distribution cannot say which pixel goes where, and
  letting the knots move freely on such evidence produced a cliff in the
  master curve on the night pair. On such a pair the proposed curve also
  keeps the brightness ranges the fit cannot trust at their identity.
- **A local correction is judged on its own pixels.** A zone, a band, a tile
  or a mask ships when it brings its own region closer by the fit's rounding;
  the rest of the frame is only a check that it got no worse by more than the
  same margin. A region's structure is read from that region alone, and
  structure is asked only where the question is structure: a local
  correction is judged on colour and tone, and its edges by the seam checks
  that exist for them.

## The reverse fit: what else changed

- **A white balance solved on the shared part of a repainted pair is judged
  there.** When the target replaced part of the scene, the fit solves the
  white balance and the exposure from the part both pictures share; it now
  also judges them there, so the replaced part cannot veto a correction it
  was never asked to answer for.
- **The finished recipe is read again as the export renders it.** The fit
  searches on a reduced preview; the export renders at full size and then
  resizes, and the two do not agree exactly on a strongly corrected frame.
  Before the recipe ships it is rendered once more the export's way, beside
  the calibration-only starting point; it ships only if it is closer there
  too, and the report gives both readings. This adds one full-size render to
  a fit.
- **The fit starts from the picture the export would make.** The starting
  picture is developed exactly as the export develops it, resized only after
  the base's per-pixel steps; an untouched recipe now reads as identical to
  its own export. A JPEG or PNG is read on its own pixels when it is the
  photo being fitted.
- **A band too small to be a zone joins its neighbour**, and a seam is charged
  to the side that is off: a region that lands on its target is no longer
  refused because the region beside it is still short of its own.
- **The colour field's stop rule compares like with like**, its solve reaches
  its tolerance (the iterative solver is scaled by the system's own
  diagonal), a cell with too few pixels to measure carries the smooth
  continuation of its neighbours instead of "no change", and a pixel that is
  part sky and part land owes each part's light through that part's own
  correction.
- **A hidden contrast rule is removed.** The tone solve forced contrast to a
  fixed small value whenever the frame was not yet close and the solved
  contrast was near zero. The rule had no recorded reason and no test; the
  solved value now stands.
- **The report says what ships.** Every sentence about a control is derived
  from the finished recipe, so a note never describes a curve or a tint that
  the solve took back, and a refusal prints the cells and the readings the
  decision was made on.

## Colour noise reduction

- **Colour noise is taken out of the RAW in the camera's own linear light**,
  on the mosaic, before the demosaic and before every nonlinear step of the
  develop. A render's dark colour is biased by its own chroma noise: the black
  clip, the transfer encode and saturation each read a noisy dark pixel's
  colour, and the mean of a nonlinear function of a noisy value is not the
  function of its mean (on the night pair the dark tenths sat 2.4–6.0 colour
  steps off their binned version, mostly too blue). Before those steps the
  noise can be taken away before anything rectifies it.
- **The threshold is the photograph's own noise, measured, not a dial.** The
  two green sites of one Bayer cell see the same scene one pixel apart, so
  their difference measures the green noise at every brightness with no
  scene in it; a photon-plus-read-noise model with the camera's radial gain
  is fitted to those readings, and only the patches the model says are noise
  alone enter the fit. A colour edge is kept where it rises above the
  window's own noise; a structure narrower than the smallest window keeps its
  own sample; a colour site whose four greens do not lie on one plane is not
  measured and keeps the develop's own colour.
- **On a processed master the stage reads the master's own noise**, measured
  on the master, and abstains above the levels it could measure: a level
  with no reading is not a measurement — no weight, no vote, no read — so a
  star core on a master keeps the master's own colour.

## The Lightroom sidecar

- **The sidecar states the lens-correction switch the develop rendered
  under.** Lightroom places every mask in the frame its own lens switch
  names. Written over a Lightroom sidecar that had the switch off, the
  sidecar kept that "off" beside a develop that rendered the lens
  correction, so Lightroom drew the photo and its masks without it. The save
  now writes the switch: on while the develop renders the lens profile's
  distortion or vignetting correction, off when it carries the profile with
  both off, and nothing when it carries no profile. A switch changed in
  Lightroom afterwards is still obeyed, and AutoShade's own sidecar still
  reads back as the develop it carries.
- **What the sidecar declares it does not carry** (`asr:NotCarried`): with an
  Adobe lens profile for the photo, Lightroom corrects the vignetting with
  that profile rather than with the camera data this app uses, so the line
  names the vignetting as this engine's; with no such profile it names the
  whole lens correction, as before. The camera base curve stays declared as
  not carried: no tone curve and exposure pair in the sidecar's own terms
  reproduces this app's starting tone exactly, so it is named rather than
  approximated.

## How this release was checked

- **Every control the fit can solve has a self-consistency test.** For each
  one — temperature, tint, each colour band's hue, saturation and luminance,
  the grade wheels, the cast curves, a mask's colour — a calibration photo is
  rendered with that control alone set to a known value, the render becomes
  the target, and the fit is asked to find it again from the straight
  conversion. 待填_SELF_CONSISTENCY
- **Nine real pairs against v1.17.1**, each fitted by the released v1.17.1
  command line and by this build at the strength of record and compared
  against its target region by region: every pair reads at or under the
  v1.17.1 distance with the same or fewer controls. The night pair's whole
  frame reads 4.66 against 5.14 and its master curve carries no cliff; the
  pair with the most controls in v1.17.1 (35) now ships 11 and reads 0.11
  further, less than the fit's own rounding amounts to on a frame.
- **Every mechanism in the law has a toy that reads it**, each pinned to the
  reading on this tree with the attribution written beside the number: the
  withheld-sky toy keeps its untrusted range at identity, the calibration
  sky ships the colour its cells vouch, the refused canyon casts land as the
  law admits them.

## Known limits

- **The night horizon band.** On the night pair the brightness ripple across
  the sky's foot reads 1.11 against v1.17.1's 0.73 and the line of 0.63 the
  pair is held to; the whole frame and the sky itself are closer than
  v1.17.1's. The ripple is the population evidence's: where the sky's pixels
  do not pair, the fit reads the sky's distribution, and the distribution
  does not say where in the sky each tone belongs.
- **A withheld brightness range can hold a global tone back.** Where a region
  of the picture is read by its distribution and its lower and upper halves
  ask opposite moves, no exposure or slider serves the rest of the frame
  without moving that region against half of its ask; the fit then ships the
  tone the law admits, which on the constructed case is none. The stage's
  proposed curve keeps such a range at identity, but a range near white that
  the proposed exposure pushes into clipping cannot be held there inside the
  curve's slope budget.
- **The white balance and exposure judged on the shared part of a repainted
  pair** apply on no calibration pair: on every pair that reaches that path
  the shared part is too small to be used, and the whole frame is read as
  before. The rule is tested on a constructed pair only.
- **The sidecar's starting tone is declared, not carried** (above): opened in
  Lightroom, a photo whose recipe starts on the camera base curve renders on
  Lightroom's own starting tone.
- **The brightest star on a processed night master** reads cyan-cored with a
  yellow tail with the colour-noise stage off and in v1.17.1 alike: it is not
  this stage's and is reported as an observation.

## Known changes

- **Recipes differ from v1.17.1's** on most pairs: the fit judges and admits
  every control by a different measure, so a fit ships fewer controls (those
  that earned nothing are at zero) and different strengths.
- **The report carries new lines**: the controls set back to zero, the class
  that was re-estimated or scaled and by how much, the readings a zone
  shipped on, and the full-size re-reading of the finished recipe beside the
  preview's and the starting point's.
- **A fit takes longer.** The solver renders the candidate many hundreds of
  times; a real pair takes from a quarter of an hour to three quarters on
  the maintainer's machine where v1.17.1 took minutes. Processing time is a
  reading, not a gate, in this project.
- **A fit takes one more full-size render** for the final check.

## Compatibility

Recipes and sidecars written by earlier versions open. No new recipe field.
A sidecar AutoShade writes now always states crs:LensProfileEnable when its
recipe carries a lens profile.

## Gates

待填_GATES
